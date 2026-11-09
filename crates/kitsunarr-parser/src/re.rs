//! A thin wrapper over `fancy-regex` with the conveniences the .NET-derived code needs.
//!
//! Porting notes (see `docs/architecture/audit/sonarr-parsing-decisions.md` §0 and §11):
//! * `fancy-regex` supports look-around (including variable-length lookbehind), backreferences
//!   and atomic groups, like .NET. `\d`, `\w` and `\b` are Unicode-aware in both.
//! * .NET lets one pattern reuse a group name. Ported patterns give each occurrence its own
//!   name (`version1`, `version2`, ...) and read them with [`first_of`] / [`any_of`].
//! * .NET's `RegexOptions.IgnorePatternWhitespace` keeps whitespace inside character classes
//!   literal, while Rust's `(?x)` drops it, so ported patterns are written without verbose mode.
//! * Engine errors (for example the backtracking limit) are treated as "no match".

use fancy_regex::{Match, Regex};

/// Captures over a `str` subject.
pub(crate) type Captures<'t> = fancy_regex::Captures<'t, str>;

pub(crate) struct Re(Regex);

impl Re {
    /// Case-sensitive pattern. Panics on an invalid pattern: patterns are compile-time
    /// constants and covered by tests.
    pub fn new(pattern: &str) -> Self {
        match Regex::new(pattern) {
            Ok(r) => Self(r),
            Err(e) => panic!("invalid pattern {pattern:?}: {e}"),
        }
    }

    /// Case-insensitive pattern (`RegexOptions.IgnoreCase`).
    pub fn ci(pattern: &str) -> Self {
        Self::new(&format!("(?i){pattern}"))
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.0.is_match(text).unwrap_or(false)
    }

    /// First match (`Regex.Match`).
    pub fn captures<'t>(&self, text: &'t str) -> Option<Captures<'t>> {
        self.0.captures(text).ok().flatten()
    }

    /// All non-overlapping matches, left to right (`Regex.Matches`).
    pub fn captures_all<'t>(&self, text: &'t str) -> Vec<Captures<'t>> {
        self.0.captures_iter(text).filter_map(Result::ok).collect()
    }

    /// `Regex.Replace` with a literal replacement (no `$` expansion).
    pub fn replace_all_literal(&self, text: &str, replacement: &str) -> String {
        self.0
            .replace_all(text, fancy_regex::NoExpand(replacement))
            .into_owned()
    }

    /// Names of the named groups.
    pub fn group_names(&self) -> Vec<String> {
        self.0.capture_names().flatten().map(String::from).collect()
    }

    /// Number of non-overlapping matches (`Regex.Matches(...).Count`).
    pub fn count(&self, text: &str) -> usize {
        self.0.find_iter(text).filter(Result::is_ok).count()
    }
}

/// The first of several same-meaning groups that participated in the match.
pub(crate) fn first_of<'t>(c: &Captures<'t>, names: &[&str]) -> Option<Match<'t>> {
    names.iter().find_map(|n| c.name(n))
}

/// Whether any of several same-meaning groups participated in the match.
pub(crate) fn any_of(c: &Captures<'_>, names: &[&str]) -> bool {
    first_of(c, names).is_some()
}

/// Whether a named group participated (`Groups[name].Success`).
pub(crate) fn has(c: &Captures<'_>, name: &str) -> bool {
    c.name(name).is_some()
}

/// Ordinal case-insensitive `string.Contains` (Sonarr's `ContainsIgnoreCase`).
pub(crate) fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}
