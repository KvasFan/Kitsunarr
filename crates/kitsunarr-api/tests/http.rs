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
    // Release sizes in these tests are made up: the free-space checks would depend on the
    // machine's disk (they are tested in the decision engine and the importer).
    let mut media: kitsunarr_domain::api::settings::MediaManagementSettings =
        kitsunarr_db::settings::get_or_default(
            db.pool(),
            kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
        )
        .await
        .unwrap();
    media.skip_free_space_check = true;
    kitsunarr_db::settings::set(
        db.pool(),
        kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
        &media,
    )
    .await
    .unwrap();
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

    // MyAnimeList's sign-in callback is public; the code waits for the editor's poll.
    let page = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/importlists/oauth/callback?code=authcode&state=nonce1",
            server.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    assert_eq!(
        server
            .state
            .oauth_codes
            .lock()
            .unwrap()
            .get("nonce1")
            .map(String::as_str),
        Some("authcode")
    );
    let no_sign_in: Result<serde_json::Value, _> = client
        .post(
            "/providers/importlists/signin/start",
            &serde_json::json!({"implementation": "AniListImport", "settings": {}}),
        )
        .await;
    assert_eq!(status(no_sign_in.unwrap_err()), 422);
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
        path: None,
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
            path: None,
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
            indexer_ids: None,
            search_session: None,
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
            indexer_ids: None,
            search_session: None,
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
            indexer_ids: None,
            search_session: None,
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
                    indexer_ids: None,
                    search_session: None,
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
            indexer_ids: None,
            search_session: None,
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
async fn torrent_blackhole_writes_grabs_and_imports_the_watch_folder() {
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
    let (indexer_url, _qbit_port, _qbit) = torrent_mocks().await;
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
    let folders = tempfile::tempdir().unwrap();
    let (torrents, watch) = (
        folders.path().join("torrents"),
        folders.path().join("watch"),
    );
    std::fs::create_dir_all(&torrents).unwrap();
    std::fs::create_dir_all(&watch).unwrap();
    let blackhole: serde_json::Value = client
        .post(
            "/providers/downloadclients",
            &serde_json::json!({
                "name": "Blackhole", "implementation": "TorrentBlackhole", "enabled": true,
                "settings": {"torrentFolder": torrents.display().to_string(),
                    "watchFolder": watch.display().to_string(), "saveMagnetFiles": true,
                    "magnetFileExtension": ".magnet", "readOnly": true, "scanGracePeriodSeconds": 0}
            }),
        )
        .await
        .unwrap();
    assert_eq!(blackhole["supported"], true);
    let test = client
        .test_download_client(&kitsunarr_domain::api::search::IndexerTestRequest {
            id: blackhole["id"].as_i64(),
            implementation: "TorrentBlackhole".into(),
            settings: serde_json::from_value(blackhole["settings"].clone()).unwrap(),
        })
        .await
        .unwrap();
    assert!(test.ok, "{}", test.message);

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
            path: None,
        })
        .await
        .unwrap();
    let episodes = client.episodes(added.id).await.unwrap();
    let pilot = episodes
        .iter()
        .find(|e| e.season_number == 1 && e.episode_number == 1)
        .unwrap();
    let found = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(pilot.id),
            complete_series: false,
            season_packs_only: false,
            indexer_ids: None,
            search_session: None,
        })
        .await
        .unwrap();
    // A release with both a torrent file and (from RSS) a magnet: the file is written.
    let release = found
        .releases
        .iter()
        .find(|r| r.title == "Breaking.Bad.S01E01.720p.BluRay.x264-GRP")
        .unwrap();
    let grabbed = client
        .grab_release(&GrabRequest {
            guid: release.guid.clone(),
            indexer_id,
            episode_ids: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(grabbed.download_client, "Blackhole");
    assert!(grabbed.download_id.is_empty(), "blackholes report no id");
    let torrent = torrents.join("Breaking.Bad.S01E01.720p.BluRay.x264-GRP.torrent");
    assert!(std::fs::read(&torrent).unwrap().starts_with(b"d8:announce"));

    // The other program finishes the download in the watch folder; Kitsunarr imports it by
    // its title, copying (read-only) rather than moving.
    let done = watch.join("Breaking.Bad.S01E01.720p.BluRay.x264-GRP");
    std::fs::create_dir_all(&done).unwrap();
    std::fs::write(
        done.join("Breaking.Bad.S01E01.720p.BluRay.x264-GRP.mkv"),
        vec![0u8; 4096],
    )
    .unwrap();
    let report = client.refresh_queue().await.unwrap();
    assert!(report.starts_with("1 download"), "{report}");
    let file = client
        .episodes(added.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.id == pilot.id)
        .unwrap()
        .file
        .expect("imported");
    assert_eq!(
        file.relative_path,
        "Season 1/Breaking.Bad.S01E01.720p.BluRay.x264-GRP.mkv"
    );
    // "Remove completed downloads" (on by default) deletes a blackhole's download once it is
    // imported, as upstream; the queue no longer lists it.
    assert!(!done.exists());
    let queue = client.queue().await.unwrap();
    assert!(!queue.items.iter().any(|i| i.title.contains("S01E01")));
    let imports = client
        .history(1, 50, Some("downloadFolderImported"))
        .await
        .unwrap();
    assert_eq!(imports.records.len(), 1);
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
            path: None,
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
            indexer_ids: None,
            search_session: None,
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
            indexer_ids: None,
            search_session: None,
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
        indexer_ids: None,
        search_session: None,
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
    // Sonarr's queue status counts it, and the series' queue details list it.
    let v3 = |p: &str| format!("{}/api/v3{p}", server.base);
    let http = reqwest::Client::new();
    let get = |p: String| {
        let r = http.get(p).header("X-Api-Key", &server.api_key).send();
        async move { r.await.unwrap().json::<serde_json::Value>().await.unwrap() }
    };
    let status = get(v3("/queue/status")).await;
    assert_eq!(status["totalCount"], queue.items.len(), "{status}");
    assert_eq!(status["unknownCount"], 0, "{status}");
    let details = get(v3(&format!("/queue/details?seriesId={}", added.id))).await;
    let record = details
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["downloadId"] == pending.download_id.as_str())
        .unwrap_or_else(|| panic!("{details}"))
        .clone();
    assert_eq!(record["status"], "delay");
    assert_eq!(
        get(v3("/queue/details?seriesId=999999")).await,
        serde_json::json!([])
    );
    // Only a pending release can be sent now.
    let other = queue
        .items
        .iter()
        .find(|i| i.tracked_state != "pending")
        .unwrap();
    assert!(client.grab_pending(&other.download_id).await.is_err());
    // "Send now" (Sonarr's `POST /queue/grab/{id}`): decided again as the user's request, so
    // the delay no longer holds it back, and grabbed.
    qbit.lock().unwrap().clear();
    let sent: serde_json::Value = http
        .post(v3(&format!("/queue/grab/{}", record["id"])))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        sent["message"],
        "Breaking.Bad.S01E03.720p.BluRay.x264-GRP was sent to the download client."
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
    assert!(
        client.grab_pending(&pending.download_id).await.is_err(),
        "it is pending no more"
    );
    set_delay(0).await;
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
            path: None,
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
            indexer_ids: None,
            search_session: None,
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
    // Tanukarr's `SourceReleaseTitle`/`SourceReleaseSize`: the grabbed release stays on the
    // file, and custom formats of the existing file read its size, not the file's 5 bytes.
    let (size, title): (Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT source_release_size, source_release_title FROM episode_files WHERE id = ?",
    )
    .bind(file.id)
    .fetch_one(server.state.db.pool())
    .await
    .unwrap();
    assert_eq!(size, Some(release.size));
    assert_eq!(
        title.as_deref(),
        Some("Breaking.Bad.S01E01.720p.HDTV.x264-GRP")
    );
    let episodes = kitsunarr_db::library::series::list_episodes(&server.state.db, added.id, None)
        .await
        .unwrap();
    let model =
        kitsunarr_library::decision_input::series_model(&server.state.db, &detail, &episodes)
            .await
            .unwrap();
    let decided = model
        .episodes
        .iter()
        .find(|e| e.id == pilot.id)
        .and_then(|e| e.file.as_ref())
        .unwrap();
    assert_eq!(decided.size, release.size);
    assert_eq!(
        decided.release_title,
        "Breaking.Bad.S01E01.720p.HDTV.x264-GRP"
    );
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
            path: None,
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
        indexer_ids: None,
        search_session: None,
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
            path: None,
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
    // "On Import Complete" only: one delivery once the download is fully imported.
    let (complete_url, complete) = webhook_mock().await;
    let _: serde_json::Value = client
        .post(
            "/providers/notifications",
            &serde_json::json!({
                "name": "Complete", "implementation": "Webhook",
                "options": {"onImportComplete": true},
                "settings": {"url": complete_url}
            }),
        )
        .await
        .unwrap();
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
            path: None,
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
            indexer_ids: None,
            search_session: None,
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
    drop(seen);
    wait_for(&complete, 1).await;
    let seen = complete.lock().unwrap();
    assert_eq!(seen.len(), 1, "{seen:#?}");
    assert_eq!(seen[0]["eventType"], "Download");
    assert_eq!(seen[0]["fileCount"], 1);
    assert_eq!(seen[0]["episodes"][0]["title"], "Pilot");
    assert_eq!(seen[0]["episodeFiles"][0]["quality"], "HDTV-720p");
    assert_eq!(seen[0]["downloadId"], "SABnzbd_nzo_x1");
    assert_eq!(
        seen[0]["release"]["releaseTitle"],
        "Breaking.Bad.S01E01.720p.HDTV.x264-GRP"
    );
    assert!(seen[0].get("isUpgrade").is_none());
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
            path: None,
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
            path: None,
        })
        .await
        .unwrap();
    let season_1 = ReleaseQuery {
        series_id: added.id,
        season_number: Some(1),
        episode_id: None,
        complete_series: false,
        season_packs_only: false,
        indexer_ids: None,
        search_session: None,
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
            path: None,
        })
        .await
        .unwrap();
    let query = ReleaseQuery {
        series_id: added.id,
        season_number: None,
        episode_id: None,
        complete_series: true,
        season_packs_only: false,
        indexer_ids: None,
        search_session: None,
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
            indexer_ids: None,
            search_session: None,
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
            indexer_ids: None,
            search_session: None,
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
            path: None,
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
            indexer_ids: None,
            search_session: None,
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

/// Tanukarr's `EpisodeRefreshedService`: episodes a refresh adds that aired in the last 14 days
/// are searched (against the indexer); older ones are not, nor anything while automation is
/// paused.
#[tokio::test]
async fn refreshes_search_episodes_that_recently_aired() {
    use axum::extract::{Path, State};
    use axum::routing::get;
    use kitsunarr_domain::api::settings::LibraryToolsSettings;
    type Extra = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
    fn show_with(extra: &Extra) -> serde_json::Value {
        let path = format!(
            "{}/../../tests/fixtures/metadata/tvmaze-show-169.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let mut show: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        show["_embedded"]["episodes"]
            .as_array_mut()
            .unwrap()
            .extend(extra.lock().unwrap().iter().cloned());
        show
    }
    async fn show(
        State(extra): State<Extra>,
        Path(_id): Path<String>,
    ) -> axum::Json<serde_json::Value> {
        axum::Json(show_with(&extra))
    }
    async fn episodes(
        State(extra): State<Extra>,
        Path(_id): Path<String>,
    ) -> axum::Json<serde_json::Value> {
        axum::Json(show_with(&extra)["_embedded"]["episodes"].clone())
    }
    let extra: Extra = Arc::default();
    let app = axum::Router::new()
        .route("/shows/{id}", get(show))
        .route("/shows/{id}/episodes", get(episodes))
        .with_state(extra.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: format!("http://{addr}"),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let (indexer_url, requests) = newznab_mock().await;
    let _: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Newznab",
                "implementation": "Newznab",
                "options": {"enableRss": true, "enableAutomaticSearch": true, "enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "k", "categories": [5030, 5040]}
            }),
        )
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().display().to_string();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    let added = client
        .add_title(&AddTitleRequest {
            provider: MetadataProvider::TvMaze,
            id: "169".into(),
            root_folder_path: root,
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
            path: None,
        })
        .await
        .unwrap();
    let searched = |episode: i64| {
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("t=tvsearch") && r.contains(&format!("&ep={episode}&")))
    };
    let episode = |number: i64, days: i64| {
        let at = chrono::Utc::now() + chrono::Duration::days(days);
        serde_json::json!({"id": 900_000 + number, "name": format!("New {number}"), "season": 5,
            "number": number, "type": "regular", "airdate": at.format("%Y-%m-%d").to_string(),
            "airstamp": at.to_rfc3339(), "runtime": 60})
    };
    extra
        .lock()
        .unwrap()
        .extend([episode(17, -1), episode(18, -30)]);
    let refreshed = client.refresh_series(added.id).await.unwrap();
    assert_eq!(refreshed.added_episodes, 2);
    let mut found = false;
    for _ in 0..100 {
        if searched(17) {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(found, "{:?}", requests.lock().unwrap());
    assert!(!searched(18), "aired a month ago");

    // Automation paused: a new recent episode waits for the user.
    client
        .put_library_tools(&LibraryToolsSettings {
            automation_paused: true,
            ..Default::default()
        })
        .await
        .unwrap();
    extra.lock().unwrap().push(episode(19, 0));
    assert_eq!(
        client
            .refresh_series(added.id)
            .await
            .unwrap()
            .added_episodes,
        1
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!searched(19));
}

/// Tanukarr's completed-fetch resume: a list whose complete read this process witnessed feeds
/// its saved items again while it is not due, so an addition that failed (the catalogue
/// answered 500) is retried on the next run without reading the list again.
#[tokio::test]
async fn witnessed_list_reads_retry_failed_additions_while_not_due() {
    use axum::Json;
    use axum::routing::post;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Clone, Default)]
    struct Mock {
        list_reads: Arc<AtomicUsize>,
        catalogue_failures: Arc<AtomicUsize>,
    }
    async fn graphql(
        axum::extract::State(m): axum::extract::State<Mock>,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let q = body["query"].as_str().unwrap_or_default();
        if q.contains("mediaList") {
            m.list_reads.fetch_add(1, Ordering::SeqCst);
            let entry = json!({"status": "CURRENT", "progress": 0, "customListEntries": [],
                "media": {"id": 5, "idMal": 105, "format": "TV", "status": "RELEASING",
                    "title": {"userPreferred": "Listed show", "romaji": "x"}, "startDate": {"year": 2024}}});
            return Json(json!({"data": {"Page": {"pageInfo": {"currentPage": 1, "hasNextPage": false}, "mediaList": [entry]}}})).into_response();
        }
        if m.catalogue_failures.load(Ordering::SeqCst) > 0 {
            m.catalogue_failures.fetch_sub(1, Ordering::SeqCst);
            return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        if q.contains("airingSchedule") {
            return Json(json!({"data": {"Media": {"airingSchedule": {"pageInfo": {"hasNextPage": false}, "nodes": []}}}})).into_response();
        }
        if q.contains("relations") {
            return Json(json!({"data": {"Media": {"id": 5, "format": "TV", "title": {"english": "Listed show"}, "relations": {"edges": []}}}})).into_response();
        }
        Json(json!({"data": {"Media": {"id": 5, "idMal": 105, "format": "TV", "status": "RELEASING", "episodes": 2,
            "duration": 24, "seasonYear": 2024, "countryOfOrigin": "JP",
            "title": {"romaji": "Listed show", "english": "Listed show", "native": "x"}, "synonyms": [],
            "startDate": {"year": 2024, "month": 1, "day": 1}, "endDate": {}, "coverImage": {}, "genres": [],
            "averageScore": 80, "popularity": 1, "description": "x", "studios": {"nodes": []},
            "nextAiringEpisode": null}}})).into_response()
    }
    let mock = Mock::default();
    // The catalogue fails the whole first add attempt (fetch, with its retry-free errors).
    mock.catalogue_failures.store(1000, Ordering::SeqCst);
    let app = axum::Router::new()
        .route("/graphql", post(graphql))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{addr}/graphql");
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            anilist: base.clone(),
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
    let _: Value = client
        .post(
            "/providers/importlists",
            &json!({
                "name": "My AniList",
                "implementation": "AniListImport",
                "enabled": true,
                "options": {"enableAutomaticAdd": true, "rootFolderPath": root, "qualityProfileId": 1,
                    "searchForMissingEpisodes": false},
                "settings": {"username": "someone", "baseUrl": base}
            }),
        )
        .await
        .unwrap();
    let sync = || kitsunarr_api::import_lists::sync(&server.state, None, false);
    let first = sync().await.unwrap();
    assert!(first.contains("0 titles added"), "{first}");
    assert_eq!(mock.list_reads.load(Ordering::SeqCst), 1);
    // Not due (6 hours), but witnessed: the saved item is retried without a list read.
    mock.catalogue_failures.store(0, Ordering::SeqCst);
    let second = sync().await.unwrap();
    assert!(second.contains("1 titles added"), "{second}");
    assert_eq!(mock.list_reads.load(Ordering::SeqCst), 1);
    let series = client.series(&LibraryQuery::default()).await.unwrap();
    assert_eq!(series.records.len(), 1);
    // Nothing left to add: the next run adds nothing and still reads no list.
    let third = sync().await.unwrap();
    assert!(third.contains("0 titles added"), "{third}");
    assert_eq!(mock.list_reads.load(Ordering::SeqCst), 1);
    // Changed settings void the witness: nothing is fed until the list is due again.
    let mut list = client.providers("importlists").await.unwrap().remove(0);
    list.name = "Renamed".into();
    client.update_provider("importlists", &list).await.unwrap();
    assert_eq!(sync().await.unwrap(), "No list was due for a sync");
}

/// Tanukarr's `SearchSeriesComparer` on the add-series search: the exact title first, though
/// AniList's own order put it last.
#[tokio::test]
async fn catalogue_search_ranks_the_closest_title_first() {
    use axum::Json;
    use axum::routing::post;
    use serde_json::{Value, json};
    async fn graphql(Json(_body): Json<Value>) -> Json<Value> {
        let media = |id: i64, title: &str, popularity: i64, synonyms: Value| {
            json!({"id": id, "idMal": null, "format": "TV", "status": "FINISHED", "episodes": 12, "duration": 24,
                "seasonYear": 2020, "countryOfOrigin": "JP", "title": {"romaji": title, "english": title, "native": null},
                "synonyms": synonyms, "startDate": {"year": 2020}, "endDate": {}, "coverImage": {}, "genres": [],
                "averageScore": 80, "popularity": popularity, "description": null, "studios": {"nodes": []},
                "nextAiringEpisode": null})
        };
        Json(json!({"data": {"Page": {"media": [
            media(1, "Attack on Titan", 900_000, json!(["AoT"])),
            media(2, "Attack on Titan: Junior High", 50_000, json!([])),
            media(3, "Attack on Titan Season 2", 600_000, json!(["Shingeki no Kyojin 2"])),
        ]}}}))
    }
    let app = axum::Router::new().route("/graphql", post(graphql));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            anilist: format!("http://{addr}/graphql"),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let titles = |found: Vec<kitsunarr_domain::api::metadata::SearchItem>| {
        found.into_iter().map(|i| i.title).collect::<Vec<_>>()
    };
    let found = client
        .metadata_search(&SearchQuery {
            provider: MetadataProvider::AniList,
            term: "Attack on Titan 2nd Season".into(),
        })
        .await
        .unwrap();
    assert_eq!(titles(found)[0], "Attack on Titan Season 2");
    let found = client
        .metadata_search(&SearchQuery {
            provider: MetadataProvider::AniList,
            term: "Attack on Titan".into(),
        })
        .await
        .unwrap();
    // Sonarr's "CSI: ..." rule ranks a `Title: subtitle` above a merely similar title.
    assert_eq!(
        titles(found),
        [
            "Attack on Titan",
            "Attack on Titan: Junior High",
            "Attack on Titan Season 2"
        ]
    );
}

/// Tanukarr's "Disabled" grouping mode over the native API: chosen when adding, shown on the
/// series, and switched back.
#[tokio::test]
async fn grouping_can_be_disabled_when_adding_and_on_the_series() {
    use axum::Json;
    use axum::routing::post;
    use serde_json::{Value, json};
    async fn graphql(Json(body): Json<Value>) -> Json<Value> {
        let q = body["query"].as_str().unwrap_or_default();
        let id = body["variables"]["id"].as_i64().unwrap_or(0);
        if q.contains("airingSchedule") {
            return Json(
                json!({"data": {"Media": {"airingSchedule": {"pageInfo": {"hasNextPage": false}, "nodes": []}}}}),
            );
        }
        if q.contains("relations") {
            let edge = |kind: &str, n: i64| json!({"relationType": kind, "node": {"id": n, "type": "ANIME", "format": "TV", "title": {"english": format!("Part {n}")}}});
            let edges = match id {
                1 => vec![edge("SEQUEL", 2)],
                2 => vec![edge("PREQUEL", 1)],
                _ => vec![],
            };
            return Json(
                json!({"data": {"Media": {"id": id, "format": "TV", "title": {"english": format!("Part {id}")}, "relations": {"edges": edges}}}}),
            );
        }
        Json(
            json!({"data": {"Media": {"id": id, "idMal": null, "format": "TV", "status": "FINISHED", "episodes": 1,
            "duration": 24, "seasonYear": 2020, "countryOfOrigin": "JP",
            "title": {"romaji": format!("Part {id}"), "english": format!("Part {id}"), "native": null}, "synonyms": [],
            "startDate": {"year": 2020, "month": 1, "day": 1}, "endDate": {}, "coverImage": {}, "genres": [],
            "averageScore": 80, "popularity": 1, "description": null, "studios": {"nodes": []},
            "nextAiringEpisode": null}}}),
        )
    }
    let app = axum::Router::new().route("/graphql", post(graphql));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            anilist: format!("http://{addr}/graphql"),
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
    let added = client
        .add_title_with(&kitsunarr_domain::api::metadata::AddTitle {
            title: AddTitleRequest {
                provider: MetadataProvider::AniList,
                id: "2".into(),
                root_folder_path: root.clone(),
                quality_profile_id: 1,
                monitor: MonitorOption::All,
                series_type: None,
                season_folder: true,
                tags: vec![],
                search_for_missing: false,
                episode_ordering: None,
                path: None,
            },
            automatic_grouping: false,
        })
        .await
        .unwrap();
    let detail = client.series_detail(added.id).await.unwrap();
    assert!(!detail.automatic_grouping);
    assert!(detail.collections.is_empty());
    assert_eq!(detail.item.path, format!("{root}/Part 2"));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        client
            .series_detail(added.id)
            .await
            .unwrap()
            .collections
            .is_empty()
    );
    // Back to automatic: grouped at once.
    let detail = client
        .update_series(
            added.id,
            &SeriesUpdate {
                automatic_grouping: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(detail.automatic_grouping);
    let mut grouped = Vec::new();
    for _ in 0..100 {
        grouped = client.series_detail(added.id).await.unwrap().collections;
        if !grouped.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(grouped[0].title, "Part 1");
    // Disabled again: the series leaves the group.
    let detail = client
        .update_series(
            added.id,
            &SeriesUpdate {
                automatic_grouping: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!detail.automatic_grouping);
    assert!(detail.collections.is_empty());
}

/// Settings › General › Backups: interval (1-7 days) and retention (1-90 days) are validated,
/// drive the Backup task and the v3 host settings, and a manual backup restarts the interval.
#[tokio::test]
async fn backup_interval_retention_and_manual_backups() {
    let server = start().await;
    let client = signed_in(&server).await;
    let mut general = client.general().await.unwrap();
    assert_eq!((general.backup_interval, general.backup_retention), (7, 28));
    general.backup_interval = 8;
    assert_eq!(status(client.put_general(&general).await.unwrap_err()), 422);
    general.backup_interval = 3;
    general.backup_retention = 91;
    assert_eq!(status(client.put_general(&general).await.unwrap_err()), 422);
    general.backup_retention = 14;
    let saved = client.put_general(&general).await.unwrap();
    assert_eq!((saved.backup_interval, saved.backup_retention), (3, 14));
    let task = |name: &str| {
        let pool = server.state.db.pool().clone();
        let name = name.to_string();
        async move {
            kitsunarr_db::tasks::list(&pool)
                .await
                .unwrap()
                .into_iter()
                .find(|t| t.name == name)
                .unwrap()
        }
    };
    assert_eq!(task("Backup").await.interval_minutes, 3 * 24 * 60);
    let host: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/v3/config/host", server.base))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (
            host["backupInterval"].as_i64(),
            host["backupRetention"].as_i64()
        ),
        (Some(3), Some(14))
    );
    // Tanukarr 1.0.4: the manual backup is the Backup task's latest run.
    sqlx::query(
        "UPDATE scheduled_tasks SET last_run_at = '2020-01-01T00:00:00Z' WHERE name = 'Backup'",
    )
    .execute(server.state.db.pool())
    .await
    .unwrap();
    let backup: serde_json::Value = client
        .post("/system/backups", &serde_json::json!({}))
        .await
        .unwrap();
    assert!(backup["name"].as_str().unwrap().ends_with("_manual.db"));
    let after = task("Backup").await;
    assert_ne!(after.last_run_at.as_deref(), Some("2020-01-01T00:00:00Z"));
    assert_eq!(after.last_status.as_deref(), Some("completed"));
    let runs = kitsunarr_db::tasks::recent_runs(server.state.db.pool(), 10)
        .await
        .unwrap();
    assert!(
        runs.iter()
            .any(|r| r.task_name == "Backup" && r.trigger == "manual")
    );
}

/// `NotificationStatusCheck` (connections backing off) and `DatabaseIntegrityCheck`.
#[tokio::test]
async fn notification_back_off_and_database_integrity_are_health_checks() {
    let server = start().await;
    let client = signed_in(&server).await;
    let make = |name: &str| {
        serde_json::json!({
            "name": name, "implementation": "Webhook", "enabled": true,
            "options": {"onGrab": true}, "settings": {"url": "http://127.0.0.1:9/hook", "method": 1}
        })
    };
    let a: serde_json::Value = client
        .post("/providers/notifications", &make("Hook A"))
        .await
        .unwrap();
    let _: serde_json::Value = client
        .post("/providers/notifications", &make("Hook B"))
        .await
        .unwrap();
    use kitsunarr_domain::api::system::{HealthCheck, HealthLevel};
    let health = || async { client.health().await.unwrap() };
    let of = |list: &[HealthCheck], source: &str| list.iter().find(|h| h.source == source).cloned();
    assert!(of(&health().await, "NotificationStatusCheck").is_none());
    assert!(of(&health().await, "DatabaseIntegrityCheck").is_none());
    let block = |id: i64| {
        let pool = server.state.db.pool().clone();
        async move {
            sqlx::query("INSERT INTO notification_status (notification_id, initial_failure, most_recent_failure, escalation_level, disabled_till) VALUES (?, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 3, '2999-01-01T00:00:00Z')")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    block(a["id"].as_i64().unwrap()).await;
    let check = of(&health().await, "NotificationStatusCheck").unwrap();
    assert_eq!(check.level, HealthLevel::Warning);
    assert_eq!(
        check.message,
        "Notifications unavailable due to failures: Hook A"
    );
    let all = client.providers("notifications").await.unwrap();
    let b = all.iter().find(|n| n.name == "Hook B").unwrap();
    block(b.id).await;
    let check = of(&health().await, "NotificationStatusCheck").unwrap();
    assert_eq!(check.level, HealthLevel::Error);
    assert_eq!(
        check.message,
        "All notifications are unavailable due to failures"
    );
    // The integrity check is cached between scheduled runs.
    let mut conn = server.state.db.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO series_tags (series_id, tag_id) VALUES (999, 999)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(of(&health().await, "DatabaseIntegrityCheck").is_none());
    kitsunarr_api::routes::system::check_health_and_notify(&server.state)
        .await
        .unwrap();
    let check = of(&health().await, "DatabaseIntegrityCheck").unwrap();
    assert_eq!(check.level, HealthLevel::Error);
    // The row misses both its series and its tag.
    assert_eq!(
        check.message,
        "SQLite database integrity check failed: 2 foreign key violations. Restore from a known-good backup."
    );
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
            path: None,
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
    assert_eq!(status["appName"], "Sonarr");
    assert_eq!(status["productName"], "Kitsunarr");
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
    // The command is tracked until it ends (Sonarr's `CommandResource`).
    let started: Value = command.json().await.unwrap();
    assert_eq!(started["name"], "RescanSeries");
    assert_eq!(started["body"]["seriesId"], id);
    let mut done = Value::Null;
    for _ in 0..100 {
        done = get(&format!("/command/{}", started["id"]))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if done["status"] != "started" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // The test's root folder is empty, so the scan is refused, and the command says so.
    assert_eq!(done["status"], "failed", "{done}");
    assert_eq!(done["result"], "unsuccessful");
    assert!(
        done["message"].as_str().unwrap().ends_with("is empty."),
        "{done}"
    );
    let listed: Value = get("/command").await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["id"], started["id"]);
    assert_eq!(get("/command/999999").await.unwrap().status(), 404);
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
async fn sonarr_health_checks_report_indexer_list_and_folder_problems() {
    let server = start().await;
    let client = signed_in(&server).await;
    let sources = |c: &Client| {
        let c = c.clone();
        async move {
            c.health()
                .await
                .unwrap()
                .into_iter()
                .map(|h| (h.source, h.message))
                .collect::<Vec<_>>()
        }
    };
    // A Jackett "all" indexer with RSS off (an indexer tied to a missing client is refused when
    // saved, so `IndexerDownloadClientCheck` only reports older data).
    client
        .post::<serde_json::Value, _>(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Jackett all", "implementation": "Torznab",
                "options": {"enableRss": false, "enableAutomaticSearch": true, "enableInteractiveSearch": true},
                "settings": {"baseUrl": "http://jackett:9117/api/v2.0/indexers/all/results/torznab", "apiPath": "/api", "apiKey": "k"}
            }),
        )
        .await
        .unwrap();
    // A recycle bin that cannot be written, and completed download handling off.
    let mut mm = client.media_management().await.unwrap();
    mm.recycle_bin = Some("/nonexistent/kitsunarr-recycle".into());
    client.put_media_management(&mm).await.unwrap();
    let health = sources(&client).await;
    let has = |src: &str, text: &str| health.iter().any(|(s, m)| s == src && m.contains(text));
    assert!(
        has(
            "IndexerRssCheck",
            "No indexers available with RSS sync enabled"
        ),
        "{health:#?}"
    );
    assert!(has("IndexerJackettAllCheck", "Jackett all"), "{health:#?}");
    assert!(has("RecyclingBinCheck", "/nonexistent/kitsunarr-recycle"));
    assert!(
        !health.iter().any(|(s, _)| s == "ApiKeyValidationCheck"),
        "generated keys are long"
    );
}

#[tokio::test]
async fn ical_feed_lists_episodes_for_calendar_apps() {
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
    client
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
            path: None,
        })
        .await
        .unwrap();
    let http = reqwest::Client::new();
    let feed = |q: &str| {
        format!(
            "{}/feed/v3/calendar/Sonarr.ics?pastDays=10000&futureDays=1{q}",
            server.base
        )
    };
    // The API key is required (in the query, as calendar apps send it).
    assert_eq!(http.get(feed("")).send().await.unwrap().status(), 401);
    let resp = http
        .get(feed(&format!("&apikey={}", server.api_key)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/calendar")
    );
    let ics = resp.text().await.unwrap();
    assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
    assert!(
        ics.contains("SUMMARY:Breaking Bad - 1x01 - Pilot\r\n"),
        "{ics}"
    );
    assert!(ics.contains("STATUS:TENTATIVE"), "no file yet");
    assert!(ics.contains("CATEGORIES:AMC"));
    let premieres = http
        .get(feed(&format!(
            "&premieresOnly=true&apikey={}",
            server.api_key
        )))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(premieres.matches("BEGIN:VEVENT").count() < ics.matches("BEGIN:VEVENT").count());
    assert!(premieres.contains("1x01"));
    assert!(!premieres.contains("1x02"));
    // A tag no series has leaves nothing.
    client.create_tag("none").await.unwrap();
    let tagged = http
        .get(feed(&format!("&tags=none&apikey={}", server.api_key)))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!tagged.contains("BEGIN:VEVENT"));
}

#[tokio::test]
async fn auto_tagging_applies_on_add_edit_and_refresh() {
    use kitsunarr_domain::api::library::SeriesUpdate;
    use kitsunarr_domain::api::settings::{AutoTag, CustomFormatSpecification};
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
    let american = client.create_tag("american").await.unwrap().id;
    let daily = client.create_tag("daily").await.unwrap().id;
    let spec = |implementation: &str, fields: serde_json::Value| CustomFormatSpecification {
        name: implementation.trim_end_matches("Specification").into(),
        implementation: implementation.into(),
        negate: false,
        required: false,
        fields: fields.as_object().cloned().unwrap(),
    };
    // Sonarr's validation: tags and conditions are required.
    let invalid = AutoTag {
        id: 0,
        name: "Nothing".into(),
        remove_tags_automatically: false,
        specifications: vec![],
        tags: vec![american],
    };
    assert_eq!(
        status(client.save_auto_tag(&invalid).await.unwrap_err()),
        400
    );
    let rule = client
        .save_auto_tag(&AutoTag {
            id: 0,
            name: "American drama".into(),
            remove_tags_automatically: false,
            specifications: vec![
                spec(
                    "OriginalCountrySpecification",
                    serde_json::json!({"value": ["USA"]}),
                ),
                spec(
                    "GenreSpecification",
                    serde_json::json!({"value": ["Drama"]}),
                ),
                spec(
                    "GenreSpecification",
                    serde_json::json!({"value": ["Comedy"]}),
                ),
            ],
            tags: vec![american],
        })
        .await
        .unwrap();
    assert!(rule.id > 0);
    let daily_rule = client
        .save_auto_tag(&AutoTag {
            id: 0,
            name: "Daily shows".into(),
            remove_tags_automatically: true,
            specifications: vec![spec(
                "SeriesTypeSpecification",
                serde_json::json!({"value": 1}),
            )],
            tags: vec![daily],
        })
        .await
        .unwrap();
    assert_eq!(client.auto_tags().await.unwrap().len(), 2);

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
            path: None,
        })
        .await
        .unwrap();
    // SkyHook's `originalCountry` ("usa") and a drama genre: tagged when added.
    let tags = |id| {
        let c = client.clone();
        async move { c.series_detail(id).await.unwrap().item.tags }
    };
    assert_eq!(tags(added.id).await, vec![american]);

    // An edit that makes it daily adds the second rule's tag; a manual tag removal of the
    // first is undone by the next refresh, as in Sonarr.
    client
        .update_series(
            added.id,
            &SeriesUpdate {
                series_type: Some(kitsunarr_domain::SeriesType::Daily),
                tags: Some(vec![]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut now = tags(added.id).await;
    now.sort_unstable();
    assert_eq!(now, vec![american, daily]);
    client
        .update_series(
            added.id,
            &SeriesUpdate {
                series_type: Some(kitsunarr_domain::SeriesType::Standard),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        tags(added.id).await,
        vec![american],
        "removed automatically"
    );

    // Deleting the rule leaves the tags; the rule is gone.
    client.delete_auto_tag(daily_rule.id).await.unwrap();
    client.refresh_series(added.id).await.unwrap();
    assert_eq!(tags(added.id).await, vec![american]);
    assert_eq!(client.auto_tags().await.unwrap().len(), 1);
    // The rule counts as a use of its tag.
    let listed = client.tags().await.unwrap();
    assert_eq!(
        listed
            .iter()
            .find(|t| t.id == american)
            .unwrap()
            .usage_count,
        2
    );
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
    assert_eq!(d.entries.len(), 2);
    assert_eq!(
        (
            d.entries[0].series_id,
            d.entries[0].display_number,
            d.entries[0].section.as_str()
        ),
        (Some(1), 1, "seasons")
    );
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
    assert!(found.considered_titles.contains(&"The Matrix 1999".into()));
    assert_eq!(found.diagnostics.len(), 1);
    assert!(
        found.diagnostics[0]
            .planned_queries
            .iter()
            .any(|p| p.contains("The Matrix 1999"))
    );
    let source = client.providers("indexers").await.unwrap()[0].id;
    let before = searches.lock().unwrap().len();
    let no_sources = client
        .search_movie_releases_selected(
            7,
            &kitsunarr_domain::api::search::MovieReleaseQuery {
                indexer_ids: Some(String::new()),
                search_session: Some("movie-search".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(no_sources.searched_indexers, 0);
    assert_eq!(searches.lock().unwrap().len(), before);
    let one = client
        .search_movie_releases_selected(
            7,
            &kitsunarr_domain::api::search::MovieReleaseQuery {
                indexer_ids: Some(source.to_string()),
                search_session: Some("movie-search".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(one.releases.len(), found.releases.len());
    assert_eq!(one.diagnostics[0].status, "success");
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

/// A local TheXEM: Breaking Bad's 1x07 is the scene's 2x01, and "Breaking Bad Part Two" names
/// scene season 2.
async fn xem_mock() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    use axum::extract::{Path, Query, State};
    use axum::routing::get;
    type Log = Arc<std::sync::Mutex<Vec<String>>>;
    async fn map(
        State(log): State<Log>,
        Path(resource): Path<String>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::Json<serde_json::Value> {
        assert_eq!(q.get("origin").map(String::as_str), Some("tvdb"));
        log.lock().unwrap().push(resource.clone());
        let v =
            |s: i32, e: i32, a: i32| serde_json::json!({"season": s, "episode": e, "absolute": a});
        axum::Json(match resource.as_str() {
            "havemap" => {
                serde_json::json!({"result": "success", "data": ["81189", "73244"], "message": ""})
            }
            "all" => {
                assert_eq!(q.get("id").map(String::as_str), Some("81189"));
                let mut data: Vec<serde_json::Value> = (1..=6)
                    .map(|e| serde_json::json!({"scene": v(1, e, e), "tvdb": v(1, e, e)}))
                    .collect();
                data.push(serde_json::json!({"scene": v(2, 1, 7), "tvdb": v(1, 7, 7)}));
                serde_json::json!({"result": "success", "data": data, "message": ""})
            }
            "allNames" => serde_json::json!({"result": "success", "data": {
                "81189": [{"Breaking Bad": -1}, {"Breaking Bad Part Two": 2}],
                "73244": [{"The Office": -1}]
            }, "message": ""}),
            _ => serde_json::json!({"result": "failure", "data": [], "message": "unknown"}),
        })
    }
    let log: Log = Arc::default();
    let app = axum::Router::new()
        .route("/map/{resource}", get(map))
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), log)
}

#[tokio::test]
async fn thexem_scene_numbering_is_applied_and_searched() {
    use kitsunarr_domain::api::search::ReleaseQuery;
    let (skyhook, _) = skyhook_mock().await;
    let (xem, xem_log) = xem_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvdb: skyhook,
            xem,
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
            path: None,
        })
        .await
        .unwrap();
    client.refresh_series(added.id).await.unwrap();
    assert!(xem_log.lock().unwrap().contains(&"all".to_string()));

    let detail = client.series_detail(added.id).await.unwrap();
    assert!(detail.use_scene_numbering);
    assert!(
        detail
            .alternate_titles
            .contains(&"Breaking Bad Part Two".to_string())
    );
    let episodes = client.episodes(added.id).await.unwrap();
    let ep = |s: i32, n: i32| {
        episodes
            .iter()
            .find(|e| e.season_number == s && e.episode_number == n)
            .unwrap()
            .clone()
    };
    let e7 = ep(1, 7);
    assert_eq!(
        (
            e7.scene_season_number,
            e7.scene_episode_number,
            e7.scene_absolute_episode_number,
            e7.unverified_scene_numbering
        ),
        (Some(2), Some(1), Some(7), false)
    );
    // TheTVDB's season 2 is past TheXEM's mappings, which shift seasons by one.
    let s2 = ep(2, 1);
    assert_eq!(
        (
            s2.scene_season_number,
            s2.scene_episode_number,
            s2.unverified_scene_numbering
        ),
        (Some(3), Some(1), true)
    );

    let (indexer_url, requests) = newznab_mock().await;
    let _: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Mock Newznab", "implementation": "Newznab",
                "options": {"enableRss": true, "enableAutomaticSearch": true, "enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "k", "categories": [5030, 5040]}
            }),
        )
        .await
        .unwrap();
    requests.lock().unwrap().clear();
    let found = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: None,
            episode_id: Some(e7.id),
            complete_series: false,
            season_packs_only: false,
            indexer_ids: None,
            search_session: None,
        })
        .await
        .unwrap();
    assert_eq!(found.description, "S01E07 (scene S02E01)");
    let searched: Vec<String> = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("t=tvsearch"))
        .cloned()
        .collect();
    assert!(!searched.is_empty());
    assert!(
        searched
            .iter()
            .all(|r| r.contains("season=2") && r.contains("ep=1")),
        "{searched:?}"
    );

    // A season search asks for each scene season the season's episodes fall in.
    requests.lock().unwrap().clear();
    let season = client
        .search_releases(&ReleaseQuery {
            series_id: added.id,
            season_number: Some(1),
            episode_id: None,
            complete_series: false,
            season_packs_only: false,
            indexer_ids: None,
            search_session: None,
        })
        .await
        .unwrap();
    assert_eq!(season.description, "Season 1 (scene seasons 1, 2)");
    let seasons: Vec<bool> = ["season=1", "season=2"]
        .iter()
        .map(|s| {
            requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.contains("t=tvsearch") && r.contains(s))
        })
        .collect();
    assert_eq!(seasons, [true, true]);
    // Tanukarr's numbering resolver: a season pack of a season whose numbering differs is
    // ambiguous without a title or an origin to decide.
    let pack = season
        .releases
        .iter()
        .find(|r| r.title == "Breaking.Bad.S01.480p.WEB-DL.AAC2.0.H.264-GRP")
        .unwrap();
    assert!(
        pack.rejections
            .iter()
            .any(|r| r.starts_with("Ambiguous scene/library numbering")),
        "{:?}",
        pack.rejections
    );
}

/// More of Sonarr's v3, as nzb360, LunaSea and Homepage call it: interactive search and grab
/// (`/release`), `/parse`, `/diskspace`, and queue and episode-file removal.
#[tokio::test]
async fn sonarr_v3_release_parse_diskspace_and_removals() {
    use serde_json::{Value, json};
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
    let (indexer_url, _) = newznab_mock().await;
    let _: Value = client
        .post(
            "/providers/indexers",
            &json!({
                "name": "Mock Newznab", "implementation": "Newznab",
                "options": {"enableRss": true, "enableAutomaticSearch": true, "enableInteractiveSearch": true},
                "settings": {"baseUrl": indexer_url, "apiPath": "/api", "apiKey": "k", "categories": [5030, 5040]}
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
            path: None,
        })
        .await
        .unwrap();

    let http = reqwest::Client::new();
    let url = |p: &str| format!("{}/api/v3{p}", server.base);
    let key = server.api_key.clone();
    let get = |p: String| http.get(url(&p)).header("X-Api-Key", &key).send();

    let releases: Value = get(format!("/release?seriesId={}&seasonNumber=1", added.id))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pack = releases
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Breaking.Bad.S01.480p.WEB-DL.AAC2.0.H.264-GRP")
        .unwrap();
    assert_eq!(pack["fullSeason"], true);
    assert_eq!(pack["seasonNumber"], 1);
    assert_eq!(pack["protocol"], "usenet");
    assert_eq!(pack["seriesId"], added.id);
    assert_eq!(pack["quality"]["quality"]["name"], "WEBDL-480p");
    // A release no longer in the search cache cannot be grabbed.
    let stale = http
        .post(url("/release"))
        .header("X-Api-Key", &key)
        .json(&json!({"guid": "nope", "indexerId": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 404);

    let parsed: Value = get("/parse?title=Breaking.Bad.S01E02.720p.HDTV.x264-GRP".into())
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(parsed["parsedEpisodeInfo"]["seriesTitle"], "Breaking Bad");
    assert_eq!(parsed["parsedEpisodeInfo"]["episodeNumbers"], json!([2]));
    assert_eq!(parsed["series"]["title"], "Breaking Bad");
    assert_eq!(parsed["episodes"][0]["episodeNumber"], 2);

    let disks: Value = get("/diskspace".into())
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(disks[0]["path"], dir.path().display().to_string());
    assert!(disks[0]["totalSpace"].as_i64().unwrap() > 0);

    let missing = |p: &str| http.delete(url(p)).header("X-Api-Key", &key).send();
    assert_eq!(missing("/queue/12345").await.unwrap().status(), 404);
    assert_eq!(missing("/episodefile/12345").await.unwrap().status(), 404);
}

/// Sonarr's v3 settings as Recyclarr syncs TRaSH guides: custom formats, quality definition
/// sizes, profile scores and release profiles.
#[tokio::test]
async fn sonarr_v3_settings_serve_recyclarr_sync() {
    use serde_json::{Value, json};
    let server = start().await;
    let http = reqwest::Client::new();
    let url = |p: &str| format!("{}/api/v3{p}", server.base);
    let key = server.api_key.clone();
    let send = |m: reqwest::Method, p: &str, body: Option<Value>| {
        let mut r = http.request(m, url(p)).header("X-Api-Key", &key);
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send()
    };
    let json_of = |r: reqwest::Response| async move {
        let status = r.status();
        let v: Value = r.json().await.unwrap();
        (status.as_u16(), v)
    };
    use reqwest::Method as M;

    let schema = json_of(send(M::GET, "/customformat/schema", None).await.unwrap())
        .await
        .1;
    assert!(
        schema
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["implementation"] == "ReleaseTitleSpecification")
    );
    let (status, cf) = json_of(
        send(
            M::POST,
            "/customformat",
            Some(json!({
                "name": "x265 (HD)",
                "includeCustomFormatWhenRenaming": false,
                "specifications": [{"name": "x265", "implementation": "ReleaseTitleSpecification",
                    "negate": false, "required": false, "fields": [{"name": "value", "value": "[xh][ ._-]?265|\\bHEVC(\\b|\\d)"}]}]
            })),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "{cf}");
    let cf_id = cf["id"].as_i64().unwrap();
    assert_eq!(
        cf["specifications"][0]["fields"][0]["value"],
        "[xh][ ._-]?265|\\bHEVC(\\b|\\d)"
    );
    let mut renamed = cf.clone();
    renamed["name"] = json!("x265");
    let (status, updated) = json_of(
        send(M::PUT, &format!("/customformat/{cf_id}"), Some(renamed))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((status, updated["name"].as_str()), (202, Some("x265")));

    // Sizes for one quality (Recyclarr's `PUT /qualitydefinition/update`).
    let defs = json_of(send(M::GET, "/qualitydefinition", None).await.unwrap())
        .await
        .1;
    let mut hdtv = defs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["quality"]["name"] == "HDTV-720p")
        .unwrap()
        .clone();
    hdtv["minSize"] = json!(10.5);
    hdtv["preferredSize"] = json!(95.0);
    hdtv["maxSize"] = json!(100.0);
    let (status, saved) = json_of(
        send(M::PUT, "/qualitydefinition/update", Some(json!([hdtv])))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(saved[0]["minSize"], 10.5);
    assert_eq!(saved[0]["maxSize"], 100.0);
    // Recyclarr leaves out null sizes (TRaSH's unlimited maximum): as in Sonarr, they are no
    // limit, so its next sync finds nothing to change (seen with a real Recyclarr 7 sync).
    let (status, saved) = json_of(
        send(
            M::PUT,
            "/qualitydefinition/update",
            Some(json!([{"id": hdtv["id"], "quality": hdtv["quality"], "title": "HDTV-720p", "weight": hdtv["weight"], "minSize": 10}])),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(
        (
            &saved[0]["minSize"],
            &saved[0]["maxSize"],
            &saved[0]["preferredSize"]
        ),
        (&json!(10.0), &Value::Null, &Value::Null)
    );

    // A profile from the schema, with the format scored.
    let mut profile = json_of(send(M::GET, "/qualityprofile/schema", None).await.unwrap())
        .await
        .1;
    profile["name"] = json!("WEB-1080p");
    for item in profile["items"].as_array_mut().unwrap() {
        if item["quality"]["name"] == "WEBDL-1080p" || item["quality"]["name"] == "HDTV-720p" {
            item["allowed"] = json!(true);
        }
    }
    let webdl = profile["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["quality"]["name"] == "WEBDL-1080p")
        .unwrap()["quality"]["id"]
        .clone();
    profile["cutoff"] = webdl;
    for f in profile["formatItems"].as_array_mut().unwrap() {
        if f["format"] == cf_id {
            f["score"] = json!(-10000);
        }
    }
    profile["minFormatScore"] = json!(0);
    let (status, created) = json_of(
        send(M::POST, "/qualityprofile", Some(profile))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let pid = created["id"].as_i64().unwrap();
    let scored = created["formatItems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["format"] == cf_id)
        .unwrap()
        .clone();
    assert_eq!(scored["score"], -10000);
    let mut changed = created.clone();
    changed["upgradeAllowed"] = json!(true);
    let (status, put) = json_of(
        send(M::PUT, &format!("/qualityprofile/{pid}"), Some(changed))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((status, put["upgradeAllowed"].as_bool()), (202, Some(true)));
    let listed = json_of(send(M::GET, "/qualityprofile", None).await.unwrap())
        .await
        .1;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "WEB-1080p")
    );

    // Release profiles with Sonarr v4's term lists.
    let (status, rp) = json_of(
        send(
            M::POST,
            "/releaseprofile",
            Some(json!({"name": "No x265", "enabled": true, "required": [], "ignored": ["x265", "HEVC"], "indexerId": 0, "tags": []})),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "{rp}");
    assert_eq!(rp["ignored"], json!(["x265", "HEVC"]));
    let rid = rp["id"].as_i64().unwrap();
    assert_eq!(
        send(M::DELETE, &format!("/releaseprofile/{rid}"), None)
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        send(M::DELETE, &format!("/customformat/{cf_id}"), None)
            .await
            .unwrap()
            .status(),
        200
    );
    let languages = json_of(send(M::GET, "/language", None).await.unwrap())
        .await
        .1;
    assert!(
        languages
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["name"] == "English")
    );
}

/// Sonarr's SignalR hub as Bazarr uses it: negotiate, the JSON hub handshake over a WebSocket
/// (key in `access_token`), then `receiveMessage` invocations for library changes.
#[tokio::test]
async fn signalr_hub_streams_series_changes() {
    use futures::{SinkExt, StreamExt};
    use serde_json::Value;
    use tokio_tungstenite::tungstenite::Message;
    let server = start().await;
    let http = reqwest::Client::new();
    let negotiated: Value = http
        .post(format!(
            "{}/signalr/messages/negotiate?negotiateVersion=1",
            server.base
        ))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        negotiated["availableTransports"][0]["transport"],
        "WebSockets"
    );
    let token = negotiated["connectionToken"].as_str().unwrap();
    // Without a key: refused.
    let refused = http
        .post(format!("{}/signalr/messages/negotiate", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401);

    let ws_url = format!(
        "{}/signalr/messages?id={token}&access_token={}",
        server.base.replace("http://", "ws://"),
        server.api_key
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
    ws.send(Message::text("{\"protocol\":\"json\",\"version\":1}\u{1e}"))
        .await
        .unwrap();
    let handshake = ws.next().await.unwrap().unwrap();
    assert_eq!(handshake.to_text().unwrap(), "{}\u{1e}");

    server
        .state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: vec![42],
            deleted: false,
        });
    let message = loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = frame
            .to_text()
            .unwrap()
            .trim_end_matches('\u{1e}')
            .to_string();
        let v: Value = serde_json::from_str(&text).unwrap();
        if v["type"] == 1 {
            break v;
        }
    };
    assert_eq!(message["target"], "receiveMessage");
    assert_eq!(message["arguments"][0]["name"], "series");
    assert_eq!(message["arguments"][0]["body"]["action"], "updated");
    assert_eq!(message["arguments"][0]["body"]["resource"]["id"], 42);
    ws.send(Message::text("{\"type\":7}\u{1e}")).await.unwrap();
}

/// Sonarr's v3 download clients and connections written by tools: create from the schema,
/// update keeping the stored secret, delete.
#[tokio::test]
async fn sonarr_v3_download_clients_and_notifications_are_written() {
    use serde_json::{Value, json};
    let server = start().await;
    let http = reqwest::Client::new();
    let url = |p: &str| format!("{}/api/v3{p}", server.base);
    let key = server.api_key.clone();
    let send = |m: reqwest::Method, p: &str, body: Option<Value>| {
        let mut r = http.request(m, url(p)).header("X-Api-Key", &key);
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send()
    };
    use reqwest::Method as M;
    let schema: Value = send(M::GET, "/downloadclient/schema", None)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut qbit = schema
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["implementation"] == "QBittorrent")
        .unwrap()
        .clone();
    qbit["name"] = json!("qBittorrent");
    for f in qbit["fields"].as_array_mut().unwrap() {
        if f["name"] == "password" {
            f["value"] = json!("s3cret");
        }
    }
    let created = send(M::POST, "/downloadclient", Some(qbit)).await.unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    let id = created["id"].as_i64().unwrap();
    let password = |v: &Value| {
        v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "password")
            .unwrap()["value"]
            .clone()
    };
    assert_ne!(password(&created), "s3cret", "secrets come back redacted");
    let mut renamed = created.clone();
    renamed["name"] = json!("qB");
    let updated = send(M::PUT, &format!("/downloadclient/{id}"), Some(renamed))
        .await
        .unwrap();
    assert_eq!(updated.status(), 202);
    let stored = kitsunarr_db::providers::get(
        server.state.db.pool(),
        kitsunarr_domain::api::settings::ProviderKind::DownloadClient,
        id,
    )
    .await
    .unwrap();
    assert_eq!(stored.name, "qB");
    assert_eq!(
        stored.settings["password"], "s3cret",
        "the redacted secret is kept"
    );

    let hook = send(
        M::POST,
        "/notification",
        Some(json!({
            "name": "Hook", "implementation": "Webhook", "onGrab": true, "onDownload": false,
            "tags": [], "fields": [{"name": "url", "value": "http://127.0.0.1:9/hook"}, {"name": "method", "value": 1}]
        })),
    )
    .await
    .unwrap();
    assert_eq!(hook.status(), 201);
    let hook: Value = hook.json().await.unwrap();
    assert_eq!(hook["onGrab"], true);
    assert_eq!(hook["onDownload"], false);
    let nid = hook["id"].as_i64().unwrap();
    assert_eq!(
        send(M::DELETE, &format!("/notification/{nid}"), None)
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        send(M::DELETE, &format!("/downloadclient/{id}"), None)
            .await
            .unwrap()
            .status(),
        200
    );
    let unsupported = send(
        M::POST,
        "/downloadclient",
        Some(json!({"name": "x", "implementation": "Nope", "fields": []})),
    )
    .await
    .unwrap();
    assert_eq!(unsupported.status(), 422);
}

/// Sonarr's "Connect to Twitter": a request token with Kitsunarr's callback, X's authorize page,
/// the callback's verifier, then the account's access token filled in.
#[tokio::test]
async fn twitter_connections_sign_in_with_oauth1() {
    use axum::routing::post;
    use kitsunarr_domain::api::settings::SignInRequest;
    let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let (s1, s2) = (seen.clone(), seen.clone());
    let app = axum::Router::new()
        .route(
            "/oauth/request_token",
            post(move |h: axum::http::HeaderMap| async move {
                s1.lock()
                    .unwrap()
                    .push(h["authorization"].to_str().unwrap().to_string());
                "oauth_token=rt&oauth_token_secret=rs&oauth_callback_confirmed=true"
            }),
        )
        .route(
            "/oauth/access_token",
            post(move |h: axum::http::HeaderMap| async move {
                s2.lock()
                    .unwrap()
                    .push(h["authorization"].to_str().unwrap().to_string());
                "oauth_token=at&oauth_token_secret=as&screen_name=me"
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let server = start().await;
    let client = signed_in(&server).await;
    let settings: serde_json::Map<String, serde_json::Value> = serde_json::from_value(
        serde_json::json!({"apiUrl": api, "consumerKey": "ck", "consumerSecret": "cs"}),
    )
    .unwrap();
    let mut req = SignInRequest {
        id: None,
        implementation: "Twitter".into(),
        settings,
        callback: Some(format!("{}/api/v1/importlists/oauth/callback", server.base)),
        state: None,
    };
    let started = client.sign_in_start(&req).await.unwrap();
    assert_eq!(started.url, format!("{api}/oauth/authorize?oauth_token=rt"));
    assert!(seen.lock().unwrap()[0].contains("oauth_callback=\""));
    req.state = Some(started.state.clone());
    req.callback = None;
    // Not authorized yet.
    assert!(!client.sign_in_poll(&req).await.unwrap().done);
    let page = reqwest::get(format!(
        "{}/api/v1/importlists/oauth/callback?oauth_token=rt&oauth_verifier=v123",
        server.base
    ))
    .await
    .unwrap();
    assert_eq!(page.status(), 200);
    let polled = client.sign_in_poll(&req).await.unwrap();
    assert!(polled.done);
    assert_eq!(polled.settings["accessToken"], "at");
    assert_eq!(polled.settings["accessTokenSecret"], "as");
    let exchange = seen.lock().unwrap()[1].clone();
    assert!(
        exchange.contains("oauth_verifier=\"v123\"") && exchange.contains("oauth_token=\"rt\""),
        "{exchange}"
    );
}

/// The same typed calls as the catalogue dialog, through the authenticated real router.
#[tokio::test]
async fn metadata_source_previews_and_changes_work_over_http() {
    use kitsunarr_domain::api::metadata::MetadataSourceRequest;
    let base = tvmaze_mock().await;
    let movie_router=axum::Router::new().route("/",axum::routing::get(|| async {
        axum::Json(serde_json::json!({"Title":"A Film","Type":"movie","imdbID":"tt0000060","Year":"2020","Response":"True"}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let movie_base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, movie_router).await.unwrap() });
    let metadata = Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: base,
            omdb: movie_base,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    );
    let server = start_with(metadata).await;
    let client = signed_in(&server).await;
    // AppState loads keys from settings; set the mock key after initialization.
    server
        .state
        .metadata
        .set_api_keys(None, Some("test-key".into()));
    sqlx::query("INSERT INTO series (id,media_kind,title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (1,'series','Breaking Bad','breakingbad','breakingbad','breaking-bad','tvdb','81189',1,'/media','/media/bb','2026-01-01')").execute(server.state.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (1,'A Film','film','film','film','tmdbMovie','60',1,'/media','/media/film','2026-01-01')").execute(server.state.db.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO movie_external_ids (movie_id,kind,external_id) VALUES (1,'imdb','tt0000060')",
    )
    .execute(server.state.db.pool())
    .await
    .unwrap();
    for movie in [false, true] {
        let mut req = MetadataSourceRequest {
            provider: if movie {
                MetadataProvider::Omdb
            } else {
                MetadataProvider::TvMaze
            },
            id: if movie { "tt0000060" } else { "169" }.into(),
            confirm_identity: false,
            mappings: vec![],
            revision: None,
        };
        let section = if movie { "movies" } else { "series" };
        let unauth = reqwest::Client::new()
            .post(format!(
                "{}/api/v1/{section}/1/metadata-source/preview",
                server.base
            ))
            .json(&req)
            .send()
            .await
            .unwrap();
        assert_eq!(unauth.status(), 401);
        assert_eq!(
            status(
                client
                    .set_metadata_source(movie, 1, &req)
                    .await
                    .unwrap_err()
            ),
            422
        );
        assert_eq!(
            status(
                client
                    .preview_metadata_source(movie, 999, &req)
                    .await
                    .unwrap_err()
            ),
            404
        );
        let p = client
            .preview_metadata_source(movie, 1, &req)
            .await
            .unwrap();
        assert!(p.identity_verified);
        assert!(p.blockers.is_empty(), "{:?}", p.blockers);
        req.revision = Some(p.revision);
        client.set_metadata_source(movie, 1, &req).await.unwrap();
        // The cached identity is now the current owner; trying the same entry is refused.
        assert_eq!(
            status(
                client
                    .preview_metadata_source(movie, 1, &req)
                    .await
                    .unwrap_err()
            ),
            422
        );
    }
    assert_eq!(
        client
            .series_detail(1)
            .await
            .unwrap()
            .item
            .metadata_provider,
        MetadataProvider::TvMaze
    );
    assert_eq!(
        client.movie_detail(1).await.unwrap().item.metadata_provider,
        MetadataProvider::Omdb
    );
    let openapi: serde_json::Value = client.get("/openapi.json").await.unwrap();
    assert!(openapi["paths"]["/api/v1/series/{id}/metadata-source/preview"]["post"].is_object());
}

/// Film file operations use the same authenticated client as the UI, against real temporary
/// folders. No catalogue or download client is contacted.
#[tokio::test]
async fn movie_files_scan_preview_rename_recycle_and_optional_folder_removal() {
    use kitsunarr_domain::api::settings::MediaManagementSettings;
    let server = start().await;
    let client = signed_in(&server).await;
    let root = server._dir.path().join("film-library");
    let bin = server._dir.path().join("film-recycle");
    std::fs::create_dir_all(&root).unwrap();
    kitsunarr_db::root_folders::create(server.state.db.pool(), root.to_str().unwrap(), None)
        .await
        .unwrap();
    kitsunarr_db::settings::set(
        server.state.db.pool(),
        kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
        &MediaManagementSettings {
            enable_media_info: false,
            recycle_bin: Some(bin.display().to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for id in 1..=3 {
        let folder = root.join(format!("Film {id}"));
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("source.1080p.WEB-DL-GRP.mkv"), b"feature").unwrap();
        sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,year,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (?,?,'film','film',?,2020,'tmdbMovie',?,1,?,?,'2026-11-14T00:00:00Z')")
            .bind(id).bind(format!("Film {id}")).bind(format!("film-{id}")).bind(id.to_string()).bind(root.to_str().unwrap()).bind(folder.to_str().unwrap()).execute(server.state.db.pool()).await.unwrap();
    }
    let raw = reqwest::Client::new();
    assert_eq!(
        raw.post(format!("{}/api/v1/movies/1/rescan", server.base))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(client.movie_rename_preview(1).await.unwrap().is_none());
    client.rescan_movie(1).await.unwrap();
    let preview = client.movie_rename_preview(1).await.unwrap().unwrap();
    assert!(root.join("Film 1").join(&preview.existing_path).is_file());
    assert!(!root.join("Film 1").join(&preview.new_path).exists());
    let mut tampered = preview.clone();
    tampered.new_path = "../forbidden.mkv".into();
    assert!(client.rename_movie_file(1, &tampered).await.is_err());
    client.rename_movie_file(1, &preview).await.unwrap();
    assert!(root.join("Film 1").join(&preview.new_path).is_file());
    assert!(!root.join("Film 1").join(&preview.existing_path).exists());
    assert!(client.movie_rename_preview(1).await.unwrap().is_none());
    assert!(
        client
            .delete_movie_file(1, preview.movie_file_id + 1000)
            .await
            .is_err()
    );
    client
        .delete_movie_file(1, preview.movie_file_id)
        .await
        .unwrap();
    assert!(client.movie_detail(1).await.unwrap().file.is_none());
    assert!(bin.join("Film 1").join(&preview.new_path).is_file());
    // Removing only the record remains the default, including when the folder has a video.
    client.rescan_movie(2).await.unwrap();
    client.delete_movie(2).await.unwrap();
    assert!(root.join("Film 2/source.1080p.WEB-DL-GRP.mkv").is_file());
    client.rescan_movie(3).await.unwrap();
    client.delete_movie_with_files(3).await.unwrap();
    assert!(!root.join("Film 3").exists());
    assert!(bin.join("Film 3/source.1080p.WEB-DL-GRP.mkv").is_file());
    assert!(client.movie_detail(3).await.is_err());
    assert!(client.rescan_movie(9999).await.is_err());
    let schema: serde_json::Value = client.get("/openapi.json").await.unwrap();
    assert!(schema["paths"]["/api/v1/movies/{id}/rename"]["post"].is_object());
}

#[tokio::test]
async fn movie_library_tools_import_audit_and_recycle_restore_over_http() {
    use kitsunarr_domain::api::library_tools::{
        MovieImportFile, MovieImportRequest, MovieRecycleRestoreRequest,
    };
    use kitsunarr_domain::api::settings::MediaManagementSettings;
    let server = start().await;
    let client = signed_in(&server).await;
    let root = server._dir.path().join("films");
    let folder = root.join("Film");
    let source = server
        ._dir
        .path()
        .join("downloads/Film.2020.WEB-DL.1080p.mkv");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, b"feature").unwrap();
    std::fs::write(source.with_extension("it.srt"), b"subtitle").unwrap();
    kitsunarr_db::root_folders::create(server.state.db.pool(), root.to_str().unwrap(), None)
        .await
        .unwrap();
    kitsunarr_db::settings::set(
        server.state.db.pool(),
        kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
        &MediaManagementSettings {
            enable_media_info: false,
            recycle_bin: Some(server._dir.path().join("recycled").display().to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,year,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (1,'Film','film','film','film',2020,'tmdbMovie','1',1,?,?,'2026-11-14T00:00:00Z')")
        .bind(root.to_str().unwrap()).bind(folder.to_str().unwrap()).execute(server.state.db.pool()).await.unwrap();
    assert_eq!(
        reqwest::Client::new()
            .get(format!(
                "{}/api/v1/manualimport/movies?folder=/",
                server.base
            ))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let scan = client
        .movie_import_scan(source.parent().unwrap().to_str().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(scan[0].movie_id, Some(1));
    assert!(scan[0].rejections.is_empty());
    assert!(source.is_file());
    assert!(client.movie_detail(1).await.unwrap().file.is_none());
    let mut request = MovieImportRequest {
        files: vec![MovieImportFile {
            path: source.display().to_string(),
            movie_id: 1,
        }],
        import_mode: "invalid".into(),
    };
    assert!(client.movie_import(&request).await.is_err());
    assert!(source.is_file());
    request.import_mode = "copy".into();
    let results = client.movie_import(&request).await.unwrap();
    assert!(results[0].imported, "{:?}", results[0].messages);
    assert!(source.is_file());
    let file = client.movie_detail(1).await.unwrap().file.unwrap();
    assert!(folder.join(&file.relative_path).is_file());
    assert!(
        folder
            .join(&file.relative_path)
            .with_extension("it.srt")
            .is_file()
    );
    let audit = client.check_library().await.unwrap();
    assert_eq!(audit.movies_checked, 1);
    assert!(audit.issues.is_empty());
    client.delete_movie_file(1, file.id).await.unwrap();
    let recycled = client.recycle_bin().await.unwrap();
    let token = recycled
        .files
        .iter()
        .find(|f| f.name.ends_with(".mkv"))
        .unwrap()
        .token
        .clone();
    client
        .restore_recycled_movie(&MovieRecycleRestoreRequest {
            token: token.clone(),
            movie_id: 1,
        })
        .await
        .unwrap();
    assert!(client.movie_detail(1).await.unwrap().file.is_some());
    assert!(
        folder
            .join(&file.relative_path)
            .with_extension("it.srt")
            .is_file()
    );
    assert!(
        client
            .restore_recycled_movie(&MovieRecycleRestoreRequest { token, movie_id: 1 })
            .await
            .is_err()
    );
    std::fs::remove_file(folder.join(&file.relative_path)).unwrap();
    let audit = client.check_library().await.unwrap();
    assert_eq!(audit.issues[0].movie_id, Some(1));
    assert_eq!(audit.issues[0].series_id, 0);
    assert_eq!(audit.issues[0].problem, "Missing on disk");
    let schema: serde_json::Value = client.get("/openapi.json").await.unwrap();
    assert!(schema["paths"]["/api/v1/manualimport/movies"]["post"].is_object());
    assert!(schema["paths"]["/api/v1/librarytools/recycle/restore-movie"]["post"].is_object());
}

#[tokio::test]
async fn movies_can_be_marked_watched_individually_or_in_bulk_and_filtered() {
    let server = start().await;
    let client = signed_in(&server).await;
    for id in 1..=2 {
        sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (?,?,'film','film',?,'tmdbMovie',?,1,'/m',?,'2026-11-15T00:00:00Z')")
            .bind(id).bind(format!("Film {id}")).bind(format!("film-{id}")).bind(id.to_string()).bind(format!("/m/film-{id}")).execute(server.state.db.pool()).await.unwrap();
    }
    assert_eq!(
        reqwest::Client::new()
            .put(format!("{}/api/v1/movies/1/watched", server.base))
            .json(&serde_json::json!({"watched":true}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let before = client.movie_detail(1).await.unwrap();
    assert!(!before.item.watched);
    let changed = client.set_movie_watched(1, true).await.unwrap();
    assert!(changed.item.watched);
    assert!(changed.item.watched_at.is_some());
    assert_eq!(before.item.monitored, changed.item.monitored);
    assert_eq!(
        client
            .set_movie_watched(1, true)
            .await
            .unwrap()
            .item
            .watched_at,
        changed.item.watched_at
    );
    assert_eq!(
        client
            .set_movies_watched(&[1, 1, 2, 999], true)
            .await
            .unwrap()
            .updated,
        2
    );
    let watched = client
        .movies(&LibraryQuery {
            watched: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(watched.total_records, 2);
    assert!(client.set_movies_watched(&[], false).await.is_err());
    assert!(client.set_movies_watched(&[0], false).await.is_err());
    assert!(
        client
            .set_movies_watched(&vec![1; 10_001], false)
            .await
            .is_err()
    );
    assert!(client.set_movie_watched(9999, true).await.is_err());
    client.set_movies_watched(&[1, 2], false).await.unwrap();
    assert_eq!(
        client
            .movies(&LibraryQuery {
                watched: Some(true),
                ..Default::default()
            })
            .await
            .unwrap()
            .total_records,
        0
    );
    let unseen = client
        .movies(&LibraryQuery {
            watched: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(unseen.total_records, 2);
    assert!(unseen.records.iter().all(|m| m.watched_at.is_none()));
    let schema: serde_json::Value = client.get("/openapi.json").await.unwrap();
    assert!(schema["paths"]["/api/v1/movies/{id}/watched"]["put"].is_object());
    assert!(schema["paths"]["/api/v1/movies/watched"]["put"].is_object());
}

/// Native movie lifecycle events keep Sonarr's one-episode webhook shape.
#[tokio::test]
async fn native_movie_notifications_cover_add_import_upgrade_delete_and_missing_files() {
    use kitsunarr_domain::api::library_tools::{MovieImportFile, MovieImportRequest};
    use kitsunarr_domain::api::settings::MediaManagementSettings;
    let router = axum::Router::new().route("/", axum::routing::get(|| async {
        axum::Json(serde_json::json!({"Title":"Film","Type":"movie","imdbID":"tt0000060","Year":"2020","Response":"True"}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            omdb: base,
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    server
        .state
        .metadata
        .set_api_keys(None, Some("mock-key".into()));
    let root = server._dir.path().join("films");
    std::fs::create_dir_all(&root).unwrap();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.display().to_string(),
            media_kind: None,
        })
        .await
        .unwrap();
    kitsunarr_db::settings::set(
        server.state.db.pool(),
        kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
        &MediaManagementSettings {
            enable_media_info: false,
            recycle_bin: Some(server._dir.path().join("bin").display().to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (url, log) = webhook_mock().await;
    let (excluded_url, excluded) = webhook_mock().await;
    for (name, target, triggers, tags) in [
        (
            "Lifecycle",
            url,
            serde_json::json!({"onSeriesAdd":true,"onDownload":true,"onUpgrade":true,"onEpisodeFileDelete":true}),
            "[]",
        ),
        (
            "Wrong tag",
            excluded_url.clone(),
            serde_json::json!({"onSeriesAdd":true,"onDownload":true,"onUpgrade":true,"onEpisodeFileDelete":true}),
            "[999]",
        ),
        (
            "Disabled triggers",
            excluded_url,
            serde_json::json!({}),
            "[]",
        ),
    ] {
        sqlx::query("INSERT INTO notifications (name, implementation, settings, triggers, tags, created_at) VALUES (?, 'Webhook', ?, ?, ?, '2026-11-15')")
            .bind(name).bind(serde_json::json!({"url": target, "method":1}).to_string()).bind(triggers.to_string()).bind(tags).execute(server.state.db.pool()).await.unwrap();
    }
    let added = client
        .add_title(&AddTitleRequest {
            provider: MetadataProvider::Omdb,
            id: "tt0000060".into(),
            root_folder_path: root.display().to_string(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: false,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
            path: None,
        })
        .await
        .unwrap();
    assert_eq!(added.media_type, "movie");
    wait_for(&log, 1).await;
    assert_eq!(log.lock().unwrap()[0]["eventType"], "SeriesAdd");
    assert_eq!(log.lock().unwrap()[0]["series"]["imdbId"], "tt0000060");
    let downloads = server._dir.path().join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let mut request = MovieImportRequest {
        files: vec![MovieImportFile {
            movie_id: added.id,
            path: String::new(),
        }],
        import_mode: "copy".into(),
    };
    for (i, name) in [
        "Film.2020.1080p.WEB-DL.JPN-GRP.mkv",
        "Film.2020.1080p.BluRay.PROPER.JPN-GRP.mkv",
    ]
    .iter()
    .enumerate()
    {
        let path = downloads.join(name);
        std::fs::write(&path, b"feature").unwrap();
        request.files[0].path = path.display().to_string();
        if i == 0 {
            sqlx::query("CREATE TRIGGER reject_import BEFORE INSERT ON history WHEN NEW.event_type = 'downloadFolderImported' BEGIN SELECT RAISE(FAIL,'test failure'); END").execute(server.state.db.pool()).await.unwrap();
            assert!(!client.movie_import(&request).await.unwrap()[0].imported);
            assert!(client.movie_detail(added.id).await.unwrap().file.is_none());
            assert!(path.is_file());
            sqlx::query("DROP TRIGGER reject_import")
                .execute(server.state.db.pool())
                .await
                .unwrap();
        }
        let result = client.movie_import(&request).await.unwrap();
        assert!(result[0].imported, "{:?}", result[0].messages);
        wait_for(&log, i + 2).await;
        let event = log.lock().unwrap()[i + 1].clone();
        assert_eq!(event["eventType"], "Download");
        assert_eq!(event["isUpgrade"], i > 0);
        assert_eq!(event["episodes"][0]["seasonNumber"], 1);
        assert_eq!(event["episodes"][0]["episodeNumber"], 1);
        assert_eq!(event["episodeFile"]["languages"][0]["name"], "Japanese");
        assert_eq!(
            event["episodeFile"]["sourcePath"],
            path.display().to_string()
        );
        assert_eq!(
            event["episodeFile"]["qualityVersion"],
            if i == 0 { 1 } else { 2 }
        );
        assert!(event["downloadClient"].is_null());
        assert!(event["downloadId"].is_null());
        if i > 0 {
            let old = &event["deletedFiles"][0];
            assert_eq!(old["quality"], "WEBDL-1080p");
            assert!(std::path::Path::new(old["recycleBinPath"].as_str().unwrap()).is_file());
        }
    }
    // A rejected repeat and a failing database write must produce no success notification.
    assert!(!client.movie_import(&request).await.unwrap()[0].imported);
    let d = client.movie_detail(added.id).await.unwrap();
    let file = d.file.unwrap();
    sqlx::query("CREATE TRIGGER reject_movie_history BEFORE INSERT ON history WHEN NEW.event_type = 'movieFileDeleted' BEGIN SELECT RAISE(FAIL,'test failure'); END").execute(server.state.db.pool()).await.unwrap();
    assert!(client.delete_movie_file(added.id, file.id).await.is_err());
    assert!(
        std::path::Path::new(&d.item.path)
            .join(&file.relative_path)
            .is_file()
    );
    sqlx::query("DROP TRIGGER reject_movie_history")
        .execute(server.state.db.pool())
        .await
        .unwrap();
    client.delete_movie_file(added.id, file.id).await.unwrap();
    wait_for(&log, 4).await;
    let deleted = log.lock().unwrap()[3].clone();
    assert_eq!(deleted["deleteReason"], "Manual");
    assert_eq!(deleted["episodeFile"]["id"], file.id);
    assert!(
        std::path::Path::new(deleted["episodeFile"]["recycleBinPath"].as_str().unwrap()).is_file()
    );
    let folder = std::path::Path::new(&d.item.path);
    std::fs::write(folder.join("Film.2020.1080p.WEB-DL.mkv"), b"feature").unwrap();
    client.rescan_movie(added.id).await.unwrap();
    std::fs::remove_file(folder.join("Film.2020.1080p.WEB-DL.mkv")).unwrap();
    sqlx::query("CREATE TRIGGER reject_missing BEFORE INSERT ON history WHEN NEW.event_type = 'movieFileDeleted' BEGIN SELECT RAISE(FAIL,'test failure'); END").execute(server.state.db.pool()).await.unwrap();
    assert!(client.rescan_movie(added.id).await.is_err());
    assert!(client.movie_detail(added.id).await.unwrap().file.is_some());
    sqlx::query("DROP TRIGGER reject_missing")
        .execute(server.state.db.pool())
        .await
        .unwrap();
    client.rescan_movie(added.id).await.unwrap();
    wait_for(&log, 5).await;
    assert_eq!(log.lock().unwrap()[4]["deleteReason"], "MissingFromDisk");
    assert!(
        log.lock().unwrap()[4]["episodeFile"]
            .get("recycleBinPath")
            .is_none()
    );
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(log.lock().unwrap().len(), 5);
    assert!(excluded.lock().unwrap().is_empty());
    let history = client
        .history_filtered(
            1,
            50,
            &kitsunarr_domain::api::library::HistoryFilter {
                movie_id: Some(added.id),
                event_type: Some("downloadFolderImported".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(history.total_records, 2);
    assert_eq!(
        history.records[0].quality_name.as_deref(),
        Some("Bluray-1080p")
    );
}

#[tokio::test]
async fn history_filters_are_combined_paged_validated_and_documented() {
    use kitsunarr_domain::api::library::HistoryFilter;
    let server = start().await;
    let client = signed_in(&server).await;
    let pool = server.state.db.pool();
    for id in 1..=2 {
        sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (?,?,'film','film',?,'tmdbMovie',?,1,'/films',?,'2026-11-15')")
            .bind(id).bind(format!("Film {id}")).bind(format!("film-{id}")).bind(id.to_string()).bind(format!("/films/{id}")).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO series (id,media_kind,title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (?,'series',?,'show','show',?,'tvmaze',?,1,'/tv',?,'2026-11-15')")
            .bind(id).bind(format!("Show {id}")).bind(format!("show-{id}")).bind(id.to_string()).bind(format!("/tv/{id}")).execute(pool).await.unwrap();
    }
    for (id, parent, ep) in [(1, 1, 1), (2, 1, 2), (3, 2, 1)] {
        sqlx::query(
            "INSERT INTO episodes (id,series_id,season_number,episode_number) VALUES (?,?,1,?)",
        )
        .bind(id)
        .bind(parent)
        .bind(ep)
        .execute(pool)
        .await
        .unwrap();
    }
    for (id, movie, series, episode, event, download) in [
        (1, Some(1), None, None, "grabbed", "Hash1"),
        (2, Some(1), None, None, "downloadFolderImported", "HASH1"),
        (3, Some(2), None, None, "grabbed", "hash2"),
        (4, None, Some(1), Some(1), "grabbed", "Hash1"),
        (5, None, Some(1), Some(2), "downloadFolderImported", "hash2"),
        (6, None, Some(2), Some(3), "grabbed", "Hash3"),
        (7, None, None, None, "downloadFailed", "quote'_%"),
    ] {
        sqlx::query("INSERT INTO history (id,event_type,date,source_title,movie_id,series_id,episode_id,download_id) VALUES (?,?,'2026-11-15T00:00:00Z',?,?,?,?,?)")
            .bind(id).bind(event).bind(format!("Release {id}")).bind(movie).bind(series).bind(episode).bind(download).execute(pool).await.unwrap();
    }
    let all = client.history(1, 50, None).await.unwrap();
    assert_eq!(all.total_records, 7);
    assert_eq!(
        all.records.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![7, 6, 5, 4, 3, 2, 1]
    );
    let filter = HistoryFilter {
        movie_id: Some(1),
        ..Default::default()
    };
    let first = client.history_filtered(1, 1, &filter).await.unwrap();
    let second = client.history_filtered(2, 1, &filter).await.unwrap();
    assert_eq!((first.total_records, second.total_records), (2, 2));
    assert_eq!((first.records[0].id, second.records[0].id), (2, 1));
    assert_eq!(first.records[0].movie_title.as_deref(), Some("Film 1"));
    for (filter, expected) in [
        (
            HistoryFilter {
                download_id: Some("hash1".into()),
                ..Default::default()
            },
            vec![4, 2, 1],
        ),
        (
            HistoryFilter {
                movie_id: Some(1),
                event_type: Some("grabbed".into()),
                download_id: Some("hash1".into()),
                ..Default::default()
            },
            vec![1],
        ),
        (
            HistoryFilter {
                series_id: Some(1),
                episode_id: Some(2),
                ..Default::default()
            },
            vec![5],
        ),
        (
            HistoryFilter {
                episode_id: Some(3),
                ..Default::default()
            },
            vec![6],
        ),
        (
            HistoryFilter {
                series_id: Some(2),
                episode_id: Some(1),
                ..Default::default()
            },
            vec![],
        ),
        (
            HistoryFilter {
                download_id: Some("quote'_%".into()),
                ..Default::default()
            },
            vec![7],
        ),
        (
            HistoryFilter {
                movie_id: Some(999),
                ..Default::default()
            },
            vec![],
        ),
    ] {
        let result = client.history_filtered(1, 50, &filter).await.unwrap();
        assert_eq!(result.total_records as usize, expected.len());
        assert_eq!(
            result.records.iter().map(|r| r.id).collect::<Vec<_>>(),
            expected
        );
    }
    let asc: kitsunarr_domain::Page<kitsunarr_domain::api::library::HistoryItem> = client
        .get("/history?movieId=1&sortDirection=asc")
        .await
        .unwrap();
    assert_eq!(
        asc.records.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    for q in [
        "movieId=0",
        "seriesId=-1",
        "episodeId=0",
        "movieId=1&seriesId=1",
        "movieId=1&episodeId=1",
    ] {
        let error = client
            .get::<serde_json::Value>(&format!("/history?{q}"))
            .await
            .unwrap_err();
        assert_eq!(status(error), 422, "{q}");
    }
    assert_eq!(
        status(
            client
                .history_filtered(
                    1,
                    50,
                    &HistoryFilter {
                        download_id: Some("x".repeat(257)),
                        ..Default::default()
                    }
                )
                .await
                .unwrap_err()
        ),
        422
    );
    let public = reqwest::get(format!("{}/api/v1/history?movieId=1", server.base))
        .await
        .unwrap();
    assert_eq!(public.status(), 401);
    let spec: serde_json::Value = client.get("/openapi.json").await.unwrap();
    let params = spec["paths"]["/api/v1/history"]["get"]["parameters"]
        .as_array()
        .unwrap();
    for name in ["movieId", "seriesId", "episodeId", "downloadId"] {
        assert!(params.iter().any(|p| p["name"] == name), "{name}");
    }
}

#[tokio::test]
async fn direct_nyaa_season_searches_survive_other_source_failures_and_can_be_repeated() {
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use kitsunarr_domain::api::search::ReleaseQuery;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU8, Ordering};
    type Log = Arc<std::sync::Mutex<Vec<HashMap<String, String>>>>;
    let mode = Arc::new(AtomicU8::new(0));
    let log: Log = Arc::default();
    let router = axum::Router::new()
        .route(
            "/",
            axum::routing::get(
                |State((mode, log)): State<(Arc<AtomicU8>, Log)>,
                 Query(query): Query<HashMap<String, String>>| async move {
                    log.lock().unwrap().push(query);
                    if mode.load(Ordering::SeqCst) == 3 {
                        std::future::pending::<()>().await;
                    }
                    if mode.load(Ordering::SeqCst) == 2 {
                        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    let body = if mode.load(Ordering::SeqCst) == 1 {
                        "<rss><channel/></rss>".into()
                    } else {
                        std::fs::read_to_string(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/../../tests/fixtures/indexers/sonarr/nyaa_2021.xml"
                        ))
                        .unwrap()
                    };
                    ([("content-type", "application/rss+xml")], body).into_response()
                },
            ),
        )
        .route(
            "/api",
            axum::routing::get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        )
        .with_state((mode.clone(), log.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let server = start().await;
    let client = signed_in(&server).await;
    let pool = server.state.db.pool();
    sqlx::query("INSERT INTO series (id,media_kind,title,sort_title,clean_title,title_slug,series_type,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (1,'anime','Nyaa Show','nyaashow','nyaashow','nyaa-show','anime','myAnimeList','1',1,'/library','/library/show','2026-11-15')").execute(pool).await.unwrap();
    for id in 1..=2 {
        sqlx::query("INSERT INTO episodes (id,series_id,season_number,episode_number,absolute_episode_number) VALUES (?,1,1,?,?)").bind(id).bind(id).bind(id).execute(pool).await.unwrap();
    }
    for (name, implementation) in [("Direct Nyaa", "Nyaa"), ("Unavailable Prowlarr", "Torznab")] {
        let _:serde_json::Value=client.post("/providers/indexers",&serde_json::json!({
            "name":name,"implementation":implementation,"enabled":true,
            "options":{"enableInteractiveSearch":true,"enableRss":false,"enableAutomaticSearch":false},
            "settings":{"baseUrl":base,"apiPath":"/api","animeStandardFormatSearch":false,"additionalParameters":"&c=1_0&f=0","categories":[5000],"animeCategories":[5070]}
        })).await.unwrap();
    }
    let q = ReleaseQuery {
        series_id: 1,
        season_number: Some(1),
        episode_id: None,
        complete_series: false,
        season_packs_only: false,
        indexer_ids: None,
        search_session: None,
    };
    let first = client.search_releases(&q).await.unwrap();
    assert_eq!(first.releases.len(), 3);
    assert_eq!(first.successful_indexers, 1);
    assert_eq!(first.failures.len(), 1);
    assert_eq!(first.failures[0].indexer, "Unavailable Prowlarr");
    assert!(first.releases.iter().all(|r| r.indexer == "Direct Nyaa"));
    let request = log.lock().unwrap()[0].clone();
    assert_eq!(request.get("term").map(String::as_str), Some("Nyaa Show"));
    assert_eq!(request.get("page").map(String::as_str), Some("rss"));
    assert_eq!(request.get("c").map(String::as_str), Some("1_0"));
    // Sonarr's `SearchAnimeSeason`: the season, then each episode by its absolute number.
    let mut terms: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| r.get("term").cloned())
        .collect();
    terms.sort();
    assert_eq!(
        terms,
        [
            "Nyaa Show",
            "Nyaa Show 01",
            "Nyaa Show 02",
            "Nyaa Show 1",
            "Nyaa Show 2"
        ]
    );
    let per_search = terms.len();
    assert_eq!(client.search_releases(&q).await.unwrap().releases.len(), 3);
    assert_eq!(
        log.lock().unwrap().len(),
        2 * per_search,
        "repeat searches actually query Nyaa again"
    );
    let before = log.lock().unwrap().len();
    assert_eq!(
        client.automatic_search(&q).await.unwrap().searched_indexers,
        0
    );
    assert_eq!(
        log.lock().unwrap().len(),
        before,
        "interactive-only Nyaa is not used automatically"
    );
    let providers = client.providers("indexers").await.unwrap();
    let nyaa = providers
        .iter()
        .find(|p| p.name == "Direct Nyaa")
        .unwrap()
        .id;
    let unavailable = providers
        .iter()
        .find(|p| p.name == "Unavailable Prowlarr")
        .unwrap()
        .id;
    let mut selected = q.clone();
    selected.indexer_ids = Some(nyaa.to_string());
    let only = client.search_releases(&selected).await.unwrap();
    assert_eq!((only.searched_indexers, only.successful_indexers), (1, 1));
    assert!(only.failures.is_empty());
    assert_eq!(only.diagnostics.len(), 1);
    assert_eq!(only.diagnostics[0].indexer_id, nyaa);
    assert_eq!(only.diagnostics[0].status, "success");
    assert!(
        only.diagnostics[0]
            .planned_queries
            .iter()
            .any(|p| p.contains("Nyaa Show"))
    );
    assert!(only.considered_titles.contains(&"Nyaa Show".into()));
    let before = log.lock().unwrap().len();
    selected.indexer_ids = Some(String::new());
    let none = client.search_releases(&selected).await.unwrap();
    assert_eq!(none.searched_indexers, 0);
    assert!(none.failures.is_empty() && none.diagnostics.is_empty());
    assert_eq!(log.lock().unwrap().len(), before);
    for invalid in ["-1".into(), "unknown".into(), "999999".into()] {
        selected.indexer_ids = Some(invalid);
        assert_eq!(
            status(client.search_releases(&selected).await.unwrap_err()),
            422
        );
    }
    // Selection and diagnostics also follow TV numbering and daily air dates.
    selected.indexer_ids = Some(nyaa.to_string());
    selected.season_number = None;
    selected.episode_id = Some(1);
    sqlx::query("INSERT INTO series_alternate_titles (series_id,title,clean_title,season_number,source) VALUES (1,'Season One Alias','seasononealias',1,'user'),(1,'Other Season Alias','otherseasonalias',2,'user')").execute(pool).await.unwrap();
    for (kind, needle) in [("standard", "s01e01"), ("daily", "2026.11.14")] {
        sqlx::query("UPDATE series SET series_type=? WHERE id=1")
            .bind(kind)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("UPDATE episodes SET air_date='2026-11-14',air_date_utc='2026-11-14T00:00:00Z' WHERE id=1")
            .execute(pool)
            .await
            .unwrap();
        let result = client.search_releases(&selected).await.unwrap();
        assert_eq!(result.successful_indexers, 1);
        assert!(
            result.diagnostics[0]
                .planned_queries
                .iter()
                .any(|p| p.contains(needle)),
            "{:?}",
            result.diagnostics
        );
        assert!(
            result
                .considered_titles
                .contains(&"Season One Alias".into())
        );
        assert!(
            !result
                .considered_titles
                .contains(&"Other Season Alias".into())
        );
    }
    sqlx::query("UPDATE series SET series_type='anime' WHERE id=1")
        .execute(pool)
        .await
        .unwrap();
    // A newer request with the same session drops the outstanding server search.
    mode.store(3, Ordering::SeqCst);
    let mut slow = q.clone();
    slow.indexer_ids = Some(nyaa.to_string());
    slow.search_session = Some("http-superseded".into());
    let before = log.lock().unwrap().len();
    let c = client.clone();
    let old = tokio::spawn(async move { c.search_releases(&slow).await });
    for _ in 0..100 {
        if log.lock().unwrap().len() > before {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(log.lock().unwrap().len() > before, "old request started");
    let mut replacement = q.clone();
    replacement.indexer_ids = Some(String::new());
    replacement.search_session = Some("http-superseded".into());
    assert_eq!(
        client
            .search_releases(&replacement)
            .await
            .unwrap()
            .searched_indexers,
        0
    );
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), old)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(status(error), 409);
    replacement.search_session = Some("bad session".into());
    assert_eq!(
        status(client.search_releases(&replacement).await.unwrap_err()),
        422
    );
    // Explicitly selecting a source disabled for interactive search is rejected.
    sqlx::query("UPDATE indexers SET enable_interactive_search=0 WHERE id=?")
        .bind(unavailable)
        .execute(pool)
        .await
        .unwrap();
    selected = q.clone();
    selected.indexer_ids = Some(unavailable.to_string());
    assert_eq!(
        status(client.search_releases(&selected).await.unwrap_err()),
        422
    );
    sqlx::query("UPDATE indexers SET enable_interactive_search=1 WHERE id=?")
        .bind(unavailable)
        .execute(pool)
        .await
        .unwrap();
    // A valid empty source with another failing source is a partial response.
    mode.store(1, Ordering::SeqCst);
    let partial = client.search_releases(&q).await.unwrap();
    assert!(partial.releases.is_empty());
    assert_eq!(partial.successful_indexers, 1);
    assert_eq!(partial.failures.len(), 1);
    sqlx::query("UPDATE indexers SET enable_interactive_search=0 WHERE implementation='Torznab'")
        .execute(pool)
        .await
        .unwrap();
    let empty = client.search_releases(&q).await.unwrap();
    assert!(empty.releases.is_empty() && empty.failures.is_empty());
    assert_eq!(empty.successful_indexers, 1);
    mode.store(2, Ordering::SeqCst);
    let failed = client.search_releases(&q).await.unwrap();
    assert!(failed.releases.is_empty());
    assert_eq!(failed.successful_indexers, 0);
    assert_eq!(failed.failures.len(), 1);
    let mut legacy = serde_json::to_value(&first).unwrap();
    legacy.as_object_mut().unwrap().remove("successfulIndexers");
    let legacy: kitsunarr_domain::api::search::ReleaseSearchResult =
        serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.successful_indexers, 0);
    // Field is part of the public schema and old captured responses still deserialize.
    let spec: serde_json::Value = client.get("/openapi.json").await.unwrap();
    assert!(
        spec["components"]["schemas"]["ReleaseSearchResult"]["properties"]["successfulIndexers"]
            .is_object()
    );
}

/// Real Prowlarr's SonarrV3Proxy.TestConnection requires this response header on the probe.
#[tokio::test]
async fn sonarr_v3_indexer_probe_exposes_compatible_version_header() {
    let server = start().await;
    let (source, _) = newznab_mock().await;
    let http = reqwest::Client::new();
    let probe = serde_json::json!({"name":"Prowlarr probe","implementation":"Newznab","configContract":"NewznabSettings",
        "fields":[{"name":"baseUrl","value":source},{"name":"apiPath","value":"/api"},{"name":"apiKey","value":"test-source-key"},{"name":"categories","value":[5000]}]});
    let path = format!("{}/api/v3/indexer/test", server.base);
    let response = http
        .post(&path)
        .header("X-Api-Key", &server.api_key)
        .json(&probe)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("X-Application-Version").unwrap(),
        "4.0.0.0"
    );
    let mut failed = probe.clone();
    failed["fields"][0]["value"] = "http://127.0.0.1:9".into();
    let response = http
        .post(&path)
        .header("X-Api-Key", &server.api_key)
        .json(&failed)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.headers().get("X-Application-Version").unwrap(),
        "4.0.0.0"
    );
    let unauthorized = http.post(&path).json(&probe).send().await.unwrap();
    assert_eq!(unauthorized.status(), 401);
    assert_eq!(
        unauthorized.headers().get("X-Application-Version").unwrap(),
        "4.0.0.0"
    );
    let native = http
        .get(format!("{}/api/v1/system/status", server.base))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(native.status(), 200);
    assert!(!native.headers().contains_key("X-Application-Version"));
    assert_eq!(
        native.json::<serde_json::Value>().await.unwrap()["appName"],
        "Kitsunarr"
    );
}

/// Real HTTP persistence: Tanukarr's profile fields survive both native and v3 APIs.
#[tokio::test]
async fn quality_profile_validation_and_tanukarr_fields_round_trip() {
    use serde_json::{Value, json};
    let server = start().await;
    let http = reqwest::Client::new();
    let request = |method, path: &str| {
        http.request(method, format!("{}{path}", server.base))
            .header("X-Api-Key", &server.api_key)
    };
    use reqwest::Method as M;
    let mut native: Value = request(M::GET, "/api/v1/qualityprofiles")
        .send()
        .await
        .unwrap()
        .json::<Vec<Value>>()
        .await
        .unwrap()
        .remove(0);
    native["id"] = json!(0);
    native["name"] = json!("Quality verification");
    let response = request(M::POST, "/api/v1/qualityprofiles")
        .json(&native)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let original: Value = response.json().await.unwrap();
    let id = original["id"].as_i64().unwrap();
    let native_path = format!("/api/v1/qualityprofiles/{id}");
    let v3_path = format!("/api/v3/qualityprofile/{id}");
    let mut profile: Value = request(M::GET, &v3_path)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let group = profile["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["id"] == 1002)
        .unwrap();
    let leaf = group["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["quality"]["id"] == 3)
        .unwrap();
    leaf["minSize"] = json!(10.0);
    leaf["preferredSize"] = json!(20.0);
    leaf["maxSize"] = json!(30.0);
    profile["codecMultipliers"] = json!({"HEVC": 1.7});
    profile["minUpgradeFormatScore"] = json!(75);
    let response = request(M::PUT, &v3_path)
        .json(&profile)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let saved: Value = response.json().await.unwrap();
    assert_eq!(saved["codecMultipliers"]["HEVC"], 1.7);
    let stored: Value = request(M::GET, &native_path)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        stored["sizeLimits"],
        json!([{"qualityId":3,"minSize":10.0,"preferredSize":20.0,"maxSize":30.0}])
    );
    assert_eq!(stored["minUpgradeFormatScore"], 75);
    assert_eq!(stored["items"], original["items"]);

    // Partial updates preserve per-quality sizes and codec overrides when omitted.
    let response = request(M::PUT, &v3_path)
        .json(&json!({"name":"Renamed profile"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let mut saved: Value = response.json().await.unwrap();
    assert_eq!(saved["codecMultipliers"]["HEVC"], 1.7);
    let group = saved["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["id"] == 1002)
        .unwrap();
    let leaf = group["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["quality"]["id"] == 3)
        .unwrap();
    assert_eq!(leaf["maxSize"], 30.0);
    leaf["maxSize"] = Value::Null; // explicit null is unlimited, not global fallback.
    let response = request(M::PUT, &v3_path).json(&saved).send().await.unwrap();
    assert_eq!(response.status(), 202);
    let valid: Value = request(M::GET, &native_path)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(valid["sizeLimits"][0]["maxSize"], Value::Null);
    assert_eq!(valid["sizeLimits"][0]["minSize"], 10.0);

    // Rejected writes must leave the previously saved profile intact.
    for (field, value) in [
        ("minFormatScore", json!(500)),
        ("minUpgradeFormatScore", json!(0)),
        ("formatItems", json!([{"formatId":999999,"score":100}])),
        ("codecMultipliers", json!({"HEVC":0.0})),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        let response = request(M::PUT, &native_path)
            .json(&invalid)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{field}");
        let unchanged: Value = request(M::GET, &native_path)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(unchanged, valid);
    }
    let response = request(M::PUT, &v3_path)
        .json(&json!({"minFormatScore":500}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn regex_custom_formats_rename_movies_through_http() {
    use serde_json::json;
    let server = start().await;
    let client = signed_in(&server).await;
    let root = server._dir.path().join("rename-proof");
    let folder = root.join("Proof Film");
    std::fs::create_dir_all(&folder).unwrap();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.display().to_string(),
            media_kind: None,
        })
        .await
        .unwrap();
    sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,year,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (1,'Proof Film','proof film','prooffilm','proof-film',2020,'tmdbMovie','1',1,?,?,'2026-11-15T00:00:00Z')")
        .bind(root.to_str().unwrap()).bind(folder.to_str().unwrap()).execute(server.state.db.pool()).await.unwrap();
    let source = folder.join("Proof.Film.2020.WEB-DL.1080p-GRP.mkv");
    std::fs::write(&source, b"video proof").unwrap();
    std::fs::write(source.with_extension("it.srt"), b"subtitle proof").unwrap();
    std::fs::write(folder.join("unrelated.srt"), b"unrelated").unwrap();
    let http = reqwest::Client::new();
    let post_format = |pattern: &str| {
        http.post(format!("{}/api/v1/customformats", server.base))
        .header("X-Api-Key", &server.api_key).json(&json!({"id":0,"name":"WEB","includeWhenRenaming":true,"specifications":[{
            "name":"WEB","implementation":"ReleaseTitleSpecification","required":true,"negate":false,
            "fields":{"value":pattern}}]})).send()
    };
    assert_eq!(post_format("(unclosed").await.unwrap().status(), 400);
    assert_eq!(
        post_format(r"(?<=\.)WEB-DL(?=\.)").await.unwrap().status(),
        201
    );
    let mut naming = client.naming().await.unwrap();
    assert!(
        !naming.rename_episodes,
        "manual renames also work when automatic renaming is off"
    );
    naming.movie_file_format =
        "{SERIES TITLE}...({Series Year}){[Custom Formats]}-{Release Group}".into();
    client.put_naming(&naming).await.unwrap(); // literal ellipses are supported by Tanukarr.
    let mut invalid = naming.clone();
    invalid.movie_file_format = "../{Movie Title}".into();
    assert_eq!(status(client.put_naming(&invalid).await.unwrap_err()), 422);
    assert_eq!(client.naming().await.unwrap(), naming);
    client.rescan_movie(1).await.unwrap();
    let preview = client.movie_rename_preview(1).await.unwrap().unwrap();
    assert_eq!(preview.new_path, "PROOF FILM...(2020)[WEB]-GRP.mkv");
    assert!(source.is_file());
    assert!(!folder.join(&preview.new_path).exists());
    client.rename_movie_file(1, &preview).await.unwrap();
    let target = folder.join(&preview.new_path);
    assert_eq!(std::fs::read(&target).unwrap(), b"video proof");
    assert_eq!(
        std::fs::read(target.with_extension("it.srt")).unwrap(),
        b"subtitle proof"
    );
    assert_eq!(
        std::fs::read(folder.join("unrelated.srt")).unwrap(),
        b"unrelated"
    );
    assert!(!source.exists());
    assert!(client.movie_rename_preview(1).await.unwrap().is_none());
    let detail = client.movie_detail(1).await.unwrap();
    assert_eq!(detail.file.unwrap().relative_path, preview.new_path);
}

#[tokio::test]
async fn quality_group_edits_persist_through_native_and_v3_profiles() {
    use kitsunarr_domain::api::settings::{OrderedF64, QualitySizeLimit};
    use kitsunarr_domain::quality_profile::{
        QualityDestination as To, QualityEdit as Edit, QualityNode as Node,
    };
    use serde_json::Value;
    let server = start().await;
    let client = signed_in(&server).await;
    let mut p = client.quality_profiles().await.unwrap().remove(0);
    p.id = 0;
    p.name = "Editable groups".into();
    p.cutoff = 7;
    p.size_limits = vec![QualitySizeLimit {
        quality_id: 7,
        min_size: Some(OrderedF64(1.0)),
        preferred_size: Some(OrderedF64(2.0)),
        max_size: Some(OrderedF64(3.0)),
    }];
    p.codec_multipliers.insert("HEVC".into(), OrderedF64(1.8));
    p.edit_qualities(Edit::CreateGroup(7)).unwrap();
    let group_id = p.cutoff;
    assert_eq!(
        status(client.create_quality_profile(&p).await.unwrap_err()),
        400,
        "one-member drafts cannot be persisted"
    );
    p.edit_qualities(Edit::RenameGroup(group_id, "HD equals".into()))
        .unwrap();
    p.edit_qualities(Edit::Move(Node::Quality(3), To::IntoGroup(group_id)))
        .unwrap();
    p.edit_qualities(Edit::Step(Node::Quality(7), true))
        .unwrap();
    p.edit_qualities(Edit::Step(Node::Group(group_id), true))
        .unwrap();
    let mut saved = client.create_quality_profile(&p).await.unwrap();
    p.id = saved.id;
    assert_eq!(saved, p);
    let http = reqwest::Client::new();
    let v3_path = format!("{}/api/v3/qualityprofile/{}", server.base, saved.id);
    let mut v3: Value = http
        .get(&v3_path)
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v3["cutoff"], group_id);
    let group = v3["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["id"] == group_id)
        .unwrap();
    assert_eq!(
        group["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["quality"]["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [3, 7]
    );
    assert_eq!(group["items"][1]["maxSize"], 3.0);
    group["name"] = "Renamed via v3".into();
    let response = http
        .put(&v3_path)
        .header("X-Api-Key", &server.api_key)
        .json(&v3)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    saved = client
        .quality_profiles()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == saved.id)
        .unwrap();
    assert_eq!(
        saved
            .items
            .iter()
            .find_map(|i| i.group.as_ref().filter(|g| g.id == group_id))
            .unwrap()
            .name,
        "Renamed via v3"
    );
    saved.edit_qualities(Edit::Ungroup(group_id)).unwrap();
    assert_eq!(saved.cutoff, 7);
    let ungrouped = client.update_quality_profile(&saved).await.unwrap();
    assert_eq!(ungrouped, saved);
    assert_eq!(ungrouped.size_limits, p.size_limits);
    assert_eq!(ungrouped.codec_multipliers, p.codec_multipliers);
}

/// The guided editor tests unsaved formats through the exact evaluator used by search/naming.
#[tokio::test]
async fn custom_format_preview_covers_conditions_anime_and_invalid_drafts_without_saving() {
    use kitsunarr_domain::api::settings::CustomFormatTestRequest;
    let server = start().await;
    let client = Client::new(&server.base).with_api_key(&server.api_key);
    let before = client.custom_formats().await.unwrap();
    let mut request: CustomFormatTestRequest = serde_json::from_value(serde_json::json!({
        "format": {"id": 0, "name": "Preview", "includeWhenRenaming": true,
            "specifications": [
                {"name":"Title", "implementation":"ReleaseTitleSpecification", "negate":false, "required":true, "fields":{"value":"(?<=Example\\.)Show"}},
                {"name":"Group", "implementation":"ReleaseGroupSpecification", "negate":false, "required":false, "fields":{"value":"^GRP$"}},
                {"name":"Language", "implementation":"LanguageSpecification", "negate":false, "required":false, "fields":{"value":-2}},
                {"name":"Source", "implementation":"SourceSpecification", "negate":false, "required":false, "fields":{"value":3}},
                {"name":"Resolution", "implementation":"ResolutionSpecification", "negate":false, "required":false, "fields":{"value":1080}},
                {"name":"Size", "implementation":"SizeSpecification", "negate":false, "required":false, "fields":{"min":4,"max":5}},
                {"name":"Flags", "implementation":"IndexerFlagSpecification", "negate":false, "required":false, "fields":{"value":17}},
                {"name":"Pack", "implementation":"ReleaseTypeSpecification", "negate":false, "required":false, "fields":{"value":3}}
            ]},
        "releaseTitle":"Example.Show.S01.1080p.WEB-DL.JAPANESE-GRP",
        "size": 5368709120i64, "indexerFlags":17, "originalLanguage":8
    })).unwrap();
    let result = client.test_custom_format(&request).await.unwrap();
    assert!(result.matches, "{result:?}");
    assert!(result.warnings.is_empty());
    assert_eq!(result.specifications.len(), 8);
    assert!(result.specifications.iter().all(|s| s.matches));
    assert_eq!(result.quality, "WEBDL-1080p");
    assert_eq!(result.release_group.as_deref(), Some("GRP"));
    assert_eq!(result.release_type, "Season pack");

    // A single failed required condition blocks the format, even alongside an alternative.
    let valid_title = request.format.specifications[0].clone();
    request.format.specifications[0].set_field("value", serde_json::json!("[unclosed"));
    request.format.specifications[0].negate = true;
    request.format.specifications.push(valid_title);
    let result = client.test_custom_format(&request).await.unwrap();
    assert!(!result.matches);
    assert!(!result.specifications[0].matches);
    assert_eq!(result.warnings.len(), 1);
    request.format.specifications[0].required = false;
    assert!(client.test_custom_format(&request).await.unwrap().matches);

    // Test file names and native anime subgroup names without hitting a real indexer.
    request.format.specifications.truncate(1);
    request.format.specifications[0].implementation = "ReleaseGroupSpecification".into();
    request.format.specifications[0].set_field("value", serde_json::json!("^SubsPlease$"));
    request.format.specifications[0].negate = false;
    request.release_title = "[SubsPlease] Nyaa Test Show - 01 (1080p) [ABCDEF12]".into();
    let anime = client.test_custom_format(&request).await.unwrap();
    assert!(anime.matches, "{anime:?}");
    assert_eq!(anime.release_group.as_deref(), Some("SubsPlease"));
    assert_eq!(anime.release_type, "Single episode");
    assert_eq!(anime.languages, ["Japanese"]);
    request.format.specifications[0].implementation = "LanguageSpecification".into();
    request.format.specifications[0].set_field("value", serde_json::json!(-2));
    request.release_title = "Example.Show.S01E01.German.DL.1080p.WEB-DL-GRP".into();
    let dual = client.test_custom_format(&request).await.unwrap();
    assert!(dual.matches, "{dual:?}");
    assert_eq!(dual.languages, ["German", "Japanese"]);
    request.format.specifications[0].implementation = "ReleaseTitleSpecification".into();
    request.format.specifications[0].set_field("value", serde_json::json!("FileOnly"));
    assert!(!client.test_custom_format(&request).await.unwrap().matches);
    request.file_name = Some("FileOnly.mkv".into());
    assert!(client.test_custom_format(&request).await.unwrap().matches);
    request.file_name = None;
    request.release_title = "Example.Movie.2024.1080p.WEB-DL-GRP".into();
    request.format.specifications[0].set_field("value", serde_json::json!("Movie.*2024"));
    assert!(client.test_custom_format(&request).await.unwrap().matches);
    request.format.specifications[0].implementation = "ReleaseTypeSpecification".into();
    request.format.specifications[0].set_field("value", serde_json::json!(1));
    request.release_type = Some(1);
    let feature = client.test_custom_format(&request).await.unwrap();
    assert!(feature.matches);
    assert_eq!(feature.release_type, "Single episode");
    request.release_type = None;

    // An unsupported specification is preserved and explained, including when negated.
    request.format.specifications[0].implementation = "FutureSpecification".into();
    request.format.specifications[0].negate = true;
    let unknown = client.test_custom_format(&request).await.unwrap();
    assert!(!unknown.matches);
    assert!(unknown.warnings[0].contains("FutureSpecification"));

    // Overrides make quality, language and size tests reproducible, without stored profiles.
    request.format.specifications[0].implementation = "LanguageSpecification".into();
    request.format.specifications[0].set_field("value", serde_json::json!(5));
    request.format.specifications[0].negate = false;
    request.languages = Some(vec![5]);
    request.quality_id = Some(7);
    let result = client.test_custom_format(&request).await.unwrap();
    assert!(result.matches);
    assert_eq!(result.quality, "Bluray-1080p");
    assert_eq!(result.languages, ["Italian"]);
    let http = reqwest::Client::new();
    for invalid in [
        serde_json::json!({"releaseTitle":""}),
        serde_json::json!({"releaseTitle":"x".repeat(2049)}),
        serde_json::json!({"size":-1}),
        serde_json::json!({"qualityId":9999}),
        serde_json::json!({"releaseType":5}),
        serde_json::json!({"languages":[9999]}),
        serde_json::json!({"originalLanguage":-2}),
    ] {
        let mut body = serde_json::to_value(&request).unwrap();
        body.as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        let response = http
            .post(format!("{}/api/v1/customformats/test", server.base))
            .header("X-Api-Key", &server.api_key)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 422, "{body}");
    }
    request.format.specifications.clear();
    assert!(matches!(
        client.test_custom_format(&request).await,
        Err(ClientError::Api { status: 422, .. })
    ));
    request.format.specifications =
        vec![
            kitsunarr_domain::custom_formats::new_specification("SourceSpecification").unwrap();
            33
        ];
    assert!(client.test_custom_format(&request).await.is_err());
    request.format.specifications.truncate(1);
    request.format.specifications[0].set_field("value", serde_json::json!("x".repeat(8193)));
    assert!(client.test_custom_format(&request).await.is_err());
    let unauthorized = http
        .post(format!("{}/api/v1/customformats/test", server.base))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);
    let openapi: serde_json::Value = http
        .get(format!("{}/api/v1/openapi.json", server.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(openapi["paths"]["/api/v1/customformats/test"]["post"].is_object());
    assert_eq!(
        client.custom_formats().await.unwrap(),
        before,
        "testing must never save a draft"
    );
}

/// Sonarr's monitoring choices: on add, from the bulk editor and through `/api/v3/seasonpass`.
#[tokio::test]
async fn monitoring_choices_apply_on_add_bulk_edit_and_season_pass() {
    use serde_json::{Value, json};
    let base = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: base,
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
    // The option names are Sonarr's, including the obsolete `lastSeason`.
    let parsed: AddTitleRequest = serde_json::from_value(json!({
        "provider": "tvMaze", "id": "169", "rootFolderPath": root, "qualityProfileId": 1,
        "monitor": "lastSeason"
    }))
    .unwrap();
    assert_eq!(parsed.monitor, MonitorOption::LatestSeason);
    let added = client
        .add_title(&AddTitleRequest {
            monitor: MonitorOption::Pilot,
            ..parsed
        })
        .await
        .unwrap();
    let episodes = client.episodes(added.id).await.unwrap();
    let monitored: Vec<(i32, i32)> = episodes
        .iter()
        .filter(|e| e.monitored)
        .map(|e| (e.season_number, e.episode_number))
        .collect();
    assert_eq!(monitored, vec![(1, 1)], "the pilot alone");
    let detail = client.series_detail(added.id).await.unwrap();
    assert!(detail.item.monitored);
    assert!(
        detail.seasons.iter().all(|s| !s.monitored),
        "no season is monitored for the pilot alone: {:?}",
        detail.seasons
    );

    // Bulk editor: every episode is missing, then "monitor new items" off.
    let edited = client
        .bulk_edit_series(&BulkEdit {
            ids: vec![added.id],
            monitor: Some(MonitorOption::Missing),
            monitor_new_items: Some(kitsunarr_domain::MonitorNewItems::None),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(edited.updated, 1);
    let episodes = client.episodes(added.id).await.unwrap();
    assert!(
        episodes
            .iter()
            .all(|e| e.monitored == (e.season_number > 0))
    );
    let detail = client.series_detail(added.id).await.unwrap();
    assert_eq!(
        detail.monitor_new_items,
        kitsunarr_domain::MonitorNewItems::None
    );
    assert!(
        detail
            .seasons
            .iter()
            .all(|s| s.monitored == (s.season_number > 0))
    );
    // Films have no episodes to choose from.
    match client
        .bulk_edit_movies(&BulkEdit {
            ids: vec![1],
            monitor: Some(MonitorOption::All),
            ..Default::default()
        })
        .await
        .unwrap_err()
    {
        ClientError::Api { status, body } => {
            assert_eq!(status, 422);
            assert_eq!(body.fields[0].field, "monitor");
        }
        other => panic!("unexpected {other:?}"),
    }

    // Season pass: the first season by Sonarr's number (5 = firstSeason).
    let http = reqwest::Client::new();
    let url = format!("{}/api/v3/seasonpass", server.base);
    let response = http
        .post(&url)
        .header("X-Api-Key", &server.api_key)
        .json(&json!({"series": [{"id": added.id, "monitored": true}], "monitoringOptions": {"monitor": 5}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let episodes = client.episodes(added.id).await.unwrap();
    assert!(
        episodes
            .iter()
            .all(|e| e.monitored == (e.season_number == 1))
    );
    // Seasons alone change only the seasons' flags; "none" also unmonitors the series.
    let response = http
        .post(&url)
        .header("X-Api-Key", &server.api_key)
        .json(&json!({"series": [{"id": added.id, "seasons": [{"seasonNumber": 2, "monitored": true}]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let detail = client.series_detail(added.id).await.unwrap();
    let season = |n: i32| {
        detail
            .seasons
            .iter()
            .find(|s| s.season_number == n)
            .unwrap()
            .monitored
    };
    assert!(season(1) && season(2) && !season(3));
    let response = http
        .post(&url)
        .header("X-Api-Key", &server.api_key)
        .json(&json!({"series": [{"id": added.id}], "monitoringOptions": {"monitor": "none"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let detail = client.series_detail(added.id).await.unwrap();
    assert!(!detail.item.monitored);
    assert!(
        client
            .episodes(added.id)
            .await
            .unwrap()
            .iter()
            .all(|e| !e.monitored)
    );
    // Unknown options and series are refused; nothing changes.
    for (body, expected) in [
        (
            json!({"series": [{"id": added.id}], "monitoringOptions": {"monitor": "everything"}}),
            422,
        ),
        (
            json!({"series": [{"id": 9999}], "monitoringOptions": {"monitor": "all"}}),
            404,
        ),
    ] {
        let response = http
            .post(&url)
            .header("X-Api-Key", &server.api_key)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected, "{body}");
    }
    let v3: Value = http
        .get(format!("{}/api/v3/series/{}", server.base, added.id))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v3["monitored"], false);
}

/// Sonarr's library import: unmapped folders of a root folder, titles added with their
/// existing folders (scanned before "existing"/"missing" monitoring applies), and the v3
/// `rootfolder` and `series/import` routes.
#[tokio::test]
async fn library_import_adds_titles_with_their_existing_folders() {
    use serde_json::{Value, json};
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
    let root = dir.path().display().to_string();
    let created = client
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    let show = dir.path().join("Breaking Bad (2008) [tvdbid-81189]");
    std::fs::create_dir_all(show.join("Season 01")).unwrap();
    for name in [
        "Season 01/Breaking.Bad.S01E01.720p.HDTV.x264-GRP.mkv",
        "Season 01/Breaking.Bad.S01E02.720p.HDTV.x264-GRP.mkv",
    ] {
        std::fs::write(show.join(name), b"video").unwrap();
    }
    std::fs::create_dir_all(dir.path().join("The.Expanse")).unwrap();
    std::fs::create_dir_all(dir.path().join("lost+found")).unwrap();
    std::fs::write(dir.path().join("loose-file.mkv"), b"x").unwrap();

    let unmapped = client.unmapped_folders(created.id).await.unwrap();
    let names: Vec<(&str, &str, Option<i32>)> = unmapped
        .iter()
        .map(|u| (u.name.as_str(), u.search_term.as_str(), u.year))
        .collect();
    assert_eq!(
        names,
        vec![
            (
                "Breaking Bad (2008) [tvdbid-81189]",
                "tvdb:81189",
                Some(2008)
            ),
            ("The.Expanse", "The Expanse", None),
        ]
    );
    assert_eq!(
        unmapped[0].relative_path,
        "Breaking Bad (2008) [tvdbid-81189]"
    );
    assert_eq!(
        status(client.unmapped_folders(9999).await.unwrap_err()),
        404
    );

    let request = |path: Option<String>| AddTitleRequest {
        provider: MetadataProvider::TvMaze,
        id: "169".into(),
        root_folder_path: root.clone(),
        quality_profile_id: 1,
        monitor: MonitorOption::Missing,
        series_type: None,
        season_folder: true,
        tags: vec![],
        search_for_missing: false,
        episode_ordering: None,
        path,
    };
    // Folders outside the root folder, missing ones and `..` are refused.
    for bad in [
        "/etc".to_string(),
        format!("{root}/Nope"),
        format!("{root}/The.Expanse/../Breaking Bad (2008) [tvdbid-81189]"),
        root.clone(),
    ] {
        match client
            .add_title(&request(Some(bad.clone())))
            .await
            .unwrap_err()
        {
            ClientError::Api { status, body } => {
                assert_eq!(status, 422, "{bad}");
                assert_eq!(body.fields[0].field, "path", "{bad}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    let added = client
        .add_title(&request(Some(unmapped[0].path.clone())))
        .await
        .unwrap();
    let detail = client.series_detail(added.id).await.unwrap();
    assert_eq!(detail.item.path, show.display().to_string());
    assert_eq!(
        detail.item.episode_file_count, 2,
        "the folder was scanned in place"
    );
    let episodes = client.episodes(added.id).await.unwrap();
    assert!(
        episodes
            .iter()
            .all(|e| e.monitored == (e.season_number > 0 && e.file.is_none())),
        "missing episodes only"
    );
    assert!(
        show.join("Season 01/Breaking.Bad.S01E01.720p.HDTV.x264-GRP.mkv")
            .exists()
    );
    // The folder is no longer offered, and cannot be used twice.
    let unmapped = client.unmapped_folders(created.id).await.unwrap();
    assert_eq!(unmapped.len(), 1);
    match client
        .add_title(&request(Some(show.display().to_string())))
        .await
        .unwrap_err()
    {
        ClientError::Api { status, .. } => assert!(status == 409 || status == 422),
        other => panic!("unexpected {other:?}"),
    }

    // v3: the root folder lists the remaining folder; an import needs a path.
    let http = reqwest::Client::new();
    let v3 = |p: &str| format!("{}/api/v3{p}", server.base);
    let roots: Value = http
        .get(v3("/rootfolder"))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(roots[0]["unmappedFolders"][0]["name"], "The.Expanse");
    assert_eq!(
        roots[0]["unmappedFolders"][0]["relativePath"],
        "The.Expanse"
    );
    let response = http
        .post(v3("/series/import"))
        .header("X-Api-Key", &server.api_key)
        .json(&json!([{"tvdbId": 81189, "rootFolderPath": root, "qualityProfileId": 1}]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
}

/// `POST /api/v3/series/import` adds TheTVDB series with their existing folders and the
/// monitoring choice by Sonarr's name.
#[tokio::test]
async fn sonarr_v3_series_import_uses_the_existing_folder() {
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
    let folder = dir.path().join("BB");
    std::fs::create_dir_all(folder.join("Season 02")).unwrap();
    std::fs::write(
        folder.join("Season 02/Breaking.Bad.S02E03.720p.HDTV.x264-GRP.mkv"),
        b"video",
    )
    .unwrap();
    let response = reqwest::Client::new()
        .post(format!("{}/api/v3/series/import", server.base))
        .header("X-Api-Key", &server.api_key)
        .json(&json!([{
            "tvdbId": 81189, "rootFolderPath": root, "path": folder.display().to_string(),
            "qualityProfileId": 1, "seasonFolder": true, "monitored": true,
            "addOptions": {"monitor": "existing", "searchForMissingEpisodes": false}
        }]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let added: Value = response.json().await.unwrap();
    assert_eq!(added[0]["path"], folder.display().to_string());
    let id = added[0]["id"].as_i64().unwrap();
    let monitored: Vec<(i32, i32)> = client
        .episodes(id)
        .await
        .unwrap()
        .iter()
        .filter(|e| e.monitored)
        .map(|e| (e.season_number, e.episode_number))
        .collect();
    assert_eq!(monitored, vec![(2, 3)], "the existing episode only");
}

/// Sonarr's series move: a new folder with its contents moved (or only recorded), bulk moves
/// to another root folder, and the v3 path edit and series editor.
#[tokio::test]
async fn series_folders_move_with_their_files() {
    use kitsunarr_domain::api::library::{BulkMoveRequest, MoveTitleRequest};
    use serde_json::{Value, json};
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
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    for root in [&a, &b] {
        std::fs::create_dir_all(root).unwrap();
        client
            .create_root_folder(&RootFolderCreate {
                path: root.display().to_string(),
                media_kind: None,
            })
            .await
            .unwrap();
    }
    let added = client
        .add_title(&AddTitleRequest {
            provider: MetadataProvider::TvMaze,
            id: "169".into(),
            root_folder_path: a.display().to_string(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
            path: None,
        })
        .await
        .unwrap();
    let id = added.id;
    let old = a.join("Breaking Bad");
    std::fs::create_dir_all(old.join("Season 01")).unwrap();
    let episode = "Season 01/Breaking.Bad.S01E01.720p.HDTV.x264-GRP.mkv";
    std::fs::write(old.join(episode), b"video").unwrap();
    client.rescan_series(id).await.unwrap();
    let files = |d: &kitsunarr_domain::api::library::SeriesDetail| d.item.episode_file_count;
    assert_eq!(files(&client.series_detail(id).await.unwrap()), 1);

    let refuse = |path: String| MoveTitleRequest {
        path,
        move_files: true,
    };
    // Outside the root folders, `..`, another title's folder and a non-empty folder are refused.
    std::fs::create_dir_all(b.join("Busy")).unwrap();
    std::fs::write(b.join("Busy/x"), b"x").unwrap();
    for bad in [
        "/tmp".to_string(),
        format!("{}/../b/BB", a.display()),
        b.join("Busy").display().to_string(),
        old.join("Season 01").display().to_string(),
    ] {
        match client
            .move_series(id, &refuse(bad.clone()))
            .await
            .unwrap_err()
        {
            ClientError::Api { status, body } => {
                assert_eq!(status, 422, "{bad}");
                assert_eq!(body.fields[0].field, "path", "{bad}: {}", body.message);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(old.join(episode).is_file(), "refusals move nothing");

    // Moved into the other root folder, below a new parent folder.
    let new = b.join("Drama/Breaking Bad");
    let moved = client
        .move_series(id, &refuse(new.display().to_string()))
        .await
        .unwrap();
    assert!(moved.moved, "{moved:?}");
    assert_eq!(moved.root_folder_path, b.display().to_string());
    assert!(!old.exists() && new.join(episode).is_file());
    let detail = client.series_detail(id).await.unwrap();
    assert_eq!(detail.item.path, new.display().to_string());
    assert_eq!(detail.root_folder_path, b.display().to_string());
    assert_eq!(files(&detail), 1, "file records follow the folder");
    assert_eq!(
        client.rescan_series(id).await.unwrap().message,
        "0 files added, 0 missing files removed, 0 sizes updated."
    );

    // Bulk: back to the first root folder, keeping the folder below the root.
    let bulk = client
        .bulk_move_series(&BulkMoveRequest {
            ids: vec![id, 9999],
            root_folder_path: a.display().to_string(),
            move_files: true,
        })
        .await
        .unwrap();
    assert_eq!(bulk.moved.len(), 1);
    assert_eq!(bulk.failures.len(), 1, "{:?}", bulk.failures);
    let back = a.join("Drama/Breaking Bad");
    assert!(back.join(episode).is_file());
    assert_eq!(
        client.series_detail(id).await.unwrap().item.path,
        back.display().to_string()
    );

    // Recording only: nothing moves.
    let recorded = client
        .move_series(
            id,
            &MoveTitleRequest {
                path: a.join("Elsewhere").display().to_string(),
                move_files: false,
            },
        )
        .await
        .unwrap();
    assert!(!recorded.moved);
    assert!(back.join(episode).is_file() && !a.join("Elsewhere").exists());

    // v3: the path edit moves with `moveFiles=true`; the editor moves and edits tags.
    let http = reqwest::Client::new();
    let v3 = |p: &str| format!("{}/api/v3{p}", server.base);
    client
        .move_series(
            id,
            &MoveTitleRequest {
                path: back.display().to_string(),
                move_files: false,
            },
        )
        .await
        .unwrap();
    let target = b.join("BB");
    let response = http
        .put(v3(&format!("/series/{id}?moveFiles=true")))
        .header("X-Api-Key", &server.api_key)
        .json(&json!({"id": id, "path": target.display().to_string()}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["path"], target.display().to_string());
    assert!(target.join(episode).is_file() && !back.exists());
    let tag: Value = http
        .post(v3("/tag"))
        .header("X-Api-Key", &server.api_key)
        .json(&json!({"label": "moved"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let response = http
        .put(v3("/series/editor"))
        .header("X-Api-Key", &server.api_key)
        .json(&json!({
            "seriesIds": [id], "rootFolderPath": a.display().to_string(), "moveFiles": true,
            "monitored": false, "tags": [tag["id"]], "applyTags": "replace", "monitorNewItems": "none"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body[0]["path"], a.join("BB").display().to_string());
    assert_eq!(body[0]["monitored"], false);
    assert_eq!(body[0]["tags"], json!([tag["id"]]));
    assert!(a.join("BB").join(episode).is_file());
}

/// Sonarr's "Unmonitor deleted episodes": a manual deletion unmonitors at once; a file missing
/// from disk unmonitors at the end of the scan unless the scan finds the episode a file again.
#[tokio::test]
async fn deleted_episodes_are_unmonitored_when_configured() {
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
    let id = client
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
            path: None,
        })
        .await
        .unwrap()
        .id;
    let season = dir.path().join("Breaking Bad/Season 01");
    std::fs::create_dir_all(&season).unwrap();
    let name = |n: i32| format!("Breaking.Bad.S01E{n:02}.720p.HDTV.x264-GRP.mkv");
    for n in 1..=4 {
        std::fs::write(season.join(name(n)), b"video").unwrap();
    }
    client.rescan_series(id).await.unwrap();
    let state = || async {
        client
            .episodes(id)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.season_number == 1 && e.episode_number <= 4)
            .map(|e| (e.episode_number, e.monitored, e.file.map(|f| f.id)))
            .collect::<Vec<_>>()
    };
    let file_of = |s: &[(i32, bool, Option<i64>)], n: i32| s[(n - 1) as usize].2.unwrap();

    // Off (the default): a deletion keeps the episode monitored.
    let before = state().await;
    client
        .delete_episode_file(id, file_of(&before, 4))
        .await
        .unwrap();
    assert!(state().await[3].1, "unchanged while the option is off");

    let mut media = client.media_management().await.unwrap();
    assert!(!media.auto_unmonitor_previously_downloaded_episodes);
    media.auto_unmonitor_previously_downloaded_episodes = true;
    client.put_media_management(&media).await.unwrap();
    client
        .delete_episode_file(id, file_of(&before, 1))
        .await
        .unwrap();
    // E02 disappears; E03 is renamed, so the scan finds it a file again.
    std::fs::remove_file(season.join(name(2))).unwrap();
    std::fs::rename(
        season.join(name(3)),
        season.join("Breaking.Bad.S01E03.REPACK.720p.HDTV.x264-GRP.mkv"),
    )
    .unwrap();
    client.rescan_series(id).await.unwrap();
    let after = state().await;
    assert_eq!(
        after
            .iter()
            .map(|(n, m, f)| (*n, *m, f.is_some()))
            .collect::<Vec<_>>(),
        vec![
            (1, false, false),
            (2, false, false),
            (3, true, true),
            (4, true, false)
        ]
    );
}

/// Removing series with Sonarr's `addImportListExclusion`, from the bulk editor and the v3
/// series editor.
#[tokio::test]
async fn removed_series_can_be_excluded_from_import_lists() {
    use kitsunarr_domain::api::library::BulkDeleteRequest;
    use serde_json::json;
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
    let root = dir.path().display().to_string();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    let add = || async {
        client
            .add_title(&AddTitleRequest {
                provider: MetadataProvider::TvMaze,
                id: "169".into(),
                root_folder_path: root.clone(),
                quality_profile_id: 1,
                monitor: MonitorOption::All,
                series_type: None,
                season_folder: true,
                tags: vec![],
                search_for_missing: false,
                episode_ordering: None,
                path: None,
            })
            .await
            .unwrap()
            .id
    };
    // A TVmaze-owned series is excluded by the TheTVDB id lists know.
    let id = add().await;
    client.delete_series_with(id, false, true).await.unwrap();
    let exclusions = client.import_list_exclusions().await.unwrap();
    assert_eq!(exclusions.len(), 1);
    assert_eq!(
        (
            exclusions[0].provider.as_str(),
            exclusions[0].external_id.as_str()
        ),
        ("tvdb", "81189")
    );
    assert_eq!(exclusions[0].title.as_deref(), Some("Breaking Bad"));

    // Bulk: an unknown id is reported, the folder goes with deleteFiles; excluding again
    // keeps one row.
    let id = add().await;
    let folder = dir.path().join("Breaking Bad");
    std::fs::create_dir_all(&folder).unwrap();
    let result = client
        .bulk_delete_series(&BulkDeleteRequest {
            ids: vec![id, 9999],
            delete_files: true,
            add_import_list_exclusion: true,
        })
        .await
        .unwrap();
    assert_eq!(result.deleted, 1);
    assert_eq!(result.failures.len(), 1, "{:?}", result.failures);
    assert!(!folder.exists());
    assert_eq!(client.import_list_exclusions().await.unwrap().len(), 1);

    // v3 series editor delete.
    let id = add().await;
    let response = reqwest::Client::new()
        .delete(format!("{}/api/v3/series/editor", server.base))
        .header("X-Api-Key", &server.api_key)
        .json(&json!({"seriesIds": [id], "deleteFiles": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(status(client.series_detail(id).await.unwrap_err()), 404);
}

/// Sonarr's "Organize" for several series (`POST /series/rename`) and the v3 `RenameFiles`
/// and `RenameSeries` commands.
#[tokio::test]
async fn several_series_are_organized_at_once() {
    use kitsunarr_domain::api::library::BulkRenameRequest;
    use serde_json::json;
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
    let root = dir.path().display().to_string();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    let id = client
        .add_title(&AddTitleRequest {
            provider: MetadataProvider::TvMaze,
            id: "169".into(),
            root_folder_path: root.clone(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
            path: None,
        })
        .await
        .unwrap()
        .id;
    let season = dir.path().join("Breaking Bad/Season 01");
    std::fs::create_dir_all(&season).unwrap();
    for n in 1..=3 {
        std::fs::write(
            season.join(format!("Breaking.Bad.S01E{n:02}.720p.HDTV.x264-GRP.mkv")),
            b"video",
        )
        .unwrap();
    }
    client.rescan_series(id).await.unwrap();
    let mut naming = client.naming().await.unwrap();
    naming.rename_episodes = true;
    naming.season_folder_format = "Season {season:00}".into();
    client.put_naming(&naming).await.unwrap();
    let file_of = |n: i32| {
        let client = &client;
        async move {
            client
                .episodes(id)
                .await
                .unwrap()
                .into_iter()
                .find(|e| e.season_number == 1 && e.episode_number == n)
                .and_then(|e| e.file)
                .unwrap()
        }
    };

    // v3 RenameFiles: the chosen file only.
    let http = reqwest::Client::new();
    let command = |body: serde_json::Value| {
        http.post(format!("{}/api/v3/command", server.base))
            .header("X-Api-Key", &server.api_key)
            .json(&body)
            .send()
    };
    let first = file_of(1).await.id;
    let response = command(json!({"name": "RenameFiles", "seriesId": id, "files": [first]}))
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    for _ in 0..50 {
        if season
            .join("Breaking Bad - S01E01 - Pilot HDTV-720p.mkv")
            .is_file()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        season
            .join("Breaking Bad - S01E01 - Pilot HDTV-720p.mkv")
            .is_file()
    );
    assert!(
        season
            .join("Breaking.Bad.S01E02.720p.HDTV.x264-GRP.mkv")
            .is_file(),
        "others wait"
    );
    assert_eq!(
        command(json!({"name": "RenameSeries"}))
            .await
            .unwrap()
            .status(),
        422,
        "a series list is required"
    );

    // Native bulk organize renames the rest; an unknown series is reported.
    let result = client
        .bulk_rename_series(&BulkRenameRequest {
            ids: vec![id, 9999],
        })
        .await
        .unwrap();
    assert_eq!(result.message, "2 files renamed in 2 series.");
    assert_eq!(result.problems.len(), 1, "{:?}", result.problems);
    assert!(
        !season
            .join("Breaking.Bad.S01E03.720p.HDTV.x264-GRP.mkv")
            .exists()
    );
    assert!(
        file_of(3)
            .await
            .relative_path
            .starts_with("Season 01/Breaking Bad - S01E03")
    );
}

/// Sonarr's `/api/v3/config/*`, tags, root folders, delay profiles, remote path mappings,
/// import lists and their exclusions, tasks, backups and logs, as sync tools and dashboards use
/// them: resources in Sonarr's names, partial writes, Kitsunarr's validation.
#[tokio::test]
async fn sonarr_v3_config_rules_lists_and_system() {
    use reqwest::Method as M;
    use serde_json::{Value, json};
    let server = start().await;
    let http = reqwest::Client::new();
    let key = server.api_key.clone();
    let call = |m: M, p: &str, body: Option<Value>| {
        let mut r = http
            .request(m, format!("{}/api/v3{p}", server.base))
            .header("X-Api-Key", &key);
        if let Some(b) = body {
            r = r.json(&b);
        }
        async move {
            let r = r.send().await.unwrap();
            let status = r.status().as_u16();
            let text = r.text().await.unwrap();
            (
                status,
                serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
            )
        }
    };

    // Media naming as Recyclarr writes it: read, change some formats, write back.
    let (status, mut naming) = call(M::GET, "/config/naming", None).await;
    assert_eq!((status, naming["id"].as_i64()), (200, Some(1)));
    assert_eq!(naming["seriesFolderFormat"], "{Series Title}");
    assert!(naming["movieFormat"].is_string());
    naming["renameEpisodes"] = json!(true);
    naming["standardEpisodeFormat"] = json!(
        "{Series TitleYear} - S{season:00}E{episode:00} - {Episode CleanTitle} [{Quality Full}]"
    );
    naming["seasonFolderFormat"] = json!("Season {season:00}");
    let (status, saved) = call(M::PUT, "/config/naming/1", Some(naming.clone())).await;
    assert_eq!(status, 202, "{saved}");
    assert_eq!(saved["seasonFolderFormat"], "Season {season:00}");
    let native: kitsunarr_domain::api::settings::NamingSettings =
        kitsunarr_db::settings::get_or_default(
            server.state.db.pool(),
            kitsunarr_db::settings::KEY_NAMING,
        )
        .await
        .unwrap();
    assert!(native.rename_episodes);
    assert_eq!(native.season_folder_format, "Season {season:00}");
    // Only the sent keys change; Sonarr's ranges and Kitsunarr's checks apply.
    let (status, _) = call(
        M::PUT,
        "/config/naming",
        Some(json!({"multiEpisodeStyle": 9})),
    )
    .await;
    assert_eq!(status, 422);
    let (status, err) = call(
        M::PUT,
        "/config/naming",
        Some(json!({"seriesFolderFormat": "../x"})),
    )
    .await;
    assert_eq!(status, 422, "{err}");
    let (_, after) = call(M::GET, "/config/naming", None).await;
    assert_eq!(after["seriesFolderFormat"], "{Series Title}");
    assert_eq!(after["seasonFolderFormat"], "Season {season:00}");

    // Media management: Recyclarr's "propers and repacks", Sonarr's names for Kitsunarr's keys.
    let (status, media) = call(M::GET, "/config/mediamanagement", None).await;
    assert_eq!(status, 200);
    assert_eq!(media["downloadPropersAndRepacks"], "preferAndUpgrade");
    assert!(media["copyUsingHardlinks"].is_boolean());
    let (status, saved) = call(
        M::PUT,
        "/config/mediamanagement",
        Some(json!({"downloadPropersAndRepacks": "doNotPrefer", "recycleBin": "/srv/recycle", "minimumFreeSpaceWhenImporting": 250, "fileDate": "utcAirDate", "rescanAfterRefresh": "never"})),
    )
    .await;
    assert_eq!(status, 202, "{saved}");
    assert_eq!(saved["downloadPropersAndRepacks"], "doNotPrefer");
    assert_eq!(saved["recycleBin"], "/srv/recycle");
    assert_eq!(saved["minimumFreeSpaceWhenImporting"], 250);
    assert_eq!(saved["fileDate"], "utcAirDate");
    let (status, _) = call(
        M::PUT,
        "/config/mediamanagement",
        Some(json!({"chmodFolder": "999"})),
    )
    .await;
    assert_eq!(status, 422);
    let (status, _) = call(
        M::PUT,
        "/config/mediamanagement",
        Some(json!({"downloadPropersAndRepacks": "always"})),
    )
    .await;
    assert_eq!(status, 422);
    let (_, media) = call(
        M::PUT,
        "/config/mediamanagement",
        Some(json!({"recycleBin": ""})),
    )
    .await;
    assert_eq!(media["recycleBin"], "");

    // Indexer, download client and import list options.
    let (status, saved) = call(
        M::PUT,
        "/config/indexer",
        Some(json!({"rssSyncInterval": 30, "retention": 1500})),
    )
    .await;
    assert_eq!(
        (
            status,
            saved["rssSyncInterval"].as_i64(),
            saved["retention"].as_i64()
        ),
        (202, Some(30), Some(1500))
    );
    let (status, _) = call(
        M::PUT,
        "/config/indexer",
        Some(json!({"rssSyncInterval": 5})),
    )
    .await;
    assert_eq!(status, 422);
    let (status, saved) = call(
        M::PUT,
        "/config/downloadclient",
        Some(json!({"autoRedownloadFailed": false})),
    )
    .await;
    assert_eq!(
        (status, saved["autoRedownloadFailed"].as_bool()),
        (202, Some(false))
    );
    assert_eq!(saved["enableCompletedDownloadHandling"], true);
    let (status, _) = call(
        M::PUT,
        "/config/importlist",
        Some(json!({"listSyncLevel": "keepAndTag", "listSyncTag": 0})),
    )
    .await;
    assert_eq!(status, 422, "keepAndTag needs a tag");
    let (status, saved) = call(
        M::PUT,
        "/config/importlist",
        Some(json!({"listSyncLevel": "logOnly"})),
    )
    .await;
    assert_eq!(
        (status, saved["listSyncLevel"].as_str()),
        (202, Some("logOnly"))
    );

    // Interface and host settings.
    let (status, saved) = call(
        M::PUT,
        "/config/ui",
        Some(json!({"firstDayOfWeek": 0, "theme": "dark"})),
    )
    .await;
    assert_eq!((status, saved["firstDayOfWeek"].as_i64()), (202, Some(0)));
    let (status, host) = call(M::GET, "/config/host", None).await;
    assert_eq!(status, 200);
    assert_eq!(host["apiKey"], "", "the key is never returned");
    assert_eq!(host["proxyEnabled"], false);
    let (status, saved) = call(
        M::PUT,
        "/config/host",
        Some(json!({"instanceName": "Den", "proxyEnabled": true, "proxyType": "socks5", "proxyHostname": "proxy.lan", "proxyPort": 1080, "proxyPassword": "pw", "certificateValidation": "disabledForLocalAddresses", "port": 1})),
    )
    .await;
    assert_eq!(status, 202, "{saved}");
    assert_eq!(saved["instanceName"], "Den");
    assert_eq!(saved["proxyType"], "socks5");
    assert_eq!(saved["proxyPassword"], "********");
    assert_eq!(saved["certificateValidation"], "disabledForLocalAddresses");
    assert_ne!(saved["port"], 1, "the port comes from config.toml");
    let (status, _) = call(M::PUT, "/config/host", Some(json!({"proxyType": "ftp"}))).await;
    assert_eq!(status, 422);
    let (_, unchanged) = call(M::PUT, "/config/host", Some(saved.clone())).await;
    assert_eq!(unchanged["proxyHostname"], "proxy.lan");
    let proxy: kitsunarr_domain::api::settings::ProxySettings =
        kitsunarr_db::settings::get_or_default(
            server.state.db.pool(),
            kitsunarr_db::settings::KEY_PROXY,
        )
        .await
        .unwrap();
    assert_eq!(
        proxy.password, "pw",
        "the redacted placeholder keeps the password"
    );

    // Tags: rename, details, delete.
    let (_, tag) = call(M::POST, "/tag", Some(json!({"label": "4k"}))).await;
    let tag_id = tag["id"].as_i64().unwrap();
    let (status, renamed) = call(
        M::PUT,
        &format!("/tag/{tag_id}"),
        Some(json!({"id": tag_id, "label": "UHD"})),
    )
    .await;
    assert_eq!((status, renamed["label"].as_str()), (202, Some("uhd")));
    let (status, _) = call(M::PUT, "/tag/9999", Some(json!({"label": "x"}))).await;
    assert_eq!(status, 404);

    // Delay profiles in Sonarr's shape (`order`), with the tag.
    let (status, delay) = call(
        M::POST,
        "/delayprofile",
        Some(json!({"enableUsenet": true, "enableTorrent": true, "preferredProtocol": "torrent", "usenetDelay": 0, "torrentDelay": 60, "tags": [tag_id]})),
    )
    .await;
    assert_eq!(status, 201, "{delay}");
    let delay_id = delay["id"].as_i64().unwrap();
    assert_eq!(delay["preferredProtocol"], "torrent");
    let (status, delay) = call(
        M::PUT,
        &format!("/delayprofile/{delay_id}"),
        Some(json!({"torrentDelay": 90})),
    )
    .await;
    assert_eq!((status, delay["torrentDelay"].as_i64()), (202, Some(90)));
    assert_eq!(delay["tags"], json!([tag_id]));
    let (_, list) = call(M::GET, "/delayprofile", None).await;
    assert!(list.as_array().unwrap().len() >= 2, "{list}");
    let (status, _) = call(
        M::POST,
        "/delayprofile",
        Some(json!({"preferredProtocol": "ftp"})),
    )
    .await;
    assert_eq!(status, 422);

    let (status, detail) = call(M::GET, &format!("/tag/detail/{tag_id}"), None).await;
    assert_eq!(status, 200);
    assert_eq!(detail["delayProfileIds"], json!([delay_id]));
    assert_eq!(detail["seriesIds"], json!([]));
    let (_, all) = call(M::GET, "/tag/detail", None).await;
    assert!(all.as_array().unwrap().iter().any(|t| t["id"] == tag_id));

    // Remote path mappings.
    let (status, mapping) = call(
        M::POST,
        "/remotepathmapping",
        Some(json!({"host": "seedbox", "remotePath": "/downloads/", "localPath": "/mnt/seedbox/"})),
    )
    .await;
    assert_eq!(status, 201, "{mapping}");
    let mapping_id = mapping["id"].as_i64().unwrap();
    let (status, mapping) = call(
        M::PUT,
        &format!("/remotepathmapping/{mapping_id}"),
        Some(json!({"host": "seedbox", "remotePath": "/data/", "localPath": "/mnt/seedbox/"})),
    )
    .await;
    assert_eq!(
        (
            status,
            mapping["remotePath"]
                .as_str()
                .map(|p| p.trim_end_matches('/'))
        ),
        (202, Some("/data"))
    );
    let (status, _) = call(M::POST, "/remotepathmapping", Some(json!({"host": "x"}))).await;
    assert_eq!(status, 422);
    assert_eq!(
        call(M::DELETE, &format!("/remotepathmapping/{mapping_id}"), None)
            .await
            .0,
        200
    );
    assert_eq!(
        call(M::GET, &format!("/remotepathmapping/{mapping_id}"), None)
            .await
            .0,
        404
    );

    // Import list exclusions: TheTVDB ids as Sonarr's, other catalogues as Kitsunarr's.
    let (status, ex) = call(
        M::POST,
        "/importlistexclusion",
        Some(json!({"tvdbId": 81189, "title": "Breaking Bad"})),
    )
    .await;
    assert_eq!(status, 201, "{ex}");
    let ex_id = ex["id"].as_i64().unwrap();
    assert_eq!(
        (ex["tvdbId"].as_i64(), ex["provider"].as_str()),
        (Some(81189), Some("tvdb"))
    );
    let (_, ex2) = call(
        M::POST,
        "/importlistexclusion",
        Some(json!({"provider": "aniList", "externalId": "21", "title": "One Piece"})),
    )
    .await;
    assert_eq!(ex2["tvdbId"], 0);
    let (status, updated) = call(
        M::PUT,
        &format!("/importlistexclusion/{ex_id}"),
        Some(json!({"tvdbId": 81190, "title": "Better Call Saul"})),
    )
    .await;
    assert_eq!((status, updated["tvdbId"].as_i64()), (202, Some(81190)));
    let (_, page) = call(M::GET, "/importlistexclusion/paged?page=1&pageSize=1", None).await;
    assert_eq!(
        (
            page["totalRecords"].as_i64(),
            page["records"].as_array().unwrap().len()
        ),
        (Some(2), 1)
    );
    assert_eq!(page["records"][0]["title"], "Better Call Saul");
    let (status, _) = call(
        M::POST,
        "/importlistexclusion",
        Some(json!({"title": "no id"})),
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(
        call(
            M::DELETE,
            "/importlistexclusion/bulk",
            Some(json!({"ids": [ex_id, ex2["id"]]}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(M::GET, "/importlistexclusion", None).await.1,
        json!([])
    );

    // Import lists: Sonarr's options are top-level keys.
    let (_, schema) = call(M::GET, "/importlist/schema", None).await;
    let mut anilist = schema
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["implementation"] == "AniListImport")
        .unwrap()
        .clone();
    anilist["name"] = json!("Watching");
    anilist["shouldMonitor"] = json!("firstSeason");
    anilist["rootFolderPath"] = json!("/anime");
    anilist["tags"] = json!([tag_id]);
    let (status, list) = call(M::POST, "/importlist", Some(anilist)).await;
    assert_eq!(status, 201, "{list}");
    let list_id = list["id"].as_i64().unwrap();
    assert_eq!(list["shouldMonitor"], "firstSeason");
    assert_eq!(list["rootFolderPath"], "/anime");
    let mut changed = list.clone();
    changed["enableAutomaticAdd"] = json!(false);
    let (status, list) = call(M::PUT, &format!("/importlist/{list_id}"), Some(changed)).await;
    assert_eq!(
        (status, list["enableAutomaticAdd"].as_bool()),
        (202, Some(false))
    );
    assert_eq!(
        call(M::GET, &format!("/importlist/{list_id}"), None)
            .await
            .1["name"],
        "Watching"
    );
    let (_, detail) = call(M::GET, &format!("/tag/detail/{tag_id}"), None).await;
    assert_eq!(detail["importListIds"], json!([list_id]));
    let (status, _) = call(
        M::POST,
        "/importlist",
        Some(json!({"name": "x", "implementation": "Nope", "fields": []})),
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(
        call(M::DELETE, &format!("/importlist/{list_id}"), None)
            .await
            .0,
        200
    );
    // Deleting the tag removes it from the delay profile.
    assert_eq!(
        call(M::DELETE, &format!("/tag/{tag_id}"), None).await.0,
        200
    );
    assert_eq!(
        call(M::GET, &format!("/delayprofile/{delay_id}"), None)
            .await
            .1["tags"],
        json!([])
    );
    assert_eq!(
        call(M::DELETE, &format!("/delayprofile/{delay_id}"), None)
            .await
            .0,
        200
    );

    // Root folders.
    let folder = tempfile::tempdir().unwrap();
    let (status, root) = call(M::POST, "/rootfolder", Some(json!({"path": folder.path()}))).await;
    assert_eq!(status, 201, "{root}");
    let root_id = root["id"].as_i64().unwrap();
    assert_eq!(root["accessible"], true);
    assert_eq!(root["unmappedFolders"], json!([]));
    let (status, _) = call(
        M::POST,
        "/rootfolder",
        Some(json!({"path": "/does/not/exist"})),
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(
        call(M::GET, &format!("/rootfolder/{root_id}"), None)
            .await
            .1["id"],
        root_id
    );
    assert_eq!(
        call(M::DELETE, &format!("/rootfolder/{root_id}"), None)
            .await
            .0,
        200
    );
    assert_eq!(
        call(M::GET, &format!("/rootfolder/{root_id}"), None)
            .await
            .0,
        404
    );

    // Tasks, backups, logs, queue status and the rename preview.
    let (status, tasks) = call(M::GET, "/system/task", None).await;
    assert_eq!(status, 200);
    let rss = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["taskName"] == "RssSync")
        .unwrap_or_else(|| panic!("{tasks}"));
    assert_eq!(rss["interval"], 30, "the interval written above");
    assert_eq!(
        call(M::GET, &format!("/system/task/{}", rss["id"]), None)
            .await
            .1["taskName"],
        "RssSync"
    );
    let (status, backups) = call(M::GET, "/system/backup", None).await;
    assert_eq!((status, backups.is_array()), (200, true));
    let (status, logs) = call(M::GET, "/log?page=1&pageSize=5", None).await;
    assert_eq!(status, 200);
    assert_eq!(logs["sortDirection"], "descending");
    assert!(logs["records"].as_array().unwrap().len() <= 5);
    let (status, q) = call(M::GET, "/queue/status", None).await;
    assert_eq!((status, q["totalCount"].as_i64()), (200, Some(0)));
    assert_eq!(call(M::GET, "/rename?seriesId=999", None).await.0, 404);
}

/// Sonarr's manual import resources and the commands post-processing scripts send
/// (`DownloadedEpisodesScan` with a path, `ManualImport` with chosen files).
#[tokio::test]
async fn sonarr_v3_manual_import_and_downloaded_episodes_scan() {
    use serde_json::{Value, json};
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
    let root = dir.path().join("tv");
    std::fs::create_dir_all(&root).unwrap();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.display().to_string(),
            media_kind: None,
        })
        .await
        .unwrap();
    let id = client
        .add_title(&AddTitleRequest {
            provider: MetadataProvider::TvMaze,
            id: "169".into(),
            root_folder_path: root.display().to_string(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
            path: None,
        })
        .await
        .unwrap()
        .id;
    // A neutral folder: a release-named one would name the series of every file in it.
    let incoming = dir.path().join("downloads/incoming");
    std::fs::create_dir_all(&incoming).unwrap();
    std::fs::write(
        incoming.join("Breaking.Bad.S01E04.720p.HDTV.x264-GRP.mkv"),
        b"ep4",
    )
    .unwrap();
    std::fs::write(
        incoming.join("Unknown.Thing.S03E04.720p.HDTV.x264-GRP.mkv"),
        b"x",
    )
    .unwrap();
    let http = reqwest::Client::new();
    let v3 = |p: &str| format!("{}/api/v3{p}", server.base);

    let items: Value = http
        .get(v3("/manualimport"))
        .query(&[("folder", incoming.display().to_string())])
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    let ep4 = items
        .iter()
        .find(|i| i["name"].as_str().unwrap().starts_with("Breaking"))
        .unwrap();
    assert_eq!(ep4["series"]["id"], id);
    assert_eq!(ep4["seasonNumber"], 1);
    assert_eq!(ep4["episodes"][0]["title"], "S01E04");
    assert_eq!(ep4["quality"]["quality"]["name"], "HDTV-720p");
    assert_eq!(ep4["rejections"], json!([]));
    assert_eq!(ep4["folderName"], "incoming");
    let unknown = items
        .iter()
        .find(|i| i["name"].as_str().unwrap().starts_with("Unknown"))
        .unwrap();
    assert_eq!(unknown["rejections"][0]["reason"], "Unknown series");

    let command = |body: Value| {
        http.post(v3("/command"))
            .header("X-Api-Key", &server.api_key)
            .json(&body)
            .send()
    };
    let episode_file = |n: i32| {
        let client = &client;
        async move {
            client
                .episodes(id)
                .await
                .unwrap()
                .into_iter()
                .find(|e| e.season_number == 1 && e.episode_number == n)
                .and_then(|e| e.file)
        }
    };
    // A post-processing script's scan of the finished folder: the matched file is imported
    // (copied, as asked), the unknown one is left.
    let r =
        command(json!({"name": "DownloadedEpisodesScan", "path": incoming, "importMode": "Copy"}))
            .await
            .unwrap();
    assert_eq!(r.status(), 201);
    let started: Value = r.json().await.unwrap();
    let mut done = Value::Null;
    for _ in 0..100 {
        done = http
            .get(v3(&format!("/command/{}", started["id"])))
            .header("X-Api-Key", &server.api_key)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if done["status"] != "started" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(done["status"], "completed", "{done}");
    assert!(
        done["message"]
            .as_str()
            .unwrap()
            .starts_with("1 of 2 files imported from"),
        "{done}"
    );
    let mut imported = None;
    for _ in 0..100 {
        imported = episode_file(4).await;
        if imported.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(imported.is_some(), "S01E04 was imported");
    assert!(
        incoming
            .join("Breaking.Bad.S01E04.720p.HDTV.x264-GRP.mkv")
            .is_file(),
        "copied"
    );
    assert!(
        incoming
            .join("Unknown.Thing.S03E04.720p.HDTV.x264-GRP.mkv")
            .is_file()
    );

    // Sonarr's manual import: the unknown file is assigned by hand and moved.
    let s01e05 = client
        .episodes(id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.season_number == 1 && e.episode_number == 5)
        .unwrap();
    let unknown_path = incoming.join("Unknown.Thing.S03E04.720p.HDTV.x264-GRP.mkv");
    let r = command(json!({"name": "ManualImport", "importMode": "move",
        "files": [{"path": unknown_path, "seriesId": id, "episodeIds": [s01e05.id]}]}))
    .await
    .unwrap();
    assert_eq!(r.status(), 201);
    let mut moved = None;
    for _ in 0..100 {
        moved = episode_file(5).await;
        if moved.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(moved.is_some(), "S01E05 was imported");
    assert!(!unknown_path.exists(), "moved");
    let r = command(json!({"name": "ManualImport", "files": [{"path": "/x.mkv"}]}))
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
}

/// Sonarr's `SearchAnimeSeason`: an anime season search also searches each episode; with
/// Tanukarr's "Prefer anime season packs", an automatic search skips the episodes an approved
/// full-season release covers.
#[tokio::test]
async fn anime_season_searches_search_each_episode_unless_a_pack_covers_it() {
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use kitsunarr_domain::api::search::ReleaseQuery;
    use std::collections::HashMap;
    type Log = Arc<std::sync::Mutex<Vec<String>>>;
    let log: Log = Arc::default();
    let router = axum::Router::new()
        .route(
            "/",
            axum::routing::get(
                |State(log): State<Log>, Query(query): Query<HashMap<String, String>>| async move {
                    let term = query.get("term").cloned().unwrap_or_default();
                    log.lock().unwrap().push(term.clone());
                    let item = if term == "Pack Show" {
                        r#"<item><title>Pack.Show.S01.1080p.WEB-DL.AAC2.0.x264-GRP</title>
                        <link>https://nyaa.si/download/1.torrent</link>
                        <guid isPermaLink="true">https://nyaa.si/view/1</guid>
                        <pubDate>Tue, 24 Aug 2021 22:18:46 -0000</pubDate>
                        <nyaa:seeders>40</nyaa:seeders><nyaa:leechers>3</nyaa:leechers>
                        <nyaa:infoHash>e8ca5e20eca876339f41c3d9e95ea66c1d7caaee</nyaa:infoHash>
                        <nyaa:categoryId>1_2</nyaa:categoryId><nyaa:size>2.0 GiB</nyaa:size></item>"#
                    } else {
                        ""
                    };
                    (
                        [("content-type", "application/rss+xml")],
                        format!(
                            r#"<rss xmlns:nyaa="https://nyaa.si/xmlns/nyaa" version="2.0"><channel>{item}</channel></rss>"#
                        ),
                    )
                        .into_response()
                },
            ),
        )
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let server = start().await;
    let client = signed_in(&server).await;
    let pool = server.state.db.pool();
    sqlx::query("INSERT INTO series (id,media_kind,title,sort_title,clean_title,title_slug,series_type,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (1,'anime','Pack Show','packshow','packshow','pack-show','anime','myAnimeList','1',1,'/library','/library/show','2026-11-15')").execute(pool).await.unwrap();
    sqlx::query("INSERT INTO seasons (series_id, season_number, monitored) VALUES (1, 1, 1)")
        .execute(pool)
        .await
        .unwrap();
    for id in 1..=2 {
        sqlx::query("INSERT INTO episodes (id,series_id,season_number,episode_number,absolute_episode_number,air_date_utc,monitored) VALUES (?,1,1,?,?,'2020-01-01T00:00:00Z',1)").bind(id).bind(id).bind(id).execute(pool).await.unwrap();
    }
    let _: serde_json::Value = client
        .post(
            "/providers/indexers",
            &serde_json::json!({
                "name": "Nyaa", "implementation": "Nyaa", "enabled": true,
                "options": {"enableInteractiveSearch": true, "enableRss": false, "enableAutomaticSearch": true},
                "settings": {"baseUrl": base, "animeStandardFormatSearch": false, "additionalParameters": "&c=1_0&f=0", "minimumSeeders": 1}
            }),
        )
        .await
        .unwrap();
    let q = ReleaseQuery {
        series_id: 1,
        season_number: Some(1),
        episode_id: None,
        complete_series: false,
        season_packs_only: false,
        indexer_ids: None,
        search_session: None,
    };
    let terms = |log: &Log| {
        let mut t = log.lock().unwrap().clone();
        t.sort();
        t
    };

    // Without the option: the season, then each episode.
    let r = client.automatic_search(&q).await.unwrap();
    assert_eq!(r.approved, 1, "{r:?}");
    assert_eq!(
        terms(&log),
        [
            "Pack Show",
            "Pack Show 01",
            "Pack Show 02",
            "Pack Show 1",
            "Pack Show 2"
        ]
    );

    // With it: the approved pack covers both episodes, so they are not searched.
    let mut tools: kitsunarr_domain::api::settings::LibraryToolsSettings =
        kitsunarr_db::settings::get_or_default(pool, kitsunarr_db::settings::KEY_LIBRARY_TOOLS)
            .await
            .unwrap();
    tools.prefer_anime_season_packs = true;
    kitsunarr_db::settings::set(pool, kitsunarr_db::settings::KEY_LIBRARY_TOOLS, &tools)
        .await
        .unwrap();
    log.lock().unwrap().clear();
    let r = client.automatic_search(&q).await.unwrap();
    assert_eq!(r.approved, 1, "{r:?}");
    assert_eq!(terms(&log), ["Pack Show"]);

    // An interactive search still searches every episode.
    log.lock().unwrap().clear();
    client.search_releases(&q).await.unwrap();
    assert_eq!(terms(&log).len(), 5);

    // A season-pack search searches the season only.
    log.lock().unwrap().clear();
    let packs = ReleaseQuery {
        season_packs_only: true,
        ..q.clone()
    };
    client.search_releases(&packs).await.unwrap();
    assert_eq!(terms(&log), ["Pack Show"]);
}
