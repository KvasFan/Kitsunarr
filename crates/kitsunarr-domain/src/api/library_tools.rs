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
