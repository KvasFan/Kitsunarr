//! Newznab and Torznab indexers (Sonarr `Core/Indexers/Newznab`, `Torznab`, `HttpIndexerBase`).
//!
//! * [`caps`]: capability documents and upstream's defaults;
//! * [`feed`]: release feeds of both dialects, indexer flags and `<error>` handling;
//! * [`request`]: Sonarr's request generator (ID and title tiers, paging, categories);
//! * [`client`]: fetching with per-indexer rate limiting and upstream's paging rules.

pub mod caps;
pub mod client;
pub mod error;
pub mod feed;
pub mod request;
pub mod xml;

pub use caps::{Capabilities, Category, capabilities_or_default, parse_capabilities};
pub use client::{Downloaded, Indexer, IndexerClient, IndexerRelease, TestReport};
pub use error::IndexerError;
pub use feed::{Dialect, Release, parse_feed};
pub use request::{Chain, Criteria, Generator, Search, SeriesIds, Settings};
