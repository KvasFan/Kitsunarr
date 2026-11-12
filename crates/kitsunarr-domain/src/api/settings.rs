use serde::{Deserialize, Serialize};

use crate::media::{DownloadProtocol, MediaKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TagItem {
    pub id: i64,
    pub label: String,
    /// Number of series, movies and providers referencing the tag.
    pub usage_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TagCreate {
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RootFolderItem {
    pub id: i64,
    pub path: String,
    pub media_kind: Option<MediaKind>,
    pub accessible: bool,
    pub free_space: Option<u64>,
    pub total_space: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RootFolderCreate {
    pub path: String,
    pub media_kind: Option<MediaKind>,
}

/// An entry of a quality profile's ordered item list (worst → best).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QualityProfileItem {
    /// Set for a single quality.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_id: Option<i32>,
    /// Set for a group of equally ranked qualities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<QualityGroup>,
    pub allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QualityGroup {
    /// Group identifiers start at 1000 (as in Sonarr) to stay distinct from quality ids.
    pub id: i32,
    pub name: String,
    pub quality_ids: Vec<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FormatScore {
    pub format_id: i64,
    pub score: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QualityProfile {
    pub id: i64,
    pub name: String,
    pub upgrade_allowed: bool,
    /// Quality id or group id at which upgrades stop.
    pub cutoff: i32,
    pub items: Vec<QualityProfileItem>,
    pub min_format_score: i32,
    pub cutoff_format_score: i32,
    pub min_upgrade_format_score: i32,
    pub format_items: Vec<FormatScore>,
    /// Per-quality size limits of this profile (Sonarr v5 and Tanukarr keep sizes on profile
    /// items); a quality without an entry uses the global quality definition.
    #[serde(default)]
    pub size_limits: Vec<QualitySizeLimit>,
    /// Tanukarr's codec size factors overriding the defaults for this profile (codec family →
    /// factor, 0.01 to 10).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = std::collections::BTreeMap<String, f64>))]
    pub codec_multipliers: std::collections::BTreeMap<String, OrderedF64>,
}

/// Size limits of one quality in a profile, in MiB per minute of runtime (`None`: no limit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QualitySizeLimit {
    pub quality_id: i32,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<f64>))]
    pub min_size: Option<OrderedF64>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<f64>))]
    pub preferred_size: Option<OrderedF64>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<f64>))]
    pub max_size: Option<OrderedF64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QualityDefinition {
    pub quality_id: i32,
    pub title: String,
    pub weight: i32,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<f64>))]
    pub min_size: Option<OrderedF64>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<f64>))]
    pub preferred_size: Option<OrderedF64>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<f64>))]
    pub max_size: Option<OrderedF64>,
    /// Tanukarr's preset criteria: the source and resolution (a custom preset's own; empty
    /// for a standard quality, which uses its built-in ones).
    #[serde(default)]
    pub source: Option<crate::quality::QualitySource>,
    #[serde(default)]
    pub resolution: Option<i32>,
    /// A codec family releases must use to get this quality (empty: any).
    #[serde(default)]
    pub codec: String,
    /// A regular expression release titles must match (empty: any).
    #[serde(default)]
    pub title_pattern: String,
    /// Retired presets are kept for history but no longer used.
    #[serde(default)]
    pub is_retired: bool,
}

/// A codec family's factor across every profile (Tanukarr's global codec factors): the value
/// when all profiles agree, else "mixed".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CodecFactorSummary {
    pub codec: String,
    pub label: String,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<f64>))]
    pub factor: Option<OrderedF64>,
    pub is_mixed: bool,
    #[cfg_attr(feature = "openapi", schema(value_type = f64))]
    pub default_factor: OrderedF64,
}

/// Creates a custom quality preset (Tanukarr's `POST /qualitydefinition`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CustomQualityCreate {
    pub title: String,
    pub source: Option<crate::quality::QualitySource>,
    pub resolution: Option<i32>,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub title_pattern: String,
    /// Copies this quality's size limits (global and per profile).
    #[serde(default)]
    pub copy_from_id: Option<i32>,
}

/// `f64` wrapper with `Eq` so that DTOs containing sizes can still derive `Eq`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrderedF64(pub f64);
impl Eq for OrderedF64 {}

/// A custom format specification (Sonarr-compatible implementation names, e.g.
/// `ReleaseTitleSpecification`, `SourceSpecification`, `LanguageSpecification`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CustomFormatSpecification {
    pub name: String,
    pub implementation: String,
    pub negate: bool,
    pub required: bool,
    /// Implementation-specific fields, e.g. `{"value": "x265"}` or `{"min": 0, "max": 2}`.
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CustomFormat {
    pub id: i64,
    pub name: String,
    pub include_when_renaming: bool,
    pub specifications: Vec<CustomFormatSpecification>,
}

/// Kinds of configurable provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    Indexer,
    DownloadClient,
    Notification,
    ImportList,
}

/// An indexer, download client, notification or import list.
///
/// `settings` holds the implementation-specific configuration. Secret fields are replaced
/// by [`REDACTED`] in responses; sending [`REDACTED`] back keeps the stored secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ProviderItem {
    pub id: i64,
    pub kind: ProviderKind,
    pub name: String,
    /// Implementation identifier, e.g. `Torznab`, `QBittorrent`, `Webhook`.
    pub implementation: String,
    pub protocol: Option<DownloadProtocol>,
    pub enabled: bool,
    pub priority: i32,
    pub tags: Vec<i64>,
    /// Whether Kitsunarr has a native implementation for `implementation` yet.
    pub supported: bool,
    /// Kind-specific switches (`enableRss`, `onGrab`, `removeCompletedDownloads`, ...).
    pub options: serde_json::Map<String, serde_json::Value>,
    pub settings: serde_json::Map<String, serde_json::Value>,
}

pub const REDACTED: &str = "********";

/// Notification implementations Kitsunarr sends (Sonarr's names).
pub const NOTIFICATION_IMPLEMENTATIONS: &[&str] = &[
    "Webhook",
    "Notifiarr",
    "Discord",
    "Slack",
    "Telegram",
    "Gotify",
    "Ntfy",
    "Pushover",
    "PushBullet",
    "Join",
    "Prowl",
    "Simplepush",
    "Signal",
    "Apprise",
    "MediaBrowser",
    "Jellyfin",
    "PlexServer",
    "Xbmc",
    "CustomScript",
    "Email",
    "Mailgun",
    "SendGrid",
    "Pushcut",
    "SynologyIndexer",
    "Trakt",
];

/// Episode/folder naming configuration (Sonarr-compatible tokens).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct NamingSettings {
    pub rename_episodes: bool,
    pub replace_illegal_characters: bool,
    /// 0 delete, 1 dash, 2 space dash, 3 space dash space, 4 smart, 5 custom.
    pub colon_replacement_format: i32,
    pub custom_colon_replacement_format: String,
    /// 0 extend, 1 duplicate, 2 repeat, 3 scene, 4 range, 5 prefixed range.
    pub multi_episode_style: i32,
    pub standard_episode_format: String,
    pub daily_episode_format: String,
    pub anime_episode_format: String,
    pub series_folder_format: String,
    pub season_folder_format: String,
    pub specials_folder_format: String,
    pub movie_file_format: String,
    pub movie_folder_format: String,
    /// Tanukarr's group folders (titles added to an anime group or TMDB collection go to
    /// `{root}/{group folder}/{member folder}`).
    #[serde(default = "group_folder_default")]
    pub anime_group_folder_format: String,
    #[serde(default = "group_season_default")]
    pub anime_group_season_folder_format: String,
    #[serde(default = "group_arc_default")]
    pub anime_group_arc_folder_format: String,
    #[serde(default = "group_movie_default")]
    pub anime_group_movie_folder_format: String,
    #[serde(default = "group_saga_default")]
    pub anime_group_movie_saga_folder_format: String,
}

fn group_folder_default() -> String {
    "{Group Title}".into()
}
fn group_season_default() -> String {
    "Season {Group Season:00}".into()
}
fn group_arc_default() -> String {
    "{Group Arc Title}".into()
}
fn group_movie_default() -> String {
    "{Series Title}".into()
}
fn group_saga_default() -> String {
    "Film {Group Movie:00}".into()
}

impl Default for NamingSettings {
    fn default() -> Self {
        Self {
            rename_episodes: false,
            replace_illegal_characters: true,
            colon_replacement_format: 4,
            custom_colon_replacement_format: String::new(),
            // PrefixedRange, Sonarr's default.
            multi_episode_style: 5,
            standard_episode_format:
                "{Series Title} - S{season:00}E{episode:00} - {Episode Title} {Quality Full}".into(),
            daily_episode_format: "{Series Title} - {Air-Date} - {Episode Title} {Quality Full}"
                .into(),
            anime_episode_format:
                "{Series Title} - S{season:00}E{episode:00} - {Episode Title} {Quality Full}".into(),
            series_folder_format: "{Series Title}".into(),
            season_folder_format: "Season {season}".into(),
            specials_folder_format: "Specials".into(),
            movie_file_format: "{Movie Title} ({Release Year}) {Quality Full}".into(),
            movie_folder_format: "{Movie Title} ({Release Year})".into(),
            anime_group_folder_format: group_folder_default(),
            anime_group_season_folder_format: group_season_default(),
            anime_group_arc_folder_format: group_arc_default(),
            anime_group_movie_folder_format: group_movie_default(),
            anime_group_movie_saga_folder_format: group_saga_default(),
        }
    }
}

/// How propers and repacks are treated (Sonarr's `ProperDownloadTypes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ProperDownloadType {
    /// Prefer them, and upgrade existing files to them.
    #[default]
    PreferAndUpgrade,
    /// Prefer them in searches, but do not replace existing files.
    DoNotUpgrade,
    /// Treat them like any other release (custom formats decide).
    DoNotPrefer,
}

impl ProperDownloadType {
    /// Sonarr's names, in any case (`PreferAndUpgrade`, `preferAndUpgrade`).
    pub fn from_sonarr(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "preferandupgrade" => Some(Self::PreferAndUpgrade),
            "donotupgrade" => Some(Self::DoNotUpgrade),
            "donotprefer" => Some(Self::DoNotPrefer),
            _ => None,
        }
    }
}

/// Indexer options (Sonarr's Settings › Indexers › Options).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct IndexerOptionsSettings {
    /// Usenet releases younger than this many minutes are rejected (0: off).
    pub minimum_age: i32,
    /// Usenet releases older than this many days are rejected (0: unlimited).
    pub retention: i32,
    /// Releases larger than this many MB are rejected (0: unlimited).
    pub maximum_size: i32,
    /// Minutes between RSS syncs: 0 (off) or 10 to 120.
    pub rss_sync_interval: i32,
    /// Tanukarr: accept multi-season packs (a season search takes the pack for its season).
    pub enable_multi_season_releases: bool,
    /// Tanukarr: inspect torrents before sending them (executables, password bait, no video).
    pub enable_fake_release_protection: bool,
}

impl Default for IndexerOptionsSettings {
    fn default() -> Self {
        Self {
            minimum_age: 0,
            retention: 0,
            maximum_size: 0,
            rss_sync_interval: 15,
            enable_multi_season_releases: true,
            enable_fake_release_protection: true,
        }
    }
}

/// Metadata catalogue settings: the API keys TMDB and OMDb need (Tanukarr's `TmdbApiKey` and
/// `OmdbApiKey`). Keys are redacted in responses; sending the placeholder back keeps them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct MetadataSourcesSettings {
    pub tmdb_api_key: Option<String>,
    pub omdb_api_key: Option<String>,
}

/// Download client options (Sonarr's Settings › Download Clients: completed and failed
/// download handling).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct DownloadClientOptionsSettings {
    /// Import completed downloads Kitsunarr grabbed.
    pub enable_completed_download_handling: bool,
    /// Search again when a download fails or is blocklisted.
    pub auto_redownload_failed: bool,
    /// Also for releases grabbed from an interactive search.
    pub auto_redownload_failed_from_interactive_search: bool,
    /// Minutes without progress after which a downloading torrent fails (0: off, at most
    /// 43200). Tanukarr's stalled torrent recovery.
    pub stalled_torrent_timeout: i32,
}

impl Default for DownloadClientOptionsSettings {
    fn default() -> Self {
        Self {
            enable_completed_download_handling: true,
            auto_redownload_failed: true,
            auto_redownload_failed_from_interactive_search: true,
            stalled_torrent_timeout: 0,
        }
    }
}

/// Sonarr's delay profile: how long to wait for a better release per protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DelayProfile {
    #[serde(default)]
    pub id: i64,
    pub enable_usenet: bool,
    pub enable_torrent: bool,
    /// `usenet` or `torrent`.
    pub preferred_protocol: String,
    /// Minutes.
    pub usenet_delay: i64,
    /// Minutes.
    pub torrent_delay: i64,
    pub bypass_if_highest_quality: bool,
    pub bypass_if_above_custom_format_score: bool,
    pub minimum_custom_format_score: i64,
    pub sort_order: i64,
    pub tags: Vec<i64>,
}

/// Sonarr's release profile: terms a release must or must not contain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfile {
    #[serde(default)]
    pub id: i64,
    pub name: Option<String>,
    pub enabled: bool,
    pub required: Vec<String>,
    pub ignored: Vec<String>,
    /// Empty = applies to every indexer.
    pub indexer_ids: Vec<i64>,
    pub tags: Vec<i64>,
}

/// Sonarr's remote path mapping: a download client's path as Kitsunarr sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RemotePathMapping {
    #[serde(default)]
    pub id: i64,
    pub host: String,
    pub remote_path: String,
    pub local_path: String,
}

/// A value a provider setting can take, read from the service (Jellyfin libraries and users).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ProviderChoice {
    pub value: String,
    pub name: String,
    pub hint: Option<String>,
}

/// Tanukarr's Library tools switches (`ProOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct LibraryToolsSettings {
    /// Pause RSS sync, scheduled list syncs and the searches Kitsunarr starts by itself; manual
    /// actions and current downloads continue.
    pub automation_paused: bool,
    /// Downloads that keep seeding are hard-linked into the library, never copied.
    pub hardlink_only: bool,
    /// Reject releases whose TheTVDB or IMDb id names another series.
    pub match_external_ids: bool,
    /// In automatic anime season searches, skip episode searches an accepted season pack covers.
    pub prefer_anime_season_packs: bool,
}

/// One search, for the "Recent searches" diagnostics (Tanukarr's `SearchActivity`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SearchActivity {
    pub date: String,
    pub series_id: i64,
    pub query: String,
    /// Interactive or automatic.
    pub automatic: bool,
    pub found: i64,
    pub accepted: i64,
    pub indexers: i64,
    /// The most frequent rejection messages, with their counts.
    pub rejections: Vec<RejectionCount>,
    pub provider_failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RejectionCount {
    pub message: String,
    pub count: i64,
}

/// What a list sync does with library titles no list contains any more (Sonarr's
/// `ListSyncLevelType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ListSyncLevel {
    #[default]
    Disabled,
    LogOnly,
    KeepAndUnmonitor,
    KeepAndTag,
}

/// Import list options (Sonarr's Settings › Import Lists: "Clean library level").
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ImportListOptionsSettings {
    pub list_sync_level: ListSyncLevel,
    /// The tag `keepAndTag` adds.
    pub list_sync_tag: Option<i64>,
}

/// File handling settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MediaManagementSettings {
    pub use_hardlinks: bool,
    pub recycle_bin: Option<String>,
    pub recycle_bin_cleanup_days: i32,
    pub delete_empty_folders: bool,
    pub create_empty_series_folders: bool,
    pub minimum_free_space_mb: i64,
    pub set_permissions: bool,
    pub chmod_folder: String,
    pub skip_free_space_check: bool,
    pub import_extra_files: bool,
    pub extra_file_extensions: String,
    #[serde(default)]
    pub download_propers_and_repacks: ProperDownloadType,
    /// Tanukarr: files whose episodes are all unmonitored are not imported from downloads.
    #[serde(default)]
    pub skip_unmonitored_episodes_from_packs: bool,
    /// Sonarr's "Change file date" (plus Tanukarr's "preserve original" for upgrades).
    #[serde(default)]
    pub file_date: FileDate,
    /// Sonarr's "Analyze video files" (`EnableMediaInfo`): disk scans read resolution, runtime
    /// and codecs from files already in the library. New downloads are always read.
    #[serde(default = "default_true")]
    pub enable_media_info: bool,
}

fn default_true() -> bool {
    true
}

/// Sonarr's `FileDateType` and Tanukarr's `PreserveOriginal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum FileDate {
    #[default]
    None,
    /// The episode's air date at the series' air time, in local time.
    LocalAirDate,
    /// The episode's UTC air date and time.
    UtcAirDate,
    /// On an upgrade, the replaced file's date.
    PreserveOriginal,
}

impl FileDate {
    /// Sonarr's numbers (0-2) and names; Tanukarr's 3 is "preserve original".
    pub fn from_sonarr(v: &str) -> Option<Self> {
        Some(match v.trim().to_ascii_lowercase().as_str() {
            "0" | "none" => Self::None,
            "1" | "localairdate" => Self::LocalAirDate,
            "2" | "utcairdate" => Self::UtcAirDate,
            "3" | "preserveoriginal" => Self::PreserveOriginal,
            _ => return None,
        })
    }
}

impl Default for MediaManagementSettings {
    fn default() -> Self {
        Self {
            use_hardlinks: true,
            recycle_bin: None,
            recycle_bin_cleanup_days: 7,
            delete_empty_folders: false,
            create_empty_series_folders: false,
            minimum_free_space_mb: 100,
            set_permissions: false,
            chmod_folder: "755".into(),
            skip_free_space_check: false,
            import_extra_files: false,
            extra_file_extensions: "srt".into(),
            download_propers_and_repacks: ProperDownloadType::PreferAndUpgrade,
            skip_unmonitored_episodes_from_packs: false,
            file_date: FileDate::None,
            enable_media_info: true,
        }
    }
}

/// General instance settings editable from the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettings {
    pub instance_name: String,
    /// Default library section for new root folders and the dashboard.
    pub default_media_kind: crate::media::MediaKind,
    pub ui_language: String,
    pub first_day_of_week: u8,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            instance_name: "Kitsunarr".into(),
            default_media_kind: crate::media::MediaKind::Series,
            ui_language: "en".into(),
            first_day_of_week: 1,
        }
    }
}

/// The proxy protocol (Sonarr's `ProxyType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ProxyType {
    #[default]
    Http,
    Socks4,
    Socks5,
}

impl ProxyType {
    /// Sonarr's names and numbers (`Http`/0, `Socks4`/1, `Socks5`/2), any case.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "http" | "0" => Some(Self::Http),
            "socks4" | "1" => Some(Self::Socks4),
            "socks5" | "2" => Some(Self::Socks5),
            _ => None,
        }
    }
}

/// The proxy for outgoing requests (Sonarr's Settings › General › Proxy). The password is
/// redacted in responses; sending the placeholder back keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ProxySettings {
    pub enabled: bool,
    pub proxy_type: ProxyType,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Comma-separated hosts (`*` wildcards, e.g. `*.lan`) and IP addresses or ranges
    /// (`192.168.1.0/24`, `10.0.0.1-10.0.0.9`) reached directly.
    pub bypass_filter: String,
    /// Loopback addresses and single-label host names are reached directly.
    pub bypass_local_addresses: bool,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            proxy_type: ProxyType::Http,
            hostname: String::new(),
            port: 8080,
            username: String::new(),
            password: String::new(),
            bypass_filter: String::new(),
            bypass_local_addresses: true,
        }
    }
}
