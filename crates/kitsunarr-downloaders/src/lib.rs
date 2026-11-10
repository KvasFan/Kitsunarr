//! Download clients for Kitsunarr, ported from Sonarr's `Download/Clients`.
//!
//! Implemented: qBittorrent (Web API v2). The torrent helpers compute the info hash Sonarr
//! tracks downloads by.

pub mod error;
pub mod qbittorrent;
pub mod torrent;

pub use error::DownloadError;
pub use qbittorrent::QBittorrent;
pub use torrent::{magnet_info_hash, torrent_info_hash};

/// A download's state in its client (`DownloadItemStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    Queued,
    Paused,
    Downloading,
    Completed,
    Failed,
    Warning,
}

/// A download as its client reports it (`DownloadClientItem`).
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadItem {
    /// Upper-case, as upstream (torrents: the info hash).
    pub download_id: String,
    pub title: String,
    pub category: String,
    pub total_size: i64,
    pub remaining_size: i64,
    /// `None` when unknown.
    pub remaining_seconds: Option<i64>,
    pub status: ItemStatus,
    pub message: Option<String>,
    /// Where the finished files are, as the client sees the path.
    pub output_path: Option<String>,
    pub seed_ratio: f64,
}

/// Whether Kitsunarr can talk to a download client implementation.
pub fn supported(implementation: &str) -> bool {
    implementation == "QBittorrent"
}
