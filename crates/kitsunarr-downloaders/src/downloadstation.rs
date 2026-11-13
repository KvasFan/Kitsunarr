//! Synology Download Station, for torrents and for NZBs (Sonarr's `TorrentDownloadStation`,
//! `UsenetDownloadStation` and their DiskStation proxies): the APIs are discovered through
//! `SYNO.API.Info`, a session id comes from `SYNO.API.Auth`, tasks go through
//! `SYNO.DownloadStation2.Task` (or the older `SYNO.DownloadStation.Task`), download ids are
//! `<hashed serial number>:<task id>`, and a finished task's shared-folder path is resolved to
//! the real path with `SYNO.FileStation.List`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use sha1::{Digest, Sha1};

use crate::error::cause;
use crate::{DownloadError, DownloadItem, ItemStatus, Source};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub username: Option<String>,
    pub password: Option<String>,
    pub category: Option<String>,
    /// A shared folder path (`TvDirectory`), without the leading `/`.
    pub directory: Option<String>,
    /// `BT` or `NZB`.
    pub usenet: bool,
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl Settings {
    pub fn from_map(map: &Map<String, Value>, usenet: bool) -> Result<Self, String> {
        let host = text(map, "host").unwrap_or_else(|| "127.0.0.1".into());
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 5000,
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
            username: text(map, "username"),
            password: text(map, "password"),
            category: text(map, "tvCategory"),
            directory: text(map, "tvDirectory").map(|d| d.trim_start_matches('/').to_string()),
            usenet,
        })
    }

    fn base(&self) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        format!("{scheme}://{}:{}/webapi/", self.host, self.port)
    }

    fn key(&self) -> String {
        format!(
            "{}@{}:{}",
            self.username.clone().unwrap_or_default(),
            self.host,
            self.port
        )
    }
}

/// `SYNO.API.Info` entry.
#[derive(Debug, Clone, PartialEq)]
struct ApiInfo {
    path: String,
    min: i64,
    max: i64,
}

const APIS: &[&str] = &[
    "SYNO.API.Auth",
    "SYNO.DownloadStation.Task",
    "SYNO.DownloadStation2.Task",
    "SYNO.DownloadStation.Info",
    "SYNO.DSM.Info",
    "SYNO.FileStation.List",
];

/// A number Synology may send as a string.
fn int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())
}

/// `DownloadStationTaskStatus`, by name (`filehosting_waiting`) or by index (DS2).
pub fn status_name(v: &Value) -> String {
    const NAMES: [&str; 12] = [
        "unknown",
        "waiting",
        "downloading",
        "paused",
        "finishing",
        "finished",
        "hash_checking",
        "seeding",
        "filehosting_waiting",
        "extracting",
        "error",
        "captcha_needed",
    ];
    match v {
        Value::String(s) => s.to_ascii_lowercase(),
        Value::Number(n) => n
            .as_u64()
            .and_then(|i| NAMES.get(i as usize))
            .unwrap_or(&"unknown")
            .to_string(),
        _ => "unknown".into(),
    }
}

/// A task as the clients read it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Task {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub size: i64,
    pub status: String,
    pub destination: String,
    pub uri: String,
    pub downloaded: Option<i64>,
    pub uploaded: Option<i64>,
    pub speed: i64,
    pub unzip_progress: Option<i64>,
    pub error_detail: Option<String>,
}

impl Task {
    pub fn from_value(t: &Value) -> Self {
        let detail = &t["additional"]["detail"];
        let transfer = &t["additional"]["transfer"];
        Task {
            id: t["id"].as_str().unwrap_or_default().to_string(),
            kind: t["type"].as_str().unwrap_or_default().to_ascii_lowercase(),
            title: t["title"].as_str().unwrap_or_default().to_string(),
            size: int(&t["size"]).unwrap_or(0),
            status: status_name(&t["status"]),
            destination: detail["destination"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            uri: detail["uri"].as_str().unwrap_or_default().to_string(),
            downloaded: int(&transfer["size_downloaded"]),
            uploaded: int(&transfer["size_uploaded"]),
            speed: int(&transfer["speed_download"]).unwrap_or(0),
            unzip_progress: int(&t["status_extra"]["unzip_progress"]),
            error_detail: t["status_extra"]["error_detail"]
                .as_str()
                .map(str::to_string),
        }
    }

    fn remaining(&self) -> i64 {
        self.size - self.downloaded.unwrap_or(0).max(0)
    }

    /// `GetStatus`.
    pub fn item_status(&self) -> ItemStatus {
        match self.status.as_str() {
            "unknown" | "waiting" | "filehosting_waiting" => {
                if self.size == 0 || self.remaining() > 0 {
                    ItemStatus::Queued
                } else {
                    ItemStatus::Completed
                }
            }
            "paused" => ItemStatus::Paused,
            "finished" | "seeding" => ItemStatus::Completed,
            "error" => ItemStatus::Failed,
            _ => ItemStatus::Downloading,
        }
    }

    /// `GetMessage`.
    pub fn message(&self) -> Option<String> {
        match self.status.as_str() {
            "extracting" => self.unzip_progress.map(|p| format!("Extracting: {p}%")),
            "error" => self.error_detail.clone(),
            _ => None,
        }
    }

    /// The task belongs to the client's directory or category.
    pub fn belongs(&self, s: &Settings) -> bool {
        let path = format!("/{}", self.destination);
        if let Some(dir) = &s.directory {
            let dir = format!("/{}", dir.trim_end_matches('/'));
            return path == dir || path.starts_with(&format!("{dir}/"));
        }
        match &s.category {
            Some(c) => path.split(['/', '\\']).any(|p| p == c),
            None => true,
        }
    }
}

type Cached<T> = Arc<Mutex<HashMap<String, (Instant, T)>>>;

fn cached<T: Clone>(c: &Cached<T>, key: &str, ttl: Duration) -> Option<T> {
    c.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .filter(|(at, _)| at.elapsed() < ttl)
        .map(|(_, v)| v.clone())
}

fn store<T>(c: &Cached<T>, key: String, v: T) {
    c.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, (Instant::now(), v));
}

#[derive(Clone, Default)]
pub struct DownloadStation {
    http: reqwest::Client,
    infos: Cached<HashMap<String, ApiInfo>>,
    sessions: Cached<String>,
    serials: Cached<String>,
    shares: Cached<String>,
}

impl DownloadStation {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            ..Default::default()
        }
    }

    async fn json(
        &self,
        req: reqwest::RequestBuilder,
        s: &Settings,
        operation: &str,
    ) -> Result<Value, DownloadError> {
        let resp = req
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        if !resp.status().is_success() {
            return Err(DownloadError::Http {
                status: resp.status().as_u16(),
            });
        }
        let v: Value = resp.json().await.map_err(|e| {
            DownloadError::Client(format!("DiskStation's answer is unreadable: {e}"))
        })?;
        if v["success"].as_bool() == Some(true) {
            return Ok(v["data"].clone());
        }
        let code = v["error"]["code"].as_i64().unwrap_or(0);
        // Session errors forget the session; 105 is a permission failure.
        if matches!(code, 105 | 106 | 107 | 119) {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&s.key());
        }
        if matches!(code, 105 | 400 | 401 | 402) {
            return Err(DownloadError::Authentication);
        }
        Err(DownloadError::Client(format!(
            "Failed to {operation}. DiskStation error code {code}"
        )))
    }

    /// `UpdateApiInfo`: the APIs' paths and versions (cached an hour).
    async fn infos(&self, s: &Settings) -> Result<HashMap<String, ApiInfo>, DownloadError> {
        let key = format!("{}:{}", s.host, s.port);
        if let Some(i) = cached(&self.infos, &key, Duration::from_secs(3600)) {
            return Ok(i);
        }
        let data = self
            .json(
                self.http.get(format!("{}query.cgi", s.base())).query(&[
                    ("api", "SYNO.API.Info"),
                    ("version", "1"),
                    ("method", "query"),
                    ("query", &APIS.join(",")),
                ]),
                s,
                "get api info",
            )
            .await?;
        let infos: HashMap<String, ApiInfo> = data
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            ApiInfo {
                                path: v["path"].as_str().unwrap_or_default().to_string(),
                                min: v["minVersion"].as_i64().unwrap_or(1),
                                max: v["maxVersion"].as_i64().unwrap_or(1),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        store(&self.infos, key, infos.clone());
        Ok(infos)
    }

    async fn info(&self, s: &Settings, api: &str) -> Result<ApiInfo, DownloadError> {
        self.infos(s).await?.get(api).cloned().ok_or_else(|| {
            DownloadError::Client(format!("Info of {api} not found on {}:{}", s.host, s.port))
        })
    }

    /// `AuthenticateClient`: a session id (kept six hours).
    async fn sid(&self, s: &Settings) -> Result<String, DownloadError> {
        if let Some(sid) = cached(&self.sessions, &s.key(), Duration::from_secs(6 * 3600)) {
            return Ok(sid);
        }
        let auth = self.info(s, "SYNO.API.Auth").await?;
        let version = if auth.max >= 7 { "6" } else { "2" };
        let data = self
            .json(
                self.http.get(format!("{}{}", s.base(), auth.path)).query(&[
                    ("api", "SYNO.API.Auth"),
                    ("version", version),
                    ("method", "login"),
                    ("account", s.username.as_deref().unwrap_or_default()),
                    ("passwd", s.password.as_deref().unwrap_or_default()),
                    ("format", "sid"),
                    ("session", "DownloadStation"),
                ]),
                s,
                "login",
            )
            .await?;
        let sid = data["sid"].as_str().unwrap_or_default().to_string();
        store(&self.sessions, s.key(), sid.clone());
        Ok(sid)
    }

    async fn get(
        &self,
        s: &Settings,
        api: &str,
        method: &str,
        version: i64,
        query: &[(&str, String)],
    ) -> Result<Value, DownloadError> {
        let info = self.info(s, api).await?;
        let sid = self.sid(s).await?;
        let mut q: Vec<(&str, String)> = vec![
            ("_sid", sid),
            ("api", api.to_string()),
            ("version", version.to_string()),
            ("method", method.to_string()),
        ];
        q.extend(query.iter().cloned());
        self.json(
            self.http
                .get(format!("{}{}", s.base(), info.path))
                .query(&q),
            s,
            method,
        )
        .await
    }

    /// `DownloadStationTaskProxySelector`: DS2 when the NAS has it.
    async fn v2(&self, s: &Settings) -> Result<bool, DownloadError> {
        let infos = self.infos(s).await?;
        if infos.contains_key("SYNO.DownloadStation2.Task") {
            Ok(true)
        } else if infos.contains_key("SYNO.DownloadStation.Task") {
            Ok(false)
        } else {
            Err(DownloadError::Client(
                "Unable to determine DownloadStation's task API version".into(),
            ))
        }
    }

    /// `GetTasks` of the client's kind.
    pub async fn tasks(&self, s: &Settings) -> Result<Vec<Task>, DownloadError> {
        let v2 = self.v2(s).await?;
        let list: Vec<Value> = if v2 {
            let api = "SYNO.DownloadStation2.Task";
            let detail = self
                .get(s, api, "list", 1, &[("additional", "detail".into())])
                .await?;
            let transfer = self
                .get(s, api, "list", 1, &[("additional", "transfer".into())])
                .await?;
            let transfers = transfer["task"].as_array().cloned().unwrap_or_default();
            detail["task"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|mut t| {
                    if let Some(x) = transfers.iter().find(|x| x["id"] == t["id"]) {
                        t["additional"]["transfer"] = x["additional"]["transfer"].clone();
                    }
                    t
                })
                .collect()
        } else {
            let data = self
                .get(
                    s,
                    "SYNO.DownloadStation.Task",
                    "list",
                    1,
                    &[("additional", "detail,transfer".into())],
                )
                .await?;
            data["tasks"].as_array().cloned().unwrap_or_default()
        };
        let kind = if s.usenet { "nzb" } else { "bt" };
        Ok(list
            .iter()
            .map(Task::from_value)
            .filter(|t| t.kind == kind)
            .collect())
    }

    /// `SerialNumberProvider`: the NAS serial number, hashed (kept five minutes).
    async fn serial(&self, s: &Settings) -> Result<String, DownloadError> {
        if let Some(x) = cached(&self.serials, &s.host, Duration::from_secs(300)) {
            return Ok(x);
        }
        let info = self.info(s, "SYNO.DSM.Info").await?;
        let data = self
            .get(s, "SYNO.DSM.Info", "getinfo", info.min, &[])
            .await?;
        let hashed = hex::encode(Sha1::digest(
            data["serial"].as_str().unwrap_or_default().as_bytes(),
        ));
        store(&self.serials, s.host.clone(), hashed.clone());
        Ok(hashed)
    }

    /// `SharedFolderResolver.RemapToFullPath`: the shared folder's real path plus the rest.
    async fn full_path(
        &self,
        s: &Settings,
        shared_path: &str,
        serial: &str,
    ) -> Result<String, DownloadError> {
        let index = shared_path[1.min(shared_path.len())..]
            .find('/')
            .map(|i| i + 1);
        let (share, rest) = match index {
            Some(i) => (&shared_path[..i], &shared_path[i..]),
            None => (shared_path, ""),
        };
        let key = format!("{serial}:{share}");
        let real = match cached(&self.shares, &key, Duration::from_secs(3600)) {
            Some(r) => r,
            None => {
                let data = self
                    .get(
                        s,
                        "SYNO.FileStation.List",
                        "getinfo",
                        2,
                        &[
                            ("path", serde_json::to_string(&[share]).unwrap_or_default()),
                            ("additional", "[\"real_path\"]".into()),
                        ],
                    )
                    .await?;
                let real = data["files"][0]["additional"]["real_path"]
                    .as_str()
                    .unwrap_or(share)
                    .to_string();
                store(&self.shares, key, real.clone());
                real
            }
        };
        Ok(format!("{}{rest}", real.trim_end_matches('/')))
    }

    /// `GetItems` (Usenet tasks share the global speed for the time left, as upstream).
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let tasks = self.tasks(s).await?;
        let serial = self.serial(s).await?;
        let global_speed: i64 = tasks
            .iter()
            .filter(|t| t.status == "downloading")
            .map(|t| t.speed)
            .sum();
        let mut total_remaining = 0i64;
        let mut out = Vec::new();
        for t in &tasks {
            if t.status != "paused" {
                total_remaining += t.remaining();
            }
            if !t.belongs(s) {
                continue;
            }
            let status = t.item_status();
            let remaining_seconds = if s.usenet {
                (status != ItemStatus::Paused && global_speed > 0)
                    .then(|| total_remaining / global_speed)
            } else {
                (t.speed > 0).then(|| t.remaining() / t.speed)
            };
            let output_path = if matches!(status, ItemStatus::Completed | ItemStatus::Failed) {
                let full = self
                    .full_path(s, &format!("/{}", t.destination), &serial)
                    .await?;
                Some(format!("{full}/{}", t.title))
            } else {
                None
            };
            out.push(DownloadItem {
                download_id: format!("{serial}:{}", t.id),
                title: t.title.clone(),
                category: s.category.clone().unwrap_or_default(),
                total_size: t.size,
                remaining_size: t.remaining(),
                remaining_seconds,
                status,
                message: t.message(),
                output_path,
                seed_ratio: match (t.downloaded, t.uploaded) {
                    (Some(d), Some(u)) if d > 0 => u as f64 / d as f64,
                    _ => 0.0,
                },
                // Torrents once finished; NZBs always.
                can_be_removed: s.usenet || t.status == "finished",
                seed_progress: None,
            });
        }
        Ok(out)
    }

    /// `GetDownloadDirectory`: the directory, else the default destination plus category.
    async fn directory(&self, s: &Settings) -> Result<Option<String>, DownloadError> {
        if let Some(d) = &s.directory {
            return Ok(Some(d.clone()));
        }
        let config = self
            .get(s, "SYNO.DownloadStation.Info", "getConfig", 1, &[])
            .await?;
        let dest = config["default_destination"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        Ok(Some(match &s.category {
            Some(c) if !dest.is_empty() => format!("{dest}/{c}"),
            _ => dest,
        })
        .filter(|d| !d.is_empty()))
    }

    async fn create(&self, s: &Settings, source: &Source<'_>) -> Result<(), DownloadError> {
        let dir = self.directory(s).await?;
        let v2 = self.v2(s).await?;
        match source {
            Source::Magnet(url) => {
                let mut q: Vec<(&str, String)> = if v2 {
                    vec![
                        ("type", "url".into()),
                        ("url", url.to_string()),
                        ("create_list", "false".into()),
                    ]
                } else {
                    vec![("uri", url.to_string())]
                };
                if let Some(d) = &dir {
                    q.push(("destination", d.clone()));
                }
                let (api, version) = if v2 {
                    ("SYNO.DownloadStation2.Task", 2)
                } else {
                    ("SYNO.DownloadStation.Task", 3)
                };
                self.get(s, api, "create", version, &q).await.map(|_| ())
            }
            Source::File { name, content } => {
                let api = if v2 {
                    "SYNO.DownloadStation2.Task"
                } else {
                    "SYNO.DownloadStation.Task"
                };
                let info = self.info(s, api).await?;
                let sid = self.sid(s).await?;
                let part =
                    reqwest::multipart::Part::bytes(content.to_vec()).file_name(name.to_string());
                let mut form = reqwest::multipart::Form::new()
                    .text("api", api)
                    .text("version", "2")
                    .text("method", "create");
                let url = format!("{}{}", s.base(), info.path);
                let req = if v2 {
                    form = form
                        .text("type", "\"file\"")
                        .text("file", "[\"fileData\"]")
                        .text("create_list", "false");
                    if let Some(d) = &dir {
                        form = form.text("destination", format!("\"{d}\""));
                    }
                    self.http
                        .post(url)
                        .query(&[("_sid", sid)])
                        .multipart(form.part("fileData", part))
                } else {
                    form = form.text("_sid", sid);
                    if let Some(d) = &dir {
                        form = form.text("destination", d.clone());
                    }
                    self.http.post(url).multipart(form.part("file", part))
                };
                self.json(req, s, "add task from data").await.map(|_| ())
            }
        }
    }

    /// `AddFromMagnetLink`/`AddFromTorrentFile`/`AddFromNzbFile`: the task is found again by
    /// its uri (the magnet, or the file name — without `.torrent` for torrents).
    pub async fn add(&self, s: &Settings, source: &Source<'_>) -> Result<String, DownloadError> {
        let serial = self.serial(s).await?;
        self.create(s, source).await?;
        let uri = match source {
            Source::Magnet(url) => url.to_string(),
            Source::File { name, .. } if s.usenet => name.to_string(),
            Source::File { name, .. } => name.trim_end_matches(".torrent").to_string(),
        };
        let tasks = self.tasks(s).await?;
        let found: Vec<&Task> = tasks.iter().filter(|t| t.uri == uri).collect();
        match found.as_slice() {
            [one] => Ok(format!("{serial}:{}", one.id)),
            _ => Err(DownloadError::Client(
                "Failed to add the task to Download Station".into(),
            )),
        }
    }

    /// `RemoveItem`: the data is deleted by the caller (`DeleteItemData`).
    pub async fn remove(&self, s: &Settings, download_id: &str) -> Result<(), DownloadError> {
        let id = download_id
            .split(':')
            .nth(1)
            .unwrap_or(download_id)
            .to_string();
        if self.v2(s).await? {
            self.get(
                s,
                "SYNO.DownloadStation2.Task",
                "delete",
                2,
                &[("id", id), ("force_complete", "false".into())],
            )
            .await
        } else {
            self.get(
                s,
                "SYNO.DownloadStation.Task",
                "delete",
                1,
                &[("id", id), ("force_complete", "false".into())],
            )
            .await
        }
        .map(|_| ())
    }

    /// `Test`: task API version 2 supported, an output folder that exists, the tasks.
    pub async fn test(&self, s: &Settings) -> Result<(), DownloadError> {
        let infos = self.infos(s).await?;
        let task = infos
            .get("SYNO.DownloadStation2.Task")
            .or_else(|| infos.get("SYNO.DownloadStation.Task"))
            .ok_or_else(|| DownloadError::Client("Download Station is not installed".into()))?;
        if task.min > 2 || task.max < 2 {
            return Err(DownloadError::Unsupported(format!(
                "Download Station API version 2 is required (it supports {} to {})",
                task.min, task.max
            )));
        }
        let Some(dir) = self.directory(s).await? else {
            return Err(DownloadError::Client(format!(
                "There is no default destination; set a directory, or log in to Download Station as {} and set one",
                s.username.clone().unwrap_or_default()
            )));
        };
        let data = self
            .get(
                s,
                "SYNO.FileStation.List",
                "getinfo",
                2,
                &[
                    (
                        "path",
                        serde_json::to_string(&[format!("/{dir}")]).unwrap_or_default(),
                    ),
                    ("additional", "[\"real_path\"]".into()),
                ],
            )
            .await?;
        let file = &data["files"][0];
        if file["additional"].is_null() {
            return Err(DownloadError::Client(format!(
                "The shared folder of {dir} does not exist"
            )));
        }
        if file["isdir"].as_bool() == Some(false) {
            return Err(DownloadError::Client(format!("{dir} is not a folder")));
        }
        self.items(s).await.map(|_| ())
    }
}

impl DownloadStation {
    /// `GetStatus`: the download folder as the NAS's real path.
    pub async fn status(&self, s: &Settings) -> Result<crate::ClientStatus, DownloadError> {
        let serial = self.serial(s).await?;
        let shared = format!("/{}", self.directory(s).await?.unwrap_or_default());
        let full = self.full_path(s, &shared, &serial).await?;
        Ok(crate::ClientStatus {
            is_localhost: crate::is_localhost(&s.host),
            output_root_folders: vec![full],
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(status: Value, downloaded: Value) -> Task {
        Task::from_value(&json!({
            "id": "dbid_1", "type": "BT", "title": "Show.S01E01", "size": "1000", "status": status,
            "status_extra": {"unzip_progress": "40", "error_detail": "broken_link"},
            "additional": {"detail": {"destination": "downloads/tv", "uri": "magnet:?xt=x"},
                "transfer": {"size_downloaded": downloaded, "size_uploaded": "500", "speed_download": "100"}}
        }))
    }

    #[test]
    fn tasks_map_as_upstream() {
        let t = task(json!("finished"), json!("1000"));
        assert_eq!(
            (t.kind.as_str(), t.size, t.destination.as_str()),
            ("bt", 1000, "downloads/tv")
        );
        assert_eq!(t.item_status(), ItemStatus::Completed);
        assert_eq!(
            task(json!("waiting"), json!("0")).item_status(),
            ItemStatus::Queued
        );
        assert_eq!(
            task(json!("waiting"), json!("1000")).item_status(),
            ItemStatus::Completed
        );
        assert_eq!(
            task(json!(3), json!("0")).item_status(),
            ItemStatus::Paused,
            "DS2 numbers"
        );
        assert_eq!(
            task(json!("seeding"), json!("1000")).item_status(),
            ItemStatus::Completed
        );
        assert_eq!(
            task(json!("error"), json!("0")).item_status(),
            ItemStatus::Failed
        );
        assert_eq!(
            task(json!("hash_checking"), json!("0")).item_status(),
            ItemStatus::Downloading
        );
        assert_eq!(
            task(json!("extracting"), json!("0")).message().as_deref(),
            Some("Extracting: 40%")
        );
        assert_eq!(
            task(json!("error"), json!("0")).message().as_deref(),
            Some("broken_link")
        );
        let mut s = Settings::from_map(&Map::new(), false).unwrap();
        assert!(t.belongs(&s));
        s.category = Some("tv".into());
        assert!(t.belongs(&s));
        s.category = Some("movies".into());
        assert!(!t.belongs(&s));
        s.directory = Some("downloads".into());
        assert!(t.belongs(&s), "the directory wins over the category");
        s.directory = Some("other".into());
        assert!(!t.belongs(&s));
    }
}
