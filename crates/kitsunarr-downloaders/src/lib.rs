//! Download clients for Kitsunarr, ported from Sonarr's `Download/Clients`.
//!
//! Implemented: qBittorrent (Web API v2), Deluge (Web UI JSON-RPC), Transmission (RPC), SABnzbd
//! (HTTP API) and NZBGet (JSON-RPC). [`Clients`] dispatches to the client a provider's
//! implementation names. The torrent helpers compute the info hash
//! Sonarr tracks downloads by.

pub mod blackhole;
pub mod deluge;
pub mod error;
pub mod fake;
pub mod nzbget;
pub mod qbittorrent;
pub mod sabnzbd;
pub mod torrent;
pub mod transmission;
pub mod usenet;

use serde_json::{Map, Value};

pub use blackhole::Blackhole;
pub use deluge::Deluge;
pub use error::DownloadError;
pub use nzbget::Nzbget;
pub use qbittorrent::{AddRequest, QBittorrent, SeedConfig, Source};
pub use sabnzbd::Sabnzbd;
pub use torrent::{magnet_info_hash, torrent_info_hash};
pub use transmission::Transmission;
pub use usenet::validate_nzb;

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
    /// The client is done with it (Usenet: always; torrents: seeding stopped at the seed
    /// limit), so it may be removed after import (`CanBeRemoved`).
    pub can_be_removed: bool,
}

/// Whether Kitsunarr can talk to a download client implementation.
pub fn supported(implementation: &str) -> bool {
    kitsunarr_domain::api::settings::DOWNLOAD_CLIENT_IMPLEMENTATIONS.contains(&implementation)
}

/// A download client's settings, by implementation.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientSettings {
    QBittorrent(qbittorrent::Settings),
    Deluge(deluge::Settings),
    Transmission(transmission::Settings),
    Sabnzbd(sabnzbd::Settings),
    Nzbget(nzbget::Settings),
    TorrentBlackhole(blackhole::TorrentSettings),
    UsenetBlackhole(blackhole::UsenetSettings),
}

impl ClientSettings {
    /// From a provider's implementation and `settings` map.
    pub fn from_provider(implementation: &str, map: &Map<String, Value>) -> Result<Self, String> {
        match implementation {
            "QBittorrent" => qbittorrent::Settings::from_map(map).map(Self::QBittorrent),
            "Deluge" => deluge::Settings::from_map(map).map(Self::Deluge),
            "Transmission" => transmission::Settings::from_map(map).map(Self::Transmission),
            "Vuze" => transmission::Settings::from_map(map)
                .map(|s| Self::Transmission(transmission::Settings { vuze: true, ..s })),
            "Sabnzbd" => sabnzbd::Settings::from_map(map).map(Self::Sabnzbd),
            "Nzbget" => nzbget::Settings::from_map(map).map(Self::Nzbget),
            "TorrentBlackhole" => {
                blackhole::TorrentSettings::from_map(map).map(Self::TorrentBlackhole)
            }
            "UsenetBlackhole" => {
                blackhole::UsenetSettings::from_map(map).map(Self::UsenetBlackhole)
            }
            other => Err(format!("{other} download clients are not supported yet")),
        }
    }

    /// The host, for remote path mappings.
    pub fn host(&self) -> &str {
        match self {
            Self::QBittorrent(s) => &s.host,
            Self::Deluge(s) => &s.host,
            Self::Transmission(s) => &s.host,
            Self::Sabnzbd(s) => &s.host,
            Self::Nzbget(s) => &s.host,
            // Blackholes are local folders (`IsLocalhost`).
            Self::TorrentBlackhole(_) | Self::UsenetBlackhole(_) => "localhost",
        }
    }

    /// Whether the client downloads from Usenet (its files may be moved on import).
    pub fn is_usenet(&self) -> bool {
        matches!(
            self,
            Self::Sabnzbd(_) | Self::Nzbget(_) | Self::UsenetBlackhole(_)
        )
    }

    /// `CanMoveFiles`: Usenet downloads, and a Torrent Blackhole's when it is not read-only.
    pub fn can_move_files(&self) -> bool {
        match self {
            Self::TorrentBlackhole(s) => !s.read_only,
            other => other.is_usenet(),
        }
    }

    /// `PreferTorrentFile`: the torrent file is sent before the magnet link.
    pub fn prefers_torrent_file(&self) -> bool {
        matches!(self, Self::TorrentBlackhole(_))
    }

    /// Whether the series' tags are added to the torrent (qBittorrent's `addSeriesTags`).
    pub fn add_series_tags(&self) -> bool {
        match self {
            Self::QBittorrent(s) => s.add_series_tags,
            _ => false,
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
    pub sabnzbd: Sabnzbd,
    pub nzbget: Nzbget,
    pub blackhole: Blackhole,
}

impl Clients {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            qbittorrent: QBittorrent::new(http.clone()),
            deluge: Deluge::new(http.clone()),
            transmission: Transmission::new(http.clone()),
            sabnzbd: Sabnzbd::new(http.clone()),
            nzbget: Nzbget::new(http),
            blackhole: Blackhole::default(),
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
                    message: format!(
                        "Connected to {} {}.",
                        if s.vuze { "Vuze" } else { "Transmission" },
                        r.version
                    ),
                    warnings: r.warnings,
                }
            }
            ClientSettings::Sabnzbd(s) => {
                let r = self.sabnzbd.test(s).await?;
                Tested {
                    message: format!("Connected to SABnzbd {}.", r.version),
                    warnings: r.warnings,
                }
            }
            ClientSettings::Nzbget(s) => {
                let r = self.nzbget.test(s).await?;
                Tested {
                    message: format!("Connected to NZBGet {}.", r.version),
                    warnings: r.warnings,
                }
            }
            ClientSettings::TorrentBlackhole(s) => {
                Blackhole::test_folder(&s.torrent_folder, "torrent folder")?;
                Blackhole::test_folder(&s.watch_folder, "watch folder")?;
                Tested {
                    message: "Both folders can be written to.".into(),
                    warnings: Vec::new(),
                }
            }
            ClientSettings::UsenetBlackhole(s) => {
                Blackhole::test_folder(&s.nzb_folder, "NZB folder")?;
                Blackhole::test_folder(&s.watch_folder, "watch folder")?;
                Tested {
                    message: "Both folders can be written to.".into(),
                    warnings: Vec::new(),
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
            ClientSettings::Deluge(s) => {
                self.deluge.add(s, &req.source, req.recent, req.seed).await
            }
            ClientSettings::Transmission(s) => {
                self.transmission
                    .add(s, &req.source, req.hash, req.recent, req.seed)
                    .await
            }
            ClientSettings::TorrentBlackhole(s) => {
                self.blackhole.add_torrent(s, req.title, &req.source)
            }
            ClientSettings::Sabnzbd(_)
            | ClientSettings::Nzbget(_)
            | ClientSettings::UsenetBlackhole(_) => Err(DownloadError::Unsupported(
                "Usenet clients cannot take torrents".into(),
            )),
        }
    }

    /// Adds an NZB (`UsenetClientBase.Download` after the file is fetched and validated);
    /// returns the client's id for it.
    pub async fn add_nzb(
        &self,
        s: &ClientSettings,
        name: &str,
        content: &[u8],
        recent: bool,
    ) -> Result<String, DownloadError> {
        validate_nzb(name, content)?;
        match s {
            ClientSettings::Sabnzbd(s) => self.sabnzbd.add(s, name, content, recent).await,
            ClientSettings::Nzbget(s) => self.nzbget.add(s, name, content, recent).await,
            ClientSettings::UsenetBlackhole(s) => self.blackhole.add_nzb(s, name, content),
            _ => Err(DownloadError::Unsupported(
                "torrent clients cannot take NZBs".into(),
            )),
        }
    }

    /// Removes a download from its client (`RemoveItem`), deleting its data when asked.
    pub async fn remove(
        &self,
        s: &ClientSettings,
        item: &DownloadItem,
        delete_data: bool,
    ) -> Result<(), DownloadError> {
        match s {
            ClientSettings::QBittorrent(s) => {
                self.qbittorrent
                    .remove(s, &item.download_id, delete_data)
                    .await
            }
            ClientSettings::Deluge(s) => {
                self.deluge.remove(s, &item.download_id, delete_data).await
            }
            ClientSettings::Transmission(s) => {
                self.transmission
                    .remove(s, &item.download_id, delete_data)
                    .await
            }
            ClientSettings::Sabnzbd(s) => {
                self.sabnzbd
                    .remove(
                        s,
                        &item.download_id,
                        delete_data,
                        item.status == ItemStatus::Failed,
                    )
                    .await
            }
            ClientSettings::Nzbget(s) => self.nzbget.remove(s, &item.download_id).await,
            ClientSettings::TorrentBlackhole(_) | ClientSettings::UsenetBlackhole(_) => {
                self.blackhole.remove(item, delete_data)
            }
        }
    }

    pub async fn items(&self, s: &ClientSettings) -> Result<Vec<DownloadItem>, DownloadError> {
        match s {
            ClientSettings::QBittorrent(s) => self.qbittorrent.items(s).await,
            ClientSettings::Deluge(s) => self.deluge.items(s).await,
            ClientSettings::Transmission(s) => self.transmission.items(s).await,
            ClientSettings::Sabnzbd(s) => self.sabnzbd.items(s).await,
            ClientSettings::Nzbget(s) => self.nzbget.items(s).await,
            ClientSettings::TorrentBlackhole(s) => {
                Ok(self
                    .blackhole
                    .items("TorrentBlackhole", &s.watch_folder, s.grace))
            }
            ClientSettings::UsenetBlackhole(s) => {
                Ok(self
                    .blackhole
                    .items("UsenetBlackhole", &s.watch_folder, s.grace))
            }
        }
    }

    pub async fn mark_imported(&self, s: &ClientSettings, id: &str) -> Result<(), DownloadError> {
        match s {
            ClientSettings::QBittorrent(s) => self.qbittorrent.mark_imported(s, id).await,
            ClientSettings::Deluge(s) => self.deluge.mark_imported(s, id).await,
            ClientSettings::Transmission(s) => self.transmission.mark_imported(s, id).await,
            // Usenet clients and blackholes have no post-import category.
            ClientSettings::Sabnzbd(_)
            | ClientSettings::Nzbget(_)
            | ClientSettings::TorrentBlackhole(_)
            | ClientSettings::UsenetBlackhole(_) => Ok(()),
        }
    }
}
