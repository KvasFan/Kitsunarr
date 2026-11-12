//! SABnzbd through its HTTP API (Sonarr's `Sabnzbd.cs` and `SabnzbdProxy.cs`).
//!
//! Every call is `GET {base}/api?mode=..&apikey=..&output=json` (the username and password
//! replace the API key when it is empty); NZBs are uploaded with `POST mode=addfile`. SABnzbd
//! returns numbers as strings and booleans as 0/1, so answers are read leniently.

use serde_json::{Map, Value};

use crate::error::DownloadError;
use crate::usenet::{base_url, flag, host, num, port, string, strings, text};
use crate::{DownloadItem, ItemStatus};

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
    /// `SabnzbdPriority`: -100 default, -2 paused, -1 low, 0 normal, 1 high, 2 force.
    pub recent_priority: i64,
    pub older_priority: i64,
}

impl Settings {
    /// From a provider's `settings` map (Sonarr's defaults and validation rules).
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let api_key = text(map, "apiKey");
        let username = text(map, "username");
        let password = text(map, "password");
        if api_key.is_none() && (username.is_none() || password.is_none()) {
            return Err(
                "an API key, or a username and password, is required to connect to SABnzbd".into(),
            );
        }
        Ok(Self {
            host: host(map)?,
            port: port(map, 8080)?,
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(false),
            url_base: text(map, "urlBase").unwrap_or_default(),
            api_key,
            username,
            password,
            // Sonarr's default when the field is absent; an empty value means none.
            category: match map.get("tvCategory") {
                None | Some(Value::Null) => Some("tv".into()),
                _ => text(map, "tvCategory"),
            },
            recent_priority: map
                .get("recentTvPriority")
                .and_then(Value::as_i64)
                .unwrap_or(-100),
            older_priority: map
                .get("olderTvPriority")
                .and_then(Value::as_i64)
                .unwrap_or(-100),
        })
    }

    pub fn url(&self, path: &str) -> String {
        base_url(self.use_ssl, &self.host, self.port, &self.url_base, path)
    }
}

/// `SabnzbdDownloadStatus` names as SABnzbd sends them.
fn status_of(v: &Value) -> String {
    string(v.get("status"))
}

/// `SabnzbdQueueTimeConverter`: `d:h:m:s` or `h:m:s`.
fn parse_time_left(s: &str) -> Option<i64> {
    let parts: Vec<i64> = s
        .split(':')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [d, h, m, sec] => Some(((d * 24 + h) * 60 + m) * 60 + sec),
        [h, m, sec] => Some((h * 60 + m) * 60 + sec),
        _ => None,
    }
}

/// `SabnzbdVersion` parsing: `major.minor.patch` (patch may be `x`), or `develop` as 3.0.0.
fn parse_version(raw: &str) -> Option<(u64, u64, u64)> {
    let re = regex_lite(raw);
    if let Some(v) = re {
        return Some(v);
    }
    raw.eq_ignore_ascii_case("develop").then_some((3, 0, 0))
}

/// `(?<major>\d+)\.(?<minor>\d+)\.(?<patch>\d+|x)` anywhere in the text.
fn regex_lite(raw: &str) -> Option<(u64, u64, u64)> {
    let b = raw.as_bytes();
    let digits = |from: usize| {
        let end = (from..b.len())
            .find(|i| !b[*i].is_ascii_digit())
            .unwrap_or(b.len());
        (end > from).then(|| (raw[from..end].parse::<u64>().unwrap_or(0), end))
    };
    for start in 0..b.len() {
        if !b[start].is_ascii_digit() || (start > 0 && b[start - 1].is_ascii_digit()) {
            continue;
        }
        let Some((major, e1)) = digits(start) else {
            continue;
        };
        if b.get(e1) != Some(&b'.') {
            continue;
        }
        let Some((minor, e2)) = digits(e1 + 1) else {
            continue;
        };
        if b.get(e2) != Some(&b'.') {
            continue;
        }
        if b.get(e2 + 1) == Some(&b'x') {
            return Some((major, minor, 0));
        }
        if let Some((patch, _)) = digits(e2 + 1) {
            return Some((major, minor, patch));
        }
    }
    None
}

/// `ContainsCategory`: an empty list means every category; no category means "Default".
fn contains_category(list: &[String], category: Option<&str>) -> bool {
    if list.is_empty() {
        return true;
    }
    let c = category.unwrap_or("Default");
    list.iter().any(|x| x == c)
}

/// What a test found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub version: String,
    pub warnings: Vec<String>,
}

/// A SABnzbd API client.
#[derive(Clone, Default)]
pub struct Sabnzbd {
    http: reqwest::Client,
}

impl Sabnzbd {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// `BuildRequest` + `ProcessRequest` + `CheckForError`.
    async fn call(
        &self,
        s: &Settings,
        mode: &str,
        params: &[(&str, String)],
        upload: Option<(&str, &[u8])>,
    ) -> Result<String, DownloadError> {
        let mut query: Vec<(&str, String)> = vec![("mode", mode.to_string())];
        query.extend(params.iter().cloned());
        match &s.api_key {
            Some(k) => query.push(("apikey", k.clone())),
            None => {
                query.push(("ma_username", s.username.clone().unwrap_or_default()));
                query.push(("ma_password", s.password.clone().unwrap_or_default()));
            }
        }
        query.push(("output", "json".into()));
        let url = s.url("api");
        let req = match upload {
            Some((name, content)) => {
                let part = reqwest::multipart::Part::bytes(content.to_vec())
                    .file_name(name.to_string())
                    .mime_str("application/x-nzb")
                    .map_err(|e| DownloadError::Client(e.to_string()))?;
                self.http
                    .post(url)
                    .query(&query)
                    .multipart(reqwest::multipart::Form::new().part("name", part))
            }
            None => self.http.get(url).query(&query),
        };
        let resp = req
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| DownloadError::Connection(crate::error::cause(&e)))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(DownloadError::Authentication);
        }
        if !status.is_success() {
            return Err(DownloadError::Http {
                status: status.as_u16(),
            });
        }
        let body = resp
            .text()
            .await
            .map_err(|e| DownloadError::Connection(crate::error::cause(&e)))?;
        check_for_error(&body)?;
        Ok(body)
    }

    async fn json(
        &self,
        s: &Settings,
        mode: &str,
        params: &[(&str, String)],
    ) -> Result<Value, DownloadError> {
        let body = self.call(s, mode, params, None).await?;
        serde_json::from_str(&body)
            .map_err(|e| DownloadError::Client(format!("SABnzbd sent an unreadable answer ({e})")))
    }

    /// `GetVersion`.
    pub async fn version(&self, s: &Settings) -> Result<String, DownloadError> {
        let body = self.call(s, "version", &[], None).await?;
        Ok(serde_json::from_str::<Value>(&body)
            .ok()
            .map(|v| string(v.get("version")))
            .unwrap_or_default())
    }

    /// `GetConfig`.
    async fn config(&self, s: &Settings) -> Result<Value, DownloadError> {
        Ok(self
            .json(s, "get_config", &[])
            .await?
            .get("config")
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// `GetCategories`: each category's directory, made absolute against the complete folder.
    async fn categories(
        &self,
        s: &Settings,
        config: &Value,
    ) -> Result<Vec<(String, String, String)>, DownloadError> {
        let misc = config.get("misc").cloned().unwrap_or(Value::Null);
        let mut complete = string(misc.get("complete_dir"));
        if !is_rooted(&complete) {
            let version = parse_version(&self.version(s).await?).unwrap_or((0, 0, 0));
            if version >= (2, 0, 0) {
                let status = self
                    .json(s, "fullstatus", &[("skip_dashboard", "1".into())])
                    .await?;
                complete = string(status.get("status").and_then(|v| v.get("completedir")));
            } else {
                let queue = self
                    .json(s, "queue", &[("start", "0".into()), ("limit", "1".into())])
                    .await?;
                let home = string(queue.get("queue").and_then(|q| q.get("my_home")));
                complete = join(&home, &complete);
            }
        }
        Ok(config
            .get("categories")
            .and_then(Value::as_array)
            .map(|cats| {
                cats.iter()
                    .map(|c| {
                        let dir = string(c.get("dir"));
                        (
                            string(c.get("name")),
                            dir.clone(),
                            join(&complete, dir.trim_end_matches('*')),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `DownloadNzb` / `AddFromNzbFile`: returns the `nzo_id`.
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
        let body = self
            .call(
                s,
                "addfile",
                &[
                    ("cat", s.category.clone().unwrap_or_default()),
                    ("priority", priority.to_string()),
                ],
                Some((name, content)),
            )
            .await?;
        let ids = serde_json::from_str::<Value>(&body)
            .ok()
            .map(|v| strings(v.get("nzo_ids")))
            .unwrap_or_default();
        ids.into_iter().find(|i| !i.is_empty()).ok_or_else(|| {
            DownloadError::Rejected("SABnzbd rejected the NZB for an unknown reason".into())
        })
    }

    /// `GetQueue` + `GetHistory`, filtered by category as `GetItems` does.
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let mut params = vec![("start", "0".to_string()), ("limit", "0".to_string())];
        if let Some(c) = &s.category {
            params.push(("category", c.clone()));
        }
        let queue = self.json(s, "queue", &params).await?;
        let queue = queue.get("queue").cloned().unwrap_or(Value::Null);
        let mut out = queue_items(&queue);
        let mut params = vec![("start", "0".to_string()), ("limit", "60".to_string())];
        if let Some(c) = &s.category {
            params.push(("category", c.clone()));
        }
        let history = self.json(s, "history", &params).await?;
        out.extend(history_items(
            history.get("history").unwrap_or(&Value::Null),
        ));
        Ok(out
            .into_iter()
            .filter(|i| match &s.category {
                Some(c) => &i.category == c,
                None => i.category == "*",
            })
            .collect())
    }

    /// `RemoveItem`: from the queue, else from the history (`archive=1` keeps a failed job in
    /// SABnzbd's archive).
    pub async fn remove(
        &self,
        s: &Settings,
        id: &str,
        delete_data: bool,
        failed: bool,
    ) -> Result<(), DownloadError> {
        let queue = self
            .json(s, "queue", &[("start", "0".into()), ("limit", "0".into())])
            .await?;
        let in_queue = queue
            .get("queue")
            .map(queue_items)
            .unwrap_or_default()
            .iter()
            .any(|i| i.download_id == id);
        let del = if delete_data { "1" } else { "0" };
        if in_queue {
            self.call(
                s,
                "queue",
                &[
                    ("name", "delete".into()),
                    ("del_files", del.into()),
                    ("value", id.into()),
                ],
                None,
            )
            .await?;
        } else {
            self.call(
                s,
                "history",
                &[
                    ("name", "delete".into()),
                    ("del_files", del.into()),
                    ("value", id.into()),
                    ("archive", if failed { "0" } else { "1" }.into()),
                ],
                None,
            )
            .await?;
        }
        Ok(())
    }

    /// `Test`: connection and version, authentication, global configuration and category.
    pub async fn test(&self, s: &Settings) -> Result<TestReport, DownloadError> {
        let raw = self.version(s).await?;
        let mut warnings = Vec::new();
        let Some(version) = parse_version(&raw) else {
            return Err(DownloadError::Client(format!(
                "unknown SABnzbd version {raw:?}"
            )));
        };
        if raw.eq_ignore_ascii_case("develop") {
            warnings.push("SABnzbd is a develop version: Kitsunarr assumes version 3.0.0 or newer, and may not support new features".into());
        } else if version < (0, 7, 0) {
            return Err(DownloadError::Client(format!(
                "SABnzbd {raw} is too old; version 0.7.0 or newer is required"
            )));
        }
        let config = match self.config(s).await {
            Ok(c) => c,
            Err(DownloadError::Client(m)) if m.to_lowercase().contains("api key required") => {
                return Err(DownloadError::Client("SABnzbd requires an API key".into()));
            }
            Err(e) => return Err(e),
        };
        let misc = config.get("misc").cloned().unwrap_or(Value::Null);
        if flag(misc.get("pre_check")) && version < (1, 1, 0) {
            return Err(DownloadError::Client(
                "disable SABnzbd's \"Check before download\" option: Kitsunarr cannot follow downloads while it runs (SABnzbd 1.1 and newer handle it)".into(),
            ));
        }
        let category = s.category.as_deref();
        let categories = self.categories(s, &config).await?;
        match categories.iter().find(|c| Some(c.0.as_str()) == category) {
            Some((_, dir, _)) if dir.ends_with('*') => {
                return Err(DownloadError::Client(
                    "enable job folders: the category's folder in SABnzbd ends with '*', so each download would not get its own folder".into(),
                ));
            }
            Some(_) => {}
            None if category.is_some() => {
                return Err(DownloadError::Client(format!(
                    "the category {:?} does not exist in SABnzbd; create it there first",
                    category.unwrap_or_default()
                )));
            }
            None => warnings.push(
                "A category is recommended: it keeps Kitsunarr's downloads apart from everything else in SABnzbd".into(),
            ),
        }
        let sorting = "SABnzbd's sorting moves files where Kitsunarr cannot find them";
        let sorters = config
            .get("sorters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if sorters.iter().any(|x| {
            flag(x.get("is_active")) && contains_category(&strings(x.get("sort_cats")), category)
        }) || (flag(misc.get("enable_tv_sorting"))
            && contains_category(&strings(misc.get("tv_categories")), category))
        {
            return Err(DownloadError::Client(format!(
                "disable TV sorting for the category: {sorting}"
            )));
        }
        if flag(misc.get("enable_movie_sorting"))
            && contains_category(&strings(misc.get("movie_categories")), category)
        {
            return Err(DownloadError::Client(format!(
                "disable movie sorting for the category: {sorting}"
            )));
        }
        if flag(misc.get("enable_date_sorting"))
            && contains_category(&strings(misc.get("date_categories")), category)
        {
            return Err(DownloadError::Client(format!(
                "disable date sorting for the category: {sorting}"
            )));
        }
        if removes_completed_downloads(&misc) {
            warnings.push("SABnzbd removes completed downloads from its history quickly; Kitsunarr may not see them to import them. Keep at least 14 days of history".into());
        }
        Ok(TestReport {
            version: raw,
            warnings,
        })
    }
}

/// `CheckForError`: a JSON `{"status": false, "error": ..}`, or plain text starting "error".
fn check_for_error(body: &str) -> Result<(), DownloadError> {
    let failed = |m: &str| {
        Err(DownloadError::Client(format!(
            "Error response received from SABnzbd: {m}"
        )))
    };
    match serde_json::from_str::<Value>(body) {
        Ok(v) => {
            let status = v.get("status");
            let is_false = match status {
                Some(Value::Bool(b)) => !b,
                Some(Value::String(s)) => s.eq_ignore_ascii_case("false"),
                _ => false,
            };
            if is_false {
                let error = string(v.get("error"));
                if error.to_lowercase().contains("api key incorrect") {
                    return Err(DownloadError::Authentication);
                }
                return failed(&error);
            }
            Ok(())
        }
        Err(_) => {
            if body.len() >= 5 && body[..5].eq_ignore_ascii_case("error") {
                if body.to_lowercase().contains("api key incorrect") {
                    return Err(DownloadError::Authentication);
                }
                return failed(body.replacen("error: ", "", 1).trim());
            }
            Ok(())
        }
    }
}

/// `GetQueue`'s mapping.
fn queue_items(queue: &Value) -> Vec<DownloadItem> {
    let paused = flag(queue.get("paused"));
    let slots = queue
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    slots
        .iter()
        .filter(|q| status_of(q) != "Deleted")
        .map(|q| {
            let status = status_of(q);
            let mut remaining = parse_time_left(&string(q.get("timeleft")));
            let item_status =
                if (paused && string(q.get("priority")) != "Force") || status == "Paused" {
                    remaining = None;
                    ItemStatus::Paused
                } else if matches!(status.as_str(), "Queued" | "Grabbing" | "Propagating") {
                    ItemStatus::Queued
                } else {
                    ItemStatus::Downloading
                };
            let mut title = string(q.get("filename"));
            if let Some(rest) = title.strip_prefix("ENCRYPTED /") {
                title = rest.trim_start().to_string();
            }
            DownloadItem {
                download_id: string(q.get("nzo_id")),
                title,
                category: string(q.get("cat")),
                total_size: (num(q.get("mb")) * 1024.0 * 1024.0) as i64,
                remaining_size: (num(q.get("mbleft")) * 1024.0 * 1024.0) as i64,
                remaining_seconds: remaining,
                status: item_status,
                message: None,
                output_path: None,
                seed_ratio: 0.0,
            }
        })
        .collect()
}

/// `GetHistory`'s mapping. The output path is the job folder: the storage path, or the
/// outermost parent named after the job.
fn history_items(history: &Value) -> Vec<DownloadItem> {
    let slots = history
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    slots
        .iter()
        .filter(|h| status_of(h) != "Deleted")
        .map(|h| {
            let fail = string(h.get("fail_message"));
            let status = match status_of(h).as_str() {
                "Failed"
                    if fail
                        .eq_ignore_ascii_case("Unpacking failed, write error or disk is full?") =>
                {
                    ItemStatus::Warning
                }
                "Failed" => ItemStatus::Failed,
                "Completed" => ItemStatus::Completed,
                _ => ItemStatus::Downloading,
            };
            let title = string(h.get("name"));
            let storage = string(h.get("storage"));
            let output_path = (!storage.trim().is_empty()).then(|| {
                let mut out = storage.clone();
                let sep = |c: char| c == '/' || c == '\\';
                let mut parent = storage.as_str();
                while let Some(i) = parent.trim_end_matches(sep).rfind(sep) {
                    parent = &parent[..i];
                    let name = parent.rsplit(sep).next().unwrap_or_default();
                    if !name.is_empty() && name == title {
                        out = parent.to_string();
                    }
                }
                out
            });
            DownloadItem {
                download_id: string(h.get("nzo_id")),
                title,
                category: string(h.get("category")),
                total_size: num(h.get("bytes")) as i64,
                remaining_size: 0,
                remaining_seconds: Some(0),
                status,
                message: (!fail.is_empty()).then_some(fail),
                output_path,
                seed_ratio: 0.0,
            }
        })
        .collect()
}

/// `RemovesCompletedDownloads`.
fn removes_completed_downloads(misc: &Value) -> bool {
    let number = num(misc.get("history_retention_number"));
    match string(misc.get("history_retention_option")).as_str() {
        "all" => return false,
        "number-archive" | "number-delete" | "all-archive" | "all-delete" => return true,
        "days-archive" | "days-delete" => return number < 14.0,
        _ => {}
    }
    let retention = string(misc.get("history_retention"));
    if retention.trim().is_empty() {
        return false;
    }
    if let Some(days) = retention.strip_suffix('d') {
        return days.parse::<i64>().unwrap_or(0) < 14;
    }
    retention != "0"
}

fn is_rooted(p: &str) -> bool {
    p.starts_with('/')
        || p.starts_with('\\')
        || (p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic())
}

/// `OsPath` addition: a rooted right side wins; otherwise the two are joined.
fn join(left: &str, right: &str) -> String {
    if is_rooted(right) || left.is_empty() {
        return right.to_string();
    }
    if right.is_empty() {
        return left.to_string();
    }
    let sep = if left.contains('\\') && !left.contains('/') {
        '\\'
    } else {
        '/'
    };
    format!("{}{sep}{}", left.trim_end_matches(['/', '\\']), right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions_parse_as_upstream() {
        assert_eq!(parse_version("4.3.2"), Some((4, 3, 2)));
        assert_eq!(parse_version("2.x.x"), None);
        assert_eq!(parse_version("1.2.x"), Some((1, 2, 0)));
        assert_eq!(parse_version("0.7.20Beta"), Some((0, 7, 20)));
        assert_eq!(parse_version("develop"), Some((3, 0, 0)));
        assert_eq!(parse_version("nonsense"), None);
    }

    #[test]
    fn queue_states_follow_sonarr() {
        let q = json!({
            "paused": false,
            "slots": [
                {"status": "Downloading", "nzo_id": "a", "filename": "ENCRYPTED / Show.S01E01", "cat": "tv",
                 "mb": "1000", "mbleft": "250.5", "timeleft": "0:01:40", "priority": "Normal"},
                {"status": "Queued", "nzo_id": "b", "filename": "x", "cat": "tv", "mb": 1, "mbleft": 1, "timeleft": "1:00:00:00"},
                {"status": "Paused", "nzo_id": "c", "filename": "x", "cat": "tv", "timeleft": "0:00:10"},
                {"status": "Deleted", "nzo_id": "d"}
            ]
        });
        let items = queue_items(&q);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].title, "Show.S01E01");
        assert_eq!(items[0].status, ItemStatus::Downloading);
        assert_eq!(items[0].total_size, 1000 * 1024 * 1024);
        assert_eq!(items[0].remaining_seconds, Some(100));
        assert_eq!(items[1].status, ItemStatus::Queued);
        assert_eq!(items[1].remaining_seconds, Some(86400));
        assert_eq!(items[2].status, ItemStatus::Paused);
        assert_eq!(items[2].remaining_seconds, None);
        // A paused queue pauses everything but forced jobs.
        let q = json!({"paused": true, "slots": [
            {"status": "Downloading", "nzo_id": "a", "priority": "Force", "timeleft": "0:0:1"},
            {"status": "Downloading", "nzo_id": "b", "priority": "High", "timeleft": "0:0:1"}]});
        let items = queue_items(&q);
        assert_eq!(items[0].status, ItemStatus::Downloading);
        assert_eq!(items[1].status, ItemStatus::Paused);
    }

    #[test]
    fn history_states_and_job_folder() {
        let h = json!({"slots": [
            {"status": "Completed", "nzo_id": "a", "name": "Show.S01E01", "category": "tv", "bytes": 1234,
             "storage": "/downloads/complete/tv/Show.S01E01/sub/file.mkv"},
            {"status": "Failed", "nzo_id": "b", "name": "x", "fail_message": "Unpacking failed, write error or disk is full?", "storage": ""},
            {"status": "Failed", "nzo_id": "c", "name": "x", "fail_message": "Aborted"},
            {"status": "Verifying", "nzo_id": "d", "name": "x", "storage": "/downloads/x"}
        ]});
        let items = history_items(&h);
        assert_eq!(items[0].status, ItemStatus::Completed);
        assert_eq!(
            items[0].output_path.as_deref(),
            Some("/downloads/complete/tv/Show.S01E01")
        );
        assert_eq!(items[0].total_size, 1234);
        assert_eq!(items[1].status, ItemStatus::Warning);
        assert_eq!(items[1].output_path, None);
        assert_eq!(items[2].status, ItemStatus::Failed);
        assert_eq!(items[2].message.as_deref(), Some("Aborted"));
        assert_eq!(items[3].status, ItemStatus::Downloading);
        assert_eq!(items[3].output_path.as_deref(), Some("/downloads/x"));
    }

    #[test]
    fn errors_and_retention() {
        assert!(check_for_error(r#"{"status": false, "error": "API Key Incorrect"}"#).is_err());
        assert!(check_for_error("error: API Key Required").is_err());
        assert!(check_for_error("ok\n").is_ok());
        assert!(check_for_error(r#"{"status": true, "nzo_ids": ["x"]}"#).is_ok());
        assert!(!removes_completed_downloads(
            &json!({"history_retention_option": "all"})
        ));
        assert!(removes_completed_downloads(
            &json!({"history_retention_option": "days-delete", "history_retention_number": 7})
        ));
        assert!(!removes_completed_downloads(
            &json!({"history_retention": ""})
        ));
        assert!(removes_completed_downloads(
            &json!({"history_retention": "7d"})
        ));
        assert!(!removes_completed_downloads(
            &json!({"history_retention": "30d"})
        ));
        assert!(!removes_completed_downloads(
            &json!({"history_retention": "0"})
        ));
        assert_eq!(
            join("/home/sab", "Downloads/complete"),
            "/home/sab/Downloads/complete"
        );
        assert_eq!(join("/home/sab", "/abs"), "/abs");
        assert!(contains_category(&[], Some("tv")));
        assert!(contains_category(&["Default".into()], None));
        assert!(!contains_category(&["movies".into()], Some("tv")));
    }
}
