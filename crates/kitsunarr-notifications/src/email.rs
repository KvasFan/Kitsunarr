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

fn addresses(s: &Map<String, Value>, key: &str) -> Vec<String> {
    match s.get(key) {
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

/// Sonarr's `Email`: SMTP with STARTTLS when offered (`useEncryption` 0), required (1) or off
/// (2); port 465 uses implicit TLS. Recipients in `to`, `cc` and `bcc`.
pub async fn smtp(s: &Map<String, Value>, event: &Event) -> NotifyResult<()> {
    use lettre::message::Mailbox;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::transport::smtp::client::{Tls, TlsParameters};
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let server = crate::required(s, "server", "The SMTP server")?;
    let port = s.get("port").and_then(Value::as_u64).unwrap_or(587) as u16;
    let from: Mailbox = crate::required(s, "from", "The sender")?
        .parse()
        .map_err(|e| crate::NotifyError::Settings(format!("the sender is not an address: {e}")))?;
    let (to, cc, bcc) = (addresses(s, "to"), addresses(s, "cc"), addresses(s, "bcc"));
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(crate::NotifyError::Settings(
            "Add at least one recipient".into(),
        ));
    }
    let (subject, body) = text(event);
    let mut builder = Message::builder().from(from).subject(subject);
    let parse = |a: &String| -> NotifyResult<Mailbox> {
        a.parse()
            .map_err(|e| crate::NotifyError::Settings(format!("{a} is not an address: {e}")))
    };
    for a in &to {
        builder = builder.to(parse(a)?);
    }
    for a in &cc {
        builder = builder.cc(parse(a)?);
    }
    for a in &bcc {
        builder = builder.bcc(parse(a)?);
    }
    let message = builder
        .body(body)
        .map_err(|e| crate::NotifyError::Settings(e.to_string()))?;
    let tls_params = TlsParameters::new(server.clone())
        .map_err(|e| crate::NotifyError::Settings(e.to_string()))?;
    let encryption = s.get("useEncryption").and_then(Value::as_i64).unwrap_or(0);
    let tls = if port == 465 {
        Tls::Wrapper(tls_params)
    } else {
        match encryption {
            1 => Tls::Required(tls_params),
            2 => Tls::None,
            _ => Tls::Opportunistic(tls_params),
        }
    };
    let mut transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(server)
        .port(port)
        .tls(tls)
        .timeout(Some(std::time::Duration::from_secs(30)));
    if let (Some(user), Some(pass)) = (crate::text(s, "username"), crate::text(s, "password")) {
        transport = transport.credentials(Credentials::new(user, pass));
    }
    transport
        .build()
        .send(message)
        .await
        .map(|_| ())
        .map_err(|e| {
            crate::NotifyError::Settings(format!("the mail server refused the message: {e}"))
        })
}
