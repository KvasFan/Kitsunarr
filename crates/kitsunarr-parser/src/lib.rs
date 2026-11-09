//! Release-name parsing: a port of Sonarr's parser.
//!
//! Kitsunarr has to read the same release names Sonarr reads (indexer results, download
//! client items, files on disk) and reach the same conclusions, so this crate ports Sonarr's
//! parser rather than reinventing it. Its tests replay Sonarr's own test corpus
//! (`tests/fixtures/parser/sonarr/`, extracted from the audited upstream revision).
//!
//! Upstream: Sonarr `src/NzbDrone.Core/Parser/` at `c018578e4` (GPL-3.0-or-later).

mod common;
pub mod extensions;
pub mod language;
pub mod quality;
mod re;
pub mod release_group;

pub use language::{
    IsoLanguage, SubtitleTitleInfo, find_iso, parse_language_tags, parse_languages,
    parse_subtitle_language, parse_subtitle_language_information,
};
pub use quality::{DetectionSource, ParsedQuality, parse_quality, parse_quality_name};
pub use release_group::parse_release_group;
