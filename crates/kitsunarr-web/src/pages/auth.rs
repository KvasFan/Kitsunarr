//! First-run setup and sign-in.

use dioxus::prelude::*;
use kitsunarr_domain::api::auth::{
    AuthStatus, AuthenticationMethod, LoginRequest, MIN_PASSWORD_LENGTH, SetupRequest,
    validate_credentials,
};

use crate::components::shell::Toasts;
use crate::route::Route;
use crate::state::{use_app, use_client};

#[component]
fn Brand() -> Element {
    rsx! {
        div { class: "brand",
            img { src: "{crate::util::url_base()}{crate::LOGO}", alt: "", width: "36", height: "36" }
            span { "Kitsunarr" }
        }
    }
}

#[component]
pub fn Setup() -> Element {
    let mut app = use_app();
    let client = use_client();
    let nav = navigator();
    let mut username = use_signal(|| "admin".to_string());
    let mut password = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);

    let submit = move |e: FormEvent| {
        e.prevent_default();
        if password() != confirm() {
            error.set(Some("The passwords do not match.".into()));
            return;
        }
        if let Err(m) = validate_credentials(&username(), &password()) {
            error.set(Some(m));
            return;
        }
        let client = client.clone();
        busy.set(true);
        spawn(async move {
            match client
                .setup(&SetupRequest {
                    username: username(),
                    password: password(),
                })
                .await
            {
                Ok(status) => {
                    app.auth.set(Some(status));
                    nav.replace(Route::Dashboard {});
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "auth-wrap",
            form { class: "auth-card", onsubmit: submit,
                Brand {}
                h1 { style: "font-size: var(--fs-xl); text-align: center", "Welcome" }
                p { class: "muted", style: "text-align: center", "Create the administrator account. You will use it to sign in to Kitsunarr." }
                if let Some(msg) = error() { div { class: "alert danger", role: "alert", "{msg}" } }
                div { class: "field",
                    label { r#for: "u", "Username" }
                    input { id: "u", class: "input", autocomplete: "username", required: true, value: "{username}", oninput: move |e| username.set(e.value()) }
                }
                div { class: "field",
                    label { r#for: "p", "Password" }
                    input { id: "p", class: "input", r#type: "password", autocomplete: "new-password", required: true, minlength: "{MIN_PASSWORD_LENGTH}", value: "{password}", oninput: move |e| password.set(e.value()) }
                    span { class: "help", "At least {MIN_PASSWORD_LENGTH} characters. A passphrase works well." }
                }
                div { class: "field",
                    label { r#for: "c", "Confirm password" }
                    input { id: "c", class: "input", r#type: "password", autocomplete: "new-password", required: true, value: "{confirm}", oninput: move |e| confirm.set(e.value()) }
                }
                button { class: "btn primary", style: "width: 100%", r#type: "submit", disabled: busy(), if busy() { "Creating…" } else { "Create account" } }
            }
        }
        Toasts {}
    }
}

/// The sign-in page's query (Sonarr's `login.html` parameters, set by the form routes).
fn query_param(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == name)
        .and_then(|(_, v)| js_sys::decode_uri_component(&v.replace('+', " ")).ok())
        .map(String::from)
}

fn flag(name: &str) -> bool {
    query_param(name).as_deref() == Some("true")
}

/// A local return URL for after signing in (`returnUrl`).
fn return_url() -> Option<String> {
    query_param("returnUrl")
        .filter(|r| r.starts_with('/') && !r.starts_with("//") && !r.starts_with("/\\"))
}

#[component]
pub fn Login() -> Element {
    let mut app = use_app();
    let client = use_client();
    let nav = navigator();
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut remember = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut status = use_signal(|| None::<AuthStatus>);

    // Redirect to setup when no account exists yet.
    let status_client = client.clone();
    use_future(move || {
        let client = status_client.clone();
        async move {
            if let Ok(s) = client.auth_status().await {
                if s.setup_required {
                    nav.replace(Route::Setup {});
                } else if s.authenticated {
                    app.auth.set(Some(s));
                    nav.replace(Route::Dashboard {});
                } else {
                    status.set(Some(s));
                }
            }
        }
    });

    let submit = move |e: FormEvent| {
        e.prevent_default();
        let client = client.clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            let req = LoginRequest {
                username: username(),
                password: password(),
                remember_me: remember(),
            };
            match client.login(&req).await {
                Ok(status) => {
                    app.auth.set(Some(status));
                    match return_url() {
                        // A full load of the page that asked for the sign-in.
                        Some(url) => {
                            if let Some(w) = web_sys::window() {
                                let _ = w.location().set_href(&url);
                            }
                        }
                        None => {
                            nav.replace(Route::Dashboard {});
                        }
                    }
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    let base = crate::util::url_base();
    let s = status();
    let oidc = s.as_ref().is_some_and(|s| {
        s.authentication_method == AuthenticationMethod::Oidc && !s.oidc_misconfigured
    });
    let misconfigured = s.as_ref().is_some_and(|s| s.oidc_misconfigured);
    let sso_href = match return_url() {
        Some(r) => format!(
            "{base}/login/sso?returnUrl={}",
            String::from(js_sys::encode_uri_component(&r))
        ),
        None => format!("{base}/login/sso"),
    };
    let lockout = query_param("lockoutUntil")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|until| *until > js_sys::Date::now())
        .map(|until| {
            let minutes = ((until - js_sys::Date::now()) / 60_000.0).ceil().max(1.0);
            format!("Too many failed sign in attempts. Please try again in {minutes} minute(s).")
        });

    if flag("resetPassword") {
        return rsx! {
            div { class: "auth-wrap",
                if flag("tokenSent") {
                    ResetPasswordPanel { base: base.clone() }
                } else {
                    div { class: "auth-card",
                        Brand {}
                        h1 { style: "font-size: var(--fs-xl); text-align: center", "Reset your password" }
                        p { class: "muted",
                            "Kitsunarr will write a single-use reset token to its log. Copy it from the "
                            code { "Auth-ResetRequested" }
                            " line in the latest log (System › Logs, or the container's output)."
                        }
                        form { method: "post", action: "{base}/reset-password/request",
                            button { class: "btn primary", style: "width: 100%", r#type: "submit", "Write a reset token to the log" }
                        }
                        div { class: "row", style: "justify-content: space-between; margin-top: var(--sp-4)",
                            a { href: "{base}/login?resetPassword=true&tokenSent=true", "I already have a reset token" }
                            a { href: "{base}/login", "Back to sign in" }
                        }
                    }
                }
            }
            Toasts {}
        };
    }

    rsx! {
        div { class: "auth-wrap",
            form { class: "auth-card", onsubmit: submit,
                Brand {}
                if flag("passwordReset") { div { class: "alert success", role: "status", "Your password has been reset, sign in with your new password." } }
                if flag("ssoFailed") { div { class: "alert danger", role: "alert", "Unable to sign in with your identity provider." } }
                if let Some(msg) = error() { div { class: "alert danger", role: "alert", "{msg}" } }
                if let Some(msg) = lockout.clone() {
                    div { class: "alert danger", role: "alert", "{msg}" }
                } else if flag("loginFailed") {
                    div { class: "alert danger", role: "alert", "Incorrect username or password." }
                }
                if misconfigured {
                    div { class: "alert warning", role: "alert", "OIDC authentication is enabled, but isn't configured correctly. Sign in with your password and update the OIDC settings (Settings › General › Security, or the configuration file)." }
                }
                if oidc {
                    a { class: "btn primary", style: "width: 100%; justify-content: center", href: "{sso_href}", "Sign in with SSO" }
                    p { class: "muted small", style: "text-align: center; margin-top: var(--sp-3)", "You will be sent to your identity provider." }
                } else {
                    div { class: "field",
                        label { r#for: "u", "Username" }
                        input { id: "u", class: "input", autocomplete: "username", autofocus: true, required: true, value: "{username}", oninput: move |e| username.set(e.value()) }
                    }
                    div { class: "field",
                        label { r#for: "p", "Password" }
                        input { id: "p", class: "input", r#type: "password", autocomplete: "current-password", required: true, value: "{password}", oninput: move |e| password.set(e.value()) }
                    }
                    div { class: "row", style: "justify-content: space-between; margin-bottom: var(--sp-4)",
                        label { class: "check",
                            input { r#type: "checkbox", checked: remember(), onchange: move |e: FormEvent| remember.set(e.checked()) }
                            "Keep me signed in"
                        }
                        a { class: "small", href: "{base}/login?resetPassword=true", "Forgot your password?" }
                    }
                    button { class: "btn primary", style: "width: 100%", r#type: "submit", disabled: busy() || lockout.is_some(), if busy() { "Signing in…" } else { "Sign in" } }
                }
            }
        }
        Toasts {}
    }
}

/// The reset form: the token from the log (or the configured one) and the new credentials,
/// posted to Sonarr's `/reset-password`.
#[component]
fn ResetPasswordPanel(base: String) -> Element {
    rsx! {
        form { class: "auth-card", method: "post", action: "{base}/reset-password",
            Brand {}
            h1 { style: "font-size: var(--fs-xl); text-align: center", "Reset your password" }
            if flag("resetFailed") { div { class: "alert danger", role: "alert", "Invalid or expired reset token." } }
            if flag("resetInvalid") {
                div { class: "alert danger", role: "alert", "Username and password are required, the passwords must match, and the password needs at least {MIN_PASSWORD_LENGTH} characters." }
            }
            if flag("tokenUnavailable") {
                div { class: "alert warning", role: "status", "A new reset token wasn't written to the log. One may already be active, so check the log first. Restarting Kitsunarr clears it and lets you request another." }
            }
            p { class: "muted",
                "Enter the token from the "
                code { "Auth-ResetRequested" }
                " line in the log. It's valid for 15 minutes and can only be used once. If you set "
                code { "KITSUNARR_AUTH_RESET_TOKEN" }
                " (or reset_token in config.toml), enter its value instead."
            }
            div { class: "field",
                label { r#for: "rt", "Reset token" }
                input { id: "rt", class: "input mono", name: "token", autocomplete: "off", autofocus: true, required: true }
            }
            div { class: "field",
                label { r#for: "ru", "Username" }
                input { id: "ru", class: "input", name: "username", autocomplete: "username", required: true }
            }
            div { class: "field",
                label { r#for: "rp", "New password" }
                input { id: "rp", class: "input", r#type: "password", name: "password", autocomplete: "new-password", required: true, minlength: "{MIN_PASSWORD_LENGTH}" }
            }
            div { class: "field",
                label { r#for: "rc", "Confirm new password" }
                input { id: "rc", class: "input", r#type: "password", name: "passwordConfirmation", autocomplete: "new-password", required: true, minlength: "{MIN_PASSWORD_LENGTH}" }
            }
            button { class: "btn primary", style: "width: 100%", r#type: "submit", "Reset password" }
            div { style: "margin-top: var(--sp-4); text-align: center",
                a { href: "{base}/login", "Back to sign in" }
            }
        }
    }
}

/// Sonarr's `/loggedout` page, after signing out of the identity provider.
#[component]
pub fn LoggedOut() -> Element {
    let base = crate::util::url_base();
    rsx! {
        div { class: "auth-wrap",
            div { class: "auth-card",
                Brand {}
                h1 { style: "font-size: var(--fs-xl); text-align: center", "Signed out" }
                p { class: "muted", style: "text-align: center", "You have been signed out of Kitsunarr." }
                a { class: "btn primary", style: "width: 100%; justify-content: center", href: "{base}/login", "Sign in again" }
            }
        }
    }
}
