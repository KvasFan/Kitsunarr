//! Pages for features that are planned but not implemented yet. They say so plainly instead
//! of showing controls that do nothing.

use dioxus::prelude::*;

use crate::route::Route;

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
