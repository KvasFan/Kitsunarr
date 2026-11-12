//! Signing in to Simkl, Trakt, Plex and MyAnimeList against local stand-ins of their OAuth
//! endpoints.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use kitsunarr_importlists::oauth::{Polled, poll, start};
use serde_json::{Map, Value, json};

/// How many polls each service answers "not yet" before handing over the token.
#[derive(Clone, Default)]
struct Mock {
    polls: Arc<Mutex<u32>>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    fn tick(&self) -> u32 {
        let mut p = self.polls.lock().unwrap();
        *p += 1;
        *p
    }
}

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn settings(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

async fn until_done(
    http: &reqwest::Client,
    implementation: &str,
    s: &Map<String, Value>,
    state: &str,
    code: Option<&str>,
) -> Map<String, Value> {
    for _ in 0..5 {
        match poll(http, implementation, s, state, code).await.unwrap() {
            Polled::Done(m) => return m,
            Polled::Pending => {}
        }
    }
    panic!("never signed in");
}

#[tokio::test]
async fn simkl_pin() {
    let mock = Mock::default();
    let app = axum::Router::new()
        .route(
            "/oauth/pin",
            get(|| async {
                Json(json!({"result": "OK", "user_code": "AB12C", "verification_url": "https://simkl.com/pin", "expires_in": 900, "interval": 5}))
            }),
        )
        .route(
            "/oauth/pin/{code}",
            get(|State(m): State<Mock>, Path(code): Path<String>| async move {
                assert_eq!(code, "AB12C");
                if m.tick() < 2 {
                    Json(json!({"result": "KO", "message": "Authorization pending"}))
                } else {
                    Json(json!({"result": "OK", "access_token": "simkl-token"}))
                }
            }),
        )
        .with_state(mock);
    let base = serve(app).await;
    let http = reqwest::Client::new();
    let s = settings(json!({"clientIdV2": "cid", "baseUrl": base}));
    let started = start(&http, "SimklUserImport", &s, "").await.unwrap();
    assert_eq!(started.code.as_deref(), Some("AB12C"));
    assert_eq!(started.url, "https://simkl.com/pin");
    let done = until_done(&http, "SimklUserImport", &s, &started.state, None).await;
    assert_eq!(done["accessToken"], "simkl-token");
    // Without a Client ID there is nothing to start.
    assert!(
        start(&http, "SimklUserImport", &Map::new(), "")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn trakt_device_code() {
    let mock = Mock::default();
    let app = axum::Router::new()
        .route(
            "/oauth/device/code",
            post(|Json(b): Json<Value>| async move {
                assert_eq!(b["client_id"], "cid");
                Json(json!({"device_code": "dev", "user_code": "XY12", "verification_url": "https://trakt.tv/activate", "expires_in": 600, "interval": 5}))
            }),
        )
        .route(
            "/oauth/device/token",
            post(|State(m): State<Mock>, Json(b): Json<Value>| async move {
                assert_eq!((b["code"].as_str(), b["client_secret"].as_str()), (Some("dev"), Some("secret")));
                if m.tick() < 2 {
                    StatusCode::BAD_REQUEST.into_response()
                } else {
                    Json(json!({"access_token": "trakt-token", "refresh_token": "r", "expires_in": 7776000})).into_response()
                }
            }),
        )
        .with_state(mock);
    let base = serve(app).await;
    let http = reqwest::Client::new();
    let s = settings(json!({"clientId": "cid", "clientSecret": "secret", "baseUrl": base}));
    let started = start(&http, "TraktUserImport", &s, "").await.unwrap();
    assert_eq!(started.code.as_deref(), Some("XY12"));
    let done = until_done(&http, "TraktUserImport", &s, &started.state, None).await;
    assert_eq!(done["accessToken"], "trakt-token");
    assert_eq!(done["refreshToken"], "r");
    assert!(done["expires"].as_str().is_some_and(|e| e.len() >= 19));
    // The device token needs the application's secret.
    let no_secret = settings(json!({"clientId": "cid", "baseUrl": base}));
    assert!(
        poll(&http, "TraktUserImport", &no_secret, &started.state, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn plex_pin() {
    let mock = Mock::default();
    let app = axum::Router::new()
        .route(
            "/api/v2/pins",
            post(
                |State(m): State<Mock>, headers: axum::http::HeaderMap| async move {
                    m.seen.lock().unwrap().push(
                        headers["x-plex-client-identifier"]
                            .to_str()
                            .unwrap()
                            .to_string(),
                    );
                    Json(json!({"id": 42, "code": "plexcode", "expiresIn": 1800}))
                },
            ),
        )
        .route(
            "/api/v2/pins/{id}",
            get(|State(m): State<Mock>, Path(id): Path<i64>| async move {
                assert_eq!(id, 42);
                if m.tick() < 2 {
                    Json(json!({"id": 42, "authToken": null}))
                } else {
                    Json(json!({"id": 42, "authToken": "plex-token"}))
                }
            }),
        )
        .with_state(mock.clone());
    let base = serve(app).await;
    let http = reqwest::Client::new();
    let s = settings(json!({"authBaseUrl": base, "clientIdentifier": "kitsunarr-test"}));
    let started = start(&http, "PlexImport", &s, "").await.unwrap();
    assert!(
        started
            .url
            .starts_with("https://app.plex.tv/auth#?clientID=kitsunarr-test&code=plexcode"),
        "{}",
        started.url
    );
    assert_eq!(mock.seen.lock().unwrap().as_slice(), ["kitsunarr-test"]);
    let done = until_done(&http, "PlexImport", &s, &started.state, None).await;
    assert_eq!(done["accessToken"], "plex-token");
}

#[tokio::test]
async fn myanimelist_pkce_through_the_callback() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    let app = axum::Router::new().route(
        "/token",
        post(move |body: String| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(body);
                Json(json!({"access_token": "mal-token", "refresh_token": "mal-refresh", "expires_in": 2678400}))
            }
        }),
    );
    let base = serve(app).await;
    let http = reqwest::Client::new();
    let s = settings(json!({"clientId": "cid", "tokenUrl": format!("{base}/token")}));
    let callback = "https://kitsunarr.example/api/v1/importlists/oauth/callback";
    let started = start(&http, "MyAnimeListImport", &s, callback)
        .await
        .unwrap();
    let state: Value = serde_json::from_str(&started.state).unwrap();
    let verifier = state["verifier"].as_str().unwrap();
    assert!((43..=128).contains(&verifier.len()));
    assert!(started.url.starts_with("https://myanimelist.net/v1/oauth2/authorize?response_type=code&client_id=cid&code_challenge="));
    assert!(started.url.contains("code_challenge_method=plain"));
    assert!(
        started
            .url
            .contains(&format!("state={}", state["nonce"].as_str().unwrap()))
    );
    assert!(started.url.contains(
        "redirect_uri=https%3A%2F%2Fkitsunarr.example%2Fapi%2Fv1%2Fimportlists%2Foauth%2Fcallback"
    ));
    // Before the callback: pending.
    assert_eq!(
        poll(&http, "MyAnimeListImport", &s, &started.state, None)
            .await
            .unwrap(),
        Polled::Pending
    );
    let done = until_done(
        &http,
        "MyAnimeListImport",
        &s,
        &started.state,
        Some("authcode"),
    )
    .await;
    assert_eq!(done["accessToken"], "mal-token");
    assert_eq!(done["refreshToken"], "mal-refresh");
    let body = seen.lock().unwrap()[0].clone();
    assert!(body.contains("grant_type=authorization_code"), "{body}");
    assert!(body.contains("code=authcode"), "{body}");
    assert!(
        body.contains(&format!("code_verifier={verifier}")),
        "{body}"
    );
}
