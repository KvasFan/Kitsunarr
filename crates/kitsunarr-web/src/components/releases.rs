//! Interactive release search results.

use dioxus::prelude::*;
use kitsunarr_domain::DownloadProtocol;
use kitsunarr_domain::api::search::{GrabRequest, ReleaseItem, ReleaseQuery};

use crate::components::common::{Empty, ErrorBox, Loading};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, relative};

fn episodes_label(r: &ReleaseItem) -> String {
    if !r.parsed {
        return "unparsed".into();
    }
    if r.full_season {
        return match r.season_numbers.as_slice() {
            [one] => format!("Season {one}"),
            [first, .., last] => format!("Seasons {first}-{last}"),
            [] => "Full season".into(),
        };
    }
    if !r.episode_numbers.is_empty() {
        let season = r.season_numbers.first().copied().unwrap_or(0);
        let eps: Vec<String> = r
            .episode_numbers
            .iter()
            .map(|e| format!("E{e:02}"))
            .collect();
        return format!("S{season:02}{}", eps.join(""));
    }
    if !r.absolute_episode_numbers.is_empty() {
        let abs: Vec<String> = r
            .absolute_episode_numbers
            .iter()
            .map(|a| a.to_string())
            .collect();
        return abs.join("-");
    }
    if r.special {
        "Special".into()
    } else {
        "—".into()
    }
}

/// Searches the indexers for a season or an episode of a series and lists the releases.
#[component]
pub fn ReleaseSearch(
    series_id: i64,
    #[props(default)] season: Option<i32>,
    #[props(default)] episode_id: Option<i64>,
    /// Tanukarr's complete-series search (torrent indexers).
    #[props(default)]
    complete_series: bool,
    /// Tanukarr's season-pack search: full-season releases only.
    #[props(default)]
    season_packs_only: bool,
    /// A film's search (Tanukarr's movies); `series_id` is then ignored.
    #[props(default)]
    movie_id: Option<i64>,
) -> Element {
    let client = use_client();
    let app = use_app();
    let mut grabbing = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| None::<String>);
    let mut grabbed = use_signal(Vec::<String>::new);
    // A release whose episodes are being chosen by hand: (indexer id, guid, title).
    let mut choosing = use_signal(|| None::<(i64, String, String)>);
    let c0 = client.clone();
    let results = use_resource(move || {
        let c = c0.clone();
        let q = ReleaseQuery {
            series_id,
            season_number: season,
            episode_id,
            complete_series,
            season_packs_only,
        };
        async move {
            match movie_id {
                Some(m) => c.search_movie_releases(m).await,
                None => c.search_releases(&q).await,
            }
        }
    });
    let read = results.read();
    let Some(outcome) = &*read else {
        return rsx! { Loading { label: "Searching indexers…" } };
    };
    let r = match outcome {
        Ok(r) => r.clone(),
        Err(e) => return rsx! { ErrorBox { message: e.to_string() } },
    };
    drop(read);
    rsx! {
        p { class: "small muted",
            "{r.description}: {r.releases.len()} releases from {r.searched_indexers} indexer(s)."
        }
        for f in r.failures.iter() {
            div { key: "{f.indexer_id}", class: "alert warning", role: "status",
                div { class: "grow", strong { "{f.indexer}: " } "{f.message}" }
            }
        }
        for (i, w) in r.warnings.iter().enumerate() {
            div { key: "w{i}", class: "alert warning", role: "status", div { class: "grow", "{w}" } }
        }
        if r.searched_indexers == 0 {
            Empty { title: "No indexers to search",
                p { "Add a Newznab or Torznab indexer and enable interactive search for it." }
                Link { to: Route::SettingsIndexers {}, class: "btn", "Indexer settings" }
            }
        } else if r.releases.is_empty() {
            Empty { title: "Nothing found",
                p { "The indexers returned no releases for this search." }
            }
        } else {
            p { class: "help small faint",
                "Releases are checked against the series' quality profile, custom formats and release profiles, and ranked, as Sonarr does. Rejected releases ask for a second click."
            }
            div { class: "table-wrap",
                table { class: "table releases",
                    thead {
                        tr {
                            th { span { class: "sr-only", "Decision" } }
                            th { "Source" }
                            th { "Age" }
                            th { "Title" }
                            th { "Episodes" }
                            th { "Indexer" }
                            th { class: "num", "Size" }
                            th { class: "num", "Peers" }
                            th { "Languages" }
                            th { "Quality" }
                            th { class: "num", "Score" }
                            th { span { class: "sr-only", "Actions" } }
                        }
                    }
                    tbody {
                        for rel in r.releases.iter() {
                            tr { key: "{rel.indexer_id}-{rel.guid}", class: if rel.approved { "" } else { "rejected" },
                                td {
                                    if rel.approved {
                                        span { class: "decision ok", title: "Accepted", "aria-label": "Accepted", "✓" }
                                    } else {
                                        span { class: "decision no", title: "{rel.rejections.join(\"\n\")}", "aria-label": "Rejected", "✕" }
                                    }
                                }
                                td {
                                    span { class: if rel.protocol == DownloadProtocol::Torrent { "badge info" } else { "badge accent" },
                                        if rel.protocol == DownloadProtocol::Torrent { "torrent" } else { "nzb" }
                                    }
                                }
                                td { class: "small age", title: "{rel.publish_date}", {relative(&rel.publish_date)} }
                                td { class: "release-title",
                                    span { class: "mono small", "{rel.title}" }
                                    for flag in rel.indexer_flags.iter() {
                                        span { key: "{flag}", class: "badge", " {flag}" }
                                    }
                                    for cf in rel.custom_formats.iter() {
                                        span { key: "cf-{cf}", class: "badge accent", "{cf}" }
                                    }
                                    if !rel.rejections.is_empty() {
                                        ul { class: "rejections small",
                                            for (i, why) in rel.rejections.iter().enumerate() {
                                                li { key: "{i}", "{why}" }
                                            }
                                        }
                                    }
                                }
                                td { class: "small", title: "{rel.mapped_episodes.join(\", \")}", {episodes_label(rel)} }
                                td { class: "small", "{rel.indexer}" }
                                td { class: "num small", {format_bytes(rel.size)} }
                                td { class: "num small",
                                    {match (rel.seeders, rel.peers) {
                                        (Some(s), Some(p)) => format!("{s} / {p}"),
                                        (Some(s), None) => s.to_string(),
                                        _ => "—".into(),
                                    }}
                                }
                                td { class: "small", {rel.languages.join(", ")} }
                                td { span { class: "badge", "{rel.quality}" } }
                                td { class: "num small", title: "Custom format score", "{rel.custom_format_score}" }
                                td { class: "actions",
                                    div { class: "row tight",
                                        if let Some(url) = rel.info_url.as_ref() {
                                            a { class: "btn sm ghost", href: "{url}", target: "_blank", rel: "noopener noreferrer", "aria-label": "Open on {rel.indexer}", "↗" }
                                        }
                                        {
                                            let key = format!("{}-{}", rel.indexer_id, rel.guid);
                                            let done = grabbed.read().contains(&key);
                                            let busy = grabbing() == Some(key.clone());
                                            let confirming = confirm() == Some(key.clone());
                                            let req = GrabRequest { guid: rel.guid.clone(), indexer_id: rel.indexer_id, episode_ids: Vec::new() };
                                            let approved = rel.approved;
                                            let label = rel.title.clone();
                                            let c = client.clone();
                                            rsx! {
                                                button {
                                                    class: if confirming { "btn sm danger" } else if approved { "btn sm primary" } else { "btn sm" },
                                                    disabled: done || busy,
                                                    "aria-label": "Grab {label}",
                                                    title: if approved { "Send to the download client" } else { "Rejected: click twice to grab anyway" },
                                                    onclick: move |_| {
                                                        if !approved && confirm() != Some(key.clone()) {
                                                            confirm.set(Some(key.clone()));
                                                            return;
                                                        }
                                                        confirm.set(None);
                                                        grabbing.set(Some(key.clone()));
                                                        let c = c.clone();
                                                        let req = req.clone();
                                                        let key = key.clone();
                                                        spawn(async move {
                                                            match c.grab_release(&req).await {
                                                                Ok(r) => {
                                                                    grabbed.write().push(key);
                                                                    app.success(&r.message);
                                                                }
                                                                Err(e) => app.error(&e),
                                                            }
                                                            grabbing.set(None);
                                                        });
                                                    },
                                                    if done { "Grabbed" } else if busy { "Grabbing…" } else if confirming { "Grab anyway?" } else { "Grab" }
                                                }
                                            }
                                        }
                                        if movie_id.is_none() {
                                            {
                                                let target = (rel.indexer_id, rel.guid.clone(), rel.title.clone());
                                                rsx! {
                                                    button { class: "btn sm ghost", "aria-label": "Choose episodes for {rel.title}",
                                                        title: "Choose the episodes of this release by hand, then grab it (the choice is kept for the import)",
                                                        onclick: move |_| choosing.set(Some(target.clone())), "Episodes…" }
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
        }
        if let Some((indexer_id, guid, title)) = choosing() {
            ChooseEpisodes { series_id, indexer_id, guid, title, on_close: move |done: bool| {
                if done && let Some((i, g, _)) = choosing() { grabbed.write().push(format!("{i}-{g}")); }
                choosing.set(None);
            } }
        }
    }
}

/// Sonarr's grab override: choose a release's episodes by hand, then grab it. The choice is
/// kept with the grab and used to import the download (Tanukarr's persistent manual mapping).
#[component]
fn ChooseEpisodes(
    series_id: i64,
    indexer_id: i64,
    guid: String,
    title: String,
    on_close: EventHandler<bool>,
) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let episodes = use_resource(move || {
        let c = c0.clone();
        async move { c.episodes(series_id).await }
    });
    let mut chosen = use_signal(Vec::<i64>::new);
    let mut busy = use_signal(|| false);
    let grab = move |_| {
        let c = client.clone();
        let req = GrabRequest {
            guid: guid.clone(),
            indexer_id,
            episode_ids: chosen(),
        };
        busy.set(true);
        spawn(async move {
            match c.grab_release(&req).await {
                Ok(r) => {
                    app.success(&r.message);
                    on_close.call(true);
                }
                Err(e) => {
                    app.error(&e);
                    busy.set(false);
                }
            }
        });
    };
    rsx! {
        crate::components::common::Modal { title: format!("Episodes of {title}"), on_close: move |_| on_close.call(false),
            footer: rsx! {
                span { class: "small muted", "{chosen().len()} chosen" }
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(false), "Cancel" }
                button { class: "btn primary", disabled: chosen().is_empty() || busy(), onclick: grab, if busy() { "Grabbing…" } else { "Grab with these episodes" } }
            },
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
