//! Add media: search a metadata catalogue and add series, anime or films to the library.
//!
//! Adding never touches the disk. The folder shown in the dialog is computed with the same
//! naming code the server uses; it is created by the import pipeline once files arrive.

use dioxus::prelude::*;
use kitsunarr_domain::api::metadata::{
    AddTitleRequest, CatalogueInfo, EntryKind, MonitorOption, SearchItem, SearchQuery,
};
use kitsunarr_domain::{MediaKind, MetadataProvider, SeriesStatus, SeriesType, naming};

use crate::components::common::{Badge, Empty, ErrorBox, Field, Loading, Modal, Switch};
use crate::components::media::Poster;
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, load_pref, save_pref};

const PROVIDER_PREF: &str = "add.provider";

const SERIES_MONITOR: &[(MonitorOption, &str, &str)] = &[
    (MonitorOption::All, "all", "All episodes"),
    (MonitorOption::Future, "future", "Future episodes"),
    (MonitorOption::FirstSeason, "firstSeason", "First season"),
    (MonitorOption::LatestSeason, "latestSeason", "Latest season"),
    (MonitorOption::None, "none", "None"),
];

const SERIES_TYPES: &[(SeriesType, &str)] = &[
    (SeriesType::Standard, "Standard"),
    (SeriesType::Daily, "Daily"),
    (SeriesType::Anime, "Anime / absolute numbering"),
];

fn kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Series => "Series",
        EntryKind::Anime => "Anime",
        EntryKind::Movie => "Film",
    }
}

fn status_label(status: SeriesStatus) -> Option<(&'static str, &'static str)> {
    match status {
        SeriesStatus::Continuing => Some(("Continuing", "success")),
        SeriesStatus::Ended => Some(("Ended", "info")),
        SeriesStatus::Upcoming => Some(("Upcoming", "warning")),
        SeriesStatus::Deleted => Some(("Removed", "danger")),
        _ => None,
    }
}

fn detail_route(media_type: &str, id: i64) -> Route {
    if media_type == "movie" {
        Route::MovieDetailPage { id }
    } else {
        Route::SeriesDetailPage { id }
    }
}

#[component]
pub fn AddMedia() -> Element {
    let client = use_client();
    let c1 = client.clone();
    let catalogues = use_resource(move || {
        let c = c1.clone();
        async move { c.metadata_providers().await }
    });
    let mut provider = use_signal(|| {
        load_pref(PROVIDER_PREF)
            .and_then(|p| p.parse::<MetadataProvider>().ok())
            .unwrap_or(MetadataProvider::TvMaze)
    });
    let mut term = use_signal(String::new);
    // The last submitted search; results follow it.
    let mut submitted = use_signal(|| None::<SearchQuery>);
    let mut adding = use_signal(|| None::<SearchItem>);
    let c2 = client.clone();
    let mut results = use_resource(move || {
        let query = submitted();
        let c = c2.clone();
        async move {
            match query {
                Some(q) => c.metadata_search(&q).await.map(Some),
                None => Ok(None),
            }
        }
    });

    let search = move |e: FormEvent| {
        e.prevent_default();
        let t = term().trim().to_string();
        if t.is_empty() {
            return;
        }
        let q = SearchQuery {
            provider: provider(),
            term: t,
        };
        if submitted.peek().as_ref() == Some(&q) {
            results.restart();
        } else {
            submitted.set(Some(q));
        }
    };

    let read = catalogues.read();
    let list: Vec<CatalogueInfo> = match &*read {
        None => return rsx! { div { class: "page-head", h1 { "Add media" } } Loading {} },
        Some(Err(e)) => {
            return rsx! { div { class: "page-head", h1 { "Add media" } } ErrorBox { message: e.to_string() } };
        }
        Some(Ok(l)) => l.clone(),
    };
    drop(read);
    let current = list
        .iter()
        .find(|c| c.provider == provider())
        .or(list.first())
        .cloned();
    let placeholder = match current.as_ref().map(|c| c.provider) {
        Some(MetadataProvider::AniList) => "Anime title, e.g. Frieren",
        _ => "Series title, e.g. Severance",
    };

    rsx! {
        div { class: "page-head",
            h1 { "Add media" }
            div { class: "spacer" }
            Link { to: Route::ImportWizard {}, class: "btn", "Import a Sonarr or Tanukarr library" }
        }
        form { class: "toolbar", role: "search", onsubmit: search,
            div { class: "seg", role: "group", "aria-label": "Catalogue",
                for c in list.iter() {
                    button {
                        key: "{c.provider}",
                        r#type: "button",
                        class: if current.as_ref().map(|x| x.provider) == Some(c.provider) { "on" } else { "" },
                        "aria-pressed": "{current.as_ref().map(|x| x.provider) == Some(c.provider)}",
                        title: "{c.description}",
                        onclick: {
                            let p = c.provider;
                            move |_| {
                                provider.set(p);
                                save_pref(PROVIDER_PREF, p.as_str());
                            }
                        },
                        "{c.name}"
                    }
                }
            }
            input {
                id: "add-search",
                class: "input search grow",
                r#type: "search",
                autofocus: true,
                autocomplete: "off",
                placeholder,
                "aria-label": "Title to search for",
                value: "{term}",
                oninput: move |e| term.set(e.value()),
            }
            button { class: "btn primary", r#type: "submit", disabled: term().trim().is_empty(), "Search" }
        }
        if let Some(c) = current.as_ref() {
            p { class: "attribution",
                "{c.description}. "
                a { href: "{c.url}", target: "_blank", rel: "noopener", "{c.attribution}" }
                "."
            }
        }
        match &*results.read() {
            Some(Ok(Some(items))) if !items.is_empty() => rsx! {
                div { class: "result-list", "aria-live": "polite",
                    for item in items.iter() {
                        ResultRow {
                            key: "{item.provider}-{item.id}",
                            item: item.clone(),
                            on_add: move |i| adding.set(Some(i)),
                        }
                    }
                }
            },
            Some(Ok(Some(_))) => rsx! {
                Empty { title: "No matches",
                    p { "Nothing in this catalogue matches the search. Try another spelling, the original title, or the other catalogue." }
                }
            },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string(), on_retry: move |_| results.restart() } },
            None => rsx! { Loading { label: "Searching…" } },
            Some(Ok(None)) => rsx! {
                Empty { title: "Find something to add",
                    p { "TVmaze covers TV series; AniList covers anime series and anime films. Adding a title creates its library entry and episode list; files are picked up later by the import pipeline." }
                }
            },
        }
        if let Some(item) = adding() {
            AddDialog { item, on_close: move |_| adding.set(None) }
        }
    }
}

#[component]
fn ResultRow(item: SearchItem, on_add: EventHandler<SearchItem>) -> Element {
    let status = status_label(item.status);
    let to_add = item.clone();
    rsx! {
        article { class: "result",
            Poster { url: item.poster_url.clone(), title: item.title.clone(), monitored: true }
            div { class: "body",
                div { class: "row tight",
                    h3 { "{item.title}" }
                    if let Some(y) = item.year { span { class: "muted", "({y})" } }
                }
                div { class: "row tight meta",
                    Badge { text: kind_label(item.kind).to_string(), tone: "accent" }
                    if let Some((label, tone)) = status { Badge { text: label.to_string(), tone } }
                    if let Some(n) = item.network.as_ref() { span { class: "small muted", "{n}" } }
                }
                if let Some(o) = item.overview.as_ref() { p { class: "overview", "{o}" } }
            }
            div { class: "actions",
                if let Some(r) = item.in_library.as_ref() {
                    Link { to: detail_route(&r.media_type, r.id), class: "btn", "In library ›" }
                } else {
                    button {
                        class: "btn primary",
                        "aria-label": "Add {item.title}",
                        onclick: move |_| on_add.call(to_add.clone()),
                        "Add"
                    }
                }
            }
        }
    }
}

#[component]
fn AddDialog(item: SearchItem, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let nav = navigator();
    let c1 = client.clone();
    let options = use_resource(move || {
        let c = c1.clone();
        async move {
            let roots = c.root_folders().await?;
            let profiles = c.quality_profiles().await?;
            let tags = c.tags().await?;
            let naming = c.naming().await?;
            Ok::<_, kitsunarr_client::ClientError>((roots, profiles, tags, naming))
        }
    });
    let is_movie = item.kind == EntryKind::Movie;
    let wanted_kind = match item.kind {
        EntryKind::Series => MediaKind::Series,
        EntryKind::Anime => MediaKind::Anime,
        EntryKind::Movie => MediaKind::Movie,
    };
    let mut root = use_signal(|| None::<String>);
    let mut profile = use_signal(|| None::<i64>);
    let mut monitor = use_signal(|| MonitorOption::All);
    let initial_type = if item.kind == EntryKind::Anime {
        SeriesType::Anime
    } else {
        SeriesType::Standard
    };
    let mut series_type = use_signal(|| initial_type);
    let mut season_folder = use_signal(|| true);
    let mut tags = use_signal(Vec::<i64>::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // Preselect a root folder meant for this kind of title (else one not reserved for another
    // kind), and the first profile.
    use_effect(move || {
        if let Some(Ok((roots, profiles, _, _))) = &*options.read() {
            if root.peek().is_none() {
                let pick = roots
                    .iter()
                    .find(|r| r.media_kind == Some(wanted_kind))
                    .or_else(|| roots.iter().find(|r| r.media_kind.is_none()))
                    .or(roots.first());
                root.set(pick.map(|r| r.path.clone()));
            }
            if profile.peek().is_none() {
                profile.set(profiles.first().map(|p| p.id));
            }
        }
    });

    let request_item = item.clone();
    let submit = move |_| {
        let (Some(root_path), Some(profile_id)) = (root(), profile()) else {
            error.set(Some("Choose a root folder and a quality profile.".into()));
            return;
        };
        let req = AddTitleRequest {
            provider: request_item.provider,
            id: request_item.id.clone(),
            root_folder_path: root_path,
            quality_profile_id: profile_id,
            monitor: monitor(),
            series_type: if is_movie { None } else { Some(series_type()) },
            season_folder: season_folder(),
            tags: tags(),
        };
        let c = client.clone();
        let name = request_item.title.clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match c.add_title(&req).await {
                Ok(added) => {
                    app.success(format!("{name} was added to the library."));
                    nav.push(detail_route(&added.media_type, added.id));
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };

    let heading = match item.year {
        Some(y) => format!("Add {} ({y})", item.title),
        None => format!("Add {}", item.title),
    };
    let read = options.read();
    let (body, ready) = match &*read {
        None => (rsx! { Loading {} }, false),
        Some(Err(e)) => (rsx! { ErrorBox { message: e.to_string() } }, false),
        Some(Ok((roots, profiles, all_tags, naming_settings))) => {
            let format = if is_movie {
                &naming_settings.movie_folder_format
            } else {
                &naming_settings.series_folder_format
            };
            let folder = naming::folder_name(
                format,
                &item.title,
                item.year,
                naming_settings.colon_replacement_format,
                &naming_settings.custom_colon_replacement_format,
            );
            let preview = root().map(|r| naming::join(&r, &folder));
            let ready = !roots.is_empty() && !profiles.is_empty();
            let body = rsx! {
                if roots.is_empty() {
                    div { class: "alert warning", role: "alert",
                        div { class: "grow",
                            "Add a root folder first: it is where this title's folder will live. "
                            Link { to: Route::SettingsMedia {}, "Media management settings" }
                        }
                    }
                } else {
                    div { class: "form-grid",
                        Field { label: "Root folder",
                            select {
                                class: "select",
                                value: root().unwrap_or_default(),
                                onchange: move |e: FormEvent| root.set(Some(e.value())),
                                for r in roots.iter() {
                                    option { key: "{r.id}", value: "{r.path}", disabled: !r.accessible,
                                        {match (r.accessible, r.free_space) {
                                            (false, _) => format!("{} (not accessible)", r.path),
                                            (true, Some(free)) => format!("{} ({} free)", r.path, format_bytes(free as i64)),
                                            (true, None) => r.path.clone(),
                                        }}
                                    }
                                }
                            }
                        }
                        Field { label: "Quality profile",
                            select {
                                class: "select",
                                value: profile().map(|p| p.to_string()).unwrap_or_default(),
                                onchange: move |e: FormEvent| profile.set(e.value().parse().ok()),
                                for p in profiles.iter() { option { key: "{p.id}", value: "{p.id}", "{p.name}" } }
                            }
                        }
                        if is_movie {
                            Field { label: "Monitor",
                                select {
                                    class: "select",
                                    value: if monitor() == MonitorOption::None { "none" } else { "all" },
                                    onchange: move |e: FormEvent| monitor.set(if e.value() == "none" { MonitorOption::None } else { MonitorOption::All }),
                                    option { value: "all", "Monitored" }
                                    option { value: "none", "Not monitored" }
                                }
                            }
                        } else {
                            Field { label: "Monitor", help: "Specials are never monitored automatically.".to_string(),
                                select {
                                    class: "select",
                                    value: SERIES_MONITOR.iter().find(|(m, _, _)| *m == monitor()).map(|(_, v, _)| *v).unwrap_or("all"),
                                    onchange: move |e: FormEvent| {
                                        if let Some((m, _, _)) = SERIES_MONITOR.iter().find(|(_, v, _)| *v == e.value()) {
                                            monitor.set(*m);
                                        }
                                    },
                                    for (_, value, label) in SERIES_MONITOR.iter() { option { key: "{value}", value: "{value}", "{label}" } }
                                }
                            }
                            Field { label: "Series type",
                                select {
                                    class: "select",
                                    value: series_type().as_str(),
                                    onchange: move |e: FormEvent| {
                                        if let Ok(t) = e.value().parse::<SeriesType>() {
                                            series_type.set(t);
                                        }
                                    },
                                    for (t, label) in SERIES_TYPES.iter() { option { key: "{t}", value: t.as_str(), "{label}" } }
                                }
                            }
                            Field { label: "Season folders",
                                Switch {
                                    checked: season_folder(),
                                    label: if season_folder() { "One folder per season".to_string() } else { "All episodes in the series folder".to_string() },
                                    onchange: move |v| season_folder.set(v),
                                }
                            }
                        }
                    }
                    if !all_tags.is_empty() {
                        Field { label: "Tags",
                            div { class: "chips", role: "group", "aria-label": "Tags",
                                for t in all_tags.iter() {
                                    button {
                                        key: "{t.id}",
                                        r#type: "button",
                                        class: if tags().contains(&t.id) { "chip on" } else { "chip" },
                                        "aria-pressed": "{tags().contains(&t.id)}",
                                        onclick: {
                                            let id = t.id;
                                            move |_| {
                                                let mut v = tags.write();
                                                if let Some(i) = v.iter().position(|x| *x == id) {
                                                    v.remove(i);
                                                } else {
                                                    v.push(id);
                                                }
                                            }
                                        },
                                        "{t.label}"
                                    }
                                }
                            }
                        }
                    }
                    if let Some(p) = preview {
                        Field { label: "Folder", help: "Created when the first file is imported. Nothing is written to disk now.".to_string(),
                            div { class: "path-preview", "{p}" }
                        }
                    }
                }
                if let Some(msg) = error() { div { class: "alert danger", role: "alert", "{msg}" } }
            };
            (body, ready)
        }
    };
    drop(read);

    rsx! {
        Modal {
            title: heading,
            wide: true,
            on_close: move |_| on_close.call(()),
            footer: rsx! {
                button { class: "btn", onclick: move |_| on_close.call(()), {crate::i18n::t().cancel} }
                button { class: "btn primary", disabled: busy() || !ready, onclick: submit,
                    if busy() { "Adding…" } else { "Add to library" }
                }
            },
            {body}
        }
    }
}
