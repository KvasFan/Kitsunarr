//! Tanukarr's Library tools: library check, recycle bin, storage outlook.

use serde::{Deserialize, Serialize};

/// One problem the library check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LibraryIssue {
    pub series_id: i64,
    pub series_title: String,
    pub path: String,
    /// `Series folder missing`, `Outside the series folder`, `Duplicate file record`,
    /// `Missing on disk`.
    pub problem: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LibraryCheck {
    pub checked_at: String,
    pub series_checked: i64,
    pub files_checked: i64,
    pub issues: Vec<LibraryIssue>,
}

/// A file in the recycle bin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RecycledFile {
    /// Opaque: base64 of the path relative to the recycle bin.
    pub token: String,
    pub name: String,
    pub relative_path: String,
    pub size: u64,
    pub recycled_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RecycleBin {
    pub configured: bool,
    pub path: Option<String>,
    pub truncated: bool,
    pub files: Vec<RecycledFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RecycleRestoreRequest {
    pub token: String,
    pub series_id: i64,
    pub season_number: i32,
}

/// Whether a root folder can take the downloads in progress (Tanukarr's storage outlook).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct StorageOutlook {
    pub root_folder: String,
    /// `None` when the free space cannot be read.
    pub free_space: Option<u64>,
    pub queued_downloads: i64,
    /// Bytes still to download.
    pub download_remaining: u64,
    /// Space the imports will take: the downloads' total size (0 with hardlink-only imports).
    pub import_space_estimate: u64,
    pub may_run_out_of_space: bool,
    pub import_mode: String,
}

/// Tanukarr's per-series naming options.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct SeriesNamingOptions {
    /// `default` (follow "Rename episodes"), `manual` (never rename on import) or `automatic`
    /// (always rename on import).
    pub renaming_mode: String,
    /// Season folder titles replacing the catalogue's (`{Season Title}`), by season number.
    pub season_titles: std::collections::BTreeMap<i32, String>,
}

/// A companion app key (the token itself is shown once, at creation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CompanionKeyItem {
    pub id: String,
    pub name: String,
    /// `read-library` or `add-series`.
    pub permission: String,
    pub created: String,
    pub expires: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CompanionKeyCreate {
    pub name: String,
    pub permission: String,
    /// Validity in days, 1 to 365.
    pub days: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CompanionKeyCreated {
    pub key: CompanionKeyItem,
    /// The token to give the companion app; it cannot be shown again.
    pub token: String,
}
