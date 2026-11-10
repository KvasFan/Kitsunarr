//! Browser helpers and formatting.

use chrono::{DateTime, Local, Utc};

/// Runtime URL base injected by the server (`<meta name="kitsunarr-url-base">`).
pub fn url_base() -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| {
            d.query_selector("meta[name=\"kitsunarr-url-base\"]")
                .ok()
                .flatten()
        })
        .and_then(|m| m.get_attribute("content"))
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_default()
}

/// `http://host:port` + URL base: the root used by the API client.
pub fn api_root() -> String {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    format!("{origin}{}", url_base())
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

pub fn load_pref(key: &str) -> Option<String> {
    storage().and_then(|s| s.get_item(key).ok().flatten())
}

pub fn save_pref(key: &str, value: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(key, value);
    }
}

pub fn prefers_light() -> bool {
    web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-color-scheme: light)")
                .ok()
                .flatten()
        })
        .is_some_and(|m| m.matches())
}

/// Pixels per `rem`: 16 unless the reader chose another default text size. (Browser zoom
/// scales CSS pixels and rems alike, so it does not change this.)
pub fn rem_px() -> f64 {
    web_sys::window()
        .and_then(|w| {
            let root = w.document()?.document_element()?;
            w.get_computed_style(&root).ok().flatten()
        })
        .and_then(|style| style.get_property_value("font-size").ok())
        .and_then(|size| size.trim_end_matches("px").parse::<f64>().ok())
        .filter(|px| *px > 0.0)
        .unwrap_or(16.0)
}

pub fn set_theme_attribute(theme: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = el.set_attribute("data-theme", theme);
    }
}

pub fn copy_to_clipboard(text: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.navigator().clipboard().write_text(text);
    }
}

pub fn focus(id: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        use wasm_bindgen::JsCast;
        if let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() {
            let _ = el.focus();
        }
    }
}

pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

pub fn format_datetime(raw: &str) -> String {
    parse_ts(raw)
        .map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| raw.to_string())
}

pub fn format_date(raw: &str) -> String {
    parse_ts(raw)
        .map(|t| t.with_timezone(&Local).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| raw.get(..10).unwrap_or(raw).to_string())
}

/// "3 hours ago" / "in 2 days".
pub fn relative(raw: &str) -> String {
    let Some(t) = parse_ts(raw) else {
        return raw.to_string();
    };
    let secs = Utc::now().signed_duration_since(t).num_seconds();
    let (future, s) = if secs < 0 {
        (true, -secs)
    } else {
        (false, secs)
    };
    let text = match s {
        0..=59 => {
            return if future {
                "in a moment".into()
            } else {
                "just now".into()
            };
        }
        60..=3599 => format!("{} min", s / 60),
        3600..=86_399 => format!("{} h", s / 3600),
        _ => format!("{} d", s / 86_400),
    };
    if future {
        format!("in {text}")
    } else {
        format!("{text} ago")
    }
}

pub fn initials(title: &str) -> String {
    title
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

pub fn percent(part: i64, whole: i64) -> f64 {
    if whole <= 0 {
        0.0
    } else {
        (part as f64 * 100.0 / whole as f64).clamp(0.0, 100.0)
    }
}
