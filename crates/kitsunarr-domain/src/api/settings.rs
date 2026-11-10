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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Default for IndexerOptionsSettings {
    fn default() -> Self {
        Self {
            minimum_age: 0,
            retention: 0,
            maximum_size: 0,
            rss_sync_interval: 15,
        }
    }
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
