//! Import lists: reading the shows of an external list, ported from Sonarr's and Tanukarr's
//! `ImportLists`.
//!
//! Each list returns [`ListItem`]s carrying whichever catalogue ids the service knows. Matching
//! them with the library, exclusions and adding titles is the sync service's job
//! (`kitsunarr-library::import_lists`). A list is read completely or not at all: any page,
//! status or shape error fails the whole fetch, so the previous membership is kept.
//!
//! Implementations (Sonarr's names): `AniListImport`, `CustomImport`, `SonarrImport`,
//! `PlexRssImport` and `MDBListImport`.

pub mod anilist;
pub mod custom;
pub mod mdblist;
pub mod rss;
pub mod sonarr;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde_json::{Map, Value};

/// One show of a list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListItem {
    pub title: String,
    pub year: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub mal_id: Option<i64>,
    pub anilist_id: Option<i64>,
    /// Per-season monitoring the list dictates (Sonarr lists with season sync), else empty.
    pub seasons: Vec<(i64, bool)>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListError {
    /// The list's settings are incomplete or invalid.
    #[error("{0}")]
    Settings(String),
    #[error("{0}")]
    Authentication(String),
    #[error("the list service answered HTTP {0}")]
    Http(u16),
    #[error("the list service is rate limiting requests")]
    RateLimited(Option<Duration>),
    #[error("the list service could not be reached ({0})")]
    Connection(String),
    /// An incomplete or malformed answer; the previous list is kept.
    #[error("{0}")]
    Invalid(String),
}

impl ListError {
    pub fn is_connection(&self) -> bool {
        matches!(self, Self::Connection(_))
    }
}

/// Implementations Kitsunarr reads.
pub const SUPPORTED: &[&str] = &[
    "AniListImport",
    "CustomImport",
    "SonarrImport",
    "PlexRssImport",
    "MDBListImport",
];

pub fn is_supported(implementation: &str) -> bool {
    SUPPORTED
        .iter()
        .any(|s| s.eq_ignore_ascii_case(implementation))
}

/// How long a list's items are reused before it is read again (Sonarr's `MinRefreshInterval`).
pub fn min_refresh_interval(implementation: &str) -> Duration {
    match implementation {
        "SonarrImport" => Duration::from_secs(5 * 60),
        "TraktListImport" | "TraktUserImport" | "TraktPopularImport" => {
            Duration::from_secs(12 * 3600)
        }
        _ => Duration::from_secs(6 * 3600),
    }
}

/// Reads a list completely.
pub async fn fetch(
    http: &reqwest::Client,
    implementation: &str,
    settings: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let items = match implementation {
        "AniListImport" => anilist::fetch(http, settings).await?,
        "CustomImport" => custom::fetch(http, settings).await?,
        "SonarrImport" => sonarr::fetch(http, settings).await?,
        "PlexRssImport" => rss::fetch(http, settings).await?,
        "MDBListImport" => mdblist::fetch(http, settings).await?,
        other => {
            return Err(ListError::Settings(format!(
                "{other} lists are not supported yet"
            )));
        }
    };
    Ok(cleanup(items))
}

/// `ImportListBase.CleanupListItems`: one entry per (title, TVDB id, IMDb id).
fn cleanup(items: Vec<ListItem>) -> Vec<ListItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert((i.title.clone(), i.tvdb_id, i.imdb_id.clone())))
        .collect()
}

// ---- Settings helpers (Sonarr's camelCase field names) ----

pub(crate) fn text(s: &Map<String, Value>, k: &str) -> Option<String> {
    s.get(k)
        .and_then(Value::as_str)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(crate) fn flag(s: &Map<String, Value>, k: &str, default: bool) -> bool {
    s.get(k).and_then(Value::as_bool).unwrap_or(default)
}

/// A list of strings (tags fields), tolerating a single string.
pub(crate) fn strings(s: &Map<String, Value>, k: &str) -> Vec<String> {
    match s.get(k) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .filter(|v| !v.is_empty())
            .collect(),
        Some(Value::String(v)) if !v.trim().is_empty() => vec![v.trim().to_string()],
        _ => Vec::new(),
    }
}

pub(crate) fn ints(s: &Map<String, Value>, k: &str) -> Vec<i64> {
    match s.get(k) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok()))
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn connection_error(e: reqwest::Error) -> ListError {
    ListError::Connection(e.without_url().to_string())
}

/// Maps an unsuccessful status: 401/403 are authentication failures, 429 a rate limit.
pub(crate) fn check_status(resp: &reqwest::Response) -> Result<(), ListError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    Err(match status.as_u16() {
        401 | 403 => ListError::Authentication(format!(
            "the list service rejected the credentials (HTTP {})",
            status.as_u16()
        )),
        429 => ListError::RateLimited(
            resp.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()?.parse().ok())
                .map(Duration::from_secs),
        ),
        n => ListError::Http(n),
    })
}

pub(crate) fn positive(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str()?.trim().parse().ok())
        .filter(|n| *n > 0)
}

/// IMDb ids as `tt` + digits.
pub(crate) fn imdb(v: Option<&str>) -> Option<String> {
    let v = v?.trim();
    (v.len() > 2 && v.starts_with("tt") && v[2..].chars().all(|c| c.is_ascii_digit()))
        .then(|| v.to_string())
}

/// `ImportListSetOperations.Compose`: the items of the included lists minus those of the
/// excluded lists, matched across TVDB, TMDB and IMDb ids (an item known by two ids links
/// them), de-duplicated in order.
pub(crate) fn compose(
    included: Vec<Vec<ListItem>>,
    excluded: Vec<Vec<ListItem>>,
) -> Result<Vec<ListItem>, ListError> {
    fn keys(i: &ListItem) -> Result<Vec<String>, ListError> {
        let mut k = Vec::new();
        if let Some(id) = i.tvdb_id {
            k.push(format!("tvdb:{id}"));
        }
        if let Some(id) = i.tmdb_id {
            k.push(format!("tmdb:{id}"));
        }
        if let Some(id) = &i.imdb_id {
            k.push(format!("imdb:{id}"));
        }
        if k.is_empty() {
            return Err(ListError::Invalid(
                "a list entry has no TVDB, TMDB or IMDb id; the previous list is kept".into(),
            ));
        }
        Ok(k)
    }
    fn root(aliases: &mut HashMap<String, String>, key: &str) -> String {
        let mut r = key.to_string();
        while aliases[&r] != r {
            r = aliases[&r].clone();
        }
        let mut k = key.to_string();
        while aliases[&k] != k {
            let next = aliases[&k].clone();
            aliases.insert(k, r.clone());
            k = next;
        }
        r
    }
    let candidates: Vec<(ListItem, Vec<String>)> = included
        .into_iter()
        .flatten()
        .map(|i| keys(&i).map(|k| (i, k)))
        .collect::<Result<_, _>>()?;
    let excluded: Vec<Vec<String>> = excluded
        .iter()
        .flatten()
        .map(keys)
        .collect::<Result<_, _>>()?;
    let mut aliases: HashMap<String, String> = HashMap::new();
    for ks in candidates.iter().map(|(_, k)| k).chain(excluded.iter()) {
        for k in ks {
            aliases.entry(k.clone()).or_insert_with(|| k.clone());
        }
        for k in &ks[1..] {
            let a = root(&mut aliases, k);
            let b = root(&mut aliases, &ks[0]);
            aliases.insert(a, b);
        }
    }
    let excluded_roots: HashSet<String> = excluded
        .iter()
        .flatten()
        .map(|k| root(&mut aliases, k))
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (item, ks) in candidates {
        let id = root(&mut aliases, &ks[0]);
        if excluded_roots.contains(&id) || !seen.insert(id) {
            continue;
        }
        out.push(item);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(tvdb: Option<i64>, tmdb: Option<i64>, imdb: Option<&str>) -> ListItem {
        ListItem {
            title: "x".into(),
            tvdb_id: tvdb,
            tmdb_id: tmdb,
            imdb_id: imdb.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn compose_links_ids_and_removes_excluded() {
        // A is known by TVDB+TMDB in one list and by TMDB only in the exclusion list.
        let included = vec![
            vec![item(Some(1), Some(10), None), item(Some(2), None, None)],
            vec![
                item(None, None, Some("tt3")),
                item(Some(2), None, Some("tt2")),
            ],
        ];
        let excluded = vec![vec![item(None, Some(10), None)]];
        let out = compose(included, excluded).unwrap();
        assert_eq!(
            out.iter()
                .map(|i| (i.tvdb_id, i.imdb_id.clone()))
                .collect::<Vec<_>>(),
            vec![(Some(2), None), (None, Some("tt3".into()))]
        );
        assert!(compose(vec![vec![item(None, None, None)]], vec![]).is_err());
    }

    #[test]
    fn imdb_ids_are_validated() {
        assert_eq!(imdb(Some(" tt0944947 ")), Some("tt0944947".into()));
        assert_eq!(imdb(Some("0944947")), None);
        assert_eq!(imdb(Some("tt")), None);
    }
}
