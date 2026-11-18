//! Indexer tests and interactive release search.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::media::DownloadProtocol;

/// `GET /releases`: what to search for. Give a season, or an episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct ReleaseQuery {
    pub series_id: i64,
    #[serde(default)]
    pub season_number: Option<i32>,
    #[serde(default)]
    pub episode_id: Option<i64>,
    /// Tanukarr's complete-series search (no season or episode): packs of the whole series,
    /// and, in interactive searches, packs of some of its seasons. Torrent indexers only.
    #[serde(default)]
    pub complete_series: bool,
    /// Tanukarr's season-pack search: a season search that lists only full-season releases.
    #[serde(default)]
    pub season_packs_only: bool,
    /// Optional comma-separated source IDs. Omitted: all eligible sources; empty: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexer_ids: Option<String>,
    /// A client-generated identifier used to supersede its previous interactive request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_session: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct MovieReleaseQuery {
    pub indexer_ids: Option<String>,
    pub search_session: Option<String>,
}

/// Bounded selection shared by series and movie searches.
pub fn selected_indexers(value: Option<&str>) -> Result<Option<Vec<i64>>, &'static str> {
    let Some(value) = value else { return Ok(None) };
    if value.len() > 2048 {
        return Err("Too many selected indexers.");
    }
    if value.trim().is_empty() {
        return Ok(Some(Vec::new()));
    }
    let mut ids = Vec::new();
    for part in value.split(',') {
        let id = part
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or("Indexer IDs must be positive integers separated by commas.")?;
        if !ids.contains(&id) {
            ids.push(id);
        }
        if ids.len() > 64 {
            return Err("Select at most 64 indexers.");
        }
    }
    Ok(Some(ids))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct IndexerSearchDiagnostic {
    pub indexer_id: i64,
    pub indexer: String,
    /// success, failed, or timedOut.
    pub status: String,
    pub duration_ms: u64,
    pub releases: usize,
    /// Generated first-page query plans, including fallback tiers which may not run.
    /// Credentials, request URLs, headers and raw POST bodies are never included.
    pub planned_queries: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A release found on an indexer, with what the parser reads from its name. Download URLs are
/// kept on the server: they usually embed the indexer API key or passkey.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ReleaseItem {
    pub guid: String,
    pub title: String,
    pub indexer_id: i64,
    pub indexer: String,
    pub protocol: DownloadProtocol,
    pub size: i64,
    /// RFC 3339 UTC.
    pub publish_date: String,
    pub info_url: Option<String>,
    pub seeders: Option<i64>,
    pub peers: Option<i64>,
    pub indexer_flags: Vec<String>,
    pub quality: String,
    pub quality_weight: i32,
    pub languages: Vec<String>,
    pub release_group: Option<String>,
    /// False when the name could not be parsed (Sonarr would reject such a release).
    pub parsed: bool,
    pub series_title: Option<String>,
    pub season_numbers: Vec<i32>,
    pub episode_numbers: Vec<i32>,
    pub absolute_episode_numbers: Vec<i32>,
    pub full_season: bool,
    pub special: bool,
    /// The release was mapped to the searched series (by title, alternate title or the
    /// identifiers the indexer reported).
    pub matches_series: bool,
    /// The decision engine accepts the release (no rejections).
    pub approved: bool,
    /// Why the release would not be grabbed automatically (Sonarr's rejection messages).
    pub rejections: Vec<String>,
    /// Matched custom formats, by name.
    pub custom_formats: Vec<String>,
    /// Their total score in the series' quality profile.
    pub custom_format_score: i32,
    /// The library episodes the release contains, as `S01E05`.
    pub mapped_episodes: Vec<String>,
}

/// Local result filters never change the search, ranking or grab decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReleaseFilter {
    #[default]
    All,
    Approved,
    Rejected,
    Unmatched,
}

impl ReleaseFilter {
    pub fn matches(self, release: &ReleaseItem, query: &str) -> bool {
        let status_matches = match self {
            Self::All => true,
            Self::Approved => release.approved,
            Self::Rejected => !release.approved,
            Self::Unmatched => !release.matches_series,
        };
        if !status_matches {
            return false;
        }
        if query.trim().is_empty() {
            return true;
        }
        let fields = std::iter::once(release.title.as_str())
            .chain([release.indexer.as_str(), release.quality.as_str()])
            .chain(release.release_group.as_deref())
            .chain(release.series_title.as_deref())
            .chain(release.custom_formats.iter().map(String::as_str))
            .chain(release.rejections.iter().map(String::as_str))
            .chain(release.languages.iter().map(String::as_str))
            .chain(release.mapped_episodes.iter().map(String::as_str))
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        query
            .to_lowercase()
            .split_whitespace()
            .all(|term| fields.iter().any(|field| field.contains(term)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct IndexerFailure {
    pub indexer_id: i64,
    pub indexer: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ReleaseSearchResult {
    pub releases: Vec<ReleaseItem>,
    pub failures: Vec<IndexerFailure>,
    /// Indexers queried (enabled for interactive search).
    pub searched_indexers: usize,
    /// Sources that returned a valid response, including a valid empty feed.
    #[serde(default)]
    pub successful_indexers: usize,
    /// Human-readable description of the search (e.g. "S01E05").
    pub description: String,
    /// Problems that affect every decision, e.g. a custom format that cannot be evaluated.
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<IndexerSearchDiagnostic>,
    /// Titles/aliases considered by the planner, rather than a claim that every source used them.
    #[serde(default)]
    pub considered_titles: Vec<String>,
}

#[cfg(test)]
mod selection_tests {
    use super::{ReleaseFilter, ReleaseItem, selected_indexers};
    #[test]
    fn selection_distinguishes_all_none_and_invalid_sources() {
        assert_eq!(selected_indexers(None), Ok(None));
        assert_eq!(selected_indexers(Some("")), Ok(Some(vec![])));
        assert_eq!(selected_indexers(Some("1, 2,1")), Ok(Some(vec![1, 2])));
        for value in ["0", "-1", "1,,2", "NaN", "9223372036854775808"] {
            assert!(selected_indexers(Some(value)).is_err());
        }
    }

    #[test]
    fn result_filters_combine_decisions_and_words_across_anime_release_fields() {
        let mut release: ReleaseItem = serde_json::from_value(serde_json::json!({
            "guid": "test", "title": "[SubsPlease] Anime Title - 01 [1080p]", "indexerId": 1,
            "indexer": "Nyaa", "protocol": "torrent", "size": 1073741824i64,
            "publishDate": "2026-11-16T00:00:00Z", "infoUrl": null, "seeders": 20,
            "peers": 25, "indexerFlags": [], "quality": "HDTV-1080p", "qualityWeight": 8,
            "languages": ["Japanese"], "releaseGroup": "SubsPlease", "parsed": true,
            "seriesTitle": "Anime Title", "seasonNumbers": [1], "episodeNumbers": [],
            "absoluteEpisodeNumbers": [1], "fullSeason": false, "special": false,
            "matchesSeries": true, "approved": false,
            "rejections": ["Custom format score 0 is below minimum 500"],
            "customFormats": ["Préféré"], "customFormatScore": 0, "mappedEpisodes": ["S01E01"]
        }))
        .unwrap();
        assert!(ReleaseFilter::All.matches(&release, "  "));
        assert!(ReleaseFilter::Rejected.matches(&release, "SUBSPLEASE minimum 500"));
        assert!(!ReleaseFilter::All.matches(&release, "NYAA japonais"));
        assert!(ReleaseFilter::All.matches(&release, "JAPANESE PRÉFÉRÉ S01E01"));
        assert!(!ReleaseFilter::Approved.matches(&release, ""));
        assert!(!ReleaseFilter::Unmatched.matches(&release, ""));
        assert!(!ReleaseFilter::Rejected.matches(&release, "HEVC"));
        release.approved = true;
        release.rejections.clear();
        assert!(ReleaseFilter::Approved.matches(&release, "1080p"));
        assert!(!ReleaseFilter::Rejected.matches(&release, ""));
        release.matches_series = false;
        assert!(ReleaseFilter::Unmatched.matches(&release, "anime"));
    }
}

/// `POST /providers/indexers/test`: unsaved settings, or a saved indexer's id whose stored
/// secrets fill in redacted fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct IndexerTestRequest {
    #[serde(default)]
    pub id: Option<i64>,
    pub implementation: String,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub settings: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct IndexerTestResult {
    pub ok: bool,
    pub message: String,
    pub releases_found: usize,
    pub warnings: Vec<String>,
}

/// `POST /releases/grab`: a release from a recent search, by indexer and GUID. Download links
/// stay on the server; it resolves them from its search cache.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GrabRequest {
    pub guid: String,
    pub indexer_id: i64,
    /// Episodes chosen by hand for a release that did not map (Sonarr's override). The choice
    /// is kept with the grab and used when the download is imported (Tanukarr).
    #[serde(default)]
    pub episode_ids: Vec<i64>,
    /// Sonarr's "Override and grab": another series for the release (then `episodeIds` are
    /// required and must belong to it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,
    /// The quality the release is grabbed, and later imported, as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<crate::quality::QualityModel>,
    /// The language ids the release is grabbed, and later imported, with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<i32>>,
    /// The download client to send it to instead of the one Kitsunarr would choose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_client_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GrabResult {
    /// The download client's name.
    pub download_client: String,
    /// The client's id for the download (a torrent's info hash).
    pub download_id: String,
    pub message: String,
}

/// A release an automatic search sent to a download client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AutomaticGrab {
    pub title: String,
    pub download_client: String,
    pub download_id: String,
}

/// A release an automatic search chose but could not send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AutomaticGrabFailure {
    pub title: String,
    pub message: String,
}

/// `POST /releases/search`: the outcome of an automatic search (Sonarr's `EpisodeSearch` and
/// `SeasonSearch` commands).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AutomaticSearchResult {
    pub description: String,
    /// Indexers queried (enabled for automatic search).
    pub searched_indexers: usize,
    pub failures: Vec<IndexerFailure>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Releases the indexers returned.
    pub found: usize,
    /// Releases with no rejection.
    pub approved: usize,
    pub grabbed: Vec<AutomaticGrab>,
    pub grab_failures: Vec<AutomaticGrabFailure>,
}

/// `POST /providers/downloadclients/test` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DownloadClientTestResult {
    pub ok: bool,
    pub message: String,
    pub warnings: Vec<String>,
}
