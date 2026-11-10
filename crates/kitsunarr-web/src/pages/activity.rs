//! Activity: download queue and history.

use dioxus::prelude::*;

use crate::components::common::{Badge, Empty, ErrorBox, Loading, Pagination};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, format_datetime};

fn time_left(secs: Option<i64>) -> String {
    match secs {
        None => "—".into(),
        Some(0) => "done".into(),
        Some(s) if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        Some(s) if s < 86_400 => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
        Some(s) => format!("{}d {}h", s / 86_400, (s % 86_400) / 3600),
    }
}

fn status_tone(status: &str) -> &'static str {
    match status {
        "completed" => "success",
        "warning" => "warning",
        "failed" => "danger",
        "downloading" => "info",
        _ => "",
    }
}

#[component]
pub fn Queue() -> Element {
    let client = use_client();
    let mut tick = use_signal(|| 0u64);
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(5000).await;
            tick += 1;
        }
    });
    let queue = use_resource(move || {
        let _ = tick();
        let c = client.clone();
        async move { c.queue().await }
    });
    let read = queue.read();
    let body = match &*read {
        None => rsx! { Loading { label: "Reading the download clients…" } },
        Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
        Some(Ok(q)) => rsx! {
            for f in q.failures.iter() {
                div { key: "{f.download_client}", class: "alert warning", role: "status",
                    div { class: "grow", strong { "{f.download_client}: " } "{f.message}" }
                }
            }
            if q.clients == 0 {
                Empty { title: "No download client to read",
                    p { "Add a qBittorrent download client and enable it. Other clients are stored but not implemented yet." }
                    Link { to: Route::SettingsDownloadClients {}, class: "btn", "Download clients" }
                }
            } else if q.items.is_empty() {
                Empty { title: "Nothing downloading",
                    p { "Grab a release from interactive search; downloads in the client's TV category show up here." }
                }
            } else {
                div { class: "table-wrap",
                    table { class: "table queue",
                        thead { tr {
                            th { "Series" } th { "Episodes" } th { "Title" } th { "Quality" }
                            th { "Status" } th { "Progress" } th { class: "num", "Time left" } th { "Client" }
                        } }
                        tbody {
                            for item in q.items.iter() {
                                {
                                    let done = if item.size > 0 { 100 - (item.size_left * 100 / item.size).clamp(0, 100) } else { 0 };
                                    rsx! {
                                        tr { key: "{item.download_id}",
                                            td {
                                                match item.series_id {
                                                    Some(id) => rsx! { Link { to: Route::SeriesDetailPage { id }, "{item.series_title.clone().unwrap_or_default()}" } },
                                                    None => rsx! { span { class: "muted", "Unknown" } },
                                                }
                                            }
                                            td { class: "small", {item.episodes.join(", ")} }
                                            td { class: "release-title",
                                                span { class: "mono small", "{item.title}" }
                                                if !item.messages.is_empty() {
                                                    ul { class: "messages small",
                                                        for (i, m) in item.messages.iter().enumerate() { li { key: "{i}", "{m}" } }
                                                    }
                                                }
                                            }
                                            td { if let Some(qn) = &item.quality { span { class: "badge", "{qn}" } } }
                                            td { Badge { text: item.status.clone(), tone: status_tone(&item.status) } }
                                            td {
                                                div { class: "progress", role: "progressbar", "aria-valuenow": "{done}", "aria-valuemin": "0", "aria-valuemax": "100", "aria-label": "Progress of {item.title}",
                                                    div { class: "bar", style: "width: {done}%" }
                                                }
                                                span { class: "small muted", "{done}% of {format_bytes(item.size)}" }
                                            }
                                            td { class: "num small", {time_left(item.time_left)} }
                                            td { class: "small", "{item.download_client}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    };
    rsx! {
        div { class: "page-head", h1 { "Download queue" } }
        p { class: "help small faint", "Read live from the download clients every few seconds. Kitsunarr does not import completed downloads yet." }
        div { class: "card", {body} }
    }
}

const EVENT_TYPES: &[(&str, &str)] = &[
    ("", "All events"),
    ("grabbed", "Grabbed"),
    ("downloadFolderImported", "Imported"),
    ("seriesFolderImported", "Imported from series folder"),
    ("downloadFailed", "Download failed"),
    ("episodeFileDeleted", "File deleted"),
    ("episodeFileRenamed", "File renamed"),
    ("downloadIgnored", "Ignored"),
];

fn tone(event: &str) -> Option<&'static str> {
    match event {
        "grabbed" => Some("info"),
        "downloadFolderImported" | "seriesFolderImported" => Some("success"),
        "downloadFailed" => Some("danger"),
        "episodeFileDeleted" => Some("warning"),
        _ => None,
    }
}

#[component]
pub fn Activity() -> Element {
    let app = use_app();
    let client = use_client();
    let mut page = use_signal(|| 1u32);
    let mut event = use_signal(String::new);
    use_effect(move || {
        let _ = event();
        if *page.peek() != 1 {
            page.set(1);
        }
    });
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let (p, e) = (page(), event());
        let c = client.clone();
        async move {
            c.history(p, 50, Some(e.as_str()).filter(|s| !s.is_empty()))
                .await
        }
    });
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "History" }
            select { class: "select", "aria-label": "Event type", onchange: move |e: FormEvent| event.set(e.value()),
                for (value, label) in EVENT_TYPES { option { value: "{value}", "{label}" } }
            }
        }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(p)) if p.records.is_empty() => rsx! { Empty { title: "No history", p { "Grabs, imports and file changes appear here. Migrated history is included." } } },
            Some(Ok(p)) => rsx! {
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Event" } th { "Title" } th { class: "hide-mobile", "Release" } th { "Quality" } th { "Date" } } }
                        tbody {
                            for h in p.records.iter() {
                                tr { key: "{h.id}",
                                    td { Badge { text: h.event_type.clone(), tone: tone(&h.event_type) } }
                                    td {
                                        if let (Some(id), Some(t)) = (h.series_id, h.series_title.clone()) {
                                            Link { to: Route::SeriesDetailPage { id }, "{t}" }
                                        } else if let (Some(id), Some(t)) = (h.movie_id, h.movie_title.clone()) {
                                            Link { to: Route::MovieDetailPage { id }, "{t}" }
                                        } else {
                                            span { class: "muted", "—" }
                                        }
                                    }
                                    td { class: "hide-mobile small ellipsis", style: "max-width: 420px", title: "{h.source_title}", "{h.source_title}" }
                                    td { {h.quality_name.clone().unwrap_or_default()} }
                                    td { class: "muted small", {format_datetime(&h.date)} }
                                }
                            }
                        }
                    }
                }
                Pagination { page: p.page, page_size: p.page_size, total: p.total_records, on_page: move |n| page.set(n) }
            },
        }
    }
}
