//! Hadouken 5.1+ over JSON-RPC (Sonarr's `Hadouken`, `HadoukenProxy`): `webui.list` rows read
//! by position, adds with the category as label, removal with or without data.

use serde_json::{Map, Value, json};

use crate::error::cause;
use crate::{DownloadError, DownloadItem, ItemStatus, Source};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub url_base: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub category: Option<String>,
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
            None | Some(Value::Null) => 7070,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        Ok(Self {
            host,
            port,
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(false),
            url_base: text(map, "urlBase").unwrap_or_default(),
            username: text(map, "username"),
            password: text(map, "password"),
            category: text(map, "category"),
        })
    }

    fn url(&self) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let base = self.url_base.trim_matches('/');
        let base = if base.is_empty() {
            String::new()
        } else {
            format!("/{base}")
        };
        format!("{scheme}://{}:{}{base}/api", self.host, self.port)
    }
}

/// `HadoukenTorrentState`, from the state flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Downloading,
    CheckingFiles,
    Paused,
    QueuedForChecking,
    Unknown,
}

fn state(flags: i64) -> State {
    if flags & 1 == 1 {
        State::Downloading
    } else if flags & 2 == 2 {
        State::CheckingFiles
    } else if flags & 32 == 32 {
        State::Paused
    } else if flags & 64 == 64 {
        State::QueuedForChecking
    } else {
        State::Unknown
    }
}

/// A `webui.list` row (`MapTorrent`).
#[derive(Debug, Clone, PartialEq)]
pub struct Torrent {
    pub info_hash: String,
    pub state: State,
    pub name: String,
    pub total_size: i64,
    pub progress: f64,
    pub downloaded: i64,
    pub uploaded: i64,
    pub download_rate: i64,
    pub label: String,
    pub error: String,
    pub save_path: String,
}

impl Torrent {
    pub fn from_row(row: &[Value]) -> Option<Self> {
        let s = |i: usize| match row.get(i) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let n = |i: usize| row.get(i).and_then(Value::as_i64).unwrap_or(0);
        Some(Torrent {
            info_hash: row.first()?.as_str()?.to_string(),
            state: state(n(1)),
            name: s(2),
            total_size: n(3),
            progress: row.get(4).and_then(Value::as_f64).unwrap_or(0.0),
            downloaded: n(5),
            uploaded: n(6),
            download_rate: n(9),
            label: s(11),
            error: s(21),
            save_path: s(26),
        })
    }

    pub fn is_finished(&self) -> bool {
        self.progress >= 1000.0
    }

    /// `GetItems`' mapping (the time left is the total size over the rate, as upstream).
    pub fn to_item(&self) -> DownloadItem {
        let (status, message) = if !self.error.is_empty() {
            (ItemStatus::Warning, Some(self.error.clone()))
        } else if self.is_finished() && self.state != State::CheckingFiles {
            (ItemStatus::Completed, None)
        } else if self.state == State::QueuedForChecking {
            (ItemStatus::Queued, None)
        } else if self.state == State::Paused {
            (ItemStatus::Paused, None)
        } else {
            (ItemStatus::Downloading, None)
        };
        let dir = self.save_path.trim_end_matches(['/', '\\']);
        DownloadItem {
            download_id: self.info_hash.to_ascii_uppercase(),
            title: self.name.clone(),
            category: self.label.clone(),
            total_size: self.total_size,
            remaining_size: self.total_size - self.downloaded,
            remaining_seconds: Some(if self.download_rate > 0 && self.total_size > 0 {
                self.total_size / self.download_rate
            } else {
                0
            }),
            status,
            message,
            output_path: Some(format!("{dir}/{}", self.name)),
            seed_ratio: if self.downloaded <= 0 {
                0.0
            } else {
                self.uploaded as f64 / self.downloaded as f64
            },
            can_be_removed: self.is_finished() && self.state == State::Paused,
            seed_progress: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct Hadouken {
    http: reqwest::Client,
}

impl Hadouken {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    async fn call(
        &self,
        s: &Settings,
        method: &str,
        params: Value,
    ) -> Result<Value, DownloadError> {
        let mut req = self.http.post(s.url()).json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        }));
        if let Some(u) = &s.username {
            req = req.basic_auth(u, s.password.as_deref());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        match resp.status().as_u16() {
            401 | 403 => return Err(DownloadError::Authentication),
            200..=299 => {}
            status => return Err(DownloadError::Http { status }),
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        if !v["error"].is_null() {
            return Err(DownloadError::Client(format!(
                "Error response received from Hadouken: {}",
                v["error"]
            )));
        }
        Ok(v["result"].clone())
    }

    pub async fn torrents(&self, s: &Settings) -> Result<Vec<Torrent>, DownloadError> {
        let v = self.call(s, "webui.list", json!([])).await?;
        Ok(v["torrents"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| Torrent::from_row(r.as_array()?))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        Ok(self
            .torrents(s)
            .await?
            .iter()
            .filter(|t| s.category.as_deref().is_none_or(|c| t.label == c))
            .map(Torrent::to_item)
            .collect())
    }

    pub async fn add(
        &self,
        s: &Settings,
        source: &Source<'_>,
        hash: &str,
    ) -> Result<String, DownloadError> {
        use base64::Engine;
        let options = json!({ "label": s.category });
        match source {
            Source::Magnet(url) => {
                self.call(s, "webui.addTorrent", json!(["url", url, options]))
                    .await?;
                Ok(hash.to_ascii_uppercase())
            }
            Source::File { content, .. } => {
                let file = base64::engine::general_purpose::STANDARD.encode(content);
                let id = self
                    .call(s, "webui.addTorrent", json!(["file", file, options]))
                    .await?;
                Ok(id
                    .as_str()
                    .filter(|i| !i.is_empty())
                    .unwrap_or(hash)
                    .to_ascii_uppercase())
            }
        }
    }

    pub async fn remove(
        &self,
        s: &Settings,
        hash: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        let action = if delete_data { "removedata" } else { "remove" };
        self.call(s, "webui.perform", json!([action, [hash]]))
            .await
            .map(|_| ())
    }

    /// `Test`: Hadouken 5.1 or newer, and the torrent list.
    pub async fn test(&self, s: &Settings) -> Result<String, DownloadError> {
        let info = self.call(s, "core.getSystemInfo", json!([])).await?;
        let version = info["versions"]["hadouken"]
            .as_str()
            .unwrap_or("0.0")
            .to_string();
        let parts: Vec<u64> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        if parts.as_slice() < [5, 1].as_slice() {
            return Err(DownloadError::Unsupported(format!(
                "Hadouken 5.1 or newer is required; it reports {version}"
            )));
        }
        self.torrents(s).await?;
        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: i64, progress: f64, error: &str) -> Vec<Value> {
        let mut r = vec![
            json!("hash"),
            json!(state),
            json!("Show.S01E01"),
            json!(1000),
            json!(progress),
            json!(500),
            json!(1000),
            json!(0),
            json!(0),
            json!(100),
            json!(0),
            json!("kitsunarr-tv"),
        ];
        r.extend((12..21).map(|_| json!(0)));
        r.push(json!(error));
        r.extend((22..26).map(|_| json!(0)));
        r.push(json!("/downloads"));
        r
    }

    #[test]
    fn torrents_map_as_upstream() {
        let t = |s, p, e| Torrent::from_row(&row(s, p, e)).unwrap().to_item();
        let done = t(32, 1000.0, "");
        assert_eq!(done.status, ItemStatus::Completed);
        assert!(done.can_be_removed);
        assert_eq!(done.output_path.as_deref(), Some("/downloads/Show.S01E01"));
        assert_eq!(done.seed_ratio, 2.0);
        assert_eq!(done.remaining_seconds, Some(10));
        assert!(!t(1, 1000.0, "").can_be_removed);
        assert_eq!(t(2, 1000.0, "").status, ItemStatus::Downloading);
        assert_eq!(t(64, 0.0, "").status, ItemStatus::Queued);
        assert_eq!(t(32, 10.0, "").status, ItemStatus::Paused);
        assert_eq!(t(1, 10.0, "").status, ItemStatus::Downloading);
        let warn = t(1, 10.0, "tracker down");
        assert_eq!(
            (warn.status, warn.message.as_deref()),
            (ItemStatus::Warning, Some("tracker down"))
        );
    }
}
