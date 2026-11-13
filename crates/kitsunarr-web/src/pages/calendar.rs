//! Agenda-style calendar of upcoming and recent episodes.

use std::collections::BTreeMap;

use chrono::{Duration, Local, NaiveDate, Utc};
use dioxus::prelude::*;
use kitsunarr_domain::api::library::CalendarEntry;

use crate::components::common::{Empty, ErrorBox, Field, Loading, Modal, Switch};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::parse_ts;

#[component]
pub fn Calendar() -> Element {
    let app = use_app();
    let client = use_client();
    let mut offset_weeks = use_signal(|| 0i64);
    let mut unmonitored = use_signal(|| false);
    let mut ical = use_signal(|| false);
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let start = Utc::now() - Duration::days(1) + Duration::weeks(offset_weeks());
        let end = start + Duration::days(15);
        let include = unmonitored();
        let c = client.clone();
        async move {
            let fmt =
                |t: chrono::DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            c.calendar(&fmt(start), &fmt(end), include).await
        }
    });
    let today = Local::now().date_naive();
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Calendar" }
            label { class: "check small", input { r#type: "checkbox", checked: unmonitored(), onchange: move |e: FormEvent| unmonitored.set(e.checked()) } "Show unmonitored" }
            div { class: "row tight",
                button { class: "btn sm", onclick: move |_| offset_weeks -= 2, "‹ Earlier" }
                button { class: "btn sm", onclick: move |_| offset_weeks.set(0), "Today" }
                button { class: "btn sm", onclick: move |_| offset_weeks += 2, "Later ›" }
                button { class: "btn sm", onclick: move |_| ical.set(true), "iCal link" }
            }
        }
        if ical() { ICalLink { on_close: move |_| ical.set(false) } }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "Nothing in this period" } },
            Some(Ok(list)) => {
                let mut days: BTreeMap<NaiveDate, Vec<CalendarEntry>> = BTreeMap::new();
                for e in list {
                    if let Some(t) = parse_ts(&e.air_date_utc) {
                        days.entry(t.with_timezone(&Local).date_naive()).or_default().push(e.clone());
                    }
                }
                rsx! {
                    for (day, entries) in days {
                        section { key: "{day}", class: if day == today { "agenda-day today" } else { "agenda-day" },
                            h3 { {day.format("%A, %d %B %Y").to_string()} if day == today { " · today" } }
                            for e in entries {
                                div { key: "{e.episode_id}", class: "agenda-item",
                                    span { class: "time", {parse_ts(&e.air_date_utc).map(|t| t.with_timezone(&Local).format("%H:%M").to_string()).unwrap_or_default()} }
                                    span { class: if e.has_file { "dot success" } else if e.monitored { "dot accent" } else { "dot" } }
                                    div { class: "grow ellipsis",
                                        Link { to: Route::SeriesDetailPage { id: e.series_id }, "{e.series_title}" }
                                        span { class: "muted small", " S{e.season_number:02}E{e.episode_number:02} " }
                                        span { class: "small", {e.title.clone().unwrap_or_default()} }
                                    }
                                    if e.has_file { span { class: "badge success", "on disk" } }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Sonarr's "iCal link" (Calendar › iCal): the feed's address with its options, for calendar
/// apps (it carries the API key).
#[component]
fn ICalLink(on_close: EventHandler<()>) -> Element {
    let client = use_client();
    let key = use_resource(move || {
        let c = client.clone();
        async move { c.api_key().await.map(|k| k.api_key) }
    });
    let mut past = use_signal(|| 7i64);
    let mut future = use_signal(|| 28i64);
    let mut unmonitored = use_signal(|| false);
    let mut premieres = use_signal(|| false);
    let mut all_day = use_signal(|| false);
    let mut tags = use_signal(String::new);
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    let url = match &*key.read() {
        Some(Ok(k)) => {
            let mut u = format!(
                "{origin}{}/feed/v3/calendar/Kitsunarr.ics?pastDays={}&futureDays={}",
                crate::util::url_base(),
                past(),
                future()
            );
            if unmonitored() {
                u.push_str("&unmonitored=true");
            }
            if premieres() {
                u.push_str("&premieresOnly=true");
            }
            if all_day() {
                u.push_str("&asAllDay=true");
            }
            let t = tags()
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(",");
            if !t.is_empty() {
                u.push_str(&format!("&tags={t}"));
            }
            u.push_str(&format!("&apikey={k}"));
            u
        }
        Some(Err(e)) => e.to_string(),
        None => String::new(),
    };
    let webcal = url
        .replacen("http://", "webcal://", 1)
        .replacen("https://", "webcal://", 1);
    rsx! {
        Modal { title: "iCal link".to_string(), on_close: move |_| on_close.call(()),
            footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| on_close.call(()), "Close" } },
            p { class: "small faint", "Subscribe to this address in a calendar app. It contains your API key: keep it private." }
            div { class: "form-grid",
                Field { label: "Past days", input { class: "input", r#type: "number", "aria-label": "Past days", value: "{past}", oninput: move |e| past.set(e.value().parse().unwrap_or(7)) } }
                Field { label: "Future days", input { class: "input", r#type: "number", "aria-label": "Future days", value: "{future}", oninput: move |e| future.set(e.value().parse().unwrap_or(28)) } }
                Field { label: "Tags (comma separated)", input { class: "input", "aria-label": "Feed tags", value: "{tags}", onchange: move |e: FormEvent| tags.set(e.value()) } }
            }
            Switch { checked: unmonitored(), label: "Include unmonitored", onchange: move |v| unmonitored.set(v) }
            Switch { checked: premieres(), label: "Season premieres only", onchange: move |v| premieres.set(v) }
            Switch { checked: all_day(), label: "Show as all-day events", onchange: move |v| all_day.set(v) }
            Field { label: "Feed address",
                input { class: "input", readonly: true, "aria-label": "iCal feed address", value: "{url}" }
            }
            div { class: "row", a { class: "btn", href: "{webcal}", "Subscribe" } }
        }
    }
}
