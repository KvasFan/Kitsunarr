//! Authentication: first-run setup, login sessions (HttpOnly cookie), API keys and the
//! middleware that guards every non-public route.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use kitsunarr_config::{CookieSecurity, IpNetwork};
use kitsunarr_db::auth as users;
use kitsunarr_domain::api::auth::{
    ApiKeyResponse, AuthStatus, AuthenticationMethod, ChangePasswordRequest, LoginRequest,
    SetupRequest, validate_credentials, validate_password,
};
use kitsunarr_domain::api::{API_KEY_HEADER, SESSION_COOKIE};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

/// Who is making the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    Session {
        session_id: i64,
        user_id: i64,
        username: String,
    },
    ApiKey,
    /// A companion app key (Sonarr v3 API only, scoped by its permission).
    Companion {
        id: String,
        permission: String,
    },
    /// Signed in through the OpenID Connect provider as the configured OIDC user.
    Sso {
        session_id: i64,
        user: String,
    },
    /// No sign-in needed: a reverse proxy authenticates (`External`), or authentication is
    /// disabled for this local client (Sonarr's anonymous user).
    Anonymous,
}

impl<S: Send + Sync> FromRequestParts<S> for Principal {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Principal>()
            .cloned()
            .ok_or(ApiError::Unauthorized)
    }
}

/// Peer address and request scheme, resolved through trusted proxies.
#[derive(Debug, Clone, Copy)]
pub struct ClientInfo {
    pub ip: IpAddr,
    pub https: bool,
}

impl FromRequestParts<AppState> for ClientInfo {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        Ok(resolve_client(
            &parts.headers,
            peer,
            &state.config.server.trusted_proxies,
        ))
    }
}

/// Honours `X-Forwarded-For` / `X-Forwarded-Proto` only when the direct peer is trusted.
pub fn resolve_client(headers: &HeaderMap, peer: IpAddr, trusted: &[IpNetwork]) -> ClientInfo {
    let is_trusted = |ip: IpAddr| trusted.iter().any(|n| n.contains(ip));
    if !is_trusted(peer) {
        return ClientInfo {
            ip: peer,
            https: false,
        };
    }
    let mut ip = peer;
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        // Walk from the closest hop outwards; the first untrusted address is the client.
        for hop in xff.rsplit(',').map(str::trim) {
            match hop.parse::<IpAddr>() {
                Ok(addr) => {
                    ip = addr;
                    if !is_trusted(addr) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    let https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|p| {
            p.split(',')
                .next()
                .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"))
        });
    ClientInfo { ip, https }
}

/// Fixed-window counter of failed logins per client address.
#[derive(Default)]
pub struct LoginLimiter {
    state: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

impl LoginLimiter {
    pub(crate) fn blocked(&self, ip: IpAddr, max: u32, window: Duration) -> Option<Duration> {
        let mut map = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.retain(|_, (_, start)| now.duration_since(*start) < window);
        match map.get(&ip) {
            Some((count, start)) if *count >= max => {
                Some(window.saturating_sub(now.duration_since(*start)))
            }
            _ => None,
        }
    }

    pub(crate) fn record_failure(&self, ip: IpAddr, window: Duration) {
        let mut map = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = map.entry(ip).or_insert((0, now));
        if now.duration_since(entry.1) >= window {
            *entry = (0, now);
        }
        entry.0 += 1;
    }

    pub(crate) fn clear(&self, ip: IpAddr) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&ip);
    }
}

fn api_key_from(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(v) = headers.get(API_KEY_HEADER).and_then(|v| v.to_str().ok()) {
        return Some(v.trim().to_string());
    }
    // `Authorization: Bearer <key>` is accepted as well.
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok())
        && let Some(token) = v.strip_prefix("Bearer ")
    {
        return Some(token.trim().to_string());
    }
    // Sonarr-compatible `?apikey=` query parameter, and `?access_token=` (SignalR clients).
    query.and_then(|q| {
        q.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k.eq_ignore_ascii_case("apikey") || k.eq_ignore_ascii_case("access_token"))
                .then(|| v.to_string())
        })
    })
}

/// Resolves the principal of a request without rejecting it: an API key, else the session
/// of the authentication method in force, else Sonarr's anonymous user when no sign-in is
/// needed (`External`, or authentication disabled for this local client). `peer` is the
/// direct peer's address.
pub async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    query: Option<&str>,
    peer: Option<IpAddr>,
) -> ApiResult<Option<Principal>> {
    if let Some(key) = api_key_from(headers, query) {
        let expected = state
            .api_key
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        return if kitsunarr_auth::secrets_equal(&key, &expected) {
            Ok(Some(Principal::ApiKey))
        } else {
            Ok(None)
        };
    }
    let settings = crate::security::effective(state).await?;
    let method = settings.effective_method();
    let jar = CookieJar::from_headers(headers);
    let session = match jar.get(SESSION_COOKIE) {
        Some(cookie) => {
            let token_hash = kitsunarr_auth::hash_token(cookie.value());
            let now = kitsunarr_db::now();
            match method {
                AuthenticationMethod::Forms => {
                    users::find_session(state.db.pool(), &token_hash, &now).await?
                }
                AuthenticationMethod::Oidc => {
                    // `OnValidatePrincipal`: only sessions of the configured OIDC user.
                    if let Some(sso) =
                        users::find_sso_session(state.db.pool(), &token_hash, &now).await?
                        && sso.subject.to_lowercase()
                            == settings.oidc_user_identifier.trim().to_lowercase()
                    {
                        let stale = chrono::DateTime::parse_from_rfc3339(&sso.last_seen_at)
                            .map(|t| {
                                Utc::now().signed_duration_since(t) > chrono::Duration::minutes(1)
                            })
                            .unwrap_or(true);
                        if stale {
                            users::touch_sso_session(state.db.pool(), sso.id).await?;
                        }
                        return Ok(Some(Principal::Sso {
                            session_id: sso.id,
                            user: sso.subject,
                        }));
                    }
                    None
                }
                AuthenticationMethod::External => None,
            }
        }
        None => None,
    };
    let Some(session) = session else {
        let exempt = method == AuthenticationMethod::External
            || crate::security::exempt(
                &settings,
                headers,
                peer,
                &state.config.server.trusted_proxies,
            );
        return Ok(exempt.then_some(Principal::Anonymous));
    };
    // Sliding refresh, at most once per minute to keep writes low.
    let stale = chrono::DateTime::parse_from_rfc3339(&session.last_seen_at)
        .map(|t| Utc::now().signed_duration_since(t) > chrono::Duration::minutes(1))
        .unwrap_or(true);
    if stale {
        users::touch_session(state.db.pool(), session.session_id, None).await?;
    }
    Ok(Some(Principal::Session {
        session_id: session.session_id,
        user_id: session.user_id,
        username: session.username,
    }))
}

/// What a companion key may call on the Sonarr v3 API (Tanukarr's allowlist): reads of the
/// library resources (a list, or one item by numeric id), and adding a series with the
/// `add-series` permission. Everything else (commands, settings, updates, deletes) is refused.
pub fn companion_allows(permission: &str, method: &axum::http::Method, path: &str) -> bool {
    let path = path.strip_prefix("/api/v3").unwrap_or(path);
    if path.contains('%') || path.contains("..") {
        return false;
    }
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let readable = [
        "series",
        "episode",
        "episodefile",
        "calendar",
        "tag",
        "rootfolder",
        "qualityprofile",
    ];
    match *method {
        axum::http::Method::GET => {
            readable.contains(&segs[0])
                && (segs.len() == 1
                    || (segs.len() == 2 && segs[1].parse::<u64>().is_ok_and(|n| n > 0)))
        }
        axum::http::Method::POST => permission == "add-series" && segs == ["series"],
        _ => false,
    }
}

/// Middleware for the Sonarr v3 API: the master key or a session, else a companion key within
/// its permission.
pub async fn require_auth_v3(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let query = req.uri().query().map(str::to_string);
    let peer = crate::security::peer_of(req.extensions());
    match authenticate(&state, req.headers(), query.as_deref(), peer).await {
        Ok(Some(principal)) => {
            req.extensions_mut().insert(principal);
            return next.run(req).await;
        }
        Err(e) => return e.into_response(),
        Ok(None) => {}
    }
    if let Some(key) = api_key_from(req.headers(), query.as_deref())
        && key.starts_with("spr_")
    {
        let hash = kitsunarr_auth::hash_companion_token(&key);
        if let Ok(Some(k)) = kitsunarr_db::companion_keys::find_valid(state.db.pool(), &hash).await
            && companion_allows(&k.permission, req.method(), req.uri().path())
        {
            req.extensions_mut().insert(Principal::Companion {
                id: k.id,
                permission: k.permission,
            });
            return next.run(req).await;
        }
    }
    ApiError::Unauthorized.into_response()
}

/// Middleware for protected routes.
pub async fn require_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let query = req.uri().query().map(str::to_string);
    let peer = crate::security::peer_of(req.extensions());
    match authenticate(&state, req.headers(), query.as_deref(), peer).await {
        Ok(Some(principal)) => {
            req.extensions_mut().insert(principal);
            next.run(req).await
        }
        Ok(None) => ApiError::Unauthorized.into_response(),
        Err(e) => e.into_response(),
    }
}

pub(crate) fn session_cookie(
    state: &AppState,
    client: ClientInfo,
    token: String,
    remember: bool,
) -> Cookie<'static> {
    let secure = match state.config.auth.secure_cookies {
        CookieSecurity::Always => true,
        CookieSecurity::Never => false,
        CookieSecurity::Auto => client.https,
    };
    let path = if state.config.server.url_base.is_empty() {
        "/".to_string()
    } else {
        state.config.server.url_base.clone()
    };
    let mut cookie = Cookie::build((SESSION_COOKIE, token))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path(path);
    if remember {
        cookie = cookie.max_age(time_days(state.config.auth.session_days));
    }
    cookie.build()
}

fn time_days(days: u32) -> time::Duration {
    time::Duration::days(i64::from(days))
}

pub(crate) async fn start_session(
    state: &AppState,
    client: ClientInfo,
    headers: &HeaderMap,
    user_id: i64,
    remember: bool,
) -> ApiResult<Cookie<'static>> {
    let token = kitsunarr_auth::generate_session_token();
    let ttl = if remember {
        chrono::Duration::days(i64::from(state.config.auth.session_days))
    } else {
        chrono::Duration::hours(i64::from(state.config.auth.short_session_hours))
    };
    let expires = (Utc::now() + ttl).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let ua = headers.get("user-agent").and_then(|v| v.to_str().ok());
    let ip = client.ip.to_string();
    users::create_session(
        state.db.pool(),
        users::NewSession {
            token_hash: &kitsunarr_auth::hash_token(&token),
            user_id,
            expires_at: &expires,
            user_agent: ua,
            remote_addr: Some(&ip),
        },
    )
    .await?;
    users::touch_login(state.db.pool(), user_id).await?;
    Ok(session_cookie(state, client, token, remember))
}

pub(crate) async fn hash_blocking(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || kitsunarr_auth::hash_password(&password))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}

async fn verify_blocking(password: String, phc: Option<String>) -> ApiResult<bool> {
    tokio::task::spawn_blocking(move || match phc {
        Some(phc) => kitsunarr_auth::verify_password(&password, &phc).unwrap_or(false),
        None => {
            kitsunarr_auth::dummy_verify(&password);
            false
        }
    })
    .await
    .map_err(ApiError::internal)
}

/// What a password sign-in found.
pub(crate) enum PasswordCheck {
    Valid(users::User),
    Invalid,
    /// Too many failures from this address: retry after the duration.
    Locked(Duration),
}

/// `AuthenticationService.Login`: only with forms sign-in; failures count per address and
/// lock it out for a while (even with the right password).
pub(crate) async fn check_password(
    state: &AppState,
    ip: IpAddr,
    username: &str,
    password: String,
) -> ApiResult<PasswordCheck> {
    if crate::security::effective(state).await?.effective_method() != AuthenticationMethod::Forms {
        return Ok(PasswordCheck::Invalid);
    }
    let window = Duration::from_secs(state.config.auth.login_window_secs);
    if let Some(wait) = state
        .login_limiter
        .blocked(ip, state.config.auth.max_failed_logins, window)
    {
        tracing::warn!(target: "auth", "Auth-Lockout ip {ip} username '{username}'");
        return Ok(PasswordCheck::Locked(wait));
    }
    let user = users::find_user_by_name(state.db.pool(), username).await?;
    let ok = verify_blocking(password, user.as_ref().map(|u| u.password_hash.clone())).await?;
    let Some(user) = user.filter(|_| ok) else {
        state.login_limiter.record_failure(ip, window);
        tracing::warn!(target: "auth", "Auth-Failure ip {ip} username '{username}'");
        return Ok(PasswordCheck::Invalid);
    };
    state.login_limiter.clear(ip);
    tracing::debug!(target: "auth", "Auth-Success ip {ip} username '{username}'");
    Ok(PasswordCheck::Valid(user))
}

/// The cookie that removes the session cookie.
pub(crate) fn removal_cookie(state: &AppState) -> Cookie<'static> {
    let path = if state.config.server.url_base.is_empty() {
        "/".to_string()
    } else {
        state.config.server.url_base.clone()
    };
    Cookie::build(SESSION_COOKIE).path(path).build()
}

async fn auth_status(state: &AppState, principal: Option<&Principal>) -> ApiResult<AuthStatus> {
    let settings = crate::security::effective(state).await?;
    let username = match principal {
        Some(Principal::Session { username, .. }) => Some(username.clone()),
        Some(Principal::Sso { user, .. }) => Some(user.clone()),
        _ => None,
    };
    Ok(AuthStatus {
        setup_required: crate::security::setup_required(state).await?,
        authenticated: principal.is_some(),
        username,
        authentication_method: settings.effective_method(),
        oidc_misconfigured: settings.authentication_method == AuthenticationMethod::Oidc
            && !settings.is_oidc_configured(),
    })
}

/// Authentication state for the current browser. Public.
#[utoipa::path(get, path = "/auth/status", tag = "auth", responses((status = 200, body = AuthStatus)))]
pub async fn status(
    State(state): State<AppState>,
    crate::security::Peer(peer): crate::security::Peer,
    headers: HeaderMap,
) -> ApiResult<Json<AuthStatus>> {
    let principal = authenticate(&state, &headers, None, peer).await?;
    Ok(Json(auth_status(&state, principal.as_ref()).await?))
}

/// Creates the first administrator. Only possible while no user exists (and with forms
/// sign-in, without a configured initial password).
#[utoipa::path(post, path = "/auth/setup", tag = "auth", request_body = SetupRequest,
    responses((status = 200, body = AuthStatus), (status = 409, description = "Already set up")))]
pub async fn setup(
    State(state): State<AppState>,
    client: ClientInfo,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<SetupRequest>,
) -> ApiResult<(CookieJar, Json<AuthStatus>)> {
    if !crate::security::setup_required(&state).await? {
        return Err(ApiError::Conflict("Kitsunarr is already set up.".into()));
    }
    validate_credentials(&body.username, &body.password)
        .map_err(|m| ApiError::field("password", m))?;
    let hash = hash_blocking(body.password).await?;
    let user_id = users::create_first_admin(&state.db, &body.username, &hash).await?;
    tracing::info!(
        username = body.username.trim(),
        "administrator account created"
    );
    let cookie = start_session(&state, client, &headers, user_id, true).await?;
    let principal = Principal::Session {
        session_id: 0,
        user_id,
        username: body.username.trim().to_string(),
    };
    Ok((
        jar.add(cookie),
        Json(auth_status(&state, Some(&principal)).await?),
    ))
}

#[utoipa::path(post, path = "/auth/login", tag = "auth", request_body = LoginRequest,
    responses((status = 200, body = AuthStatus), (status = 403, description = "Invalid credentials, or password sign-in is off"), (status = 429)))]
pub async fn login(
    State(state): State<AppState>,
    client: ClientInfo,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<LoginRequest>,
) -> ApiResult<(CookieJar, Json<AuthStatus>)> {
    if crate::security::effective(&state).await?.effective_method() != AuthenticationMethod::Forms {
        return Err(ApiError::Forbidden(
            "Password sign-in is turned off; sign in with your identity provider.".into(),
        ));
    }
    let user = match check_password(&state, client.ip, &body.username, body.password).await? {
        PasswordCheck::Valid(user) => user,
        PasswordCheck::Invalid => {
            return Err(ApiError::Forbidden("Invalid username or password.".into()));
        }
        PasswordCheck::Locked(wait) => {
            return Err(ApiError::TooManyRequests(format!(
                "Too many failed logins. Try again in {} seconds.",
                wait.as_secs().max(1)
            )));
        }
    };
    let cookie = start_session(&state, client, &headers, user.id, body.remember_me).await?;
    let principal = Principal::Session {
        session_id: 0,
        user_id: user.id,
        username: user.username,
    };
    Ok((
        jar.add(cookie),
        Json(auth_status(&state, Some(&principal)).await?),
    ))
}

#[utoipa::path(post, path = "/auth/logout", tag = "auth", responses((status = 204)))]
pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> ApiResult<(CookieJar, StatusCode)> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let hash = kitsunarr_auth::hash_token(cookie.value());
        users::delete_session(state.db.pool(), &hash).await?;
        users::delete_sso_session(state.db.pool(), &hash).await?;
    }
    Ok((jar.remove(removal_cookie(&state)), StatusCode::NO_CONTENT))
}

#[utoipa::path(post, path = "/auth/password", tag = "auth", request_body = ChangePasswordRequest,
    responses((status = 204), (status = 403)))]
pub async fn change_password(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<ChangePasswordRequest>,
) -> ApiResult<StatusCode> {
    let Principal::Session {
        session_id,
        user_id,
        ..
    } = principal
    else {
        return Err(ApiError::Forbidden(
            "Password changes require an interactive session.".into(),
        ));
    };
    validate_password(&body.new_password).map_err(|m| ApiError::field("newPassword", m))?;
    let user = users::find_user(state.db.pool(), user_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if !verify_blocking(body.current_password, Some(user.password_hash)).await? {
        return Err(ApiError::Forbidden(
            "The current password is incorrect.".into(),
        ));
    }
    let hash = hash_blocking(body.new_password).await?;
    users::set_password(state.db.pool(), user_id, &hash).await?;
    let revoked = users::delete_user_sessions(state.db.pool(), user_id, Some(session_id)).await?;
    tracing::info!(revoked, "password changed; other sessions revoked");
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/auth/apikey", tag = "auth", responses((status = 200, body = ApiKeyResponse)))]
pub async fn get_api_key(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<ApiKeyResponse>> {
    if principal == Principal::ApiKey {
        return Err(ApiError::Forbidden(
            "The API key can only be viewed from a signed-in session.".into(),
        ));
    }
    Ok(Json(ApiKeyResponse {
        api_key: state
            .api_key
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
    }))
}

#[utoipa::path(post, path = "/auth/apikey/regenerate", tag = "auth", responses((status = 200, body = ApiKeyResponse)))]
pub async fn regenerate_api_key(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<ApiKeyResponse>> {
    if principal == Principal::ApiKey {
        return Err(ApiError::Forbidden(
            "The API key can only be regenerated from a signed-in session.".into(),
        ));
    }
    let key = kitsunarr_auth::generate_api_key();
    kitsunarr_db::settings::set(state.db.pool(), kitsunarr_db::settings::KEY_API_KEY, &key).await?;
    *state.api_key.write().unwrap_or_else(|e| e.into_inner()) = key.clone();
    tracing::info!("API key regenerated");
    Ok(Json(ApiKeyResponse { api_key: key }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_allowlist() {
        use axum::http::Method;
        assert!(companion_allows("read-library", &Method::GET, "/series"));
        assert!(companion_allows(
            "read-library",
            &Method::GET,
            "/api/v3/episode/12"
        ));
        assert!(!companion_allows(
            "read-library",
            &Method::GET,
            "/series/lookup"
        ));
        assert!(!companion_allows("read-library", &Method::GET, "/series/0"));
        assert!(!companion_allows("read-library", &Method::POST, "/series"));
        assert!(companion_allows("add-series", &Method::POST, "/series"));
        assert!(!companion_allows("add-series", &Method::PUT, "/series/1"));
        assert!(!companion_allows("add-series", &Method::GET, "/command"));
        assert!(!companion_allows(
            "read-library",
            &Method::GET,
            "/series/%2e%2e"
        ));
    }

    #[test]
    fn forwarded_headers_only_from_trusted_proxies() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.9, 10.0.0.2".parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        let trusted: Vec<IpNetwork> = vec!["10.0.0.0/8".parse().unwrap()];
        let peer: IpAddr = "10.0.0.1".parse().unwrap();
        let c = resolve_client(&headers, peer, &trusted);
        assert_eq!(c.ip, "203.0.113.9".parse::<IpAddr>().unwrap());
        assert!(c.https);
        let untrusted = resolve_client(&headers, "198.51.100.1".parse().unwrap(), &trusted);
        assert_eq!(
            untrusted.ip,
            "198.51.100.1".parse::<IpAddr>().unwrap(),
            "spoofed header ignored"
        );
        assert!(!untrusted.https);
    }

    #[test]
    fn api_key_sources() {
        let mut h = HeaderMap::new();
        assert_eq!(api_key_from(&h, Some("a=1&apikey=abc")), Some("abc".into()));
        h.insert("authorization", "Bearer xyz".parse().unwrap());
        assert_eq!(api_key_from(&h, None), Some("xyz".into()));
        h.insert(API_KEY_HEADER, "k1".parse().unwrap());
        assert_eq!(api_key_from(&h, None), Some("k1".into()));
    }

    #[test]
    fn limiter_blocks_after_max_failures() {
        let l = LoginLimiter::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let w = Duration::from_secs(60);
        for _ in 0..3 {
            assert!(l.blocked(ip, 3, w).is_none());
            l.record_failure(ip, w);
        }
        assert!(l.blocked(ip, 3, w).is_some());
        l.clear(ip);
        assert!(l.blocked(ip, 3, w).is_none());
    }
}
