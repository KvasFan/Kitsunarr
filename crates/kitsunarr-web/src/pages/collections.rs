//! Collections of series and movies.

use dioxus::prelude::*;

use crate::components::common::{Badge, Empty, ErrorBox, Loading};
use crate::components::media::Poster;
use crate::state::{use_app, use_client};

#[component]
pub fn Collections() -> Element {
    let app = use_app();
    let client = use_client();
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let c = client.clone();
        async move { c.collections().await }
    });
    rsx! {
        div { class: "page-head", h1 { "Collections" } }
        p { class: "muted", "Groups of related series and movies: provider collections, anime franchises (prequel/sequel groups) and manual groups, including those migrated from Tanukarr." }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "No collections yet", p { "Collections appear when they are migrated from Tanukarr or created from provider data." } } },
            Some(Ok(list)) => rsx! {
                div { class: "media-grid",
                    for c in list.iter() {
                        div { key: "{c.id}", class: "media-card",
                            Poster { url: c.poster_url.clone(), title: c.title.clone(), monitored: true }
                            div { class: "meta",
                                span { class: "t ellipsis", title: "{c.title}", "{c.title}" }
                                div { class: "row tight",
                                    Badge { text: match c.kind.as_str() { "animeGroup" => "franchise".to_string(), other => other.to_string() } }
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
