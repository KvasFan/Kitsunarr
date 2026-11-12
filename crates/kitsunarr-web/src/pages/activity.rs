//! Activity: download queue, history and blocklist.

use dioxus::prelude::*;

use kitsunarr_domain::api::queue::{BlocklistRemoval, QueueItem, QueueRemoval};

use crate::components::common::{Badge, Confirm, Empty, ErrorBox, Loading, Modal, Pagination};
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
    let app = use_app();
    let mut checking = use_signal(|| false);
    let mut tick = use_signal(|| 0u64);
    // The item being removed and the chosen options.
    let mut removing = use_signal(|| None::<QueueItem>);
    let mut removal = use_signal(QueueRemoval::default);
    let c_remove = client.clone();
    let confirm_remove = move |_| {
        let Some(item) = removing() else { return };
        let options = removal();
        let c = c_remove.clone();
        removing.set(None);
        spawn(async move {
            match c.remove_queue_item(&item.download_id, &options).await {
                Ok(r) => app.success(r.message),
                Err(e) => app.error(&e),
            }
            tick += 1;
        });
    };
    let c_check = client.clone();
    let check_now = move |_| {
        let c = c_check.clone();
        checking.set(true);
        spawn(async move {
            match c.refresh_queue().await {
                Ok(summary) => app.success(format!("Checked: {summary}.")),
                Err(e) => app.error(&e),
            }
            checking.set(false);
            tick += 1;
        });
    };
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
                    p { "Add a download client (qBittorrent, Deluge, Transmission, SABnzbd or NZBGet) and enable it." }
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
                            th { "Status" } th { "Progress" } th { class: "num", "Time left" } th { "Client" } th { "" }
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
                                            td { class: "shrink",
                                                button { class: "btn sm", "aria-label": "Remove {item.title}",
                                                    onclick: { let item = item.clone(); move |_| {
                                                        removal.set(QueueRemoval {
                                                            remove_from_client: true,
                                                            blocklist: false,
                                                            skip_redownload: false,
                                                        });
                                                        removing.set(Some(item.clone()));
                                                    } },
                                                    "Remove"
                                                }
                                            }
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
        div { class: "page-head",
            h1 { class: "grow", "Download queue" }
            button { class: "btn", disabled: checking(), onclick: check_now,
                if checking() { "Checking…" } else { "Check now" }
            }
        }
        p { class: "help small faint", "Read live from the download clients every few seconds. Completed downloads Kitsunarr grabbed are imported into the series folder every minute (torrents are copied or hard-linked, so they keep seeding; Usenet downloads are moved). Failed downloads are blocklisted and searched again." }
        div { class: "card", {body} }
        if let Some(item) = removing() {
            Modal {
                title: "Remove from queue".to_string(),
                on_close: move |_| removing.set(None),
                footer: rsx! {
                    div { class: "spacer" }
                    button { class: "btn", onclick: move |_| removing.set(None), "Cancel" }
                    button { class: "btn danger solid", onclick: confirm_remove, "Remove" }
                },
                p { class: "mono small", "{item.title}" }
                div { class: "stack",
                    label { class: "check",
                        input { r#type: "checkbox", checked: removal().remove_from_client,
                            onchange: move |e: FormEvent| removal.write().remove_from_client = e.checked() }
                        "Remove from the download client (deletes its files there)"
                    }
                    label { class: "check",
                        input { r#type: "checkbox", checked: removal().blocklist, disabled: item.series_id.is_none(),
                            onchange: move |e: FormEvent| removal.write().blocklist = e.checked() }
                        "Blocklist the release: it is never grabbed again"
                    }
                    if removal().blocklist {
                        label { class: "check",
                            input { r#type: "checkbox", checked: removal().skip_redownload,
                                onchange: move |e: FormEvent| removal.write().skip_redownload = e.checked() }
                            "Do not search for a replacement"
                        }
                    }
                }
            }
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
    let mut refresh = use_signal(|| 0u32);
    let mut failing = use_signal(|| None::<(i64, String)>);
    let c_fail = client.clone();
    let mark_failed = move |_| {
        let Some((id, _)) = failing() else { return };
        failing.set(None);
        let c = c_fail.clone();
        spawn(async move {
            match c.mark_history_failed(id).await {
                Ok(r) => app.success(r.message),
                Err(e) => app.error(&e),
            }
            refresh += 1;
        });
    };
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let _ = refresh();
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
                        thead { tr { th { "Event" } th { "Title" } th { class: "hide-mobile", "Release" } th { "Quality" } th { "Date" } th { "" } } }
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
                                    td { class: "hide-mobile small ellipsis", style: "max-width: 26rem", title: "{h.source_title}", "{h.source_title}" }
                                    td { {h.quality_name.clone().unwrap_or_default()} }
                                    td { class: "muted small", {format_datetime(&h.date)} }
                                    td { class: "shrink",
                                        if h.event_type == "grabbed" && h.download_id.is_some() {
                                            button { class: "btn sm", title: "Blocklist the release and search again",
                                                onclick: { let (id, t) = (h.id, h.source_title.clone()); move |_| failing.set(Some((id, t.clone()))) },
                                                "Mark as failed"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Pagination { page: p.page, page_size: p.page_size, total: p.total_records, on_page: move |n| page.set(n) }
            },
        }
        if let Some((_, title)) = failing() {
            Confirm {
                title: "Mark as failed",
                message: format!("Blocklist “{title}” and search for a replacement?"),
                confirm_label: "Mark as failed",
                danger: true,
                on_confirm: mark_failed,
                on_cancel: move |_| failing.set(None),
            }
        }
    }
}

/// Blocklisted releases (Sonarr's Activity › Blocklist).
#[component]
pub fn Blocklist() -> Element {
    let app = use_app();
    let client = use_client();
    let mut page = use_signal(|| 1u32);
    let mut refresh = use_signal(|| 0u32);
    let mut confirm_clear = use_signal(|| false);
    let c0 = client.clone();
    let data = use_resource(move || {
        let _ = refresh();
        let p = page();
        let c = c0.clone();
        async move { c.blocklist(p, 50).await }
    });
    let c1 = client.clone();
    let remove = move |id: i64| {
        let c = c1.clone();
        spawn(async move {
            match c.delete_blocklist(id).await {
                Ok(()) => app.success("Removed from the blocklist."),
                Err(e) => app.error(&e),
            }
            refresh += 1;
        });
    };
    let c2 = client.clone();
    let clear = move |_| {
        confirm_clear.set(false);
        let c = c2.clone();
        spawn(async move {
            match c
                .remove_blocklist(&BlocklistRemoval {
                    ids: vec![],
                    all: true,
                })
                .await
            {
                Ok(n) => app.success(format!("Cleared {n} releases from the blocklist.")),
                Err(e) => app.error(&e),
            }
            refresh += 1;
        });
    };
    let has_entries = matches!(&*data.read(), Some(Ok(p)) if p.total_records > 0);
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Blocklist" }
            if has_entries {
                button { class: "btn danger", onclick: move |_| confirm_clear.set(true), "Clear blocklist" }
            }
        }
        p { class: "help small faint", "Releases that failed or were blocklisted by hand. Searches and RSS sync never grab them again; remove one to allow it." }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(p)) if p.records.is_empty() => rsx! { Empty { title: "The blocklist is empty" } },
            Some(Ok(p)) => rsx! {
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Series" } th { "Release" } th { "Quality" } th { class: "hide-mobile", "Indexer" } th { class: "hide-mobile", "Message" } th { "Date" } th { "" } } }
                        tbody {
                            for b in p.records.iter() {
                                tr { key: "{b.id}",
                                    td { Link { to: Route::SeriesDetailPage { id: b.series_id }, {b.series_title.clone().unwrap_or_default()} } }
                                    td { class: "small ellipsis", style: "max-width: 26rem", title: "{b.source_title}", "{b.source_title}" }
                                    td { {b.quality.clone().unwrap_or_default()} }
                                    td { class: "hide-mobile small", {b.indexer.clone().unwrap_or_default()} }
                                    td { class: "hide-mobile small muted", {b.message.clone().unwrap_or_default()} }
                                    td { class: "muted small", {format_datetime(&b.date)} }
                                    td { class: "shrink",
                                        button { class: "btn sm", "aria-label": "Remove {b.source_title} from the blocklist",
                                            onclick: { let id = b.id; let r = remove.clone(); move |_| r(id) }, "Remove" }
                                    }
                                }
                            }
                        }
                    }
                }
                Pagination { page: p.page, page_size: p.page_size, total: p.total_records, on_page: move |n| page.set(n) }
            },
        }
        if confirm_clear() {
            Confirm {
                title: "Clear blocklist",
                message: "Remove every release from the blocklist?".to_string(),
                confirm_label: "Clear",
                danger: true,
                on_confirm: clear,
                on_cancel: move |_| confirm_clear.set(false),
            }
        }
    }
}

/// Sonarr's manual import: the video files of a folder, matched to series and episodes, imported
/// with the user's choices.
#[component]
pub fn ManualImport() -> Element {
    use kitsunarr_domain::api::library_tools::{
        ManualImportFile, ManualImportItem, ManualImportRequest,
    };
    let app = use_app();
    let client = use_client();
    let mut folder = use_signal(String::new);
    let mut items = use_signal(Vec::<ManualImportItem>::new);
    let mut chosen = use_signal(Vec::<usize>::new);
    let mut mode = use_signal(|| "copy".to_string());
    let mut busy = use_signal(|| false);
    let mut picking = use_signal(|| None::<usize>);
    let c0 = client.clone();
    let series = use_resource(move || {
        let c = c0.clone();
        async move {
            c.series(&kitsunarr_domain::api::library::LibraryQuery {
                page_size: Some(5000),
                ..Default::default()
            })
            .await
            .map(|p| p.records)
            .unwrap_or_default()
        }
    });
    let c1 = client.clone();
    let scan = move |_| {
        let c = c1.clone();
        let f = folder();
        busy.set(true);
        spawn(async move {
            match c.manual_import_scan(&f, None).await {
                Ok(list) => {
                    chosen.set(
                        (0..list.len())
                            .filter(|i| list[*i].rejections.is_empty())
                            .collect(),
                    );
                    items.set(list);
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let c2 = client.clone();
    let run = move |_| {
        let c = c2.clone();
        let list = items();
        let files: Vec<ManualImportFile> = chosen()
            .into_iter()
            .filter_map(|i| list.get(i))
            .filter_map(|it| {
                Some(ManualImportFile {
                    path: it.path.clone(),
                    series_id: it.series_id?,
                    episode_ids: it.episode_ids.clone(),
                })
            })
            .collect();
        let req = ManualImportRequest {
            files,
            import_mode: mode(),
        };
        busy.set(true);
        spawn(async move {
            match c.manual_import(&req).await {
                Ok(results) => {
                    let ok = results.iter().filter(|r| r.imported).count();
                    let problems: Vec<String> = results
                        .iter()
                        .filter(|r| !r.imported)
                        .map(|r| format!("{}: {}", r.path, r.messages.join(", ")))
                        .collect();
                    if problems.is_empty() {
                        app.success(format!("{ok} files imported."));
                    } else {
                        app.toast(
                            crate::state::ToastLevel::Warning,
                            format!("{ok} imported; {}", problems.join("; ")),
                        );
                    }
                    items.set(Vec::new());
                    chosen.set(Vec::new());
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let ready = !chosen().is_empty()
        && chosen().iter().all(|i| {
            items()
                .get(*i)
                .is_some_and(|it| it.series_id.is_some() && !it.episode_ids.is_empty())
        });
    rsx! {
        div { class: "page-head", h1 { "Manual import" } }
        p { class: "muted small", "Import video files from any folder the server can read: check the series and episodes each file is for, then copy or move them into the library (renamed by the naming settings)." }
        div { class: "row wrap",
            input { class: "input grow", "aria-label": "Folder", placeholder: "/downloads/complete/Show", value: "{folder}", oninput: move |e| folder.set(e.value()) }
            button { class: "btn", disabled: busy() || folder().trim().is_empty(), onclick: scan, "Scan folder" }
        }
        if !items().is_empty() {
            div { class: "table-wrap", style: "margin-top: var(--sp-3)",
                table { class: "table",
                    thead { tr { th { "" } th { "File" } th { "Series" } th { "Episodes" } th { "Quality" } th { "" } } }
                    tbody {
                        for (i, it) in items().into_iter().enumerate() {
                            tr { key: "{i}",
                                td { class: "shrink", input { r#type: "checkbox", "aria-label": "Import {it.name}", checked: chosen().contains(&i),
                                    onchange: move |e: FormEvent| { let mut w = chosen.write(); w.retain(|x| *x != i); if e.checked() { w.push(i); } } } }
                                td { class: "small", title: "{it.path}", "{it.relative_path}" }
                                td {
                                    select { class: "select", "aria-label": "Series of {it.name}",
                                        onchange: move |e| {
                                            let id = e.value().parse::<i64>().ok();
                                            let mut w = items.write();
                                            if let Some(x) = w.get_mut(i) && x.series_id != id { x.series_id = id; x.episode_ids.clear(); x.episodes.clear(); }
                                        },
                                        option { value: "", "Choose…" }
                                        for s in series().unwrap_or_default() { option { key: "{s.id}", value: "{s.id}", selected: it.series_id == Some(s.id), "{s.title}" } }
                                    }
                                }
                                td {
                                    span { class: "small", {if it.episodes.is_empty() { "—".to_string() } else { it.episodes.join(", ") }} }
                                    " "
                                    button { class: "btn sm ghost", disabled: it.series_id.is_none(), onclick: move |_| picking.set(Some(i)), "Choose…" }
                                }
                                td { span { class: "badge", "{it.quality}" } }
                                td { for r in it.rejections.iter() { div { class: "small warning", "{r}" } } }
                            }
                        }
                    }
                }
            }
            div { class: "row", style: "margin-top: var(--sp-3)",
                select { class: "select", style: "width: auto", "aria-label": "Import mode", onchange: move |e| mode.set(e.value()),
                    option { value: "copy", selected: mode() == "copy", "Copy (keep the originals)" }
                    option { value: "move", selected: mode() == "move", "Move" }
                }
                div { class: "spacer" }
                button { class: "btn primary", disabled: busy() || !ready, onclick: run, if busy() { "Importing…" } else { "Import {chosen().len()} files" } }
            }
        }
        if let Some(i) = picking() {
            if let Some(sid) = items().get(i).and_then(|it| it.series_id) {
                PickEpisodes { series_id: sid, initial: items().get(i).map(|it| it.episode_ids.clone()).unwrap_or_default(),
                    on_done: move |picked: Option<Vec<(i64, String)>>| {
                        if let Some(p) = picked && let Some(x) = items.write().get_mut(i) {
                            x.episode_ids = p.iter().map(|(id, _)| *id).collect();
                            x.episodes = p.into_iter().map(|(_, l)| l).collect();
                        }
                        picking.set(None);
                    } }
            }
        }
    }
}

#[component]
fn PickEpisodes(
    series_id: i64,
    initial: Vec<i64>,
    on_done: EventHandler<Option<Vec<(i64, String)>>>,
) -> Element {
    let client = use_client();
    let episodes = use_resource(move || {
        let c = client.clone();
        async move { c.episodes(series_id).await }
    });
    let mut chosen = use_signal(|| initial.clone());
    let done = move |_| {
        let picked = match &*episodes.read() {
            Some(Ok(list)) => list
                .iter()
                .filter(|e| chosen().contains(&e.id))
                .map(|e| {
                    (
                        e.id,
                        format!("S{:02}E{:02}", e.season_number, e.episode_number),
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        on_done.call(Some(picked));
    };
    rsx! {
        Modal { title: "Episodes".to_string(), on_close: move |_| on_done.call(None),
            footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| on_done.call(None), "Cancel" } button { class: "btn primary", onclick: done, "Use these episodes" } },
            match &*episodes.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) => rsx! {
                    div { class: "stack", style: "max-height: 60vh; overflow: auto",
                        for e in list.iter() {
                            {
                                let id = e.id;
                                rsx! {
                                    label { key: "{e.id}", class: "check small",
                                        input { r#type: "checkbox", checked: chosen().contains(&id),
                                            onchange: move |ev: FormEvent| { let mut w = chosen.write(); w.retain(|x| *x != id); if ev.checked() { w.push(id); } } }
                                        "S{e.season_number:02}E{e.episode_number:02} "
                                        {e.title.clone().unwrap_or_default()}
                                        if e.file.is_some() { " " Badge { text: "has a file".to_string() } }
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
