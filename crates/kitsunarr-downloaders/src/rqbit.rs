//! rqbit 8+ (Sonarr's `RQBit`, `RQbitProxy`): its HTTP API, torrents added by URL or file
//! (`POST /torrents?overwrite=true`), listed with their stats, removed by `delete` (with data)
//! or `forget`. rqbit has no labels and enforces no seed goals (see `SeedProgress`).

use serde_json::{Map, Value};

use crate::error::cause;
use crate::{DownloadError, DownloadItem, ItemStatus, SeedProgress, Source};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub url_base: String,
}

impl Settings {
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let host = map
            .get("host")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("localhost")
            .to_string();
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 3030,
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
            url_base: map
                .get("urlBase")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .trim()
                .to_string(),
        })
    }

    fn url(&self, path: &str) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let base = self.url_base.trim_matches('/');
        let base = if base.is_empty() {
            String::new()
        } else {
            format!("/{base}")
        };
        format!("{scheme}://{}:{}{base}{path}", self.host, self.port)
    }
}

/// A torrent from `/torrents?with_stats=true` (`RQBitTorrent`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Torrent {
    pub name: String,
    pub hash: String,
    pub path: String,
    pub total_size: i64,
    pub remaining_size: i64,
    pub down_rate: i64,
    pub ratio: f64,
    pub is_finished: bool,
    pub is_active: bool,
    pub message: Option<String>,
}

fn state_name(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_ascii_lowercase(),
        Value::Number(n) => match n.as_i64() {
            Some(0) => "initializing",
            Some(1) => "paused",
            Some(2) => "live",
            Some(3) => "error",
            _ => "invalid",
        }
        .into(),
        _ => String::new(),
    }
}

impl Torrent {
    pub fn from_value(t: &Value) -> Option<Self> {
        let stats = &t["stats"];
        let name = t["name"].as_str().unwrap_or_default().to_string();
        let mut path = t["output_folder"].as_str().unwrap_or_default().to_string();
        if !path.ends_with(&name) {
            path.push_str(&name);
        }
        let total = stats["total_bytes"].as_i64().unwrap_or(0);
        let progress = stats["progress_bytes"].as_i64().unwrap_or(0);
        let uploaded = stats["uploaded_bytes"].as_i64().unwrap_or(0);
        let state = state_name(&stats["state"]);
        Some(Torrent {
            hash: t["info_hash"].as_str()?.to_string(),
            name,
            path,
            total_size: total,
            remaining_size: total - progress,
            // MiB/s to bytes per second.
            down_rate: (stats["live"]["download_speed"]["mbps"]
                .as_f64()
                .unwrap_or(0.0)
                * 1_048_576.0) as i64,
            ratio: if progress > 0 {
                uploaded as f64 / progress as f64
            } else {
                0.0
            },
            is_finished: stats["finished"].as_bool().unwrap_or(false),
            is_active: matches!(state.as_str(), "live" | "initializing"),
            message: stats["error"]
                .as_str()
                .filter(|e| !e.is_empty())
                .map(str::to_string),
        })
    }

    pub fn to_item(&self) -> DownloadItem {
        let status = if self.is_finished {
            ItemStatus::Completed
        } else if self.is_active {
            ItemStatus::Downloading
        } else {
            ItemStatus::Paused
        };
        DownloadItem {
            download_id: self.hash.to_ascii_uppercase(),
            title: self.name.clone(),
            category: String::new(),
            total_size: self.total_size,
            remaining_size: self.remaining_size,
            remaining_seconds: Some(if self.down_rate > 0 {
                self.remaining_size / self.down_rate
            } else {
                0
            }),
            status,
            message: self.message.clone(),
            output_path: Some(self.path.clone()),
            seed_ratio: self.ratio,
            can_be_removed: false,
            // rqbit reports no finish time: only the seed ratio can be reached.
            seed_progress: self.is_finished.then_some(SeedProgress {
                ratio: self.ratio,
                finished_at: None,
            }),
        }
    }
}

#[derive(Clone, Default)]
pub struct RQBit {
    http: reqwest::Client,
}

impl RQBit {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, DownloadError> {
        let resp = req
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        match resp.status().as_u16() {
            401 | 403 => Err(DownloadError::Authentication),
            200..=299 => Ok(resp),
            status => Err(DownloadError::Http { status }),
        }
    }

    pub async fn version(&self, s: &Settings) -> Result<String, DownloadError> {
        let v: Value = self
            .send(self.http.get(s.url("/")))
            .await?
            .json()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        Ok(v["version"].as_str().unwrap_or_default().to_string())
    }

    /// `GetTorrents`: those with an absolute output path.
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let v: Value = self
            .send(self.http.get(s.url("/torrents?with_stats=true")))
            .await?
            .json()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        Ok(v["torrents"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Torrent::from_value)
                    .filter(|t| !t.path.trim().is_empty() && !t.path.starts_with('.'))
                    .map(|t| t.to_item())
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn add(
        &self,
        s: &Settings,
        source: &Source<'_>,
        hash: &str,
    ) -> Result<String, DownloadError> {
        let body: Vec<u8> = match source {
            Source::Magnet(url) => url.as_bytes().to_vec(),
            Source::File { content, .. } => content.to_vec(),
        };
        let v: Value = self
            .send(self.http.post(s.url("/torrents?overwrite=true")).body(body))
            .await?
            .json()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        Ok(v["details"]["info_hash"]
            .as_str()
            .unwrap_or(hash)
            .to_ascii_uppercase())
    }

    pub async fn remove(
        &self,
        s: &Settings,
        hash: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        let action = if delete_data { "delete" } else { "forget" };
        self.send(
            self.http
                .post(s.url(&format!("/torrents/{}/{action}", hash.to_ascii_lowercase()))),
        )
        .await
        .map(|_| ())
    }

    /// `Test`: version 8.0.0 or newer.
    pub async fn test(&self, s: &Settings) -> Result<String, DownloadError> {
        let version = self.version(s).await?;
        let re = regex::Regex::new(r"(\d+)\.(\d+)\.(\d+)").expect("valid");
        let Some(c) = re.captures(&version) else {
            return Err(DownloadError::Client(format!(
                "Unable to parse rqbit's version {version:?}"
            )));
        };
        let major: u64 = c[1].parse().unwrap_or(0);
        if major < 8 {
            return Err(DownloadError::Unsupported(format!(
                "rqbit 8.0.0 or newer is required; it reports {version}"
            )));
        }
        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn torrents_map_as_upstream() {
        let t = |state: Value, finished: bool| {
            Torrent::from_value(&json!({
                "id": 1, "info_hash": "abc", "name": "Show.S01E01", "output_folder": "/dl/",
                "stats": {"state": state, "progress_bytes": 500, "uploaded_bytes": 1000, "total_bytes": 1000,
                    "finished": finished, "error": null,
                    "live": {"download_speed": {"mbps": 1.0}}}
            }))
            .unwrap()
            .to_item()
        };
        let done = t(json!("live"), true);
        assert_eq!(done.status, ItemStatus::Completed);
        assert_eq!(done.output_path.as_deref(), Some("/dl/Show.S01E01"));
        assert_eq!(done.seed_ratio, 2.0);
        assert_eq!(done.download_id, "ABC");
        assert!(done.seed_progress.is_some());
        let live = t(json!("live"), false);
        assert_eq!(live.status, ItemStatus::Downloading);
        assert_eq!(live.remaining_seconds, Some(0));
        assert_eq!(t(json!(1), false).status, ItemStatus::Paused);
        assert_eq!(
            t(json!("initializing"), false).status,
            ItemStatus::Downloading
        );
    }
}
