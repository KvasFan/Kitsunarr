//! `Parser/SceneChecker.cs`.

use crate::title::parse_title;

/// Whether a title looks like an unmodified scene release name. Prefers false negatives over
/// false positives: better not to use a title that might be scene than one that is not.
pub fn is_scene_title(title: &str) -> bool {
    if !title.contains('.') || title.contains(' ') {
        return false;
    }
    match parse_title(title) {
        Some(p) => {
            p.release_group.is_some()
                && p.quality.quality.id != 0
                && !p.series_title.trim().is_empty()
        }
        None => false,
    }
}
