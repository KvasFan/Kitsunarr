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
    /// The library film, for a film's download (Tanukarr's movies).
    #[serde(default)]
    pub movie_id: Option<i64>,
    #[serde(default)]
    pub movie_title: Option<String>,
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
    /// What Kitsunarr does with it (Sonarr's tracked state): `downloading`, `importPending`,
    /// `importing` (files being copied or moved), `importBlocked`, `imported`, `failed`,
    /// `pending` (held by a delay profile) or `unknown` (not grabbed by Kitsunarr).
    pub tracked_state: String,
    pub messages: Vec<String>,
    /// Local path of the finished files (after remote path mappings).
    pub output_path: Option<String>,
    /// When it was grabbed (RFC 3339), for downloads Kitsunarr grabbed.
    pub added: Option<String>,
    /// Tanukarr's per-file import progress (`ImportProgressResource`), once an import of the
    /// download has started; kept until the download leaves the queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_progress: Option<ImportProgress>,
    /// The download client has a post-import category, so the item can be moved there
    /// instead of removed (Sonarr's `downloadClientHasPostImportCategory`).
    #[serde(default)]
    pub download_client_has_post_import_category: bool,
}

/// Tanukarr's `ImportProgressResource`: how many of a download's expected episodes are
/// imported, being imported, waiting, failed or rejected, and each file's state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub total_episodes: i32,
    pub imported_episodes: i32,
    pub importing_episodes: i32,
    pub waiting_episodes: i32,
    pub failed_episodes: i32,
    pub rejected_episodes: i32,
    pub items: Vec<ImportProgressItem>,
}

/// Tanukarr's `ImportProgressItemResource`: one file of the download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ImportProgressItem {
    pub id: String,
    pub episode_ids: Vec<i64>,
    pub season_number: Option<i32>,
    pub episode_numbers: Vec<i32>,
    pub source_path: String,
    pub size: i64,
    pub state: ImportProgressState,
    /// RFC 3339.
    pub started: Option<String>,
    /// RFC 3339.
    pub completed: Option<String>,
    pub errors: Vec<String>,
}

/// Tanukarr's `ImportProgressState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ImportProgressState {
    Waiting,
    Importing,
    Imported,
    Rejected,
    Failed,
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
    /// 0 for a film's entry.
    pub series_id: i64,
    /// A film's entry (Tanukarr's movies); `series_title` then holds the film's title.
    #[serde(default)]
    pub movie_id: Option<i64>,
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
    /// Instead of removing it, move the download to the client's post-import category
    /// (Sonarr's `changeCategory`); ignored with `removeFromClient`.
    pub change_category: bool,
}

/// Several queue items removed with the same options (Sonarr's `DELETE /queue/bulk`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct QueueBulkRemoval {
    pub download_ids: Vec<String>,
    #[serde(flatten)]
    pub removal: QueueRemoval,
}

/// Several pending releases sent now (Sonarr's queue `grab/bulk`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct QueueBulkGrab {
    pub download_ids: Vec<String>,
}

/// What a bulk queue action did: one message per item done, one per failure.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QueueBulkResult {
    pub done: Vec<String>,
    pub failures: Vec<String>,
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
