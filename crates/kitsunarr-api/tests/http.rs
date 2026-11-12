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
use kitsunarr_metadata::Metadata;

struct Server {
    base: String,
    api_key: String,
    state: AppState,
    _dir: tempfile::TempDir,
}

async fn start() -> Server {
    start_with(offline_metadata()).await
}

/// Catalogue clients pointed at a closed local port: tests never reach the real services.
fn offline_metadata() -> Metadata {
    Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: "http://127.0.0.1:9".to_string(),
            anilist: "http://127.0.0.1:9/graphql".to_string(),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    )
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
    let app = kitsunarr_api::app(state.clone());
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
        state,
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

    // The proxy: validated, its password redacted and kept when the placeholder comes back.
    use kitsunarr_domain::api::settings::{ProxySettings, ProxyType};
    assert_eq!(client.proxy().await.unwrap(), ProxySettings::default());
    let mut proxy = ProxySettings {
        enabled: true,
        proxy_type: ProxyType::Socks5,
        hostname: "http://proxy.invalid".into(),
        port: 1080,
        username: "me".into(),
        password: "proxy-secret".into(),
        ..Default::default()
    };
    assert_eq!(status(client.put_proxy(&proxy).await.unwrap_err()), 422);
    proxy.hostname = "proxy.invalid".into();
    let saved = client.put_proxy(&proxy).await.unwrap();
    assert_eq!(saved.password, REDACTED);
    let mut again = client.proxy().await.unwrap();
    assert_eq!(again.password, REDACTED);
    assert_eq!(
        server.state.proxy.current().unwrap().password,
        "proxy-secret"
    );
    again.port = 1081;
    client.put_proxy(&again).await.unwrap();
    let applied = server.state.proxy.current().unwrap();
    assert_eq!(
        (applied.port, applied.password.as_str()),
        (1081, "proxy-secret")
    );
    // `ProxyCheck`: a host that does not resolve is reported.
    assert!(
        client
            .health()
            .await
            .unwrap()
            .iter()
            .any(|h| h.source == "ProxyCheck"),
    );
    again.enabled = false;
    client.put_proxy(&again).await.unwrap();
    assert!(server.state.proxy.current().is_none());
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
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: base.clone(),
            anilist: "http://127.0.0.1:9/graphql".to_string(),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
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
    // TheTVDB (SkyHook) is unreachable in this test.
    let unreachable = client
        .metadata_search(&query(MetadataProvider::Tvdb, "breaking bad"))
        .await
        .unwrap_err();
    assert_eq!(status(unreachable), 502);

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
        search_for_missing: false,
        episode_ordering: None,
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

    // A title owned by TMDB (for example migrated from Tanukarr) cannot refresh without the
    // user's TMDB key, and says so instead of failing with a server error.
    let db = Database::open(&server._dir.path().join("k.db"))
        .await
        .unwrap();
    sqlx::query("UPDATE series SET metadata_provider = 'tmdb' WHERE id = ?")
        .bind(added.id)
        .execute(db.pool())
        .await
        .unwrap();
    match client.refresh_series(added.id).await.unwrap_err() {
        ClientError::Api { status, body } => {
            assert_eq!(status, 400);
            assert!(body.message.contains("API key"), "{}", body.message);
        }
        other => panic!("unexpected {other:?}"),
    }
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
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: tvmaze.clone(),
            anilist: "http://127.0.0.1:9/graphql".to_string(),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
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
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();

    requests.lock().unwrap().clear();
    let season = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: Some(1),
            episode_id: None,
            complete_series: false,
            season_packs_only: false,
        })
        .await
        .unwrap();
    assert_eq!((season.searched_indexers, season.failures.len()), (1, 0));
    // An indexer type without a client yet is reported rather than skipped silently.
    let _: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Some Tracker", "implementation": "Omgwtfnzbs",
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
            complete_series: false,
            season_packs_only: false,
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
            complete_series: false,
            season_packs_only: false,
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
            "Searching Omgwtfnzbs indexers is not supported yet."
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
                    episode_id: None,
                    complete_series: false,
                    season_packs_only: false,
                })
                .await
                .unwrap_err()
        ),
        422
    );
    // Library tools › Recent searches records each search, newest first.
    let recent = client.recent_searches().await.unwrap();
    assert!(!recent.is_empty());
    assert!(recent[0].indexers >= 1, "{recent:?}");

    // An indexer backing off after failures is skipped and reported (`IndexerStatusCheck`).
    kitsunarr_db::indexer_status::record_failure(
        server.state.db.pool(),
        id,
        kitsunarr_db::indexer_status::Failure::Failure,
        server.state.started_at,
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    let skipped = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
            complete_series: false,
            season_packs_only: false,
        })
        .await
        .unwrap();
    assert_eq!(skipped.searched_indexers, 0);
    assert!(
        skipped
            .failures
            .iter()
            .any(|f| f.indexer_id == id && f.message.starts_with("Temporarily disabled")),
        "{:?}",
        skipped.failures
    );
    let health = client.health().await.unwrap();
    assert!(
        health.iter().any(|h| h.source == "IndexerStatusCheck"),
        "{health:?}"
    );
    kitsunarr_db::indexer_status::record_success(server.state.db.pool(), id)
        .await
        .unwrap();
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
        // The RSS feed (no query, no season) also lists S01E03 and another show.
        let rss_only = if !q.contains_key("season") && !q.contains_key("q") {
            r#"<item><title>Breaking.Bad.S01E03.720p.BluRay.x264-GRP</title><guid>bb-s01e03</guid>
<link>magnet:?xt=urn:btih:d12fe1c06bba254a9dc9f519b335aa7c1367a88b&amp;dn=bb3</link>
<pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate><size>1500000000</size>
<enclosure url="magnet:?xt=urn:btih:d12fe1c06bba254a9dc9f519b335aa7c1367a88b&amp;dn=bb3" length="1500000000" type="application/x-bittorrent;x-scheme-handler/magnet" />
<torznab:attr name="seeders" value="40" /><torznab:attr name="peers" value="45" /></item>
<item><title>Some.Other.Show.S02E05.720p.HDTV.x264-GRP</title><guid>other</guid>
<link>magnet:?xt=urn:btih:e12fe1c06bba254a9dc9f519b335aa7c1367a88c&amp;dn=o</link>
<pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate><size>900000000</size>
<enclosure url="magnet:?xt=urn:btih:e12fe1c06bba254a9dc9f519b335aa7c1367a88c&amp;dn=o" length="900000000" type="application/x-bittorrent;x-scheme-handler/magnet" />
<torznab:attr name="seeders" value="40" /></item>"#
        } else {
            ""
        };
        let feed = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel><title>Mock</title>
<item><title>Breaking.Bad.S01E01.720p.BluRay.x264-GRP</title><guid>bb-s01e01</guid>
<link>{link}</link><pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate><size>1500000000</size>
<enclosure url="{link}" length="1500000000" type="application/x-bittorrent" />
<torznab:attr name="seeders" value="50" /><torznab:attr name="peers" value="60" /></item>
<item><title>Breaking.Bad.S01E02.720p.BluRay.x264-GRP</title><guid>bb-s01e02</guid>
<link>magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&amp;dn=bb</link>
<pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate><size>1500000000</size>
<enclosure url="magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&amp;dn=bb" length="1500000000" type="application/x-bittorrent;x-scheme-handler/magnet" />
<torznab:attr name="magneturl" value="magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&amp;dn=bb" />
<torznab:attr name="seeders" value="40" /><torznab:attr name="peers" value="45" /></item>
{rss_only}
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
            b"d6:lengthi734003200e4:name15:Show.S01E01.mkv12:piece lengthi16384e6:pieces20:",
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
            "/api/v2/torrents/properties" => {
                let hash = uri
                    .query()
                    .unwrap_or_default()
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("hash="))
                    .unwrap_or_default()
                    .to_string();
                if QBIT_TORRENTS
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|t| t["hash"].as_str() == Some(hash.as_str()))
                {
                    axum::Json(serde_json::json!({})).into_response()
                } else {
                    axum::http::StatusCode::NOT_FOUND.into_response()
                }
            }
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
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: tvmaze.clone(),
            anilist: "http://127.0.0.1:9/graphql".to_string(),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let (indexer_url, qbit_port, qbit) = torrent_mocks().await;

    let indexer: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Torznab", "implementation": "Torznab",
                "options": {"enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "t0rznab-key", "categories": [5040],
                    "seedCriteria.seedRatio": "2.5", "seedCriteria.seedTime": null}
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
            search_for_missing: false,
            episode_ordering: None,
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
            episode_ids: Vec::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(status(missing), 404);

    let found = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
            complete_series: false,
            season_packs_only: false,
        })
        .await
        .unwrap();
    let release = found
        .releases
        .iter()
        .find(|r| r.title == "Breaking.Bad.S01E01.720p.BluRay.x264-GRP")
        .unwrap();
    assert!(release.approved, "{:?}", release.rejections);

    qbit.lock().unwrap().clear();
    let grabbed = client
        .grab_release(&GrabRequest {
            guid: release.guid.clone(),
            indexer_id,
            episode_ids: Vec::new(),
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
        "/api/v2/torrents/add category=tv-kitsunarr ratioLimit=2.5 stopped=false torrents=<142 bytes>"
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
    // While it downloads, the same release is rejected because it is in the queue.
    let again = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
            complete_series: false,
            season_packs_only: false,
        })
        .await
        .unwrap();
    let queued = again
        .releases
        .iter()
        .find(|r| r.guid == "bb-s01e01")
        .unwrap();
    assert!(
        queued
            .rejections
            .iter()
            .any(|m| m.starts_with("Release in queue")),
        "{:?}",
        queued.rejections
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
    // Library tools: hardlink-only imports link the seeding download into the library.
    client
        .put_library_tools(&kitsunarr_domain::api::settings::LibraryToolsSettings {
            hardlink_only: true,
            ..Default::default()
        })
        .await
        .unwrap();
    qbit.lock().unwrap().clear();
    let summary = client.refresh_queue().await.unwrap();
    assert_eq!(summary, "1 downloads imported, 0 need attention");
    let detail = client.series_detail(added.id).await.unwrap();
    let imported = std::path::Path::new(&detail.item.path)
        .join("Season 01")
        .join("Breaking Bad - S01E01 - Pilot Bluray-720p.mkv");
    assert_eq!(std::fs::read(&imported).unwrap(), b"video");
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(&imported).unwrap().ino(),
            std::fs::metadata(folder.join("Breaking.Bad.S01E01.720p.BluRay.x264-GRP.mkv"))
                .unwrap()
                .ino(),
            "hard-linked, not copied"
        );
    }
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

    // An automatic season search grabs what is missing (S01E02, by magnet) and skips S01E01,
    // which is on disk now.
    qbit.lock().unwrap().clear();
    let season = ReleaseQuery {
        series_id: added.id,
        season_number: Some(1),
        episode_id: None,
        complete_series: false,
        season_packs_only: false,
    };
    let auto = client.automatic_search(&season).await.unwrap();
    assert_eq!((auto.found, auto.approved), (2, 1), "{auto:?}");
    assert!(auto.grab_failures.is_empty(), "{auto:?}");
    assert_eq!(auto.grabbed.len(), 1);
    assert_eq!(
        auto.grabbed[0].title,
        "Breaking.Bad.S01E02.720p.BluRay.x264-GRP"
    );
    assert_eq!(
        auto.grabbed[0].download_id,
        "c12fe1c06bba254a9dc9f519b335aa7c1367a88a"
    );
    assert!(
        qbit.lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("/api/v2/torrents/add") && c.contains("urls=magnet:")),
        "{:#?}",
        qbit.lock().unwrap()
    );
    // Searching again grabs nothing: S01E02 is in the queue now.
    let auto = client.automatic_search(&season).await.unwrap();
    assert_eq!((auto.approved, auto.grabbed.len()), (0, 0), "{auto:?}");

    // A long torrent delay holds S01E03 back: it becomes a pending release, shown in the queue.
    let set_delay = |minutes: i64| {
        let pool = server.state.db.pool().clone();
        async move {
            sqlx::query("DELETE FROM delay_profiles")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO delay_profiles (torrent_delay, preferred_protocol, bypass_if_highest_quality, sort_order) \
                 VALUES (?, 'usenet', 0, 2147483647)",
            )
            .bind(minutes)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    set_delay(10_000_000).await;
    let summary = kitsunarr_api::rss::sync(&server.state).await.unwrap();
    assert_eq!(
        summary,
        "4 releases found (3 for library series), 0 approved, 0 grabbed"
    );
    let queue = client.queue().await.unwrap();
    let pending = queue
        .items
        .iter()
        .find(|i| i.tracked_state == "pending")
        .expect("the delayed release is in the queue");
    assert_eq!(pending.title, "Breaking.Bad.S01E03.720p.BluRay.x264-GRP");
    assert_eq!(pending.episodes, vec!["S01E03"]);
    // The delay is over: the pending release is decided again and grabbed.
    set_delay(0).await;
    // RSS sync: S01E03 is new and grabbed; S01E01 is on disk, S01E02 queued, and the other
    // show is not in the library.
    qbit.lock().unwrap().clear();
    let summary = kitsunarr_api::rss::sync(&server.state).await.unwrap();
    assert_eq!(
        summary,
        "4 releases found (3 for library series, 1 pending before), 1 approved, 1 grabbed: Breaking.Bad.S01E03.720p.BluRay.x264-GRP"
    );
    assert!(
        kitsunarr_db::pending::list(server.state.db.pool())
            .await
            .unwrap()
            .is_empty(),
        "a grabbed release is no longer pending"
    );
    assert!(
        qbit.lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("d12fe1c06bba254a9dc9f519b335aa7c1367a88b")),
        "{:#?}",
        qbit.lock().unwrap()
    );
    // The next sync grabs nothing again.
    let summary = kitsunarr_api::rss::sync(&server.state).await.unwrap();
    assert_eq!(
        summary,
        "4 releases found (3 for library series), 0 approved, 0 grabbed"
    );

    // Wanted › Missing: every aired monitored episode without a file (S01E01 is on disk).
    use kitsunarr_domain::api::wanted::{WantedList, WantedQuery, WantedSearchRequest};
    let all = WantedQuery {
        page_size: Some(100),
        ..Default::default()
    };
    let missing = client.wanted_missing(&all).await.unwrap();
    assert_eq!(missing.total_records, 61);
    assert!(
        missing
            .records
            .iter()
            .all(|e| e.series_title == "Breaking Bad")
    );
    assert!(!missing.records.iter().any(|e| e.episode_id == pilot.id));
    let newest = &missing.records[0];
    assert_eq!((newest.season_number, newest.episode_number), (5, 16));
    let s2e1 = missing
        .records
        .iter()
        .find(|e| e.season_number == 2 && e.episode_number == 1)
        .unwrap()
        .clone();
    assert_eq!(s2e1.last_search_at, None, "never searched");

    // Searching chosen episodes runs in the background; queued episodes are skipped.
    let queued = episodes
        .iter()
        .find(|e| e.season_number == 1 && e.episode_number == 2)
        .unwrap();
    let started = client
        .search_wanted(&WantedSearchRequest {
            list: WantedList::Missing,
            episode_ids: vec![queued.id],
            monitored: None,
        })
        .await
        .unwrap();
    assert_eq!((started.episodes, started.searches), (0, 0));
    assert_eq!(
        started.message,
        "Nothing to search: no wanted episode outside the queue."
    );
    let started = client
        .search_wanted(&WantedSearchRequest {
            list: WantedList::Missing,
            episode_ids: vec![s2e1.episode_id, queued.id],
            monitored: None,
        })
        .await
        .unwrap();
    assert_eq!((started.episodes, started.searches), (1, 1));
    // The search records its time (nothing in the mock feed is S02E01, so nothing is grabbed).
    let mut searched = None;
    for _ in 0..100 {
        let page = client.wanted_missing(&all).await.unwrap();
        searched = page
            .records
            .into_iter()
            .find(|e| e.episode_id == s2e1.episode_id)
            .and_then(|e| e.last_search_at);
        if searched.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(searched.is_some(), "the background search ran");

    // Cutoff unmet: with upgrades to Bluray-1080p, the Bluray-720p pilot qualifies.
    assert_eq!(client.wanted_cutoff(&all).await.unwrap().total_records, 0);
    let mut profile = client
        .quality_profiles()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == 1)
        .unwrap();
    profile.upgrade_allowed = true;
    profile.cutoff = 7;
    client.update_quality_profile(&profile).await.unwrap();
    let cutoff = client.wanted_cutoff(&all).await.unwrap();
    assert_eq!(cutoff.total_records, 1);
    assert_eq!(cutoff.records[0].episode_id, pilot.id);
    assert_eq!(
        cutoff.records[0].quality_name.as_deref(),
        Some("Bluray-720p")
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

/// A Newznab indexer whose release links to an NZB, and SABnzbd: the upload, then the job in
/// the history once it completes.
async fn usenet_mocks() -> (String, u16, Arc<std::sync::Mutex<serde_json::Value>>) {
    usenet_mocks_with("Breaking.Bad.S01E01.720p.HDTV.x264-GRP").await
}

/// As [`usenet_mocks`], with the indexer offering one release of the given title.
async fn usenet_mocks_with(title: &str) -> (String, u16, Arc<std::sync::Mutex<serde_json::Value>>) {
    use axum::extract::{Multipart, Query, State};
    use axum::response::IntoResponse;
    use axum::routing::get;
    type Sab = Arc<std::sync::Mutex<serde_json::Value>>;

    async fn newznab(
        State((base, title)): State<(Arc<std::sync::Mutex<String>>, Arc<String>)>,
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
        let link = format!("{base}/getnzb/77.nzb?apikey=n3wznab-key");
        let feed = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/"><channel><title>Mock</title>
<item><title>{title}</title><guid>nzb-77</guid>
<link>{link}</link><pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate>
<enclosure url="{link}" length="1500000000" type="application/x-nzb" />
<newznab:attr name="size" value="1500000000" /></item>
</channel></rss>"#
        );
        ([("content-type", "application/rss+xml")], feed).into_response()
    }

    async fn nzb(
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        assert_eq!(q.get("apikey").map(String::as_str), Some("n3wznab-key"));
        (
            [("content-type", "application/x-nzb")],
            r#"<?xml version="1.0"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><file subject="x"><segments/></file></nzb>"#,
        )
            .into_response()
    }

    async fn sab(
        State(state): State<Sab>,
        Query(q): Query<std::collections::HashMap<String, String>>,
        multipart: Option<Multipart>,
    ) -> axum::response::Response {
        assert_eq!(q.get("apikey").map(String::as_str), Some("sab-key"));
        let mut uploaded = None;
        if let Some(mut mp) = multipart {
            let field = mp.next_field().await.unwrap().unwrap();
            uploaded = field.file_name().map(str::to_string);
        }
        let mut s = state.lock().unwrap();
        let body = match q.get("mode").map(String::as_str).unwrap_or_default() {
            "version" => serde_json::json!({"version": "4.3.2"}),
            "get_config" => serde_json::json!({"config": {
                "misc": {"complete_dir": "/complete", "history_retention_option": "all"},
                "categories": [{"name": "tv", "dir": "tv"}], "sorters": []}}),
            "addfile" => {
                s["uploaded"] = uploaded.unwrap_or_default().into();
                s["category"] = q.get("cat").cloned().unwrap_or_default().into();
                serde_json::json!({"status": true, "nzo_ids": ["SABnzbd_nzo_x1"]})
            }
            mode @ ("queue" | "history") if q.contains_key("name") => {
                let mut call = format!("{mode} {}", q["name"]);
                for k in ["value", "del_files", "archive"] {
                    if let Some(v) = q.get(k) {
                        call.push_str(&format!(" {k}={v}"));
                    }
                }
                if !s["calls"].is_array() {
                    s["calls"] = serde_json::json!([]);
                }
                s["calls"].as_array_mut().unwrap().push(call.into());
                serde_json::json!({"status": true})
            }
            "queue" => serde_json::json!({"queue": {"paused": false, "slots": s["queue"].clone()}}),
            "history" => serde_json::json!({"history": {"slots": s["history"].clone()}}),
            _ => serde_json::json!({"status": false, "error": "unexpected"}),
        };
        axum::Json(body).into_response()
    }

    let base: Arc<std::sync::Mutex<String>> = Arc::default();
    let app = axum::Router::new()
        .route("/api", get(newznab))
        .route("/getnzb/77.nzb", get(nzb))
        .with_state((base.clone(), Arc::new(title.to_string())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let indexer = format!("http://{}", listener.local_addr().unwrap());
    *base.lock().unwrap() = indexer.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let state: Sab = Arc::new(std::sync::Mutex::new(
        serde_json::json!({"queue": [], "history": []}),
    ));
    let app = axum::Router::new()
        .route("/api", get(sab).post(sab))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (indexer, port, state)
}

#[tokio::test]
async fn usenet_grab_goes_to_sabnzbd_and_imports_by_moving() {
    use kitsunarr_domain::api::search::{GrabRequest, ReleaseQuery};
    let tvmaze = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: tvmaze.clone(),
            anilist: "http://127.0.0.1:9/graphql".to_string(),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let (indexer_url, sab_port, sab) = usenet_mocks().await;
    let indexer: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Newznab", "implementation": "Newznab",
                "options": {"enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "n3wznab-key", "categories": [5040]}
            }),
        )
        .await
        .unwrap();
    let indexer_id = indexer["id"].as_i64().unwrap();
    let sabnzbd: serde_json::Value = client
        .post(
            "/providers/downloadclients",
            &serde_json::json!({
                "name": "SABnzbd", "implementation": "Sabnzbd", "enabled": true,
                "settings": {"host": "127.0.0.1", "port": sab_port, "apiKey": "sab-key", "tvCategory": "tv"}
            }),
        )
        .await
        .unwrap();
    assert_eq!(sabnzbd["supported"], true);
    assert_eq!(sabnzbd["protocol"], "usenet");

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
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    let pilot = client
        .episodes(added.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.season_number == 1 && e.episode_number == 1)
        .unwrap();
    let found = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
            complete_series: false,
            season_packs_only: false,
        })
        .await
        .unwrap();
    let release = found.releases.iter().find(|r| r.guid == "nzb-77").unwrap();
    assert!(release.approved, "{:?}", release.rejections);
    let grabbed = client
        .grab_release(&GrabRequest {
            guid: release.guid.clone(),
            indexer_id,
            episode_ids: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(grabbed.download_client, "SABnzbd");
    assert_eq!(grabbed.download_id, "SABnzbd_nzo_x1");
    assert_eq!(
        sab.lock().unwrap()["uploaded"],
        "Breaking.Bad.S01E01.720p.HDTV.x264-GRP.nzb"
    );
    assert_eq!(sab.lock().unwrap()["category"], "tv");

    // Downloading: the queue matches the job to the grab.
    sab.lock().unwrap()["queue"] = serde_json::json!([{"status": "Downloading", "nzo_id": "SABnzbd_nzo_x1",
        "filename": "Breaking.Bad.S01E01.720p.HDTV.x264-GRP", "cat": "tv", "mb": "1430", "mbleft": "715",
        "timeleft": "0:02:00", "priority": "Normal"}]);
    let queue = client.queue().await.unwrap();
    assert_eq!(queue.items.len(), 1);
    assert_eq!(queue.items[0].series_title.as_deref(), Some("Breaking Bad"));
    assert_eq!(queue.items[0].time_left, Some(120));

    // Completed: the file is moved into the series folder.
    let downloads = tempfile::tempdir().unwrap();
    let job = downloads
        .path()
        .join("Breaking.Bad.S01E01.720p.HDTV.x264-GRP");
    std::fs::create_dir_all(&job).unwrap();
    let video = job.join("Breaking.Bad.S01E01.720p.HDTV.x264-GRP.mkv");
    std::fs::write(&video, b"video").unwrap();
    {
        let mut s = sab.lock().unwrap();
        s["queue"] = serde_json::json!([]);
        s["history"] = serde_json::json!([{"status": "Completed", "nzo_id": "SABnzbd_nzo_x1",
            "name": "Breaking.Bad.S01E01.720p.HDTV.x264-GRP", "category": "tv", "bytes": 5,
            "storage": video.display().to_string()}]);
    }
    assert_eq!(
        client.refresh_queue().await.unwrap(),
        "1 downloads imported, 0 need attention"
    );
    assert!(!video.exists(), "Usenet downloads are moved, not copied");
    let file = client
        .episodes(added.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.id == pilot.id)
        .and_then(|e| e.file)
        .expect("the pilot has a file");
    assert_eq!(file.quality_name, "HDTV-720p");
    let detail = client.series_detail(added.id).await.unwrap();
    let on_disk = std::path::Path::new(&detail.item.path).join(&file.relative_path);
    assert_eq!(std::fs::read(on_disk).unwrap(), b"video");
}

#[tokio::test]
async fn failed_downloads_are_blocklisted_and_queue_items_removed() {
    use kitsunarr_domain::api::queue::{BlocklistRemoval, QueueRemoval};
    use kitsunarr_domain::api::search::{GrabRequest, ReleaseQuery};
    let tvmaze = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: tvmaze.clone(),
            anilist: "http://127.0.0.1:9/graphql".to_string(),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let (indexer_url, sab_port, sab) = usenet_mocks().await;
    let indexer: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Newznab", "implementation": "Newznab",
                "options": {"enableInteractiveSearch": true, "enableAutomaticSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "n3wznab-key", "categories": [5040]}
            }),
        )
        .await
        .unwrap();
    let indexer_id = indexer["id"].as_i64().unwrap();
    let _: serde_json::Value = client
        .post(
            "/providers/downloadclients",
            &serde_json::json!({
                "name": "SABnzbd", "implementation": "Sabnzbd", "enabled": true,
                "settings": {"host": "127.0.0.1", "port": sab_port, "apiKey": "sab-key", "tvCategory": "tv"}
            }),
        )
        .await
        .unwrap();
    // No automatic search after failures here: the test checks the blocklist by hand.
    let mut options = client.download_client_options().await.unwrap();
    assert!(options.auto_redownload_failed);
    options.auto_redownload_failed = false;
    client.put_download_client_options(&options).await.unwrap();

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
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    let pilot = client
        .episodes(added.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.season_number == 1 && e.episode_number == 1)
        .unwrap();
    let query = ReleaseQuery {
        series_id: added.id,
        season_number: None,
        episode_id: Some(pilot.id),
        complete_series: false,
        season_packs_only: false,
    };
    let search = || async {
        client
            .search_releases(&query)
            .await
            .unwrap()
            .releases
            .into_iter()
            .find(|r| r.guid == "nzb-77")
            .unwrap()
    };
    assert!(search().await.approved);
    client
        .grab_release(&GrabRequest {
            guid: "nzb-77".into(),
            indexer_id,
            episode_ids: Vec::new(),
        })
        .await
        .unwrap();

    // SABnzbd reports the job as failed: failed download handling records it, blocklists the
    // release and removes the job (Usenet jobs can always be removed).
    sab.lock().unwrap()["history"] = serde_json::json!([{"status": "Failed", "nzo_id": "SABnzbd_nzo_x1",
        "name": "Breaking.Bad.S01E01.720p.HDTV.x264-GRP", "category": "tv", "bytes": 5,
        "fail_message": "Aborted, cannot be completed", "storage": ""}]);
    assert_eq!(
        client.refresh_queue().await.unwrap(),
        "0 downloads imported, 0 need attention, 1 failed"
    );
    let failed = client.history(1, 50, Some("downloadFailed")).await.unwrap();
    assert_eq!(failed.records.len(), 1);
    assert_eq!(
        failed.records[0].data["message"],
        "Aborted, cannot be completed"
    );
    let blocked = client.blocklist(1, 50).await.unwrap();
    assert_eq!(blocked.total_records, 1);
    assert_eq!(
        blocked.records[0].source_title,
        "Breaking.Bad.S01E01.720p.HDTV.x264-GRP"
    );
    assert_eq!(blocked.records[0].indexer.as_deref(), Some("Mock Newznab"));
    assert_eq!(
        sab.lock().unwrap()["calls"],
        serde_json::json!(["history delete value=SABnzbd_nzo_x1 del_files=1 archive=0"])
    );
    let queue = client.queue().await.unwrap();
    assert_eq!(queue.items[0].tracked_state, "failed");
    // Handled once: the next check does nothing more.
    assert_eq!(
        client.refresh_queue().await.unwrap(),
        "0 downloads imported, 0 need attention"
    );
    // The release is rejected from now on.
    let rejected = search().await;
    assert!(!rejected.approved);
    assert!(
        rejected
            .rejections
            .contains(&"Release is blocklisted".to_string()),
        "{:?}",
        rejected.rejections
    );

    // Taken off the blocklist, it can be grabbed again; removing it from the queue with
    // "blocklist" blocklists it again and removes the job from SABnzbd.
    client
        .delete_blocklist(blocked.records[0].id)
        .await
        .unwrap();
    assert!(search().await.approved);
    client
        .grab_release(&GrabRequest {
            guid: "nzb-77".into(),
            indexer_id,
            episode_ids: Vec::new(),
        })
        .await
        .unwrap();
    {
        let mut s = sab.lock().unwrap();
        s["history"] = serde_json::json!([]);
        s["calls"] = serde_json::json!([]);
        s["queue"] = serde_json::json!([{"status": "Downloading", "nzo_id": "SABnzbd_nzo_x1",
            "filename": "Breaking.Bad.S01E01.720p.HDTV.x264-GRP", "cat": "tv", "mb": "1430", "mbleft": "715",
            "timeleft": "0:02:00", "priority": "Normal"}]);
    }
    let removed = client
        .remove_queue_item(
            "SABnzbd_nzo_x1",
            &QueueRemoval {
                remove_from_client: true,
                blocklist: true,
                skip_redownload: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        removed.message,
        "\"Breaking.Bad.S01E01.720p.HDTV.x264-GRP\": blocklisted the release, removed it from SABnzbd."
    );
    assert_eq!(
        sab.lock().unwrap()["calls"],
        serde_json::json!(["queue delete value=SABnzbd_nzo_x1 del_files=1"])
    );
    assert_eq!(client.blocklist(1, 50).await.unwrap().total_records, 1);
    // Clearing the blocklist.
    assert_eq!(
        client
            .remove_blocklist(&BlocklistRemoval {
                ids: vec![],
                all: true
            })
            .await
            .unwrap(),
        1
    );
    // A download Kitsunarr does not know cannot be removed.
    let missing = client
        .remove_queue_item("nope", &QueueRemoval::default())
        .await
        .unwrap_err();
    assert_eq!(status(missing), 404);
}

/// A local stand-in for SkyHook replaying recorded responses (Breaking Bad, the search).
async fn skyhook_mock() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    use axum::extract::{Path, Query, State};
    use axum::response::IntoResponse;
    use axum::routing::get;
    type Log = Arc<std::sync::Mutex<Vec<String>>>;
    fn fixture(name: &str) -> serde_json::Value {
        let path = format!(
            "{}/../../tests/fixtures/metadata/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }
    async fn show(
        State(log): State<Log>,
        Path((lang, id, season_type)): Path<(String, String, String)>,
    ) -> axum::response::Response {
        log.lock()
            .unwrap()
            .push(format!("shows/{lang}/{id}/{season_type}"));
        if id != "81189" {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        axum::Json(fixture("skyhook-show-81189.json")).into_response()
    }
    async fn search(
        State(log): State<Log>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::Json<serde_json::Value> {
        log.lock().unwrap().push(format!(
            "search {} {}",
            q.get("language").cloned().unwrap_or_default(),
            q.get("term").cloned().unwrap_or_default()
        ));
        axum::Json(fixture("skyhook-search-breaking-bad.json"))
    }
    let log: Log = Arc::default();
    let app = axum::Router::new()
        .route("/search/", get(search))
        .route("/shows/{lang}/{id}/{season_type}", get(show))
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), log)
}

#[tokio::test]
async fn thetvdb_titles_are_added_and_refreshed_through_skyhook() {
    let (skyhook, log) = skyhook_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvdb: skyhook,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let found = client
        .metadata_search(&SearchQuery {
            provider: MetadataProvider::Tvdb,
            term: "Breaking Bad".into(),
        })
        .await
        .unwrap();
    assert_eq!(found[0].title, "Breaking Bad");
    assert_eq!(found[0].id, "81189");
    // `tvdb:` terms look the title up directly.
    let direct = client
        .metadata_search(&SearchQuery {
            provider: MetadataProvider::Tvdb,
            term: "tvdb:81189".into(),
        })
        .await
        .unwrap();
    assert_eq!(direct.len(), 1);
    assert_eq!(
        log.lock().unwrap().as_slice(),
        ["search eng breaking bad", "shows/eng/81189/official"]
    );

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
            provider: MetadataProvider::Tvdb,
            id: "81189".into(),
            root_folder_path: dir.path().display().to_string(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    let episodes = client.episodes(added.id).await.unwrap();
    let pilot = episodes
        .iter()
        .find(|e| e.season_number == 1 && e.episode_number == 1)
        .unwrap();
    assert_eq!(pilot.title.as_deref(), Some("Pilot"));
    assert!(episodes.iter().any(|e| e.season_number == 0));
    let refreshed = client.refresh_series(added.id).await.unwrap();
    assert_eq!(refreshed.added_episodes, 0);
    assert_eq!(refreshed.updated_episodes as usize, episodes.len());

    // Sonarr's metadata language: the preferred one for searches and new series, each series
    // its own (changing it refreshes in the new language).
    assert_eq!(
        client
            .series_detail(added.id)
            .await
            .unwrap()
            .metadata_language,
        Some(1)
    );
    client
        .put_metadata_sources(&kitsunarr_domain::api::settings::MetadataSourcesSettings {
            preferred_metadata_language: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    let bad = client
        .put_metadata_sources(&kitsunarr_domain::api::settings::MetadataSourcesSettings {
            preferred_metadata_language: Some(9999),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(status(bad), 422);
    log.lock().unwrap().clear();
    client
        .metadata_search(&SearchQuery {
            provider: MetadataProvider::Tvdb,
            term: "Breaking Bad".into(),
        })
        .await
        .unwrap();
    assert_eq!(log.lock().unwrap().as_slice(), ["search fra breaking bad"]);
    // The series keeps English until it is changed.
    client.refresh_series(added.id).await.unwrap();
    assert!(
        log.lock()
            .unwrap()
            .contains(&"shows/eng/81189/official".to_string())
    );
    let changed = client
        .update_series(
            added.id,
            &kitsunarr_domain::api::library::SeriesUpdate {
                metadata_language: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(changed.metadata_language, Some(2));
    let mut refreshed_in_french = false;
    for _ in 0..100 {
        if log
            .lock()
            .unwrap()
            .contains(&"shows/fra/81189/official".to_string())
        {
            refreshed_in_french = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(refreshed_in_french, "{:?}", log.lock().unwrap());

    // TMDB and OMDb need the user's keys: unavailable, then available once a key is saved,
    // which never comes back in clear.
    let providers = client.metadata_providers().await.unwrap();
    let tmdb = providers
        .iter()
        .find(|c| c.provider == MetadataProvider::TmdbTv)
        .unwrap();
    assert!(!tmdb.available);
    let missing = client
        .metadata_search(&SearchQuery {
            provider: MetadataProvider::TmdbTv,
            term: "x".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(status(missing), 400);
    let saved = client
        .put_metadata_sources(&kitsunarr_domain::api::settings::MetadataSourcesSettings {
            tmdb_api_key: Some("secret-key".into()),
            omdb_api_key: None,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(saved.tmdb_api_key.as_deref(), Some(REDACTED));
    assert_eq!(saved.omdb_api_key, None);
    // Sending the placeholder back keeps the key.
    client.put_metadata_sources(&saved).await.unwrap();
    let providers = client.metadata_providers().await.unwrap();
    assert!(
        providers
            .iter()
            .find(|c| c.provider == MetadataProvider::TmdbMovie)
            .unwrap()
            .available
    );
    assert!(
        !providers
            .iter()
            .find(|c| c.provider == MetadataProvider::Omdb)
            .unwrap()
            .available
    );
    // With a key, TMDB is queried (and is unreachable here).
    let unreachable = client
        .metadata_search(&SearchQuery {
            provider: MetadataProvider::TmdbTv,
            term: "x".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(status(unreachable), 502);
}

/// Records webhook deliveries.
async fn webhook_mock() -> (String, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
    use axum::extract::State;
    use axum::routing::post;
    type Log = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
    async fn hook(State(log): State<Log>, body: String) -> &'static str {
        log.lock()
            .unwrap()
            .push(serde_json::from_str(&body).unwrap());
        "ok"
    }
    let log: Log = Arc::default();
    let app = axum::Router::new()
        .route("/hook", post(hook))
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/hook"), log)
}

async fn wait_for(log: &Arc<std::sync::Mutex<Vec<serde_json::Value>>>, n: usize) {
    for _ in 0..100 {
        if log.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected {n} deliveries, got {:#?}", log.lock().unwrap());
}

#[tokio::test]
async fn notifications_follow_series_grabs_and_imports() {
    use kitsunarr_domain::api::search::{GrabRequest, IndexerTestRequest, ReleaseQuery};
    let tvmaze = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let (hook_url, hook) = webhook_mock().await;
    let (indexer_url, sab_port, sab) = usenet_mocks().await;

    // The template lists the services; Test sends Sonarr's test payload.
    let templates: Vec<serde_json::Value> =
        client.get("/providers/notifications/schema").await.unwrap();
    assert!(templates.iter().any(|t| t["implementation"] == "Discord"));
    let tested = client
        .test_notification(&IndexerTestRequest {
            id: None,
            implementation: "Webhook".into(),
            settings: serde_json::from_value(serde_json::json!({"url": hook_url})).unwrap(),
        })
        .await
        .unwrap();
    assert!(tested.ok, "{}", tested.message);
    assert_eq!(hook.lock().unwrap()[0]["eventType"], "Test");
    let unreachable = client
        .test_notification(&IndexerTestRequest {
            id: None,
            implementation: "Webhook".into(),
            settings: serde_json::from_value(serde_json::json!({"url": "http://127.0.0.1:9/x"}))
                .unwrap(),
        })
        .await
        .unwrap();
    assert!(!unreachable.ok);

    let created: serde_json::Value = client
        .post(
            "/providers/notifications",
            &serde_json::json!({
                "name": "Hook", "implementation": "Webhook",
                "options": {"onGrab": true, "onDownload": true, "onSeriesAdd": true},
                "settings": {"url": hook_url}
            }),
        )
        .await
        .unwrap();
    assert_eq!(created["supported"], true);
    let indexer: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Newznab", "implementation": "Newznab",
                "options": {"enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "n3wznab-key", "categories": [5040]}
            }),
        )
        .await
        .unwrap();
    let _: serde_json::Value = client
        .post(
            "/providers/downloadclients",
            &serde_json::json!({
                "name": "SABnzbd", "implementation": "Sabnzbd", "enabled": true,
                "settings": {"host": "127.0.0.1", "port": sab_port, "apiKey": "sab-key", "tvCategory": "tv"}
            }),
        )
        .await
        .unwrap();
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
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    wait_for(&hook, 2).await;
    {
        let seen = hook.lock().unwrap();
        assert_eq!(seen[1]["eventType"], "SeriesAdd");
        assert_eq!(seen[1]["series"]["title"], "Breaking Bad");
        assert_eq!(seen[1]["series"]["tvdbId"], 81189);
    }
    let pilot = client
        .episodes(added.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.season_number == 1 && e.episode_number == 1)
        .unwrap();
    client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
            complete_series: false,
            season_packs_only: false,
        })
        .await
        .unwrap();
    client
        .grab_release(&GrabRequest {
            guid: "nzb-77".into(),
            indexer_id: indexer["id"].as_i64().unwrap(),
            episode_ids: Vec::new(),
        })
        .await
        .unwrap();
    wait_for(&hook, 3).await;
    {
        let seen = hook.lock().unwrap();
        assert_eq!(seen[2]["eventType"], "Grab");
        assert_eq!(seen[2]["episodes"][0]["title"], "Pilot");
        assert_eq!(seen[2]["release"]["quality"], "HDTV-720p");
        assert_eq!(seen[2]["downloadClient"], "SABnzbd");
        assert_eq!(seen[2]["downloadId"], "SABnzbd_nzo_x1");
    }
    let downloads = tempfile::tempdir().unwrap();
    let job = downloads
        .path()
        .join("Breaking.Bad.S01E01.720p.HDTV.x264-GRP");
    std::fs::create_dir_all(&job).unwrap();
    let video = job.join("Breaking.Bad.S01E01.720p.HDTV.x264-GRP.mkv");
    std::fs::write(&video, b"video").unwrap();
    sab.lock().unwrap()["history"] = serde_json::json!([{"status": "Completed", "nzo_id": "SABnzbd_nzo_x1",
        "name": "Breaking.Bad.S01E01.720p.HDTV.x264-GRP", "category": "tv", "bytes": 5,
        "storage": video.display().to_string()}]);
    client.refresh_queue().await.unwrap();
    wait_for(&hook, 4).await;
    let seen = hook.lock().unwrap();
    assert_eq!(seen[3]["eventType"], "Download");
    assert_eq!(seen[3]["isUpgrade"], false);
    assert_eq!(seen[3]["episodeFile"]["quality"], "HDTV-720p");
    assert_eq!(
        seen[3]["release"]["releaseTitle"],
        "Breaking.Bad.S01E01.720p.HDTV.x264-GRP"
    );
}

#[tokio::test]
async fn disk_scan_rename_and_file_deletion() {
    let tvmaze = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
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
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    let detail = client.series_detail(added.id).await.unwrap();
    let folder = std::path::PathBuf::from(&detail.item.path);
    std::fs::create_dir_all(folder.join("Season 01")).unwrap();
    for name in [
        "Season 01/Breaking.Bad.S01E01.720p.HDTV.x264-GRP.mkv",
        "Season 01/Breaking.Bad.S01E02.1080p.BluRay.x264-GRP.mkv",
        "Season 01/notes.mkv",
    ] {
        std::fs::write(folder.join(name), b"video").unwrap();
    }

    let scanned = client.rescan_series(added.id).await.unwrap();
    assert_eq!(
        scanned.message,
        "2 files added, 0 missing files removed, 0 sizes updated."
    );
    assert_eq!(scanned.problems.len(), 1, "{:?}", scanned.problems);
    let episodes = client.episodes(added.id).await.unwrap();
    let file_of = |s: i32, e: i32| {
        episodes
            .iter()
            .find(|x| x.season_number == s && x.episode_number == e)
            .and_then(|x| x.file.clone())
    };
    assert_eq!(file_of(1, 1).unwrap().quality_name, "HDTV-720p");
    assert_eq!(file_of(1, 2).unwrap().quality_name, "Bluray-1080p");
    // Scanning again changes nothing.
    assert_eq!(
        client.rescan_series(added.id).await.unwrap().message,
        "0 files added, 0 missing files removed, 0 sizes updated."
    );

    // Rename to the naming settings.
    let mut naming = client.naming().await.unwrap();
    naming.rename_episodes = true;
    naming.season_folder_format = "Season {season:00}".into();
    client.put_naming(&naming).await.unwrap();
    let previews = client.rename_preview(added.id, None).await.unwrap();
    assert_eq!(previews.len(), 2);
    assert_eq!(
        previews[0].new_path,
        "Season 01/Breaking Bad - S01E01 - Pilot HDTV-720p.mkv"
    );
    assert_eq!(
        client
            .rename_preview(added.id, Some(2))
            .await
            .unwrap()
            .len(),
        0
    );
    let renamed = client.rename_files(added.id, &[]).await.unwrap();
    assert_eq!(renamed.message, "2 files renamed.");
    assert!(renamed.problems.is_empty(), "{:?}", renamed.problems);
    assert!(
        folder
            .join("Season 01/Breaking Bad - S01E01 - Pilot HDTV-720p.mkv")
            .is_file()
    );
    assert!(
        !folder
            .join("Season 01/Breaking.Bad.S01E01.720p.HDTV.x264-GRP.mkv")
            .exists()
    );
    assert!(
        client
            .rename_preview(added.id, None)
            .await
            .unwrap()
            .is_empty()
    );
    let history = client
        .history(1, 50, Some("episodeFileRenamed"))
        .await
        .unwrap();
    assert_eq!(history.records.len(), 2);

    // A file that disappeared is forgotten by the next scan.
    std::fs::remove_file(folder.join("Season 01/Breaking Bad - S01E01 - Pilot HDTV-720p.mkv"))
        .unwrap();
    assert_eq!(
        client.rescan_series(added.id).await.unwrap().message,
        "0 files added, 1 missing files removed, 0 sizes updated."
    );
    // Deleting a file removes it from disk and the library.
    let second = client
        .episodes(added.id)
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.season_number == 1 && x.episode_number == 2)
        .and_then(|x| x.file)
        .unwrap();
    client
        .delete_episode_file(added.id, second.id)
        .await
        .unwrap();
    assert!(!folder.join(&second.relative_path).exists());
    let deleted = client
        .history(1, 50, Some("episodeFileDeleted"))
        .await
        .unwrap();
    assert_eq!(deleted.records.len(), 2);
    assert!(
        client
            .episodes(added.id)
            .await
            .unwrap()
            .iter()
            .all(|e| e.file.is_none())
    );

    // Deleting the series with its files removes the folder.
    client.delete_series_with_files(added.id).await.unwrap();
    assert!(!folder.exists());
    assert!(dir.path().is_dir());
}

#[tokio::test]
async fn multi_season_packs_serve_season_searches_and_keep_their_source() {
    use kitsunarr_domain::api::search::{GrabRequest, ReleaseQuery};
    let tvmaze = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let (indexer_url, sab_port, sab) =
        usenet_mocks_with("Breaking.Bad.S01-S02.720p.HDTV.x264-GRP").await;
    let indexer: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Newznab", "implementation": "Newznab",
                "options": {"enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "n3wznab-key", "categories": [5040]}
            }),
        )
        .await
        .unwrap();
    let _: serde_json::Value = client
        .post(
            "/providers/downloadclients",
            &serde_json::json!({
                "name": "SABnzbd", "implementation": "Sabnzbd", "enabled": true,
                "settings": {"host": "127.0.0.1", "port": sab_port, "apiKey": "sab-key", "tvCategory": "tv"}
            }),
        )
        .await
        .unwrap();
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
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    let season_1 = ReleaseQuery {
        series_id: added.id,
        season_number: Some(1),
        episode_id: None,
        complete_series: false,
        season_packs_only: false,
    };
    let found = client.search_releases(&season_1).await.unwrap();
    let pack = found.releases.iter().find(|r| r.guid == "nzb-77").unwrap();
    assert!(pack.approved, "{:?}", pack.rejections);
    // The pack stands for season 1 in a season-1 search.
    assert!(
        pack.mapped_episodes.iter().all(|e| e.starts_with("S01")),
        "{:?}",
        pack.mapped_episodes
    );
    assert_eq!(pack.mapped_episodes.len(), 7);

    // Disabled in the indexer options, multi-season packs are rejected.
    let mut options = client.indexer_options().await.unwrap();
    options.enable_multi_season_releases = false;
    client.put_indexer_options(&options).await.unwrap();
    let rejected = client.search_releases(&season_1).await.unwrap();
    assert!(
        rejected.releases[0]
            .rejections
            .contains(&"Multi-season releases are not enabled".to_string())
    );
    options.enable_multi_season_releases = true;
    client.put_indexer_options(&options).await.unwrap();
    client.search_releases(&season_1).await.unwrap();
    client
        .grab_release(&GrabRequest {
            guid: "nzb-77".into(),
            indexer_id: indexer["id"].as_i64().unwrap(),
            episode_ids: Vec::new(),
        })
        .await
        .unwrap();

    // The completed pack holds two seasons: only the grabbed season 1 is imported, by copy,
    // and the job stays in SABnzbd.
    let downloads = tempfile::tempdir().unwrap();
    let job = downloads
        .path()
        .join("Breaking.Bad.S01-S02.720p.HDTV.x264-GRP");
    std::fs::create_dir_all(&job).unwrap();
    for n in 1..=7 {
        std::fs::write(
            job.join(format!("Breaking.Bad.S01E{n:02}.720p.HDTV.x264-GRP.mkv")),
            b"s1",
        )
        .unwrap();
    }
    std::fs::write(
        job.join("Breaking.Bad.S02E01.720p.HDTV.x264-GRP.mkv"),
        b"s2",
    )
    .unwrap();
    // Pack trimming: an unmonitored episode is skipped, and the download still counts as
    // imported.
    let mut media = client.media_management().await.unwrap();
    media.skip_unmonitored_episodes_from_packs = true;
    client.put_media_management(&media).await.unwrap();
    let e7 = client
        .episodes(added.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.season_number == 1 && e.episode_number == 7)
        .unwrap();
    client
        .set_episodes_monitored(&EpisodesMonitoredUpdate {
            episode_ids: vec![e7.id],
            monitored: false,
        })
        .await
        .unwrap();
    sab.lock().unwrap()["history"] = serde_json::json!([{"status": "Completed", "nzo_id": "SABnzbd_nzo_x1",
        "name": "Breaking.Bad.S01-S02.720p.HDTV.x264-GRP", "category": "tv", "bytes": 15,
        "storage": job.display().to_string()}]);
    assert_eq!(
        client.refresh_queue().await.unwrap(),
        "1 downloads imported, 0 need attention"
    );
    let episodes = client.episodes(added.id).await.unwrap();
    assert_eq!(
        episodes
            .iter()
            .filter(|e| e.season_number == 1 && e.file.is_some())
            .count(),
        6
    );
    assert!(
        episodes
            .iter()
            .filter(|e| e.season_number == 2)
            .all(|e| e.file.is_none())
    );
    assert!(
        job.join("Breaking.Bad.S01E01.720p.HDTV.x264-GRP.mkv")
            .exists()
    );
    assert!(
        sab.lock().unwrap()["calls"].is_null(),
        "the job is not removed"
    );
}

#[tokio::test]
async fn complete_series_search_reads_packs_of_the_series() {
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use kitsunarr_domain::api::search::ReleaseQuery;
    type Log = Arc<std::sync::Mutex<Vec<String>>>;
    async fn torznab(
        State(log): State<Log>,
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
        log.lock().unwrap().push(format!(
            "{} {}",
            q.get("t").cloned().unwrap_or_default(),
            q.get("q").cloned().unwrap_or_default()
        ));
        let item = |title: &str, guid: &str, hash: &str| {
            format!(
                r#"<item><title>{title}</title><guid>{guid}</guid>
<link>magnet:?xt=urn:btih:{hash}</link><pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate><size>50000000000</size>
<enclosure url="magnet:?xt=urn:btih:{hash}" length="50000000000" type="application/x-bittorrent;x-scheme-handler/magnet" />
<torznab:attr name="seeders" value="40" /></item>"#
            )
        };
        let feed = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>{}{}{}</channel></rss>"#,
            item(
                "Breaking.Bad.S01-S05.1080p.BluRay.x264-GRP",
                "pack",
                "a12fe1c06bba254a9dc9f519b335aa7c1367a88a"
            ),
            item(
                "Breaking.Bad.S01E01.720p.HDTV.x264-GRP",
                "episode",
                "b12fe1c06bba254a9dc9f519b335aa7c1367a88a"
            ),
            item(
                "Breaking.Bad.S02.1080p.BluRay.x264-GRP",
                "season",
                "c12fe1c06bba254a9dc9f519b335aa7c1367a88a"
            ),
        );
        ([("content-type", "application/rss+xml")], feed).into_response()
    }
    let log: Log = Arc::default();
    let app = axum::Router::new()
        .route("/api", get(torznab))
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let indexer_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let tvmaze = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
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
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    let query = ReleaseQuery {
        series_id: added.id,
        season_number: None,
        episode_id: None,
        complete_series: true,
        season_packs_only: false,
    };
    // Torrent indexers only.
    let none = client.search_releases(&query).await.unwrap_err();
    assert_eq!(status(none), 422);
    let _: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Torznab", "implementation": "Torznab",
                "options": {"enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "k", "categories": [5040]}
            }),
        )
        .await
        .unwrap();
    let found = client.search_releases(&query).await.unwrap();
    // The single episode is not a run of the series and is left out; the season pack shows
    // in interactive searches.
    let guids: Vec<&str> = found.releases.iter().map(|r| r.guid.as_str()).collect();
    assert_eq!(guids, vec!["pack", "season"], "{:?}", found.releases);
    let pack = &found.releases[0];
    assert!(pack.approved, "{:?}", pack.rejections);
    assert_eq!(pack.season_numbers, vec![1, 2, 3, 4, 5]);
    assert_eq!(pack.mapped_episodes.len(), 62);
    assert_eq!(found.releases[1].season_numbers, vec![2]);
    let queries = log.lock().unwrap().clone();
    for q in [
        "search Breaking Bad",
        "search Breaking Bad S01-S05",
        "search Breaking Bad complete",
        "search Breaking Bad S03",
    ] {
        assert!(queries.contains(&q.to_string()), "{q}: {queries:?}");
    }
    // A season-pack search lists full-season releases only (the S01-S05 pack stands for
    // season 2 there).
    let packs = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: Some(2),
            episode_id: None,
            complete_series: false,
            season_packs_only: true,
        })
        .await
        .unwrap();
    let mut guids: Vec<&str> = packs.releases.iter().map(|r| r.guid.as_str()).collect();
    guids.sort_unstable();
    assert_eq!(guids, vec!["pack", "season"]);
    assert!(packs.releases.iter().all(|r| r.full_season));
    let invalid = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: None,
            complete_series: false,
            season_packs_only: true,
        })
        .await
        .unwrap_err();
    assert_eq!(status(invalid), 422);
    // Tanukarr's missing searches try a complete-series pack first for a series missing more
    // than one season (no download client here: the grab fails, but the pack is chosen first).
    log.lock().unwrap().clear();
    let started = client
        .search_wanted(&kitsunarr_domain::api::wanted::WantedSearchRequest {
            list: kitsunarr_domain::api::wanted::WantedList::Missing,
            episode_ids: vec![],
            monitored: None,
        })
        .await
        .unwrap();
    assert!(started.episodes > 0, "{started:?}");
    let mut complete_first = false;
    for _ in 0..200 {
        let queries = log.lock().unwrap().clone();
        if let Some(complete) = queries
            .iter()
            .position(|q| q == "search Breaking Bad complete")
        {
            // The complete-series search (series by id, then its titles) comes first.
            complete_first = complete < 4;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(complete_first, "{:?}", log.lock().unwrap());
    // Wait for the search to finish (the next one would conflict).
    for _ in 0..200 {
        if server.state.wanted_search.try_lock().is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn fake_torrents_are_rejected_before_grabbing() {
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use kitsunarr_domain::api::search::{GrabRequest, ReleaseQuery};
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
        let link = format!("{}/dl/fake.torrent", base.lock().unwrap());
        let feed = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>
<item><title>Breaking.Bad.S01E01.720p.HDTV.x264-GRP</title><guid>fake</guid><link>{link}</link>
<pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate><size>734003200</size>
<enclosure url="{link}" length="734003200" type="application/x-bittorrent" />
<torznab:attr name="seeders" value="40" /></item></channel></rss>"#
        );
        ([("content-type", "application/rss+xml")], feed).into_response()
    }
    async fn torrent() -> axum::response::Response {
        let f: &[u8] = b"d4:infod5:filesld6:lengthi734003200e4:pathl38:Breaking.Bad.S01E01.720p.HDTV.x264.mkveed6:lengthi4096e4:pathl9:setup.exeeee4:name4:Show12:piece lengthi16384e6:pieces20:\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07\x07ee";
        ([("content-type", "application/x-bittorrent")], f).into_response()
    }
    let base: Arc<std::sync::Mutex<String>> = Arc::default();
    let app = axum::Router::new()
        .route("/api", get(torznab))
        .route("/dl/fake.torrent", get(torrent))
        .with_state(base.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let indexer_url = format!("http://{}", listener.local_addr().unwrap());
    *base.lock().unwrap() = indexer_url.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let tvmaze = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let indexer: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Torznab", "implementation": "Torznab",
                "options": {"enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "k", "categories": [5040]}
            }),
        )
        .await
        .unwrap();
    // The client is never reached.
    let _: serde_json::Value = client
        .post(
            "/providers/downloadclients",
            &serde_json::json!({"name": "qBittorrent", "implementation": "QBittorrent", "enabled": true,
                "settings": {"host": "127.0.0.1", "port": 9}}),
        )
        .await
        .unwrap();
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
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    let pilot = client
        .episodes(added.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.season_number == 1 && e.episode_number == 1)
        .unwrap();
    client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
            complete_series: false,
            season_packs_only: false,
        })
        .await
        .unwrap();
    match client
        .grab_release(&GrabRequest {
            guid: "fake".into(),
            indexer_id: indexer["id"].as_i64().unwrap(),
            episode_ids: Vec::new(),
        })
        .await
        .unwrap_err()
    {
        ClientError::Api { status, body } => {
            assert_eq!(status, 422);
            assert!(
                body.message.contains(
                    "Fake release: Contains an executable or script payload: 'setup.exe'"
                ),
                "{}",
                body.message
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    // With the protection off, the grab goes ahead (and fails at the unreachable client).
    let mut options = client.indexer_options().await.unwrap();
    options.enable_fake_release_protection = false;
    client.put_indexer_options(&options).await.unwrap();
    match client
        .grab_release(&GrabRequest {
            guid: "fake".into(),
            indexer_id: indexer["id"].as_i64().unwrap(),
            episode_ids: Vec::new(),
        })
        .await
        .unwrap_err()
    {
        ClientError::Api { body, .. } => {
            assert!(!body.message.contains("Fake release"), "{}", body.message)
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// A custom import list (Sonarr's `CustomImport`) whose contents the test changes.
async fn custom_list_mock() -> (String, Arc<std::sync::Mutex<serde_json::Value>>) {
    use axum::extract::State;
    use axum::routing::get;
    type Body = Arc<std::sync::Mutex<serde_json::Value>>;
    async fn list(State(b): State<Body>) -> axum::Json<serde_json::Value> {
        axum::Json(b.lock().unwrap().clone())
    }
    let body: Body = Arc::new(std::sync::Mutex::new(serde_json::json!([])));
    let app = axum::Router::new()
        .route("/list", get(list))
        .with_state(body.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/list"), body)
}

#[tokio::test]
async fn import_lists_add_titles_respect_exclusions_and_clean_the_library() {
    use kitsunarr_domain::api::import_lists::ImportListExclusionItem;
    use kitsunarr_domain::api::search::IndexerTestRequest;
    use kitsunarr_domain::api::settings::{
        ImportListOptionsSettings, ListSyncLevel, ProviderItem, ProviderKind,
    };
    let (skyhook, log) = skyhook_mock().await;
    let (list_url, list) = custom_list_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvdb: skyhook,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().display().to_string();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    let tag = client.create_tag("from-list").await.unwrap();
    *list.lock().unwrap() = serde_json::json!([
        {"title": "Breaking Bad", "tvdbId": 81189},
        {"title": "Excluded show", "tvdbId": 424242},
        {"title": "", "tvdbId": 0}
    ]);

    // The template is filled in as the editor does.
    let mut item: ProviderItem = client
        .provider_schema("importlists")
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.implementation == "CustomImport")
        .unwrap();
    item.name = "My list".into();
    item.settings
        .insert("baseUrl".into(), list_url.clone().into());
    item.options
        .insert("rootFolderPath".into(), root.clone().into());
    item.options.insert("qualityProfileId".into(), 1.into());
    item.options
        .insert("searchForMissingEpisodes".into(), false.into());
    item.tags = vec![tag.id];
    let test = client
        .test_import_list(&IndexerTestRequest {
            id: None,
            implementation: item.implementation.clone(),
            settings: item.settings.clone(),
        })
        .await
        .unwrap();
    assert!(test.ok, "{}", test.message);
    assert_eq!(test.message, "Read 3 shows.");
    let created = client.create_provider("importlists", &item).await.unwrap();
    assert!(created.supported);

    client
        .create_import_list_exclusion(&ImportListExclusionItem {
            id: 0,
            provider: "tvdb".into(),
            external_id: "424242".into(),
            title: Some("Excluded show".into()),
        })
        .await
        .unwrap();
    let result = client.sync_import_lists(None).await.unwrap();
    assert!(
        result.message.contains("1 titles added"),
        "{}",
        result.message
    );
    // The excluded show was never looked up.
    assert!(!log.lock().unwrap().iter().any(|l| l.contains("424242")));
    let series = client.series(&LibraryQuery::default()).await.unwrap();
    assert_eq!(series.records.len(), 1);
    assert_eq!(series.records[0].title, "Breaking Bad");
    assert_eq!(series.records[0].tags, vec![tag.id]);
    let series_id = series.records[0].id;

    let entries = client.import_list_entries(created.id).await.unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].series_id, Some(series_id));
    assert!(entries[1].excluded);
    let status = client.import_list_status().await.unwrap();
    assert!(status[0].last_info_sync.is_some() && status[0].last_error.is_none());
    assert_eq!(status[0].item_count, 3);

    // A second sync adds nothing; a scheduled run within the interval reads nothing.
    let again = client.sync_import_lists(Some(created.id)).await.unwrap();
    assert!(
        again.message.contains("0 titles added"),
        "{}",
        again.message
    );
    assert_eq!(
        kitsunarr_api::import_lists::sync(&server.state, None, false)
            .await
            .unwrap(),
        "No list was due for a sync"
    );

    // Clean library: the series leaves the list and is unmonitored.
    client
        .put_import_list_options(&ImportListOptionsSettings {
            list_sync_level: ListSyncLevel::KeepAndUnmonitor,
            list_sync_tag: None,
        })
        .await
        .unwrap();
    *list.lock().unwrap() = serde_json::json!([{"title": "Excluded show", "tvdbId": 424242}]);
    let cleaned = client.sync_import_lists(None).await.unwrap();
    assert!(cleaned.message.contains("1 cleaned"), "{}", cleaned.message);
    let series = client.series(&LibraryQuery::default()).await.unwrap();
    assert!(!series.records[0].monitored);

    // A failing list keeps its previous membership and reports the error.
    *list.lock().unwrap() = serde_json::json!({"not": "a list"});
    let failed = client.sync_import_lists(None).await.unwrap();
    assert!(
        failed.message.contains("failed: My list"),
        "{}",
        failed.message
    );
    let status = client.import_list_status().await.unwrap();
    assert!(status[0].last_error.is_some());
    assert_eq!(status[0].item_count, 1);
    let _ = ProviderKind::ImportList;
}

#[tokio::test]
async fn paused_automation_skips_rss_and_scheduled_list_syncs() {
    use kitsunarr_domain::api::settings::LibraryToolsSettings;
    let server = start().await;
    let client = signed_in(&server).await;
    assert_eq!(
        client.library_tools().await.unwrap(),
        LibraryToolsSettings::default()
    );
    let saved = client
        .put_library_tools(&LibraryToolsSettings {
            automation_paused: true,
            match_external_ids: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(saved.automation_paused);
    assert_eq!(
        kitsunarr_api::rss::sync(&server.state).await.unwrap(),
        kitsunarr_api::library_tools::PAUSED
    );
    assert_eq!(
        kitsunarr_api::import_lists::sync(&server.state, None, false)
            .await
            .unwrap(),
        kitsunarr_api::library_tools::PAUSED
    );
    // "Sync now" is a manual action.
    assert_ne!(
        client.sync_import_lists(None).await.unwrap().message,
        kitsunarr_api::library_tools::PAUSED
    );
    assert!(client.recent_searches().await.unwrap().is_empty());
}

#[tokio::test]
async fn series_naming_options_round_trip_and_validate() {
    use kitsunarr_domain::api::library_tools::SeriesNamingOptions;
    let (skyhook, _log) = skyhook_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvdb: skyhook,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
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
            provider: MetadataProvider::Tvdb,
            id: "81189".into(),
            root_folder_path: dir.path().display().to_string(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
        })
        .await
        .unwrap();
    let initial = client.series_naming(added.id).await.unwrap();
    assert_eq!(initial.renaming_mode, "default");
    assert!(initial.season_titles.is_empty());
    let saved = client
        .put_series_naming(
            added.id,
            &SeriesNamingOptions {
                renaming_mode: "manual".into(),
                season_titles: [(1, " Pilot Season ".to_string()), (2, String::new())].into(),
                keep_versions: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.renaming_mode, "manual");
    assert!(saved.keep_versions);
    assert_eq!(
        saved.season_titles,
        [(1, "Pilot Season".to_string())].into()
    );
    for bad in [
        SeriesNamingOptions {
            renaming_mode: "sometimes".into(),
            ..Default::default()
        },
        SeriesNamingOptions {
            renaming_mode: "default".into(),
            season_titles: [(99, "No such season".to_string())].into(),
            ..Default::default()
        },
    ] {
        assert_eq!(
            status(client.put_series_naming(added.id, &bad).await.unwrap_err()),
            400
        );
    }
}

/// Sonarr `/api/v3` as Overseerr (lookup, add with season choices, update) and Prowlarr
/// (indexer sync) use it.
#[tokio::test]
async fn sonarr_v3_api_serves_overseerr_and_prowlarr_flows() {
    use serde_json::{Value, json};
    let (skyhook, _log) = skyhook_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvdb: skyhook,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let http = reqwest::Client::new();
    let url = |p: &str| format!("{}/api/v3{p}", server.base);
    let key = server.api_key.clone();
    let get = |p: &str| http.get(url(p)).header("X-Api-Key", &key).send();
    // Without a key: 401.
    assert_eq!(
        http.get(url("/system/status"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let status: Value = get("/system/status").await.unwrap().json().await.unwrap();
    assert_eq!(status["version"], "4.0.0.0");
    assert_eq!(status["appName"], "Kitsunarr");
    let profiles: Value = get("/qualityprofile").await.unwrap().json().await.unwrap();
    assert!(
        profiles
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "Any")
    );
    assert_eq!(
        get("/languageprofile")
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()[0]["id"],
        1
    );
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().display().to_string();
    signed_in(&server)
        .await
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    let roots: Value = get("/rootfolder").await.unwrap().json().await.unwrap();
    assert_eq!(roots[0]["path"], root.as_str());
    let tag: Value = http
        .post(url("/tag"))
        .header("X-Api-Key", &key)
        .json(&json!({"label": "Overseerr"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tag["label"], "overseerr");

    // Overseerr: lookup by TheTVDB id, then add with only season 1 monitored.
    let found: Value = get("/series/lookup?term=tvdb:81189")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found[0]["title"], "Breaking Bad");
    assert!(found[0].get("id").is_none());
    assert!(found[0]["seasons"].as_array().unwrap().len() > 1);
    let added = http
        .post(url("/series"))
        .header("X-Api-Key", &key)
        .json(&json!({
            "tvdbId": 81189, "title": "Breaking Bad", "qualityProfileId": 1, "languageProfileId": 1,
            "rootFolderPath": root, "seasonFolder": true, "monitored": true, "seriesType": "standard",
            "tags": [tag["id"]], "seasons": [{"seasonNumber": 1, "monitored": true}, {"seasonNumber": 2, "monitored": false}],
            "addOptions": {"ignoreEpisodesWithFiles": true, "searchForMissingEpisodes": false}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(added.status(), 201);
    let added: Value = added.json().await.unwrap();
    let id = added["id"].as_i64().unwrap();
    // The v5 routes the compression worker calls back on, with the same key.
    let v5: Value = http
        .get(format!("{}/api/v5/series/{id}", server.base))
        .header("X-Api-Key", &key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v5["title"], "Breaking Bad");
    assert!(v5["path"].as_str().is_some_and(|p| !p.is_empty()), "{v5}");
    assert_eq!(
        http.get(format!("{}/api/v5/episodefile?seriesId={id}", server.base))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(added["tvdbId"], 81189);
    let season = |v: &Value, n: i64| {
        v["seasons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["seasonNumber"] == n)
            .unwrap()["monitored"]
            .clone()
    };
    assert_eq!(season(&added, 1), true);
    assert_eq!(season(&added, 2), false);
    // Now in the library: lookups and filters return it with its id.
    let again: Value = get("/series/lookup?term=tvdb:81189")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again[0]["id"], id);
    let listed: Value = get("/series?tvdbId=81189")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    // Overseerr's later request for season 2: PUT /series with the id in the body.
    let mut body = again[0].clone();
    body["seasons"] =
        json!([{"seasonNumber": 1, "monitored": true}, {"seasonNumber": 2, "monitored": true}]);
    let updated: Value = http
        .put(url("/series"))
        .header("X-Api-Key", &key)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(season(&updated, 2), true);
    let episodes: Value = get(&format!("/episode?seriesId={id}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let first = &episodes.as_array().unwrap()[0];
    let one: Value = get(&format!("/episode/{}", first["id"]))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one["id"], first["id"]);
    assert_eq!(
        get(&format!("/episodefile?seriesId={id}"))
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        json!([])
    );
    let command = http
        .post(url("/command"))
        .header("X-Api-Key", &key)
        .json(&json!({"name": "RescanSeries", "seriesId": id}))
        .send()
        .await
        .unwrap();
    assert_eq!(command.status(), 201);
    let queue: Value = get("/queue?page=1&pageSize=50")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queue["totalRecords"], 0);

    // Prowlarr: schema, add a Torznab indexer, read it back (key masked), update, delete.
    let schema: Value = get("/indexer/schema").await.unwrap().json().await.unwrap();
    assert!(
        schema
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["implementation"] == "Torznab")
    );
    let created = http
        .post(url("/indexer"))
        .header("X-Api-Key", &key)
        .json(&json!({
            "name": "Nyaa (Prowlarr)", "implementation": "Torznab", "configContract": "TorznabSettings",
            "enableRss": true, "enableAutomaticSearch": true, "enableInteractiveSearch": false, "priority": 25,
            "tags": [], "fields": [
                {"name": "baseUrl", "value": "http://prowlarr:9696/1/"}, {"name": "apiPath", "value": "/api"},
                {"name": "apiKey", "value": "prowlarr-secret"}, {"name": "categories", "value": [5030, 5040]},
                {"name": "animeCategories", "value": [5070]}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201, "{:?}", created.text().await);
    let created: Value = created.json().await.unwrap();
    let iid = created["id"].as_i64().unwrap();
    assert_eq!(created["enableInteractiveSearch"], false);
    let field = |v: &Value, n: &str| {
        v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == n)
            .unwrap()["value"]
            .clone()
    };
    assert_eq!(field(&created, "apiKey"), "********");
    let mut edit = created.clone();
    edit["name"] = json!("Nyaa");
    let edited: Value = http
        .put(url(&format!("/indexer/{iid}")))
        .header("X-Api-Key", &key)
        .json(&edit)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(edited["name"], "Nyaa");
    let stored = kitsunarr_db::providers::get(
        server.state.db.pool(),
        kitsunarr_domain::api::settings::ProviderKind::Indexer,
        iid,
    )
    .await
    .unwrap();
    assert_eq!(
        stored.settings["apiKey"], "prowlarr-secret",
        "the masked key is kept"
    );
    assert_eq!(
        http.delete(url(&format!("/indexer/{iid}")))
            .header("X-Api-Key", &key)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // Release push (Prowlarr, autobrr): decided with the RSS rules.
    let pushed = http
        .post(url("/release/push"))
        .header("X-Api-Key", &key)
        .json(&json!({"title": "Unknown.Show.S01E01.1080p.WEB-DL-GRP", "downloadUrl": "magnet:?xt=urn:btih:abc",
            "protocol": "torrent", "indexer": "Nyaa", "publishDate": "2026-11-01T00:00:00Z", "size": 1000}))
        .send()
        .await
        .unwrap();
    assert_eq!(pushed.status(), 200);
    let pushed: Value = pushed.json().await.unwrap();
    assert_eq!(pushed[0]["approved"], false);
    assert_eq!(pushed[0]["rejections"][0], "Unknown Series");
    assert_eq!(pushed[0]["protocol"], "torrent");
    assert!(pushed[0]["guid"].as_str().unwrap().starts_with("PUSH-"));
    let missing = http
        .post(url("/release/push"))
        .header("X-Api-Key", &key)
        .json(&json!({"title": "No.Url.S01E01"}))
        .send()
        .await
        .unwrap();
    assert!(missing.status().is_client_error(), "{}", missing.status());

    // A library series' release is mapped and decided (no download client here, so not grabbed).
    let library: Value = get("/series").await.unwrap().json().await.unwrap();
    if let Some(bb) = library
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Breaking Bad")
    {
        let pushed: Value = http
            .post(url("/release/push"))
            .header("X-Api-Key", &key)
            .json(&json!({"title": "Breaking.Bad.S01E01.1080p.WEB-DL-GRP", "downloadUrl": "magnet:?xt=urn:btih:def",
                "publishDate": "2026-11-01T00:00:00Z", "size": 1_500_000_000_i64}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(pushed[0]["seriesId"], bb["id"], "{pushed}");
    } else {
        panic!("the added series is gone: {library}");
    }

    // History, blocklist, download clients and connections.
    let history: Value = get("/history?page=1&pageSize=5&eventType=1")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history["page"], 1);
    assert!(history["records"].is_array());
    let since = get("/history/since?date=2026-01-01T00:00:00Z")
        .await
        .unwrap();
    assert_eq!(since.status(), 200);
    assert!(
        get("/history/series")
            .await
            .unwrap()
            .status()
            .is_client_error()
    );
    let blocklist: Value = get("/blocklist").await.unwrap().json().await.unwrap();
    assert_eq!(blocklist["totalRecords"], 0);
    let clients: Value = get("/downloadclient").await.unwrap().json().await.unwrap();
    assert!(clients.is_array());
    let connections: Value = get("/notification").await.unwrap().json().await.unwrap();
    assert!(connections.is_array());
    let missing: Value =
        get("/wanted/missing?page=1&pageSize=10&sortDirection=ascending&includeSeries=true")
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(missing["sortDirection"], "ascending");
    assert!(missing["records"].is_array(), "{missing}");
    if let Some(first) = missing["records"].as_array().unwrap().first() {
        assert_eq!(first["series"]["title"], "Breaking Bad");
    }
    let cutoff = get("/wanted/cutoff").await.unwrap();
    assert_eq!(cutoff.status(), 200);
}

/// Companion app keys reach only the Sonarr v3 library reads (and adds, with that permission).
#[tokio::test]
async fn companion_keys_are_scoped_to_v3_library_reads_and_adds() {
    use kitsunarr_domain::api::library_tools::CompanionKeyCreate;
    use serde_json::json;
    let server = start().await;
    let client = signed_in(&server).await;
    let created = client
        .create_companion_key(&CompanionKeyCreate {
            name: "Phone".into(),
            permission: "read-library".into(),
            days: 30,
        })
        .await
        .unwrap();
    assert!(created.token.starts_with("spr_"));
    let token = created.token.clone();
    let http = reqwest::Client::new();
    let call = |method: reqwest::Method, path: &str, token: &str| {
        http.request(method, format!("{}{path}", server.base))
            .header("X-Api-Key", token)
            .json(&json!({}))
            .send()
    };
    let code = |r: reqwest::Response| r.status().as_u16();
    assert_eq!(
        code(
            call(reqwest::Method::GET, "/api/v3/series", &token)
                .await
                .unwrap()
        ),
        200
    );
    assert_eq!(
        code(
            call(reqwest::Method::GET, "/api/v3/qualityprofile", &token)
                .await
                .unwrap()
        ),
        200
    );
    // Not a library read, not v3, or not allowed for reading keys.
    for (m, p) in [
        (reqwest::Method::GET, "/api/v3/system/status"),
        (reqwest::Method::GET, "/api/v3/series/lookup"),
        (reqwest::Method::GET, "/api/v1/series"),
        (reqwest::Method::POST, "/api/v3/series"),
        (reqwest::Method::POST, "/api/v3/command"),
        (reqwest::Method::DELETE, "/api/v3/series/1"),
        (reqwest::Method::GET, "/api/v3/series/..%2Fsystem"),
    ] {
        assert_eq!(
            code(call(m.clone(), p, &token).await.unwrap()),
            401,
            "{m} {p}"
        );
    }
    // An "add series" key may add, within the policy.
    let adder = client
        .create_companion_key(&CompanionKeyCreate {
            name: "Requests".into(),
            permission: "add-series".into(),
            days: 1,
        })
        .await
        .unwrap();
    let post = |body: serde_json::Value| {
        http.post(format!("{}/api/v3/series", server.base))
            .header("X-Api-Key", &adder.token)
            .json(&body)
            .send()
    };
    let r = post(json!({"tvdbId": 81189, "rootFolderPath": "/somewhere/else", "path": "/x"}))
        .await
        .unwrap();
    assert_eq!(code(r), 422);
    let r = post(json!({"tvdbId": 81189, "rootFolderPath": "/nowhere"}))
        .await
        .unwrap();
    assert_eq!(code(r), 422, "not a registered root folder");
    // Revoked keys stop working; keys are listed without their token.
    let keys = client.companion_keys().await.unwrap();
    assert_eq!(keys.len(), 2);
    client.delete_companion_key(&created.key.id).await.unwrap();
    assert_eq!(
        code(
            call(reqwest::Method::GET, "/api/v3/series", &token)
                .await
                .unwrap()
        ),
        401
    );
}

/// Tanukarr's import-list preview and selective add.
#[tokio::test]
async fn import_list_shows_can_be_added_selectively() {
    use kitsunarr_domain::api::import_lists::{ImportListAddRequest, ImportListExclusionItem};
    let (skyhook, _log) = skyhook_mock().await;
    let (list_url, list) = custom_list_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvdb: skyhook,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().display().to_string();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    *list.lock().unwrap() = serde_json::json!([
        {"title": "Breaking Bad", "tvdbId": 81189},
        {"title": "Excluded show", "tvdbId": 424242}
    ]);
    let mut item = client
        .provider_schema("importlists")
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.implementation == "CustomImport")
        .unwrap();
    item.name = "Picks".into();
    item.settings.insert("baseUrl".into(), list_url.into());
    item.options
        .insert("enableAutomaticAdd".into(), false.into());
    let created = client.create_provider("importlists", &item).await.unwrap();
    client
        .create_import_list_exclusion(&ImportListExclusionItem {
            id: 0,
            provider: "tvdb".into(),
            external_id: "424242".into(),
            title: None,
        })
        .await
        .unwrap();
    // Reading the list on its own adds nothing: automatic add is off.
    let read = client.sync_import_lists(Some(created.id)).await.unwrap();
    assert!(read.message.contains("0 titles added"), "{}", read.message);
    assert_eq!(
        client.import_list_entries(created.id).await.unwrap().len(),
        2
    );
    let request = |items: Vec<usize>| ImportListAddRequest {
        items,
        root_folder_path: root.clone(),
        quality_profile_id: 1,
        monitored: true,
    };
    let results = client
        .add_import_list_items(created.id, &request(vec![0, 1]))
        .await
        .unwrap();
    assert_eq!(results[0].outcome, "added", "{results:?}");
    assert_eq!(results[1].outcome, "excluded");
    let again = client
        .add_import_list_items(created.id, &request(vec![0]))
        .await
        .unwrap();
    assert_eq!(again[0].outcome, "exists");
    assert_eq!(
        status(
            client
                .add_import_list_items(created.id, &request(vec![7]))
                .await
                .unwrap_err()
        ),
        422
    );
    let entries = client.import_list_entries(created.id).await.unwrap();
    assert_eq!(entries[0].series_id, results[0].series_id);
}

#[tokio::test]
async fn delay_release_profiles_and_remote_path_mappings_are_managed() {
    use kitsunarr_domain::api::settings::{DelayProfile, ReleaseProfile, RemotePathMapping};
    let server = start().await;
    let client = signed_in(&server).await;
    // Sonarr's default delay profile exists from the start.
    let delays = client.delay_profiles().await.unwrap();
    assert_eq!(delays.len(), 1);
    assert_eq!(delays[0].sort_order, i64::from(i32::MAX));
    let created = client
        .save_delay_profile(&DelayProfile {
            id: 0,
            enable_usenet: true,
            enable_torrent: true,
            preferred_protocol: "torrent".into(),
            usenet_delay: 0,
            torrent_delay: 120,
            bypass_if_highest_quality: true,
            bypass_if_above_custom_format_score: false,
            minimum_custom_format_score: 0,
            sort_order: 0,
            tags: vec![],
        })
        .await
        .unwrap();
    assert!(created.id > 0 && created.sort_order < i64::from(i32::MAX));
    let mut edit = created.clone();
    edit.torrent_delay = 30;
    client.save_delay_profile(&edit).await.unwrap();
    let delays = client.delay_profiles().await.unwrap();
    assert_eq!(
        delays[0].torrent_delay, 30,
        "new profiles come before the default"
    );
    let mut bad = edit.clone();
    bad.preferred_protocol = "carrier pigeon".into();
    assert_eq!(
        status(client.save_delay_profile(&bad).await.unwrap_err()),
        400
    );
    client.delete_delay_profile(created.id).await.unwrap();

    let release = client
        .save_release_profile(&ReleaseProfile {
            id: 0,
            name: Some("No CAMs".into()),
            enabled: true,
            required: vec![],
            ignored: vec!["CAM".into(), "/\\bTS\\b/".into()],
            indexer_ids: vec![],
            tags: vec![],
        })
        .await
        .unwrap();
    assert_eq!(client.release_profiles().await.unwrap()[0].ignored.len(), 2);
    client.delete_release_profile(release.id).await.unwrap();

    let mapping = client
        .save_remote_path_mapping(&RemotePathMapping {
            id: 0,
            host: "qbittorrent".into(),
            remote_path: "/downloads/".into(),
            local_path: "/data/downloads/".into(),
        })
        .await
        .unwrap();
    let mut m = mapping.clone();
    m.local_path = "/mnt/downloads/".into();
    client.save_remote_path_mapping(&m).await.unwrap();
    assert_eq!(
        client.remote_path_mappings().await.unwrap()[0].local_path,
        "/mnt/downloads/"
    );
    client.delete_remote_path_mapping(mapping.id).await.unwrap();
    assert!(client.remote_path_mappings().await.unwrap().is_empty());
}

/// Tanukarr's manual grouping: create a group, add, relabel, reorder and remove members; a
/// title joining a group leaves its previous franchise or manual group.
#[tokio::test]
async fn collections_can_be_edited_by_hand() {
    use kitsunarr_domain::api::library::{CollectionEdit, CollectionMemberEdit};
    let server = start().await;
    let client = signed_in(&server).await;
    let pool = server.state.db.pool();
    let profile: i64 = sqlx::query_scalar("SELECT MIN(id) FROM quality_profiles")
        .fetch_one(pool)
        .await
        .unwrap();
    for (id, title) in [(1, "Part One"), (2, "Part Two")] {
        sqlx::query(
            "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, metadata_provider, metadata_id, \
             quality_profile_id, root_folder_path, path, added_at) VALUES (?, 'anime', ?, ?, ?, ?, 'aniList', ?, ?, '/a', ?, '2026-11-01T00:00:00Z')",
        )
        .bind(id)
        .bind(title)
        .bind(title.to_lowercase())
        .bind(title.to_lowercase())
        .bind(format!("slug-{id}"))
        .bind(id.to_string())
        .bind(profile)
        .bind(format!("/a/{id}"))
        .execute(pool)
        .await
        .unwrap();
    }
    let created = client
        .create_collection(&CollectionEdit {
            title: "My Franchise".into(),
            overview: None,
        })
        .await
        .unwrap();
    assert_eq!(created.kind, "manual");
    let cid = created.id;
    let add = |series: i64, role: &str| CollectionMemberEdit {
        series_id: Some(series),
        role: Some(role.into()),
        ..Default::default()
    };
    client
        .add_collection_member(cid, &add(1, "season"))
        .await
        .unwrap();
    let d = client
        .add_collection_member(cid, &add(2, "season"))
        .await
        .unwrap();
    let labels: Vec<_> = d
        .members
        .iter()
        .map(|m| m.label.clone().unwrap_or_default())
        .collect();
    assert_eq!(labels, vec!["Season 1", "Season 2"]);
    assert_eq!(d.members[0].title, "Part One");
    // Tanukarr's "Search collection": every member is searched.
    let started = client.search_collection(cid).await.unwrap();
    assert_eq!(started.searches, 2);
    // The same title twice: a conflict; an unknown role: invalid.
    assert!(
        client
            .add_collection_member(cid, &add(1, "season"))
            .await
            .is_err()
    );
    let other = client
        .create_collection(&CollectionEdit {
            title: "Elsewhere".into(),
            overview: None,
        })
        .await
        .unwrap();
    assert!(
        client
            .add_collection_member(other.id, &add(1, "chapter"))
            .await
            .is_err()
    );
    // Moving Part One to another group takes it out of the first.
    client
        .add_collection_member(other.id, &add(1, "season"))
        .await
        .unwrap();
    let d = client.collection(cid).await.unwrap();
    assert_eq!(d.members.len(), 1);
    // Relabel and reorder, rename, remove, delete.
    let m = d.members[0].id;
    let d = client
        .update_collection_member(
            cid,
            m,
            &CollectionMemberEdit {
                label: Some("Final Season".into()),
                position: Some(9),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (d.members[0].label.as_deref(), d.members[0].position),
        (Some("Final Season"), 9)
    );
    let d = client
        .update_collection(
            cid,
            &CollectionEdit {
                title: "Renamed".into(),
                overview: Some("Story".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (d.title.as_str(), d.overview.as_deref()),
        ("Renamed", Some("Story"))
    );
    let d = client.remove_collection_member(cid, m).await.unwrap();
    assert!(d.members.is_empty());
    client.delete_collection(cid).await.unwrap();
    assert!(client.collection(cid).await.is_err());
    let series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(series, 2, "titles stay in the library");
}

/// Tanukarr's custom quality presets through the API: create, list, retire, restore.
#[tokio::test]
async fn custom_quality_presets_round_trip() {
    use kitsunarr_domain::api::settings::CustomQualityCreate;
    use kitsunarr_domain::quality::QualitySource;
    let server = start().await;
    let client = signed_in(&server).await;
    let d = client
        .create_custom_quality(&CustomQualityCreate {
            title: "Anime BD 1080p HEVC".into(),
            source: Some(QualitySource::Bluray),
            resolution: Some(1080),
            codec: "HEVC".into(),
            title_pattern: String::new(),
            copy_from_id: Some(7),
        })
        .await
        .unwrap();
    assert!(d.quality_id >= 10_000);
    let defs = client.quality_definitions().await.unwrap();
    let stored = defs.iter().find(|x| x.quality_id == d.quality_id).unwrap();
    assert_eq!(stored.codec, "HEVC");
    let profiles = client.quality_profiles().await.unwrap();
    assert!(
        profiles
            .iter()
            .all(|p| p.items.iter().any(|i| i.quality_id == Some(d.quality_id)))
    );
    client.retire_quality(d.quality_id).await.unwrap();
    assert!(
        client
            .quality_definitions()
            .await
            .unwrap()
            .iter()
            .any(|x| x.quality_id == d.quality_id && x.is_retired)
    );
    client.restore_quality(d.quality_id).await.unwrap();
    assert!(
        client
            .quality_definitions()
            .await
            .unwrap()
            .iter()
            .any(|x| x.quality_id == d.quality_id && !x.is_retired)
    );
    // Criteria on a standard quality are kept by the bulk update.
    let mut defs = client.quality_definitions().await.unwrap();
    defs.iter_mut()
        .find(|x| x.quality_id == 3)
        .unwrap()
        .title_pattern = r"\bAMZN\b".into();
    let saved = client.update_quality_definitions(&defs).await.unwrap();
    assert_eq!(
        saved
            .iter()
            .find(|x| x.quality_id == 3)
            .unwrap()
            .title_pattern,
        r"\bAMZN\b"
    );

    // Global codec factors: one profile overriding HEVC makes it mixed; a global value is
    // written into every profile; null resets.
    use kitsunarr_domain::api::settings::OrderedF64;
    let factors = client.codec_factors().await.unwrap();
    let hevc = factors.iter().find(|f| f.codec == "HEVC").unwrap();
    assert_eq!(
        (hevc.factor, hevc.is_mixed),
        (Some(OrderedF64(1.67)), false)
    );
    let mut p = client.quality_profiles().await.unwrap().remove(0);
    p.codec_multipliers.insert("HEVC".into(), OrderedF64(1.2));
    client.update_quality_profile(&p).await.unwrap();
    let hevc = client
        .codec_factors()
        .await
        .unwrap()
        .into_iter()
        .find(|f| f.codec == "HEVC")
        .unwrap();
    assert!(hevc.is_mixed && hevc.factor.is_none());
    let set = |v: Option<f64>| {
        std::collections::BTreeMap::from([("HEVC".to_string(), v.map(OrderedF64))])
    };
    let after = client.update_codec_factors(&set(Some(1.5))).await.unwrap();
    let hevc = after.iter().find(|f| f.codec == "HEVC").unwrap();
    assert_eq!((hevc.factor, hevc.is_mixed), (Some(OrderedF64(1.5)), false));
    assert!(client.update_codec_factors(&set(Some(42.0))).await.is_err());
    client.update_codec_factors(&set(None)).await.unwrap();
    assert!(
        client
            .quality_profiles()
            .await
            .unwrap()
            .iter()
            .all(|p| !p.codec_multipliers.contains_key("HEVC"))
    );
}

/// Tanukarr's movie search: "Title Year" on the movie categories, releases decided as the
/// film's feature, the best one grabbed into the film's history.
#[tokio::test]
async fn movies_are_searched_and_grabbed() {
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    type Seen = Arc<std::sync::Mutex<Vec<String>>>;
    async fn torznab(
        State(seen): State<Seen>,
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
        seen.lock().unwrap().push(format!(
            "t={} q={} cat={}",
            q.get("t").cloned().unwrap_or_default(),
            q.get("q").cloned().unwrap_or_default(),
            q.get("cat").cloned().unwrap_or_default()
        ));
        let item = |title: &str, hash: &str| {
            format!(
                r#"<item><title>{title}</title><guid>{hash}</guid>
<link>magnet:?xt=urn:btih:{hash}&amp;dn=x&amp;tr=udp://t</link><pubDate>Mon, 03 Nov 2025 10:00:00 +0000</pubDate>
<size>8000000000</size><enclosure url="magnet:?xt=urn:btih:{hash}&amp;dn=x&amp;tr=udp://t" length="8000000000" type="application/x-bittorrent;x-scheme-handler/magnet" />
<torznab:attr name="seeders" value="50" /></item>"#
            )
        };
        let feed = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel><title>Mock</title>{}{}{}</channel></rss>"#,
            item(
                "The.Matrix.1999.1080p.BluRay.x264-GRP",
                "a12fe1c06bba254a9dc9f519b335aa7c1367a88a"
            ),
            item(
                "The.Matrix.Reloaded.2003.1080p.BluRay.x264-GRP",
                "b12fe1c06bba254a9dc9f519b335aa7c1367a88b"
            ),
            item(
                "The.Matrix.1999.720p.HDTV.x264-GRP",
                "c12fe1c06bba254a9dc9f519b335aa7c1367a88c"
            ),
        );
        ([("content-type", "application/rss+xml")], feed).into_response()
    }
    async fn qbit(
        State(seen): State<Seen>,
        uri: axum::http::Uri,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let path = uri.path().to_string();
        seen.lock()
            .unwrap()
            .push(format!("{path} {}", String::from_utf8_lossy(&body)));
        match path.as_str() {
            "/api/v2/auth/login" => "Ok.".into_response(),
            "/api/v2/app/webapiVersion" => "2.11.4".into_response(),
            "/api/v2/app/version" => "v5.0.0".into_response(),
            "/api/v2/app/preferences" => {
                axum::Json(serde_json::json!({"dht": true})).into_response()
            }
            "/api/v2/torrents/info" => axum::Json(serde_json::json!([])).into_response(),
            "/api/v2/torrents/categories" => {
                axum::Json(serde_json::json!({"tv-kitsunarr": {}})).into_response()
            }
            "/api/v2/torrents/add" => "Ok.".into_response(),
            "/api/v2/torrents/properties" => axum::http::StatusCode::NOT_FOUND.into_response(),
            _ => ().into_response(),
        }
    }
    let searches: Seen = Arc::default();
    let app = axum::Router::new()
        .route("/api", axum::routing::get(torznab))
        .with_state(searches.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let indexer_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let adds: Seen = Arc::default();
    let app = axum::Router::new()
        .route("/api/v2/{*rest}", axum::routing::get(qbit).post(qbit))
        .with_state(adds.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let qbit_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let server = start().await;
    let client = signed_in(&server).await;
    client
        .post::<serde_json::Value, _>(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Movies", "implementation": "Torznab",
                "options": {"enableInteractiveSearch": true, "enableAutomaticSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "categories": [5040], "movieCategories": [2000, 2040]}
            }),
        )
        .await
        .unwrap();
    client
        .post::<serde_json::Value, _>(
            "/providers/downloadclients",
            &serde_json::json!({
                "name": "qBittorrent", "implementation": "QBittorrent", "enabled": true,
                "settings": {"host": "127.0.0.1", "port": qbit_port, "tvCategory": "tv-kitsunarr"}
            }),
        )
        .await
        .unwrap();
    let pool = server.state.db.pool();
    let profile: i64 = sqlx::query_scalar("SELECT id FROM quality_profiles WHERE name = 'Any'")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO movies (id, title, sort_title, clean_title, title_slug, year, runtime, metadata_provider, metadata_id, \
         quality_profile_id, root_folder_path, path, added_at) VALUES (7, 'The Matrix', 'matrix', 'thematrix', 'the-matrix', 1999, 136, \
         'tmdbMovie', '603', ?, '/movies', '/movies/The Matrix (1999)', '2026-11-01T00:00:00Z')",
    )
    .bind(profile)
    .execute(pool)
    .await
    .unwrap();

    // Wanted: a monitored film without a file, released (1999, no dates known).
    let missing = client.missing_movies().await.unwrap();
    assert_eq!(missing.iter().map(|m| m.id).collect::<Vec<_>>(), vec![7]);
    let found = client.search_movie_releases(7).await.unwrap();
    assert_eq!(found.description, "The Matrix (1999)");
    assert_eq!(found.searched_indexers, 1);
    let seen = searches.lock().unwrap().clone();
    assert!(
        seen.iter()
            .any(|s| s == "t=search q=The Matrix 1999 cat=2000,2040"),
        "{seen:?}"
    );
    let by_title = |t: &str| found.releases.iter().find(|r| r.title == t).unwrap();
    assert!(by_title("The.Matrix.1999.1080p.BluRay.x264-GRP").approved);
    let wrong = by_title("The.Matrix.Reloaded.2003.1080p.BluRay.x264-GRP");
    assert!(!wrong.approved && wrong.rejections.iter().any(|r| r == "Wrong movie"));
    // Best first: the Blu-ray before the HDTV release.
    assert_eq!(
        found.releases[0].title,
        "The.Matrix.1999.1080p.BluRay.x264-GRP"
    );

    // A webhook hears the film's grab (as a one-episode series, Tanukarr's shape).
    let heard: Seen = Arc::default();
    let h = heard.clone();
    let hook = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |body: String| {
            let h = h.clone();
            async move {
                h.lock().unwrap().push(body);
                "ok"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, hook).await.unwrap() });
    sqlx::query("INSERT INTO notifications (name, implementation, settings, triggers, created_at) VALUES ('Hook', 'Webhook', ?, '{\"onGrab\": true}', '2026-11-01T00:00:00Z')")
        .bind(serde_json::json!({"url": hook_url, "method": 1}).to_string())
        .execute(pool)
        .await
        .unwrap();
    // A long torrent delay holds the film's releases back: pending, shown in the queue.
    sqlx::query("DELETE FROM delay_profiles")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO delay_profiles (torrent_delay, preferred_protocol, bypass_if_highest_quality, sort_order) \
         VALUES (10000000, 'usenet', 0, 2147483647)",
    )
    .execute(pool)
    .await
    .unwrap();
    let held = kitsunarr_api::rss::sync(&server.state).await.unwrap();
    assert!(held.contains("0 grabbed"), "{held}");
    let queue = client.queue().await.unwrap();
    let pending: Vec<_> = queue
        .items
        .iter()
        .filter(|i| i.tracked_state == "pending")
        .collect();
    assert!(!pending.is_empty(), "{:?}", queue.items);
    assert!(
        pending
            .iter()
            .all(|p| p.movie_id == Some(7) && p.movie_title.as_deref() == Some("The Matrix")),
        "{pending:?}"
    );
    sqlx::query("UPDATE delay_profiles SET torrent_delay = 0")
        .execute(pool)
        .await
        .unwrap();
    // RSS sync: a release no series claims is matched with the film and grabbed; the next
    // sync leaves the film alone while that grab downloads.
    let summary = kitsunarr_api::rss::sync(&server.state).await.unwrap();
    assert!(
        summary.contains("1 grabbed: The.Matrix.1999.1080p.BluRay.x264-GRP"),
        "{summary}"
    );
    assert!(
        kitsunarr_db::pending::list(server.state.db.pool())
            .await
            .unwrap()
            .is_empty(),
        "a grab for the film ends its pending releases"
    );
    let again = kitsunarr_api::rss::sync(&server.state).await.unwrap();
    assert!(again.contains("0 grabbed"), "{again}");
    for _ in 0..50 {
        if !heard.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let grab: serde_json::Value = serde_json::from_str(&heard.lock().unwrap()[0]).unwrap();
    assert_eq!(grab["eventType"], "Grab");
    assert_eq!(grab["series"]["title"], "The Matrix");
    assert_eq!(grab["episodes"][0]["episodeNumber"], 1);

    let result = client.automatic_movie_search(7).await.unwrap();
    assert_eq!(result.grabbed.len(), 1, "{result:?}");
    assert_eq!(
        result.grabbed[0].title,
        "The.Matrix.1999.1080p.BluRay.x264-GRP"
    );
    let adds = adds.lock().unwrap().clone();
    assert!(
        adds.iter().any(|a| a.starts_with("/api/v2/torrents/add ")
            && a.contains("a12fe1c06bba254a9dc9f519b335aa7c1367a88a")),
        "{adds:?}"
    );
    let history: (Option<i64>, Option<i64>, String) = sqlx::query_as(
        "SELECT movie_id, series_id, event_type FROM history WHERE source_title = 'The.Matrix.1999.1080p.BluRay.x264-GRP'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(history, (Some(7), None, "grabbed".into()));
}
