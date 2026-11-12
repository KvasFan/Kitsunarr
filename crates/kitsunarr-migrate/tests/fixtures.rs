//! Migration tests against real fixtures captured from Sonarr 4.0.20.3014 and Tanukarr 0.1.5
//! (see `tests/fixtures/README.md`). The "API" tests replay captured API responses from a
//! local HTTP server (simulated source); the backup tests read the applications' own backups.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kitsunarr_db::Database;
use kitsunarr_domain::api::migration::{
    ApiSourceRequest, ConflictPolicy, MigrationOptions, PathMapping, SourceApp,
};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_migrate::{LoadedSource, Mode, load_from_api, load_from_backup, run};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

async fn fresh_db(dir: &Path) -> Database {
    let db = Database::open(&dir.join("kitsunarr.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    db
}

/// Creates the (empty) series folders the source references, below `root`.
fn make_media(root: &Path, paths: &[&str]) {
    for p in paths {
        std::fs::create_dir_all(root.join(p.trim_start_matches('/'))).unwrap();
    }
}

/// Recursive listing (path, size) used to prove nothing on disk changed.
fn tree(root: &Path) -> BTreeMap<PathBuf, u64> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                stack.push(entry.path());
            }
            out.insert(entry.path(), meta.len());
        }
    }
    out
}

fn options(media_root: &Path) -> MigrationOptions {
    MigrationOptions {
        path_mappings: vec![PathMapping {
            source_prefix: "/media".into(),
            target_prefix: media_root.display().to_string(),
        }],
        ..Default::default()
    }
}

async fn count(db: &Database, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(db.pool()).await.unwrap()
}

fn noop(_: &str, _: u64, _: u64) {}

#[tokio::test]
async fn sonarr_backup_is_detected_and_normalised() {
    let tmp = tempfile::tempdir().unwrap();
    let src = load_from_backup(&fixture("sonarr-4.0.20/backup.zip"), tmp.path())
        .await
        .unwrap();
    let d = &src.detected;
    assert_eq!(d.app, SourceApp::Sonarr);
    assert_eq!(d.app_version, "4.0.20.3014");
    assert_eq!(d.schema_version, Some(217));
    assert_eq!(d.instance_name.as_deref(), Some("Sonarr"));
    assert!(d.secrets_available);
    let s = &src.snapshot;
    assert_eq!(s.series.len(), 3);
    assert_eq!(s.episodes.len(), 127);
    assert_eq!(s.episode_files.len(), 14);
    assert_eq!(s.tags.len(), 4);
    assert_eq!(s.custom_formats.len(), 4);
    assert_eq!(s.quality_profiles.len(), 7);
    assert_eq!(s.delay_profiles.len(), 2);
    assert_eq!(s.release_profiles.len(), 3);
    assert_eq!(s.remote_path_mappings.len(), 1);
    assert_eq!(s.exclusions.len(), 1);
    let kinds = |k| s.providers.iter().filter(|p| p.kind == k).count();
    assert_eq!(
        (
            kinds(ProviderKind::Indexer),
            kinds(ProviderKind::DownloadClient),
            kinds(ProviderKind::Notification)
        ),
        (2, 3, 2)
    );
    let qbit = s
        .providers
        .iter()
        .find(|p| p.implementation == "QBittorrent")
        .unwrap();
    assert_eq!(
        qbit.settings["password"], "fixture-qbit-password",
        "backups carry real secrets"
    );
    let bebop = s.series.iter().find(|x| x.title == "Cowboy Bebop").unwrap();
    assert_eq!(bebop.series_type, kitsunarr_domain::SeriesType::Anime);
    assert_eq!(bebop.mal_ids, vec![1, 5]);
    assert!(
        s.internal_settings_skipped >= 4,
        "crypto salts are skipped, not imported"
    );
    // The temporary extraction directory is gone.
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn sonarr_backup_dry_run_then_apply_then_rerun() {
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    make_media(
        &media,
        &["/tv/Breaking Bad", "/tv/Chernobyl", "/anime/Cowboy Bebop"],
    );
    let before = tree(&media);
    let fixture_bytes = std::fs::read(fixture("sonarr-4.0.20/backup.zip")).unwrap();

    let src = load_from_backup(&fixture("sonarr-4.0.20/backup.zip"), tmp.path())
        .await
        .unwrap();
    let db = fresh_db(tmp.path()).await;
    let opts = options(&media);

    // Dry run: full preview, database untouched.
    let preview = run(&db, &src, &opts, Mode::DryRun, &noop).await.unwrap();
    let get = |o: &kitsunarr_migrate::Outcome, e: &str| {
        o.summary
            .iter()
            .find(|s| s.entity == e)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(get(&preview, "series").create, 3);
    assert_eq!(get(&preview, "episodes").create, 127);
    assert_eq!(get(&preview, "episodeFiles").create, 14);
    assert!(preview.blocking.is_empty(), "{:?}", preview.blocking);
    assert!(
        preview.validation.iter().all(|c| c.passed),
        "{:?}",
        preview.validation
    );
    assert!(
        preview.paths.iter().all(|p| p.exists == Some(true)),
        "{:?}",
        preview.paths
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM series").await,
        0,
        "dry run leaves no trace"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM migration_runs").await, 0);

    // Apply.
    let applied = run(
        &db,
        &src,
        &opts,
        Mode::Apply {
            backup_dir: tmp.path().join("backups"),
        },
        &noop,
    )
    .await
    .unwrap();
    assert!(applied.run_id.is_some());
    assert!(
        Path::new(applied.backup_path.as_deref().unwrap()).exists(),
        "pre-migration backup written"
    );
    assert!(
        applied.validation.iter().all(|c| c.passed),
        "{:?}",
        applied.validation
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM series").await, 3);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM episodes").await, 127);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM episode_files").await, 14);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM series WHERE media_kind = 'anime'"
        )
        .await,
        1
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM tags").await, 4);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM custom_formats").await, 4);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM quality_profiles").await,
        7,
        "6 defaults updated + 1 custom created"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM indexers").await, 2);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM download_clients").await, 3);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM notifications").await, 2);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM release_profiles").await, 3);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM remote_path_mappings").await,
        1
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM import_list_exclusions").await,
        1
    );

    // Monitoring flags match the source exactly.
    let src_monitored = src.snapshot.episodes.iter().filter(|e| e.monitored).count() as i64;
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM episodes WHERE monitored = 1").await,
        src_monitored
    );
    let unmonitored_seasons = src
        .snapshot
        .series
        .iter()
        .flat_map(|s| &s.seasons)
        .filter(|s| !s.monitored)
        .count() as i64;
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM seasons WHERE monitored = 0").await,
        unmonitored_seasons
    );
    // Paths are mapped, relative file paths preserved verbatim.
    let bb_path: String =
        sqlx::query_scalar("SELECT path FROM series WHERE title = 'Breaking Bad'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(bb_path, media.join("tv/Breaking Bad").display().to_string());
    let rel: String = sqlx::query_scalar(
        "SELECT relative_path FROM episode_files WHERE relative_path LIKE '%S01E01.1080p%'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        rel,
        "Season 01/Breaking.Bad.S01E01.1080p.BluRay.x264-ROVERS.mkv"
    );
    // Provider identity and secrets.
    let ids: Vec<(String, String)> =
        sqlx::query_as("SELECT metadata_provider, metadata_id FROM series ORDER BY metadata_id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        ids,
        vec![
            ("tvdb".into(), "360893".into()),
            ("tvdb".into(), "76885".into()),
            ("tvdb".into(), "81189".into())
        ]
    );
    let settings: String = sqlx::query_scalar(
        "SELECT settings FROM download_clients WHERE implementation = 'QBittorrent'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(settings.contains("fixture-qbit-password"));
    // Quality profile scores reference the imported custom formats.
    let fixture_profile: String = sqlx::query_scalar(
        "SELECT format_items FROM quality_profiles WHERE name = 'Kitsunarr Fixture 1080p'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(
        fixture_profile.contains("-500") && fixture_profile.contains("100"),
        "{fixture_profile}"
    );
    let integrity = kitsunarr_db::maintenance::integrity_check(&db)
        .await
        .unwrap();
    assert!(integrity.ok(), "{integrity:?}");

    // Re-running is idempotent.
    let again = run(
        &db,
        &src,
        &opts,
        Mode::Apply {
            backup_dir: tmp.path().join("backups"),
        },
        &noop,
    )
    .await
    .unwrap();
    assert_eq!(get(&again, "series").create, 0);
    assert_eq!(get(&again, "series").skip, 3);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM series").await, 3);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM episodes").await, 127);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM indexers").await, 2);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM migration_runs WHERE status = 'completed'"
        )
        .await,
        2
    );

    // Nothing on disk was created, moved or deleted, and the source is untouched.
    assert_eq!(tree(&media), before);
    assert_eq!(
        std::fs::read(fixture("sonarr-4.0.20/backup.zip")).unwrap(),
        fixture_bytes
    );
}

#[tokio::test]
async fn update_policy_overwrites_and_skip_policy_preserves() {
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    make_media(
        &media,
        &["/tv/Breaking Bad", "/tv/Chernobyl", "/anime/Cowboy Bebop"],
    );
    let src = load_from_backup(&fixture("sonarr-4.0.20/backup.zip"), tmp.path())
        .await
        .unwrap();
    let db = fresh_db(tmp.path()).await;
    let mut opts = options(&media);
    run(
        &db,
        &src,
        &opts,
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE series SET monitored = 0")
        .execute(db.pool())
        .await
        .unwrap();

    run(
        &db,
        &src,
        &opts,
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM series WHERE monitored = 1").await,
        0,
        "skip keeps local edits"
    );

    opts.conflict_policy = ConflictPolicy::Update;
    run(
        &db,
        &src,
        &opts,
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    let expected = src.snapshot.series.iter().filter(|s| s.monitored).count() as i64;
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM series WHERE monitored = 1").await,
        expected,
        "update restores source values"
    );
}

/// Sonarr's proxy, stored in its `Config` table as text, becomes Kitsunarr's proxy settings.
#[tokio::test]
async fn sonarr_proxy_settings_are_imported() {
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    make_media(
        &media,
        &["/tv/Breaking Bad", "/tv/Chernobyl", "/anime/Cowboy Bebop"],
    );
    let mut src = load_from_backup(&fixture("sonarr-4.0.20/backup.zip"), tmp.path())
        .await
        .unwrap();
    let config = &mut src.snapshot.media_management.as_mut().unwrap().0;
    for (k, v) in [
        ("proxyenabled", "True"),
        ("proxytype", "Socks5"),
        ("proxyhostname", "proxy.lan"),
        ("proxyport", "1080"),
        ("proxyusername", "me"),
        ("proxypassword", "secret"),
        ("proxybypassfilter", "*.lan,192.168.0.0/16"),
        ("proxybypasslocaladdresses", "False"),
    ] {
        config.insert(k.into(), v.into());
    }
    let db = fresh_db(tmp.path()).await;
    run(
        &db,
        &src,
        &options(&media),
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    use kitsunarr_domain::api::settings::{ProxySettings, ProxyType};
    let proxy: ProxySettings =
        kitsunarr_db::settings::get_or_default(db.pool(), kitsunarr_db::settings::KEY_PROXY)
            .await
            .unwrap();
    assert_eq!(
        proxy,
        ProxySettings {
            enabled: true,
            proxy_type: ProxyType::Socks5,
            hostname: "proxy.lan".into(),
            port: 1080,
            username: "me".into(),
            password: "secret".into(),
            bypass_filter: "*.lan,192.168.0.0/16".into(),
            bypass_local_addresses: false,
        }
    );
}

#[tokio::test]
async fn missing_media_blocks_apply_but_not_preview() {
    let tmp = tempfile::tempdir().unwrap();
    let src = load_from_backup(&fixture("sonarr-4.0.20/backup.zip"), tmp.path())
        .await
        .unwrap();
    let db = fresh_db(tmp.path()).await;
    let opts = options(&tmp.path().join("does-not-exist"));
    let preview = run(&db, &src, &opts, Mode::DryRun, &noop).await.unwrap();
    assert!(!preview.blocking.is_empty());
    assert!(preview.paths.iter().all(|p| p.exists == Some(false)));
    let err = run(
        &db,
        &src,
        &opts,
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            kitsunarr_migrate::MigrateError::ValidationFailed { .. }
        ),
        "{err}"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM series").await,
        0,
        "nothing committed"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM migration_runs WHERE status = 'failed'"
        )
        .await,
        1,
        "failure recorded"
    );
    // With verification disabled the import proceeds (paths are imported as-is).
    let mut lenient = opts.clone();
    lenient.verify_paths = false;
    run(
        &db,
        &src,
        &lenient,
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM series").await, 3);
}

#[tokio::test]
async fn tanukarr_backup_preserves_provider_identities_and_movies() {
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    make_media(
        &media,
        &[
            "/tv/Breaking Bad",
            "/tv/Chernobyl",
            "/anime/Cowboy Bebop",
            "/tv/Cowboy Bebop",
            "/movies/Cowboy Bebop - The Movie - Knockin' on Heaven's Door",
        ],
    );
    let before = tree(&media);
    let src = load_from_backup(&fixture("tanukarr-0.1.5/backup.zip"), tmp.path())
        .await
        .unwrap();
    assert_eq!(src.detected.app, SourceApp::Tanukarr);
    assert_eq!(src.detected.schema_version, Some(254));
    assert_eq!(src.snapshot.movie_count(), 1);
    let db = fresh_db(tmp.path()).await;
    let out = run(
        &db,
        &src,
        &options(&media),
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    assert!(
        out.validation.iter().all(|c| c.passed),
        "{:?}",
        out.validation
    );

    let series: Vec<(String, String, String)> =
        sqlx::query_as("SELECT metadata_provider, metadata_id, media_kind FROM series ORDER BY metadata_provider, metadata_id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        series,
        vec![
            ("tvMaze".into(), "39892".into(), "series".into()),
            ("tvdb".into(), "360893".into(), "series".into()),
            ("tvdb".into(), "76885".into(), "anime".into()),
            ("tvdb".into(), "81189".into(), "series".into()),
        ]
    );
    let movie: (String, String, String) =
        sqlx::query_as("SELECT title, metadata_provider, metadata_id FROM movies")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        movie,
        (
            "Cowboy Bebop: The Movie - Knockin' on Heaven's Door".into(),
            "aniList".into(),
            "5".into()
        )
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM movie_files").await,
        1,
        "the movie's file becomes a movie file"
    );
    // A negative placeholder TVDB id is never stored as an external identity.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM movie_external_ids WHERE kind = 'tvdb'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM movie_external_ids WHERE kind = 'aniList' AND external_id = '5'"
        )
        .await,
        1
    );
    // Alternate titles from AniList are kept on the movie as its original title.
    let original: Option<String> = sqlx::query_scalar("SELECT original_title FROM movies")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(original.is_some());
    // Tanukarr's per-profile item sizes and codec factors are applied, not just reported.
    let profile = kitsunarr_db::quality::list_profiles(db.pool())
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Kitsunarr Fixture 1080p")
        .unwrap();
    assert_eq!(profile.codec_multipliers.len(), 18);
    assert_eq!(profile.codec_multipliers["HEVC"].0, 1.67);
    let sdtv = profile
        .size_limits
        .iter()
        .find(|l| l.quality_id == 1)
        .unwrap();
    assert_eq!(
        (
            sdtv.min_size.map(|v| v.0),
            sdtv.preferred_size.map(|v| v.0),
            sdtv.max_size.map(|v| v.0)
        ),
        (Some(2.0), Some(95.0), Some(100.0))
    );
    assert_eq!(tree(&media), before, "no filesystem changes");
}

// ---------------------------------------------------------------------------------------------
// Simulated API source: replays captured /api/v3 responses.
// ---------------------------------------------------------------------------------------------

async fn serve_captures(dir: PathBuf) -> String {
    use axum::extract::Request;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let handler = move |req: Request| {
        let dir = dir.clone();
        async move {
            if req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) != Some("fixture-key") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let path = req.uri().path().trim_start_matches("/api/v3/").to_string();
            let query = req.uri().query().unwrap_or_default().to_string();
            let file = match path.as_str() {
                "system/status" => "system-status".to_string(),
                "episode" | "episodefile" => {
                    let id = query
                        .split('&')
                        .find_map(|kv| kv.strip_prefix("seriesId="))
                        .unwrap_or("0");
                    format!("{path}-series{id}")
                }
                "history" if !query.contains("page=1&") => {
                    return axum::Json(serde_json::json!({"records": []})).into_response();
                }
                "config/naming" => "config-naming".into(),
                "config/mediamanagement" => "config-mediamanagement".into(),
                "config/indexer" => "config-indexer".into(),
                other => other.to_string(),
            };
            match std::fs::read_to_string(dir.join(format!("{file}.json"))) {
                Ok(body) => ([("content-type", "application/json")], body).into_response(),
                Err(_) => StatusCode::NOT_FOUND.into_response(),
            }
        }
    };
    let app = axum::Router::new().fallback(handler);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn api_request(url: String) -> ApiSourceRequest {
    ApiSourceRequest {
        url,
        api_key: "fixture-key".into(),
        allow_private_network: true,
    }
}

#[tokio::test]
async fn api_source_matches_backup_source() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve_captures(fixture("sonarr-4.0.20/api")).await;
    let api: LoadedSource = load_from_api(&api_request(url), "kitsunarr-test", &|_, _, _| {})
        .await
        .unwrap();
    let backup = load_from_backup(&fixture("sonarr-4.0.20/backup.zip"), tmp.path())
        .await
        .unwrap();
    assert_eq!(api.detected.app, SourceApp::Sonarr);
    assert!(!api.detected.secrets_available);
    let (a, b) = (&api.snapshot, &backup.snapshot);
    assert_eq!(a.series.len(), b.series.len());
    assert_eq!(a.episodes.len(), b.episodes.len());
    assert_eq!(a.episode_files.len(), b.episode_files.len());
    assert_eq!(
        a.custom_formats, b.custom_formats,
        "custom formats normalise identically from both encodings"
    );
    for (x, y) in a.series.iter().zip(&b.series) {
        assert_eq!(x.identity(), y.identity());
        assert_eq!(x.path, y.path);
        assert_eq!(x.monitored, y.monitored);
        assert_eq!(
            x.seasons
                .iter()
                .map(|s| (s.number, s.monitored))
                .collect::<Vec<_>>(),
            y.seasons
                .iter()
                .map(|s| (s.number, s.monitored))
                .collect::<Vec<_>>()
        );
    }
    let mut ae: Vec<_> = a
        .episodes
        .iter()
        .map(|e| (e.id, e.monitored, e.episode_file_id, e.air_date_utc.clone()))
        .collect();
    let mut be: Vec<_> = b
        .episodes
        .iter()
        .map(|e| (e.id, e.monitored, e.episode_file_id, e.air_date_utc.clone()))
        .collect();
    ae.sort();
    be.sort();
    assert_eq!(ae, be, "episode monitoring, file links and air dates agree");
    let masked: Vec<_> = a
        .providers
        .iter()
        .filter(|p| !p.masked_fields.is_empty())
        .map(|p| p.name.as_str())
        .collect();
    assert!(masked.contains(&"qBittorrent (fixture)"), "{masked:?}");
}

#[tokio::test]
async fn api_source_wrong_key_is_rejected() {
    let url = serve_captures(fixture("sonarr-4.0.20/api")).await;
    let mut req = api_request(url);
    req.api_key = "wrong".into();
    let err = load_from_api(&req, "kitsunarr-test", &|_, _, _| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("API key"), "{err}");
}

#[tokio::test]
async fn tanukarr_api_source_is_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    make_media(
        &media,
        &[
            "/tv/Breaking Bad",
            "/tv/Chernobyl",
            "/anime/Cowboy Bebop",
            "/tv/Cowboy Bebop",
            "/movies/Cowboy Bebop - The Movie - Knockin' on Heaven's Door",
        ],
    );
    let url = serve_captures(fixture("tanukarr-0.1.5/api")).await;
    let src = load_from_api(&api_request(url), "kitsunarr-test", &|_, _, _| {})
        .await
        .unwrap();
    assert_eq!(src.detected.app, SourceApp::Tanukarr);
    assert_eq!(src.detected.product_version.as_deref(), Some("0.1.5"));
    assert_eq!(src.snapshot.movie_count(), 1);
    let db = fresh_db(tmp.path()).await;
    let out = run(
        &db,
        &src,
        &options(&media),
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    assert!(
        out.validation.iter().all(|c| c.passed),
        "{:?}",
        out.validation
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM movies WHERE metadata_provider = 'aniList'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM series WHERE metadata_provider = 'tvMaze'"
        )
        .await,
        1
    );
}

/// Live test against real running instances. Run with
/// `KITSUNARR_TEST_SOURCE_URL=http://... KITSUNARR_TEST_SOURCE_KEY=... cargo test -- --ignored live`.
#[tokio::test]
#[ignore = "requires a running Sonarr/Tanukarr instance"]
async fn live_source_dry_run() {
    let (Ok(url), Ok(key)) = (
        std::env::var("KITSUNARR_TEST_SOURCE_URL"),
        std::env::var("KITSUNARR_TEST_SOURCE_KEY"),
    ) else {
        panic!("set KITSUNARR_TEST_SOURCE_URL and KITSUNARR_TEST_SOURCE_KEY");
    };
    let tmp = tempfile::tempdir().unwrap();
    let src = load_from_api(
        &ApiSourceRequest {
            url,
            api_key: key,
            allow_private_network: true,
        },
        "kitsunarr-test",
        &|_, _, _| {},
    )
    .await
    .unwrap();
    let db = fresh_db(tmp.path()).await;
    let opts = MigrationOptions {
        verify_paths: false,
        ..Default::default()
    };
    let out = run(&db, &src, &opts, Mode::DryRun, &noop).await.unwrap();
    assert!(
        out.validation.iter().all(|c| c.passed),
        "{:?}",
        out.validation
    );
    println!("{:#?}", out.summary);
}
