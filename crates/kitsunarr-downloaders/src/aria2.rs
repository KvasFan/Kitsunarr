//! Aria2 over XML-RPC (Sonarr's `Aria2`, `Aria2Proxy`): the secret token as first argument,
//! torrents added by magnet (`aria2.addUri`) or file (`aria2.addTorrent`) into the optional
//! directory, and listed from the active, waiting and stopped queues. Aria2 cannot delete
//! data, so Kitsunarr removes the files itself.

use std::time::Duration;

use serde_json::{Map, Value as Json};

use crate::error::cause;
use crate::xmlrpc::{self, Arg, Value};
use crate::{DownloadError, DownloadItem, ItemStatus, Source};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub rpc_path: String,
    pub use_ssl: bool,
    pub secret_token: String,
    pub directory: Option<String>,
}

fn text(map: &Map<String, Json>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl Settings {
    pub fn from_map(map: &Map<String, Json>) -> Result<Self, String> {
        let host = text(map, "host").unwrap_or_else(|| "localhost".into());
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        let port = match map.get("port") {
            None | Some(Json::Null) => 6800,
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
            rpc_path: text(map, "rpcPath").unwrap_or_else(|| "/rpc".into()),
            use_ssl: map.get("useSsl").and_then(Json::as_bool).unwrap_or(false),
            secret_token: text(map, "secretToken").unwrap_or_default(),
            directory: text(map, "directory"),
        })
    }

    fn url(&self) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        format!(
            "{scheme}://{}:{}/{}",
            self.host,
            self.port,
            self.rpc_path.trim_start_matches('/')
        )
    }

    fn token(&self) -> String {
        format!("token:{}", self.secret_token)
    }
}

/// A download from `aria2.tell*` (`Aria2Status`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub gid: String,
    pub info_hash: String,
    pub status: String,
    pub name: String,
    pub completed_length: i64,
    pub total_length: i64,
    pub upload_length: i64,
    pub download_speed: i64,
    pub error_message: Option<String>,
    pub files: Vec<String>,
}

fn find_name(v: &Value) -> Option<String> {
    match v {
        Value::Struct(members) => members.iter().find_map(|(k, v)| {
            if k == "name" {
                v.as_str().map(str::to_string)
            } else {
                find_name(v)
            }
        }),
        _ => None,
    }
}

impl Status {
    pub fn from_value(v: &Value) -> Self {
        let s = |k: &str| {
            v.member(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let n = |k: &str| v.member(k).and_then(Value::as_i64).unwrap_or(0);
        Status {
            gid: s("gid"),
            info_hash: s("infoHash"),
            status: s("status"),
            name: v
                .member("bittorrent")
                .and_then(find_name)
                .unwrap_or_default(),
            completed_length: n("completedLength"),
            total_length: n("totalLength"),
            upload_length: n("uploadLength"),
            download_speed: n("downloadSpeed"),
            error_message: v
                .member("errorMessage")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            files: v
                .member("files")
                .map(|f| {
                    f.as_array()
                        .iter()
                        .filter_map(|x| x.member("path").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// `GetOutputPath`: the one file, or the files' longest common folder.
    pub fn output_path(&self) -> String {
        match self.files.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [first, rest @ ..] => {
                let mut len = first.len();
                for p in rest {
                    len = len.min(p.len());
                    if let Some(i) = first
                        .bytes()
                        .zip(p.bytes())
                        .take(len)
                        .position(|(a, b)| a != b)
                    {
                        len = i;
                    }
                }
                let common = &first[..len];
                common
                    .rfind(['/', '\\'])
                    .map(|i| common[..i].to_string())
                    .unwrap_or_default()
            }
        }
    }

    /// `GetItems`' mapping; `None` for metadata downloads and removed ones.
    pub fn to_item(&self) -> Option<DownloadItem> {
        if self.files.first().is_some_and(|f| f.contains("[METADATA]")) || self.status == "removed"
        {
            return None;
        }
        let status = match self.status.as_str() {
            "active" if self.completed_length == self.total_length => ItemStatus::Completed,
            "active" => ItemStatus::Downloading,
            "waiting" => ItemStatus::Queued,
            "paused" => ItemStatus::Paused,
            "complete" => ItemStatus::Completed,
            _ => ItemStatus::Failed,
        };
        let remaining = self.total_length - self.completed_length;
        Some(DownloadItem {
            download_id: self.info_hash.to_ascii_uppercase(),
            title: self.name.clone(),
            category: String::new(),
            total_size: self.total_length,
            remaining_size: remaining,
            remaining_seconds: (self.download_speed > 0).then(|| remaining / self.download_speed),
            status,
            message: self.error_message.clone(),
            output_path: Some(self.output_path()),
            seed_ratio: if self.total_length > 0 {
                self.upload_length as f64 / self.total_length as f64
            } else {
                0.0
            },
            can_be_removed: self.status == "complete",
            seed_progress: None,
        })
    }
}

#[derive(Clone, Default)]
pub struct Aria2 {
    http: reqwest::Client,
}

impl Aria2 {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    async fn call(
        &self,
        s: &Settings,
        method: &str,
        args: &[Arg<'_>],
    ) -> Result<Value, DownloadError> {
        let token = s.token();
        let mut all = vec![Arg::Str(&token)];
        all.extend(args.iter().cloned());
        let resp = self
            .http
            .post(s.url())
            .header("Content-Type", "text/xml")
            .body(xmlrpc::request(method, &all))
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        match resp.status().as_u16() {
            401 | 403 => return Err(DownloadError::Authentication),
            200..=299 | 400 => {}
            status => return Err(DownloadError::Http { status }),
        }
        let body = resp
            .text()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        xmlrpc::response(&body, "Aria2")
    }

    pub async fn version(&self, s: &Settings) -> Result<String, DownloadError> {
        let v = self.call(s, "aria2.getVersion", &[]).await?;
        Ok(v.member("version")
            .and_then(Value::as_str)
            .unwrap_or("0.0.0")
            .to_string())
    }

    /// `GetTorrents`: active, waiting and stopped.
    pub async fn torrents(&self, s: &Settings) -> Result<Vec<Status>, DownloadError> {
        let mut out = Vec::new();
        for (method, paged) in [
            ("aria2.tellActive", false),
            ("aria2.tellWaiting", true),
            ("aria2.tellStopped", true),
        ] {
            let args: Vec<Arg<'_>> = if paged {
                vec![Arg::Int(0), Arg::Int(10 * 1024)]
            } else {
                Vec::new()
            };
            let v = self.call(s, method, &args).await?;
            out.extend(v.as_array().iter().map(Status::from_value));
        }
        Ok(out)
    }

    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        Ok(self
            .torrents(s)
            .await?
            .iter()
            .filter_map(Status::to_item)
            .collect())
    }

    /// `AddFromMagnetLink`/`AddFromTorrentFile`, then `WaitForTorrent` (10 × 0.5 s; only a
    /// warning when it does not show up).
    pub async fn add(
        &self,
        s: &Settings,
        source: &Source<'_>,
        hash: &str,
    ) -> Result<String, DownloadError> {
        let options: Vec<(&str, Arg<'_>)> = s
            .directory
            .as_deref()
            .map(|d| vec![("dir", Arg::Str(d))])
            .unwrap_or_default();
        let gid = match source {
            Source::Magnet(url) => {
                self.call(
                    s,
                    "aria2.addUri",
                    &[Arg::List(vec![Arg::Str(url)]), Arg::Struct(options)],
                )
                .await?
            }
            Source::File { content, .. } => {
                self.call(
                    s,
                    "aria2.addTorrent",
                    &[
                        Arg::Bytes(content),
                        Arg::List(Vec::new()),
                        Arg::Struct(options),
                    ],
                )
                .await?
            }
        };
        let gid = gid.as_str().unwrap_or_default().to_string();
        for _ in 0..10 {
            if let Ok(v) = self.call(s, "aria2.tellStatus", &[Arg::Str(&gid)]).await
                && Status::from_value(&v).info_hash.eq_ignore_ascii_case(hash)
            {
                return Ok(hash.to_ascii_uppercase());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tracing::warn!(
            "Aria2 did not show the torrent within 5 seconds; the download may stay stuck"
        );
        Ok(hash.to_ascii_uppercase())
    }

    /// `RemoveItem`: the download result of a finished one, else a forced removal; the data
    /// is deleted by the caller (`DeleteItemData`).
    pub async fn remove(&self, s: &Settings, hash: &str) -> Result<(), DownloadError> {
        let torrents = self.torrents(s).await?;
        let Some(t) = torrents
            .iter()
            .find(|t| t.info_hash.eq_ignore_ascii_case(hash))
        else {
            tracing::error!(hash, "Aria2 could not find the download to remove");
            return Ok(());
        };
        let done = matches!(t.status.as_str(), "complete" | "error" | "removed");
        let method = if done {
            "aria2.removeDownloadResult"
        } else {
            "aria2.forceRemove"
        };
        let v = self.call(s, method, &[Arg::Str(&t.gid)]).await?;
        let ok = if done {
            v.as_str() == Some("OK")
        } else {
            v.as_str() == Some(t.gid.as_str())
        };
        if !ok {
            return Err(DownloadError::Client(format!(
                "Aria2 could not remove {hash}"
            )));
        }
        Ok(())
    }

    /// `Test`: Aria2 1.34.0 or newer.
    pub async fn test(&self, s: &Settings) -> Result<String, DownloadError> {
        let version = self.version(s).await?;
        let parts: Vec<u64> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        if parts.as_slice() < [1, 34, 0].as_slice() {
            return Err(DownloadError::Unsupported(format!(
                "Aria2 1.34.0 or newer is required; it reports {version}"
            )));
        }
        Ok(version)
    }
}

impl Aria2 {
    /// `GetStatus`: aria2's global `dir`.
    pub async fn status(&self, s: &Settings) -> Result<crate::ClientStatus, DownloadError> {
        let globals = self.call(s, "aria2.getGlobalOption", &[]).await?;
        let dir = globals
            .member("dir")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(crate::ClientStatus {
            is_localhost: crate::is_localhost(&s.host),
            output_root_folders: vec![dir].into_iter().filter(|d| !d.is_empty()).collect(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: &str, done: i64, files: &[&str]) -> Status {
        Status {
            gid: "g".into(),
            info_hash: "abc".into(),
            status: state.into(),
            name: "Show".into(),
            completed_length: done,
            total_length: 100,
            upload_length: 150,
            download_speed: 10,
            error_message: None,
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn statuses_and_paths_follow_aria2() {
        let one = ["/dl/Show/e1.mkv"];
        assert_eq!(
            status("active", 100, &one).to_item().unwrap().status,
            ItemStatus::Completed
        );
        assert_eq!(
            status("active", 50, &one).to_item().unwrap().status,
            ItemStatus::Downloading
        );
        assert_eq!(
            status("waiting", 0, &one).to_item().unwrap().status,
            ItemStatus::Queued
        );
        assert_eq!(
            status("paused", 0, &one).to_item().unwrap().status,
            ItemStatus::Paused
        );
        assert_eq!(
            status("error", 0, &one).to_item().unwrap().status,
            ItemStatus::Failed
        );
        let done = status("complete", 100, &one).to_item().unwrap();
        assert!(done.can_be_removed);
        assert_eq!(done.seed_ratio, 1.5);
        assert_eq!(done.download_id, "ABC");
        assert_eq!(done.output_path.as_deref(), Some("/dl/Show/e1.mkv"));
        assert!(status("removed", 0, &one).to_item().is_none());
        assert!(status("active", 0, &["[METADATA]abc"]).to_item().is_none());
        assert_eq!(
            status(
                "complete",
                100,
                &["/dl/Show/e1.mkv", "/dl/Show/e2.mkv", "/dl/Show/Subs/e1.srt"]
            )
            .output_path(),
            "/dl/Show"
        );
        let v = Value::Struct(vec![
            ("gid".into(), Value::Str("2089b05ecca3d829".into())),
            ("infoHash".into(), Value::Str("abc".into())),
            (
                "bittorrent".into(),
                Value::Struct(vec![(
                    "info".into(),
                    Value::Struct(vec![("name".into(), Value::Str("Show.S01".into()))]),
                )]),
            ),
            ("totalLength".into(), Value::Str("100".into())),
        ]);
        let s = Status::from_value(&v);
        assert_eq!((s.name.as_str(), s.total_length), ("Show.S01", 100));
    }
}
