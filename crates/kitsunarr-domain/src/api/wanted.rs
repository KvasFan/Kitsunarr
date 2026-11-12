//! Wanted episodes (Sonarr's Wanted › Missing and Cutoff Unmet) and searching for them.

use serde::{Deserialize, Serialize};

use crate::SortDirection;

/// `GET /wanted/missing` and `GET /wanted/cutoff`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct WantedQuery {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
    /// By air date; newest first by default.
    #[serde(default)]
    pub sort_direction: Option<SortDirection>,
    /// `true` (default): monitored episodes of monitored series; `false`: the others.
    #[serde(default)]
    pub monitored: Option<bool>,
}

/// An episode in a wanted list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct WantedEpisode {
    pub episode_id: i64,
    pub series_id: i64,
    pub series_title: String,
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub title: Option<String>,
    pub air_date_utc: Option<String>,
    pub monitored: bool,
    pub last_search_at: Option<String>,
    /// The file's quality (cutoff unmet only).
    pub quality_name: Option<String>,
}

/// Which wanted list a search covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum WantedList {
    Missing,
    CutoffUnmet,
}

/// `POST /wanted/search`: search for some (or all) episodes of a wanted list, as Sonarr's
/// `EpisodeSearch`, `MissingEpisodeSearch` and `CutoffUnmetEpisodeSearch` commands do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct WantedSearchRequest {
    pub list: WantedList,
    /// Episodes to search; empty: every episode of the list.
    #[serde(default)]
    pub episode_ids: Vec<i64>,
    /// The list's filter when searching every episode (default: monitored).
    #[serde(default)]
    pub monitored: Option<bool>,
}

/// A monitored film without a file that is available by its minimum availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct WantedMovie {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub minimum_availability: String,
    pub last_search_at: Option<String>,
}

/// `POST /wanted/movies/search`: search for some (or all) missing films.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct WantedMovieSearchRequest {
    /// Films to search; empty: every missing film.
    #[serde(default)]
    pub movie_ids: Vec<i64>,
}

/// The search started in the background; its outcome arrives as a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct WantedSearchStarted {
    /// Episodes to search (episodes already in the queue are left out).
    pub episodes: usize,
    /// Searches: one per season with several episodes, one per single episode.
    pub searches: usize,
    pub message: String,
}
