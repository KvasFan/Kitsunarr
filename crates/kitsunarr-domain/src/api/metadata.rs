//! Metadata search and "add to library" contract.

use serde::{Deserialize, Serialize};

use crate::media::{ExternalId, MetadataProvider, SeriesStatus, SeriesType};

/// Catalogues Kitsunarr can currently search, add from and refresh from.
pub const SUPPORTED_CATALOGUES: &[MetadataProvider] = &[
    MetadataProvider::Tvdb,
    MetadataProvider::TmdbTv,
    MetadataProvider::TvMaze,
    MetadataProvider::AniList,
    MetadataProvider::MyAnimeList,
    MetadataProvider::TmdbMovie,
    MetadataProvider::Omdb,
];

/// What a catalogue entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    Series,
    Anime,
    Movie,
}

/// A metadata catalogue Kitsunarr can search, add from and refresh from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CatalogueInfo {
    pub provider: MetadataProvider,
    pub name: String,
    /// Short description of what the catalogue covers.
    pub description: String,
    pub kinds: Vec<EntryKind>,
    /// Credit required by the catalogue's licence or terms; shown wherever its data is used.
    pub attribution: String,
    pub url: String,
    /// Usable now: it needs no API key, or its key is set.
    #[serde(default = "available_default")]
    pub available: bool,
}

fn available_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub provider: MetadataProvider,
    pub term: String,
}

/// `GET /library/folder-preview`: the folder a title would get.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct FolderPreviewQuery {
    pub title: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub movie: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LibraryRef {
    /// `series` or `movie`.
    pub media_type: String,
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub provider: MetadataProvider,
    pub id: String,
    pub kind: EntryKind,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub status: SeriesStatus,
    pub poster_url: Option<String>,
    pub external_ids: Vec<ExternalId>,
    /// Set when the title (or one of its identities) is already in the library.
    pub in_library: Option<LibraryRef>,
}

/// Which episodes to monitor when adding a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum MonitorOption {
    #[default]
    All,
    Future,
    FirstSeason,
    LatestSeason,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AddTitleRequest {
    pub provider: MetadataProvider,
    pub id: String,
    pub root_folder_path: String,
    pub quality_profile_id: i64,
    #[serde(default)]
    pub monitor: MonitorOption,
    /// Overrides the provider's suggestion (anime catalogues default to `anime`).
    #[serde(default)]
    pub series_type: Option<SeriesType>,
    #[serde(default = "default_true")]
    pub season_folder: bool,
    #[serde(default)]
    pub tags: Vec<i64>,
    /// Search for the missing monitored episodes once added (Sonarr's "Start search for missing
    /// episodes").
    #[serde(default)]
    pub search_for_missing: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RefreshResult {
    pub added_episodes: u64,
    pub updated_episodes: u64,
}
