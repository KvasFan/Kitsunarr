//! Authenticated application shell: sidebar, top bar, toasts, live events, shortcuts.

use dioxus::prelude::*;
use futures::StreamExt;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::components::common::Loading;
use crate::i18n::t;
use crate::route::Route;
use crate::state::{use_app, use_client};

/// Ensures the user is signed in; redirects to setup or login otherwise.
#[component]
pub fn AppShell() -> Element {
    let mut app = use_app();
    let client = use_client();
    let nav = navigator();

    use_future(move || {
        let client = client.clone();
        async move {
            match client.auth_status().await {
                Ok(s) => app.auth.set(Some(s)),
                Err(e) => app.toast(
                    crate::state::ToastLevel::Error,
                    format!("Cannot reach the server: {e}"),
                ),
            }
        }
    });

    use_effect(move || {
        if let Some(s) = app.auth.read().as_ref() {
            if s.setup_required {
                nav.replace(Route::Setup {});
            } else if !s.authenticated {
                nav.replace(Route::Login {});
            }
        }
    });

    let ready = app.auth.read().as_ref().is_some_and(|s| s.authenticated);
    if !ready {
        return rsx! { div { class: "auth-wrap", Loading {} } Toasts {} };
    }
    let open = (app.nav_open)();
    rsx! {
        EventsBridge {}
        Shortcuts {}
        div { class: if open { "app nav-open" } else { "app" },
            Sidebar {}
            div { class: "scrim", onclick: move |_| app.nav_open.set(false) }
            div { class: "main",
                TopBar {}
                main { class: "content", id: "content", Outlet::<Route> {} }
            }
        }
        Toasts {}
    }
}

/// Mounted only when signed in, so the event stream never runs unauthenticated.
#[component]
fn EventsBridge() -> Element {
    crate::events::use_server_events(use_app());
    rsx! {}
}

#[derive(Clone, Copy)]
enum Shortcut {
    Search,
    Go(char),
    Escape,
}

/// Keyboard shortcuts: `/` focuses search, `g` then a letter navigates, `Esc` closes menus.
#[component]
fn Shortcuts() -> Element {
    let mut app = use_app();
    let nav = navigator();
    let co = use_coroutine(move |mut rx: UnboundedReceiver<Shortcut>| async move {
        while let Some(s) = rx.next().await {
            match s {
                Shortcut::Search => crate::util::focus("global-search"),
                Shortcut::Escape => app.nav_open.set(false),
                Shortcut::Go(c) => {
                    let target = match c {
                        'd' => Some(Route::Dashboard {}),
                        's' => Some(Route::SeriesLibrary {}),
                        'a' => Some(Route::AnimeLibrary {}),
                        'm' => Some(Route::MoviesLibrary {}),
                        'c' => Some(Route::Calendar {}),
                        'h' => Some(Route::Activity {}),
                        'i' => Some(Route::ImportWizard {}),
                        ',' => Some(Route::SettingsGeneral {}),
                        _ => None,
                    };
                    if let Some(r) = target {
                        nav.push(r);
                    }
                }
            }
        }
    });
    use_hook(move || {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let mut pending_g = false;
        let handler =
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                let typing = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
                    .is_some_and(|el| {
                        matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                            || el.is_content_editable()
                    });
                if e.ctrl_key() || e.meta_key() || e.alt_key() {
                    return;
                }
                let key = e.key();
                if key == "Escape" {
                    co.send(Shortcut::Escape);
                    return;
                }
                if typing {
                    return;
                }
                if pending_g {
                    pending_g = false;
                    if let Some(c) = key.chars().next() {
                        co.send(Shortcut::Go(c));
                    }
                    return;
                }
                match key.as_str() {
                    "/" => {
                        e.prevent_default();
                        co.send(Shortcut::Search);
                    }
                    "g" => pending_g = true,
                    _ => {}
                }
            });
        let _ = doc.add_event_listener_with_callback("keydown", handler.as_ref().unchecked_ref());
        handler.forget();
    });
    rsx! {}
}

#[component]
fn NavLink(to: Route, icon: &'static str, label: &'static str) -> Element {
    let mut app = use_app();
    let current = use_route::<Route>();
    let active = current == to;
    rsx! {
        Link {
            to: to.clone(),
            class: if active { "nav-link active" } else { "nav-link" },
            onclick: move |_| app.nav_open.set(false),
            span { class: "ico", "aria-hidden": "true", "{icon}" }
            span { "{label}" }
        }
    }
}

#[component]
fn Sidebar() -> Element {
    let s = t();
    rsx! {
        nav { class: "sidebar", "aria-label": "Main navigation",
            Link { to: Route::Dashboard {}, class: "brand",
                img { src: "{crate::util::url_base()}{crate::FAVICON}", alt: "" }
                span { "{s.app_name}" }
            }
            NavLink { to: Route::Dashboard {}, icon: "◧", label: s.dashboard }
            div { class: "nav-section", "{s.library}" }
            NavLink { to: Route::SeriesLibrary {}, icon: "▶", label: s.series }
            NavLink { to: Route::AnimeLibrary {}, icon: "✿", label: s.anime }
            NavLink { to: Route::MoviesLibrary {}, icon: "◉", label: s.movies }
            NavLink { to: Route::Collections {}, icon: "▤", label: s.collections }
            NavLink { to: Route::Calendar {}, icon: "▦", label: s.calendar }
            div { class: "nav-section", "{s.activity}" }
            NavLink { to: Route::Queue {}, icon: "⇣", label: s.queue }
            NavLink { to: Route::Activity {}, icon: "↺", label: s.history }
            NavLink { to: Route::WantedMissing {}, icon: "◌", label: s.wanted }
            NavLink { to: Route::InteractiveSearch {}, icon: "⌕", label: s.search }
            NavLink { to: Route::AddMedia {}, icon: "+", label: s.add_media }
            NavLink { to: Route::ImportWizard {}, icon: "⇪", label: s.import_migration }
            div { class: "nav-section", "{s.settings}" }
            NavLink { to: Route::SettingsGeneral {}, icon: "⚙", label: s.general }
            NavLink { to: Route::SettingsMedia {}, icon: "▣", label: s.media_management }
            NavLink { to: Route::SettingsProfiles {}, icon: "★", label: s.profiles }
            NavLink { to: Route::SettingsFormats {}, icon: "≡", label: s.custom_formats }
            NavLink { to: Route::SettingsIndexers {}, icon: "⌗", label: s.indexers }
            NavLink { to: Route::SettingsDownloadClients {}, icon: "⇩", label: s.download_clients }
            NavLink { to: Route::SettingsNotifications {}, icon: "✉", label: s.notifications }
            NavLink { to: Route::SettingsImportLists {}, icon: "☰", label: s.import_lists }
            NavLink { to: Route::SettingsMetadata {}, icon: "ⓘ", label: s.metadata }
            NavLink { to: Route::SettingsTags {}, icon: "#", label: s.tags }
            div { class: "nav-section", "{s.system}" }
            NavLink { to: Route::SystemStatusPage {}, icon: "♥", label: s.status }
            NavLink { to: Route::SystemTasks {}, icon: "⏱", label: s.tasks }
            NavLink { to: Route::SystemBackups {}, icon: "⛁", label: s.backups }
            NavLink { to: Route::SystemLogs {}, icon: "☷", label: s.logs }
        }
    }
}

#[component]
fn TopBar() -> Element {
    let mut app = use_app();
    let client = use_client();
    let route = use_route::<Route>();
    let nav = navigator();
    let username = app
        .auth
        .read()
        .as_ref()
        .and_then(|a| a.username.clone())
        .unwrap_or_default();
    let theme_label = if app.theme.read().as_str() == "light" {
        "☾"
    } else {
        "☀"
    };
    let connected = (app.connected)();
    let logout = move |_| {
        let client = client.clone();
        spawn(async move {
            let _ = client.logout().await;
            // A full reload clears every cached signal and closes the event stream.
            if let Some(w) = web_sys::window() {
                let _ = w
                    .location()
                    .set_href(&format!("{}/login", crate::util::url_base()));
            }
        });
    };
    let on_search_key = move |e: KeyboardEvent| {
        if e.key() == Key::Enter
            && !matches!(
                route,
                Route::SeriesLibrary {} | Route::AnimeLibrary {} | Route::MoviesLibrary {}
            )
        {
            nav.push(Route::SeriesLibrary {});
        }
    };
    rsx! {
        header { class: "topbar",
            button { class: "btn ghost icon menu-btn", "aria-label": "Open navigation", onclick: move |_| app.nav_open.set(true), "☰" }
            input {
                id: "global-search",
                class: "input search grow",
                style: "max-width: 26rem",
                r#type: "search",
                placeholder: t().search_placeholder,
                "aria-label": "Search the library",
                value: "{app.search}",
                oninput: move |e: FormEvent| app.search.set(e.value()),
                onkeydown: on_search_key,
            }
            div { class: "spacer" }
            span {
                class: if connected { "badge success hide-mobile" } else { "badge hide-mobile" },
                title: if connected { "Live updates connected" } else { "Live updates disconnected" },
                if connected { "● live" } else { "○ offline" }
            }
            button { class: "btn ghost icon", "aria-label": "Toggle colour theme", onclick: move |_| app.toggle_theme(), "{theme_label}" }
            span { class: "small muted hide-mobile", "{username}" }
            button { class: "btn sm", onclick: logout, {t().sign_out} }
        }
    }
}

#[component]
pub fn Toasts() -> Element {
    let mut app = use_app();
    let toasts = app.toasts.read().clone();
    rsx! {
        div { class: "toasts", "aria-live": "polite",
            for toast in toasts {
                div { key: "{toast.id}", class: toast.level.class(), role: "status",
                    div { class: "grow", "{toast.message}" }
                    button {
                        class: "btn ghost sm",
                        "aria-label": "Dismiss",
                        onclick: move |_| app.toasts.write().retain(|t| t.id != toast.id),
                        "✕"
                    }
                }
            }
        }
    }
}
