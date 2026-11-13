//! NZBGet through its JSON-RPC API (Sonarr's `Nzbget.cs` and `NzbgetProxy.cs`).
//!
//! Calls are `POST {base}/jsonrpc` with HTTP basic authentication. NZBGet 12 or newer is
//! needed, as upstream. Downloads are tracked by a `drone` parameter with a random id: NZBGet 16
//! takes it with `append`, older versions get it set on the new queue item. Sizes arrive as two
//! 32-bit halves.

use serde_json::{Map, Value, json};

use crate::error::DownloadError;
use crate::usenet::{base_url, host, num, port, string, text};
use crate::{DownloadItem, ItemStatus};

/// Settings, as stored with Sonarr's field names.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub use_ssl: bool,
    pub url_base: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub category: Option<String>,
    /// `NzbgetPriority`: -100, -50, 0, 50, 100, 900 (force).
    pub recent_priority: i64,
    pub older_priority: i64,
    pub add_paused: bool,
}

impl Settings {
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let username = text(map, "username");
        let password = text(map, "password");
        if username.is_some() != password.is_some() {
            return Err("enter both the username and the password, or neither".into());
        }
        Ok(Self {
            host: host(map)?,
            port: port(map, 6789)?,
            use_ssl: map.get("useSsl").and_then(Value::as_bool).unwrap_or(false),
            url_base: text(map, "urlBase").unwrap_or_default(),
            username,
            password,
            category: match map.get("tvCategory") {
                None | Some(Value::Null) => Some("tv".into()),
                _ => text(map, "tvCategory"),
            },
            recent_priority: map
                .get("recentTvPriority")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            older_priority: map
                .get("olderTvPriority")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            add_paused: map
                .get("addPaused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub fn url(&self, path: &str) -> String {
        base_url(self.use_ssl, &self.host, self.port, &self.url_base, path)
    }
}

/// `MakeInt64`.
fn int64(v: &Value, hi: &str, lo: &str) -> i64 {
    ((num(v.get(hi)) as u32 as i64) << 32) | (num(v.get(lo)) as u32 as i64)
}

/// The `drone` parameter's value, else NZBGet's own id.
fn download_id(v: &Value, id_key: &str) -> String {
    v.get("Parameters")
        .and_then(Value::as_array)
        .and_then(|ps| {
            ps.iter()
                .find(|p| string(p.get("Name")) == "drone")
                .map(|p| string(p.get("Value")))
        })
        .unwrap_or_else(|| string(v.get(id_key)))
}

const SUCCESS: [&str; 2] = ["SUCCESS", "NONE"];
const DELETE_FAILED: [&str; 5] = ["HEALTH", "DUPE", "SCAN", "COPY", "BAD"];

/// `GetQueue`'s mapping (`listgroups` with `status`).
fn queue_items(status: &Value, groups: &[Value]) -> Vec<DownloadItem> {
    let paused = status
        .get("DownloadPaused")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rate = num(status.get("DownloadRate")) as i64;
    let mut total_remaining = 0i64;
    groups
        .iter()
        .map(|g| {
            let total = int64(g, "FileSizeHi", "FileSizeLo");
            let paused_size = int64(g, "PausedSizeHi", "PausedSizeLo");
            let remaining = int64(g, "RemainingSizeHi", "RemainingSizeLo");
            let (state, remaining_size, seconds) =
                if paused || (remaining == paused_size && remaining != 0) {
                    (ItemStatus::Paused, remaining, None)
                } else {
                    let state = if num(g.get("ActiveDownloads")) == 0.0 && remaining != 0 {
                        ItemStatus::Queued
                    } else {
                        ItemStatus::Downloading
                    };
                    let left = remaining - paused_size;
                    let seconds = (rate != 0).then(|| {
                        let s = (total_remaining + left) / rate;
                        total_remaining += left;
                        s
                    });
                    (state, left, seconds)
                };
            DownloadItem {
                download_id: download_id(g, "NZBID"),
                title: string(g.get("NZBName")),
                category: string(g.get("Category")),
                total_size: total,
                remaining_size,
                remaining_seconds: seconds,
                status: state,
                message: None,
                output_path: None,
                seed_ratio: 0.0,
                can_be_removed: true,
                seed_progress: None,
            }
        })
        .collect()
}

/// `GetHistory`'s mapping.
fn history_items(history: &[Value]) -> Vec<DownloadItem> {
    let mut out = Vec::new();
    for h in history.iter().take(60) {
        let field = |k: &str| string(h.get(k));
        let (par, unpack, mv, script, delete, mark) = (
            field("ParStatus"),
            field("UnpackStatus"),
            field("MoveStatus"),
            field("ScriptStatus"),
            field("DeleteStatus"),
            field("MarkStatus"),
        );
        let dir = Some(field("FinalDir"))
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| field("DestDir"));
        let mut item = DownloadItem {
            download_id: download_id(h, "ID"),
            title: field("Name"),
            category: field("Category"),
            total_size: int64(h, "FileSizeHi", "FileSizeLo"),
            remaining_size: 0,
            remaining_seconds: Some(0),
            status: ItemStatus::Completed,
            message: Some(format!(
                "PAR Status: {par} - Unpack Status: {unpack} - Move Status: {mv} - Script Status: {script} - Delete Status: {delete} - Mark Status: {mark}"
            )),
            output_path: (!dir.trim().is_empty()).then_some(dir),
            seed_ratio: 0.0,
            can_be_removed: true,
            seed_progress: None,
        };
        if delete == "MANUAL" {
            if mark == "BAD" {
                item.status = ItemStatus::Failed;
                out.push(item);
            }
            continue;
        }
        let ok = |s: &str| SUCCESS.contains(&s);
        if !ok(&par) {
            item.status = ItemStatus::Failed;
        }
        if unpack == "SPACE" {
            item.status = ItemStatus::Warning;
        } else if !ok(&unpack) {
            item.status = ItemStatus::Failed;
        }
        if !ok(&mv) {
            item.status = ItemStatus::Warning;
        }
        if !ok(&script) {
            item.status = ItemStatus::Failed;
        }
        if !ok(&delete) && !delete.trim().is_empty() {
            item.status = if DELETE_FAILED.contains(&delete.as_str()) {
                ItemStatus::Failed
            } else {
                ItemStatus::Warning
            };
        }
        out.push(item);
    }
    out
}

/// `GetCategories`: names and destination folders from the configuration.
fn categories(config: &[(String, String)]) -> Vec<(String, String)> {
    let get = |k: &str| config.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let mut out = Vec::new();
    for i in 1..100 {
        let Some(name) = get(&format!("Category{i}.Name")) else {
            break;
        };
        let mut dest = get(&format!("Category{i}.DestDir")).unwrap_or_default();
        if dest.trim().is_empty() {
            let main = get("MainDir").unwrap_or_default();
            dest = get("DestDir")
                .unwrap_or_default()
                .replace("${MainDir}", &main);
            if get("AppendCategoryDir").as_deref().unwrap_or("yes") == "yes" {
                dest = format!("{}/{name}", dest.trim_end_matches(['/', '\\']));
            }
        }
        out.push((name, dest));
    }
    out
}

/// What a test found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub version: String,
    pub warnings: Vec<String>,
}

/// An NZBGet JSON-RPC client.
#[derive(Clone, Default)]
pub struct Nzbget {
    http: reqwest::Client,
}

impl Nzbget {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// `ProcessRequest`.
    async fn call(
        &self,
        s: &Settings,
        method: &str,
        params: Value,
    ) -> Result<Value, DownloadError> {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": 1 });
        let mut req = self
            .http
            .post(s.url("jsonrpc"))
            .header(
                reqwest::header::ACCEPT,
                "application/json-rpc, application/json",
            )
            .json(&body);
        if let Some(u) = &s.username {
            req = req.basic_auth(u, s.password.as_deref());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| DownloadError::Connection(crate::error::cause(&e)))?;
        match resp.status() {
            reqwest::StatusCode::UNAUTHORIZED => return Err(DownloadError::Authentication),
            s if !s.is_success() => return Err(DownloadError::Http { status: s.as_u16() }),
            _ => {}
        }
        let value: Value = resp.json().await.map_err(|e| {
            DownloadError::Client(format!("NZBGet sent an unreadable answer ({e})"))
        })?;
        if let Some(err) = value.get("error").filter(|e| !e.is_null()) {
            let message = err
                .get("message")
                .map(|m| string(Some(m)))
                .unwrap_or_else(|| err.to_string());
            if message.to_lowercase().contains("authentication failed") {
                return Err(DownloadError::Authentication);
            }
            return Err(DownloadError::Client(format!(
                "Error response received from NZBGet: {message}"
            )));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn version(&self, s: &Settings) -> Result<String, DownloadError> {
        Ok(string(Some(&self.call(s, "version", json!([])).await?)))
    }

    async fn major(&self, s: &Settings) -> Result<u64, DownloadError> {
        let v = self.version(s).await?;
        Ok(v.split(['.', '-'])
            .next()
            .and_then(|m| m.trim().parse().ok())
            .unwrap_or(0))
    }

    async fn config(&self, s: &Settings) -> Result<Vec<(String, String)>, DownloadError> {
        let list = self.call(s, "config", json!([])).await?;
        Ok(list
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| (string(c.get("Name")), string(c.get("Value"))))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn list(&self, s: &Settings, method: &str) -> Result<Vec<Value>, DownloadError> {
        Ok(self
            .call(s, method, json!([]))
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// `DownloadNzb`: returns the `drone` id. NZBGet 16+ takes it with `append`; 13–15 get it
    /// set on the new queue item (`DownloadNzbLegacy13`); 12 has the old `append` signature,
    /// and the item is found by name (`DownloadNzbLegacy12`).
    pub async fn add(
        &self,
        s: &Settings,
        name: &str,
        content: &[u8],
        recent: bool,
    ) -> Result<String, DownloadError> {
        use base64::Engine as _;
        let major = self.major(s).await?;
        let priority = if recent {
            s.recent_priority
        } else {
            s.older_priority
        };
        let nzb = base64::engine::general_purpose::STANDARD.encode(content);
        let category = s.category.clone().unwrap_or_default();
        let rejected =
            || DownloadError::Rejected("NZBGet rejected the NZB for an unknown reason".into());
        let drone = uuid_hex();
        if major >= 16 {
            let id = self
                .call(
                    s,
                    "append",
                    json!([
                        name,
                        nzb,
                        category,
                        priority,
                        false,
                        s.add_paused,
                        "",
                        0,
                        "all",
                        ["drone", drone]
                    ]),
                )
                .await?;
            if num(Some(&id)) <= 0.0 {
                return Err(rejected());
            }
            return Ok(drone);
        }
        let group_id = if major >= 13 {
            let id = self
                .call(
                    s,
                    "append",
                    json!([name, nzb, category, priority, false, false, "", 0, "all"]),
                )
                .await?;
            let id = num(Some(&id)) as i64;
            if id <= 0 {
                return Err(rejected());
            }
            let groups = self.list(s, "listgroups").await?;
            groups
                .iter()
                .find(|g| num(g.get("NZBID")) as i64 == id)
                .map(|g| num(g.get("NZBID")) as i64)
        } else {
            let ok = self
                .call(s, "append", json!([name, category, priority, false, nzb]))
                .await?;
            if ok != Value::Bool(true) {
                return Err(rejected());
            }
            let title = name.strip_suffix(".nzb").unwrap_or(name);
            let groups = self.list(s, "listgroups").await?;
            groups
                .iter()
                .find(|g| string(g.get("NZBName")) == title)
                .map(|g| num(g.get("LastID")) as i64)
        };
        let Some(group_id) = group_id else {
            return Err(rejected());
        };
        let set = self
            .call(
                s,
                "editqueue",
                json!(["GroupSetParameter", 0, format!("drone={drone}"), group_id]),
            )
            .await?;
        if set != Value::Bool(true) {
            tracing::warn!(group_id, "NZBGet did not keep the download's drone id");
        }
        Ok(drone)
    }

    /// `GetItems`: queue and history in Kitsunarr's category.
    pub async fn items(&self, s: &Settings) -> Result<Vec<DownloadItem>, DownloadError> {
        let status = self.call(s, "status", json!([])).await?;
        let groups = self.list(s, "listgroups").await?;
        let history = self.list(s, "history").await?;
        let category = s.category.clone().unwrap_or_default();
        Ok(queue_items(&status, &groups)
            .into_iter()
            .chain(history_items(&history))
            .filter(|i| i.category == category)
            .collect())
    }

    /// `RemoveItem`: `GroupFinalDelete` from the queue, else `HistoryDelete`.
    pub async fn remove(&self, s: &Settings, id: &str) -> Result<(), DownloadError> {
        let groups = self.list(s, "listgroups").await?;
        let history = self.list(s, "history").await?;
        let numeric = (id.len() < 10).then(|| id.parse::<i64>().ok()).flatten();
        let matches = |v: &Value, key: &str| match numeric {
            Some(n) => num(v.get(key)) as i64 == n,
            None => download_id(v, key) == id,
        };
        let (command, nzb) = if let Some(g) = groups.iter().find(|g| matches(g, "NZBID")) {
            ("GroupFinalDelete", num(g.get("NZBID")) as i64)
        } else if let Some(h) = history.iter().find(|h| matches(h, "ID")) {
            ("HistoryDelete", num(h.get("ID")) as i64)
        } else {
            tracing::warn!(%id, "NZBGet does not know this download");
            return Ok(());
        };
        let ok = self
            .call(s, "editqueue", json!([command, 0, "", nzb]))
            .await?;
        if ok != Value::Bool(true) {
            tracing::warn!(%id, command, "NZBGet did not remove the download");
        }
        Ok(())
    }

    /// `Test`: version 12+, the category exists, and the history is kept.
    pub async fn test(&self, s: &Settings) -> Result<TestReport, DownloadError> {
        let version = self.version(s).await?;
        let mut warnings = Vec::new();
        let major = version
            .split(['.', '-'])
            .next()
            .and_then(|m| m.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if major < 12 {
            return Err(DownloadError::Client(format!(
                "NZBGet {version} is too old; version 12 or newer is required"
            )));
        }
        let config = self.config(s).await?;
        match &s.category {
            Some(c) if !categories(&config).iter().any(|(n, _)| n == c) => {
                return Err(DownloadError::Client(format!(
                    "the category {c:?} does not exist in NZBGet; create it there first"
                )));
            }
            Some(_) => {}
            None => warnings.push(
                "A category is recommended: it keeps Kitsunarr's downloads apart from everything else in NZBGet".into(),
            ),
        }
        let keep = config
            .iter()
            .find(|(n, _)| n == "KeepHistory")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "7".into());
        match keep.trim().parse::<u64>() {
            Ok(0) | Err(_) => {
                return Err(DownloadError::Client(
                    "NZBGet's KeepHistory is 0: Kitsunarr cannot see completed downloads. Set it to a number of days".into(),
                ));
            }
            Ok(n) if n > 25000 => {
                return Err(DownloadError::Client(
                    "NZBGet's KeepHistory is above the maximum of 25000 days".into(),
                ));
            }
            Ok(_) => {}
        }
        Ok(TestReport { version, warnings })
    }
}

/// A random 32-character hex id (`Guid.NewGuid().ToString().Replace("-", "")`).
fn uuid_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl Nzbget {
    /// `GetStatus`: the category's destination folder.
    pub async fn status(&self, s: &Settings) -> Result<crate::ClientStatus, DownloadError> {
        let config = self.config(s).await?;
        let dest = categories(&config)
            .into_iter()
            .find(|(n, _)| Some(n.as_str()) == s.category.as_deref())
            .map(|(_, d)| d);
        Ok(crate::ClientStatus {
            is_localhost: crate::is_localhost(&s.host),
            output_root_folders: dest.into_iter().filter(|d| !d.is_empty()).collect(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_join_their_halves() {
        let v = json!({"FileSizeHi": 1, "FileSizeLo": 5});
        assert_eq!(int64(&v, "FileSizeHi", "FileSizeLo"), (1i64 << 32) + 5);
    }

    #[test]
    fn queue_states_and_time_left() {
        let status = json!({"DownloadPaused": false, "DownloadRate": 100});
        let groups = vec![
            json!({"NZBID": 1, "NZBName": "a", "Category": "tv", "FileSizeLo": 1000, "RemainingSizeLo": 500,
                   "PausedSizeLo": 0, "ActiveDownloads": 1, "Parameters": [{"Name": "drone", "Value": "abc"}]}),
            json!({"NZBID": 2, "NZBName": "b", "Category": "tv", "FileSizeLo": 1000, "RemainingSizeLo": 300,
                   "PausedSizeLo": 0, "ActiveDownloads": 0, "Parameters": []}),
            json!({"NZBID": 3, "NZBName": "c", "Category": "tv", "FileSizeLo": 1000, "RemainingSizeLo": 300,
                   "PausedSizeLo": 300, "ActiveDownloads": 0}),
        ];
        let items = queue_items(&status, &groups);
        assert_eq!(items[0].download_id, "abc");
        assert_eq!(items[0].status, ItemStatus::Downloading);
        assert_eq!(items[0].remaining_seconds, Some(5));
        assert_eq!(items[1].download_id, "2");
        assert_eq!(items[1].status, ItemStatus::Queued);
        // Queued behind the first: (500 + 300) / 100.
        assert_eq!(items[1].remaining_seconds, Some(8));
        assert_eq!(items[2].status, ItemStatus::Paused);
    }

    #[test]
    fn history_states_follow_sonarr() {
        let h = |par: &str, unpack: &str, mv: &str, script: &str, delete: &str, mark: &str| {
            json!({"ID": 7, "Name": "x", "Category": "tv", "ParStatus": par, "UnpackStatus": unpack,
                   "MoveStatus": mv, "ScriptStatus": script, "DeleteStatus": delete, "MarkStatus": mark,
                   "DestDir": "/d/x", "FinalDir": ""})
        };
        let st = |v: Value| history_items(&[v]).first().map(|i| i.status);
        assert_eq!(
            st(h("SUCCESS", "SUCCESS", "SUCCESS", "NONE", "NONE", "NONE")),
            Some(ItemStatus::Completed)
        );
        assert_eq!(
            st(h("FAILURE", "NONE", "SUCCESS", "NONE", "", "")),
            Some(ItemStatus::Failed)
        );
        assert_eq!(
            st(h("SUCCESS", "SPACE", "SUCCESS", "NONE", "", "")),
            Some(ItemStatus::Warning)
        );
        assert_eq!(
            st(h("SUCCESS", "SUCCESS", "FAILURE", "NONE", "", "")),
            Some(ItemStatus::Warning)
        );
        assert_eq!(
            st(h("SUCCESS", "SUCCESS", "SUCCESS", "NONE", "HEALTH", "")),
            Some(ItemStatus::Failed)
        );
        assert_eq!(
            st(h("SUCCESS", "SUCCESS", "SUCCESS", "NONE", "MANUAL", "")),
            None
        );
        assert_eq!(
            st(h("SUCCESS", "SUCCESS", "SUCCESS", "NONE", "MANUAL", "BAD")),
            Some(ItemStatus::Failed)
        );
        let item = &history_items(&[h("SUCCESS", "SUCCESS", "SUCCESS", "NONE", "", "")])[0];
        assert_eq!(item.output_path.as_deref(), Some("/d/x"));
    }

    #[test]
    fn categories_fall_back_to_the_destination_folder() {
        let c = vec![
            ("MainDir".to_string(), "/data".to_string()),
            ("DestDir".to_string(), "${MainDir}/dst".to_string()),
            ("Category1.Name".to_string(), "tv".to_string()),
            ("Category1.DestDir".to_string(), String::new()),
            ("Category2.Name".to_string(), "movies".to_string()),
            ("Category2.DestDir".to_string(), "/m".to_string()),
        ];
        assert_eq!(
            categories(&c),
            vec![
                ("tv".to_string(), "/data/dst/tv".to_string()),
                ("movies".to_string(), "/m".to_string())
            ]
        );
        assert_eq!(uuid_hex().len(), 32);
    }
}
