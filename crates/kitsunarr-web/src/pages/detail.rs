//! Series and movie detail pages.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::library::{
    EpisodeItem, EpisodesMonitoredUpdate, SeasonMonitoring, SeriesUpdate,
};
use kitsunarr_domain::api::metadata::SUPPORTED_CATALOGUES;

use crate::components::common::{Badge, Confirm, ErrorBox, Loading, Switch};
use crate::components::media::Poster;
use crate::components::vlist::VirtualList;
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, format_date, relative};

const ROW_HEIGHT: f64 = 42.0;

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
    let c4 = client.clone();
    let delete = move |_| {
        let c = c4.clone();
        spawn(async move {
            match c.delete_series(id).await {
                Ok(()) => {
                    app.success("Series removed from Kitsunarr. Files on disk were not touched.");
                    nav.push(Route::SeriesLibrary {});
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
                    button { class: "btn danger", onclick: move |_| confirm_delete.set(true), "Remove from library" }
                }
            }
        }
        if confirm_delete() {
            Confirm {
                title: "Remove series",
                message: format!("Remove “{title}” and its episode records from Kitsunarr? Files on disk are not deleted."),
                confirm_label: "Remove",
                danger: true,
                on_confirm: delete,
                on_cancel: move |_| confirm_delete.set(false),
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
    let height = (list.len() as f64 * ROW_HEIGHT).clamp(ROW_HEIGHT, 520.0);
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
            }
            if open() {
                div { class: "season-body",
                    VirtualList {
                        count: list.len(),
                        row_height: ROW_HEIGHT,
                        height,
                        label: label.clone(),
                        render: move |i: usize| {
                            let e = list[i].clone();
                            let ep_id = e.id;
                            let toggle = toggle_episode.clone();
                            rsx! {
                                span { class: "faint small", style: "width: 52px",
                                    "{e.episode_number}"
                                    if let Some(a) = e.absolute_episode_number { span { class: "xs", " ({a})" } }
                                }
                                span { class: "grow ellipsis", title: e.title.clone().unwrap_or_default(), {e.title.clone().unwrap_or_else(|| "TBA".into())} }
                                span { class: "small muted hide-mobile", style: "width: 96px", {e.air_date.clone().unwrap_or_default()} }
                                span { style: "width: 150px; text-align: right",
                                    if let Some(f) = &e.file {
                                        span { class: "badge success", title: "{f.relative_path}", "{f.quality_name}" }
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
