//! Flood (Sonarr's `Flood`, `FloodProxy`): its REST API behind a session cookie. Torrents
//! carry tags (the client's, plus optional additional tags from the release), finished ones
//! get the post-import tags, and Flood leaves seed goals to Kitsunarr (see `SeedProgress`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::error::cause;
use crate::{DownloadError, DownloadItem, ItemStatus, ReleaseInfo, SeedProgress, Source};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub url_base: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub destination: Option<String>,
    pub tags: Vec<String>,
    pub post_import_tags: Vec<String>,
    /// `AdditionalTags`: 0 title slug, 1 quality, 2 languages, 3 release group, 4 year,
    /// 5 indexer, 6 network.
    pub additional_tags: Vec<i64>,
    pub start_on_add: bool,
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn strings(map: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    match map.get(key)? {
        Value::Array(a) => Some(
            a.iter()
                .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()))
                .map(str::to_string)
                .collect(),
        ),
        Value::String(s) => Some(
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => None,
    }
}

impl Settings {
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let host = text(map, "host").unwrap_or_else(|| "localhost".into());
        if host.contains('/') || host.contains(' ') {
            return Err("the host must be a host name or address, without a scheme or path".into());
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 3000,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        let additional_tags: Vec<i64> = map
            .get("additionalTags")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        if additional_tags.iter().any(|t| !(0..=6).contains(t)) {
            return Err("unexpected additional tag".into());
        }
        Ok(Self {
            host,
            port,
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(false),
            url_base: text(map, "urlBase").unwrap_or_default(),
            username: text(map, "username"),
            password: text(map, "password"),
            destination: text(map, "destination"),
            tags: strings(map, "tags").unwrap_or_else(|| vec!["kitsunarr".into()]),
            post_import_tags: strings(map, "postImportTags").unwrap_or_default(),
            additional_tags,
            start_on_add: map
                .get("startOnAdd")
                .and_then(Value::as_bool)
                .unwrap_or(true),
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
        format!("{scheme}://{}:{}{base}/api{path}", self.host, self.port)
    }

    fn key(&self) -> String {
        format!(
            "{}:{}",
            self.api(""),
            self.username.clone().unwrap_or_default()
        )
    }

    /// `HandleTags`: the client's tags and the chosen additional tags.
    pub fn tags_for(&self, info: &ReleaseInfo) -> Vec<String> {
        let mut out: Vec<String> = self.tags.clone();
        let mut add = |t: &str| {
            if !t.trim().is_empty() && !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        };
        for t in &self.additional_tags {
            match t {
                0 => add(&info.title_slug),
                1 => add(&info.quality),
                2 => info.languages.iter().for_each(|l| add(l)),
                3 => add(&info.release_group),
                4 => add(&info.year),
                5 => add(&info.indexer),
                6 => add(&info.network),
                _ => {}
            }
        }
        out
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Torrent {
    pub bytes_done: i64,
    pub directory: String,
    pub eta: i64,
    pub message: Option<String>,
    pub name: String,
    pub ratio: f64,
    pub size_bytes: i64,
    pub status: Vec<String>,
    pub tags: Vec<String>,
    /// Flood 4.5.
    pub date_finished: Option<i64>,
}

impl Torrent {
    /// `GetItems`' mapping.
    pub fn to_item(&self, hash: &str) -> DownloadItem {
        let has = |s: &str| self.status.iter().any(|x| x == s);
        let status = if has("seeding") || has("complete") {
            ItemStatus::Completed
        } else if has("stopped") {
            ItemStatus::Paused
        } else if has("error") {
            ItemStatus::Warning
        } else if has("downloading") {
            ItemStatus::Downloading
        } else {
            ItemStatus::Queued
        };
        DownloadItem {
            download_id: hash.to_ascii_uppercase(),
            title: self.name.clone(),
            category: self.tags.first().cloned().unwrap_or_default(),
            total_size: self.size_bytes,
            remaining_size: self.size_bytes - self.bytes_done,
            remaining_seconds: (self.eta > 0).then_some(self.eta),
            status,
            message: self.message.clone().filter(|m| !m.is_empty()),
            output_path: Some(self.directory.clone()),
            seed_ratio: self.ratio,
            can_be_removed: false,
            seed_progress: (status == ItemStatus::Completed).then_some(SeedProgress {
                ratio: self.ratio,
                finished_at: self.date_finished.filter(|d| *d > 0),
            }),
        }
    }
}

/// `GetImportItem`: a single file's path, or the folder every file shares.
pub fn import_path(directory: &str, contents: &[String]) -> Option<String> {
    let dir = directory.trim_end_matches(['/', '\\']);
    match contents {
        [] => None,
        [one] => Some(format!("{dir}/{}", one.trim_start_matches(['/', '\\']))),
        many => {
            let firsts: Vec<&str> = many
                .iter()
                .filter_map(|p| p.split(['/', '\\']).find(|s| !s.is_empty()))
                .collect();
            Some(match firsts.first() {
                Some(f) if firsts.iter().all(|x| x == f) && firsts.len() == many.len() => {
                    format!("{dir}/{f}")
                }
                _ => dir.to_string(),
            })
        }
    }
}

#[derive(Clone, Default)]
pub struct Flood {
    http: reqwest::Client,
    cookies: Arc<Mutex<HashMap<String, String>>>,
}

impl Flood {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            cookies: Arc::default(),
        }
    }

    /// `AuthAuthenticate`: the session cookie, cached per URL and user.
    async fn cookie(&self, s: &Settings, force: bool) -> Result<String, DownloadError> {
        if !force
            && let Some(c) = self
                .cookies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&s.key())
        {
            return Ok(c.clone());
        }
        let resp = self
            .http
            .post(s.api("/auth/authenticate"))
            .json(&json!({ "username": s.username, "password": s.password }))
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        if matches!(resp.status().as_u16(), 401 | 403) {
            return Err(DownloadError::Authentication);
        }
        if !resp.status().is_success() {
            return Err(DownloadError::Http {
                status: resp.status().as_u16(),
            });
        }
        let cookie = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(s.key(), cookie.clone());
        Ok(cookie)
    }

    async fn request(
        &self,
        s: &Settings,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<String, DownloadError> {
        for attempt in 0..2 {
            let cookie = self.cookie(s, attempt > 0).await?;
            let mut req = self
                .http
                .request(method.clone(), s.api(path))
                .header(reqwest::header::COOKIE, cookie);
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| DownloadError::Connection(cause(&e)))?;
            match resp.status().as_u16() {
                401 | 403 if attempt == 0 => continue,
                401 | 403 => {
                    self.cookies
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&s.key());
                    return Err(DownloadError::Authentication);
                }
                200..=299 => {
                    return resp
                        .text()
                        .await
                        .map_err(|e| DownloadError::Connection(cause(&e)));
                }
                status => return Err(DownloadError::Http { status }),
            }
        }
        Err(DownloadError::Authentication)
    }

    pub async fn torrents(&self, s: &Settings) -> Result<HashMap<String, Torrent>, DownloadError> {
        #[derive(Deserialize)]
        struct Summary {
            #[serde(default)]
            torrents: HashMap<String, Torrent>,
        }
        let body = self
            .request(s, reqwest::Method::GET, "/torrents", None)
            .await?;
        serde_json::from_str::<Summary>(&body)
            .map(|x| x.torrents)
            .map_err(|e| DownloadError::Client(format!("Flood's torrent list is unreadable: {e}")))
    }

    async fn contents(&self, s: &Settings, hash: &str) -> Result<Vec<String>, DownloadError> {
        #[derive(Deserialize)]
        struct Content {
            path: String,
        }
        let body = self
            .request(
                s,
                reqwest::Method::GET,
                &format!("/torrents/{hash}/contents"),
                None,
            )
            .await?;
        serde_json::from_str::<Vec<Content>>(&body)
            .map(|c| c.into_iter().map(|c| c.path).collect())
            .map_err(|e| {
                DownloadError::Client(format!("Flood's torrent contents are unreadable: {e}"))
            })
    }

    /// `GetItems`: torrents with every tag of the client and not yet all post-import tags;
    /// finished ones point at their files (`GetImportItem`).
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let mut out = Vec::new();
        let mut torrents: Vec<(String, Torrent)> = self.torrents(s).await?.into_iter().collect();
        torrents.sort_by(|a, b| a.0.cmp(&b.0));
        for (hash, t) in torrents {
            if !s.tags.iter().all(|tag| t.tags.contains(tag)) {
                continue;
            }
            if !s.post_import_tags.is_empty()
                && s.post_import_tags.iter().all(|tag| t.tags.contains(tag))
            {
                continue;
            }
            let mut item = t.to_item(&hash);
            if item.status == ItemStatus::Completed {
                match self.contents(s, &hash).await {
                    Ok(contents) => item.output_path = import_path(&t.directory, &contents),
                    Err(e) => {
                        tracing::warn!(torrent = %t.name, error = %e, "Flood did not list the torrent's files");
                        item.output_path = None;
                    }
                }
            }
            out.push(item);
        }
        Ok(out)
    }

    pub async fn add(
        &self,
        s: &Settings,
        source: &Source<'_>,
        hash: &str,
        info: &ReleaseInfo,
    ) -> Result<String, DownloadError> {
        let mut body = match source {
            Source::Magnet(url) => json!({ "urls": [url] }),
            Source::File { content, .. } => json!({
                "files": [base64::engine::general_purpose::STANDARD.encode(content)]
            }),
        };
        body["tags"] = json!(s.tags_for(info));
        if let Some(d) = &s.destination {
            body["destination"] = json!(d);
        }
        if s.start_on_add {
            body["start"] = json!(true);
        }
        let path = match source {
            Source::Magnet(_) => "/torrents/add-urls",
            Source::File { .. } => "/torrents/add-files",
        };
        self.request(s, reqwest::Method::POST, path, Some(body))
            .await?;
        Ok(hash.to_ascii_uppercase())
    }

    pub async fn remove(
        &self,
        s: &Settings,
        hash: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        self.request(
            s,
            reqwest::Method::POST,
            "/torrents/delete",
            Some(json!({ "hashes": [hash], "deleteData": delete_data })),
        )
        .await
        .map(|_| ())
    }

    /// `MarkItemAsImported`: the post-import tags join the torrent's.
    pub async fn mark_imported(&self, s: &Settings, hash: &str) -> Result<(), DownloadError> {
        if s.post_import_tags.is_empty() {
            return Ok(());
        }
        let torrents = self.torrents(s).await?;
        let Some((key, t)) = torrents.iter().find(|(k, _)| k.eq_ignore_ascii_case(hash)) else {
            return Ok(());
        };
        let mut tags = t.tags.clone();
        for p in &s.post_import_tags {
            if !tags.contains(p) {
                tags.push(p.clone());
            }
        }
        self.request(
            s,
            reqwest::Method::PATCH,
            "/torrents/tags",
            Some(json!({ "hashes": [key], "tags": tags })),
        )
        .await
        .map(|_| ())
    }

    /// `Test`: the credentials (`/auth/verify`).
    pub async fn test(&self, s: &Settings) -> Result<(), DownloadError> {
        self.cookie(s, true).await?;
        self.request(s, reqwest::Method::GET, "/auth/verify", None)
            .await
            .map(|_| ())
    }
}

impl Flood {
    /// `GetStatus`: the destination, else Flood's default directory.
    pub async fn status(&self, s: &Settings) -> Result<crate::ClientStatus, DownloadError> {
        let dest = match s.destination.clone().filter(|d| !d.trim().is_empty()) {
            Some(d) => d,
            None => {
                let body = self
                    .request(s, reqwest::Method::GET, "client/settings", None)
                    .await?;
                serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v["directoryDefault"].as_str().map(str::to_string))
                    .unwrap_or_default()
            }
        };
        Ok(crate::ClientStatus {
            is_localhost: crate::is_localhost(&s.host),
            output_root_folders: vec![dest].into_iter().filter(|d| !d.is_empty()).collect(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_paths_and_tags_follow_flood_fixture() {
        let t = |status: &[&str]| Torrent {
            name: "Droned.S01E01".into(),
            directory: "/downloads".into(),
            size_bytes: 1000,
            bytes_done: 400,
            ratio: 1.25,
            status: status.iter().map(|s| s.to_string()).collect(),
            tags: vec!["kitsunarr".into()],
            date_finished: Some(1_700_000_000),
            ..Default::default()
        };
        assert_eq!(
            t(&["seeding", "active"]).to_item("abc").status,
            ItemStatus::Completed
        );
        assert_eq!(
            t(&["complete"]).to_item("abc").status,
            ItemStatus::Completed
        );
        assert_eq!(t(&["stopped"]).to_item("abc").status, ItemStatus::Paused);
        assert_eq!(t(&["error"]).to_item("abc").status, ItemStatus::Warning);
        assert_eq!(
            t(&["downloading"]).to_item("abc").status,
            ItemStatus::Downloading
        );
        let done = t(&["seeding"]).to_item("abc");
        assert_eq!(done.download_id, "ABC");
        assert_eq!(done.category, "kitsunarr");
        assert_eq!(done.remaining_size, 600);
        assert_eq!(
            done.seed_progress,
            Some(SeedProgress {
                ratio: 1.25,
                finished_at: Some(1_700_000_000)
            })
        );
        // `GetImportItem`.
        assert_eq!(
            import_path("/downloads/", &["Droned.S01E01.mkv".into()]).as_deref(),
            Some("/downloads/Droned.S01E01.mkv")
        );
        assert_eq!(
            import_path("/downloads", &["Pack/a.mkv".into(), "Pack/b.mkv".into()]).as_deref(),
            Some("/downloads/Pack")
        );
        assert_eq!(
            import_path("/downloads", &["a.mkv".into(), "Other/b.mkv".into()]).as_deref(),
            Some("/downloads")
        );
        assert_eq!(import_path("/downloads", &[]), None);

        let mut map = Map::new();
        map.insert("additionalTags".into(), json!([0, 1, 2, 4]));
        let s = Settings::from_map(&map).unwrap();
        assert_eq!(s.tags, ["kitsunarr"]);
        let info = ReleaseInfo {
            title_slug: "droned".into(),
            quality: "WEBDL-1080p".into(),
            languages: vec!["English".into(), "French".into()],
            year: "2016".into(),
            ..Default::default()
        };
        assert_eq!(
            s.tags_for(&info),
            [
                "kitsunarr",
                "droned",
                "WEBDL-1080p",
                "English",
                "French",
                "2016"
            ]
        );
        map.insert("additionalTags".into(), json!([9]));
        assert!(Settings::from_map(&map).is_err());
        assert_eq!(
            Settings::from_map(&Map::new()).unwrap().api("/torrents"),
            "http://localhost:3000/api/torrents"
        );
    }
}
