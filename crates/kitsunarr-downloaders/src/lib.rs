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

/// Whether Kitsunarr can talk to a download client implementation.
pub fn supported(implementation: &str) -> bool {
    implementation == "QBittorrent"
}
