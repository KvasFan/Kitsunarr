//! Transmission through its RPC API (Sonarr's `Transmission.cs`, `TransmissionBase.cs` and
//! `TransmissionProxy.cs`).
//!
//! Requests go to `POST {base}/rpc` as `{"method", "arguments"}` with HTTP basic
//! authentication. Transmission answers 409 with an `X-Transmission-Session-Id` header until the
//! request carries it; the id is cached per address. Labels need Transmission 4; with older
//! versions the TV category is a subfolder of the download directory, as upstream.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use reqwest::{StatusCode, header};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::error::DownloadError;
use crate::qbittorrent::Source;
use crate::{DownloadItem, ItemStatus};

const SESSION_HEADER: &str = "X-Transmission-Session-Id";

/// Settings, as stored with Sonarr's field names.
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
    /// `TvDirectory`: an explicit download directory.
    pub directory: Option<String>,
    /// 0 last, 1 first.
    pub recent_priority: i64,
    pub older_priority: i64,
    pub add_paused: bool,
    /// Vuze (`Vuze : TransmissionBase`): Transmission's RPC, protocol version 14 or newer,
    /// no labels, and uTorrent's output paths.
    pub vuze: bool,
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl Settings {
    /// From a provider's `settings` map (Sonarr's defaults for missing fields).
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let host = text(map, "host").unwrap_or_else(|| "localhost".into());
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 9091,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        let category = text(map, "tvCategory");
        let directory = text(map, "tvDirectory");
        if let Some(c) = &category
            && !c
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
        {
            return Err(format!(
                "the category {c:?} may only contain letters, digits, '-', '_' and '.'"
            ));
        }
        Ok(Self {
            host,
            port,
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(false),
            // Sonarr's default when the field is absent; an empty value means none.
            url_base: match map.get("urlBase") {
                None | Some(Value::Null) => "/transmission/".into(),
                Some(v) => v.as_str().unwrap_or_default().trim().to_string(),
            },
            username: text(map, "username"),
            password: text(map, "password"),
            category,
            imported_category: text(map, "tvImportedCategory"),
            directory,
            recent_priority: map
                .get("recentTvPriority")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            older_priority: map
                .get("olderTvPriority")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            add_paused: map
                .get("addPaused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            vuze: false,
        })
    }

    /// `BuildBaseUrl(useSsl, host, port, urlBase)` + `rpc`.
    pub fn rpc_url(&self) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let base = self.url_base.trim_matches('/');
        if base.is_empty() {
            format!("{scheme}://{host}:{}/rpc", self.port)
        } else {
            format!("{scheme}://{host}:{}/{base}/rpc", self.port)
        }
    }

    fn session_key(&self) -> String {
        format!(
            "{}:{}",
            self.rpc_url(),
            self.password.as_deref().unwrap_or_default()
        )
    }
}

/// `TransmissionTorrentStatus`.
pub mod status {
    pub const STOPPED: i64 = 0;
    pub const CHECK_WAIT: i64 = 1;
    pub const CHECK: i64 = 2;
    pub const QUEUED: i64 = 3;
    pub const SEEDING_WAIT: i64 = 5;
    pub const SEEDING: i64 = 6;
}

/// A torrent from `torrent-get` (`TransmissionTorrent`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Torrent {
    pub hash_string: String,
    pub name: String,
    pub download_dir: String,
    pub total_size: i64,
    pub left_until_done: i64,
    pub is_finished: bool,
    pub eta: i64,
    pub status: i64,
    pub error_string: String,
    pub uploaded_ever: i64,
    pub downloaded_ever: i64,
    pub labels: Vec<String>,
    pub seed_ratio_mode: i64,
    pub seed_ratio_limit: f64,
    pub seed_idle_mode: i64,
    pub seed_idle_limit: i64,
    pub seconds_seeding: i64,
    #[serde(alias = "file-count")]
    pub file_count: i64,
}

/// The session's seeding limits, for `HasReachedSeedLimit`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeedConfig {
    pub ratio_limited: bool,
    pub ratio_limit: f64,
    pub idle_limit_enabled: bool,
}

/// `torrent-get`'s fields (`GetTorrentStatus`).
const FIELDS: [&str; 21] = [
    "id",
    "hashString",
    "name",
    "downloadDir",
    "totalSize",
    "leftUntilDone",
    "isFinished",
    "eta",
    "status",
    "secondsDownloading",
    "secondsSeeding",
    "errorString",
    "uploadedEver",
    "downloadedEver",
    "seedRatioLimit",
    "seedRatioMode",
    "seedIdleLimit",
    "seedIdleMode",
    "fileCount",
    "file-count",
    "labels",
];

impl Torrent {
    /// `HasReachedSeedLimit`.
    pub fn has_reached_seed_limit(&self, ratio: f64, config: &SeedConfig) -> bool {
        let stopped = self.status == status::STOPPED;
        let seeding = self.status == status::SEEDING;
        match self.seed_ratio_mode {
            1 if stopped && ratio >= self.seed_ratio_limit => return true,
            0 if stopped && config.ratio_limited && ratio >= config.ratio_limit => return true,
            _ => {}
        }
        match self.seed_idle_mode {
            1 => (stopped || seeding) && self.seconds_seeding > self.seed_idle_limit * 60,
            // The global idle limit is a real idle limit: stopped is enough.
            0 => stopped && config.idle_limit_enabled,
            _ => false,
        }
    }

    /// `GetItems`' mapping of one torrent.
    pub fn to_item(&self, category: Option<&str>) -> DownloadItem {
        self.to_item_for(category, false)
    }

    /// `GetItems`' mapping; Vuze's `GetOutputPath` keeps a job folder (a multi-file torrent,
    /// or a directory already named for it) and appends the name only for single files.
    pub fn to_item_for(&self, category: Option<&str>, vuze: bool) -> DownloadItem {
        let (item_status, message) = if !self.error_string.trim().is_empty() {
            (
                ItemStatus::Warning,
                Some(format!(
                    "Transmission is reporting an error: {}",
                    self.error_string
                )),
            )
        } else if self.total_size == 0 {
            (ItemStatus::Queued, None)
        } else if (self.left_until_done == 0
            && matches!(
                self.status,
                status::STOPPED | status::SEEDING | status::SEEDING_WAIT
            ))
            || (self.is_finished && !matches!(self.status, status::CHECK | status::CHECK_WAIT))
        {
            (ItemStatus::Completed, None)
        } else if self.status == status::QUEUED {
            (ItemStatus::Queued, None)
        } else {
            (ItemStatus::Downloading, None)
        };
        let dir = self.download_dir.trim_end_matches(['/', '\\']);
        DownloadItem {
            download_id: self.hash_string.to_ascii_uppercase(),
            title: self.name.clone(),
            category: category.unwrap_or_default().to_string(),
            total_size: self.total_size,
            remaining_size: self.left_until_done,
            remaining_seconds: (self.eta >= 0).then_some(self.eta),
            status: item_status,
            message,
            // `GetOutputPath`: the directory plus the name, with ':' replaced.
            output_path: Some(
                if vuze
                    && (dir.rsplit(['/', '\\']).next() == Some(self.name.as_str())
                        || self.file_count > 1)
                {
                    dir.to_string()
                } else {
                    format!("{dir}/{}", self.name.replace(':', "_"))
                },
            ),
            seed_ratio: if self.downloaded_ever <= 0 {
                0.0
            } else {
                self.uploaded_ever as f64 / self.downloaded_ever as f64
            },
            can_be_removed: false,
            seed_progress: None,
        }
    }
}

/// What a test found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub version: String,
    pub warnings: Vec<String>,
}

/// The first dotted version number in `"4.0.5 (a6fe2a64aa)"`.
fn parse_version(v: &str) -> Vec<u64> {
    v.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|p| p.chars().any(|c| c.is_ascii_digit()))
        .unwrap_or_default()
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

fn at_least(v: &[u64], min: &[u64]) -> bool {
    for i in 0..v.len().max(min.len()) {
        let (a, b) = (
            v.get(i).copied().unwrap_or(0),
            min.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return a > b;
        }
    }
    true
}

/// A Transmission RPC client; session ids are cached per address and password.
#[derive(Clone, Default)]
pub struct Transmission {
    http: reqwest::Client,
    sessions: Arc<Mutex<HashMap<String, String>>>,
}

impl Transmission {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            ..Default::default()
        }
    }

    async fn send(
        &self,
        s: &Settings,
        session: Option<&str>,
        body: &Value,
    ) -> Result<reqwest::Response, DownloadError> {
        let mut req = self.http.post(s.rpc_url()).json(body);
        if let Some(user) = &s.username {
            req = req.basic_auth(user, s.password.as_deref());
        }
        if let Some(id) = session {
            req = req.header(SESSION_HEADER, id);
        }
        req.send()
            .await
            .map_err(|e| DownloadError::Connection(crate::error::cause(&e)))
    }

    /// `ProcessRequest`, with `AuthenticateClient`'s session handshake.
    async fn call(
        &self,
        s: &Settings,
        method: &str,
        arguments: Option<Value>,
    ) -> Result<Value, DownloadError> {
        let mut body = json!({ "method": method });
        if let Some(a) = arguments {
            body["arguments"] = a;
        }
        let cached = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&s.session_key())
            .cloned();
        let mut resp = self.send(s, cached.as_deref(), &body).await?;
        if resp.status() == StatusCode::CONFLICT {
            let id = resp
                .headers()
                .get(SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .ok_or_else(|| {
                    DownloadError::Client("Transmission did not return a session id".into())
                })?;
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(s.session_key(), id.clone());
            resp = self.send(s, Some(&id), &body).await?;
        }
        match resp.status() {
            StatusCode::UNAUTHORIZED => return Err(DownloadError::Authentication),
            StatusCode::FORBIDDEN => {
                return Err(DownloadError::Client(
                    "Transmission refused the connection; it may be necessary to add Kitsunarr's IP address to its RPC whitelist".into(),
                ));
            }
            s if s.is_redirection() => {
                let to = resp
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                return Err(DownloadError::Client(format!(
                    "Transmission redirected to {to}; check the URL base"
                )));
            }
            s if !s.is_success() => return Err(DownloadError::Http { status: s.as_u16() }),
            _ => {}
        }
        let value: Value = resp.json().await.map_err(|e| {
            DownloadError::Client(format!("Transmission sent an unreadable answer ({e})"))
        })?;
        match value.get("result").and_then(Value::as_str) {
            Some("success") => Ok(value.get("arguments").cloned().unwrap_or(Value::Null)),
            Some(other) => Err(DownloadError::Client(format!("Transmission: {other}"))),
            None => Err(DownloadError::Client(
                "Transmission sent an unexpected answer".into(),
            )),
        }
    }

    /// `session-get`.
    async fn config(&self, s: &Settings) -> Result<Value, DownloadError> {
        self.call(s, "session-get", None).await
    }

    /// `GetClientVersion`.
    pub async fn version(&self, s: &Settings) -> Result<String, DownloadError> {
        Ok(self
            .config(s)
            .await?
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    /// `SupportsLabels`: Transmission 4.0 or newer (never Vuze).
    async fn supports_labels(&self, s: &Settings) -> Result<bool, DownloadError> {
        if s.vuze {
            return Ok(false);
        }
        Ok(at_least(&parse_version(&self.version(s).await?), &[4, 0]))
    }

    /// `GetDownloadDirectory`: the TV directory, else (without labels) the category subfolder
    /// of Transmission's download directory.
    async fn download_directory(
        &self,
        s: &Settings,
        labels: bool,
    ) -> Result<Option<String>, DownloadError> {
        if let Some(d) = &s.directory {
            return Ok(Some(d.clone()));
        }
        let Some(category) = &s.category else {
            return Ok(None);
        };
        if labels {
            return Ok(None);
        }
        let config = self.config(s).await?;
        let dir = config
            .get("download-dir")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(Some(format!("{}/{category}", dir.trim_end_matches('/'))))
    }

    pub async fn torrents(
        &self,
        s: &Settings,
        hashes: Option<&[String]>,
    ) -> Result<Vec<Torrent>, DownloadError> {
        let mut args = json!({ "fields": FIELDS });
        if let Some(h) = hashes {
            args["ids"] = json!(h);
        }
        let result = self.call(s, "torrent-get", Some(args)).await?;
        Ok(result
            .get("torrents")
            .and_then(|t| serde_json::from_value(t.clone()).ok())
            .unwrap_or_default())
    }

    /// `GetItems`: the torrents with Kitsunarr's label (or, before Transmission 4, in its
    /// category folder) and inside the download directory.
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let labels = self.supports_labels(s).await?;
        let directory = self.download_directory(s, labels).await?;
        let torrents = self.torrents(s, None).await?;
        let config = self.config(s).await?;
        let seed = SeedConfig {
            ratio_limited: config
                .get("seedRatioLimited")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ratio_limit: config
                .get("seedRatioLimit")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            idle_limit_enabled: config
                .get("idle-seeding-limit-enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        Ok(torrents
            .iter()
            .filter(|t| {
                let Some(category) = s.category.as_deref().filter(|_| labels) else {
                    return true;
                };
                if !t.labels.is_empty() {
                    t.labels.iter().any(|l| l.eq_ignore_ascii_case(category))
                } else {
                    // Torrents from before Transmission 4 are matched by their folder.
                    t.download_dir.split(['/', '\\']).any(|p| p == category)
                }
            })
            .filter(|t| {
                directory.as_deref().is_none_or(|d| {
                    let d = d.trim_end_matches(['/', '\\']);
                    let dir = t.download_dir.trim_end_matches(['/', '\\']);
                    dir == d
                        || dir.starts_with(&format!("{d}/"))
                        || dir.starts_with(&format!("{d}\\"))
                })
            })
            .map(|t| {
                let mut item = t.to_item_for(s.category.as_deref(), s.vuze);
                item.can_be_removed = t.has_reached_seed_limit(item.seed_ratio, &seed);
                item
            })
            .collect())
    }

    /// `MarkItemAsImported` (Transmission 4): swaps the TV label for the post-import one.
    /// `RemoveItem`: `torrent-remove`, with the local data when asked.
    pub async fn remove(
        &self,
        s: &Settings,
        hash: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        // Vuze takes the id as listed; Transmission the lower-case hash.
        let id = if s.vuze {
            hash.to_string()
        } else {
            hash.to_ascii_lowercase()
        };
        self.call(
            s,
            "torrent-remove",
            Some(json!({ "ids": [id], "delete-local-data": delete_data })),
        )
        .await
        .map(|_| ())
    }

    pub async fn mark_imported(&self, s: &Settings, hash: &str) -> Result<(), DownloadError> {
        let Some(imported) = s
            .imported_category
            .as_ref()
            .filter(|c| s.category.as_ref() != Some(*c))
        else {
            return Ok(());
        };
        if !self.supports_labels(s).await? {
            return Ok(());
        }
        let hash = hash.to_ascii_lowercase();
        let Some(torrent) = self
            .torrents(s, Some(std::slice::from_ref(&hash)))
            .await?
            .into_iter()
            .next()
        else {
            tracing::warn!(%hash, "could not find the torrent in Transmission to set its post-import label");
            return Ok(());
        };
        let mut labels: Vec<String> = torrent
            .labels
            .into_iter()
            .filter(|l| {
                s.category
                    .as_ref()
                    .is_none_or(|c| !l.eq_ignore_ascii_case(c))
            })
            .collect();
        if !labels.iter().any(|l| l.eq_ignore_ascii_case(imported)) {
            labels.push(imported.clone());
        }
        if let Err(e) = self
            .call(
                s,
                "torrent-set",
                Some(json!({ "ids": [hash], "labels": labels })),
            )
            .await
        {
            tracing::warn!(error = %e, "failed to set the post-import label in Transmission");
        }
        Ok(())
    }

    /// `AddFromMagnetLink` / `AddFromTorrentFile`: adds with the label (Transmission 4) or into
    /// the category folder, then moves recent (or older) episodes to the top of the queue when
    /// their priority is "first". Returns the expected info hash, upper-case.
    pub async fn add(
        &self,
        s: &Settings,
        source: &Source<'_>,
        hash: &str,
        recent: bool,
        seed: Option<crate::qbittorrent::SeedConfig>,
    ) -> Result<String, DownloadError> {
        let labels = self.supports_labels(s).await?;
        let mut args = match source {
            Source::Magnet(url) => json!({ "filename": url }),
            Source::File { content, .. } => {
                json!({ "metainfo": base64::engine::general_purpose::STANDARD.encode(content) })
            }
        };
        args["paused"] = json!(s.add_paused);
        if let Some(dir) = self.download_directory(s, labels).await? {
            args["download-dir"] = json!(dir);
        }
        if let Some(category) = &s.category {
            args["labels"] = json!([category]);
        }
        self.call(s, "torrent-add", Some(args)).await?;
        let hash = hash.to_ascii_lowercase();
        // `SetTorrentSeedingConfiguration`: the ratio, and the seed time as the idle limit.
        if let Some(c) = seed.filter(crate::qbittorrent::SeedConfig::is_set) {
            let mut set = json!({ "ids": [hash] });
            if let Some(r) = c.ratio {
                set["seedRatioLimit"] = json!(r);
                set["seedRatioMode"] = json!(1);
            }
            if let Some(t) = c.seed_time_minutes {
                set["seedIdleLimit"] = json!(t);
                set["seedIdleMode"] = json!(1);
            }
            self.call(s, "torrent-set", Some(set)).await?;
        }
        let priority = if recent {
            s.recent_priority
        } else {
            s.older_priority
        };
        if priority == 1
            && let Err(e) = self
                .call(s, "queue-move-top", Some(json!({ "ids": [hash] })))
                .await
        {
            tracing::warn!(error = %e, %hash, "failed to move the torrent to the top of the queue");
        }
        Ok(hash.to_ascii_uppercase())
    }

    /// `Test`: version (2.40 or newer), category and directory together only with labels, and
    /// listing torrents.
    pub async fn test(&self, s: &Settings) -> Result<TestReport, DownloadError> {
        if s.vuze {
            // `Vuze.ValidateVersion`: the RPC protocol version, 14 or newer.
            let config = self.config(s).await?;
            let protocol = config
                .get("rpc-version")
                .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(0);
            if protocol < 14 {
                return Err(DownloadError::Unsupported(format!(
                    "Vuze's RPC protocol version 14 or newer is required; it reports {protocol}"
                )));
            }
            self.torrents(s, None).await?;
            return Ok(TestReport {
                version: format!("(protocol {protocol})"),
                warnings: Vec::new(),
            });
        }
        let version = self.version(s).await?;
        let v = parse_version(&version);
        if !at_least(&v, &[2, 40]) {
            return Err(DownloadError::Unsupported(format!(
                "Transmission 2.40 or newer is required; it reports {version}"
            )));
        }
        let labels = at_least(&v, &[4, 0]);
        if !labels && s.category.is_some() && s.directory.is_some() {
            return Err(DownloadError::Client(
                "a category and a directory cannot be used together before Transmission 4".into(),
            ));
        }
        let mut warnings = Vec::new();
        if s.category.is_none() {
            warnings.push(
                "A category is recommended: it keeps Kitsunarr's downloads apart from everything else in Transmission".into(),
            );
        }
        self.torrents(s, None).await?;
        Ok(TestReport { version, warnings })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn seed_limits_follow_sonarr() {
        let t = |status: i64, ratio_mode: i64, idle_mode: i64| Torrent {
            status,
            seed_ratio_mode: ratio_mode,
            seed_ratio_limit: 2.0,
            seed_idle_mode: idle_mode,
            seed_idle_limit: 10,
            seconds_seeding: 601,
            ..Default::default()
        };
        let global = SeedConfig {
            ratio_limited: true,
            ratio_limit: 1.0,
            idle_limit_enabled: false,
        };
        // Per-torrent ratio, once stopped.
        assert!(t(status::STOPPED, 1, 2).has_reached_seed_limit(2.0, &global));
        assert!(!t(status::SEEDING, 1, 2).has_reached_seed_limit(2.0, &global));
        assert!(!t(status::STOPPED, 1, 2).has_reached_seed_limit(1.5, &global));
        // The global ratio.
        assert!(t(status::STOPPED, 0, 2).has_reached_seed_limit(1.0, &global));
        // Per-torrent idle limit: seeding long enough.
        assert!(t(status::SEEDING, 2, 1).has_reached_seed_limit(0.0, &global));
        // The global idle limit: stopped is enough.
        let idle = SeedConfig {
            idle_limit_enabled: true,
            ..Default::default()
        };
        assert!(t(status::STOPPED, 2, 0).has_reached_seed_limit(0.0, &idle));
        assert!(!t(status::SEEDING, 2, 0).has_reached_seed_limit(0.0, &idle));
    }

    use super::*;

    #[test]
    fn vuze_output_paths_follow_vuze_fixture() {
        // `should_have_correct_output_directory_for_multifile_torrents`: the directory is the
        // job folder.
        let multi = Torrent {
            name: "Droned.S01E01.Pilot.1080p.WEB-DL-DRONE".into(),
            download_dir: "/downloads/Droned.S01E01.Pilot.1080p.WEB-DL-DRONE".into(),
            ..Default::default()
        };
        assert_eq!(
            multi.to_item_for(None, true).output_path.as_deref(),
            Some("/downloads/Droned.S01E01.Pilot.1080p.WEB-DL-DRONE")
        );
        let several = Torrent {
            name: "Pack".into(),
            download_dir: "/downloads/elsewhere".into(),
            file_count: 3,
            ..Default::default()
        };
        assert_eq!(
            several.to_item_for(None, true).output_path.as_deref(),
            Some("/downloads/elsewhere")
        );
        // `…_for_singlefile_torrents`: the file in the root folder.
        let single = Torrent {
            name: "Droned.S01E01.mkv".into(),
            download_dir: "/downloads".into(),
            file_count: 1,
            ..Default::default()
        };
        assert_eq!(
            single.to_item_for(None, true).output_path.as_deref(),
            Some("/downloads/Droned.S01E01.mkv")
        );
        // Transmission always appends the name.
        assert_eq!(
            multi.to_item(None).output_path.as_deref(),
            Some(
                "/downloads/Droned.S01E01.Pilot.1080p.WEB-DL-DRONE/Droned.S01E01.Pilot.1080p.WEB-DL-DRONE"
            )
        );
        let map = serde_json::Map::new();
        let s = crate::ClientSettings::from_provider("Vuze", &map).unwrap();
        assert!(matches!(s, crate::ClientSettings::Transmission(t) if t.vuze));
    }

    #[test]
    fn versions_and_states() {
        assert_eq!(parse_version("4.0.5 (a6fe2a64aa)"), [4, 0, 5]);
        assert_eq!(parse_version("2.94 (d8e60ee44f)"), [2, 94]);
        assert!(at_least(&parse_version("2.94"), &[2, 40]));
        assert!(!at_least(&parse_version("2.33"), &[2, 40]));
        let t = |status, left, finished, error: &str, total| Torrent {
            hash_string: "ab".into(),
            name: "Show: S01E01".into(),
            download_dir: "/downloads/tv/".into(),
            total_size: total,
            left_until_done: left,
            is_finished: finished,
            eta: -1,
            status,
            error_string: error.into(),
            ..Default::default()
        };
        let item = t(status::SEEDING, 0, false, "", 10).to_item(Some("tv"));
        assert_eq!(item.status, ItemStatus::Completed);
        assert_eq!(
            item.output_path.as_deref(),
            Some("/downloads/tv/Show_ S01E01")
        );
        assert_eq!(item.remaining_seconds, None);
        assert_eq!(
            t(4, 5, false, "", 10).to_item(None).status,
            ItemStatus::Downloading
        );
        assert_eq!(
            t(status::CHECK, 0, true, "", 10).to_item(None).status,
            ItemStatus::Downloading
        );
        assert_eq!(
            t(status::QUEUED, 5, false, "", 10).to_item(None).status,
            ItemStatus::Queued
        );
        assert_eq!(
            t(4, 5, false, "", 0).to_item(None).status,
            ItemStatus::Queued
        );
        let warn = t(4, 5, false, "tracker gone", 10).to_item(None);
        assert_eq!(warn.status, ItemStatus::Warning);
        assert_eq!(
            warn.message.as_deref(),
            Some("Transmission is reporting an error: tracker gone")
        );
    }
}
