//! Contracts of Tanukarr's interface pages Kitsunarr mirrors: naming samples, library
//! statistics, collection settings, host settings, log files and the parse tester.

use serde::{Deserialize, Serialize};

use crate::media::SeriesType;

/// Sonarr's `NamingExampleResource` (`FileNameSampleService`): the names the naming settings
/// give sample titles. `None` is an invalid or empty format.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct NamingExamples {
    pub single_episode_example: Option<String>,
    pub multi_episode_example: Option<String>,
    pub daily_episode_example: Option<String>,
    pub anime_episode_example: Option<String>,
    pub anime_multi_episode_example: Option<String>,
    pub series_folder_example: Option<String>,
    pub season_folder_example: Option<String>,
    pub specials_folder_example: Option<String>,
    pub movie_example: Option<String>,
    /// Kitsunarr's movie folder format (movies are their own library).
    pub movie_folder_example: Option<String>,
    pub anime_group_folder_example: Option<String>,
    pub anime_group_season_folder_example: Option<String>,
    pub anime_group_arc_folder_example: Option<String>,
    pub anime_group_movie_folder_example: Option<String>,
    pub anime_group_movie_saga_folder_example: Option<String>,
    pub anime_group_movie_episode_example: Option<String>,
}

/// Filters of the statistics page (Sonarr v5's `StatisticsFilter`, one value each).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase", default)]
pub struct StatisticsQuery {
    pub monitored: Option<bool>,
    pub series_type: Option<SeriesType>,
    pub quality_profile_id: Option<i64>,
    pub tag_id: Option<i64>,
}

/// Sonarr v5's `StatisticsResource` (Tanukarr's statistics page): series, seasons, episodes and
/// files of the library, with breakdowns by quality profile, file quality and tag.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct LibraryStatistics {
    pub series_count: i64,
    pub monitored_series_count: i64,
    pub completed_series_count: i64,
    pub continuing_series_count: i64,
    pub ended_series_count: i64,
    pub upcoming_series_count: i64,
    pub deleted_series_count: i64,
    pub standard_series_count: i64,
    pub daily_series_count: i64,
    pub anime_series_count: i64,
    pub season_count: i64,
    pub completed_season_count: i64,
    pub total_episode_count: i64,
    pub monitored_episode_count: i64,
    pub downloaded_episode_count: i64,
    pub missing_episode_count: i64,
    pub unaired_episode_count: i64,
    pub episode_file_count: i64,
    pub size_on_disk: i64,
    pub quality_profile_statistics: Vec<QualityProfileStatistics>,
    pub quality_statistics: Vec<QualityStatistics>,
    pub tag_statistics: Vec<TagStatistics>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct QualityProfileStatistics {
    pub quality_profile_id: i64,
    pub name: String,
    pub series_count: i64,
    pub episode_file_count: i64,
    pub size_on_disk: i64,
}

/// A file quality (`{id, name}` of Sonarr's quality resource).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct QualityName {
    pub id: i32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct QualityStatistics {
    pub quality: QualityName,
    pub episode_file_count: i64,
    pub size_on_disk: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct TagStatistics {
    pub tag_id: i64,
    pub label: String,
    pub series_count: i64,
    pub episode_file_count: i64,
    pub size_on_disk: i64,
}

/// Tanukarr's `AnimeGroupKind`.
pub const GROUP_KINDS: &[&str] = &["series", "movieSaga"];
/// Tanukarr's `AnimeGroupMovieFolderStyle`.
pub const MOVIE_FOLDER_STYLES: &[&str] = &["automatic", "numbered", "title"];

/// Tanukarr's collection settings (`CollectionSettingsModal`): the group's kind and folder
/// style are stored on the collection; monitoring, quality profile and season folders are
/// applied to every member and not stored. Absent fields are left unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct CollectionSettingsUpdate {
    pub title: Option<String>,
    /// `series` or `movieSaga` (a saga holds only films).
    pub group_kind: Option<String>,
    /// `automatic`, `numbered` or `title`.
    pub movie_folder_style: Option<String>,
    pub monitored: Option<bool>,
    pub quality_profile_id: Option<i64>,
    /// Series members only.
    pub season_folder: Option<bool>,
}

/// Settings › General › Host and Logging: values of `config.toml`, applied at the next start.
/// `overridden` names the keys an environment variable or command-line flag sets, which win
/// over the file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct HostSettings {
    pub bind_address: String,
    pub port: u16,
    pub url_base: String,
    /// `trace`, `debug`, `info`, `warn` or `error` (or a `tracing` filter).
    pub log_level: String,
    /// Empty: `<data dir>/backups`.
    pub backup_folder: String,
    /// Read only: the values the running server uses.
    pub running: HostValues,
    /// Read only: config keys set by `KITSUNARR_*` variables or flags (`port`, `bindAddress`,
    /// `urlBase`, `logLevel`, `backupFolder`).
    pub overridden: Vec<String>,
    /// Read only: the configuration file the values are written to.
    pub config_file: String,
    /// Read only: saved values differ from the running ones until Kitsunarr restarts.
    pub restart_required: bool,
}

/// The host values in effect.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct HostValues {
    pub bind_address: String,
    pub port: u16,
    pub url_base: String,
    pub log_level: String,
    pub backup_folder: String,
}

/// A log file of `<data dir>/logs` (Sonarr's `LogFileResource`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LogFileItem {
    pub filename: String,
    pub last_write_time: String,
    pub size: u64,
}

/// What the parser reads from a release title, and the library entries it maps to (Sonarr's
/// `ParseResource`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ParseResult {
    pub title: String,
    pub parsed: Option<ParsedRelease>,
    pub series: Option<ParsedSeries>,
    pub episodes: Vec<ParsedEpisode>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ParsedRelease {
    pub release_title: String,
    pub series_title: String,
    pub season_numbers: Vec<i32>,
    pub episode_numbers: Vec<i32>,
    pub absolute_episode_numbers: Vec<i32>,
    pub air_date: Option<String>,
    pub quality: String,
    pub quality_source: String,
    pub resolution: Option<i32>,
    pub revision: i32,
    pub is_repack: bool,
    pub languages: Vec<String>,
    pub release_group: Option<String>,
    pub release_hash: Option<String>,
    pub full_season: bool,
    pub is_partial_season: bool,
    pub is_multi_season: bool,
    pub is_season_extra: bool,
    pub special: bool,
    pub is_daily: bool,
    pub is_absolute_numbering: bool,
    pub is_possible_special_episode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ParsedSeries {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ParsedEpisode {
    pub id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub title: Option<String>,
    pub air_date: Option<String>,
    pub has_file: bool,
}
