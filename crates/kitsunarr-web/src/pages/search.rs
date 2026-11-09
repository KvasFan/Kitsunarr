//! Interactive search: pick a series, then a season or an episode, and review what the
//! indexers offer.

use dioxus::prelude::*;
use kitsunarr_domain::api::library::LibraryQuery;

use crate::components::common::{Empty, Field, Loading};
use crate::components::releases::ReleaseSearch;
use crate::state::use_client;

#[component]
pub fn InteractiveSearch() -> Element {
    let client = use_client();
    let mut term = use_signal(String::new);
    let mut series_id = use_signal(|| None::<i64>);
    let mut season = use_signal(|| None::<i32>);
    let mut episode = use_signal(|| None::<i64>);
    // What the results below show; set by the Search button.
    let mut target = use_signal(|| None::<(i64, Option<i32>, Option<i64>)>);

    let c1 = client.clone();
    let matches = use_resource(move || {
        let t = term();
        let c = c1.clone();
        async move {
            if t.trim().len() < 2 {
                return Ok(Vec::new());
            }
            let q = LibraryQuery {
                search: Some(t),
                page_size: Some(12),
                ..Default::default()
            };
            c.series(&q).await.map(|p| p.records)
        }
    });
    let c2 = client.clone();
    let detail = use_resource(move || {
        let id = series_id();
        let c = c2.clone();
        async move {
            match id {
                Some(id) => c.series_detail(id).await.map(Some),
                None => Ok(None),
            }
        }
    });
    let c3 = client.clone();
    let episodes = use_resource(move || {
        let id = series_id();
        let c = c3.clone();
        async move {
            match id {
                Some(id) => c.episodes(id).await,
                None => Ok(Vec::new()),
            }
        }
    });

    let selected = detail
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok().cloned().flatten());
    let season_episodes: Vec<_> = match (&*episodes.read(), season()) {
        (Some(Ok(list)), Some(s)) => list
            .iter()
            .filter(|e| e.season_number == s)
            .cloned()
            .collect(),
        _ => Vec::new(),
    };

    rsx! {
        div { class: "page-head", h1 { "Interactive search" } }
        div { class: "card",
            div { class: "form-grid",
                Field { label: "Series",
                    input {
                        class: "input search",
                        r#type: "search",
                        placeholder: "Find a series in your library",
                        "aria-label": "Series",
                        value: "{term}",
                        oninput: move |e| term.set(e.value()),
                    }
                }
                if let Some(s) = selected.as_ref() {
                    Field { label: "Season",
                        select {
                            class: "select",
                            "aria-label": "Season",
                            onchange: move |e: FormEvent| {
                                season.set(e.value().parse().ok());
                                episode.set(None);
                            },
                            option { value: "", "Choose a season" }
                            for se in s.seasons.iter().rev() {
                                option { key: "{se.season_number}", value: "{se.season_number}", selected: season() == Some(se.season_number),
                                    if se.season_number == 0 { "Specials" } else { "Season {se.season_number}" }
                                }
                            }
                        }
                    }
                    Field { label: "Episode",
                        select {
                            class: "select",
                            "aria-label": "Episode",
                            disabled: season().is_none(),
                            onchange: move |e: FormEvent| episode.set(e.value().parse().ok()),
                            option { value: "", "Whole season" }
                            for e in season_episodes.iter() {
                                option { key: "{e.id}", value: "{e.id}",
                                    {format!("E{:02} {}", e.episode_number, e.title.clone().unwrap_or_default())}
                                }
                            }
                        }
                    }
                }
            }
            if selected.is_none() {
                match &*matches.read() {
                    Some(Ok(list)) if !list.is_empty() => rsx! {
                        div { class: "chips", role: "listbox", "aria-label": "Matching series",
                            for s in list.iter() {
                                button {
                                    key: "{s.id}",
                                    class: "chip",
                                    r#type: "button",
                                    onclick: {
                                        let id = s.id;
                                        let title = s.title.clone();
                                        move |_| {
                                            series_id.set(Some(id));
                                            term.set(title.clone());
                                        }
                                    },
                                    "{s.title}"
                                    if let Some(y) = s.year { " ({y})" }
                                }
                            }
                        }
                    },
                    Some(Ok(_)) if term().trim().len() >= 2 => rsx! { p { class: "small muted", "No series in your library match." } },
                    _ => rsx! {},
                }
            } else {
                div { class: "row",
                    button { class: "btn", onclick: move |_| { series_id.set(None); season.set(None); episode.set(None); target.set(None); }, "Change series" }
                    div { class: "spacer" }
                    button {
                        class: "btn primary",
                        disabled: season().is_none(),
                        onclick: move |_| {
                            if let Some(id) = series_id() {
                                let ep = episode();
                                target.set(Some((id, if ep.is_some() { None } else { season() }, ep)));
                            }
                        },
                        "Search indexers"
                    }
                }
            }
        }
        match target() {
            Some((id, s, e)) => rsx! {
                div { class: "card",
                    ReleaseSearch { key: "{id}-{s:?}-{e:?}", series_id: id, season: s, episode_id: e }
                }
            },
            None if detail.read().is_none() && series_id().is_some() => rsx! { Loading {} },
            None => rsx! {
                Empty { title: "Search your indexers",
                    p { "Choose a series and a season or episode. Results show what each release is, as Kitsunarr's parser reads it." }
                }
            },
        }
    }
}
