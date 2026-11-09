//! Activity: download queue and history.

use dioxus::prelude::*;

use crate::components::common::{Badge, Empty, ErrorBox, Loading, NotReady, Pagination};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::format_datetime;

#[component]
pub fn Queue() -> Element {
    rsx! {
        div { class: "page-head", h1 { "Download queue" } }
        NotReady {
            feature: "Download queue",
            detail: "Kitsunarr does not talk to download clients yet (qBittorrent, Transmission, Deluge, SABnzbd and NZBGet support is the next milestone). Your imported download-client settings are stored and will be used once the integrations land.",
        }
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
