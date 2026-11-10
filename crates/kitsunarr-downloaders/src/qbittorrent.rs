//! qBittorrent through its Web API v2 (Sonarr's `QBittorrent.cs` and `QBittorrentProxyV2.cs`).
//! qBittorrent 4.1 or newer is required; Sonarr's v1 proxy for older versions is not ported.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{Method, StatusCode, header};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::DownloadError;

/// Settings, as stored with Sonarr's field names.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub url_base: String,
    pub api_key: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub category: Option<String>,
    pub imported_category: Option<String>,
    /// 0 last, 1 first.
    pub recent_priority: i64,
    pub older_priority: i64,
    /// 0 started, 1 force started, 2 stopped.
    pub initial_state: i64,
    pub sequential_order: bool,
    pub first_and_last: bool,
    /// 0 qBittorrent's default, 1 original, 2 subfolder.
    pub content_layout: i64,
    pub add_series_tags: bool,
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn int(map: &Map<String, Value>, key: &str) -> i64 {
    map.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn flag(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).and_then(Value::as_bool).unwrap_or(false)
}

impl Settings {
    /// From a provider's `settings` map (Sonarr's defaults for missing fields).
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let host = text(map, "host").unwrap_or_else(|| "localhost".into());
        let port = match map.get("port") {
            None | Some(Value::Null) => 8080,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        Ok(Self {
            host,
            port,
            use_ssl: flag(map, "useSsl"),
            url_base: text(map, "urlBase").unwrap_or_default(),
            api_key: text(map, "apiKey"),
            username: text(map, "username"),
            password: text(map, "password"),
            category: text(map, "tvCategory"),
            imported_category: text(map, "tvImportedCategory"),
            recent_priority: int(map, "recentTvPriority"),
            older_priority: int(map, "olderTvPriority"),
            initial_state: int(map, "initialState"),
            sequential_order: flag(map, "sequentialOrder"),
            first_and_last: flag(map, "firstAndLast"),
            content_layout: int(map, "contentLayout"),
            add_series_tags: flag(map, "addSeriesTags"),
        })
    }

    /// `HttpRequestBuilder(useSsl, host, port, urlBase)`.
    pub fn base_url(&self) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let base = self.url_base.trim_matches('/');
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if base.is_empty() {
            format!("{scheme}://{host}:{}", self.port)
        } else {
            format!("{scheme}://{host}:{}/{base}", self.port)
        }
    }

    fn cookie_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.base_url(),
            self.username.as_deref().unwrap_or_default(),
            self.password.as_deref().unwrap_or_default()
        )
    }
}

/// `/api/v2/app/preferences` (the fields Sonarr reads).
#[derive(Debug, Clone, Deserialize)]
pub struct Preferences {
    #[serde(default)]
    pub save_path: String,
    #[serde(default)]
    pub max_ratio_enabled: bool,
    #[serde(default)]
    pub max_seeding_time_enabled: bool,
    #[serde(default)]
    pub max_seeding_time: i64,
    #[serde(default)]
    pub max_ratio_act: i64,
    #[serde(default = "default_true")]
    pub dht: bool,
}

fn default_true() -> bool {
    true
}

impl Preferences {
    /// `RemovesCompletedDownloads`: share limits that remove torrents before two weeks.
    pub fn removes_completed_downloads(&self) -> bool {
        let minimum_retention = 60 * 24 * 14;
        (self.max_ratio_enabled
            || (self.max_seeding_time_enabled && self.max_seeding_time < minimum_retention))
            && (self.max_ratio_act == 1 || self.max_ratio_act == 3)
    }
}

/// A torrent from `/api/v2/torrents/info`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Torrent {
    pub hash: String,
    pub name: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub eta: i64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub save_path: String,
    #[serde(default)]
    pub content_path: Option<String>,
    /// qBittorrent 5 (Web API 2.11.2): the torrent's top folder.
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub ratio: f64,
}

impl Torrent {
    /// `GetRemainingTime`: unknown when negative, over a year, or qBittorrent's 8640000.
    fn remaining_seconds(&self) -> Option<i64> {
        (self.eta >= 0 && self.eta <= 365 * 24 * 3600 && self.eta != 8_640_000).then_some(self.eta)
    }

    /// `GetItems`: Sonarr's mapping of qBittorrent states.
    pub fn to_item(&self, api_version: &str, dht: bool) -> crate::DownloadItem {
        use crate::ItemStatus::*;
        let mut remaining_seconds = self.remaining_seconds();
        let (status, message) = match self.state.as_str() {
            "error" => (
                Warning,
                Some("qBittorrent reports an error for this torrent".to_string()),
            ),
            "stoppedDL" | "pausedDL" => (Paused, None),
            "queuedDL" | "checkingDL" | "checkingUP" | "checkingResumeData" => (Queued, None),
            "pausedUP" | "stoppedUP" | "uploading" | "stalledUP" | "queuedUP" | "forcedUP" => {
                remaining_seconds = Some(0);
                (Completed, None)
            }
            "stalledDL" => (
                Warning,
                Some("The download is stalled with no connections".into()),
            ),
            "missingFiles" => (Warning, Some("The torrent is missing files".into())),
            "metaDL" | "forcedMetaDL" if dht => {
                (Queued, Some("qBittorrent is downloading metadata".into()))
            }
            "metaDL" | "forcedMetaDL" => (
                Warning,
                Some("qBittorrent cannot download the metadata while DHT is disabled".into()),
            ),
            "forcedDL" | "moving" | "downloading" => (Downloading, None),
            other => (
                Downloading,
                Some(format!("Unknown download state: {other}")),
            ),
        };
        let mut item = crate::DownloadItem {
            download_id: self.hash.to_ascii_uppercase(),
            title: self.name.clone(),
            category: self.category.clone(),
            total_size: self.size,
            remaining_size: (self.size as f64 * (1.0 - self.progress)) as i64,
            remaining_seconds,
            status,
            message,
            output_path: None,
            seed_ratio: self.ratio,
        };
        if status == Completed && at_least(&version(api_version), &[2, 6, 1]) {
            let root = self
                .root_path
                .as_deref()
                .filter(|r| !r.trim().is_empty() && *r != self.save_path)
                .filter(|_| at_least(&version(api_version), &[2, 11, 2]));
            let content = self
                .content_path
                .as_deref()
                .filter(|c| *c != self.save_path);
            match root.or(content) {
                Some(path) => item.output_path = Some(path.to_string()),
                None => {
                    item.status = Warning;
                    item.message = Some(
                        "qBittorrent reports the download folder itself as the content path; set a category save path or let qBittorrent create subfolders".into(),
                    );
                }
            }
        }
        item
    }
}

/// What a test found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub api_version: String,
    pub warnings: Vec<String>,
}

/// What to add, with the bits of the release Sonarr's add path reads.
#[derive(Debug, Clone)]
pub struct AddRequest<'a> {
    pub source: Source<'a>,
    /// The expected info hash (from the magnet or the file).
    pub hash: &'a str,
    /// An episode aired within the last two weeks (`IsRecentEpisode`).
    pub recent: bool,
    /// Series tag labels (for `addSeriesTags`).
    pub tags: &'a [String],
}

#[derive(Debug, Clone)]
pub enum Source<'a> {
    Magnet(&'a str),
    File { name: &'a str, content: &'a [u8] },
}

/// Parses `"2.11.2"` into comparable parts.
fn version(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

fn at_least(v: &[u64], min: &[u64]) -> bool {
    let n = v.len().max(min.len());
    for i in 0..n {
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

/// A qBittorrent API client; login cookies are cached per address and credentials.
#[derive(Clone, Default)]
pub struct QBittorrent {
    http: reqwest::Client,
    cookies: Arc<Mutex<HashMap<String, String>>>,
}

impl QBittorrent {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            cookies: Arc::default(),
        }
    }

    fn connection_error(e: reqwest::Error) -> DownloadError {
        DownloadError::Connection(crate::error::cause(&e))
    }

    /// `AuthenticateClient`: logs in when credentials are set, caching the `SID` cookie.
    async fn login(&self, s: &Settings, force: bool) -> Result<Option<String>, DownloadError> {
        let (Some(user), Some(pass)) = (&s.username, &s.password) else {
            return if force {
                Err(DownloadError::Authentication)
            } else {
                Ok(None)
            };
        };
        let key = s.cookie_key();
        if !force
            && let Some(c) = self
                .cookies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .cloned()
        {
            return Ok(Some(c));
        }
        let response = self
            .http
            .post(format!("{}/api/v2/auth/login", s.base_url()))
            .form(&[("username", user.as_str()), ("password", pass.as_str())])
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(Self::connection_error)?;
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(DownloadError::Authentication);
        }
        if !response.status().is_success() {
            return Err(DownloadError::Http {
                status: response.status().as_u16(),
            });
        }
        let cookie = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        let body = response.text().await.unwrap_or_default();
        // qBittorrent answers "Fails." to a bad login.
        if !body.trim().is_empty() && body.trim() != "Ok." {
            return Err(DownloadError::Authentication);
        }
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, cookie.clone());
        Ok(Some(cookie))
    }

    /// `ProcessRequest`: one retry after logging in again on 403.
    async fn request(
        &self,
        s: &Settings,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        form: Option<Vec<(&str, String)>>,
        file: Option<(&str, &[u8])>,
    ) -> Result<String, DownloadError> {
        let mut force = false;
        loop {
            let cookie = if s.api_key.is_some() {
                None
            } else {
                self.login(s, force).await?
            };
            let mut req = self
                .http
                .request(method.clone(), format!("{}{path}", s.base_url()))
                .query(query)
                .timeout(Duration::from_secs(60));
            if let Some(key) = &s.api_key {
                req = req.bearer_auth(key);
            }
            if let Some(c) = &cookie {
                req = req.header(header::COOKIE, c);
            }
            if let Some(fields) = &form {
                let mut multipart = reqwest::multipart::Form::new();
                for (k, v) in fields {
                    multipart = multipart.text(k.to_string(), v.clone());
                }
                if let Some((name, content)) = file {
                    let part = reqwest::multipart::Part::bytes(content.to_vec())
                        .file_name(name.to_string())
                        .mime_str("application/x-bittorrent")
                        .map_err(|e| DownloadError::Client(e.to_string()))?;
                    multipart = multipart.part("torrents", part);
                }
                req = req.multipart(multipart);
            }
            let response = req.send().await.map_err(Self::connection_error)?;
            let status = response.status();
            if status == StatusCode::FORBIDDEN {
                if force || s.api_key.is_some() {
                    return Err(DownloadError::Authentication);
                }
                force = true;
                continue;
            }
            if status == StatusCode::CONFLICT {
                return Err(DownloadError::Rejected(
                    "qBittorrent rejected the torrent due to a conflict (it may already be added)"
                        .into(),
                ));
            }
            if !status.is_success() {
                return Err(DownloadError::Http {
                    status: status.as_u16(),
                });
            }
            return response
                .text()
                .await
                .map_err(|e| DownloadError::Connection(crate::error::cause(&e)));
        }
    }

    async fn get(
        &self,
        s: &Settings,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<String, DownloadError> {
        self.request(s, Method::GET, path, query, None, None).await
    }

    async fn post(
        &self,
        s: &Settings,
        path: &str,
        form: Vec<(&str, String)>,
    ) -> Result<String, DownloadError> {
        self.request(s, Method::POST, path, &[], Some(form), None)
            .await
    }

    fn json<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, DownloadError> {
        serde_json::from_str(body)
            .map_err(|e| DownloadError::Client(format!("unexpected response: {e}")))
    }

    pub async fn api_version(&self, s: &Settings) -> Result<String, DownloadError> {
        match self.get(s, "/api/v2/app/webapiVersion", &[]).await {
            Err(DownloadError::Http { status: 404 }) => Err(DownloadError::Unsupported(
                "qBittorrent 4.1 or newer is required (Web API v2)".into(),
            )),
            other => other.map(|v| v.trim().to_string()),
        }
    }

    pub async fn preferences(&self, s: &Settings) -> Result<Preferences, DownloadError> {
        Self::json(&self.get(s, "/api/v2/app/preferences", &[]).await?)
    }

    pub async fn categories(&self, s: &Settings) -> Result<Vec<String>, DownloadError> {
        let map: Map<String, Value> =
            Self::json(&self.get(s, "/api/v2/torrents/categories", &[]).await?)?;
        Ok(map.keys().cloned().collect())
    }

    /// Torrents in the TV category (all torrents when no category is set).
    pub async fn torrents(&self, s: &Settings) -> Result<Vec<Torrent>, DownloadError> {
        let query: Vec<(&str, &str)> = s
            .category
            .as_deref()
            .map(|c| vec![("category", c)])
            .unwrap_or_default();
        Self::json(&self.get(s, "/api/v2/torrents/info", &query).await?)
    }

    /// `GetItems`: the downloads in the TV category, with Sonarr's status mapping.
    pub async fn items(&self, s: &Settings) -> Result<Vec<crate::DownloadItem>, DownloadError> {
        let version = self.api_version(s).await?;
        let dht = self.preferences(s).await?.dht;
        Ok(self
            .torrents(s)
            .await?
            .iter()
            .map(|t| t.to_item(&version, dht))
            .collect())
    }

    /// `Test`: connection and version, categories (created when missing, as upstream does),
    /// share-limit settings that would remove downloads, and listing torrents.
    pub async fn test(&self, s: &Settings) -> Result<TestReport, DownloadError> {
        let api_version = self.api_version(s).await?;
        let v = version(&api_version);
        let mut warnings = Vec::new();
        if !at_least(&v, &[2, 0]) {
            return Err(DownloadError::Unsupported(format!(
                "qBittorrent 4.1 or newer is required; it reports Web API {api_version}"
            )));
        }
        if s.category.is_none() {
            warnings.push(
                "A category is recommended: it keeps Kitsunarr's downloads apart from everything else in qBittorrent".into(),
            );
        }
        if self.preferences(s).await?.removes_completed_downloads() {
            return Err(DownloadError::Client(
                "qBittorrent is set to remove torrents when they reach their share limits; Kitsunarr could lose completed downloads before importing them. Change the action to pause instead".into(),
            ));
        }
        for category in [&s.category, &s.imported_category].into_iter().flatten() {
            if !self.categories(s).await?.contains(category) {
                self.post(
                    s,
                    "/api/v2/torrents/createCategory",
                    vec![("category", category.clone())],
                )
                .await?;
                if !self.categories(s).await?.contains(category) {
                    return Err(DownloadError::Client(format!(
                        "the category {category:?} could not be created in qBittorrent"
                    )));
                }
            }
        }
        self.torrents(s).await?;
        Ok(TestReport {
            api_version,
            warnings,
        })
    }

    /// `AddTorrentDownloadFormParameters`.
    async fn add_fields(&self, s: &Settings) -> Result<Vec<(&'static str, String)>, DownloadError> {
        let mut form = Vec::new();
        if let Some(c) = &s.category {
            form.push(("category", c.clone()));
        }
        if s.initial_state == 0 || s.initial_state == 2 {
            let v = version(&self.api_version(s).await?);
            let name = if at_least(&v, &[2, 11, 0]) {
                "stopped"
            } else {
                "paused"
            };
            form.push((name, (s.initial_state == 2).to_string()));
        }
        if s.sequential_order {
            form.push(("sequentialDownload", "true".into()));
        }
        if s.first_and_last {
            form.push(("firstLastPiecePrio", "true".into()));
        }
        match s.content_layout {
            1 => form.push(("contentLayout", "Original".into())),
            2 => form.push(("contentLayout", "Subfolder".into())),
            _ => {}
        }
        Ok(form)
    }

    async fn is_loaded(&self, s: &Settings, hash: &str) -> bool {
        self.get(s, "/api/v2/torrents/properties", &[("hash", hash)])
            .await
            .is_ok()
    }

    /// `AddFromMagnetLink` / `AddFromTorrentFile`. Returns the info hash.
    pub async fn add(&self, s: &Settings, req: &AddRequest<'_>) -> Result<String, DownloadError> {
        let hash = req.hash.to_ascii_lowercase();
        let mut form = self.add_fields(s).await?;
        let body = match req.source {
            Source::Magnet(url) => {
                if !url.contains("&tr=") && !self.preferences(s).await?.dht {
                    return Err(DownloadError::Unsupported(
                        "magnet links without trackers are not supported while DHT is disabled in qBittorrent".into(),
                    ));
                }
                form.push(("urls", url.to_string()));
                self.request(
                    s,
                    Method::POST,
                    "/api/v2/torrents/add",
                    &[],
                    Some(form),
                    None,
                )
                .await?
            }
            Source::File { name, content } => {
                self.request(
                    s,
                    Method::POST,
                    "/api/v2/torrents/add",
                    &[],
                    Some(form),
                    Some((name, content)),
                )
                .await?
            }
        };
        if body.trim() == "Fails." {
            return Err(DownloadError::Client(
                "qBittorrent failed to add the torrent".into(),
            ));
        }
        let priority = if req.recent {
            s.recent_priority
        } else {
            s.older_priority
        };
        let move_to_top = priority == 1;
        let force_start = s.initial_state == 1;
        let tag = s.add_series_tags && !req.tags.is_empty();
        if move_to_top || force_start || tag {
            // `WaitForTorrent`: up to ten checks, 100 ms apart.
            let mut loaded = false;
            for _ in 0..10 {
                if self.is_loaded(s, &hash).await {
                    loaded = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if loaded {
                // Upstream logs and ignores failures of these follow-up calls.
                if move_to_top
                    && let Err(e) = self
                        .post(
                            s,
                            "/api/v2/torrents/topPrio",
                            vec![("hashes", hash.clone())],
                        )
                        .await
                {
                    tracing::warn!(error = %e, %hash, "failed to set the torrent priority");
                }
                if force_start
                    && let Err(e) = self
                        .post(
                            s,
                            "/api/v2/torrents/setForceStart",
                            vec![("hashes", hash.clone()), ("value", "true".into())],
                        )
                        .await
                {
                    tracing::warn!(error = %e, %hash, "failed to force-start the torrent");
                }
                if tag
                    && let Err(e) = self
                        .post(
                            s,
                            "/api/v2/torrents/addTags",
                            vec![("hashes", hash.clone()), ("tags", req.tags.join(","))],
                        )
                        .await
                {
                    tracing::warn!(error = %e, %hash, "failed to add tags");
                }
            }
        }
        Ok(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_follow_sonarr_names_and_defaults() {
        let s = Settings::from_map(&Map::new()).unwrap();
        assert_eq!(s.base_url(), "http://localhost:8080");
        let map: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "host": "qbit", "port": "8443", "useSsl": true, "urlBase": "/qb/",
            "username": "admin", "password": "pw", "tvCategory": "tv-kitsunarr", "initialState": 1
        }))
        .unwrap();
        let s = Settings::from_map(&map).unwrap();
        assert_eq!(s.base_url(), "https://qbit:8443/qb");
        assert_eq!(s.category.as_deref(), Some("tv-kitsunarr"));
        assert_eq!(s.initial_state, 1);
        let bad: Map<String, Value> =
            serde_json::from_value(serde_json::json!({"host": "http://x"})).unwrap();
        assert!(Settings::from_map(&bad).is_err());
    }

    #[test]
    fn states_map_as_upstream() {
        let torrent = |state: &str, eta: i64| Torrent {
            hash: "abc".into(),
            name: "Show.S01E01".into(),
            size: 1000,
            progress: 0.25,
            eta,
            state: state.into(),
            category: "tv".into(),
            save_path: "/downloads".into(),
            content_path: Some("/downloads/Show.S01E01".into()),
            root_path: None,
            ratio: 0.5,
        };
        use crate::ItemStatus::*;
        let item = torrent("downloading", 120).to_item("2.11.4", true);
        assert_eq!(
            (
                item.download_id.as_str(),
                item.status,
                item.remaining_size,
                item.remaining_seconds
            ),
            ("ABC", Downloading, 750, Some(120))
        );
        assert_eq!(
            torrent("downloading", 8_640_000)
                .to_item("2.11.4", true)
                .remaining_seconds,
            None
        );
        assert_eq!(
            torrent("stalledDL", 0).to_item("2.11.4", true).status,
            Warning
        );
        assert_eq!(torrent("metaDL", 0).to_item("2.11.4", true).status, Queued);
        assert_eq!(
            torrent("metaDL", 0).to_item("2.11.4", false).status,
            Warning
        );
        assert_eq!(
            torrent("pausedDL", 0).to_item("2.11.4", true).status,
            Paused
        );
        let done = torrent("stalledUP", 8_640_000).to_item("2.11.4", true);
        assert_eq!(
            (
                done.status,
                done.remaining_seconds,
                done.output_path.as_deref()
            ),
            (Completed, Some(0), Some("/downloads/Show.S01E01"))
        );
        // Content path equal to the save path: no folder to import from.
        let mut flat = torrent("uploading", 0);
        flat.content_path = Some("/downloads".into());
        assert_eq!(flat.to_item("2.11.4", true).status, Warning);
        // qBittorrent 5's root path wins when set.
        let mut rooted = torrent("stoppedUP", 0);
        rooted.root_path = Some("/downloads/Show.S01E01.Pack".into());
        assert_eq!(
            rooted.to_item("2.11.4", true).output_path.as_deref(),
            Some("/downloads/Show.S01E01.Pack")
        );
        assert_eq!(
            torrent("weirdNewState", 5).to_item("2.11.4", true).status,
            Downloading
        );
    }

    #[test]
    fn versions_and_share_limits() {
        assert!(at_least(&version("2.11.2"), &[2, 11, 0]));
        assert!(!at_least(&version("2.8.3"), &[2, 11, 0]));
        assert!(at_least(&version("2.0"), &[2, 0]));
        let prefs = |ratio: bool, seeding: bool, time: i64, act: i64| Preferences {
            save_path: String::new(),
            max_ratio_enabled: ratio,
            max_seeding_time_enabled: seeding,
            max_seeding_time: time,
            max_ratio_act: act,
            dht: true,
        };
        assert!(prefs(true, false, 0, 1).removes_completed_downloads());
        assert!(
            !prefs(true, false, 0, 0).removes_completed_downloads(),
            "pause"
        );
        assert!(prefs(false, true, 60, 3).removes_completed_downloads());
        assert!(!prefs(false, true, 60 * 24 * 14, 3).removes_completed_downloads());
    }
}
