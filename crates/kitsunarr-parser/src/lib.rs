//! Release-name parsing: a port of Sonarr's parser.
//!
//! Kitsunarr has to read the same release names Sonarr reads (indexer results, download
//! client items, files on disk) and reach the same conclusions, so this crate ports Sonarr's
//! parser rather than reinventing it. Its tests replay Sonarr's own test corpus
//! (`tests/fixtures/parser/sonarr/`, extracted from the audited upstream revision).
//!
//! Upstream: Sonarr `src/NzbDrone.Core/Parser/` at `c018578e4` (GPL-3.0-or-later).

pub mod extensions;
pub mod quality;
mod re;

pub use quality::{DetectionSource, ParsedQuality, parse_quality, parse_quality_name};
