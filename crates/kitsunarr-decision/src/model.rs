//! Inputs and outputs of the decision engine.

use chrono::{DateTime, Utc};
use kitsunarr_domain::api::settings::{QualityDefinition, QualityProfile};
use kitsunarr_domain::quality::QualityModel;
use kitsunarr_domain::{DownloadProtocol, Language, SeriesType};
use kitsunarr_parser::{ParsedEpisodeInfo, ParsedQuality, ReleaseType};
use serde::Serialize;

use crate::profile::ProperDownloadType;

/// A release offered by an indexer (`ReleaseInfo`/`TorrentInfo`), with the indexer settings
/// the decisions need.
#[derive(Debug, Clone)]
pub struct Release {
    pub guid: String,
    pub title: String,
    pub size: i64,
    pub publish_date: DateTime<Utc>,
    pub protocol: DownloadProtocol,
    pub indexer_id: i64,
    pub indexer_priority: i32,
    pub indexer_tags: Vec<i64>,
    pub indexer_flags: u32,
    /// Languages the indexer declared.
    pub languages: Vec<&'static Language>,
    pub seeders: Option<i64>,
    pub peers: Option<i64>,
    pub tvdb_id: i64,
    pub tv_rage_id: i64,
    pub imdb_id: Option<String>,
    /// The torrent indexer's "minimum seeders" setting.
    pub minimum_seeders: Option<i64>,
    /// The indexer's "multi languages" setting (language ids).
    pub multi_languages: Vec<i32>,
    /// The indexer's name, for blocklist matching.
    pub indexer: String,
    /// Torrents: the info hash the indexer gave (or the magnet link's).
    pub info_hash: Option<String>,
}

/// A blocklisted release (`ReleaseComparerModel`).
#[derive(Debug, Clone)]
pub struct Blocklisted {
    pub series_id: i64,
    pub title: String,
    pub protocol: DownloadProtocol,
    pub indexer: Option<String>,
    pub published_date: Option<DateTime<Utc>>,
    pub torrent_info_hash: Option<String>,
}

/// A file on disk, as custom formats and upgrade rules see it.
#[derive(Debug, Clone)]
pub struct EpisodeFile {
    pub id: i64,
    pub quality: QualityModel,
    /// The scene name, else the original file name, else the file name.
    pub release_title: String,
    pub file_name: String,
    pub release_group: Option<String>,
    pub languages: Vec<&'static Language>,
    pub size: i64,
    pub indexer_flags: u32,
    pub release_type: ReleaseType,
    /// When the file was added (`DateAdded`), for the propers rule of RSS sync.
    pub date_added: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct Episode {
    pub id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub scene_season_number: Option<i32>,
    pub scene_episode_number: Option<i32>,
    pub title: Option<String>,
    /// `yyyy-MM-dd`.
    pub air_date: Option<String>,
    pub air_date_utc: Option<DateTime<Utc>>,
    /// Minutes; 0 when unknown.
    pub runtime: i32,
    pub monitored: bool,
    pub file: Option<EpisodeFile>,
}

/// The searched series.
#[derive(Debug, Clone)]
pub struct Series {
    pub id: i64,
    pub monitored: bool,
    pub title: String,
    /// 0 when unknown.
    pub year: i32,
    pub series_type: SeriesType,
    pub tvdb_id: i64,
    pub tv_rage_id: i64,
    pub imdb_id: Option<String>,
    /// Other names of the series; they play the role of Sonarr's scene mappings.
    pub alternate_titles: Vec<String>,
    /// Minutes; 0 when unknown.
    pub runtime: i32,
    pub original_language: Option<&'static Language>,
    pub use_scene_numbering: bool,
    pub tags: Vec<i64>,
    pub profile: QualityProfile,
    pub episodes: Vec<Episode>,
}

/// Another title of the library, to recognise releases of other series.
#[derive(Debug, Clone)]
pub struct LibrarySeries {
    pub id: i64,
    pub title: String,
    pub year: i32,
    pub alternate_titles: Vec<String>,
    pub tvdb_id: i64,
    pub tv_rage_id: i64,
    pub imdb_id: Option<String>,
}

/// The kind of search that found the releases (the `SearchCriteriaBase` subclass).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    SingleEpisode {
        season: i32,
        episode: i32,
    },
    Season {
        season: i32,
    },
    DailyEpisode,
    AnimeEpisode {
        season_search: bool,
    },
    AnimeSeason,
    Special,
    /// RSS sync: no search criteria (`searchCriteria == null` upstream).
    Rss,
    /// Tanukarr's complete-series search; interactive searches also take packs of some seasons.
    CompleteSeries {
        include_season_packs: bool,
    },
}

#[derive(Debug, Clone)]
pub struct SearchCriteria {
    pub kind: SearchKind,
    /// The requested episodes (`SearchCriteria.Episodes`).
    pub episode_ids: Vec<i64>,
    /// Only monitored episodes are wanted (automatic season and missing searches); interactive
    /// and single-episode searches take any episode.
    pub monitored_episodes_only: bool,
}

impl SearchCriteria {
    /// RSS sync (`GetRssDecision`): the RSS-only rules apply, the search-only ones do not, and
    /// only monitored episodes are wanted.
    pub fn rss() -> Self {
        Self {
            kind: SearchKind::Rss,
            episode_ids: Vec::new(),
            monitored_episodes_only: true,
        }
    }

    pub fn is_rss(&self) -> bool {
        self.kind == SearchKind::Rss
    }
}

/// A download in the queue (grabbed, still in the download client, not imported).
#[derive(Debug, Clone)]
pub struct QueuedRelease {
    pub series_id: i64,
    pub episode_ids: Vec<i64>,
    /// The grabbed release's title, for its custom formats.
    pub title: String,
    pub quality: QualityModel,
    pub size: i64,
    pub languages: Vec<&'static Language>,
    pub indexer_flags: u32,
}

/// An episode's most recent history event, when it is a grab (`MostRecentForEpisode`), for the
/// history rule of RSS sync.
#[derive(Debug, Clone)]
pub struct HistoryGrab {
    pub episode_id: i64,
    pub date: DateTime<Utc>,
    /// The grabbed release's title, for its custom formats.
    pub title: String,
    pub quality: QualityModel,
    pub size: i64,
    pub languages: Vec<&'static Language>,
    pub indexer_flags: u32,
    pub release_type: ReleaseType,
}

#[derive(Debug, Clone)]
pub struct ReleaseProfile {
    pub enabled: bool,
    pub required: Vec<String>,
    pub ignored: Vec<String>,
    /// Empty: every indexer.
    pub indexer_ids: Vec<i64>,
    pub tags: Vec<i64>,
}

#[derive(Debug, Clone)]
pub struct DelayProfile {
    pub enable_usenet: bool,
    pub enable_torrent: bool,
    pub preferred_protocol: DownloadProtocol,
    /// Minutes.
    pub usenet_delay: i64,
    /// Minutes.
    pub torrent_delay: i64,
    pub bypass_if_highest_quality: bool,
    pub bypass_if_above_custom_format_score: bool,
    pub minimum_custom_format_score: i32,
    pub order: i64,
    pub tags: Vec<i64>,
}

/// Instance settings the decisions depend on.
#[derive(Debug, Clone)]
pub struct Settings {
    pub quality_definitions: Vec<QualityDefinition>,
    pub propers: ProperDownloadType,
    pub release_profiles: Vec<ReleaseProfile>,
    pub delay_profiles: Vec<DelayProfile>,
    /// Usenet minimum age in minutes (0: off).
    pub minimum_age_minutes: i64,
    /// Usenet retention in days (0: unlimited).
    pub retention_days: i64,
    /// Maximum release size in MB (0: unlimited).
    pub maximum_size_mb: i64,
    /// Downloads in progress, for `QueueSpecification`.
    pub queue: Vec<QueuedRelease>,
    /// Episodes whose most recent history event is a grab, for `HistorySpecification`.
    pub last_grabs: Vec<HistoryGrab>,
    /// Blocklisted releases, for `BlocklistSpecification`.
    pub blocklist: Vec<Blocklisted>,
    /// Tanukarr's "multi-season releases" option.
    pub enable_multi_season_releases: bool,
    /// Tanukarr's Library tools "match external ids".
    pub match_external_ids: bool,
    pub now: DateTime<Utc>,
}

/// Why a release was rejected (`DownloadRejectionReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RejectionReason {
    UnableToParse,
    UnknownSeries,
    UnknownEpisode,
    WrongSeries,
    WrongSeason,
    WrongEpisode,
    FullSeason,
    QualityNotWanted,
    CustomFormatMinimumScore,
    BelowMinimumSize,
    AboveMaximumSize,
    UnknownRuntime,
    MaximumSizeExceeded,
    MinimumAge,
    MaximumAge,
    Sample,
    Raw,
    SplitEpisode,
    MultiSeason,
    FullSeasonNotAired,
    MustContainMissing,
    MustNotContainPresent,
    MinimumSeeders,
    ProtocolDisabled,
    NoMatchingTag,
    DiskCutoffMet,
    DiskHigherPreference,
    DiskHigherRevision,
    DiskCustomFormatCutoffMet,
    DiskCustomFormatScore,
    DiskCustomFormatScoreIncrement,
    DiskUpgradesNotAllowed,
    DiskNotUpgrade,
    QualityUpgradesDisabled,
    RepackDisabled,
    RepackUnknownReleaseGroup,
    RepackReleaseGroupDoesNotMatch,
    UnknownReleaseGroup,
    ReleaseGroupDoesNotMatch,
    ExistingFileHasMoreEpisodes,
    SeriesNotMonitored,
    EpisodeNotMonitored,
    QueueCutoffMet,
    QueueHigherPreference,
    QueueHigherRevision,
    QueueCustomFormatCutoffMet,
    QueueCustomFormatScore,
    QueueCustomFormatScoreIncrement,
    QueueUpgradesNotAllowed,
    QueuePropersDisabled,
    HistoryRecentCutoffMet,
    HistoryHigherPreference,
    HistoryHigherRevision,
    HistoryCutoffMet,
    HistoryCustomFormatCutoffMet,
    HistoryCustomFormatScore,
    HistoryCustomFormatScoreIncrement,
    HistoryUpgradesNotAllowed,
    Blocklisted,
    MinimumAgeDelay,
    PropersDisabled,
    ProperForOldFile,
    DecisionError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rejection {
    pub reason: RejectionReason,
    pub message: String,
}

impl Rejection {
    pub(crate) fn new(reason: RejectionReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

/// The engine's verdict on one release (`DownloadDecision`).
#[derive(Debug, Clone)]
pub struct Decision {
    /// Index of the release in the evaluated list.
    pub release: usize,
    pub parsed: Option<ParsedEpisodeInfo>,
    pub quality: ParsedQuality,
    pub languages: Vec<&'static Language>,
    /// The library series the release was mapped to.
    pub series_id: Option<i64>,
    /// Mapped episodes, as (id, season, episode, runtime).
    pub episodes: Vec<MappedEpisode>,
    pub custom_formats: Vec<String>,
    pub custom_format_score: i32,
    pub rejections: Vec<Rejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappedEpisode {
    pub id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub runtime: i32,
}

impl Decision {
    /// `Approved`: no rejections.
    pub fn approved(&self) -> bool {
        self.rejections.is_empty()
    }
}
