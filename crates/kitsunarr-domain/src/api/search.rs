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
    /// The parsed series title matches the searched series (title or an alternate title,
    /// compared as Sonarr compares clean titles).
    pub matches_series: bool,
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
    /// Human-readable description of the search (e.g. "S01E05").
    pub description: String,
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
