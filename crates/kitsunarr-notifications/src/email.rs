//! Mailgun and SendGrid (Sonarr's `Mailgun`, `SendGrid`): the event's title and message mailed
//! through each service's HTTP API.

use serde_json::{Map, Value, json};

use crate::{Event, Notifier, NotifyResult};

fn recipients(s: &Map<String, Value>) -> Vec<String> {
    match s.get("recipients") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(v)) => v
            .split(',')
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn text(event: &Event) -> (String, String) {
    (format!("Kitsunarr - {}", event.title()), event.message())
}

/// `MailgunProxy.SendNotification`: `POST /v3/{domain}/messages` with the API key.
pub async fn mailgun(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult<()> {
    let key = crate::required(s, "apiKey", "The API key")?;
    let domain = crate::required(s, "senderDomain", "The sender domain")?;
    let from = crate::required(s, "from", "The sender")?;
    let to = recipients(s);
    if to.is_empty() {
        return Err(crate::NotifyError::Settings(
            "Add at least one recipient".into(),
        ));
    }
    let base = crate::text(s, "baseUrl").unwrap_or_else(|| {
        if crate::flag(s, "useEuEndpoint", false) {
            "https://api.eu.mailgun.net".into()
        } else {
            "https://api.mailgun.net".into()
        }
    });
    let (subject, body) = text(event);
    let mut form = vec![("from", from), ("subject", subject), ("text", body)];
    for r in to {
        form.push(("to", r));
    }
    let resp = n
        .http
        .post(format!(
            "{}/v3/{domain}/messages",
            base.trim_end_matches('/')
        ))
        .basic_auth("api", Some(key))
        .form(&form)
        .send()
        .await
        .map_err(crate::connection_error)?;
    crate::check(resp).await.map(|_| ())
}

/// `SendGridProxy.SendNotification`: `POST /v3/mail/send`.
pub async fn sendgrid(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult<()> {
    let key = crate::required(s, "apiKey", "The API key")?;
    let from = crate::required(s, "from", "The sender")?;
    let to = recipients(s);
    if to.is_empty() {
        return Err(crate::NotifyError::Settings(
            "Add at least one recipient".into(),
        ));
    }
    let base = crate::text(s, "baseUrl").unwrap_or_else(|| "https://api.sendgrid.com".into());
    let (subject, body) = text(event);
    let payload = json!({
        "personalizations": [{"to": to.iter().map(|e| json!({"email": e})).collect::<Vec<_>>()}],
        "from": {"email": from},
        "subject": subject,
        "content": [{"type": "text/plain", "value": body}],
    });
    n.post_json(
        n.http
            .post(format!("{}/v3/mail/send", base.trim_end_matches('/')))
            .bearer_auth(key),
        &payload,
    )
    .await
    .map(|_| ())
}
