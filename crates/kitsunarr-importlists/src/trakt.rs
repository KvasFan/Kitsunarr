//! Trakt (`TraktListImport`, `TraktUserImport`, `TraktPopularImport`): a user's list, watchlist,
//! watched shows or collection, or a popular/trending chart.
//!
//! Sonarr authenticates through its own Trakt application and the servarr OAuth relay, which
//! Kitsunarr cannot use: the user's own Trakt application Client ID is required (`clientId`),
//! and an access token of that application (`accessToken`) reads private lists.

use serde_json::{Map, Value};

use crate::{
    ListError, ListItem, check_status, compose, connection_error, imdb, positive, strings, text,
};

const MAX_PAGES: i64 = 100;

/// `TraktQueryHelper.BuildFilterParameters`.
fn filters(s: &Map<String, Value>) -> Vec<(String, String)> {
    let reserved = ["extended", "genres", "ratings", "years", "limit", "page"];
    let mut out: Vec<(String, String)> = Vec::new();
    if let Some(extra) = text(s, "traktAdditionalParameters") {
        for p in extra.trim_start_matches(['?', '&']).split('&') {
            if let Some((k, v)) = p.split_once('=')
                && !k.trim().is_empty()
                && !reserved.iter().any(|r| r.eq_ignore_ascii_case(k.trim()))
            {
                out.push((k.trim().into(), v.trim().into()));
            }
        }
    }
    if let Some(g) = text(s, "genres") {
        out.push(("genres".into(), g.to_lowercase()));
    }
    if let Some(r) = text(s, "rating") {
        out.push(("ratings".into(), r));
    }
    if let Some(y) = text(s, "years") {
        out.push(("years".into(), y));
    }
    out
}

fn limit(s: &Map<String, Value>) -> i64 {
    s.get("limit")
        .and_then(Value::as_i64)
        .filter(|l| *l > 0)
        .unwrap_or(100)
}

fn enum_code(s: &Map<String, Value>, k: &str) -> i64 {
    s.get(k).and_then(Value::as_i64).unwrap_or(0)
}

/// A slug as Sonarr's `ToUrlSlug` makes it: lower case, non-alphanumerics to dashes.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn show_item(v: &Value) -> Option<ListItem> {
    let show = v.get("show").filter(|s| s.is_object()).unwrap_or(v);
    let ids = &show["ids"];
    let item = ListItem {
        title: show["title"].as_str().unwrap_or_default().to_string(),
        year: show["year"].as_i64(),
        tvdb_id: positive(&ids["tvdb"]),
        tmdb_id: positive(&ids["tmdb"]),
        imdb_id: imdb(ids["imdb"].as_str()),
        ..Default::default()
    };
    (item.tvdb_id.is_some() || item.tmdb_id.is_some() || item.imdb_id.is_some()).then_some(item)
}

struct Api<'a> {
    http: &'a reqwest::Client,
    base: String,
    client_id: String,
    token: Option<String>,
}

impl Api<'_> {
    /// Every page of `path` (`X-Pagination-Page-Count`), up to `max` rows.
    async fn rows(
        &self,
        path: &str,
        query: &[(String, String)],
        max: i64,
    ) -> Result<Vec<Value>, ListError> {
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let mut req = self
                .http
                .get(format!("{}{path}", self.base.trim_end_matches('/')))
                .header("trakt-api-version", "2")
                .header("trakt-api-key", &self.client_id)
                .query(query)
                .query(&[
                    ("page", page.to_string()),
                    ("limit", max.min(100).to_string()),
                ]);
            if let Some(t) = &self.token {
                req = req.bearer_auth(t);
            }
            let resp = req.send().await.map_err(connection_error)?;
            if resp.status().as_u16() == 404 {
                return Err(ListError::Settings(format!("Trakt has no list at {path}")));
            }
            check_status(&resp)?;
            let pages = resp
                .headers()
                .get("X-Pagination-Page-Count")
                .and_then(|v| v.to_str().ok()?.parse::<i64>().ok());
            let rows: Vec<Value> = resp.json().await.map_err(|_| {
                ListError::Invalid("Trakt did not answer a list; the previous list is kept".into())
            })?;
            let empty = rows.is_empty();
            out.extend(rows);
            if empty || out.len() as i64 >= max || pages.is_none_or(|p| page >= p) {
                out.truncate(max as usize);
                return Ok(out);
            }
        }
        Err(ListError::Invalid(
            "Trakt pagination could not be completed; the previous list is kept".into(),
        ))
    }
}

pub async fn fetch(
    http: &reqwest::Client,
    implementation: &str,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let client_id = text(s, "clientId").ok_or(ListError::Settings(
        "Enter the Client ID of your Trakt application (https://trakt.tv/oauth/applications)"
            .into(),
    ))?;
    let api = Api {
        http,
        base: text(s, "baseUrl").unwrap_or_else(|| "https://api.trakt.tv".into()),
        client_id,
        token: text(s, "accessToken"),
    };
    let max = limit(s);
    let query = filters(s);
    let user = text(s, "username").or_else(|| text(s, "authUser"));
    let shows = |rows: Vec<Value>| rows.iter().filter_map(show_item).collect::<Vec<_>>();
    match implementation {
        "TraktListImport" => {
            let user = user.ok_or(ListError::Settings("Enter the Trakt username".into()))?;
            let mut included = strings(s, "includedLists");
            if let Some(name) = text(s, "listname") {
                included.push(slug(&name));
            }
            if included.is_empty() {
                return Err(ListError::Settings("Enter the Trakt list name".into()));
            }
            let mut inc = Vec::new();
            for l in &included {
                let path = format!("/users/{user}/lists/{l}/items/show,season,episode");
                inc.push(shows(api.rows(&path, &query, max).await?));
            }
            let mut exc = Vec::new();
            for l in strings(s, "excludedLists") {
                let path = format!("/users/{user}/lists/{l}/items/show,season,episode");
                exc.push(shows(api.rows(&path, &[], i64::MAX).await?));
            }
            compose(inc, exc)
        }
        "TraktUserImport" => {
            let user = user.ok_or(ListError::Settings("Enter the Trakt username".into()))?;
            match enum_code(s, "traktListType") {
                1 => {
                    let mut q = query.clone();
                    q.push(("extended".into(), "full".into()));
                    let rows = api
                        .rows(&format!("/users/{user}/watched/shows"), &q, max)
                        .await?;
                    let wanted = enum_code(s, "traktWatchedListType");
                    Ok(rows
                        .iter()
                        .filter(|r| {
                            let aired = r["show"]["aired_episodes"].as_i64().unwrap_or(0);
                            let watched: i64 = r["seasons"]
                                .as_array()
                                .map(|ss| {
                                    ss.iter()
                                        .filter(|s| s["number"].as_i64().unwrap_or(0) > 0)
                                        .map(|s| {
                                            s["episodes"].as_array().map_or(0, |e| e.len() as i64)
                                        })
                                        .sum()
                                })
                                .unwrap_or(0);
                            match wanted {
                                1 => watched < aired,
                                2 => aired > 0 && watched >= aired,
                                _ => true,
                            }
                        })
                        .filter_map(show_item)
                        .collect())
                }
                2 => Ok(shows(
                    api.rows(&format!("/users/{user}/collection/shows"), &query, max)
                        .await?,
                )),
                _ => {
                    let sort = match enum_code(s, "traktWatchSorting") {
                        1 => "added",
                        2 => "title",
                        3 => "released",
                        _ => "rank",
                    };
                    Ok(shows(
                        api.rows(
                            &format!("/users/{user}/watchlist/shows/{sort}"),
                            &query,
                            max,
                        )
                        .await?,
                    ))
                }
            }
        }
        "TraktSmartListImport" => {
            let raw = text(s, "smartList")
                .ok_or(ListError::Settings("Enter the Trakt smart list".into()))?;
            // `TraktSmartListSlug.Parse`: a smart list URL or its slug.
            let slug = regex::Regex::new(r"(?i)/(?:lists/smart/view|smart-lists)/([^/?#]+)")
                .ok()
                .and_then(|re| re.captures(&raw).map(|c| c[1].to_string()))
                .unwrap_or_else(|| raw.trim_matches('/').to_string());
            if slug.is_empty()
                || !slug
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return Err(ListError::Settings(format!(
                    "{raw} is not a Trakt smart list"
                )));
            }
            Ok(shows(
                api.rows(&format!("/smart-lists/{slug}/items/shows"), &query, max)
                    .await?,
            ))
        }
        "TraktPopularImport" => {
            let path = match enum_code(s, "traktListType") {
                1 => "/shows/popular",
                2 => "/shows/anticipated",
                3 => "/shows/watched/weekly",
                4 => "/shows/watched/monthly",
                5 => "/shows/watched/yearly",
                6 => "/shows/watched/all",
                7 => "/shows/recommended/weekly",
                8 => "/shows/recommended/monthly",
                9 => "/shows/recommended/yearly",
                10 => "/shows/recommended/all",
                _ => "/shows/trending",
            };
            Ok(shows(api.rows(path, &query, max).await?))
        }
        other => Err(ListError::Settings(format!("{other} is not a Trakt list"))),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn slugs_and_items() {
        assert_eq!(super::slug("My Anime  List!"), "my-anime-list");
        let wrapped = serde_json::json!({"show": {"title": "A", "year": 2020, "ids": {"tvdb": 5, "imdb": "tt0000005"}}});
        let bare = serde_json::json!({"title": "B", "ids": {"tmdb": 7}});
        assert_eq!(super::show_item(&wrapped).unwrap().tvdb_id, Some(5));
        assert_eq!(super::show_item(&bare).unwrap().tmdb_id, Some(7));
        assert!(
            super::show_item(&serde_json::json!({"show": {"title": "C", "ids": {"trakt": 1}}}))
                .is_none()
        );
    }
}
