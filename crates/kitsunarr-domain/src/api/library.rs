use serde::{Deserialize, Serialize};

use crate::media::{
    ExternalId, MediaImage, MediaKind, MetadataProvider, MinimumAvailability, MonitorNewItems,
    MovieStatus, SeriesStatus, SeriesType,
};
use crate::paging::SortDirection;

/// Library list filters (combined with [`crate::PageRequest`] semantics).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct LibraryQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    /// `title`, `added`, `year`, `size`, `status`, `nextAiring`.
    pub sort_key: Option<String>,
    pub sort_direction: Option<SortDirection>,
    pub search: Option<String>,
    pub kind: Option<MediaKind>,
    pub monitored: Option<bool>,
    pub status: Option<String>,
    pub tag_id: Option<i64>,
    pub quality_profile_id: Option<i64>,
    pub provider: Option<MetadataProvider>,
}

impl LibraryQuery {
    pub fn page_request(&self) -> crate::PageRequest {
        crate::PageRequest {
            page: self.page,
            page_size: self.page_size,
            sort_key: self.sort_key.clone(),
            sort_direction: self.sort_direction,
            search: self.search.clone(),
        }
    }
}

/// One row of the series/anime library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SeriesListItem {
    pub id: i64,
    pub kind: MediaKind,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i32>,
    pub status: SeriesStatus,
    pub series_type: SeriesType,
    pub network: Option<String>,
    pub poster_url: Option<String>,
    pub monitored: bool,
    pub quality_profile_id: i64,
    pub quality_profile_name: String,
    pub metadata_provider: MetadataProvider,
    pub path: String,
    pub season_count: i64,
    pub episode_count: i64,
    pub episode_file_count: i64,
    pub monitored_missing_count: i64,
    pub size_on_disk: i64,
    pub next_airing: Option<String>,
    pub added_at: String,
    pub tags: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SeasonItem {
    pub season_number: i32,
    pub monitored: bool,
    pub title: Option<String>,
    pub episode_count: i64,
    pub episode_file_count: i64,
    pub size_on_disk: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SeriesDetail {
    #[serde(flatten)]
    pub item: SeriesListItem,
    pub overview: Option<String>,
    pub runtime: Option<i32>,
    pub certification: Option<String>,
    pub genres: Vec<String>,
    pub images: Vec<MediaImage>,
    pub metadata_id: String,
    pub external_ids: Vec<ExternalId>,
    pub alternate_titles: Vec<String>,
    pub episode_ordering: Option<String>,
    pub monitor_new_items: MonitorNewItems,
    pub season_folder: bool,
    pub use_scene_numbering: bool,
    pub root_folder_path: String,
    pub first_aired: Option<String>,
    pub last_aired: Option<String>,
    /// When provider metadata was last refreshed (RFC 3339 UTC).
    pub last_refreshed_at: Option<String>,
    pub seasons: Vec<SeasonItem>,
    pub collections: Vec<CollectionRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EpisodeFileItem {
    pub id: i64,
    pub relative_path: String,
    pub size: i64,
    pub quality_id: i32,
    pub quality_name: String,
    pub languages: Vec<i32>,
    pub release_group: Option<String>,
    pub added_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EpisodeItem {
    pub id: i64,
    pub series_id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub scene_season_number: Option<i32>,
    pub scene_episode_number: Option<i32>,
    pub title: Option<String>,
    pub air_date: Option<String>,
    pub air_date_utc: Option<String>,
    pub runtime: Option<i32>,
    pub monitored: bool,
    /// Watched in Jellyfin (watched sync) or in Tanukarr before migration.
    #[serde(default)]
    pub watched: bool,
    pub file: Option<EpisodeFileItem>,
}

/// A row of the movie library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MovieListItem {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i32>,
    pub status: MovieStatus,
    pub studio: Option<String>,
    pub poster_url: Option<String>,
    pub monitored: bool,
    pub minimum_availability: MinimumAvailability,
    pub quality_profile_id: i64,
    pub quality_profile_name: String,
    pub metadata_provider: MetadataProvider,
    pub path: String,
    pub has_file: bool,
    pub size_on_disk: i64,
    pub added_at: String,
    pub tags: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MovieFileItem {
    pub id: i64,
    pub relative_path: String,
    pub size: i64,
    pub quality_id: i32,
    pub quality_name: String,
    pub languages: Vec<i32>,
    pub release_group: Option<String>,
    pub added_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MovieDetail {
    #[serde(flatten)]
    pub item: MovieListItem,
    pub overview: Option<String>,
    pub runtime: Option<i32>,
    pub certification: Option<String>,
    pub genres: Vec<String>,
    pub images: Vec<MediaImage>,
    pub metadata_id: String,
    pub external_ids: Vec<ExternalId>,
    pub root_folder_path: String,
    pub in_cinemas: Option<String>,
    pub digital_release: Option<String>,
    pub physical_release: Option<String>,
    /// When provider metadata was last refreshed (RFC 3339 UTC).
    pub last_refreshed_at: Option<String>,
    pub file: Option<MovieFileItem>,
    pub collections: Vec<CollectionRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CollectionRef {
    pub id: i64,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CollectionItem {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub poster_url: Option<String>,
    pub member_count: i64,
    pub series_count: i64,
    pub movie_count: i64,
}

/// A collection with its members (Tanukarr's collection page).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CollectionDetail {
    pub id: i64,
    pub title: String,
    /// `manual`, `provider` or `animeGroup`.
    pub kind: String,
    pub overview: Option<String>,
    pub members: Vec<CollectionMember>,
}

/// A series or movie in a collection, in position order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CollectionMember {
    pub id: i64,
    pub series_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub title: String,
    pub year: Option<i32>,
    pub poster_url: Option<String>,
    pub position: i32,
    /// `season`, `movie` or `special` (Tanukarr's member kinds), else empty.
    pub role: Option<String>,
    pub label: Option<String>,
}

/// Creates a manual collection, or renames one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CollectionEdit {
    pub title: String,
    #[serde(default)]
    pub overview: Option<String>,
}

/// Adds a series or a movie to a collection, or changes a member.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CollectionMemberEdit {
    #[serde(default)]
    pub series_id: Option<i64>,
    #[serde(default)]
    pub movie_id: Option<i64>,
    /// `season`, `movie` or `special`.
    #[serde(default)]
    pub role: Option<String>,
    /// Default: "Season N", "Movie N" or "Special N" for the role's next number.
    #[serde(default)]
    pub label: Option<String>,
    /// Default: after the last member.
    #[serde(default)]
    pub position: Option<i32>,
}

/// Partial update of a series; absent fields are left unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SeriesUpdate {
    pub monitored: Option<bool>,
    pub quality_profile_id: Option<i64>,
    pub series_type: Option<SeriesType>,
    pub season_folder: Option<bool>,
    pub monitor_new_items: Option<MonitorNewItems>,
    pub tags: Option<Vec<i64>>,
    /// `(seasonNumber, monitored)` pairs.
    pub seasons: Option<Vec<SeasonMonitoring>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SeasonMonitoring {
    pub season_number: i32,
    pub monitored: bool,
}

/// Bulk editor request (series or movies).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkEdit {
    pub ids: Vec<i64>,
    pub monitored: Option<bool>,
    pub quality_profile_id: Option<i64>,
    #[serde(default)]
    pub add_tags: Vec<i64>,
    #[serde(default)]
    pub remove_tags: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EpisodesMonitoredUpdate {
    pub episode_ids: Vec<i64>,
    pub monitored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkEditResult {
    pub updated: u64,
}

/// Dashboard statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub series: i64,
    pub anime: i64,
    pub movies: i64,
    pub collections: i64,
    pub episodes: i64,
    pub episode_files: i64,
    pub monitored_missing_episodes: i64,
    pub movie_files: i64,
    pub size_on_disk: i64,
}

/// An upcoming or recent episode for the calendar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CalendarEntry {
    pub episode_id: i64,
    pub series_id: i64,
    pub series_title: String,
    pub kind: MediaKind,
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub title: Option<String>,
    pub air_date_utc: String,
    pub monitored: bool,
    pub has_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct CalendarQuery {
    /// RFC 3339 start (inclusive). Defaults to now minus one day.
    pub start: Option<String>,
    /// RFC 3339 end (exclusive). Defaults to start plus 14 days.
    pub end: Option<String>,
    #[serde(default)]
    pub unmonitored: bool,
}

/// One entry of the history/activity log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: i64,
    pub event_type: String,
    pub date: String,
    pub source_title: String,
    pub series_id: Option<i64>,
    pub series_title: Option<String>,
    pub episode_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub movie_title: Option<String>,
    pub quality_name: Option<String>,
    pub download_id: Option<String>,
    pub data: serde_json::Value,
}

/// A file whose name differs from what the naming settings give (Sonarr's
/// `RenameEpisodeResource`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RenamePreview {
    pub episode_file_id: i64,
    pub season_number: i32,
    pub episode_numbers: Vec<i32>,
    /// Relative to the series folder.
    pub existing_path: String,
    pub new_path: String,
}

/// Files to rename (`episodeFileIds`; empty: every previewed file).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct RenameRequest {
    pub episode_file_ids: Vec<i64>,
}

/// The outcome of a file operation (disk scan, rename).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FileOperationResult {
    pub message: String,
    pub problems: Vec<String>,
}

/// An episode ordering a series can use (TMDB episode groups, TheTVDB season types).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EpisodeOrderingItem {
    /// Empty for the catalogue's default order.
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// The series uses it now.
    pub current: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct EpisodeOrderingUpdate {
    /// `None` or empty: the catalogue's default order.
    pub ordering: Option<String>,
}
