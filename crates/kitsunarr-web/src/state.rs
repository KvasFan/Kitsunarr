//! Application-wide state shared through Dioxus context.

use dioxus::prelude::*;
use kitsunarr_client::{Client, ClientError};
use kitsunarr_domain::api::auth::AuthStatus;

use crate::util;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastLevel {
    pub fn class(self) -> &'static str {
        match self {
            ToastLevel::Info => "toast",
            ToastLevel::Success => "toast success",
            ToastLevel::Warning => "toast warning",
            ToastLevel::Error => "toast error",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Toast {
    pub id: u64,
    pub level: ToastLevel,
    pub message: String,
}

/// Latest progress message of a migration session (from SSE).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct MigrationProgress {
    pub session_id: String,
    pub stage: String,
    pub done: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Clone, Copy)]
pub struct AppState {
    /// `None` until the first status request finished.
    pub auth: Signal<Option<AuthStatus>>,
    pub toasts: Signal<Vec<Toast>>,
    pub theme: Signal<String>,
    pub nav_open: Signal<bool>,
    /// Bumped when the server reports library/settings/health/task changes.
    pub library_rev: Signal<u64>,
    pub settings_rev: Signal<u64>,
    pub health_rev: Signal<u64>,
    pub tasks_rev: Signal<u64>,
    pub migration: Signal<MigrationProgress>,
    pub connected: Signal<bool>,
    /// Library search term shared by the top bar and the library pages.
    pub search: Signal<String>,
    next_toast: Signal<u64>,
}

pub fn provide_app_state() {
    use_context_provider(|| Client::new(util::api_root()));
    let theme = util::load_pref("kitsunarr-theme").unwrap_or_else(|| {
        if util::prefers_light() {
            "light".into()
        } else {
            "dark".into()
        }
    });
    util::set_theme_attribute(&theme);
    use_context_provider(|| AppState {
        auth: Signal::new(None),
        toasts: Signal::new(Vec::new()),
        theme: Signal::new(theme),
        nav_open: Signal::new(false),
        library_rev: Signal::new(0),
        settings_rev: Signal::new(0),
        health_rev: Signal::new(0),
        tasks_rev: Signal::new(0),
        migration: Signal::new(MigrationProgress::default()),
        connected: Signal::new(false),
        search: Signal::new(String::new()),
        next_toast: Signal::new(0),
    });
}

/// Hook: the shared state.
pub fn use_app() -> AppState {
    use_context::<AppState>()
}

/// Hook: the API client.
pub fn use_client() -> Client {
    use_context::<Client>()
}

impl AppState {
    pub fn toast(mut self, level: ToastLevel, message: impl Into<String>) {
        let id = {
            let mut n = self.next_toast.write();
            *n += 1;
            *n
        };
        self.toasts.write().push(Toast {
            id,
            level,
            message: message.into(),
        });
        let mut toasts = self.toasts;
        dioxus::core::spawn_forever(async move {
            gloo_timers::future::TimeoutFuture::new(if level == ToastLevel::Error {
                9000
            } else {
                5000
            })
            .await;
            toasts.write().retain(|t| t.id != id);
        });
    }

    pub fn success(self, message: impl Into<String>) {
        self.toast(ToastLevel::Success, message);
    }

    /// Reports an API error; an expired session sends the user back to the login page.
    pub fn error(mut self, e: &ClientError) {
        if e.is_unauthorized() {
            self.auth.set(Some(AuthStatus {
                setup_required: false,
                authenticated: false,
                username: None,
            }));
        } else {
            self.toast(ToastLevel::Error, e.to_string());
        }
    }

    pub fn toggle_theme(mut self) {
        let next = if self.theme.read().as_str() == "light" {
            "dark"
        } else {
            "light"
        };
        util::set_theme_attribute(next);
        util::save_pref("kitsunarr-theme", next);
        self.theme.set(next.to_string());
    }
}
