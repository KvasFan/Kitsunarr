//! Sonarr's media management options on a real folder: empty series folders created or
//! deleted, "rescan after refresh", user-rejected extensions, episode titles and script import.

use std::path::{Path, PathBuf};

use kitsunarr_db::Database;
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::settings::{
    EpisodeTitleRequired, MediaManagementSettings, RescanAfterRefresh,
};
use kitsunarr_library::import::{Download, import_download};

/// A series "Show" in `<dir>/tv/Show` (not created) with two episodes of season 1 that aired
/// long ago (episode 2 has no title).
async fn library(dir: &Path) -> Database {
    let root = dir.join("tv");
    std::fs::create_dir_all(&root).unwrap();
    // A root folder with something in it: an empty one would look like a missing drive.
    std::fs::create_dir_all(root.join("Other")).unwrap();
    let db = Database::open(&dir.join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let pool = db.pool();
    kitsunarr_db::root_folders::create(pool, root.to_str().unwrap(), None)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, year, status, overview, network, \
         genres, images, metadata_provider, metadata_id, quality_profile_id, root_folder_path, path, added_at) \
         VALUES (1, 'series', 'Show', 'show', 'show', 'show', 2008, 'ended', '', 'AMC', '[\"Drama\"]', '[]', \
         'tvdb', '81189', 1, ?, ?, '2026-11-01T00:00:00Z')",
    )
    .bind(root.display().to_string())
    .bind(root.join("Show").display().to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO seasons (series_id, season_number, monitored, images) VALUES (1, 1, 1, '[]')",
    )
    .execute(pool)
    .await
    .unwrap();
    for (n, title) in [(1, Some("Pilot")), (2, None)] {
        sqlx::query(
            "INSERT INTO episodes (series_id, season_number, episode_number, title, air_date, air_date_utc, provider_episode_id, monitored) \
             VALUES (1, 1, ?, ?, '2008-01-20', '2008-01-20T02:00:00Z', ?, 1)",
        )
        .bind(n)
        .bind(title)
        .bind(format!("{}", 349_230 + n))
        .execute(pool)
        .await
        .unwrap();
    }
    set_media(&db, |m| m.enable_media_info = false).await;
    db
}

async fn set_media(db: &Database, change: impl FnOnce(&mut MediaManagementSettings)) {
    let mut media: MediaManagementSettings =
        kv::get_or_default(db.pool(), kv::KEY_MEDIA_MANAGEMENT)
            .await
            .unwrap();
    change(&mut media);
    kv::set(db.pool(), kv::KEY_MEDIA_MANAGEMENT, &media)
        .await
        .unwrap();
}

async fn file_paths(db: &Database) -> Vec<String> {
    sqlx::query_scalar("SELECT relative_path FROM episode_files ORDER BY relative_path")
        .fetch_all(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn empty_series_folders_are_created_or_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let db = library(dir.path()).await;
    let show = dir.path().join("tv/Show");

    // Off by default: a missing folder stays missing.
    kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    assert!(!show.exists());
    // "Create empty series folders", unless "delete empty folders" is on too.
    set_media(&db, |m| {
        m.create_empty_series_folders = true;
        m.delete_empty_folders = true;
    })
    .await;
    kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    assert!(!show.exists());
    set_media(&db, |m| m.delete_empty_folders = false).await;
    kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    assert!(show.is_dir());

    // "Delete empty folders": empty folders below the series folder, then the series folder.
    std::fs::create_dir_all(show.join("Season 01/empty")).unwrap();
    std::fs::create_dir_all(show.join("Season 02")).unwrap();
    std::fs::write(
        show.join("Season 01/Show.S01E01.720p.HDTV.x264-GRP.mkv"),
        b"video",
    )
    .unwrap();
    set_media(&db, |m| {
        m.create_empty_series_folders = false;
        m.delete_empty_folders = true;
    })
    .await;
    let scan = kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    assert_eq!(scan.added, 1);
    assert!(!show.join("Season 01/empty").exists());
    assert!(!show.join("Season 02").exists());
    assert!(show.join("Season 01").is_dir());

    // Deleting the last file by hand empties the series folder, which goes too.
    let file_id: i64 = sqlx::query_scalar("SELECT id FROM episode_files")
        .fetch_one(db.pool())
        .await
        .unwrap();
    kitsunarr_library::files::delete_file(&db, 1, file_id)
        .await
        .unwrap();
    assert!(!show.exists());
    assert!(dir.path().join("tv").is_dir(), "the root folder stays");
}

#[tokio::test]
async fn rescan_after_refresh_follows_the_setting() {
    let dir = tempfile::tempdir().unwrap();
    let db = library(dir.path()).await;
    let season = dir.path().join("tv/Show/Season 01");
    std::fs::create_dir_all(&season).unwrap();
    std::fs::write(season.join("Show.S01E01.720p.HDTV.x264-GRP.mkv"), b"video").unwrap();

    set_media(&db, |m| m.rescan_after_refresh = RescanAfterRefresh::Never).await;
    assert!(
        kitsunarr_library::rescan_after_refresh(&db, 1, true)
            .await
            .is_none()
    );
    set_media(&db, |m| {
        m.rescan_after_refresh = RescanAfterRefresh::AfterManual
    })
    .await;
    assert!(
        kitsunarr_library::rescan_after_refresh(&db, 1, false)
            .await
            .is_none()
    );
    assert!(file_paths(&db).await.is_empty());
    let scan = kitsunarr_library::rescan_after_refresh(&db, 1, true)
        .await
        .unwrap();
    assert_eq!(scan.added, 1);
    set_media(&db, |m| m.rescan_after_refresh = RescanAfterRefresh::Always).await;
    assert!(
        kitsunarr_library::rescan_after_refresh(&db, 1, false)
            .await
            .is_some()
    );
}

fn download<'a>(output: &'a Path, title: &'a str) -> Download<'a> {
    Download {
        download_id: "SABnzbd_nzo_1",
        title,
        output_path: output,
        series_id: 1,
        grabs: &[],
        client_name: "SABnzbd",
        client_implementation: "Sabnzbd",
        move_files: false,
        progress: None,
    }
}

/// A download folder `<dir>/downloads/<name>/<name>.mkv`.
fn downloaded(dir: &Path, name: &str) -> PathBuf {
    let folder = dir.join("downloads").join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(format!("{name}.mkv")), b"video").unwrap();
    folder
}

#[tokio::test]
async fn user_rejected_extensions_block_single_file_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let db = library(dir.path()).await;
    let file = dir.path().join("Show.S01E01.720p.HDTV.x264-GRP.wmv");
    std::fs::write(&file, b"video").unwrap();
    set_media(&db, |m| m.user_rejected_extensions = "exe, .WMV".into()).await;
    let out = import_download(&db, &download(&file, "Show.S01E01.720p.HDTV.x264-GRP"))
        .await
        .unwrap();
    assert_eq!(
        out.blocked.as_deref(),
        Some("Caution: Found file with user defined rejected extension: '.wmv'")
    );
    set_media(&db, |m| m.user_rejected_extensions = "exe".into()).await;
    let out = import_download(&db, &download(&file, "Show.S01E01.720p.HDTV.x264-GRP"))
        .await
        .unwrap();
    assert!(out.blocked.is_none(), "{out:?}");
    assert_eq!(out.imported_episodes(), 1, "{out:?}");
}

#[tokio::test]
async fn episode_titles_are_required_unless_never() {
    let dir = tempfile::tempdir().unwrap();
    let db = library(dir.path()).await;
    // Episode 2 has no title; recent air dates make it count.
    sqlx::query("UPDATE episodes SET air_date_utc = ? WHERE episode_number = 2")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
    // Renaming with a format that uses the episode title.
    let mut naming: kitsunarr_domain::api::settings::NamingSettings =
        kv::get_or_default(db.pool(), kv::KEY_NAMING).await.unwrap();
    naming.rename_episodes = true;
    naming.standard_episode_format =
        "{Series Title} - S{season:00}E{episode:00} - {Episode Title}".into();
    kv::set(db.pool(), kv::KEY_NAMING, &naming).await.unwrap();
    let name = "Show.S01E02.720p.HDTV.x264-GRP";
    let folder = downloaded(dir.path(), name);
    let out = import_download(&db, &download(&folder, name))
        .await
        .unwrap();
    assert_eq!(out.imported_episodes(), 0);
    assert!(
        out.results[0]
            .errors
            .contains(&"Episode does not have a title and recently aired".to_string()),
        "{out:?}"
    );
    set_media(&db, |m| {
        m.episode_title_required = EpisodeTitleRequired::Never
    })
    .await;
    let out = import_download(&db, &download(&folder, name))
        .await
        .unwrap();
    assert_eq!(out.imported_episodes(), 1, "{out:?}");
}

#[cfg(unix)]
fn script(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("import.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[tokio::test]
async fn an_import_script_puts_the_file_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let db = library(dir.path()).await;
    let name = "Show.S01E01.720p.HDTV.x264-GRP";
    let folder = downloaded(dir.path(), name);
    let env_dump = dir.path().join("env.txt");
    let s = script(
        dir.path(),
        &format!(
            "mkdir -p \"$(dirname \"$2\")\"\ncp \"$1\" \"$2\"\nenv | grep ^Sonarr_ > '{}'\necho '[MoveStatus] MoveComplete'",
            env_dump.display()
        ),
    );
    set_media(&db, |m| {
        m.use_script_import = true;
        m.script_import_path = s.display().to_string();
    })
    .await;
    let out = import_download(&db, &download(&folder, name))
        .await
        .unwrap();
    assert_eq!(out.imported_episodes(), 1, "{out:?}");
    let paths = file_paths(&db).await;
    assert_eq!(paths.len(), 1);
    assert!(dir.path().join("tv/Show").join(&paths[0]).is_file());
    let env = std::fs::read_to_string(&env_dump).unwrap();
    for line in [
        "Sonarr_Series_Title=Show",
        "Sonarr_Series_TvdbId=0",
        "Sonarr_Series_Genres=Drama",
        "Sonarr_EpisodeFile_EpisodeNumbers=1",
        "Sonarr_EpisodeFile_SeasonNumber=1",
        "Sonarr_EpisodeFile_EpisodeTitles=Pilot",
        "Sonarr_TransferMode=HardLinkOrCopy",
        "Sonarr_Download_Client=SABnzbd",
        "Sonarr_Download_Id=SABnzbd_nzo_1",
    ] {
        assert!(env.lines().any(|l| l == line), "{line} in {env}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_import_script_can_convert_defer_or_fail() {
    let dir = tempfile::tempdir().unwrap();
    let db = library(dir.path()).await;
    // A "conversion" to MP4, renamed by Kitsunarr.
    let mp4 = dir.path().join("converted.mp4");
    let s = script(
        dir.path(),
        &format!(
            "mkdir -p \"$(dirname \"$2\")\"\ncp \"$1\" \"$(dirname \"$2\")/converted.mp4\"\necho \"[MediaFile] $(dirname \"$2\")/converted.mp4\"\necho '[MoveStatus] RenameRequested'\n: '{}'",
            mp4.display()
        ),
    );
    set_media(&db, |m| {
        m.use_script_import = true;
        m.script_import_path = s.display().to_string();
    })
    .await;
    let name = "Show.S01E01.720p.HDTV.x264-GRP";
    let folder = downloaded(dir.path(), name);
    let out = import_download(&db, &download(&folder, name))
        .await
        .unwrap();
    assert_eq!(out.imported_episodes(), 1, "{out:?}");
    let paths = file_paths(&db).await;
    assert!(paths[0].ends_with(".mp4"), "{paths:?}");
    assert!(!paths[0].contains("converted"), "{paths:?}");
    assert!(dir.path().join("tv/Show").join(&paths[0]).is_file());

    // A script that defers leaves the transfer to Kitsunarr; a failing one fails the import.
    sqlx::query("UPDATE episodes SET title = 'Two' WHERE episode_number = 2")
        .execute(db.pool())
        .await
        .unwrap();
    script(dir.path(), "echo '[MoveStatus] DeferMove'");
    let name = "Show.S01E02.720p.HDTV.x264-GRP";
    let folder = downloaded(dir.path(), name);
    let out = import_download(&db, &download(&folder, name))
        .await
        .unwrap();
    assert_eq!(out.imported_episodes(), 1, "{out:?}");
    assert_eq!(file_paths(&db).await.len(), 2);

    script(dir.path(), "exit 2");
    sqlx::query("DELETE FROM episode_files")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE episodes SET episode_file_id = NULL")
        .execute(db.pool())
        .await
        .ok();
    let name = "Show.S01E01.720p.HDTV.x264-GRP.REPACK";
    let folder = downloaded(dir.path(), name);
    std::fs::remove_dir_all(dir.path().join("tv/Show")).unwrap();
    let out = import_download(&db, &download(&folder, name))
        .await
        .unwrap();
    assert_eq!(out.imported_episodes(), 0, "{out:?}");
    assert!(
        out.results[0]
            .errors
            .iter()
            .any(|e| e == "Failed to import episode: Script exited with non-zero exit code: 2"),
        "{out:?}"
    );
}
