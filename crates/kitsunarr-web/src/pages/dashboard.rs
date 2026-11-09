//! Dashboard: library statistics, health, upcoming episodes and recent activity.

use dioxus::prelude::*;
use kitsunarr_domain::api::system::HealthLevel;

use crate::components::common::{Badge, ErrorBox, Loading, StatCard};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, format_datetime, relative};

#[component]
pub fn Dashboard() -> Element {
    let app = use_app();
    let client = use_client();
    let c1 = client.clone();
    let stats = use_resource(move || {
        let _ = (app.library_rev)();
        let c = c1.clone();
        async move { c.library_stats().await }
    });
    let c2 = client.clone();
    let health = use_resource(move || {
        let _ = (app.health_rev)();
        let _ = (app.settings_rev)();
        let c = c2.clone();
        async move { c.health().await }
    });
    let c3 = client.clone();
    let upcoming = use_resource(move || {
        let _ = (app.library_rev)();
        let c = c3.clone();
        async move {
            let now = chrono::Utc::now();
            let fmt = |t: chrono::DateTime<chrono::Utc>| {
                t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            };
            c.calendar(
                &fmt(now - chrono::Duration::hours(6)),
                &fmt(now + chrono::Duration::days(7)),
                false,
            )
            .await
        }
    });
    let c4 = client.clone();
    let recent = use_resource(move || {
        let _ = (app.library_rev)();
        let c = c4.clone();
        async move { c.history(1, 8, None).await }
    });

    rsx! {
        div { class: "page-head", h1 { "Dashboard" } }
        match &*stats.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(s)) => rsx! {
                if s.series + s.anime + s.movies == 0 {
                    div { class: "alert info",
                        div { class: "grow",
                            strong { "Your library is empty. " }
                            "Coming from Sonarr or Tanukarr? Import your existing series, settings and history in a few steps."
                        }
                        Link { to: Route::ImportWizard {}, class: "btn primary", "Start migration" }
                    }
                }
                div { class: "grid-stats",
                    StatCard { label: "TV series", value: s.series.to_string() }
                    StatCard { label: "Anime", value: s.anime.to_string() }
                    StatCard { label: "Movies", value: s.movies.to_string() }
                    StatCard { label: "Collections", value: s.collections.to_string() }
                    StatCard { label: "Episodes on disk", value: format!("{} / {}", s.episode_files, s.episodes) }
                    StatCard { label: "Missing (monitored)", value: s.monitored_missing_episodes.to_string(), accent: s.monitored_missing_episodes > 0 }
                    StatCard { label: "Size on disk", value: format_bytes(s.size_on_disk) }
                }
            },
        }
        div { class: "two-col",
            div {
                div { class: "card",
                    div { class: "card-head", h3 { class: "grow", "Upcoming 7 days" } Link { to: Route::Calendar {}, class: "small", "Calendar →" } }
                    match &*upcoming.read() {
                        None => rsx! { Loading {} },
                        Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                        Some(Ok(list)) if list.is_empty() => rsx! { p { class: "muted", "Nothing airs in the next seven days." } },
                        Some(Ok(list)) => rsx! {
                            for e in list.iter().take(12) {
                                div { key: "{e.episode_id}", class: "agenda-item",
                                    span { class: if e.has_file { "dot success" } else if e.monitored { "dot accent" } else { "dot" } }
                                    div { class: "grow ellipsis",
                                        Link { to: Route::SeriesDetailPage { id: e.series_id }, "{e.series_title}" }
                                        span { class: "muted small", " S{e.season_number:02}E{e.episode_number:02} " }
                                        span { class: "small", {e.title.clone().unwrap_or_default()} }
                                    }
                                    span { class: "xs faint", {relative(&e.air_date_utc)} }
                                }
                            }
                        },
                    }
                }
                div { class: "card",
                    div { class: "card-head", h3 { class: "grow", "Recent activity" } Link { to: Route::Activity {}, class: "small", "History →" } }
                    match &*recent.read() {
                        None => rsx! { Loading {} },
                        Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                        Some(Ok(page)) if page.records.is_empty() => rsx! { p { class: "muted", "No activity yet." } },
                        Some(Ok(page)) => rsx! {
                            for h in page.records.iter() {
                                div { key: "{h.id}", class: "agenda-item",
                                    Badge { text: h.event_type.clone() }
                                    span { class: "grow ellipsis small", title: "{h.source_title}", "{h.source_title}" }
                                    span { class: "xs faint", {format_datetime(&h.date)} }
                                }
                            }
                        },
                    }
                }
            }
            div { class: "card",
                div { class: "card-head", h3 { class: "grow", "Health" } Link { to: Route::SystemStatusPage {}, class: "small", "System →" } }
                match &*health.read() {
                    None => rsx! { Loading {} },
                    Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                    Some(Ok(list)) if list.is_empty() => rsx! { p { class: "muted", "✓ No issues detected." } },
                    Some(Ok(list)) => rsx! {
                        for (i, h) in list.iter().enumerate() {
                            div {
                                key: "{i}",
                                class: match h.level { HealthLevel::Error => "alert danger", HealthLevel::Warning => "alert warning", _ => "alert info" },
                                span { class: "small", "{h.message}" }
                            }
                        }
                    },
                }
            }
        }
    }
}
