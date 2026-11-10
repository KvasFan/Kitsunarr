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
