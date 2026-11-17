//! Series and movie detail pages.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::library::{
    EpisodeItem, EpisodesMonitoredUpdate, SeasonMonitoring, SeriesUpdate,
};
use kitsunarr_domain::api::metadata::SUPPORTED_CATALOGUES;

use crate::components::common::{Badge, Confirm, ErrorBox, Field, Loading, Modal, Switch};
use crate::components::media::Poster;
use crate::components::releases::ReleaseSearch;
use crate::components::vlist::VirtualList;
use crate::route::Route;
use crate::state::{ToastLevel, use_app, use_client};
use crate::util::{format_bytes, format_date, relative};

/// Episode row height in rems (42 px at the default text size).
const ROW_HEIGHT_REM: f64 = 2.625;

/// The toast for an automatic search's outcome.
pub(super) fn automatic_summary(
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
    let mut exclude_from_lists = use_signal(|| false);
    let mut delete_folder = use_signal(|| false);
    let mut renaming = use_signal(|| false);
    let mut ordering = use_signal(|| false);
    let mut source_open = use_signal(|| false);
    let mut history_open = use_signal(|| false);
    let mut naming_open = use_signal(|| false);
    let mut versions_open = use_signal(|| false);
    let mut compression_open = use_signal(|| false);
    let mut monitoring_open = use_signal(|| false);
    let mut move_open = use_signal(|| false);
    let mut scanning = use_signal(|| false);
    // An episode file to delete: (file id, relative path).
    let mut deleting_file = use_signal(|| None::<(i64, String)>);
    // Interactive search target: (season, episode id, label).
    let mut searching = use_signal(|| None::<(Option<i32>, Option<i64>, String)>);
    let mut searching_complete = use_signal(|| false);
    let mut searching_packs = use_signal(|| None::<i32>);
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
                season_packs_only: false,
                indexer_ids: None,
                search_session: None,
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
        let exclude = exclude_from_lists();
        confirm_delete.set(false);
        spawn(async move {
            let done = c.delete_series_with(id, with_files, exclude).await;
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
    let update_language = update.clone();
    let update_grouping = update.clone();
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
                    if d.item.metadata_provider == MetadataProvider::Tvdb {
                        dt { "Metadata language" }
                        dd {
                            select { class: "select", "aria-label": "Metadata language", value: "{d.metadata_language.unwrap_or(1)}",
                                title: "TheTVDB titles in this language; changing it refreshes the series",
                                onchange: move |e: FormEvent| {
                                    if let Ok(l) = e.value().parse::<i32>() {
                                        update_language(SeriesUpdate { metadata_language: Some(l), ..Default::default() });
                                    }
                                },
                                for l in kitsunarr_domain::Language::all().iter().filter(|l| l.id > 0) {
                                    option { key: "{l.id}", value: "{l.id}", "{l.name}" }
                                }
                            }
                        }
                    }
                    // Tanukarr's anime grouping mode.
                    if matches!(d.item.metadata_provider, MetadataProvider::AniList | MetadataProvider::MyAnimeList) {
                        dt { "Grouping" }
                        dd {
                            select { class: "select", "aria-label": "Grouping", value: if d.automatic_grouping { "automatic" } else { "disabled" },
                                title: "Disabled removes the series from its anime group and keeps it out of automatic grouping",
                                onchange: move |e: FormEvent| update_grouping(SeriesUpdate { automatic_grouping: Some(e.value() != "disabled"), ..Default::default() }),
                                option { value: "automatic", "Automatic (official relations)" }
                                option { value: "disabled", "Disabled" }
                            }
                        }
                    }
                }
                div { class: "row", style: "margin-top: var(--sp-4)",
                    Switch {
                        checked: monitored,
                        label: if monitored { "Monitored".to_string() } else { "Unmonitored".to_string() },
                        onchange: move |v: bool| update_monitored(SeriesUpdate { monitored: Some(v), ..Default::default() }),
                    }
                    button { class: "btn", title: "Choose which episodes are monitored, as when adding the series", onclick: move |_| monitoring_open.set(true), "Monitoring" }
                    div { class: "spacer" }
                    RefreshButton { provider: d.item.metadata_provider, busy: refreshing(), on_click: refresh }
                    button { class: "btn", onclick: move |_| source_open.set(true), "Change catalogue" }
                    button { class: "btn", onclick: move |_| history_open.set(true), "History" }
                    button { class: "btn", disabled: scanning(), title: "Find files added to or removed from the series folder", onclick: rescan,
                        if scanning() { "Scanning…" } else { "Scan disk" }
                    }
                    button { class: "btn", title: "Rename files to the naming settings", onclick: move |_| renaming.set(true), "Rename files" }
                    button { class: "btn", title: "This series' renaming mode, season folder titles and kept versions", onclick: move |_| naming_open.set(true), "Naming" }
                    button { class: "btn", title: "Files upgrades replaced, kept as versions", onclick: move |_| versions_open.set(true), "Versions" }
                    button { class: "btn", title: "Re-encode this series' files with the compression worker", onclick: move |_| compression_open.set(true), "Compression" }
                    if matches!(d.item.metadata_provider, MetadataProvider::TmdbTv | MetadataProvider::Tvdb) {
                        button { class: "btn", title: "Number episodes by another order of the catalogue (TMDB episode groups, TheTVDB DVD or absolute order)", onclick: move |_| ordering.set(true), "Episode order" }
                    }
                    button { class: "btn", title: "Interactive search for packs of the whole series (and of its seasons) on torrent indexers", onclick: move |_| searching_complete.set(true), "Complete series" }
                    button { class: "btn", title: "Move the series folder, or point the series at another folder", onclick: move |_| move_open.set(true), "Move folder" }
                    button { class: "btn danger", onclick: move |_| { delete_folder.set(false); exclude_from_lists.set(false); confirm_delete.set(true) }, "Remove from library" }
                }
            }
        }
        if let Some(season) = searching_packs() {
            Modal {
                title: format!("Search: {} — season {season} packs", d.item.title),
                wide: true,
                on_close: move |_| searching_packs.set(None),
                ReleaseSearch { key: "packs-{season}", series_id: id, season: Some(season), season_packs_only: true }
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
                label { class: "check",
                    input { r#type: "checkbox", checked: exclude_from_lists(), onchange: move |e: FormEvent| exclude_from_lists.set(e.checked()) }
                    "Prevent import lists from adding it again"
                }
            }
        }
        if compression_open() {
            CompressionDialog { series_id: id, on_close: move |_| compression_open.set(false) }
        }
        if move_open() {
            MoveFolderDialog {
                movie: false, id,
                current_path: d.item.path.clone(),
                current_root: d.root_folder_path.clone(),
                on_close: move |_| move_open.set(false),
            }
        }
        if monitoring_open() {
            MonitoringDialog {
                series_ids: vec![id],
                monitor_new_items: Some(d.monitor_new_items),
                on_close: move |_| monitoring_open.set(false),
            }
        }
        if versions_open() {
            VersionsDialog { series_id: id, on_close: move |_| versions_open.set(false) }
        }
        if naming_open() {
            NamingDialog {
                series_id: id,
                seasons: d.seasons.iter().map(|s| (s.season_number, s.title.clone().unwrap_or_default())).collect::<Vec<_>>(),
                on_close: move |_| naming_open.set(false),
            }
        }
        if history_open() {
            Modal { title: "Series history", wide: true, on_close: move |_| history_open.set(false),
                super::activity::HistoryTable { series_id: id }
            }
        }
        if source_open() {
            MetadataSourceDialog { id, movie: false, current_provider: d.item.metadata_provider, current_id: d.metadata_id.clone(), on_close: move |_| source_open.set(false) }
        }
        if ordering() {
            OrderingDialog { series_id: id, on_close: move |_| ordering.set(false) }
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
                on_pack_search: move |season| searching_packs.set(Some(season)),
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
    on_pack_search: EventHandler<i32>,
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
                if season_number > 0 {
                    button {
                        class: "btn sm ghost hide-mobile",
                        "aria-label": "Search season packs for {label}",
                        title: "Interactive search for full-season packs only",
                        onclick: move |e: MouseEvent| {
                            e.stop_propagation();
                            on_pack_search.call(season_number);
                        },
                        "Packs"
                    }
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
                                if let (Some(ss), Some(se)) = (e.scene_season_number, e.scene_episode_number)
                                    && (ss, se) != (e.season_number, e.episode_number)
                                {
                                    span {
                                        class: "badge",
                                        title: if e.unverified_scene_numbering { "Scene numbering extrapolated from TheXEM (unverified)" } else { "Scene numbering from TheXEM" },
                                        if e.unverified_scene_numbering { "scene {ss}x{se:02}?" } else { "scene {ss}x{se:02}" }
                                    }
                                }
                                if e.watched {
                                    span { class: "badge info", title: "Watched (Jellyfin watched sync)", "watched" }
                                }
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
    let mut delete_folder = use_signal(|| false);
    let mut removing = use_signal(|| false);
    let mut scanning = use_signal(|| false);
    let mut renaming = use_signal(|| false);
    let mut deleting_file = use_signal(|| None::<(i64, String)>);
    let mut searching = use_signal(|| false);
    let mut source_open = use_signal(|| false);
    let mut history_open = use_signal(|| false);
    let mut auto_busy = use_signal(|| false);
    let c5 = client.clone();
    let auto_search = move |_| {
        let c = c5.clone();
        auto_busy.set(true);
        spawn(async move {
            match c.automatic_movie_search(id).await {
                Ok(r) if !r.grabbed.is_empty() => {
                    app.success(format!("Grabbed {}.", r.grabbed[0].title))
                }
                Ok(r) => app.toast(
                    ToastLevel::Info,
                    format!(
                        "{} releases found, {} accepted, nothing grabbed.",
                        r.found, r.approved
                    ),
                ),
                Err(e) => app.error(&e),
            }
            auto_busy.set(false);
        });
    };
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
    let c9 = client.clone();
    let set_grouping = move |enabled: bool| {
        let c = c9.clone();
        spawn(async move {
            match c.set_movie_automatic_grouping(id, enabled).await {
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
        let with_files = delete_folder();
        removing.set(true);
        spawn(async move {
            let result = if with_files {
                c.delete_movie_with_files(id).await
            } else {
                c.delete_movie(id).await
            };
            match result {
                Ok(()) => {
                    app.success(if with_files {
                        "Movie removed, and its folder deleted or recycled."
                    } else {
                        "Movie removed from Kitsunarr. Files on disk were not touched."
                    });
                    nav.push(Route::MoviesLibrary {});
                }
                Err(e) => app.error(&e),
            }
            removing.set(false);
            confirm_delete.set(false);
        });
    };
    let c6 = client.clone();
    let rescan = move |_| {
        let c = c6.clone();
        scanning.set(true);
        spawn(async move {
            match c.rescan_movie(id).await {
                Ok(r) if r.problems.is_empty() => {
                    app.success(format!("Disk scanned: {}", r.message))
                }
                Ok(r) => app.toast(
                    ToastLevel::Warning,
                    format!("Disk scanned: {} {}", r.message, r.problems.join("; ")),
                ),
                Err(e) => app.error(&e),
            }
            detail.restart();
            scanning.set(false);
        });
    };
    let c7 = client.clone();
    let remove_file = move |_| {
        let Some((file_id, _)) = deleting_file() else {
            return;
        };
        deleting_file.set(None);
        let c = c7.clone();
        spawn(async move {
            match c.delete_movie_file(id, file_id).await {
                Ok(()) => {
                    app.success("File deleted.");
                    detail.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let mut watching = use_signal(|| false);
    let mut move_open = use_signal(|| false);
    let c8 = client.clone();
    let mut set_watched = move |watched: bool| {
        let c = c8.clone();
        watching.set(true);
        spawn(async move {
            match c.set_movie_watched(id, watched).await {
                Ok(_) => detail.restart(),
                Err(e) => app.error(&e),
            }
            watching.set(false);
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
                    dt { "Watched" }
                    dd {
                        Badge { text: if d.item.watched {"watched"} else {"unwatched"}, tone: if d.item.watched {Some("success")} else {None} }
                        if let Some(at)=&d.item.watched_at {span {class:"small muted"," · ",{crate::util::format_datetime(at)}}}
                    }
                    dt { "Metadata" }
                    dd { {d.last_refreshed_at.as_deref().map(|t| format!("refreshed {}", relative(t))).unwrap_or_else(|| "never refreshed".into())} }
                    // Tanukarr's grouping mode (anime groups, TMDB collections).
                    if matches!(d.item.metadata_provider, MetadataProvider::TmdbMovie | MetadataProvider::AniList | MetadataProvider::MyAnimeList) {
                        dt { "Grouping" }
                        dd {
                            select { class: "select", "aria-label": "Grouping", value: if d.automatic_grouping { "automatic" } else { "disabled" },
                                title: "Disabled removes the film from its collection and keeps it out of automatic grouping",
                                onchange: move |e: FormEvent| set_grouping(e.value() != "disabled"),
                                option { value: "automatic", "Automatic (official relations)" }
                                option { value: "disabled", "Disabled" }
                            }
                        }
                    }
                }
                div { class: "row", style: "margin-top: var(--sp-4)",
                    Switch { checked: d.item.monitored, label: if d.item.monitored { "Monitored".to_string() } else { "Unmonitored".to_string() }, onchange: set_monitored }
                    div { class: "spacer" }
                    button {class:"btn",disabled:watching(),title:"Jellyfin watched sync can update this status.",onclick:move |_|set_watched(!d.item.watched),if d.item.watched {"Mark unwatched"}else{"Mark watched"}}
                    RefreshButton { provider: d.item.metadata_provider, busy: refreshing(), on_click: refresh }
                    button { class: "btn", onclick: move |_| source_open.set(true), "Change catalogue" }
                    button { class: "btn", onclick: move |_| history_open.set(true), "History" }
                    button { class: "btn", title: "Search the indexers and choose a release", onclick: move |_| searching.set(true), "Search" }
                    button { class: "btn", disabled: auto_busy(), title: "Search the indexers and grab the best accepted release", onclick: auto_search,
                        if auto_busy() { "Searching…" } else { "Automatic search" }
                    }
                    button { class: "btn", disabled: scanning(), onclick: rescan, if scanning() { "Scanning…" } else { "Rescan disk" } }
                    button { class: "btn", disabled: d.file.is_none(), onclick: move |_| renaming.set(true), "Rename file" }
                    if let Some(file) = &d.file {
                        button { class: "btn danger", onclick: { let file = file.clone(); move |_| deleting_file.set(Some((file.id,file.relative_path.clone()))) }, "Delete file" }
                    }
                    button { class: "btn", title: "Move the movie folder, or point the movie at another folder", onclick: move |_| move_open.set(true), "Move folder" }
                    button { class: "btn danger", onclick: move |_| { delete_folder.set(false); confirm_delete.set(true); }, "Remove from library" }
                }
            }
        }
        if move_open() {
            MoveFolderDialog {
                movie: true, id,
                current_path: d.item.path.clone(),
                current_root: d.root_folder_path.clone(),
                on_close: move |_| move_open.set(false),
            }
        }
        if history_open() {
            Modal { title: "Movie history", wide: true, on_close: move |_| history_open.set(false),
                super::activity::HistoryTable { movie_id: id }
            }
        }
        if source_open() {
            MetadataSourceDialog { id, movie: true, current_provider: d.item.metadata_provider, current_id: d.metadata_id.clone(), on_close: move |_| source_open.set(false) }
        }
        if searching() {
            Modal {
                title: format!("Search: {}", d.item.title),
                wide: true,
                on_close: move |_| searching.set(false),
                ReleaseSearch { series_id: 0, movie_id: Some(id) }
            }
        }
        if renaming() {
            MovieRenameDialog { movie_id: id, on_close: move |_| { renaming.set(false); detail.restart(); } }
        }
        if let Some((_,path)) = deleting_file() {
            Confirm { title: "Delete file", message: format!("Delete “{path}” and its matching subtitles? They go to the recycle bin when one is set."), confirm_label: "Delete", danger: true,
                on_confirm: remove_file, on_cancel: move |_| deleting_file.set(None) }
        }
        if confirm_delete() {
            Modal {
                title: "Remove movie", on_close: move |_| { if !removing() { confirm_delete.set(false); } },
                footer: rsx! {
                    button { class: "btn", disabled: removing(), onclick: move |_| confirm_delete.set(false), "Cancel" }
                    button { class: "btn danger solid", disabled: removing(), onclick: delete, if removing() { "Removing…" } else { "Remove" } }
                },
                p { "Remove “{title}” from Kitsunarr?" }
                Switch { checked: delete_folder(), disabled: removing(), label: "Delete the movie folder and its files", onchange: move |v| delete_folder.set(v) }
                p { class: "help small", "With this option off, files stay on disk. Deleted folders go to the recycle bin when one is set." }
            }
        }
    }
}

#[component]
fn MovieRenameDialog(movie_id: i64, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let preview = use_resource(move || {
        let c = c0.clone();
        async move { c.movie_rename_preview(movie_id).await }
    });
    let mut busy = use_signal(|| false);
    let mut failure = use_signal(|| None::<String>);
    let apply = move |_| {
        let Some(Ok(Some(p))) = &*preview.read() else {
            return;
        };
        let p = p.clone();
        let c = client.clone();
        busy.set(true);
        failure.set(None);
        spawn(async move {
            match c.rename_movie_file(movie_id, &p).await {
                Ok(r) => {
                    app.success(r.message);
                    on_close.call(());
                }
                Err(e) => failure.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    rsx! {
        Modal { title: "Rename file", wide: true, on_close: move |_| { if !busy() { on_close.call(()); } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || !matches!(&*preview.read(),Some(Ok(Some(_)))), onclick: apply, if busy() { "Renaming…" } else { "Rename" } }
            },
            if let Some(error)=failure() {ErrorBox {message:error}}
            match &*preview.read() {
                None=>rsx!{Loading{}},
                Some(Err(e))=>rsx!{ErrorBox{message:e.to_string()}},
                Some(Ok(None))=>rsx!{p{class:"muted","Every file already follows the naming settings."}},
                Some(Ok(Some(p)))=>rsx!{
                    div {class:"stack",
                        div {class:"mono small muted","{p.existing_path}"}
                        div {class:"mono small","→ {p.new_path}"}
                        p {class:"help small","Matching subtitles follow the file's new name."}
                    }
                },
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

/// Chooses the episode ordering (Tanukarr's episode groups): the series is renumbered and
/// refreshed; episodes keep their files.
/// Changes a series' or film's folder (Sonarr's path edit): a root folder and the folder below
/// it, with or without moving the files.
#[component]
pub fn MoveFolderDialog(
    movie: bool,
    id: i64,
    current_path: String,
    current_root: String,
    on_close: EventHandler<()>,
) -> Element {
    use kitsunarr_domain::api::library::MoveTitleRequest;
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let roots = use_resource(move || {
        let c = c0.clone();
        async move { c.root_folders().await }
    });
    let relative = current_path
        .strip_prefix(&format!("{current_root}/"))
        .map(str::to_string)
        .unwrap_or_else(|| {
            current_path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or_default()
                .to_string()
        });
    let mut root = use_signal(|| current_root.clone());
    let mut folder = use_signal(move || relative.clone());
    let mut move_files = use_signal(|| true);
    let mut busy = use_signal(|| false);
    let mut failure = use_signal(|| None::<String>);
    let target = format!(
        "{}/{}",
        root().trim_end_matches('/'),
        folder().trim_matches('/')
    );
    let unchanged = target == current_path || folder().trim().is_empty();
    let save_target = target.clone();
    let apply = move |_| {
        let c = client.clone();
        let req = MoveTitleRequest {
            path: save_target.clone(),
            move_files: move_files(),
        };
        busy.set(true);
        failure.set(None);
        spawn(async move {
            let result = if movie {
                c.move_movie(id, &req).await
            } else {
                c.move_series(id, &req).await
            };
            match result {
                Ok(r) => {
                    app.success(r.message);
                    on_close.call(());
                }
                Err(e) => failure.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    rsx! {
        Modal {
            title: "Move folder".to_string(),
            on_close: move |_| { if !busy() { on_close.call(()) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || unchanged, onclick: apply,
                    if busy() { "Moving…" } else if move_files() { "Move" } else { "Save path" }
                }
            },
            p { class: "help small", "Current folder: " span { class: "mono", "{current_path}" } }
            Field { label: "Root folder",
                select { class: "select", "aria-label": "Root folder", disabled: busy(),
                    onchange: move |e: FormEvent| root.set(e.value()),
                    if let Some(Ok(list)) = &*roots.read() {
                        for r in list.iter() {
                            option { key: "{r.id}", value: "{r.path}", selected: r.path == root(), "{r.path}" }
                        }
                    }
                }
            }
            Field { label: "Folder", help: "Below the root folder; use / for sub-folders.".to_string(),
                input { class: "input", "aria-label": "Folder", value: "{folder}", disabled: busy(),
                    oninput: move |e| folder.set(e.value()) }
            }
            p { class: "path-preview mono", "{target}" }
            Switch { checked: move_files(), disabled: busy(), label: "Move the files to the new folder", onchange: move |v| move_files.set(v) }
            p { class: "help small",
                if move_files() {
                    "The folder is renamed, or copied then removed on another disk. The new folder must be missing or empty."
                } else {
                    "Only the recorded folder changes; use this when the files are already there, then scan the disk."
                }
            }
            if let Some(f) = failure() { div { class: "alert danger", role: "alert", "{f}" } }
        }
    }
}

/// Sonarr's monitoring options dialog (its season pass): applies a monitoring choice to the
/// episodes and seasons of one or more series, and sets "monitor new items".
#[component]
pub fn MonitoringDialog(
    series_ids: Vec<i64>,
    /// The current setting when one series is edited; `None` keeps each series' own.
    monitor_new_items: Option<kitsunarr_domain::MonitorNewItems>,
    on_close: EventHandler<()>,
) -> Element {
    use kitsunarr_domain::MonitorNewItems;
    use kitsunarr_domain::api::library::BulkEdit;
    use kitsunarr_domain::api::metadata::MonitorOption;
    let app = use_app();
    let client = use_client();
    let mut monitor = use_signal(|| MonitorOption::Skip);
    let mut new_items = use_signal(move || monitor_new_items);
    let mut busy = use_signal(|| false);
    let count = series_ids.len();
    let ids = series_ids.clone();
    let apply = move |_| {
        let c = client.clone();
        let edit = BulkEdit {
            ids: ids.clone(),
            monitor: Some(monitor()).filter(|m| *m != MonitorOption::Skip),
            monitor_new_items: new_items(),
            ..Default::default()
        };
        busy.set(true);
        spawn(async move {
            match c.bulk_edit_series(&edit).await {
                Ok(r) => app.success(format!("Monitoring updated for {} series.", r.updated)),
                Err(e) => app.error(&e),
            }
            busy.set(false);
            on_close.call(());
        });
    };
    let choices = std::iter::once(MonitorOption::Skip)
        .chain(super::add::SERIES_MONITOR.iter().copied())
        .collect::<Vec<_>>();
    let unchanged = monitor() == MonitorOption::Skip && new_items() == monitor_new_items;
    rsx! {
        Modal {
            title: if count == 1 { "Monitoring".to_string() } else { format!("Monitoring — {count} series") },
            on_close: move |_| on_close.call(()),
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || unchanged, onclick: apply,
                    if busy() { "Saving…" } else { "Save" }
                }
            },
            p { class: "help small", "The choice replaces the monitored state of every episode (specials are only changed by the two specials choices), then monitors each season that has a monitored episode. \"None\" also unmonitors the series." }
            Field { label: "Monitor",
                select { class: "select", "aria-label": "Monitor", value: monitor().as_str(),
                    onchange: move |e: FormEvent| {
                        if let Some(m) = MonitorOption::from_sonarr(&e.value()) {
                            monitor.set(m);
                        }
                    },
                    for m in choices.iter() { option { key: "{m.as_str()}", value: m.as_str(), "{m.label()}" } }
                }
            }
            Field { label: "Monitor new seasons",
                select { class: "select", "aria-label": "Monitor new seasons",
                    value: new_items().map(|n| n.as_str()).unwrap_or(""),
                    onchange: move |e: FormEvent| new_items.set(e.value().parse::<MonitorNewItems>().ok()),
                    if monitor_new_items.is_none() { option { value: "", "Don't change" } }
                    option { value: "all", "All seasons" }
                    option { value: "none", "None" }
                }
            }
        }
    }
}

#[component]
fn OrderingDialog(series_id: i64, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let orderings = use_resource(move || {
        let c = c0.clone();
        async move { c.series_orderings(series_id).await }
    });
    let mut chosen = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let apply = move |_| {
        let Some(id) = chosen() else { return };
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            match c
                .set_series_ordering(series_id, Some(id).filter(|i| !i.is_empty()))
                .await
            {
                Ok(r) => app.success(format!(
                    "Episode order changed: {} episodes updated, {} added.",
                    r.updated_episodes, r.added_episodes
                )),
                Err(e) => app.error(&e),
            }
            busy.set(false);
            on_close.call(());
        });
    };
    rsx! {
        Modal {
            title: "Episode order".to_string(),
            on_close: move |_| on_close.call(()),
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || chosen().is_none(), onclick: apply,
                    if busy() { "Applying…" } else { "Apply" }
                }
            },
            p { class: "help small", "Episodes are renumbered by the chosen order; files, monitoring and history stay with their episodes. Rename files afterwards to match the new numbers." }
            match &*orderings.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) => rsx! {
                    div { class: "stack",
                        for o in list.iter() {
                            label { key: "{o.id}", class: "check",
                                input { r#type: "radio", name: "ordering",
                                    checked: chosen().map_or(o.current, |c| c == o.id),
                                    onchange: { let id = o.id.clone(); move |_| chosen.set(Some(id.clone())) } }
                                span {
                                    strong { "{o.name}" }
                                    if o.current { " " Badge { text: "current", tone: "accent" } }
                                    if let Some(desc) = &o.description { div { class: "small muted", "{desc}" } }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Tanukarr's per-series naming options: renaming on import and season folder titles.
#[component]
fn NamingDialog(
    series_id: i64,
    seasons: Vec<(i32, String)>,
    on_close: EventHandler<()>,
) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut options =
        use_signal(|| None::<kitsunarr_domain::api::library_tools::SeriesNamingOptions>);
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.series_naming(series_id).await {
                Ok(o) => options.set(Some(o)),
                Err(e) => app.error(&e),
            }
        }
    });
    let save = move |_| {
        let Some(o) = options() else { return };
        let c = client.clone();
        spawn(async move {
            match c.put_series_naming(series_id, &o).await {
                Ok(_) => {
                    app.success("Naming options saved.");
                    on_close.call(());
                }
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        Modal { title: "Naming".to_string(), on_close: move |_| on_close.call(()),
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: options().is_none(), onclick: save, "Save" }
            },
            match options() {
                None => rsx! { Loading {} },
                Some(o) => rsx! {
                    label { class: "field",
                        span { class: "label", "Rename files on import" }
                        select { class: "select", "aria-label": "Renaming mode",
                            onchange: move |e| { if let Some(x) = options.write().as_mut() { x.renaming_mode = e.value(); } },
                            for (v, l) in [("default", "As Settings › Media management says"), ("manual", "Never: keep the release's names"), ("automatic", "Always, even when renaming is off")] {
                                option { key: "{v}", value: v, selected: o.renaming_mode == v, "{l}" }
                            }
                        }
                    }
                    label { class: "check", style: "margin-bottom: var(--sp-3)",
                        input { r#type: "checkbox", checked: o.keep_versions,
                            onchange: move |e: FormEvent| { if let Some(x) = options.write().as_mut() { x.keep_versions = e.checked(); } } }
                        "Keep files replaced by upgrades as versions (in \"Plex Versions\")"
                    }
                    p { class: "help small", "Season folder titles replace the catalogue's in {{Season Title}}. Leave empty to keep the catalogue's." }
                    div { class: "form-grid",
                        for (n, catalogue) in seasons.iter().filter(|(n, _)| *n > 0).cloned() {
                            label { key: "{n}", class: "field",
                                span { class: "label", "Season {n}" }
                                input { class: "input", "aria-label": "Season {n} title", placeholder: "{catalogue}",
                                    value: "{o.season_titles.get(&n).cloned().unwrap_or_default()}",
                                    oninput: move |e: FormEvent| {
                                        if let Some(x) = options.write().as_mut() {
                                            if e.value().trim().is_empty() { x.season_titles.remove(&n); } else { x.season_titles.insert(n, e.value()); }
                                        }
                                    } }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Tanukarr's retained versions: files upgrades replaced, which can be made current again.
/// Tanukarr's per-show compression panel: the worker's modes, the series' job, start and
/// cancel. Refreshed every 30 seconds while open.
#[component]
fn CompressionDialog(series_id: i64, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let mut mode = use_signal(String::new);
    let mut min_size = use_signal(|| 0i64);
    let c0 = client.clone();
    let mut status = use_resource(move || {
        let c = c0.clone();
        async move { c.compression(series_id).await }
    });
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(30_000).await;
            status.restart();
        }
    });
    let c1 = client.clone();
    let start = move |_| {
        let c = c1.clone();
        let (m, size) = (mode(), min_size());
        spawn(async move {
            match c.start_compression(series_id, &m, size).await {
                Ok(_) => {
                    app.success("Compression queued.");
                    status.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let cancel = move |_| {
        let c = client.clone();
        spawn(async move {
            match c.cancel_compression(series_id).await {
                Ok(()) => status.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    let text = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    rsx! {
        Modal { title: "Compression".to_string(), wide: true, on_close: move |_| on_close.call(()),
            footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| on_close.call(()), "Close" } },
            match &*status.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(s)) if s["available"] == serde_json::json!(false) => rsx! {
                    p { class: "muted", {s["message"].as_str().unwrap_or("Compression worker is not connected.").to_string()} }
                    p { class: "help small faint", "The worker is an optional host component (Tanukarr's compression worker) reached through the socket at KITSUNARR_COMPRESSION_SOCKET." }
                },
                Some(Ok(s)) => {
                    let job = s["job"].clone();
                    let active = matches!(job["status"].as_str(), Some("queued" | "running" | "cancelling"));
                    let modes: Vec<serde_json::Value> = s["modes"].as_array().cloned().unwrap_or_default();
                    rsx! {
                        if !job.is_null() {
                            dl { class: "kv",
                                dt { "Job" } dd { "{text(&job, \"status\")} · {text(&job, \"modeName\")}" }
                                dt { "Progress" } dd { "{job[\"completedFiles\"]} / {job[\"totalFiles\"]} files · {job[\"percent\"]}% · saved {crate::util::format_bytes(job[\"savedBytes\"].as_i64().unwrap_or(0))}" }
                                if !text(&job, "currentFile").is_empty() { dt { "Current file" } dd { class: "mono small", "{text(&job, \"currentFile\")}" } }
                                if !text(&job, "message").is_empty() { dt { "Message" } dd { "{text(&job, \"message\")}" } }
                            }
                        }
                        if active {
                            div { class: "row end", button { class: "btn danger", onclick: cancel, "Cancel compression" } }
                        } else {
                            div { class: "form-grid",
                                Field { label: "Mode",
                                    select { class: "select", "aria-label": "Compression mode", value: "{mode}",
                                        onchange: move |e: FormEvent| mode.set(e.value()),
                                        option { value: "", "Choose a mode" }
                                        for m in modes.iter().filter(|m| m["available"] == serde_json::json!(true)) {
                                            option { key: "{text(m, \"id\")}", value: "{text(m, \"id\")}", title: "{text(m, \"description\")}", "{text(m, \"name\")}" }
                                        }
                                    }
                                }
                                Field { label: "Skip files smaller than (MB)",
                                    input { class: "input", r#type: "number", min: "0", max: "100000", value: "{min_size}",
                                        oninput: move |e: FormEvent| min_size.set(e.value().parse().unwrap_or(0)) }
                                }
                            }
                            div { class: "row end", button { class: "btn primary", disabled: mode().is_empty(), onclick: start, "Compress series" } }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn VersionsDialog(series_id: i64, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut versions = use_resource(move || {
        let c = c0.clone();
        async move { c.series_versions(series_id).await }
    });
    let c1 = client.clone();
    let activate = move |v: i64| {
        let c = c1.clone();
        spawn(async move {
            match c.activate_version(series_id, v).await {
                Ok(m) => {
                    app.success(m);
                    versions.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let remove = move |v: i64| {
        let c = client.clone();
        spawn(async move {
            match c.delete_version(series_id, v).await {
                Ok(()) => versions.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        Modal { title: "Kept versions".to_string(), wide: true, on_close: move |_| on_close.call(()),
            footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| on_close.call(()), "Close" } },
            match &*versions.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) if list.is_empty() => rsx! { p { class: "muted", "No kept versions. Turn on \"Keep files replaced by upgrades\" in Naming." } },
                Some(Ok(list)) => rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr { th { "File" } th { "Kept" } th { class: "num", "Size" } th { "" } } }
                            tbody {
                                for v in list.iter() {
                                    {
                                        let (id, activate, remove) = (v.id, activate.clone(), remove.clone());
                                        rsx! {
                                            tr { key: "{v.id}",
                                                td { class: "small", title: "{v.archive_path}", "{v.relative_path}"
                                                    if !v.available { " " Badge { text: "missing".to_string(), tone: "danger" } } }
                                                td { class: "small", {relative(&v.created)} }
                                                td { class: "num", {format_bytes(v.size)} }
                                                td { class: "shrink",
                                                    div { class: "row tight",
                                                        button { class: "btn sm", disabled: !v.available, onclick: move |_| activate(id), "Make current" }
                                                        button { class: "btn sm danger", onclick: move |_| remove(id), "Delete" }
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
            }
        }
    }
}

/// Preview catalogue ownership and resolve ambiguous episode associations before applying.
#[component]
fn MetadataSourceDialog(
    id: i64,
    movie: bool,
    current_provider: MetadataProvider,
    current_id: String,
    on_close: EventHandler<()>,
) -> Element {
    use kitsunarr_domain::api::metadata::{
        EntryKind, MetadataSourcePreview, MetadataSourceRequest, SearchItem, SearchQuery,
        SourceEpisodeMapping,
    };
    let mut app = use_app();
    let client = use_client();
    let mut request = use_signal(move || MetadataSourceRequest {
        provider: current_provider,
        id: String::new(),
        confirm_identity: false,
        mappings: vec![],
        revision: None,
    });
    let mut term = use_signal(String::new);
    let mut results = use_signal(Vec::<SearchItem>::new);
    let mut preview = use_signal(|| None::<MetadataSourcePreview>);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut editing_episode = use_signal(|| None::<i64>);
    let mut page = use_signal(|| 0usize);
    let c0 = client.clone();
    let providers = use_resource(move || {
        let c = c0.clone();
        async move { c.metadata_providers().await }
    });
    let c1 = client.clone();
    let search = move |_| {
        let c = c1.clone();
        let q = SearchQuery {
            provider: request().provider,
            term: term(),
        };
        busy.set(true);
        error.set(None);
        results.set(vec![]);
        spawn(async move {
            match c.metadata_search(&q).await {
                Ok(items) => results.set(
                    items
                        .into_iter()
                        .filter(|r| (r.kind == EntryKind::Movie) == movie)
                        .collect(),
                ),
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    let c2 = client.clone();
    let review = move |_| {
        let c = c2.clone();
        let req = request();
        busy.set(true);
        error.set(None);
        request.write().revision = None;
        spawn(async move {
            match c.preview_metadata_source(movie, id, &req).await {
                Ok(p) => {
                    request.write().revision = Some(p.revision.clone());
                    preview.set(Some(p));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    let apply = move |_| {
        let c = client.clone();
        let req = request();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match c.set_metadata_source(movie, id, &req).await {
                Ok(_) => {
                    app.library_rev += 1;
                    app.success("Catalogue changed.");
                    on_close.call(());
                }
                Err(e) => {
                    request.write().revision = None;
                    error.set(Some(e.to_string()));
                }
            }
            busy.set(false);
        });
    };
    let can_apply =
        request().revision.is_some() && preview().is_some_and(|p| p.blockers.is_empty());
    rsx! {
        Modal {
            title: "Change catalogue", wide: true,
            on_close: move |_| {if !busy() {on_close.call(());}},
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn", disabled: busy() || request().id.trim().is_empty(), onclick: review, "Preview" }
                button { class: "btn primary", disabled: busy() || !can_apply, onclick: apply, if busy() { "Applying…" } else { "Change catalogue" } }
            },
            p { class: "help", "Review the target title and episode matches. Files, paths, monitoring and history are kept. Unmatched episodes keep their numbers only when those numbers are free in the new catalogue." }
            p { class: "small muted", "{current_provider.display_name()} · {current_id}" }
            if let Some(e)=error() { ErrorBox {message:e} }
            Field { label: "Catalogue",
                select { class: "select", "aria-label":"Target catalogue", disabled:busy(), value: "{request().provider}",
                    onchange: move |e:FormEvent| {if let Ok(p)=e.value().parse() {request.set(MetadataSourceRequest{provider:p,id:String::new(),confirm_identity:false,mappings:vec![],revision:None});preview.set(None);results.set(vec![]);}},
                    if let Some(Ok(list))=&*providers.read() {
                        for p in list.iter().filter(|p| if movie {p.kinds.contains(&EntryKind::Movie)} else {p.kinds.iter().any(|k| *k!=EntryKind::Movie)}) {
                            option { value:"{p.provider}", disabled:!p.available, "{p.name}" }
                        }
                    }
                }
            }
            div {class:"row",
                input {class:"input", "aria-label":"Find target title",placeholder:"Search titles…",value:"{term}",disabled:busy(),oninput:move |e|term.set(e.value())}
                button {class:"btn",disabled:busy() || term().trim().is_empty(),onclick:search,"Search"}
            }
            if !results().is_empty() {
                div {class:"stack",style:"max-height: 12rem; overflow: auto",
                    for r in results() {
                        button {class:"btn",disabled:busy(),onclick:move |_| {request.write().id=r.id.clone();request.write().mappings.clear();request.write().revision=None;request.write().confirm_identity=false;preview.set(None);},"{r.title} · {r.id}"}
                    }
                }
            }
            Field {label:"Catalogue ID",
                input {class:"input", "aria-label":"Target catalogue ID",value:"{request().id}",disabled:busy(),oninput:move |e| {request.write().id=e.value();request.write().revision=None;request.write().mappings.clear();request.write().confirm_identity=false;preview.set(None);}}
            }
            label {class:"check",
                input {r#type:"checkbox",checked:request().confirm_identity,disabled:busy(),onchange:move |e| {request.write().confirm_identity=e.checked();request.write().revision=None;}}
                "I confirm that the target catalogue entry is the same title."
            }
            if let Some(p)=preview() {
                h3 {"{p.title}" if let Some(y)=p.year {" ({y})"}}
                if p.identity_verified {p {class:"small muted","The catalogues share an external identity."}}
                for b in &p.blockers {p {class:"help warning","{b}"}}
                if !movie {
                    p {"{p.matched_episodes} matched · {p.retained_episodes} kept at current numbers · {p.new_episodes} new episodes"}
                    details {
                        summary {"Episode matches"}
                        div {class:"stack",style:"max-height: 24rem; overflow: auto",
                            for e in p.episodes.iter().skip(page() * 50).take(50) {
                                div {class:"row", key:"{e.episode_id}",
                                    span {"S{e.season_number:02}E{e.episode_number:02} · {e.title.clone().unwrap_or_default()}"}
                                    if editing_episode() == Some(e.episode_id) {
                                    select {class:"select","aria-label":format!("Match episode {}",e.episode_id),disabled:busy(),
                                        value:e.target.as_ref().map(|t|format!("{}:{}",t.season_number,t.episode_number)).unwrap_or_default(),
                                        onchange:{let eid=e.episode_id;move |event:FormEvent| {
                                            let mut req=request.write();req.mappings.retain(|m|m.episode_id!=eid);req.revision=None;
                                            if let Some((s,n))=event.value().split_once(':') && let (Ok(s),Ok(n))=(s.parse(),n.parse()) {req.mappings.push(SourceEpisodeMapping{episode_id:eid,season_number:s,episode_number:n});}
                                        }},
                                        option {value:"","Automatic match / keep current number"}
                                        for t in &p.target_episodes {option {value:format!("{}:{}",t.season_number,t.episode_number),"S{t.season_number:02}E{t.episode_number:02} · {t.title.clone().unwrap_or_default()}"}}
                                    }
                                    } else {
                                        span {class:"small muted",{e.target.as_ref().map(|t|format!("→ S{:02}E{:02}",t.season_number,t.episode_number)).unwrap_or_else(||"→ current number".into())}}
                                        button {class:"btn",disabled:busy(),onclick:{let eid=e.episode_id;move |_| editing_episode.set(Some(eid))},"Match"}
                                    }
                                }
                            }
                        }
                        div {class:"row",
                            button {class:"btn",disabled:page()==0,onclick:move |_| page -= 1,"Previous"}
                            span {"{page() + 1}"}
                            button {class:"btn",disabled:(page()+1)*50>=p.episodes.len(),onclick:move |_| page += 1,"Next"}
                        }
                    }
                }
                if request().revision.is_none() {p {class:"help","Preview again to review your changes."}}
            }
        }
    }
}
