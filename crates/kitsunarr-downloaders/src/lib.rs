//! Download clients for Kitsunarr, ported from Sonarr's `Download/Clients`.
//!
//! Implemented: qBittorrent (Web API v2), Deluge (Web UI JSON-RPC) and Transmission (RPC). [`Clients`] dispatches to
//! the client a provider's implementation names. The torrent helpers compute the info hash
//! Sonarr tracks downloads by.

pub mod deluge;
pub mod error;
pub mod qbittorrent;
pub mod torrent;
pub mod transmission;

use serde_json::{Map, Value};

pub use deluge::Deluge;
pub use error::DownloadError;
pub use qbittorrent::{AddRequest, QBittorrent, Source};
pub use torrent::{magnet_info_hash, torrent_info_hash};
pub use transmission::Transmission;

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
    matches!(implementation, "QBittorrent" | "Deluge" | "Transmission")
}

/// A download client's settings, by implementation.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientSettings {
    QBittorrent(qbittorrent::Settings),
    Deluge(deluge::Settings),
    Transmission(transmission::Settings),
}

impl ClientSettings {
    /// From a provider's implementation and `settings` map.
    pub fn from_provider(implementation: &str, map: &Map<String, Value>) -> Result<Self, String> {
        match implementation {
            "QBittorrent" => qbittorrent::Settings::from_map(map).map(Self::QBittorrent),
            "Deluge" => deluge::Settings::from_map(map).map(Self::Deluge),
            "Transmission" => transmission::Settings::from_map(map).map(Self::Transmission),
            other => Err(format!("{other} download clients are not supported yet")),
        }
    }

    /// The host, for remote path mappings.
    pub fn host(&self) -> &str {
        match self {
            Self::QBittorrent(s) => &s.host,
            Self::Deluge(s) => &s.host,
            Self::Transmission(s) => &s.host,
        }
    }

    /// Whether the series' tags are added to the torrent (qBittorrent's `addSeriesTags`).
    pub fn add_series_tags(&self) -> bool {
        match self {
            Self::QBittorrent(s) => s.add_series_tags,
            Self::Deluge(_) | Self::Transmission(_) => false,
        }
    }
}

/// A successful test: what to tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tested {
    pub message: String,
    pub warnings: Vec<String>,
}

/// Every client Kitsunarr implements, sharing one HTTP client and their login sessions.
#[derive(Clone, Default)]
pub struct Clients {
    pub qbittorrent: QBittorrent,
    pub deluge: Deluge,
    pub transmission: Transmission,
}

impl Clients {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            qbittorrent: QBittorrent::new(http.clone()),
            deluge: Deluge::new(http.clone()),
            transmission: Transmission::new(http),
        }
    }

    pub async fn test(&self, s: &ClientSettings) -> Result<Tested, DownloadError> {
        Ok(match s {
            ClientSettings::QBittorrent(s) => {
                let r = self.qbittorrent.test(s).await?;
                Tested {
                    message: format!("Connected to qBittorrent (Web API {}).", r.api_version),
                    warnings: r.warnings,
                }
            }
            ClientSettings::Deluge(s) => {
                let r = self.deluge.test(s).await?;
                Tested {
                    message: format!("Connected to Deluge {}.", r.version),
                    warnings: r.warnings,
                }
            }
            ClientSettings::Transmission(s) => {
                let r = self.transmission.test(s).await?;
                Tested {
                    message: format!("Connected to Transmission {}.", r.version),
                    warnings: r.warnings,
                }
            }
        })
    }

    /// Adds a torrent; returns the client's id for it (the info hash).
    pub async fn add(
        &self,
        s: &ClientSettings,
        req: &AddRequest<'_>,
    ) -> Result<String, DownloadError> {
        match s {
            ClientSettings::QBittorrent(s) => self.qbittorrent.add(s, req).await,
            ClientSettings::Deluge(s) => self.deluge.add(s, &req.source, req.recent).await,
            ClientSettings::Transmission(s) => {
                self.transmission
                    .add(s, &req.source, req.hash, req.recent)
                    .await
            }
        }
    }

    pub async fn items(&self, s: &ClientSettings) -> Result<Vec<DownloadItem>, DownloadError> {
        match s {
            ClientSettings::QBittorrent(s) => self.qbittorrent.items(s).await,
            ClientSettings::Deluge(s) => self.deluge.items(s).await,
            ClientSettings::Transmission(s) => self.transmission.items(s).await,
        }
    }

    pub async fn mark_imported(&self, s: &ClientSettings, id: &str) -> Result<(), DownloadError> {
        match s {
            ClientSettings::QBittorrent(s) => self.qbittorrent.mark_imported(s, id).await,
            ClientSettings::Deluge(s) => self.deluge.mark_imported(s, id).await,
            ClientSettings::Transmission(s) => self.transmission.mark_imported(s, id).await,
        }
    }
}
