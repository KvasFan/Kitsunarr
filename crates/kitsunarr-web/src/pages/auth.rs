//! First-run setup and sign-in.

use dioxus::prelude::*;
use kitsunarr_domain::api::auth::{
    LoginRequest, MIN_PASSWORD_LENGTH, SetupRequest, validate_credentials,
};

use crate::components::shell::Toasts;
use crate::route::Route;
use crate::state::{use_app, use_client};

#[component]
fn Brand() -> Element {
    rsx! {
        div { class: "brand",
            img { src: "{crate::util::url_base()}{crate::FAVICON}", alt: "", width: "36", height: "36" }
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
                if let Some(msg) = error() { div { class: "alert danger", role: "alert", "{msg}" } }
                div { class: "field",
                    label { r#for: "u", "Username" }
                    input { id: "u", class: "input", autocomplete: "username", autofocus: true, required: true, value: "{username}", oninput: move |e| username.set(e.value()) }
                }
                div { class: "field",
                    label { r#for: "p", "Password" }
                    input { id: "p", class: "input", r#type: "password", autocomplete: "current-password", required: true, value: "{password}", oninput: move |e| password.set(e.value()) }
                }
                label { class: "check", style: "margin-bottom: var(--sp-4)",
                    input { r#type: "checkbox", checked: remember(), onchange: move |e: FormEvent| remember.set(e.checked()) }
                    "Keep me signed in"
                }
                button { class: "btn primary", style: "width: 100%", r#type: "submit", disabled: busy(), if busy() { "Signing in…" } else { "Sign in" } }
            }
        }
        Toasts {}
    }
}
