//! Tanukarr's "Test parsing" (`ParseModal`): what Kitsunarr reads from a release title and
//! the library episodes it maps to; and System › Logs' log files.

use dioxus::prelude::*;
use kitsunarr_domain::api::views::{LogFileItem, ParseResult};

use crate::components::common::{Badge, Empty, ErrorBox, Loading, Modal};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, format_datetime};

#[component]
pub fn ParsePage() -> Element {
    let app = use_app();
    let client = use_client();
    let mut title = use_signal(String::new);
    let mut result = use_signal(|| None::<ParseResult>);
    let mut busy = use_signal(|| false);
    let run = move |e: FormEvent| {
        e.prevent_default();
        let t = title().trim().to_string();
        if t.is_empty() {
            return;
        }
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            match c.parse(&t).await {
                Ok(r) => result.set(Some(r)),
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    rsx! {
        div { class: "page-head", h1 { "Test parsing" } }
        form { class: "card", onsubmit: run,
            p { class: "muted small", "Paste a release title or file name to see how Kitsunarr reads it and which episodes it matches." }
            div { class: "row",
                input { class: "input mono grow", "aria-label": "Release title", placeholder: "Series.Title.S01E01.1080p.WEB-DL.x264-GROUP", value: "{title}", oninput: move |e| title.set(e.value()) }
                button { class: "btn primary", r#type: "submit", disabled: busy() || title().trim().is_empty(), "Parse" }
            }
        }
        if let Some(r) = result() {
            ParseDetails { r }
        }
    }
}

fn numbers(list: &[i32]) -> String {
    if list.is_empty() {
        "—".into()
    } else {
        list.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn yes_no(v: bool) -> &'static str {
    if v { "Yes" } else { "No" }
}

#[component]
fn ParseDetails(r: ParseResult) -> Element {
    let Some(p) = r.parsed.clone() else {
        return rsx! { div { class: "card", role: "status", p { "Kitsunarr could not read this title." } } };
    };
    rsx! {
        div { class: "card", role: "region", "aria-label": "Parse result",
            h3 { "Release" }
            dl { class: "kv",
                dt { "Series title" } dd { "{p.series_title}" }
                dt { "Season" } dd { {numbers(&p.season_numbers)} }
                dt { "Episodes" } dd { {numbers(&p.episode_numbers)} }
                dt { "Absolute episodes" } dd { {numbers(&p.absolute_episode_numbers)} }
                dt { "Air date" } dd { {p.air_date.clone().unwrap_or_else(|| "—".into())} }
                dt { "Quality" } dd {
                    "{p.quality}"
                    if p.revision > 1 { " v{p.revision}" }
                    if p.is_repack { " " Badge { text: "repack" } }
                }
                dt { "Languages" } dd { {if p.languages.is_empty() { "—".to_string() } else { p.languages.join(", ") }} }
                dt { "Release group" } dd { {p.release_group.clone().unwrap_or_else(|| "—".into())} }
                if let Some(h) = p.release_hash.as_ref() { dt { "Release hash" } dd { class: "mono", "{h}" } }
                dt { "Full season" } dd { {yes_no(p.full_season)} }
                dt { "Multi-season" } dd { {yes_no(p.is_multi_season)} }
                dt { "Special" } dd { {yes_no(p.special || p.is_possible_special_episode)} }
                dt { "Daily" } dd { {yes_no(p.is_daily)} }
                dt { "Absolute numbering" } dd { {yes_no(p.is_absolute_numbering)} }
            }
        }
        div { class: "card",
            h3 { "Library match" }
            match r.series.as_ref() {
                None => rsx! { p { class: "muted", "No series in the library matches this title." } },
                Some(s) => rsx! {
                    p { Link { to: Route::SeriesDetailPage { id: s.id }, "{s.title}" } if let Some(y) = s.year { span { class: "muted", " ({y})" } } }
                    if r.episodes.is_empty() {
                        p { class: "muted small", "No episode of the series matches." }
                    } else {
                        div { class: "table-wrap",
                            table { class: "table",
                                thead { tr { th { "Episode" } th { "Title" } th { "Air date" } th { "File" } } }
                                tbody {
                                    for e in r.episodes.iter() {
                                        tr { key: "{e.id}",
                                            td { {format!("S{:02}E{:02}", e.season_number, e.episode_number)} if let Some(a) = e.absolute_episode_number { span { class: "muted xs", " ({a})" } } }
                                            td { {e.title.clone().unwrap_or_default()} }
                                            td { {e.air_date.clone().unwrap_or_default()} }
                                            td { if e.has_file { Badge { text: "on disk", tone: "success" } } else { Badge { text: "missing", tone: "warning" } } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// System › Logs: the rolling log files of the data folder, to read or download.
#[component]
pub fn LogFiles() -> Element {
    let client = use_client();
    let mut open = use_signal(|| None::<String>);
    let c0 = client.clone();
    let mut files = use_resource(move || {
        let c = c0.clone();
        async move { c.log_files().await }
    });
    rsx! {
        div { class: "card", role: "region", "aria-label": "Log files",
            div { class: "card-head",
                h3 { class: "grow", "Log files" }
                button { class: "btn sm", onclick: move |_| files.restart(), "Refresh" }
            }
            p { class: "muted small", "Kitsunarr writes its log to files in the data folder, rolled at 1 MiB. The log level is set in Settings › General." }
            match &*files.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "No log files yet" } },
                Some(Ok(list)) => rsx! { LogFileTable { list: list.clone(), on_open: move |n: String| open.set(Some(n)) } },
            }
        }
        if let Some(name) = open() {
            LogFileViewer { name, on_close: move |_| open.set(None) }
        }
    }
}

#[component]
fn LogFileTable(list: Vec<LogFileItem>, on_open: EventHandler<String>) -> Element {
    let client = use_client();
    rsx! {
        div { class: "table-wrap",
            table { class: "table",
                thead { tr { th { "File" } th { "Last written" } th { class: "num", "Size" } th { "" } } }
                tbody {
                    for f in list.iter() {
                        tr { key: "{f.filename}",
                            td { class: "mono", "{f.filename}" }
                            td { {format_datetime(&f.last_write_time)} }
                            td { class: "num", {format_bytes(f.size as i64)} }
                            td { class: "shrink",
                                div { class: "row tight",
                                    button { class: "btn sm", onclick: { let n = f.filename.clone(); move |_| on_open.call(n.clone()) }, "View" }
                                    a { class: "btn sm", href: "{client.log_file_download_url(&f.filename)}", download: "{f.filename}", "Download" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn LogFileViewer(name: String, on_close: EventHandler<()>) -> Element {
    let client = use_client();
    let n = name.clone();
    let text = use_resource(move || {
        let c = client.clone();
        let n = n.clone();
        async move { c.log_file_text(&n).await }
    });
    rsx! {
        Modal { title: name.clone(), wide: true, on_close: move |_| on_close.call(()),
            match &*text.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(t)) => rsx! { pre { class: "log log-file", "{t}" } },
            }
        }
    }
}
