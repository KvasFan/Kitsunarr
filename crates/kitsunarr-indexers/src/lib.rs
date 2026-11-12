//! Indexers (Sonarr `Core/Indexers`: Newznab, Torznab, Nyaa, Torrent RSS, IPTorrents,
//! TorrentLeech, Fanzub, FileList, HDBits, BroadcasTheNet, `HttpIndexerBase`).
//!
//! * [`caps`]: capability documents and upstream's defaults;
//! * [`feed`]: Newznab/Torznab/Nyaa feeds, indexer flags and `<error>` handling;
//! * [`torrent_rss`]: the generic RSS parsers, Torrent RSS layout detection, IPTorrents and
//!   TorrentLeech;
//! * [`fanzub`], [`filelist`], [`hdbits`], [`broadcasthenet`]: those indexers' requests and
//!   parsers;
//! * [`request`]: Sonarr's request generator (ID and title tiers, paging, categories);
//! * [`client`]: fetching with per-indexer rate limiting and upstream's paging rules.

pub mod broadcasthenet;
pub mod caps;
pub mod client;
pub mod error;
pub mod fanzub;
pub mod feed;
pub mod filelist;
pub mod hdbits;
mod lenient;
pub mod request;
pub mod torrent_rss;
pub mod xml;

pub use caps::{Capabilities, Category, capabilities_or_default, parse_capabilities};
pub use client::{
    Downloaded, Indexer, IndexerClient, IndexerRelease, LastRelease, RecentFetch, TestReport,
};
pub use error::IndexerError;
pub use feed::{Dialect, Release, parse_feed};
pub use request::{Chain, Criteria, Generator, Search, SeriesIds, Settings};
