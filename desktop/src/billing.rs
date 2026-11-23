use crate::account::{self, Session};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    pub amount_kopecks: u64,
    pub days: u32,
    currency: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub id: String,
    pub status: String,
    pub days: Option<u32>,
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub pending_sync: bool,
    pub amount_kopecks: Option<u64>,
    pub promo_code: Option<String>,
    #[serde(skip_serializing)]
    payment_url: Option<String>,
    #[serde(default)]
    pub can_pay: bool,
}

#[derive(Deserialize, Serialize)]
pub struct Billing {
    plan: Plan,
    #[serde(default)]
    pub plans: Vec<Plan>,
    pub order: Option<Order>,
    pub instructions: Option<String>,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Checkout {
    Create {
        plan_id: String,
        email: String,
        promo_code: String,
    },
    Resume {
        order_id: String,
    },
}

const UNAVAILABLE: &str = "Оплата временно недоступна.";
const OPEN_FAILED: &str = "Не удалось открыть страницу оплаты. Проверь заказ и попробуй снова.";

fn request<T: serde::de::DeserializeOwned>(
    session: &Session,
    route: &str,
    payload: Option<serde_json::Value>,
) -> Result<T, String> {
    let agent = account::agent();
    let url = format!("{}{route}", account::ROOT);
    let auth = format!("Bearer {}", session.token());
    let response = match payload {
        Some(payload) => agent
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .send(payload.to_string().as_bytes()),
        None => agent.get(&url).header("Authorization", &auth).call(),
    }
    .map_err(|_| UNAVAILABLE)?;
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .into_with_config()
        .limit(128 * 1024)
        .read_to_string()
        .map_err(|_| UNAVAILABLE)?;
    if !(200..300).contains(&status) {
        if status == 401 {
            return Err("Сессия истекла. Войди снова.".into());
        }
        let error = serde_json::from_str::<serde_json::Value>(&body).ok();
        return Err(match error.as_ref().and_then(|e| e["error"].as_str()) {
            Some("invalid_email") => "Укажи корректную почту для чека.",
            Some("invalid_promo") => "Промокод недействителен.",
            Some(
                "promo_order_locked"
                | "existing_order_different_plan"
                | "invoice_requires_review"
                | "order_requires_review",
            ) => "Проверь существующий заказ. Если проблема повторится, обратись в поддержку.",
            Some("rate_limited") => "Слишком много попыток. Попробуй позже.",
            _ => UNAVAILABLE,
        }
        .into());
    }
    serde_json::from_str(&body).map_err(|_| UNAVAILABLE.into())
}

pub fn load(session: &Session) -> Result<Billing, String> {
    let mut billing: Billing = request(session, "/v1/billing", None)?;
    if billing.plan.currency.as_deref() != Some("RUB") {
        return Err(UNAVAILABLE.into());
    }
    if billing.plans.is_empty() {
        let plan = &billing.plan;
        billing.plans.push(Plan {
            id: plan.id.clone(),
            amount_kopecks: plan.amount_kopecks,
            days: plan.days,
            currency: Some("RUB".into()),
        });
    }
    billing.plans.retain(|p| {
        p.days > 0 && p.amount_kopecks > 0 && p.currency.as_deref().is_none_or(|c| c == "RUB")
    });
    if let Some(order) = &mut billing.order {
        order.can_pay = order.url().is_some();
    }
    Ok(billing)
}

pub fn checkout(session: &Session, action: Checkout) -> Result<Order, String> {
    let payload = match action {
        Checkout::Create {
            plan_id,
            email,
            promo_code,
        } => {
            let email = email.trim();
            if email.len() > 254
                || email.contains(char::is_whitespace)
                || !email.split_once('@').is_some_and(|(local, domain)| {
                    !local.is_empty()
                        && !domain.contains('@')
                        && domain
                            .split_once('.')
                            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
                })
            {
                return Err("Укажи корректную почту для чека.".into());
            }
            let promo = promo_code.trim().to_ascii_uppercase();
            if !promo.is_empty()
                && (!(3..=36).contains(&promo.len())
                    || !promo
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
            {
                return Err("Промокод недействителен.".into());
            }
            serde_json::json!({"planId":plan_id, "email":email, "promoCode":promo})
        }
        Checkout::Resume { order_id } => serde_json::json!({"orderId":order_id}),
    };
    #[derive(Deserialize)]
    struct Response {
        order: Order,
    }
    let mut order = request::<Response>(session, "/v1/billing/orders", Some(payload))?.order;
    order.can_pay = order.url().is_some();
    Ok(order)
}

impl Order {
    fn url(&self) -> Option<Url> {
        if self.status != "pending" {
            return None;
        }
        let url = Url::parse(self.payment_url.as_deref()?).ok()?;
        (url.scheme() == "https"
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none())
        .then_some(url)
    }

    pub fn open_checkout(&self) -> Result<(), String> {
        let Some(url) = self.url() else { return Ok(()) };
        open(&url).then_some(()).ok_or_else(|| OPEN_FAILED.into())
    }
}

#[cfg(windows)]
fn open(url: &Url) -> bool {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
        },
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    };
    let encoded: Vec<u16> = url.as_str().encode_utf16().chain([0]).collect();
    unsafe {
        let initialized = CoInitializeEx(
            null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
        let result = ShellExecuteW(
            null_mut(),
            null(),
            encoded.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        ) as isize;
        if initialized >= 0 {
            CoUninitialize();
        }
        result > 32
    }
}

#[cfg(not(windows))]
fn open(url: &Url) -> bool {
    let executable = if cfg!(target_os = "macos") {
        "/usr/bin/open"
    } else {
        "/usr/bin/xdg-open"
    };
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(executable)
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    match child.try_wait() {
        Ok(Some(status)) => status.success(),
        Ok(None) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pending_orders_can_open_https_checkout() {
        for (status, url, allowed) in [
            ("pending", "https://checkout.example.org/order", true),
            ("paid", "https://checkout.example.org/order", false),
            ("pending", "http://checkout.example.org/order", false),
            ("pending", "https://buyer@checkout.example.org/order", false),
            ("pending", "file:///etc/passwd", false),
        ] {
            let order: Order = serde_json::from_value(serde_json::json!({
                "id": "test", "status": status, "paymentUrl": url, "pendingSync": false
            }))
            .unwrap();
            assert_eq!(order.url().is_some(), allowed);
            assert!(
                serde_json::to_value(order)
                    .unwrap()
                    .get("paymentUrl")
                    .is_none()
            );
        }
    }
}
