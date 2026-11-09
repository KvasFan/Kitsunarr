//! Lenient accessors over source records.
//!
//! Records from both sources are JSON objects whose keys are lower-cased: SQLite columns
//! (`QualityProfileId`) and API fields (`qualityProfileId`) then share one key
//! (`qualityprofileid`). Accessors accept every encoding seen in the wild: integers or
//! numeric strings, `0/1`/`true`/`'False'` booleans, JSON stored as text, etc.

use serde_json::{Map, Value};

/// A source record with lower-cased keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rec(pub Map<String, Value>);

impl Rec {
    /// Builds a record, lower-casing keys (later duplicates win).
    pub fn from_map(map: Map<String, Value>) -> Self {
        Rec(map
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect())
    }

    pub fn from_value(v: Value) -> Self {
        match v {
            Value::Object(m) => Self::from_map(m),
            _ => Rec::default(),
        }
    }

    pub fn raw(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    pub fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn i64(&self, key: &str) -> Option<i64> {
        as_i64(self.raw(key)?)
    }

    pub fn i32(&self, key: &str) -> Option<i32> {
        self.i64(key).and_then(|v| i32::try_from(v).ok())
    }

    pub fn f64(&self, key: &str) -> Option<f64> {
        match self.raw(key)? {
            Value::Number(n) => n.as_f64(),
            // Sonarr's history data may contain locale-formatted numbers ("1,5").
            Value::String(s) => s.trim().replace(',', ".").parse().ok(),
            _ => None,
        }
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        as_bool(self.raw(key)?)
    }

    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        self.bool(key).unwrap_or(default)
    }

    /// Trimmed, non-empty string (numbers are stringified).
    pub fn str(&self, key: &str) -> Option<String> {
        match self.raw(key)? {
            Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    /// Value that may be stored as JSON text (SQLite) or inline (API).
    pub fn json(&self, key: &str) -> Option<Value> {
        match self.raw(key)? {
            Value::String(s) => {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    serde_json::from_str(t)
                        .ok()
                        .or_else(|| Some(Value::String(s.clone())))
                }
            }
            other => Some(other.clone()),
        }
    }

    pub fn array(&self, key: &str) -> Vec<Value> {
        match self.json(key) {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        }
    }

    pub fn object(&self, key: &str) -> Option<Rec> {
        match self.json(key)? {
            Value::Object(m) => Some(Rec::from_map(m)),
            _ => None,
        }
    }

    /// Integer list from a JSON array (`[1,2]`), tolerating `[{"id":1}]` objects.
    pub fn int_list(&self, key: &str) -> Vec<i64> {
        self.array(key)
            .iter()
            .filter_map(|v| match v {
                Value::Object(o) => o.get("id").and_then(as_i64),
                other => as_i64(other),
            })
            .collect()
    }

    /// String list from a JSON array or a comma-separated string (older release profiles).
    pub fn string_list(&self, key: &str) -> Vec<String> {
        match self.json(key) {
            Some(Value::Array(a)) => a
                .into_iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
            Some(Value::String(s)) => s
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Normalised UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`), `None` for empty/sentinel dates.
    pub fn ts(&self, key: &str) -> Option<String> {
        self.str(key).and_then(|s| normalize_timestamp(&s))
    }

    /// Calendar date (`YYYY-MM-DD`).
    pub fn date(&self, key: &str) -> Option<String> {
        let s = self.str(key)?;
        let d = s.get(..10)?;
        chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .ok()
            .filter(|d| d.year_ce().1 > 1)
            .map(|d| d.to_string())
    }
}

use chrono::Datelike;

pub fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

pub fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_i64().map(|i| i != 0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" | "" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Accepts `2026-11-09 17:28:34.966664Z`, `2026-11-09T17:28:34Z`, `2026-11-09T17:28:34+02:00`,
/// `2026-11-09 17:28:34` (treated as UTC) and `2026-11-09`. Year-1 sentinels become `None`.
pub fn normalize_timestamp(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let utc = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        dt.with_timezone(&chrono::Utc)
    } else {
        let trimmed = s.trim_end_matches('Z').replace('T', " ");
        let naive = chrono::NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%d %H:%M:%S%.f")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%d %H:%M"))
            .or_else(|_| {
                chrono::NaiveDate::parse_from_str(&trimmed, "%Y-%m-%d")
                    .map(|d| d.and_hms_opt(0, 0, 0).unwrap_or_default())
            })
            .ok()?;
        naive.and_utc()
    };
    if utc.year() <= 1 {
        return None;
    }
    Some(utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(v: Value) -> Rec {
        Rec::from_value(v)
    }

    #[test]
    fn keys_are_case_insensitive_and_values_lenient() {
        let r = rec(
            json!({"QualityProfileId": "7", "Monitored": 1, "EnableRss": "False", "Tags": "[\n  2\n]",
                           "Title": "  Breaking Bad ", "Empty": "", "Ratings": "{\"votes\": 3, \"value\": 8.9}"}),
        );
        assert_eq!(r.i64("qualityprofileid"), Some(7));
        assert_eq!(r.bool("monitored"), Some(true));
        assert_eq!(
            r.bool("enablerss"),
            Some(false),
            "text booleans written by Sonarr migration 158"
        );
        assert_eq!(r.int_list("tags"), vec![2]);
        assert_eq!(r.str("title").as_deref(), Some("Breaking Bad"));
        assert_eq!(r.str("empty"), None);
        assert_eq!(r.object("ratings").unwrap().f64("value"), Some(8.9));
    }

    #[test]
    fn string_lists_accept_json_and_csv() {
        assert_eq!(
            rec(json!({"x": "[\"CAM\", \"TS\"]"})).string_list("x"),
            vec!["CAM", "TS"]
        );
        assert_eq!(
            rec(json!({"x": "CAM, TS,"})).string_list("x"),
            vec!["CAM", "TS"]
        );
        assert_eq!(rec(json!({"x": ["a"]})).string_list("x"), vec!["a"]);
    }

    #[test]
    fn timestamps() {
        assert_eq!(
            normalize_timestamp("2026-11-09 17:28:34.966664Z").as_deref(),
            Some("2026-11-09T17:28:34Z")
        );
        assert_eq!(
            normalize_timestamp("2026-11-09T17:28:34Z").as_deref(),
            Some("2026-11-09T17:28:34Z")
        );
        assert_eq!(
            normalize_timestamp("2026-11-09T19:28:34+02:00").as_deref(),
            Some("2026-11-09T17:28:34Z")
        );
        assert_eq!(
            normalize_timestamp("1998-04-03 08:25:00").as_deref(),
            Some("1998-04-03T08:25:00Z")
        );
        assert_eq!(
            normalize_timestamp("1998-04-03").as_deref(),
            Some("1998-04-03T00:00:00Z")
        );
        assert_eq!(normalize_timestamp("0001-01-01 00:00:00Z"), None);
        assert_eq!(normalize_timestamp("garbage"), None);
        assert_eq!(
            rec(json!({"d": "1998-04-03 00:00:00Z"}))
                .date("d")
                .as_deref(),
            Some("1998-04-03")
        );
    }
}
