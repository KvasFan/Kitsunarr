//! rTorrent client against a stand-in XML-RPC endpoint: basic authentication, the version
//! check, adds with label, priority and directory commands, waiting for the torrent, the
//! category filter, the post-import label and view, and removal with the data.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use kitsunarr_downloaders::rtorrent::IMPORTED_VIEW;
use kitsunarr_downloaders::{
    AddRequest, ClientSettings, Clients, DownloadError, ItemStatus, Source,
};
use serde_json::{Map, json};

#[derive(Default)]
struct Mock {
    calls: Vec<String>,
    added: bool,
}

type Shared = Arc<Mutex<Mock>>;

fn ok(value: &str) -> Response {
    (
        [(header::CONTENT_TYPE, "text/xml")],
        format!(
            "<?xml version=\"1.0\"?><methodResponse><params><param><value>{value}</value></param></params></methodResponse>"
        ),
    )
        .into_response()
}

fn row(name: &str, hash: &str, path: &str, label: &str, finished: bool) -> String {
    let s = |v: &str| format!("<value><string>{v}</string></value>");
    let i = |v: i64| format!("<value><i8>{v}</i8></value>");
    format!(
        "<value><array><data>{}{}{}{}{}{}{}{}{}{}{}{}</data></array></value>",
        s(name),
        s(hash),
        s(path),
        s(label),
        i(1000),
        i(if finished { 0 } else { 400 }),
        i(if finished { 0 } else { 100 }),
        i(2500),
        i(1),
        i(1),
        i(i64::from(finished)),
        i(if finished { 1_700_000_000 } else { 0 })
    )
}

async fn rpc(State(m): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    // "admin:secret"
    if auth != "Basic YWRtaW46c2VjcmV0" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let method = body
        .split("<methodName>")
        .nth(1)
        .and_then(|r| r.split("</methodName>").next())
        .unwrap_or_default()
        .to_string();
    let strings: Vec<String> = body
        .split("<string>")
        .skip(1)
        .filter_map(|p| p.split("</string>").next())
        .map(str::to_string)
        .collect();
    let mut m = m.lock().unwrap();
    m.calls.push(format!("{method} {}", strings.join(" ")));
    match method.as_str() {
        "system.client_version" => ok("<string>0.9.8</string>"),
        "load.raw_start" | "load.start" => {
            assert!(body.contains("<base64>") || method == "load.start");
            m.added = true;
            ok("<i4>0</i4>")
        }
        "d.name" => {
            if m.added {
                ok("<string>Droned.S01E01.Pilot.1080p.WEB-DL-DRONE</string>")
            } else {
                ok(&format!("<string>{}.meta</string>", strings[0]))
            }
        }
        "d.multicall2" => ok(&format!(
            "<array><data>{}{}{}</data></array>",
            row(
                "Droned.S01E01.Pilot.1080p.WEB-DL-DRONE",
                "HASH1",
                "/downloads/Droned.S01E01",
                "tv-kitsunarr",
                true
            ),
            row("Other", "HASH2", "/downloads/Other", "movies", false),
            row("NoPath", "HASH3", "", "tv-kitsunarr", false)
        )),
        "d.custom1.set" => ok(&format!("<string>{}</string>", strings[1])),
        "d.views.push_back_unique" | "d.erase" => ok("<i4>0</i4>"),
        other => panic!("unexpected {other}"),
    }
}

async fn mock() -> (String, Shared) {
    let shared: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/RPC2", post(rpc))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, shared)
}

fn settings(port: &str, password: &str) -> ClientSettings {
    let map: Map<String, serde_json::Value> = serde_json::from_value(json!({
        "host": "127.0.0.1", "port": port.parse::<u16>().unwrap(), "username": "admin",
        "password": password, "tvCategory": "tv-kitsunarr", "tvImportedCategory": "tv-done",
        "tvDirectory": "/downloads/tv", "recentTvPriority": 3, "olderTvPriority": 2
    }))
    .unwrap();
    ClientSettings::from_provider("RTorrent", &map).unwrap()
}

#[tokio::test]
async fn rtorrent_adds_lists_marks_and_removes() {
    let (port, m) = mock().await;
    let clients = Clients::new(reqwest::Client::new());
    assert_eq!(
        clients.test(&settings(&port, "wrong")).await.unwrap_err(),
        DownloadError::Authentication
    );
    let s = settings(&port, "secret");
    assert_eq!(
        clients.test(&s).await.unwrap().message,
        "Connected to rTorrent 0.9.8."
    );

    let id = clients
        .add(
            &s,
            &AddRequest {
                title: "Droned.S01E01.Pilot.1080p.WEB-DL-DRONE",
                source: Source::File {
                    name: "x.torrent",
                    content: b"d4:infoe",
                },
                hash: "abcdef",
                recent: true,
                tags: &[],
                seed: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(id, "ABCDEF");
    let calls = m.lock().unwrap().calls.clone();
    // The empty target, the file (base64, not listed) and the commands.
    assert!(
        calls.contains(
            &"load.raw_start  d.custom1.set=tv-kitsunarr d.priority.set=3 d.directory.set=/downloads/tv"
                .to_string()
        ),
        "{calls:#?}"
    );

    let items = clients.items(&s).await.unwrap();
    assert_eq!(items.len(), 1, "other labels and empty paths are skipped");
    assert_eq!(items[0].status, ItemStatus::Completed);
    assert_eq!(
        items[0].output_path.as_deref(),
        Some("/downloads/Droned.S01E01")
    );
    assert!(items[0].seed_progress.is_some());

    clients.mark_imported(&s, "HASH1").await.unwrap();
    let calls = m.lock().unwrap().calls.clone();
    assert!(calls.contains(&"d.custom1.set hash1 tv-done".to_string()));
    assert!(calls.contains(&format!("d.views.push_back_unique hash1 {IMPORTED_VIEW}")));

    // Removing with data deletes the (local) folder, then erases the torrent.
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("Droned.S01E01");
    std::fs::create_dir_all(&data).unwrap();
    let mut item = items[0].clone();
    item.output_path = Some(data.display().to_string());
    clients.remove(&s, &item, true).await.unwrap();
    assert!(!data.exists());
    assert!(
        m.lock()
            .unwrap()
            .calls
            .contains(&"d.erase HASH1".to_string())
    );
}
