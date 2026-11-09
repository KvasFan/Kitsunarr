//! Agenda-style calendar of upcoming and recent episodes.

use std::collections::BTreeMap;

use chrono::{Duration, Local, NaiveDate, Utc};
use dioxus::prelude::*;
use kitsunarr_domain::api::library::CalendarEntry;

use crate::components::common::{Empty, ErrorBox, Loading};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::parse_ts;

#[component]
pub fn Calendar() -> Element {
    let app = use_app();
    let client = use_client();
    let mut offset_weeks = use_signal(|| 0i64);
    let mut unmonitored = use_signal(|| false);
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
            }
        }
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
