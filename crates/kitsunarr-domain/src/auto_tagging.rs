//! Sonarr's auto tagging (`AutoTaggingService.GetTagChanges`, `Core/AutoTagging`).
//!
//! A rule's conditions are grouped by implementation; a group matches unless one of its
//! required conditions fails or all of them fail (`SpecificationMatchesGroup`), and a rule
//! matches when every group does. A matching rule adds its tags the title lacks; a rule that
//! does not match, with "remove tags automatically", removes its tags. Removals are applied
//! before additions, so a tag one rule adds survives another rule's removal.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::api::settings::{AutoTag, CustomFormatSpecification};
use crate::{MovieStatus, SeriesStatus, SeriesType};

/// Condition implementations, with Sonarr's display names (`ImplementationName`).
pub const SPECIFICATIONS: &[(&str, &str)] = &[
    ("GenreSpecification", "Genre"),
    ("MonitoredSpecification", "Monitored"),
    ("NetworkSpecification", "Network"),
    ("OriginalCountrySpecification", "Original Country"),
    ("OriginalLanguageSpecification", "Original Language"),
    ("QualityProfileSpecification", "Quality Profile"),
    ("RootFolderSpecification", "Root Folder"),
    ("SeriesTypeSpecification", "Series Type"),
    ("StatusSpecification", "Status"),
    ("TagSpecification", "Tag"),
    ("YearSpecification", "Year"),
];

/// What a rule is evaluated against: a series, or a film (Tanukarr's films are series).
#[derive(Debug, Clone, Default)]
pub struct AutoTagSubject {
    pub genres: Vec<String>,
    pub network: Option<String>,
    pub original_country: Option<String>,
    /// Sonarr language id.
    pub original_language: Option<i64>,
    pub quality_profile_id: i64,
    pub root_folder_path: String,
    /// Sonarr's `SeriesTypes`: 0 standard, 1 daily, 2 anime.
    pub series_type: i64,
    /// Sonarr's `SeriesStatusType`: -1 deleted, 0 continuing, 1 ended, 2 upcoming.
    pub status: i64,
    pub tags: Vec<i64>,
    pub year: Option<i32>,
    pub monitored: bool,
}

/// Sonarr's integer for a series type.
pub fn series_type_value(t: SeriesType) -> i64 {
    match t {
        SeriesType::Standard => 0,
        SeriesType::Daily => 1,
        SeriesType::Anime => 2,
    }
}

/// Sonarr's integer for a series status (unknown reads as continuing, Sonarr's default).
pub fn series_status_value(s: SeriesStatus) -> i64 {
    match s {
        SeriesStatus::Deleted => -1,
        SeriesStatus::Continuing | SeriesStatus::Unknown => 0,
        SeriesStatus::Ended => 1,
        SeriesStatus::Upcoming => 2,
    }
}

/// A film's status as the series status Tanukarr stores for it (released films have ended).
pub fn movie_status_value(s: MovieStatus) -> i64 {
    match s {
        MovieStatus::Deleted => -1,
        MovieStatus::InCinemas | MovieStatus::Unknown => 0,
        MovieStatus::Released => 1,
        MovieStatus::Announced => 2,
    }
}

/// Tags to add and to remove.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoTagChanges {
    pub add: BTreeSet<i64>,
    pub remove: BTreeSet<i64>,
}

impl AutoTagChanges {
    /// Applies the changes to a tag list (removals first, as `UpdateAutoTaggingTags`); true
    /// when the list changed.
    pub fn apply(&self, tags: &mut Vec<i64>) -> bool {
        let before = tags.clone();
        tags.retain(|t| !self.remove.contains(t));
        for t in &self.add {
            if !tags.contains(t) {
                tags.push(*t);
            }
        }
        tags.sort_unstable();
        let mut sorted = before;
        sorted.sort_unstable();
        *tags != sorted
    }
}

/// `GetTagChanges`.
pub fn tag_changes(rules: &[AutoTag], subject: &AutoTagSubject) -> AutoTagChanges {
    let mut changes = AutoTagChanges::default();
    for rule in rules {
        if rule_matches(rule, subject) {
            for t in &rule.tags {
                if !subject.tags.contains(t) {
                    changes.add.insert(*t);
                }
            }
        } else if rule.remove_tags_automatically {
            changes.remove.extend(rule.tags.iter().copied());
        }
    }
    changes
}

fn rule_matches(rule: &AutoTag, subject: &AutoTagSubject) -> bool {
    let mut groups: Vec<(&str, Vec<(bool, bool)>)> = Vec::new();
    for spec in &rule.specifications {
        let matched = spec_matches(spec, subject) != spec.negate;
        match groups
            .iter_mut()
            .find(|(k, _)| *k == spec.implementation.as_str())
        {
            Some((_, v)) => v.push((spec.required, matched)),
            None => groups.push((&spec.implementation, vec![(spec.required, matched)])),
        }
    }
    groups.iter().all(|(_, matches)| {
        !(matches.iter().any(|(required, m)| *required && !m) || matches.iter().all(|(_, m)| !m))
    })
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// `PathEquals` on Linux: case-sensitive, trailing separators ignored.
fn path_equals(a: &str, b: &str) -> bool {
    let trim = |p: &str| {
        let t = p.trim_end_matches(['/', '\\']);
        if t.is_empty() {
            "/".to_string()
        } else {
            t.to_string()
        }
    };
    !a.is_empty() && !b.is_empty() && trim(a) == trim(b)
}

/// One condition, before negation. Unknown implementations never match.
fn spec_matches(spec: &CustomFormatSpecification, s: &AutoTagSubject) -> bool {
    let value = spec.fields.get("value");
    match spec.implementation.as_str() {
        "GenreSpecification" => {
            let values = strings(value);
            s.genres
                .iter()
                .any(|g| values.iter().any(|v| eq_ignore_case(v, g)))
        }
        "MonitoredSpecification" => s.monitored,
        "NetworkSpecification" => s
            .network
            .as_deref()
            .is_some_and(|n| strings(value).iter().any(|v| eq_ignore_case(v, n))),
        "OriginalCountrySpecification" => s
            .original_country
            .as_deref()
            .is_some_and(|c| strings(value).iter().any(|v| eq_ignore_case(v, c))),
        "OriginalLanguageSpecification" => {
            int(value).is_some_and(|v| Some(v) == s.original_language)
        }
        "QualityProfileSpecification" => int(value) == Some(s.quality_profile_id),
        "RootFolderSpecification" => value
            .and_then(Value::as_str)
            .is_some_and(|v| path_equals(v, &s.root_folder_path)),
        "SeriesTypeSpecification" => int(value) == Some(s.series_type),
        "StatusSpecification" => int(spec.fields.get("status")) == Some(s.status),
        "TagSpecification" => int(value).is_some_and(|v| s.tags.contains(&v)),
        "YearSpecification" => {
            let year = i64::from(s.year.unwrap_or(0));
            let min = int(spec.fields.get("min")).unwrap_or(0);
            let max = int(spec.fields.get("max")).unwrap_or(0);
            year >= min && year <= max
        }
        _ => false,
    }
}

/// Sonarr's validation of a rule (`AutoTaggingController` and each specification's
/// validator): a name, at least one tag and one condition, named conditions with values.
pub fn validate(rule: &AutoTag) -> Result<(), String> {
    if rule.name.trim().is_empty() {
        return Err("Name must not be empty.".into());
    }
    if rule.tags.is_empty() {
        return Err("Tags must not be empty.".into());
    }
    if rule.specifications.is_empty() {
        return Err("Must contain at least one Condition".into());
    }
    if rule.specifications.iter().any(|s| s.name.trim().is_empty()) {
        return Err("Condition name(s) cannot be empty or consist of only spaces".into());
    }
    for spec in &rule.specifications {
        let value = spec.fields.get("value");
        let ok = match spec.implementation.as_str() {
            "GenreSpecification" | "NetworkSpecification" | "OriginalCountrySpecification" => {
                !strings(value).is_empty()
            }
            "MonitoredSpecification" => true,
            "OriginalLanguageSpecification" | "SeriesTypeSpecification" => int(value).is_some(),
            "QualityProfileSpecification" | "TagSpecification" => int(value).is_some_and(|v| v > 0),
            "RootFolderSpecification" => value
                .and_then(Value::as_str)
                .is_some_and(|v| !v.trim().is_empty()),
            "StatusSpecification" => int(spec.fields.get("status")).is_some(),
            "YearSpecification" => {
                let (min, max) = (int(spec.fields.get("min")), int(spec.fields.get("max")));
                matches!((min, max), (Some(a), Some(b)) if a > 0 && b > 0 && b >= a)
            }
            other => return Err(format!("Unknown condition type {other}.")),
        };
        if !ok {
            return Err(format!("Condition `{}` needs a valid value.", spec.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Sonarr's `AutoTaggingServiceFixture`, plus the condition semantics.
    use super::*;
    use serde_json::json;

    fn spec(implementation: &str, fields: Value) -> CustomFormatSpecification {
        CustomFormatSpecification {
            name: implementation.trim_end_matches("Specification").into(),
            implementation: implementation.into(),
            negate: false,
            required: false,
            fields: fields.as_object().cloned().unwrap_or_default(),
        }
    }

    fn comedy_rule() -> AutoTag {
        AutoTag {
            id: 1,
            name: "Test".into(),
            remove_tags_automatically: false,
            specifications: vec![spec("GenreSpecification", json!({"value": ["Comedy"]}))],
            tags: vec![1],
        }
    }

    fn series() -> AutoTagSubject {
        AutoTagSubject {
            genres: vec!["Comedy".into()],
            ..Default::default()
        }
    }

    #[test]
    fn should_not_have_changes_if_there_are_no_auto_tags() {
        assert_eq!(tag_changes(&[], &series()), AutoTagChanges::default());
    }

    #[test]
    fn should_have_tags_to_add_if_series_does_not_have_match_tag() {
        let c = tag_changes(&[comedy_rule()], &series());
        assert_eq!(c.add, BTreeSet::from([1]));
        assert!(c.remove.is_empty());
    }

    #[test]
    fn should_not_have_tags_to_remove_if_series_has_matching_tag_but_remove_is_false() {
        let s = AutoTagSubject {
            tags: vec![1],
            genres: vec!["NotComedy".into()],
            ..Default::default()
        };
        assert_eq!(tag_changes(&[comedy_rule()], &s), AutoTagChanges::default());
    }

    #[test]
    fn should_have_tags_to_remove_if_series_has_matching_tag_and_remove_is_true() {
        let s = AutoTagSubject {
            tags: vec![1],
            genres: vec!["NotComedy".into()],
            ..Default::default()
        };
        let mut rule = comedy_rule();
        rule.remove_tags_automatically = true;
        let c = tag_changes(&[rule], &s);
        assert!(c.add.is_empty());
        assert_eq!(c.remove, BTreeSet::from([1]));
    }

    #[test]
    fn should_have_tags_to_add_if_series_does_not_have_match_tag_and_series_matches_all_rules() {
        let mut rule = comedy_rule();
        rule.specifications
            .push(spec("SeriesTypeSpecification", json!({"value": 0})));
        let c = tag_changes(&[rule], &series());
        assert_eq!(c.add, BTreeSet::from([1]));
    }

    #[test]
    fn should_match_if_specification_is_negated() {
        let s = AutoTagSubject {
            genres: vec!["NotComedy".into()],
            ..Default::default()
        };
        let mut rule = comedy_rule();
        rule.specifications[0].negate = true;
        assert_eq!(tag_changes(&[rule], &s).add, BTreeSet::from([1]));
    }

    #[test]
    fn groups_need_one_match_and_every_required_one() {
        let mut rule = comedy_rule();
        // Two genre conditions: one matching is enough…
        rule.specifications
            .push(spec("GenreSpecification", json!({"value": ["Drama"]})));
        assert_eq!(tag_changes(&[rule.clone()], &series()).add.len(), 1);
        // …unless the failing one is required.
        rule.specifications[1].required = true;
        assert!(tag_changes(&[rule.clone()], &series()).add.is_empty());
        // A second group must match too.
        rule.specifications[1].required = false;
        rule.specifications
            .push(spec("YearSpecification", json!({"min": 2000, "max": 2010})));
        let mut s = series();
        s.year = Some(2011);
        assert!(tag_changes(&[rule.clone()], &s).add.is_empty());
        s.year = Some(2010);
        assert_eq!(tag_changes(&[rule], &s).add.len(), 1);
    }

    #[test]
    fn conditions_read_sonarr_values() {
        let s = AutoTagSubject {
            network: Some("BBC One".into()),
            original_country: Some("gb".into()),
            original_language: Some(1),
            quality_profile_id: 4,
            root_folder_path: "/tv/".into(),
            series_type: 2,
            status: 1,
            tags: vec![7],
            monitored: true,
            ..Default::default()
        };
        let one = |sp| {
            tag_changes(
                &[AutoTag {
                    specifications: vec![sp],
                    ..comedy_rule()
                }],
                &s,
            )
        };
        for (implementation, fields) in [
            ("NetworkSpecification", json!({"value": ["bbc one"]})),
            ("OriginalCountrySpecification", json!({"value": ["GB"]})),
            ("OriginalLanguageSpecification", json!({"value": 1})),
            ("QualityProfileSpecification", json!({"value": 4})),
            ("RootFolderSpecification", json!({"value": "/tv"})),
            ("SeriesTypeSpecification", json!({"value": 2})),
            ("StatusSpecification", json!({"status": 1})),
            ("TagSpecification", json!({"value": 7})),
            ("MonitoredSpecification", json!({})),
        ] {
            assert_eq!(
                one(spec(implementation, fields)).add.len(),
                1,
                "{implementation}"
            );
        }
        for (implementation, fields) in [
            ("NetworkSpecification", json!({"value": ["HBO"]})),
            ("RootFolderSpecification", json!({"value": "/TV"})),
            ("StatusSpecification", json!({"status": 0})),
            ("SeriesTypeSpecification", json!({"value": 0})),
            ("UnknownSpecification", json!({})),
        ] {
            assert!(
                one(spec(implementation, fields)).add.is_empty(),
                "{implementation}"
            );
        }
    }

    #[test]
    fn a_tag_one_rule_adds_survives_another_rules_removal() {
        let mut other = comedy_rule();
        other.specifications = vec![spec("GenreSpecification", json!({"value": ["Drama"]}))];
        other.remove_tags_automatically = true;
        let c = tag_changes(&[comedy_rule(), other], &series());
        let mut tags = vec![];
        assert!(c.apply(&mut tags));
        assert_eq!(tags, vec![1]);
        assert!(!c.apply(&mut tags));
    }

    #[test]
    fn validation_follows_sonarr() {
        assert!(validate(&comedy_rule()).is_ok());
        let mut r = comedy_rule();
        r.tags.clear();
        assert!(validate(&r).is_err());
        let mut r = comedy_rule();
        r.specifications = vec![spec("YearSpecification", json!({"min": 2010, "max": 2000}))];
        assert!(validate(&r).is_err());
        r.specifications[0].name = " ".into();
        assert!(validate(&r).is_err());
    }
}
