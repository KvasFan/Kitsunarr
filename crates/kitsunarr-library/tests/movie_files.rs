//! Film file management on real temporary directory trees. Catalogues are not contacted.
use kitsunarr_db::library::movies;
use kitsunarr_db::{Database, settings};
use kitsunarr_domain::api::settings::{MediaManagementSettings, NamingSettings};
use kitsunarr_library::movie_files;
use std::path::{Path, PathBuf};

async fn library(dir: &Path) -> (Database, PathBuf, PathBuf) {
    let root = dir.join("movies");
    let folder = root.join("Film");
    std::fs::create_dir_all(&folder).unwrap();
    let db = Database::open(&dir.join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    kitsunarr_db::root_folders::create(db.pool(), root.to_str().unwrap(), None)
        .await
        .unwrap();
    settings::set(
        db.pool(),
        settings::KEY_MEDIA_MANAGEMENT,
        &MediaManagementSettings {
            enable_media_info: false,
            recycle_bin: Some(dir.join("bin").display().to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,year,runtime,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (1,'Film','film','film','film',2020,100,'tmdbMovie','1',1,?,?,'2026-11-14T00:00:00Z')")
        .bind(root.to_str().unwrap()).bind(folder.to_str().unwrap()).execute(db.pool()).await.unwrap();
    (db, root, folder)
}
async fn feature(db: &Database, folder: &Path) -> PathBuf {
    let path = folder.join("source.1080p.WEB-DL-GRP.mkv");
    std::fs::write(&path, b"feature").unwrap();
    assert_eq!(movie_files::scan(db, 1).await.unwrap().added, 1);
    path
}

#[tokio::test]
async fn scans_register_update_and_keep_the_existing_feature() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let path = feature(&db, &folder).await;
    let before = movies::get_detail(&db, 1).await.unwrap().file.unwrap();
    assert!(before.quality_id > 0);
    assert!(path.is_file(), "scans never move media");
    std::fs::write(&path, b"changed feature").unwrap();
    std::fs::write(folder.join("another.2160p.mkv"), b"alternative").unwrap();
    let result = movie_files::scan(&db, 1).await.unwrap();
    assert_eq!((result.added, result.removed, result.updated), (0, 0, 1));
    let after = movies::get_detail(&db, 1).await.unwrap().file.unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.relative_path, before.relative_path);
    assert_eq!(after.size, 15);
}

#[tokio::test]
async fn scans_report_ambiguity_and_preserve_records_when_root_disappears() {
    let dir = tempfile::tempdir().unwrap();
    let (db, root, folder) = library(dir.path()).await;
    std::fs::write(folder.join("one.mkv"), b"one").unwrap();
    std::fs::write(folder.join("two.mkv"), b"two").unwrap();
    let summary = movie_files::scan(&db, 1).await.unwrap();
    assert_eq!(summary.added, 0);
    assert_eq!(summary.skipped.len(), 1);
    std::fs::remove_file(folder.join("two.mkv")).unwrap();
    movie_files::scan(&db, 1).await.unwrap();
    std::fs::rename(&root, dir.path().join("disconnected")).unwrap();
    assert!(movie_files::scan(&db, 1).await.is_err());
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_some());
    std::fs::rename(dir.path().join("disconnected"), &root).unwrap();
    std::fs::remove_file(folder.join("one.mkv")).unwrap();
    assert_eq!(movie_files::scan(&db, 1).await.unwrap().removed, 1);
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_none());
}

#[tokio::test]
async fn previews_are_read_only_and_renames_preserve_subtitle_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let path = feature(&db, &folder).await;
    let subtitle = path.with_extension("en.forced.srt");
    std::fs::write(&subtitle, b"subtitle").unwrap();
    std::fs::write(folder.join("other.en.srt"), b"other").unwrap();
    let p = movie_files::rename_preview(&db, 1).await.unwrap().unwrap();
    assert!(path.is_file());
    assert!(subtitle.is_file());
    assert!(!folder.join(&p.new_path).exists());
    movie_files::rename(&db, 1, &p).await.unwrap();
    assert_eq!(std::fs::read(folder.join(&p.new_path)).unwrap(), b"feature");
    assert_eq!(
        std::fs::read(folder.join(&p.new_path).with_extension("en.forced.srt")).unwrap(),
        b"subtitle"
    );
    assert!(folder.join("other.en.srt").is_file());
    assert!(!path.exists());
    assert!(!subtitle.exists());
    assert_eq!(
        movies::get_detail(&db, 1).await.unwrap().file.unwrap().id,
        p.movie_file_id
    );
    assert!(movie_files::rename_preview(&db, 1).await.unwrap().is_none());
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM history WHERE movie_id=1 AND event_type='movieFileRenamed'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn collisions_and_stale_previews_leave_every_file_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let path = feature(&db, &folder).await;
    let p = movie_files::rename_preview(&db, 1).await.unwrap().unwrap();
    let dest = folder.join(&p.new_path);
    std::fs::write(&dest, b"existing").unwrap();
    assert!(
        movie_files::rename(&db, 1, &p)
            .await
            .unwrap_err()
            .contains("already exists")
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"existing");
    assert_eq!(std::fs::read(&path).unwrap(), b"feature");
    std::fs::remove_file(&dest).unwrap();
    std::fs::write(path.with_extension("en.srt"), b"old").unwrap();
    std::fs::write(dest.with_extension("en.srt"), b"existing subtitle").unwrap();
    assert!(movie_files::rename(&db, 1, &p).await.is_err());
    assert!(path.is_file());
    settings::set(
        db.pool(),
        settings::KEY_NAMING,
        &NamingSettings {
            movie_file_format: "{Movie Title}".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        movie_files::rename(&db, 1, &p)
            .await
            .unwrap_err()
            .contains("preview changed")
    );
    assert!(path.is_file());
}

#[tokio::test]
async fn rejected_database_writes_roll_back_video_and_subtitle_renames() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let path = feature(&db, &folder).await;
    std::fs::write(path.with_extension("en.srt"), b"subtitle").unwrap();
    let p = movie_files::rename_preview(&db, 1).await.unwrap().unwrap();
    sqlx::query("CREATE TRIGGER reject_rename BEFORE UPDATE OF relative_path ON movie_files BEGIN SELECT RAISE(ABORT,'simulated write failure'); END").execute(db.pool()).await.unwrap();
    assert!(movie_files::rename(&db, 1, &p).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"feature");
    assert!(path.with_extension("en.srt").is_file());
    assert!(!folder.join(&p.new_path).exists());
    assert_eq!(
        movies::get_detail(&db, 1)
            .await
            .unwrap()
            .file
            .unwrap()
            .relative_path,
        p.existing_path
    );
}

#[tokio::test]
async fn file_deletion_recycles_its_subtitles_and_keeps_the_movie() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let path = feature(&db, &folder).await;
    std::fs::write(path.with_extension("it.srt"), b"sub").unwrap();
    let file = movies::get_detail(&db, 1).await.unwrap().file.unwrap();
    assert!(movie_files::delete_file(&db, 1, file.id + 1).await.is_err());
    assert!(path.is_file());
    movie_files::delete_file(&db, 1, file.id).await.unwrap();
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_none());
    assert!(!path.exists());
    assert_eq!(
        std::fs::read(dir.path().join("bin/Film").join(&file.relative_path)).unwrap(),
        b"feature"
    );
    assert!(
        dir.path()
            .join("bin/Film")
            .join(&file.relative_path)
            .with_extension("it.srt")
            .is_file()
    );
}

#[tokio::test]
async fn folder_deletion_is_scoped_and_recycling_never_overwrites_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    let (db, root, folder) = library(dir.path()).await;
    feature(&db, &folder).await;
    std::fs::create_dir_all(dir.path().join("bin/Film")).unwrap();
    std::fs::write(dir.path().join("bin/Film/keep"), b"keep").unwrap();
    movie_files::delete_folder(&db, 1).await.unwrap();
    assert!(!folder.exists());
    assert!(dir.path().join("bin/Film_2").is_dir());
    assert!(dir.path().join("bin/Film/keep").is_file());
    sqlx::query("UPDATE movies SET path = root_folder_path WHERE id=1")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(movie_files::delete_folder(&db, 1).await.is_err());
    assert!(root.is_dir());
    sqlx::query("UPDATE movies SET path = ? WHERE id=1")
        .bind(dir.path().to_str().unwrap())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(movie_files::delete_folder(&db, 1).await.is_err());
    assert!(dir.path().join("bin/Film/keep").is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn traversal_and_symbolic_links_cannot_touch_another_folder() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let path = feature(&db, &folder).await;
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("protected.mkv"), b"protected").unwrap();
    sqlx::query("UPDATE movie_files SET relative_path = '../../outside/protected.mkv'")
        .execute(db.pool())
        .await
        .unwrap();
    let id = movies::get_detail(&db, 1).await.unwrap().file.unwrap().id;
    assert!(movie_files::delete_file(&db, 1, id).await.is_err());
    assert!(movie_files::rename_preview(&db, 1).await.is_err());
    assert!(movie_files::scan(&db, 1).await.is_err());
    std::os::unix::fs::symlink(&outside, folder.join("link")).unwrap();
    sqlx::query("UPDATE movie_files SET relative_path = 'link/protected.mkv'")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(movie_files::delete_file(&db, 1, id).await.is_err());
    sqlx::query("UPDATE movies SET path = ?")
        .bind(folder.join("link").to_str().unwrap())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(movie_files::delete_folder(&db, 1).await.is_err());
    assert_eq!(
        std::fs::read(outside.join("protected.mkv")).unwrap(),
        b"protected"
    );
    assert!(path.exists());
}

#[tokio::test]
async fn scans_use_measured_resolution_and_refuse_measured_samples() {
    use std::process::Command;
    if !["ffmpeg", "ffprobe"].iter().all(|c| {
        Command::new(c)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    }) {
        eprintln!("ffmpeg/ffprobe unavailable; media-info fixture was not run");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    settings::set(
        db.pool(),
        settings::KEY_MEDIA_MANAGEMENT,
        &MediaManagementSettings {
            enable_media_info: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE movies SET runtime=1 WHERE id=1")
        .execute(db.pool())
        .await
        .unwrap();
    let path = folder.join("Film.2020.1080p.WEB-DL.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=1280x720:rate=1",
            "-t",
            "16",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(movie_files::scan(&db, 1).await.unwrap().added, 1);
    let f = movies::get_detail(&db, 1).await.unwrap().file.unwrap();
    assert_eq!(
        kitsunarr_domain::Quality::by_id(f.quality_id)
            .unwrap()
            .resolution,
        720
    );
    let info: Option<String> = sqlx::query_scalar("SELECT media_info FROM movie_files WHERE id=?")
        .bind(f.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(info.is_some());
    sqlx::query("DELETE FROM movie_files")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE movies SET runtime=100 WHERE id=1")
        .execute(db.pool())
        .await
        .unwrap();
    let result = movie_files::scan(&db, 1).await.unwrap();
    assert_eq!(result.added, 0);
    assert!(result.skipped[0].contains("sample"));
    assert!(path.is_file());
}

#[tokio::test]
async fn permanent_file_removal_cleans_staging_and_keeps_unrelated_files() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    settings::set(
        db.pool(),
        settings::KEY_MEDIA_MANAGEMENT,
        &MediaManagementSettings {
            enable_media_info: false,
            recycle_bin: None,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let path = feature(&db, &folder).await;
    std::fs::write(path.with_extension("it.srt"), b"sub").unwrap();
    std::fs::write(folder.join("notes.txt"), b"keep").unwrap();
    let id = movies::get_detail(&db, 1).await.unwrap().file.unwrap().id;
    movie_files::delete_file(&db, 1, id).await.unwrap();
    let remaining: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(remaining, vec![std::ffi::OsString::from("notes.txt")]);
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_inside_the_recycle_bin_cannot_redirect_file_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let path = feature(&db, &folder).await;
    std::fs::write(path.with_extension("it.srt"), b"sub").unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::create_dir(dir.path().join("bin")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.path().join("bin/Film")).unwrap();
    let id = movies::get_detail(&db, 1).await.unwrap().file.unwrap().id;
    assert!(movie_files::delete_file(&db, 1, id).await.is_err());
    assert!(path.is_file());
    assert!(path.with_extension("it.srt").is_file());
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_some());
}

#[tokio::test]
async fn deletion_refuses_overlapping_title_folders_and_failed_database_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (db, root, folder) = library(dir.path()).await;
    let path = feature(&db, &folder).await;
    let id = movies::get_detail(&db, 1).await.unwrap().file.unwrap().id;
    sqlx::query("CREATE TRIGGER reject_delete BEFORE DELETE ON movie_files BEGIN SELECT RAISE(ABORT,'simulated write failure'); END").execute(db.pool()).await.unwrap();
    assert!(movie_files::delete_file(&db, 1, id).await.is_err());
    assert!(path.is_file());
    sqlx::query("INSERT INTO movies (title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES ('Other','other','other','other','tmdbMovie','2',1,?,?,'2026-11-14T00:00:00Z')")
        .bind(root.to_str().unwrap()).bind(folder.join("Other").to_str().unwrap()).execute(db.pool()).await.unwrap();
    assert!(movie_files::delete_folder(&db, 1).await.is_err());
    assert!(path.is_file());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn feature_recycling_across_filesystems_keeps_bytes_and_folder_moves_fail_safely() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let Ok(bin) = tempfile::tempdir_in("/dev/shm") else {
        eprintln!("No second filesystem for the recycle fixture");
        return;
    };
    if std::fs::metadata(&folder).unwrap().dev() == std::fs::metadata(bin.path()).unwrap().dev() {
        eprintln!("Recycle fixture directories share a filesystem");
        return;
    }
    settings::set(
        db.pool(),
        settings::KEY_MEDIA_MANAGEMENT,
        &MediaManagementSettings {
            enable_media_info: false,
            recycle_bin: Some(bin.path().display().to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let path = feature(&db, &folder).await;
    std::fs::write(path.with_extension("it.srt"), b"sub").unwrap();
    let id = movies::get_detail(&db, 1).await.unwrap().file.unwrap().id;
    movie_files::delete_file(&db, 1, id).await.unwrap();
    assert!(!path.exists());
    assert_eq!(
        std::fs::read(bin.path().join("Film").join(path.file_name().unwrap())).unwrap(),
        b"feature"
    );
    assert!(
        bin.path()
            .join("Film")
            .join(path.file_name().unwrap())
            .with_extension("it.srt")
            .is_file()
    );
    let path = feature(&db, &folder).await;
    assert!(movie_files::delete_folder(&db, 1).await.is_err());
    assert!(path.is_file());
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_some());
}

#[tokio::test]
async fn film_folders_move_with_their_feature_and_refuse_unsafe_targets() {
    use kitsunarr_library::move_title::{Title, move_title};
    let dir = tempfile::tempdir().unwrap();
    let (db, root, folder) = library(dir.path()).await;
    let video = feature(&db, &folder).await;
    std::fs::write(folder.join("source.1080p.WEB-DL-GRP.en.srt"), b"sub").unwrap();
    // Unsafe or occupied targets change nothing.
    std::fs::create_dir_all(root.join("Taken")).unwrap();
    std::fs::write(root.join("Taken/x"), b"x").unwrap();
    for bad in [
        root.join("Taken").display().to_string(),
        folder.join("Inner").display().to_string(),
        dir.path().join("elsewhere/Film").display().to_string(),
    ] {
        assert!(
            move_title(&db, Title::Movie(1), &bad, true).await.is_err(),
            "{bad}"
        );
    }
    assert!(video.is_file());
    let target = root.join("Films/Film (2020)");
    let moved = move_title(&db, Title::Movie(1), target.to_str().unwrap(), true)
        .await
        .unwrap();
    assert!(moved.moved);
    assert!(!folder.exists());
    assert!(target.join("source.1080p.WEB-DL-GRP.mkv").is_file());
    assert!(
        target.join("source.1080p.WEB-DL-GRP.en.srt").is_file(),
        "sidecars move too"
    );
    let detail = movies::get_detail(&db, 1).await.unwrap();
    assert_eq!(detail.item.path, target.display().to_string());
    assert!(
        detail.file.is_some(),
        "the feature record follows its folder"
    );
    let rescan = movie_files::scan(&db, 1).await.unwrap();
    assert_eq!((rescan.added, rescan.removed), (0, 0));
}

#[tokio::test]
async fn film_folders_move_across_filesystems_by_copying() {
    use kitsunarr_library::move_title::{Title, move_title};
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
        eprintln!("No second filesystem for the move fixture");
        return;
    };
    if std::fs::metadata(&folder).unwrap().dev() == std::fs::metadata(other.path()).unwrap().dev() {
        eprintln!("Move fixture directories share a filesystem");
        return;
    }
    kitsunarr_db::root_folders::create(db.pool(), other.path().to_str().unwrap(), None)
        .await
        .unwrap();
    feature(&db, &folder).await;
    let target = other.path().join("Film");
    let moved = move_title(&db, Title::Movie(1), target.to_str().unwrap(), true)
        .await
        .unwrap();
    assert!(moved.moved);
    assert_eq!(moved.root_folder_path, other.path().display().to_string());
    assert!(!folder.exists(), "the source is removed after the copy");
    assert_eq!(
        std::fs::read(target.join("source.1080p.WEB-DL-GRP.mkv")).unwrap(),
        b"feature"
    );
}
