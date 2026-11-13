//! µTorrent client against a stand-in Web UI: the token and its cookie (renewed on 400),
//! adds with seed goals, label, queue position and state, the incremental list (`cid`),
//! the post-import label and removal with data.

use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use kitsunarr_downloaders::{AddRequest, ClientSettings, Clients, ItemStatus, SeedConfig, Source};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct Mock {
    calls: Vec<String>,
    tokens: usize,
    lists: usize,
}

type Shared = Arc<Mutex<Mock>>;

fn torrent(hash: &str, status: i64, label: &str) -> Value {
    let mut r = vec![
        json!(hash),
        json!(status),
        json!(format!("Show.{hash}")),
        json!(1000),
        json!(1000),
        json!(0),
        json!(0),
        json!(2000),
        json!(0),
        json!(0),
        json!(0),
        json!(label),
    ];
    r.extend((12..18).map(|_| json!(0)));
    r.push(json!(0));
    r.extend((19..26).map(|_| json!("")));
    r.push(json!("/downloads"));
    Value::Array(r)
}

async fn token(State(m): State<Shared>, headers: HeaderMap) -> Response {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some("Basic YWRtaW46c2VjcmV0")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut m = m.lock().unwrap();
    m.tokens += 1;
    (
        [(header::SET_COOKIE, format!("GUID=g{}", m.tokens))],
        format!(
            "<html><div id='token' style='display:none;'>t{}</div></html>",
            m.tokens
        ),
    )
        .into_response()
}

async fn gui(
    State(m): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<Vec<(String, String)>>,
) -> Response {
    let mut m = m.lock().unwrap();
    let token = q
        .iter()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    // Only the latest token is valid (the first one "expires").
    if token != format!("t{}", m.tokens) || cookie != format!("GUID=g{}", m.tokens) || m.tokens < 2
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let rest: Vec<String> = q
        .iter()
        .filter(|(k, _)| k != "token")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    m.calls.push(rest.join("&"));
    if q.iter().any(|(k, _)| k == "list") {
        m.lists += 1;
        if q.iter().any(|(k, _)| k == "cid") {
            // Only what changed: HASH2 is gone, HASH1 is updated.
            return axum::Json(
                json!({"build": 46000, "torrentp": [torrent("HASH1", 136, "tv-kitsunarr")],
                "torrentm": ["HASH2"], "torrentc": "2"}),
            )
            .into_response();
        }
        return axum::Json(json!({"build": 46000, "torrents": [
            torrent("HASH1", 201, "tv-kitsunarr"), torrent("HASH2", 136, "tv-kitsunarr"), torrent("HASH3", 136, "movies")
        ], "torrentc": "1"}))
        .into_response();
    }
    axum::Json(json!({"build": 46000})).into_response()
}

async fn mock() -> (u16, Shared) {
    let shared: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/gui/token.html", get(token))
        .route("/gui/", get(gui).post(gui))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, shared)
}

#[tokio::test]
async fn utorrent_adds_lists_marks_and_removes() {
    let (port, m) = mock().await;
    let map: Map<String, Value> = serde_json::from_value(json!({
        "host": "127.0.0.1", "port": port, "username": "admin", "password": "secret",
        "tvCategory": "tv-kitsunarr", "tvImportedCategory": "tv-done", "recentTvPriority": 1,
        "intialState": 1
    }))
    .unwrap();
    let s = ClientSettings::from_provider("UTorrent", &map).unwrap();
    let clients = Clients::new(reqwest::Client::new());
    assert_eq!(
        clients.test(&s).await.unwrap().message,
        "Connected to uTorrent (build 46000)."
    );
    assert_eq!(
        m.lock().unwrap().tokens,
        2,
        "the first token is renewed once"
    );

    let id = clients
        .add(
            &s,
            &AddRequest {
                source: Source::Magnet("magnet:?xt=urn:btih:abc"),
                title: "Show",
                release: None,
                hash: "abc",
                recent: true,
                tags: &[],
                seed: Some(SeedConfig {
                    ratio: Some(1.5),
                    seed_time_minutes: Some(60),
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(id, "ABC");
    let calls = m.lock().unwrap().calls.clone();
    for expected in [
        "action=add-url&s=magnet:?xt=urn:btih:abc",
        "action=setprops&hash=ABC&s=seed_override&v=1&s=seed_ratio&v=1500&s=seed_time&v=3600",
        "action=setprops&hash=ABC&s=label&v=tv-kitsunarr",
        "action=queuetop&hash=ABC",
        "action=forcestart&hash=ABC",
    ] {
        assert!(
            calls.contains(&expected.to_string()),
            "{expected}: {calls:#?}"
        );
    }

    let first = clients.items(&s).await.unwrap();
    assert_eq!(first.len(), 2, "other labels are skipped");
    // The next list only sends changes: HASH2 removed, HASH1 now completed.
    let next = clients.items(&s).await.unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].download_id, "HASH1");
    assert_eq!(next[0].status, ItemStatus::Completed);
    assert!(m.lock().unwrap().calls.iter().any(|c| c == "list=1&cid=1"));

    clients.mark_imported(&s, "HASH1").await.unwrap();
    clients.remove(&s, &next[0], true).await.unwrap();
    let calls = m.lock().unwrap().calls.clone();
    assert!(calls.contains(&"action=setprops&hash=hash1&s=label&v=tv-done".to_string()));
    assert!(
        calls.contains(&"action=setprops&hash=hash1&s=label&v=tv-kitsunarr&s=label&v=".to_string())
    );
    assert!(calls.contains(&"action=removedata&hash=HASH1".to_string()));
}
