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
