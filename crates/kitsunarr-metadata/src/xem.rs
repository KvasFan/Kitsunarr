//! TheXEM (<https://thexem.info>): scene numbering and scene names of TheTVDB series, as
//! Sonarr's `XemProxy` reads them. Requests carry `origin=tvdb`; numbering is keyed by TheTVDB
//! id.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::TtlCache;

pub const DEFAULT_BASE: &str = "https://thexem.info";

/// Failure messages that only mean "nothing mapped" (`XemProxy.IgnoredErrors`).
const IGNORED_ERRORS: [&str; 2] = ["no single connection", "no show with the tvdb_id"];

/// How long the list of mapped series and the scene names are kept (Sonarr: 3 hours).
const CACHE_TTL: Duration = Duration::from_secs(3 * 3600);

/// One numbering of an episode (`XemValues`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct XemValues {
    #[serde(default)]
    pub season: i32,
    #[serde(default)]
    pub episode: i32,
    #[serde(default)]
    pub absolute: i32,
}

/// An episode's scene numbering against TheTVDB's (`XemSceneTvdbMapping`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XemMapping {
    pub scene: XemValues,
    pub tvdb: XemValues,
}

/// A scene name of a series: `season` is the scene season it names, `None` for every season
/// (XEM's `-1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemName {
    pub title: String,
    pub season: Option<i32>,
}

#[derive(Deserialize)]
struct XemResult {
    #[serde(default)]
    result: String,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct RawMapping {
    scene: Option<XemValues>,
    tvdb: Option<XemValues>,
}

#[derive(Clone)]
pub struct Xem {
    http: reqwest::Client,
    base: String,
    have_map: TtlCache<Arc<HashSet<i64>>>,
    names: TtlCache<Arc<HashMap<i64, Vec<XemName>>>>,
}

impl Xem {
    pub fn with_base(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            have_map: TtlCache::new(CACHE_TTL, 1),
            names: TtlCache::new(CACHE_TTL, 1),
        }
    }

    async fn get(&self, resource: &str, query: &[(&str, String)]) -> Result<XemResult, String> {
        let mut params: Vec<(&str, String)> = query.to_vec();
        params.push(("origin", "tvdb".into()));
        let response = self
            .http
            .get(format!("{}/map/{resource}", self.base))
            .query(&params)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("TheXEM request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("TheXEM answered HTTP {}", response.status()));
        }
        response
            .json::<XemResult>()
            .await
            .map_err(|e| format!("TheXEM returned unexpected data: {e}"))
    }

    /// `CheckForFailureResult`.
    fn check(result: &XemResult) -> Result<(), String> {
        let message = result.message.as_deref().unwrap_or_default();
        if result.result.eq_ignore_ascii_case("failure")
            && !IGNORED_ERRORS.iter().any(|e| message.contains(e))
        {
            return Err(format!("Error response received from TheXEM: {message}"));
        }
        Ok(())
    }

    /// TheTVDB ids of the series TheXEM maps (`GetXemSeriesIds`), cached for three hours.
    pub async fn mapped_series(&self) -> Result<Arc<HashSet<i64>>, String> {
        if let Some(ids) = self.have_map.get("") {
            return Ok(ids);
        }
        let result = self.get("havemap", &[]).await?;
        Self::check(&result)?;
        let ids: HashSet<i64> = result
            .data
            .as_array()
            .map(|a| a.iter().filter_map(as_i64).filter(|id| *id > 0).collect())
            .unwrap_or_default();
        let ids = Arc::new(ids);
        // An empty list is not kept: Sonarr retries it (`ExtendTTL` keeps the old list).
        if !ids.is_empty() {
            self.have_map.insert(String::new(), ids.clone());
        }
        Ok(ids)
    }

    /// Forgets the list of mapped series (a manual refresh asks again, as Sonarr's
    /// `SeriesRefreshStartingEvent` does).
    pub fn expire_mapped_series(&self) {
        self.have_map.remove("");
    }

    /// The scene numbering of a series (`GetSceneTvdbMappings`); entries without scene numbers
    /// are left out.
    pub async fn mappings(&self, tvdb_id: i64) -> Result<Vec<XemMapping>, String> {
        let result = self.get("all", &[("id", tvdb_id.to_string())]).await?;
        Self::check(&result)?;
        let raw: Vec<RawMapping> = match result.data {
            Value::Array(_) => serde_json::from_value(result.data)
                .map_err(|e| format!("TheXEM returned unexpected data: {e}"))?,
            _ => Vec::new(),
        };
        Ok(raw
            .into_iter()
            .filter_map(|m| {
                Some(XemMapping {
                    scene: m.scene?,
                    tvdb: m.tvdb?,
                })
            })
            .collect())
    }

    /// Scene names of every mapped series, by TheTVDB id (`GetSceneTvdbNames`), cached for
    /// three hours.
    pub async fn names(&self) -> Result<Arc<HashMap<i64, Vec<XemName>>>, String> {
        if let Some(names) = self.names.get("") {
            return Ok(names);
        }
        let result = self
            .get("allNames", &[("seasonNumbers", "true".into())])
            .await?;
        Self::check(&result)?;
        let names = Arc::new(parse_names(&result.data));
        if !names.is_empty() {
            self.names.insert(String::new(), names.clone());
        }
        Ok(names)
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `{"<tvdb id>": [{"<name>": <season>}, …]}`; names whose season is not a number are
/// skipped, as are Fate/Zero's later seasons (Sonarr's hack for TheTVDB 79151).
fn parse_names(data: &Value) -> HashMap<i64, Vec<XemName>> {
    let mut out: HashMap<i64, Vec<XemName>> = HashMap::new();
    let Some(series) = data.as_object() else {
        return out;
    };
    for (id, names) in series {
        let Ok(tvdb_id) = id.trim().parse::<i64>() else {
            continue;
        };
        for name in names.as_array().into_iter().flatten() {
            let Some(name) = name.as_object() else {
                continue;
            };
            for (title, season) in name {
                let Some(season) = as_i64(season).and_then(|s| i32::try_from(s).ok()) else {
                    continue;
                };
                if tvdb_id == 79151 && season > 1 {
                    continue;
                }
                if title.trim().is_empty() {
                    continue;
                }
                out.entry(tvdb_id).or_default().push(XemName {
                    title: title.trim().to_string(),
                    season: (season >= 0).then_some(season),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_keep_numbered_seasons_only() {
        let names = parse_names(&json!({
            "79151": [{"Fate Zero": 1}, {"Fate Zero 2": 2}],
            "121361": [{"Game of Thrones": -1}, {"GoT": "all"}, {"Game of Thrones S2": "2"}],
            "x": [{"Ignored": 1}],
        }));
        assert_eq!(
            names[&79151],
            vec![XemName {
                title: "Fate Zero".into(),
                season: Some(1)
            }]
        );
        assert_eq!(
            names[&121361],
            vec![
                XemName {
                    title: "Game of Thrones".into(),
                    season: None
                },
                XemName {
                    title: "Game of Thrones S2".into(),
                    season: Some(2)
                },
            ]
        );
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn known_failures_are_ignored() {
        let r = |message: &str| XemResult {
            result: "failure".into(),
            data: Value::Null,
            message: Some(message.into()),
        };
        assert!(Xem::check(&r("no show with the tvdb_id 1 found")).is_ok());
        assert!(Xem::check(&r("no single connection")).is_ok());
        assert!(Xem::check(&r("database down")).is_err());
    }
}
