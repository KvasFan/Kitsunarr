//! NZBVortex 3 (API level 2.3+; Sonarr's `NzbVortex`, `NzbVortexProxy`): HTTPS, a session
//! from the nonce handshake (`base64(sha256(nonce:cnonce:apikey))`), renewed when a call
//! answers "not logged in", NZBs added to the category's group with a priority, the queue
//! with its last 30 finished jobs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::cause;
use crate::{DownloadError, DownloadItem, ItemStatus};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub url_base: String,
    pub api_key: String,
    pub category: Option<String>,
    /// `NzbVortexPriority`: -1 low, 0 normal, 1 high.
    pub recent_priority: i64,
    pub older_priority: i64,
    /// Upstream always uses HTTPS; `useSsl: false` is for tests against plain HTTP.
    pub use_ssl: bool,
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
            None | Some(Value::Null) => 4321,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        let priority = |k: &str| {
            map.get(k)
                .and_then(Value::as_i64)
                .filter(|p| (-1..=1).contains(p))
                .unwrap_or(0)
        };
        Ok(Self {
            host,
            port,
            url_base: text(map, "urlBase").unwrap_or_default(),
            api_key: text(map, "apiKey").ok_or("the API key is required")?,
            category: text(map, "tvCategory"),
            recent_priority: priority("recentTvPriority"),
            older_priority: priority("olderTvPriority"),
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(true),
        })
    }

    fn api(&self, path: &str) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let base = self.url_base.trim_matches('/');
        let base = if base.is_empty() {
            String::new()
        } else {
            format!("/{base}")
        };
        format!("{scheme}://{}:{}{base}/api/{path}", self.host, self.port)
    }
}

/// Case-insensitive member (Newtonsoft matches names that way).
fn get<'v>(v: &'v Value, key: &str) -> &'v Value {
    v.as_object()
        .and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)))
        .map_or(&Value::Null, |(_, v)| v)
}

fn result_type(v: &Value) -> String {
    get(v, "result")
        .as_str()
        .unwrap_or_default()
        .replace('_', "")
        .to_ascii_lowercase()
}

/// `NzbVortexStateType`.
pub mod state {
    pub const WAITING: i64 = 0;
    pub const PASSWORD_REQUEST: i64 = 5;
    pub const DONE: i64 = 20;
    pub const UNCOMPRESS_FAILED: i64 = 21;
    pub const CHECK_FAILED_DATA_CORRUPT: i64 = 22;
    pub const BADLY_ENCODED: i64 = 24;
}

/// A queue entry (`NzbVortexQueueItem`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueueItem {
    pub id: i64,
    pub ui_title: String,
    pub destination_path: String,
    pub is_paused: bool,
    pub state: i64,
    pub downloaded_size: i64,
    pub total_download_size: i64,
    pub add_uuid: Option<String>,
    pub group_name: String,
}

impl QueueItem {
    pub fn from_value(v: &Value) -> Self {
        let s = |k: &str| get(v, k).as_str().unwrap_or_default().to_string();
        let n = |k: &str| get(v, k).as_i64().unwrap_or(0);
        QueueItem {
            id: n("id"),
            ui_title: s("uiTitle"),
            destination_path: s("destinationPath"),
            is_paused: get(v, "isPaused").as_bool().unwrap_or(false),
            state: n("state"),
            downloaded_size: n("downloadedSize"),
            total_download_size: n("totalDownloadSize"),
            add_uuid: get(v, "addUUID")
                .as_str()
                .filter(|u| !u.is_empty())
                .map(str::to_string),
            group_name: s("groupName"),
        }
    }

    /// `GetItems`' mapping (the output path is filled in by the caller).
    pub fn to_item(&self) -> DownloadItem {
        let status = if self.is_paused {
            ItemStatus::Paused
        } else {
            match self.state {
                state::WAITING => ItemStatus::Queued,
                state::DONE => ItemStatus::Completed,
                state::UNCOMPRESS_FAILED
                | state::CHECK_FAILED_DATA_CORRUPT
                | state::BADLY_ENCODED => ItemStatus::Failed,
                _ => ItemStatus::Downloading,
            }
        };
        DownloadItem {
            download_id: self.add_uuid.clone().unwrap_or_else(|| self.id.to_string()),
            title: self.ui_title.clone(),
            category: self.group_name.clone(),
            total_size: self.total_download_size,
            remaining_size: self.total_download_size - self.downloaded_size,
            remaining_seconds: (status == ItemStatus::Completed).then_some(0),
            status,
            message: (self.state == state::PASSWORD_REQUEST)
                .then(|| "The download is encrypted (password requested)".to_string()),
            output_path: None,
            seed_ratio: 0.0,
            can_be_removed: true,
            seed_progress: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct NzbVortex {
    http: reqwest::Client,
    sessions: Arc<Mutex<HashMap<String, String>>>,
}

impl NzbVortex {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            sessions: Arc::default(),
        }
    }

    async fn json(&self, req: reqwest::RequestBuilder) -> Result<Value, DownloadError> {
        let resp = req
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        if !resp.status().is_success() {
            return Err(DownloadError::Http {
                status: resp.status().as_u16(),
            });
        }
        resp.json()
            .await
            .map_err(|e| DownloadError::Client(format!("NZBVortex's response is unreadable: {e}")))
    }

    /// `AuthenticateClient`: the nonce handshake.
    async fn session(&self, s: &Settings, renew: bool) -> Result<String, DownloadError> {
        let key = format!("{}:{}", s.api(""), s.api_key);
        if !renew
            && let Some(id) = self
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
        {
            return Ok(id.clone());
        }
        let nonce = self.json(self.http.get(s.api("auth/nonce"))).await?;
        let nonce = get(&nonce, "authNonce")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let cnonce = uuid::Uuid::new_v4().to_string();
        let hash = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(
            format!("{nonce}:{cnonce}:{}", s.api_key).as_bytes(),
        ));
        let auth = self
            .json(self.http.get(s.api("auth/login")).query(&[
                ("nonce", nonce.as_str()),
                ("cnonce", cnonce.as_str()),
                ("hash", hash.as_str()),
            ]))
            .await?;
        let login = get(&auth, "loginResult")
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if login == "failed" {
            return Err(DownloadError::Authentication);
        }
        let id = get(&auth, "sessionId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, id.clone());
        Ok(id)
    }

    /// `ProcessRequest` with authentication: retried once after "not logged in".
    async fn call(
        &self,
        s: &Settings,
        build: impl Fn(&str) -> reqwest::RequestBuilder,
    ) -> Result<Value, DownloadError> {
        for attempt in 0..2 {
            let session = self.session(s, attempt > 0).await?;
            let v = self.json(build(&session)).await?;
            if result_type(&v) != "notloggedin" {
                return Ok(v);
            }
        }
        Err(DownloadError::Client(
            "Unable to remain authenticated to NZBVortex".into(),
        ))
    }

    pub async fn queue(
        &self,
        s: &Settings,
        done_limit: i64,
    ) -> Result<Vec<QueueItem>, DownloadError> {
        let v = self
            .call(s, |session| {
                let mut q = vec![
                    ("limitDone", done_limit.to_string()),
                    ("sessionid", session.to_string()),
                ];
                if let Some(c) = &s.category {
                    q.push(("groupName", c.clone()));
                }
                self.http.get(s.api("nzb")).query(&q)
            })
            .await?;
        Ok(get(&v, "nzbs")
            .as_array()
            .map(|a| a.iter().map(QueueItem::from_value).collect())
            .unwrap_or_default())
    }

    async fn files(&self, s: &Settings, id: i64) -> Result<Vec<String>, DownloadError> {
        let v = self
            .call(s, |session| {
                self.http
                    .get(s.api(&format!("file/{id}")))
                    .query(&[("sessionid", session)])
            })
            .await?;
        Ok(get(&v, "files")
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| get(f, "fileName").as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `GetItems` with `GetOutputPath`: the destination when it is named for the job, else
    /// (once done) its first file, with a warning when there are several.
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let mut out = Vec::new();
        for q in self.queue(s, 30).await? {
            let mut item = q.to_item();
            let dest = q.destination_path.trim_end_matches(['/', '\\']);
            if dest.rsplit(['/', '\\']).next() == Some(q.ui_title.as_str()) {
                item.output_path = Some(dest.to_string());
            } else if q.state == state::DONE {
                let files = self.files(s, q.id).await?;
                if files.len() > 1 {
                    item.status = ItemStatus::Warning;
                    item.message = Some(format!(
                        "Download contains multiple files and is not in a job folder: {dest}"
                    ));
                }
                item.output_path = files.first().map(|f| format!("{dest}/{f}"));
            }
            out.push(item);
        }
        Ok(out)
    }

    /// `DownloadNzb`: returns the job's `add_uuid`.
    pub async fn add(
        &self,
        s: &Settings,
        name: &str,
        content: &[u8],
        recent: bool,
    ) -> Result<String, DownloadError> {
        let priority = if recent {
            s.recent_priority
        } else {
            s.older_priority
        };
        let v = self
            .call(s, |session| {
                let part = reqwest::multipart::Part::bytes(content.to_vec())
                    .file_name(name.to_string())
                    .mime_str("application/x-nzb")
                    .unwrap_or_else(|_| reqwest::multipart::Part::bytes(Vec::new()));
                let mut q = vec![
                    ("priority", priority.to_string()),
                    ("sessionid", session.to_string()),
                ];
                if let Some(c) = &s.category {
                    q.push(("groupname", c.clone()));
                }
                self.http
                    .post(s.api("nzb/add"))
                    .query(&q)
                    .multipart(reqwest::multipart::Form::new().part("name", part))
            })
            .await?;
        get(&v, "add_uuid")
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .ok_or_else(|| DownloadError::Client(format!("Failed to add nzb {name}")))
    }

    /// `RemoveItem`: by numeric id, else the queue entry with the add id.
    pub async fn remove(
        &self,
        s: &Settings,
        download_id: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        let id = match download_id.parse::<i64>() {
            Ok(id) => Some(id),
            Err(_) => self
                .queue(s, 30)
                .await?
                .into_iter()
                .find(|q| q.add_uuid.as_deref() == Some(download_id))
                .map(|q| q.id),
        };
        let Some(id) = id else { return Ok(()) };
        let action = if delete_data {
            "cancelDelete"
        } else {
            "cancel"
        };
        self.call(s, |session| {
            self.http
                .get(s.api(&format!("nzb/{id}/{action}")))
                .query(&[("sessionid", session)])
        })
        .await
        .map(|_| ())
    }

    /// `Test`: the version, API level 2.3 or newer, the API key and the category's group.
    pub async fn test(&self, s: &Settings) -> Result<(String, Vec<String>), DownloadError> {
        let version = self.json(self.http.get(s.api("app/appversion"))).await?;
        let version = get(&version, "version")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let level = self.json(self.http.get(s.api("app/apilevel"))).await?;
        let level = get(&level, "apiLevel")
            .as_str()
            .unwrap_or("0.0")
            .to_string();
        let parts: Vec<u64> = level.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        if parts.as_slice() < [2, 3].as_slice() {
            return Err(DownloadError::Unsupported(format!(
                "NZBVortex API level 2.3 or newer is required; it reports {level}"
            )));
        }
        self.queue(s, 1).await?;
        let mut warnings = Vec::new();
        if let Some(c) = &s.category {
            let groups = self
                .call(s, |session| {
                    self.http
                        .get(s.api("group"))
                        .query(&[("sessionid", session)])
                })
                .await?;
            let found = get(&groups, "groups").as_array().is_some_and(|g| {
                g.iter()
                    .any(|x| get(x, "groupName").as_str() == Some(c.as_str()))
            });
            if !found {
                warnings.push(format!(
                    "The group {c:?} does not exist in NZBVortex; create it so downloads are kept apart."
                ));
            }
        }
        Ok((version, warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queue_items_map_as_upstream() {
        let item = |state: i64, paused: bool| {
            QueueItem::from_value(&json!({
                "id": 7, "uiTitle": "Show.S01E01", "destinationPath": "/dl/Show.S01E01",
                "isPaused": paused, "state": state, "downloadedSize": 40, "totalDownloadSize": 100,
                "addUUID": "uuid-1", "groupName": "TV Shows"
            }))
            .to_item()
        };
        assert_eq!(item(0, false).status, ItemStatus::Queued);
        assert_eq!(item(1, false).status, ItemStatus::Downloading);
        assert_eq!(item(20, false).status, ItemStatus::Completed);
        assert_eq!(item(20, false).remaining_seconds, Some(0));
        assert_eq!(item(21, false).status, ItemStatus::Failed);
        assert_eq!(item(24, false).status, ItemStatus::Failed);
        assert_eq!(item(1, true).status, ItemStatus::Paused);
        assert!(item(5, false).message.is_some());
        let i = item(1, false);
        assert_eq!((i.download_id.as_str(), i.remaining_size), ("uuid-1", 60));
        assert_eq!(
            result_type(&json!({"result": "not_logged_in"})),
            "notloggedin"
        );
    }
}
