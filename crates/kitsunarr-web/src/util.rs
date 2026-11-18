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

/// Settings › UI as the formatting helpers apply it (Sonarr's moment formats).
#[derive(Debug, Clone, PartialEq)]
pub struct UiFormat {
    pub short_date: String,
    pub long_date: String,
    pub time: String,
    /// IANA name; empty follows the browser.
    pub time_zone: String,
    pub relative_dates: bool,
    pub week_column_header: String,
}

impl Default for UiFormat {
    fn default() -> Self {
        Self::from_settings(&kitsunarr_domain::api::settings::GeneralSettings::default())
    }
}

impl UiFormat {
    pub fn from_settings(g: &kitsunarr_domain::api::settings::GeneralSettings) -> Self {
        Self {
            short_date: g.short_date_format.clone(),
            long_date: g.long_date_format.clone(),
            time: g.time_format.clone(),
            time_zone: g.time_zone.trim().to_string(),
            relative_dates: g.show_relative_dates,
            week_column_header: g.calendar_week_column_header.clone(),
        }
    }
}

thread_local! {
    static UI_FORMAT: std::cell::RefCell<UiFormat> = std::cell::RefCell::new(UiFormat::default());
}

/// Applies Settings › UI: the date formats, time zone and relative dates of every helper below,
/// and the colour-impaired palette.
pub fn apply_ui_settings(g: &kitsunarr_domain::api::settings::GeneralSettings) {
    UI_FORMAT.with(|f| *f.borrow_mut() = UiFormat::from_settings(g));
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = if g.enable_color_impaired_mode {
            el.set_attribute("data-color-impaired", "true")
        } else {
            el.remove_attribute("data-color-impaired")
        };
    }
}

pub fn ui_format() -> UiFormat {
    UI_FORMAT.with(|f| f.borrow().clone())
}

/// The wall-clock time of an instant in the chosen time zone (the browser's when none, or when
/// the browser does not know the zone).
pub fn local_naive(t: DateTime<Utc>) -> chrono::NaiveDateTime {
    let tz = ui_format().time_zone;
    if !tz.is_empty()
        && let Some(n) = in_zone(t, &tz)
    {
        return n;
    }
    t.with_timezone(&Local).naive_local()
}

/// `Intl.DateTimeFormat` with `timeZone`, read back part by part.
fn in_zone(t: DateTime<Utc>, tz: &str) -> Option<chrono::NaiveDateTime> {
    use js_sys::{Array, Function, Object, Reflect};
    use wasm_bindgen::{JsCast, JsValue};
    let intl = Reflect::get(&js_sys::global(), &"Intl".into()).ok()?;
    let ctor: Function = Reflect::get(&intl, &"DateTimeFormat".into())
        .ok()?
        .dyn_into()
        .ok()?;
    let options = Object::new();
    for (k, v) in [
        ("timeZone", tz),
        ("hourCycle", "h23"),
        ("year", "numeric"),
        ("month", "2-digit"),
        ("day", "2-digit"),
        ("hour", "2-digit"),
        ("minute", "2-digit"),
        ("second", "2-digit"),
    ] {
        Reflect::set(&options, &k.into(), &v.into()).ok()?;
    }
    let args = Array::of2(&JsValue::from_str("en-US"), &options);
    // An unknown zone throws a RangeError, caught here.
    let fmt = Reflect::construct(&ctor, &args).ok()?;
    let to_parts: Function = Reflect::get(&fmt, &"formatToParts".into())
        .ok()?
        .dyn_into()
        .ok()?;
    let date = js_sys::Date::new(&JsValue::from_f64(t.timestamp_millis() as f64));
    let parts: Array = to_parts.call1(&fmt, &date).ok()?.dyn_into().ok()?;
    let mut v = std::collections::HashMap::new();
    for p in parts.iter() {
        let kind = Reflect::get(&p, &"type".into()).ok()?.as_string()?;
        let value = Reflect::get(&p, &"value".into()).ok()?.as_string()?;
        v.insert(kind, value);
    }
    let num = |k: &str| v.get(k)?.parse::<u32>().ok();
    chrono::NaiveDate::from_ymd_opt(v.get("year")?.parse().ok()?, num("month")?, num("day")?)?
        .and_hms_opt(num("hour")? % 24, num("minute")?, num("second")?)
}

/// The short date and time (`Mar 25 2014 5:30pm` by default).
pub fn format_datetime(raw: &str) -> String {
    let f = ui_format();
    parse_ts(raw)
        .map(|t| {
            let n = local_naive(t);
            format!(
                "{} {}",
                kitsunarr_domain::moment::format(&n, &f.short_date),
                kitsunarr_domain::moment::format_time(&n, &f.time)
            )
        })
        .unwrap_or_else(|| raw.to_string())
}

/// The time alone, in the chosen time format.
pub fn format_time(raw: &str) -> String {
    parse_ts(raw)
        .map(|t| kitsunarr_domain::moment::format_time(&local_naive(t), &ui_format().time))
        .unwrap_or_default()
}

/// The long date of a day (`Tuesday, March 25 2014`).
pub fn format_long_day(day: chrono::NaiveDate) -> String {
    kitsunarr_domain::moment::format(
        &day.and_hms_opt(0, 0, 0).unwrap_or_default(),
        &ui_format().long_date,
    )
}

/// The short date; with relative dates on, Sonarr's "Today", "Yesterday" and "Tomorrow".
pub fn format_date(raw: &str) -> String {
    let f = ui_format();
    let day = match parse_ts(raw) {
        Some(t) => local_naive(t).date(),
        None => {
            let head = raw.get(..10).unwrap_or(raw);
            match chrono::NaiveDate::parse_from_str(head, "%Y-%m-%d") {
                Ok(d) => d,
                Err(_) => return head.to_string(),
            }
        }
    };
    if f.relative_dates {
        let today = local_naive(Utc::now()).date();
        match (day - today).num_days() {
            0 => return "Today".into(),
            -1 => return "Yesterday".into(),
            1 => return "Tomorrow".into(),
            _ => {}
        }
    }
    kitsunarr_domain::moment::format(&day.and_hms_opt(0, 0, 0).unwrap_or_default(), &f.short_date)
}

/// A calendar week column header (`Tue 3/25`).
pub fn format_week_header(day: chrono::NaiveDate) -> String {
    kitsunarr_domain::moment::format(
        &day.and_hms_opt(0, 0, 0).unwrap_or_default(),
        &ui_format().week_column_header,
    )
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
