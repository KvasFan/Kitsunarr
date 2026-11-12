//! Library check and recycle-bin restore on a real folder tree.

use base64::Engine as _;
use kitsunarr_db::Database;
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::settings::MediaManagementSettings;

#[tokio::test]
async fn check_finds_missing_files_and_restore_brings_a_recycled_episode_back() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tv");
    let series_path = root.join("Show");
    let bin = dir.path().join("recycle");
    std::fs::create_dir_all(series_path.join("Season 01")).unwrap();
    std::fs::create_dir_all(bin.join("Show/Season 01")).unwrap();
    std::fs::write(bin.join("Show/Season 01/Show - S01E02 - Two.mkv"), b"two").unwrap();
    std::fs::write(bin.join("Show/notes.txt"), b"not media").unwrap();

    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let mut media: MediaManagementSettings =
        kv::get_or_default(db.pool(), kv::KEY_MEDIA_MANAGEMENT)
            .await
            .unwrap();
    media.recycle_bin = Some(bin.display().to_string());
    kv::set(db.pool(), kv::KEY_MEDIA_MANAGEMENT, &media)
        .await
        .unwrap();
    kitsunarr_db::root_folders::create(db.pool(), &root.display().to_string(), None)
        .await
        .unwrap();
    let pool = db.pool();
    sqlx::query(
        "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, metadata_provider, metadata_id, \
         quality_profile_id, root_folder_path, path, added_at) VALUES (1, 'series', 'Show', 'show', 'show', 'show', \
         'tvMaze', '1', 1, ?, ?, '2026-11-01T00:00:00Z')",
    )
    .bind(root.display().to_string())
    .bind(series_path.display().to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO seasons (series_id, season_number, monitored) VALUES (1, 1, 1)")
        .execute(pool)
        .await
        .unwrap();
    for n in 1..=2 {
        sqlx::query("INSERT INTO episodes (series_id, season_number, episode_number, title) VALUES (1, 1, ?, 'x')")
            .bind(n)
            .execute(pool)
            .await
            .unwrap();
    }
    // A record of a file that is gone.
    sqlx::query("INSERT INTO episode_files (id, series_id, season_number, relative_path, added_at) VALUES (9, 1, 1, 'Season 01/Show - S01E01.mkv', '2026-11-01T00:00:00Z')")
        .execute(pool)
        .await
        .unwrap();

    let check = kitsunarr_library::tools::check_library(&db).await.unwrap();
    assert_eq!((check.series_checked, check.files_checked), (1, 1));
    assert_eq!(check.issues.len(), 1);
    assert_eq!(check.issues[0].problem, "Missing on disk");

    let listed = kitsunarr_library::tools::browse_recycle_bin(&db)
        .await
        .unwrap();
    assert!(listed.configured);
    assert_eq!(listed.files.len(), 1, "only media and subtitles");
    let token = listed.files[0].token.clone();
    kitsunarr_library::tools::restore_recycled(&db, &token, 1, 1)
        .await
        .unwrap();
    // Sonarr's default season folder format is "Season {season}".
    let restored = series_path.join("Season 1/Show - S01E02 - Two.mkv");
    assert_eq!(std::fs::read(&restored).unwrap(), b"two");
    assert!(!bin.join("Show/Season 01/Show - S01E02 - Two.mkv").exists());
    let linked: Option<i64> = sqlx::query_scalar(
        "SELECT episode_file_id FROM episodes WHERE series_id = 1 AND episode_number = 2",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(linked.is_some(), "the rescan registers the restored file");

    // Never overwritten; tokens outside the bin are refused.
    std::fs::write(bin.join("Show/Season 01/Show - S01E02 - Two.mkv"), b"again").unwrap();
    let err = kitsunarr_library::tools::restore_recycled(&db, &token, 1, 1)
        .await
        .unwrap_err();
    assert!(err.contains("will not be overwritten"), "{err}");
    let outside = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("../k.db");
    assert!(
        kitsunarr_library::tools::restore_recycled(&db, &outside, 1, 1)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn kept_versions_can_be_made_current_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tv");
    let show = root.join("Show");
    std::fs::create_dir_all(show.join("Season 1")).unwrap();
    std::fs::write(show.join("Season 1/Show - S01E01 - WEB.mkv"), b"old web").unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let pool = db.pool();
    sqlx::query(
        "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, metadata_provider, metadata_id, \
         quality_profile_id, root_folder_path, path, added_at, keep_versions) VALUES (1, 'series', 'Show', 'show', 'show', 'show', \
         'tvMaze', '1', 1, ?, ?, '2026-11-01T00:00:00Z', 1)",
    )
    .bind(root.display().to_string())
    .bind(show.display().to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO seasons (series_id, season_number, monitored) VALUES (1, 1, 1)")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO episode_files (id, series_id, season_number, relative_path, size, quality_id, added_at, release_group) VALUES (1, 1, 1, 'Season 1/Show - S01E01 - WEB.mkv', 7, 3, '2026-11-01T00:00:00Z', 'WEBGRP')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO episodes (id, series_id, season_number, episode_number, episode_file_id) VALUES (5, 1, 1, 1, 1)")
        .execute(pool)
        .await
        .unwrap();

    // An upgrade keeps the WEB file, then the new Bluray file takes its record's place.
    kitsunarr_library::versions::preserve(&db, 1, &show, 1, &[5])
        .await
        .unwrap();
    assert!(!show.join("Season 1/Show - S01E01 - WEB.mkv").exists());
    std::fs::write(
        show.join("Season 1/Show - S01E01 - Bluray.mkv"),
        b"new bluray",
    )
    .unwrap();
    sqlx::query("UPDATE episode_files SET relative_path = 'Season 1/Show - S01E01 - Bluray.mkv', size = 10, quality_id = 7, release_group = 'BDGRP' WHERE id = 1")
        .execute(pool)
        .await
        .unwrap();
    let versions = kitsunarr_library::versions::list(&db, 1).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert!(versions[0].available && versions[0].archive_path.starts_with("Plex Versions/"));
    assert_eq!(versions[0].release_group.as_deref(), Some("WEBGRP"));

    // Making the WEB file current: it is copied back under the same record, the Bluray file
    // becomes a version, and the archive stays.
    kitsunarr_library::versions::activate(&db, 1, versions[0].id)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(show.join("Season 1/Show - S01E01 - WEB.mkv")).unwrap(),
        b"old web"
    );
    assert!(!show.join("Season 1/Show - S01E01 - Bluray.mkv").exists());
    let (path, group, quality): (String, Option<String>, i64) = sqlx::query_as(
        "SELECT relative_path, release_group, quality_id FROM episode_files WHERE id = 1",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        (path.as_str(), group.as_deref(), quality),
        ("Season 1/Show - S01E01 - WEB.mkv", Some("WEBGRP"), 3)
    );
    let versions = kitsunarr_library::versions::list(&db, 1).await.unwrap();
    assert_eq!(versions.len(), 2);
    assert!(versions.iter().all(|v| v.available));
    // Deleting a version removes its file; disk scans never see "Plex Versions".
    let bluray = versions
        .iter()
        .find(|v| v.release_group.as_deref() == Some("BDGRP"))
        .unwrap();
    kitsunarr_library::versions::delete(&db, 1, bluray.id)
        .await
        .unwrap();
    assert_eq!(
        kitsunarr_library::versions::list(&db, 1)
            .await
            .unwrap()
            .len(),
        1
    );
    let scan = kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    assert_eq!(
        scan.added, 0,
        "kept versions are not imported by disk scans"
    );
}

#[tokio::test]
async fn manual_import_matches_files_and_imports_the_chosen_episodes() {
    use kitsunarr_domain::api::library_tools::ManualImportFile;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tv");
    let show = root.join("Show");
    std::fs::create_dir_all(&show).unwrap();
    let incoming = dir.path().join("incoming");
    std::fs::create_dir_all(&incoming).unwrap();
    std::fs::write(incoming.join("Show.S01E02.720p.HDTV.x264-GRP.mkv"), b"ep2").unwrap();
    std::fs::write(
        incoming.join("Unknown.Thing.S03E04.720p.HDTV.x264-GRP.mkv"),
        b"x",
    )
    .unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let pool = db.pool();
    sqlx::query(
        "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, metadata_provider, metadata_id, \
         quality_profile_id, root_folder_path, path, added_at) VALUES (1, 'series', 'Show', 'show', 'show', 'show', \
         'tvMaze', '1', 1, ?, ?, '2026-11-01T00:00:00Z')",
    )
    .bind(root.display().to_string())
    .bind(show.display().to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO seasons (series_id, season_number, monitored) VALUES (1, 1, 1)")
        .execute(pool)
        .await
        .unwrap();
    for n in 1..=3 {
        sqlx::query("INSERT INTO episodes (id, series_id, season_number, episode_number, title, air_date_utc) VALUES (?, 1, 1, ?, 'x', '2020-01-01T00:00:00Z')")
            .bind(n)
            .bind(n)
            .execute(pool)
            .await
            .unwrap();
    }

    let items = kitsunarr_library::manual_import::scan(&db, &incoming, None)
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    let ep2 = items.iter().find(|i| i.name.starts_with("Show.")).unwrap();
    assert_eq!(ep2.series_id, Some(1));
    assert_eq!(ep2.episodes, vec!["S01E02"]);
    assert!(ep2.rejections.is_empty(), "{:?}", ep2.rejections);
    let unknown = items
        .iter()
        .find(|i| i.name.starts_with("Unknown"))
        .unwrap();
    assert_eq!(unknown.rejections, vec!["Unknown series"]);

    // A webhook hears about the import (`OnDownload`).
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let h = heard.clone();
    let hook = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |axum::Json(v): axum::Json<serde_json::Value>| {
            let h = h.clone();
            async move {
                h.lock().unwrap().push(v);
                "ok"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, hook).await.unwrap() });
    sqlx::query("INSERT INTO notifications (name, implementation, settings, triggers, created_at) VALUES ('Hook', 'Webhook', ?, '{\"onDownload\": true}', '2026-11-01T00:00:00Z')")
        .bind(serde_json::json!({"url": hook_url, "method": 1}).to_string())
        .execute(pool)
        .await
        .unwrap();
    let notifier = kitsunarr_notifications::Notifier::new(reqwest::Client::new());

    // Imported as episode 3 by hand: the choice wins over the name.
    let results = kitsunarr_library::manual_import::import(
        &db,
        &notifier,
        &[ManualImportFile {
            path: ep2.path.clone(),
            series_id: 1,
            episode_ids: vec![3],
        }],
        false,
    )
    .await;
    let outcome = results[0].1.as_ref().unwrap();
    assert!(outcome.results.iter().any(|f| f.imported), "{outcome:?}");
    let linked: Option<i64> =
        sqlx::query_scalar("SELECT episode_file_id FROM episodes WHERE id = 3")
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(linked.is_some());
    assert!(
        incoming.join("Show.S01E02.720p.HDTV.x264-GRP.mkv").exists(),
        "copied, not moved"
    );
    // Episodes of another series are refused.
    let bad = kitsunarr_library::manual_import::import(
        &db,
        &notifier,
        &[ManualImportFile {
            path: ep2.path.clone(),
            series_id: 1,
            episode_ids: vec![99],
        }],
        false,
    )
    .await;
    assert!(bad[0].1.is_err());
    for _ in 0..50 {
        if !heard.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0]["eventType"], "Download");
    assert_eq!(heard[0]["downloadClient"], "Manual Import");
    assert_eq!(heard[0]["episodes"][0]["episodeNumber"], 3);
}

/// `RecycleBinProvider.Cleanup`: files recycled longer ago than the setting are deleted with
/// the folders they leave empty; recent ones stay; 0 days turns it off.
#[tokio::test]
async fn the_recycle_bin_is_cleaned_after_the_configured_days() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(bin.join("Show/Season 01")).unwrap();
    let old = bin.join("Show/Season 01/old.mkv");
    let fresh = bin.join("fresh.mkv");
    std::fs::write(&old, b"x").unwrap();
    std::fs::write(&fresh, b"y").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86_400))
        .unwrap();
    let mut media: kitsunarr_domain::api::settings::MediaManagementSettings =
        kv::get_or_default(db.pool(), kv::KEY_MEDIA_MANAGEMENT)
            .await
            .unwrap();
    assert_eq!(
        kitsunarr_library::tools::clean_recycle_bin(&db)
            .await
            .unwrap(),
        "The recycle bin is not set"
    );
    media.recycle_bin = Some(bin.display().to_string());
    media.recycle_bin_cleanup_days = 0;
    kv::set(db.pool(), kv::KEY_MEDIA_MANAGEMENT, &media)
        .await
        .unwrap();
    kitsunarr_library::tools::clean_recycle_bin(&db)
        .await
        .unwrap();
    assert!(old.exists(), "0 days: off");
    media.recycle_bin_cleanup_days = 7;
    kv::set(db.pool(), kv::KEY_MEDIA_MANAGEMENT, &media)
        .await
        .unwrap();
    let summary = kitsunarr_library::tools::clean_recycle_bin(&db)
        .await
        .unwrap();
    assert!(summary.starts_with("1 files"), "{summary}");
    assert!(!old.exists() && !bin.join("Show").exists());
    assert!(fresh.exists() && bin.exists());
}

/// "Application Update" goes out once per new version, never on the first start.
#[tokio::test]
async fn a_new_version_is_announced_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let h = heard.clone();
    let hook = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |axum::Json(v): axum::Json<serde_json::Value>| {
            let h = h.clone();
            async move {
                h.lock().unwrap().push(v);
                "ok"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, hook).await.unwrap() });
    sqlx::query("INSERT INTO notifications (name, implementation, settings, triggers, created_at) VALUES ('Hook', 'Webhook', ?, '{\"onApplicationUpdate\": true}', '2026-11-01T00:00:00Z')")
        .bind(serde_json::json!({"url": hook_url, "method": 1}).to_string())
        .execute(db.pool())
        .await
        .unwrap();
    let notifier = kitsunarr_notifications::Notifier::new(reqwest::Client::new());
    use kitsunarr_library::notify::announce_version;
    announce_version(&db, &notifier, "0.1.0").await;
    announce_version(&db, &notifier, "0.1.0").await;
    announce_version(&db, &notifier, "0.2.0").await;
    for _ in 0..50 {
        if !heard.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0]["eventType"], "ApplicationUpdate");
    assert_eq!(heard[0]["newVersion"], "0.2.0");
}

/// A finished film download: its main video becomes the film's file, named with the movie
/// format; a lower quality afterwards is not an upgrade.
#[tokio::test]
async fn film_downloads_are_imported() {
    use kitsunarr_library::movie_import::{MovieDownload, import_movie_download};
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let root = dir.path().join("movies");
    std::fs::create_dir_all(&root).unwrap();
    let movie_path = root.join("The Matrix (1999)");
    let profile: i64 = sqlx::query_scalar("SELECT id FROM quality_profiles WHERE name = 'Any'")
        .fetch_one(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO movies (id, title, sort_title, clean_title, title_slug, year, runtime, metadata_provider, metadata_id, \
         quality_profile_id, root_folder_path, path, added_at) VALUES (7, 'The Matrix', 'matrix', 'thematrix', 'the-matrix', 1999, 136, \
         'tmdbMovie', '603', ?, ?, ?, '2026-11-01T00:00:00Z')",
    )
    .bind(profile)
    .bind(root.display().to_string())
    .bind(movie_path.display().to_string())
    .execute(db.pool())
    .await
    .unwrap();
    let mut naming: kitsunarr_domain::api::settings::NamingSettings =
        kv::get_or_default(db.pool(), kv::KEY_NAMING).await.unwrap();
    naming.rename_episodes = true;
    kv::set(db.pool(), kv::KEY_NAMING, &naming).await.unwrap();
    let download = dir
        .path()
        .join("downloads/The.Matrix.1999.1080p.BluRay.x264-GRP");
    std::fs::create_dir_all(&download).unwrap();
    std::fs::write(
        download.join("The.Matrix.1999.1080p.BluRay.x264-GRP.mkv"),
        vec![1u8; 4096],
    )
    .unwrap();
    std::fs::write(download.join("sample.mkv"), vec![1u8; 8192]).unwrap();
    let grab = |title: &str| kitsunarr_db::history::GrabRecord {
        download_id: "ABC".into(),
        date: "2026-11-01T00:00:00Z".into(),
        source_title: title.into(),
        series_id: None,
        episode_id: None,
        quality_id: None,
        revision: None,
        languages: None,
        data: "{}".into(),
        movie_id: Some(7),
    };
    let grabs = vec![grab("The.Matrix.1999.1080p.BluRay.x264-GRP")];
    let d = MovieDownload {
        download_id: "ABC",
        title: "The.Matrix.1999.1080p.BluRay.x264-GRP",
        output_path: &download,
        movie_id: 7,
        grabs: &grabs,
        client_name: "qBittorrent",
        client_implementation: "QBittorrent",
        move_files: false,
    };
    let outcome = import_movie_download(&db, &d).await.unwrap();
    assert!(outcome.results.iter().any(|r| r.imported), "{outcome:?}");
    let imported = movie_path.join("The Matrix (1999) Bluray-1080p.mkv");
    let listed: Vec<_> = std::fs::read_dir(&movie_path)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(imported.is_file(), "{listed:?}");
    assert_eq!(
        std::fs::metadata(&imported).unwrap().len(),
        4096,
        "the sample is skipped"
    );
    let detail = kitsunarr_db::library::movies::get_detail(&db, 7)
        .await
        .unwrap();
    assert_eq!(detail.file.unwrap().quality_name, "Bluray-1080p");
    let event: String = sqlx::query_scalar("SELECT event_type FROM history WHERE movie_id = 7")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(event, "downloadFolderImported");

    // A lower quality is refused and the film's file stays.
    let worse = dir
        .path()
        .join("downloads/The.Matrix.1999.720p.HDTV.x264-GRP.mkv");
    std::fs::write(&worse, vec![2u8; 100]).unwrap();
    let grabs = vec![grab("The.Matrix.1999.720p.HDTV.x264-GRP")];
    let outcome = import_movie_download(
        &db,
        &MovieDownload {
            title: "The.Matrix.1999.720p.HDTV.x264-GRP",
            output_path: &worse,
            grabs: &grabs,
            ..d
        },
    )
    .await
    .unwrap();
    assert!(!outcome.results[0].imported);
    assert_eq!(
        outcome.results[0].errors,
        vec!["Not an upgrade for existing movie file(s)"]
    );
    assert!(imported.is_file());
}

/// A failed film download is recorded, blocklisted for the film and searched for again.
#[tokio::test]
async fn failed_film_downloads_are_blocklisted() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let profile: i64 = sqlx::query_scalar("SELECT MIN(id) FROM quality_profiles")
        .fetch_one(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO movies (id, title, sort_title, clean_title, title_slug, year, metadata_provider, metadata_id, \
         quality_profile_id, root_folder_path, path, added_at) VALUES (9, 'Up', 'up', 'up', 'up', 2009, 'tmdbMovie', '14160', ?, '/m', '/m/Up', '2026-11-01T00:00:00Z')",
    )
    .bind(profile)
    .execute(db.pool())
    .await
    .unwrap();
    let grabs = vec![kitsunarr_db::history::GrabRecord {
        download_id: "XYZ".into(),
        date: "2026-11-01T00:00:00Z".into(),
        source_title: "Up.2009.1080p.BluRay.x264-GRP".into(),
        series_id: None,
        episode_id: None,
        quality_id: Some(7),
        revision: None,
        languages: None,
        data: r#"{"indexer": "Mock", "protocol": "torrent", "releaseSource": "Search"}"#.into(),
        movie_id: Some(9),
    }];
    let redownload = kitsunarr_library::failed::mark_failed(
        &db,
        &grabs,
        &kitsunarr_library::failed::Failure {
            message: "Manually marked as failed",
            source: None,
            torrent_hash: None,
            skip_redownload: false,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(redownload.movie_id, Some(9));
    let blocked = kitsunarr_db::blocklist::for_movie(&db, 9).await.unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].source_title, "Up.2009.1080p.BluRay.x264-GRP");
    let failed: String = sqlx::query_scalar("SELECT event_type FROM history WHERE movie_id = 9")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(failed, "downloadFailed");
    let page = kitsunarr_db::blocklist::list(&db, &Default::default())
        .await
        .unwrap();
    assert_eq!(
        (
            page.records[0].movie_id,
            page.records[0].series_title.as_deref()
        ),
        (Some(9), Some("Up"))
    );
    let model_blocklist = kitsunarr_library::decision_input::movie_blocklist(&db, 9)
        .await
        .unwrap();
    assert_eq!(model_blocklist[0].series_id, 9);
}
