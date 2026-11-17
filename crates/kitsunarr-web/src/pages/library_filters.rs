//! Tanukarr's series index filters (`useSeries.ts` `FILTERS`, saved custom filters and
//! "Combine filters"), applied by the server through [`LibraryQuery`].

use kitsunarr_client::contracts::{CustomFilter, CustomFilterCondition};
use kitsunarr_domain::SeriesType;
use kitsunarr_domain::api::library::LibraryQuery;
use serde_json::{Value, json};

/// The custom filter list (Sonarr's `type`) of the series index.
pub const FILTER_TYPE: &str = "series";

/// Tanukarr's built-in series filters (key, label), "all" first.
pub const PRESETS: &[(&str, &str)] = &[
    ("all", "All"),
    ("monitored", "Monitored only"),
    ("unmonitored", "Unmonitored only"),
    ("continuing", "Continuing only"),
    ("ended", "Ended only"),
    ("missing", "Missing episodes"),
    ("anime", "Anime"),
];

/// The conditions of a built-in filter, as Tanukarr writes them.
pub fn preset_conditions(key: &str) -> Vec<CustomFilterCondition> {
    let c = |key: &str, value: Value| CustomFilterCondition {
        key: key.into(),
        value,
        kind: "equal".into(),
    };
    match key {
        "monitored" => vec![c("monitored", json!([true]))],
        "unmonitored" => vec![c("monitored", json!([false]))],
        "continuing" => vec![c("status", json!("continuing"))],
        "ended" => vec![c("status", json!("ended"))],
        "missing" => vec![c("missing", json!([true]))],
        "anime" => vec![c("seriesType", json!("anime"))],
        _ => Vec::new(),
    }
}

/// A selected filter: a built-in one by key, or a saved one by id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterRef {
    Preset(String),
    Saved(i64),
}

impl FilterRef {
    /// Stored as the preset key or `custom:{id}`.
    pub fn parse(value: &str) -> Self {
        match value.strip_prefix("custom:").and_then(|id| id.parse().ok()) {
            Some(id) => Self::Saved(id),
            None => Self::Preset(value.to_string()),
        }
    }

    pub fn encode(&self) -> String {
        match self {
            Self::Preset(k) => k.clone(),
            Self::Saved(id) => format!("custom:{id}"),
        }
    }

    fn conditions(&self, saved: &[CustomFilter]) -> Option<Vec<CustomFilterCondition>> {
        match self {
            Self::Preset(k) => Some(preset_conditions(k)),
            Self::Saved(id) => saved
                .iter()
                .find(|f| f.id == *id)
                .map(|f| f.filters.clone()),
        }
    }
}

/// What the selected filters mean for the server query.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Applied {
    /// Two filters ask for different values of the same field: nothing can match.
    pub conflict: bool,
    /// Conditions the server cannot apply (left out, shown to the user).
    pub ignored: Vec<String>,
}

/// The single value of a condition (`"x"` or `["x"]`).
fn single(value: &Value) -> Option<&Value> {
    match value {
        Value::Array(items) if items.len() == 1 => items.first(),
        Value::Array(_) => None,
        other => Some(other),
    }
}

fn set<T: PartialEq>(slot: &mut Option<T>, value: T, applied: &mut Applied) {
    match slot {
        Some(current) if *current != value => applied.conflict = true,
        _ => *slot = Some(value),
    }
}

/// Applies every selected filter (they all must match, as Tanukarr's "Combine filters").
pub fn apply(selected: &[FilterRef], saved: &[CustomFilter], q: &mut LibraryQuery) -> Applied {
    let mut applied = Applied::default();
    for f in selected {
        let Some(conditions) = f.conditions(saved) else {
            continue;
        };
        for c in &conditions {
            if !apply_condition(c, q, &mut applied) {
                applied.ignored.push(c.key.clone());
            }
        }
    }
    applied
}

fn apply_condition(c: &CustomFilterCondition, q: &mut LibraryQuery, applied: &mut Applied) -> bool {
    let equal = c.kind == "equal";
    let value = single(&c.value);
    match (c.key.as_str(), value) {
        ("monitored", Some(Value::Bool(b))) if equal => set(&mut q.monitored, *b, applied),
        ("missing", Some(Value::Bool(b))) if equal => set(&mut q.missing, *b, applied),
        ("status", Some(Value::String(s))) if equal => set(&mut q.status, s.clone(), applied),
        ("seriesType", Some(Value::String(s))) if equal => match s.parse::<SeriesType>() {
            Ok(t) => set(&mut q.series_type, t, applied),
            Err(_) => return false,
        },
        ("path", Some(Value::String(s))) if equal || c.kind == "contains" => {
            set(&mut q.path, s.clone(), applied)
        }
        ("qualityProfileId", Some(Value::Number(n))) if equal => match n.as_i64() {
            Some(id) => set(&mut q.quality_profile_id, id, applied),
            None => return false,
        },
        ("tags", Some(Value::Number(n))) if c.kind == "contains" || equal => match n.as_i64() {
            Some(id) => set(&mut q.tag_id, id, applied),
            None => return false,
        },
        ("metadataSource", Some(Value::String(s))) if equal => match s.parse() {
            Ok(p) => set(&mut q.provider, p, applied),
            Err(_) => return false,
        },
        _ => return false,
    }
    true
}

/// The conditions a saved filter gets from the current choices (what [`apply`] put in the
/// query), in Tanukarr's form.
pub fn conditions_of(q: &LibraryQuery) -> Vec<CustomFilterCondition> {
    let c = |key: &str, value: Value| CustomFilterCondition {
        key: key.into(),
        value,
        kind: "equal".into(),
    };
    let mut out = Vec::new();
    if let Some(m) = q.monitored {
        out.push(c("monitored", json!([m])));
    }
    if let Some(s) = &q.status {
        out.push(c("status", json!(s)));
    }
    if let Some(t) = q.series_type {
        out.push(c("seriesType", json!(t.as_str())));
    }
    if let Some(m) = q.missing {
        out.push(c("missing", json!([m])));
    }
    if let Some(p) = &q.path {
        out.push(c("path", json!(p)));
    }
    if let Some(id) = q.quality_profile_id {
        out.push(c("qualityProfileId", json!(id)));
    }
    if let Some(id) = q.tag_id {
        out.push(CustomFilterCondition {
            key: "tags".into(),
            value: json!([id]),
            kind: "contains".into(),
        });
    }
    if let Some(p) = q.provider {
        out.push(c("metadataSource", json!(p.as_str())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(id: i64, filters: Value) -> CustomFilter {
        CustomFilter {
            id,
            kind: FILTER_TYPE.into(),
            label: format!("Saved {id}"),
            filters: serde_json::from_value(filters).unwrap(),
        }
    }

    #[test]
    fn presets_and_saved_filters_combine() {
        let list = vec![saved(
            7,
            json!([
                {"key": "path", "value": "/tv", "type": "equal"},
                {"key": "tags", "value": [3], "type": "contains"},
                {"key": "network", "value": "HBO", "type": "equal"}
            ]),
        )];
        let mut q = LibraryQuery::default();
        let applied = apply(
            &[
                FilterRef::Preset("monitored".into()),
                FilterRef::Preset("missing".into()),
                FilterRef::Saved(7),
            ],
            &list,
            &mut q,
        );
        assert_eq!(q.monitored, Some(true));
        assert_eq!(q.missing, Some(true));
        assert_eq!(q.path.as_deref(), Some("/tv"));
        assert_eq!(q.tag_id, Some(3));
        assert!(!applied.conflict);
        assert_eq!(applied.ignored, ["network"]);
        // Saving the result gives back the same query.
        let mut again = LibraryQuery::default();
        let round = saved(8, serde_json::to_value(conditions_of(&q)).unwrap());
        apply(&[FilterRef::Saved(8)], &[round], &mut again);
        assert_eq!(again, q);
    }

    #[test]
    fn contradicting_filters_match_nothing() {
        let mut q = LibraryQuery::default();
        let applied = apply(
            &[
                FilterRef::Preset("continuing".into()),
                FilterRef::Preset("ended".into()),
            ],
            &[],
            &mut q,
        );
        assert!(applied.conflict);
        let mut q = LibraryQuery::default();
        apply(&[FilterRef::Preset("anime".into())], &[], &mut q);
        assert_eq!(q.series_type, Some(SeriesType::Anime));
        assert_eq!(FilterRef::parse("custom:4"), FilterRef::Saved(4));
        assert_eq!(FilterRef::parse("ended").encode(), "ended");
    }
}
