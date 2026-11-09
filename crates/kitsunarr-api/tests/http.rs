//! HTTP integration tests: the real router served on a random local port, exercised through
//! the typed client (the same client the web UI uses) and raw requests.

use std::net::SocketAddr;
use std::sync::Arc;

use kitsunarr_api::{AppState, BuildInfo, LogBuffer, NoAssets, NoTasks};
use kitsunarr_client::{Client, ClientError};
use kitsunarr_config::Config;
use kitsunarr_db::Database;
use kitsunarr_domain::api::auth::{ChangePasswordRequest, LoginRequest, SetupRequest};
use kitsunarr_domain::api::library::{
    BulkEdit, EpisodesMonitoredUpdate, LibraryQuery, SeriesUpdate,
};
use kitsunarr_domain::api::migration::{MigrationOptions, PathMapping, SessionStatus};
use kitsunarr_domain::api::settings::{REDACTED, RootFolderCreate};

struct Server {
    base: String,
    api_key: String,
    _dir: tempfile::TempDir,
}

async fn start() -> Server {
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
        LogBuffer::default(),
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
