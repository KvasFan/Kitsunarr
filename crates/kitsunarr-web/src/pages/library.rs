//! Library pages (TV series, anime, movies) with server-side paging, search, sort, filters,
//! poster and table views, and bulk editing.

use std::collections::HashSet;

use dioxus::prelude::*;
use kitsunarr_domain::api::library::{BulkEdit, LibraryQuery};
use kitsunarr_domain::{MediaKind, SortDirection};

use crate::components::common::{Empty, ErrorBox, Loading, Pagination};
use crate::components::media::{MovieCard, MovieTable, SeriesCard, SeriesTable};
use crate::route::Route;
use crate::state::{use_app, use_client};

const PAGE_SIZE: u32 = 60;

#[component]
pub fn SeriesLibrary() -> Element {
    rsx! { SeriesLibraryView { kind: MediaKind::Series, title: "TV Series" } }
}

#[component]
pub fn AnimeLibrary() -> Element {
    rsx! { SeriesLibraryView { kind: MediaKind::Anime, title: "Anime" } }
}

/// Debounces a signal: the returned signal follows `source` after `ms` of inactivity.
fn use_debounced(source: Signal<String>, ms: u32) -> Signal<String> {
    let mut out = use_signal(|| source.peek().clone());
    let mut generation = use_signal(|| 0u64);
    use_effect(move || {
        let value = source();
        // peek: reading `generation` reactively here would re-trigger this effect forever.
        let ticket = *generation.peek() + 1;
        generation.set(ticket);
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(ms).await;
            // An unchanged initial value must not reset paging/selection 300 ms after mount.
            if *generation.peek() == ticket && *out.peek() != value {
                out.set(value);
            }
        });
    });
    out
}

#[component]
fn ViewToggle(grid: Signal<bool>) -> Element {
    rsx! {
        div { class: "seg", role: "group", "aria-label": "View",
            button { class: if grid() { "on" } else { "" }, onclick: move |_| grid.set(true), "Posters" }
            button { class: if !grid() { "on" } else { "" }, onclick: move |_| grid.set(false), "Table" }
        }
    }
}

#[component]
fn SeriesLibraryView(kind: MediaKind, title: &'static str) -> Element {
    let app = use_app();
    let client = use_client();
    let search = use_debounced(app.search, 300);
    let mut page = use_signal(|| 1u32);
    let mut sort_key = use_signal(|| "title".to_string());
    let mut descending = use_signal(|| false);
    let mut monitored = use_signal(|| None::<bool>);
    let grid = use_signal(|| crate::util::load_pref("kitsunarr-view").as_deref() != Some("table"));
    let mut selected = use_signal(HashSet::<i64>::new);
    let mut bulk_profile = use_signal(String::new);

    // Reset to the first page when filters change.
    use_effect(move || {
        let _ = (search(), sort_key(), descending(), monitored());
        if *page.peek() != 1 {
            page.set(1);
        }
    });
    use_effect(move || {
        crate::util::save_pref("kitsunarr-view", if grid() { "grid" } else { "table" })
    });

    let c = client.clone();
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let q = LibraryQuery {
            page: Some(page()),
            page_size: Some(PAGE_SIZE),
            sort_key: Some(sort_key()),
            sort_direction: Some(if descending() {
                SortDirection::Desc
            } else {
                SortDirection::Asc
            }),
            search: Some(search()).filter(|s| !s.trim().is_empty()),
            kind: Some(kind),
            monitored: monitored(),
            ..Default::default()
        };
        let c = c.clone();
        async move { c.series(&q).await }
    });
    let pc = client.clone();
    let profiles = use_resource(move || {
        let c = pc.clone();
        async move { c.quality_profiles().await }
    });

    let bulk = use_callback(move |edit: BulkEdit| {
        let client = client.clone();
        spawn(async move {
            match client.bulk_edit_series(&edit).await {
                Ok(r) => {
                    app.success(format!("Updated {} items.", r.updated));
                    selected.write().clear();
                }
                Err(e) => app.error(&e),
            }
        });
    });
    let selected_ids = move || selected.read().iter().copied().collect::<Vec<_>>();
    let mut monitoring_open = use_signal(|| false);
    let mut move_open = use_signal(|| false);
    let mut remove_open = use_signal(|| false);
    let mut organizing = use_signal(|| false);
    let mut confirm_organize = use_signal(|| false);
    let organize_client = use_client();
    let organize = move |_| {
        confirm_organize.set(false);
        let c = organize_client.clone();
        let ids = selected_ids();
        organizing.set(true);
        spawn(async move {
            match c
                .bulk_rename_series(&kitsunarr_domain::api::library::BulkRenameRequest { ids })
                .await
            {
                Ok(r) if r.problems.is_empty() => app.success(r.message),
                Ok(r) => app.toast(
                    crate::state::ToastLevel::Warning,
                    format!("{} Not renamed: {}", r.message, r.problems.join("; ")),
                ),
                Err(e) => app.error(&e),
            }
            organizing.set(false);
        });
    };

    rsx! {
        if confirm_organize() {
            crate::components::common::Confirm {
                title: "Organize files".to_string(),
                message: format!(
                    "Rename every file of the {} selected series to the naming settings? Use a series' Rename files preview to check names first.",
                    selected.read().len()
                ),
                confirm_label: "Organize".to_string(),
                on_confirm: organize,
                on_cancel: move |_| confirm_organize.set(false),
            }
        }
        if remove_open() {
            BulkRemoveDialog {
                series_ids: selected_ids(),
                on_close: move |removed: bool| {
                    remove_open.set(false);
                    if removed {
                        selected.write().clear();
                    }
                },
            }
        }
        if move_open() {
            BulkMoveDialog {
                series_ids: selected_ids(),
                on_close: move |_| move_open.set(false),
            }
        }
        if monitoring_open() {
            super::detail::MonitoringDialog {
                series_ids: selected_ids(),
                monitor_new_items: None,
                on_close: move |_| monitoring_open.set(false),
            }
        }
        div { class: "page-head",
            h1 { class: "grow", "{title}" }
            Link { to: Route::ImportWizard {}, class: "btn", "Import from Sonarr/Tanukarr" }
        }
        div { class: "toolbar",
            select {
                class: "select",
                "aria-label": "Sort by",
                value: "{sort_key}",
                onchange: move |e: FormEvent| sort_key.set(e.value()),
                option { value: "title", "Title" }
                option { value: "added", "Date added" }
                option { value: "year", "Year" }
                option { value: "size", "Size on disk" }
                option { value: "nextAiring", "Next airing" }
                option { value: "status", "Status" }
            }
            button { class: "btn icon", "aria-label": "Toggle sort direction", onclick: move |_| descending.toggle(), if descending() { "↓" } else { "↑" } }
            select {
                class: "select",
                "aria-label": "Filter by monitoring",
                onchange: move |e: FormEvent| monitored.set(match e.value().as_str() { "yes" => Some(true), "no" => Some(false), _ => None }),
                option { value: "all", "All" }
                option { value: "yes", {crate::i18n::t().monitored} }
                option { value: "no", {crate::i18n::t().unmonitored} }
            }
            div { class: "spacer" }
            ViewToggle { grid }
        }
        if !selected.read().is_empty() {
            div { class: "alert info", role: "region", "aria-label": "Bulk actions",
                div { class: "row grow",
                    strong { "{selected.read().len()} selected" }
                    button { class: "btn sm", onclick: move |_| bulk.call(BulkEdit { ids: selected_ids(), monitored: Some(true), ..Default::default() }), "Monitor" }
                    button { class: "btn sm", onclick: move |_| bulk.call(BulkEdit { ids: selected_ids(), monitored: Some(false), ..Default::default() }), "Unmonitor" }
                    button { class: "btn sm", title: "Choose which episodes of the selected series are monitored", onclick: move |_| monitoring_open.set(true), "Monitoring…" }
                    button { class: "btn sm", title: "Move the selected series into another root folder", onclick: move |_| move_open.set(true), "Move to root folder…" }
                    button { class: "btn sm", disabled: organizing(), title: "Rename every file of the selected series to the naming settings", onclick: move |_| confirm_organize.set(true),
                        if organizing() { "Organizing…" } else { "Organize" }
                    }
                    button { class: "btn sm danger", title: "Remove the selected series from Kitsunarr", onclick: move |_| remove_open.set(true), "Remove…" }
                    select {
                        class: "select",
                        "aria-label": "Quality profile",
                        onchange: move |e: FormEvent| bulk_profile.set(e.value()),
                        option { value: "", "Set quality profile…" }
                        if let Some(Ok(list)) = &*profiles.read() {
                            for p in list.iter() { option { value: "{p.id}", "{p.name}" } }
                        }
                    }
                    button {
                        class: "btn sm",
                        disabled: bulk_profile().is_empty(),
                        onclick: move |_| {
                            if let Ok(id) = bulk_profile().parse::<i64>() {
                                bulk.call(BulkEdit { ids: selected_ids(), quality_profile_id: Some(id), ..Default::default() });
                            }
                        },
                        "Apply profile"
                    }
                    div { class: "spacer" }
                    button { class: "btn ghost sm", onclick: move |_| selected.write().clear(), "Clear" }
                }
            }
        }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(p)) if p.records.is_empty() => rsx! {
                Empty { title: if search().is_empty() { format!("No {} yet", title.to_lowercase()) } else { "No matches".to_string() },
                    if search().is_empty() {
                        p { "Add titles from TVmaze or AniList, or bring over an existing Sonarr or Tanukarr library." }
                        div { class: "row", style: "justify-content: center",
                            Link { to: Route::AddMedia {}, class: "btn primary", "Add media" }
                            Link { to: Route::ImportWizard {}, class: "btn", "Import library" }
                        }
                    }
                }
            },
            Some(Ok(p)) => rsx! {
                if grid() {
                    div { class: "media-grid",
                        for item in p.records.iter() {
                            SeriesCard { key: "{item.id}", item: item.clone(), selected: selected.read().contains(&item.id) }
                        }
                    }
                } else {
                    SeriesTable {
                        items: p.records.clone(),
                        selected,
                        sort_key: sort_key(),
                        on_sort: move |k: String| { if sort_key() == k { descending.toggle(); } else { sort_key.set(k); } },
                    }
                }
                Pagination { page: p.page, page_size: p.page_size, total: p.total_records, on_page: move |n| page.set(n) }
            },
        }
    }
}

#[component]
pub fn MoviesLibrary() -> Element {
    let app = use_app();
    let client = use_client();
    let search = use_debounced(app.search, 300);
    let mut page = use_signal(|| 1u32);
    let mut sort_key = use_signal(|| "title".to_string());
    let mut watched = use_signal(|| None::<bool>);
    let grid = use_signal(|| crate::util::load_pref("kitsunarr-view").as_deref() != Some("table"));
    let mut selected = use_signal(HashSet::<i64>::new);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        let _ = search();
        if *page.peek() != 1 {
            page.set(1);
        }
        selected.write().clear();
    });
    use_effect(move || {
        crate::util::save_pref("kitsunarr-view", if grid() { "grid" } else { "table" })
    });
    let c0 = client.clone();
    let mut data = use_resource(move || {
        let _ = (app.library_rev)();
        let q = LibraryQuery {
            page: Some(page()),
            page_size: Some(PAGE_SIZE),
            sort_key: Some(sort_key()),
            search: Some(search()).filter(|s| !s.trim().is_empty()),
            watched: watched(),
            ..Default::default()
        };
        let c = c0.clone();
        async move { c.movies(&q).await }
    });
    let bulk = use_callback(move |(watch, monitor): (Option<bool>, Option<bool>)| {
        let ids = selected.read().iter().copied().collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            let result = if let Some(watched) = watch {
                c.set_movies_watched(&ids, watched).await
            } else {
                c.bulk_edit_movies(&BulkEdit {
                    ids,
                    monitored: monitor,
                    ..Default::default()
                })
                .await
            };
            match result {
                Ok(r) => {
                    app.success(format!("Updated {} items.", r.updated));
                    selected.write().clear();
                    data.restart();
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    });
    rsx! {
        div{class:"page-head",h1{class:"grow","Movies"}}
        div{class:"toolbar",
            select{class:"select","aria-label":"Sort by",value:sort_key(),onchange:move |e:FormEvent|{selected.write().clear();page.set(1);sort_key.set(e.value());},
                option{value:"title","Title"}option{value:"added","Date added"}option{value:"year","Year"}option{value:"size","Size on disk"}option{value:"watchedAt","Watched date"}
            }
            select{class:"select","aria-label":"Filter by watched state",onchange:move |e:FormEvent|{selected.write().clear();page.set(1);watched.set(match e.value().as_str(){"yes"=>Some(true),"no"=>Some(false),_=>None});},
                option{value:"all","All"}option{value:"yes","Watched"}option{value:"no","Unwatched"}
            }
            div{class:"spacer"}ViewToggle{grid}
        }
        if !selected.read().is_empty(){div{class:"alert info",role:"region","aria-label":"Movie bulk actions",
            div{class:"row grow",
                strong{"{selected.read().len()} selected"}
                button{class:"btn sm",disabled:busy(),onclick:move |_|bulk.call((Some(true),None)),"Mark watched"}
                button{class:"btn sm",disabled:busy(),onclick:move |_|bulk.call((Some(false),None)),"Mark unwatched"}
                button{class:"btn sm",disabled:busy(),onclick:move |_|bulk.call((None,Some(true))),"Monitor"}
                button{class:"btn sm",disabled:busy(),onclick:move |_|bulk.call((None,Some(false))),"Unmonitor"}
                button{class:"btn ghost sm",disabled:busy(),onclick:move |_|selected.write().clear(),"Clear"}
            }
        }}
        match &*data.read(){
            None=>rsx!{Loading{}},Some(Err(e))=>rsx!{ErrorBox{message:e.to_string()}},
            Some(Ok(p)) if p.records.is_empty()=>rsx!{
                Empty{title:if search().trim().is_empty() && watched().is_none(){"No movies yet"}else{"No matches"},
                    p{"Add a movie, or change the search and watched filter."}
                    div{class:"row",style:"justify-content:center",Link{to:Route::AddMedia{},class:"btn primary","Add media"}Link{to:Route::ImportWizard{},class:"btn","Import library"}}
                }
            },
            Some(Ok(p))=>rsx!{
                if grid(){
                    div{class:"media-grid",
                        for item in &p.records{
                            div{key:"{item.id}",style:"position:relative;min-width:0",
                                MovieCard{item:item.clone(),selected:selected.read().contains(&item.id)}
                                label{style:"position:absolute;top:.5rem;inset-inline-end:.5rem;padding:.3rem;background:var(--bg-elev);border-radius:var(--r-sm)",
                                    input{r#type:"checkbox","aria-label":"Select {item.title}",disabled:busy(),checked:selected.read().contains(&item.id),onchange:{let id=item.id;move |e:FormEvent|{if e.checked(){selected.write().insert(id);}else{selected.write().remove(&id);}}}}
                                }
                            }
                        }
                    }
                }else{MovieTable{items:p.records.clone(),selected,disabled:busy()}}
                Pagination{page:p.page,page_size:p.page_size,total:p.total_records,on_page:move |n|page.set(n)}
            },
        }
    }
}

/// Moves the selected series into another root folder (Sonarr's series editor root folder with
/// "move files"); each keeps its folder below the root folder.
#[component]
fn BulkMoveDialog(series_ids: Vec<i64>, on_close: EventHandler<()>) -> Element {
    use crate::components::common::{Field, Modal, Switch};
    use kitsunarr_domain::api::library::BulkMoveRequest;
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let roots = use_resource(move || {
        let c = c0.clone();
        async move { c.root_folders().await }
    });
    let mut root = use_signal(String::new);
    let mut move_files = use_signal(|| true);
    let mut busy = use_signal(|| false);
    let count = series_ids.len();
    let apply = move |_| {
        let c = client.clone();
        let req = BulkMoveRequest {
            ids: series_ids.clone(),
            root_folder_path: root(),
            move_files: move_files(),
        };
        busy.set(true);
        spawn(async move {
            match c.bulk_move_series(&req).await {
                Ok(r) if r.failures.is_empty() => {
                    app.success(format!("{} series moved.", r.moved.len()));
                    on_close.call(());
                }
                Ok(r) => {
                    app.toast(
                        crate::state::ToastLevel::Warning,
                        format!(
                            "{} series moved; not moved: {}",
                            r.moved.len(),
                            r.failures.join("; ")
                        ),
                    );
                    on_close.call(());
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    rsx! {
        Modal {
            title: format!("Move {count} series"),
            on_close: move |_| { if !busy() { on_close.call(()) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || root().is_empty(), onclick: apply,
                    if busy() { "Moving…" } else { "Move" }
                }
            },
            Field { label: "Root folder",
                select { class: "select", "aria-label": "Root folder", disabled: busy(),
                    onchange: move |e: FormEvent| root.set(e.value()),
                    option { value: "", selected: root().is_empty(), "Choose a root folder…" }
                    if let Some(Ok(list)) = &*roots.read() {
                        for r in list.iter() { option { key: "{r.id}", value: "{r.path}", "{r.path}" } }
                    }
                }
            }
            Switch { checked: move_files(), disabled: busy(), label: "Move the files", onchange: move |v| move_files.set(v) }
            p { class: "help small", "Each series keeps its folder below the root folder. A series whose new folder exists and is not empty keeps its current folder." }
        }
    }
}

/// Removes the selected series (Sonarr's series editor delete), optionally with their folders and
/// an import list exclusion.
#[component]
fn BulkRemoveDialog(series_ids: Vec<i64>, on_close: EventHandler<bool>) -> Element {
    use crate::components::common::{Modal, Switch};
    use kitsunarr_domain::api::library::BulkDeleteRequest;
    let app = use_app();
    let client = use_client();
    let mut delete_files = use_signal(|| false);
    let mut exclude = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let count = series_ids.len();
    let apply = move |_| {
        let c = client.clone();
        let req = BulkDeleteRequest {
            ids: series_ids.clone(),
            delete_files: delete_files(),
            add_import_list_exclusion: exclude(),
        };
        busy.set(true);
        spawn(async move {
            match c.bulk_delete_series(&req).await {
                Ok(r) if r.failures.is_empty() => {
                    app.success(format!("{} series removed.", r.deleted));
                }
                Ok(r) => app.toast(
                    crate::state::ToastLevel::Warning,
                    format!(
                        "{} series removed; kept: {}",
                        r.deleted,
                        r.failures.join("; ")
                    ),
                ),
                Err(e) => app.error(&e),
            }
            busy.set(false);
            on_close.call(true);
        });
    };
    rsx! {
        Modal {
            title: format!("Remove {count} series"),
            on_close: move |_| { if !busy() { on_close.call(false) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(false), "Cancel" }
                button { class: "btn danger solid", disabled: busy(), onclick: apply,
                    if busy() { "Removing…" } else { "Remove" }
                }
            },
            p { "Remove the {count} selected series and their episode records from Kitsunarr?" }
            Switch { checked: delete_files(), disabled: busy(), label: "Also delete their folders and files (to the recycle bin when one is set)", onchange: move |v| delete_files.set(v) }
            Switch { checked: exclude(), disabled: busy(), label: "Prevent import lists from adding them again", onchange: move |v| exclude.set(v) }
        }
    }
}
