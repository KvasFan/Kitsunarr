//! Server-Sent Events → reactive state.
//!
//! Browsers allow only six HTTP/1.1 connections per host, and every tab plus every page kept
//! in the back/forward cache shares them. An event stream left open in each of those would
//! eventually leave no connection for API requests: the UI then stalls with requests queued
//! inside the browser. So the stream is held only while the page is in use:
//!
//! * it closes when the page is hidden for more than [`HIDDEN_GRACE_MS`] or is put into the
//!   back/forward cache (`pagehide`);
//! * it reopens when the page is shown again, followed by a refetch of everything that may
//!   have changed in the meantime;
//! * after a connection error, the browser reconnects by itself and the next `open` also
//!   triggers a refetch, since events may have been missed.
//!
//! The `EventSource` callbacks forward messages into a Dioxus coroutine, which updates
//! signals inside the Dioxus runtime.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use futures::StreamExt;
use gloo_timers::callback::Timeout;
use kitsunarr_domain::api::events::ServerEvent;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::state::{AppState, MigrationProgress, ToastLevel};
use crate::util;

/// A hidden tab keeps its stream this long, so quick tab switches do not reconnect.
const HIDDEN_GRACE_MS: u32 = 10_000;

enum Msg {
    Event(String),
    Open,
    Error,
    /// The stream was closed on purpose (page hidden or cached).
    Closed,
    /// The stream was reopened after being closed on purpose.
    Reopened,
}

/// An open `EventSource` and the callbacks registered on it.
struct Stream {
    source: web_sys::EventSource,
    _listeners: Vec<Closure<dyn FnMut(web_sys::MessageEvent)>>,
    _status: Vec<Closure<dyn FnMut()>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // No event is dispatched after `close()`, so the callbacks can be dropped with it.
        self.source.close();
    }
}

struct Lifecycle {
    co: Coroutine<Msg>,
    stream: RefCell<Option<Stream>>,
    hide_timer: RefCell<Option<Timeout>>,
}

impl Lifecycle {
    /// Opens the stream unless it is open already. Returns whether it was opened.
    fn open(&self) -> bool {
        if self.stream.borrow().is_some() {
            return false;
        }
        let url = format!("{}/api/v1/events", util::api_root());
        let Ok(source) = web_sys::EventSource::new(&url) else {
            return false;
        };
        let co = self.co;
        let listeners = kitsunarr_client::EVENT_NAMES
            .iter()
            .map(|name| {
                let listener = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
                    move |e: web_sys::MessageEvent| {
                        co.send(Msg::Event(e.data().as_string().unwrap_or_default()));
                    },
                );
                let _ = source
                    .add_event_listener_with_callback(name, listener.as_ref().unchecked_ref());
                listener
            })
            .collect();
        let on_open = Closure::<dyn FnMut()>::new(move || co.send(Msg::Open));
        source.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        let on_error = Closure::<dyn FnMut()>::new(move || co.send(Msg::Error));
        source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        *self.stream.borrow_mut() = Some(Stream {
            source,
            _listeners: listeners,
            _status: vec![on_open, on_error],
        });
        true
    }

    fn close(&self) {
        self.hide_timer.borrow_mut().take();
        if self.stream.borrow_mut().take().is_some() {
            self.co.send(Msg::Closed);
        }
    }

    fn page_shown(&self) {
        // Dropping a pending timer cancels it.
        self.hide_timer.borrow_mut().take();
        if self.open() {
            self.co.send(Msg::Reopened);
        }
    }

    fn page_hidden(self: &Rc<Self>) {
        let this = Rc::clone(self);
        *self.hide_timer.borrow_mut() = Some(Timeout::new(HIDDEN_GRACE_MS, move || this.close()));
    }
}

/// Page lifecycle listeners, removed again when the component unmounts.
struct PageListeners {
    lifecycle: Rc<Lifecycle>,
    visibility: Closure<dyn FnMut()>,
    pagehide: Closure<dyn FnMut()>,
    pageshow: Closure<dyn FnMut()>,
}

impl PageListeners {
    fn attach(lifecycle: Rc<Lifecycle>) -> Self {
        let lc = Rc::clone(&lifecycle);
        let visibility = Closure::<dyn FnMut()>::new(move || {
            if page_hidden() {
                lc.page_hidden();
            } else {
                lc.page_shown();
            }
        });
        let lc = Rc::clone(&lifecycle);
        let pagehide = Closure::<dyn FnMut()>::new(move || lc.close());
        let lc = Rc::clone(&lifecycle);
        let pageshow = Closure::<dyn FnMut()>::new(move || {
            if !page_hidden() {
                lc.page_shown();
            }
        });
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc.add_event_listener_with_callback(
                "visibilitychange",
                visibility.as_ref().unchecked_ref(),
            );
        }
        if let Some(w) = web_sys::window() {
            let _ =
                w.add_event_listener_with_callback("pagehide", pagehide.as_ref().unchecked_ref());
            let _ =
                w.add_event_listener_with_callback("pageshow", pageshow.as_ref().unchecked_ref());
        }
        Self {
            lifecycle,
            visibility,
            pagehide,
            pageshow,
        }
    }

    fn detach(&self) {
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc.remove_event_listener_with_callback(
                "visibilitychange",
                self.visibility.as_ref().unchecked_ref(),
            );
        }
        if let Some(w) = web_sys::window() {
            let _ = w.remove_event_listener_with_callback(
                "pagehide",
                self.pagehide.as_ref().unchecked_ref(),
            );
            let _ = w.remove_event_listener_with_callback(
                "pageshow",
                self.pageshow.as_ref().unchecked_ref(),
            );
        }
        // Also breaks the timer → lifecycle reference cycle.
        self.lifecycle.close();
    }
}

fn page_hidden() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_some_and(|d| d.hidden())
}

/// Marks everything that is cheap to refetch as stale.
fn refetch_all(app: &mut AppState) {
    app.library_rev += 1;
    app.settings_rev += 1;
    app.health_rev += 1;
    app.tasks_rev += 1;
}

/// Keeps the event stream open while the application is mounted and the page is in use.
pub fn use_server_events(app: AppState) {
    let co = use_coroutine(move |mut rx: UnboundedReceiver<Msg>| async move {
        let mut app = app;
        let mut missed_events = false;
        while let Some(msg) = rx.next().await {
            match msg {
                Msg::Open => {
                    app.connected.set(true);
                    if std::mem::take(&mut missed_events) {
                        refetch_all(&mut app);
                    }
                }
                Msg::Error => {
                    app.connected.set(false);
                    missed_events = true;
                }
                Msg::Closed => app.connected.set(false),
                Msg::Reopened => refetch_all(&mut app),
                Msg::Event(data) => match serde_json::from_str::<ServerEvent>(&data) {
                    Ok(ServerEvent::LibraryChanged { .. })
                    | Ok(ServerEvent::EpisodeFileChanged { .. }) => app.library_rev += 1,
                    Ok(ServerEvent::QueueChanged) => {}
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
                    // `resync` (the server dropped events for us) or unknown.
                    Err(_) => refetch_all(&mut app),
                },
            }
        }
    });

    let listeners = use_hook(move || {
        let lifecycle = Rc::new(Lifecycle {
            co,
            stream: RefCell::new(None),
            hide_timer: RefCell::new(None),
        });
        if !page_hidden() {
            lifecycle.open();
        }
        Rc::new(PageListeners::attach(lifecycle))
    });
    use_drop(move || listeners.detach());
}
