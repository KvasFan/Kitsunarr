//! Calendar (Sonarr's and Tanukarr's `Calendar`): month, week, forecast, day and agenda views
//! of the episodes airing in a period, with "Search for missing" and the iCal link.

use std::collections::BTreeMap;

use chrono::{Datelike, Days, Local, Months, NaiveDate, TimeZone, Utc};
use dioxus::prelude::*;
use kitsunarr_domain::api::library::CalendarEntry;
use kitsunarr_domain::api::wanted::{WantedList, WantedSearchRequest};

use crate::components::common::{Empty, ErrorBox, Field, Loading, Modal, Switch};
use crate::route::Route;
use crate::state::{ToastLevel, use_app, use_client};
use crate::util::{load_pref, parse_ts, save_pref};

const VIEW_PREF: &str = "calendar.view";
const UNMONITORED_PREF: &str = "calendar.unmonitored";
/// Tanukarr's forecast shows yesterday and the following days (`dayCount`, 7 on a desktop).
const FORECAST_DAYS: u64 = 7;

/// Tanukarr's calendar views (`calendarViews.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Month,
    Week,
    Forecast,
    Day,
    Agenda,
}

impl View {
    const ALL: [View; 5] = [
        View::Month,
        View::Week,
        View::Forecast,
        View::Day,
        View::Agenda,
    ];

    fn key(self) -> &'static str {
        match self {
            View::Month => "month",
            View::Week => "week",
            View::Forecast => "forecast",
            View::Day => "day",
            View::Agenda => "agenda",
        }
    }

    fn label(self) -> &'static str {
        match self {
            View::Month => "Month",
            View::Week => "Week",
            View::Forecast => "Forecast",
            View::Day => "Day",
            View::Agenda => "Agenda",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == value)
    }
}

/// The first day of the week containing `day` (`first`: 0 Sunday, 1 Monday, 6 Saturday).
fn week_start(day: NaiveDate, first: u8) -> NaiveDate {
    let from_sunday = day.weekday().num_days_from_sunday();
    let back = (from_sunday + 7 - u32::from(first % 7)) % 7;
    day - Days::new(u64::from(back))
}

/// Tanukarr's `getDates`: the days a view shows around `time` (inclusive).
pub fn range(view: View, time: NaiveDate, first: u8) -> (NaiveDate, NaiveDate) {
    match view {
        View::Day => (time, time),
        View::Week => {
            let start = week_start(time, first);
            (start, start + Days::new(6))
        }
        View::Forecast => (time - Days::new(1), time + Days::new(FORECAST_DAYS - 2)),
        View::Month => {
            let first_of_month = time.with_day(1).unwrap_or(time);
            let last_of_month = (first_of_month + Months::new(1)) - Days::new(1);
            (
                week_start(first_of_month, first),
                week_start(last_of_month, first) + Days::new(6),
            )
        }
        View::Agenda => (time - Days::new(1), time + Months::new(1)),
    }
}

/// Tanukarr's `goToPreviousRange`/`goToNextRange`.
pub fn step(view: View, time: NaiveDate, forward: bool) -> NaiveDate {
    let shift = |t: NaiveDate, days: u64| {
        if forward {
            t + Days::new(days)
        } else {
            t - Days::new(days)
        }
    };
    match view {
        View::Day => shift(time, 1),
        View::Week => shift(time, 7),
        View::Forecast => shift(time, FORECAST_DAYS),
        View::Month | View::Agenda => {
            if forward {
                time + Months::new(1)
            } else {
                time - Months::new(1)
            }
        }
    }
}

/// The header's title for the shown period.
fn period_title(view: View, time: NaiveDate, start: NaiveDate, end: NaiveDate) -> String {
    match view {
        View::Month => time.format("%B %Y").to_string(),
        View::Day => time.format("%A, %d %B %Y").to_string(),
        _ if start.year() != end.year() => {
            format!("{} – {}", start.format("%d %b %Y"), end.format("%d %b %Y"))
        }
        _ if start.month() != end.month() => {
            format!("{} – {}", start.format("%d %b"), end.format("%d %b %Y"))
        }
        _ => format!("{} – {}", start.format("%d"), end.format("%d %B %Y")),
    }
}

/// Tanukarr's `getStatusStyle`: downloaded, unmonitored, unaired or missing.
fn status(e: &CalendarEntry, now: chrono::DateTime<Utc>) -> (&'static str, &'static str) {
    if e.has_file {
        ("downloaded", "On disk")
    } else if !e.monitored {
        ("unmonitored", "Unmonitored")
    } else if parse_ts(&e.air_date_utc).is_none_or(|t| t > now) {
        ("unaired", "Not aired yet")
    } else {
        ("missing", "Missing")
    }
}

/// `CalendarMissingEpisodeSearchButton`: the shown episodes that aired and have no file.
pub fn missing_episode_ids(entries: &[CalendarEntry], now: chrono::DateTime<Utc>) -> Vec<i64> {
    entries
        .iter()
        .filter(|e| !e.has_file && parse_ts(&e.air_date_utc).is_some_and(|t| t <= now))
        .map(|e| e.episode_id)
        .collect()
}

fn local_start(day: NaiveDate) -> chrono::DateTime<Utc> {
    Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default())
        .earliest()
        .map_or_else(
            || Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default()),
            |t| t.with_timezone(&Utc),
        )
}

#[component]
pub fn Calendar() -> Element {
    let app = use_app();
    let client = use_client();
    let mut view = use_signal(|| {
        load_pref(VIEW_PREF)
            .and_then(|v| View::parse(&v))
            .unwrap_or(View::Week)
    });
    let mut time = use_signal(|| Local::now().date_naive());
    let mut unmonitored = use_signal(|| load_pref(UNMONITORED_PREF).as_deref() == Some("true"));
    let mut ical = use_signal(|| false);
    let mut searching = use_signal(|| false);
    use_effect(move || {
        save_pref(VIEW_PREF, view().key());
        save_pref(
            UNMONITORED_PREF,
            if unmonitored() { "true" } else { "false" },
        );
    });
    let cg = client.clone();
    let general = use_resource(move || {
        let c = cg.clone();
        async move { c.general().await.map(|g| g.first_day_of_week).unwrap_or(1) }
    });
    let first_day = general.read().unwrap_or(1);
    let (start, end) = range(view(), time(), first_day);
    let cd = client.clone();
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let first = general.read().unwrap_or(1);
        let (start, end) = range(view(), time(), first);
        let include = unmonitored();
        let c = cd.clone();
        async move {
            let fmt =
                |t: chrono::DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            c.calendar(
                &fmt(local_start(start)),
                &fmt(local_start(end + Days::new(1))),
                include,
            )
            .await
        }
    });
    let now = Utc::now();
    let today = Local::now().date_naive();
    let entries: Vec<CalendarEntry> = match &*data.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    let missing = missing_episode_ids(&entries, now);
    let c_search = client.clone();
    let search_missing = move |_| {
        let ids = missing_episode_ids(
            &match &*data.read() {
                Some(Ok(list)) => list.clone(),
                _ => Vec::new(),
            },
            Utc::now(),
        );
        if ids.is_empty() {
            return;
        }
        let c = c_search.clone();
        searching.set(true);
        spawn(async move {
            match c
                .search_wanted(&WantedSearchRequest {
                    list: WantedList::Missing,
                    episode_ids: ids,
                    monitored: None,
                })
                .await
            {
                Ok(started) => app.toast(ToastLevel::Info, started.message),
                Err(e) => app.error(&e),
            }
            searching.set(false);
        });
    };
    let mut by_day: BTreeMap<NaiveDate, Vec<CalendarEntry>> = BTreeMap::new();
    for e in &entries {
        if let Some(t) = parse_ts(&e.air_date_utc) {
            by_day
                .entry(crate::util::local_naive(t).date())
                .or_default()
                .push(e.clone());
        }
    }
    let days: Vec<NaiveDate> = start.iter_days().take_while(|d| *d <= end).collect();
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Calendar" }
            label { class: "check small", input { r#type: "checkbox", checked: unmonitored(), onchange: move |e: FormEvent| unmonitored.set(e.checked()) } "Show unmonitored" }
            button { class: "btn sm", disabled: missing.is_empty() || searching(),
                title: "Search for the episodes shown that aired and have no file",
                onclick: search_missing,
                if searching() { "Searching…" } else { "Search for missing" }
            }
            button { class: "btn sm", onclick: move |_| ical.set(true), "iCal link" }
        }
        div { class: "toolbar calendar-header",
            div { class: "row tight",
                button { class: "btn sm", "aria-label": "Previous period", onclick: move |_| time.set(step(view(), time(), false)), "‹" }
                button { class: "btn sm", onclick: move |_| time.set(Local::now().date_naive()), "Today" }
                button { class: "btn sm", "aria-label": "Next period", onclick: move |_| time.set(step(view(), time(), true)), "›" }
            }
            h2 { class: "grow calendar-title", {period_title(view(), time(), start, end)} }
            div { class: "seg", role: "group", "aria-label": "Calendar view",
                for v in View::ALL {
                    button { key: "{v.key()}", class: if view() == v { "on" } else { "" }, "aria-pressed": "{view() == v}", onclick: move |_| view.set(v), "{v.label()}" }
                }
            }
        }
        if ical() { ICalLink { on_close: move |_| ical.set(false) } }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(_)) if view() == View::Agenda => rsx! {
                if by_day.is_empty() {
                    Empty { title: "Nothing in this period" }
                }
                for (day, list) in by_day.clone() {
                    section { key: "{day}", class: if day == today { "agenda-day today" } else { "agenda-day" },
                        h3 { {crate::util::format_long_day(day)} if day == today { " · today" } }
                        for e in list {
                            AgendaItem { key: "{e.episode_id}", entry: e.clone() }
                        }
                    }
                }
            },
            Some(Ok(_)) => rsx! {
                div { class: if view() == View::Day { "calendar-days single" } else { "calendar-days" }, role: "grid", "aria-label": "Calendar",
                    if view() != View::Day {
                        for d in days.iter().take(7) {
                            div { key: "h{d}", class: "calendar-dow", role: "columnheader",
                                if view() == View::Week { {crate::util::format_week_header(*d)} } else { {d.format("%a").to_string()} }
                            }
                        }
                    }
                    for d in days.iter() {
                        {
                            let list = by_day.get(d).cloned().unwrap_or_default();
                            let outside = view() == View::Month && d.month() != time().month();
                            let class = match (*d == today, outside) {
                                (true, _) => "calendar-day today",
                                (false, true) => "calendar-day outside",
                                _ => "calendar-day",
                            };
                            rsx! {
                                div { key: "{d}", class, role: "gridcell", "aria-label": "{d.format(\"%A %d %B\")}",
                                    div { class: "calendar-date",
                                        if view() == View::Day { {d.format("%A, %d %B").to_string()} } else { {d.format("%d").to_string()} }
                                    }
                                    for e in list.iter() {
                                        CalendarEvent { key: "{e.episode_id}", entry: e.clone(), now }
                                    }
                                }
                            }
                        }
                    }
                }
                if entries.is_empty() {
                    p { class: "muted small", "Nothing airs in this period." }
                }
            },
        }
        div { class: "row tight calendar-legend small", "aria-label": "Legend",
            span { class: "cal-status downloaded", "On disk" }
            span { class: "cal-status missing", "Missing" }
            span { class: "cal-status unaired", "Not aired yet" }
            span { class: "cal-status unmonitored", "Unmonitored" }
        }
    }
}

/// One episode in a day cell (Tanukarr's `CalendarEvent`).
#[component]
fn CalendarEvent(entry: CalendarEntry, now: chrono::DateTime<Utc>) -> Element {
    let (class, label) = status(&entry, now);
    let time = crate::util::format_time(&entry.air_date_utc);
    rsx! {
        Link { to: Route::SeriesDetailPage { id: entry.series_id }, class: "cal-event cal-status {class}", title: "{label}",
            span { class: "ellipsis strong", "{entry.series_title}" }
            span { class: "xs",
                "S{entry.season_number:02}E{entry.episode_number:02} · {time}"
            }
            if let Some(t) = entry.title.as_ref() { span { class: "xs muted ellipsis", "{t}" } }
        }
    }
}

/// One episode in the agenda.
#[component]
fn AgendaItem(entry: CalendarEntry) -> Element {
    let e = entry;
    rsx! {
        div { class: "agenda-item",
            span { class: "time", {crate::util::format_time(&e.air_date_utc)} }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn view_ranges_follow_tanukarr() {
        // Wednesday 18 November 2026.
        let t = d(2026, 11, 18);
        assert_eq!(range(View::Day, t, 1), (t, t));
        assert_eq!(range(View::Week, t, 1), (d(2026, 11, 16), d(2026, 11, 22)));
        assert_eq!(range(View::Week, t, 0), (d(2026, 11, 15), d(2026, 11, 21)));
        assert_eq!(range(View::Week, t, 6), (d(2026, 11, 14), d(2026, 11, 20)));
        assert_eq!(
            range(View::Forecast, t, 1),
            (d(2026, 11, 17), d(2026, 11, 23))
        );
        assert_eq!(range(View::Month, t, 1), (d(2026, 10, 26), d(2026, 12, 6)));
        assert_eq!(
            range(View::Agenda, t, 1),
            (d(2026, 11, 17), d(2026, 12, 18))
        );
        assert_eq!(step(View::Week, t, false), d(2026, 11, 11));
        assert_eq!(step(View::Month, d(2026, 1, 31), true), d(2026, 2, 28));
        assert_eq!(step(View::Forecast, t, true), d(2026, 11, 25));
        assert_eq!(
            period_title(View::Week, t, d(2026, 11, 30), d(2026, 12, 6)),
            "30 Nov – 06 Dec 2026"
        );
    }

    #[test]
    fn missing_episodes_aired_without_a_file() {
        let entry = |id, air: &str, has_file, monitored| CalendarEntry {
            episode_id: id,
            series_id: 1,
            series_title: "Show".into(),
            kind: kitsunarr_domain::MediaKind::Series,
            season_number: 1,
            episode_number: id as i32,
            absolute_episode_number: None,
            title: None,
            air_date_utc: air.into(),
            monitored,
            has_file,
        };
        let now = parse_ts("2026-11-18T12:00:00Z").unwrap();
        let list = vec![
            entry(1, "2026-11-17T02:00:00Z", false, true),
            entry(2, "2026-11-17T02:00:00Z", true, true),
            entry(3, "2026-11-19T02:00:00Z", false, true),
            entry(4, "2026-11-16T02:00:00Z", false, false),
        ];
        assert_eq!(missing_episode_ids(&list, now), [1, 4]);
        assert_eq!(status(&list[0], now).0, "missing");
        assert_eq!(status(&list[1], now).0, "downloaded");
        assert_eq!(status(&list[2], now).0, "unaired");
        assert_eq!(status(&list[3], now).0, "unmonitored");
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
