//! qBittorrent client against a mock of the Web API v2 (login cookie, 403 without a session,
//! multipart adds), plus an opt-in test against a real qBittorrent.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use kitsunarr_downloaders::qbittorrent::{AddRequest, Settings, Source};
use kitsunarr_downloaders::{DownloadError, QBittorrent, torrent_info_hash};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct Mock {
    /// Calls as `"path field=value …"` (form and query fields, sorted).
    calls: Vec<String>,
    categories: Vec<String>,
    torrents: Vec<Value>,
    logins: usize,
    remove_at_ratio: bool,
}

type Shared = Arc<Mutex<Mock>>;

fn authorised(headers: &HeaderMap) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .is_some_and(|c| c.contains("SID=good"))
}

async fn login(State(m): State<Shared>, body: String) -> Response {
    m.lock().unwrap().logins += 1;
    if body.contains("username=admin") && body.contains("password=secret") {
        ([(header::SET_COOKIE, "SID=good; HttpOnly; path=/")], "Ok.").into_response()
    } else {
        "Fails.".into_response()
    }
}

async fn api(
    State(m): State<Shared>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    Query(q): Query<HashMap<String, String>>,
    multipart: Option<Multipart>,
) -> Response {
    if !authorised(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let path = uri.path().to_string();
    let mut fields: Vec<String> = q.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut file: Option<Vec<u8>> = None;
    if let Some(mut mp) = multipart {
        while let Ok(Some(field)) = mp.next_field().await {
            let name = field.name().unwrap_or_default().to_string();
            if name == "torrents" {
                file = Some(field.bytes().await.unwrap().to_vec());
                fields.push("torrents=<file>".into());
            } else {
                fields.push(format!("{name}={}", field.text().await.unwrap()));
            }
        }
    }
    fields.sort();
    let mut mock = m.lock().unwrap();
    mock.calls.push(format!("{path} {}", fields.join(" ")));
    let field = |k: &str| {
        fields
            .iter()
            .find_map(|f| f.strip_prefix(&format!("{k}=")).map(str::to_string))
    };
    match path.as_str() {
        "/api/v2/app/webapiVersion" => "2.11.4".into_response(),
        "/api/v2/app/preferences" => axum::Json(json!({
            "save_path": "/downloads", "dht": true, "max_ratio_enabled": mock.remove_at_ratio,
            "max_ratio_act": 1, "max_seeding_time_enabled": false, "max_seeding_time": -1
        }))
        .into_response(),
        "/api/v2/torrents/categories" => {
            let map: Map<String, Value> = mock
                .categories
                .iter()
                .map(|c| (c.clone(), json!({"name": c, "savePath": ""})))
                .collect();
            axum::Json(Value::Object(map)).into_response()
        }
        "/api/v2/torrents/createCategory" => {
            let c = field("category").unwrap();
            mock.categories.push(c);
            ().into_response()
        }
        "/api/v2/torrents/info" => {
            let category = field("category");
            let list: Vec<Value> = mock
                .torrents
                .iter()
                .filter(|t| category.as_deref().is_none_or(|c| t["category"] == c))
                .cloned()
                .collect();
            axum::Json(Value::Array(list)).into_response()
        }
        "/api/v2/torrents/add" => {
            let hash = match (field("urls"), &file) {
                (Some(url), _) => kitsunarr_downloaders::magnet_info_hash(&url).unwrap(),
                (None, Some(bytes)) => torrent_info_hash(bytes).unwrap(),
                _ => return "Fails.".into_response(),
            };
            if mock.torrents.iter().any(|t| t["hash"] == hash.as_str()) {
                return StatusCode::CONFLICT.into_response();
            }
            let category = field("category").unwrap_or_default();
            mock.torrents.push(json!({"hash": hash, "name": "x", "size": 1, "progress": 0.0,
                "eta": 8640000, "state": "stalledDL", "category": category, "save_path": "/downloads",
                "ratio": 0.0}));
            "Ok.".into_response()
        }
        "/api/v2/torrents/properties" => {
            let hash = field("hash").unwrap_or_default();
            if mock.torrents.iter().any(|t| t["hash"] == hash.as_str()) {
                axum::Json(json!({})).into_response()
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
        _ => ().into_response(),
    }
}

async fn mock_qbittorrent() -> (Settings, Shared) {
    let state: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/api/v2/auth/login", post(login))
        .route("/api/v2/{*rest}", get(api).post(api))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let settings: Map<String, Value> = serde_json::from_value(json!({
        "host": "127.0.0.1", "port": port, "username": "admin", "password": "secret",
        "tvCategory": "tv-kitsunarr"
    }))
    .unwrap();
    (Settings::from_map(&settings).unwrap(), state)
}

/// A minimal single-file torrent.
fn torrent_file() -> Vec<u8> {
    let mut f = b"d8:announce23:http://tracker/announce4:info".to_vec();
    f.extend_from_slice(
        b"d6:lengthi1024e4:name15:Show.S01E01.mkv12:piece lengthi16384e6:pieces20:",
    );
    f.extend_from_slice(&[7u8; 20]);
    f.extend_from_slice(b"ee");
    f
}

#[tokio::test]
async fn test_creates_the_category_and_reports_share_limits() {
    let (settings, mock) = mock_qbittorrent().await;
    let client = QBittorrent::new(reqwest::Client::new());
    let report = client.test(&settings).await.unwrap();
    assert_eq!(report.api_version, "2.11.4");
    assert!(report.warnings.is_empty());
    assert_eq!(mock.lock().unwrap().categories, vec!["tv-kitsunarr"]);
    // One login, reused for every call.
    assert_eq!(mock.lock().unwrap().logins, 1);

    mock.lock().unwrap().remove_at_ratio = true;
    let err = client.test(&settings).await.unwrap_err();
    assert!(err.to_string().contains("share limits"), "{err}");

    let mut wrong = settings.clone();
    wrong.password = Some("nope".into());
    assert_eq!(
        client.test(&wrong).await.unwrap_err(),
        DownloadError::Authentication
    );
    let mut closed = settings.clone();
    closed.port = 9;
    assert!(matches!(
        client.test(&closed).await.unwrap_err(),
        DownloadError::Connection(_)
    ));
}

#[tokio::test]
async fn adds_magnets_and_files_with_the_category() {
    let (mut settings, mock) = mock_qbittorrent().await;
    let client = QBittorrent::new(reqwest::Client::new());
    let magnet =
        "magnet:?xt=urn:btih:c9e15763f722f23e98a29decdfae341b98d53056&dn=Show&tr=udp%3A%2F%2Ft";
    let hash = client
        .add(
            &settings,
            &AddRequest {
                source: Source::Magnet(magnet),
                hash: "C9E15763F722F23E98A29DECDFAE341B98D53056",
                recent: false,
                tags: &[],
                seed: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(hash, "c9e15763f722f23e98a29decdfae341b98d53056");
    {
        let calls = &mock.lock().unwrap().calls;
        let add = calls
            .iter()
            .find(|c| c.starts_with("/api/v2/torrents/add"))
            .unwrap();
        // Started (qBittorrent 5 names the flag `stopped`), in the category, from the URL.
        assert_eq!(
            add,
            &format!("/api/v2/torrents/add category=tv-kitsunarr stopped=false urls={magnet}")
        );
    }

    // A file, force-started and moved to the top for a recent episode, with tags.
    settings.initial_state = 1;
    settings.recent_priority = 1;
    settings.add_series_tags = true;
    let file = torrent_file();
    let expected = torrent_info_hash(&file).unwrap();
    let hash = client
        .add(
            &settings,
            &AddRequest {
                source: Source::File {
                    name: "Show.S01E01.torrent",
                    content: &file,
                },
                hash: &expected,
                recent: true,
                tags: &["anime".into(), "4k".into()],
                seed: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(hash, expected);
    let calls = mock.lock().unwrap().calls.clone();
    assert!(
        calls.contains(&"/api/v2/torrents/add category=tv-kitsunarr torrents=<file>".to_string())
    );
    for call in [
        format!("/api/v2/torrents/topPrio hashes={expected}"),
        format!("/api/v2/torrents/setForceStart hashes={expected} value=true"),
        format!("/api/v2/torrents/addTags hashes={expected} tags=anime,4k"),
    ] {
        assert!(calls.contains(&call), "{call} not in {calls:#?}");
    }
    let listed = client.torrents(&settings).await.unwrap();
    assert_eq!(listed.len(), 2);

    // Tanukarr's reuse: a torrent qBittorrent already has is tagged, not added again.
    let again = client
        .add(
            &settings,
            &AddRequest {
                source: Source::File {
                    name: "Show.S01E01.torrent",
                    content: &file,
                },
                hash: &expected,
                recent: false,
                tags: &[],
                seed: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(again, expected);
    let calls = mock.lock().unwrap().calls.clone();
    let tag = kitsunarr_downloaders::qbittorrent::reuse_tag(&settings);
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("/api/v2/torrents/addTags") && c.contains(&tag)),
        "{calls:#?}"
    );

    // The indexer's seed criteria go with the add (Web API 2.8.1 and newer).
    let seeded = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&tr=udp://t";
    client
        .add(
            &settings,
            &AddRequest {
                source: Source::Magnet(seeded),
                hash: "0123456789abcdef0123456789abcdef01234567",
                recent: false,
                tags: &[],
                seed: Some(kitsunarr_downloaders::SeedConfig {
                    ratio: Some(2.0),
                    seed_time_minutes: Some(90),
                }),
            },
        )
        .await
        .unwrap();
    let calls = mock.lock().unwrap().calls.clone();
    let add = calls
        .iter()
        .rfind(|c| c.starts_with("/api/v2/torrents/add "))
        .unwrap();
    assert!(
        add.contains("ratioLimit=2 ") && add.contains("seedingTimeLimit=90 "),
        "{add}"
    );
}

/// Against a real qBittorrent: set `KITSUNARR_QBIT_URL` (e.g. `http://127.0.0.1:8080`),
/// `KITSUNARR_QBIT_USER` and `KITSUNARR_QBIT_PASS`. Creates the `kitsunarr-live-test` category
/// and adds a small stopped torrent with no peers.
#[tokio::test]
#[ignore = "needs a running qBittorrent (KITSUNARR_QBIT_URL, _USER, _PASS)"]
async fn live_qbittorrent() {
    let url = std::env::var("KITSUNARR_QBIT_URL").expect("KITSUNARR_QBIT_URL");
    let (host, port) = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .split_once(':')
        .expect("http://host:port");
    let settings: Map<String, Value> = serde_json::from_value(json!({
        "host": host, "port": port.parse::<u16>().unwrap(),
        "username": std::env::var("KITSUNARR_QBIT_USER").unwrap_or_default(),
        "password": std::env::var("KITSUNARR_QBIT_PASS").unwrap_or_default(),
        "tvCategory": "kitsunarr-live-test", "initialState": 2
    }))
    .unwrap();
    let settings = Settings::from_map(&settings).unwrap();
    let client = QBittorrent::new(reqwest::Client::new());
    let report = client.test(&settings).await.unwrap();
    println!("qBittorrent Web API {}", report.api_version);
    let file = torrent_file();
    let hash = torrent_info_hash(&file).unwrap();
    let added = client
        .add(
            &settings,
            &AddRequest {
                source: Source::File {
                    name: "kitsunarr-live-test.torrent",
                    content: &file,
                },
                hash: &hash,
                recent: false,
                tags: &[],
                seed: None,
            },
        )
        .await;
    assert!(
        added.is_ok() || matches!(added, Err(DownloadError::Rejected(_))),
        "{added:?}"
    );
    let listed = client.torrents(&settings).await.unwrap();
    assert!(listed.iter().any(|t| t.hash == hash), "{listed:?}");
}
