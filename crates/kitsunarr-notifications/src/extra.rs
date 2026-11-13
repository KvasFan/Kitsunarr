//! Pushcut, Synology Indexer and Trakt (Sonarr's `Pushcut`, `SynologyIndexer`, `Trakt`).

use serde_json::{Map, Value, json};

use crate::{Event, Notifier, NotifyError, NotifyResult, flag, required, text};

/// `PushcutProxy.SendNotification`: the named Pushcut notification, with the poster when asked.
pub async fn pushcut(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let name = required(s, "notificationName", "The notification name")?;
    let key = required(s, "apiKey", "The API key")?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.pushcut.io".into());
    let mut body = json!({
        "title": event.title(),
        "text": event.message(),
        "isTimeSensitive": flag(s, "timeSensitive", false),
        "actions": [],
    });
    if flag(s, "includePoster", false)
        && let Some(poster) = event.series().and_then(|x| x.poster())
    {
        body["image"] = poster.into();
    }
    let url = format!(
        "{}/v1/notifications/{}",
        base.trim_end_matches('/'),
        encode(&name)
    );
    n.post_json(n.http.post(url).header("API-Key", key), &body)
        .await
        .map(|_| ())
}

/// Percent-encodes a URL path segment.
fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `SynologyIndexerProxy`: `synoindex` on the NAS itself keeps its media index current.
pub async fn synology(s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let tool = text(s, "synoindexPath").unwrap_or_else(|| "/usr/syno/bin/synoindex".into());
    if !std::path::Path::new(&tool).is_file() {
        return Err(NotifyError::Settings(
            "synoindex is not available: Kitsunarr must run on the Synology itself".into(),
        ));
    }
    if matches!(event, Event::Test) || !flag(s, "updateLibrary", true) {
        return Ok(());
    }
    let join = |base: &str, rel: &str| format!("{}/{rel}", base.trim_end_matches('/'));
    let mut calls: Vec<(&str, String)> = Vec::new();
    match event {
        Event::Download(d) => {
            for old in &d.deleted_files {
                calls.push(("-d", old.path.clone()));
            }
            calls.push(("-a", join(&d.series.path, &d.file.relative_path)));
        }
        Event::Rename { series, .. } | Event::SeriesAdd(series) => {
            calls.push(("-R", series.path.clone()))
        }
        Event::EpisodeFileDelete { file, .. } => calls.push(("-d", file.path.clone())),
        Event::SeriesDelete {
            series,
            deleted_files: true,
        } => calls.push(("-D", series.path.clone())),
        _ => {}
    }
    for (flag, path) in calls {
        let out = tokio::process::Command::new(&tool)
            .arg(flag)
            .arg(&path)
            .output()
            .await
            .map_err(|e| NotifyError::Connection(format!("synoindex could not run: {e}")))?;
        if !out.stdout.is_empty() || !out.stderr.is_empty() {
            return Err(NotifyError::Service(format!(
                "synoindex returned an error: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )));
        }
    }
    Ok(())
}

/// Sonarr's Trakt resolution names.
fn resolution(r: i32) -> &'static str {
    match r {
        2160 => "uhd_4k",
        1080 => "hd_1080p",
        720 => "hd_720p",
        576 => "sd_576p",
        480 => "sd_480p",
        _ => "",
    }
}

fn media_type(source: kitsunarr_domain::quality::QualitySource) -> &'static str {
    use kitsunarr_domain::quality::QualitySource as Q;
    match source {
        Q::Web | Q::WebRip => "digital",
        Q::Bluray | Q::BlurayRaw => "bluray",
        Q::Television | Q::TelevisionRaw => "vhs",
        Q::Dvd => "dvd",
        Q::Unknown => "",
    }
}

/// `Trakt`: imported episodes join the user's Trakt collection; deleted files and series
/// leave it (`sync/collection`, `sync/collection/remove`).
pub async fn trakt(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let token = required(s, "accessToken", "The access token")?;
    let client_id = required(s, "clientId", "The Client ID of your Trakt application")?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.trakt.tv".into());
    let base = base.trim_end_matches('/').to_string();
    let send = |path: &str, body: Value| {
        n.http
            .post(format!("{base}/{path}"))
            .header("trakt-api-version", "2")
            .header("trakt-api-key", &client_id)
            .bearer_auth(&token)
            .json(&body)
    };
    let show = |series: &crate::SeriesRef, seasons: Value| {
        let mut ids = json!({"tvdb": series.tvdb_id});
        if let Some(imdb) = &series.imdb_id {
            ids["imdb"] = imdb.clone().into();
        }
        json!({"shows": [{"title": series.title, "year": series.year, "ids": ids, "seasons": seasons}]})
    };
    let (path, body) = match event {
        Event::Test => {
            let resp = n
                .http
                .get(format!("{base}/users/settings"))
                .header("trakt-api-version", "2")
                .header("trakt-api-key", &client_id)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(crate::connection_error)?;
            return crate::check(resp).await.map(|_| ());
        }
        Event::Download(d) => {
            let q = kitsunarr_domain::quality::Quality::by_name(&d.file.quality);
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let episodes: Vec<Value> = d
                .episodes
                .iter()
                .map(|e| {
                    json!({"number": e.episode_number, "collected_at": now,
                        "resolution": q.map_or("", |q| resolution(q.resolution)),
                        "media_type": q.map_or("", |q| media_type(q.source))})
                })
                .collect();
            let season = d.episodes.first().map_or(0, |e| e.season_number);
            (
                "sync/collection",
                show(&d.series, json!([{"number": season, "episodes": episodes}])),
            )
        }
        Event::EpisodeFileDelete {
            series, episodes, ..
        } => {
            let season = episodes.first().map_or(0, |e| e.season_number);
            let numbers: Vec<Value> = episodes
                .iter()
                .map(|e| json!({"number": e.episode_number}))
                .collect();
            (
                "sync/collection/remove",
                show(series, json!([{"number": season, "episodes": numbers}])),
            )
        }
        Event::SeriesDelete { series, .. } => {
            let mut ids = json!({"tvdb": series.tvdb_id});
            if let Some(imdb) = &series.imdb_id {
                ids["imdb"] = imdb.clone().into();
            }
            (
                "sync/collection/remove",
                json!({"shows": [{"title": series.title, "year": series.year, "ids": ids}]}),
            )
        }
        _ => return Ok(()),
    };
    let resp = send(path, body)
        .send()
        .await
        .map_err(crate::connection_error)?;
    crate::check(resp).await.map(|_| ())
}

/// Sonarr's Twitter connection on the X API v2: a post (with the mention appended), or a direct
/// message to the mentioned account (`TwitterService.SendNotification`), signed with OAuth 1.0a
/// from the app's consumer key and the account's access token.
pub async fn twitter(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let keys = OAuth1 {
        consumer_key: required(s, "consumerKey", "The consumer key")?,
        consumer_secret: required(s, "consumerSecret", "The consumer secret")?,
        token: required(s, "accessToken", "The access token")?,
        token_secret: required(s, "accessTokenSecret", "The access token secret")?,
    };
    let base = text(s, "apiUrl").unwrap_or_else(|| "https://api.x.com".into());
    let base = base.trim_end_matches('/');
    let mention = text(s, "mention").map(|m| m.trim_start_matches('@').to_string());
    let mut message = twitter_message(event);
    if flag(s, "directMessage", true) {
        let mention = mention.ok_or_else(|| {
            NotifyError::Settings("A mention is required for direct messages".into())
        })?;
        let url = format!("{base}/2/users/by/username/{}", encode(&mention));
        let resp = n
            .http
            .get(&url)
            .header("Authorization", keys.header("GET", &url))
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(crate::connection_error)?;
        let body: Value = serde_json::from_str(&crate::check(resp).await?).unwrap_or_default();
        let id = body["data"]["id"]
            .as_str()
            .ok_or_else(|| NotifyError::Service(format!("The account @{mention} was not found")))?;
        let url = format!("{base}/2/dm_conversations/with/{}/messages", encode(id));
        let req = n
            .http
            .post(&url)
            .header("Authorization", keys.header("POST", &url));
        return n
            .post_json(req, &json!({ "text": message }))
            .await
            .map(|_| ());
    }
    if let Some(m) = mention {
        message.push_str(&format!(" @{m}"));
    }
    let url = format!("{base}/2/tweets");
    let req = n
        .http
        .post(&url)
        .header("Authorization", keys.header("POST", &url));
    n.post_json(req, &json!({ "text": message }))
        .await
        .map(|_| ())
}

/// `Twitter.On…`: the event's message after Sonarr's prefix.
fn twitter_message(event: &Event) -> String {
    let prefix = match event {
        Event::Test => return "This is a test message from Kitsunarr".into(),
        Event::Grab(_) => "Grabbed",
        Event::Download(_) => "Imported",
        Event::EpisodeFileDelete { .. } => "Episode Deleted",
        Event::SeriesAdd(_) => "Series Added",
        Event::SeriesDelete { .. } => "Series Deleted",
        Event::HealthIssue(_) => "Health Issue",
        Event::HealthRestored(_) => "Health Issue Resolved",
        Event::ApplicationUpdate { .. } => "Application Updated",
        Event::ManualInteractionRequired(_) => "Manual Interaction Required",
        Event::Rename { .. } => "Renamed",
    };
    format!("{prefix}: {}", event.message())
}

/// OAuth 1.0a credentials (RFC 5849), HMAC-SHA1 signed.
struct OAuth1 {
    consumer_key: String,
    consumer_secret: String,
    token: String,
    token_secret: String,
}

impl OAuth1 {
    fn header_extra(&self, method: &str, url: &str, extra: &[(&str, &str)]) -> String {
        let (nonce, timestamp) = nonce_and_time();
        self.header_full(method, url, &[], extra, &nonce, &timestamp)
    }

    fn header(&self, method: &str, url: &str) -> String {
        let (nonce, timestamp) = nonce_and_time();
        self.header_with(method, url, &[], &nonce, &timestamp)
    }

    /// `form`: form-encoded body parameters (signed too); JSON bodies are not.
    fn header_with(
        &self,
        method: &str,
        url: &str,
        form: &[(&str, &str)],
        nonce: &str,
        timestamp: &str,
    ) -> String {
        self.header_full(method, url, form, &[], nonce, timestamp)
    }

    /// `extra`: further `oauth_` parameters (`oauth_callback`, `oauth_verifier`); without a
    /// token (`request_token`) no `oauth_token` is sent.
    fn header_full(
        &self,
        method: &str,
        url: &str,
        form: &[(&str, &str)],
        extra: &[(&str, &str)],
        nonce: &str,
        timestamp: &str,
    ) -> String {
        let (base_url, query) = url.split_once('?').unwrap_or((url, ""));
        let mut params: Vec<(String, String)> = vec![
            ("oauth_consumer_key".into(), self.consumer_key.clone()),
            ("oauth_nonce".into(), nonce.into()),
            ("oauth_signature_method".into(), "HMAC-SHA1".into()),
            ("oauth_timestamp".into(), timestamp.into()),
        ];
        if !self.token.is_empty() {
            params.push(("oauth_token".into(), self.token.clone()));
        }
        params.push(("oauth_version".into(), "1.0".into()));
        params.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        params.sort();
        let oauth: Vec<(String, String)> = params.clone();
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            params.push((k.to_string(), v.to_string()));
        }
        params.extend(form.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let mut encoded: Vec<(String, String)> =
            params.iter().map(|(k, v)| (encode(k), encode(v))).collect();
        encoded.sort();
        let joined = encoded
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let base = format!("{method}&{}&{}", encode(base_url), encode(&joined));
        let key = format!(
            "{}&{}",
            encode(&self.consumer_secret),
            encode(&self.token_secret)
        );
        use base64::Engine as _;
        let signature = base64::engine::general_purpose::STANDARD
            .encode(hmac_sha1(key.as_bytes(), base.as_bytes()));
        let mut parts: Vec<String> = oauth
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", encode(v)))
            .collect();
        parts.push(format!("oauth_signature=\"{}\"", encode(&signature)));
        format!("OAuth {}", parts.join(", "))
    }
}

fn nonce_and_time() -> (String, String) {
    let nonce = hex::encode(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
            .to_le_bytes(),
    );
    (nonce, chrono::Utc::now().timestamp().to_string())
}

/// `TwitterProxy.GetOAuthRedirect` (OAuth 1.0a, step 1): a request token for the app, and the
/// page where the account authorizes it. Returns (authorize URL, token, token secret).
pub async fn twitter_request_token(
    http: &reqwest::Client,
    api_url: &str,
    consumer_key: &str,
    consumer_secret: &str,
    callback: &str,
) -> NotifyResult<(String, String, String)> {
    let keys = OAuth1 {
        consumer_key: consumer_key.into(),
        consumer_secret: consumer_secret.into(),
        token: String::new(),
        token_secret: String::new(),
    };
    let base = api_url.trim_end_matches('/');
    let url = format!("{base}/oauth/request_token");
    let resp = http
        .post(&url)
        .header(
            "Authorization",
            keys.header_extra("POST", &url, &[("oauth_callback", callback)]),
        )
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(crate::connection_error)?;
    let body = crate::check(resp).await?;
    let token = form_value(&body, "oauth_token")
        .ok_or_else(|| NotifyError::Service("X sent no request token".into()))?;
    let secret = form_value(&body, "oauth_token_secret").unwrap_or_default();
    Ok((
        format!("{base}/oauth/authorize?oauth_token={}", encode(&token)),
        token,
        secret,
    ))
}

/// `TwitterProxy.GetOAuthToken` (step 3): the account's access token and secret.
pub async fn twitter_access_token(
    http: &reqwest::Client,
    api_url: &str,
    consumer_key: &str,
    consumer_secret: &str,
    request_token: &str,
    request_secret: &str,
    verifier: &str,
) -> NotifyResult<(String, String)> {
    let keys = OAuth1 {
        consumer_key: consumer_key.into(),
        consumer_secret: consumer_secret.into(),
        token: request_token.into(),
        token_secret: request_secret.into(),
    };
    let url = format!("{}/oauth/access_token", api_url.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .header(
            "Authorization",
            keys.header_extra("POST", &url, &[("oauth_verifier", verifier)]),
        )
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(crate::connection_error)?;
    let body = crate::check(resp).await?;
    match (
        form_value(&body, "oauth_token"),
        form_value(&body, "oauth_token_secret"),
    ) {
        (Some(t), Some(s)) => Ok((t, s)),
        _ => Err(NotifyError::Service("X sent no access token".into())),
    }
}

/// A value of a form-encoded answer.
fn form_value(body: &str, key: &str) -> Option<String> {
    body.trim().split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| {
            v.replace("%2F", "/")
                .replace("%3D", "=")
                .replace("%2B", "+")
        })
    })
}

/// HMAC-SHA1 (RFC 2104).
fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    use sha1::{Digest, Sha1};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha1::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha1::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

#[cfg(test)]
mod twitter_tests {
    use super::*;

    /// The worked example of X's "Creating a signature" documentation.
    #[test]
    fn oauth1_signatures_match_the_reference() {
        let keys = OAuth1 {
            consumer_key: "xvz1evFS4wEEPTGEFPHBog".into(),
            consumer_secret: "kAcSOqF21Fu85e7zjz7ZN2U4ZRhfV3WpwPAoE3Z7kBw".into(),
            token: "370773112-GmHxMAgYyLbNEtIKZeRNFsMKPR9EyMZeS9weJAEb".into(),
            token_secret: "LswwdoUaIvS8ltyTt5jkRh4J50vUPVVHtR2YPi5kE".into(),
        };
        let header = keys.header_with(
            "POST",
            "https://api.twitter.com/1.1/statuses/update.json?include_entities=true",
            &[(
                "status",
                "Hello Ladies + Gentlemen, a signed OAuth request!",
            )],
            "kYjzVBB8Y0ZFabxSWbWovY3uYSQ2pTgmZeNu2VS4cg",
            "1318622958",
        );
        assert!(header.starts_with("OAuth oauth_consumer_key=\"xvz1evFS4wEEPTGEFPHBog\""));
        assert!(
            header.ends_with("oauth_signature=\"hCtSmYh%2BiHYCEqBWrE7C7hYmtUk%3D\""),
            "{header}"
        );
        assert_eq!(
            hex::encode(hmac_sha1(
                b"key",
                b"The quick brown fox jumps over the lazy dog"
            )),
            "de7c9b85b8b78aa6bc8a7a36f70a90701c9db4d9"
        );
    }

    #[test]
    fn messages_carry_sonarrs_prefixes() {
        assert_eq!(
            twitter_message(&Event::HealthIssue(crate::Health {
                level: "warning".into(),
                message: "Indexers are down".into(),
                source: "IndexerStatusCheck".into(),
                wiki_url: None,
            })),
            "Health Issue: Indexers are down"
        );
    }
}
