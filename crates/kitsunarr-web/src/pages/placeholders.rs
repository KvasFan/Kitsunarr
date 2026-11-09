//! Pages for features that are planned but not implemented yet. They say so plainly instead
//! of showing controls that do nothing.

use dioxus::prelude::*;

use crate::components::common::NotReady;
use crate::route::Route;

#[component]
pub fn InteractiveSearch() -> Element {
    rsx! {
        div { class: "page-head", h1 { "Interactive search" } }
        NotReady {
            feature: "Interactive search",
            detail: "Searching indexers needs the Torznab/Newznab clients and the release parser, which are being ported from Sonarr. Imported indexer settings are kept and will be used automatically.",
        }
    }
}

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let path = segments.join("/");
    rsx! {
        div { class: "empty",
            h3 { "Page not found" }
            p { class: "muted", "There is nothing at /{path}." }
            Link { to: Route::Dashboard {}, class: "btn", "Back to the dashboard" }
        }
    }
}
