//! Settings › General › Security over HTTP, as Tanukarr has it: Sonarr's form routes, password
//! resets through the log or a configured token, allowed hosts, local exemptions, external
//! authentication, configuration overrides, OIDC sign-in against a stand-in provider, Sonarr
//! v3 `config/host`, and the sign-in flows and custom-format fields added with them.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use kitsunarr_api::{AppState, BuildInfo, LogBuffer, NoAssets, NoTasks};
use kitsunarr_client::{Client, ClientError};
use kitsunarr_config::Config;
use kitsunarr_db::Database;
use kitsunarr_domain::api::auth::{
    AuthenticationMethod, AuthenticationRequired, LoginRequest, SecuritySettings, SetupRequest,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Server {
    base: String,
    api_key: String,
    state: AppState,
    logs: LogBuffer,
    _dir: tempfile::TempDir,
}

async fn start_with(configure: impl FnOnce(&mut Config)) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let api_key = kitsunarr_db::seed::ensure_api_key(&db, kitsunarr_auth::generate_api_key)
        .await
        .unwrap();
    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        auth: kitsunarr_config::AuthConfig {
            max_failed_logins: 3,
            ..Default::default()
        },
        ..Default::default()
    };
    configure(&mut config);
    let logs = LogBuffer::default();
    kitsunarr_api::security::create_initial_user(&db, &config)
        .await
        .unwrap();
    let state = AppState::new(
        db,
        Arc::new(config),
        kitsunarr_events::EventBus::default(),
        api_key.clone(),
        BuildInfo {
            version: "test",
            git_revision: None,
        },
        Arc::new(NoAssets),
        Arc::new(NoTasks),
        reqwest::Client::new(),
        kitsunarr_metadata::Metadata::with(
            reqwest::Client::new(),
            &kitsunarr_metadata::Endpoints::offline(),
        ),
        logs.clone(),
    );
    let app = kitsunarr_api::app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server {
        base: format!("http://{addr}"),
        api_key,
        state,
        logs,
        _dir: dir,
    }
}

async fn start() -> Server {
    start_with(|_| {}).await
}

/// A client that does not follow redirects and keeps no cookies (tests read them).
fn raw() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn location(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

/// `name=value` of a cookie the response sets.
fn set_cookie(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with(&format!("{name}=")))
        .map(|c| c.split(';').next().unwrap().to_string())
}

fn status(e: ClientError) -> u16 {
    e.status().unwrap_or(0)
}

async fn admin(server: &Server) -> Client {
    let client = Client::new(&server.base);
    client
        .setup(&SetupRequest {
            username: "admin".into(),
            password: "correct horse battery".into(),
        })
        .await
        .unwrap();
    client
}

async fn auth_status(server: &Server, cookie: Option<&str>) -> Value {
    let mut req = raw().get(format!("{}/api/v1/auth/status", server.base));
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    req.send().await.unwrap().json().await.unwrap()
}

#[tokio::test]
async fn sonarr_form_routes_sign_in_set_up_and_out() {
    let server = start().await;
    let http = raw();
    let post_form = |path: &str, form: &[(&str, &str)]| {
        http.post(format!("{}{path}", server.base))
            .form(form)
            .send()
    };
    // First-run setup from the sign-in page.
    let bad = post_form(
        "/login/setup",
        &[
            ("username", "admin"),
            ("password", "correct horse battery"),
            ("passwordConfirmation", "different horse battery"),
        ],
    )
    .await
    .unwrap();
    assert_eq!(bad.status(), StatusCode::FOUND);
    assert_eq!(location(&bad), "/login?setupInvalid=true");
    let created = post_form(
        "/login/setup",
        &[
            ("username", "admin"),
            ("password", "correct horse battery"),
            ("passwordConfirmation", "correct horse battery"),
        ],
    )
    .await
    .unwrap();
    assert_eq!(location(&created), "/");
    let cookie = set_cookie(&created, "kitsunarr_session").expect("signed in");
    assert_eq!(
        auth_status(&server, Some(&cookie)).await["authenticated"],
        true
    );
    let again = post_form(
        "/login/setup",
        &[
            ("username", "evil"),
            ("password", "correct horse battery"),
            ("passwordConfirmation", "correct horse battery"),
        ],
    )
    .await
    .unwrap();
    assert_eq!(location(&again), "/login?setupInvalid=true", "only once");

    // Form sign-in: local return URLs only, "Remember Me" makes the cookie persistent.
    let ok = post_form(
        "/login?returnUrl=/series/1",
        &[
            ("username", "admin"),
            ("password", "correct horse battery"),
            ("rememberMe", "on"),
        ],
    )
    .await
    .unwrap();
    assert_eq!(location(&ok), "/series/1");
    let persistent = ok
        .headers()
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().unwrap().contains("Max-Age"));
    assert!(persistent);
    let session = set_cookie(&ok, "kitsunarr_session").unwrap();
    let evil = post_form(
        "/login?returnUrl=//evil.example/",
        &[("username", "admin"), ("password", "correct horse battery")],
    )
    .await
    .unwrap();
    assert_eq!(location(&evil), "/");
    assert!(
        !evil
            .headers()
            .get_all("set-cookie")
            .iter()
            .any(|v| v.to_str().unwrap().contains("Max-Age")),
        "a session cookie without Remember Me"
    );

    // Sign-out ends the session.
    let out = http
        .get(format!("{}/logout", server.base))
        .header("cookie", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(location(&out), "/");
    assert_eq!(
        auth_status(&server, Some(&session)).await["authenticated"],
        false
    );
    // The sign-in page itself is the web application (this build has none: its stand-in page).
    let spa = http
        .get(format!("{}/series", server.base))
        .send()
        .await
        .unwrap();
    let page = http
        .get(format!("{}/login", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), spa.status());
    assert_eq!(page.text().await.unwrap(), spa.text().await.unwrap());

    // Failures come back with loginFailed, then with the lockout's end.
    let failed = post_form(
        "/login?returnUrl=/series/1",
        &[("username", "admin"), ("password", "wrong wrong wrong")],
    )
    .await
    .unwrap();
    assert_eq!(
        location(&failed),
        "/login?returnUrl=%2Fseries%2F1&loginFailed=true"
    );
    for _ in 0..2 {
        post_form("/login", &[("username", "admin"), ("password", "nope")])
            .await
            .unwrap();
    }
    let locked = post_form(
        "/login",
        &[("username", "admin"), ("password", "correct horse battery")],
    )
    .await
    .unwrap();
    let target = location(&locked);
    assert!(
        target.contains("loginFailed=true&lockoutUntil="),
        "{target}"
    );
    let until: i64 = target.rsplit('=').next().unwrap().parse().unwrap();
    assert!(until > chrono::Utc::now().timestamp_millis());
    assert!(set_cookie(&locked, "kitsunarr_session").is_none());
}

/// The token of the newest `Auth-ResetRequested` log line.
fn logged_token(logs: &LogBuffer) -> Option<String> {
    logs.snapshot(tracing::Level::TRACE, 200)
        .into_iter()
        .find(|e| e.message.contains("Auth-ResetRequested"))
        .and_then(|e| {
            e.message
                .split("token: ")
                .nth(1)
                .map(|rest| rest.split(' ').next().unwrap().to_string())
        })
}

#[tokio::test(flavor = "current_thread")]
async fn password_resets_with_the_logged_and_the_configured_token() {
    use tracing_subscriber::layer::SubscriberExt;
    let configured = "configured-reset-token-0123";
    let server = start_with(|c| c.auth.reset_token = Some(configured.into())).await;
    // Everything runs on this thread: the subscriber sees the server's log lines.
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(server.logs.layer()));
    let client = admin(&server).await;
    let health = client.health().await.unwrap();
    let notice = health
        .iter()
        .find(|h| h.source == "PasswordResetTokenCheck")
        .expect("configured token reported");
    assert_eq!(
        notice.level,
        kitsunarr_domain::api::system::HealthLevel::Notice
    );

    let http = raw();
    let request = || {
        http.post(format!("{}/reset-password/request", server.base))
            .send()
    };
    let issued = request().await.unwrap();
    assert_eq!(
        location(&issued),
        "/login?resetPassword=true&tokenSent=true"
    );
    let token = logged_token(&server.logs).expect("token written to the log");
    assert_eq!(token.len(), 20);
    let throttled = request().await.unwrap();
    assert_eq!(
        location(&throttled),
        "/login?resetPassword=true&tokenSent=true&tokenUnavailable=true"
    );

    let reset = |token: &str, user: &str, pw: &str, confirm: &str| {
        http.post(format!("{}/reset-password", server.base))
            .form(&[
                ("token", token),
                ("username", user),
                ("password", pw),
                ("passwordConfirmation", confirm),
            ])
            .send()
    };
    let mismatch = reset(&token, "boss", "a new long passphrase", "another one")
        .await
        .unwrap();
    assert!(location(&mismatch).ends_with("resetInvalid=true"));
    let wrong = reset(
        "WRONGTOKENWRONGTOKEN",
        "boss",
        "a new long passphrase",
        "a new long passphrase",
    )
    .await
    .unwrap();
    assert!(location(&wrong).ends_with("resetFailed=true"));
    let done = reset(
        &token,
        "boss",
        "a new long passphrase",
        "a new long passphrase",
    )
    .await
    .unwrap();
    assert_eq!(location(&done), "/login?passwordReset=true");
    // The account is renamed, its sessions end, the new password works.
    assert_eq!(
        status(client.library_stats().await.unwrap_err()),
        401,
        "sessions were revoked"
    );
    let signed_in = Client::new(&server.base);
    signed_in
        .login(&LoginRequest {
            username: "boss".into(),
            password: "a new long passphrase".into(),
            remember_me: false,
        })
        .await
        .unwrap();
    // A token works once.
    let reused = reset(
        &token,
        "boss",
        "third long passphrase",
        "third long passphrase",
    )
    .await
    .unwrap();
    assert!(location(&reused).ends_with("resetFailed=true"));
    // The configured token too, until the next restart.
    let configured_reset = reset(
        configured,
        "admin",
        "fourth long passphrase",
        "fourth long passphrase",
    )
    .await
    .unwrap();
    assert_eq!(location(&configured_reset), "/login?passwordReset=true");
    let configured_again = reset(
        configured,
        "admin",
        "fifth long passphrase!",
        "fifth long passphrase!",
    )
    .await
    .unwrap();
    assert!(location(&configured_again).ends_with("resetFailed=true"));
    Client::new(&server.base)
        .login(&LoginRequest {
            username: "admin".into(),
            password: "fourth long passphrase".into(),
            remember_me: false,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_short_configured_reset_token_is_ignored_and_reported() {
    let server = start_with(|c| c.auth.reset_token = Some("too-short".into())).await;
    let client = admin(&server).await;
    let health = client.health().await.unwrap();
    let warning = health
        .iter()
        .find(|h| h.source == "PasswordResetTokenCheck")
        .unwrap();
    assert_eq!(
        warning.level,
        kitsunarr_domain::api::system::HealthLevel::Warning
    );
    let resp = raw()
        .post(format!("{}/reset-password", server.base))
        .form(&[
            ("token", "too-short"),
            ("username", "admin"),
            ("password", "a new long passphrase"),
            ("passwordConfirmation", "a new long passphrase"),
        ])
        .send()
        .await
        .unwrap();
    assert!(location(&resp).ends_with("resetFailed=true"));
}

#[tokio::test]
async fn the_configured_initial_password_creates_the_account() {
    let server = start_with(|c| {
        c.auth.initial_username = Some("operator".into());
        c.auth.initial_password = Some("from the environment".into());
    })
    .await;
    let s = auth_status(&server, None).await;
    assert_eq!(s["setupRequired"], false);
    Client::new(&server.base)
        .login(&LoginRequest {
            username: "operator".into(),
            password: "from the environment".into(),
            remember_me: false,
        })
        .await
        .unwrap();
    // Too short: no account, but no setup page either (as Tanukarr).
    let weak = start_with(|c| c.auth.initial_password = Some("short".into())).await;
    assert_eq!(
        kitsunarr_db::auth::user_count(weak.state.db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn allowed_hosts_local_exemptions_and_external_authentication() {
    let server = start().await;
    let client = admin(&server).await;
    let mut s = client.security().await.unwrap();
    assert_eq!(s, SecuritySettings::default());

    // Tanukarr's validation.
    s.authentication_required = AuthenticationRequired::DisabledForLocalAddresses;
    let e = client.put_security(&s).await.unwrap_err();
    assert_eq!(status(e), 422);
    s.allowed_hosts = "http://kitsunarr.lan:8990".into();
    assert_eq!(status(client.put_security(&s).await.unwrap_err()), 422);
    s.allowed_hosts = "0.0.0.0".into();
    assert_eq!(status(client.put_security(&s).await.unwrap_err()), 422);
    s.allowed_hosts = "kitsunarr.lan, *.example.com".into();
    client.put_security(&s).await.unwrap();

    // Allowed hosts: other names are refused with 400; loopback and listed names pass.
    let http = raw();
    let port = server.base.rsplit(':').next().unwrap().to_string();
    let with_host = |host: String| {
        http.get(format!("{}/api/v1/auth/status", server.base))
            .header("host", host)
            .send()
    };
    let refused = with_host(format!("rebind.attacker.net:{port}"))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(refused.text().await.unwrap().contains("Invalid Hostname"));
    for host in [
        format!("kitsunarr.lan:{port}"),
        format!("tv.example.com:{port}"),
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
    ] {
        assert_eq!(
            with_host(host.clone()).await.unwrap().status(),
            200,
            "{host}"
        );
    }

    // Disabled for local addresses: this client (127.0.0.1) needs no sign-in…
    let anon = Client::new(&server.base);
    assert!(anon.library_stats().await.is_ok());
    assert_eq!(auth_status(&server, None).await["authenticated"], true);
    // …unless an untrusted proxy hides the real client.
    let proxied = http
        .get(format!("{}/api/v1/library/stats", server.base))
        .header("x-forwarded-for", "203.0.113.7")
        .send()
        .await
        .unwrap();
    assert_eq!(proxied.status(), 401);
    // Disabled for localhost works the same for this client; enabled requires a sign-in.
    s.authentication_required = AuthenticationRequired::DisabledForLocalhost;
    client.put_security(&s).await.unwrap();
    assert!(anon.library_stats().await.is_ok());
    s.authentication_required = AuthenticationRequired::Enabled;
    client.put_security(&s).await.unwrap();
    assert_eq!(status(anon.library_stats().await.unwrap_err()), 401);

    // External: a reverse proxy authenticates; Kitsunarr asks for nothing and password
    // sign-in is off.
    s.authentication_method = AuthenticationMethod::External;
    client.put_security(&s).await.unwrap();
    assert!(anon.library_stats().await.is_ok());
    let st = auth_status(&server, None).await;
    assert_eq!(st["authenticationMethod"], "external");
    assert_eq!(st["setupRequired"], false);
    let login = anon
        .login(&LoginRequest {
            username: "admin".into(),
            password: "correct horse battery".into(),
            remember_me: false,
        })
        .await
        .unwrap_err();
    assert_eq!(status(login), 403);
    // The Sonarr v3 API follows the same rules.
    let v3 = http
        .get(format!("{}/api/v3/system/status", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(v3.status(), 200);
}

#[tokio::test]
async fn configuration_overrides_win_and_are_locked() {
    let server = start_with(|c| {
        c.auth.method = Some(kitsunarr_config::AuthMethod::External);
        c.server.allowed_hosts = Some("kitsunarr.lan".into());
    })
    .await;
    let client = Client::new(&server.base).with_api_key(&server.api_key);
    let mut s = client.security().await.unwrap();
    assert_eq!(s.authentication_method, AuthenticationMethod::External);
    assert_eq!(s.allowed_hosts, "kitsunarr.lan");
    assert_eq!(s.locked, vec!["authenticationMethod", "allowedHosts"]);
    s.authentication_method = AuthenticationMethod::Forms;
    s.allowed_hosts = String::new();
    s.trust_cgnat_ip_addresses = true;
    let saved = client.put_security(&s).await.unwrap();
    assert_eq!(saved.authentication_method, AuthenticationMethod::External);
    assert_eq!(saved.allowed_hosts, "kitsunarr.lan");
    assert!(
        saved.trust_cgnat_ip_addresses,
        "unlocked settings are saved"
    );
    assert!(Client::new(&server.base).library_stats().await.is_ok());
}

// ---- OIDC -----------------------------------------------------------------------------------

/// A code the provider issued: (nonce, PKCE challenge, redirect URI, subject, e-mail verified).
type Grant = (String, String, String, String, bool);

#[derive(Clone, Default)]
struct Provider {
    base: Arc<Mutex<String>>,
    /// code → (nonce, PKCE challenge, redirect URI, subject, e-mail verified)
    codes: Arc<Mutex<HashMap<String, Grant>>>,
    key: Arc<Mutex<Option<Arc<kitsunarr_auth::oidc::testing::TestKey>>>>,
    token_requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

impl Provider {
    fn base(&self) -> String {
        self.base.lock().unwrap().clone()
    }
    fn key(&self) -> Arc<kitsunarr_auth::oidc::testing::TestKey> {
        self.key.lock().unwrap().clone().unwrap()
    }
}

async fn discovery(State(p): State<Provider>) -> axum::Json<Value> {
    let b = p.base();
    axum::Json(json!({
        "issuer": b,
        "authorization_endpoint": format!("{b}/authorize"),
        "token_endpoint": format!("{b}/token"),
        "userinfo_endpoint": format!("{b}/userinfo"),
        "jwks_uri": format!("{b}/jwks"),
        "end_session_endpoint": format!("{b}/logout"),
        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
        "code_challenge_methods_supported": ["S256"],
    }))
}

async fn token(
    State(p): State<Provider>,
    Form(form): Form<HashMap<String, String>>,
) -> axum::response::Response {
    p.token_requests.lock().unwrap().push(form.clone());
    let Some((nonce, challenge, redirect, sub, verified)) = p
        .codes
        .lock()
        .unwrap()
        .remove(form.get("code").map(String::as_str).unwrap_or_default())
    else {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid_grant").into_response();
    };
    if form.get("client_secret").map(String::as_str) != Some("provider-secret")
        || form.get("redirect_uri") != Some(&redirect)
        || kitsunarr_auth::oidc::pkce_challenge(
            form.get("code_verifier")
                .map(String::as_str)
                .unwrap_or_default(),
        ) != challenge
    {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid_client").into_response();
    }
    let now = chrono::Utc::now().timestamp();
    let id_token = p.key().sign(&json!({
        "iss": p.base(), "aud": "kitsunarr", "sub": sub, "nonce": nonce,
        "iat": now, "exp": now + 300,
        "email": "fox@example.com", "email_verified": verified,
    }));
    axum::Json(json!({"access_token": format!("access-{sub}"), "token_type": "Bearer", "id_token": id_token, "expires_in": 300}))
        .into_response()
}

async fn userinfo(headers: HeaderMap) -> axum::response::Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some(sub) = auth.strip_prefix("Bearer access-") else {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    };
    // The user-info answer decides about the e-mail: verified only for "fox-1".
    axum::Json(json!({"sub": sub, "email": "fox@example.com", "email_verified": sub == "fox-1"}))
        .into_response()
}

async fn start_provider() -> Provider {
    let provider = Provider::default();
    *provider.key.lock().unwrap() = Some(Arc::new(
        kitsunarr_auth::oidc::testing::TestKey::generate("key-1"),
    ));
    let app =
        axum::Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route(
                "/jwks",
                get(|State(p): State<Provider>| async move {
                    axum::Json(json!({"keys": [p.key().jwk()]}))
                }),
            )
            .route("/token", post(token))
            .route("/userinfo", get(userinfo))
            .with_state(provider.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    *provider.base.lock().unwrap() = format!("http://{addr}");
    provider
}

/// Starts a sign-in at Kitsunarr: the provider's authorize URL and the correlation cookie.
async fn begin_sso(server: &Server, return_url: &str) -> (url::Url, String) {
    let resp = raw()
        .get(format!("{}/login/sso?returnUrl={return_url}", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let cookie = set_cookie(&resp, "kitsunarr_oidc").expect("correlation cookie");
    (url::Url::parse(&location(&resp)).unwrap(), cookie)
}

fn param(url: &url::Url, name: &str) -> String {
    url.query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// The provider "signs the user in" and sends the browser back to Kitsunarr.
async fn finish_sso(
    server: &Server,
    provider: &Provider,
    authorize: &url::Url,
    cookie: &str,
    sub: &str,
    verified: bool,
) -> reqwest::Response {
    let code = format!("code-{}", param(authorize, "state").len() + rand_suffix());
    provider.codes.lock().unwrap().insert(
        code.clone(),
        (
            param(authorize, "nonce"),
            param(authorize, "code_challenge"),
            param(authorize, "redirect_uri"),
            sub.to_string(),
            verified,
        ),
    );
    let redirect = param(authorize, "redirect_uri");
    assert_eq!(redirect, format!("{}/signin-oidc", server.base));
    raw()
        .get(format!(
            "{redirect}?code={code}&state={}",
            urlencode(&param(authorize, "state"))
        ))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
}

fn rand_suffix() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[tokio::test]
async fn oidc_sign_in_against_a_stand_in_provider() {
    let provider = start_provider().await;
    let server = start().await;
    let client = admin(&server).await;
    let keyed = Client::new(&server.base).with_api_key(&server.api_key);

    // Validation as Tanukarr's: https (or this machine), a reachable configuration, every
    // field, and the openid scope.
    let mut s = client.security().await.unwrap();
    s.authentication_method = AuthenticationMethod::Oidc;
    assert_eq!(status(client.put_security(&s).await.unwrap_err()), 422);
    s.oidc_authority = "http://idp.example".into();
    s.oidc_client_id = "kitsunarr".into();
    s.oidc_client_secret = "provider-secret".into();
    s.oidc_user_identifier = "fox@example.com".into();
    let insecure = client.put_security(&s).await.unwrap_err();
    assert!(insecure.to_string().contains("https://"), "{insecure}");
    s.oidc_authority = "http://127.0.0.1:9".into();
    let unreachable = client.put_security(&s).await.unwrap_err();
    assert!(
        unreachable.to_string().contains("Unable to retrieve"),
        "{unreachable}"
    );
    s.oidc_authority = provider.base();
    s.oidc_scopes = "profile email".into();
    assert_eq!(status(client.put_security(&s).await.unwrap_err()), 422);
    s.oidc_scopes = "openid profile email".into();
    let saved = client.put_security(&s).await.unwrap();
    assert_eq!(saved.oidc_client_secret, "********");

    // Password sessions end with OIDC; the sign-in page offers the provider.
    assert_eq!(status(client.library_stats().await.unwrap_err()), 401);
    let st = auth_status(&server, None).await;
    assert_eq!(st["authenticationMethod"], "oidc");
    assert_eq!(st["oidcMisconfigured"], false);
    assert_eq!(st["setupRequired"], false);
    assert_eq!(
        status(
            Client::new(&server.base)
                .login(&LoginRequest {
                    username: "admin".into(),
                    password: "correct horse battery".into(),
                    remember_me: false,
                })
                .await
                .unwrap_err()
        ),
        403
    );

    // The authorization request: code flow with PKCE S256, nonce, state, Tanukarr's
    // redirect URI.
    let (authorize, cookie) = begin_sso(&server, "/series/7").await;
    assert_eq!(authorize.path(), "/authorize");
    assert_eq!(param(&authorize, "client_id"), "kitsunarr");
    assert_eq!(param(&authorize, "response_type"), "code");
    assert_eq!(param(&authorize, "scope"), "openid profile email");
    assert_eq!(param(&authorize, "code_challenge_method"), "S256");
    assert!(!param(&authorize, "nonce").is_empty());

    // The provider sends the user back: verified e-mail matches the configured user.
    let back = finish_sso(&server, &provider, &authorize, &cookie, "fox-1", true).await;
    assert_eq!(location(&back), "/series/7");
    let session = set_cookie(&back, "kitsunarr_session").expect("signed in");
    let st = auth_status(&server, Some(&session)).await;
    assert_eq!(st["authenticated"], true);
    assert_eq!(st["username"], "fox@example.com");
    let stats = raw()
        .get(format!("{}/api/v1/library/stats", server.base))
        .header("cookie", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(stats.status(), 200);
    let form = provider.token_requests.lock().unwrap()[0].clone();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["client_id"], "kitsunarr");

    // A state is used once.
    let replay = finish_sso(&server, &provider, &authorize, &cookie, "fox-1", true).await;
    assert_eq!(location(&replay), "/login?ssoFailed=true");
    // Another browser (no correlation cookie) cannot finish someone's sign-in.
    let (authorize2, _) = begin_sso(&server, "/").await;
    let stolen = finish_sso(
        &server,
        &provider,
        &authorize2,
        "kitsunarr_oidc=forged",
        "fox-1",
        true,
    )
    .await;
    assert_eq!(location(&stolen), "/login?ssoFailed=true");
    assert!(set_cookie(&stolen, "kitsunarr_session").is_none());
    // A user whose e-mail is not verified (and whose subject is not the configured user) is
    // refused.
    let (authorize3, cookie3) = begin_sso(&server, "/").await;
    let stranger = finish_sso(&server, &provider, &authorize3, &cookie3, "mallory", false).await;
    assert_eq!(location(&stranger), "/login?ssoFailed=true");
    // A token signed by another key with the same key id is refused (the keys Kitsunarr
    // read from the provider do not verify it).
    let (authorize4, cookie4) = begin_sso(&server, "/").await;
    let real_key = provider.key();
    *provider.key.lock().unwrap() = Some(Arc::new(
        kitsunarr_auth::oidc::testing::TestKey::generate("key-1"),
    ));
    let bad_sig = finish_sso(&server, &provider, &authorize4, &cookie4, "fox-1", true).await;
    assert_eq!(location(&bad_sig), "/login?ssoFailed=true");
    *provider.key.lock().unwrap() = Some(real_key);

    // The Sonarr v3 host resource reports the real values, the secret masked.
    let host: Value = raw()
        .get(format!("{}/api/v3/config/host", server.base))
        .header("x-api-key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(host["authenticationMethod"], "oidc");
    assert_eq!(host["oidcClientSecret"], "********");
    assert_eq!(host["oidcUserIdentifier"], "fox@example.com");
    assert_eq!(host["username"], "admin");

    // Signing out goes through the provider's end-session endpoint and back to /loggedout.
    let out = raw()
        .get(format!("{}/logout", server.base))
        .header("cookie", &session)
        .send()
        .await
        .unwrap();
    let end = url::Url::parse(&location(&out)).unwrap();
    assert_eq!(end.path(), "/logout");
    assert_eq!(
        param(&end, "post_logout_redirect_uri"),
        format!("{}/signout-callback-oidc", server.base)
    );
    assert!(!param(&end, "id_token_hint").is_empty());
    assert_eq!(
        auth_status(&server, Some(&session)).await["authenticated"],
        false
    );
    let back = raw()
        .get(format!("{}/signout-callback-oidc", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(location(&back), "/loggedout");

    // No OIDC health problem while the provider answers.
    assert!(
        !keyed
            .health()
            .await
            .unwrap()
            .iter()
            .any(|h| h.source == "OidcConfigurationCheck")
    );

    // Back to forms through Sonarr's v3 API: `basic` is refused as Tanukarr does.
    let basic = raw()
        .put(format!("{}/api/v3/config/host/1", server.base))
        .header("x-api-key", &server.api_key)
        .json(&json!({"id": 1, "authenticationMethod": "basic"}))
        .send()
        .await
        .unwrap();
    assert!(basic.status().is_client_error());
    assert!(basic.text().await.unwrap().contains("no longer supported"));
    let forms = raw()
        .put(format!("{}/api/v3/config/host/1", server.base))
        .header("x-api-key", &server.api_key)
        .json(
            &json!({"id": 1, "authenticationMethod": "forms", "authenticationRequired": "enabled"}),
        )
        .send()
        .await
        .unwrap();
    assert!(forms.status().is_success(), "{}", forms.status());
    let sso_now = raw()
        .get(format!("{}/login/sso", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(location(&sso_now), "/login", "SSO is off with forms");
    Client::new(&server.base)
        .login(&LoginRequest {
            username: "admin".into(),
            password: "correct horse battery".into(),
            remember_me: false,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn misconfigured_oidc_keeps_password_sign_in_and_is_reported() {
    // Pinned in the configuration without a client secret: OIDC is not usable.
    let server = start_with(|c| {
        c.auth.method = Some(kitsunarr_config::AuthMethod::Oidc);
        c.auth.oidc_authority = Some("https://idp.example".into());
        c.auth.oidc_client_id = Some("kitsunarr".into());
        c.auth.oidc_user_identifier = Some("fox@example.com".into());
    })
    .await;
    let st = auth_status(&server, None).await;
    assert_eq!(st["authenticationMethod"], "forms");
    assert_eq!(st["oidcMisconfigured"], true);
    assert_eq!(
        st["setupRequired"], false,
        "setup only with forms configured"
    );
    let keyed = Client::new(&server.base).with_api_key(&server.api_key);
    let health = keyed.health().await.unwrap();
    let check = health
        .iter()
        .find(|h| h.source == "OidcConfigurationCheck")
        .unwrap();
    assert!(check.message.contains("not all configured"));
    let sso = raw()
        .get(format!("{}/login/sso", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(location(&sso), "/login");
}

// ---- sign-in flows for AniList lists and connections ----------------------------------------

#[tokio::test]
async fn anilist_and_trakt_connection_sign_in_through_the_api() {
    let server = start().await;
    let client = admin(&server).await;
    // AniList: start, AniList calls Kitsunarr's callback, the next poll exchanges the code.
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let log = seen.clone();
    let anilist = axum::Router::new().route(
        "/token",
        post(move |axum::Json(b): axum::Json<Value>| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(b);
                axum::Json(json!({"access_token": "anilist-token", "token_type": "Bearer", "expires_in": 31536000}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let anilist_base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, anilist).await.unwrap() });
    let callback = format!("{}/api/v1/importlists/oauth/callback", server.base);
    let settings = json!({"username": "fox", "clientId": "77", "clientSecret": "anilist-secret",
        "tokenUrl": format!("{anilist_base}/token")});
    let started: Value = client
        .post(
            "/providers/importlists/signin/start",
            &json!({"implementation": "AniListImport", "settings": settings, "callback": callback}),
        )
        .await
        .unwrap();
    let url = url::Url::parse(started["url"].as_str().unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("anilist.co"));
    assert_eq!(param(&url, "redirect_uri"), callback);
    let nonce = param(&url, "state");
    let pending: Value = client
        .post(
            "/providers/importlists/signin/poll",
            &json!({"implementation": "AniListImport", "settings": settings, "state": started["state"]}),
        )
        .await
        .unwrap();
    assert_eq!(pending["done"], false);
    let page = raw()
        .get(format!("{callback}?code=anilist-code&state={nonce}"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let done: Value = client
        .post(
            "/providers/importlists/signin/poll",
            &json!({"implementation": "AniListImport", "settings": settings, "state": started["state"]}),
        )
        .await
        .unwrap();
    assert_eq!(done["done"], true);
    assert_eq!(done["settings"]["accessToken"], "anilist-token");
    assert_eq!(seen.lock().unwrap()[0]["code"], "anilist-code");

    // The Trakt connection reuses its stored secret (a notification, not a list).
    let trakt = axum::Router::new()
        .route(
            "/oauth/device/code",
            post(|| async {
                axum::Json(json!({"device_code": "dev", "user_code": "ABCD", "verification_url": "https://trakt.tv/activate", "expires_in": 600, "interval": 5}))
            }),
        )
        .route(
            "/oauth/device/token",
            post(|axum::Json(b): axum::Json<Value>| async move {
                assert_eq!(b["client_secret"], "stored-trakt-secret");
                axum::Json(json!({"access_token": "trakt-conn", "refresh_token": "r", "expires_in": 600}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let trakt_base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, trakt).await.unwrap() });
    let created: Value = client
        .post(
            "/providers/notifications",
            &json!({"name": "Trakt", "implementation": "Trakt", "options": {"onDownload": true},
                "settings": {"clientId": "cid", "clientSecret": "stored-trakt-secret", "accessToken": "", "baseUrl": trakt_base}}),
        )
        .await
        .unwrap();
    let id = created["id"].as_i64().unwrap();
    let redacted = json!({"clientId": "cid", "clientSecret": "********", "accessToken": "", "baseUrl": trakt_base});
    let started: Value = client
        .post(
            "/providers/importlists/signin/start",
            &json!({"id": id, "implementation": "Trakt", "settings": redacted}),
        )
        .await
        .unwrap();
    assert_eq!(started["code"], "ABCD");
    let done: Value = client
        .post(
            "/providers/importlists/signin/poll",
            &json!({"id": id, "implementation": "Trakt", "settings": redacted, "state": started["state"]}),
        )
        .await
        .unwrap();
    assert_eq!(done["settings"]["accessToken"], "trakt-conn");
}

// ---- custom formats (Sonarr v3) -------------------------------------------------------------

#[tokio::test]
async fn v3_custom_formats_carry_alternative_groups_and_ignored_episode_titles() {
    let server = start().await;
    let http = raw();
    let v3 = |path: &str| format!("{}/api/v3/{path}", server.base);
    let schema: Value = http
        .get(v3("customformat/schema"))
        .header("x-api-key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let title = schema
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["implementation"] == "ReleaseTitleSpecification")
        .unwrap();
    let names: Vec<&str> = title["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["value", "excludeEpisodeTitles", "alternativeGroup"]
    );
    let group = &title["fields"][2];
    assert_eq!(
        (group["order"].as_i64(), group["value"].as_i64()),
        (Some(100), Some(0))
    );
    assert_eq!(group["isFloat"], false);
    assert_eq!(title["fields"][1]["value"], false);

    let created: Value = http
        .post(v3("customformat"))
        .header("x-api-key", &server.api_key)
        .json(&json!({
            "name": "HEVC or GRP",
            "includeCustomFormatWhenRenaming": false,
            "specifications": [
                {"name": "1080p", "implementation": "ReleaseTitleSpecification", "negate": false, "required": false,
                 "fields": [{"name": "value", "value": "1080p"}, {"name": "alternativeGroup", "value": 0}]},
                {"name": "x265", "implementation": "ReleaseTitleSpecification", "negate": false, "required": false,
                 "fields": [{"name": "value", "value": "x265"}, {"name": "excludeEpisodeTitles", "value": true},
                            {"name": "alternativeGroup", "value": 1}]},
                {"name": "GRP", "implementation": "ReleaseGroupSpecification", "negate": false, "required": false,
                 "fields": [{"name": "value", "value": "^GRP$"}, {"name": "alternativeGroup", "value": 2}]}
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let field = |spec: usize, name: &str| {
        created["specifications"][spec]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .map(|f| f["value"].clone())
            .unwrap()
    };
    assert_eq!(field(1, "excludeEpisodeTitles"), true);
    assert_eq!(field(1, "alternativeGroup"), 1);
    assert_eq!(field(2, "alternativeGroup"), 2);
    // The native API holds the same values, and the format tests as A AND (B OR C).
    let client = Client::new(&server.base).with_api_key(&server.api_key);
    let formats: Vec<kitsunarr_domain::api::settings::CustomFormat> =
        client.get("/customformats").await.unwrap();
    let format = formats.iter().find(|f| f.name == "HEVC or GRP").unwrap();
    assert_eq!(format.specifications[2].alternative_group(), 2);
    let test = |release: &str| {
        let req = kitsunarr_domain::api::settings::CustomFormatTestRequest {
            format: format.clone(),
            release_title: release.into(),
            file_name: None,
            size: 0,
            indexer_flags: 0,
            languages: None,
            original_language: None,
            quality_id: None,
            release_type: None,
            episode_titles: vec!["The x265 Story".into()],
        };
        let client = &client;
        async move { client.test_custom_format(&req).await.unwrap().matches }
    };
    assert!(test("Show.S01E01.1080p.WEB.x265-OTHER").await);
    assert!(test("Show.S01E01.1080p.WEB.x264-GRP").await);
    assert!(!test("Show.S01E01.720p.WEB.x265-GRP").await);
    assert!(!test("Show.S01E01.The.x265.Story.1080p.WEB.x264-OTHER").await);
}
