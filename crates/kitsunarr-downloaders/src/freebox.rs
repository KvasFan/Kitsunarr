//! Freebox Download (Sonarr's `TorrentFreeboxDownload`, `FreeboxDownloadProxy`): the Freebox
//! OS API with an app token — the session password is the HMAC-SHA1 of the login challenge —
//! torrent tasks into a per-release folder (paths are base64 on the wire), then pause, queue
//! position and the indexer's seed ratio.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};

use crate::error::cause;
use crate::{DownloadError, DownloadItem, ItemStatus, SeedConfig, Source};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub api_url: String,
    pub app_id: String,
    pub app_token: String,
    pub destination: Option<String>,
    pub category: Option<String>,
    /// `FreeboxDownloadPriority`: 0 last, 1 first.
    pub recent_priority: i64,
    pub older_priority: i64,
    pub add_paused: bool,
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
        let host = text(map, "host").unwrap_or_else(|| "mafreebox.freebox.fr".into());
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 443,
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
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(true),
            api_url: text(map, "apiUrl").unwrap_or_else(|| "/api/v1/".into()),
            app_id: text(map, "appId").ok_or("the App ID is required")?,
            app_token: text(map, "appToken").ok_or("the App Token is required")?,
            destination: text(map, "destinationDirectory"),
            category: text(map, "category"),
            recent_priority: map
                .get("recentPriority")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            older_priority: map
                .get("olderPriority")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            add_paused: map
                .get("addPaused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn url(&self, path: &str) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        format!(
            "{scheme}://{}:{}/{}/{}",
            self.host,
            self.port,
            self.api_url.trim_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn key(&self) -> String {
        format!("{}:{}:{}", self.host, self.app_id, self.app_token)
    }
}

/// HMAC-SHA1 (RFC 2104), lower-case hex.
pub fn hmac_sha1_hex(key: &[u8], message: &[u8]) -> String {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| block.iter().map(|k| k ^ b).collect::<Vec<u8>>();
    let mut inner = Sha1::new();
    inner.update(pad(0x36));
    inner.update(message);
    let mut outer = Sha1::new();
    outer.update(pad(0x5c));
    outer.update(inner.finalize());
    hex::encode(outer.finalize())
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn unb64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default()
}

/// A task as `GetItems` maps it (only BitTorrent tasks are listed).
pub fn to_item(t: &Value, s: &Settings) -> Option<DownloadItem> {
    if !t["type"]
        .as_str()
        .is_some_and(|x| x.eq_ignore_ascii_case("bt"))
    {
        return None;
    }
    let dir = unb64(t["download_dir"].as_str().unwrap_or_default());
    if let Some(dest) = &s.destination {
        let dest = dest.trim_end_matches('/');
        if !(dir == dest || dir.starts_with(&format!("{dest}/"))) {
            return None;
        }
    }
    if let Some(c) = &s.category
        && !dir.split(['/', '\\']).any(|p| p == c)
    {
        return None;
    }
    let size = t["size"].as_i64().unwrap_or(0);
    let received = t["rx_pct"].as_i64().unwrap_or(0) as f64 / 10_000.0;
    let eta = t["eta"].as_i64().unwrap_or(0);
    let stop_ratio = t["stop_ratio"].as_i64().unwrap_or(0);
    let status = t["status"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (item_status, message) = match status.as_str() {
        "stopped" | "stopping" => (ItemStatus::Paused, None),
        "queued" => (ItemStatus::Queued, None),
        "starting" | "downloading" | "retry" | "checking" => (ItemStatus::Downloading, None),
        "error" => (
            ItemStatus::Warning,
            Some(format!(
                "Freebox reports an error: {}",
                t["error"].as_str().unwrap_or("unknown")
            )),
        ),
        "done" | "seeding" => (ItemStatus::Completed, None),
        other => (
            ItemStatus::Downloading,
            Some(format!("Unknown download state: {other}")),
        ),
    };
    Some(DownloadItem {
        download_id: t["id"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| t["id"].to_string()),
        title: t["name"].as_str().unwrap_or_default().to_string(),
        category: s.category.clone().unwrap_or_default(),
        total_size: size,
        remaining_size: (size as f64 * (1.0 - received)) as i64,
        remaining_seconds: (eta > 0).then_some(eta),
        status: item_status,
        message,
        output_path: Some(dir),
        seed_ratio: if stop_ratio <= 0 {
            0.0
        } else {
            stop_ratio as f64 / 100.0
        },
        can_be_removed: status == "done",
        seed_progress: None,
    })
}

#[derive(Clone, Default)]
pub struct Freebox {
    http: reqwest::Client,
    sessions: Arc<Mutex<HashMap<String, String>>>,
}

impl Freebox {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            sessions: Arc::default(),
        }
    }

    async fn process(
        &self,
        s: &Settings,
        req: reqwest::RequestBuilder,
    ) -> Result<Value, DownloadError> {
        let resp = req
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        match status {
            401 | 403 => {
                self.sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&s.key());
                Err(DownloadError::Authentication)
            }
            404 => Err(DownloadError::Client(
                "Unable to reach the Freebox API; check the API URL (base and version).".into(),
            )),
            200 if v["success"].as_bool() == Some(true) => Ok(v["result"].clone()),
            200 => Err(DownloadError::Client(format!(
                "Freebox API returned an error: {} ({})",
                v["msg"].as_str().unwrap_or_default(),
                v["error_code"].as_str().unwrap_or_default()
            ))),
            status => Err(DownloadError::Http { status }),
        }
    }

    /// `GetSessionToken`: the challenge, its HMAC with the app token, the session.
    async fn session(&self, s: &Settings) -> Result<String, DownloadError> {
        if let Some(t) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&s.key())
        {
            return Ok(t.clone());
        }
        let login = self.process(s, self.http.get(s.url("login"))).await?;
        let challenge = login["challenge"].as_str().unwrap_or_default();
        let password = hmac_sha1_hex(s.app_token.as_bytes(), challenge.as_bytes());
        let session = self
            .process(
                s,
                self.http
                    .post(s.url("login/session"))
                    .json(&json!({ "app_id": s.app_id, "password": password })),
            )
            .await?;
        let token = session["session_token"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(s.key(), token.clone());
        Ok(token)
    }

    async fn call(
        &self,
        s: &Settings,
        build: impl Fn(&str) -> reqwest::RequestBuilder,
    ) -> Result<Value, DownloadError> {
        let token = self.session(s).await?;
        self.process(s, build(&token)).await
    }

    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let v = self
            .call(s, |t| {
                self.http
                    .get(s.url("downloads/"))
                    .header("X-Fbx-App-Auth", t)
            })
            .await?;
        Ok(v.as_array()
            .map(|a| a.iter().filter_map(|t| to_item(t, s)).collect())
            .unwrap_or_default())
    }

    /// `GetDownloadDirectory`: the destination, else the default folder (plus the category),
    /// then a folder named for the release.
    async fn directory(&self, s: &Settings, title: &str) -> Result<String, DownloadError> {
        let base = match &s.destination {
            Some(d) => d.trim_end_matches('/').to_string(),
            None => {
                let c = self
                    .call(s, |t| {
                        self.http
                            .get(s.url("downloads/config/"))
                            .header("X-Fbx-App-Auth", t)
                    })
                    .await?;
                let dir = unb64(c["download_dir"].as_str().unwrap_or_default());
                let dir = dir.trim_end_matches('/').to_string();
                match &s.category {
                    Some(cat) => format!("{dir}/{cat}"),
                    None => dir,
                }
            }
        };
        if title.trim().is_empty() {
            return Ok(base);
        }
        Ok(format!(
            "{base}/{}",
            crate::blackhole::clean_file_name(title)
        ))
    }

    pub async fn add(
        &self,
        s: &Settings,
        source: &Source<'_>,
        title: &str,
        recent: bool,
        seed: Option<SeedConfig>,
    ) -> Result<String, DownloadError> {
        let dir = b64(&self.directory(s, title).await?);
        let added = match source {
            Source::Magnet(url) => {
                self.call(s, |t| {
                    self.http
                        .post(s.url("downloads/add"))
                        .header("X-Fbx-App-Auth", t)
                        .form(&[
                            ("download_url", url.to_string()),
                            ("download_dir", dir.clone()),
                        ])
                })
                .await?
            }
            Source::File { name, content } => {
                self.call(s, |t| {
                    let part = reqwest::multipart::Part::bytes(content.to_vec())
                        .file_name(name.to_string());
                    let form = reqwest::multipart::Form::new()
                        .part("download_file", part)
                        .text("download_dir", dir.clone());
                    self.http
                        .post(s.url("downloads/add"))
                        .header("X-Fbx-App-Auth", t)
                        .multipart(form)
                })
                .await?
            }
        };
        let id = match &added["id"] {
            Value::String(x) => x.clone(),
            other => other.to_string(),
        };
        // `SetTorrentSettings`.
        let first = (if recent {
            s.recent_priority
        } else {
            s.older_priority
        }) == 1;
        let mut body = Map::new();
        if s.add_paused {
            body.insert("status".into(), json!("stopped"));
        }
        if first {
            body.insert("queue_pos".into(), json!("1"));
        }
        if let Some(r) = seed.and_then(|c| c.ratio) {
            body.insert("stop_ratio".into(), json!(r * 100.0));
        }
        if !body.is_empty() {
            self.call(s, |t| {
                self.http
                    .put(s.url(&format!("downloads/{id}")))
                    .header("X-Fbx-App-Auth", t)
                    .json(&body)
            })
            .await?;
        }
        Ok(id)
    }

    pub async fn remove(
        &self,
        s: &Settings,
        id: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        let path = if delete_data {
            format!("downloads/{id}/erase")
        } else {
            format!("downloads/{id}")
        };
        self.call(s, |t| {
            self.http.delete(s.url(&path)).header("X-Fbx-App-Auth", t)
        })
        .await
        .map(|_| ())
    }

    /// `Test`: the session (`/login` reports it logged in).
    pub async fn test(&self, s: &Settings) -> Result<(), DownloadError> {
        let v = self
            .call(s, |t| {
                self.http.get(s.url("login")).header("X-Fbx-App-Auth", t)
            })
            .await?;
        if v["logged_in"].as_bool() == Some(false) {
            return Err(DownloadError::Authentication);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_2202() {
        assert_eq!(
            hmac_sha1_hex(b"key", b"The quick brown fox jumps over the lazy dog"),
            "de7c9b85b8b78aa6bc8a7a36f70a90701c9db4d9"
        );
    }

    #[test]
    fn tasks_map_as_upstream() {
        let s = Settings::from_map(
            serde_json::from_value::<Map<String, Value>>(json!({
                "appId": "fr.kitsunarr", "appToken": "t", "category": "tv"
            }))
            .as_ref()
            .unwrap(),
        )
        .unwrap();
        let task = |status: &str, dir: &str| {
            json!({"id": 12, "name": "Show.S01E01", "download_dir": b64(dir), "status": status,
                "type": "bt", "size": 1000, "rx_pct": 2500, "eta": 0, "stop_ratio": 150})
        };
        let done = to_item(
            &task("done", "/Disque dur/Téléchargements/tv/Show.S01E01"),
            &s,
        )
        .unwrap();
        assert_eq!(done.status, ItemStatus::Completed);
        assert!(done.can_be_removed);
        assert_eq!(done.download_id, "12");
        assert_eq!(done.remaining_size, 750);
        assert_eq!(done.seed_ratio, 1.5);
        assert_eq!(done.remaining_seconds, None);
        assert_eq!(
            done.output_path.as_deref(),
            Some("/Disque dur/Téléchargements/tv/Show.S01E01")
        );
        assert!(
            !to_item(&task("seeding", "/d/tv/x"), &s)
                .unwrap()
                .can_be_removed
        );
        assert_eq!(
            to_item(&task("stopped", "/d/tv/x"), &s).unwrap().status,
            ItemStatus::Paused
        );
        assert_eq!(
            to_item(&task("error", "/d/tv/x"), &s).unwrap().status,
            ItemStatus::Warning
        );
        assert!(
            to_item(&task("done", "/d/movies/x"), &s).is_none(),
            "another category"
        );
        let mut http = task("done", "/d/tv/x");
        http["type"] = json!("http");
        assert!(to_item(&http, &s).is_none());
    }
}
