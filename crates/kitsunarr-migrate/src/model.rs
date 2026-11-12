//! Source-agnostic snapshot of a Sonarr/Tanukarr installation.

use std::collections::HashMap;

use kitsunarr_domain::api::settings::{CustomFormatSpecification, ProviderKind};
use kitsunarr_domain::{MediaImage, MetadataProvider, MonitorNewItems, SeriesStatus, SeriesType};
use serde_json::{Map, Value};

use crate::values::Rec;

/// How the raw records were obtained (affects a few encodings, e.g. masked secrets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Database,
    Api,
}

/// Raw records keyed by lower-case table/resource name.
#[derive(Debug, Clone)]
pub struct RawSource {
    pub format: SourceFormat,
    pub tables: HashMap<String, Vec<Rec>>,
}

impl RawSource {
    pub fn table(&self, name: &str) -> &[Rec] {
        self.tables.get(name).map(Vec::as_slice).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcMediaType {
    Series,
    Movie,
    Special,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcSeason {
    pub number: i32,
    pub monitored: bool,
    pub images: Vec<MediaImage>,
}

/// Tanukarr group/collection membership columns.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SrcGroup {
    pub group_id: Option<String>,
    pub group_title: Option<String>,
    /// 0 series group, 1 movie saga.
    pub group_kind: i64,
    /// 0 automatic, 1 manual, 2 disabled.
    pub grouping_mode: i64,
    /// 0 season, 1 movie, 2 special.
    pub member_kind: i64,
    pub season_number: Option<i64>,
    pub movie_number: Option<i64>,
    pub special_number: Option<i64>,
}

impl SrcGroup {
    pub fn position(&self) -> i64 {
        match self.member_kind {
            1 => 1000 + self.movie_number.unwrap_or(0),
            2 => 2000 + self.special_number.unwrap_or(0),
            _ => self.season_number.unwrap_or(0),
        }
    }

    pub fn role(&self) -> &'static str {
        match self.member_kind {
            1 => "movie",
            2 => "special",
            _ => "season",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcSeries {
    pub id: i64,
    pub title: String,
    pub status: SeriesStatus,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub air_time: Option<String>,
    pub images: Vec<MediaImage>,
    pub path: String,
    pub root_folder_path: Option<String>,
    pub monitored: bool,
    pub monitor_new_items: MonitorNewItems,
    pub season_folder: bool,
    pub use_scene_numbering: bool,
    pub series_type: SeriesType,
    pub runtime: Option<i32>,
    pub year: Option<i32>,
    pub first_aired: Option<String>,
    pub last_aired: Option<String>,
    pub certification: Option<String>,
    pub genres: Vec<String>,
    pub ratings: Option<Value>,
    pub original_language: Option<i32>,
    pub quality_profile_id: i64,
    pub tags: Vec<i64>,
    pub added: Option<String>,
    pub last_info_sync: Option<String>,
    pub seasons: Vec<SrcSeason>,
    pub tvdb_id: i64,
    pub tv_rage_id: i64,
    pub tv_maze_id: i64,
    pub tmdb_id: i64,
    pub imdb_id: Option<String>,
    pub mal_ids: Vec<i64>,
    pub anilist_ids: Vec<i64>,
    /// `(title, season)`.
    pub alternate_titles: Vec<(String, Option<i32>)>,
    // Tanukarr additions
    pub metadata_source: Option<MetadataProvider>,
    pub foreign_id: Option<String>,
    pub media_type: SrcMediaType,
    pub ordering_id: Option<String>,
    pub collection_title: Option<String>,
    pub group: Option<SrcGroup>,
    pub original_title: Option<String>,
    /// User-level values the Kitsunarr model cannot hold yet (recorded, never dropped silently).
    pub unmapped: Map<String, Value>,
}

impl SrcSeries {
    /// Owning catalogue and native identifier. Tanukarr records carry them explicitly; Sonarr
    /// series are always TheTVDB-owned.
    pub fn identity(&self) -> (MetadataProvider, String) {
        if let (Some(source), Some(id)) = (self.metadata_source, self.foreign_id.as_deref()) {
            return (source, id.to_string());
        }
        (MetadataProvider::Tvdb, self.tvdb_id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcEpisode {
    pub id: i64,
    pub series_id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub scene_absolute_episode_number: Option<i32>,
    pub scene_season_number: Option<i32>,
    pub scene_episode_number: Option<i32>,
    pub unverified_scene_numbering: bool,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub air_date_utc: Option<String>,
    pub runtime: Option<i32>,
    pub monitored: bool,
    pub episode_file_id: Option<i64>,
    pub last_search_time: Option<String>,
    pub provider_episode_id: Option<String>,
    pub unmapped: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SrcRevision {
    pub version: i32,
    pub real: i32,
    pub is_repack: bool,
}

impl Default for SrcRevision {
    fn default() -> Self {
        Self {
            version: 1,
            real: 0,
            is_repack: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcEpisodeFile {
    pub id: i64,
    pub series_id: i64,
    pub season_number: i32,
    pub relative_path: String,
    pub size: i64,
    pub date_added: Option<String>,
    pub scene_name: Option<String>,
    pub release_group: Option<String>,
    pub quality_id: i32,
    pub revision: SrcRevision,
    pub languages: Vec<i32>,
    pub media_info: Option<Value>,
    pub original_file_path: Option<String>,
    pub indexer_flags: i64,
    pub release_type: i64,
    pub unmapped: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcProfileItem {
    pub quality_id: Option<i32>,
    pub group: Option<(i32, String, Vec<i32>)>,
    pub allowed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcQualityProfile {
    pub id: i64,
    pub name: String,
    pub upgrade_allowed: bool,
    pub cutoff: i32,
    pub items: Vec<SrcProfileItem>,
    pub format_items: Vec<(i64, i32)>,
    pub min_format_score: i32,
    pub cutoff_format_score: i32,
    pub min_upgrade_format_score: i32,
    /// Per-quality size limits from profile items (Sonarr v5, Tanukarr).
    pub size_limits: Vec<SrcSizeLimit>,
    /// Tanukarr's codec size factors (family, factor).
    pub codec_multipliers: Vec<(String, f64)>,
    pub unmapped: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcSizeLimit {
    pub quality_id: i32,
    pub min_size: Option<f64>,
    pub preferred_size: Option<f64>,
    pub max_size: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcQualityDefinition {
    pub quality_id: i32,
    pub title: String,
    pub min_size: Option<f64>,
    pub preferred_size: Option<f64>,
    pub max_size: Option<f64>,
    /// Tanukarr's preset criteria (migration 251).
    pub source: Option<kitsunarr_domain::quality::QualitySource>,
    pub resolution: Option<i32>,
    pub codec: String,
    pub title_pattern: String,
    pub is_retired: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcCustomFormat {
    pub id: i64,
    pub name: String,
    pub include_when_renaming: bool,
    pub specifications: Vec<CustomFormatSpecification>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcProvider {
    pub kind: ProviderKind,
    pub id: i64,
    pub name: String,
    pub implementation: String,
    pub config_contract: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub tags: Vec<i64>,
    pub options: Map<String, Value>,
    pub settings: Map<String, Value>,
    /// Secret fields the source returned masked (`********`); must be re-entered.
    pub masked_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcDelayProfile {
    pub id: i64,
    pub enable_usenet: bool,
    pub enable_torrent: bool,
    pub preferred_protocol: String,
    pub usenet_delay: i64,
    pub torrent_delay: i64,
    pub bypass_if_highest_quality: bool,
    pub bypass_if_above_custom_format_score: bool,
    pub minimum_custom_format_score: i64,
    pub order: i64,
    pub tags: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcReleaseProfile {
    pub id: i64,
    pub name: Option<String>,
    pub enabled: bool,
    pub required: Vec<String>,
    pub ignored: Vec<String>,
    pub indexer_ids: Vec<i64>,
    pub tags: Vec<i64>,
    pub unmapped: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcExclusion {
    pub id: i64,
    pub provider: MetadataProvider,
    pub external_id: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrcHistory {
    pub id: i64,
    pub series_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub source_title: String,
    pub date: String,
    pub event_type: String,
    pub quality_id: Option<i32>,
    pub revision: Option<SrcRevision>,
    pub languages: Vec<i32>,
    pub download_id: Option<String>,
    pub data: Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceSnapshot {
    pub tags: Vec<(i64, String)>,
    pub root_folders: Vec<(i64, String)>,
    pub quality_definitions: Vec<SrcQualityDefinition>,
    pub custom_formats: Vec<SrcCustomFormat>,
    pub quality_profiles: Vec<SrcQualityProfile>,
    pub delay_profiles: Vec<SrcDelayProfile>,
    pub release_profiles: Vec<SrcReleaseProfile>,
    pub providers: Vec<SrcProvider>,
    pub exclusions: Vec<SrcExclusion>,
    /// `(id, host, remote, local)`.
    pub remote_path_mappings: Vec<(i64, String, String, String)>,
    pub naming: Option<Rec>,
    pub media_management: Option<Rec>,
    /// The API's indexer options (`/config/indexer`); databases keep them in `Config`.
    pub indexer_options: Option<Rec>,
    /// The API's host settings (`/config/host`), for the proxy; databases keep it in `Config`.
    pub host_options: Option<Rec>,
    /// The API's download client options (`/config/downloadclient`); databases keep them in
    /// `Config`.
    pub download_client_options: Option<Rec>,
    /// The API's import list options (`/config/importlist`); databases keep them in `Config`.
    pub import_list_options: Option<Rec>,
    pub series: Vec<SrcSeries>,
    pub episodes: Vec<SrcEpisode>,
    pub episode_files: Vec<SrcEpisodeFile>,
    pub history: Vec<SrcHistory>,
    /// Source settings Kitsunarr does not model (key → value), already filtered of internals.
    pub unmapped_settings: Map<String, Value>,
    /// Count of internal source keys deliberately skipped (crypto salts, client ids, ...).
    pub internal_settings_skipped: usize,
}

impl SourceSnapshot {
    pub fn movie_count(&self) -> usize {
        self.series
            .iter()
            .filter(|s| s.media_type == SrcMediaType::Movie)
            .count()
    }

    /// Entity inventory for the wizard.
    pub fn inventory(&self) -> Vec<(String, u64)> {
        let providers =
            |k: ProviderKind| self.providers.iter().filter(|p| p.kind == k).count() as u64;
        vec![
            (
                "series".into(),
                (self.series.len() - self.movie_count()) as u64,
            ),
            ("movies".into(), self.movie_count() as u64),
            ("episodes".into(), self.episodes.len() as u64),
            ("episodeFiles".into(), self.episode_files.len() as u64),
            ("qualityProfiles".into(), self.quality_profiles.len() as u64),
            ("customFormats".into(), self.custom_formats.len() as u64),
            ("tags".into(), self.tags.len() as u64),
            ("rootFolders".into(), self.root_folders.len() as u64),
            ("indexers".into(), providers(ProviderKind::Indexer)),
            (
                "downloadClients".into(),
                providers(ProviderKind::DownloadClient),
            ),
            (
                "notifications".into(),
                providers(ProviderKind::Notification),
            ),
            ("importLists".into(), providers(ProviderKind::ImportList)),
            ("importListExclusions".into(), self.exclusions.len() as u64),
            ("delayProfiles".into(), self.delay_profiles.len() as u64),
            ("releaseProfiles".into(), self.release_profiles.len() as u64),
            (
                "remotePathMappings".into(),
                self.remote_path_mappings.len() as u64,
            ),
            ("history".into(), self.history.len() as u64),
        ]
    }

    /// Distinct root folder paths, including parents of series paths not under a known root.
    pub fn root_paths(&self) -> Vec<String> {
        let mut roots: Vec<String> = self.root_folders.iter().map(|(_, p)| p.clone()).collect();
        for s in &self.series {
            if let Some(r) = &s.root_folder_path
                && !roots.contains(r)
            {
                roots.push(r.clone());
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }
}
