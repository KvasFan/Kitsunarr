//! Plex watchlist (`PlexImport`): the shows of a Plex account's watchlist through Plex's discover
//! service, read completely in pages of 100. The account token is entered in the settings.

use serde_json::{Map, Value};

use crate::{ListError, ListItem, check_status, connection_error, imdb, text};

const PAGE: i64 = 100;
const MAX_PAGES: i64 = 1000;

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let token =
        text(s, "accessToken").ok_or(ListError::Settings("Enter the Plex account token".into()))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://discover.provider.plex.tv".into());
    let client = text(s, "clientIdentifier").unwrap_or_else(|| "kitsunarr".into());
    let invalid = || {
        ListError::Invalid(
            "Plex returned an incomplete watchlist page; the previous list is kept".into(),
        )
    };
    let mut out = Vec::new();
    let mut offset = 0i64;
    for _ in 0..MAX_PAGES {
        let resp = http
            .get(format!(
                "{}/library/sections/watchlist/all",
                base.trim_end_matches('/')
            ))
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[
                ("clientID", client.as_str()),
                ("context[device][product]", "Kitsunarr"),
                ("includeFields", "title,type,year,ratingKey"),
                ("excludeElements", "Image"),
                ("includeGuids", "1"),
                ("sort", "watchlistedAt:desc"),
                ("type", "2"),
                ("X-Plex-Container-Size", &PAGE.to_string()),
                ("X-Plex-Container-Start", &offset.to_string()),
                ("X-Plex-Token", &token),
            ])
            .send()
            .await
            .map_err(connection_error)?;
        check_status(&resp)?;
        let body: Value = resp.json().await.map_err(|_| invalid())?;
        let c = body
            .get("MediaContainer")
            .filter(|c| c.is_object())
            .ok_or_else(invalid)?;
        let size = c["size"].as_i64().ok_or_else(invalid)?;
        if c["offset"].as_i64().is_some_and(|o| o != offset) {
            return Err(invalid());
        }
        let total = c["totalSize"].as_i64();
        let rows = c["Metadata"].as_array().cloned().unwrap_or_default();
        if rows.len() as i64 != size || total.is_some_and(|t| offset + size > t) {
            return Err(invalid());
        }
        for r in &rows {
            if r["type"].as_str() != Some("show") {
                continue;
            }
            let guid = |scheme: &str| {
                r["Guid"].as_array().into_iter().flatten().find_map(|g| {
                    g["id"]
                        .as_str()?
                        .strip_prefix(&format!("{scheme}://"))
                        .map(str::to_string)
                })
            };
            let item = ListItem {
                title: r["title"].as_str().unwrap_or_default().to_string(),
                year: r["year"].as_i64(),
                tvdb_id: guid("tvdb").and_then(|v| v.parse().ok()).filter(|n| *n > 0),
                tmdb_id: guid("tmdb").and_then(|v| v.parse().ok()).filter(|n| *n > 0),
                imdb_id: imdb(guid("imdb").as_deref()),
                ..Default::default()
            };
            if item.title.is_empty()
                || (item.tvdb_id.is_none() && item.tmdb_id.is_none() && item.imdb_id.is_none())
            {
                return Err(invalid());
            }
            out.push(item);
        }
        offset += size;
        if size == 0 || total.is_none_or(|t| offset >= t) {
            return Ok(out);
        }
    }
    Err(invalid())
}
