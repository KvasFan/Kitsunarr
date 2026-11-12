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
