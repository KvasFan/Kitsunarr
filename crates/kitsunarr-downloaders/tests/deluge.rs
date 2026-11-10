//! Deluge client against a mock of the Web UI's JSON-RPC API: the session cookie, connecting
//! the daemon, labels, adds and the torrent list.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use base64::Engine as _;
use kitsunarr_downloaders::deluge::Settings;
use kitsunarr_downloaders::{ClientSettings, Clients, Deluge, DownloadError, ItemStatus, Source};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct Mock {
    /// Calls as `method params`, after login.
    calls: Vec<String>,
    sessions: Vec<String>,
    logins: usize,
    connected: bool,
    label_plugin: bool,
    labels: Vec<String>,
    torrents: Vec<(String, Value)>,
}

type Shared = Arc<Mutex<Mock>>;

fn rpc(id: &Value, result: Value) -> Response {
    axum::Json(json!({ "id": id, "result": result, "error": null })).into_response()
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Response {
    axum::Json(json!({ "id": id, "result": null, "error": { "message": message, "code": code } }))
        .into_response()
}

async fn json_rpc(State(m): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let req: Value = serde_json::from_str(&body).unwrap();
    let (id, method, params) = (&req["id"], req["method"].as_str().unwrap(), &req["params"]);
    let mut m = m.lock().unwrap();
    if method == "auth.login" {
        m.logins += 1;
        if params[0] == "secret" {
            let session = format!("s{}", m.logins);
            m.sessions.push(session.clone());
            return (
                [(
                    header::SET_COOKIE,
                    format!("_session_id={session}; Path=/json"),
                )],
                axum::Json(json!({ "id": id, "result": true, "error": null })),
            )
                .into_response();
        }
        return rpc(id, json!(false));
    }
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .unwrap_or_default();
    if !m
        .sessions
        .iter()
        .any(|s| cookie == format!("_session_id={s}"))
    {
        return rpc_error(id, 1, "Not authenticated");
    }
    m.calls.push(format!("{method} {params}"));
    match method {
        "web.connected" => rpc(id, json!(m.connected)),
        "web.get_hosts" => rpc(
            id,
            json!([
                ["remote", "10.0.0.9", 58846, "Offline"],
                ["local", "127.0.0.1", 58846, "Online"]
            ]),
        ),
        "web.connect" => {
            assert_eq!(params[0], "local");
            m.connected = true;
            rpc(id, json!([]))
        }
        _ if !m.connected => rpc_error(id, 2, "Not connected to a daemon"),
        "system.listMethods" => {
            let mut methods = vec![
                "daemon.get_version",
                "core.add_torrent_magnet",
                "web.update_ui",
            ];
            if m.label_plugin {
                methods.extend(["label.add", "label.get_labels", "label.set_torrent"]);
            }
            rpc(id, json!(methods))
        }
        "daemon.get_version" => rpc(id, json!("2.1.1")),
        "label.get_labels" => rpc(id, json!(m.labels)),
        "label.add" => {
            let label = params[0].as_str().unwrap().to_string();
            m.labels.push(label);
            rpc(id, Value::Null)
        }
        "label.set_torrent" => rpc(id, Value::Null),
        "core.queue_top" => rpc(id, Value::Null),
        "core.add_torrent_magnet" => rpc(id, json!("c12fe1c06bba254a9dc9f519b335aa7c1367a88a")),
        "core.add_torrent_file" => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(params[1].as_str().unwrap())
                .unwrap();
            assert_eq!(bytes, b"d4:infod4:name1:xee");
            rpc(id, json!("aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44"))
        }
        "web.update_ui" => {
            let label = params[1]["label"].as_str().map(str::to_string);
            let torrents: Map<String, Value> = m
                .torrents
                .iter()
                .filter(|(l, _)| label.as_ref().is_none_or(|x| x == l))
                .map(|(_, t)| (t["hash"].as_str().unwrap().to_string(), t.clone()))
                .collect();
            rpc(
                id,
                json!({ "connected": true, "torrents": torrents, "stats": {} }),
            )
        }
        other => rpc_error(id, 3, &format!("Unknown method {other}")),
    }
}

async fn mock_deluge() -> (Settings, Shared) {
    let shared: Shared = Arc::new(Mutex::new(Mock {
        label_plugin: true,
        ..Default::default()
    }));
    let app = axum::Router::new()
        .route("/deluge/json", post(json_rpc))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let settings = Settings::from_map(
        json!({
            "host": "127.0.0.1", "port": port, "urlBase": "/deluge/", "password": "secret",
            "tvCategory": "tv-kitsunarr", "tvImportedCategory": "tv-imported",
            "recentTvPriority": 1, "olderTvPriority": 0, "addPaused": false,
            "downloadDirectory": "/downloads/incoming"
        })
        .as_object()
        .unwrap(),
    )
    .unwrap();
    (settings, shared)
}

#[tokio::test]
async fn test_logs_in_connects_the_daemon_and_creates_labels() {
    let (s, mock) = mock_deluge().await;
    let deluge = Deluge::new(reqwest::Client::new());
    let report = deluge.test(&s).await.unwrap();
    assert_eq!(report.version, "2.1.1");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    {
        let m = mock.lock().unwrap();
        assert!(m.connected, "the local daemon was connected");
        assert_eq!(m.labels, ["tv-kitsunarr", "tv-imported"]);
        assert_eq!(m.logins, 1, "the session is reused");
    }

    // Through the shared client layer, with Sonarr's message.
    let clients = Clients::new(reqwest::Client::new());
    let tested = clients.test(&ClientSettings::Deluge(s)).await.unwrap();
    assert_eq!(tested.message, "Connected to Deluge 2.1.1.");
}

#[tokio::test]
async fn wrong_password_and_missing_label_plugin_are_reported() {
    let (mut s, mock) = mock_deluge().await;
    let deluge = Deluge::new(reqwest::Client::new());
    s.password = "wrong".into();
    assert_eq!(
        deluge.test(&s).await.unwrap_err(),
        DownloadError::Authentication
    );
    s.password = "secret".into();
    mock.lock().unwrap().label_plugin = false;
    let err = deluge.test(&s).await.unwrap_err();
    assert!(err.to_string().contains("Label plugin"), "{err}");
}

#[tokio::test]
async fn adds_label_and_queue_position() {
    let (s, mock) = mock_deluge().await;
    let deluge = Deluge::new(reqwest::Client::new());
    let magnet = "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=x";
    let id = deluge.add(&s, &Source::Magnet(magnet), true).await.unwrap();
    assert_eq!(id, "C12FE1C06BBA254A9DC9F519B335AA7C1367A88A");
    let id = deluge
        .add(
            &s,
            &Source::File {
                name: "x.torrent",
                content: b"d4:infod4:name1:xee",
            },
            false,
        )
        .await
        .unwrap();
    assert_eq!(id, "AA11BB22CC33DD44EE55FF66AA11BB22CC33DD44");
    let calls = mock.lock().unwrap().calls.clone();
    let after_login: Vec<&String> = calls.iter().filter(|c| !c.starts_with("web.")).collect();
    assert_eq!(
        after_login[0],
        &format!(
            "core.add_torrent_magnet [\"{magnet}\",{{\"add_paused\":false,\"download_location\":\"/downloads/incoming\",\"remove_at_ratio\":false}}]"
        )
    );
    assert_eq!(
        after_login[1],
        "label.set_torrent [\"c12fe1c06bba254a9dc9f519b335aa7c1367a88a\",\"tv-kitsunarr\"]"
    );
    // Recent episode with priority "first": moved to the top. The older one is not.
    assert_eq!(
        after_login[2],
        "core.queue_top [[\"c12fe1c06bba254a9dc9f519b335aa7c1367a88a\"]]"
    );
    assert!(after_login[3].starts_with("core.add_torrent_file [\"x.torrent\","));
    assert_eq!(
        after_login[4],
        "label.set_torrent [\"aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44\",\"tv-kitsunarr\"]"
    );
    assert_eq!(after_login.len(), 5);
}

#[tokio::test]
async fn lists_labelled_torrents_with_sonarr_states() {
    let (s, mock) = mock_deluge().await;
    let torrent = |hash: &str, name: &str, state: &str, finished: bool| {
        json!({
            "hash": hash, "name": name, "state": state, "is_finished": finished, "progress": 50.0,
            "eta": 120.0, "save_path": "/downloads/complete", "total_size": 1000, "total_done": 400,
            "ratio": 0.5, "message": "OK"
        })
    };
    mock.lock().unwrap().torrents = vec![
        (
            "tv-kitsunarr".into(),
            torrent("aa", "Show.S01E01", "Seeding", true),
        ),
        (
            "tv-kitsunarr".into(),
            torrent("bb", "Show.S01E02", "Checking", true),
        ),
        (
            "tv-kitsunarr".into(),
            torrent("cc", "Show.S01E03", "Queued", false),
        ),
        (
            "tv-kitsunarr".into(),
            torrent("dd", "Show.S01E04", "Paused", false),
        ),
        (
            "tv-kitsunarr".into(),
            torrent("ee", "Show.S01E05", "Error", false),
        ),
        (
            "tv-kitsunarr".into(),
            torrent("ff", "Show.S01E06", "Downloading", false),
        ),
        (
            "tv-kitsunarr".into(),
            torrent("gg", "", "Downloading", false),
        ),
        ("movies".into(), torrent("hh", "Film", "Seeding", true)),
    ];
    let deluge = Deluge::new(reqwest::Client::new());
    let items = deluge.items(&s).await.unwrap();
    let states: Vec<(&str, ItemStatus)> = items
        .iter()
        .map(|i| (i.download_id.as_str(), i.status))
        .collect();
    assert_eq!(
        states,
        [
            ("AA", ItemStatus::Completed),
            ("BB", ItemStatus::Downloading),
            ("CC", ItemStatus::Queued),
            ("DD", ItemStatus::Paused),
            ("EE", ItemStatus::Warning),
            ("FF", ItemStatus::Downloading),
        ],
        "other labels and torrents without a name are left out"
    );
    let first = &items[0];
    assert_eq!(
        first.output_path.as_deref(),
        Some("/downloads/complete/Show.S01E01")
    );
    assert_eq!((first.total_size, first.remaining_size), (1000, 600));
    assert_eq!(first.remaining_seconds, Some(120));
    assert_eq!(first.category, "tv-kitsunarr");
    assert_eq!(
        items[4].message.as_deref(),
        Some("Deluge is reporting an error")
    );
}

#[tokio::test]
async fn expired_sessions_log_in_again_and_imports_get_the_label() {
    let (s, mock) = mock_deluge().await;
    let deluge = Deluge::new(reqwest::Client::new());
    deluge.version(&s).await.unwrap();
    // The Web UI restarts: sessions are gone and the daemon disconnected.
    {
        let mut m = mock.lock().unwrap();
        m.sessions.clear();
        m.connected = false;
    }
    deluge
        .mark_imported(&s, "C12FE1C06BBA254A9DC9F519B335AA7C1367A88A")
        .await
        .unwrap();
    let m = mock.lock().unwrap();
    assert_eq!(m.logins, 2);
    assert_eq!(
        m.calls.last().unwrap(),
        "label.set_torrent [\"c12fe1c06bba254a9dc9f519b335aa7c1367a88a\",\"tv-imported\"]"
    );
}

#[test]
fn settings_follow_sonarr_defaults_and_label_rules() {
    let s = Settings::from_map(&Map::new()).unwrap();
    assert_eq!(
        (s.host.as_str(), s.port, s.password.as_str()),
        ("localhost", 8112, "deluge")
    );
    assert_eq!(s.rpc_url(), "http://localhost:8112/json");
    let bad = Settings::from_map(json!({ "tvCategory": "TV Shows" }).as_object().unwrap());
    assert!(bad.unwrap_err().contains("a-z, 0-9 and -"));
}
