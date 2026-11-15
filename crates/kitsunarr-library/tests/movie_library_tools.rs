//! Movie library tools on real temporary directories; no remote services are contacted.
use kitsunarr_db::library::movies;
use kitsunarr_db::{Database, settings};
use kitsunarr_domain::api::library_tools::MovieImportFile;
use kitsunarr_domain::api::settings::MediaManagementSettings;
use kitsunarr_library::{movie_files, movie_manual_import as manual, tools};
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
fn source(dir: &Path, name: &str) -> PathBuf {
    let downloads = dir.join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let file = downloads.join(name);
    std::fs::write(&file, b"feature").unwrap();
    std::fs::write(file.with_extension("it.forced.srt"), b"subtitle").unwrap();
    file
}
async fn run(
    db: &Database,
    file: &Path,
    moving: bool,
) -> kitsunarr_domain::api::library_tools::ManualImportResult {
    manual::import(
        db,
        &[MovieImportFile {
            path: file.display().to_string(),
            movie_id: 1,
        }],
        moving,
    )
    .await
    .remove(0)
}

#[tokio::test]
async fn audit_includes_movies_without_changing_records_or_files() {
    let dir = tempfile::tempdir().unwrap();
    let (db, root, folder) = library(dir.path()).await;
    let file = folder.join("Film.2020.WEB-DL.1080p.mkv");
    std::fs::write(&file, b"feature").unwrap();
    movie_files::scan(&db, 1).await.unwrap();
    std::fs::remove_file(&file).unwrap();
    let audit = tools::check_library(&db).await.unwrap();
    assert_eq!(
        (
            audit.series_checked,
            audit.movies_checked,
            audit.files_checked
        ),
        (0, 1, 1)
    );
    assert_eq!(audit.issues[0].movie_id, Some(1));
    assert_eq!(audit.issues[0].problem, "Missing on disk");
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_some());
    sqlx::query("UPDATE movie_files SET relative_path='../outside.mkv'")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(!tools::check_library(&db).await.unwrap().issues.is_empty());
    std::fs::remove_dir(&folder).unwrap();
    assert_eq!(
        tools::check_library(&db).await.unwrap().issues[0].problem,
        "Movie folder missing"
    );
    std::fs::remove_dir(&root).unwrap();
    assert!(
        tools::check_library(&db).await.unwrap().issues[0]
            .problem
            .contains("unavailable")
    );
}

fn queue(
    id: &str,
    movie: Option<i64>,
    series: Option<i64>,
) -> kitsunarr_domain::api::queue::QueueItem {
    serde_json::from_value(serde_json::json!({"downloadId":id,"title":"download","downloadClient":"Client","protocol":"torrent","seriesId":series,"seriesTitle":null,"movieId":movie,"episodes":[],"quality":null,"indexer":null,"size":100,"sizeLeft":30,"timeLeft":null,"status":"downloading","trackedState":"downloading","messages":[],"outputPath":null,"added":null})).unwrap()
}
#[test]
fn storage_counts_films_once_and_uses_the_closest_root() {
    let roots = vec!["/media".into(), "/media/movies".into()];
    let films = vec![(1, "/media/movies/Film".into())];
    let series = vec![(1, "/media/tv/Show".into())];
    let mut duplicate = queue("HASH", Some(1), None);
    duplicate.size = 120;
    let mut delayed = queue("pending", Some(1), None);
    delayed.tracked_state = "pending".into();
    let q = vec![
        queue("hash", Some(1), None),
        duplicate,
        queue("series", None, Some(1)),
        delayed,
    ];
    let result = tools::storage_outlook(&roots, &series, &films, &q, false, true);
    assert_eq!(
        (result[0].queued_downloads, result[0].import_space_estimate),
        (1, 100)
    );
    assert_eq!(
        (
            result[1].queued_downloads,
            result[1].import_space_estimate,
            result[1].download_remaining
        ),
        (1, 120, 30)
    );
    assert_eq!(
        tools::storage_outlook(&roots, &series, &films, &q, true, true)[1].import_space_estimate,
        0
    );
}

#[tokio::test]
async fn restore_recycled_feature_and_subtitle_registers_the_movie() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let file = folder.join("Film.2020.WEB-DL.1080p.mkv");
    std::fs::write(&file, b"feature").unwrap();
    std::fs::write(file.with_extension("it.srt"), b"subtitle").unwrap();
    movie_files::scan(&db, 1).await.unwrap();
    let id = movies::get_detail(&db, 1).await.unwrap().file.unwrap().id;
    movie_files::delete_file(&db, 1, id).await.unwrap();
    let recycled = tools::browse_recycle_bin(&db).await.unwrap();
    let token = &recycled
        .files
        .iter()
        .find(|f| f.name.ends_with(".mkv"))
        .unwrap()
        .token;
    tools::restore_movie(&db, token, 1).await.unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"feature");
    assert!(file.with_extension("it.srt").is_file());
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_some());
    assert!(
        tools::browse_recycle_bin(&db)
            .await
            .unwrap()
            .files
            .is_empty()
    );
}

#[tokio::test]
async fn restore_refuses_collisions_and_rolls_back_failed_database_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let bin = dir.path().join("bin/Film");
    std::fs::create_dir_all(&bin).unwrap();
    let file = bin.join("Film.mkv");
    std::fs::write(&file, b"feature").unwrap();
    std::fs::write(file.with_extension("en.srt"), b"subtitle").unwrap();
    let list = tools::browse_recycle_bin(&db).await.unwrap();
    let token = &list
        .files
        .iter()
        .find(|f| f.name == "Film.mkv")
        .unwrap()
        .token;
    std::fs::write(folder.join("Film.en.srt"), b"collision").unwrap();
    assert!(tools::restore_movie(&db, token, 1).await.is_err());
    assert!(file.exists());
    assert!(!folder.join("Film.mkv").exists());
    std::fs::remove_file(folder.join("Film.en.srt")).unwrap();
    sqlx::query("CREATE TRIGGER reject_feature BEFORE INSERT ON movie_files BEGIN SELECT RAISE(ABORT,'simulated failure'); END").execute(db.pool()).await.unwrap();
    assert!(tools::restore_movie(&db, token, 1).await.is_err());
    assert!(file.is_file());
    assert!(file.with_extension("en.srt").is_file());
    assert!(!folder.join("Film.mkv").exists());
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_none());
}

#[tokio::test]
async fn manual_scan_matches_uniquely_and_never_moves_files() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, _) = library(dir.path()).await;
    let path = source(dir.path(), "Film.2020.WEB-DL.1080p.mkv");
    let items = manual::scan(&db, path.parent().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].movie_id, Some(1));
    assert!(items[0].rejections.is_empty());
    assert!(path.is_file());
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_none());
    let unknown = source(dir.path(), "Unknown.2021.mkv");
    assert!(
        manual::scan(&db, &unknown, None).await.unwrap()[0]
            .movie_id
            .is_none()
    );
    assert_eq!(
        manual::scan(&db, &unknown, Some(1)).await.unwrap()[0].movie_id,
        Some(1)
    );
    let sample = source(dir.path(), "Film.2020.sample.mkv");
    assert!(
        manual::scan(&db, &sample, Some(1)).await.unwrap()[0]
            .rejections
            .iter()
            .any(|r| r == "Sample")
    );
}

#[tokio::test]
async fn copies_keep_the_source_and_bring_named_subtitles() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    settings::set(
        db.pool(),
        settings::KEY_NAMING,
        &kitsunarr_domain::api::settings::NamingSettings {
            rename_episodes: true,
            movie_file_format: "{Movie Title}".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let path = source(dir.path(), "Film.2020.WEB-DL.1080p.mkv");
    let result = run(&db, &path, false).await;
    assert!(result.imported, "{:?}", result.messages);
    assert!(path.is_file());
    assert!(path.with_extension("it.forced.srt").is_file());
    let f = movies::get_detail(&db, 1).await.unwrap().file.unwrap();
    assert_eq!(f.relative_path, "Film.mkv");
    assert_eq!(
        std::fs::read(folder.join(&f.relative_path)).unwrap(),
        b"feature"
    );
    assert!(
        folder
            .join(&f.relative_path)
            .with_extension("it.forced.srt")
            .is_file()
    );
    let audit = tools::check_library(&db).await.unwrap();
    assert!(audit.issues.is_empty());
}

#[tokio::test]
async fn moves_remove_only_the_selected_source_and_its_subtitles() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, _) = library(dir.path()).await;
    let path = source(dir.path(), "Film.2020.WEB-DL.1080p.mkv");
    let keep = path.parent().unwrap().join("notes.txt");
    std::fs::write(&keep, b"keep").unwrap();
    let result = run(&db, &path, true).await;
    assert!(result.imported, "{:?}", result.messages);
    assert!(!path.exists());
    assert!(!path.with_extension("it.forced.srt").exists());
    assert!(keep.is_file());
}

#[tokio::test]
async fn upgrades_recycle_the_previous_feature_and_database_errors_keep_it_current() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let old = folder.join("Film.2020.DVD.mkv");
    std::fs::write(&old, b"old").unwrap();
    movie_files::scan(&db, 1).await.unwrap();
    let new = source(dir.path(), "Film.2020.Bluray.1080p.mkv");
    sqlx::query("CREATE TRIGGER reject_upgrade BEFORE UPDATE ON movie_files BEGIN SELECT RAISE(ABORT,'simulated failure'); END").execute(db.pool()).await.unwrap();
    let result = run(&db, &new, false).await;
    assert!(!result.imported);
    assert_eq!(std::fs::read(&old).unwrap(), b"old");
    assert!(new.is_file());
    assert_eq!(
        movies::get_detail(&db, 1)
            .await
            .unwrap()
            .file
            .unwrap()
            .relative_path,
        "Film.2020.DVD.mkv"
    );
    sqlx::query("DROP TRIGGER reject_upgrade")
        .execute(db.pool())
        .await
        .unwrap();
    let result = run(&db, &new, false).await;
    assert!(result.imported, "{:?}", result.messages);
    assert!(!old.exists());
    assert_eq!(
        std::fs::read(dir.path().join("bin/Film/Film.2020.DVD.mkv")).unwrap(),
        b"old"
    );
    assert!(
        !run(&db, &new, false).await.imported,
        "a duplicate is not an upgrade"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn source_and_destination_symlinks_cannot_redirect_manual_import() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let path = source(dir.path(), "Film.2020.WEB-DL.1080p.mkv");
    let link = path.parent().unwrap().join("link.mkv");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(manual::scan(&db, &link, Some(1)).await.is_err());
    assert!(!run(&db, &link, true).await.imported);
    assert!(path.is_file());
    std::fs::remove_dir(&folder).unwrap();
    std::os::unix::fs::symlink(path.parent().unwrap(), &folder).unwrap();
    assert!(!run(&db, &path, false).await.imported);
    assert!(path.is_file());
}

#[tokio::test]
async fn a_subtitle_can_be_restored_only_to_its_own_current_feature() {
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let feature = folder.join("Film.2020.WEB-DL.1080p.mkv");
    std::fs::write(&feature, b"feature").unwrap();
    movie_files::scan(&db, 1).await.unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("Film.2020.WEB-DL.1080p.en.srt"), b"sub").unwrap();
    std::fs::write(bin.join("Other.en.srt"), b"other").unwrap();
    let recycled = tools::browse_recycle_bin(&db).await.unwrap();
    let good = &recycled
        .files
        .iter()
        .find(|f| f.name.starts_with("Film"))
        .unwrap()
        .token;
    let bad = &recycled
        .files
        .iter()
        .find(|f| f.name.starts_with("Other"))
        .unwrap()
        .token;
    assert!(tools::restore_movie(&db, bad, 1).await.is_err());
    tools::restore_movie(&db, good, 1).await.unwrap();
    assert!(feature.with_extension("en.srt").is_file());
    assert!(bin.join("Other.en.srt").is_file());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn hardlink_only_import_refuses_a_copy_across_filesystems() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
        eprintln!("Second filesystem unavailable");
        return;
    };
    if std::fs::metadata(&folder).unwrap().dev() == std::fs::metadata(other.path()).unwrap().dev() {
        eprintln!("Source and destination share a filesystem");
        return;
    }
    settings::set(
        db.pool(),
        settings::KEY_LIBRARY_TOOLS,
        &kitsunarr_domain::api::settings::LibraryToolsSettings {
            hardlink_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let path = source(other.path(), "Film.2020.WEB-DL.1080p.mkv");
    let result = run(&db, &path, false).await;
    assert!(!result.imported);
    assert!(result.messages[0].contains("Hardlink-only"));
    assert!(path.is_file());
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_none());
}

#[tokio::test]
async fn ambiguous_film_names_require_a_choice_and_existing_destinations_are_never_replaced() {
    use kitsunarr_domain::api::settings::NamingSettings;
    let dir = tempfile::tempdir().unwrap();
    let (db, root, folder) = library(dir.path()).await;
    sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,year,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (2,'Film','film','film','film-2',2020,'tmdbMovie','2',1,?,?,'2026-11-14T00:00:00Z')")
        .bind(root.to_str().unwrap()).bind(root.join("Film 2").to_str().unwrap()).execute(db.pool()).await.unwrap();
    let path = source(dir.path(), "Film.2020.WEB-DL.1080p.mkv");
    let preview = manual::scan(&db, &path, None).await.unwrap();
    assert!(preview[0].movie_id.is_none());
    assert!(preview[0].rejections[0].contains("Several movies"));
    assert_eq!(
        manual::scan(&db, &path, Some(1)).await.unwrap()[0].movie_id,
        Some(1)
    );
    settings::set(
        db.pool(),
        settings::KEY_NAMING,
        &NamingSettings {
            rename_episodes: true,
            movie_file_format: "{Movie Title}".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let existing = folder.join("Film.mkv");
    std::fs::write(&existing, b"unregistered feature").unwrap();
    let result = run(&db, &path, true).await;
    assert!(!result.imported);
    assert_eq!(std::fs::read(existing).unwrap(), b"unregistered feature");
    assert!(path.is_file());
    assert!(movies::get_detail(&db, 1).await.unwrap().file.is_none());
}

#[tokio::test]
async fn regex_custom_formats_drive_movie_import_and_real_rename() {
    use kitsunarr_domain::api::settings::{
        CustomFormat, CustomFormatSpecification, NamingSettings,
    };
    let dir = tempfile::tempdir().unwrap();
    let (db, _, folder) = library(dir.path()).await;
    for (name, pattern, include) in [
        ("WEB", r"(?<=\.)WEB-DL(?=\.)", true),
        ("Hidden", r"1080p", false),
    ] {
        kitsunarr_db::quality::create_custom_format(
            db.pool(),
            &CustomFormat {
                id: 0,
                name: name.into(),
                include_when_renaming: include,
                specifications: vec![CustomFormatSpecification {
                    name: name.into(),
                    implementation: "ReleaseTitleSpecification".into(),
                    negate: false,
                    required: true,
                    fields: serde_json::json!({"value":pattern})
                        .as_object()
                        .unwrap()
                        .clone(),
                }],
            },
        )
        .await
        .unwrap();
    }
    let mut naming = NamingSettings {
        rename_episodes: true,
        movie_file_format: "{MOVIE TITLE} ({Series Year}) {[Custom Formats]}".into(),
        ..Default::default()
    };
    settings::set(db.pool(), settings::KEY_NAMING, &naming)
        .await
        .unwrap();
    let original = source(dir.path(), "Film.2020.WEB-DL.1080p-GRP.mkv");
    let imported = run(&db, &original, false).await;
    assert!(imported.imported, "{:?}", imported.messages);
    let old = folder.join("FILM (2020) [WEB].mkv");
    assert_eq!(std::fs::read(&old).unwrap(), b"feature");
    assert_eq!(
        std::fs::read(old.with_extension("it.forced.srt")).unwrap(),
        b"subtitle"
    );
    assert!(original.is_file(), "copy import preserves the source");
    assert!(movie_files::rename_preview(&db, 1).await.unwrap().is_none());

    // Completed-download imports share the exact renderer and regex format inputs.
    sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,year,runtime,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) SELECT 2,'Auto Film','auto film','autofilm','auto-film',2020,100,'tmdbMovie','2',quality_profile_id,root_folder_path,?,'2026-11-15T00:00:00Z' FROM movies WHERE id=1")
        .bind(folder.parent().unwrap().join("Auto Film").to_str().unwrap()).execute(db.pool()).await.unwrap();
    let incoming = source(dir.path(), "Auto.Film.2020.WEB-DL.1080p-GRP.mkv");
    let automatic = kitsunarr_library::movie_import::import_movie_download(
        &db,
        &kitsunarr_library::movie_import::MovieDownload {
            movie_id: 2,
            download_id: "REGEX-PROOF",
            title: "Auto.Film.2020.WEB-DL.1080p-GRP",
            output_path: &incoming,
            source: Some(&incoming),
            grabs: &[],
            client_name: "Fixture",
            client_implementation: "QBittorrent",
            move_files: false,
        },
    )
    .await
    .unwrap();
    assert!(
        automatic.results.iter().any(|r| r.imported),
        "{automatic:?}"
    );
    assert_eq!(
        std::fs::read(
            folder
                .parent()
                .unwrap()
                .join("Auto Film/AUTO FILM (2020) [WEB].mkv")
        )
        .unwrap(),
        b"feature"
    );
    assert!(movie_files::rename_preview(&db, 2).await.unwrap().is_none());

    naming.movie_file_format =
        "{Series.Title}...({Series.Year}){[Custom Format:WEB]}-{Release Group}".into();
    settings::set(db.pool(), settings::KEY_NAMING, &naming)
        .await
        .unwrap();
    let preview = movie_files::rename_preview(&db, 1).await.unwrap().unwrap();
    assert_eq!(preview.new_path, "Film...(2020)[WEB]-GRP.mkv");
    let new = folder.join(&preview.new_path);
    assert!(
        old.is_file() && !new.exists(),
        "preview has no filesystem effects"
    );
    std::fs::write(&new, b"occupied").unwrap();
    assert!(movie_files::rename(&db, 1, &preview).await.is_err());
    assert_eq!(std::fs::read(&new).unwrap(), b"occupied");
    assert!(old.is_file());
    std::fs::remove_file(&new).unwrap();
    movie_files::rename(&db, 1, &preview).await.unwrap();
    assert_eq!(std::fs::read(&new).unwrap(), b"feature");
    assert_eq!(
        std::fs::read(new.with_extension("it.forced.srt")).unwrap(),
        b"subtitle"
    );
    assert!(!old.exists() && !old.with_extension("it.forced.srt").exists());
    assert!(movie_files::rename_preview(&db, 1).await.unwrap().is_none());
}
