//! Release-name parsing: a port of Sonarr's parser.
//!
//! Kitsunarr has to read the same release names Sonarr reads (indexer results, download
//! client items, files on disk) and reach the same conclusions, so this crate ports Sonarr's
//! parser rather than reinventing it. Its tests replay Sonarr's own test corpus
//! (`tests/fixtures/parser/sonarr/`, extracted from the audited upstream revision).
//!
//! Upstream: Sonarr `src/NzbDrone.Core/Parser/` at `c018578e4` (GPL-3.0-or-later).

mod common;
mod diacritics;
pub mod extensions;
pub mod language;
pub mod model;
pub mod movie;
pub mod netregex;
pub mod quality;
mod re;
pub mod release_group;
pub mod scene;
pub mod title;
// Generated from upstream verbatim, including the pattern upstream itself leaves unused.
#[allow(dead_code)]
mod title_patterns;

pub use language::{
    IsoLanguage, SubtitleTitleInfo, find_iso, iso_for, parse_language_tags, parse_languages,
    parse_subtitle_language, parse_subtitle_language_information,
};
pub use model::{ParsedEpisodeInfo, ReleaseType, SeriesTitleInfo, format_season_numbers};
pub use quality::{DetectionSource, ParsedQuality, parse_quality, parse_quality_name};
pub use release_group::parse_release_group;
pub use scene::is_scene_title;
pub use title::{
    clean_series_title, has_multiple_languages, normalize_episode_title, normalize_imdb_id,
    normalize_title, parse_path, parse_season_folder, parse_season_title, parse_series_name,
    parse_title, remove_diacritics, simplify_title,
};
