//! Signing in with an OpenID Connect provider (Tanukarr's OIDC authentication): the
//! authorization-code flow with PKCE, as ASP.NET's `OpenIdConnectHandler` runs it for
//! Tanukarr — `GET /login/sso` sends the browser to the provider, which returns it to
//! `/signin-oidc` (the same redirect URI as Tanukarr's, so a provider set up for Tanukarr keeps
//! working); the code is exchanged for tokens, the ID token's signature and claims are checked,
//! the user-info endpoint is read, and the verified e-mail or subject must be the configured
//! OIDC user. Signing out goes through the provider's end-session endpoint when it has one and
//! comes back through `/signout-callback-oidc` to `/loggedout`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use kitsunarr_config::CookieSecurity;
use kitsunarr_db::auth as users;
use kitsunarr_domain::api::auth::{AuthenticationMethod, SecuritySettings};
use serde::Deserialize;
use serde_json::Value;

use crate::AppState;
use crate::auth::ClientInfo;
use crate::security::{Peer, ReturnUrl, effective, redirect_url};

/// The cookie that ties a sign-in to the browser that started it (ASP.NET's correlation
/// cookie).
const CORRELATION_COOKIE: &str = "kitsunarr_oidc";
/// How long a sign-in at the provider may take.
const PENDING_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_PENDING: usize = 200;
/// How long a provider's configuration and keys are reused.
const DISCOVERY_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
/// How long a failed discovery is remembered (health checks run often).
const FAILED_DISCOVERY_LIFETIME: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// ASP.NET's default clock skew.
const CLOCK_SKEW_SECS: i64 = 5 * 60;

/// A provider's configuration (`/.well-known/openid-configuration`) and keys.
#[derive(Debug, Clone)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: Option<String>,
    pub end_session_endpoint: Option<String>,
    pub jwks_uri: Option<String>,
    pub token_auth_methods: Vec<String>,
    pub jwks: Value,
}

struct Pending {
    verifier: String,
    nonce: String,
    redirect_uri: String,
    return_url: Option<String>,
    correlation_hash: String,
    created: Instant,
}

/// A discovery result for an authority, and when it was read.
type CachedDiscovery = (String, Instant, Result<Arc<Discovery>, String>);

#[derive(Default)]
pub struct OidcState {
    pending: Mutex<HashMap<String, Pending>>,
    discovery: Mutex<Option<CachedDiscovery>>,
}

impl OidcState {
    pub fn forget_discovery(&self) {
        *self.discovery.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

async fn get_json(state: &AppState, url: &str) -> Result<Value, String> {
    let resp = state
        .http
        .get(url)
        .header(header::ACCEPT, "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{url} answered HTTP {}", resp.status().as_u16()));
    }
    resp.json()
        .await
        .map_err(|e| format!("{url} sent an unreadable answer ({e})"))
}

async fn fetch_discovery(state: &AppState, authority: &str) -> Result<Discovery, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        authority.trim().trim_end_matches('/')
    );
    let doc = get_json(state, &url).await?;
    let text = |k: &str| {
        doc[k]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let (Some(authorization_endpoint), Some(token_endpoint)) =
        (text("authorization_endpoint"), text("token_endpoint"))
    else {
        return Err(format!(
            "the OIDC configuration from {url} is missing the authorization or token endpoint"
        ));
    };
    let jwks_uri = text("jwks_uri");
    let jwks = match &jwks_uri {
        Some(uri) => get_json(state, uri).await?,
        None => Value::Null,
    };
    Ok(Discovery {
        issuer: text("issuer").unwrap_or_else(|| authority.trim().to_string()),
        authorization_endpoint,
        token_endpoint,
        userinfo_endpoint: text("userinfo_endpoint"),
        end_session_endpoint: text("end_session_endpoint"),
        jwks_uri,
        token_auth_methods: doc["token_endpoint_auth_methods_supported"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        jwks,
    })
}

/// The provider's configuration, cached (12 hours; failures for a minute). `refresh` reads it
/// again (a changed authority, or keys that do not match).
pub async fn discover(
    state: &AppState,
    authority: &str,
    refresh: bool,
) -> Result<Arc<Discovery>, String> {
    if !refresh
        && let Some((cached_for, at, result)) = state
            .security
            .oidc
            .discovery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        && cached_for == authority
    {
        let ttl = if result.is_ok() {
            DISCOVERY_LIFETIME
        } else {
            FAILED_DISCOVERY_LIFETIME
        };
        if at.elapsed() < ttl {
            return result.clone();
        }
    }
    let result = fetch_discovery(state, authority).await.map(Arc::new);
    if let Err(e) = &result {
        tracing::debug!(error = %e, "unable to retrieve the OIDC configuration");
    }
    *state
        .security
        .oidc
        .discovery
        .lock()
        .unwrap_or_else(|e| e.into_inner()) =
        Some((authority.to_string(), Instant::now(), result.clone()));
    result
}

/// The scheme and host the browser used (through trusted proxies' forwarded headers).
pub fn origin(state: &AppState, headers: &HeaderMap, peer: Option<IpAddr>) -> String {
    let trusted = &state.config.server.trusted_proxies;
    let peer_trusted = peer.is_some_and(|p| trusted.iter().any(|n| n.contains(p)));
    let client = crate::auth::resolve_client(
        headers,
        peer.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        trusted,
    );
    let forwarded_host = peer_trusted
        .then(|| headers.get("x-forwarded-host"))
        .flatten()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let host = forwarded_host
        .or_else(|| headers.get(header::HOST).and_then(|v| v.to_str().ok()))
        .unwrap_or("localhost");
    let https = client.https && peer_trusted;
    format!("{}://{host}", if https { "https" } else { "http" })
}

fn redirect_uri(state: &AppState, origin: &str) -> String {
    format!("{origin}{}/signin-oidc", state.config.server.url_base)
}

fn login_url(state: &AppState, query: &str) -> String {
    format!("{}/login{query}", state.config.server.url_base)
}

fn found(location: String) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn cookie_path(state: &AppState) -> String {
    if state.config.server.url_base.is_empty() {
        "/".into()
    } else {
        state.config.server.url_base.clone()
    }
}

fn secure_cookie(state: &AppState, client: ClientInfo) -> bool {
    match state.config.auth.secure_cookies {
        CookieSecurity::Always => true,
        CookieSecurity::Never => false,
        CookieSecurity::Auto => client.https,
    }
}

/// `GET /login/sso`: sends the browser to the identity provider (`Challenge`).
pub async fn login_sso(
    State(state): State<AppState>,
    client: ClientInfo,
    Peer(peer): Peer,
    headers: HeaderMap,
    jar: CookieJar,
    Query(q): Query<ReturnUrl>,
) -> Response {
    let Ok(settings) = effective(&state).await else {
        return found(login_url(&state, ""));
    };
    if settings.authentication_method != AuthenticationMethod::Oidc {
        return found(login_url(&state, ""));
    }
    if !settings.is_oidc_configured() {
        tracing::error!(
            "OIDC authentication is enabled, but Authority, Client ID, Client Secret, User and Scopes are not all configured"
        );
        return found(login_url(&state, ""));
    }
    let discovery = match discover(&state, &settings.oidc_authority, false).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "OIDC authentication failed");
            return found(login_url(&state, "?ssoFailed=true"));
        }
    };
    let state_param = kitsunarr_auth::generate_session_token();
    let nonce = kitsunarr_auth::generate_session_token();
    let verifier = format!(
        "{}{}",
        kitsunarr_auth::generate_session_token(),
        kitsunarr_auth::generate_session_token()
    );
    let correlation = kitsunarr_auth::generate_session_token();
    let redirect = redirect_uri(&state, &origin(&state, &headers, peer));
    {
        let mut pending = state
            .security
            .oidc
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, p| p.created.elapsed() < PENDING_LIFETIME);
        if pending.len() >= MAX_PENDING
            && let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, p)| p.created)
                .map(|(k, _)| k.clone())
        {
            pending.remove(&oldest);
        }
        pending.insert(
            state_param.clone(),
            Pending {
                verifier: verifier.clone(),
                nonce: nonce.clone(),
                redirect_uri: redirect.clone(),
                return_url: q.return_url,
                correlation_hash: kitsunarr_auth::hash_token(&correlation),
                created: Instant::now(),
            },
        );
    }
    let scopes = settings.oidc_scopes();
    let mut unique: Vec<&str> = Vec::new();
    for s in scopes {
        if !unique.contains(&s) {
            unique.push(s);
        }
    }
    let separator = if discovery.authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    let url = format!(
        "{}{separator}client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&response_mode=query&nonce={}&state={}",
        discovery.authorization_endpoint,
        encode(&settings.oidc_client_id),
        encode(&redirect),
        encode(&unique.join(" ")),
        kitsunarr_auth::oidc::pkce_challenge(&verifier),
        encode(&nonce),
        encode(&state_param),
    );
    let cookie = Cookie::build((CORRELATION_COOKIE, correlation))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure_cookie(&state, client))
        .path(cookie_path(&state))
        .max_age(time::Duration::seconds(PENDING_LIFETIME.as_secs() as i64))
        .build();
    (jar.add(cookie), found(url)).into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
    /// RFC 9207's issuer, when the provider sends it.
    pub iss: Option<String>,
}

/// The token endpoint's answer.
#[derive(Debug, Deserialize)]
struct Tokens {
    id_token: Option<String>,
    access_token: Option<String>,
}

/// Exchanges the code, checks the ID token and the user; the configured OIDC user on success.
async fn complete(
    state: &AppState,
    settings: &SecuritySettings,
    pending: &Pending,
    code: &str,
    iss: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let mut discovery = discover(state, &settings.oidc_authority, false).await?;
    if let Some(iss) = iss
        && iss != discovery.issuer
    {
        return Err(format!("the sign-in came back from another issuer ({iss})"));
    }
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", pending.redirect_uri.as_str()),
        ("code_verifier", pending.verifier.as_str()),
    ];
    let basic_only = !discovery.token_auth_methods.is_empty()
        && !discovery
            .token_auth_methods
            .iter()
            .any(|m| m == "client_secret_post")
        && discovery
            .token_auth_methods
            .iter()
            .any(|m| m == "client_secret_basic");
    let mut request = state
        .http
        .post(&discovery.token_endpoint)
        .header(header::ACCEPT, "application/json")
        .timeout(REQUEST_TIMEOUT);
    if basic_only {
        request = request.basic_auth(
            encode(&settings.oidc_client_id),
            Some(encode(&settings.oidc_client_secret)),
        );
    } else {
        form.push(("client_id", settings.oidc_client_id.as_str()));
        form.push(("client_secret", settings.oidc_client_secret.as_str()));
    }
    let resp = request
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("token endpoint: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "the token endpoint answered HTTP {status}: {}",
            body.chars().take(300).collect::<String>()
        ));
    }
    let tokens: Tokens = resp
        .json()
        .await
        .map_err(|e| format!("the token endpoint sent an unreadable answer ({e})"))?;
    let id_token = tokens
        .id_token
        .ok_or("the token endpoint sent no ID token")?;
    let now = Utc::now().timestamp();
    let expected = kitsunarr_auth::oidc::Expected {
        issuer: &discovery.issuer,
        client_id: &settings.oidc_client_id,
        nonce: &pending.nonce,
        now,
        leeway: CLOCK_SKEW_SECS,
    };
    let claims =
        match kitsunarr_auth::oidc::validate_id_token(&id_token, &discovery.jwks, &expected) {
            Ok(c) => c,
            // The provider may have rotated its keys.
            Err(kitsunarr_auth::oidc::JwtError::UnknownKey) => {
                let fresh = discover(state, &settings.oidc_authority, true).await?;
                let claims = kitsunarr_auth::oidc::validate_id_token(
                    &id_token,
                    &fresh.jwks,
                    &kitsunarr_auth::oidc::Expected {
                        issuer: &fresh.issuer,
                        ..expected
                    },
                )
                .map_err(|e| e.to_string())?;
                discovery = fresh;
                claims
            }
            Err(e) => return Err(e.to_string()),
        };
    // `GetClaimsFromUserInfoEndpoint`.
    let userinfo = match (&discovery.userinfo_endpoint, &tokens.access_token) {
        (Some(endpoint), Some(access)) => {
            let resp = state
                .http
                .get(endpoint)
                .bearer_auth(access)
                .header(header::ACCEPT, "application/json")
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
                .map_err(|e| format!("user-info endpoint: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!(
                    "the user-info endpoint answered HTTP {}",
                    resp.status().as_u16()
                ));
            }
            let body = resp.text().await.map_err(|e| e.to_string())?;
            let info: Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                // A signed user-info answer (`application/jwt`).
                Err(_) => {
                    let jws = kitsunarr_auth::oidc::decode(&body).map_err(|e| e.to_string())?;
                    jws.verify(&discovery.jwks).map_err(|e| e.to_string())?;
                    jws.claims
                }
            };
            if info["sub"].as_str() != claims["sub"].as_str() {
                return Err("the user-info subject differs from the ID token's".into());
            }
            Some(info)
        }
        _ => None,
    };
    let identifiers = kitsunarr_auth::oidc::identifiers(&claims, userinfo.as_ref());
    let configured = settings.oidc_user_identifier.trim();
    if configured.is_empty() {
        return Err("No authorized users configured".into());
    }
    if !identifiers
        .iter()
        .any(|i| i.to_lowercase() == configured.to_lowercase())
    {
        return Err(format!(
            "User '{}' is not authorized to access this application",
            identifiers.first().map_or("unknown", String::as_str)
        ));
    }
    Ok((configured.to_string(), Some(id_token)))
}

/// `GET /signin-oidc`: the provider's redirect after signing in.
pub async fn callback(
    State(state): State<AppState>,
    client: ClientInfo,
    headers: HeaderMap,
    jar: CookieJar,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let failed = |jar: CookieJar| {
        (
            jar.remove(Cookie::build(CORRELATION_COOKIE).path(cookie_path(&state))),
            found(login_url(&state, "?ssoFailed=true")),
        )
            .into_response()
    };
    let pending = q.state.as_deref().and_then(|s| {
        state
            .security
            .oidc
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(s)
    });
    let Some(pending) = pending.filter(|p| p.created.elapsed() < PENDING_LIFETIME) else {
        tracing::error!("OIDC authentication failed: unknown or expired sign-in");
        return failed(jar);
    };
    let correlated = jar.get(CORRELATION_COOKIE).is_some_and(|c| {
        kitsunarr_auth::secrets_equal(
            &kitsunarr_auth::hash_token(c.value()),
            &pending.correlation_hash,
        )
    });
    if !correlated {
        tracing::error!("OIDC authentication failed: the sign-in was started in another browser");
        return failed(jar);
    }
    if let Some(error) = &q.error {
        tracing::error!(
            "OIDC authentication failed: {error} {}",
            q.error_description.as_deref().unwrap_or_default()
        );
        return failed(jar);
    }
    let Some(code) = q.code.as_deref().filter(|c| !c.is_empty()) else {
        tracing::error!("OIDC authentication failed: no authorization code");
        return failed(jar);
    };
    let Ok(settings) = effective(&state).await else {
        return failed(jar);
    };
    if settings.effective_method() != AuthenticationMethod::Oidc {
        return failed(jar);
    }
    let (user, id_token) = match complete(&state, &settings, &pending, code, q.iss.as_deref()).await
    {
        Ok(ok) => ok,
        Err(e) => {
            tracing::error!(target: "auth", "OIDC authentication failed: {e}");
            return failed(jar);
        }
    };
    let token = kitsunarr_auth::generate_session_token();
    let expires = (Utc::now() + chrono::Duration::days(i64::from(state.config.auth.session_days)))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let ua = headers.get("user-agent").and_then(|v| v.to_str().ok());
    let ip = client.ip.to_string();
    if let Err(e) = users::create_sso_session(
        state.db.pool(),
        users::NewSsoSession {
            token_hash: &kitsunarr_auth::hash_token(&token),
            subject: &user,
            id_token: id_token.as_deref(),
            expires_at: &expires,
            user_agent: ua,
            remote_addr: Some(&ip),
        },
    )
    .await
    {
        tracing::error!(error = %e, "could not store the session");
        return failed(jar);
    }
    tracing::info!(target: "auth", "Auth-Success ip {} username '{user}' (OIDC)", client.ip);
    let cookie = crate::auth::session_cookie(&state, client, token, true);
    let jar = jar
        .remove(Cookie::build(CORRELATION_COOKIE).path(cookie_path(&state)))
        .add(cookie);
    let target = redirect_url(&state.config.server.url_base, pending.return_url.as_deref());
    (jar, found(target)).into_response()
}

/// Where signing out continues: the provider's end-session endpoint (with the ID token as
/// hint and `/signout-callback-oidc` to come back to), or `None` to sign out locally only.
pub async fn end_session_url(
    state: &AppState,
    settings: &SecuritySettings,
    id_token: Option<String>,
    origin: &str,
) -> Option<String> {
    let discovery = match discover(state, &settings.oidc_authority, false).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "Unable to sign out of the OIDC provider, signing out locally only");
            return None;
        }
    };
    let Some(endpoint) = &discovery.end_session_endpoint else {
        tracing::debug!(
            "The OIDC provider doesn't advertise an end session endpoint, signing out locally only"
        );
        return None;
    };
    let back = format!(
        "{origin}{}/signout-callback-oidc",
        state.config.server.url_base
    );
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{endpoint}{separator}post_logout_redirect_uri={}&client_id={}",
        encode(&back),
        encode(&settings.oidc_client_id)
    );
    if let Some(hint) = id_token {
        url.push_str(&format!("&id_token_hint={}", encode(&hint)));
    }
    Some(url)
}

/// `GET /signout-callback-oidc`: back from the provider's sign-out.
pub async fn signed_out(State(state): State<AppState>) -> Response {
    found(format!("{}/loggedout", state.config.server.url_base))
}
