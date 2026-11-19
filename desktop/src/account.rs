use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ROOT: &str = "https://auth.tngl.pro:8443";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Login<'a> {
    pub id: &'a str,
    pub password: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_code: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    #[serde(default)]
    email_required: bool,
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

pub fn login(agent: &ureq::Agent, login: Login<'_>) -> Result<Option<Session>, String> {
    let payload = serde_json::to_vec(&login).map_err(|_| "Не удалось подготовить вход")?;
    let response = agent
        .post(format!("{ROOT}/v1/mobile-auth/login"))
        .header("Content-Type", "application/json")
        .send(payload.as_slice())
        .map_err(|_| "Сервис входа недоступен")?;
    match response.status().as_u16() {
        400 | 401 | 403 => return Err("Проверь логин, пароль и код из письма".into()),
        429 => return Err("Слишком много попыток. Попробуй позже.".into()),
        code if !(200..300).contains(&code) => return Err("Сервис входа недоступен".into()),
        _ => {}
    }
    let text = response
        .into_body()
        .into_with_config()
        .limit(32 * 1024)
        .read_to_string()
        .map_err(|_| "Не удалось прочитать ответ на вход")?;
    parse_login(&text)
}

fn parse_login(text: &str) -> Result<Option<Session>, String> {
    let response: LoginResponse =
        serde_json::from_str(text).map_err(|_| "Некорректный ответ на вход")?;
    if response.email_required {
        return Ok(None);
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
    Ok(Some(session))
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
    fn parses_email_challenge() {
        assert!(parse_login(r#"{"emailRequired":true}"#).unwrap().is_none());
    }

    #[test]
    fn validates_session_shape_and_expiration() {
        assert!(
            parse_login(r#"{"token":"test","expiresAt":4102444800}"#)
                .unwrap()
                .is_some()
        );
        assert!(parse_login(r#"{"token":"test","expiresAt":1}"#).is_err());
        assert!(parse_login(r#"{"token":"","expiresAt":4102444800}"#).is_err());
    }
}
