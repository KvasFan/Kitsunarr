//! Deluge through its Web UI's JSON-RPC API (Sonarr's `Deluge.cs` and `DelugeProxy.cs`).
//!
//! Requests go to `POST {base}/json` as `{"method", "params", "id"}`. The Web UI logs in with
//! its password (a `_session_id` cookie, cached per address) and must be connected to a daemon:
//! when it is not, the local daemon (`127.0.0.1`) is connected, as upstream does.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use reqwest::{StatusCode, header};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::error::DownloadError;
use crate::{DownloadItem, ItemStatus};

/// Settings, as stored with Sonarr's field names.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub url_base: String,
    pub password: String,
    /// The label of Kitsunarr's downloads (`tvCategory`).
    pub category: Option<String>,
    pub imported_category: Option<String>,
    /// 0 last, 1 first.
    pub recent_priority: i64,
    pub older_priority: i64,
    pub add_paused: bool,
    pub download_directory: Option<String>,
    pub completed_directory: Option<String>,
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn label_ok(label: &str) -> bool {
    label
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl Settings {
    /// From a provider's `settings` map (Sonarr's defaults for missing fields).
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let host = text(map, "host").unwrap_or_else(|| "localhost".into());
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 8112,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        let category = text(map, "tvCategory");
        let imported_category = text(map, "tvImportedCategory");
        for label in [&category, &imported_category].into_iter().flatten() {
            if !label_ok(label) {
                return Err(format!(
                    "the label {label:?} may only contain a-z, 0-9 and -"
                ));
            }
        }
        Ok(Self {
            host,
            port,
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(false),
            url_base: text(map, "urlBase").unwrap_or_default(),
            password: map
                .get("password")
                .and_then(Value::as_str)
                .unwrap_or("deluge")
                .to_string(),
            category,
            imported_category,
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
            download_directory: text(map, "downloadDirectory"),
            completed_directory: text(map, "completedDirectory"),
        })
    }

    /// `HttpRequestBuilder.BuildBaseUrl(useSsl, host, port, urlBase)` + `json`.
    pub fn rpc_url(&self) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let base = self.url_base.trim_matches('/');
        if base.is_empty() {
            format!("{scheme}://{host}:{}/json", self.port)
        } else {
            format!("{scheme}://{host}:{}/{base}/json", self.port)
        }
    }

    fn cookie_key(&self) -> String {
        format!("{}:{}", self.rpc_url(), self.password)
    }

    /// Options for `core.add_torrent_*`.
    fn add_options(&self) -> Value {
        let mut o = json!({ "add_paused": self.add_paused, "remove_at_ratio": false });
        if let Some(d) = &self.download_directory {
            o["download_location"] = json!(d);
        }
        if let Some(d) = &self.completed_directory {
            o["move_completed_path"] = json!(d);
            o["move_completed"] = json!(true);
        }
        o
    }
}

/// A torrent from `web.update_ui` (`DelugeTorrent`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Torrent {
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub eta: f64,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub is_finished: bool,
    #[serde(default)]
    pub save_path: String,
    #[serde(default)]
    pub total_size: i64,
    #[serde(default)]
    pub total_done: i64,
    #[serde(default)]
    pub ratio: f64,
    #[serde(default)]
    pub is_auto_managed: bool,
    #[serde(default)]
    pub stop_at_ratio: bool,
    #[serde(default)]
    pub stop_ratio: f64,
}

/// `web.update_ui`'s properties (`RequiredProperties`).
const PROPERTIES: [&str; 17] = [
    "hash",
    "name",
    "state",
    "progress",
    "eta",
    "message",
    "is_finished",
    "save_path",
    "total_size",
    "total_done",
    "time_added",
    "active_time",
    "ratio",
    "is_auto_managed",
    "stop_at_ratio",
    "remove_at_ratio",
    "stop_ratio",
];

/// `Path.Combine`-like join with the separator the client uses.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    let sep = if dir.contains('\\') && !dir.contains('/') {
        '\\'
    } else {
        '/'
    };
    format!("{}{sep}{name}", dir.trim_end_matches(['/', '\\']))
}

impl Torrent {
    /// `GetItems`' mapping of one torrent.
    pub fn to_item(&self, category: Option<&str>) -> Option<DownloadItem> {
        let hash = self.hash.as_deref().filter(|h| !h.trim().is_empty())?;
        let name = self.name.as_deref().filter(|n| !n.trim().is_empty())?;
        let (status, message) = match self.state.as_str() {
            "Error" => (
                ItemStatus::Warning,
                Some("Deluge is reporting an error".to_string()),
            ),
            s if self.is_finished && s != "Checking" => (ItemStatus::Completed, None),
            "Queued" => (ItemStatus::Queued, None),
            "Paused" => (ItemStatus::Paused, None),
            _ => (ItemStatus::Downloading, None),
        };
        Some(DownloadItem {
            download_id: hash.to_ascii_uppercase(),
            title: name.to_string(),
            category: category.unwrap_or_default().to_string(),
            total_size: self.total_size,
            remaining_size: (self.total_size - self.total_done).max(0),
            remaining_seconds: (self.eta >= 0.0 && self.eta.is_finite()).then_some(self.eta as i64),
            status,
            message,
            output_path: Some(join(&self.save_path, name)),
            seed_ratio: self.ratio,
            // Deluge manages it and stopped it at its ratio.
            can_be_removed: self.is_auto_managed
                && self.stop_at_ratio
                && self.ratio >= self.stop_ratio
                && self.state == "Paused",
        })
    }
}

/// What a test found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub version: String,
    pub warnings: Vec<String>,
}

/// A JSON-RPC error (`DelugeError`).
#[derive(Debug, Deserialize)]
struct RpcError {
    #[serde(default)]
    message: String,
    #[serde(default)]
    code: i64,
}

enum Call {
    Ok(Value),
    /// Codes 1 and 2: not authenticated / not connected to a daemon.
    Unauthorised,
    Failed(String),
}

/// A Deluge Web UI client; session cookies are cached per address and password.
#[derive(Clone, Default)]
pub struct Deluge {
    http: reqwest::Client,
    cookies: Arc<Mutex<HashMap<String, String>>>,
    next_id: Arc<AtomicU64>,
}

impl Deluge {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            ..Default::default()
        }
    }

    fn cookie(&self, s: &Settings) -> Option<String> {
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&s.cookie_key())
            .cloned()
    }

    /// `ExecuteRequest`: one call with the given cookie.
    async fn execute(
        &self,
        s: &Settings,
        cookie: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<(Call, Option<String>), DownloadError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut req = self
            .http
            .post(s.rpc_url())
            .timeout(Duration::from_secs(15))
            .json(&json!({ "method": method, "params": params, "id": id }));
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| DownloadError::Connection(crate::error::cause(&e)))?;
        let status = resp.status();
        if status == StatusCode::REQUEST_TIMEOUT {
            // The daemon connection may have broken: reconnect, as upstream does.
            return Ok((Call::Unauthorised, None));
        }
        if !status.is_success() {
            return Err(DownloadError::Http {
                status: status.as_u16(),
            });
        }
        let session = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .find(|c| c.trim_start().starts_with("_session_id="))
            .map(|c| c.trim().to_string());
        let body: Value = resp.json().await.map_err(|e| {
            DownloadError::Client(format!("Deluge sent an unreadable answer ({e})"))
        })?;
        let call = match body.get("error").filter(|e| !e.is_null()) {
            Some(err) => {
                let err: RpcError = serde_json::from_value(err.clone()).unwrap_or(RpcError {
                    message: err.to_string(),
                    code: 0,
                });
                if err.code == 1 || err.code == 2 {
                    Call::Unauthorised
                } else {
                    Call::Failed(err.message)
                }
            }
            None => Call::Ok(body.get("result").cloned().unwrap_or(Value::Null)),
        };
        Ok((call, session))
    }

    /// `AuthenticateClient` with `reauthenticate`: logs in and connects the daemon.
    async fn login(&self, s: &Settings) -> Result<String, DownloadError> {
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&s.cookie_key());
        let (call, session) = self
            .execute(s, None, "auth.login", json!([s.password]))
            .await?;
        match call {
            Call::Ok(Value::Bool(true)) => {}
            _ => return Err(DownloadError::Authentication),
        }
        let cookie = session.ok_or_else(|| {
            DownloadError::Client("Deluge did not return a session cookie".into())
        })?;
        self.connect_daemon(s, &cookie).await?;
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(s.cookie_key(), cookie.clone());
        Ok(cookie)
    }

    /// `ConnectDaemon`: connects the Web UI to the local daemon when it is not connected.
    async fn connect_daemon(&self, s: &Settings, cookie: &str) -> Result<(), DownloadError> {
        let ok = |call: Call| match call {
            Call::Ok(v) => Ok(v),
            Call::Unauthorised => Err(DownloadError::Authentication),
            Call::Failed(m) => Err(DownloadError::Client(m)),
        };
        let connected = ok(self
            .execute(s, Some(cookie), "web.connected", json!([]))
            .await?
            .0)?;
        if connected.as_bool() == Some(true) {
            return Ok(());
        }
        let hosts = ok(self
            .execute(s, Some(cookie), "web.get_hosts", json!([]))
            .await?
            .0)?;
        let local = hosts.as_array().and_then(|hosts| {
            hosts
                .iter()
                .find(|h| h.get(1).and_then(Value::as_str) == Some("127.0.0.1"))
        });
        if let Some(id) = local.and_then(|h| h.get(0)).cloned() {
            ok(self
                .execute(s, Some(cookie), "web.connect", json!([id]))
                .await?
                .0)?;
            return Ok(());
        }
        Err(DownloadError::Client(
            "the Deluge Web UI is not connected to a daemon, and no local daemon (127.0.0.1) is listed in its Connection Manager".into(),
        ))
    }

    /// `ProcessRequest`: calls `method`, logging in again once when the session expired.
    async fn call(
        &self,
        s: &Settings,
        method: &str,
        params: Value,
    ) -> Result<Value, DownloadError> {
        let cookie = match self.cookie(s) {
            Some(c) => c,
            None => self.login(s).await?,
        };
        let (mut call, _) = self
            .execute(s, Some(&cookie), method, params.clone())
            .await?;
        if matches!(call, Call::Unauthorised) {
            let cookie = self.login(s).await?;
            call = self.execute(s, Some(&cookie), method, params).await?.0;
        }
        match call {
            Call::Ok(v) => Ok(v),
            Call::Unauthorised => Err(DownloadError::Authentication),
            Call::Failed(m) => Err(DownloadError::Client(format!("Deluge: {m}"))),
        }
    }

    /// `GetVersion`.
    pub async fn version(&self, s: &Settings) -> Result<String, DownloadError> {
        let methods = self.methods(s).await?;
        let method = if methods.iter().any(|m| m == "daemon.get_version") {
            "daemon.get_version"
        } else {
            "daemon.info"
        };
        Ok(self
            .call(s, method, json!([]))
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    async fn methods(&self, s: &Settings) -> Result<Vec<String>, DownloadError> {
        Ok(strings(
            self.call(s, "system.listMethods", json!([])).await?,
        ))
    }

    async fn labels(&self, s: &Settings) -> Result<Vec<String>, DownloadError> {
        Ok(strings(self.call(s, "label.get_labels", json!([])).await?))
    }

    /// `GetTorrents` / `GetTorrentsByLabel` through `web.update_ui`.
    pub async fn torrents(&self, s: &Settings) -> Result<Vec<Torrent>, DownloadError> {
        let filter = match &s.category {
            Some(label) => json!({ "label": label }),
            None => json!({}),
        };
        let result = self
            .call(s, "web.update_ui", json!([PROPERTIES, filter]))
            .await?;
        let Some(map) = result.get("torrents").and_then(Value::as_object) else {
            return Ok(Vec::new());
        };
        Ok(map
            .iter()
            .map(|(hash, t)| {
                let mut torrent: Torrent = serde_json::from_value(t.clone()).unwrap_or_default();
                if torrent.hash.as_deref().is_none_or(str::is_empty) {
                    torrent.hash = Some(hash.clone());
                }
                torrent
            })
            .collect())
    }

    /// `GetItems`. Torrents without a hash or name are skipped (upstream reconnects the daemon
    /// once and warns).
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let torrents = self.torrents(s).await?;
        let items: Vec<DownloadItem> = torrents
            .iter()
            .filter_map(|t| t.to_item(s.category.as_deref()))
            .collect();
        let ignored = torrents.len() - items.len();
        if ignored > 0 {
            tracing::warn!(
                ignored,
                "Deluge listed torrents without a hash or name; it may have lost its daemon connection"
            );
        }
        Ok(items)
    }

    /// `MarkItemAsImported`: the post-import label, when one is set and differs from the TV
    /// label. Failures are logged, as upstream.
    /// `RemoveItem`: `core.remove_torrent`, with the data when asked.
    pub async fn remove(
        &self,
        s: &Settings,
        hash: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        self.call(
            s,
            "core.remove_torrent",
            json!([hash.to_ascii_lowercase(), delete_data]),
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
        if let Err(e) = self
            .call(
                s,
                "label.set_torrent",
                json!([hash.to_ascii_lowercase(), imported]),
            )
            .await
        {
            tracing::warn!(error = %e, label = %imported, "could not set the post-import label in Deluge; does the label exist?");
        }
        Ok(())
    }

    /// `AddFromMagnetLink` / `AddFromTorrentFile`: adds, labels and (for a "first" priority)
    /// moves the torrent to the top of the queue. Returns the info hash, upper-case.
    pub async fn add(
        &self,
        s: &Settings,
        source: &crate::qbittorrent::Source<'_>,
        recent: bool,
        seed: Option<crate::qbittorrent::SeedConfig>,
    ) -> Result<String, DownloadError> {
        let options = s.add_options();
        let (result, what) = match source {
            crate::qbittorrent::Source::Magnet(url) => (
                self.call(s, "core.add_torrent_magnet", json!([url, options]))
                    .await?,
                "magnet",
            ),
            crate::qbittorrent::Source::File { name, content } => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(content);
                (
                    self.call(s, "core.add_torrent_file", json!([name, encoded, options]))
                        .await?,
                    "torrent",
                )
            }
        };
        let hash = result
            .as_str()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| DownloadError::Client(format!("Deluge failed to add the {what}")))?
            .to_string();
        // `SetTorrentSeedingConfiguration`: Deluge takes the ratio only.
        if let Some(ratio) = seed.and_then(|c| c.ratio) {
            self.call(
                s,
                "core.set_torrent_options",
                json!([[hash], {"stop_ratio": ratio, "stop_at_ratio": 1}]),
            )
            .await?;
        }
        if let Some(label) = &s.category {
            self.call(s, "label.set_torrent", json!([hash, label]))
                .await?;
        }
        let priority = if recent {
            s.recent_priority
        } else {
            s.older_priority
        };
        if priority == 1
            && let Err(e) = self.call(s, "core.queue_top", json!([[hash]])).await
        {
            tracing::warn!(error = %e, %hash, "failed to move the torrent to the top of the queue");
        }
        Ok(hash.to_ascii_uppercase())
    }

    /// `Test`: connection and version, the Label plugin and labels (created when missing, as
    /// upstream does), and listing torrents.
    pub async fn test(&self, s: &Settings) -> Result<TestReport, DownloadError> {
        let version = self.version(s).await?;
        let mut warnings = Vec::new();
        if s.category.is_none() {
            warnings.push(
                "A label is recommended: it keeps Kitsunarr's downloads apart from everything else in Deluge".into(),
            );
        }
        let wanted: Vec<&String> = [&s.category, &s.imported_category]
            .into_iter()
            .flatten()
            .collect();
        if !wanted.is_empty() {
            if !self
                .methods(s)
                .await?
                .iter()
                .any(|m| m.starts_with("label."))
            {
                return Err(DownloadError::Client(
                    "the Label plugin is not enabled in Deluge; enable it (Preferences › Plugins) to use labels".into(),
                ));
            }
            for label in wanted {
                if !self.labels(s).await?.contains(label) {
                    self.call(s, "label.add", json!([label])).await?;
                    if !self.labels(s).await?.contains(label) {
                        return Err(DownloadError::Client(format!(
                            "the label {label:?} could not be created in Deluge"
                        )));
                    }
                }
            }
        }
        self.torrents(s).await?;
        Ok(TestReport { version, warnings })
    }
}

fn strings(v: Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
