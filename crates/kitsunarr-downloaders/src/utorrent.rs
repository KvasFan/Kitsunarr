//! µTorrent's Web UI (Sonarr's `UTorrent`, `UTorrentProxy`): a token from `token.html` with
//! its session cookie, the torrent list with its cache id (`cid`, only changes are sent
//! back), labels, the indexer's seed goals, queue position and the initial state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::error::cause;
use crate::{DownloadError, DownloadItem, ItemStatus, SeedConfig, Source};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub url_base: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub category: Option<String>,
    pub imported_category: Option<String>,
    /// `UTorrentPriority`: 0 last, 1 first.
    pub recent_priority: i64,
    pub older_priority: i64,
    /// `UTorrentState`: 0 start, 1 force start, 2 pause, 3 stop.
    pub initial_state: i64,
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl Settings {
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let host = text(map, "host").unwrap_or_else(|| "localhost".into());
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 8080,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        let int = |k: &str, max: i64| {
            map.get(k)
                .and_then(Value::as_i64)
                .filter(|v| (0..=max).contains(v))
                .unwrap_or(0)
        };
        Ok(Self {
            host,
            port,
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(false),
            url_base: text(map, "urlBase").unwrap_or_default(),
            username: text(map, "username"),
            password: text(map, "password"),
            category: text(map, "tvCategory"),
            imported_category: text(map, "tvImportedCategory"),
            recent_priority: int("recentTvPriority", 1),
            older_priority: int("olderTvPriority", 1),
            initial_state: int("intialState", 3).max(int("initialState", 3)),
        })
    }

    fn gui(&self, path: &str) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let base = self.url_base.trim_matches('/');
        let base = if base.is_empty() {
            String::new()
        } else {
            format!("/{base}")
        };
        format!("{scheme}://{}:{}{base}/gui/{path}", self.host, self.port)
    }

    fn key(&self) -> String {
        format!(
            "{}:{}",
            self.gui(""),
            self.password.clone().unwrap_or_default()
        )
    }
}

/// `UTorrentTorrentStatus` flags.
pub mod status {
    pub const STARTED: i64 = 1;
    pub const CHECKED: i64 = 8;
    pub const ERROR: i64 = 16;
    pub const PAUSED: i64 = 32;
    pub const QUEUED: i64 = 64;
    pub const LOADED: i64 = 128;
}

/// A torrent row of `list=1` (`UTorrentTorrent`, read by position).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Torrent {
    pub hash: String,
    pub status: i64,
    pub name: String,
    pub size: i64,
    /// 0 to 1.
    pub progress: f64,
    pub ratio: f64,
    pub eta: i64,
    pub label: String,
    pub remaining: i64,
    pub root_download_path: String,
}

impl Torrent {
    pub fn from_row(row: &[Value]) -> Option<Self> {
        let s = |i: usize| {
            row.get(i)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let n = |i: usize| row.get(i).and_then(Value::as_i64).unwrap_or(0);
        Some(Torrent {
            hash: row.first()?.as_str()?.to_string(),
            status: n(1),
            name: s(2),
            size: n(3),
            progress: n(4) as f64 / 1000.0,
            ratio: n(7) as f64 / 1000.0,
            eta: n(10),
            label: s(11),
            remaining: n(18),
            // Builds before 25406 stop at the remaining size.
            root_download_path: s(26),
        })
    }

    /// `GetItems`' mapping.
    pub fn to_item(&self) -> DownloadItem {
        let has = |flag: i64| self.status & flag != 0;
        let (item_status, message) = if has(status::ERROR) {
            (
                ItemStatus::Warning,
                Some("uTorrent is reporting an error".to_string()),
            )
        } else if has(status::LOADED)
            && has(status::CHECKED)
            && self.remaining == 0
            && self.progress == 1.0
        {
            (ItemStatus::Completed, None)
        } else if has(status::PAUSED) {
            (ItemStatus::Paused, None)
        } else if has(status::STARTED) {
            (ItemStatus::Downloading, None)
        } else {
            (ItemStatus::Queued, None)
        };
        let root = self.root_download_path.trim_end_matches(['/', '\\']);
        let output = if root.rsplit(['/', '\\']).next() == Some(self.name.as_str()) {
            root.to_string()
        } else {
            format!("{root}/{}", self.name)
        };
        DownloadItem {
            download_id: self.hash.to_ascii_uppercase(),
            title: self.name.clone(),
            category: self.label.clone(),
            total_size: self.size,
            remaining_size: self.remaining,
            remaining_seconds: (self.eta != -1).then_some(self.eta),
            status: item_status,
            message,
            output_path: Some(output),
            seed_ratio: self.ratio,
            // 'Started' without 'Queued' is forced seeding: not done.
            can_be_removed: !has(status::QUEUED) && !has(status::STARTED),
            seed_progress: None,
        }
    }
}

struct Session {
    token: String,
    cookie: String,
}

type ListCache = (Instant, String, Vec<Torrent>);

#[derive(Clone, Default)]
pub struct UTorrent {
    http: reqwest::Client,
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    lists: Arc<Mutex<HashMap<String, ListCache>>>,
}

impl UTorrent {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            ..Default::default()
        }
    }

    fn with_auth(&self, s: &Settings, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &s.username {
            Some(u) => req.basic_auth(u, s.password.as_deref()),
            None => req,
        }
    }

    /// `AuthenticateClient`: the token and cookie, cached per URL and password.
    async fn session(&self, s: &Settings, renew: bool) -> Result<Arc<Session>, DownloadError> {
        if !renew
            && let Some(x) = self
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&s.key())
        {
            return Ok(x.clone());
        }
        let resp = self
            .with_auth(s, self.http.get(s.gui("token.html")))
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        match resp.status().as_u16() {
            401 => return Err(DownloadError::Authentication),
            200..=299 => {}
            status => return Err(DownloadError::Http { status }),
        }
        let cookie = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok()?.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        let body = resp
            .text()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        // `<html><div id='token' style='display:none;'>TOKEN</div></html>`
        let token = body
            .split('>')
            .nth(2)
            .and_then(|t| t.split('<').next())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| DownloadError::Client("uTorrent sent no token".into()))?
            .to_string();
        let session = Arc::new(Session { token, cookie });
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(s.key(), session.clone());
        Ok(session)
    }

    /// `ProcessRequest`: logs in again on 400/401 once.
    async fn request(
        &self,
        s: &Settings,
        query: &[(&str, String)],
        file: Option<(&str, &[u8])>,
    ) -> Result<Value, DownloadError> {
        for attempt in 0..2 {
            let session = self.session(s, attempt > 0).await?;
            let mut params: Vec<(&str, String)> = vec![("token", session.token.clone())];
            params.extend(query.iter().cloned());
            let req = match file {
                Some((name, content)) => {
                    let part = reqwest::multipart::Part::bytes(content.to_vec())
                        .file_name(name.to_string())
                        .mime_str("application/x-bittorrent")
                        .map_err(|e| DownloadError::Client(e.to_string()))?;
                    self.http
                        .post(s.gui(""))
                        .multipart(reqwest::multipart::Form::new().part("torrent_file", part))
                }
                None => self.http.get(s.gui("")),
            };
            let resp = self
                .with_auth(s, req.query(&params))
                .header(reqwest::header::COOKIE, session.cookie.clone())
                .header("Cache-Control", "no-cache")
                .send()
                .await
                .map_err(|e| DownloadError::Connection(cause(&e)))?;
            match resp.status().as_u16() {
                400 | 401 if attempt == 0 => continue,
                401 => return Err(DownloadError::Authentication),
                200..=299 => {
                    let body = resp
                        .text()
                        .await
                        .map_err(|e| DownloadError::Connection(cause(&e)))?;
                    return Ok(serde_json::from_str(&body).unwrap_or(Value::Null));
                }
                status => return Err(DownloadError::Http { status }),
            }
        }
        Err(DownloadError::Authentication)
    }

    /// `GetVersion`: the build number.
    pub async fn build(&self, s: &Settings) -> Result<i64, DownloadError> {
        Ok(self
            .request(s, &[("action", "getsettings".into())], None)
            .await?["build"]
            .as_i64()
            .unwrap_or(0))
    }

    /// `GetTorrents` with the cache id: unchanged lists come back as changes only.
    pub async fn torrents(&self, s: &Settings) -> Result<Vec<Torrent>, DownloadError> {
        let key = format!(
            "{}:{}:{}",
            s.host,
            s.port,
            s.category.clone().unwrap_or_default()
        );
        let cached = self
            .lists
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .filter(|(at, _, _)| at.elapsed() < Duration::from_secs(900))
            .map(|(_, cid, list)| (cid.clone(), list.clone()));
        let mut query = vec![("list", "1".to_string())];
        if let Some((cid, _)) = &cached {
            query.push(("cid", cid.clone()));
        }
        let v = self.request(s, &query, None).await?;
        let rows = |k: &str| -> Vec<Torrent> {
            v[k].as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|r| Torrent::from_row(r.as_array()?))
                        .collect()
                })
                .unwrap_or_default()
        };
        let torrents = match (&cached, v.get("torrents")) {
            (Some((_, list)), None) => {
                let changed = rows("torrentp");
                let removed: Vec<String> = v["torrentm"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|h| h.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut kept: Vec<Torrent> = list
                    .iter()
                    .filter(|t| {
                        !removed.contains(&t.hash) && !changed.iter().any(|c| c.hash == t.hash)
                    })
                    .cloned()
                    .collect();
                kept.extend(changed);
                kept
            }
            _ => rows("torrents"),
        };
        let cid = match &v["torrentc"] {
            Value::String(c) => c.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        self.lists
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (Instant::now(), cid, torrents.clone()));
        Ok(torrents)
    }

    /// `GetItems`: the category's torrents.
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        Ok(self
            .torrents(s)
            .await?
            .iter()
            .filter(|t| t.label == s.category.clone().unwrap_or_default())
            .map(Torrent::to_item)
            .collect())
    }

    async fn set_label(&self, s: &Settings, hash: &str, label: &str) -> Result<(), DownloadError> {
        self.request(
            s,
            &[
                ("action", "setprops".into()),
                ("hash", hash.into()),
                ("s", "label".into()),
                ("v", label.into()),
            ],
            None,
        )
        .await
        .map(|_| ())
    }

    /// `AddFromMagnetLink`/`AddFromTorrentFile`: then the seed goals, label, queue position
    /// and initial state.
    pub async fn add(
        &self,
        s: &Settings,
        source: &Source<'_>,
        hash: &str,
        recent: bool,
        seed: Option<SeedConfig>,
    ) -> Result<String, DownloadError> {
        match source {
            Source::Magnet(url) => {
                self.request(
                    s,
                    &[("action", "add-url".into()), ("s", url.to_string())],
                    None,
                )
                .await?;
            }
            Source::File { name, content } => {
                self.request(
                    s,
                    &[("action", "add-file".into()), ("path", String::new())],
                    Some((name, content)),
                )
                .await?;
            }
        }
        let hash = hash.to_ascii_uppercase();
        if let Some(seed) = seed.filter(SeedConfig::is_set) {
            let mut q = vec![
                ("action", "setprops".to_string()),
                ("hash", hash.clone()),
                ("s", "seed_override".into()),
                ("v", "1".into()),
            ];
            if let Some(r) = seed.ratio {
                q.push(("s", "seed_ratio".into()));
                q.push(("v", ((r * 1000.0) as i64).to_string()));
            }
            if let Some(m) = seed.seed_time_minutes {
                q.push(("s", "seed_time".into()));
                q.push(("v", (m * 60).to_string()));
            }
            self.request(s, &q, None).await?;
        }
        if let Some(label) = &s.category {
            self.set_label(s, &hash, label).await?;
        }
        let priority = if recent {
            s.recent_priority
        } else {
            s.older_priority
        };
        if priority == 1 {
            self.request(
                s,
                &[("action", "queuetop".into()), ("hash", hash.clone())],
                None,
            )
            .await?;
        }
        let state = match s.initial_state {
            1 => "forcestart",
            2 => "pause",
            3 => "stop",
            _ => "start",
        };
        self.request(s, &[("action", state.into()), ("hash", hash.clone())], None)
            .await?;
        Ok(hash)
    }

    pub async fn remove(
        &self,
        s: &Settings,
        hash: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        let action = if delete_data { "removedata" } else { "remove" };
        self.request(s, &[("action", action.into()), ("hash", hash.into())], None)
            .await
            .map(|_| ())
    }

    /// `MarkItemAsImported`: the post-import label, then the old one removed explicitly.
    pub async fn mark_imported(&self, s: &Settings, hash: &str) -> Result<(), DownloadError> {
        let Some(imported) = s
            .imported_category
            .as_deref()
            .filter(|c| s.category.as_deref() != Some(*c))
        else {
            return Ok(());
        };
        let hash = hash.to_ascii_lowercase();
        self.set_label(s, &hash, imported).await?;
        if let Some(old) = &s.category {
            self.request(
                s,
                &[
                    ("action", "setprops".into()),
                    ("hash", hash.clone()),
                    ("s", "label".into()),
                    ("v", old.clone()),
                    ("s", "label".into()),
                    ("v", String::new()),
                ],
                None,
            )
            .await?;
        }
        Ok(())
    }

    /// `Test`: build 25406 (3.0) or newer, and the torrent list.
    pub async fn test(&self, s: &Settings) -> Result<i64, DownloadError> {
        let build = self.build(s).await?;
        if build < 25406 {
            return Err(DownloadError::Unsupported(format!(
                "uTorrent 3.0 or newer is required; it reports build {build}"
            )));
        }
        // `TestGetTorrents` reads the full list, outside the cache.
        self.request(s, &[("list", "1".into())], None).await?;
        Ok(build)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(status: i64, progress: i64, remaining: i64, path: &str) -> Vec<Value> {
        let mut r = vec![
            json!("HASH"),
            json!(status),
            json!("Droned.S01E01.Pilot.1080p.WEB-DL-DRONE"),
            json!(1000),
            json!(progress),
            json!(0),
            json!(0),
            json!(1500),
            json!(0),
            json!(0),
            json!(-1),
            json!("tv-kitsunarr"),
        ];
        r.extend((12..18).map(|_| json!(0)));
        r.push(json!(remaining));
        r.extend((19..26).map(|_| json!("")));
        r.push(json!(path));
        r
    }

    #[test]
    fn torrents_map_as_upstream() {
        let t = |status, progress, remaining| {
            Torrent::from_row(&row(status, progress, remaining, "/downloads")).unwrap()
        };
        let done = t(status::LOADED | status::CHECKED, 1000, 0).to_item();
        assert_eq!(done.status, ItemStatus::Completed);
        assert!(done.can_be_removed);
        assert_eq!(
            done.output_path.as_deref(),
            Some("/downloads/Droned.S01E01.Pilot.1080p.WEB-DL-DRONE")
        );
        assert_eq!(done.seed_ratio, 1.5);
        assert_eq!(done.remaining_seconds, None);
        let seeding = t(
            status::LOADED | status::CHECKED | status::STARTED | status::QUEUED,
            1000,
            0,
        )
        .to_item();
        assert_eq!(seeding.status, ItemStatus::Completed);
        assert!(!seeding.can_be_removed);
        assert_eq!(
            t(status::PAUSED | status::STARTED, 500, 5).to_item().status,
            ItemStatus::Paused
        );
        assert_eq!(
            t(status::STARTED, 500, 5).to_item().status,
            ItemStatus::Downloading
        );
        assert_eq!(t(0, 0, 5).to_item().status, ItemStatus::Queued);
        assert_eq!(t(status::ERROR, 0, 5).to_item().status, ItemStatus::Warning);
        // A multi-file torrent's root path is its folder.
        let folder = Torrent::from_row(&row(
            status::LOADED | status::CHECKED,
            1000,
            0,
            "/downloads/Droned.S01E01.Pilot.1080p.WEB-DL-DRONE",
        ))
        .unwrap()
        .to_item();
        assert_eq!(
            folder.output_path.as_deref(),
            Some("/downloads/Droned.S01E01.Pilot.1080p.WEB-DL-DRONE")
        );
    }
}
