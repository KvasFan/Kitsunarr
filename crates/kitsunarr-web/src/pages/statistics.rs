//! Tanukarr's statistics page: the library's series, seasons, episodes and files, broken down
//! by status, type, episode state, quality profile, file quality and tag.

use dioxus::prelude::*;
use kitsunarr_domain::SeriesType;
use kitsunarr_domain::api::views::{LibraryStatistics, StatisticsQuery};

use crate::components::common::{ErrorBox, Field, Loading, StatCard};
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, load_pref, save_pref};

/// A labelled share of a whole (one segment of a stacked bar).
#[derive(Clone, PartialEq)]
struct Slice {
    label: &'static str,
    value: i64,
    tone: &'static str,
}

/// Tanukarr's doughnut charts as a stacked bar with a legend.
#[component]
fn Breakdown(title: &'static str, slices: Vec<Slice>) -> Element {
    let total: i64 = slices.iter().map(|s| s.value).sum();
    rsx! {
        div { class: "card stat-breakdown", role: "group", "aria-label": "{title}",
            h3 { "{title}" }
            div { class: "stack-bar", role: "img", "aria-label": "{title}",
                for s in slices.iter().filter(|s| s.value > 0) {
                    span { key: "{s.label}", class: "seg-{s.tone}", style: "flex-grow: {s.value}", title: "{s.label}: {s.value}" }
                }
            }
            ul { class: "legend",
                for s in slices.iter() {
                    li { key: "{s.label}",
                        span { class: "dot seg-{s.tone}", "aria-hidden": "true" }
                        span { class: "grow", "{s.label}" }
                        strong { "{s.value}" }
                        span { class: "muted xs", {format!(" {:.0}%", if total > 0 { s.value as f64 * 100.0 / total as f64 } else { 0.0 })} }
                    }
                }
            }
        }
    }
}

/// One bar of a bar chart: (label, value, details).
#[derive(Clone, PartialEq)]
struct Bar {
    label: String,
    value: i64,
    detail: String,
}

/// Tanukarr's bar charts as horizontal bars.
#[component]
fn Bars(title: &'static str, unit: &'static str, bars: Vec<Bar>) -> Element {
    let max = bars.iter().map(|b| b.value).max().unwrap_or(0).max(1);
    rsx! {
        div { class: "card stat-bars", role: "group", "aria-label": "{title}",
            h3 { "{title}" }
            if bars.is_empty() {
                p { class: "muted small", "Nothing to show." }
            }
            for b in bars.iter() {
                div { key: "{b.label}", class: "bar-row", title: "{b.detail}",
                    span { class: "bar-label ellipsis", "{b.label}" }
                    span { class: "bar-track",
                        span { class: "bar-fill", style: "width: {b.value as f64 * 100.0 / max as f64}%" }
                    }
                    span { class: "bar-value small", "{b.value} {unit}" }
                    span { class: "bar-detail xs muted", "{b.detail}" }
                }
            }
        }
    }
}

fn percent(part: i64, whole: i64) -> String {
    if whole <= 0 {
        "0%".into()
    } else {
        format!("{:.0}%", part as f64 * 100.0 / whole as f64)
    }
}

#[component]
pub fn Statistics() -> Element {
    let app = use_app();
    let client = use_client();
    // The filters are kept per browser, as Tanukarr's statistics options.
    let mut monitored = use_signal(|| load_pref("statistics.monitored").unwrap_or_default());
    let mut series_type = use_signal(|| load_pref("statistics.seriesType").unwrap_or_default());
    let mut profile = use_signal(|| load_pref("statistics.profile").unwrap_or_default());
    let mut tag = use_signal(|| load_pref("statistics.tag").unwrap_or_default());
    use_effect(move || {
        save_pref("statistics.monitored", &monitored());
        save_pref("statistics.seriesType", &series_type());
        save_pref("statistics.profile", &profile());
        save_pref("statistics.tag", &tag());
    });
    let c0 = client.clone();
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let q = StatisticsQuery {
            monitored: match monitored().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            series_type: series_type().parse::<SeriesType>().ok(),
            quality_profile_id: profile().parse().ok(),
            tag_id: tag().parse().ok(),
        };
        let c = c0.clone();
        async move { c.statistics(&q).await }
    });
    let c1 = client.clone();
    let profiles = use_resource(move || {
        let c = c1.clone();
        async move { c.quality_profiles().await.unwrap_or_default() }
    });
    let tags = use_resource(move || {
        let c = client.clone();
        async move { c.tags().await.unwrap_or_default() }
    });
    let filtered = !monitored().is_empty()
        || !series_type().is_empty()
        || !profile().is_empty()
        || !tag().is_empty();
    rsx! {
        div { class: "page-head", h1 { class: "grow", "Statistics" }
            if filtered {
                button { class: "btn sm ghost", onclick: move |_| { monitored.set(String::new()); series_type.set(String::new()); profile.set(String::new()); tag.set(String::new()); }, "Clear filters" }
            }
        }
        div { class: "toolbar",
            Field { label: "Monitored",
                select { class: "select", "aria-label": "Monitored", onchange: move |e: FormEvent| monitored.set(e.value()),
                    option { value: "", selected: monitored().is_empty(), "All series" }
                    option { value: "true", selected: monitored() == "true", "Monitored" }
                    option { value: "false", selected: monitored() == "false", "Unmonitored" }
                }
            }
            Field { label: "Series type",
                select { class: "select", "aria-label": "Series type", onchange: move |e: FormEvent| series_type.set(e.value()),
                    option { value: "", selected: series_type().is_empty(), "All types" }
                    option { value: "standard", selected: series_type() == "standard", "Standard" }
                    option { value: "daily", selected: series_type() == "daily", "Daily" }
                    option { value: "anime", selected: series_type() == "anime", "Anime" }
                }
            }
            Field { label: "Quality profile",
                select { class: "select", "aria-label": "Quality profile", onchange: move |e: FormEvent| profile.set(e.value()),
                    option { value: "", selected: profile().is_empty(), "All profiles" }
                    if let Some(list) = &*profiles.read() {
                        for p in list.iter() { option { key: "{p.id}", value: "{p.id}", selected: profile() == p.id.to_string(), "{p.name}" } }
                    }
                }
            }
            Field { label: "Tag",
                select { class: "select", "aria-label": "Tag", onchange: move |e: FormEvent| tag.set(e.value()),
                    option { value: "", selected: tag().is_empty(), "All tags" }
                    if let Some(list) = &*tags.read() {
                        for t in list.iter() { option { key: "{t.id}", value: "{t.id}", selected: tag() == t.id.to_string(), "{t.label}" } }
                    }
                }
            }
        }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(s)) => rsx! { StatisticsBody { s: s.clone() } },
        }
    }
}

#[component]
fn StatisticsBody(s: LibraryStatistics) -> Element {
    let wanted = s.downloaded_episode_count + s.missing_episode_count;
    let average = if s.episode_file_count > 0 {
        s.size_on_disk / s.episode_file_count
    } else {
        0
    };
    let other = (s.total_episode_count
        - s.downloaded_episode_count
        - s.missing_episode_count
        - s.unaired_episode_count)
        .max(0);
    let status = vec![
        Slice {
            label: "Continuing",
            value: s.continuing_series_count,
            tone: "success",
        },
        Slice {
            label: "Ended",
            value: s.ended_series_count,
            tone: "info",
        },
        Slice {
            label: "Upcoming",
            value: s.upcoming_series_count,
            tone: "warning",
        },
        Slice {
            label: "Deleted",
            value: s.deleted_series_count,
            tone: "danger",
        },
    ];
    let types = vec![
        Slice {
            label: "Standard",
            value: s.standard_series_count,
            tone: "info",
        },
        Slice {
            label: "Daily",
            value: s.daily_series_count,
            tone: "warning",
        },
        Slice {
            label: "Anime",
            value: s.anime_series_count,
            tone: "accent",
        },
    ];
    let episodes = vec![
        Slice {
            label: "Downloaded",
            value: s.downloaded_episode_count,
            tone: "success",
        },
        Slice {
            label: "Missing",
            value: s.missing_episode_count,
            tone: "danger",
        },
        Slice {
            label: "Unaired",
            value: s.unaired_episode_count,
            tone: "accent",
        },
        Slice {
            label: "Unmonitored",
            value: other,
            tone: "muted",
        },
    ];
    let profiles: Vec<Bar> = s
        .quality_profile_statistics
        .iter()
        .map(|p| Bar {
            label: p.name.clone(),
            value: p.series_count,
            detail: format!(
                "{} files · {}",
                p.episode_file_count,
                format_bytes(p.size_on_disk)
            ),
        })
        .collect();
    let qualities: Vec<Bar> = s
        .quality_statistics
        .iter()
        .map(|q| Bar {
            label: q.quality.name.clone(),
            value: q.episode_file_count,
            detail: format_bytes(q.size_on_disk),
        })
        .collect();
    let tags: Vec<Bar> = s
        .tag_statistics
        .iter()
        .map(|t| Bar {
            label: t.label.clone(),
            value: t.series_count,
            detail: format!(
                "{} files · {}",
                t.episode_file_count,
                format_bytes(t.size_on_disk)
            ),
        })
        .collect();
    rsx! {
        div { class: "grid-stats", "aria-label": "Summary",
            StatCard { label: "Series", value: s.series_count.to_string() }
            StatCard { label: "Monitored series", value: s.monitored_series_count.to_string() }
            StatCard { label: "Completed series", value: s.completed_series_count.to_string() }
            StatCard { label: "Seasons", value: format!("{} ({} completed)", s.season_count, s.completed_season_count) }
            StatCard { label: "Episodes", value: format!("{} ({} monitored)", s.total_episode_count, s.monitored_episode_count) }
            StatCard { label: "Episode files", value: format!("{} ({} downloaded)", s.episode_file_count, percent(s.downloaded_episode_count, s.total_episode_count)) }
            StatCard { label: "Missing episodes", value: format!("{} ({} completed)", s.missing_episode_count, percent(s.downloaded_episode_count, wanted)), accent: s.missing_episode_count > 0 }
            StatCard { label: "Size on disk", value: format!("{} ({} per episode)", format_bytes(s.size_on_disk), format_bytes(average)) }
        }
        div { class: "stat-grid",
            Breakdown { title: "Series status", slices: status }
            Breakdown { title: "Series type", slices: types }
            Breakdown { title: "Episodes", slices: episodes }
            Bars { title: "Quality profiles", unit: "series", bars: profiles }
            Bars { title: "File qualities", unit: "files", bars: qualities }
            Bars { title: "Tags", unit: "series", bars: tags }
        }
    }
}
