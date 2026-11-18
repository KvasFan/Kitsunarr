//! Library pages (TV series, anime, movies) with server-side paging, search, sort, filters,
//! poster and table views, and bulk editing.

use std::collections::HashSet;

use dioxus::prelude::*;
use kitsunarr_domain::api::library::{BulkEdit, LibraryQuery};
use kitsunarr_domain::{MediaKind, SortDirection};

use crate::components::common::{Empty, ErrorBox, Loading, Pagination};
use crate::components::media::{
    ColumnChooser, MOVIE_COLUMNS, MovieCard, MovieOverview, MovieTable, SERIES_COLUMNS, SeriesCard,
    SeriesOverview, SeriesTable, ViewSwitch, use_columns, use_view,
};
use crate::pages::library_filters::{self, FilterRef};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::{load_pref, save_pref};

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

#[component]
fn SeriesLibraryView(kind: MediaKind, title: &'static str) -> Element {
    let app = use_app();
    let client = use_client();
    let search = use_debounced(app.search, 300);
    let mut page = use_signal(|| 1u32);
    let mut sort_key = use_signal(|| "title".to_string());
    let mut descending = use_signal(|| false);
    // Tanukarr's series index options: the selected filter, the combined ones, the path
    // filter and "Group anime seasons" (on by default), kept per library.
    let pref = move |name: &str| format!("library.{}.{name}", kind.as_str());
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
    let mut group = use_signal(|| load_pref(&pref("group")).as_deref() != Some("false"));
    let mut saving_filter = use_signal(|| false);
    let view = use_view();
    let columns = use_columns(kind.as_str(), SERIES_COLUMNS);
    let tags = use_tags();
    let tag_list = move || tags.read().clone().unwrap_or_default();
    let mut selected = use_signal(HashSet::<i64>::new);
    let mut bulk_profile = use_signal(String::new);

    // Reset to the first page when filters change.
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
        save_pref(&pref("filter"), &filter().encode());
        let list: Vec<String> = combined().iter().map(FilterRef::encode).collect();
        save_pref(&pref("combined"), &list.join(","));
        save_pref(&pref("path"), &path());
        save_pref(&pref("group"), if group() { "true" } else { "false" });
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
        async move { c.series_path_filters(Some(kind)).await }
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
    let ct = client.clone();
    let total = use_resource(move || {
        let _ = (app.library_rev)();
        let c = ct.clone();
        async move {
            c.series(&LibraryQuery {
                kind: Some(kind),
                page_size: Some(1),
                ..Default::default()
            })
            .await
            .map(|p| p.total_records)
        }
    });
    let c = client.clone();
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let (filters, applied) = composed();
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
            group_anime_seasons: Some(group()),
            ..filters
        };
        let c = c.clone();
        async move {
            // Filters that contradict each other match nothing.
            if applied.conflict {
                return Ok(kitsunarr_domain::Page {
                    page: 1,
                    page_size: PAGE_SIZE,
                    total_records: 0,
                    records: Vec::new(),
                });
            }
            c.series(&q).await
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
            label { class: "check small", title: "Show each anime group (Tanukarr's anime grouping) once, by its first season",
                input { r#type: "checkbox", checked: group(), onchange: move |e: FormEvent| group.set(e.checked()) }
                "Group anime seasons"
            }
            button { class: "btn sm", disabled: !filtering(), title: "Save the current filters as a filter of your own", onclick: move |_| saving_filter.set(true), "Save filter…" }
            if let FilterRef::Saved(id) = filter() {
                DeleteSavedFilter { id, on_deleted: move |_| { filter.set(FilterRef::Preset("all".into())); saved_filters.restart(); } }
            }
            div { class: "spacer" }
            if let (Some(Ok(p)), Some(Ok(t))) = (&*data.read(), &*total.read()) {
                span { class: "small muted", "aria-label": "Shown series", "{p.total_records} / {t}" }
            }
            ColumnChooser { all: SERIES_COLUMNS, columns }
            ViewSwitch { view }
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
                Empty { title: if search().is_empty() && !filtering() { format!("No {} yet", title.to_lowercase()) } else { "No matches".to_string() },
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
            Some(Ok(p)) => rsx! {
                match view().as_str() {
                    "table" => rsx! {
                        SeriesTable {
                            items: p.records.clone(),
                            selected,
                            sort_key: sort_key(),
                            on_sort: move |k: String| { if sort_key() == k { descending.toggle(); } else { sort_key.set(k); } },
                            columns: Some(columns()),
                            tags: tag_list(),
                        }
                    },
                    "overview" => rsx! {
                        SeriesOverview { items: p.records.clone(), selected, columns: columns(), tags: tag_list() }
                    },
                    _ => rsx! {
                        div { class: "media-grid",
                            for item in p.records.iter() {
                                SeriesCard { key: "{item.id}", item: item.clone(), selected: selected.read().contains(&item.id), grouped: group(), columns: columns(), tags: tag_list() }
                            }
                        }
                    },
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
    let view = use_view();
    let columns = use_columns("movies", MOVIE_COLUMNS);
    let tags = use_tags();
    let tag_list = move || tags.read().clone().unwrap_or_default();
    let mut selected = use_signal(HashSet::<i64>::new);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        let _ = search();
        if *page.peek() != 1 {
            page.set(1);
        }
        selected.write().clear();
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
            div{class:"spacer"}ColumnChooser{all:MOVIE_COLUMNS,columns}ViewSwitch{view}
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
                if view()=="overview"{
                    MovieOverview{items:p.records.clone(),selected,columns:columns(),tags:tag_list(),disabled:busy()}
                }else if view()!="table"{
                    div{class:"media-grid",
                        for item in &p.records{
                            div{key:"{item.id}",style:"position:relative;min-width:0",
                                MovieCard{item:item.clone(),selected:selected.read().contains(&item.id),columns:columns(),tags:tag_list()}
                                label{style:"position:absolute;top:.5rem;inset-inline-end:.5rem;padding:.3rem;background:var(--bg-elev);border-radius:var(--r-sm)",
                                    input{r#type:"checkbox","aria-label":"Select {item.title}",disabled:busy(),checked:selected.read().contains(&item.id),onchange:{let id=item.id;move |e:FormEvent|{if e.checked(){selected.write().insert(id);}else{selected.write().remove(&id);}}}}
                                }
                            }
                        }
                    }
                }else{MovieTable{items:p.records.clone(),selected,disabled:busy(),columns:Some(columns()),tags:tag_list()}}
                Pagination{page:p.page,page_size:p.page_size,total:p.total_records,on_page:move |n|page.set(n)}
            },
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
