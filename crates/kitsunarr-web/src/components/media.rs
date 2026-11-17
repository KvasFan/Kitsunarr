//! Library presentation: poster cards and tables.

use std::collections::HashSet;

use dioxus::prelude::*;
use kitsunarr_domain::api::library::{MovieListItem, SeriesListItem};

use crate::components::common::{Badge, Progress};
use crate::route::Route;
use crate::util::{format_bytes, format_date, initials, percent};

#[component]
pub fn Poster(url: Option<String>, title: String, monitored: bool) -> Element {
    rsx! {
        div { class: "poster",
            if let Some(src) = url {
                img { src: "{src}", alt: "", loading: "lazy", referrerpolicy: "no-referrer" }
            } else {
                span { class: "initials", "aria-hidden": "true", {initials(&title)} }
            }
            if !monitored { div { class: "unmonitored", title: "Unmonitored" } }
        }
    }
}

#[component]
pub fn SeriesCard(
    item: SeriesListItem,
    selected: bool,
    /// The library shows anime groups once ("Group anime seasons").
    #[props(default)]
    grouped: bool,
) -> Element {
    let pct = percent(item.episode_file_count, item.episode_count);
    let tone = if item.episode_count > 0 && item.episode_file_count >= item.episode_count {
        Some("success")
    } else {
        None
    };
    let id = item.id;
    rsx! {
        Link { to: Route::SeriesDetailPage { id }, class: if selected { "media-card selected" } else { "media-card" },
            Poster { url: item.poster_url.clone(), title: item.title.clone(), monitored: item.monitored }
            div { class: "meta",
                span { class: "t ellipsis", title: "{item.title}", "{item.title}" }
                span { class: "xs muted",
                    {item.year.map(|y| y.to_string()).unwrap_or_default()}
                    " · {item.episode_file_count}/{item.episode_count} ep"
                }
                if let Some(g) = item.anime_group.as_ref().filter(|g| grouped && g.series_count > 1) {
                    span { class: "xs accent ellipsis", title: "Anime group {g.title}", "Group of {g.series_count} · {g.title}" }
                }
                Progress { percent: pct, tone }
            }
        }
    }
}

#[component]
pub fn MovieCard(item: MovieListItem, #[props(default)] selected: bool) -> Element {
    let id = item.id;
    rsx! {
        Link { to: Route::MovieDetailPage { id }, class: if selected {"media-card selected"}else{"media-card"},
            Poster { url: item.poster_url.clone(), title: item.title.clone(), monitored: item.monitored }
            div { class: "meta",
                span { class: "t ellipsis", title: "{item.title}", "{item.title}" }
                span { class: "xs muted",
                    {item.year.map(|y| y.to_string()).unwrap_or_default()}
                    if item.has_file { " · on disk" } else { " · missing" }
                }
                if item.watched {Badge {text:"watched",tone:"success"}}
            }
        }
    }
}

fn status_badge(status: &str) -> (&'static str, String) {
    match status {
        "continuing" => ("success", "Continuing".into()),
        "ended" => ("info", "Ended".into()),
        "upcoming" => ("warning", "Upcoming".into()),
        "deleted" => ("danger", "Deleted".into()),
        other => ("", other.to_string()),
    }
}

/// Sortable, selectable series table.
#[component]
pub fn SeriesTable(
    items: Vec<SeriesListItem>,
    selected: Signal<HashSet<i64>>,
    sort_key: String,
    on_sort: EventHandler<String>,
) -> Element {
    let all_ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let all_selected = !all_ids.is_empty() && all_ids.iter().all(|id| selected.read().contains(id));
    let header = move |key: &'static str, label: &'static str| {
        let active = sort_key == key;
        rsx! {
            th {
                class: "sortable",
                "aria-sort": if active { "ascending" } else { "none" },
                onclick: move |_| on_sort.call(key.to_string()),
                "{label}"
                if active { " ▾" }
            }
        }
    };
    rsx! {
        div { class: "table-wrap",
            table { class: "table",
                thead {
                    tr {
                        th { class: "shrink",
                            input {
                                r#type: "checkbox",
                                "aria-label": "Select all",
                                checked: all_selected,
                                onchange: move |e: FormEvent| {
                                    let mut sel = selected.write();
                                    if e.checked() { sel.extend(all_ids.iter().copied()); } else { for id in &all_ids { sel.remove(id); } }
                                },
                            }
                        }
                        {header("title", "Title")}
                        th { class: "hide-mobile", "Network" }
                        th { class: "hide-mobile", "Profile" }
                        {header("status", "Status")}
                        th { class: "num", "Episodes" }
                        {header("size", "Size")}
                        {header("added", "Added")}
                    }
                }
                tbody {
                    for item in items {
                        SeriesRow { key: "{item.id}", item: item.clone(), selected }
                    }
                }
            }
        }
    }
}

#[component]
fn SeriesRow(item: SeriesListItem, selected: Signal<HashSet<i64>>) -> Element {
    let id = item.id;
    let is_selected = selected.read().contains(&id);
    let (tone, label) = status_badge(&item.status.to_string());
    rsx! {
        tr { class: if is_selected { "selected" } else { "" },
            td {
                input {
                    r#type: "checkbox",
                    "aria-label": "Select {item.title}",
                    checked: is_selected,
                    onchange: move |e: FormEvent| { if e.checked() { selected.write().insert(id); } else { selected.write().remove(&id); } },
                }
            }
            td {
                div { class: "row tight",
                    if !item.monitored { Badge { text: "unmonitored" } }
                    Link { to: Route::SeriesDetailPage { id }, "{item.title}" }
                    span { class: "faint xs", {item.year.map(|y| y.to_string()).unwrap_or_default()} }
                }
            }
            td { class: "hide-mobile muted", {item.network.clone().unwrap_or_default()} }
            td { class: "hide-mobile", "{item.quality_profile_name}" }
            td { Badge { text: label, tone: if tone.is_empty() { None } else { Some(tone) } } }
            td { class: "num", "{item.episode_file_count} / {item.episode_count}" }
            td { class: "num", {format_bytes(item.size_on_disk)} }
            td { class: "muted", {format_date(&item.added_at)} }
        }
    }
}

#[component]
pub fn MovieTable(
    items: Vec<MovieListItem>,
    mut selected: Signal<HashSet<i64>>,
    #[props(default)] disabled: bool,
) -> Element {
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let all = !ids.is_empty() && ids.iter().all(|id| selected.read().contains(id));
    rsx! {
        div { class: "table-wrap",
            table { class: "table",
                thead { tr { th { input {r#type:"checkbox",disabled,"aria-label":"Select all movies on this page",checked:all,onchange:move |e:FormEvent|{if e.checked(){selected.write().extend(ids.iter().copied());}else{for id in &ids{selected.write().remove(id);}}}} } th { "Title" } th { "Year" } th { class: "hide-mobile", "Profile" } th { "File" } th { "Watched" } th { class: "num", "Size" } th { "Added" } } }
                tbody {
                    for item in items {
                        tr { key: "{item.id}",
                            td {input {r#type:"checkbox",disabled,"aria-label":"Select {item.title}",checked:selected.read().contains(&item.id),onchange:move |e:FormEvent|{if e.checked(){selected.write().insert(item.id);}else{selected.write().remove(&item.id);}}}}
                            td {
                                div { class: "row tight",
                                    if !item.monitored { Badge { text: "unmonitored" } }
                                    Link { to: Route::MovieDetailPage { id: item.id }, "{item.title}" }
                                }
                            }
                            td { {item.year.map(|y| y.to_string()).unwrap_or_default()} }
                            td { class: "hide-mobile", "{item.quality_profile_name}" }
                            td { if item.has_file { Badge { text: "on disk", tone: "success" } } else { Badge { text: "missing", tone: "warning" } } }
                            td {span {title:item.watched_at.as_deref().map(crate::util::format_datetime).unwrap_or_default(),if item.watched {"watched"}else{"unwatched"}}}
                            td { class: "num", {format_bytes(item.size_on_disk)} }
                            td { class: "muted", {format_date(&item.added_at)} }
                        }
                    }
                }
            }
        }
    }
}
