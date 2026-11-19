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
    /// Movie watched filter; independent of monitoring.
    pub watched: Option<bool>,
    pub status: Option<String>,
    pub tag_id: Option<i64>,
    pub quality_profile_id: Option<i64>,
    pub provider: Option<MetadataProvider>,
    /// Tanukarr's path filter (series): titles in this folder or below it, compared without
    /// case, with `\` read as `/`.
    #[serde(default)]
    pub path: Option<String>,
    /// Tanukarr's "Missing episodes" filter (series): `true` keeps titles with monitored aired
    /// episodes that have no file, `false` the others.
    #[serde(default)]
    pub missing: Option<bool>,
    /// Tanukarr's "Anime" filter (series): the series type.
    #[serde(default)]
    pub series_type: Option<SeriesType>,
    /// Tanukarr's "Group anime seasons" (series): one row per anime group, its first member
    /// among the titles the other filters keep; [`SeriesListItem::anime_group`] tells the rest.
    #[serde(default)]
    pub group_anime_seasons: Option<bool>,
    /// Library index: also return every title the filters keep ([`LibraryIndex::keys`]), for
    /// Tanukarr's "Select all".
    #[serde(default)]
    pub include_keys: Option<bool>,
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
    /// The anime group (Tanukarr's anime grouping) the series belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anime_group: Option<AnimeGroupRef>,
    /// Tanukarr's index columns and overview view.
    #[serde(default)]
    pub index: IndexColumns,
}

/// A title of the library index: a series (TV or anime) or a movie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LibraryKey {
    pub movie: bool,
    pub id: i64,
}

/// One entry of the library index (Tanukarr's single series index lists series, anime and
/// movies together, and an anime group once).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LibraryEntry {
    Series(SeriesListItem),
    Movie(MovieListItem),
    /// Tanukarr's grouped entry (`AnimeGroupSummary`): the members the filters keep.
    Group(LibraryGroup),
}

/// An anime group (or manual group) shown once by "Group anime seasons".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LibraryGroup {
    pub collection_id: i64,
    pub title: String,
    pub poster_url: Option<String>,
    /// The members the filters keep, in group order (seasons, movies, specials).
    pub members: Vec<LibraryKey>,
    /// Native seasons, movies and specials of those members (the collection page's sections).
    pub season_count: i64,
    pub movie_count: i64,
    pub special_count: i64,
    /// Episodes (a movie counts as one) and those with a file, specials left out.
    pub episode_count: i64,
    pub episode_file_count: i64,
    /// The group number of a group's only season (Tanukarr's "Season N").
    pub single_season: Option<i32>,
}

/// A letter of Tanukarr's jump bar and the index of the first entry it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct JumpBarItem {
    pub character: String,
    pub index: u64,
}

/// Tanukarr's series index footer: the whole library, whatever the filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LibraryFooter {
    pub titles: i64,
    pub ended: i64,
    pub continuing: i64,
    pub monitored: i64,
    pub unmonitored: i64,
    pub episodes: i64,
    pub files: i64,
    pub size_on_disk: i64,
}

/// A page of the library index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndex {
    pub page: u32,
    pub page_size: u32,
    /// Entries the filters keep (an anime group counts once when grouped).
    pub total_records: u64,
    pub records: Vec<LibraryEntry>,
    /// Titles the filters keep, each group member counted (Tanukarr's "{count} / {total}").
    pub filtered_items: u64,
    /// Every title of the library, before the filters.
    pub total_items: u64,
    /// Some title the filters keep belongs to a group (Tanukarr shows the grouping toggle).
    pub has_groups: bool,
    /// Tanukarr's jump bar, when sorted by title.
    pub jump_bar: Vec<JumpBarItem>,
    pub footer: LibraryFooter,
    /// Every title the filters keep, group members included (only with `includeKeys`).
    #[serde(default)]
    pub keys: Vec<LibraryKey>,
}

/// What Tanukarr's library index shows beyond the basic row (table columns and the overview
/// view), kept apart so the detail resources that flatten a row keep their own fields.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct IndexColumns {
    pub overview: Option<String>,
    pub genres: Vec<String>,
    /// The catalogue rating (0-10).
    pub rating: Option<f64>,
    pub certification: Option<String>,
    pub runtime: Option<i32>,
    /// Series: the last episode that aired (RFC 3339 UTC).
    pub previous_airing: Option<String>,
    /// Release groups of the files.
    pub release_groups: Vec<String>,
    /// Series: whether episodes go to season folders.
    pub season_folder: Option<bool>,
    /// Movies: the cinema release.
    pub in_cinemas: Option<String>,
    /// Movies: the file's quality.
    pub file_quality: Option<String>,
}

/// A series' anime group: the `animeGroup` collection and how many series it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AnimeGroupRef {
    pub collection_id: i64,
    pub title: String,
    pub series_count: i64,
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
    /// The language TheTVDB titles are read in (a language id; English when unset).
    #[serde(default)]
    pub metadata_language: Option<i32>,
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
    /// Tanukarr's anime grouping mode: `false` is "Disabled", the series stays out of
    /// automatic anime groups.
    #[serde(default = "crate::api::library::yes")]
    pub automatic_grouping: bool,
    /// The franchise group (`animeGroup`) or manual group the series belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_membership: Option<GroupMembershipRef>,
}

/// A series' place in an anime group or a manual group (Tanukarr's `AnimeGroup*` fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GroupMembershipRef {
    pub collection_id: i64,
    /// `animeGroup` (found from catalogue relations) or `manual`.
    pub kind: String,
    /// The group's catalogue key (`anilist:{root id}`, `mal:{root id}`) for automatic groups.
    pub group_key: Option<String>,
    pub title: String,
    /// `season`, `movie` or `special`.
    pub role: Option<String>,
    pub label: Option<String>,
    pub position: i64,
    /// The group's `AnimeGroupKind` (`series` or `movieSaga`).
    #[serde(default = "series_kind")]
    pub group_kind: String,
    /// The group's `AnimeGroupMovieFolderStyle`.
    #[serde(default = "automatic_style")]
    pub movie_folder_style: String,
}

pub(crate) fn yes() -> bool {
    true
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
    /// TheXEM's scene absolute number (anime searches use it).
    #[serde(default)]
    pub scene_absolute_episode_number: Option<i32>,
    /// The scene numbering was extrapolated, not listed by TheXEM.
    #[serde(default)]
    pub unverified_scene_numbering: bool,
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
    #[serde(default)]
    pub watched: bool,
    #[serde(default)]
    pub watched_at: Option<String>,
    pub minimum_availability: MinimumAvailability,
    pub quality_profile_id: i64,
    pub quality_profile_name: String,
    pub metadata_provider: MetadataProvider,
    pub path: String,
    pub has_file: bool,
    pub size_on_disk: i64,
    pub added_at: String,
    pub tags: Vec<i64>,
    /// Tanukarr's index columns and overview view.
    #[serde(default)]
    pub index: IndexColumns,
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

/// A reviewed rename for the movie's one feature. Echo the preview when applying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MovieRenamePreview {
    pub movie_file_id: i64,
    pub existing_path: String,
    pub new_path: String,
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
    /// `false`: the film stays out of automatic anime groups and TMDB collections.
    #[serde(default = "crate::api::library::yes")]
    pub automatic_grouping: bool,
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
    #[serde(default)]
    pub poster_url: Option<String>,
    /// Native seasons and features, presented in collection order (Tanukarr CollectionPage).
    #[serde(default)]
    pub entries: Vec<CollectionEntry>,
    /// Tanukarr's `AnimeGroupKind`: `series` or `movieSaga`.
    #[serde(default = "series_kind")]
    pub group_kind: String,
    /// Tanukarr's `AnimeGroupMovieFolderStyle`: `automatic`, `numbered` or `title`.
    #[serde(default = "automatic_style")]
    pub movie_folder_style: String,
}

pub(crate) fn series_kind() -> String {
    "series".into()
}

pub(crate) fn automatic_style() -> String {
    "automatic".into()
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

/// One native season or movie shown within a collection. Display numbering never replaces
/// the native season number used by searches, monitoring and episode links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CollectionEntry {
    pub member_id: i64,
    pub series_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub season_number: Option<i32>,
    pub display_number: i32,
    /// `seasons`, `movies` or `specials`.
    pub section: String,
    pub title: String,
    pub series_title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub monitored: bool,
    pub episode_count: i64,
    pub episode_file_count: i64,
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
    /// Sonarr's `Series.Language` for TheTVDB titles (a language id); changing it refreshes.
    #[serde(default)]
    pub metadata_language: Option<i32>,
    /// Tanukarr's anime grouping mode: `false` ("Disabled") removes the series from its
    /// automatic groups and keeps it out of them; `true` lets the next refresh group it.
    #[serde(default)]
    pub automatic_grouping: Option<bool>,
    /// A new series folder (Sonarr's series edit with `path`): absolute, inside a root folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// With `path`: move the folder's contents there (Sonarr's `moveFiles`); otherwise only the
    /// recorded folder changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub move_files: bool,
}

/// Partial update of a movie (the movie page's Edit dialog); absent fields are left unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MovieUpdate {
    #[serde(default)]
    pub monitored: Option<bool>,
    #[serde(default)]
    pub quality_profile_id: Option<i64>,
    /// Replaces the movie's tags.
    #[serde(default)]
    pub tags: Option<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_availability: Option<MinimumAvailability>,
    /// `false` removes the film from its automatic groups (anime groups, TMDB collections) and
    /// keeps it out of them; `true` lets it be grouped again.
    #[serde(default)]
    pub automatic_grouping: Option<bool>,
    /// A new movie folder: absolute, inside a root folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// With `path`: move the folder's contents there; otherwise only the recorded folder changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub move_files: bool,
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
    /// Series only: applies a monitoring choice to the episodes and seasons (Sonarr's season
    /// pass `monitoringOptions`), after `monitored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<crate::api::metadata::MonitorOption>,
    /// Series only: whether seasons a refresh discovers are monitored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor_new_items: Option<MonitorNewItems>,
    /// Replaces every title's tags (Sonarr's `applyTags: replace`); applied before `addTags`
    /// and `removeTags`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_tags: Option<Vec<i64>>,
    /// Series only: the series type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_type: Option<SeriesType>,
    /// Series only: whether episodes go to season folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_folder: Option<bool>,
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

/// Optional history filters are combined with AND. Download ids are case-insensitive,
/// matching download tracking. An episode can be scoped further to its parent series.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct HistoryFilter {
    pub event_type: Option<String>,
    pub series_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub download_id: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MovieWatchedUpdate {
    pub watched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MoviesWatchedUpdate {
    pub ids: Vec<i64>,
    pub watched: bool,
}

/// A folder of a root folder that no title uses (Sonarr's `UnmappedFolder`), offered for
/// library import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct UnmappedFolder {
    pub name: String,
    pub path: String,
    /// The path below the root folder.
    pub relative_path: String,
    /// What to search the catalogues for: an identity (`tvdb:…`, `tmdb:…`, `tt…`) named in the
    /// folder, else its title without year and tags.
    pub search_term: String,
    /// A year named in the folder, `(2008)`.
    pub year: Option<i32>,
}

/// The catalogue search for a folder name, and the year it names: Sonarr's and Radarr's id
/// tags (`[tvdbid-81189]`, `{tvdb-81189}`, `[tmdbid-1396]`, `{imdb-tt0903747}`) give an exact
/// lookup; otherwise bracketed tags and a trailing `(year)` are dropped, and dots or
/// underscores become spaces when the name has no spaces.
pub fn folder_search_term(name: &str) -> (String, Option<i32>) {
    let lower = name.to_ascii_lowercase();
    for (tag, prefix) in [
        ("tvdbid-", "tvdb:"),
        ("tvdb-", "tvdb:"),
        ("tmdbid-", "tmdb:"),
        ("tmdb-", "tmdb:"),
        ("imdbid-", ""),
        ("imdb-", ""),
    ] {
        if let Some(at) = lower.find(tag) {
            let rest = &lower[at + tag.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let valid = if prefix.is_empty() {
                id.len() > 2 && id.starts_with("tt") && id[2..].chars().all(|c| c.is_ascii_digit())
            } else {
                !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())
            };
            if valid {
                return (format!("{prefix}{id}"), folder_year(name));
            }
        }
    }
    let mut title = String::with_capacity(name.len());
    let mut depth = 0i32;
    for c in name.chars() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => title.push(c),
            _ => {}
        }
    }
    let year = folder_year(&title);
    let spaced = title.trim().contains(' ');
    if let Some(y) = year {
        title = title.replace(&format!("({y})"), " ");
    }
    if !spaced {
        title = title.replace(['.', '_'], " ");
    }
    let term = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (term, year)
}

/// The last `(yyyy)` in a name, 1900–2199.
fn folder_year(name: &str) -> Option<i32> {
    let bytes = name.as_bytes();
    (0..bytes.len().saturating_sub(5)).rev().find_map(|i| {
        (bytes[i] == b'(' && bytes[i + 5] == b')')
            .then(|| name.get(i + 1..i + 5)?.parse::<i32>().ok())
            .flatten()
            .filter(|y| (1900..2200).contains(y))
    })
}

#[cfg(test)]
mod folder_term_tests {
    use super::folder_search_term;

    #[test]
    fn folder_names_become_catalogue_searches() {
        let cases = [
            ("Breaking Bad (2008)", "Breaking Bad", Some(2008)),
            (
                "Breaking Bad (2008) [tvdbid-81189]",
                "tvdb:81189",
                Some(2008),
            ),
            ("Breaking Bad {tvdb-81189}", "tvdb:81189", None),
            ("Spirited Away (2001) {tmdb-129}", "tmdb:129", Some(2001)),
            ("Fargo [imdbid-tt2802850]", "tt2802850", None),
            ("Fargo [imdbid-nope]", "Fargo", None),
            ("The.Expanse", "The Expanse", None),
            ("Mr. Robot", "Mr. Robot", None),
            ("Cowboy_Bebop_(1998)", "Cowboy Bebop", Some(1998)),
            ("Show (1080p) [Group]", "Show (1080p)", None),
            ("Doctor Who (2005) (2005)", "Doctor Who", Some(2005)),
        ];
        for (name, term, year) in cases {
            assert_eq!(folder_search_term(name), (term.to_string(), year), "{name}");
        }
    }
}

fn default_move_files() -> bool {
    true
}

/// Changes a title's folder (Sonarr's series path edit with `moveFiles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MoveTitleRequest {
    /// The new folder: absolute, inside a root folder.
    pub path: String,
    /// Move the folder's contents there; otherwise only the recorded folder changes (the files
    /// are expected there already).
    #[serde(default = "default_move_files")]
    pub move_files: bool,
}

/// Moves series to another root folder (Sonarr's series editor root folder with `moveFiles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkMoveRequest {
    pub ids: Vec<i64>,
    pub root_folder_path: String,
    #[serde(default = "default_move_files")]
    pub move_files: bool,
}

/// The outcome of a folder change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MoveResult {
    pub id: i64,
    pub path: String,
    pub root_folder_path: String,
    /// Whether files were moved (false when only the recorded folder changed).
    pub moved: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkMoveResult {
    pub moved: Vec<MoveResult>,
    /// `Title: reason` for each series that kept its folder.
    pub failures: Vec<String>,
}

/// Removes several series (Sonarr's series editor delete).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkDeleteRequest {
    pub ids: Vec<i64>,
    /// Also delete each series folder (to the recycle bin when one is set).
    #[serde(default)]
    pub delete_files: bool,
    /// Keep import lists from adding them again.
    #[serde(default)]
    pub add_import_list_exclusion: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkDeleteResult {
    pub deleted: u64,
    /// `Title: reason` for each series that was kept.
    pub failures: Vec<String>,
}

/// Renames every file of several series (Sonarr's series editor "Organize").
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkRenameRequest {
    pub ids: Vec<i64>,
}

/// A file's media information, formatted as Sonarr's file details show it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MediaInfoSummary {
    pub video_codec: String,
    pub video_dynamic_range: String,
    /// `1920x1080`.
    pub resolution: String,
    /// `m:ss` or `h:mm:ss`.
    pub run_time: String,
    pub video_bitrate: i64,
    pub video_bit_depth: i32,
    pub video_fps: f64,
    pub scan_type: Option<String>,
    pub audio: Vec<AudioStreamSummary>,
    /// Subtitle stream languages (or formats when a stream names no language).
    pub subtitles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AudioStreamSummary {
    pub language: Option<String>,
    pub codec: String,
    pub channels: f64,
}

/// An episode file with what Sonarr's file details and episode file editor show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EpisodeFileDetail {
    pub id: i64,
    pub series_id: i64,
    pub season_number: i32,
    /// `S01E02` labels of the file's episodes.
    pub episodes: Vec<String>,
    pub relative_path: String,
    pub path: String,
    pub size: i64,
    pub quality: crate::quality::QualityModel,
    pub quality_name: String,
    pub languages: Vec<i32>,
    pub release_group: Option<String>,
    pub scene_name: Option<String>,
    /// `unknown`, `singleEpisode`, `multiEpisode` or `seasonPack`.
    pub release_type: String,
    pub added_at: String,
    pub media_info: Option<MediaInfoSummary>,
}

/// `GET /episodes/{id}`: Sonarr's episode details (summary, file, history).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EpisodeDetail {
    pub episode: EpisodeItem,
    pub series_title: String,
    pub overview: Option<String>,
    pub file: Option<EpisodeFileDetail>,
}

/// Sonarr's episode file editor: the same changes for several files; absent fields are kept.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EpisodeFileEditRequest {
    pub episode_file_ids: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<crate::quality::QualityModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_group: Option<String>,
    /// `unknown`, `singleEpisode`, `multiEpisode` or `seasonPack`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_type: Option<String>,
}

/// Episode files to delete (to the recycle bin when one is set).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EpisodeFileIds {
    pub episode_file_ids: Vec<i64>,
}

/// Converts a TheTVDB series that is really an anime into Tanukarr-style AniList group members.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AnimeConversionRequest {
    /// Only plan the conversion: nothing is written and no file is moved.
    #[serde(default)]
    pub dry_run: bool,
}

/// [`AnimeConversionRequest`] for several series; each is planned or converted on its own.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkAnimeConversionRequest {
    pub series_ids: Vec<i64>,
    #[serde(default)]
    pub dry_run: bool,
}

/// One AniList entry the series' episodes map to.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AnimeConversionMember {
    pub anilist_id: i64,
    pub title: String,
    /// The entry's season number in the group ("Season N").
    pub ordinal: i32,
    /// The member's folder.
    pub path: String,
    /// The member's episodes (AniList's count).
    pub episodes: i32,
    /// The TheTVDB episodes mapped to it, as `S01E13–S01E24`.
    pub source_episodes: String,
    pub files: i32,
    pub monitored_episodes: i32,
    /// A library series already owns this AniList entry and receives the episodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_series_id: Option<i64>,
    /// The member's series once converted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,
}

/// A file moved into its member's folder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AnimeConversionMove {
    pub from: String,
    pub to: String,
}

/// The plan (dry run) or outcome of an anime conversion.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AnimeConversionPlan {
    pub series_id: i64,
    pub title: String,
    /// Whether the series can be converted (no problem found).
    pub convertible: bool,
    /// Whether the conversion was carried out (never on a dry run).
    pub applied: bool,
    /// Why the series cannot be converted.
    pub problems: Vec<String>,
    /// `anilist:{root id}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_title: Option<String>,
    /// The group folder (the series' current folder).
    pub group_path: String,
    /// The members to create or fill: only the seasons followed (with files or monitored
    /// episodes).
    pub members: Vec<AnimeConversionMember>,
    pub moves: Vec<AnimeConversionMove>,
    /// What is not carried over: seasons not followed, episodes beyond the AniList entries.
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BulkAnimeConversionResult {
    pub results: Vec<AnimeConversionPlan>,
}
