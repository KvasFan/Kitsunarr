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
            if *generation.peek() == ticket {
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

    rsx! {
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
                        p { "Adding titles requires metadata providers, which are not implemented yet. You can import an existing Sonarr or Tanukarr library now." }
                        Link { to: Route::ImportWizard {}, class: "btn primary", "Import library" }
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
    let grid = use_signal(|| crate::util::load_pref("kitsunarr-view").as_deref() != Some("table"));
    use_effect(move || {
        let _ = (search(), sort_key());
        if *page.peek() != 1 {
            page.set(1);
        }
    });
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let q = LibraryQuery {
            page: Some(page()),
            page_size: Some(PAGE_SIZE),
            sort_key: Some(sort_key()),
            search: Some(search()).filter(|s| !s.trim().is_empty()),
            ..Default::default()
        };
        let c = client.clone();
        async move { c.movies(&q).await }
    });
    rsx! {
        div { class: "page-head", h1 { class: "grow", "Movies" } }
        div { class: "toolbar",
            select {
                class: "select",
                "aria-label": "Sort by",
                onchange: move |e: FormEvent| sort_key.set(e.value()),
                option { value: "title", "Title" }
                option { value: "added", "Date added" }
                option { value: "year", "Year" }
                option { value: "size", "Size on disk" }
            }
            div { class: "spacer" }
            ViewToggle { grid }
        }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(p)) if p.records.is_empty() => rsx! {
                Empty { title: "No movies yet",
                    p { "Movies come from Tanukarr migrations for now; adding movies needs the TMDB/OMDb providers, which are not implemented yet." }
                }
            },
            Some(Ok(p)) => rsx! {
                if grid() {
                    div { class: "media-grid",
                        for item in p.records.iter() { MovieCard { key: "{item.id}", item: item.clone() } }
                    }
                } else {
                    MovieTable { items: p.records.clone() }
                }
                Pagination { page: p.page, page_size: p.page_size, total: p.total_records, on_page: move |n| page.set(n) }
            },
        }
    }
}
