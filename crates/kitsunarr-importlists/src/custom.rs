//! Sonarr's custom list (`CustomImport`): a URL answering a JSON array of
//! `{title, tvdbId, tmdbId, imdbId}`.

use serde_json::{Map, Value};

use crate::{ListError, ListItem, check_status, connection_error, imdb, positive, text};

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let Some(url) = text(s, "baseUrl") else {
        return Err(ListError::Settings("The list URL is required".into()));
    };
    let resp = http
        .get(url.trim_end_matches('/'))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(connection_error)?;
    check_status(&resp)?;
    let rows: Vec<Value> = resp
        .json()
        .await
        .map_err(|_| ListError::Invalid("the custom list is not a JSON array".into()))?;
    Ok(rows
        .iter()
        .map(|r| {
            let tvdb = positive(&r["tvdbId"]);
            ListItem {
                title: r["title"]
                    .as_str()
                    .filter(|t| !t.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("TvdbId: {}", tvdb.unwrap_or_default())),
                tvdb_id: tvdb,
                tmdb_id: positive(&r["tmdbId"]),
                imdb_id: imdb(r["imdbId"].as_str()),
                ..Default::default()
            }
        })
        .collect())
}
