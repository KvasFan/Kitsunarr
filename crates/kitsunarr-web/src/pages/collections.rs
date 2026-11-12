//! Collections of series and movies, and Tanukarr's manual grouping.

use dioxus::prelude::*;
use kitsunarr_domain::api::library::{
    CollectionDetail, CollectionEdit, CollectionMember, CollectionMemberEdit, LibraryQuery,
};

use crate::components::common::{Badge, Confirm, Empty, ErrorBox, Field, Loading};
use crate::components::media::Poster;
use crate::route::Route;
use crate::state::{use_app, use_client};

fn kind_label(kind: &str) -> String {
    match kind {
        "animeGroup" => "franchise".into(),
        other => other.into(),
    }
}

#[component]
pub fn Collections() -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let c = c0.clone();
        async move { c.collections().await }
    });
    let mut new_title = use_signal(String::new);
    let nav = use_navigator();
    let create = move |_| {
        let c = client.clone();
        let title = new_title();
        spawn(async move {
            match c
                .create_collection(&CollectionEdit {
                    title,
                    overview: None,
                })
                .await
            {
                Ok(d) => {
                    new_title.set(String::new());
                    nav.push(Route::CollectionDetailPage { id: d.id });
                }
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        div { class: "page-head", h1 { "Collections" } }
        p { class: "muted", "Groups of related series and movies: provider collections, anime franchises (prequel/sequel groups) and manual groups, including those migrated from Tanukarr." }
        div { class: "row", style: "margin-bottom: var(--sp-4)",
            input { class: "input", placeholder: "New group title", "aria-label": "New group title", value: "{new_title}",
                oninput: move |e| new_title.set(e.value()) }
            button { class: "btn primary", disabled: new_title().trim().is_empty(), onclick: create, "Create group" }
        }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "No collections yet", p { "Collections appear when they are migrated from Tanukarr, created from provider data, or created here." } } },
            Some(Ok(list)) => rsx! {
                div { class: "media-grid",
                    for c in list.iter() {
                        Link { key: "{c.id}", class: "media-card", to: Route::CollectionDetailPage { id: c.id },
                            Poster { url: c.poster_url.clone(), title: c.title.clone(), monitored: true }
                            div { class: "meta",
                                span { class: "t ellipsis", title: "{c.title}", "{c.title}" }
                                div { class: "row tight",
                                    Badge { text: kind_label(&c.kind) }
                                    span { class: "xs muted", "{c.series_count} series · {c.movie_count} movies" }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}

/// A collection's members, editable by hand (Tanukarr's manual grouping).
#[component]
pub fn CollectionDetailPage(id: i64) -> Element {
    let client = use_client();
    let c0 = client.clone();
    let mut data = use_resource(move || {
        let c = c0.clone();
        async move { c.collection(id).await }
    });
    let read = data.read();
    let d = match &*read {
        None => return rsx! { Loading {} },
        Some(Err(e)) => return rsx! { ErrorBox { message: e.to_string() } },
        Some(Ok(d)) => d.clone(),
    };
    drop(read);
    rsx! {
        CollectionEditor { key: "{d.id}", detail: d, on_change: move |_| data.restart() }
    }
}

#[component]
fn CollectionEditor(detail: CollectionDetail, on_change: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let id = detail.id;
    let mut title = use_signal(|| detail.title.clone());
    let mut overview = use_signal(|| detail.overview.clone().unwrap_or_default());
    let mut confirm_delete = use_signal(|| false);
    let nav = use_navigator();
    let c1 = client.clone();
    let save = move |_| {
        let c = c1.clone();
        let edit = CollectionEdit {
            title: title(),
            overview: Some(overview()),
        };
        spawn(async move {
            match c.update_collection(id, &edit).await {
                Ok(_) => {
                    app.success("Saved");
                    on_change.call(());
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c2 = client.clone();
    let delete = move |_| {
        let c = c2.clone();
        spawn(async move {
            match c.delete_collection(id).await {
                Ok(()) => {
                    nav.push(Route::Collections {});
                }
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        div { class: "page-head",
            h1 { "{detail.title}" }
            Badge { text: kind_label(&detail.kind) }
            div { class: "spacer" }
            Link { class: "btn ghost", to: Route::Collections {}, "All collections" }
            {
                let c = client.clone();
                rsx! {
                    button { class: "btn", title: "Search every member's missing episodes (films: the film)",
                        onclick: move |_| {
                            let c = c.clone();
                            spawn(async move {
                                match c.search_collection(id).await {
                                    Ok(r) => app.success(r.message),
                                    Err(e) => app.error(&e),
                                }
                            });
                        },
                        "Search collection"
                    }
                }
            }
            button { class: "btn danger", onclick: move |_| confirm_delete.set(true), "Delete group" }
        }
        div { class: "card stack",
            div { class: "form-grid",
                Field { label: "Title",
                    input { class: "input", value: "{title}", oninput: move |e| title.set(e.value()) }
                }
                Field { label: "Overview",
                    input { class: "input", value: "{overview}", oninput: move |e| overview.set(e.value()) }
                }
            }
            div { class: "row", button { class: "btn primary", disabled: title().trim().is_empty(), onclick: save, "Save" } }
            if detail.kind == "animeGroup" {
                p { class: "help small faint", "A franchise group found from AniList or MyAnimeList relations. Titles added later from the same franchise still join it." }
            }
        }
        h2 { "Members" }
        if detail.members.is_empty() {
            Empty { title: "No members", p { "Add series or movies below." } }
        } else {
            div { class: "table-wrap",
                table { class: "table",
                    thead { tr { th { "Title" } th { "Kind" } th { "Label" } th { class: "num", "Position" } th { span { class: "sr-only", "Actions" } } } }
                    tbody {
                        for m in detail.members.iter() {
                            MemberRow { key: "{m.id}", collection_id: id, member: m.clone(), on_change }
                        }
                    }
                }
            }
        }
        AddMember { collection_id: id, on_change }
        if confirm_delete() {
            Confirm {
                title: "Delete group",
                message: format!("Delete “{}”? Its series and movies stay in the library.", detail.title),
                confirm_label: "Delete",
                danger: true,
                on_confirm: delete,
                on_cancel: move |_| confirm_delete.set(false),
            }
        }
    }
}

#[component]
fn MemberRow(collection_id: i64, member: CollectionMember, on_change: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let mut label = use_signal(|| member.label.clone().unwrap_or_default());
    let mut position = use_signal(|| member.position.to_string());
    let mut role = use_signal(|| member.role.clone().unwrap_or_default());
    let mid = member.id;
    let c1 = client.clone();
    let save = move |_| {
        let c = c1.clone();
        let edit = CollectionMemberEdit {
            role: Some(role()).filter(|r| !r.is_empty()),
            label: Some(label()),
            position: position().trim().parse().ok(),
            ..Default::default()
        };
        spawn(async move {
            match c.update_collection_member(collection_id, mid, &edit).await {
                Ok(_) => on_change.call(()),
                Err(e) => app.error(&e),
            }
        });
    };
    let c2 = client.clone();
    let remove = move |_| {
        let c = c2.clone();
        spawn(async move {
            match c.remove_collection_member(collection_id, mid).await {
                Ok(_) => on_change.call(()),
                Err(e) => app.error(&e),
            }
        });
    };
    let link = match (member.series_id, member.movie_id) {
        (Some(s), _) => Some(Route::SeriesDetailPage { id: s }),
        (_, Some(m)) => Some(Route::MovieDetailPage { id: m }),
        _ => None,
    };
    let year = member.year.map(|y| format!(" ({y})")).unwrap_or_default();
    rsx! {
        tr {
            td {
                if let Some(to) = link { Link { to, "{member.title}{year}" } } else { "{member.title}{year}" }
            }
            td {
                select { class: "input", "aria-label": "Kind of {member.title}", value: "{role}", onchange: move |e| role.set(e.value()),
                    option { value: "", "—" }
                    option { value: "season", "Season" }
                    option { value: "movie", "Movie" }
                    option { value: "special", "Special" }
                }
            }
            td { input { class: "input", "aria-label": "Label of {member.title}", value: "{label}", oninput: move |e| label.set(e.value()) } }
            td { class: "num", input { class: "input", style: "width: 6rem", r#type: "number", "aria-label": "Position of {member.title}", value: "{position}", oninput: move |e| position.set(e.value()) } }
            td { class: "actions",
                div { class: "row tight",
                    button { class: "btn sm", onclick: save, "Save" }
                    button { class: "btn sm ghost", "aria-label": "Remove {member.title}", onclick: remove, "Remove" }
                }
            }
        }
    }
}

#[component]
fn AddMember(collection_id: i64, on_change: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    // Every series and movie: (key, label).
    let titles = use_resource(move || {
        let c = c0.clone();
        async move {
            let q = LibraryQuery {
                page_size: Some(5000),
                ..Default::default()
            };
            let mut out: Vec<(String, String)> = Vec::new();
            if let Ok(p) = c.series(&q).await {
                out.extend(p.records.into_iter().map(|s| {
                    let year = s.year.map(|y| format!(" ({y})")).unwrap_or_default();
                    (format!("s{}", s.id), format!("{}{year}", s.title))
                }));
            }
            if let Ok(p) = c.movies(&q).await {
                out.extend(p.records.into_iter().map(|m| {
                    let year = m.year.map(|y| format!(" ({y})")).unwrap_or_default();
                    (format!("m{}", m.id), format!("{}{year} — movie", m.title))
                }));
            }
            out.sort_by_key(|a| a.1.to_lowercase());
            out
        }
    });
    let mut chosen = use_signal(String::new);
    let mut role = use_signal(|| "season".to_string());
    let add = move |_| {
        let c = client.clone();
        let key = chosen();
        let id: Option<i64> = key.get(1..).and_then(|v| v.parse().ok());
        let edit = CollectionMemberEdit {
            series_id: id.filter(|_| key.starts_with('s')),
            movie_id: id.filter(|_| key.starts_with('m')),
            role: Some(role()).filter(|r| !r.is_empty()),
            ..Default::default()
        };
        spawn(async move {
            match c.add_collection_member(collection_id, &edit).await {
                Ok(_) => {
                    chosen.set(String::new());
                    on_change.call(());
                }
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        h2 { "Add a title" }
        p { class: "help small faint", "A title joining a franchise or manual group leaves the group it was in. Files and folders are not moved." }
        div { class: "row",
            select { class: "input", "aria-label": "Title to add", value: "{chosen}", onchange: move |e| chosen.set(e.value()),
                option { value: "", "Choose a series or movie…" }
                if let Some(list) = &*titles.read() {
                    for (key, label) in list.iter() {
                        option { key: "{key}", value: "{key}", "{label}" }
                    }
                }
            }
            select { class: "input", "aria-label": "Kind", value: "{role}", onchange: move |e| role.set(e.value()),
                option { value: "season", "Season" }
                option { value: "movie", "Movie" }
                option { value: "special", "Special" }
                option { value: "", "Other" }
            }
            button { class: "btn primary", disabled: chosen().is_empty(), onclick: add, "Add" }
        }
    }
}
