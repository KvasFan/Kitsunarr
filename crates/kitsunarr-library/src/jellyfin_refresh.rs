//! Jellyfin library refresh queue: Tanukarr's `JellyfinRefreshService` and `JellyfinProxy`.
//!
//! A Jellyfin connection with a selected library refreshes that library after imports. Each
//! import bumps the connection's imported generation; "each import" mode refreshes right away
//! (one request per import), "interval" mode refreshes once per interval when imports are
//! pending, and both can be combined. Before every refresh the library must be ready (every
//! folder known to Jellyfin); a failed refresh keeps the work pending and the scheduled task
//! retries it every minute. Generations survive restarts.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, SecondsFormat, Utc};
use kitsunarr_db::Database;
use kitsunarr_notifications::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::notify;

/// Connections with a refresh running (Tanukarr's in-flight set).
static IN_FLIGHT: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(Default::default);
/// Serialises state reads and writes (Tanukarr's per-connection lock).
static STATE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    EachImport = 0,
    Interval = 1,
    EachImportAndInterval = 2,
}

impl Mode {
    fn immediate(self) -> bool {
        matches!(self, Self::EachImport | Self::EachImportAndInterval)
    }
    fn interval(self) -> bool {
        matches!(self, Self::Interval | Self::EachImportAndInterval)
    }
}

/// A connection's refresh settings.
#[derive(Debug, Clone)]
pub struct RefreshSettings {
    pub id: i64,
    base: String,
    api_key: String,
    library: String,
    pub mode: Mode,
    pub interval_minutes: i64,
    fingerprint: String,
}

fn text(c: &Connection, k: &str) -> Option<String> {
    c.settings
        .get(k)
        .and_then(Value::as_str)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl RefreshSettings {
    /// `IsActive`: a saved `Jellyfin` connection with "on import" and a library.
    pub fn from_connection(c: &Connection) -> Option<Self> {
        if !c.implementation.eq_ignore_ascii_case("Jellyfin")
            || c.id <= 0
            || !c
                .triggers
                .get("onDownload")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return None;
        }
        let library = text(c, "libraryId")?.replace('-', "").to_lowercase();
        let host = text(c, "host")?;
        let api_key = text(c, "apiKey")?;
        let port = c
            .settings
            .get("port")
            .and_then(Value::as_i64)
            .unwrap_or(8096);
        let ssl = c
            .settings
            .get("useSsl")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let url_base = text(c, "urlBase").unwrap_or_default();
        let url_base = url_base.trim_end_matches('/');
        let mode = match c.settings.get("refreshMode").and_then(Value::as_i64) {
            Some(0) => Mode::EachImport,
            Some(2) => Mode::EachImportAndInterval,
            _ => Mode::Interval,
        };
        // The API key is not part of the target: rotating it keeps pending work.
        let fingerprint = hex::encode(Sha256::digest(
            format!(
                "{}\n{port}\n{ssl}\n{url_base}\n{library}",
                host.to_lowercase()
            )
            .as_bytes(),
        ));
        let scheme = if ssl { "https" } else { "http" };
        let url_base = url_base.trim_start_matches('/');
        Some(Self {
            id: c.id,
            base: if url_base.is_empty() {
                format!("{scheme}://{host}:{port}")
            } else {
                format!("{scheme}://{host}:{port}/{url_base}")
            },
            api_key,
            library,
            mode,
            interval_minutes: c
                .settings
                .get("refreshIntervalMinutes")
                .and_then(Value::as_i64)
                .unwrap_or(60)
                .max(1),
            fingerprint,
        })
    }
}

/// A `Jellyfin` connection with a selected library: refreshed through the queue only.
pub fn is_queue_managed(c: &Connection) -> bool {
    c.implementation.eq_ignore_ascii_case("Jellyfin") && text(c, "libraryId").is_some()
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct State {
    target_fingerprint: String,
    state_token: String,
    last_refresh_mode: i64,
    imported_generation: i64,
    immediate_generation: i64,
    tick_generation: i64,
    last_tick_at: String,
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

async fn save(db: &Database, id: i64, s: &State) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO jellyfin_refresh_states (notification_id, target_fingerprint, state_token, last_refresh_mode, \
         imported_generation, immediate_generation, tick_generation, last_tick_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (notification_id) DO UPDATE SET target_fingerprint = excluded.target_fingerprint, \
         state_token = excluded.state_token, last_refresh_mode = excluded.last_refresh_mode, \
         imported_generation = excluded.imported_generation, immediate_generation = excluded.immediate_generation, \
         tick_generation = excluded.tick_generation, last_tick_at = excluded.last_tick_at",
    )
    .bind(id)
    .bind(&s.target_fingerprint)
    .bind(&s.state_token)
    .bind(s.last_refresh_mode)
    .bind(s.imported_generation)
    .bind(s.immediate_generation)
    .bind(s.tick_generation)
    .bind(&s.last_tick_at)
    .execute(db.pool())
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// `GetState`: creates the row, resets it when the target changed, and settles a mode change
/// (acknowledged work is not replayed; pending imports stay pending).
async fn state(db: &Database, r: &RefreshSettings) -> Result<State, String> {
    let row: Option<State> = sqlx::query_as(
        "SELECT target_fingerprint, state_token, last_refresh_mode, imported_generation, immediate_generation, \
         tick_generation, last_tick_at FROM jellyfin_refresh_states WHERE notification_id = ?",
    )
    .bind(r.id)
    .fetch_optional(db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let mode = r.mode as i64;
    let fresh = || State {
        target_fingerprint: r.fingerprint.clone(),
        state_token: token(),
        last_refresh_mode: mode,
        imported_generation: 0,
        immediate_generation: 0,
        tick_generation: 0,
        last_tick_at: now(),
    };
    let s = match row {
        Some(s) if s.target_fingerprint == r.fingerprint => {
            if s.last_refresh_mode == mode {
                return Ok(s);
            }
            let acked = s.immediate_generation.max(s.tick_generation);
            State {
                last_refresh_mode: mode,
                immediate_generation: acked,
                tick_generation: acked,
                last_tick_at: now(),
                ..s
            }
        }
        _ => fresh(),
    };
    save(db, r.id, &s).await?;
    Ok(s)
}

/// `OnImport`: one call per imported file.
pub async fn on_import(db: &Database, http: &reqwest::Client, r: &RefreshSettings) {
    {
        let _g = STATE.lock().await;
        match state(db, r).await {
            Ok(mut s) => {
                s.imported_generation = s.imported_generation.saturating_add(1);
                if let Err(e) = save(db, r.id, &s).await {
                    tracing::warn!(connection = r.id, error = %e, "Jellyfin refresh state could not be saved");
                    return;
                }
            }
            Err(e) => {
                tracing::warn!(connection = r.id, error = %e, "Jellyfin refresh state could not be read");
                return;
            }
        }
    }
    if r.mode.immediate() {
        try_refresh(db, http, r, false).await;
    }
}

/// `ProcessPending`: drops the state of connections that no longer refresh, then refreshes the
/// due ones. One failing server does not stop the others.
pub async fn process_pending(db: &Database, http: &reqwest::Client) -> Result<String, String> {
    let active: Vec<RefreshSettings> = notify::connections(db)
        .await
        .iter()
        .filter_map(RefreshSettings::from_connection)
        .collect();
    let ids: Vec<i64> = active.iter().map(|r| r.id).collect();
    {
        let _g = STATE.lock().await;
        sqlx::query("DELETE FROM jellyfin_refresh_states WHERE notification_id NOT IN (SELECT value FROM json_each(?))")
            .bind(serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into()))
            .execute(db.pool())
            .await
            .map_err(|e| e.to_string())?;
    }
    if active.is_empty() {
        return Ok("No Jellyfin connection refreshes a library".into());
    }
    let mut refreshed = 0;
    for r in &active {
        refreshed += try_refresh(db, http, r, true).await;
    }
    Ok(format!("{refreshed} Jellyfin library refreshes"))
}

struct Flight(i64);

impl Drop for Flight {
    fn drop(&mut self) {
        if let Ok(mut f) = IN_FLIGHT.lock() {
            f.remove(&self.0);
        }
    }
}

/// `TryRefresh`: refreshes while work is pending; returns the number of refresh requests sent.
async fn try_refresh(
    db: &Database,
    http: &reqwest::Client,
    r: &RefreshSettings,
    process_tick: bool,
) -> usize {
    if !IN_FLIGHT
        .lock()
        .map(|mut f| f.insert(r.id))
        .unwrap_or(false)
    {
        // A running loop sees the new generation.
        return 0;
    }
    let _flight = Flight(r.id);
    let mut sent = 0;
    for _ in 0..1000 {
        let (fingerprint, token, ack_immediate, ack_tick, immediate_generation, tick_generation) = {
            let _g = STATE.lock().await;
            let Ok(mut s) = state(db, r).await else {
                return sent;
            };
            let last_tick = DateTime::parse_from_rfc3339(&s.last_tick_at)
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let tick_due = process_tick
                && r.mode.interval()
                && Utc::now().signed_duration_since(last_tick).num_minutes() >= r.interval_minutes;
            let ack_immediate =
                r.mode.immediate() && s.imported_generation > s.immediate_generation;
            let ack_tick = tick_due && s.imported_generation > s.tick_generation;
            if tick_due && !ack_tick {
                // An idle tick: nothing to refresh.
                s.last_tick_at = now();
                let _ = save(db, r.id, &s).await;
            }
            if !ack_immediate && !ack_tick {
                return sent;
            }
            (
                s.target_fingerprint.clone(),
                s.state_token.clone(),
                ack_immediate,
                ack_tick,
                s.immediate_generation + 1,
                s.imported_generation,
            )
        };
        if let Err(e) = refresh_library(http, r).await {
            tracing::warn!(connection = r.id, error = %e, "Jellyfin library refresh failed; the imports stay pending");
            return sent;
        }
        sent += 1;
        let _g = STATE.lock().await;
        let Ok(mut s) = state(db, r).await else {
            return sent;
        };
        if s.target_fingerprint != fingerprint || s.state_token != token {
            // The target changed while refreshing: nothing is acknowledged.
            continue;
        }
        if ack_immediate {
            s.immediate_generation = s.immediate_generation.max(immediate_generation);
        }
        if ack_tick {
            s.tick_generation = s.tick_generation.max(tick_generation);
            s.last_tick_at = now();
        }
        if save(db, r.id, &s).await.is_err() {
            return sent;
        }
    }
    sent
}

/// Windows-looking paths compare without case, with `\` separators; others exactly. Trailing
/// separators are ignored.
fn same_path(location: &str, item: &str) -> bool {
    let windows = location.contains('\\') || location.as_bytes().get(1) == Some(&b':');
    if windows {
        location
            .replace('/', "\\")
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(item.replace('/', "\\").trim_end_matches('\\'))
    } else {
        location.trim_end_matches('/') == item.trim_end_matches('/')
    }
}

async fn send(
    r: &RefreshSettings,
    req: reqwest::RequestBuilder,
) -> Result<reqwest::Response, String> {
    let resp = req
        .header(
            "Authorization",
            format!("MediaBrowser Token=\"{}\"", r.api_key),
        )
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| "Jellyfin could not be reached".to_string())?;
    match resp.status().as_u16() {
        401 | 403 => Err("Jellyfin rejected the API key or denied administrator access".into()),
        s if !(200..300).contains(&s) => Err(format!("Jellyfin answered HTTP {s}")),
        _ => Ok(resp),
    }
}

/// The readiness check and the refresh of the selected library (never the server-wide
/// `/Library/Refresh`).
pub async fn refresh_library(http: &reqwest::Client, r: &RefreshSettings) -> Result<(), String> {
    let folders: Vec<Value> = send(r, http.get(format!("{}/Library/VirtualFolders", r.base)))
        .await?
        .json()
        .await
        .map_err(|_| "Jellyfin returned an invalid library list".to_string())?;
    let matching: Vec<&Value> = folders
        .iter()
        .filter(|f| {
            f["ItemId"]
                .as_str()
                .map(|i| i.replace('-', "").to_lowercase())
                .as_deref()
                == Some(r.library.as_str())
        })
        .collect();
    let [library] = matching.as_slice() else {
        return Err("The selected Jellyfin library no longer exists; select it again".into());
    };
    let mut pending: Vec<String> = library["Locations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.as_str().map(str::to_string))
        .collect();
    if pending.is_empty() {
        return Err("Initialize every folder of this Jellyfin library with a scan".into());
    }
    let mut start = 0usize;
    for _ in 0..1000 {
        let page: Value = send(
            r,
            http.get(format!("{}/Items", r.base)).query(&[
                ("parentId", r.library.as_str()),
                ("recursive", "true"),
                ("includeItemTypes", "Folder"),
                ("fields", "Path"),
                ("startIndex", &start.to_string()),
                ("limit", "200"),
                ("enableTotalRecordCount", "true"),
            ]),
        )
        .await?
        .json()
        .await
        .map_err(|_| "Jellyfin returned an invalid folder list".to_string())?;
        let items = page["Items"].as_array().cloned().unwrap_or_default();
        for i in &items {
            if let Some(p) = i["Path"].as_str() {
                pending.retain(|l| !same_path(l, p));
            }
        }
        start += items.len();
        let total = page["TotalRecordCount"].as_u64().unwrap_or(0) as usize;
        if pending.is_empty() || items.is_empty() || start >= total {
            break;
        }
    }
    if !pending.is_empty() {
        return Err(
            "Initialize every folder of this Jellyfin library with a scan; imports stay pending"
                .into(),
        );
    }
    let resp = send(
        r,
        http.post(format!("{}/Items/{}/Refresh", r.base, r.library))
            .query(&[
                ("metadataRefreshMode", "FullRefresh"),
                ("imageRefreshMode", "Default"),
                ("replaceAllMetadata", "false"),
                ("replaceAllImages", "false"),
            ]),
    )
    .await?;
    if resp.status().as_u16() != 204 {
        return Err(format!(
            "Jellyfin did not confirm the refresh (HTTP {})",
            resp.status().as_u16()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn paths_compare_like_tanukarr() {
        assert!(super::same_path("/media/anime/", "/media/anime"));
        assert!(!super::same_path("/media/Anime", "/media/anime"));
        assert!(super::same_path("D:\\Media\\Anime", "d:/media/anime/"));
    }
}
