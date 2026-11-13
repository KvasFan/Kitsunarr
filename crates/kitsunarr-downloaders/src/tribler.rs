//! Tribler 8 (Sonarr's `TriblerDownloadClient`): its REST API with the `X-Api-Key` header,
//! magnet links only (anonymity hops and safe seeding as set), downloads in the category's
//! folder under Tribler's default save path, seed goals from Tribler's own seeding mode.

use serde_json::{Map, Value, json};

use crate::error::cause;
use crate::{DownloadError, DownloadItem, ItemStatus, Source};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub url_base: String,
    pub api_key: String,
    pub category: Option<String>,
    pub directory: Option<String>,
    pub anonymity_level: i64,
    pub safe_seeding: bool,
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
            None | Some(Value::Null) => 20100,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("the port must be between 1 and 65535")?,
        };
        let hops = map
            .get("anonymityLevel")
            .and_then(Value::as_i64)
            .unwrap_or(1);
        if !(0..=3).contains(&hops) {
            return Err("the anonymity level must be between 0 and 3".into());
        }
        Ok(Self {
            host,
            port,
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(false),
            url_base: text(map, "urlBase").unwrap_or_default(),
            api_key: text(map, "apiKey").unwrap_or_default(),
            category: text(map, "tvCategory"),
            directory: text(map, "tvDirectory"),
            anonymity_level: hops,
            safe_seeding: map
                .get("safeSeeding")
                .and_then(Value::as_bool)
                .unwrap_or(true),
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
        format!("{scheme}://{}:{}{base}/{path}", self.host, self.port)
    }
}

/// Tribler's default seeding (`download_defaults`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeedingDefaults {
    pub save_as: String,
    /// `ratio`, `time`, `never` or `forever`.
    pub mode: String,
    pub ratio: Option<f64>,
    pub time_seconds: Option<f64>,
}

/// Snake-case member, or its camel-case form.
fn field<'v>(v: &'v Value, snake: &str) -> &'v Value {
    if !v[snake].is_null() {
        return &v[snake];
    }
    let camel: String = snake
        .split('_')
        .enumerate()
        .map(|(i, p)| {
            if i == 0 {
                p.to_string()
            } else {
                let mut c = p.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            }
        })
        .collect();
    &v[camel.as_str()]
}

/// A download (`Download`) and its files, as `GetItems` maps it.
pub fn to_item(
    d: &Value,
    files: &[String],
    seeding: &SeedingDefaults,
    now: i64,
) -> Option<DownloadItem> {
    let size = field(d, "size").as_i64().unwrap_or(0);
    // A magnet still fetching its metadata.
    if size == 0 {
        return None;
    }
    let name = field(d, "name").as_str().unwrap_or_default().to_string();
    let progress = field(d, "progress").as_f64().unwrap_or(0.0);
    let status = field(d, "status")
        .as_str()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let error = field(d, "error")
        .as_str()
        .filter(|e| !e.is_empty())
        .map(str::to_string);
    let ratio = field(d, "all_time_ratio").as_f64();
    let mut item_status = match status.as_str() {
        "HASHCHECKING" | "WAITING4HASHCHECK" | "CIRCUITS" | "EXIT_NODES" | "DOWNLOADING" => {
            ItemStatus::Downloading
        }
        "METADATA" | "ALLOCATING_DISKSPACE" => ItemStatus::Queued,
        "SEEDING" | "STOPPED" => ItemStatus::Completed,
        "STOPPED_ON_ERROR" => ItemStatus::Failed,
        _ => ItemStatus::Downloading,
    };
    let mut message = error.clone();
    if !matches!(
        status.as_str(),
        "HASHCHECKING"
            | "WAITING4HASHCHECK"
            | "CIRCUITS"
            | "EXIT_NODES"
            | "DOWNLOADING"
            | "METADATA"
            | "ALLOCATING_DISKSPACE"
            | "SEEDING"
            | "STOPPED"
            | "STOPPED_ON_ERROR"
    ) {
        message = Some(format!("Unknown download state: {status}"));
    }
    if status == "STOPPED" && progress < 1.0 {
        item_status = ItemStatus::Paused;
    }
    if error.is_some() {
        item_status = ItemStatus::Warning;
        message = error;
    }
    let dest = field(d, "destination")
        .as_str()
        .unwrap_or_default()
        .trim_end_matches('/');
    let output = match files {
        [one] => format!("{dest}/{one}"),
        _ => format!("{dest}/{name}"),
    };
    let eta = field(d, "eta").as_f64().map(|e| {
        if e >= 365.0 * 86_400.0 {
            365 * 86_400
        } else if e < 0.0 {
            0
        } else {
            e as i64
        }
    });
    // `HasReachedSeedLimit`: only stopped downloads, by Tribler's seeding mode.
    let reached = status == "STOPPED"
        && match seeding.mode.as_str() {
            "ratio" => ratio.zip(seeding.ratio).is_some_and(|(r, goal)| r >= goal),
            "time" => field(d, "time_added")
                .as_i64()
                .is_some_and(|added| added + (seeding.time_seconds.unwrap_or(0.0) as i64) < now),
            "never" => true,
            _ => false,
        };
    Some(DownloadItem {
        download_id: field(d, "infohash").as_str()?.to_ascii_uppercase(),
        title: name,
        category: String::new(),
        total_size: size,
        remaining_size: (size as f64 * (1.0 - progress)) as i64,
        remaining_seconds: eta,
        status: item_status,
        message,
        output_path: Some(output),
        seed_ratio: ratio.unwrap_or(0.0),
        can_be_removed: reached,
        seed_progress: None,
    })
}

#[derive(Clone, Default)]
pub struct Tribler {
    http: reqwest::Client,
}

impl Tribler {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    async fn request(
        &self,
        s: &Settings,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, DownloadError> {
        let mut req = self
            .http
            .request(method, s.url(path))
            .header("X-Api-Key", &s.api_key)
            .header("Accept", "application/json");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| DownloadError::Connection(cause(&e)))?;
        match resp.status().as_u16() {
            401 => return Err(DownloadError::Authentication),
            200..=299 => {}
            status => return Err(DownloadError::Http { status }),
        }
        resp.json()
            .await
            .map_err(|e| DownloadError::Client(format!("Tribler's response is unreadable: {e}")))
    }

    async fn seeding(&self, s: &Settings) -> Result<SeedingDefaults, DownloadError> {
        let v = self
            .request(s, reqwest::Method::GET, "api/settings", None)
            .await?;
        let d = &v["settings"]["libtorrent"]["download_defaults"];
        Ok(SeedingDefaults {
            save_as: d["saveas"].as_str().unwrap_or_default().to_string(),
            mode: d["seeding_mode"]
                .as_str()
                .unwrap_or("forever")
                .to_ascii_lowercase(),
            ratio: d["seeding_ratio"].as_f64(),
            time_seconds: d["seeding_time"].as_f64(),
        })
    }

    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let seeding = self.seeding(s).await?;
        let v = self
            .request(s, reqwest::Method::GET, "api/downloads", None)
            .await?;
        let now = chrono_now();
        let mut out = Vec::new();
        for d in v["downloads"].as_array().cloned().unwrap_or_default() {
            if field(&d, "size").as_i64().unwrap_or(0) == 0 {
                continue;
            }
            let hash = field(&d, "infohash")
                .as_str()
                .unwrap_or_default()
                .to_string();
            let files = self
                .request(
                    s,
                    reqwest::Method::GET,
                    &format!("api/downloads/{hash}/files"),
                    None,
                )
                .await?;
            let names: Vec<String> = files["files"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|f| f["name"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            out.extend(to_item(&d, &names, &seeding, now));
        }
        Ok(out)
    }

    /// `GetDownloadDirectory`: the directory, else the category under the default save path.
    async fn directory(&self, s: &Settings) -> Result<Option<String>, DownloadError> {
        if let Some(d) = &s.directory {
            return Ok(Some(d.clone()));
        }
        let Some(c) = &s.category else {
            return Ok(None);
        };
        let save = self.seeding(s).await?.save_as;
        Ok(Some(format!("{}/{c}", save.trim_end_matches('/'))))
    }

    pub async fn add(&self, s: &Settings, source: &Source<'_>) -> Result<String, DownloadError> {
        let Source::Magnet(url) = source else {
            return Err(DownloadError::Unsupported(
                "Tribler does not support torrent files, only magnet links".into(),
            ));
        };
        let mut body = json!({
            "uri": url, "safe_seeding": s.safe_seeding, "anon_hops": s.anonymity_level
        });
        if let Some(d) = self.directory(s).await? {
            body["destination"] = json!(d);
        }
        let v = self
            .request(s, reqwest::Method::PUT, "api/downloads", Some(body))
            .await?;
        Ok(v["infohash"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_uppercase())
    }

    pub async fn remove(
        &self,
        s: &Settings,
        hash: &str,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        self.request(
            s,
            reqwest::Method::DELETE,
            &format!("api/downloads/{}", hash.to_ascii_lowercase()),
            Some(json!({ "remove_data": delete_data })),
        )
        .await
        .map(|_| ())
    }

    /// `TestConnection`: reading the downloads.
    pub async fn test(&self, s: &Settings) -> Result<(), DownloadError> {
        self.items(s).await.map(|_| ())
    }
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_map_as_upstream() {
        let seeding = SeedingDefaults {
            save_as: "/dl".into(),
            mode: "ratio".into(),
            ratio: Some(2.0),
            time_seconds: None,
        };
        let d = |status: &str, progress: f64, ratio: f64| {
            json!({"name": "Show.S01", "infohash": "abc", "size": 1000, "progress": progress,
                "status": status, "all_time_ratio": ratio, "destination": "/dl/tv", "eta": -5, "error": ""})
        };
        let item = |status, progress, ratio, files: &[&str]| {
            let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
            to_item(&d(status, progress, ratio), &files, &seeding, 0).unwrap()
        };
        let done = item("STOPPED", 1.0, 2.5, &["Show.S01E01.mkv"]);
        assert_eq!(done.status, ItemStatus::Completed);
        assert!(done.can_be_removed);
        assert_eq!(done.output_path.as_deref(), Some("/dl/tv/Show.S01E01.mkv"));
        assert_eq!(done.remaining_seconds, Some(0));
        let seeding_now = item("SEEDING", 1.0, 2.5, &["a", "b"]);
        assert_eq!(seeding_now.status, ItemStatus::Completed);
        assert!(!seeding_now.can_be_removed, "still running");
        assert_eq!(seeding_now.output_path.as_deref(), Some("/dl/tv/Show.S01"));
        assert_eq!(item("STOPPED", 0.5, 0.0, &[]).status, ItemStatus::Paused);
        assert_eq!(item("METADATA", 0.0, 0.0, &[]).status, ItemStatus::Queued);
        assert_eq!(
            item("STOPPED_ON_ERROR", 0.0, 0.0, &[]).status,
            ItemStatus::Failed
        );
        assert_eq!(
            item("CIRCUITS", 0.0, 0.0, &[]).status,
            ItemStatus::Downloading
        );
        let unknown = item("SOMETHING", 0.0, 0.0, &[]);
        assert_eq!(
            unknown.message.as_deref(),
            Some("Unknown download state: SOMETHING")
        );
        let mut magnet = d("METADATA", 0.0, 0.0);
        magnet["size"] = json!(0);
        assert!(to_item(&magnet, &[], &seeding, 0).is_none());
    }
}
