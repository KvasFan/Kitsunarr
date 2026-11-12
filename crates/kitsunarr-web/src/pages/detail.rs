//! Series and movie detail pages.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::library::{
    EpisodeItem, EpisodesMonitoredUpdate, SeasonMonitoring, SeriesUpdate,
};
use kitsunarr_domain::api::metadata::SUPPORTED_CATALOGUES;

use crate::components::common::{Badge, Confirm, ErrorBox, Loading, Modal, Switch};
use crate::components::media::Poster;
use crate::components::releases::ReleaseSearch;
use crate::components::vlist::VirtualList;
use crate::route::Route;
use crate::state::{ToastLevel, use_app, use_client};
use crate::util::{format_bytes, format_date, relative};

/// Episode row height in rems (42 px at the default text size).
const ROW_HEIGHT_REM: f64 = 2.625;

/// The toast for an automatic search's outcome.
fn automatic_summary(
    label: &str,
    r: &kitsunarr_domain::api::search::AutomaticSearchResult,
) -> (ToastLevel, String) {
    if r.searched_indexers == 0 {
        return (
            ToastLevel::Warning,
            "No indexer can search this series automatically: none has automatic search enabled and either no tags or one of the series' tags.".into(),
        );
    }
    let mut parts = Vec::new();
    if r.grabbed.is_empty() {
        parts.push(format!(
            "Automatic search for {label}: {} releases found, {} approved, nothing grabbed.",
            r.found, r.approved
        ));
    } else {
        let titles: Vec<String> = r
            .grabbed
            .iter()
            .map(|g| format!("{} ({})", g.title, g.download_client))
            .collect();
        parts.push(format!(
            "Automatic search for {label}: grabbed {}.",
            titles.join(", ")
        ));
    }
    for f in &r.grab_failures {
        parts.push(format!("Could not grab {}: {}", f.title, f.message));
    }
    if !r.failures.is_empty() {
        for f in &r.failures {
            parts.push(format!("{} failed: {}", f.indexer, f.message));
        }
    }
    let level = if !r.grab_failures.is_empty() || !r.failures.is_empty() {
        ToastLevel::Warning
    } else if r.grabbed.is_empty() {
        ToastLevel::Info
    } else {
        ToastLevel::Success
    };
    (level, parts.join(" "))
}

/// "Refresh metadata" button. Disabled, with an explanation, for titles owned by a catalogue
/// Kitsunarr cannot query yet (for example TheTVDB series migrated from Sonarr).
#[component]
fn RefreshButton(provider: MetadataProvider, busy: bool, on_click: EventHandler<()>) -> Element {
    let supported = SUPPORTED_CATALOGUES.contains(&provider);
    let hint = if supported {
        format!("Fetch the latest metadata from {}", provider.display_name())
    } else {
        format!(
            "Refreshing from {} is not supported yet; the imported metadata is kept as is",
            provider.display_name()
        )
    };
    rsx! {
        button {
            class: "btn",
            disabled: busy || !supported,
            title: "{hint}",
            onclick: move |_| on_click.call(()),
            if busy { "Refreshing…" } else { "Refresh metadata" }
        }
    }
}

#[component]
pub fn SeriesDetailPage(id: i64) -> Element {
    let app = use_app();
    let client = use_client();
    let nav = navigator();
    let mut confirm_delete = use_signal(|| false);
    let mut delete_folder = use_signal(|| false);
    let mut renaming = use_signal(|| false);
    let mut scanning = use_signal(|| false);
    // An episode file to delete: (file id, relative path).
    let mut deleting_file = use_signal(|| None::<(i64, String)>);
    // Interactive search target: (season, episode id, label).
    let mut searching = use_signal(|| None::<(Option<i32>, Option<i64>, String)>);
    let mut searching_complete = use_signal(|| false);
    let c1 = client.clone();
    let detail = use_resource(move || {
        let _ = (app.library_rev)();
        let c = c1.clone();
        async move { c.series_detail(id).await }
    });
    let c2 = client.clone();
    let mut episodes = use_resource(move || {
        let _ = (app.library_rev)();
        let c = c2.clone();
        async move { c.episodes(id).await }
    });

    let c3 = client.clone();
    let update = move |u: SeriesUpdate| {
        let c = c3.clone();
        spawn(async move {
            if let Err(e) = c.update_series(id, &u).await {
                app.error(&e);
            }
        });
    };
    let mut refreshing = use_signal(|| false);
    let c5 = client.clone();
    let refresh = move |_| {
        let c = c5.clone();
        refreshing.set(true);
        spawn(async move {
            match c.refresh_series(id).await {
                Ok(r) => app.success(format!(
                    "Metadata refreshed: {} new episodes, {} updated.",
                    r.added_episodes, r.updated_episodes
                )),
                Err(e) => app.error(&e),
            }
            refreshing.set(false);
        });
    };
    let c6 = client.clone();
    let auto_search = move |(season, episode_id, label): (Option<i32>, Option<i64>, String)| {
        let c = c6.clone();
        app.toast(ToastLevel::Info, format!("Searching for {label}…"));
        spawn(async move {
            let q = kitsunarr_domain::api::search::ReleaseQuery {
                series_id: id,
                season_number: season,
                episode_id,
                complete_series: false,
            };
            match c.automatic_search(&q).await {
                Ok(r) => {
                    let (level, message) = automatic_summary(&label, &r);
                    app.toast(level, message);
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c4 = client.clone();
    let delete = move |_| {
        let c = c4.clone();
        let with_files = delete_folder();
        confirm_delete.set(false);
        spawn(async move {
            let done = if with_files {
                c.delete_series_with_files(id).await
            } else {
                c.delete_series(id).await
            };
            match done {
                Ok(()) => {
                    app.success(if with_files {
                        "Series removed, and its folder deleted."
                    } else {
                        "Series removed from Kitsunarr. Files on disk were not touched."
                    });
                    nav.push(Route::SeriesLibrary {});
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c7 = client.clone();
    let rescan = move |_| {
        let c = c7.clone();
        scanning.set(true);
        spawn(async move {
            match c.rescan_series(id).await {
                Ok(r) if r.problems.is_empty() => {
                    app.success(format!("Disk scanned: {}", r.message))
                }
                Ok(r) => app.toast(
                    ToastLevel::Warning,
                    format!(
                        "Disk scanned: {} Not matched: {}",
                        r.message,
                        r.problems.join("; ")
                    ),
                ),
                Err(e) => app.error(&e),
            }
            scanning.set(false);
        });
    };
    let c8 = client.clone();
    let delete_file = move |_| {
        let Some((file_id, _)) = deleting_file() else {
            return;
        };
        deleting_file.set(None);
        let c = c8.clone();
        spawn(async move {
            match c.delete_episode_file(id, file_id).await {
                Ok(()) => {
                    app.success("File deleted.");
                    episodes.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };

    let detail_read = detail.read();
    let d = match &*detail_read {
        None => return rsx! { Loading {} },
        Some(Err(e)) => return rsx! { ErrorBox { message: e.to_string() } },
        Some(Ok(d)) => d.clone(),
    };
    drop(detail_read);
    let poster = d
        .images
        .iter()
        .find(|i| i.cover_type == "poster")
        .map(|i| i.remote_url.clone());
    let by_season: BTreeMap<i32, Vec<EpisodeItem>> = match &*episodes.read() {
        Some(Ok(list)) => {
            let mut m: BTreeMap<i32, Vec<EpisodeItem>> = BTreeMap::new();
            for e in list {
                m.entry(e.season_number).or_default().push(e.clone());
            }
            m
        }
        _ => BTreeMap::new(),
    };
    let monitored = d.item.monitored;
    let update_monitored = update.clone();
    let title = d.item.title.clone();

    rsx! {
        div { class: "hero",
            Poster { url: poster, title: d.item.title.clone(), monitored }
            div {
                div { class: "row tight",
                    h1 { style: "margin: 0", "{d.item.title}" }
                    if let Some(y) = d.item.year { span { class: "muted", "({y})" } }
                }
                div { class: "row tight", style: "margin: var(--sp-2) 0 var(--sp-3)",
                    Badge { text: d.item.kind.to_string(), tone: "accent" }
                    Badge { text: d.item.status.to_string() }
                    Badge { text: d.item.series_type.to_string() }
                    Badge { text: format!("{} · {}", d.item.metadata_provider.display_name(), d.metadata_id), tone: "info" }
                }
                if let Some(o) = d.overview.as_ref().filter(|o| !o.is_empty()) { p { class: "muted", "{o}" } }
                dl { class: "kv",
                    dt { "Path" } dd { class: "mono", "{d.item.path}" }
                    dt { "Quality profile" } dd { "{d.item.quality_profile_name}" }
                    dt { "Network" } dd { {d.item.network.clone().unwrap_or_else(|| "—".into())} }
                    dt { "Episodes" } dd { "{d.item.episode_file_count} / {d.item.episode_count} on disk · {format_bytes(d.item.size_on_disk)}" }
                    if !d.external_ids.is_empty() {
                        dt { "Identities" }
                        dd { {d.external_ids.iter().map(|x| format!("{}:{}", x.kind, x.value)).collect::<Vec<_>>().join("  ")} }
                    }
                    if !d.alternate_titles.is_empty() {
                        dt { "Also known as" } dd { {d.alternate_titles.join(" · ")} }
                    }
                    if !d.collections.is_empty() {
                        dt { "Collections" } dd { {d.collections.iter().map(|c| c.title.clone()).collect::<Vec<_>>().join(", ")} }
                    }
                    dt { "Metadata" }
                    dd { {d.last_refreshed_at.as_deref().map(|t| format!("refreshed {}", relative(t))).unwrap_or_else(|| "never refreshed".into())} }
                }
                div { class: "row", style: "margin-top: var(--sp-4)",
                    Switch {
                        checked: monitored,
                        label: if monitored { "Monitored".to_string() } else { "Unmonitored".to_string() },
                        onchange: move |v: bool| update_monitored(SeriesUpdate { monitored: Some(v), ..Default::default() }),
                    }
                    div { class: "spacer" }
                    RefreshButton { provider: d.item.metadata_provider, busy: refreshing(), on_click: refresh }
                    button { class: "btn", disabled: scanning(), title: "Find files added to or removed from the series folder", onclick: rescan,
                        if scanning() { "Scanning…" } else { "Scan disk" }
                    }
                    button { class: "btn", title: "Rename files to the naming settings", onclick: move |_| renaming.set(true), "Rename files" }
                    button { class: "btn", title: "Interactive search for packs of the whole series (and of its seasons) on torrent indexers", onclick: move |_| searching_complete.set(true), "Complete series" }
                    button { class: "btn danger", onclick: move |_| { delete_folder.set(false); confirm_delete.set(true) }, "Remove from library" }
                }
            }
        }
        if searching_complete() {
            Modal {
                title: format!("Search: {} — complete series", d.item.title),
                wide: true,
                on_close: move |_| searching_complete.set(false),
                ReleaseSearch { series_id: id, complete_series: true }
            }
        }
        if let Some((season, episode_id, label)) = searching() {
            Modal {
                title: format!("Search: {} — {label}", d.item.title),
                wide: true,
                on_close: move |_| searching.set(None),
                ReleaseSearch { key: "{season:?}-{episode_id:?}", series_id: id, season, episode_id }
            }
        }
        if confirm_delete() {
            Modal {
                title: "Remove series".to_string(),
                on_close: move |_| confirm_delete.set(false),
                footer: rsx! {
                    div { class: "spacer" }
                    button { class: "btn", onclick: move |_| confirm_delete.set(false), "Cancel" }
                    button { class: "btn danger solid", onclick: delete, "Remove" }
                },
                p { "Remove “{title}” and its episode records from Kitsunarr?" }
                label { class: "check",
                    input { r#type: "checkbox", checked: delete_folder(), onchange: move |e: FormEvent| delete_folder.set(e.checked()) }
                    "Also delete the series folder and its files (to the recycle bin when one is set)"
                }
            }
        }
        if renaming() {
            RenameDialog { series_id: id, on_close: move |_| { renaming.set(false); episodes.restart(); } }
        }
        if let Some((_, path)) = deleting_file() {
            Confirm {
                title: "Delete file",
                message: format!("Delete “{path}”? It goes to the recycle bin when one is set."),
                confirm_label: "Delete",
                danger: true,
                on_confirm: delete_file,
                on_cancel: move |_| deleting_file.set(None),
            }
        }
        h2 { "Seasons" }
        if episodes.read().is_none() { Loading {} }
        for season in d.seasons.iter().rev() {
            SeasonPanel {
                key: "{season.season_number}",
                series_id: id,
                season_number: season.season_number,
                monitored: season.monitored,
                episode_count: season.episode_count,
                file_count: season.episode_file_count,
                size: season.size_on_disk,
                episodes: by_season.get(&season.season_number).cloned().unwrap_or_default(),
                on_season_monitor: {
                    let update = update.clone();
                    move |(n, v): (i32, bool)| update(SeriesUpdate { seasons: Some(vec![SeasonMonitoring { season_number: n, monitored: v }]), ..Default::default() })
                },
                on_changed: move |_| episodes.restart(),
                on_delete_file: move |target| deleting_file.set(Some(target)),
                on_search: move |target| searching.set(Some(target)),
                on_auto_search: auto_search.clone(),
            }
        }
    }
}

#[component]
fn SeasonPanel(
    series_id: i64,
    season_number: i32,
    monitored: bool,
    episode_count: i64,
    file_count: i64,
    size: i64,
    episodes: Vec<EpisodeItem>,
    on_season_monitor: EventHandler<(i32, bool)>,
    on_changed: EventHandler<()>,
    on_delete_file: EventHandler<(i64, String)>,
    on_search: EventHandler<(Option<i32>, Option<i64>, String)>,
    on_auto_search: EventHandler<(Option<i32>, Option<i64>, String)>,
) -> Element {
    let app = use_app();
    let client = use_client();
    let mut open = use_signal(|| season_number > 0 && episodes.len() <= 60);
    let label = if season_number == 0 {
        "Specials".to_string()
    } else {
        format!("Season {season_number}")
    };
    let complete = episode_count > 0 && file_count >= episode_count;
    let list = episodes.clone();
    let toggle_episode = move |(ep_id, value): (i64, bool)| {
        let c = client.clone();
        spawn(async move {
            match c
                .set_episodes_monitored(&EpisodesMonitoredUpdate {
                    episode_ids: vec![ep_id],
                    monitored: value,
                })
                .await
            {
                Ok(_) => on_changed.call(()),
                Err(e) => app.error(&e),
            }
        });
    };
    // The list shows every row up to a large cap; CSS limits it to part of the viewport, so it
    // follows the window (and browser zoom) instead of a fixed height.
    let row_height = ROW_HEIGHT_REM * crate::util::rem_px();
    let height = (list.len() as f64 * row_height).clamp(row_height, 40.0 * row_height);
    rsx! {
        section { class: "season",
            div { class: "season-head", role: "button", tabindex: "0", "aria-expanded": "{open}", onclick: move |_| open.toggle(),
                span { if open() { "▾" } else { "▸" } }
                strong { class: "grow", "{label}" }
                span { class: if complete { "badge success" } else { "badge" }, "{file_count}/{episode_count}" }
                span { class: "small muted hide-mobile", {format_bytes(size)} }
                span { onclick: move |e| e.stop_propagation(),
                    Switch { checked: monitored, label: "Monitored", onchange: move |v| on_season_monitor.call((season_number, v)) }
                }
                button {
                    class: "btn sm ghost",
                    "aria-label": "Automatic search for {label}",
                    title: "Search the indexers and grab the best releases for the monitored episodes",
                    onclick: {
                        let label = label.clone();
                        move |e: MouseEvent| {
                            e.stop_propagation();
                            on_auto_search.call((Some(season_number), None, label.clone()));
                        }
                    },
                    "Search"
                }
                button {
                    class: "btn sm ghost",
                    "aria-label": "Search indexers for {label}",
                    title: "Interactive search: see every release and choose",
                    onclick: {
                        let label = label.clone();
                        move |e: MouseEvent| {
                            e.stop_propagation();
                            on_search.call((Some(season_number), None, label.clone()));
                        }
                    },
                    "Interactive"
                }
            }
            if open() {
                div { class: "season-body",
                    VirtualList {
                        count: list.len(),
                        row_height,
                        height,
                        label: label.clone(),
                        render: move |i: usize| {
                            let e = list[i].clone();
                            let ep_id = e.id;
                            let toggle = toggle_episode.clone();
                            rsx! {
                                span { class: "faint small", style: "width: 3.25rem; flex: none",
                                    "{e.episode_number}"
                                    if let Some(a) = e.absolute_episode_number { span { class: "xs", " ({a})" } }
                                }
                                span { class: "grow ellipsis", title: e.title.clone().unwrap_or_default(), {e.title.clone().unwrap_or_else(|| "TBA".into())} }
                                span { class: "small muted hide-mobile", style: "width: 6rem; flex: none", {e.air_date.clone().unwrap_or_default()} }
                                span { style: "width: 9.5rem; flex: none; text-align: right",
                                    if let Some(f) = &e.file {
                                        span { class: "badge success", title: "{f.relative_path}", "{f.quality_name}" }
                                        button {
                                            class: "btn sm ghost icon",
                                            "aria-label": "Delete the file of episode {e.episode_number}",
                                            title: "Delete file",
                                            onclick: {
                                                let target = (f.id, f.relative_path.clone());
                                                move |_| on_delete_file.call(target.clone())
                                            },
                                            "✕"
                                        }
                                    } else {
                                        span { class: "badge", "missing" }
                                    }
                                }
                                input {
                                    r#type: "checkbox",
                                    "aria-label": "Monitor episode {e.episode_number}",
                                    checked: e.monitored,
                                    onchange: move |ev: FormEvent| toggle((ep_id, ev.checked())),
                                }
                                {
                                    let label = format!("S{:02}E{:02}", e.season_number, e.episode_number);
                                    let label2 = label.clone();
                                    rsx! {
                                        button {
                                            class: "btn sm ghost icon",
                                            "aria-label": "Automatic search for {label}",
                                            title: "Automatic search: grab the best release",
                                            onclick: move |_| on_auto_search.call((None, Some(ep_id), label2.clone())),
                                            "⌕"
                                        }
                                        button {
                                            class: "btn sm ghost icon",
                                            "aria-label": "Search indexers for {label}",
                                            title: "Interactive search: see every release and choose",
                                            onclick: move |_| on_search.call((None, Some(ep_id), label.clone())),
                                            "☰"
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}

#[component]
pub fn MovieDetailPage(id: i64) -> Element {
    let app = use_app();
    let client = use_client();
    let nav = navigator();
    let mut confirm_delete = use_signal(|| false);
    let c1 = client.clone();
    let mut detail = use_resource(move || {
        let _ = (app.library_rev)();
        let c = c1.clone();
        async move { c.movie_detail(id).await }
    });
    let c2 = client.clone();
    let set_monitored = move |v: bool| {
        let c = c2.clone();
        spawn(async move {
            match c.set_movie_monitored(id, v).await {
                Ok(_) => detail.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    let mut refreshing = use_signal(|| false);
    let c4 = client.clone();
    let refresh = move |_| {
        let c = c4.clone();
        refreshing.set(true);
        spawn(async move {
            match c.refresh_movie(id).await {
                Ok(()) => {
                    app.success("Metadata refreshed.");
                    detail.restart();
                }
                Err(e) => app.error(&e),
            }
            refreshing.set(false);
        });
    };
    let c3 = client.clone();
    let delete = move |_| {
        let c = c3.clone();
        spawn(async move {
            match c.delete_movie(id).await {
                Ok(()) => {
                    app.success("Movie removed from Kitsunarr. Files on disk were not touched.");
                    nav.push(Route::MoviesLibrary {});
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let read = detail.read();
    let d = match &*read {
        None => return rsx! { Loading {} },
        Some(Err(e)) => return rsx! { ErrorBox { message: e.to_string() } },
        Some(Ok(d)) => d.clone(),
    };
    drop(read);
    let poster = d
        .images
        .iter()
        .find(|i| i.cover_type == "poster")
        .map(|i| i.remote_url.clone());
    let title = d.item.title.clone();
    rsx! {
        div { class: "hero",
            Poster { url: poster, title: d.item.title.clone(), monitored: d.item.monitored }
            div {
                div { class: "row tight",
                    h1 { style: "margin: 0", "{d.item.title}" }
                    if let Some(y) = d.item.year { span { class: "muted", "({y})" } }
                }
                div { class: "row tight", style: "margin: var(--sp-2) 0 var(--sp-3)",
                    Badge { text: d.item.status.to_string() }
                    Badge { text: format!("{} · {}", d.item.metadata_provider.display_name(), d.metadata_id), tone: "info" }
                }
                if let Some(o) = d.overview.as_ref().filter(|o| !o.is_empty()) { p { class: "muted", "{o}" } }
                dl { class: "kv",
                    dt { "Path" } dd { class: "mono", "{d.item.path}" }
                    dt { "Quality profile" } dd { "{d.item.quality_profile_name}" }
                    dt { "Studio" } dd { {d.item.studio.clone().unwrap_or_else(|| "—".into())} }
                    if let Some(f) = &d.file {
                        dt { "File" } dd { class: "mono", "{f.relative_path}" }
                        dt { "Quality" } dd { "{f.quality_name} · {format_bytes(f.size)} · added {format_date(&f.added_at)}" }
                    } else {
                        dt { "File" } dd { span { class: "badge warning", "missing" } }
                    }
                    if !d.external_ids.is_empty() {
                        dt { "Identities" }
                        dd { {d.external_ids.iter().map(|x| format!("{}:{}", x.kind, x.value)).collect::<Vec<_>>().join("  ")} }
                    }
                    dt { "Metadata" }
                    dd { {d.last_refreshed_at.as_deref().map(|t| format!("refreshed {}", relative(t))).unwrap_or_else(|| "never refreshed".into())} }
                }
                div { class: "row", style: "margin-top: var(--sp-4)",
                    Switch { checked: d.item.monitored, label: if d.item.monitored { "Monitored".to_string() } else { "Unmonitored".to_string() }, onchange: set_monitored }
                    div { class: "spacer" }
                    RefreshButton { provider: d.item.metadata_provider, busy: refreshing(), on_click: refresh }
                    button { class: "btn danger", onclick: move |_| confirm_delete.set(true), "Remove from library" }
                }
            }
        }
        if confirm_delete() {
            Confirm {
                title: "Remove movie",
                message: format!("Remove “{title}” from Kitsunarr? Files on disk are not deleted."),
                confirm_label: "Remove",
                danger: true,
                on_confirm: delete,
                on_cancel: move |_| confirm_delete.set(false),
            }
        }
    }
}

/// Rename preview (Sonarr's "Organize"): the files whose names differ from the naming
/// settings, each selectable, renamed on confirmation.
#[component]
fn RenameDialog(series_id: i64, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let previews = use_resource(move || {
        let c = c0.clone();
        async move { c.rename_preview(series_id, None).await }
    });
    let mut unchecked = use_signal(std::collections::HashSet::<i64>::new);
    let mut busy = use_signal(|| false);
    let rename = move |_| {
        let Some(Ok(list)) = &*previews.read() else {
            return;
        };
        let ids: Vec<i64> = list
            .iter()
            .map(|p| p.episode_file_id)
            .filter(|i| !unchecked().contains(i))
            .collect();
        if ids.is_empty() {
            return;
        }
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            match c.rename_files(series_id, &ids).await {
                Ok(r) if r.problems.is_empty() => app.success(r.message),
                Ok(r) => app.toast(
                    ToastLevel::Warning,
                    format!("{} {}", r.message, r.problems.join("; ")),
                ),
                Err(e) => app.error(&e),
            }
            busy.set(false);
            on_close.call(());
        });
    };
    rsx! {
        Modal {
            title: "Rename files".to_string(),
            wide: true,
            on_close: move |_| on_close.call(()),
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || !matches!(&*previews.read(), Some(Ok(l)) if !l.is_empty()), onclick: rename,
                    if busy() { "Renaming…" } else { "Rename" }
                }
            },
            match &*previews.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) if list.is_empty() => rsx! { p { class: "muted", "Every file already follows the naming settings." } },
                Some(Ok(list)) => rsx! {
                    div { class: "stack",
                        for p in list.iter() {
                            label { key: "{p.episode_file_id}", class: "check rename-row",
                                input {
                                    r#type: "checkbox",
                                    checked: !unchecked().contains(&p.episode_file_id),
                                    onchange: {
                                        let id = p.episode_file_id;
                                        move |e: FormEvent| { if e.checked() { unchecked.write().remove(&id); } else { unchecked.write().insert(id); } }
                                    },
                                }
                                div { class: "grow",
                                    div { class: "mono small muted", "{p.existing_path}" }
                                    div { class: "mono small", "→ {p.new_path}" }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}
