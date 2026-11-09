//! Server-Sent Events → reactive state.
//!
//! The browser's `EventSource` reconnects by itself; its callbacks forward messages into a
//! Dioxus coroutine, which updates signals inside the Dioxus runtime.

use dioxus::prelude::*;
use futures::StreamExt;
use kitsunarr_domain::api::events::ServerEvent;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::state::{AppState, MigrationProgress, ToastLevel};
use crate::util;

enum Msg {
    Event(String),
    Open,
    Error,
}

/// Opens the event stream once per application lifetime.
pub fn use_server_events(app: AppState) {
    let co = use_coroutine(move |mut rx: UnboundedReceiver<Msg>| async move {
        let mut app = app;
        while let Some(msg) = rx.next().await {
            match msg {
                Msg::Open => app.connected.set(true),
                Msg::Error => app.connected.set(false),
                Msg::Event(data) => match serde_json::from_str::<ServerEvent>(&data) {
                    Ok(ServerEvent::LibraryChanged { .. }) => app.library_rev += 1,
                    Ok(ServerEvent::SettingsChanged { .. }) => app.settings_rev += 1,
                    Ok(ServerEvent::HealthChanged) => app.health_rev += 1,
                    Ok(ServerEvent::Task { .. }) => app.tasks_rev += 1,
                    Ok(ServerEvent::Migration {
                        session_id,
                        stage,
                        done,
                        total,
                        message,
                    }) => app.migration.set(MigrationProgress {
                        session_id,
                        stage,
                        done,
                        total,
                        message,
                    }),
                    Ok(ServerEvent::Notice { level, message }) => app.toast(
                        match level.as_str() {
                            "error" => ToastLevel::Error,
                            "warning" => ToastLevel::Warning,
                            "success" => ToastLevel::Success,
                            _ => ToastLevel::Info,
                        },
                        message,
                    ),
                    // `resync` or unknown: refresh everything that is cheap to refetch.
                    Err(_) => {
                        app.library_rev += 1;
                        app.settings_rev += 1;
                        app.health_rev += 1;
                    }
                },
            }
        }
    });

    use_hook(move || {
        let url = format!("{}/api/v1/events", util::api_root());
        let Ok(source) = web_sys::EventSource::new(&url) else {
            return;
        };
        for name in kitsunarr_client::EVENT_NAMES {
            let listener = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
                move |e: web_sys::MessageEvent| {
                    co.send(Msg::Event(e.data().as_string().unwrap_or_default()));
                },
            );
            let _ =
                source.add_event_listener_with_callback(name, listener.as_ref().unchecked_ref());
            listener.forget();
        }
        let on_open = Closure::<dyn FnMut()>::new(move || co.send(Msg::Open));
        source.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();
        let on_error = Closure::<dyn FnMut()>::new(move || co.send(Msg::Error));
        source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();
        // The EventSource lives for the whole session (it is never dropped explicitly).
        std::mem::forget(source);
    });
}
