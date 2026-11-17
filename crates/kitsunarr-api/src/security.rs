//! Settings › General › Security, as Tanukarr has it: the authentication method (forms,
//! external, OIDC), when authentication is required (always, or not for local addresses or
//! localhost), allowed hosts, password resets through a token written to the log or
//! configured, the initial administrator from the configuration, and Sonarr's root form routes
//! (`/login`, `/login/setup`, `/logout`, `/reset-password/request`, `/reset-password`).
//!
//! The settings are stored in the database and edited in the web interface; `config.toml`'s
//! `[auth]` (and `KITSUNARR_AUTH_*`) can pin any of them, as Tanukarr's `SONARR__AUTH__*`
//! options do. Every request reads the effective settings from a cache that saving refreshes.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use axum_extra::extract::cookie::CookieJar;
use kitsunarr_config::{AuthMethod, AuthRequired, Config, IpNetwork};
use kitsunarr_db::auth as users;
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::SESSION_COOKIE;
use kitsunarr_domain::api::auth::{
    AuthenticationMethod, AuthenticationRequired, ResetPasswordForm, SecuritySettings,
    is_bind_address, is_valid_host, parse_allowed_hosts, validate_credentials,
};
use kitsunarr_domain::api::error::FieldError;
use kitsunarr_domain::api::settings::REDACTED;
use kitsunarr_domain::api::system::{HealthCheck, HealthLevel};
use serde::Deserialize;

use crate::AppState;
use crate::auth::{ClientInfo, resolve_client};
use crate::error::{ApiError, ApiResult};

/// Tanukarr's `ResetTokenLifetime`.
const RESET_TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// Tanukarr's `ResetRequestThrottle` (per client address).
const RESET_REQUEST_THROTTLE: Duration = Duration::from_secs(60);
/// Tanukarr's `MinimumConfiguredResetTokenLength`.
const MIN_CONFIGURED_RESET_TOKEN: usize = 20;
/// Hosts always accepted once allowed hosts are configured (`ConfigureHostFilteringOptions`).
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// Authentication state shared by every request.
pub struct SecurityState {
    effective: RwLock<Option<Arc<SecuritySettings>>>,
    reset: Mutex<ResetState>,
    /// This machine's names, accepted as hosts (`GetServerHostNames`).
    hostnames: Vec<String>,
    pub(crate) oidc: crate::oidc::OidcState,
}

#[derive(Default)]
struct ResetState {
    /// The hash of the token written to the log, and when it was issued.
    issued: Option<(String, Instant)>,
    /// Reset requests per client address (throttle).
    requests: HashMap<IpAddr, Instant>,
    /// The hash of the configured token, until it is used once.
    configured: Option<String>,
}

impl SecurityState {
    pub fn new(config: &Config) -> Self {
        let configured = config
            .auth
            .reset_token
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .and_then(|t| {
                if t.chars().count() < MIN_CONFIGURED_RESET_TOKEN {
                    tracing::error!(
                        "Configured password reset token is shorter than {MIN_CONFIGURED_RESET_TOKEN} characters and will be ignored"
                    );
                    None
                } else {
                    Some(kitsunarr_auth::hash_reset_token(t))
                }
            });
        Self {
            effective: RwLock::new(None),
            reset: Mutex::new(ResetState {
                configured,
                ..Default::default()
            }),
            hostnames: server_hostnames(),
            oidc: Default::default(),
        }
    }
}

fn server_hostnames() -> Vec<String> {
    let mut out = Vec::new();
    for name in [
        std::fs::read_to_string("/proc/sys/kernel/hostname").ok(),
        std::fs::read_to_string("/etc/hostname").ok(),
        std::env::var("HOSTNAME").ok(),
        std::env::var("COMPUTERNAME").ok(),
    ]
    .into_iter()
    .flatten()
    {
        let name = name.trim().to_ascii_lowercase();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

// ---- effective settings ---------------------------------------------------------------------

/// The configuration's overrides, applied to the stored settings; `locked` lists them.
pub fn apply_overrides(config: &Config, s: &mut SecuritySettings) {
    let a = &config.auth;
    let mut locked = Vec::new();
    if let Some(m) = a.method {
        s.authentication_method = match m {
            AuthMethod::Forms => AuthenticationMethod::Forms,
            AuthMethod::External => AuthenticationMethod::External,
            AuthMethod::Oidc => AuthenticationMethod::Oidc,
        };
        locked.push("authenticationMethod");
    }
    if let Some(r) = a.required {
        s.authentication_required = match r {
            AuthRequired::Enabled => AuthenticationRequired::Enabled,
            AuthRequired::DisabledForLocalAddresses => {
                AuthenticationRequired::DisabledForLocalAddresses
            }
            AuthRequired::DisabledForLocalhost => AuthenticationRequired::DisabledForLocalhost,
        };
        locked.push("authenticationRequired");
    }
    if let Some(t) = a.trust_cgnat_ip_addresses {
        s.trust_cgnat_ip_addresses = t;
        locked.push("trustCgnatIpAddresses");
    }
    for (value, slot, name) in [
        (&a.oidc_authority, &mut s.oidc_authority, "oidcAuthority"),
        (&a.oidc_client_id, &mut s.oidc_client_id, "oidcClientId"),
        (
            &a.oidc_client_secret,
            &mut s.oidc_client_secret,
            "oidcClientSecret",
        ),
        (
            &a.oidc_user_identifier,
            &mut s.oidc_user_identifier,
            "oidcUserIdentifier",
        ),
        (&a.oidc_scopes, &mut s.oidc_scopes, "oidcScopes"),
        (
            &config.server.allowed_hosts,
            &mut s.allowed_hosts,
            "allowedHosts",
        ),
    ] {
        if let Some(v) = value {
            *slot = v.trim().to_string();
            locked.push(name);
        }
    }
    s.locked = locked.into_iter().map(str::to_string).collect();
}

/// The settings in force (stored, then the configuration's overrides).
pub async fn effective(state: &AppState) -> ApiResult<Arc<SecuritySettings>> {
    if let Some(s) = state
        .security
        .effective
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Ok(s);
    }
    let mut stored: SecuritySettings =
        kv::get_or_default(state.db.pool(), kv::KEY_SECURITY).await?;
    apply_overrides(&state.config, &mut stored);
    let s = Arc::new(stored);
    *state
        .security
        .effective
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(s.clone());
    Ok(s)
}

fn redacted(s: &SecuritySettings) -> SecuritySettings {
    let mut out = s.clone();
    if !out.oidc_client_secret.is_empty() {
        out.oidc_client_secret = REDACTED.into();
    }
    out
}

/// Tanukarr's host-settings validation of the security fields. Problems with values the
/// configuration pins are not the user's to fix and are skipped.
async fn validate(
    state: &AppState,
    next: &SecuritySettings,
    before: &SecuritySettings,
) -> ApiResult<()> {
    let mut problems: Vec<(&str, String)> = Vec::new();
    let hosts = parse_allowed_hosts(&next.allowed_hosts);
    if !next.allowed_hosts.trim().is_empty() {
        if hosts.is_empty() || !hosts.iter().all(|h| is_valid_host(h)) {
            problems.push((
                "allowedHosts",
                "Must be a comma separated list of host names or IP addresses only, without a protocol or port".into(),
            ));
        } else if hosts.iter().any(|h| is_bind_address(h)) {
            problems.push((
                "allowedHosts",
                "0.0.0.0 and [::] are not host names, leave empty to allow all hosts".into(),
            ));
        }
    }
    if next.authentication_required != AuthenticationRequired::Enabled && hosts.is_empty() {
        problems.push((
            "allowedHosts",
            "Allowed Hosts is required when 'Authentication Required' is not 'Enabled'".into(),
        ));
    }
    if next.authentication_method == AuthenticationMethod::Oidc {
        let empty = |v: &str| v.trim().is_empty();
        if empty(&next.oidc_authority) {
            problems.push((
                "oidcAuthority",
                "'OIDC Authority' must not be empty.".into(),
            ));
        } else if !next.is_oidc_authority_secure() {
            problems.push((
                "oidcAuthority",
                "OIDC Authority must start with 'https://'".into(),
            ));
        } else if next.oidc_authority != before.oidc_authority
            && crate::oidc::discover(state, &next.oidc_authority, true)
                .await
                .is_err()
        {
            problems.push((
                "oidcAuthority",
                "Unable to retrieve the OIDC configuration from the Authority".into(),
            ));
        }
        if empty(&next.oidc_client_id) {
            problems.push(("oidcClientId", "'OIDC Client ID' must not be empty.".into()));
        }
        if empty(&next.oidc_client_secret) {
            problems.push((
                "oidcClientSecret",
                "'OIDC Client Secret' must not be empty.".into(),
            ));
        }
        if empty(&next.oidc_user_identifier) {
            problems.push((
                "oidcUserIdentifier",
                "'OIDC User' must not be empty.".into(),
            ));
        }
        if empty(&next.oidc_scopes) {
            problems.push(("oidcScopes", "'OIDC Scopes' must not be empty.".into()));
        } else if !next.oidc_scopes().contains(&"openid") {
            problems.push(("oidcScopes", "OIDC Scopes must include 'openid'".into()));
        }
    }
    let fields: Vec<FieldError> = problems
        .into_iter()
        .filter(|(f, _)| !next.locked.iter().any(|l| l == f))
        .map(|(field, message)| FieldError {
            field: field.into(),
            message,
        })
        .collect();
    if fields.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Validation(fields))
    }
}

/// Stores new security settings (the client secret placeholder keeps the stored secret;
/// values the configuration pins are kept as stored) and applies them at once.
pub async fn save(state: &AppState, mut body: SecuritySettings) -> ApiResult<SecuritySettings> {
    let stored: SecuritySettings = kv::get_or_default(state.db.pool(), kv::KEY_SECURITY).await?;
    if body.oidc_client_secret == REDACTED {
        body.oidc_client_secret = stored.oidc_client_secret.clone();
    }
    body.allowed_hosts = body.allowed_hosts.trim().to_string();
    body.oidc_authority = body.oidc_authority.trim().to_string();
    body.oidc_client_id = body.oidc_client_id.trim().to_string();
    body.oidc_user_identifier = body.oidc_user_identifier.trim().to_string();
    body.oidc_scopes = body.oidc_scopes.trim().to_string();
    body.locked = Vec::new();
    let before = effective(state).await?;
    let mut next = body.clone();
    apply_overrides(&state.config, &mut next);
    validate(state, &next, &before).await?;
    kv::set(state.db.pool(), kv::KEY_SECURITY, &body).await?;
    if next.oidc_authority != before.oidc_authority {
        state.security.oidc.forget_discovery();
    }
    let next = Arc::new(next);
    *state
        .security
        .effective
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(next.clone());
    tracing::info!(
        method = ?next.authentication_method,
        required = ?next.authentication_required,
        "security settings saved"
    );
    crate::routes::settings::settings_changed(state, "security");
    Ok(redacted(&next))
}

/// Settings › General › Security. The OIDC client secret is redacted.
#[utoipa::path(get, path = "/settings/security", tag = "settings",
    responses((status = 200, body = SecuritySettings)))]
pub async fn get_security(State(state): State<AppState>) -> ApiResult<Json<SecuritySettings>> {
    Ok(Json(redacted(&*effective(&state).await?)))
}

/// Saves Settings › General › Security; it applies to the next request.
#[utoipa::path(put, path = "/settings/security", tag = "settings", request_body = SecuritySettings,
    responses((status = 200, body = SecuritySettings), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn put_security(
    State(state): State<AppState>,
    Json(body): Json<SecuritySettings>,
) -> ApiResult<Json<SecuritySettings>> {
    Ok(Json(save(&state, body).await?))
}

// ---- who needs to sign in -------------------------------------------------------------------

/// The direct peer's address, when the server knows it.
#[derive(Debug, Clone, Copy)]
pub struct Peer(pub Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Peer(peer_of(&parts.extensions)))
    }
}

pub fn peer_of(extensions: &axum::http::Extensions) -> Option<IpAddr> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// `IPAddressExtensions.IsLocalAddress`: loopback, private and link-local ranges.
pub fn is_local_address(ip: IpAddr) -> bool {
    let ip = canonical(ip);
    if ip.is_loopback() {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            (o[0] == 169 && o[1] == 254)
                || o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            // Link-local fe80::/10, unique local fc00::/7, site-local fec0::/10.
            (first & 0xffc0) == 0xfe80 || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfec0
        }
    }
}

/// `IsCgnatIpAddress`: 100.64.0.0/10.
pub fn is_cgnat_address(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (64..=127).contains(&o[1])
        }
        IpAddr::V6(_) => false,
    }
}

/// `IsClientAddressKnown`: no forwarded address is left unexplained. Without
/// `X-Forwarded-For` the peer is the client; behind trusted proxies every hop up to the
/// client must be accounted for.
pub fn client_address_known(headers: &HeaderMap, peer: IpAddr, trusted: &[IpNetwork]) -> bool {
    let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let is_trusted = |ip: IpAddr| trusted.iter().any(|n| n.contains(ip));
    if !is_trusted(peer) {
        return false;
    }
    let hops: Vec<&str> = xff.split(',').map(str::trim).collect();
    for (i, hop) in hops.iter().enumerate().rev() {
        match hop.parse::<IpAddr>() {
            Ok(addr) if is_trusted(addr) => continue,
            Ok(_) => return i == 0,
            Err(_) => return false,
        }
    }
    true
}

/// `UiAuthorizationHandler`: whether this request needs no sign-in because authentication is
/// disabled for local addresses or localhost. A client hidden behind an untrusted proxy is
/// never exempt.
pub fn exempt(
    settings: &SecuritySettings,
    headers: &HeaderMap,
    peer: Option<IpAddr>,
    trusted: &[IpNetwork],
) -> bool {
    if settings.authentication_required == AuthenticationRequired::Enabled {
        return false;
    }
    let Some(peer) = peer else {
        return false;
    };
    if !client_address_known(headers, peer, trusted) {
        return false;
    }
    let ip = canonical(resolve_client(headers, peer, trusted).ip);
    match settings.authentication_required {
        AuthenticationRequired::Enabled => false,
        AuthenticationRequired::DisabledForLocalAddresses => {
            is_local_address(ip) || (settings.trust_cgnat_ip_addresses && is_cgnat_address(ip))
        }
        AuthenticationRequired::DisabledForLocalhost => ip.is_loopback(),
    }
}

/// `IsInitialSetupRequired`: forms sign-in, no account and no configured initial password.
pub async fn setup_required(state: &AppState) -> ApiResult<bool> {
    let settings = effective(state).await?;
    Ok(
        settings.authentication_method == AuthenticationMethod::Forms
            && state
                .config
                .auth
                .initial_password
                .as_deref()
                .is_none_or(|p| p.trim().is_empty())
            && users::user_count(state.db.pool()).await? == 0,
    )
}

// ---- allowed hosts --------------------------------------------------------------------------

/// The host of a `Host` header without its port, lower case (`[::1]` keeps its brackets).
fn host_without_port(host: &str) -> String {
    let host = host.trim();
    let bare = if host.starts_with('[') {
        host.find(']').map_or(host, |i| &host[..=i])
    } else if host.matches(':').count() == 1 {
        host.split(':').next().unwrap_or_default()
    } else {
        host
    };
    bare.to_ascii_lowercase()
}

/// Whether ASP.NET's host filtering would accept a host for these allowed hosts.
pub fn host_allowed(allowed: &[String], hostnames: &[String], host: &str) -> bool {
    let host = host_without_port(host);
    if host.is_empty() {
        return true;
    }
    let configured: Vec<&String> = allowed.iter().filter(|h| !is_bind_address(h)).collect();
    if configured.is_empty() {
        return true;
    }
    configured
        .iter()
        .map(|h| h.as_str())
        .chain(LOOPBACK_HOSTS)
        .chain(hostnames.iter().map(String::as_str))
        .any(|entry| {
            if entry == "*" {
                true
            } else if let Some(suffix) = entry.strip_prefix('*') {
                host.len() > suffix.len() && host.ends_with(&suffix.to_ascii_lowercase())
            } else {
                entry.eq_ignore_ascii_case(&host)
            }
        })
}

/// Allowed hosts (`HostFilteringMiddleware`): a request for another host name is refused
/// with 400, which stops DNS rebinding from reaching the API through a victim's browser.
pub async fn host_filter(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Ok(settings) = effective(&state).await else {
        return next.run(req).await;
    };
    if settings.allowed_hosts.trim().is_empty() {
        return next.run(req).await;
    }
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()))
        .unwrap_or_default();
    let allowed = parse_allowed_hosts(&settings.allowed_hosts);
    if host_allowed(&allowed, &state.security.hostnames, &host) {
        return next.run(req).await;
    }
    tracing::debug!(host = %host, "request for a host that is not allowed");
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01//EN\"\"http://www.w3.org/TR/html4/strict.dtd\"><HTML><HEAD><TITLE>Bad Request</TITLE>\r\n<META HTTP-EQUIV=\"Content-Type\" Content=\"text/html; charset=us-ascii\"></ HEAD >\r\n<BODY><h2>Bad Request - Invalid Hostname</h2>\r\n<hr><p>HTTP Error 400. The request hostname is invalid.</p>\r\n</BODY></HTML>",
    )
        .into_response()
}

// ---- password resets ------------------------------------------------------------------------

/// `RequestPasswordReset`: writes a new 20-character token to the log (valid 15 minutes,
/// one at a time, one request per address and minute). `None` when no token was issued.
pub async fn request_reset(state: &AppState, ip: IpAddr) -> ApiResult<Option<String>> {
    if effective(state).await?.effective_method() != AuthenticationMethod::Forms {
        return Ok(None);
    }
    let mut reset = state
        .security
        .reset
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    reset
        .requests
        .retain(|_, at| now.duration_since(*at) < RESET_REQUEST_THROTTLE);
    if reset.requests.contains_key(&ip) {
        tracing::debug!(target: "auth", "Auth-ResetThrottled ip {ip}");
        return Ok(None);
    }
    reset.requests.insert(ip, now);
    if reset
        .issued
        .as_ref()
        .is_some_and(|(_, at)| now.duration_since(*at) < RESET_TOKEN_LIFETIME)
    {
        tracing::info!(
            target: "auth",
            "Auth-ResetTokenActive ip {ip} a password reset token has already been issued, restart Kitsunarr to issue another"
        );
        return Ok(None);
    }
    let token = kitsunarr_auth::generate_reset_token();
    reset.issued = Some((kitsunarr_auth::hash_reset_token(&token), now));
    tracing::warn!(
        target: "auth",
        "Auth-ResetRequested ip {ip} token: {token} valid for {} minutes",
        RESET_TOKEN_LIFETIME.as_secs() / 60
    );
    Ok(Some(token))
}

fn token_matches(expected_hash: &str, token: &str) -> bool {
    !token.trim().is_empty()
        && kitsunarr_auth::secrets_equal(expected_hash, &kitsunarr_auth::hash_reset_token(token))
}

/// `ResetPassword`: with the token from the log or the configured one, the administrator
/// gets the given name and password (created when there is none). Failures count towards
/// the sign-in lockout; every session of the account is ended.
pub async fn reset_password(
    state: &AppState,
    ip: IpAddr,
    token: &str,
    username: &str,
    password: &str,
) -> ApiResult<bool> {
    if effective(state).await?.effective_method() != AuthenticationMethod::Forms {
        return Ok(false);
    }
    let window = Duration::from_secs(state.config.auth.login_window_secs);
    if state
        .login_limiter
        .blocked(ip, state.config.auth.max_failed_logins, window)
        .is_some()
    {
        tracing::warn!(target: "auth", "Auth-Lockout ip {ip} username '{username}'");
        return Ok(false);
    }
    if username.trim().is_empty() {
        state.login_limiter.record_failure(ip, window);
        tracing::warn!(target: "auth", "Auth-Failure ip {ip} username '{username}'");
        return Ok(false);
    }
    let (log_token, configured_token) = {
        let mut reset = state
            .security
            .reset
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if reset
            .issued
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= RESET_TOKEN_LIFETIME)
        {
            reset.issued = None;
        }
        let log_token = reset
            .issued
            .as_ref()
            .is_some_and(|(hash, _)| token_matches(hash, token));
        let configured_token = reset
            .configured
            .as_ref()
            .is_some_and(|hash| token_matches(hash, token));
        if log_token {
            reset.issued = None;
        }
        if configured_token {
            reset.configured = None;
            tracing::info!(
                target: "auth",
                "Auth-ResetTokenDisabled ip {ip} the configured password reset token has been used and is now disabled, remove the KITSUNARR_AUTH_RESET_TOKEN environment variable (or reset_token in config.toml)"
            );
        }
        (log_token, configured_token)
    };
    if !log_token && !configured_token {
        state.login_limiter.record_failure(ip, window);
        tracing::warn!(target: "auth", "Auth-Failure ip {ip} username '{username}'");
        return Ok(false);
    }
    let hash = crate::auth::hash_blocking(password.to_string()).await?;
    let user_id = users::upsert_admin(&state.db, username, &hash).await?;
    let revoked = users::delete_user_sessions(state.db.pool(), user_id, None).await?;
    state.login_limiter.clear(ip);
    tracing::warn!(target: "auth", revoked, "Auth-PasswordReset ip {ip} username '{}'", username.trim());
    Ok(true)
}

// ---- Sonarr's root form routes --------------------------------------------------------------

/// Sonarr's `AuthenticationController` routes, below the URL base (public).
pub fn form_routes() -> Router<AppState> {
    Router::new()
        .route("/login", get(crate::assets::serve).post(form_login))
        .route("/login/setup", post(form_setup))
        .route("/login/sso", get(crate::oidc::login_sso))
        .route("/signin-oidc", get(crate::oidc::callback))
        .route("/signout-callback-oidc", get(crate::oidc::signed_out))
        .route("/logout", get(logout))
        .route("/reset-password/request", post(form_request_reset))
        .route("/reset-password", post(form_reset_password))
}

fn base(state: &AppState) -> &str {
    &state.config.server.url_base
}

fn found(location: String) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// `Url.IsLocalUrl`: a path on this server (`/…`, not `//…` or `/\…`).
fn is_local_url(url: &str) -> bool {
    (url.starts_with('/') && !url.starts_with("//") && !url.starts_with("/\\"))
        && !url.chars().any(|c| c.is_control())
}

/// `GetRedirectUrl`: a local return URL below the URL base, else the home page.
pub fn redirect_url(url_base: &str, return_url: Option<&str>) -> String {
    let Some(r) = return_url.filter(|r| !r.trim().is_empty() && is_local_url(r)) else {
        return format!("{url_base}/");
    };
    if url_base.is_empty()
        || r.eq_ignore_ascii_case(url_base)
        || r.to_ascii_lowercase()
            .starts_with(&format!("{}/", url_base.to_ascii_lowercase()))
    {
        r.to_string()
    } else {
        format!("{url_base}{r}")
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReturnUrl {
    pub return_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LoginForm {
    pub username: String,
    pub password: String,
    /// `on` when "Remember Me" is ticked.
    pub remember_me: Option<String>,
}

/// `POST /login`: Sonarr's form sign-in; back to the page with `loginFailed` (and
/// `lockoutUntil`, Unix milliseconds) on failure.
async fn form_login(
    State(state): State<AppState>,
    client: ClientInfo,
    headers: HeaderMap,
    jar: CookieJar,
    Query(q): Query<ReturnUrl>,
    Form(form): Form<LoginForm>,
) -> ApiResult<Response> {
    let return_url = q.return_url.unwrap_or_default();
    let failed = |state: &AppState| {
        let window = Duration::from_secs(state.config.auth.login_window_secs);
        let lockout = state
            .login_limiter
            .blocked(client.ip, state.config.auth.max_failed_logins, window)
            .map(|wait| {
                (chrono::Utc::now() + chrono::Duration::from_std(wait).unwrap_or_default())
                    .timestamp_millis()
            });
        let mut url = format!(
            "{}/login?returnUrl={}&loginFailed=true",
            base(state),
            encode(&return_url)
        );
        if let Some(until) = lockout {
            url.push_str(&format!("&lockoutUntil={until}"));
        }
        found(url)
    };
    match crate::auth::check_password(&state, client.ip, &form.username, form.password).await? {
        crate::auth::PasswordCheck::Valid(user) => {
            let remember = form.remember_me.as_deref() == Some("on");
            let cookie =
                crate::auth::start_session(&state, client, &headers, user.id, remember).await?;
            let target = redirect_url(base(&state), Some(&return_url));
            Ok((jar.add(cookie), found(target)).into_response())
        }
        _ => Ok(failed(&state)),
    }
}

/// `POST /login/setup`: creates the first account from the sign-in page.
async fn form_setup(
    State(state): State<AppState>,
    client: ClientInfo,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<ResetPasswordForm>,
) -> ApiResult<Response> {
    let invalid = || found(format!("{}/login?setupInvalid=true", base(&state)));
    if form.password != form.password_confirmation
        || validate_credentials(&form.username, &form.password).is_err()
        || !setup_required(&state).await?
    {
        return Ok(invalid());
    }
    let hash = crate::auth::hash_blocking(form.password).await?;
    let user_id = match users::create_first_admin(&state.db, &form.username, &hash).await {
        Ok(id) => id,
        Err(kitsunarr_db::DbError::Conflict(_)) => return Ok(invalid()),
        Err(e) => return Err(e.into()),
    };
    tracing::info!(target: "auth", "Initial account created in the browser");
    let cookie = crate::auth::start_session(&state, client, &headers, user_id, false).await?;
    Ok((jar.add(cookie), found(format!("{}/", base(&state)))).into_response())
}

/// `POST /reset-password/request`.
async fn form_request_reset(
    State(state): State<AppState>,
    client: ClientInfo,
) -> ApiResult<Response> {
    let issued = request_reset(&state, client.ip).await?;
    Ok(found(format!(
        "{}/login?resetPassword=true&tokenSent=true{}",
        base(&state),
        if issued.is_some() {
            ""
        } else {
            "&tokenUnavailable=true"
        }
    )))
}

/// `POST /reset-password`.
async fn form_reset_password(
    State(state): State<AppState>,
    client: ClientInfo,
    Form(form): Form<ResetPasswordForm>,
) -> ApiResult<Response> {
    let b = base(&state);
    if form.username.trim().is_empty()
        || form.password.trim().is_empty()
        || form.password != form.password_confirmation
        || validate_credentials(&form.username, &form.password).is_err()
    {
        return Ok(found(format!(
            "{b}/login?resetPassword=true&tokenSent=true&resetInvalid=true"
        )));
    }
    if !reset_password(
        &state,
        client.ip,
        &form.token,
        &form.username,
        &form.password,
    )
    .await?
    {
        return Ok(found(format!(
            "{b}/login?resetPassword=true&tokenSent=true&resetFailed=true"
        )));
    }
    Ok(found(format!("{b}/login?passwordReset=true")))
}

/// `GET /logout`: ends the session; with OIDC, signs out of the provider too when it offers
/// an end-session endpoint, else goes to `/loggedout`.
async fn logout(
    State(state): State<AppState>,
    client: ClientInfo,
    Peer(peer): Peer,
    headers: HeaderMap,
    jar: CookieJar,
) -> ApiResult<Response> {
    let mut id_token = None;
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let hash = kitsunarr_auth::hash_token(cookie.value());
        users::delete_session(state.db.pool(), &hash).await?;
        if let Some(sso) = users::delete_sso_session(state.db.pool(), &hash).await? {
            id_token = sso.id_token;
        }
    }
    tracing::info!(target: "auth", "Auth-Logout ip {}", client.ip);
    let jar = jar.remove(crate::auth::removal_cookie(&state));
    let b = base(&state);
    let settings = effective(&state).await?;
    if settings.effective_method() == AuthenticationMethod::Oidc {
        let origin = crate::oidc::origin(&state, &headers, peer);
        let target = match crate::oidc::end_session_url(&state, &settings, id_token, &origin).await
        {
            Some(url) => url,
            None => format!("{b}/loggedout"),
        };
        return Ok((jar, found(target)).into_response());
    }
    Ok((jar, found(format!("{b}/"))).into_response())
}

// ---- first start, health --------------------------------------------------------------------

/// Tanukarr's `InitialUserService`: with forms sign-in, no account and a configured initial
/// password, the administrator is created on start.
pub async fn create_initial_user(db: &kitsunarr_db::Database, config: &Config) -> ApiResult<()> {
    let Some(password) = config
        .auth
        .initial_password
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    else {
        return Ok(());
    };
    if users::user_count(db.pool()).await? > 0 {
        return Ok(());
    }
    let mut settings: SecuritySettings = kv::get_or_default(db.pool(), kv::KEY_SECURITY).await?;
    apply_overrides(config, &mut settings);
    if settings.authentication_method != AuthenticationMethod::Forms {
        return Ok(());
    }
    let username = config
        .auth
        .initial_username
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or("admin");
    if let Err(message) = validate_credentials(username, password) {
        tracing::error!(
            "the configured initial account was not created: {message} Open Kitsunarr in your browser to create your account."
        );
        return Ok(());
    }
    let hash = crate::auth::hash_blocking(password.to_string()).await?;
    match users::create_first_admin(db, username, &hash).await {
        Ok(_) | Err(kitsunarr_db::DbError::Conflict(_)) => {}
        Err(e) => return Err(e.into()),
    }
    tracing::info!(
        "Authentication is enabled and no user existed, created the account with the configured password"
    );
    Ok(())
}

/// `PasswordResetTokenCheck` and `OidcConfigurationCheck`.
pub async fn health_checks(state: &AppState, out: &mut Vec<HealthCheck>) {
    if let Some(token) = state
        .config
        .auth
        .reset_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        out.push(if token.chars().count() < MIN_CONFIGURED_RESET_TOKEN {
            HealthCheck {
                source: "PasswordResetTokenCheck".into(),
                level: HealthLevel::Warning,
                message: format!(
                    "The configured password reset token is shorter than {MIN_CONFIGURED_RESET_TOKEN} characters and is being ignored"
                ),
            }
        } else {
            HealthCheck {
                source: "PasswordResetTokenCheck".into(),
                level: HealthLevel::Notice,
                message: "A password reset token is configured. Remove the KITSUNARR_AUTH_RESET_TOKEN environment variable (or reset_token in config.toml) once you no longer need it".into(),
            }
        });
    }
    let Ok(settings) = effective(state).await else {
        return;
    };
    if settings.authentication_method != AuthenticationMethod::Oidc {
        return;
    }
    let error = |message: String| HealthCheck {
        source: "OidcConfigurationCheck".into(),
        level: HealthLevel::Error,
        message,
    };
    if !settings.has_oidc_settings() {
        out.push(error("OIDC authentication is enabled, but Authority, Client ID, Client Secret, User and Scopes are not all configured".into()));
    } else if !settings.is_oidc_authority_secure() {
        out.push(error(
            "The OIDC Authority must start with 'https://'".into(),
        ));
    } else if crate::oidc::discover(state, &settings.oidc_authority, false)
        .await
        .is_err()
    {
        out.push(error(format!(
            "Unable to retrieve the OIDC configuration from {}, signing in with your identity provider will fail until it is reachable",
            settings.oidc_authority
        )));
    }
}

// ---- Sonarr v3 `config/host` ----------------------------------------------------------------

/// Sonarr's `HostConfigResource` with the security fields in force.
pub async fn with_v3_host_fields(
    state: &AppState,
    mut resource: serde_json::Value,
) -> ApiResult<serde_json::Value> {
    let s = effective(state).await?;
    let user = users::first_admin(state.db.pool()).await?;
    let json = serde_json::json!({
        "authenticationMethod": s.authentication_method,
        "authenticationRequired": s.authentication_required,
        "allowedHosts": s.allowed_hosts,
        "trustCgnatIpAddresses": s.trust_cgnat_ip_addresses,
        "oidcAuthority": s.oidc_authority,
        "oidcClientId": s.oidc_client_id,
        "oidcClientSecret": if s.oidc_client_secret.is_empty() { "" } else { REDACTED },
        "oidcUserIdentifier": s.oidc_user_identifier,
        "oidcScopes": s.oidc_scopes,
        "username": user.as_ref().map(|u| u.username.clone()).unwrap_or_default(),
        "password": if user.is_some() { REDACTED } else { "" },
        "passwordConfirmation": "",
    });
    if let (Some(target), Some(fields)) = (resource.as_object_mut(), json.as_object()) {
        for (k, v) in fields {
            target.insert(k.clone(), v.clone());
        }
    }
    Ok(resource)
}

/// Applies the security fields of a `PUT config/host` (absent fields keep their value).
/// `basic` is refused as Tanukarr does; a username with a new password (not the placeholder)
/// becomes the administrator's.
pub async fn apply_v3_host(state: &AppState, body: &serde_json::Value) -> ApiResult<()> {
    let text = |k: &str| body.get(k).and_then(serde_json::Value::as_str);
    let mut next: SecuritySettings = kv::get_or_default(state.db.pool(), kv::KEY_SECURITY).await?;
    if let Some(m) = text("authenticationMethod") {
        if m.eq_ignore_ascii_case("basic") {
            return Err(ApiError::field(
                "authenticationMethod",
                "'Basic' is no longer supported, switch to 'Forms' instead.",
            ));
        }
        next.authentication_method = AuthenticationMethod::parse(m).ok_or_else(|| {
            ApiError::field("authenticationMethod", format!("Unknown method '{m}'."))
        })?;
    }
    if let Some(r) = text("authenticationRequired") {
        next.authentication_required = AuthenticationRequired::parse(r).ok_or_else(|| {
            ApiError::field("authenticationRequired", format!("Unknown value '{r}'."))
        })?;
    }
    if let Some(v) = body
        .get("trustCgnatIpAddresses")
        .and_then(serde_json::Value::as_bool)
    {
        next.trust_cgnat_ip_addresses = v;
    }
    for (key, slot) in [
        ("allowedHosts", &mut next.allowed_hosts),
        ("oidcAuthority", &mut next.oidc_authority),
        ("oidcClientId", &mut next.oidc_client_id),
        ("oidcClientSecret", &mut next.oidc_client_secret),
        ("oidcUserIdentifier", &mut next.oidc_user_identifier),
        ("oidcScopes", &mut next.oidc_scopes),
    ] {
        if let Some(v) = text(key) {
            *slot = v.to_string();
        }
    }
    save(state, next).await?;
    let (username, password) = (
        text("username").unwrap_or_default(),
        text("password").unwrap_or_default(),
    );
    if !username.trim().is_empty() && !password.is_empty() && password != REDACTED {
        if text("passwordConfirmation").is_some_and(|c| c != password) {
            return Err(ApiError::field(
                "passwordConfirmation",
                "Must match Password",
            ));
        }
        validate_credentials(username, password).map_err(|m| ApiError::field("password", m))?;
        let hash = crate::auth::hash_blocking(password.to_string()).await?;
        users::upsert_admin(&state.db, username, &hash).await?;
        tracing::info!(target: "auth", "the administrator's credentials were changed through config/host");
    } else if !username.trim().is_empty()
        && let Some(user) = users::first_admin(state.db.pool()).await?
        && user.username != username.trim()
    {
        users::upsert_admin(&state.db, username, &user.password_hash).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_stay_local_and_below_the_url_base() {
        assert_eq!(redirect_url("", None), "/");
        assert_eq!(redirect_url("", Some("/series/3")), "/series/3");
        assert_eq!(redirect_url("", Some("//evil.example/x")), "/");
        assert_eq!(redirect_url("", Some("https://evil.example")), "/");
        assert_eq!(redirect_url("", Some("/\\evil.example")), "/");
        assert_eq!(redirect_url("/k", Some("/series")), "/k/series");
        assert_eq!(redirect_url("/k", Some("/k/series")), "/k/series");
        assert_eq!(redirect_url("/k", Some("/K")), "/K");
        assert_eq!(redirect_url("/k", Some("")), "/k/");
    }

    #[test]
    fn local_addresses_follow_sonarr() {
        for local in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.0.5",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1",
        ] {
            assert!(is_local_address(local.parse().unwrap()), "{local}");
        }
        for remote in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "2001:db8::1"] {
            assert!(!is_local_address(remote.parse().unwrap()), "{remote}");
        }
        assert!(is_cgnat_address("100.64.0.1".parse().unwrap()));
        assert!(is_cgnat_address("100.127.255.255".parse().unwrap()));
        assert!(!is_cgnat_address("100.128.0.1".parse().unwrap()));
    }

    #[test]
    fn exemptions_need_a_known_client_address() {
        let local = SecuritySettings {
            authentication_required: AuthenticationRequired::DisabledForLocalAddresses,
            ..Default::default()
        };
        let none = HeaderMap::new();
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        assert!(exempt(&local, &none, Some(lan), &[]));
        assert!(!exempt(&local, &none, None, &[]));
        assert!(!exempt(
            &local,
            &none,
            Some("8.8.8.8".parse().unwrap()),
            &[]
        ));
        // An untrusted proxy hides the client.
        let mut forwarded = HeaderMap::new();
        forwarded.insert("x-forwarded-for", "8.8.8.8".parse().unwrap());
        assert!(!exempt(&local, &forwarded, Some(lan), &[]));
        // A trusted proxy reveals it.
        let trusted: Vec<IpNetwork> = vec!["192.168.1.20".parse().unwrap()];
        assert!(!exempt(&local, &forwarded, Some(lan), &trusted));
        forwarded.insert("x-forwarded-for", "192.168.1.30".parse().unwrap());
        assert!(exempt(&local, &forwarded, Some(lan), &trusted));
        // CGNAT only when trusted.
        let cgnat: IpAddr = "100.100.1.1".parse().unwrap();
        assert!(!exempt(&local, &none, Some(cgnat), &[]));
        let trusting = SecuritySettings {
            trust_cgnat_ip_addresses: true,
            ..local.clone()
        };
        assert!(exempt(&trusting, &none, Some(cgnat), &[]));
        // Localhost only.
        let localhost = SecuritySettings {
            authentication_required: AuthenticationRequired::DisabledForLocalhost,
            ..Default::default()
        };
        assert!(exempt(
            &localhost,
            &none,
            Some("127.0.0.1".parse().unwrap()),
            &[]
        ));
        assert!(!exempt(&localhost, &none, Some(lan), &[]));
        assert!(!exempt(
            &SecuritySettings::default(),
            &none,
            Some("127.0.0.1".parse().unwrap()),
            &[]
        ));
    }

    #[test]
    fn allowed_hosts_match_like_host_filtering() {
        let allowed = parse_allowed_hosts("kitsunarr.lan, *.example.com, 192.168.1.5, 0.0.0.0");
        let names = vec!["nas".to_string()];
        for ok in [
            "kitsunarr.lan",
            "KITSUNARR.LAN:8990",
            "tv.example.com",
            "192.168.1.5:8990",
            "localhost:8990",
            "127.0.0.1",
            "[::1]:8990",
            "nas",
            "",
        ] {
            assert!(host_allowed(&allowed, &names, ok), "{ok}");
        }
        for bad in [
            "evil.example",
            "example.com",
            "192.168.1.6",
            "rebind.attacker.net:8990",
        ] {
            assert!(!host_allowed(&allowed, &names, bad), "{bad}");
        }
        assert!(host_allowed(&[], &names, "anything"));
        assert!(
            host_allowed(&parse_allowed_hosts("0.0.0.0"), &names, "anything"),
            "bind addresses alone mean any host"
        );
    }

    #[test]
    fn overrides_lock_settings() {
        let mut config = Config::default();
        config.auth.method = Some(AuthMethod::External);
        config.server.allowed_hosts = Some(" kitsunarr.lan ".into());
        let mut s = SecuritySettings::default();
        apply_overrides(&config, &mut s);
        assert_eq!(s.authentication_method, AuthenticationMethod::External);
        assert_eq!(s.allowed_hosts, "kitsunarr.lan");
        assert_eq!(s.locked, vec!["authenticationMethod", "allowedHosts"]);
    }
}
