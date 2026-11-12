//! Lenient reads of JSON API responses, converting as Json.NET does when Sonarr deserializes
//! them into typed models: numbers given as strings, booleans given as 0/1, ids as numbers or
//! strings.

use chrono::{DateTime, Utc};
use serde_json::Value;

/// An integer, from a number or a numeric string.
pub(crate) fn int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// A string, from a string or a number.
pub(crate) fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A boolean, from a boolean, 0/1 or "true"/"false".
pub(crate) fn boolean(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true") || s.trim() == "1",
        _ => false,
    }
}

/// A date: ISO 8601 (with or without a colon in the offset) or `yyyy-MM-dd HH:mm:ss`, taken
/// as UTC without an offset.
pub(crate) fn date(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let s = text(v)?;
    let s = s.trim();
    DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z")
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| crate::feed::parse_date(s))
}

/// `tt0000000` from an IMDb number (with or without the `tt` prefix).
pub(crate) fn imdb(v: &str) -> Option<String> {
    v.trim_start_matches('t')
        .parse::<i32>()
        .ok()
        .filter(|n| *n > 0)
        .map(|n| format!("tt{n:07}"))
}

/// Deserializes a response, ignoring a byte-order mark as Json.NET does.
pub(crate) fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, crate::IndexerError> {
    serde_json::from_str(body.trim_start_matches('\u{feff}'))
        .map_err(|e| crate::IndexerError::Unexpected(format!("invalid JSON: {e}")))
}
