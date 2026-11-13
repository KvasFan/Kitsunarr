//! Download Station against a stand-in DiskStation: API discovery, the session id, the
//! DownloadStation2 task list (detail and transfer merged), the hashed serial number in the
//! download id, the shared folder's real path, and adding a magnet found again by its uri.

use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use kitsunarr_downloaders::{AddRequest, ClientSettings, Clients, ItemStatus, Source};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct Mock {
    calls: Vec<String>,
    added: bool,
}

type Shared = Arc<Mutex<Mock>>;

fn ok(data: Value) -> Response {
    axum::Json(json!({"success": true, "data": data})).into_response()
}

async fn webapi(State(m): State<Shared>, Query(q): Query<Vec<(String, String)>>) -> Response {
    let get = |k: &str| {
        q.iter()
            .find(|(x, _)| x == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let (api, method) = (get("api"), get("method"));
    let mut m = m.lock().unwrap();
    m.calls.push(format!("{api}.{method}"));
    if api != "SYNO.API.Info" && api != "SYNO.API.Auth" && get("_sid") != "sid1" {
        return axum::Json(json!({"success": false, "error": {"code": 119}})).into_response();
    }
    match (api.as_str(), method.as_str()) {
        ("SYNO.API.Info", "query") => ok(json!({
            "SYNO.API.Auth": {"path": "auth.cgi", "minVersion": 1, "maxVersion": 7},
            "SYNO.DownloadStation2.Task": {"path": "entry.cgi", "minVersion": 1, "maxVersion": 2},
            "SYNO.DownloadStation.Info": {"path": "DownloadStation/info.cgi", "minVersion": 1, "maxVersion": 2},
            "SYNO.DSM.Info": {"path": "entry.cgi", "minVersion": 2, "maxVersion": 2},
            "SYNO.FileStation.List": {"path": "entry.cgi", "minVersion": 1, "maxVersion": 2}
        })),
        ("SYNO.API.Auth", "login") => {
            assert_eq!(
                (get("version").as_str(), get("account").as_str()),
                ("6", "admin")
            );
            if get("passwd") != "secret" {
                return axum::Json(json!({"success": false, "error": {"code": 400}}))
                    .into_response();
            }
            ok(json!({"sid": "sid1"}))
        }
        ("SYNO.DSM.Info", "getinfo") => ok(json!({"serial": "1234ABCD"})),
        ("SYNO.DownloadStation.Info", "getConfig") => {
            ok(json!({"default_destination": "downloads"}))
        }
        ("SYNO.FileStation.List", "getinfo") => {
            let path: Vec<String> = serde_json::from_str(&get("path")).unwrap();
            ok(json!({"files": [{"path": path[0], "isdir": true,
                "additional": {"real_path": format!("/volume1{}", path[0])}}]}))
        }
        ("SYNO.DownloadStation2.Task", "list") => {
            let detail = get("additional") == "detail";
            let mut tasks = vec![
                json!({"id": "dbid_1", "type": "bt", "title": "Show.S01E01", "size": 1000,
                "status": 5, "additional": if detail {
                    json!({"detail": {"destination": "downloads/tv", "uri": "Show.S01E01"}})
                } else {
                    json!({"transfer": {"size_downloaded": 1000, "size_uploaded": 2000, "speed_download": 0}})
                }}),
            ];
            if m.added {
                tasks.push(json!({"id": "dbid_2", "type": "bt", "title": "Show.S01E02", "size": 0, "status": 1,
                    "additional": if detail {
                        json!({"detail": {"destination": "downloads/tv", "uri": "magnet:?xt=urn:btih:abc"}})
                    } else {
                        json!({"transfer": {"size_downloaded": 0, "size_uploaded": 0, "speed_download": 0}})
                    }}));
            }
            ok(json!({"total": tasks.len(), "task": tasks}))
        }
        ("SYNO.DownloadStation2.Task", "create") => {
            assert_eq!(
                (get("type").as_str(), get("destination").as_str()),
                ("url", "downloads/tv")
            );
            m.added = true;
            ok(json!({}))
        }
        ("SYNO.DownloadStation2.Task", "delete") => {
            assert_eq!(get("id"), "dbid_1");
            ok(json!({}))
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn download_station_lists_adds_and_removes() {
    let shared: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/webapi/{*path}", get(webapi))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let map: Map<String, Value> = serde_json::from_value(json!({
        "host": "127.0.0.1", "port": port, "username": "admin", "password": "secret", "tvCategory": "tv"
    }))
    .unwrap();
    let s = ClientSettings::from_provider("TorrentDownloadStation", &map).unwrap();
    let clients = Clients::new(reqwest::Client::new());
    assert_eq!(
        clients.test(&s).await.unwrap().message,
        "Connected to Download Station."
    );
    let items = clients.items(&s).await.unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.status, ItemStatus::Completed);
    assert_eq!(
        item.output_path.as_deref(),
        Some("/volume1/downloads/tv/Show.S01E01")
    );
    assert_eq!(item.seed_ratio, 2.0);
    assert!(item.can_be_removed);
    // `<sha1 of the serial>:<task id>`.
    let (serial, id) = item.download_id.split_once(':').unwrap();
    assert_eq!((serial.len(), id), (40, "dbid_1"));

    let added = clients
        .add(
            &s,
            &AddRequest {
                source: Source::Magnet("magnet:?xt=urn:btih:abc"),
                title: "Show.S01E02",
                release: None,
                hash: "abc",
                recent: false,
                tags: &[],
                seed: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(added, format!("{serial}:dbid_2"));
    clients.remove(&s, item, false).await.unwrap();
    let calls = shared.lock().unwrap().calls.clone();
    assert_eq!(
        calls.iter().filter(|c| *c == "SYNO.API.Auth.login").count(),
        1,
        "the session is kept"
    );
    assert!(calls.contains(&"SYNO.DownloadStation2.Task.delete".to_string()));
}
