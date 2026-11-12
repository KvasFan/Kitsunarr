//! Plex watchlist RSS (`PlexRssImport`): `channel/item` elements of category `show` whose guid
//! is `imdb://`, `tvdb://` or `tmdb://`.

use serde_json::{Map, Value};

use crate::{ListError, ListItem, check_status, connection_error, imdb, text};

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let url = text(s, "url").ok_or(ListError::Settings("The RSS URL is required".into()))?;
    let resp = http.get(url).send().await.map_err(connection_error)?;
    check_status(&resp)?;
    let body = resp.text().await.map_err(connection_error)?;
    parse_plex(&body)
}

/// Text of the `title`, `guid` and `category` children of each `channel/item`.
fn items(xml: &str) -> Result<Vec<[Option<String>; 3]>, ListError> {
    let root = kitsunarr_indexers::xml::parse(xml)
        .map_err(|_| ListError::Invalid("the RSS feed is not valid XML".into()))?;
    let Some(channel) = root.child("channel") else {
        return Ok(Vec::new());
    };
    Ok(channel
        .children_named("item")
        .map(|i| {
            ["title", "guid", "category"].map(|n| i.child_text(n).map(|t| t.trim().to_string()))
        })
        .collect())
}

/// `PlexRssImportParser`.
pub fn parse_plex(xml: &str) -> Result<Vec<ListItem>, ListError> {
    Ok(items(xml)?
        .into_iter()
        .filter(|[_, _, category]| category.as_deref() == Some("show"))
        .filter_map(|[title, guid, _]| {
            let guid = guid.unwrap_or_default();
            let mut item = ListItem {
                title: title.unwrap_or_else(|| "Unknown".into()),
                ..Default::default()
            };
            if let Some(rest) = guid.strip_prefix("imdb://") {
                item.imdb_id = regex::Regex::new(r"(?i)(tt\d{7,8})")
                    .ok()?
                    .captures(rest)
                    .and_then(|c| imdb(Some(&c[1].to_lowercase())));
            }
            item.tvdb_id = guid.strip_prefix("tvdb://").and_then(|v| v.parse().ok());
            item.tmdb_id = guid.strip_prefix("tmdb://").and_then(|v| v.parse().ok());
            (item.imdb_id.is_some() || item.tvdb_id.is_some() || item.tmdb_id.is_some())
                .then_some(item)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn plex_watchlist_items() {
        let xml = r#"<?xml version="1.0"?><rss><channel><title>Watchlist</title>
            <item><title>Frieren</title><guid>tvdb://424536</guid><category>show</category></item>
            <item><title>Severance</title><guid>imdb://tt11280740</guid><category>show</category></item>
            <item><title>Dune</title><guid>tmdb://438631</guid><category>movie</category></item>
            <item><title>No id</title><guid>plex://show/1</guid><category>show</category></item>
            </channel></rss>"#;
        let items = super::parse_plex(xml).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].tvdb_id, Some(424536));
        assert_eq!(items[1].imdb_id.as_deref(), Some("tt11280740"));
    }
}
