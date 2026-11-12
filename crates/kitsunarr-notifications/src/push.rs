//! Chat and push services, each sending Sonarr's title and message: Slack, Telegram, Gotify,
//! ntfy, Pushover, Pushbullet, Join, Prowl, Simplepush, Signal (signal-cli REST API) and
//! Apprise. Settings use Sonarr's field names, so migrated connections work unchanged.

use serde_json::{Map, Value, json};

use crate::{
    Event, Notifier, NotifyError, NotifyResult, base_url, flag, int, required, strings, text,
};

fn branded(title: &str) -> String {
    format!("Kitsunarr - {title}")
}

/// `Slack`: an attachment with the title, message and Sonarr's colours.
pub async fn slack(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let url = required(s, "webHookUrl", "The webhook URL")?;
    let color = match event {
        Event::Download(_) | Event::SeriesAdd(_) | Event::HealthRestored(_) => "good",
        Event::HealthIssue(h) if h.level == "error" => "danger",
        Event::HealthIssue(_) => "warning",
        Event::SeriesDelete { .. } | Event::EpisodeFileDelete { .. } => "danger",
        _ => "warning",
    };
    let mut body = json!({
        "attachments": [{
            "fallback": event.message(),
            "title": event.title(),
            "text": event.message(),
            "color": color,
        }]
    });
    if let Some(u) = text(s, "username") {
        body["username"] = u.into();
    }
    if let Some(c) = text(s, "channel") {
        body["channel"] = c.into();
    }
    if let Some(icon) = text(s, "icon") {
        if icon.starts_with(':') {
            body["icon_emoji"] = icon.into();
        } else {
            body["icon_url"] = icon.into();
        }
    }
    n.post_json(n.http.post(url), &body).await.map(|_| ())
}

/// `TelegramProxy.SendNotification`: HTML message to a chat (and topic).
pub async fn telegram(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let token = required(s, "botToken", "The bot token")?;
    let chat = required(s, "chatId", "The chat id")?;
    let title = if flag(s, "includeAppNameInTitle", false) {
        branded(event.title())
    } else {
        event.title().to_string()
    };
    let escape = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut body = json!({
        "chat_id": chat,
        "parse_mode": "HTML",
        "text": format!("<b>{}</b>\n{}", escape(&title), escape(&event.message())),
        "disable_notification": flag(s, "sendSilently", false),
        "link_preview_options": {"is_disabled": true},
    });
    if let Some(topic) = text(s, "topicId") {
        body["message_thread_id"] = topic
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or(topic.into());
    }
    let base = text(s, "apiUrl").unwrap_or_else(|| "https://api.telegram.org".into());
    let url = format!("{}/bot{token}/sendMessage", base.trim_end_matches('/'));
    n.post_json(n.http.post(url), &body)
        .await
        .map_err(|e| match e {
            NotifyError::Http { status: 401, .. } => {
                NotifyError::Settings("Telegram rejected the bot token".into())
            }
            other => other,
        })
        .map(|_| ())
}

/// `GotifyProxy`: `POST {server}/message` with the app token.
pub async fn gotify(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let server = required(s, "server", "The Gotify server")?;
    let token = required(s, "appToken", "The app token")?;
    let mut body = json!({
        "title": event.title(),
        "message": event.message(),
        "priority": int(s, "priority", 5),
    });
    if flag(s, "includeSeriesPoster", false)
        && let Some(poster) = event.series().and_then(|x| x.poster())
    {
        body["message"] = format!("![]({poster})\n{}", event.message()).into();
        body["extras"] = json!({"client::display": {"contentType": "text/markdown"}});
    }
    let url = format!("{}/message", server.trim_end_matches('/'));
    n.post_json(n.http.post(url).header("X-Gotify-Key", token), &body)
        .await
        .map(|_| ())
}

/// `NtfyProxy`: one POST per topic, with title, priority, tags and click headers.
pub async fn ntfy(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let server = text(s, "serverUrl").unwrap_or_else(|| "https://ntfy.sh".into());
    let topics = strings(s, "topics");
    if topics.is_empty() {
        return Err(NotifyError::Settings(
            "at least one topic is required".into(),
        ));
    }
    let mut last = Ok(());
    for topic in topics {
        let mut req = n
            .http
            .post(format!("{}/{topic}", server.trim_end_matches('/')))
            .header("X-Title", event.title())
            .header("X-Priority", int(s, "priority", 3).to_string())
            .body(event.message());
        let tags = strings(s, "tags");
        if !tags.is_empty() {
            req = req.header("X-Tags", tags.join(","));
        }
        if let Some(click) = text(s, "clickUrl") {
            req = req.header("X-Click", click);
        }
        if let Some(token) = text(s, "accessToken") {
            req = req.bearer_auth(token);
        } else if let Some(user) = text(s, "userName") {
            req = req.basic_auth(user, text(s, "password"));
        }
        let resp = req.send().await.map_err(crate::connection_error)?;
        if let Err(e) = crate::check(resp).await {
            last = Err(e);
        }
    }
    last
}

/// `PushoverProxy`: the messages API, with devices, priority (emergency: retry/expire) and
/// sound.
pub async fn pushover(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let token = required(s, "apiKey", "The API key")?;
    let user = required(s, "userKey", "The user key")?;
    let priority = int(s, "priority", 0);
    let mut form = vec![
        ("token", token),
        ("user", user),
        ("title", event.title().to_string()),
        ("message", event.message()),
        ("priority", priority.to_string()),
    ];
    let devices = strings(s, "devices");
    if !devices.is_empty() {
        form.push(("device", devices.join(",")));
    }
    if priority == 2 {
        form.push(("retry", int(s, "retry", 60).to_string()));
        form.push(("expire", int(s, "expire", 3600).to_string()));
    }
    if let Some(sound) = text(s, "sound") {
        form.push(("sound", sound));
    }
    let ttl = int(s, "ttl", 0);
    if ttl > 0 {
        form.push(("ttl", ttl.to_string()));
    }
    let resp = n
        .http
        .post("https://api.pushover.net/1/messages.json")
        .form(&form)
        .send()
        .await
        .map_err(crate::connection_error)?;
    crate::check(resp).await.map(|_| ())
}

/// `PushBulletProxy`: a note pushed to every device id and channel tag (or to all devices).
pub async fn pushbullet(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let key = required(s, "apiKey", "The access token")?;
    let base = json!({"type": "note", "title": event.title(), "body": event.message()});
    let mut targets: Vec<(&str, String)> = Vec::new();
    for d in strings(s, "deviceIds") {
        targets.push(("device_iden", d));
    }
    for c in strings(s, "channelTags") {
        targets.push(("channel_tag", c));
    }
    if targets.is_empty() {
        targets.push(("", String::new()));
    }
    for (k, v) in targets {
        let mut body = base.clone();
        if !k.is_empty() {
            body[k] = v.into();
        }
        if let Some(sender) = text(s, "senderId") {
            body["source_device_iden"] = sender.into();
        }
        n.post_json(
            n.http
                .post("https://api.pushbullet.com/v2/pushes")
                .header("Access-Token", key.clone()),
            &body,
        )
        .await?;
    }
    Ok(())
}

/// `JoinProxy`: `sendPush` to the chosen devices (or the group of all devices).
pub async fn join(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let key = required(s, "apiKey", "The API key")?;
    let mut q: Vec<(&str, String)> = vec![
        ("apikey", key),
        ("title", event.title().to_string()),
        ("text", event.message()),
        ("icon", "https://raw.githubusercontent.com/KvasFan/Kitsunarr/main/crates/kitsunarr-web/assets/favicon.svg".into()),
        ("priority", int(s, "priority", 0).to_string()),
    ];
    let names = strings(s, "deviceNames");
    let ids = strings(s, "deviceIds");
    if !names.is_empty() {
        q.push(("deviceNames", names.join(",")));
    } else if !ids.is_empty() {
        q.push(("deviceIds", ids.join(",")));
    } else {
        q.push(("deviceId", "group.all".into()));
    }
    let resp = n
        .http
        .get("https://joinjoaomgcd.appspot.com/_ah/api/messaging/v1/sendPush")
        .query(&q)
        .send()
        .await
        .map_err(crate::connection_error)?;
    let body = crate::check(resp).await?;
    let v: Value = serde_json::from_str(&body).unwrap_or_default();
    if v.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(NotifyError::Service(
            v.get("errorMessage")
                .and_then(Value::as_str)
                .unwrap_or("Join refused the notification")
                .into(),
        ));
    }
    Ok(())
}

/// `ProwlProxy`: the public `add` API.
pub async fn prowl(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let key = required(s, "apiKey", "The API key")?;
    let resp = n
        .http
        .post("https://api.prowlapp.com/publicapi/add")
        .form(&[
            ("apikey", key),
            ("application", "Kitsunarr".into()),
            ("event", event.title().to_string()),
            ("description", event.message()),
            ("priority", int(s, "priority", 0).to_string()),
        ])
        .send()
        .await
        .map_err(crate::connection_error)?;
    crate::check(resp).await.map(|_| ())
}

/// `SimplepushProxy`.
pub async fn simplepush(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let key = required(s, "key", "The key")?;
    let mut body = json!({"key": key, "title": event.title(), "msg": event.message()});
    if let Some(e) = text(s, "event") {
        body["event"] = e.into();
    }
    n.post_json(n.http.post("https://api.simplepush.io/send"), &body)
        .await
        .map(|_| ())
}

/// `SignalProxy`: signal-cli REST API (`POST /v2/send`).
pub async fn signal(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let base = base_url(s, 8080)?;
    let sender = required(s, "senderNumber", "The sender number")?;
    let receiver = required(s, "receiverId", "The receiver")?;
    let body = json!({
        "message": format!("{}\n\n{}", event.title(), event.message()),
        "number": sender,
        "recipients": [receiver],
    });
    let mut req = n.http.post(format!("{base}/v2/send"));
    if let Some(user) = text(s, "authUsername") {
        req = req.basic_auth(user, text(s, "authPassword"));
    }
    n.post_json(req, &body).await.map(|_| ())
}

/// `AppriseProxy`: a configuration key (`/notify/{key}`) or stateless URLs (`/notify`).
pub async fn apprise(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let server = required(s, "serverUrl", "The Apprise server URL")?;
    let kind = match int(s, "notificationType", 0) {
        1 => "success",
        2 => "warning",
        3 => "failure",
        _ => "info",
    };
    let mut body = json!({"title": event.title(), "body": event.message(), "type": kind});
    let tags = strings(s, "tags");
    if !tags.is_empty() {
        body["tag"] = tags.join(",").into();
    }
    let url = match text(s, "configurationKey") {
        Some(key) => format!("{}/notify/{key}", server.trim_end_matches('/')),
        None => {
            let urls = strings(s, "statelessUrls");
            if urls.is_empty() {
                return Err(NotifyError::Settings(
                    "a configuration key or stateless URLs are required".into(),
                ));
            }
            body["urls"] = urls.join(",").into();
            format!("{}/notify", server.trim_end_matches('/'))
        }
    };
    let mut req = n.http.post(url);
    if let Some(user) = text(s, "authUsername") {
        req = req.basic_auth(user, text(s, "authPassword"));
    }
    n.post_json(req, &body).await.map(|_| ())
}
