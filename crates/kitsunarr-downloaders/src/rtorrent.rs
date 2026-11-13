//! rTorrent over XML-RPC (Sonarr's `RTorrent`, `RTorrentProxy`): torrents are added with
//! their label (`d.custom1`), priority and directory, listed with `d.multicall2`, and imported
//! ones get the post-import label and the `kitsunarr_imported` view. rTorrent does not enforce
//! seed goals: Kitsunarr checks the grabbing indexer's (see `SeedProgress`).

use std::time::Duration;

use serde_json::{Map, Value as Json};

use crate::error::cause;
use crate::xmlrpc::{self, Arg, Value};
use crate::{DownloadError, DownloadItem, ItemStatus, SeedProgress, Source};

/// The view imported torrents join (`{appname}_imported`).
pub const IMPORTED_VIEW: &str = "kitsunarr_imported";

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    /// `UrlBase`: the XML-RPC path (`RPC2`).
    pub url_base: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub category: Option<String>,
    pub imported_category: Option<String>,
    pub directory: Option<String>,
    /// `RTorrentPriority`: 0 very low, 1 low, 2 normal, 3 high.
    pub recent_priority: i64,
    pub older_priority: i64,
    pub add_stopped: bool,
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
            None | Some(Json::Null) => 8080,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        let priority = |k: &str| {
            map.get(k)
                .and_then(Json::as_i64)
                .filter(|p| (0..=3).contains(p))
                .unwrap_or(2)
        };
        Ok(Self {
            host,
            port,
            use_ssl: map.get("useSsl").and_then(Json::as_bool).unwrap_or(false),
            url_base: match map.get("urlBase") {
                None | Some(Json::Null) => "RPC2".into(),
                Some(v) => v.as_str().unwrap_or_default().trim().to_string(),
            },
            username: text(map, "username"),
            password: text(map, "password"),
            category: text(map, "tvCategory"),
            imported_category: text(map, "tvImportedCategory"),
            directory: text(map, "tvDirectory"),
            recent_priority: priority("recentTvPriority"),
            older_priority: priority("olderTvPriority"),
            add_stopped: map
                .get("addStopped")
                .and_then(Json::as_bool)
                .unwrap_or(false),
        })
    }

    /// `XmlRpcRequestBuilder(useSsl, host, port, urlBase)`.
    pub fn url(&self) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let base = self.url_base.trim_matches('/');
        format!("{scheme}://{}:{}/{base}", self.host, self.port)
    }
}

/// A torrent from `d.multicall2` (`RTorrentTorrent`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Torrent {
    pub name: String,
    pub hash: String,
    pub path: String,
    pub category: String,
    pub total_size: i64,
    pub remaining_size: i64,
    pub down_rate: i64,
    /// Thousandths.
    pub ratio: i64,
    pub is_open: bool,
    pub is_active: bool,
    pub is_finished: bool,
    pub finished_time: i64,
}

/// `HttpUtility.UrlDecode` (ruTorrent stores labels URL-encoded).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Torrent {
    fn from_row(row: &[Value]) -> Self {
        let s = |i: usize| {
            row.get(i)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let n = |i: usize| row.get(i).and_then(Value::as_i64).unwrap_or(0);
        Torrent {
            name: s(0),
            hash: s(1),
            path: s(2),
            category: url_decode(&s(3)),
            total_size: n(4),
            remaining_size: n(5),
            down_rate: n(6),
            ratio: n(7),
            is_open: n(8) != 0,
            is_active: n(9) != 0,
            is_finished: n(10) != 0,
            finished_time: n(11),
        }
    }

    /// `GetItems`' mapping.
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
            category: self.category.clone(),
            total_size: self.total_size,
            remaining_size: self.remaining_size,
            remaining_seconds: Some(if self.down_rate > 0 {
                self.remaining_size / self.down_rate
            } else {
                0
            }),
            status,
            message: None,
            output_path: Some(self.path.clone()),
            seed_ratio: self.ratio as f64 / 1000.0,
            can_be_removed: false,
            seed_progress: self.is_finished.then_some(SeedProgress {
                ratio: self.ratio as f64 / 1000.0,
                finished_at: (self.finished_time > 0).then_some(self.finished_time),
            }),
        }
    }
}

#[derive(Clone, Default)]
pub struct RTorrent {
    http: reqwest::Client,
}

impl RTorrent {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    async fn call(
        &self,
        s: &Settings,
        method: &str,
        args: &[Arg<'_>],
    ) -> Result<Value, DownloadError> {
        let mut req = self
            .http
            .post(s.url())
            .header("Content-Type", "text/xml")
            .body(xmlrpc::request(method, args));
        if let Some(user) = &s.username {
            req = req.basic_auth(user, s.password.as_deref());
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
        let body = resp
            .text()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        xmlrpc::response(&body, "rTorrent")
    }

    /// `GetVersion` (`system.client_version`).
    pub async fn version(&self, s: &Settings) -> Result<String, DownloadError> {
        Ok(self
            .call(s, "system.client_version", &[])
            .await?
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or("0.0.0")
            .to_string())
    }

    /// `GetTorrents`.
    pub async fn torrents(&self, s: &Settings) -> Result<Vec<Torrent>, DownloadError> {
        let fields = [
            "",
            "",
            "d.name=",
            "d.hash=",
            "d.base_path=",
            "d.custom1=",
            "d.size_bytes=",
            "d.left_bytes=",
            "d.down.rate=",
            "d.ratio=",
            "d.is_open=",
            "d.is_active=",
            "d.complete=",
            "d.timestamp.finished=",
        ];
        let args: Vec<Arg<'_>> = fields.iter().map(|f| Arg::Str(f)).collect();
        let v = self.call(s, "d.multicall2", &args).await?;
        Ok(v.as_array()
            .iter()
            .map(|row| Torrent::from_row(row.as_array()))
            .collect())
    }

    /// `GetItems`: the category's torrents with an absolute download path.
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        Ok(self
            .torrents(s)
            .await?
            .iter()
            .filter(|t| s.category.as_deref().is_none_or(|c| t.category == c))
            .filter(|t| {
                let ok = !t.path.trim().is_empty() && !t.path.starts_with('.');
                if !ok {
                    tracing::warn!(torrent = %t.name, "rTorrent reports no absolute download path; not processed");
                }
                ok
            })
            .map(Torrent::to_item)
            .collect())
    }

    /// `GetCommands`.
    fn commands(s: &Settings, priority: i64) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(c) = &s.category {
            out.push(format!("d.custom1.set={c}"));
        }
        if priority != 2 {
            out.push(format!("d.priority.set={priority}"));
        }
        if let Some(d) = &s.directory {
            out.push(format!("d.directory.set={d}"));
        }
        out
    }

    /// `HasHashTorrent`: rTorrent knows the torrent (not just its `.meta` placeholder).
    async fn has(&self, s: &Settings, hash: &str) -> bool {
        match self.call(s, "d.name", &[Arg::Str(hash)]).await {
            Ok(v) => v
                .as_str()
                .is_some_and(|n| !n.trim().is_empty() && n != format!("{hash}.meta")),
            Err(_) => false,
        }
    }

    /// `AddFromMagnetLink`/`AddFromTorrentFile`, then `WaitForTorrent` (10 × 0.5 s).
    pub async fn add(
        &self,
        s: &Settings,
        source: &Source<'_>,
        hash: &str,
        recent: bool,
    ) -> Result<String, DownloadError> {
        let priority = if recent {
            s.recent_priority
        } else {
            s.older_priority
        };
        let commands = Self::commands(s, priority);
        let mut args = vec![Arg::Str("")];
        let (method, magnet) = match source {
            Source::Magnet(url) => {
                args.push(Arg::Str(url));
                (
                    if s.add_stopped {
                        "load.normal"
                    } else {
                        "load.start"
                    },
                    true,
                )
            }
            Source::File { content, .. } => {
                args.push(Arg::Bytes(content));
                (
                    if s.add_stopped {
                        "load.raw"
                    } else {
                        "load.raw_start"
                    },
                    false,
                )
            }
        };
        args.extend(commands.iter().map(|c| Arg::Str(c)));
        if self.call(s, method, &args).await?.as_i64() != Some(0) {
            return Err(DownloadError::Client(
                "rTorrent could not add the torrent".into(),
            ));
        }
        let hash = hash.to_ascii_lowercase();
        for _ in 0..10 {
            if self.has(s, &hash).await {
                return Ok(hash.to_ascii_uppercase());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if magnet {
            tracing::warn!(
                "rTorrent could not resolve the magnet within 5 seconds; the download may stay stuck"
            );
            Ok(hash.to_ascii_uppercase())
        } else {
            Err(DownloadError::Client("Downloading torrent failed".into()))
        }
    }

    /// `RemoveItem`: `d.erase` (the data is deleted by the caller beforehand).
    pub async fn remove(&self, s: &Settings, hash: &str) -> Result<(), DownloadError> {
        if self.call(s, "d.erase", &[Arg::Str(hash)]).await?.as_i64() != Some(0) {
            return Err(DownloadError::Client(format!(
                "rTorrent could not remove {hash}"
            )));
        }
        Ok(())
    }

    /// `MarkItemAsImported`: the post-import label, then the imported view (failures only
    /// warn, as upstream).
    pub async fn mark_imported(&self, s: &Settings, hash: &str) -> Result<(), DownloadError> {
        let hash = hash.to_ascii_lowercase();
        if let Some(imported) = s
            .imported_category
            .as_deref()
            .filter(|c| s.category.as_deref() != Some(*c))
        {
            match self
                .call(s, "d.custom1.set", &[Arg::Str(&hash), Arg::Str(imported)])
                .await
            {
                Ok(v) if v.as_str() == Some(imported) => {}
                Ok(_) | Err(_) => tracing::warn!(
                    label = imported,
                    "failed to set the post-import label in rTorrent; does it exist?"
                ),
            }
        }
        if let Err(e) = self
            .call(
                s,
                "d.views.push_back_unique",
                &[Arg::Str(&hash), Arg::Str(IMPORTED_VIEW)],
            )
            .await
        {
            tracing::warn!(error = %e, "failed to set the post-import view in rTorrent");
        }
        Ok(())
    }

    /// `Test`: rTorrent 0.9.0 or newer, and its torrents can be listed.
    pub async fn test(&self, s: &Settings) -> Result<String, DownloadError> {
        let version = self.version(s).await?;
        let parts: Vec<u64> = version
            .split('.')
            .map(|p| p.trim().parse().unwrap_or(0))
            .collect();
        if parts.as_slice() < [0, 9, 0].as_slice() {
            return Err(DownloadError::Unsupported(format!(
                "rTorrent 0.9.0 or newer is required; it reports {version}"
            )));
        }
        self.torrents(s).await?;
        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrents_map_as_upstream() {
        let row = |finished: i64, active: i64| {
            vec![
                Value::Str("Droned.S01E01.Pilot.1080p.WEB-DL-DRONE".into()),
                Value::Str("HASH".into()),
                Value::Str("/downloads/Droned.S01E01".into()),
                Value::Str("tv%20kitsunarr".into()),
                Value::Int(1000),
                Value::Int(if finished == 1 { 0 } else { 100 }),
                Value::Int(10),
                Value::Int(1500),
                Value::Int(1),
                Value::Int(active),
                Value::Int(finished),
                Value::Int(1_700_000_000),
            ]
        };
        let done = Torrent::from_row(&row(1, 1)).to_item();
        assert_eq!(done.status, ItemStatus::Completed);
        assert_eq!(done.category, "tv kitsunarr");
        assert_eq!(done.download_id, "HASH");
        assert_eq!(done.seed_ratio, 1.5);
        assert_eq!(
            done.seed_progress,
            Some(SeedProgress {
                ratio: 1.5,
                finished_at: Some(1_700_000_000)
            })
        );
        let downloading = Torrent::from_row(&row(0, 1)).to_item();
        assert_eq!(downloading.status, ItemStatus::Downloading);
        assert_eq!(downloading.remaining_seconds, Some(10));
        assert!(downloading.seed_progress.is_none());
        assert_eq!(
            Torrent::from_row(&row(0, 0)).to_item().status,
            ItemStatus::Paused
        );
        let s = Settings::from_map(&Map::new()).unwrap();
        assert_eq!(s.url(), "http://localhost:8080/RPC2");
        assert_eq!(RTorrent::commands(&s, 2), Vec::<String>::new());
        let s = Settings {
            category: Some("tv".into()),
            directory: Some("/dl".into()),
            ..s
        };
        assert_eq!(
            RTorrent::commands(&s, 3),
            [
                "d.custom1.set=tv",
                "d.priority.set=3",
                "d.directory.set=/dl"
            ]
        );
    }
}
