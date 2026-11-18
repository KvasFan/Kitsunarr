//! Tanukarr's `NotificationMetadataLinkGenerator`: the catalogue links Telegram and Pushcut
//! add to series messages (`metadataLinks`: 0 IMDb, 1 TheTVDB, 2 TVmaze, 3 Trakt).

use serde_json::{Map, Value};

use crate::{NotifyError, NotifyResult, SeriesRef};

/// `MetadataLinkType`.
pub const IMDB: i64 = 0;
pub const TVDB: i64 = 1;
pub const TVMAZE: i64 = 2;
pub const TRAKT: i64 = 3;
/// `MetadataLinkPreviewType.None`.
pub const NO_PREVIEW: i64 = -1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataLink {
    /// `None` for links that are not catalogue links (the test message's).
    pub kind: Option<i64>,
    pub label: String,
    pub url: String,
}

/// The chosen link types of the settings; unknown numbers are refused like Tanukarr's
/// validator ("MetadataLink is not valid").
pub fn chosen(s: &Map<String, Value>) -> NotifyResult<Vec<i64>> {
    let list: Vec<i64> = match s.get("metadataLinks") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                    .ok_or_else(|| NotifyError::Settings(format!("MetadataLink is not valid: {v}")))
            })
            .collect::<NotifyResult<_>>()?,
        _ => Vec::new(),
    };
    if let Some(bad) = list.iter().find(|l| !(IMDB..=TRAKT).contains(*l)) {
        return Err(NotifyError::Settings(format!(
            "MetadataLink is not valid: {bad}"
        )));
    }
    Ok(list)
}

/// `GenerateLinks`: the links of the chosen types the series has ids for, in the chosen order.
pub fn generate(series: Option<&SeriesRef>, chosen: &[i64]) -> Vec<MetadataLink> {
    let Some(series) = series else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for kind in chosen {
        let link = |label: &str, url: String| MetadataLink {
            kind: Some(*kind),
            label: label.into(),
            url,
        };
        match *kind {
            IMDB => {
                if let Some(imdb) = series.imdb_id.as_deref().filter(|i| !i.trim().is_empty()) {
                    out.push(link("IMDb", format!("https://www.imdb.com/title/{imdb}")));
                }
            }
            TVDB if series.tvdb_id > 0 => out.push(link(
                "TVDb",
                format!("http://www.thetvdb.com/?tab=series&id={}", series.tvdb_id),
            )),
            TRAKT if series.tvdb_id > 0 => out.push(link(
                "Trakt",
                format!(
                    "http://trakt.tv/search/tvdb/{}?id_type=show",
                    series.tvdb_id
                ),
            )),
            TVMAZE if series.tv_maze_id > 0 => out.push(link(
                "TVMaze",
                format!("http://www.tvmaze.com/shows/{}/_", series.tv_maze_id),
            )),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn links_follow_the_chosen_types_and_known_ids() {
        let series = SeriesRef {
            tvdb_id: 81189,
            tv_maze_id: 169,
            imdb_id: Some("tt0903747".into()),
            ..Default::default()
        };
        let l = generate(Some(&series), &[TVMAZE, IMDB, TRAKT]);
        assert_eq!(
            l.iter().map(|l| l.label.as_str()).collect::<Vec<_>>(),
            ["TVMaze", "IMDb", "Trakt"]
        );
        assert_eq!(l[0].url, "http://www.tvmaze.com/shows/169/_");
        assert_eq!(l[1].url, "https://www.imdb.com/title/tt0903747");
        let bare = SeriesRef::default();
        assert!(generate(Some(&bare), &[IMDB, TVDB, TVMAZE, TRAKT]).is_empty());
        assert!(generate(None, &[IMDB]).is_empty());
        let s = json!({"metadataLinks": [0, "2"]});
        assert_eq!(chosen(s.as_object().unwrap()).unwrap(), vec![0, 2]);
        let bad = json!({"metadataLinks": [7]});
        assert!(chosen(bad.as_object().unwrap()).is_err());
        assert!(chosen(json!({}).as_object().unwrap()).unwrap().is_empty());
    }
}
