//! Kitsunarr web interface (Dioxus, WebAssembly).
//!
//! All data comes from the REST API through `kitsunarr-client`; this crate holds presentation
//! only. Live updates arrive over Server-Sent Events.

mod components;
mod events;
mod i18n;
mod pages;
mod route;
mod state;
mod translate;
mod util;

use std::rc::Rc;

use dioxus::prelude::*;

const MAIN_CSS: Asset = asset!("/assets/main.css");
pub const FAVICON: Asset = asset!("/assets/favicon.svg");

fn main() {
    // The interface language chosen in Settings › General (English needs nothing).
    translate::install();
    let prefix = util::url_base();
    let history = dioxus::web::WebHistory::new(
        if prefix.is_empty() {
            None
        } else {
            Some(prefix)
        },
        true,
    );
    dioxus::LaunchBuilder::new()
        .with_cfg(dioxus::web::Config::new().history(Rc::new(history)))
        .launch(App);
}

#[component]
fn App() -> Element {
    state::provide_app_state();
    let base = util::url_base();
    rsx! {
        document::Title { "Kitsunarr" }
        document::Link { rel: "icon", r#type: "image/svg+xml", href: "{base}{FAVICON}" }
        document::Stylesheet { href: "{base}{MAIN_CSS}" }
        Router::<route::Route> {}
    }
}
