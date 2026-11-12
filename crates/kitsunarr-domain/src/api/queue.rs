//! The download queue (Sonarr's `QueueResource`).

use serde::{Deserialize, Serialize};

use crate::media::DownloadProtocol;

/// One download in a download client, with what Kitsunarr knows about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub download_id: String,
    pub title: String,
    pub download_client: String,
    pub protocol: DownloadProtocol,
    /// The library series, when Kitsunarr grabbed the download.
    pub series_id: Option<i64>,
    pub series_title: Option<String>,
    /// Episodes as `S01E05`.
    pub episodes: Vec<String>,
    pub quality: Option<String>,
    pub indexer: Option<String>,
    pub size: i64,
    pub size_left: i64,
    /// Seconds; `None` when unknown.
    pub time_left: Option<i64>,
    /// `queued`, `paused`, `downloading`, `completed`, `failed` or `warning`.
    pub status: String,
    /// What Kitsunarr does with it next: `downloading`, `importPending`, `unknown`.
    pub tracked_state: String,
    pub messages: Vec<String>,
    /// Local path of the finished files (after remote path mappings).
    pub output_path: Option<String>,
    /// When it was grabbed (RFC 3339), for downloads Kitsunarr grabbed.
    pub added: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QueueClientFailure {
    pub download_client: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QueueResult {
    pub items: Vec<QueueItem>,
    pub failures: Vec<QueueClientFailure>,
    /// Enabled download clients Kitsunarr can read.
    pub clients: usize,
}

/// A blocklisted release (Sonarr's `BlocklistResource`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BlocklistItem {
    pub id: i64,
    pub series_id: i64,
    pub series_title: Option<String>,
    pub episode_ids: Vec<i64>,
    pub source_title: String,
    pub quality: Option<String>,
    /// When it was blocklisted (RFC 3339).
    pub date: String,
    pub protocol: DownloadProtocol,
    pub indexer: Option<String>,
    pub message: Option<String>,
    pub source: Option<String>,
}

/// `DELETE /queue/{downloadId}` options (Sonarr's `QueueController.Remove`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema, utoipa::IntoParams))]
#[serde(rename_all = "camelCase", default)]
pub struct QueueRemoval {
    /// Remove the download (and its data) from the download client.
    pub remove_from_client: bool,
    /// Blocklist the release (marks the download as failed).
    pub blocklist: bool,
    /// With `blocklist`: do not search for a replacement.
    pub skip_redownload: bool,
}

/// What removing a queue item did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QueueRemovalResult {
    pub message: String,
}

/// Releases to take off the blocklist.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct BlocklistRemoval {
    pub ids: Vec<i64>,
    /// Clear the whole blocklist (`ids` is ignored).
    pub all: bool,
}
