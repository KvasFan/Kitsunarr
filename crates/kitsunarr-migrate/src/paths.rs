//! Path mapping between the source installation and Kitsunarr (e.g. Docker volume layouts).
//!
//! Mapping is purely textual: nothing on disk is created, moved, renamed or deleted.

use kitsunarr_domain::api::migration::PathMapping;

#[derive(Debug, Clone, Default)]
pub struct PathMapper {
    /// `(source prefix, target prefix)`, longest source prefix first.
    rules: Vec<(String, String)>,
}

fn trim_sep(p: &str) -> &str {
    let t = p.trim();
    let stripped = t.trim_end_matches(['/', '\\']);
    if stripped.is_empty() {
        &t[..t.len().min(1)]
    } else {
        stripped
    }
}

impl PathMapper {
    pub fn new(mappings: &[PathMapping]) -> Self {
        let mut rules: Vec<(String, String)> = mappings
            .iter()
            .filter(|m| !m.source_prefix.trim().is_empty() && !m.target_prefix.trim().is_empty())
            .map(|m| {
                (
                    trim_sep(&m.source_prefix).to_string(),
                    trim_sep(&m.target_prefix).to_string(),
                )
            })
            .collect();
        rules.sort_by_key(|r| std::cmp::Reverse(r.0.len()));
        Self { rules }
    }

    fn rule_for(&self, path: &str) -> Option<(&str, &str, usize)> {
        for (src, dst) in &self.rules {
            let windows = src.contains('\\') || src.chars().nth(1) == Some(':');
            let matches_prefix = if windows {
                path.len() >= src.len() && path[..src.len()].eq_ignore_ascii_case(src)
            } else {
                path.starts_with(src.as_str())
            };
            if !matches_prefix {
                continue;
            }
            let rest = &path[src.len()..];
            // Only match at a path-segment boundary: `/tv` must not match `/tvshows`.
            if rest.is_empty() || rest.starts_with(['/', '\\']) || src.ends_with(['/', '\\']) {
                return Some((src, dst, src.len()));
            }
        }
        None
    }

    pub fn is_mapped(&self, path: &str) -> bool {
        self.rule_for(path).is_some()
    }

    /// Applies the longest matching rule; unmatched paths are returned unchanged.
    pub fn map(&self, path: &str) -> String {
        let Some((_, dst, len)) = self.rule_for(path) else {
            return path.to_string();
        };
        let rest = &path[len..];
        let posix_target = dst.starts_with('/');
        let rest = if posix_target {
            rest.replace('\\', "/")
        } else {
            rest.to_string()
        };
        if dst == "/" && rest.starts_with('/') {
            rest
        } else {
            format!("{dst}{rest}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, &str)]) -> PathMapper {
        PathMapper::new(
            &pairs
                .iter()
                .map(|(s, t)| PathMapping {
                    source_prefix: s.to_string(),
                    target_prefix: t.to_string(),
                })
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn segment_boundaries_and_longest_prefix() {
        let mapper = m(&[
            ("/tv", "/media/tv"),
            ("/tv/anime", "/media/anime"),
            ("/data/", "/srv/data"),
        ]);
        assert_eq!(mapper.map("/tv/Breaking Bad"), "/media/tv/Breaking Bad");
        assert_eq!(
            mapper.map("/tv/anime/Cowboy Bebop"),
            "/media/anime/Cowboy Bebop"
        );
        assert_eq!(
            mapper.map("/tvshows/X"),
            "/tvshows/X",
            "no partial-segment match"
        );
        assert_eq!(mapper.map("/tv"), "/media/tv");
        assert_eq!(mapper.map("/data/x"), "/srv/data/x");
        assert!(!mapper.is_mapped("/movies/a"));
        assert_eq!(mapper.map("/movies/a"), "/movies/a");
    }

    #[test]
    fn windows_sources_become_posix_targets() {
        let mapper = m(&[("D:\\TV\\", "/media/tv")]);
        assert_eq!(
            mapper.map("D:\\TV\\Breaking Bad\\Season 01"),
            "/media/tv/Breaking Bad/Season 01"
        );
        assert_eq!(
            mapper.map("d:\\tv\\X"),
            "/media/tv/X",
            "drive letters are case-insensitive"
        );
    }
}
