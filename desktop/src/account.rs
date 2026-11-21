use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ROOT: &str = "https://auth.tngl.pro:8443";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Login<'a> {
    pub id: &'a str,
    pub password: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_code: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    #[serde(default)]
    email_required: bool,
    retry_after_seconds: Option<u64>,
    token: Option<String>,
    expires_at: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    token: String,
    expires_at: u64,
}

impl Session {
    pub fn expired(&self) -> bool {
        self.token.is_empty()
            || SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .is_ok_and(|now| now.as_secs() >= self.expires_at)
    }
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(25)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build(),
    )
}

pub enum Authentication {
    EmailRequired(u64),
    SignedIn(Session),
}

pub fn authenticate(agent: &ureq::Agent, request: Login<'_>) -> Result<Authentication, String> {
    let route = if request.email.is_some() {
        "register"
    } else {
        "login"
    };
    parse_login(&post(agent, route, &request)?)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recovery<'a> {
    pub id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStep {
    Activation,
    Code,
    Support,
    Done,
}

#[derive(Serialize)]
pub struct RecoveryResult {
    pub step: RecoveryStep,
    pub retry_after_seconds: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryResponse {
    step: Option<RecoveryStep>,
    #[serde(default)]
    email_required: bool,
    #[serde(default)]
    ok: bool,
    retry_after_seconds: Option<u64>,
}

pub fn recover(agent: &ureq::Agent, request: Recovery<'_>) -> Result<RecoveryResult, String> {
    let route = if request.invite_code.is_some() {
        "recover-first"
    } else {
        "recover"
    };
    let confirming = request.email_code.is_some();
    parse_recovery(&post(agent, route, &request)?, confirming)
}

fn parse_recovery(text: &str, confirming: bool) -> Result<RecoveryResult, String> {
    let response: RecoveryResponse =
        serde_json::from_str(text).map_err(|_| "Некорректный ответ на восстановление пароля")?;
    let step = match (
        confirming,
        response.step,
        response.email_required,
        response.ok,
    ) {
        (true, _, _, true) => RecoveryStep::Done,
        (false, Some(RecoveryStep::Code), true, _) => RecoveryStep::Code,
        (false, Some(step @ (RecoveryStep::Activation | RecoveryStep::Support)), _, _) => step,
        _ => return Err("Сервис не подтвердил восстановление пароля".into()),
    };
    Ok(RecoveryResult {
        step,
        retry_after_seconds: if step == RecoveryStep::Code {
            response.retry_after_seconds.unwrap_or(60)
        } else {
            0
        },
    })
}

fn post(agent: &ureq::Agent, route: &str, payload: &impl Serialize) -> Result<String, String> {
    let payload = serde_json::to_vec(payload).map_err(|_| "Не удалось подготовить запрос")?;
    let response = agent
        .post(format!("{ROOT}/v1/mobile-auth/{route}"))
        .header("Content-Type", "application/json")
        .send(payload.as_slice())
        .map_err(|_| "Сервис аккаунта недоступен")?;
    let status = response.status().as_u16();
    let text = response
        .into_body()
        .into_with_config()
        .limit(32 * 1024)
        .read_to_string()
        .map_err(|_| "Не удалось прочитать ответ сервиса аккаунта")?;
    if (200..300).contains(&status) {
        return Ok(text);
    }
    let error = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
    Err(
        match error.get("error").and_then(serde_json::Value::as_str) {
            Some("invalid_email" | "email_required") => "Проверь адрес электронной почты",
            Some("invalid_email_code") => "Код из письма неверный или истёк",
            Some(
                "account_exists" | "id_taken" | "registration_conflict" | "already_registered",
            ) => "Этот логин уже занят",
            Some("email_already_used" | "email_in_use") => "Эта почта уже используется",
            Some("invalid_id" | "registration_invalid_id") => "Проверь логин",
            Some("password_policy") => "Пароль должен содержать от 12 до 128 символов",
            Some("recovery_unavailable") => {
                "Восстановление для этого логина недоступно. Поддержка: tangledlink@gmail.com"
            }
            Some("invalid_credentials" | "invalid_activation" | "recovery_changed")
                if route.starts_with("recover") =>
            {
                "Проверь логин и код активации"
            }
            _ if status == 429 => "Слишком много попыток. Попробуй позже.",
            _ if status == 409 => "Аккаунт уже существует или эти данные заняты",
            _ if status < 500 && route == "login" => "Проверь логин, пароль и код из письма",
            _ if status < 500 => "Сервис отклонил запрос. Проверь введённые данные.",
            _ => "Сервис аккаунта недоступен",
        }
        .into(),
    )
}

fn parse_login(text: &str) -> Result<Authentication, String> {
    let response: LoginResponse =
        serde_json::from_str(text).map_err(|_| "Некорректный ответ на вход")?;
    if response.email_required {
        return Ok(Authentication::EmailRequired(
            response.retry_after_seconds.unwrap_or(60),
        ));
    }
    let session = Session {
        token: response
            .token
            .filter(|token| !token.is_empty())
            .ok_or("Сервис входа не вернул сессию")?,
        expires_at: response
            .expires_at
            .ok_or("Сервис входа не вернул срок сессии")?,
    };
    if session.expired() {
        return Err("Срок сессии истёк".into());
    }
    Ok(Authentication::SignedIn(session))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub expires_at: Option<u64>,
}

impl Usage {
    pub fn from_header(header: &str) -> Option<Self> {
        let (mut upload, mut download, mut total, mut expiry) = (None, None, None, None);
        for field in header.split(';') {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            let value = value.trim().parse().ok();
            match key.trim() {
                "upload" => upload = value,
                "download" => download = value,
                "total" => total = value,
                "expire" => expiry = value,
                _ => {}
            }
        }
        let used = upload
            .zip(download)
            .and_then(|(upload, download): (u64, u64)| upload.checked_add(download));
        if used.is_none() && total.is_none() && expiry.is_none() {
            return None;
        }
        Some(Self {
            used,
            limit: total,
            expires_at: expiry.filter(|value| *value > 0),
        })
    }
}

pub enum Subscription {
    Available { body: String, usage: Option<Usage> },
    Inactive,
    Expired,
}

pub fn subscription(agent: &ureq::Agent, session: &Session) -> Result<Subscription, String> {
    let response = agent
        .get(format!("{ROOT}/v1/subscription"))
        .header("Authorization", format!("Bearer {}", session.token))
        .call()
        .map_err(|_| "Не удалось загрузить подписку аккаунта")?;
    match response.status().as_u16() {
        401 => Ok(Subscription::Expired),
        403 => Ok(Subscription::Inactive),
        200 => {
            let usage = response
                .headers()
                .get("subscription-userinfo")
                .and_then(|value| value.to_str().ok())
                .and_then(Usage::from_header);
            let body = response
                .into_body()
                .into_with_config()
                .limit(512 * 1024)
                .read_to_string()
                .map_err(|_| "Не удалось прочитать подписку аккаунта")?;
            Ok(Subscription::Available { body, usage })
        }
        _ => Err("Подписка аккаунта недоступна".into()),
    }
}

pub fn change_password(
    agent: &ureq::Agent,
    session: &Session,
    current: &str,
    new_password: &str,
) -> Result<(), String> {
    let body =
        serde_json::json!({"currentPassword":current,"newPassword":new_password}).to_string();
    let response = agent
        .post(format!("{ROOT}/v1/password"))
        .header("Authorization", format!("Bearer {}", session.token))
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|_| "Сервис аккаунта недоступен")?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        400..=403 => Err("Не удалось изменить пароль. Проверь текущий пароль и сессию.".into()),
        429 => Err("Слишком много попыток. Попробуй позже.".into()),
        _ => Err("Сервис аккаунта недоступен".into()),
    }
}

pub fn logout(agent: &ureq::Agent, session: &Session) -> bool {
    agent
        .post(format!("{ROOT}/v1/logout"))
        .header("Authorization", format!("Bearer {}", session.token))
        .header("Content-Type", "application/json")
        .send("{}")
        .is_ok_and(|response| response.status().is_success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_traffic_and_expiry_fields() {
        assert_eq!(
            Usage::from_header("upload=3; download=5; total=20; expire=4000000000"),
            Some(Usage {
                used: Some(8),
                limit: Some(20),
                expires_at: Some(4000000000)
            })
        );
        assert_eq!(
            Usage::from_header("upload=unknown; download=5; total=0; expire=0"),
            Some(Usage {
                used: None,
                limit: Some(0),
                expires_at: None
            })
        );
        assert!(Usage::from_header("unknown=0").is_none());
        assert_eq!(
            Usage::from_header("upload=18446744073709551615; download=1; total=1")
                .unwrap()
                .used,
            None
        );
    }

    #[test]
    fn recovery_requires_confirmed_challenge_and_completion() {
        assert_eq!(
            parse_recovery(r#"{"step":"activation"}"#, false)
                .unwrap()
                .step,
            RecoveryStep::Activation
        );
        assert_eq!(
            parse_recovery(r#"{"step":"support"}"#, false).unwrap().step,
            RecoveryStep::Support
        );
        let challenge = parse_recovery(
            r#"{"step":"code","emailRequired":true,"retryAfterSeconds":30}"#,
            false,
        )
        .unwrap();
        assert_eq!(
            (challenge.step, challenge.retry_after_seconds),
            (RecoveryStep::Code, 30)
        );
        assert!(parse_recovery(r#"{"step":"code"}"#, false).is_err());
        assert!(parse_recovery(r#"{"step":"code","emailRequired":true}"#, true).is_err());
        assert_eq!(
            parse_recovery(r#"{"ok":true}"#, true).unwrap().step,
            RecoveryStep::Done
        );
    }

    #[test]
    fn parses_email_challenge() {
        assert!(matches!(
            parse_login(r#"{"emailRequired":true,"retryAfterSeconds":45}"#).unwrap(),
            Authentication::EmailRequired(45)
        ));
    }

    #[test]
    fn validates_session_shape_and_expiration() {
        assert!(matches!(
            parse_login(r#"{"token":"test","expiresAt":4102444800}"#).unwrap(),
            Authentication::SignedIn(_)
        ));
        assert!(parse_login(r#"{"token":"test","expiresAt":1}"#).is_err());
        assert!(parse_login(r#"{"token":"","expiresAt":4102444800}"#).is_err());
    }
}
