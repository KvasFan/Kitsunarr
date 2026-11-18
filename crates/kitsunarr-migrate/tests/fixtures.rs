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
async fn sonarr_proxy_settings_and_blocklist_are_imported() {
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
        ("preferredmetadatalanguage", "2"),
        ("autounmonitorpreviouslydownloadedepisodes", "True"),
        // Tanukarr's `MetadataSourceType` numbers: AniList, TheTVDB, TVmaze first; films off.
        ("metadatasourceorder", "2,0,4,99,2"),
        ("disabledmetadatasources", "5,6"),
    ] {
        config.insert(k.into(), v.into());
    }
    // Sonarr's blocklist: an entry of an imported series, and one of a series that is not.
    let series = src.snapshot.series[0].clone();
    let episode = src
        .snapshot
        .episodes
        .iter()
        .find(|e| e.series_id == series.id)
        .unwrap()
        .clone();
    let entry = kitsunarr_migrate::model::SrcBlocklist {
        id: 1,
        series_id: series.id,
        episode_ids: vec![episode.id],
        source_title: "Show.S01E01.720p.HDTV.x264-BAD".into(),
        quality_id: Some(4),
        revision: None,
        languages: vec![1],
        date: "2026-01-01T10:00:00Z".into(),
        published_date: None,
        size: Some(1000),
        protocol: "torrent".into(),
        indexer: Some("Nyaa".into()),
        indexer_flags: 0,
        release_type: Some("SingleEpisode".into()),
        message: Some("Manually marked as failed".into()),
        source: None,
        torrent_info_hash: Some("abc".into()),
    };
    src.snapshot.blocklist = vec![
        entry.clone(),
        kitsunarr_migrate::model::SrcBlocklist {
            id: 2,
            series_id: 999_999,
            ..entry
        },
    ];
    let db = fresh_db(tmp.path()).await;
    for _ in 0..2 {
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
    }
    // Imported once (the second run skips it), with the episode mapped; the other is skipped.
    let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT source_title, episode_ids, protocol, torrent_info_hash FROM blocklist",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let episode_id: i64 = sqlx::query_scalar(
        "SELECT e.id FROM episodes e JOIN series s ON s.id = e.series_id WHERE s.title = ? AND e.season_number = ? AND e.episode_number = ?",
    )
    .bind(&series.title)
    .bind(episode.season_number)
    .bind(episode.episode_number)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(rows[0].1, format!("[{episode_id}]"));
    assert_eq!(
        (rows[0].2.as_str(), rows[0].3.as_deref()),
        ("torrent", Some("abc"))
    );
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
    let sources: kitsunarr_domain::api::settings::MetadataSourcesSettings =
        kitsunarr_db::settings::get_or_default(
            db.pool(),
            kitsunarr_db::settings::KEY_METADATA_SOURCES,
        )
        .await
        .unwrap();
    assert_eq!(sources.preferred_metadata_language, Some(2));
    use kitsunarr_domain::MetadataProvider as P;
    assert_eq!(sources.source_order, [P::AniList, P::Tvdb, P::TvMaze]);
    assert_eq!(sources.disabled_sources, [P::TmdbMovie, P::Omdb]);
    assert_eq!(
        kitsunarr_domain::api::metadata::enabled_sources(&sources),
        [P::AniList, P::Tvdb, P::TvMaze, P::TmdbTv, P::MyAnimeList]
    );
    let media: kitsunarr_domain::api::settings::MediaManagementSettings =
        kitsunarr_db::settings::get_or_default(
            db.pool(),
            kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
        )
        .await
        .unwrap();
    assert!(media.auto_unmonitor_previously_downloaded_episodes);
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

/// Tanukarr's retained episode versions come over with their record and episodes; the archived
/// files are left where they are.
#[tokio::test]
async fn tanukarr_retained_versions_are_imported() {
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
    // The backup's database, with a version of Breaking Bad S01E01 (episode 19) and an episode
    // that no longer exists.
    let file = std::fs::File::open(fixture("tanukarr-0.1.5/backup.zip")).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let db_path = tmp.path().join("sonarr.db");
    std::io::copy(
        &mut archive.by_name("sonarr.db").unwrap(),
        &mut std::fs::File::create(&db_path).unwrap(),
    )
    .unwrap();
    let source = sqlx::SqlitePool::connect(&format!("sqlite://{}", db_path.display()))
        .await
        .unwrap();
    let metadata = serde_json::json!({
        "seriesId": 1, "seasonNumber": 1, "relativePath": "Season 01/Breaking.Bad.S01E01.720p.HDTV.x264-OLD.mkv",
        "size": 123, "dateAdded": "2026-10-01T10:00:00Z", "sceneName": "Breaking.Bad.S01E01.720p.HDTV.x264-OLD",
        "releaseGroup": "OLD",
        "quality": {"quality": {"id": 4, "name": "HDTV-720p", "source": "television", "resolution": 720},
                    "revision": {"version": 2, "real": 0, "isRepack": true}},
        "indexerFlags": 0, "languages": [{"id": 1, "name": "English"}], "releaseType": "singleEpisode",
        "mediaInfo": {"schemaRevision": 14, "width": 1280, "height": 720, "runTime": "00:47:00"}
    });
    sqlx::query("INSERT INTO RetainedVersions (SeriesId, RelativePath, MetadataJson, EpisodeIdsJson, Created) VALUES (1, ?, ?, '[19, 999]', '2026-10-02 10:00:00Z')")
        .bind("Plex Versions/0123abcd/Breaking.Bad.S01E01.720p.HDTV.x264-OLD.mkv")
        .bind(metadata.to_string())
        .execute(&source)
        .await
        .unwrap();
    source.close().await;

    let src = load_from_backup(&db_path, tmp.path()).await.unwrap();
    assert_eq!(src.snapshot.retained_versions.len(), 1);
    let db = fresh_db(tmp.path()).await;
    for _ in 0..2 {
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
    }
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT series_id, relative_path, archive_path, episode_ids FROM retained_versions",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "a second run does not duplicate it");
    let (series_id, relative, archive, episodes) = &rows[0];
    let breaking_bad: i64 =
        sqlx::query_scalar("SELECT id FROM series WHERE title = 'Breaking Bad'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(*series_id, breaking_bad);
    // Tanukarr's `Series.Language` for a TheTVDB series.
    let language: Option<i64> =
        sqlx::query_scalar("SELECT metadata_language FROM series WHERE id = ?")
            .bind(breaking_bad)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(language, Some(1));
    assert_eq!(
        relative,
        "Season 01/Breaking.Bad.S01E01.720p.HDTV.x264-OLD.mkv"
    );
    assert_eq!(
        archive,
        "Plex Versions/0123abcd/Breaking.Bad.S01E01.720p.HDTV.x264-OLD.mkv"
    );
    let e1: i64 = sqlx::query_scalar(
        "SELECT id FROM episodes WHERE series_id = ? AND season_number = 1 AND episode_number = 1",
    )
    .bind(breaking_bad)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(episodes, &format!("[{e1}]"));
    // The record snapshot is in Kitsunarr's shape (as `versions::preserve` writes it).
    let metadata: String = sqlx::query_scalar("SELECT metadata FROM retained_versions")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let m: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(m["releaseGroup"], "OLD");
    assert_eq!(m["qualityId"], 4);
    assert_eq!(
        (m["revisionVersion"].clone(), m["isRepack"].clone()),
        (2.into(), true.into())
    );
    assert_eq!(m["languages"], serde_json::json!([1]));
    assert_eq!(m["releaseType"], 1);
    assert_eq!(m["mediaInfo"]["height"], 720);
    assert_eq!(m["addedAt"], "2026-10-01T10:00:00Z");
}

/// Tanukarr's `AnimeGroupingMode.Disabled` keeps titles out of automatic grouping after the
/// migration, and its `SourceReleaseTitle`/`SourceReleaseSize` (migration 237) stay on the
/// imported files and films.
#[tokio::test]
async fn tanukarr_disabled_grouping_and_source_releases_are_imported() {
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
    let file = std::fs::File::open(fixture("tanukarr-0.1.5/backup.zip")).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let db_path = tmp.path().join("sonarr.db");
    std::io::copy(
        &mut archive.by_name("sonarr.db").unwrap(),
        &mut std::fs::File::create(&db_path).unwrap(),
    )
    .unwrap();
    let source = sqlx::SqlitePool::connect(&format!("sqlite://{}", db_path.display()))
        .await
        .unwrap();
    // Disabled grouping clears the group (Tanukarr's `Series.ApplyChanges`): only the mode is
    // left. Series 3 is the anime Cowboy Bebop, series 4 the film.
    sqlx::query("UPDATE Series SET AnimeGroupingMode = 2, AnimeGroupId = NULL WHERE Id IN (3, 4)")
        .execute(&source)
        .await
        .unwrap();
    sqlx::query("UPDATE EpisodeFiles SET SourceReleaseSize = 30000000000, SourceReleaseTitle = 'Breaking.Bad.S01.1080p.BluRay.x264-ROVERS' WHERE SeriesId = 1")
        .execute(&source)
        .await
        .unwrap();
    sqlx::query("UPDATE EpisodeFiles SET SourceReleaseSize = 0, SourceReleaseTitle = 'ignored' WHERE SeriesId = 2")
        .execute(&source)
        .await
        .unwrap();
    sqlx::query("UPDATE EpisodeFiles SET SourceReleaseSize = 45000000000, SourceReleaseTitle = 'Cowboy.Bebop.Collection.1080p.BluRay-GRP' WHERE Id = 15")
        .execute(&source)
        .await
        .unwrap();
    source.close().await;

    let src = load_from_backup(&db_path, tmp.path()).await.unwrap();
    let db = fresh_db(tmp.path()).await;
    for _ in 0..2 {
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
    }
    let excluded: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT s.path, m.title FROM collection_grouping_exclusions x \
         LEFT JOIN series s ON s.id = x.series_id LEFT JOIN movies m ON m.id = x.movie_id ORDER BY s.path, m.title",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(excluded.len(), 2, "{excluded:?}");
    assert!(
        excluded.iter().any(|(path, _)| path
            .as_deref()
            .is_some_and(|p| p.ends_with("/anime/Cowboy Bebop"))),
        "{excluded:?}"
    );
    assert!(
        excluded.iter().any(|(_, title)| title.is_some()),
        "{excluded:?}"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM collection_members m JOIN collections c ON c.id = m.collection_id WHERE c.kind = 'animeGroup' AND m.series_id IN (SELECT series_id FROM collection_grouping_exclusions)").await,
        0
    );
    let sources: Vec<(String, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT relative_path, source_release_size, source_release_title FROM episode_files ORDER BY relative_path",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    let breaking_bad = sources
        .iter()
        .find(|(p, _, _)| p.contains("Breaking.Bad.S01E01"))
        .unwrap();
    assert_eq!(
        (breaking_bad.1, breaking_bad.2.as_deref()),
        (
            Some(30_000_000_000),
            Some("Breaking.Bad.S01.1080p.BluRay.x264-ROVERS")
        )
    );
    // A size of 0 is no source release.
    let chernobyl = sources
        .iter()
        .find(|(p, _, _)| p.contains("Chernobyl.S01E01"))
        .unwrap();
    assert_eq!((chernobyl.1, chernobyl.2.as_deref()), (None, None));
    let film: (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT source_release_size, source_release_title FROM movie_files")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        film,
        (
            Some(45_000_000_000),
            Some("Cowboy.Bebop.Collection.1080p.BluRay-GRP".into())
        )
    );
}

#[tokio::test]
async fn auto_tagging_and_original_country_are_imported() {
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
    let file = std::fs::File::open(fixture("tanukarr-0.1.5/backup.zip")).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let db_path = tmp.path().join("sonarr.db");
    std::io::copy(
        &mut archive.by_name("sonarr.db").unwrap(),
        &mut std::fs::File::create(&db_path).unwrap(),
    )
    .unwrap();
    let source = sqlx::SqlitePool::connect(&format!("sqlite://{}", db_path.display()))
        .await
        .unwrap();
    // Sonarr's stored shape: `{type, body}` conditions, tag ids, integer enums.
    let specifications = serde_json::json!([
        {"type": "QualityProfileSpecification", "body": {"name": "Fixture profile", "value": 7, "negate": false, "required": false}},
        {"type": "OriginalCountrySpecification", "body": {"name": "American", "value": ["usa"], "negate": false, "required": false}},
        {"type": "RootFolderSpecification", "body": {"name": "TV folder", "value": "/media/tv", "negate": false, "required": true}},
        {"type": "TagSpecification", "body": {"name": "Requested", "value": 4, "negate": true, "required": false}}
    ]);
    sqlx::query("INSERT INTO AutoTagging (Name, Specifications, RemoveTagsAutomatically, Tags) VALUES ('US drama', ?, 1, '[3]')")
        .bind(specifications.to_string())
        .execute(&source)
        .await
        .unwrap();
    // Kodi metadata turned on with episode thumbs in the NFO; Plex with episode mappings.
    sqlx::query("UPDATE Metadata SET Enable = 1, Settings = '{\"seriesMetadata\": true, \"seriesMetadataEpisodeGuide\": false, \"seriesMetadataUrl\": false, \"episodeMetadata\": true, \"episodeImageThumb\": true, \"seriesImages\": false, \"seasonImages\": true, \"episodeImages\": true}' WHERE Implementation = 'XbmcMetadata'")
        .execute(&source)
        .await
        .unwrap();
    sqlx::query("UPDATE Metadata SET Enable = 1, Settings = '{\"seriesPlexMatchFile\": true, \"episodeMappings\": true}' WHERE Implementation = 'PlexMetadata'")
        .execute(&source)
        .await
        .unwrap();
    sqlx::query("INSERT INTO Config (Key, Value) VALUES ('certificatevalidation', 'DisabledForLocalAddresses'), ('uilanguage', '4')")
        .execute(&source)
        .await
        .unwrap();
    source.close().await;

    let src = load_from_backup(&db_path, tmp.path()).await.unwrap();
    assert_eq!(src.snapshot.auto_tags.len(), 1);
    let db = fresh_db(tmp.path()).await;
    for _ in 0..2 {
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
    }
    let rules = kitsunarr_db::auto_tags::list(db.pool()).await.unwrap();
    assert_eq!(rules.len(), 1, "a second run does not duplicate it");
    let rule = &rules[0];
    assert!(rule.remove_tags_automatically);
    let tag_ids: std::collections::HashMap<String, i64> =
        sqlx::query_as::<_, (String, i64)>("SELECT label, id FROM tags")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .collect();
    let tag = |label: &str| std::future::ready(tag_ids[label]);
    assert_eq!(rule.tags, vec![tag("kids").await]);
    let profile: i64 = sqlx::query_scalar(
        "SELECT id FROM quality_profiles WHERE name = 'Kitsunarr Fixture 1080p'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let field = |i: usize, k: &str| rule.specifications[i].fields[k].clone();
    assert_eq!(field(0, "value"), serde_json::json!(profile));
    assert_eq!(field(1, "value"), serde_json::json!(["usa"]));
    assert_eq!(
        field(2, "value"),
        serde_json::json!(media.join("tv").display().to_string())
    );
    assert!(rule.specifications[2].required);
    assert_eq!(field(3, "value"), serde_json::json!(tag("requested").await));
    assert!(rule.specifications[3].negate);

    // Sonarr's metadata consumers become Settings › Metadata's.
    let consumers: kitsunarr_domain::api::settings::MetadataConsumersSettings =
        kitsunarr_db::settings::get_or_default(
            db.pool(),
            kitsunarr_db::settings::KEY_METADATA_CONSUMERS,
        )
        .await
        .unwrap();
    assert!(consumers.kodi.enable && consumers.kodi.episode_image_thumb);
    assert!(!consumers.kodi.series_images);
    assert!(consumers.plex.enable && consumers.plex.episode_mappings);
    assert!(!consumers.wdtv.enable && !consumers.roksbox.enable);
    let general: kitsunarr_domain::api::settings::GeneralSettings =
        kitsunarr_db::settings::get_or_default(db.pool(), kitsunarr_db::settings::KEY_GENERAL)
            .await
            .unwrap();
    assert_eq!(
        general.certificate_validation,
        kitsunarr_domain::api::settings::CertificateValidation::DisabledForLocalAddresses
    );
    // Sonarr's interface language (4 = German).
    assert_eq!(general.ui_language, "de");

    // Tanukarr's `OriginalCountry` is kept, from SkyHook ("usa") and AniList ("JP").
    let countries: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT title, original_country FROM series ORDER BY id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert!(countries.contains(&("Breaking Bad".into(), Some("usa".into()))));
    let film: Option<String> = sqlx::query_scalar("SELECT original_country FROM movies")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(film.as_deref(), Some("JP"));

    // The imported rule tags Breaking Bad (American, profile 7, in the TV folder, not
    // requested) when it is next edited; Chernobyl is requested.
    let mut conn = db.pool().acquire().await.unwrap();
    let ids: Vec<(i64, String)> = sqlx::query_as("SELECT id, title FROM series")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    for (id, _) in &ids {
        kitsunarr_db::auto_tags::apply_series(&mut conn, *id)
            .await
            .unwrap();
    }
    let tagged: Vec<String> = sqlx::query_scalar(
        "SELECT s.title FROM series s JOIN series_tags t ON t.series_id = s.id WHERE t.tag_id = ?",
    )
    .bind(tag("kids").await)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(tagged, vec!["Breaking Bad".to_string()]);
}

/// Tanukarr's alternative groups and "Ignore episode titles" survive a backup migration and
/// keep their meaning: `1080p AND (x265 OR group GRP)`, with episode titles ignored by the
/// x265 condition.
#[tokio::test]
async fn tanukarr_custom_format_alternative_groups_are_imported() {
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
    let file = std::fs::File::open(fixture("tanukarr-0.1.5/backup.zip")).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let db_path = tmp.path().join("sonarr.db");
    std::io::copy(
        &mut archive.by_name("sonarr.db").unwrap(),
        &mut std::fs::File::create(&db_path).unwrap(),
    )
    .unwrap();
    let source = sqlx::SqlitePool::connect(&format!("sqlite://{}", db_path.display()))
        .await
        .unwrap();
    // Tanukarr 0.1.5's stored shape (as in the fixture's own formats).
    let specifications = serde_json::json!([
        {"type": "ReleaseTitleSpecification", "body": {"order": 1, "implementationName": "Release Title",
            "infoLink": "https://wiki.servarr.com/sonarr/settings#custom-formats-2", "excludeEpisodeTitles": false,
            "value": "1080p", "name": "1080p", "negate": false, "required": false, "alternativeGroup": 0}},
        {"type": "ReleaseTitleSpecification", "body": {"order": 1, "implementationName": "Release Title",
            "infoLink": "https://wiki.servarr.com/sonarr/settings#custom-formats-2", "excludeEpisodeTitles": true,
            "value": "\\bx265\\b", "name": "x265", "negate": false, "required": false, "alternativeGroup": 1}},
        {"type": "ReleaseGroupSpecification", "body": {"order": 9, "implementationName": "Release Group",
            "infoLink": "https://wiki.servarr.com/sonarr/settings#custom-formats-2",
            "value": "^GRP$", "name": "GRP", "negate": false, "required": false, "alternativeGroup": 2}}
    ]);
    sqlx::query("INSERT INTO CustomFormats (Name, Specifications, IncludeCustomFormatWhenRenaming) VALUES ('Grouped', ?, 0)")
        .bind(specifications.to_string())
        .execute(&source)
        .await
        .unwrap();
    source.close().await;

    let src = load_from_backup(&db_path, tmp.path()).await.unwrap();
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
    let formats = kitsunarr_db::quality::list_custom_formats(db.pool())
        .await
        .unwrap();
    let grouped = formats.iter().find(|f| f.name == "Grouped").unwrap();
    let groups: Vec<i64> = grouped
        .specifications
        .iter()
        .map(|s| s.alternative_group())
        .collect();
    assert_eq!(groups, vec![0, 1, 2]);
    assert!(grouped.specifications[1].flag("excludeEpisodeTitles"));
    assert!(!grouped.specifications[0].flag("excludeEpisodeTitles"));

    let (compiled, problems) = kitsunarr_decision::compile_formats(std::slice::from_ref(grouped));
    assert!(problems.is_empty(), "{problems:?}");
    let titles = vec!["The x265 Affair".to_string()];
    let check = |title: &str, group: Option<&str>, titles: &[String]| {
        compiled[0].matches(&kitsunarr_decision::CustomFormatInput {
            release_title: title,
            file_name: None,
            release_group: group,
            quality: kitsunarr_domain::Quality::unknown(),
            size: 0,
            indexer_flags: 0,
            languages: &[],
            release_type: kitsunarr_parser::ReleaseType::SingleEpisode,
            original_language: None,
            episode_titles: titles,
        })
    };
    assert!(check("Show.S01E01.1080p.x265-OTHER", Some("OTHER"), &[]));
    assert!(check("Show.S01E01.1080p.x264-GRP", Some("GRP"), &[]));
    assert!(!check("Show.S01E01.1080p.x264-OTHER", Some("OTHER"), &[]));
    assert!(!check("Show.S01E01.720p.x265-GRP", Some("GRP"), &[]));
    // "x265" only in the episode title does not count.
    assert!(!check(
        "Show.S01E01.The.x265.Affair.1080p.x264-OTHER",
        Some("OTHER"),
        &titles
    ));
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

#[tokio::test]
async fn a_tanukarr_films_watched_feature_is_imported_without_overriding_a_local_choice() {
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
    let mut source = load_from_backup(&fixture("tanukarr-0.1.5/backup.zip"), tmp.path())
        .await
        .unwrap();
    let film_id = source
        .snapshot
        .series
        .iter()
        .find(|s| s.media_type == kitsunarr_migrate::model::SrcMediaType::Movie)
        .unwrap()
        .id;
    let episode = source
        .snapshot
        .episodes
        .iter_mut()
        .find(|e| e.series_id == film_id && e.season_number == 1 && e.episode_number == 1)
        .unwrap();
    episode
        .unmapped
        .insert("watched".into(), serde_json::json!(true));
    episode.unmapped.insert(
        "watchedatutc".into(),
        serde_json::json!("2026-11-14T21:00:00Z"),
    );
    let db = fresh_db(tmp.path()).await;
    let mut opts = options(&media);
    run(
        &db,
        &source,
        &opts,
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    let (id, watched, at, owner): (i64, bool, Option<String>, Option<String>) =
        sqlx::query_as("SELECT id,watched,watched_at,watched_source FROM movies")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(watched);
    assert_eq!(at.as_deref(), Some("2026-11-14T21:00:00Z"));
    assert_eq!(owner.as_deref(), Some("migrated"));
    kitsunarr_db::library::movies::set_watched(&db, &[id], false)
        .await
        .unwrap();
    opts.conflict_policy = ConflictPolicy::Update;
    run(
        &db,
        &source,
        &opts,
        Mode::Apply {
            backup_dir: tmp.path().join("b"),
        },
        &noop,
    )
    .await
    .unwrap();
    assert!(
        !kitsunarr_db::library::movies::get_detail(&db, id)
            .await
            .unwrap()
            .item
            .watched
    );
    assert_eq!(tree(&media), before);
}

/// Sonarr's release profile air-date options and excluded tags, and the media management
/// settings Kitsunarr now has (`Config` keys, as the database keeps them: lower-case keys,
/// enum names), come over from a Tanukarr backup.
#[tokio::test]
async fn tanukarr_release_profile_options_and_media_management_are_imported() {
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
    let file = std::fs::File::open(fixture("tanukarr-0.1.5/backup.zip")).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let db_path = tmp.path().join("sonarr.db");
    std::io::copy(
        &mut archive.by_name("sonarr.db").unwrap(),
        &mut std::fs::File::create(&db_path).unwrap(),
    )
    .unwrap();
    let source = sqlx::SqlitePool::connect(&format!("sqlite://{}", db_path.display()))
        .await
        .unwrap();
    // Tag 3 is "kids".
    sqlx::query(
        "UPDATE ReleaseProfiles SET AirDateRestriction = 1, AirDateGracePeriod = -6, \
         AllowSeasonPackWithoutAllEpisodesAired = 1, ExcludedTags = '[3]' WHERE Id = 1",
    )
    .execute(&source)
    .await
    .unwrap();
    for (key, value) in [
        ("rescanafterrefresh", "AfterManual"),
        ("episodetitlerequired", "BulkSeasonReleases"),
        ("seasonpackupgrade", "Threshold"),
        ("seasonpackupgradethreshold", "75.5"),
        ("chowngroup", "media"),
        ("userrejectedextensions", "exe,bat"),
        ("usescriptimport", "True"),
        ("scriptimportpath", "/media/scripts/import.sh"),
        ("skipfreespacecheckwhengrabbing", "True"),
        ("filedate", "PreserveOriginal"),
        ("createemptyseriesfolders", "True"),
    ] {
        sqlx::query("INSERT INTO Config (Key, Value) VALUES (?, ?)")
            .bind(key)
            .bind(value)
            .execute(&source)
            .await
            .unwrap();
    }
    source.close().await;

    let src = load_from_backup(&db_path, tmp.path()).await.unwrap();
    for key in ["rescanafterrefresh", "seasonpackupgrade", "chowngroup"] {
        assert!(
            !src.snapshot.unmapped_settings.contains_key(key),
            "{key} is mapped"
        );
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

    let profiles = kitsunarr_db::rules::list_release_profiles(db.pool())
        .await
        .unwrap();
    let p = profiles
        .iter()
        .find(|p| p.name.as_deref() == Some("No CAM"))
        .unwrap();
    let kids: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE label = 'kids'")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        (
            p.air_date_restriction,
            p.air_date_grace_period,
            p.allow_season_pack_without_all_episodes_aired,
            p.excluded_tags.clone(),
            p.indexer_ids.clone()
        ),
        (true, -6, true, vec![kids], vec![])
    );

    use kitsunarr_domain::api::settings::{
        EpisodeTitleRequired, FileDate, MediaManagementSettings, RescanAfterRefresh,
        SeasonPackUpgrade,
    };
    let m: MediaManagementSettings = kitsunarr_db::settings::get_or_default(
        db.pool(),
        kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
    )
    .await
    .unwrap();
    assert_eq!(m.rescan_after_refresh, RescanAfterRefresh::AfterManual);
    assert_eq!(
        m.episode_title_required,
        EpisodeTitleRequired::BulkSeasonReleases
    );
    assert_eq!(m.season_pack_upgrade, SeasonPackUpgrade::Threshold);
    assert_eq!(m.season_pack_upgrade_threshold.0, 75.5);
    assert_eq!(m.chown_group, "media");
    assert_eq!(m.user_rejected_extensions, "exe,bat");
    assert!(m.use_script_import);
    assert_eq!(
        m.script_import_path,
        media.join("scripts/import.sh").display().to_string()
    );
    assert!(m.skip_free_space_check_when_grabbing);
    assert!(!m.skip_free_space_check);
    assert_eq!(m.file_date, FileDate::PreserveOriginal);
    assert!(m.create_empty_series_folders);
    assert!(m.delete_empty_folders);
}
