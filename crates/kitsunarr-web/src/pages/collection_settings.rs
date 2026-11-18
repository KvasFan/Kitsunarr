//! Tanukarr's `CollectionSettingsModal`: the group's name, kind and film folder style, and the
//! monitoring, quality profile and season folders applied to every member.

use dioxus::prelude::*;
use kitsunarr_domain::api::library::CollectionDetail;
use kitsunarr_domain::api::views::CollectionSettingsUpdate;

use crate::components::common::{Field, Modal};
use crate::state::{use_app, use_client};

#[component]
pub fn CollectionSettingsDialog(detail: CollectionDetail, on_close: EventHandler<bool>) -> Element {
    let app = use_app();
    let client = use_client();
    let id = detail.id;
    let mut title = use_signal(|| detail.title.clone());
    let mut kind = use_signal(|| detail.group_kind.clone());
    let mut style = use_signal(|| detail.movie_folder_style.clone());
    let mut monitored = use_signal(String::new);
    let mut profile = use_signal(String::new);
    let mut season_folder = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let c0 = client.clone();
    let profiles = use_resource(move || {
        let c = c0.clone();
        async move { c.quality_profiles().await.unwrap_or_default() }
    });
    let all_films = detail
        .members
        .iter()
        .all(|m| m.movie_id.is_some() || m.role.as_deref() == Some("movie"));
    let (orig_title, orig_kind, orig_style) = (
        detail.title.clone(),
        detail.group_kind.clone(),
        detail.movie_folder_style.clone(),
    );
    let changed = title().trim() != orig_title
        || kind() != orig_kind
        || style() != orig_style
        || !monitored().is_empty()
        || !profile().is_empty()
        || !season_folder().is_empty();
    let save = move |_| {
        let flag = |v: String| match v.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        let update = CollectionSettingsUpdate {
            title: Some(title().trim().to_string()).filter(|t| *t != orig_title),
            group_kind: Some(kind()).filter(|k| *k != orig_kind),
            movie_folder_style: Some(style()).filter(|s| *s != orig_style),
            monitored: flag(monitored()),
            quality_profile_id: profile().parse().ok(),
            season_folder: flag(season_folder()),
        };
        let c = client.clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match c.update_collection_settings(id, &update).await {
                Ok(_) => {
                    app.success("Collection settings saved.");
                    on_close.call(true);
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    busy.set(false);
                }
            }
        });
    };
    rsx! {
        Modal {
            title: "Collection settings".to_string(),
            on_close: move |_| { if !busy() { on_close.call(false) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(false), "Cancel" }
                button { class: "btn primary", disabled: busy() || !changed || title().trim().is_empty(), onclick: save,
                    if busy() { "Saving…" } else { "Save" }
                }
            },
            if let Some(e) = error() { div { class: "alert danger", role: "alert", "{e}" } }
            div { class: "form-grid",
                Field { label: "Name",
                    input { class: "input", "aria-label": "Collection name", value: "{title}", oninput: move |e| title.set(e.value()) }
                }
                Field { label: "Collection type", help: if all_films { None } else { Some("A movie saga holds only films.".to_string()) },
                    select { class: "select", "aria-label": "Collection type", onchange: move |e: FormEvent| kind.set(e.value()),
                        option { value: "series", selected: kind() == "series", "Series" }
                        option { value: "movieSaga", selected: kind() == "movieSaga", disabled: !all_films, "Movie saga" }
                    }
                }
                Field { label: "Movie folder format", help: "How member folders are named when titles join: automatic uses numbered folders in sagas and story-arc folders for arcs.".to_string(),
                    select { class: "select", "aria-label": "Movie folder format", onchange: move |e: FormEvent| style.set(e.value()),
                        option { value: "automatic", selected: style() == "automatic", "Automatic" }
                        option { value: "title", selected: style() == "title", "Title" }
                        option { value: "numbered", selected: style() == "numbered", "Numbered" }
                    }
                }
                Field { label: "Monitored",
                    select { class: "select", "aria-label": "Monitored", onchange: move |e: FormEvent| monitored.set(e.value()),
                        option { value: "", selected: monitored().is_empty(), "No change" }
                        option { value: "true", "Monitored" }
                        option { value: "false", "Unmonitored" }
                    }
                }
                Field { label: "Quality profile",
                    select { class: "select", "aria-label": "Quality profile", onchange: move |e: FormEvent| profile.set(e.value()),
                        option { value: "", selected: profile().is_empty(), "No change" }
                        if let Some(list) = &*profiles.read() {
                            for p in list.iter() { option { key: "{p.id}", value: "{p.id}", "{p.name}" } }
                        }
                    }
                }
                Field { label: "Season folder",
                    select { class: "select", "aria-label": "Season folder", onchange: move |e: FormEvent| season_folder.set(e.value()),
                        option { value: "", selected: season_folder().is_empty(), "No change" }
                        option { value: "true", "Yes" }
                        option { value: "false", "No" }
                    }
                }
            }
            p { class: "help small", "Monitoring, quality profile and season folder apply to every member now; the type and folder format apply to titles joining from now on." }
        }
    }
}
