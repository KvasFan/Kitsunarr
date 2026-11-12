//! Webhook (Sonarr's `Webhook`, `WebhookBase`, `WebhookProxy`) and Notifiarr, which receives the
//! same payloads.
//!
//! Payloads keep Sonarr's shape so existing receivers work unchanged: `eventType` in Sonarr's
//! casing (`Grab`, `Download`, ...), the other properties in camelCase.

use serde_json::{Map, Value, json};

use crate::{Event, Instance, Notifier, NotifyError, NotifyResult, required, text};

/// `BuildOn*Payload`.
pub fn payload(event: &Event, instance: &Instance) -> Value {
    let base = |event_type: &str| {
        let mut m = Map::new();
        m.insert("eventType".into(), event_type.into());
        m.insert("instanceName".into(), instance.name.clone().into());
        m.insert(
            "applicationUrl".into(),
            instance.application_url.clone().unwrap_or_default().into(),
        );
        m
    };
    let m = match event {
        Event::Test => {
            let mut m = base("Test");
            m.insert(
                "series".into(),
                json!({"id": 1, "title": "Test Title", "path": "C:\\testpath", "tvdbId": 1234, "type": "standard", "year": 0}),
            );
            m.insert(
                "episodes".into(),
                json!([{"id": 123, "episodeNumber": 1, "seasonNumber": 1, "title": "Test title"}]),
            );
            m
        }
        Event::Grab(g) => {
            let mut m = base("Grab");
            m.insert("series".into(), json!(g.series));
            m.insert("episodes".into(), json!(g.episodes));
            m.insert("release".into(), json!(g.release));
            m.insert("downloadClient".into(), g.download_client.clone().into());
            m.insert(
                "downloadClientType".into(),
                g.download_client_type.clone().into(),
            );
            m.insert("downloadId".into(), g.download_id.clone().into());
            m.insert(
                "customFormatInfo".into(),
                json!({
                    "customFormats": g.release.custom_formats.iter().map(|n| json!({"name": n})).collect::<Vec<_>>(),
                    "customFormatScore": g.release.custom_format_score,
                }),
            );
            m
        }
        Event::Download(d) => {
            let mut m = base("Download");
            m.insert("series".into(), json!(d.series));
            m.insert("episodes".into(), json!(d.episodes));
            m.insert("episodeFile".into(), json!(d.file));
            m.insert("release".into(), json!(d.release));
            m.insert("isUpgrade".into(), (!d.deleted_files.is_empty()).into());
            m.insert("downloadClient".into(), json!(d.download_client));
            m.insert("downloadClientType".into(), json!(d.download_client_type));
            m.insert("downloadId".into(), json!(d.download_id));
            if !d.deleted_files.is_empty() {
                m.insert("deletedFiles".into(), json!(d.deleted_files));
            }
            m
        }
        Event::Rename { series, files } => {
            let mut m = base("Rename");
            m.insert("series".into(), json!(series));
            m.insert("renamedEpisodeFiles".into(), json!(files));
            m
        }
        Event::SeriesAdd(s) => {
            let mut m = base("SeriesAdd");
            m.insert("series".into(), json!(s));
            m
        }
        Event::SeriesDelete {
            series,
            deleted_files,
        } => {
            let mut m = base("SeriesDelete");
            m.insert("series".into(), json!(series));
            m.insert("deletedFiles".into(), (*deleted_files).into());
            m
        }
        Event::EpisodeFileDelete {
            series,
            episodes,
            file,
            reason,
        } => {
            let mut m = base("EpisodeFileDelete");
            m.insert("series".into(), json!(series));
            m.insert("episodes".into(), json!(episodes));
            m.insert("episodeFile".into(), json!(file));
            m.insert("deleteReason".into(), reason.clone().into());
            m
        }
        Event::HealthIssue(h) | Event::HealthRestored(h) => {
            let mut m = base(if matches!(event, Event::HealthIssue(_)) {
                "Health"
            } else {
                "HealthRestored"
            });
            m.remove("applicationUrl");
            m.insert(
                "level".into(),
                (if h.level == "error" {
                    "error"
                } else {
                    "warning"
                })
                .into(),
            );
            m.insert("message".into(), h.message.clone().into());
            m.insert("type".into(), h.source.clone().into());
            m.insert("wikiUrl".into(), json!(h.wiki_url));
            m
        }
    };
    Value::Object(m)
}

/// `WebhookProxy.SendWebhook`: POST (1) or PUT (2), optional basic authentication and headers.
pub async fn send(
    n: &Notifier,
    s: &Map<String, Value>,
    event: &Event,
    instance: &Instance,
) -> NotifyResult {
    let url = required(s, "url", "The webhook URL")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(NotifyError::Settings(
            "the webhook URL must start with http:// or https://".into(),
        ));
    }
    let mut req = match s.get("method").and_then(Value::as_i64).unwrap_or(1) {
        2 => n.http.put(&url),
        _ => n.http.post(&url),
    };
    if let Some(user) = text(s, "username") {
        req = req.basic_auth(user, text(s, "password"));
    }
    if let Some(Value::Array(headers)) = s.get("headers") {
        for h in headers {
            let key = h.get("key").and_then(Value::as_str).unwrap_or_default();
            let value = h.get("value").and_then(Value::as_str).unwrap_or_default();
            if !key.trim().is_empty() {
                req = req.header(key.trim(), value);
            }
        }
    }
    n.post_json(req, &payload(event, instance))
        .await
        .map(|_| ())
}

/// `NotifiarrProxy`: the webhook payload to Notifiarr's Sonarr endpoint, keyed by `apiKey`.
pub async fn notifiarr(
    n: &Notifier,
    s: &Map<String, Value>,
    event: &Event,
    instance: &Instance,
) -> NotifyResult {
    let key = required(s, "apiKey", "The Notifiarr API key")?;
    let base = text(s, "url").unwrap_or_else(|| "https://notifiarr.com".into());
    let url = format!("{}/api/v1/notification/sonarr", base.trim_end_matches('/'));
    n.post_json(
        n.http.post(url).header("X-API-Key", key),
        &payload(event, instance),
    )
    .await
    .map_err(|e| match e {
        NotifyError::Http { status: 401, .. } => {
            NotifyError::Settings("Notifiarr rejected the API key".into())
        }
        other => other,
    })
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::grab;

    #[test]
    fn grab_payload_has_sonarrs_shape() {
        let p = payload(
            &grab(),
            &Instance {
                name: "Kitsunarr".into(),
                application_url: Some("http://k:8990".into()),
            },
        );
        assert_eq!(p["eventType"], "Grab");
        assert_eq!(p["instanceName"], "Kitsunarr");
        assert_eq!(p["series"]["tvdbId"], 81189);
        assert_eq!(p["series"]["type"], "standard");
        assert_eq!(p["series"]["tags"][0], "drama");
        assert!(p["series"].get("tagIds").is_none());
        assert_eq!(p["episodes"][1]["episodeNumber"], 2);
        assert_eq!(p["release"]["qualityVersion"], 2);
        assert_eq!(p["release"]["releaseGroup"], "GRP");
        assert_eq!(p["downloadId"], "ABC");
        assert_eq!(p["customFormatInfo"]["customFormatScore"], 0);
    }
}
