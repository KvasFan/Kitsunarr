//! Media servers: Emby and Jellyfin (Sonarr's `MediaBrowser`, Tanukarr's `Jellyfin`), Plex Media
//! Server (`PlexServer`) and Kodi (`Xbmc`). They refresh the series' folder when files change
//! (imports, deletions) and, where the server supports it, show the notification.

use serde_json::{Map, Value, json};

use crate::{Event, Notifier, NotifyError, NotifyResult, SeriesRef, base_url, flag, int, text};

/// `MapFrom`/`MapTo` (Sonarr) or `tanukarrPathPrefix`/`jellyfinPathPrefix` (Tanukarr): the
/// series path as the media server sees it.
pub fn map_path(s: &Map<String, Value>, path: &str) -> String {
    let pairs = [
        (text(s, "mapFrom"), text(s, "mapTo")),
        (text(s, "tanukarrPathPrefix"), text(s, "jellyfinPathPrefix")),
    ];
    for (from, to) in pairs {
        if let (Some(from), Some(to)) = (from, to) {
            let from = from.trim_end_matches(['/', '\\']);
            if let Some(rest) = path.strip_prefix(from)
                && (rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\'))
            {
                return format!("{}{rest}", to.trim_end_matches(['/', '\\']));
            }
        }
    }
    path.to_string()
}

/// The library change an event makes, if any: (series, Emby update type).
fn library_change(event: &Event) -> Option<(&SeriesRef, &'static str)> {
    match event {
        Event::Download(d) => Some((&d.series, "Created")),
        Event::EpisodeFileDelete { series, .. } => Some((series, "Deleted")),
        Event::SeriesDelete { series, .. } => Some((series, "Deleted")),
        _ => None,
    }
}

fn notification_text(event: &Event) -> (String, String) {
    let message = match event {
        Event::HealthRestored(h) => format!("The following issue is now resolved: {}", h.message),
        other => other.message(),
    };
    (format!("Kitsunarr - {}", event.title()), message)
}

/// Emby and Jellyfin: `POST /Notifications/Admin` when `notify` is on, and
/// `POST /Library/Media/Updated` for the series folder when `updateLibrary` is on (Jellyfin
/// connections migrated from Tanukarr refresh their library). A test reads
/// `/System/Configuration`.
pub async fn emby(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let base = base_url(s, 8096)?;
    let key = crate::required(s, "apiKey", "The API key")?;
    let request = |method: reqwest::Method, path: &str| {
        n.http
            .request(method, format!("{base}{path}"))
            .header("X-MediaBrowser-Token", key.clone())
            .header("X-Emby-Token", key.clone())
    };
    if matches!(event, Event::Test) {
        let resp = request(reqwest::Method::GET, "/System/Configuration")
            .send()
            .await
            .map_err(crate::connection_error)?;
        return crate::check(resp).await.map(|_| ()).map_err(|e| match e {
            NotifyError::Http { status: 401, .. } => {
                NotifyError::Settings("the server rejected the API key".into())
            }
            other => other,
        });
    }
    // Tanukarr's Jellyfin connections have no "notify" switch; they refresh only.
    if flag(s, "notify", false) {
        let (title, message) = notification_text(event);
        let body = json!({
            "Name": title,
            "Description": message,
            "ImageUrl": "https://raw.githubusercontent.com/KvasFan/Kitsunarr/main/crates/kitsunarr-web/assets/favicon.svg",
        });
        if let Err(e) = n
            .post_json(
                request(reqwest::Method::POST, "/Notifications/Admin"),
                &body,
            )
            .await
        {
            // Jellyfin 10.9 removed admin notifications; the refresh still matters.
            tracing::debug!(error = %e, "the media server did not take the notification");
        }
    }
    let update = s
        .get("updateLibrary")
        .and_then(Value::as_bool)
        .unwrap_or(s.contains_key("refreshMode") || s.contains_key("libraryId"));
    if update && let Some((series, kind)) = library_change(event) {
        let path = map_path(s, &series.path);
        let body = json!({"Updates": [{"Path": path, "UpdateType": kind}]});
        n.post_json(
            request(reqwest::Method::POST, "/Library/Media/Updated"),
            &body,
        )
        .await?;
    }
    Ok(())
}

/// Plex Media Server: refresh the TV sections that hold the series folder
/// (`GET /library/sections`, then `GET /library/sections/{key}/refresh?path=`).
pub async fn plex(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let base = base_url(s, 32400)?;
    let token = crate::required(s, "authToken", "The Plex token")?;
    let get = |path: &str| {
        n.http
            .get(format!("{base}{path}"))
            .header("X-Plex-Token", token.clone())
            .header("Accept", "application/json")
    };
    let resp = get("/library/sections")
        .send()
        .await
        .map_err(crate::connection_error)?;
    let body = crate::check(resp).await.map_err(|e| match e {
        NotifyError::Http { status: 401, .. } => {
            NotifyError::Settings("Plex rejected the token".into())
        }
        other => other,
    })?;
    let sections: Value = serde_json::from_str(&body)
        .map_err(|e| NotifyError::Service(format!("Plex sent an unreadable answer ({e})")))?;
    let dirs = sections["MediaContainer"]["Directory"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if matches!(event, Event::Test) {
        return Ok(());
    }
    if !flag(s, "updateLibrary", true) {
        return Ok(());
    }
    let Some((series, _)) = library_change(event) else {
        return Ok(());
    };
    let path = map_path(s, &series.path);
    for d in dirs.iter().filter(|d| d["type"] == "show") {
        let key = d["key"].as_str().unwrap_or_default();
        let holds = d["Location"].as_array().is_some_and(|locations| {
            locations.iter().any(|l| {
                l["path"].as_str().is_some_and(|p| {
                    let p = p.trim_end_matches(['/', '\\']);
                    path == p
                        || path.starts_with(&format!("{p}/"))
                        || path.starts_with(&format!("{p}\\"))
                })
            })
        });
        if holds {
            let resp = get(&format!("/library/sections/{key}/refresh"))
                .query(&[("path", path.as_str())])
                .send()
                .await
                .map_err(crate::connection_error)?;
            crate::check(resp).await?;
        }
    }
    Ok(())
}

/// Kodi JSON-RPC: `GUI.ShowNotification` when `notify` is on; `VideoLibrary.Scan` of the series
/// folder (and `VideoLibrary.Clean` when asked) when the library changes.
pub async fn kodi(n: &Notifier, s: &Map<String, Value>, event: &Event) -> NotifyResult {
    let base = base_url(s, 8080)?;
    let call = |method: &str, params: Value| {
        let mut req = n.http.post(format!("{base}/jsonrpc"));
        if let Some(user) = text(s, "username") {
            req = req.basic_auth(user, text(s, "password"));
        }
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params, "id": 1});
        async move { n.post_json(req, &body).await }
    };
    if matches!(event, Event::Test) {
        return call("JSONRPC.Version", json!({})).await.map(|_| ());
    }
    if flag(s, "notify", false) {
        let (title, message) = notification_text(event);
        call(
            "GUI.ShowNotification",
            json!({"title": title, "message": message, "displaytime": int(s, "displayTime", 5) * 1000}),
        )
        .await?;
    }
    if flag(s, "updateLibrary", false)
        && let Some((series, _)) = library_change(event)
    {
        let dir = format!(
            "{}/",
            map_path(s, &series.path).trim_end_matches(['/', '\\'])
        );
        call("VideoLibrary.Scan", json!({"directory": dir})).await?;
        if flag(s, "cleanLibrary", false) {
            call("VideoLibrary.Clean", json!({})).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_mapped_by_prefix() {
        let s = json!({"mapFrom": "/tv", "mapTo": "/media/tv/"});
        let s = s.as_object().unwrap();
        assert_eq!(map_path(s, "/tv/Breaking Bad"), "/media/tv/Breaking Bad");
        assert_eq!(map_path(s, "/tvx/Other"), "/tvx/Other");
        let t = json!({"tanukarrPathPrefix": "/data/tv", "jellyfinPathPrefix": "/srv/tv"});
        assert_eq!(
            map_path(t.as_object().unwrap(), "/data/tv/Show"),
            "/srv/tv/Show"
        );
    }
}
