//! Settings › General › Security (Tanukarr's): how users sign in, when authentication is
//! required, allowed hosts and the OIDC provider.

use dioxus::prelude::*;
use kitsunarr_domain::api::auth::{AuthenticationMethod, AuthenticationRequired, SecuritySettings};

use crate::components::common::{Field, Loading};
use crate::state::{use_app, use_client};

fn method_value(m: AuthenticationMethod) -> &'static str {
    match m {
        AuthenticationMethod::Forms => "forms",
        AuthenticationMethod::External => "external",
        AuthenticationMethod::Oidc => "oidc",
    }
}

fn required_value(r: AuthenticationRequired) -> &'static str {
    match r {
        AuthenticationRequired::Enabled => "enabled",
        AuthenticationRequired::DisabledForLocalAddresses => "disabledForLocalAddresses",
        AuthenticationRequired::DisabledForLocalhost => "disabledForLocalhost",
    }
}

#[component]
pub fn SecurityCard() -> Element {
    let app = use_app();
    let client = use_client();
    let mut form = use_signal(|| None::<SecuritySettings>);
    let mut saved_method = use_signal(|| None::<AuthenticationMethod>);
    let c0 = client.clone();
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.security().await {
                Ok(s) => {
                    saved_method.set(Some(s.authentication_method));
                    form.set(Some(s));
                }
                Err(e) => app.error(&e),
            }
        }
    });
    let save = move |_| {
        let Some(s) = form() else { return };
        let c = client.clone();
        let to_oidc = s.authentication_method == AuthenticationMethod::Oidc
            && saved_method() != Some(AuthenticationMethod::Oidc);
        spawn(async move {
            match c.put_security(&s).await {
                Ok(saved) => {
                    saved_method.set(Some(saved.authentication_method));
                    form.set(Some(saved));
                    app.success("Security settings saved.");
                    if to_oidc {
                        // Password sessions end with OIDC: sign in through the provider.
                        if let Some(w) = web_sys::window() {
                            let _ = w
                                .location()
                                .set_href(&format!("{}/login", crate::util::url_base()));
                        }
                    }
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let redirect_uri = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .map(|origin| format!("{origin}{}/signin-oidc", crate::util::url_base()))
        .unwrap_or_default();

    rsx! {
        div { class: "card",
            h3 { "Security" }
            match form() {
                None => rsx! { Loading {} },
                Some(s) => {
                    let locked = |name: &str| s.locked.iter().any(|l| l == name);
                    let method = s.authentication_method;
                    rsx! {
                        p { class: "muted small", "To prevent remote access without authentication, Kitsunarr requires authentication. You can optionally disable it for local addresses." }
                        div { class: "form-grid",
                            Field { label: "Authentication", help: "How users sign in: Kitsunarr's sign-in page, or your OpenID Connect identity provider.".to_string(),
                                select { class: "select", "aria-label": "Authentication", disabled: locked("authenticationMethod"),
                                    onchange: move |e: FormEvent| { if let (Some(f), Some(m)) = (form.write().as_mut(), AuthenticationMethod::parse(&e.value())) { f.authentication_method = m; } },
                                    option { value: "{method_value(AuthenticationMethod::Forms)}", selected: method == AuthenticationMethod::Forms, "Forms (sign-in page)" }
                                    option { value: "{method_value(AuthenticationMethod::Oidc)}", selected: method == AuthenticationMethod::Oidc, "OIDC (identity provider)" }
                                    // Tanukarr keeps External out of the list unless it is in use.
                                    if method == AuthenticationMethod::External {
                                        option { value: "{method_value(AuthenticationMethod::External)}", selected: true, "External (reverse proxy)" }
                                    }
                                }
                            }
                            Field { label: "Authentication required", help: "Require authentication for every request, or not for requests from local addresses or this machine.".to_string(),
                                select { class: "select", "aria-label": "Authentication required", disabled: locked("authenticationRequired"),
                                    onchange: move |e: FormEvent| { if let (Some(f), Some(r)) = (form.write().as_mut(), AuthenticationRequired::parse(&e.value())) { f.authentication_required = r; } },
                                    for (r, label) in [
                                        (AuthenticationRequired::Enabled, "Enabled"),
                                        (AuthenticationRequired::DisabledForLocalAddresses, "Disabled for local addresses"),
                                        (AuthenticationRequired::DisabledForLocalhost, "Disabled for localhost"),
                                    ] {
                                        option { key: "{required_value(r)}", value: "{required_value(r)}", selected: s.authentication_required == r, "{label}" }
                                    }
                                }
                            }
                            Field { label: "Allowed hosts", help: "Comma separated list of host names and IP addresses Kitsunarr should be accessible as, including the system's host name or the container name. Leave empty to allow all hosts. Required when authentication is not always required. For example: kitsunarr, kitsunarr.example.com, 192.168.0.100".to_string(),
                                input { class: "input", "aria-label": "Allowed hosts", disabled: locked("allowedHosts"), value: "{s.allowed_hosts}",
                                    oninput: move |e| { if let Some(f) = form.write().as_mut() { f.allowed_hosts = e.value(); } } }
                            }
                        }
                        if s.authentication_required == AuthenticationRequired::DisabledForLocalAddresses {
                            label { class: "check small", style: "margin-bottom: var(--sp-4)",
                                input { r#type: "checkbox", "aria-label": "Trust CGNAT addresses", disabled: locked("trustCgnatIpAddresses"), checked: s.trust_cgnat_ip_addresses,
                                    onchange: move |e: FormEvent| { if let Some(f) = form.write().as_mut() { f.trust_cgnat_ip_addresses = e.checked(); } } }
                                "Treat CGNAT addresses (100.64.0.0/10, e.g. Tailscale) as local"
                            }
                        }
                        if method == AuthenticationMethod::External {
                            div { class: "alert warning", role: "status", "External: Kitsunarr asks for no sign-in and relies on the reverse proxy in front of it. Only use this behind a proxy that authenticates every request." }
                        }
                        if method == AuthenticationMethod::Oidc {
                            h4 { "OpenID Connect" }
                            p { class: "muted small",
                                "Register Kitsunarr with your provider using the redirect URI "
                                code { "{redirect_uri}" }
                                ". Saving signs out password sessions; you then sign in through the provider."
                            }
                            div { class: "form-grid",
                                Field { label: "OIDC Authority", help: "The URL of the OIDC provider (https://).".to_string(),
                                    input { class: "input", "aria-label": "OIDC Authority", placeholder: "https://auth.example.com", disabled: locked("oidcAuthority"), value: "{s.oidc_authority}",
                                        oninput: move |e| { if let Some(f) = form.write().as_mut() { f.oidc_authority = e.value(); } } }
                                }
                                Field { label: "OIDC Client ID", help: "The Client ID registered with the OIDC provider.".to_string(),
                                    input { class: "input", "aria-label": "OIDC Client ID", disabled: locked("oidcClientId"), value: "{s.oidc_client_id}",
                                        oninput: move |e| { if let Some(f) = form.write().as_mut() { f.oidc_client_id = e.value(); } } }
                                }
                                Field { label: "OIDC Client Secret", help: "The Client Secret registered with the OIDC provider.".to_string(),
                                    input { class: "input", r#type: "password", autocomplete: "off", "aria-label": "OIDC Client Secret", disabled: locked("oidcClientSecret"), value: "{s.oidc_client_secret}",
                                        oninput: move |e| { if let Some(f) = form.write().as_mut() { f.oidc_client_secret = e.value(); } } }
                                }
                                Field { label: "OIDC User", help: "The verified e-mail address or subject identifier (sub) on the OIDC provider that may sign in to Kitsunarr.".to_string(),
                                    input { class: "input", "aria-label": "OIDC User", disabled: locked("oidcUserIdentifier"), value: "{s.oidc_user_identifier}",
                                        oninput: move |e| { if let Some(f) = form.write().as_mut() { f.oidc_user_identifier = e.value(); } } }
                                }
                                Field { label: "OIDC Scopes", help: "The scopes to request from the OIDC provider; must include openid.".to_string(),
                                    input { class: "input", "aria-label": "OIDC Scopes", disabled: locked("oidcScopes"), value: "{s.oidc_scopes}",
                                        oninput: move |e| { if let Some(f) = form.write().as_mut() { f.oidc_scopes = e.value(); } } }
                                }
                            }
                        }
                        if !s.locked.is_empty() {
                            p { class: "help small", "Set in config.toml or the environment, which take precedence: {s.locked.join(\", \")}." }
                        }
                        div { class: "row end", button { class: "btn primary", onclick: save, {crate::i18n::t().save} } }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_values_round_trip() {
        for m in [
            AuthenticationMethod::Forms,
            AuthenticationMethod::External,
            AuthenticationMethod::Oidc,
        ] {
            assert_eq!(AuthenticationMethod::parse(method_value(m)), Some(m));
        }
        for r in [
            AuthenticationRequired::Enabled,
            AuthenticationRequired::DisabledForLocalAddresses,
            AuthenticationRequired::DisabledForLocalhost,
        ] {
            assert_eq!(AuthenticationRequired::parse(required_value(r)), Some(r));
        }
    }
}
