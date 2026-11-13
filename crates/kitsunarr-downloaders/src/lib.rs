//! Download clients for Kitsunarr, ported from Sonarr's `Download/Clients`.
//!
//! Implemented: qBittorrent (Web API v2), Deluge (Web UI JSON-RPC), Transmission (RPC), SABnzbd
//! (HTTP API) and NZBGet (JSON-RPC). [`Clients`] dispatches to the client a provider's
//! implementation names. The torrent helpers compute the info hash
//! Sonarr tracks downloads by.

pub mod aria2;
pub mod blackhole;
pub mod deluge;
pub mod error;
pub mod fake;
pub mod flood;
pub mod hadouken;
pub mod nzbget;
pub mod pneumatic;
pub mod qbittorrent;
pub mod rtorrent;
pub mod sabnzbd;
pub mod torrent;
pub mod transmission;
pub mod usenet;
pub mod utorrent;
pub mod xmlrpc;

use serde_json::{Map, Value};

pub use aria2::Aria2;
pub use blackhole::Blackhole;
pub use deluge::Deluge;
pub use error::DownloadError;
pub use flood::Flood;
pub use hadouken::Hadouken;
pub use nzbget::Nzbget;
pub use qbittorrent::{AddRequest, QBittorrent, SeedConfig, Source};
pub use rtorrent::RTorrent;
pub use sabnzbd::Sabnzbd;
pub use torrent::{magnet_info_hash, torrent_info_hash};
pub use transmission::Transmission;
pub use usenet::validate_nzb;
pub use utorrent::UTorrent;

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
    /// Set by clients that leave seed goals to Kitsunarr (rTorrent, Flood, RQBit): the
    /// finished torrent's ratio and when it finished, checked against the grabbing indexer's
    /// seed criteria (`DownloadSeedConfigProvider`).
    pub seed_progress: Option<SeedProgress>,
}

/// The release and its title, for clients that tag downloads with them (Flood's additional
/// tags).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReleaseInfo {
    pub title_slug: String,
    pub quality: String,
    pub languages: Vec<String>,
    pub release_group: String,
    pub year: String,
    pub indexer: String,
    pub network: String,
}

/// A finished torrent's seeding, for clients that do not enforce seed goals themselves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeedProgress {
    pub ratio: f64,
    /// Unix seconds.
    pub finished_at: Option<i64>,
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
    RTorrent(rtorrent::Settings),
    Flood(flood::Settings),
    UTorrent(utorrent::Settings),
    Aria2(aria2::Settings),
    Hadouken(hadouken::Settings),
    Pneumatic(pneumatic::Settings),
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
            "RTorrent" => rtorrent::Settings::from_map(map).map(Self::RTorrent),
            "Flood" => flood::Settings::from_map(map).map(Self::Flood),
            "UTorrent" => utorrent::Settings::from_map(map).map(Self::UTorrent),
            "Aria2" => aria2::Settings::from_map(map).map(Self::Aria2),
            "Hadouken" => hadouken::Settings::from_map(map).map(Self::Hadouken),
            "Pneumatic" => pneumatic::Settings::from_map(map).map(Self::Pneumatic),
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
            Self::RTorrent(s) => &s.host,
            Self::Flood(s) => &s.host,
            Self::UTorrent(s) => &s.host,
            Self::Aria2(s) => &s.host,
            Self::Hadouken(s) => &s.host,
            // Blackholes and Pneumatic are local folders (`IsLocalhost`).
            Self::TorrentBlackhole(_) | Self::UsenetBlackhole(_) | Self::Pneumatic(_) => {
                "localhost"
            }
        }
    }

    /// Whether the client downloads from Usenet (its files may be moved on import).
    pub fn is_usenet(&self) -> bool {
        matches!(
            self,
            Self::Sabnzbd(_) | Self::Nzbget(_) | Self::UsenetBlackhole(_) | Self::Pneumatic(_)
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
    pub rtorrent: RTorrent,
    pub flood: Flood,
    pub utorrent: UTorrent,
    pub aria2: Aria2,
    pub hadouken: Hadouken,
}

impl Clients {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            qbittorrent: QBittorrent::new(http.clone()),
            deluge: Deluge::new(http.clone()),
            transmission: Transmission::new(http.clone()),
            sabnzbd: Sabnzbd::new(http.clone()),
            nzbget: Nzbget::new(http.clone()),
            blackhole: Blackhole::default(),
            rtorrent: RTorrent::new(http.clone()),
            flood: Flood::new(http.clone()),
            utorrent: UTorrent::new(http.clone()),
            aria2: Aria2::new(http.clone()),
            hadouken: Hadouken::new(http),
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
            ClientSettings::Pneumatic(s) => {
                Blackhole::test_folder(&s.nzb_folder, "NZB folder")?;
                Blackhole::test_folder(&s.strm_folder, "STRM folder")?;
                Tested {
                    message: "Both folders can be written to.".into(),
                    warnings: Vec::new(),
                }
            }
            ClientSettings::Hadouken(s) => Tested {
                message: format!("Connected to Hadouken {}.", self.hadouken.test(s).await?),
                warnings: Vec::new(),
            },
            ClientSettings::Aria2(s) => Tested {
                message: format!("Connected to Aria2 {}.", self.aria2.test(s).await?),
                warnings: Vec::new(),
            },
            ClientSettings::UTorrent(s) => Tested {
                message: format!(
                    "Connected to uTorrent (build {}).",
                    self.utorrent.test(s).await?
                ),
                warnings: Vec::new(),
            },
            ClientSettings::Flood(s) => {
                self.flood.test(s).await?;
                Tested {
                    message: "Connected to Flood.".into(),
                    warnings: Vec::new(),
                }
            }
            ClientSettings::RTorrent(s) => Tested {
                message: format!("Connected to rTorrent {}.", self.rtorrent.test(s).await?),
                warnings: Vec::new(),
            },
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
            ClientSettings::RTorrent(s) => {
                self.rtorrent
                    .add(s, &req.source, req.hash, req.recent)
                    .await
            }
            ClientSettings::UTorrent(s) => {
                self.utorrent
                    .add(s, &req.source, req.hash, req.recent, req.seed)
                    .await
            }
            ClientSettings::Aria2(s) => self.aria2.add(s, &req.source, req.hash).await,
            ClientSettings::Hadouken(s) => self.hadouken.add(s, &req.source, req.hash).await,
            ClientSettings::Flood(s) => {
                let info = req.release.cloned().unwrap_or_default();
                self.flood.add(s, &req.source, req.hash, &info).await
            }
            ClientSettings::TorrentBlackhole(s) => {
                self.blackhole.add_torrent(s, req.title, &req.source)
            }
            ClientSettings::Sabnzbd(_)
            | ClientSettings::Nzbget(_)
            | ClientSettings::UsenetBlackhole(_)
            | ClientSettings::Pneumatic(_) => Err(DownloadError::Unsupported(
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
            ClientSettings::Pneumatic(s) => pneumatic::add(s, name, content),
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
            ClientSettings::RTorrent(s) => {
                if delete_data {
                    delete_item_data(item)?;
                }
                self.rtorrent.remove(s, &item.download_id).await
            }
            ClientSettings::Flood(s) => self.flood.remove(s, &item.download_id, delete_data).await,
            ClientSettings::UTorrent(s) => {
                self.utorrent
                    .remove(s, &item.download_id, delete_data)
                    .await
            }
            ClientSettings::Hadouken(s) => {
                self.hadouken
                    .remove(s, &item.download_id, delete_data)
                    .await
            }
            ClientSettings::Pneumatic(_) => Err(DownloadError::Unsupported(
                "Pneumatic downloads cannot be removed".into(),
            )),
            ClientSettings::Aria2(s) => {
                self.aria2.remove(s, &item.download_id).await?;
                if delete_data {
                    delete_item_data(item)?;
                }
                Ok(())
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
            ClientSettings::RTorrent(s) => self.rtorrent.items(s).await,
            ClientSettings::Flood(s) => self.flood.items(s).await,
            ClientSettings::UTorrent(s) => self.utorrent.items(s).await,
            ClientSettings::Aria2(s) => self.aria2.items(s).await,
            ClientSettings::Hadouken(s) => self.hadouken.items(s).await,
            ClientSettings::Pneumatic(s) => Ok(pneumatic::items(s)),
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
            ClientSettings::RTorrent(s) => self.rtorrent.mark_imported(s, id).await,
            ClientSettings::Flood(s) => self.flood.mark_imported(s, id).await,
            ClientSettings::UTorrent(s) => self.utorrent.mark_imported(s, id).await,
            // Usenet clients, blackholes and Aria2 have no post-import category.
            ClientSettings::Aria2(_)
            | ClientSettings::Hadouken(_)
            | ClientSettings::Pneumatic(_)
            | ClientSettings::Sabnzbd(_)
            | ClientSettings::Nzbget(_)
            | ClientSettings::TorrentBlackhole(_)
            | ClientSettings::UsenetBlackhole(_) => Ok(()),
        }
    }
}

/// `DeleteItemData`: removes a download's files or folder (its output path as Kitsunarr sees
/// it), for clients that cannot delete data themselves.
pub fn delete_item_data(item: &DownloadItem) -> Result<(), DownloadError> {
    let Some(path) = item
        .output_path
        .as_deref()
        .map(std::path::Path::new)
        .filter(|p| p.is_absolute())
    else {
        return Ok(());
    };
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else if path.is_file() {
        std::fs::remove_file(path)
    } else {
        Ok(())
    };
    result.map_err(|e| DownloadError::Client(format!("could not delete {}: {e}", path.display())))
}
