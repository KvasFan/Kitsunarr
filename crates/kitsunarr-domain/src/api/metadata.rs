//! Metadata search and "add to library" contract.

use serde::{Deserialize, Serialize};

use crate::api::settings::MetadataSourcesSettings;
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

/// Tanukarr's default catalogue order (`MetadataSourceType` values 0 to 6).
pub const DEFAULT_SOURCE_ORDER: &[MetadataProvider] = &[
    MetadataProvider::Tvdb,
    MetadataProvider::TmdbTv,
    MetadataProvider::AniList,
    MetadataProvider::MyAnimeList,
    MetadataProvider::TvMaze,
    MetadataProvider::TmdbMovie,
    MetadataProvider::Omdb,
];

/// Tanukarr's `MetadataSourceType` number of a catalogue (its `Config` values store these).
pub fn tanukarr_source_id(provider: MetadataProvider) -> i32 {
    DEFAULT_SOURCE_ORDER
        .iter()
        .position(|p| *p == provider)
        .map_or(0, |i| i as i32)
}

/// The catalogue a Tanukarr `MetadataSourceType` number names.
pub fn from_tanukarr_source_id(id: i32) -> Option<MetadataProvider> {
    usize::try_from(id)
        .ok()
        .and_then(|i| DEFAULT_SOURCE_ORDER.get(i).copied())
}

/// `MetadataSourcePolicy.Order`: the configured order, then every other catalogue in the
/// default order.
pub fn source_order(settings: &MetadataSourcesSettings) -> Vec<MetadataProvider> {
    let mut out: Vec<MetadataProvider> = Vec::with_capacity(DEFAULT_SOURCE_ORDER.len());
    for p in settings.source_order.iter().chain(DEFAULT_SOURCE_ORDER) {
        if !out.contains(p) {
            out.push(*p);
        }
    }
    out
}

/// `MetadataSourcePolicy.Enabled`: the ordered catalogues that are not disabled.
pub fn enabled_sources(settings: &MetadataSourcesSettings) -> Vec<MetadataProvider> {
    source_order(settings)
        .into_iter()
        .filter(|p| !settings.disabled_sources.contains(p))
        .collect()
}

/// `MetadataSourcePolicy.Save` validation: `(field, message)` for each problem.
pub fn validate_sources(
    order: &[MetadataProvider],
    disabled: &[MetadataProvider],
) -> Vec<(&'static str, &'static str)> {
    let mut failures = Vec::new();
    let distinct =
        |list: &[MetadataProvider]| list.iter().enumerate().all(|(i, p)| !list[..i].contains(p));
    if !distinct(order) {
        failures.push(("sourceOrder", "Choose each metadata source only once."));
    }
    if !distinct(disabled) {
        failures.push((
            "disabledSources",
            "Choose valid metadata sources only once.",
        ));
    }
    if DEFAULT_SOURCE_ORDER.iter().all(|p| disabled.contains(p)) {
        failures.push(("disabledSources", "Enable at least one metadata source."));
    }
    failures
}

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
    /// Offered and searched (Tanukarr's `DisabledMetadataSources` turns a catalogue off).
    #[serde(default = "available_default")]
    pub enabled: bool,
}

fn available_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// The catalogue to search; the first enabled one when omitted (ignored with
    /// `allSources`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<MetadataProvider>,
    pub term: String,
    /// Tanukarr's `allSources`: search every enabled catalogue and merge the results in the
    /// configured catalogue order.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_sources: bool,
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

/// Which episodes to monitor (Sonarr's `MonitorTypes`), when adding a series or applied to an
/// existing one. Specials are only changed by `monitorSpecials`/`unmonitorSpecials` (and
/// unmonitored by every other choice except `skip`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum MonitorOption {
    #[default]
    All,
    /// Episodes airing from now on, and those without an air date.
    Future,
    /// Episodes without a file.
    Missing,
    /// Episodes with a file.
    Existing,
    FirstSeason,
    #[serde(alias = "lastSeason")]
    LatestSeason,
    /// The first episode of the first season.
    Pilot,
    /// Episodes aired in the last 90 days, future ones and those without an air date.
    Recent,
    /// Monitors the specials and leaves the other episodes as they are.
    MonitorSpecials,
    /// Unmonitors the specials and leaves the other episodes as they are.
    UnmonitorSpecials,
    None,
    /// Leaves the episodes' monitoring as it is.
    Skip,
}

impl MonitorOption {
    /// Every choice in Sonarr's order, with its API value.
    pub const ALL: [(MonitorOption, &'static str); 12] = [
        (MonitorOption::All, "all"),
        (MonitorOption::Future, "future"),
        (MonitorOption::Missing, "missing"),
        (MonitorOption::Existing, "existing"),
        (MonitorOption::FirstSeason, "firstSeason"),
        (MonitorOption::LatestSeason, "latestSeason"),
        (MonitorOption::Pilot, "pilot"),
        (MonitorOption::Recent, "recent"),
        (MonitorOption::MonitorSpecials, "monitorSpecials"),
        (MonitorOption::UnmonitorSpecials, "unmonitorSpecials"),
        (MonitorOption::None, "none"),
        (MonitorOption::Skip, "skip"),
    ];

    pub fn as_str(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(o, _)| *o == self)
            .map_or("all", |(_, s)| s)
    }

    /// Parses Sonarr's value (`MonitorTypes` names, case-insensitive, or their numbers);
    /// `unknown` and unrecognised values give `None`.
    pub fn from_sonarr(value: &str) -> Option<Self> {
        let v = value.trim();
        if let Ok(n) = v.parse::<i64>() {
            return match n {
                1 => Some(Self::All),
                2 => Some(Self::Future),
                3 => Some(Self::Missing),
                4 => Some(Self::Existing),
                5 => Some(Self::FirstSeason),
                6 | 7 => Some(Self::LatestSeason),
                8 => Some(Self::Pilot),
                9 => Some(Self::Recent),
                10 => Some(Self::MonitorSpecials),
                11 => Some(Self::UnmonitorSpecials),
                12 => Some(Self::None),
                13 => Some(Self::Skip),
                _ => None,
            };
        }
        if v.eq_ignore_ascii_case("lastSeason") {
            return Some(Self::LatestSeason);
        }
        Self::ALL
            .iter()
            .find(|(_, s)| s.eq_ignore_ascii_case(v))
            .map(|(o, _)| *o)
    }

    /// An English label for pickers.
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All episodes",
            Self::Future => "Future episodes",
            Self::Missing => "Missing episodes",
            Self::Existing => "Existing episodes",
            Self::FirstSeason => "First season",
            Self::LatestSeason => "Last season",
            Self::Pilot => "Pilot episode",
            Self::Recent => "Recent episodes",
            Self::MonitorSpecials => "Monitor specials",
            Self::UnmonitorSpecials => "Unmonitor specials",
            Self::None => "None",
            Self::Skip => "Don't change",
        }
    }
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
    /// Tanukarr's ordering chosen when adding (a TMDB episode group, a TheTVDB season type);
    /// `None` for the catalogue's default.
    #[serde(default)]
    pub episode_ordering: Option<String>,
    /// An existing folder inside the root folder to use as the title's folder (Sonarr's
    /// library import). Its files are scanned in place before the monitoring choice applies,
    /// so "existing" and "missing" see them. `None` plans the folder from the naming format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

fn default_true() -> bool {
    true
}

/// `POST /library/add`: Sonarr's add request with Kitsunarr's native add options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AddTitle {
    #[serde(flatten)]
    pub title: AddTitleRequest,
    /// Tanukarr's anime grouping mode: `false` is "Disabled", the title gets no group folder
    /// and stays out of automatic anime groups and TMDB collections.
    #[serde(default = "default_true")]
    pub automatic_grouping: bool,
    /// Tanukarr's "Manual" grouping: the title joins this existing anime group or manual
    /// collection (and goes to its group folder) instead of being grouped automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_group_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RefreshResult {
    pub added_episodes: u64,
    pub updated_episodes: u64,
    /// Server side only: monitored episodes the refresh added, gave an absolute number (anime)
    /// or moved into the recent past (Tanukarr's `EpisodeRefreshedService`); they are searched
    /// once the refresh is done. Not part of the API.
    #[serde(skip)]
    pub recently_aired_episode_ids: Vec<i64>,
}

/// Preview a catalogue change first, then send its revision when applying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MetadataSourceRequest {
    pub provider: MetadataProvider,
    pub id: String,
    #[serde(default)]
    pub confirm_identity: bool,
    #[serde(default)]
    pub mappings: Vec<SourceEpisodeMapping>,
    #[serde(default)]
    pub revision: Option<String>,
}

/// An explicit episode association, used when catalogue numbering differs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SourceEpisodeMapping {
    pub episode_id: i64,
    pub season_number: i32,
    pub episode_number: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SourceEpisodePreview {
    pub episode_id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
    pub target: Option<SourceEpisodeMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MetadataSourcePreview {
    pub provider: MetadataProvider,
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub identity_verified: bool,
    pub revision: String,
    pub blockers: Vec<String>,
    pub matched_episodes: usize,
    pub retained_episodes: usize,
    pub new_episodes: usize,
    pub episodes: Vec<SourceEpisodePreview>,
    pub target_episodes: Vec<SourceTargetEpisode>,
}

/// One target episode shown when manually associating episodes across catalogues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SourceTargetEpisode {
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
}
