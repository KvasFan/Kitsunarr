//! SABnzbd and NZBGet against mock APIs: tests, NZB uploads, the queue and history, removal.

use std::sync::{Arc, Mutex};

use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use kitsunarr_downloaders::{ClientSettings, Clients, DownloadError, ItemStatus, nzbget, sabnzbd};
use serde_json::{Value, json};
use std::collections::HashMap;

const NZB: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><file subject="x"><segments/></file></nzb>"#;

#[derive(Default)]
struct Sab {
    calls: Vec<String>,
    uploads: Vec<(String, usize)>,
    config: Value,
}

type SabShared = Arc<Mutex<Sab>>;

async fn sab_get(State(m): State<SabShared>, Query(q): Query<HashMap<String, String>>) -> Response {
    sab_answer(&m, &q, None)
}

async fn sab_post(
    State(m): State<SabShared>,
    Query(q): Query<HashMap<String, String>>,
    mut multipart: Multipart,
) -> Response {
    let mut file = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.file_name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.unwrap();
        file = Some((name, bytes.len()));
    }
    sab_answer(&m, &q, file)
}

fn sab_answer(
    m: &SabShared,
    q: &HashMap<String, String>,
    file: Option<(String, usize)>,
) -> Response {
    let mut m = m.lock().unwrap();
    if q.get("output").map(String::as_str) != Some("json") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if q.get("apikey").map(String::as_str) != Some("key") {
        return axum::Json(json!({"status": false, "error": "API Key Incorrect"})).into_response();
    }
    let mode = q.get("mode").cloned().unwrap_or_default();
    let mut call = mode.clone();
    for k in [
        "name",
        "value",
        "cat",
        "priority",
        "del_files",
        "archive",
        "category",
    ] {
        if let Some(v) = q.get(k) {
            call.push_str(&format!(" {k}={v}"));
        }
    }
    m.calls.push(call);
    let body = match mode.as_str() {
        "version" => json!({"version": "4.3.2"}),
        "get_config" => json!({"config": m.config}),
        "fullstatus" => json!({"status": {"completedir": "/data/complete"}}),
        "addfile" => {
            m.uploads.push(file.unwrap());
            json!({"status": true, "nzo_ids": ["SABnzbd_nzo_abc"]})
        }
        "queue" if q.contains_key("name") => json!({"status": true}),
        "queue" => json!({"queue": {"paused": false, "slots": [
            {"status": "Downloading", "nzo_id": "SABnzbd_nzo_abc", "filename": "Show.S01E01", "cat": "tv",
             "mb": "100", "mbleft": "50", "timeleft": "0:00:30", "priority": "Normal"},
            {"status": "Downloading", "nzo_id": "SABnzbd_nzo_other", "filename": "Movie", "cat": "movies",
             "mb": "1", "mbleft": "1", "timeleft": "0:00:30"}
        ]}}),
        "history" if q.contains_key("name") => json!({"status": true}),
        "history" => json!({"history": {"slots": [
            {"status": "Completed", "nzo_id": "SABnzbd_nzo_done", "name": "Show.S01E02", "category": "tv",
             "bytes": 2048, "storage": "/data/complete/tv/Show.S01E02/Show.S01E02.mkv"}
        ]}}),
        _ => json!({"status": false, "error": "not implemented"}),
    };
    axum::Json(body).into_response()
}

async fn sab_mock(config: Value) -> (ClientSettings, SabShared) {
    let shared = Arc::new(Mutex::new(Sab {
        config,
        ..Default::default()
    }));
    let app = axum::Router::new()
        .route("/sab/api", get(sab_get).post(sab_post))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let settings = json!({"host": "127.0.0.1", "port": port, "urlBase": "/sab", "apiKey": "key", "tvCategory": "tv"});
    (
        ClientSettings::from_provider("Sabnzbd", settings.as_object().unwrap()).unwrap(),
        shared,
    )
}

fn sab_config() -> Value {
    json!({
        "misc": {"complete_dir": "complete", "enable_tv_sorting": 0, "tv_categories": ["tv"],
                 "pre_check": 0, "history_retention_option": "all"},
        "categories": [{"name": "*", "dir": ""}, {"name": "tv", "dir": "tv"}],
        "sorters": []
    })
}

#[tokio::test]
async fn sabnzbd_test_add_list_and_remove() {
    let (settings, mock) = sab_mock(sab_config()).await;
    let clients = Clients::default();
    let tested = clients.test(&settings).await.unwrap();
    assert_eq!(tested.message, "Connected to SABnzbd 4.3.2.");
    assert!(tested.warnings.is_empty(), "{:?}", tested.warnings);

    let id = clients
        .add_nzb(&settings, "Show.S01E01.nzb", NZB, true)
        .await
        .unwrap();
    assert_eq!(id, "SABnzbd_nzo_abc");
    assert_eq!(
        mock.lock().unwrap().uploads,
        vec![("Show.S01E01.nzb".to_string(), NZB.len())]
    );
    assert!(
        mock.lock()
            .unwrap()
            .calls
            .contains(&"addfile cat=tv priority=-100".to_string())
    );

    let items = clients.items(&settings).await.unwrap();
    assert_eq!(items.len(), 2, "other categories are left out: {items:?}");
    assert_eq!(items[0].status, ItemStatus::Downloading);
    assert_eq!(items[0].remaining_seconds, Some(30));
    assert_eq!(items[1].status, ItemStatus::Completed);
    assert_eq!(
        items[1].output_path.as_deref(),
        Some("/data/complete/tv/Show.S01E02")
    );

    clients.remove(&settings, &items[0], true).await.unwrap();
    clients.remove(&settings, &items[1], false).await.unwrap();
    let calls = mock.lock().unwrap().calls.clone();
    assert!(
        calls.contains(&"queue name=delete value=SABnzbd_nzo_abc del_files=1".to_string()),
        "{calls:?}"
    );
    assert!(
        calls.contains(
            &"history name=delete value=SABnzbd_nzo_done del_files=0 archive=1".to_string()
        )
    );

    // Invalid NZBs never reach the client.
    let err = clients
        .add_nzb(&settings, "x.nzb", b"<html/>", false)
        .await
        .unwrap_err();
    assert!(matches!(err, DownloadError::Rejected(_)), "{err:?}");
}

#[tokio::test]
async fn sabnzbd_test_reports_problems() {
    let mut config = sab_config();
    config["misc"]["enable_tv_sorting"] = json!(1);
    let (settings, _) = sab_mock(config).await;
    let err = Clients::default().test(&settings).await.unwrap_err();
    assert!(err.to_string().contains("TV sorting"), "{err}");

    let mut config = sab_config();
    config["categories"] = json!([{"name": "tv", "dir": "tv*"}]);
    let (settings, _) = sab_mock(config).await;
    let err = Clients::default().test(&settings).await.unwrap_err();
    assert!(err.to_string().contains("job folders"), "{err}");

    let mut config = sab_config();
    config["categories"] = json!([{"name": "movies", "dir": "m"}]);
    config["misc"]["history_retention_option"] = json!("days-delete");
    config["misc"]["history_retention_number"] = json!(3);
    let (settings, _) = sab_mock(config).await;
    let err = Clients::default().test(&settings).await.unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");

    let (settings, _) = sab_mock(sab_config()).await;
    let ClientSettings::Sabnzbd(good) = settings else {
        unreachable!()
    };
    let port = good.port;
    let wrong = json!({"host": "127.0.0.1", "port": port, "urlBase": "/sab", "apiKey": "nope"});
    let wrong = ClientSettings::from_provider("Sabnzbd", wrong.as_object().unwrap()).unwrap();
    assert_eq!(
        Clients::default().test(&wrong).await.unwrap_err(),
        DownloadError::Authentication
    );
    let _ = sabnzbd::Settings::from_map(json!({}).as_object().unwrap()).unwrap_err();
}

#[derive(Default)]
struct Nzb {
    calls: Vec<String>,
    version: String,
    keep_history: String,
}

type NzbShared = Arc<Mutex<Nzb>>;

async fn nzb_rpc(State(m): State<NzbShared>, headers: HeaderMap, body: String) -> Response {
    let mut m = m.lock().unwrap();
    // "nzb:pass"
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some("Basic bnpiOnBhc3M=")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let req: Value = serde_json::from_str(&body).unwrap();
    let method = req["method"].as_str().unwrap().to_string();
    let params = req["params"].clone();
    let result = match method.as_str() {
        "version" => json!(m.version),
        "config" => json!([
            {"Name": "MainDir", "Value": "/data"},
            {"Name": "DestDir", "Value": "${MainDir}/dst"},
            {"Name": "KeepHistory", "Value": m.keep_history},
            {"Name": "Category1.Name", "Value": "tv"},
            {"Name": "Category1.DestDir", "Value": ""}
        ]),
        "append" => {
            let p = params.as_array().unwrap();
            match p.len() {
                // NZBGet 12: name, category, priority, add to top, content; true on success.
                5 => {
                    m.calls.push(format!("append12 {} {} {}", p[0], p[1], p[2]));
                    assert!(!p[4].as_str().unwrap().is_empty());
                    json!(true)
                }
                // NZBGet 13–15: no parameters.
                9 => {
                    m.calls.push(format!(
                        "append13 {} {} {} paused={}",
                        p[0], p[2], p[3], p[5]
                    ));
                    json!(42)
                }
                _ => {
                    m.calls.push(format!(
                        "append {} {} {} paused={} {}",
                        p[0], p[2], p[3], p[5], p[9][0]
                    ));
                    assert!(!p[1].as_str().unwrap().is_empty());
                    json!(42)
                }
            }
        }
        "status" => json!({"DownloadPaused": false, "DownloadRate": 10}),
        "listgroups" => json!([
            {"NZBID": 42, "LastID": 99, "NZBName": "Show.S01E01", "Category": "tv", "FileSizeLo": 100, "FileSizeHi": 0,
             "RemainingSizeLo": 50, "RemainingSizeHi": 0, "PausedSizeLo": 0, "PausedSizeHi": 0, "ActiveDownloads": 1,
             "Parameters": [{"Name": "drone", "Value": "aaaabbbbccccddddeeeeffff00001111"}]}
        ]),
        "history" => json!([
            {"ID": 7, "Name": "Show.S01E02", "Category": "tv", "FileSizeLo": 10, "FileSizeHi": 0,
             "ParStatus": "SUCCESS", "UnpackStatus": "SUCCESS", "MoveStatus": "SUCCESS", "ScriptStatus": "NONE",
             "DeleteStatus": "NONE", "MarkStatus": "NONE", "DestDir": "/data/dst/tv/Show.S01E02", "FinalDir": "",
             "Parameters": []},
            {"ID": 8, "Name": "Other", "Category": "movies", "ParStatus": "SUCCESS", "UnpackStatus": "SUCCESS",
             "MoveStatus": "SUCCESS", "ScriptStatus": "NONE", "DeleteStatus": "", "MarkStatus": "", "DestDir": "/m"}
        ]),
        "editqueue" if params[0] == "GroupSetParameter" => {
            let text = params[2].as_str().unwrap();
            assert!(text.starts_with("drone=") && text.len() == 6 + 32, "{text}");
            m.calls
                .push(format!("editqueue {} {}", params[0], params[3]));
            json!(true)
        }
        "editqueue" => {
            m.calls
                .push(format!("editqueue {} {}", params[0], params[3]));
            json!(true)
        }
        _ => {
            return axum::Json(json!({"error": {"code": 1, "message": "unknown"}})).into_response();
        }
    };
    axum::Json(json!({"version": "1.1", "result": result})).into_response()
}

async fn nzb_mock(version: &str, keep: &str, password: &str) -> (ClientSettings, NzbShared) {
    let shared = Arc::new(Mutex::new(Nzb {
        version: version.into(),
        keep_history: keep.into(),
        ..Default::default()
    }));
    let app = axum::Router::new()
        .route("/jsonrpc", post(nzb_rpc))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let settings = json!({"host": "127.0.0.1", "port": port, "username": "nzb", "password": password,
                          "tvCategory": "tv", "recentTvPriority": 50, "addPaused": true});
    (
        ClientSettings::from_provider("Nzbget", settings.as_object().unwrap()).unwrap(),
        shared,
    )
}

#[tokio::test]
async fn nzbget_test_add_list_and_remove() {
    let (settings, mock) = nzb_mock("21.1", "7", "pass").await;
    let clients = Clients::default();
    let tested = clients.test(&settings).await.unwrap();
    assert_eq!(tested.message, "Connected to NZBGet 21.1.");

    let id = clients
        .add_nzb(&settings, "Show.S01E01.nzb", NZB, true)
        .await
        .unwrap();
    assert_eq!(id.len(), 32);
    let calls = mock.lock().unwrap().calls.clone();
    assert_eq!(
        calls,
        vec![r#"append "Show.S01E01.nzb" "tv" 50 paused=true "drone""#.to_string()]
    );

    let items = clients.items(&settings).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].download_id, "aaaabbbbccccddddeeeeffff00001111");
    assert_eq!(items[0].remaining_seconds, Some(5));
    assert_eq!(items[1].download_id, "7");
    assert_eq!(items[1].status, ItemStatus::Completed);
    assert_eq!(
        items[1].output_path.as_deref(),
        Some("/data/dst/tv/Show.S01E02")
    );

    clients.remove(&settings, &items[0], false).await.unwrap();
    clients.remove(&settings, &items[1], false).await.unwrap();
    let calls = mock.lock().unwrap().calls.clone();
    assert!(calls.contains(&r#"editqueue "GroupFinalDelete" 42"#.to_string()));
    assert!(calls.contains(&r#"editqueue "HistoryDelete" 7"#.to_string()));
}

/// `DownloadNzbLegacy13` and `DownloadNzbLegacy12`: the drone id is set on the queued item.
#[tokio::test]
async fn older_nzbget_versions_get_the_drone_id_afterwards() {
    for (version, expected) in [
        (
            "15.0",
            vec![
                r#"append13 "Show.S01E01.nzb" "tv" 50 paused=false"#,
                r#"editqueue "GroupSetParameter" 42"#,
            ],
        ),
        (
            "12.0-r1",
            vec![
                r#"append12 "Show.S01E01.nzb" "tv" 50"#,
                r#"editqueue "GroupSetParameter" 99"#,
            ],
        ),
    ] {
        let (settings, mock) = nzb_mock(version, "7", "pass").await;
        let clients = Clients::default();
        assert!(clients.test(&settings).await.is_ok(), "{version}");
        let id = clients
            .add_nzb(&settings, "Show.S01E01.nzb", NZB, true)
            .await
            .unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(mock.lock().unwrap().calls, expected, "{version}");
    }
    // Not in the queue after adding: rejected, as upstream.
    let (settings, _) = nzb_mock("12.0", "7", "pass").await;
    let err = Clients::default()
        .add_nzb(&settings, "Unknown.nzb", NZB, true)
        .await
        .unwrap_err();
    assert!(matches!(err, DownloadError::Rejected(_)), "{err}");
}

#[tokio::test]
async fn nzbget_test_reports_problems() {
    let (settings, _) = nzb_mock("11.0", "7", "pass").await;
    let err = Clients::default().test(&settings).await.unwrap_err();
    assert!(err.to_string().contains("too old"), "{err}");
    let (settings, _) = nzb_mock("21.1", "0", "pass").await;
    let err = Clients::default().test(&settings).await.unwrap_err();
    assert!(err.to_string().contains("KeepHistory"), "{err}");
    let (settings, _) = nzb_mock("21.1", "7", "wrong").await;
    assert_eq!(
        Clients::default().test(&settings).await.unwrap_err(),
        DownloadError::Authentication
    );
    let half = json!({"username": "x"});
    assert!(nzbget::Settings::from_map(half.as_object().unwrap()).is_err());
}

/// `GetStatus`: the category's folder under the complete folder, the category's sorting, and
/// whether SABnzbd keeps finished downloads.
#[tokio::test]
async fn sabnzbd_status_reports_folders_sorting_and_history_retention() {
    let (settings, _) = sab_mock(sab_config()).await;
    let status = Clients::default().status(&settings).await.unwrap();
    assert!(status.is_localhost);
    assert_eq!(status.output_root_folders, ["/data/complete/tv"]);
    assert_eq!(status.sorting_mode, None);
    assert!(!status.removes_completed_downloads);

    let mut config = sab_config();
    config["misc"]["enable_tv_sorting"] = json!(1);
    config["misc"]["history_retention_option"] = json!("number-delete");
    let (settings, _) = sab_mock(config).await;
    let status = Clients::default().status(&settings).await.unwrap();
    assert_eq!(status.sorting_mode.as_deref(), Some("TV"));
    assert!(status.removes_completed_downloads);
}
