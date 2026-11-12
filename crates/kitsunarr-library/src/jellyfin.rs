//! Jellyfin watched-state sync: Tanukarr's `JellyfinWatchedService` and `JellyfinWatchPath`.
//!
//! A Jellyfin connection (Settings › Notifications) with "watched sync" on is polled: one
//! user's played flags for every episode of one library are read, the paths translated to
//! Kitsunarr's, and the matching episodes marked watched or unwatched. Optionally, watched
//! episodes are unmonitored, and episodes unmonitored this way are monitored again when they
//! become unwatched. The whole snapshot is validated before anything is written; items Jellyfin
//! does not list never mark anything unwatched.

use std::collections::{HashMap, HashSet};

use kitsunarr_db::Database;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::notify;

/// The largest library read (Tanukarr's limit).
const MAX_ITEMS: i64 = 100_000;
const PAGE: i64 = 200;
const INVALID: &str = "Jellyfin returned an incomplete or invalid watched snapshot; the previous watched state is kept";

/// A Jellyfin connection's watched-sync settings (Tanukarr's field names).
#[derive(Debug, Clone, PartialEq)]
pub struct WatchedSettings {
    pub connection_id: i64,
    pub host: String,
    pub port: i64,
    pub use_ssl: bool,
    pub url_base: String,
    pub api_key: String,
    pub library_id: String,
    pub user_id: String,
    pub interval_minutes: i64,
    pub unmonitor_watched: bool,
    pub remonitor_unwatched: bool,
    pub jellyfin_prefix: Option<String>,
    pub local_prefix: Option<String>,
    pub tags: Vec<i64>,
}

fn text(s: &Map<String, Value>, k: &str) -> Option<String> {
    s.get(k)
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn flag(s: &Map<String, Value>, k: &str) -> bool {
    s.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// A Jellyfin id without hyphens, lower case.
fn plain_id(id: &str) -> String {
    id.replace('-', "").to_lowercase()
}

impl WatchedSettings {
    pub fn from_connection(c: &kitsunarr_notifications::Connection) -> Result<Self, String> {
        let s = &c.settings;
        let required = |k: &str, label: &str| text(s, k).ok_or(format!("{label} is required"));
        Ok(Self {
            connection_id: c.id,
            host: required("host", "The host")?,
            port: s.get("port").and_then(Value::as_i64).unwrap_or(8096),
            use_ssl: flag(s, "useSsl"),
            url_base: text(s, "urlBase").unwrap_or_default(),
            api_key: required("apiKey", "The API key")?,
            library_id: plain_id(&required("libraryId", "The library")?),
            user_id: plain_id(&required("watchedUserId", "The user")?),
            interval_minutes: s
                .get("watchedSyncIntervalMinutes")
                .and_then(Value::as_i64)
                .unwrap_or(5)
                .max(1),
            unmonitor_watched: flag(s, "unmonitorWatchedEpisodes"),
            remonitor_unwatched: flag(s, "remonitorUnwatchedEpisodes"),
            jellyfin_prefix: text(s, "jellyfinPathPrefix"),
            local_prefix: text(s, "kitsunarrPathPrefix").or_else(|| text(s, "tanukarrPathPrefix")),
            tags: c.tags.clone(),
        })
    }

    fn base(&self) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let base = self.url_base.trim_matches('/');
        if base.is_empty() {
            format!("{scheme}://{}:{}", self.host, self.port)
        } else {
            format!("{scheme}://{}:{}/{base}", self.host, self.port)
        }
    }

    /// The source fingerprint: the authority that owns the watched flags it sets.
    pub fn fingerprint(&self) -> String {
        let n = |p: &Option<String>| p.as_deref().and_then(normalize).unwrap_or_default();
        let raw = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.connection_id,
            self.host.to_lowercase(),
            self.port,
            self.use_ssl,
            self.url_base.trim_end_matches('/'),
            self.library_id,
            self.user_id,
            n(&self.jellyfin_prefix),
            n(&self.local_prefix)
        );
        hex::encode(Sha256::digest(raw.as_bytes()))
    }
}

/// `JellyfinWatchPath.Normalize`: forward slashes, no `.`/`..` segments, no trailing slash;
/// Windows paths upper-cased (compared without case). `None` for unusable paths.
pub fn normalize(path: &str) -> Option<String> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    let b = path.as_bytes();
    let windows = path.starts_with("\\\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'/' || b[2] == b'\\'));
    let p = if windows {
        path.replace('\\', "/")
    } else {
        path.to_string()
    };
    if !windows && !p.starts_with('/') {
        return None;
    }
    if p.split('/').any(|seg| seg == "." || seg == "..") {
        return None;
    }
    let trimmed = p.trim_end_matches('/');
    let p = if trimmed.is_empty() { "/" } else { trimmed };
    Some(if windows {
        p.to_uppercase()
    } else {
        p.to_string()
    })
}

/// `JellyfinWatchPath.Translate`: a Jellyfin path as Kitsunarr sees it, or `None` when it lies
/// outside the Jellyfin prefix (directory boundaries respected).
pub fn translate(s: &WatchedSettings, path: &str) -> Option<String> {
    let (Some(jf), Some(local)) = (&s.jellyfin_prefix, &s.local_prefix) else {
        return normalize(path);
    };
    let src = normalize(jf)?.trim_end_matches('/').to_string();
    let full = normalize(path)?;
    let normalized_input = full.clone();
    if !normalized_input.starts_with(&format!("{src}/")) {
        return None;
    }
    // Keep the suffix's own case (a Windows server seen from Linux).
    let raw = path.replace('\\', "/");
    let suffix = &raw[raw.len() - (full.len() - src.len())..];
    normalize(&format!("{}{suffix}", local.trim_end_matches(['/', '\\'])))
}

/// One episode or film of the library, as Jellyfin reports it for the user.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteItem {
    pub path: String,
    pub played: bool,
    pub last_played: Option<String>,
}

async fn get(http: &reqwest::Client, s: &WatchedSettings, path: &str) -> Result<Value, String> {
    let resp = http
        .get(format!("{}{path}", s.base()))
        .header("X-Emby-Token", &s.api_key)
        .header("X-MediaBrowser-Token", &s.api_key)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Jellyfin could not be reached ({e})"))?;
    if !resp.status().is_success() {
        return Err(format!("Jellyfin answered HTTP {}", resp.status().as_u16()));
    }
    resp.json()
        .await
        .map_err(|e| format!("Jellyfin sent an unreadable answer ({e})"))
}

/// `JellyfinWatchProxy.GetSnapshot`: the user must exist, the library must exist and be visible
/// to the user; every page is checked. Any inconsistency rejects the whole snapshot.
pub async fn snapshot(
    http: &reqwest::Client,
    s: &WatchedSettings,
) -> Result<Vec<RemoteItem>, String> {
    let users = get(http, s, "/Users").await?;
    let users = users
        .as_array()
        .ok_or("Jellyfin's user list is not a list")?;
    if !users
        .iter()
        .any(|u| u["Id"].as_str().map(plain_id).as_deref() == Some(s.user_id.as_str()))
    {
        return Err("the selected Jellyfin user does not exist".into());
    }
    let folders = get(http, s, "/Library/VirtualFolders").await?;
    if !folders.as_array().is_some_and(|f| {
        f.iter()
            .any(|x| x["ItemId"].as_str().map(plain_id).as_deref() == Some(s.library_id.as_str()))
    }) {
        return Err("the selected Jellyfin library does not exist".into());
    }
    let lib = get(
        http,
        s,
        &format!("/Users/{}/Items/{}", s.user_id, s.library_id),
    )
    .await?;
    if lib["Id"].as_str().map(plain_id).as_deref() != Some(s.library_id.as_str()) {
        return Err("the Jellyfin user cannot see the library".into());
    }
    let mut items = Vec::new();
    let mut ids = HashSet::new();
    let mut total: Option<i64> = None;
    loop {
        let start = items.len() as i64;
        let page = get(
            http,
            s,
            &format!(
                "/Users/{}/Items?parentId={}&recursive=true&includeItemTypes=Episode,Movie&fields=Path&enableUserData=true&enableTotalRecordCount=true&sortBy=SortName&sortOrder=Ascending&startIndex={start}&limit={PAGE}",
                s.user_id, s.library_id
            ),
        )
        .await?;
        let count = page["TotalRecordCount"]
            .as_i64()
            .filter(|n| *n >= 0)
            .ok_or("Jellyfin did not report a total")?;
        if count > MAX_ITEMS {
            return Err(format!("the library has more than {MAX_ITEMS} items"));
        }
        if *total.get_or_insert(count) != count {
            return Err(INVALID.into());
        }
        let list = page["Items"]
            .as_array()
            .cloned()
            .ok_or("Jellyfin sent a page without items")?;
        if list.len() as i64 > PAGE || items.len() as i64 + list.len() as i64 > count {
            return Err(INVALID.into());
        }
        for it in &list {
            let id = it["Id"].as_str().map(plain_id).unwrap_or_default();
            let played = it["UserData"]["Played"].as_bool();
            let path = it["Path"].as_str();
            if id.is_empty()
                || !ids.insert(id)
                || !matches!(it["Type"].as_str(), Some("Episode" | "Movie"))
                || played.is_none()
                || path.and_then(normalize).is_none()
            {
                return Err(INVALID.into());
            }
            items.push(RemoteItem {
                path: path.unwrap_or_default().to_string(),
                played: played.unwrap_or_default(),
                last_played: it["UserData"]["LastPlayedDate"]
                    .as_str()
                    .map(str::to_string),
            });
        }
        if items.len() as i64 == count {
            break;
        }
        if list.is_empty() {
            return Err(INVALID.into());
        }
    }
    Ok(items)
}

#[derive(sqlx::FromRow)]
struct EpisodeState {
    id: i64,
    monitored: bool,
    watched: bool,
    watched_source: Option<String>,
    jellyfin_unmonitor_owner: Option<String>,
    episode_file_id: i64,
    series_monitored: bool,
    season_monitored: Option<bool>,
}

/// What a sync changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSummary {
    pub matched_files: usize,
    pub updated_episodes: usize,
}

/// `ApplySnapshot`: matches translated paths with library files and updates their episodes.
pub async fn apply(
    db: &Database,
    s: &WatchedSettings,
    items: &[RemoteItem],
    apply_initial_unmonitor: bool,
) -> Result<SyncSummary, String> {
    let err = |e: sqlx::Error| e.to_string();
    // Remote: translated path → played; conflicting duplicates reject the snapshot.
    let mut remote: HashMap<String, &RemoteItem> = HashMap::new();
    for it in items {
        let Some(p) = translate(s, &it.path) else {
            continue;
        };
        if let Some(prev) = remote.get(&p)
            && prev.played != it.played
        {
            return Err(format!(
                "Jellyfin lists {p} twice with different played states"
            ));
        }
        remote.insert(p, it);
    }
    // Local: every file of the series in the connection's tag scope.
    let files: Vec<(i64, i64, String, String)> = sqlx::query_as(
        "SELECT f.id, f.series_id, s.path, f.relative_path FROM episode_files f JOIN series s ON s.id = f.series_id",
    )
    .fetch_all(db.pool())
    .await
    .map_err(err)?;
    let tagged: HashSet<i64> = if s.tags.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT DISTINCT series_id FROM series_tags WHERE tag_id IN (SELECT value FROM json_each(?))",
        )
        .bind(serde_json::to_string(&s.tags).unwrap_or_default())
        .fetch_all(db.pool())
        .await
        .map_err(err)?
        .into_iter()
        .collect()
    };
    let mut local: HashMap<String, Option<i64>> = HashMap::new();
    for (file_id, series_id, series_path, relative) in files {
        if !s.tags.is_empty() && !tagged.contains(&series_id) {
            continue;
        }
        let Some(p) = normalize(&format!(
            "{}/{}",
            series_path.trim_end_matches(['/', '\\']),
            relative.replace('\\', "/")
        )) else {
            continue;
        };
        // A path two files share is ambiguous and skipped.
        local
            .entry(p)
            .and_modify(|v| *v = None)
            .or_insert(Some(file_id));
    }
    let fp = s.fingerprint();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut summary = SyncSummary::default();
    let mut tx = db.pool().begin().await.map_err(err)?;
    for (path, item) in &remote {
        let Some(Some(file_id)) = local.get(path) else {
            continue;
        };
        summary.matched_files += 1;
        let episodes: Vec<EpisodeState> = sqlx::query_as(
            "SELECT e.id, e.monitored, e.watched, e.watched_source, \
             e.jellyfin_unmonitor_owner, e.episode_file_id, s.monitored AS series_monitored, \
             se.monitored AS season_monitored FROM episodes e JOIN series s ON s.id = e.series_id \
             LEFT JOIN seasons se ON se.series_id = e.series_id AND se.season_number = e.season_number \
             WHERE e.episode_file_id = ?",
        )
        .bind(file_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(err)?;
        for e in episodes {
            let played = item.played;
            let same_source = e.watched_source.as_deref() == Some(fp.as_str());
            let unmonitor =
                s.unmonitor_watched && (apply_initial_unmonitor || !e.watched || !same_source);
            let remonitor =
                s.remonitor_unwatched && e.series_monitored && e.season_monitored == Some(true);
            let owned = e.jellyfin_unmonitor_owner.as_deref() == Some(fp.as_str());
            if e.watched == played
                && same_source
                && !(played && unmonitor && e.monitored)
                && !(!played && owned)
            {
                continue;
            }
            let at = item.last_played.clone().unwrap_or_else(|| now.clone());
            // One statement against the current row, so a manual unmonitor made after the
            // snapshot is never overwritten; a replaced file's stale id matches nothing.
            sqlx::query(
                "UPDATE episodes SET watched = ?1, watched_source = ?2, \
                 watched_at = CASE WHEN ?1 THEN CASE WHEN watched_source = ?2 THEN COALESCE(watched_at, ?3) ELSE ?3 END ELSE NULL END, \
                 monitored = CASE WHEN ?1 AND ?4 AND monitored THEN 0 \
                   WHEN NOT ?1 AND ?5 AND jellyfin_unmonitor_owner = ?2 \
                   AND EXISTS (SELECT 1 FROM series s WHERE s.id = episodes.series_id AND s.monitored = 1) THEN 1 \
                   ELSE monitored END, \
                 jellyfin_unmonitor_owner = CASE WHEN ?1 AND ?4 AND monitored THEN ?2 \
                   WHEN NOT ?1 AND jellyfin_unmonitor_owner = ?2 THEN NULL ELSE jellyfin_unmonitor_owner END \
                 WHERE id = ?6 AND episode_file_id = ?7",
            )
            .bind(played)
            .bind(&fp)
            .bind(at)
            .bind(unmonitor)
            .bind(remonitor)
            .bind(e.id)
            .bind(e.episode_file_id)
            .execute(&mut *tx)
            .await
            .map_err(err)?;
            summary.updated_episodes += 1;
        }
    }
    tx.commit().await.map_err(err)?;
    Ok(summary)
}

/// `JellyfinWatchedService.Sync`: the one connection with watched sync on, when due (or
/// `force`). Returns a summary for the task history.
pub async fn sync(db: &Database, http: &reqwest::Client, force: bool) -> Result<String, String> {
    let connections: Vec<_> = notify::connections(db)
        .await
        .into_iter()
        .filter(|c| {
            c.implementation.eq_ignore_ascii_case("Jellyfin")
                && c.settings
                    .get("enableWatchedSync")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .collect();
    let c = match connections.as_slice() {
        [] => return Ok("No Jellyfin connection syncs watched state".into()),
        [one] => one,
        _ => {
            return Err("More than one Jellyfin connection has watched sync on; keep one".into());
        }
    };
    let s = WatchedSettings::from_connection(c)?;
    let fp = s.fingerprint();
    let state: Option<(String, String, bool)> = sqlx::query_as(
        "SELECT target_fingerprint, last_sync_at, unmonitor_watched_episodes FROM jellyfin_watch_states WHERE notification_id = ?",
    )
    .bind(c.id)
    .fetch_optional(db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    if !force
        && let Some((state_fp, last, _)) = &state
        && *state_fp == fp
        && chrono::DateTime::parse_from_rfc3339(last)
            .is_ok_and(|l| now.signed_duration_since(l).num_minutes() < s.interval_minutes)
    {
        return Ok("Not due".into());
    }
    let apply_initial = s.unmonitor_watched
        && state
            .as_ref()
            .is_none_or(|(state_fp, _, unmonitor)| *state_fp != fp || !unmonitor);
    let items = snapshot(http, &s).await?;
    let summary = apply(db, &s, &items, apply_initial).await?;
    sqlx::query(
        "INSERT INTO jellyfin_watch_states (notification_id, target_fingerprint, last_sync_at, unmonitor_watched_episodes) \
         VALUES (?, ?, ?, ?) ON CONFLICT (notification_id) DO UPDATE SET target_fingerprint = excluded.target_fingerprint, \
         last_sync_at = excluded.last_sync_at, unmonitor_watched_episodes = excluded.unmonitor_watched_episodes",
    )
    .bind(c.id)
    .bind(&fp)
    .bind(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    .bind(s.unmonitor_watched)
    .execute(db.pool())
    .await
    .map_err(|e| e.to_string())?;
    Ok(format!(
        "{} items read, {} files matched, {} episodes updated",
        items.len(),
        summary.matched_files,
        summary.updated_episodes
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(jf: Option<&str>, local: Option<&str>) -> WatchedSettings {
        WatchedSettings {
            connection_id: 1,
            host: "jf".into(),
            port: 8096,
            use_ssl: false,
            url_base: String::new(),
            api_key: "k".into(),
            library_id: "lib".into(),
            user_id: "user".into(),
            interval_minutes: 5,
            unmonitor_watched: false,
            remonitor_unwatched: false,
            jellyfin_prefix: jf.map(str::to_string),
            local_prefix: local.map(str::to_string),
            tags: vec![],
        }
    }

    /// Cases after Tanukarr's `JellyfinWatchPathFixture`.
    #[test]
    fn paths_normalize_and_translate() {
        assert_eq!(normalize("/tv/Show/"), Some("/tv/Show".into()));
        assert_eq!(normalize("C:\\TV\\Show"), Some("C:/TV/SHOW".into()));
        assert_eq!(normalize("relative/path"), None);
        assert_eq!(normalize("/tv/../etc"), None);
        assert_eq!(normalize("/"), Some("/".into()));
        let s = settings(Some("/media/tv"), Some("/data/tv"));
        assert_eq!(
            translate(&s, "/media/tv/Show/S01E01.mkv"),
            Some("/data/tv/Show/S01E01.mkv".into())
        );
        // Directory boundary.
        assert_eq!(translate(&s, "/media/tv-other/Show.mkv"), None);
        // A Windows server seen from Linux keeps the suffix's case.
        let w = settings(Some("D:\\Media\\TV"), Some("/data/tv"));
        assert_eq!(
            translate(&w, "D:\\Media\\TV\\Show\\Ep.mkv"),
            Some("/data/tv/Show/Ep.mkv".into())
        );
        assert_ne!(
            settings(None, None).fingerprint(),
            settings(Some("/a"), Some("/b")).fingerprint()
        );
    }
}
