//! Flood client against a stand-in of its REST API: the session cookie (renewed when it
//! expires), adds with tags, the tag filter, a finished torrent's files, post-import tags and
//! removal.

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use kitsunarr_downloaders::{
    AddRequest, ClientSettings, Clients, DownloadError, ItemStatus, ReleaseInfo, Source,
};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct Mock {
    calls: Vec<String>,
    logins: usize,
    /// The current valid session.
    session: String,
}

type Shared = Arc<Mutex<Mock>>;

fn authorised(m: &Mock, headers: &HeaderMap) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .is_some_and(|c| c == format!("jwt={}", m.session))
}

async fn authenticate(State(m): State<Shared>, body: String) -> Response {
    let v: Value = serde_json::from_str(&body).unwrap();
    let mut m = m.lock().unwrap();
    if v["password"] != "secret" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    m.logins += 1;
    m.session = format!("s{}", m.logins);
    (
        [(
            header::SET_COOKIE,
            format!("jwt={}; Path=/; HttpOnly", m.session),
        )],
        axum::Json(json!({"success": true})),
    )
        .into_response()
}

async fn any(
    State(m): State<Shared>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    body: String,
) -> Response {
    let mut m = m.lock().unwrap();
    if !authorised(&m, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    m.calls.push(format!("{} {body}", uri.path()));
    match uri.path() {
        "/api/auth/verify" => axum::Json(json!({"username": "admin"})).into_response(),
        "/api/torrents" => axum::Json(json!({"id": 1, "torrents": {
            "HASH1": {"name": "Droned.S01E01", "directory": "/downloads", "bytesDone": 1000, "sizeBytes": 1000,
                "eta": -1, "ratio": 2.0, "status": ["seeding", "complete"], "tags": ["kitsunarr"], "dateFinished": 1700000000},
            "HASH2": {"name": "Other", "directory": "/downloads", "bytesDone": 0, "sizeBytes": 10,
                "eta": 5, "ratio": 0.0, "status": ["downloading"], "tags": ["movies"]},
            "HASH3": {"name": "Imported", "directory": "/downloads", "bytesDone": 10, "sizeBytes": 10,
                "eta": 0, "ratio": 0.0, "status": ["seeding"], "tags": ["kitsunarr", "imported"]}
        }}))
        .into_response(),
        _ => axum::Json(json!({})).into_response(),
    }
}

async fn contents(
    State(m): State<Shared>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Response {
    let m = m.lock().unwrap();
    if !authorised(&m, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert_eq!(hash, "HASH1");
    axum::Json(json!([{"path": "Droned.S01E01/episode.mkv"}, {"path": "Droned.S01E01/sample.mkv"}]))
        .into_response()
}

async fn mock() -> (u16, Shared) {
    let shared: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/api/auth/authenticate", post(authenticate))
        .route("/api/torrents/{hash}/contents", get(contents))
        .route("/api/torrents/tags", patch(any))
        .route("/api/torrents/add-files", post(any))
        .route("/api/torrents/add-urls", post(any))
        .route("/api/torrents/delete", post(any))
        .route("/api/torrents", get(any))
        .route("/api/auth/verify", get(any))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, shared)
}

fn settings(port: u16, password: &str) -> ClientSettings {
    let map: Map<String, Value> = serde_json::from_value(json!({
        "host": "127.0.0.1", "port": port, "username": "admin", "password": password,
        "tags": ["kitsunarr"], "postImportTags": ["imported"], "additionalTags": [0, 1, 5],
        "destination": "/downloads"
    }))
    .unwrap();
    ClientSettings::from_provider("Flood", &map).unwrap()
}

#[tokio::test]
async fn flood_adds_lists_marks_and_removes() {
    let (port, m) = mock().await;
    let clients = Clients::new(reqwest::Client::new());
    assert_eq!(
        clients.test(&settings(port, "wrong")).await.unwrap_err(),
        DownloadError::Authentication
    );
    let s = settings(port, "secret");
    assert_eq!(
        clients.test(&s).await.unwrap().message,
        "Connected to Flood."
    );

    let info = ReleaseInfo {
        title_slug: "droned".into(),
        quality: "WEBDL-1080p".into(),
        indexer: "Tracker".into(),
        ..Default::default()
    };
    let id = clients
        .add(
            &s,
            &AddRequest {
                source: Source::File {
                    name: "x.torrent",
                    content: b"d4:infoe",
                },
                title: "Droned.S01E01",
                release: Some(&info),
                hash: "hash1",
                recent: false,
                tags: &[],
                seed: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(id, "HASH1");
    let added = m.lock().unwrap().calls.last().cloned().unwrap();
    let body: Value =
        serde_json::from_str(added.trim_start_matches("/api/torrents/add-files ")).unwrap();
    assert_eq!(body["files"], json!(["ZDQ6aW5mb2U="]));
    assert_eq!(
        body["tags"],
        json!(["kitsunarr", "droned", "WEBDL-1080p", "Tracker"])
    );
    assert_eq!(body["destination"], "/downloads");
    assert_eq!(body["start"], true);

    // An expired session is renewed once.
    m.lock().unwrap().session = "expired".into();
    let items = clients.items(&s).await.unwrap();
    assert_eq!(
        items.len(),
        1,
        "other tags and imported torrents are skipped: {items:?}"
    );
    assert_eq!(items[0].status, ItemStatus::Completed);
    assert_eq!(
        items[0].output_path.as_deref(),
        Some("/downloads/Droned.S01E01")
    );
    assert!(items[0].seed_progress.is_some());

    clients.mark_imported(&s, "HASH1").await.unwrap();
    assert!(m.lock().unwrap().calls.contains(
        &r#"/api/torrents/tags {"hashes":["HASH1"],"tags":["kitsunarr","imported"]}"#.to_string()
    ));
    clients.remove(&s, &items[0], true).await.unwrap();
    assert!(
        m.lock().unwrap().calls.contains(
            &r#"/api/torrents/delete {"deleteData":true,"hashes":["HASH1"]}"#.to_string()
        )
    );
}
