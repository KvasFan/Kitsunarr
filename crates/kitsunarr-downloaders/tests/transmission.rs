//! Transmission client against a mock RPC endpoint: the 409 session handshake, basic
//! authentication, labels (Transmission 4) and category folders (older versions).

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use kitsunarr_downloaders::transmission::Settings;
use kitsunarr_downloaders::{DownloadError, ItemStatus, Source, Transmission};
use serde_json::{Value, json};

struct Mock {
    version: &'static str,
    session: &'static str,
    handshakes: usize,
    calls: Vec<String>,
    torrents: Vec<Value>,
}

type Shared = Arc<Mutex<Mock>>;

async fn rpc(State(m): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let mut m = m.lock().unwrap();
    // "admin:secret"
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some("Basic YWRtaW46c2VjcmV0")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if headers
        .get("X-Transmission-Session-Id")
        .and_then(|v| v.to_str().ok())
        != Some(m.session)
    {
        m.handshakes += 1;
        return (
            StatusCode::CONFLICT,
            [("X-Transmission-Session-Id", m.session)],
        )
            .into_response();
    }
    let req: Value = serde_json::from_str(&body).unwrap();
    let method = req["method"].as_str().unwrap().to_string();
    let args = req.get("arguments").cloned().unwrap_or(Value::Null);
    if method != "session-get" {
        m.calls.push(format!("{method} {args}"));
    }
    let arguments = match method.as_str() {
        "session-get" => json!({ "version": m.version, "download-dir": "/downloads" }),
        "torrent-get" => {
            let ids = args.get("ids").cloned();
            let list: Vec<Value> = m
                .torrents
                .iter()
                .filter(|t| {
                    ids.as_ref()
                        .is_none_or(|ids| ids.as_array().unwrap().contains(&t["hashString"]))
                })
                .cloned()
                .collect();
            json!({ "torrents": list })
        }
        _ => json!({}),
    };
    axum::Json(json!({ "result": "success", "arguments": arguments })).into_response()
}

async fn mock(version: &'static str) -> (Settings, Shared) {
    let shared = Arc::new(Mutex::new(Mock {
        version,
        session: "abc123",
        handshakes: 0,
        calls: Vec::new(),
        torrents: Vec::new(),
    }));
    let app = axum::Router::new()
        .route("/transmission/rpc", post(rpc))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let settings = Settings::from_map(
        json!({
            "host": "127.0.0.1", "port": port, "username": "admin", "password": "secret",
            "tvCategory": "tv-kitsunarr", "tvImportedCategory": "tv-imported",
            "recentTvPriority": 1
        })
        .as_object()
        .unwrap(),
    )
    .unwrap();
    (settings, shared)
}

fn torrent(hash: &str, dir: &str, labels: &[&str], status: i64, left: i64) -> Value {
    json!({
        "hashString": hash, "name": format!("Show {hash}"), "downloadDir": dir,
        "totalSize": 100, "leftUntilDone": left, "isFinished": false, "eta": 60,
        "status": status, "errorString": "", "uploadedEver": 50, "downloadedEver": 100,
        "labels": labels
    })
}

#[tokio::test]
async fn transmission_4_uses_labels() {
    let (s, mock) = mock("4.0.5 (a6fe2a64aa)").await;
    let t = Transmission::new(reqwest::Client::new());
    let report = t.test(&s).await.unwrap();
    assert_eq!(report.version, "4.0.5 (a6fe2a64aa)");
    assert_eq!(
        mock.lock().unwrap().handshakes,
        1,
        "the session id is reused"
    );

    let magnet = "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
    let id = t
        .add(
            &s,
            &Source::Magnet(magnet),
            "c12fe1c06bba254a9dc9f519b335aa7c1367a88a",
            true,
            Some(kitsunarr_downloaders::SeedConfig {
                ratio: Some(1.5),
                seed_time_minutes: Some(60),
            }),
        )
        .await
        .unwrap();
    assert_eq!(id, "C12FE1C06BBA254A9DC9F519B335AA7C1367A88A");
    {
        let m = mock.lock().unwrap();
        let n = m.calls.len();
        // The seed criteria: the ratio, and the seed time as the idle limit.
        assert_eq!(
            m.calls[n - 2],
            "torrent-set {\"ids\":[\"c12fe1c06bba254a9dc9f519b335aa7c1367a88a\"],\"seedIdleLimit\":60,\"seedIdleMode\":1,\"seedRatioLimit\":1.5,\"seedRatioMode\":1}"
        );
        assert_eq!(
            m.calls[n - 3],
            format!(
                "torrent-add {{\"filename\":\"{magnet}\",\"labels\":[\"tv-kitsunarr\"],\"paused\":false}}"
            )
        );
        assert_eq!(
            m.calls[n - 1],
            "queue-move-top {\"ids\":[\"c12fe1c06bba254a9dc9f519b335aa7c1367a88a\"]}"
        );
    }

    mock.lock().unwrap().torrents = vec![
        torrent("aa", "/downloads", &["tv-kitsunarr"], 6, 0),
        torrent("bb", "/downloads", &["movies"], 6, 0),
        // From Transmission 3, before labels: matched by its folder.
        torrent("cc", "/downloads/tv-kitsunarr", &[], 4, 40),
    ];
    let items = t.items(&s).await.unwrap();
    let ids: Vec<(&str, ItemStatus)> = items
        .iter()
        .map(|i| (i.download_id.as_str(), i.status))
        .collect();
    assert_eq!(
        ids,
        [
            ("AA", ItemStatus::Completed),
            ("CC", ItemStatus::Downloading)
        ]
    );
    assert_eq!(items[0].output_path.as_deref(), Some("/downloads/Show aa"));
    assert_eq!(items[0].seed_ratio, 0.5);

    t.mark_imported(&s, "AA").await.unwrap();
    assert_eq!(
        mock.lock().unwrap().calls.last().unwrap(),
        "torrent-set {\"ids\":[\"aa\"],\"labels\":[\"tv-imported\"]}"
    );
}

#[tokio::test]
async fn older_transmission_uses_the_category_folder() {
    let (s, mock) = mock("3.00 (bb6b5a062e)").await;
    let t = Transmission::new(reqwest::Client::new());
    t.add(
        &s,
        &Source::File {
            name: "x.torrent",
            content: b"d4:infod4:name1:xee",
        },
        "aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44",
        false,
        None,
    )
    .await
    .unwrap();
    let add = mock.lock().unwrap().calls.last().unwrap().clone();
    assert!(
        add.contains("\"download-dir\":\"/downloads/tv-kitsunarr\""),
        "{add}"
    );
    mock.lock().unwrap().torrents = vec![
        torrent("aa", "/downloads/tv-kitsunarr", &[], 6, 0),
        torrent("bb", "/downloads/other", &[], 6, 0),
    ];
    let items = t.items(&s).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].download_id, "AA");
    // No labels before Transmission 4: nothing to change after import.
    let before = mock.lock().unwrap().calls.len();
    t.mark_imported(&s, "AA").await.unwrap();
    assert_eq!(mock.lock().unwrap().calls.len(), before);
}

#[tokio::test]
async fn bad_credentials_and_old_versions_are_reported() {
    let (mut s, _) = mock("2.33 (14714)").await;
    let t = Transmission::new(reqwest::Client::new());
    let err = t.test(&s).await.unwrap_err();
    assert!(err.to_string().contains("2.40 or newer"), "{err}");
    s.password = Some("wrong".into());
    assert_eq!(t.test(&s).await.unwrap_err(), DownloadError::Authentication);
}
