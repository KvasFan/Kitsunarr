//! Interactive release search results.

use dioxus::prelude::*;
use kitsunarr_domain::DownloadProtocol;
use kitsunarr_domain::api::search::{
    GrabRequest, MovieReleaseQuery, ReleaseFilter, ReleaseItem, ReleaseQuery,
};

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
    /// Explicitly repeat the same search from the search page.
    #[props(default)]
    search_revision: u64,
) -> Element {
    let client = use_client();
    let session = use_signal(|| uuid::Uuid::new_v4().to_string());
    let mut draft = use_signal(|| None::<Vec<i64>>);
    let mut selected = use_signal(|| None::<String>);
    let sources = use_resource(move || {
        let c = client.clone();
        async move { c.providers("indexers").await }
    });
    rsx! {
        details { class: "card search-sources", open: true,
            summary { "Search sources" }
            match &*sources.read() {
                Some(Ok(items)) => rsx! {
                    div { class: "row wrap",
                        for source in items.iter().filter(|p| p.enabled && p.supported && p.options.get("enableInteractiveSearch").and_then(|v| v.as_bool()).unwrap_or(true)) {
                            {
                                let id=source.id;
                                let all:Vec<i64>=items.iter().filter(|p| p.enabled && p.supported && p.options.get("enableInteractiveSearch").and_then(|v|v.as_bool()).unwrap_or(true)).map(|p|p.id).collect();
                                rsx! { label { class: "row tight",
                                    input { r#type: "checkbox", "aria-label": "Search {source.name}",
                                        checked: draft.read().as_ref().is_none_or(|ids|ids.contains(&id)),
                                        onchange: move |evt| { let mut ids=draft().unwrap_or_else(||all.clone()); if evt.checked() { if !ids.contains(&id) {ids.push(id);} } else { ids.retain(|v|*v!=id); } draft.set(Some(ids)); }
                                    }
                                    "{source.name}"
                                } }
                            }
                        }
                    }
                    div { class: "row",
                        button { class: "btn sm", onclick: move |_| selected.set(draft().map(|ids|ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(","))), "Apply sources" }
                        button { class: "btn sm ghost", onclick: move |_| { draft.set(None); selected.set(None); }, "All sources" }
                    }
                },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                None => rsx! { p { "Loading sources…" } },
            }
        }
        ReleaseResults { series_id, season, episode_id, complete_series, season_packs_only, movie_id, search_revision, indexer_ids: selected(), search_session: session() }
    }
}

#[component]
fn ReleaseResults(
    series_id: i64,
    season: Option<i32>,
    episode_id: Option<i64>,
    complete_series: bool,
    season_packs_only: bool,
    movie_id: Option<i64>,
    search_revision: u64,
    indexer_ids: Option<String>,
    search_session: String,
) -> Element {
    let client = use_client();
    let app = use_app();
    let mut grabbing = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| None::<String>);
    let mut grabbed = use_signal(Vec::<String>::new);
    let mut filter_text = use_signal(String::new);
    let mut filter_status = use_signal(|| ReleaseFilter::All);
    // A release whose episodes are being chosen by hand: (indexer id, guid, title).
    let mut choosing = use_signal(|| None::<(i64, String, String)>);
    let c0 = client.clone();
    // Resource dependencies must track ordinary props too: changing a component key alone
    // does not reliably remount a static child in this Dioxus tree.
    let source_scope = (indexer_ids, search_session);
    let mut results = use_resource(use_reactive!(
        |series_id,
         season,
         episode_id,
         complete_series,
         season_packs_only,
         movie_id,
         search_revision,
         source_scope| {
            let (indexer_ids, search_session) = source_scope;
            let _ = search_revision;
            confirm.set(None);
            choosing.set(None);
            let c = c0.clone();
            let q = ReleaseQuery {
                series_id,
                season_number: season,
                episode_id,
                complete_series,
                season_packs_only,
                indexer_ids: indexer_ids.clone(),
                search_session: Some(search_session.clone()),
            };
            async move {
                match movie_id {
                    Some(m) => {
                        c.search_movie_releases_selected(
                            m,
                            &MovieReleaseQuery {
                                indexer_ids,
                                search_session: Some(search_session),
                            },
                        )
                        .await
                    }
                    None => c.search_releases(&q).await,
                }
            }
        }
    ));
    if *results.state().read() == UseResourceState::Pending {
        return rsx! { Loading { label: "Searching indexers…" } };
    }
    let read = results.read();
    let Some(outcome) = &*read else {
        return rsx! { Loading { label: "Searching indexers…" } };
    };
    let r = match outcome {
        Ok(r) => r.clone(),
        Err(e) => {
            return rsx! { ErrorBox { message: e.to_string() } button { class: "btn sm", onclick: move |_| results.restart(), "Search again" } };
        }
    };
    drop(read);
    let visible = r
        .releases
        .iter()
        .filter(|rel| filter_status().matches(rel, &filter_text()))
        .collect::<Vec<_>>();
    rsx! {
        div { class: "row",
            p { class: "small muted grow",
                "{r.description}: {r.releases.len()} releases from {r.searched_indexers} indexer(s). {r.successful_indexers} responding."
            }
            button { class: "btn sm", disabled: grabbing().is_some(), onclick: move |_| { confirm.set(None); results.restart(); }, "Search again" }
        }
        for f in r.failures.iter() {
            div { key: "{f.indexer_id}", class: "alert warning", role: "status",
                div { class: "grow", strong { "{f.indexer}: " } "{f.message}" }
            }
        }
        for (i, w) in r.warnings.iter().enumerate() {
            div { key: "w{i}", class: "alert warning", role: "status", div { class: "grow", "{w}" } }
        }
        details { class: "card search-diagnostics",
            summary { "Search diagnostics" }
            p { class: "small", "Titles considered: " {r.considered_titles.join(", ")} }
            p { class: "small muted", "Query plans include fallback tiers that may not run. Sources have a 35-second budget; interactive searches have a 45-second total budget." }
            for diagnostic in r.diagnostics.iter() {
                div { key: "d{diagnostic.indexer_id}",
                    strong { "{diagnostic.indexer}: {diagnostic.status}" }
                    span { class: "small", " · {diagnostic.duration_ms} ms · {diagnostic.releases} releases" }
                    for (i,note) in diagnostic.notes.iter().enumerate() { p { key: "n{i}", class: "small muted", "{note}" } }
                    ul { class: "small mono",
                        for (i,plan) in diagnostic.planned_queries.iter().enumerate() { li { key: "{i}", "{plan}" } }
                    }
                }
            }
            p { class: "small", "Parsed: {r.releases.iter().filter(|v|v.parsed).count()}; title/identifier matched: {r.releases.iter().filter(|v|v.matches_series).count()}; approved: {r.releases.iter().filter(|v|v.approved).count()}." }
            p { class: "small muted", "For unmatched releases, check the parsed title and episode numbers below against the titles considered. Add an alternate title or correct the episode mapping in the library when needed." }
        }
        if r.releases.is_empty() && !r.failures.is_empty() {
            if r.successful_indexers == 0 {
                Empty { title: "Search sources unavailable",
                    p { "No indexer responded successfully. Check the source errors above, then try again." }
                    Link { to: Route::SettingsIndexers {}, class: "btn", "Indexer settings" }
                }
            } else {
                Empty { title: "Search incomplete",
                    p { "The responding indexers returned no releases. Other sources failed; check the errors above and try again." }
                }
            }
        } else if r.searched_indexers == 0 {
            Empty { title: "No indexers to search",
                p { "Select a source above, or enable interactive search in indexer settings. Source tags can also restrict which series are searched." }
                Link { to: Route::SettingsIndexers {}, class: "btn", "Indexer settings" }
            }
        } else if r.releases.is_empty() {
            Empty { title: "Nothing found",
                p { "The indexers returned no releases for this search." }
            }
        } else {
            div { class: "card release-filters",
                div { class: "row wrap",
                    input { class: "input grow", r#type: "search", "aria-label": "Filter releases",
                        placeholder: "Title, group, quality or rejection reason…", value: "{filter_text}",
                        oninput: move |e| { filter_text.set(e.value()); confirm.set(None); }
                    }
                    select { class: "select", "aria-label": "Filter release decisions",
                        onchange: move |e| { filter_status.set(match e.value().as_str() { "approved" => ReleaseFilter::Approved, "rejected" => ReleaseFilter::Rejected, "unmatched" => ReleaseFilter::Unmatched, _ => ReleaseFilter::All }); confirm.set(None); },
                        option { value: "all", selected: filter_status() == ReleaseFilter::All, "All decisions" }
                        option { value: "approved", selected: filter_status() == ReleaseFilter::Approved, "Accepted" }
                        option { value: "rejected", selected: filter_status() == ReleaseFilter::Rejected, "Rejected" }
                        option { value: "unmatched", selected: filter_status() == ReleaseFilter::Unmatched, "Unmatched titles" }
                    }
                    button { class: "btn sm", onclick: move |_| { filter_text.set(String::new()); filter_status.set(ReleaseFilter::All); confirm.set(None); }, "Clear filters" }
                }
                p { class: "small muted", role: "status", "Showing {visible.len()} of {r.releases.len()} releases. Filters apply to these results without searching again." }
            }
            p { class: "help small faint",
                "Releases are checked against the series' quality profile, custom formats and release profiles, and ranked, as Sonarr does. Rejected releases ask for a second click."
            }
            if visible.is_empty() {
                Empty { title: "No releases match these filters", p { "Clear or adjust the filters to see more results." } }
            } else { div { class: "table-wrap",
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
                        for rel in visible {
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
                                    if rel.parsed && !rel.matches_series {
                                        p { class: "small muted", "Parsed title: " {rel.series_title.as_deref().unwrap_or("unknown")} }
                                    }
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
            } }
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
