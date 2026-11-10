//! HTTP integration tests: the real router served on a random local port, exercised through
//! the typed client (the same client the web UI uses) and raw requests.

use std::net::SocketAddr;
use std::sync::Arc;

use kitsunarr_api::{AppState, BuildInfo, LogBuffer, NoAssets, NoTasks};
use kitsunarr_client::{Client, ClientError};
use kitsunarr_config::Config;
use kitsunarr_db::Database;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::auth::{ChangePasswordRequest, LoginRequest, SetupRequest};
use kitsunarr_domain::api::library::{
    BulkEdit, EpisodesMonitoredUpdate, LibraryQuery, SeriesUpdate,
};
use kitsunarr_domain::api::metadata::{AddTitleRequest, MonitorOption, SearchQuery};
use kitsunarr_domain::api::migration::{MigrationOptions, PathMapping, SessionStatus};
use kitsunarr_domain::api::settings::{REDACTED, RootFolderCreate};
use kitsunarr_metadata::{Metadata, anilist::AniList, tvmaze::TvMaze};

struct Server {
    base: String,
    api_key: String,
    _dir: tempfile::TempDir,
}

async fn start() -> Server {
    start_with(offline_metadata()).await
}

/// Catalogue clients pointed at a closed local port: tests never reach the real services.
fn offline_metadata() -> Metadata {
    let http = reqwest::Client::new();
    Metadata {
        tvmaze: TvMaze::with_base(http.clone(), "http://127.0.0.1:9"),
        anilist: AniList::with_base(http, "http://127.0.0.1:9/graphql"),
    }
}

async fn start_with(metadata: Metadata) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let api_key = kitsunarr_db::seed::ensure_api_key(&db, kitsunarr_auth::generate_api_key)
        .await
        .unwrap();
    let config = Config {
        data_dir: dir.path().to_path_buf(),
        auth: kitsunarr_config::AuthConfig {
            max_failed_logins: 3,
            ..Default::default()
        },
        ..Default::default()
    };
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
        metadata,
        LogBuffer::default(),
    )
    .with_indexer_client(
        kitsunarr_indexers::IndexerClient::new(reqwest::Client::new())
            .with_rate_limit(std::time::Duration::ZERO),
    );
    let app = kitsunarr_api::app(state);
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
        _dir: dir,
    }
}

async fn signed_in(server: &Server) -> Client {
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

fn status(e: ClientError) -> u16 {
    e.status().unwrap_or(0)
}

#[tokio::test]
async fn first_run_setup_login_and_logout() {
    let server = start().await;
    let anon = Client::new(&server.base);
    let s = anon.auth_status().await.unwrap();
    assert!(s.setup_required && !s.authenticated);
    assert_eq!(
        status(anon.library_stats().await.unwrap_err()),
        401,
        "protected routes need auth"
    );

    // Weak passwords are rejected; setup works once.
    let weak = anon
        .setup(&SetupRequest {
            username: "admin".into(),
            password: "short".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(status(weak), 422);
    let client = signed_in(&server).await;
    assert!(client.auth_status().await.unwrap().authenticated);
    let again = Client::new(&server.base)
        .setup(&SetupRequest {
            username: "evil".into(),
            password: "correct horse battery".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(status(again), 409, "setup is only possible once");

    // Logout invalidates the session; login creates a new one.
    client.logout().await.unwrap();
    assert_eq!(status(client.library_stats().await.unwrap_err()), 401);
    let bad = client
        .login(&LoginRequest {
            username: "admin".into(),
            password: "wrong password!".into(),
            remember_me: false,
        })
        .await
        .unwrap_err();
    assert_eq!(status(bad), 403);
    client
        .login(&LoginRequest {
            username: "ADMIN".into(),
            password: "correct horse battery".into(),
            remember_me: true,
        })
        .await
        .unwrap();
    assert!(client.library_stats().await.is_ok());

    // Changing the password requires the current one and keeps this session.
    let wrong = client
        .change_password(&ChangePasswordRequest {
            current_password: "nope".into(),
            new_password: "another long passphrase".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(status(wrong), 403);
    client
        .change_password(&ChangePasswordRequest {
            current_password: "correct horse battery".into(),
            new_password: "another long passphrase".into(),
        })
        .await
        .unwrap();
    assert!(client.library_stats().await.is_ok());
}

#[tokio::test]
async fn login_throttling_and_api_keys() {
    let server = start().await;
    let _admin = signed_in(&server).await;
    let attacker = Client::new(&server.base);
    for _ in 0..3 {
        let e = attacker
            .login(&LoginRequest {
                username: "admin".into(),
                password: "guess guess guess".into(),
                remember_me: false,
            })
            .await
            .unwrap_err();
        assert_eq!(status(e), 403);
    }
    let e = attacker
        .login(&LoginRequest {
            username: "admin".into(),
            password: "correct horse battery".into(),
            remember_me: false,
        })
        .await
        .unwrap_err();
    assert_eq!(status(e), 429, "throttled even with the right password");

    let keyed = Client::new(&server.base).with_api_key(&server.api_key);
    assert!(keyed.library_stats().await.is_ok());
    assert_eq!(
        status(
            Client::new(&server.base)
                .with_api_key("0".repeat(32))
                .library_stats()
                .await
                .unwrap_err()
        ),
        401
    );
    let query = reqwest::get(format!(
        "{}/api/v1/library/stats?apikey={}",
        server.base, server.api_key
    ))
    .await
    .unwrap();
    assert_eq!(query.status(), 200, "Sonarr-style ?apikey= works");
    assert_eq!(
        status(keyed.api_key().await.unwrap_err()),
        403,
        "an API key cannot read itself"
    );
}

#[tokio::test]
async fn cross_site_writes_are_rejected() {
    let server = start().await;
    let _ = signed_in(&server).await;
    let http = reqwest::Client::new();
    let url = format!("{}/api/v1/tags", server.base);
    let cross = http
        .post(&url)
        .header("x-api-key", &server.api_key)
        .header("origin", "https://evil.example")
        .header("sec-fetch-site", "cross-site")
        .json(&serde_json::json!({"label": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), 403);
    let same = http
        .post(&url)
        .header("x-api-key", &server.api_key)
        .header("sec-fetch-site", "same-origin")
        .json(&serde_json::json!({"label": "Late Night"}))
        .send()
        .await
        .unwrap();
    assert_eq!(same.status(), 201);
    let reads = http
        .get(&url)
        .header("x-api-key", &server.api_key)
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(reads.status(), 200, "safe methods are unaffected");
}

#[tokio::test]
async fn settings_validation_and_secret_redaction() {
    let server = start().await;
    let client = signed_in(&server).await;
    // Root folders must exist and be absolute.
    let missing = client
        .create_root_folder(&RootFolderCreate {
            path: "/definitely/not/here".into(),
            media_kind: None,
        })
        .await
        .unwrap_err();
    assert_eq!(status(missing), 422);
    let relative = client
        .create_root_folder(&RootFolderCreate {
            path: "media/tv".into(),
            media_kind: None,
        })
        .await
        .unwrap_err();
    assert_eq!(status(relative), 422);
    let dir = tempfile::tempdir().unwrap();
    let root = client
        .create_root_folder(&RootFolderCreate {
            path: dir.path().display().to_string(),
            media_kind: None,
        })
        .await
        .unwrap();
    assert!(root.accessible && root.free_space.is_some());

    // Tags are normalised and duplicates conflict.
    let tag = client.create_tag("Late Night").await.unwrap();
    assert_eq!(tag.label, "late-night");
    assert_eq!(
        status(client.create_tag("late night").await.unwrap_err()),
        409
    );

    // Quality profiles are validated.
    let mut profile = client.quality_profiles().await.unwrap().remove(0);
    profile.cutoff = 21; // not allowed in the first default profile
    assert_eq!(
        status(client.update_quality_profile(&profile).await.unwrap_err()),
        400
    );

    // Provider secrets are redacted and preserved on update.
    let created: serde_json::Value = client
        .post(
            "/providers/downloadclients",
            &serde_json::json!({"name": "qb", "implementation": "QBittorrent", "settings": {"host": "qb", "password": "hunter2-secret"}}),
        )
        .await
        .unwrap();
    assert_eq!(created["settings"]["password"], REDACTED);
    let mut item = client.providers("downloadclients").await.unwrap().remove(0);
    item.settings.insert("host".into(), "qb2".into());
    let updated = client
        .update_provider("downloadclients", &item)
        .await
        .unwrap();
    assert_eq!(updated.settings["host"], "qb2");
    let raw = reqwest::Client::new()
        .get(format!("{}/api/v1/providers/downloadclients", server.base))
        .header("x-api-key", &server.api_key)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !raw.contains("hunter2-secret"),
        "secret never leaves the server"
    );
    let rejected: Result<serde_json::Value, _> = client
        .post("/providers/indexers", &serde_json::json!({"name": "x", "implementation": "Torznab", "settings": {"apiKey": REDACTED}}))
        .await;
    assert_eq!(
        status(rejected.unwrap_err()),
        400,
        "the placeholder is not a valid new secret"
    );
}

#[tokio::test]
async fn migration_over_http_then_library_operations() {
    let server = start().await;
    let client = signed_in(&server).await;
    let media = tempfile::tempdir().unwrap();
    for p in ["tv/Breaking Bad", "tv/Chernobyl", "anime/Cowboy Bebop"] {
        std::fs::create_dir_all(media.path().join(p)).unwrap();
    }
    let backup = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sonarr-4.0.20/backup.zip"
    ))
    .unwrap();
    let session = client.migration_upload_bytes(backup).await.unwrap();
    let mut s = session.clone();
    for _ in 0..100 {
        s = client.migration_session(&session.id).await.unwrap();
        if s.status != SessionStatus::Loading {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(s.status, SessionStatus::Ready, "{:?}", s.error);
    assert_eq!(
        s.source_root_folders,
        vec!["/media/anime".to_string(), "/media/tv".to_string()]
    );
    let options = MigrationOptions {
        path_mappings: vec![PathMapping {
            source_prefix: "/media".into(),
            target_prefix: media.path().display().to_string(),
        }],
        ..Default::default()
    };
    let preview = client.migration_plan(&session.id, &options).await.unwrap();
    assert!(preview.blocking.is_empty());
    assert_eq!(
        preview
            .summary
            .iter()
            .find(|x| x.entity == "series")
            .unwrap()
            .create,
        3
    );
    assert_eq!(
        client
            .series(&LibraryQuery::default())
            .await
            .unwrap()
            .total_records,
        0,
        "the plan is a dry run"
    );

    client.migration_apply(&session.id, &options).await.unwrap();
    for _ in 0..200 {
        s = client.migration_session(&session.id).await.unwrap();
        if s.status != SessionStatus::Applying {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(s.status, SessionStatus::Completed, "{:?}", s.error);
    let report = client.migration_report(s.run_id.unwrap()).await.unwrap();
    assert!(report.validation.iter().all(|c| c.passed));
    assert_eq!(client.migration_runs().await.unwrap().len(), 1);

    // Library: paging, search, sort, filters.
    let all = client
        .series(&LibraryQuery {
            page_size: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!((all.total_records, all.records.len()), (3, 2));
    let anime = client
        .series(&LibraryQuery {
            kind: Some(kitsunarr_domain::MediaKind::Anime),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(anime.records[0].title, "Cowboy Bebop");
    let search = client
        .series(&LibraryQuery {
            search: Some("cherno".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(search.total_records, 1);
    let stats = client.library_stats().await.unwrap();
    assert_eq!((stats.series, stats.anime, stats.episode_files), (2, 1, 14));

    // Update, episodes, bulk edit, delete.
    let bb = search.records.first().map(|r| r.id).unwrap();
    let detail = client
        .update_series(
            bb,
            &SeriesUpdate {
                monitored: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!detail.item.monitored);
    let episodes = client.episodes(bb).await.unwrap();
    let ids: Vec<i64> = episodes.iter().take(3).map(|e| e.id).collect();
    let r = client
        .set_episodes_monitored(&EpisodesMonitoredUpdate {
            episode_ids: ids,
            monitored: false,
        })
        .await
        .unwrap();
    assert_eq!(r.updated, 3);
    let edited = client
        .bulk_edit_series(&BulkEdit {
            ids: all.records.iter().map(|r| r.id).collect(),
            monitored: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(edited.updated, 2);
    client.delete_series(bb).await.unwrap();
    assert_eq!(status(client.series_detail(bb).await.unwrap_err()), 404);
    let history = client.history(1, 10, None).await.unwrap();
    assert_eq!(history.total_records, 0, "the fixture has no history rows");
}

#[tokio::test]
async fn openapi_spa_fallback_and_events() {
    let server = start().await;
    let http = reqwest::Client::new();
    let spec: serde_json::Value = http
        .get(format!("{}/api/v1/openapi.json", server.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(spec["paths"].as_object().unwrap().len() >= 45);
    assert!(spec["paths"]["/api/v1/migration/sessions/backup"].is_object());
    let missing_api = http
        .get(format!("{}/api/v1/nope", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_api.status(), 404);
    assert!(
        missing_api.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("json")
    );
    let page = http
        .get(format!("{}/series/42", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        page.status(),
        503,
        "without an embedded UI the notice page is served"
    );
    let missing_asset = http
        .get(format!("{}/assets/app.js", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        missing_asset.status(),
        404,
        "missing assets are not masked by index.html"
    );
    let ping = http
        .get(format!("{}/api/v1/ping", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(ping.status(), 200);
    assert_eq!(ping.headers()["x-content-type-options"], "nosniff");
    let events = http
        .get(format!("{}/api/v1/events", server.base))
        .header("x-api-key", &server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(events.status(), 200);
    assert!(
        events.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
}

/// A local stand-in for TVmaze replaying recorded responses; show 503 simulates an outage.
async fn tvmaze_mock() -> String {
    use axum::extract::Path;
    use axum::response::IntoResponse;
    use axum::routing::get;
    fn fixture(name: &str) -> serde_json::Value {
        let path = format!(
            "{}/../../tests/fixtures/metadata/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }
    async fn show(Path(id): Path<String>) -> axum::response::Response {
        if id == "503" {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        axum::Json(fixture("tvmaze-show-169.json")).into_response()
    }
    async fn episodes(Path(_id): Path<String>) -> axum::Json<serde_json::Value> {
        axum::Json(fixture("tvmaze-show-169.json")["_embedded"]["episodes"].clone())
    }
    async fn search() -> axum::Json<serde_json::Value> {
        axum::Json(fixture("tvmaze-search.json"))
    }
    let app = axum::Router::new()
        .route("/search/shows", get(search))
        .route("/shows/{id}", get(show))
        .route("/shows/{id}/episodes", get(episodes));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn catalogue_search_add_and_refresh() {
    let base = tvmaze_mock().await;
    let http = reqwest::Client::new();
    let server = start_with(Metadata {
        tvmaze: TvMaze::with_base(http.clone(), &base),
        anilist: AniList::with_base(http, "http://127.0.0.1:9/graphql"),
    })
    .await;
    let client = signed_in(&server).await;

    let providers = client.metadata_providers().await.unwrap();
    assert!(
        providers
            .iter()
            .any(|p| p.provider == MetadataProvider::TvMaze && p.attribution.contains("TVmaze"))
    );

    let query = |provider, term: &str| SearchQuery {
        provider,
        term: term.into(),
    };
    let empty = client
        .metadata_search(&query(MetadataProvider::TvMaze, "  "))
        .await
        .unwrap_err();
    assert_eq!(status(empty), 422);
    let unsupported = client
        .metadata_search(&query(MetadataProvider::Tvdb, "breaking bad"))
        .await
        .unwrap_err();
    assert_eq!(status(unsupported), 422);

    let results = client
        .metadata_search(&query(MetadataProvider::TvMaze, "breaking bad"))
        .await
        .unwrap();
    let hit = results
        .iter()
        .find(|r| r.id == "169")
        .expect("Breaking Bad");
    assert!(hit.in_library.is_none());

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().display().to_string();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    let mut add = AddTitleRequest {
        provider: MetadataProvider::TvMaze,
        id: "169".into(),
        root_folder_path: "/not/a/root".into(),
        quality_profile_id: 1,
        monitor: MonitorOption::LatestSeason,
        series_type: None,
        season_folder: true,
        tags: vec![],
    };
    match client.add_title(&add).await.unwrap_err() {
        ClientError::Api { status, body } => {
            assert_eq!(status, 422);
            assert_eq!(body.fields[0].field, "rootFolderPath");
        }
        other => panic!("unexpected {other:?}"),
    }
    add.root_folder_path = root.clone();
    let added = client.add_title(&add).await.unwrap();
    assert_eq!(added.media_type, "series");
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "adding never creates folders on disk"
    );
    let detail = client.series_detail(added.id).await.unwrap();
    assert_eq!(detail.item.title, "Breaking Bad");
    assert_eq!(detail.item.path, format!("{root}/Breaking Bad"));
    assert_eq!(detail.item.episode_count, 62);
    assert!(detail.last_refreshed_at.is_some());
    let episodes = client.episodes(added.id).await.unwrap();
    assert!(
        episodes
            .iter()
            .all(|e| e.monitored == (e.season_number == 5)),
        "latest season only"
    );

    // The search now reports the title as present, and a second add is refused.
    let results = client
        .metadata_search(&query(MetadataProvider::TvMaze, "breaking bad"))
        .await
        .unwrap();
    let hit = results.iter().find(|r| r.id == "169").unwrap();
    assert_eq!(hit.in_library.as_ref().map(|r| r.id), Some(added.id));
    assert_eq!(status(client.add_title(&add).await.unwrap_err()), 409);

    let refreshed = client.refresh_series(added.id).await.unwrap();
    assert_eq!(
        (refreshed.added_episodes, refreshed.updated_episodes),
        (0, 62)
    );
    assert_eq!(status(client.refresh_series(9999).await.unwrap_err()), 404);

    // Catalogue outage → 502 with a readable message; nothing is added.
    add.id = "503".into();
    match client.add_title(&add).await.unwrap_err() {
        ClientError::Api { status, body } => {
            assert_eq!(status, 502);
            assert!(body.message.contains("TVmaze"), "{}", body.message);
        }
        other => panic!("unexpected {other:?}"),
    }

    // Titles owned by a catalogue Kitsunarr cannot query yet (e.g. migrated from Sonarr)
    // are reported as such instead of failing with a server error.
    let db = Database::open(&server._dir.path().join("k.db"))
        .await
        .unwrap();
    sqlx::query("UPDATE series SET metadata_provider = 'tvdb' WHERE id = ?")
        .bind(added.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        status(client.refresh_series(added.id).await.unwrap_err()),
        400
    );
}

/// A local Newznab indexer replaying Sonarr's recorded responses; records the requests.
async fn newznab_mock() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use axum::routing::get;
    type Log = Arc<std::sync::Mutex<Vec<String>>>;
    async fn api(
        State(log): State<Log>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        let mut keys: Vec<String> = q.iter().map(|(k, v)| format!("{k}={v}")).collect();
        keys.sort();
        log.lock().unwrap().push(keys.join("&"));
        let file = if q.get("t").map(String::as_str) == Some("caps") {
            "newznab_caps.xml"
        } else {
            "newznab_nzb_su.xml"
        };
        let mut body = std::fs::read_to_string(format!(
            "{}/../../tests/fixtures/indexers/sonarr/{file}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        if q.get("t").map(String::as_str) == Some("tvsearch") {
            // One release of the searched series among the recorded feed's other shows.
            body = body.replacen(
                "<title>White.Collar.S03E04.720p.HDTV.X264-DIMENSION</title>",
                "<title>Breaking.Bad.S01.480p.WEB-DL.AAC2.0.H.264-GRP</title>",
                1,
            );
        }
        ([("content-type", "application/rss+xml")], body).into_response()
    }
    let log: Log = Arc::default();
    let app = axum::Router::new()
        .route("/api", get(api))
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), log)
}

#[tokio::test]
async fn indexer_test_and_interactive_search() {
    use kitsunarr_domain::api::search::{IndexerTestRequest, ReleaseQuery};
    let tvmaze = tvmaze_mock().await;
    let http = reqwest::Client::new();
    let server = start_with(Metadata {
        tvmaze: TvMaze::with_base(http.clone(), &tvmaze),
        anilist: AniList::with_base(http, "http://127.0.0.1:9/graphql"),
    })
    .await;
    let client = signed_in(&server).await;
    let (indexer_url, requests) = newznab_mock().await;

    // An indexer whose API key the browser only ever sees redacted.
    let created: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Newznab",
                "implementation": "Newznab",
                "options": {"enableRss": true, "enableAutomaticSearch": true, "enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "s3cret-key", "categories": [5030, 5040]}
            }),
        )
        .await
        .unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["settings"]["apiKey"], REDACTED);

    let result = client
        .test_indexer(&IndexerTestRequest {
            id: Some(id),
            implementation: "Newznab".into(),
            settings: serde_json::from_value(created["settings"].clone()).unwrap(),
        })
        .await
        .unwrap();
    assert!(result.ok, "{}", result.message);
    assert_eq!(result.releases_found, 100);
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.contains("apikey=s3cret-key")),
        "the stored key replaces the redaction placeholder"
    );

    let unreachable = client
        .test_indexer(&IndexerTestRequest {
            id: None,
            implementation: "Newznab".into(),
            settings: serde_json::from_value(
                serde_json::json!({"baseUrl": "http://127.0.0.1:9", "categories": [5030]}),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    assert!(!unreachable.ok);
    // The underlying cause is reported, without the request URL.
    assert!(
        unreachable.message.contains("Could not connect")
            && unreachable.message.contains("refused")
            && !unreachable.message.contains("/api?"),
        "{}",
        unreachable.message
    );

    // A series with known identities (TVmaze, TheTVDB, IMDb, TVRage).
    let dir = tempfile::tempdir().unwrap();
    client
        .create_root_folder(&RootFolderCreate {
            path: dir.path().display().to_string(),
            media_kind: None,
        })
        .await
        .unwrap();
    let added = client
        .add_title(&AddTitleRequest {
            provider: MetadataProvider::TvMaze,
            id: "169".into(),
            root_folder_path: dir.path().display().to_string(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
        })
        .await
        .unwrap();

    requests.lock().unwrap().clear();
    let season = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: Some(1),
            episode_id: None,
        })
        .await
        .unwrap();
    assert_eq!((season.searched_indexers, season.failures.len()), (1, 0));
    // An indexer type without a client yet is reported rather than skipped silently.
    let _: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Some Tracker", "implementation": "BroadcastheNet",
                "options": {"enableInteractiveSearch": true}, "settings": {"apiKey": "k"}
            }),
        )
        .await
        .unwrap();
    assert_eq!(season.releases.len(), 100);
    assert_eq!(season.description, "Season 1");
    let first = requests.lock().unwrap()[0].clone();
    assert!(
        first.contains("rid=18164") && first.contains("season=1") && first.contains("t=tvsearch"),
        "{first}"
    );
    let white_collar = season
        .releases
        .iter()
        .find(|r| r.title == "White.Collar.S03E05.720p.HDTV.X264-DIMENSION")
        .unwrap();
    assert!(white_collar.parsed);
    assert_eq!(white_collar.quality, "HDTV-720p");
    assert_eq!(
        (
            white_collar.season_numbers.clone(),
            white_collar.episode_numbers.clone()
        ),
        (vec![3], vec![5])
    );
    assert_eq!(white_collar.release_group.as_deref(), Some("DIMENSION"));
    // The decision engine maps the season pack to the series' seven season-1 episodes and
    // accepts it; the recorded feed's other shows are not in the library.
    let bb = &season.releases[0];
    assert_eq!(bb.title, "Breaking.Bad.S01.480p.WEB-DL.AAC2.0.H.264-GRP");
    assert!(bb.matches_series && bb.full_season && bb.season_numbers == vec![1]);
    assert!(bb.approved, "{:?}", bb.rejections);
    assert_eq!(bb.mapped_episodes.len(), 7);
    assert_eq!(bb.mapped_episodes[0], "S01E01");
    assert!(!white_collar.matches_series && !white_collar.approved);
    assert_eq!(white_collar.rejections, vec!["Unknown Series"]);
    assert_eq!(
        season.releases.iter().filter(|r| r.matches_series).count(),
        1
    );
    assert!(season.releases[1..].iter().all(|r| !r.approved));
    assert!(season.warnings.is_empty(), "{:?}", season.warnings);

    // Indexer options are validated as Sonarr does, and reach the decisions.
    use kitsunarr_domain::api::settings::IndexerOptionsSettings;
    let invalid = IndexerOptionsSettings {
        rss_sync_interval: 5,
        ..Default::default()
    };
    assert_eq!(
        status(client.put_indexer_options(&invalid).await.unwrap_err()),
        422
    );
    let limited = IndexerOptionsSettings {
        maximum_size: 100,
        ..Default::default()
    };
    client.put_indexer_options(&limited).await.unwrap();
    assert_eq!(client.indexer_options().await.unwrap(), limited);
    let capped = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: Some(1),
            episode_id: None,
        })
        .await
        .unwrap();
    let pack = &capped.releases[0];
    assert!(!pack.approved);
    assert!(
        pack.rejections
            .iter()
            .any(|r| r.ends_with("maximum size is 100.0 MB (Settings->Indexers->Maximum Size)")),
        "{:?}",
        pack.rejections
    );
    client
        .put_indexer_options(&IndexerOptionsSettings::default())
        .await
        .unwrap();

    // Download links embed indexer keys: they stay on the server.
    let raw = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/releases?seriesId={}&seasonNumber=1",
            server.base, added.id
        ))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!raw.contains("getnzb") && !raw.contains("s3cret-key") && !raw.contains("downloadUrl"));

    let episodes = client.episodes(added.id).await.unwrap();
    let pilot = episodes
        .iter()
        .find(|e| e.season_number == 1 && e.episode_number == 1)
        .unwrap();
    let episode = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
        })
        .await
        .unwrap();
    assert_eq!(episode.description, "S01E01");
    assert_eq!(episode.searched_indexers, 1);
    assert_eq!(
        episode
            .failures
            .iter()
            .map(|f| (f.indexer.as_str(), f.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            "Some Tracker",
            "Searching BroadcastheNet indexers is not supported yet."
        )]
    );
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("ep=1") && r.contains("season=1"))
    );

    assert_eq!(
        status(
            client
                .search_releases(&ReleaseQuery {
                    series_id: added.id,
                    season_number: None,
                    episode_id: None
                })
                .await
                .unwrap_err()
        ),
        422
    );
}

/// Torrents the qBittorrent mock was given (one test uses it).
static QBIT_TORRENTS: std::sync::LazyLock<std::sync::Mutex<Vec<serde_json::Value>>> =
    std::sync::LazyLock::new(Default::default);

/// A Torznab indexer with one Breaking Bad episode whose download link (with an API key) serves
/// a real `.torrent` file, and a qBittorrent Web API mock that records what it is sent.
async fn torrent_mocks() -> (String, String, Arc<std::sync::Mutex<Vec<String>>>) {
    use axum::extract::{Multipart, Query, State};
    use axum::http::{HeaderMap, header};
    use axum::response::IntoResponse;
    use axum::routing::get;
    type Log = Arc<std::sync::Mutex<Vec<String>>>;

    async fn torznab(
        State(base): State<Arc<std::sync::Mutex<String>>>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        if q.get("t").map(String::as_str) == Some("caps") {
            let caps = std::fs::read_to_string(format!(
                "{}/../../tests/fixtures/indexers/sonarr/newznab_caps.xml",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
            return ([("content-type", "application/xml")], caps).into_response();
        }
        let base = base.lock().unwrap().clone();
        let link = format!("{base}/dl/42.torrent?apikey=t0rznab-key");
        let feed = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel><title>Mock</title>
<item><title>Breaking.Bad.S01E01.720p.BluRay.x264-GRP</title><guid>bb-s01e01</guid>
<link>{link}</link><pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate><size>1500000000</size>
<enclosure url="{link}" length="1500000000" type="application/x-bittorrent" />
<torznab:attr name="seeders" value="50" /><torznab:attr name="peers" value="60" /></item>
</channel></rss>"#
        );
        ([("content-type", "application/rss+xml")], feed).into_response()
    }

    async fn torrent_file(
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        assert_eq!(q.get("apikey").map(String::as_str), Some("t0rznab-key"));
        let mut f = b"d8:announce23:http://tracker/announce4:info".to_vec();
        f.extend_from_slice(
            b"d6:lengthi1024e4:name15:Show.S01E01.mkv12:piece lengthi16384e6:pieces20:",
        );
        f.extend_from_slice(&[7u8; 20]);
        f.extend_from_slice(b"ee");
        ([("content-type", "application/x-bittorrent")], f).into_response()
    }

    async fn qbit(
        State(log): State<Log>,
        headers: HeaderMap,
        uri: axum::http::Uri,
        multipart: Option<Multipart>,
    ) -> axum::response::Response {
        let path = uri.path().to_string();
        if path == "/api/v2/auth/login" {
            return ([(header::SET_COOKIE, "SID=ok; path=/")], "Ok.").into_response();
        }
        if !headers
            .get(header::COOKIE)
            .and_then(|c| c.to_str().ok())
            .is_some_and(|c| c.contains("SID=ok"))
        {
            return axum::http::StatusCode::FORBIDDEN.into_response();
        }
        let mut fields = Vec::new();
        if let Some(mut mp) = multipart {
            while let Ok(Some(f)) = mp.next_field().await {
                let name = f.name().unwrap_or_default().to_string();
                if name == "torrents" {
                    let bytes = f.bytes().await.unwrap();
                    fields.push(format!("torrents=<{} bytes>", bytes.len()));
                    let hash = kitsunarr_downloaders::torrent_info_hash(&bytes).unwrap();
                    QBIT_TORRENTS.lock().unwrap().push(serde_json::json!({
                        "hash": hash, "name": "Breaking.Bad.S01E01.720p.BluRay.x264-GRP",
                        "size": 1000, "progress": 0.4, "eta": 90, "state": "downloading",
                        "category": "tv-kitsunarr", "save_path": "/downloads", "ratio": 0.0
                    }));
                } else {
                    fields.push(format!("{name}={}", f.text().await.unwrap()));
                }
            }
        }
        fields.sort();
        log.lock()
            .unwrap()
            .push(format!("{path} {}", fields.join(" ")).trim().to_string());
        match path.as_str() {
            "/api/v2/app/webapiVersion" => "2.11.4".into_response(),
            "/api/v2/app/preferences" => {
                axum::Json(serde_json::json!({"dht": true})).into_response()
            }
            "/api/v2/torrents/categories" => {
                axum::Json(serde_json::json!({"tv-kitsunarr": {}, "tv-imported": {}}))
                    .into_response()
            }
            "/api/v2/torrents/info" => axum::Json(serde_json::Value::Array(
                QBIT_TORRENTS.lock().unwrap().clone(),
            ))
            .into_response(),
            "/api/v2/torrents/add" => "Ok.".into_response(),
            _ => ().into_response(),
        }
    }

    let base: Arc<std::sync::Mutex<String>> = Arc::default();
    let app = axum::Router::new()
        .route("/api", get(torznab))
        .route("/dl/42.torrent", get(torrent_file))
        .with_state(base.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let indexer = format!("http://{}", listener.local_addr().unwrap());
    *base.lock().unwrap() = indexer.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let log: Log = Arc::default();
    let app = axum::Router::new()
        .route("/api/v2/{*rest}", get(qbit).post(qbit))
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (indexer, port.to_string(), log)
}

#[tokio::test]
async fn grab_sends_the_torrent_to_qbittorrent_and_records_history() {
    use kitsunarr_domain::api::search::{GrabRequest, IndexerTestRequest, ReleaseQuery};
    let tvmaze = tvmaze_mock().await;
    let http = reqwest::Client::new();
    let server = start_with(Metadata {
        tvmaze: TvMaze::with_base(http.clone(), &tvmaze),
        anilist: AniList::with_base(http, "http://127.0.0.1:9/graphql"),
    })
    .await;
    let client = signed_in(&server).await;
    let (indexer_url, qbit_port, qbit) = torrent_mocks().await;

    let indexer: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Torznab", "implementation": "Torznab",
                "options": {"enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "t0rznab-key", "categories": [5040]}
            }),
        )
        .await
        .unwrap();
    let indexer_id = indexer["id"].as_i64().unwrap();
    let qb: serde_json::Value = client
        .post(
            "/providers/downloadclients",
            &serde_json::json!({
                "name": "qBittorrent", "implementation": "QBittorrent", "enabled": true,
                "settings": {"host": "127.0.0.1", "port": qbit_port.parse::<u16>().unwrap(),
                    "username": "admin", "password": "secret", "tvCategory": "tv-kitsunarr",
                    "tvImportedCategory": "tv-imported"}
            }),
        )
        .await
        .unwrap();
    assert_eq!(qb["supported"], true);
    assert_eq!(qb["settings"]["password"], REDACTED);

    // The saved client tests with its stored password.
    let test = client
        .test_download_client(&IndexerTestRequest {
            id: qb["id"].as_i64(),
            implementation: "QBittorrent".into(),
            settings: serde_json::from_value(qb["settings"].clone()).unwrap(),
        })
        .await
        .unwrap();
    assert!(test.ok, "{}", test.message);
    assert_eq!(test.message, "Connected to qBittorrent (Web API 2.11.4).");

    let dir = tempfile::tempdir().unwrap();
    client
        .create_root_folder(&RootFolderCreate {
            path: dir.path().display().to_string(),
            media_kind: None,
        })
        .await
        .unwrap();
    let added = client
        .add_title(&AddTitleRequest {
            provider: MetadataProvider::TvMaze,
            id: "169".into(),
            root_folder_path: dir.path().display().to_string(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
        })
        .await
        .unwrap();
    let episodes = client.episodes(added.id).await.unwrap();
    let pilot = episodes
        .iter()
        .find(|e| e.season_number == 1 && e.episode_number == 1)
        .unwrap();

    // Grabbing needs a search first.
    let missing = client
        .grab_release(&GrabRequest {
            guid: "bb-s01e01".into(),
            indexer_id,
        })
        .await
        .unwrap_err();
    assert_eq!(status(missing), 404);

    let found = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
        })
        .await
        .unwrap();
    let release = &found.releases[0];
    assert_eq!(release.title, "Breaking.Bad.S01E01.720p.BluRay.x264-GRP");
    assert!(release.approved, "{:?}", release.rejections);

    qbit.lock().unwrap().clear();
    let grabbed = client
        .grab_release(&GrabRequest {
            guid: release.guid.clone(),
            indexer_id,
        })
        .await
        .unwrap();
    assert_eq!(grabbed.download_client, "qBittorrent");
    assert_eq!(grabbed.download_id.len(), 40);
    let calls = qbit.lock().unwrap().clone();
    let add = calls
        .iter()
        .find(|c| c.starts_with("/api/v2/torrents/add"))
        .unwrap_or_else(|| panic!("{calls:#?}"));
    // The file itself was sent (the indexer's link, with its key, never left the server).
    assert_eq!(
        add,
        "/api/v2/torrents/add category=tv-kitsunarr stopped=false torrents=<137 bytes>"
    );

    let history = client.history(1, 50, Some("grabbed")).await.unwrap();
    let grab = history
        .records
        .iter()
        .find(|h| h.event_type == "grabbed")
        .unwrap();
    assert_eq!(
        grab.source_title,
        "Breaking.Bad.S01E01.720p.BluRay.x264-GRP"
    );
    assert_eq!(
        grab.download_id.as_deref(),
        Some(grabbed.download_id.as_str())
    );
    // The queue reads qBittorrent and recognises the grab.
    let queue = client.queue().await.unwrap();
    assert_eq!((queue.clients, queue.failures.len()), (1, 0));
    let item = &queue.items[0];
    assert_eq!(item.download_id, grabbed.download_id.to_uppercase());
    assert_eq!(item.series_title.as_deref(), Some("Breaking Bad"));
    assert_eq!(item.episodes, vec!["S01E01"]);
    assert_eq!(item.quality.as_deref(), Some("Bluray-720p"));
    assert_eq!(item.indexer.as_deref(), Some("Mock Torznab"));
    assert_eq!(
        (
            item.status.as_str(),
            item.tracked_state.as_str(),
            item.size_left,
            item.time_left
        ),
        ("downloading", "downloading", 600, Some(90))
    );

    // The torrent completes; the next check imports it into the series folder.
    let downloads = tempfile::tempdir().unwrap();
    let folder = downloads
        .path()
        .join("Breaking.Bad.S01E01.720p.BluRay.x264-GRP");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("Breaking.Bad.S01E01.720p.BluRay.x264-GRP.mkv"),
        b"video",
    )
    .unwrap();
    std::fs::write(folder.join("sample.mkv"), b"sample").unwrap();
    {
        let mut torrents = QBIT_TORRENTS.lock().unwrap();
        torrents[0]["state"] = "stalledUP".into();
        torrents[0]["progress"] = 1.0.into();
        torrents[0]["save_path"] = downloads.path().display().to_string().into();
        torrents[0]["content_path"] = folder.display().to_string().into();
    }
    // Renaming on, with Sonarr's default formats.
    let mut naming = client.naming().await.unwrap();
    naming.rename_episodes = true;
    naming.season_folder_format = "Season {season:00}".into();
    client.put_naming(&naming).await.unwrap();
    qbit.lock().unwrap().clear();
    let summary = client.refresh_queue().await.unwrap();
    assert_eq!(summary, "1 downloads imported, 0 need attention");
    let detail = client.series_detail(added.id).await.unwrap();
    let imported = std::path::Path::new(&detail.item.path)
        .join("Season 01")
        .join("Breaking Bad - S01E01 - Pilot Bluray-720p.mkv");
    assert_eq!(std::fs::read(&imported).unwrap(), b"video");
    // The download keeps its data (it is still seeding).
    assert!(
        folder
            .join("Breaking.Bad.S01E01.720p.BluRay.x264-GRP.mkv")
            .exists()
    );
    let episodes = client.episodes(added.id).await.unwrap();
    let file = episodes
        .iter()
        .find(|e| e.id == pilot.id)
        .and_then(|e| e.file.clone())
        .expect("the pilot has a file");
    assert_eq!(file.quality_name, "Bluray-720p");
    assert_eq!(
        file.relative_path,
        "Season 01/Breaking Bad - S01E01 - Pilot Bluray-720p.mkv"
    );
    let imports = client
        .history(1, 50, Some("downloadFolderImported"))
        .await
        .unwrap();
    assert_eq!(imports.records.len(), 1);
    assert!(
        qbit.lock().unwrap().contains(&format!(
            "/api/v2/torrents/setCategory category=tv-imported hashes={}",
            grabbed.download_id
        )),
        "{:#?}",
        qbit.lock().unwrap()
    );
    let queue = client.queue().await.unwrap();
    assert_eq!(queue.items[0].tracked_state, "imported");
    // A second check does not import it again.
    assert_eq!(
        client.refresh_queue().await.unwrap(),
        "0 downloads imported, 0 need attention"
    );

    // The indexer key is in neither the search results nor the history.
    for path in [
        "/api/v1/history?page=1&pageSize=50".to_string(),
        format!(
            "/api/v1/releases?seriesId={}&episodeId={}",
            added.id, pilot.id
        ),
    ] {
        let raw = reqwest::Client::new()
            .get(format!("{}{path}", server.base))
            .header("X-Api-Key", &server.api_key)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(
            !raw.contains("t0rznab-key") && !raw.contains("/dl/42"),
            "{path}: {raw}"
        );
    }
}
