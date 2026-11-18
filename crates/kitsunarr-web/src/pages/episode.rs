//! Sonarr's episode details (summary, history, search) and episode file editor ("Manage
//! episodes") of the series page.

use std::collections::HashSet;

use dioxus::prelude::*;
use kitsunarr_domain::api::library::{EpisodeFileDetail, EpisodeFileEditRequest};
use kitsunarr_domain::api::search::ReleaseQuery;
use kitsunarr_domain::quality::QualityModel;
use kitsunarr_domain::{Language, Quality};

use crate::components::common::{Badge, Confirm, ErrorBox, Field, Loading, Modal};
use crate::components::releases::ReleaseSearch;
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, format_date};

fn language_names(ids: &[i32]) -> String {
    let names: Vec<&str> = ids
        .iter()
        .filter_map(|l| Language::by_id(*l))
        .map(|l| l.name)
        .collect();
    if names.is_empty() {
        "—".into()
    } else {
        names.join(", ")
    }
}

fn release_type_label(t: &str) -> &'static str {
    match t {
        "singleEpisode" => "Single episode",
        "multiEpisode" => "Multi-episode",
        "seasonPack" => "Season pack",
        "multiSeasonPack" => "Multi-season pack",
        _ => "Unknown",
    }
}

/// The file part of the episode details: path, size, quality, languages and media information.
#[component]
fn FileSummary(file: EpisodeFileDetail) -> Element {
    rsx! {
        dl { class: "kv",
            dt { "Path" } dd { class: "mono small", "{file.path}" }
            dt { "Size" } dd { {format_bytes(file.size)} }
            dt { "Quality" }
            dd {
                Badge { text: file.quality_name.clone(), tone: "success" }
                if file.quality.revision.version > 1 { span { class: "small muted", " v{file.quality.revision.version}" } }
            }
            dt { "Languages" } dd { {language_names(&file.languages)} }
            dt { "Release group" } dd { {file.release_group.clone().unwrap_or_else(|| "—".into())} }
            if let Some(scene) = &file.scene_name { dt { "Scene name" } dd { class: "mono small", "{scene}" } }
            dt { "Added" } dd { {format_date(&file.added_at)} }
            if let Some(m) = &file.media_info {
                dt { "Video" }
                dd { "{m.video_codec} · {m.resolution}"
                    if !m.video_dynamic_range.is_empty() { " · {m.video_dynamic_range}" }
                    if m.video_fps > 0.0 { " · {m.video_fps} fps" }
                }
                dt { "Run time" } dd { "{m.run_time}" }
                dt { "Audio" }
                dd {
                    {m.audio.iter().map(|a| match &a.language {
                        Some(l) => format!("{} {:.1} ({l})", a.codec, a.channels),
                        None => format!("{} {:.1}", a.codec, a.channels),
                    }).collect::<Vec<_>>().join(", ")}
                }
                if !m.subtitles.is_empty() { dt { "Subtitles" } dd { {m.subtitles.join(", ")} } }
            }
        }
    }
}

/// Sonarr's episode details: the summary (overview, air date, file), the episode's history
/// and its searches.
#[component]
pub fn EpisodeDetailsDialog(
    series_id: i64,
    episode_id: i64,
    on_close: EventHandler<()>,
) -> Element {
    let app = use_app();
    let client = use_client();
    let mut tab = use_signal(|| "details");
    let mut interactive = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let mut searching = use_signal(|| false);
    let c0 = client.clone();
    let mut detail = use_resource(move || {
        let _ = (app.library_rev)();
        let c = c0.clone();
        async move { c.episode_detail(episode_id).await }
    });
    let c1 = client.clone();
    let mut auto_search = move |label: String| {
        let c = c1.clone();
        searching.set(true);
        spawn(async move {
            let q = ReleaseQuery {
                series_id,
                season_number: None,
                episode_id: Some(episode_id),
                complete_series: false,
                season_packs_only: false,
                indexer_ids: None,
                search_session: None,
            };
            match c.automatic_search(&q).await {
                Ok(r) => {
                    let (level, message) = super::detail::automatic_summary(&label, &r);
                    app.toast(level, message);
                }
                Err(e) => app.error(&e),
            }
            searching.set(false);
        });
    };
    let c2 = client.clone();
    let mut delete_file = move |file_id: i64| {
        let c = c2.clone();
        confirm_delete.set(false);
        spawn(async move {
            match c.delete_episode_file(series_id, file_id).await {
                Ok(()) => {
                    app.success("File deleted.");
                    detail.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let read = detail.read();
    let d = match &*read {
        None => {
            return rsx! { Modal { title: "Episode".to_string(), wide: true, on_close: move |_| on_close.call(()), Loading {} } };
        }
        Some(Err(e)) => {
            return rsx! { Modal { title: "Episode".to_string(), wide: true, on_close: move |_| on_close.call(()), ErrorBox { message: e.to_string() } } };
        }
        Some(Ok(d)) => d.clone(),
    };
    drop(read);
    let e = d.episode.clone();
    let label = format!("S{:02}E{:02}", e.season_number, e.episode_number);
    let title = format!(
        "{} — {label} — {}",
        d.series_title,
        e.title.clone().unwrap_or_else(|| "TBA".into())
    );
    let file_id = d.file.as_ref().map(|f| f.id);
    rsx! {
        Modal { title, wide: true, on_close: move |_| on_close.call(()),
            div { class: "row tight", role: "tablist", style: "margin-bottom: var(--sp-3)",
                button { class: if tab() == "details" { "btn sm primary" } else { "btn sm" }, role: "tab", "aria-selected": "{tab() == \"details\"}", onclick: move |_| tab.set("details"), "Details" }
                button { class: if tab() == "history" { "btn sm primary" } else { "btn sm" }, role: "tab", "aria-selected": "{tab() == \"history\"}", onclick: move |_| tab.set("history"), "History" }
                button { class: if tab() == "search" { "btn sm primary" } else { "btn sm" }, role: "tab", "aria-selected": "{tab() == \"search\"}", onclick: move |_| tab.set("search"), "Search" }
            }
            match tab() {
                "history" => rsx! { super::activity::HistoryTable { series_id, episode_id } },
                "search" => rsx! {
                    div { class: "row",
                        button { class: "btn", disabled: searching(),
                            title: "Search the indexers and grab the best release",
                            onclick: { let label = label.clone(); move |_| auto_search(label.clone()) },
                            if searching() { "Searching…" } else { "Automatic search" }
                        }
                        button { class: "btn", disabled: interactive(),
                            title: "See every release and choose",
                            onclick: move |_| interactive.set(true),
                            "Interactive search"
                        }
                    }
                    if interactive() {
                        ReleaseSearch { key: "episode-{episode_id}", series_id, episode_id }
                    }
                },
                _ => rsx! {
                    div { class: "row tight", style: "margin-bottom: var(--sp-2)",
                        Badge { text: if e.monitored { "monitored".to_string() } else { "unmonitored".to_string() }, tone: if e.monitored { Some("info") } else { None } }
                        if e.watched { Badge { text: "watched".to_string(), tone: "info" } }
                        if let Some(a) = e.absolute_episode_number { Badge { text: format!("absolute {a}") } }
                    }
                    dl { class: "kv",
                        dt { "Air date" } dd { {e.air_date_utc.as_deref().map(crate::util::format_datetime).or(e.air_date.clone()).unwrap_or_else(|| "TBA".into())} }
                        if let Some(r) = e.runtime { dt { "Run time" } dd { "{r} min" } }
                    }
                    if let Some(o) = &d.overview { p { class: "muted", "{o}" } } else { p { class: "muted small", "No overview." } }
                    h3 { "File" }
                    match &d.file {
                        Some(f) => rsx! {
                            FileSummary { file: f.clone() }
                            button { class: "btn sm danger", onclick: move |_| confirm_delete.set(true), "Delete file" }
                        },
                        None => rsx! { p { span { class: "badge warning", "missing" } } },
                    }
                },
            }
        }
        if confirm_delete() && let Some(id) = file_id {
            Confirm {
                title: "Delete file",
                message: "Delete this episode's file? It goes to the recycle bin when one is set.".to_string(),
                confirm_label: "Delete",
                danger: true,
                on_confirm: move |_| delete_file(id),
                on_cancel: move |_| confirm_delete.set(false),
            }
        }
    }
}

/// Sonarr's "Manage episodes" (episode file editor): every file of the series, with the
/// quality, languages, release group or release type of the selected ones changed together,
/// or the selected files deleted.
#[component]
pub fn ManageEpisodesDialog(series_id: i64, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut files = use_resource(move || {
        let c = c0.clone();
        async move { c.episode_files(series_id).await }
    });
    let mut selected = use_signal(HashSet::<i64>::new);
    let mut field = use_signal(String::new);
    let mut quality = use_signal(|| 0i32);
    let mut languages = use_signal(Vec::<i32>::new);
    let mut group = use_signal(String::new);
    let mut release_type = use_signal(|| "singleEpisode".to_string());
    let mut busy = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let c1 = client.clone();
    let apply = move |_| {
        let ids: Vec<i64> = selected().into_iter().collect();
        let mut req = EpisodeFileEditRequest {
            episode_file_ids: ids,
            ..Default::default()
        };
        match field().as_str() {
            "quality" => {
                req.quality = Some(QualityModel {
                    quality_id: quality(),
                    revision: Default::default(),
                })
            }
            "languages" => req.languages = Some(languages()),
            "releaseGroup" => req.release_group = Some(group()),
            "releaseType" => req.release_type = Some(release_type()),
            _ => return,
        }
        let c = c1.clone();
        busy.set(true);
        spawn(async move {
            match c.edit_episode_files(&req).await {
                Ok(r) => {
                    app.success(format!("{} files updated.", r.updated));
                    field.set(String::new());
                    files.restart();
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let c2 = client.clone();
    let delete = move |_| {
        let ids: Vec<i64> = selected().into_iter().collect();
        let c = c2.clone();
        confirm_delete.set(false);
        busy.set(true);
        spawn(async move {
            match c.delete_episode_files(&ids).await {
                Ok(r) if r.failures.is_empty() => {
                    app.success(format!("{} files deleted.", r.deleted))
                }
                Ok(r) => app.toast(
                    crate::state::ToastLevel::Warning,
                    format!("{} files deleted; {}", r.deleted, r.failures.join("; ")),
                ),
                Err(e) => app.error(&e),
            }
            selected.write().clear();
            files.restart();
            busy.set(false);
        });
    };
    let list = match &*files.read() {
        Some(Ok(l)) => Some(l.clone()),
        _ => None,
    };
    let count = selected.read().len();
    let ready = count > 0
        && match field().as_str() {
            "quality" => true,
            "languages" => !languages().is_empty(),
            "releaseGroup" | "releaseType" => true,
            _ => false,
        };
    rsx! {
        Modal { title: "Manage episodes".to_string(), wide: true, on_close: move |_| { if !busy() { on_close.call(()) } },
            footer: rsx! {
                span { class: "small muted", "{count} selected" }
                div { class: "spacer" }
                button { class: "btn danger", disabled: busy() || count == 0, onclick: move |_| confirm_delete.set(true), "Delete files" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(()), "Close" }
            },
            match &*files.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(l)) if l.is_empty() => rsx! { p { class: "muted", "The series has no files." } },
                Some(Ok(_)) => rsx! {},
            }
            if let Some(list) = list.filter(|l| !l.is_empty()) {
                div { class: "row wrap", style: "margin-bottom: var(--sp-3)",
                    select { class: "select", style: "width: auto", "aria-label": "Change", value: field(), disabled: busy(),
                        onchange: move |e: FormEvent| field.set(e.value()),
                        option { value: "", "Change for the selected files…" }
                        option { value: "quality", "Quality" }
                        option { value: "languages", "Languages" }
                        option { value: "releaseGroup", "Release group" }
                        option { value: "releaseType", "Release type" }
                    }
                    match field().as_str() {
                        "quality" => rsx! {
                            select { class: "select", style: "width: auto", "aria-label": "Quality", disabled: busy(),
                                onchange: move |e: FormEvent| { if let Ok(q) = e.value().parse() { quality.set(q); } },
                                for q in Quality::all().iter() { option { key: "{q.id}", value: "{q.id}", selected: q.id == quality(), "{q.name}" } }
                            }
                        },
                        "releaseGroup" => rsx! {
                            input { class: "input", style: "width: auto", "aria-label": "Release group", placeholder: "Empty clears it", value: "{group}", disabled: busy(), oninput: move |e| group.set(e.value()) }
                        },
                        "releaseType" => rsx! {
                            select { class: "select", style: "width: auto", "aria-label": "Release type", value: release_type(), disabled: busy(),
                                onchange: move |e: FormEvent| release_type.set(e.value()),
                                option { value: "unknown", "Unknown" }
                                option { value: "singleEpisode", "Single episode" }
                                option { value: "multiEpisode", "Multi-episode" }
                                option { value: "seasonPack", "Season pack" }
                            }
                        },
                        _ => rsx! {},
                    }
                    if !field().is_empty() {
                        button { class: "btn primary", disabled: busy() || !ready, onclick: apply, if busy() { "Saving…" } else { "Apply" } }
                    }
                }
                if field() == "languages" {
                    Field { label: "Languages",
                        div { class: "chips", role: "group", "aria-label": "Languages",
                            for l in Language::all().iter().filter(|l| l.id > 0) {
                                {
                                    let id = l.id;
                                    let on = languages().contains(&id);
                                    rsx! {
                                        button { key: "{id}", r#type: "button", class: if on { "chip on" } else { "chip" }, "aria-pressed": "{on}",
                                            onclick: move |_| { let mut w = languages.write(); if let Some(i) = w.iter().position(|x| *x == id) { w.remove(i); } else { w.push(id); } },
                                            "{l.name}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr {
                            th { class: "shrink",
                                input { r#type: "checkbox", "aria-label": "Select every file",
                                    checked: !list.is_empty() && selected.read().len() == list.len(),
                                    onchange: {
                                        let all: Vec<i64> = list.iter().map(|f| f.id).collect();
                                        move |e: FormEvent| { let mut w = selected.write(); w.clear(); if e.checked() { w.extend(all.iter().copied()); } }
                                    }
                                }
                            }
                            th { "Episodes" } th { "File" } th { "Languages" } th { "Quality" } th { "Release group" } th { "Release type" } th { class: "num", "Size" }
                        } }
                        tbody {
                            for f in list.iter() {
                                {
                                    let id = f.id;
                                    rsx! {
                                        tr { key: "{f.id}",
                                            td { class: "shrink", input { r#type: "checkbox", "aria-label": "Select {f.relative_path}", checked: selected.read().contains(&id),
                                                onchange: move |e: FormEvent| { if e.checked() { selected.write().insert(id); } else { selected.write().remove(&id); } } } }
                                            td { class: "small", {f.episodes.join(", ")} }
                                            td { class: "small mono ellipsis", title: "{f.path}", "{f.relative_path}" }
                                            td { class: "small", {language_names(&f.languages)} }
                                            td { span { class: "badge", "{f.quality_name}" } }
                                            td { class: "small", {f.release_group.clone().unwrap_or_default()} }
                                            td { class: "small", {release_type_label(&f.release_type)} }
                                            td { class: "num small", {format_bytes(f.size)} }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if confirm_delete() {
            Confirm {
                title: "Delete files",
                message: format!("Delete the {count} selected files? They go to the recycle bin when one is set."),
                confirm_label: "Delete",
                danger: true,
                on_confirm: delete,
                on_cancel: move |_| confirm_delete.set(false),
            }
        }
    }
}
