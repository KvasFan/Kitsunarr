//! The library, as Tanukarr's single series index: series, anime and movies in one list with
//! server-side paging, search, sort, filters, anime grouping (on by default), poster, table and
//! overview views, the jump bar, select mode with the mass editor, and the footer statistics.

use std::collections::HashSet;

use dioxus::prelude::*;
use kitsunarr_domain::SortDirection;
use kitsunarr_domain::api::library::{BulkEdit, LibraryIndex, LibraryKey, LibraryQuery};

use crate::components::common::{Empty, ErrorBox, Loading, Pagination};
use crate::components::media::{
    ColumnChooser, LibraryOverview, LibraryPosters, LibraryTable, SERIES_COLUMNS, SortMenu,
    ViewSwitch, use_columns, use_view,
};
use crate::pages::library_filters::{self, FilterRef};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, load_pref, save_pref};

const PAGE_SIZE: u32 = 60;

/// A preference of the library index (kept per browser, as Tanukarr's `series_options`).
fn pref(name: &str) -> String {
    format!("library.index.{name}")
}

/// Tanukarr's "Group anime seasons" starts on. Before the single list each library page kept
/// its own choice, written only as "false" when someone turned grouping off on the anime page;
/// that explicit choice carries over, nothing else does.
fn initial_grouping() -> bool {
    match load_pref(&pref("group")) {
        Some(saved) => saved != "false",
        None => load_pref("library.anime.group").as_deref() != Some("false"),
    }
}

/// The library (Tanukarr's "Series" page).
#[component]
pub fn SeriesLibrary() -> Element {
    rsx! { LibraryIndexPage {} }
}

/// The former anime page: the library with Tanukarr's "Anime" filter.
#[component]
pub fn AnimeLibrary() -> Element {
    rsx! { RedirectWithFilter { filter: "anime" } }
}

/// The former movie page: the library with the "Movies" filter.
#[component]
pub fn MoviesLibrary() -> Element {
    rsx! { RedirectWithFilter { filter: "movies" } }
}

/// Selects a filter of the library and opens it (old links keep showing the same titles).
#[component]
fn RedirectWithFilter(filter: &'static str) -> Element {
    let nav = navigator();
    use_effect(move || {
        save_pref(&pref("filter"), filter);
        save_pref(&pref("combined"), "");
        save_pref(&pref("path"), "");
        nav.replace(Route::SeriesLibrary {});
    });
    rsx! { Loading {} }
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

/// Tag labels for the index's tags column.
fn use_tags() -> Resource<Vec<kitsunarr_domain::api::settings::TagItem>> {
    let client = use_client();
    let app = use_app();
    use_resource(move || {
        let _ = (app.settings_rev)();
        let c = client.clone();
        async move { c.tags().await.unwrap_or_default() }
    })
}

fn empty_index() -> LibraryIndex {
    LibraryIndex {
        page: 1,
        page_size: PAGE_SIZE,
        total_records: 0,
        records: Vec::new(),
        filtered_items: 0,
        total_items: 0,
        has_groups: false,
        jump_bar: Vec::new(),
        footer: Default::default(),
        keys: Vec::new(),
    }
}

/// Brings an entry of the shown page into view (the jump bar).
fn scroll_to_entry(index: u64) -> bool {
    let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&format!("library-entry-{index}")))
    else {
        return false;
    };
    el.scroll_into_view();
    true
}

#[component]
fn LibraryIndexPage() -> Element {
    let app = use_app();
    let client = use_client();
    let search = use_debounced(app.search, 300);
    let mut page = use_signal(|| 1u32);
    // Tanukarr's options: sort, filters, path and grouping are remembered.
    let mut sort_key = use_signal(|| load_pref(&pref("sort")).unwrap_or_else(|| "title".into()));
    let mut descending =
        use_signal(|| load_pref(&pref("sortDirection")).as_deref() == Some("descending"));
    let mut filter = use_signal(|| {
        load_pref(&pref("filter"))
            .map(|v| FilterRef::parse(&v))
            .unwrap_or(FilterRef::Preset("all".into()))
    });
    let mut combined = use_signal(|| {
        load_pref(&pref("combined"))
            .unwrap_or_default()
            .split(',')
            .filter(|v| !v.is_empty())
            .map(FilterRef::parse)
            .collect::<Vec<_>>()
    });
    let mut path = use_signal(|| load_pref(&pref("path")).unwrap_or_default());
    let mut group = use_signal(initial_grouping);
    let mut saving_filter = use_signal(|| false);
    let view = use_view();
    let columns = use_columns("index", SERIES_COLUMNS);
    let tags = use_tags();
    let tag_list = move || tags.read().clone().unwrap_or_default();
    let mut select_mode = use_signal(|| false);
    let mut selected = use_signal(HashSet::<LibraryKey>::new);
    let mut bulk_profile = use_signal(String::new);
    let mut jump_to = use_signal(|| None::<u64>);

    // Back to the first page when the list changes.
    use_effect(move || {
        let _ = (
            search(),
            sort_key(),
            descending(),
            filter(),
            combined(),
            path(),
            group(),
        );
        if *page.peek() != 1 {
            page.set(1);
        }
    });
    use_effect(move || {
        save_pref(&pref("sort"), &sort_key());
        save_pref(
            &pref("sortDirection"),
            if descending() {
                "descending"
            } else {
                "ascending"
            },
        );
        save_pref(&pref("filter"), &filter().encode());
        let list: Vec<String> = combined().iter().map(FilterRef::encode).collect();
        save_pref(&pref("combined"), &list.join(","));
        save_pref(&pref("path"), &path());
    });
    // Saved filters (Sonarr's custom filters of the series index).
    let cf = client.clone();
    let mut saved_filters = use_resource(move || {
        let c = cf.clone();
        async move {
            c.custom_filters().await.map(|list| {
                list.into_iter()
                    .filter(|f| f.kind == library_filters::FILTER_TYPE)
                    .collect::<Vec<_>>()
            })
        }
    });
    let saved_list = move || match &*saved_filters.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    let cp = client.clone();
    let path_choices = use_resource(move || {
        let _ = (app.library_rev)();
        let c = cp.clone();
        async move { c.series_path_filters(None).await }
    });
    // The filter part of the query (what a saved filter stores).
    let composed = move || {
        let mut q = LibraryQuery::default();
        let p = path();
        if !p.trim().is_empty() {
            q.path = Some(p);
        }
        let mut chosen = vec![filter()];
        for f in combined() {
            if !chosen.contains(&f) {
                chosen.push(f);
            }
        }
        let applied = library_filters::apply(&chosen, &saved_list(), &mut q);
        (q, applied)
    };
    let query = move |page: u32, page_size: u32| {
        let (filters, applied) = composed();
        let q = LibraryQuery {
            page: Some(page),
            page_size: Some(page_size),
            sort_key: Some(sort_key()),
            sort_direction: Some(if descending() {
                SortDirection::Desc
            } else {
                SortDirection::Asc
            }),
            search: Some(search()).filter(|s| !s.trim().is_empty()),
            group_anime_seasons: Some(group()),
            ..filters
        };
        (q, applied.conflict)
    };
    let c = client.clone();
    let mut data = use_resource(move || {
        let _ = (app.library_rev)();
        let (q, conflict) = query(page(), PAGE_SIZE);
        let c = c.clone();
        async move {
            // Filters that contradict each other match nothing.
            if conflict {
                return Ok(empty_index());
            }
            c.library_index(&q).await
        }
    });
    // After a jump to another page, scroll once its entries are shown.
    use_effect(move || {
        let _ = data.read();
        if let Some(i) = jump_to()
            && scroll_to_entry(i)
        {
            jump_to.set(None);
        }
    });
    let filtering = move || {
        filter() != FilterRef::Preset("all".into()) || !combined().is_empty() || !path().is_empty()
    };
    let pc = client.clone();
    let profiles = use_resource(move || {
        let c = pc.clone();
        async move { c.quality_profiles().await }
    });

    let series_ids = move || {
        let mut ids: Vec<i64> = selected
            .read()
            .iter()
            .filter(|k| !k.movie)
            .map(|k| k.id)
            .collect();
        ids.sort_unstable();
        ids
    };
    let movie_ids = move || {
        let mut ids: Vec<i64> = selected
            .read()
            .iter()
            .filter(|k| k.movie)
            .map(|k| k.id)
            .collect();
        ids.sort_unstable();
        ids
    };
    let mut busy = use_signal(|| false);
    // Monitoring and the quality profile apply to the selected series and movies alike.
    let bc = client.clone();
    let bulk = use_callback(move |edit: BulkEdit| {
        let c = bc.clone();
        let (series, movies) = (series_ids(), movie_ids());
        busy.set(true);
        spawn(async move {
            let mut updated = 0;
            let mut failed = None;
            if !series.is_empty() {
                match c
                    .bulk_edit_series(&BulkEdit {
                        ids: series,
                        ..edit.clone()
                    })
                    .await
                {
                    Ok(r) => updated += r.updated,
                    Err(e) => failed = Some(e),
                }
            }
            if !movies.is_empty() && failed.is_none() {
                match c
                    .bulk_edit_movies(&BulkEdit {
                        ids: movies,
                        ..edit
                    })
                    .await
                {
                    Ok(r) => updated += r.updated,
                    Err(e) => failed = Some(e),
                }
            }
            match failed {
                None => {
                    app.success(format!("Updated {updated} items."));
                    selected.write().clear();
                }
                Some(e) => app.error(&e),
            }
            busy.set(false);
            data.restart();
        });
    });
    let wc = client.clone();
    let mark_watched = use_callback(move |watched: bool| {
        let c = wc.clone();
        let ids = movie_ids();
        busy.set(true);
        spawn(async move {
            match c.set_movies_watched(&ids, watched).await {
                Ok(r) => {
                    app.success(format!("Updated {} items.", r.updated));
                    selected.write().clear();
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
            data.restart();
        });
    });
    // Tanukarr's "Select all": every title the filters keep, on every page.
    let ac = client.clone();
    let select_all = move |_| {
        let (mut q, conflict) = query(1, 1);
        if conflict {
            return;
        }
        q.group_anime_seasons = Some(false);
        q.include_keys = Some(true);
        let c = ac.clone();
        spawn(async move {
            match c.library_index(&q).await {
                Ok(index) => selected.set(index.keys.into_iter().collect()),
                Err(e) => app.error(&e),
            }
        });
    };
    let on_sort = move |key: String| {
        // Tanukarr's `applySort`: the sorted key again reverses it, another keeps the direction.
        if sort_key() == key {
            descending.toggle();
        } else {
            sort_key.set(key);
        }
    };
    let mut jump = move |index: u64| {
        let target = (index / u64::from(PAGE_SIZE)) as u32 + 1;
        jump_to.set(Some(index));
        if page() != target {
            page.set(target);
        }
    };
    let mut monitoring_open = use_signal(|| false);
    let mut move_open = use_signal(|| false);
    let mut remove_open = use_signal(|| false);
    // Some(movie) opens the edit or tags dialog for the selected series (false) or movies.
    let mut edit_open = use_signal(|| None::<bool>);
    let mut tags_open = use_signal(|| None::<bool>);
    let mut convert_open = use_signal(|| false);
    let mut organizing = use_signal(|| false);
    let mut confirm_organize = use_signal(|| false);
    let organize_client = use_client();
    let organize = move |_| {
        confirm_organize.set(false);
        let c = organize_client.clone();
        let ids = series_ids();
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
    let loaded = match &*data.read() {
        Some(Ok(index)) => Some(index.clone()),
        _ => None,
    };
    let has_groups = loaded.as_ref().is_some_and(|i| i.has_groups);
    let n_series = selected.read().iter().filter(|k| !k.movie).count();
    let n_movies = selected.read().iter().filter(|k| k.movie).count();
    let n_selected = n_series + n_movies;
    let none_selected = n_selected == 0 || busy();

    rsx! {
        if confirm_organize() {
            crate::components::common::Confirm {
                title: "Organize files".to_string(),
                message: format!(
                    "Rename every file of the {} selected series to the naming settings? Use a series' Rename files preview to check names first.",
                    n_series
                ),
                confirm_label: "Organize".to_string(),
                on_confirm: organize,
                on_cancel: move |_| confirm_organize.set(false),
            }
        }
        if remove_open() {
            BulkRemoveDialog {
                series_ids: series_ids(),
                on_close: move |removed: bool| {
                    remove_open.set(false);
                    if removed {
                        selected.write().clear();
                    }
                },
            }
        }
        if let Some(movie) = edit_open() {
            super::edit::BulkEditDialog {
                movie,
                ids: if movie { movie_ids() } else { series_ids() },
                on_close: move |done: bool| {
                    edit_open.set(None);
                    if done {
                        selected.write().clear();
                        data.restart();
                    }
                },
            }
        }
        if let Some(movie) = tags_open() {
            super::edit::BulkTagsDialog {
                movie,
                ids: if movie { movie_ids() } else { series_ids() },
                on_close: move |done: bool| {
                    tags_open.set(None);
                    if done {
                        selected.write().clear();
                        data.restart();
                    }
                },
            }
        }
        if convert_open() {
            super::anime_conversion::BulkAnimeConversionDialog {
                series_ids: series_ids(),
                on_close: move |done: bool| {
                    convert_open.set(false);
                    if done {
                        selected.write().clear();
                    }
                },
            }
        }
        if move_open() {
            BulkMoveDialog {
                series_ids: series_ids(),
                on_close: move |_| move_open.set(false),
            }
        }
        if monitoring_open() {
            super::detail::MonitoringDialog {
                series_ids: series_ids(),
                monitor_new_items: None,
                on_close: move |_| monitoring_open.set(false),
            }
        }
        div { class: "page-head",
            h1 { class: "grow", "Series" }
            Link { to: Route::Collections {}, class: "btn", "Collections" }
            Link { to: Route::ImportWizard {}, class: "btn", "Import from Sonarr/Tanukarr" }
        }
        div { class: "toolbar",
            button {
                class: if select_mode() { "btn sm on" } else { "btn sm" },
                "aria-pressed": if select_mode() { "true" } else { "false" },
                onclick: move |_| {
                    select_mode.toggle();
                    selected.write().clear();
                },
                if select_mode() { "Stop selecting" } else { "Select multiple series" }
            }
            if select_mode() {
                button { class: "btn sm", onclick: select_all, "Select all" }
            }
            div { class: "spacer" }
            if has_groups {
                button {
                    class: "btn sm",
                    onclick: move |_| {
                        group.toggle();
                        save_pref(&pref("group"), if group() { "true" } else { "false" });
                        if let Some(w) = web_sys::window() { w.scroll_to_with_x_and_y(0.0, 0.0); }
                    },
                    if group() { "Show individual series" } else { "Show groups" }
                }
            }
            ColumnChooser { all: SERIES_COLUMNS, columns }
            ViewSwitch { view }
            SortMenu { sort_key: sort_key(), descending: descending(), on_sort }
            select {
                class: "select",
                "aria-label": "Filter",
                value: "{filter().encode()}",
                onchange: move |e: FormEvent| filter.set(FilterRef::parse(&e.value())),
                for (key, label) in library_filters::PRESETS.iter() {
                    option { key: "{key}", value: "{key}", selected: filter() == FilterRef::Preset(key.to_string()), "{label}" }
                }
                for f in saved_list() {
                    option { key: "custom-{f.id}", value: "custom:{f.id}", selected: filter() == FilterRef::Saved(f.id), "{f.label}" }
                }
            }
            details { class: "combine",
                summary { class: "btn sm",
                    "Combine filters"
                    if !combined().is_empty() { " ({combined().len()})" }
                }
                fieldset { class: "menu stack tight",
                    legend { class: "small muted", "Show series matching every selected filter" }
                    for (key, label) in library_filters::PRESETS.iter().filter(|(k, _)| *k != "all") {
                        {
                            let f = FilterRef::Preset(key.to_string());
                            let on = combined().contains(&f);
                            rsx! {
                                label { key: "{key}", class: "check small",
                                    input { r#type: "checkbox", checked: on,
                                        onchange: move |e: FormEvent| { let mut c = combined.write(); c.retain(|x| *x != f); if e.checked() { c.push(f.clone()); } } }
                                    "{label}"
                                }
                            }
                        }
                    }
                    for saved in saved_list() {
                        {
                            let f = FilterRef::Saved(saved.id);
                            let on = combined().contains(&f);
                            rsx! {
                                label { key: "custom-{saved.id}", class: "check small",
                                    input { r#type: "checkbox", checked: on,
                                        onchange: move |e: FormEvent| { let mut c = combined.write(); c.retain(|x| *x != f); if e.checked() { c.push(f.clone()); } } }
                                    "{saved.label}"
                                }
                            }
                        }
                    }
                    button { class: "btn sm ghost", onclick: move |_| combined.write().clear(), "Clear combined filters" }
                }
            }
            button { class: "btn sm", disabled: !filtering(), title: "Save the current filters as a filter of your own", onclick: move |_| saving_filter.set(true), "Save filter…" }
            if let FilterRef::Saved(id) = filter() {
                DeleteSavedFilter { id, on_deleted: move |_| { filter.set(FilterRef::Preset("all".into())); saved_filters.restart(); } }
            }
        }
        // Tanukarr's path filter, with the count of titles shown.
        div { class: "toolbar",
            select {
                class: "select",
                "aria-label": "Filter by path",
                value: "{path}",
                onchange: move |e: FormEvent| path.set(e.value()),
                option { value: "", "All paths" }
                if let Some(Ok(list)) = &*path_choices.read() {
                    for p in list.iter() { option { key: "{p}", value: "{p}", selected: path() == *p, "{p}" } }
                }
                // A path stored earlier that no longer exists stays visible so it can be cleared.
                if !path().is_empty() && !matches!(&*path_choices.read(), Some(Ok(list)) if list.contains(&path())) {
                    option { value: "{path}", selected: true, "{path}" }
                }
            }
            if !path().is_empty() {
                button { class: "btn sm ghost", onclick: move |_| path.set(String::new()), "Clear" }
            }
            if let Some(index) = &loaded {
                span { class: "small muted", "aria-label": "Shown series", "{index.filtered_items} / {index.total_items}" }
            }
        }
        {
            let (_, applied) = composed();
            rsx! {
                if !applied.ignored.is_empty() {
                    div { class: "alert warning small", role: "status",
                        "Kitsunarr cannot apply these conditions of the saved filter, so they are left out: "
                        {applied.ignored.join(", ")}
                    }
                }
                if applied.conflict {
                    div { class: "alert info small", role: "status", "The selected filters contradict each other, so no series can match all of them." }
                }
            }
        }
        if saving_filter() {
            SaveFilterDialog {
                conditions: library_filters::conditions_of(&composed().0),
                on_close: move |saved: Option<i64>| {
                    saving_filter.set(false);
                    if let Some(id) = saved {
                        saved_filters.restart();
                        filter.set(FilterRef::Saved(id));
                        combined.write().clear();
                        path.set(String::new());
                    }
                },
            }
        }
        // Tanukarr's select footer (the mass editor) while selecting.
        if select_mode() {
            div { class: "alert info", role: "region", "aria-label": "Bulk actions",
                div { class: "row grow",
                    strong { "{n_selected} selected" }
                    button { class: "btn sm", disabled: none_selected, onclick: move |_| bulk.call(BulkEdit { monitored: Some(true), ..Default::default() }), "Monitor" }
                    button { class: "btn sm", disabled: none_selected, onclick: move |_| bulk.call(BulkEdit { monitored: Some(false), ..Default::default() }), "Unmonitor" }
                    if n_series > 0 {
                        button { class: "btn sm", title: "Choose which episodes of the selected series are monitored", onclick: move |_| monitoring_open.set(true), "Monitoring…" }
                    }
                    if n_series > 0 || n_movies > 0 {
                        button { class: "btn sm", disabled: busy(), title: "Monitoring, quality profile, series type and season folders of the selected series", onclick: move |_| edit_open.set(Some(n_series == 0)), "Edit…" }
                        button { class: "btn sm", disabled: busy(), title: "Add, remove or replace the tags of the selected series", onclick: move |_| tags_open.set(Some(n_series == 0)), "Tags…" }
                    }
                    if n_series > 0 && n_movies > 0 {
                        button { class: "btn sm", disabled: busy(), title: "Monitoring and quality profile of the selected movies", onclick: move |_| edit_open.set(Some(true)), "Edit movies…" }
                        button { class: "btn sm", disabled: busy(), title: "Add, remove or replace the tags of the selected movies", onclick: move |_| tags_open.set(Some(true)), "Movie tags…" }
                    }
                    if n_movies > 0 {
                        button { class: "btn sm", disabled: busy(), onclick: move |_| mark_watched.call(true), "Mark watched" }
                        button { class: "btn sm", disabled: busy(), onclick: move |_| mark_watched.call(false), "Mark unwatched" }
                    }
                    if n_series > 0 {
                        button { class: "btn sm", title: "Move the selected series into another root folder", onclick: move |_| move_open.set(true), "Move to root folder…" }
                        button { class: "btn sm", title: "Split the selected TheTVDB series into AniList seasons grouped as in Tanukarr", onclick: move |_| convert_open.set(true), "Convert to AniList groups…" }
                        button { class: "btn sm", disabled: organizing(), title: "Rename every file of the selected series to the naming settings", onclick: move |_| confirm_organize.set(true),
                            if organizing() { "Organizing…" } else { "Organize" }
                        }
                        button { class: "btn sm danger", title: "Remove the selected series from Kitsunarr", onclick: move |_| remove_open.set(true), "Remove…" }
                    }
                    select {
                        class: "select",
                        "aria-label": "Quality profile",
                        disabled: none_selected,
                        onchange: move |e: FormEvent| bulk_profile.set(e.value()),
                        option { value: "", "Set quality profile…" }
                        if let Some(Ok(list)) = &*profiles.read() {
                            for p in list.iter() { option { value: "{p.id}", "{p.name}" } }
                        }
                    }
                    button {
                        class: "btn sm",
                        disabled: none_selected || bulk_profile().is_empty(),
                        onclick: move |_| {
                            if let Ok(id) = bulk_profile().parse::<i64>() {
                                bulk.call(BulkEdit { quality_profile_id: Some(id), ..Default::default() });
                            }
                        },
                        "Apply profile"
                    }
                    div { class: "spacer" }
                    button { class: "btn ghost sm", disabled: n_selected == 0, onclick: move |_| selected.write().clear(), "Clear" }
                }
            }
        }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(p)) if p.records.is_empty() => rsx! {
                Empty { title: if search().is_empty() && !filtering() { "No series yet".to_string() } else { "No matches".to_string() },
                    if filtering() {
                        p { "No series match the selected filters." }
                        button { class: "btn", onclick: move |_| { filter.set(FilterRef::Preset("all".into())); combined.write().clear(); path.set(String::new()); }, "Clear filters" }
                    } else if search().is_empty() {
                        p { "Add titles from TVmaze or AniList, or bring over an existing Sonarr or Tanukarr library." }
                        div { class: "row", style: "justify-content: center",
                            Link { to: Route::AddMedia {}, class: "btn primary", "Add media" }
                            Link { to: Route::ImportWizard {}, class: "btn", "Import library" }
                        }
                    }
                }
            },
            Some(Ok(p)) => {
                let first_index = u64::from(p.page.saturating_sub(1)) * u64::from(p.page_size);
                let jump_bar = p.jump_bar.clone();
                let has_jump_bar = !jump_bar.is_empty();
                let footer = p.footer;
                rsx! {
                    div { class: if !has_jump_bar { "library-body" } else { "library-body has-jump-bar" },
                        div { class: "library-entries",
                            match view().as_str() {
                                "table" => rsx! {
                                    LibraryTable {
                                        entries: p.records.clone(),
                                        selected,
                                        select_mode: select_mode(),
                                        sort_key: sort_key(),
                                        descending: descending(),
                                        on_sort,
                                        columns: columns(),
                                        tags: tag_list(),
                                        first_index,
                                    }
                                },
                                "overview" => rsx! {
                                    LibraryOverview { entries: p.records.clone(), selected, select_mode: select_mode(), columns: columns(), tags: tag_list(), first_index }
                                },
                                _ => rsx! {
                                    LibraryPosters { entries: p.records.clone(), selected, select_mode: select_mode(), columns: columns(), tags: tag_list(), first_index }
                                },
                            }
                            Pagination { page: p.page, page_size: p.page_size, total: p.total_records, on_page: move |n| page.set(n) }
                        }
                        // Tanukarr's jump bar (sorted by title): a letter opens the page it starts on.
                        if has_jump_bar {
                            nav { class: "jump-bar", "aria-label": "Jump to",
                                for item in jump_bar {
                                    button { key: "{item.character}", class: "btn ghost xs", onclick: move |_| jump(item.index), "{item.character}" }
                                }
                            }
                        }
                    }
                    // Tanukarr's footer: the whole library.
                    div { class: "library-footer",
                        dl { class: "footer-stats small",
                            div { dt { "Series" } dd { "{footer.titles}" } }
                            div { dt { "Ended" } dd { "{footer.ended}" } }
                            div { dt { "Continuing" } dd { "{footer.continuing}" } }
                            div { dt { "Monitored" } dd { "{footer.monitored}" } }
                            div { dt { "Unmonitored" } dd { "{footer.unmonitored}" } }
                            div { dt { "Episodes" } dd { "{footer.episodes}" } }
                            div { dt { "Files" } dd { "{footer.files}" } }
                            div { dt { "Total file size" } dd { {format_bytes(footer.size_on_disk)} } }
                        }
                    }
                }
            }
        }
    }
}

/// Names the current filters and saves them as a custom filter (Sonarr's
/// `CustomFilterResource`, list `series`).
#[component]
fn SaveFilterDialog(
    conditions: Vec<kitsunarr_client::contracts::CustomFilterCondition>,
    on_close: EventHandler<Option<i64>>,
) -> Element {
    use crate::components::common::{Field, Modal};
    let app = use_app();
    let client = use_client();
    let mut label = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let save = move |_| {
        let c = client.clone();
        let filter = kitsunarr_client::contracts::CustomFilter {
            id: 0,
            kind: library_filters::FILTER_TYPE.into(),
            label: label().trim().to_string(),
            filters: conditions.clone(),
        };
        busy.set(true);
        spawn(async move {
            match c.create_custom_filter(&filter).await {
                Ok(saved) => {
                    app.success(format!("Filter \"{}\" saved.", saved.label));
                    on_close.call(Some(saved.id));
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    rsx! {
        Modal { title: "Save filter".to_string(), on_close: move |_| on_close.call(None),
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(None), "Cancel" }
                button { class: "btn primary", disabled: busy() || label().trim().is_empty(), onclick: save, "Save" }
            },
            p { class: "small faint", "The filter keeps the selected filter, the combined filters and the path; it is listed with the built-in filters." }
            Field { label: "Label",
                input { class: "input", "aria-label": "Filter label", value: "{label}", oninput: move |e| label.set(e.value()) }
            }
        }
    }
}

/// Deletes the selected saved filter, after confirmation.
#[component]
fn DeleteSavedFilter(id: i64, on_deleted: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let mut confirm = use_signal(|| false);
    let delete = move |_| {
        confirm.set(false);
        let c = client.clone();
        spawn(async move {
            match c.delete_custom_filter(id).await {
                Ok(()) => on_deleted.call(()),
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        button { class: "btn sm ghost danger", onclick: move |_| confirm.set(true), "Delete saved filter" }
        if confirm() {
            crate::components::common::Confirm {
                title: "Delete saved filter".to_string(),
                message: "Delete this saved filter? The series are not changed.".to_string(),
                confirm_label: "Delete".to_string(),
                danger: true,
                on_confirm: delete,
                on_cancel: move |_| confirm.set(false),
            }
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
