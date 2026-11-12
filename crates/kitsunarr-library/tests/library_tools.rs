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

    // Imported as episode 3 by hand: the choice wins over the name.
    let results = kitsunarr_library::manual_import::import(
        &db,
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
        &[ManualImportFile {
            path: ep2.path.clone(),
            series_id: 1,
            episode_ids: vec![99],
        }],
        false,
    )
    .await;
    assert!(bad[0].1.is_err());
}
