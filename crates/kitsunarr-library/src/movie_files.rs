//! Film equivalents of Sonarr's `DiskScanService`, `RenameEpisodeFileService` and
//! `MediaFileDeletionService`. Tanukarr represents a film as one feature; Kitsunarr keeps
//! that feature in `movie_files`. Manual renames always use the configured movie format.

use std::path::{Component, Path, PathBuf};

use kitsunarr_db::history::{self, NewHistory};
use kitsunarr_db::library::movies;
use kitsunarr_db::{Database, settings};
use kitsunarr_domain::api::library::{MovieDetail, MovieRenamePreview};
use kitsunarr_domain::api::settings::{MediaManagementSettings, NamingSettings};
use sqlx::SqliteConnection;

use crate::files::ScanSummary;
use crate::import::{aggregate_quality_with_media, extension, stem, video_files};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Resolve only normal components below a trusted directory. Missing suffixes are allowed,
/// but existing symlinks (including dangling links) and parent traversal are never followed.
pub(crate) fn child(base: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty() {
        return Err("An empty relative path is not allowed.".into());
    }
    let mut path = base.to_path_buf();
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err("The path must stay inside the movie folder.".into());
        };
        path.push(name);
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("Symbolic links are not allowed in movie file operations.".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err(e)),
        }
    }
    Ok(path)
}

pub(crate) async fn location(db: &Database, d: &MovieDetail) -> Result<(PathBuf, PathBuf), String> {
    let root = PathBuf::from(&d.root_folder_path);
    let folder = PathBuf::from(&d.item.path);
    if !root.is_absolute() || !folder.is_absolute() || !root.is_dir() {
        return Err("The movie root folder is unavailable.".into());
    }
    let registered = kitsunarr_db::root_folders::list(db.pool())
        .await
        .map_err(err)?;
    if !registered.iter().any(|r| Path::new(&r.path) == root) {
        return Err("The movie root folder is not registered.".into());
    }
    let relative = folder
        .strip_prefix(&root)
        .map_err(|_| "The movie folder is outside its root folder.")?;
    let folder = child(&root, relative)?;
    // A path shared with another title is not a dedicated film folder.
    let others: Vec<String> =
        sqlx::query_scalar("SELECT path FROM movies WHERE id != ? UNION SELECT path FROM series")
            .bind(d.item.id)
            .fetch_all(db.pool())
            .await
            .map_err(err)?;
    if others
        .iter()
        .any(|other| folder.starts_with(other) || Path::new(other).starts_with(&folder))
    {
        return Err("The movie folder overlaps another library title.".into());
    }
    Ok((root, folder))
}

pub(crate) async fn unchanged(conn: &mut SqliteConnection, d: &MovieDetail) -> Result<(), String> {
    let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM movies WHERE id = ? AND path = ? AND root_folder_path = ?) AND ((SELECT id FROM movie_files WHERE movie_id = ?) IS ?) AND ((SELECT relative_path FROM movie_files WHERE movie_id = ?) IS ?)")
        .bind(d.item.id).bind(&d.item.path).bind(&d.root_folder_path)
        .bind(d.item.id).bind(d.file.as_ref().map(|f|f.id))
        .bind(d.item.id).bind(d.file.as_ref().map(|f|f.relative_path.as_str()))
        .fetch_one(conn).await.map_err(err)?;
    if !matches {
        return Err("The movie or its file changed. Reload and try again.".into());
    }
    Ok(())
}

pub(crate) async fn record(
    conn: &mut SqliteConnection,
    d: &MovieDetail,
    event: &str,
    source: &str,
    data: serde_json::Value,
) -> Result<(), String> {
    history::insert(
        conn,
        &NewHistory {
            event_type: event.into(),
            date: kitsunarr_db::now(),
            source_title: source.into(),
            series_id: None,
            episode_id: None,
            movie_id: Some(d.item.id),
            quality_id: d.file.as_ref().map(|f| f.quality_id),
            revision: None,
            languages: d
                .file
                .as_ref()
                .map(|f| f.languages.clone())
                .unwrap_or_default(),
            custom_format_score: None,
            download_id: None,
            data,
        },
    )
    .await
    .map_err(err)?;
    Ok(())
}

/// Scan in place. A missing or empty root never clears a feature record. Existing features
/// take precedence over every other file; an ambiguous folder needs explicit housekeeping.
pub async fn scan(db: &Database, id: i64) -> Result<ScanSummary, String> {
    scan_inner(db, id, None).await
}

pub async fn scan_notifying(
    db: &Database,
    id: i64,
    notifier: &kitsunarr_notifications::Notifier,
) -> Result<ScanSummary, String> {
    scan_inner(db, id, Some(notifier)).await
}

async fn scan_inner(
    db: &Database,
    id: i64,
    notifier: Option<&kitsunarr_notifications::Notifier>,
) -> Result<ScanSummary, String> {
    let d = movies::get_detail(db, id).await.map_err(err)?;
    let (root, folder) = location(db, &d).await?;
    let notification = if notifier.is_some() {
        if let Some(f) = &d.file {
            crate::notify::movie_file_event(db, id, f.id, "MissingFromDisk").await
        } else {
            None
        }
    } else {
        None
    };

    if std::fs::read_dir(&root).map_err(err)?.next().is_none() {
        return Err("The movie root folder is empty; its file record was kept.".into());
    }
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    let mut summary = ScanSummary::default();
    let existing = d
        .file
        .as_ref()
        .map(|f| child(&folder, Path::new(&f.relative_path.replace('\\', "/"))))
        .transpose()?;
    let current = existing.as_ref().filter(|p| p.is_file()).cloned();
    let mut candidates = Vec::new();
    for path in video_files(&folder) {
        if stem(&path)
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|s| s.eq_ignore_ascii_case("sample"))
        {
            continue;
        }
        let relative = path.strip_prefix(&folder).map_err(err)?;
        child(&folder, relative)?;
        candidates.push(path);
    }
    let chosen = if let Some(current) = current {
        Some(current)
    } else if candidates.len() == 1 {
        candidates.pop()
    } else if candidates.len() > 1 {
        let candidate = kitsunarr_parser::movie::MovieCandidate {
            title: &d.item.title,
            original_title: None,
            alternate_titles: &[],
            year: d.item.year.unwrap_or(0),
            imdb_id: None,
            anime_catalogue: matches!(
                d.item.metadata_provider,
                kitsunarr_domain::MetadataProvider::AniList
                    | kitsunarr_domain::MetadataProvider::MyAnimeList
            ),
            collection: None,
        };
        let matching: Vec<_> = candidates
            .iter()
            .filter(|p| kitsunarr_parser::movie::match_movie(&stem(p), &candidate).is_some())
            .collect();
        if matching.len() == 1 {
            Some(matching[0].clone())
        } else {
            summary
                .skipped
                .push("Several videos could be the movie feature; no file was selected.".into());
            None
        }
    } else {
        None
    };
    // Probe outside the write transaction: a slow ffprobe must not block library edits.
    let info = if media.enable_media_info {
        if let Some(path) = &chosen {
            crate::media_info::reader::read(path).await
        } else {
            None
        }
    } else {
        None
    };
    let sample = info
        .as_ref()
        .is_some_and(|m| m.is_sample(d.runtime.unwrap_or(0)));
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.map_err(err)?;
    unchanged(&mut tx, &d).await?;
    if let Some(f) = d
        .file
        .as_ref()
        .filter(|_| existing.as_ref().is_none_or(|p| !p.is_file()))
    {
        record(
            &mut tx,
            &d,
            "movieFileDeleted",
            &f.relative_path,
            serde_json::json!({"reason":"MissingFromDisk"}),
        )
        .await?;
        sqlx::query("DELETE FROM movie_files WHERE movie_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(err)?;
        summary.removed = 1;
    }
    if let Some(path) = chosen {
        if sample && summary.removed == 0 && d.file.is_some() {
            summary.skipped.push("The current feature is shorter than the movie runtime; its existing record was kept.".into());
        } else if sample {
            summary.skipped.push(format!("{}: sample", path.display()));
        } else {
            let relative = path
                .strip_prefix(&folder)
                .map_err(err)?
                .to_string_lossy()
                .replace('\\', "/");
            let size = std::fs::metadata(&path).map_err(err)?.len() as i64;
            let quality = aggregate_quality_with_media(
                &[kitsunarr_parser::parse_quality(&stem(&path))],
                info.as_ref(),
            );
            let json = info.as_ref().and_then(|m| serde_json::to_value(m).ok());
            if let Some(f) = d.file.as_ref().filter(|_| summary.removed == 0) {
                let changed: bool = sqlx::query_scalar("SELECT size != ? OR (? IS NOT NULL AND media_info IS NOT ?) FROM movie_files WHERE movie_id = ?")
                    .bind(size).bind(json.as_ref().map(serde_json::Value::to_string)).bind(json.as_ref().map(serde_json::Value::to_string)).bind(id).fetch_one(&mut *tx).await.map_err(err)?;
                sqlx::query("UPDATE movie_files SET size = ?, media_info = COALESCE(?,media_info), quality_id = CASE WHEN ? THEN ? ELSE quality_id END WHERE movie_id = ?")
                    .bind(size).bind(json.as_ref().map(serde_json::Value::to_string)).bind(info.is_some()).bind(quality.quality_id).bind(id).execute(&mut *tx).await.map_err(err)?;
                if changed || f.size != size {
                    summary.updated = 1;
                }
            } else {
                let languages = kitsunarr_parser::parse_languages(&stem(&path));
                let languages = if languages.is_empty() || languages.iter().all(|l| l.id == 0) {
                    vec![
                        sqlx::query_scalar::<_, Option<i32>>(
                            "SELECT original_language FROM movies WHERE id = ?",
                        )
                        .bind(id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(err)?
                        .unwrap_or(0),
                    ]
                } else {
                    languages.iter().map(|l| l.id).collect()
                };
                movies::upsert_file(
                    &mut tx,
                    &movies::NewMovieFile {
                        movie_id: id,
                        relative_path: relative.clone(),
                        size,
                        added_at: kitsunarr_db::now(),
                        scene_name: Some(stem(&path)),
                        release_group: kitsunarr_parser::parse_release_group(&stem(&path)),
                        quality_id: quality.quality_id,
                        revision_version: quality.revision.version,
                        revision_real: quality.revision.real,
                        is_repack: quality.revision.is_repack,
                        languages,
                        media_info: json,
                        original_path: None,
                        edition: None,
                    },
                )
                .await
                .map_err(err)?;
                record(
                    &mut tx,
                    &d,
                    "movieFileImported",
                    &relative,
                    serde_json::json!({"newDownload":false,"relativePath":relative}),
                )
                .await?;
                summary.added = 1;
            }
        }
    }
    tx.commit().await.map_err(err)?;
    if summary.removed > 0
        && let (Some(notifier), Some(event)) = (notifier, notification)
    {
        crate::notify::spawn(db, notifier, event);
    }
    if summary.added > 0 || summary.updated > 0 {
        crate::metadata_files::after_movie_change(db, id).await;
    }
    Ok(summary)
}

/// Read-only preview. The request must be echoed back when applying, so a changed naming
/// format or file record cannot silently apply a different rename from the reviewed one.
pub async fn rename_preview(db: &Database, id: i64) -> Result<Option<MovieRenamePreview>, String> {
    let d = movies::get_detail(db, id).await.map_err(err)?;
    let Some(f) = &d.file else { return Ok(None) };
    let (_, folder) = location(db, &d).await?;
    let source = child(&folder, Path::new(&f.relative_path.replace('\\', "/")))?;
    if !source.is_file() {
        return Err("The movie file is missing.".into());
    }
    let naming: NamingSettings = settings::get_or_default(db.pool(), settings::KEY_NAMING)
        .await
        .map_err(err)?;
    let input = crate::movie_naming::stored_file(db, &d).await?;
    let new_path =
        crate::movie_naming::file_name(db, &d, &input, &extension(&source), &naming).await?;
    child(&folder, Path::new(&new_path))?;
    if new_path == f.relative_path.replace('\\', "/") {
        return Ok(None);
    }
    Ok(Some(MovieRenamePreview {
        movie_file_id: f.id,
        existing_path: f.relative_path.clone(),
        new_path,
    }))
}

/// Matching sidecars only: `Feature.en.forced.srt`, `Feature.nfo`, etc. Artwork and files
/// naming another feature stay in place. No directory or symbolic link is followed.
pub(crate) fn sidecars(video: &Path) -> Result<Vec<PathBuf>, String> {
    let Some(parent) = video.parent() else {
        return Ok(Vec::new());
    };
    if !parent.is_dir() {
        return Ok(Vec::new());
    }
    let prefix = stem(video);
    let mut result = Vec::new();
    for entry in std::fs::read_dir(parent).map_err(err)? {
        let entry = entry.map_err(err)?;
        if !entry.file_type().map_err(err)?.is_file() {
            continue;
        }
        let path = entry.path();
        let ext = extension(&path);
        if !crate::extras::SUBTITLE_EXTENSIONS.contains(&ext.as_str()) && ext != ".nfo" {
            continue;
        }
        let name = stem(&path);
        if name == prefix
            || name
                .strip_prefix(&prefix)
                .is_some_and(|s| s.starts_with('.'))
        {
            result.push(path);
        }
    }
    result.sort();
    Ok(result)
}

/// Hard links give a no-overwrite move within a film's filesystem and retain timestamps.
/// A failed unlink removes only the link just created, leaving the source untouched.
pub(crate) fn move_file(source: &Path, target: &Path) -> Result<(), String> {
    match std::fs::hard_link(source, target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            let mut input = std::fs::File::open(source).map_err(err)?;
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)
                .map_err(err)?;
            let copied = (|| -> Result<(), std::io::Error> {
                std::io::copy(&mut input, &mut output)?;
                output.set_permissions(input.metadata()?.permissions())?;
                if let Ok(modified) = input.metadata()?.modified() {
                    output.set_modified(modified)?;
                }
                output.sync_all()?;
                Ok(())
            })();
            if let Err(e) = copied {
                let _ = std::fs::remove_file(target);
                return Err(err(e));
            }
        }
        Err(e) => return Err(err(e)),
    }
    if let Err(e) = std::fs::remove_file(source) {
        let _ = std::fs::remove_file(target);
        return Err(err(e));
    }
    Ok(())
}

pub(crate) fn recycle_directory(bin: &Path, folder: &Path) -> Result<PathBuf, String> {
    if !bin.is_absolute() || bin.starts_with(folder) {
        return Err("The recycle bin must be outside the movie folder.".into());
    }
    std::fs::create_dir_all(bin).map_err(err)?;
    let canonical = bin.canonicalize().map_err(err)?;
    if canonical.starts_with(folder.canonicalize().map_err(err)?) {
        return Err("The recycle bin must be outside the movie folder.".into());
    }
    Ok(canonical)
}

pub(crate) fn unused_target(dir: &Path, source: &Path) -> Result<PathBuf, String> {
    let mut candidate = dir.join(source.file_name().ok_or("Invalid movie file name")?);
    let mut number = 2;
    loop {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                candidate = dir.join(format!("{}_{number}{}", stem(source), extension(source)));
                number += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Err(e) => return Err(err(e)),
        }
    }
}

pub(crate) fn rollback(moved: &[(PathBuf, PathBuf)]) -> String {
    let mut problems = Vec::new();
    for (source, target) in moved.iter().rev() {
        if let Err(e) = move_file(target, source) {
            problems.push(format!("Could not restore {}: {e}", source.display()));
        }
    }
    if problems.is_empty() {
        String::new()
    } else {
        format!(" {}", problems.join("; "))
    }
}

pub async fn rename(db: &Database, id: i64, preview: &MovieRenamePreview) -> Result<(), String> {
    if rename_preview(db, id).await?.as_ref() != Some(preview) {
        return Err("The rename preview changed. Preview the movie again.".into());
    }
    let d = movies::get_detail(db, id).await.map_err(err)?;
    let (_, folder) = location(db, &d).await?;
    let src = child(
        &folder,
        Path::new(&preview.existing_path.replace('\\', "/")),
    )?;
    let dst = child(&folder, Path::new(&preview.new_path))?;
    let mut plan = vec![(src.clone(), dst.clone())];
    for source in sidecars(&src)? {
        let name = source.file_name().unwrap().to_string_lossy();
        let old = stem(&src);
        let suffix = name.strip_prefix(&old).ok_or("Invalid sidecar name")?;
        let target = dst.with_file_name(format!("{}{suffix}", stem(&dst)));
        plan.push((source, target));
    }
    for (_, target) in &plan {
        let relative = target.strip_prefix(&folder).map_err(err)?;
        child(&folder, relative)?;
        if target.try_exists().map_err(err)? {
            return Err(format!(
                "{} already exists and will not be overwritten.",
                target.display()
            ));
        }
    }
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.map_err(err)?;
    unchanged(&mut tx, &d).await?;
    let mut moved = Vec::new();
    for (source, target) in plan {
        if let Err(e) = move_file(&source, &target) {
            return Err(format!("{e}{}", rollback(&moved)));
        }
        moved.push((source, target));
    }
    let result=async {
        sqlx::query("UPDATE movie_files SET relative_path = ? WHERE id = ? AND movie_id = ?").bind(&preview.new_path).bind(preview.movie_file_id).bind(id).execute(&mut *tx).await.map_err(err)?;
        record(&mut tx,&d,"movieFileRenamed",&preview.existing_path,serde_json::json!({"sourceRelativePath":preview.existing_path,"relativePath":preview.new_path})).await?;
        tx.commit().await.map_err(err)
    }.await;
    if let Err(e) = result {
        return Err(format!("{e}{}", rollback(&moved)));
    }
    crate::metadata_files::after_movie_change(db, id).await;
    Ok(())
}

/// Explicit removal retains the film. Files are moved together with rollback on an I/O or
/// database error. Without a recycle bin, a private staging folder is purged after commit.
pub async fn delete_file(db: &Database, id: i64, file_id: i64) -> Result<(), String> {
    delete_file_inner(db, id, file_id, None).await
}

pub async fn delete_file_notifying(
    db: &Database,
    id: i64,
    file_id: i64,
    notifier: &kitsunarr_notifications::Notifier,
) -> Result<(), String> {
    delete_file_inner(db, id, file_id, Some(notifier)).await
}

async fn delete_file_inner(
    db: &Database,
    id: i64,
    file_id: i64,
    notifier: Option<&kitsunarr_notifications::Notifier>,
) -> Result<(), String> {
    let d = movies::get_detail(db, id).await.map_err(err)?;
    let f = d
        .file
        .as_ref()
        .filter(|f| f.id == file_id)
        .ok_or("The file is not in this movie.")?;
    let mut notification = if notifier.is_some() {
        crate::notify::movie_file_event(db, id, file_id, "Manual").await
    } else {
        None
    };
    let (root, folder) = location(db, &d).await?;
    let path = child(&folder, Path::new(&f.relative_path.replace('\\', "/")))?;
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.map_err(err)?;
    unchanged(&mut tx, &d).await?;
    // Write history first: a rejected database write must not remove a file.
    record(
        &mut tx,
        &d,
        "movieFileDeleted",
        &f.relative_path,
        serde_json::json!({"reason":"Manual"}),
    )
    .await?;
    sqlx::query("DELETE FROM movie_files WHERE id = ? AND movie_id = ?")
        .bind(file_id)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(err)?;
    let mut files = sidecars(&path)?;
    if path.is_file() {
        files.push(path.clone());
    }
    let bin = media
        .recycle_bin
        .as_deref()
        .filter(|b| !b.trim().is_empty());
    let stage = if bin.is_none() && !files.is_empty() {
        let p = folder.join(format!(
            ".kitsunarr-delete-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&p).map_err(err)?;
        Some(p)
    } else {
        None
    };
    let mut moved = Vec::new();
    let result = async {
        for source in files {
            let dir = if let Some(bin) = bin {
                let base = recycle_directory(Path::new(bin), &folder)?;
                let relative = source
                    .parent()
                    .ok_or("Invalid movie file")?
                    .strip_prefix(&root)
                    .map_err(err)?;
                let dir = child(&base, relative)?;
                std::fs::create_dir_all(&dir).map_err(err)?;
                dir
            } else {
                stage.clone().ok_or("Missing deletion staging folder")?
            };
            let target = unused_target(&dir, &source)?;
            move_file(&source, &target)?;
            moved.push((source, target));
        }
        tx.commit().await.map_err(err)
    }
    .await;
    if let Err(e) = result {
        let recovery = rollback(&moved);
        if let Some(stage) = stage {
            let _ = std::fs::remove_dir(stage);
        }
        return Err(format!("{e}{recovery}"));
    }
    if let Some(kitsunarr_notifications::Event::EpisodeFileDelete { file, .. }) = &mut notification
        && bin.is_some()
    {
        file.recycle_bin_path = moved
            .iter()
            .find(|(source, _)| source == &path)
            .map(|(_, target)| target.display().to_string());
    }
    if let (Some(notifier), Some(event)) = (notifier, notification) {
        crate::notify::spawn(db, notifier, event);
    }
    if let Some(stage) = stage {
        std::fs::remove_dir_all(stage).map_err(err)?;
    } else {
        for (_, target) in moved {
            crate::tools::touch(&target);
        }
    }

    Ok(())
}

/// Remove a film's own folder only, keeping the default title-removal operation read-only
/// towards disk. Whole-folder recycling stays on the same filesystem and never overwrites.
pub async fn delete_folder(db: &Database, id: i64) -> Result<(), String> {
    let d = movies::get_detail(db, id).await.map_err(err)?;
    let (_, folder) = location(db, &d).await?;
    if !folder.exists() {
        return Ok(());
    }
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    match media
        .recycle_bin
        .as_deref()
        .filter(|b| !b.trim().is_empty())
    {
        None => std::fs::remove_dir_all(&folder).map_err(err),
        Some(bin) => {
            let canonical = recycle_directory(Path::new(bin), &folder)?;
            let name = folder
                .file_name()
                .ok_or("Invalid movie folder")?
                .to_string_lossy();
            let mut target = canonical.join(name.as_ref());
            let mut number = 2;
            while target.try_exists().map_err(err)? {
                target = canonical.join(format!("{name}_{number}"));
                number += 1;
            }
            std::fs::rename(&folder, &target).map_err(err)?;
            crate::tools::touch_all(&target);
            Ok(())
        }
    }
}
