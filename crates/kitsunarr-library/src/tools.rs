//! Tanukarr's Library tools that read or restore files: the library check and the recycle-bin
//! browser with its guarded restore.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use kitsunarr_db::Database;
use kitsunarr_db::library::series;
use kitsunarr_db::settings;
use kitsunarr_domain::api::library_tools::{LibraryCheck, LibraryIssue, RecycleBin, RecycledFile};
use kitsunarr_domain::api::settings::{MediaManagementSettings, NamingSettings};

use crate::file_names;

/// Files listed by the recycle-bin browser.
const MAX_RECYCLED: usize = 2000;
/// Media and subtitle extensions the browser lists and restores.
const RESTORABLE: &[&str] = &[
    "mkv", "mp4", "avi", "m4v", "mov", "wmv", "mpg", "mpeg", "ts", "m2ts", "webm", "flv", "ogm",
    "divx", "xvid", "srt", "ass", "ssa", "sub", "idx", "vtt",
];

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// `LibraryAudit`: read-only. A missing series folder is reported once (its files are not
/// checked); each file record reports the first of: outside the series folder, a duplicate
/// record of the same path, missing on disk.
pub async fn check_library(db: &Database) -> Result<LibraryCheck, String> {
    let all: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, title, path FROM series ORDER BY sort_title")
            .fetch_all(db.pool())
            .await
            .map_err(err)?;
    let mut issues = Vec::new();
    let mut files_checked = 0i64;
    for (id, title, path) in &all {
        let folder = Path::new(path);
        let issue = |path: String, problem: &str| LibraryIssue {
            movie_id: None,
            series_id: *id,
            series_title: title.clone(),
            path,
            problem: problem.into(),
        };
        if !folder.is_dir() {
            issues.push(issue(path.clone(), "Series folder missing"));
            continue;
        }
        let mut seen = HashSet::new();
        for f in series::file_records(db, *id).await.map_err(err)? {
            files_checked += 1;
            let full = folder.join(&f.relative_path);
            let shown = full.display().to_string();
            if !inside(&f.relative_path) {
                issues.push(issue(shown, "Outside the series folder"));
            } else if !seen.insert(f.relative_path.to_lowercase()) {
                issues.push(issue(shown, "Duplicate file record"));
            } else if !full.is_file() {
                issues.push(issue(shown, "Missing on disk"));
            }
        }
    }
    let movies: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id,title,path FROM movies ORDER BY sort_title")
            .fetch_all(db.pool())
            .await
            .map_err(err)?;
    for (id, title, path) in &movies {
        let issue = |path: String, problem: &str| LibraryIssue {
            movie_id: Some(*id),
            series_id: 0,
            series_title: title.clone(),
            path,
            problem: problem.into(),
        };
        let d = kitsunarr_db::library::movies::get_detail(db, *id)
            .await
            .map_err(err)?;
        let folder = match crate::movie_files::location(db, &d).await {
            Ok((_, folder)) => folder,
            Err(message) => {
                issues.push(issue(path.clone(), &message));
                continue;
            }
        };
        if !folder.is_dir() {
            issues.push(issue(path.clone(), "Movie folder missing"));
            continue;
        }
        if let Some(file) = &d.file {
            files_checked += 1;
            match crate::movie_files::child(
                &folder,
                Path::new(&file.relative_path.replace('\\', "/")),
            ) {
                Err(message) => issues.push(issue(file.relative_path.clone(), &message)),
                Ok(full) if !full.is_file() => {
                    issues.push(issue(full.display().to_string(), "Missing on disk"))
                }
                Ok(_) => {}
            }
        }
    }
    Ok(LibraryCheck {
        checked_at: kitsunarr_db::now(),
        series_checked: all.len() as i64,
        movies_checked: movies.len() as i64,
        files_checked,
        issues,
    })
}

/// A relative path that stays inside its base (no root, no `..`).
fn inside(relative: &str) -> bool {
    let p = Path::new(relative);
    !relative.is_empty()
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn restorable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RESTORABLE.contains(&e.to_lowercase().as_str()))
}

async fn recycle_root(db: &Database) -> Result<Option<PathBuf>, String> {
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    Ok(media
        .recycle_bin
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from))
}

/// Marks a recycled file as recycled now (the cleanup's clock).
pub(crate) fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// [`touch`] for every file of a recycled folder.
pub(crate) fn touch_all(path: &Path) {
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            touch(&dir);
            continue;
        };
        for e in entries.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => touch(&e.path()),
                _ => {}
            }
        }
    }
}

/// `RecycleBinProvider.Cleanup`: deletes recycled files older than the "recycle bin cleanup"
/// days (0: never) and the folders left empty. Symbolic links are neither followed nor removed.
pub async fn clean_recycle_bin(db: &Database) -> Result<String, String> {
    let Some(root) = recycle_root(db).await? else {
        return Ok("The recycle bin is not set".into());
    };
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    let days = media.recycle_bin_cleanup_days;
    if days <= 0 {
        return Ok("Automatic cleanup of the recycle bin is off".into());
    }
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(days as u64 * 86_400);
    let (mut removed, mut kept) = (0usize, 0usize);
    let mut dirs = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                dirs.push(e.path());
                stack.push(e.path());
            } else if t.is_file() {
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|m| m < cutoff);
                if old && std::fs::remove_file(e.path()).is_ok() {
                    removed += 1;
                } else {
                    kept += 1;
                }
            }
        }
    }
    // Deepest first, so parents empty out after their children.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        let _ = std::fs::remove_dir(d);
    }
    Ok(format!(
        "{removed} files older than {days} days removed from the recycle bin, {kept} kept"
    ))
}

/// Lists the media and subtitle files of the recycle bin (symbolic links are skipped), newest
/// first, at most 2000.
pub async fn browse_recycle_bin(db: &Database) -> Result<RecycleBin, String> {
    let Some(root) = recycle_root(db).await? else {
        return Ok(RecycleBin {
            configured: false,
            path: None,
            truncated: false,
            files: Vec::new(),
        });
    };
    let mut files = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            let path = entry.path();
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && restorable(&path) {
                let Ok(relative) = path.strip_prefix(&root) else {
                    continue;
                };
                let relative = relative.to_string_lossy().into_owned();
                let modified = meta.modified().ok().map(|t| {
                    chrono::DateTime::<chrono::Utc>::from(t)
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                });
                files.push(RecycledFile {
                    token: base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .encode(relative.as_bytes()),
                    name: entry.file_name().to_string_lossy().into_owned(),
                    relative_path: relative,
                    size: meta.len(),
                    recycled_at: modified,
                });
            }
        }
    }
    files.sort_by(|a, b| {
        b.recycled_at
            .cmp(&a.recycled_at)
            .then(a.relative_path.cmp(&b.relative_path))
    });
    let truncated = files.len() > MAX_RECYCLED;
    files.truncate(MAX_RECYCLED);
    Ok(RecycleBin {
        configured: true,
        path: Some(root.display().to_string()),
        truncated,
        files,
    })
}

/// `ArchivePathGuard.Resolve`: the token's file inside the recycle bin, refusing absolute
/// paths, `..`, the bin itself and symbolic links on the way.
fn resolve(root: &Path, token: &str) -> Result<PathBuf, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.trim())
        .map_err(|_| "Unknown recycled file.".to_string())?;
    let relative = String::from_utf8(bytes).map_err(|_| "Unknown recycled file.".to_string())?;
    if !inside(&relative) {
        return Err("The recycled file is outside the recycle bin.".into());
    }
    let full = root.join(&relative);
    let mut cursor = full.clone();
    while cursor != root {
        let meta = std::fs::symlink_metadata(&cursor)
            .map_err(|_| "The recycled file no longer exists.".to_string())?;
        if meta.file_type().is_symlink() {
            return Err("Symbolic links are not restored.".into());
        }
        if !cursor.pop() {
            return Err("The recycled file is outside the recycle bin.".into());
        }
    }
    Ok(full)
}

/// Moves a recycled file back into a series' season folder (never overwriting) and rescans
/// the series, which registers it like any file found on disk.
pub async fn restore_recycled(
    db: &Database,
    token: &str,
    series_id: i64,
    season: i32,
) -> Result<String, String> {
    if token.is_empty() || token.len() > 4096 {
        return Err("Unknown recycled file.".into());
    }
    let root = recycle_root(db)
        .await?
        .ok_or("No recycle bin is configured.")?;
    let source = resolve(&root, token)?;
    if !source.is_file() || !restorable(&source) {
        return Err("Only media and subtitle files are restored.".into());
    }
    let detail = series::get_detail(db, series_id).await.map_err(err)?;
    if !detail.seasons.iter().any(|s| s.season_number == season) {
        return Err(format!("The series has no season {season}."));
    }
    let series_path = PathBuf::from(&detail.item.path);
    if !series_path.is_dir() {
        return Err("The series folder does not exist.".into());
    }
    let naming: NamingSettings = settings::get_or_default(db.pool(), settings::KEY_NAMING)
        .await
        .map_err(err)?;
    let (naming_series, _) = crate::files::naming_series(db, &detail).await?;
    let dir = match file_names::season_path(&naming_series, season, detail.season_folder, &naming) {
        Some(s) => series_path.join(s),
        None => series_path.clone(),
    };
    let name = source.file_name().ok_or("Unknown recycled file.")?;
    let target = dir.join(name);
    if target.exists() {
        return Err(format!(
            "{} exists and will not be overwritten.",
            target.display()
        ));
    }
    std::fs::create_dir_all(&dir).map_err(err)?;
    if std::fs::rename(&source, &target).is_err() {
        std::fs::copy(&source, &target).map_err(err)?;
        std::fs::remove_file(&source).map_err(err)?;
    }
    let scan = crate::files::scan_series(db, series_id).await?;
    Ok(format!(
        "Restored to {} ({} files added to the library).",
        target.display(),
        scan.added
    ))
}

/// Movie restore uses the same path and rollback guards as manual movie renames. A feature
/// brings its named sidecars; a subtitle alone must belong to the movie's current feature.
pub async fn restore_movie(db: &Database, token: &str, movie_id: i64) -> Result<String, String> {
    use crate::movie_files as files;
    use kitsunarr_db::library::movies;
    if token.is_empty() || token.len() > 4096 {
        return Err("Unknown recycled file.".into());
    }
    let bin = recycle_root(db)
        .await?
        .ok_or("No recycle bin is configured.")?;
    if !bin.is_absolute() {
        return Err("The recycle bin must have an absolute path.".into());
    }
    let bin = bin.canonicalize().map_err(err)?;
    let source = resolve(&bin, token)?;
    if !source.is_file() || !restorable(&source) {
        return Err("Only media and subtitle files are restored.".into());
    }
    let d = movies::get_detail(db, movie_id).await.map_err(err)?;
    let (_, folder) = files::location(db, &d).await?;
    let video =
        crate::import::VIDEO_EXTENSIONS.contains(&crate::import::extension(&source).as_str());
    if video && (d.file.is_some() || !crate::import::video_files(&folder).is_empty()) {
        return Err(
            "The movie already has a feature. Remove it explicitly before restoring another."
                .into(),
        );
    }
    if !video {
        let f = d
            .file
            .as_ref()
            .ok_or("Restore the movie feature before its subtitles.")?;
        let base = crate::import::stem(Path::new(&f.relative_path));
        let subtitle = crate::import::stem(&source);
        if subtitle != base
            && !subtitle
                .strip_prefix(&base)
                .is_some_and(|s| s.starts_with('.'))
        {
            return Err("The subtitle does not name the movie's current feature.".into());
        }
    }
    let mut sources = vec![source.clone()];
    if video {
        sources.extend(files::sidecars(&source)?);
    }
    let mut plan = Vec::new();
    for path in sources {
        let target = files::child(
            &folder,
            Path::new(path.file_name().ok_or("Invalid recycled file")?),
        )?;
        if target.try_exists().map_err(err)? {
            return Err(format!(
                "{} exists and will not be overwritten.",
                target.display()
            ));
        }
        plan.push((path, target));
    }
    let target = plan[0].1.clone();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.map_err(err)?;
    files::unchanged(&mut tx, &d).await?;
    std::fs::create_dir_all(&folder).map_err(err)?;
    let mut moved = Vec::new();
    for (source, target) in plan {
        if let Err(e) = files::move_file(&source, &target) {
            return Err(format!("{e}{}", files::rollback(&moved)));
        }
        moved.push((source, target));
    }
    let result=async {
        if video {
            let relative=target.file_name().ok_or("Invalid restored path")?.to_string_lossy().into_owned();
            let f=crate::movie_manual_import::file_record(movie_id,&target,&relative,None)?;
            movies::upsert_file(&mut tx,&f).await.map_err(err)?;
        }
        files::record(&mut tx,&d,"movieFileRestored",&source.display().to_string(),serde_json::json!({"relativePath":target.file_name().map(|p|p.to_string_lossy()),"withSubtitles":video})).await?;
        tx.commit().await.map_err(err)
    }.await;
    if let Err(e) = result {
        return Err(format!("{e}{}", files::rollback(&moved)));
    }
    crate::metadata_files::after_movie_change(db, movie_id).await;
    Ok(format!(
        "Restored to {} ({} files restored).",
        target.display(),
        moved.len()
    ))
}

/// Tanukarr's storage outlook extended to native film rows. Each download belongs to the
/// most specific registered root, and duplicate client/hash rows count only once per root.
pub fn storage_outlook(
    roots: &[String],
    series: &[(i64, String)],
    movies: &[(i64, String)],
    queue: &[kitsunarr_domain::api::queue::QueueItem],
    hardlink_only: bool,
    use_hardlinks: bool,
) -> Vec<kitsunarr_domain::api::library_tools::StorageOutlook> {
    use kitsunarr_domain::api::library_tools::StorageOutlook;
    let mut downloads =
        vec![std::collections::HashMap::<(String, String), (u64, u64)>::new(); roots.len()];
    for item in queue {
        if matches!(
            item.tracked_state.as_str(),
            "imported" | "ignored" | "failed" | "pending"
        ) {
            continue;
        }
        let path = item
            .movie_id
            .and_then(|id| movies.iter().find(|(mid, _)| *mid == id))
            .or_else(|| {
                item.series_id
                    .and_then(|id| series.iter().find(|(sid, _)| *sid == id))
            })
            .map(|(_, path)| Path::new(path));
        let Some(path) = path else {
            continue;
        };
        let Some(index) = roots
            .iter()
            .enumerate()
            .filter(|(_, root)| path.starts_with(root))
            .max_by_key(|(_, root)| Path::new(root).components().count())
            .map(|(index, _)| index)
        else {
            continue;
        };
        let entry = downloads[index]
            .entry((
                item.download_client.to_lowercase(),
                item.download_id.to_ascii_lowercase(),
            ))
            .or_default();
        entry.0 = entry.0.max(item.size_left.max(0) as u64);
        entry.1 = entry.1.max(item.size.max(0) as u64);
    }
    roots
        .iter()
        .zip(downloads)
        .map(|(root, downloads)| {
            let (remaining, total) =
                downloads
                    .values()
                    .fold((0u64, 0u64), |(remaining, total), (left, size)| {
                        (remaining.saturating_add(*left), total.saturating_add(*size))
                    });
            let estimate = if hardlink_only { 0 } else { total };
            let free = fs4::available_space(root).ok();
            StorageOutlook {
                root_folder: root.clone(),
                free_space: free,
                queued_downloads: downloads.len() as i64,
                download_remaining: remaining,
                import_space_estimate: estimate,
                may_run_out_of_space: free.is_some_and(|f| estimate > f),
                import_mode: if hardlink_only {
                    "Hardlink-only: imports take no extra space"
                } else if use_hardlinks {
                    "Hard link when possible, else copy: the estimate assumes copies"
                } else {
                    "Copy: imports take the downloads' size again"
                }
                .into(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    #[test]
    fn tokens_cannot_leave_the_recycle_bin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Show")).unwrap();
        std::fs::write(dir.path().join("Show/a.mkv"), b"x").unwrap();
        let enc = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        assert!(super::resolve(dir.path(), &enc("Show/a.mkv")).is_ok());
        assert!(super::resolve(dir.path(), &enc("../etc/passwd")).is_err());
        assert!(super::resolve(dir.path(), &enc("/etc/passwd")).is_err());
        assert!(super::resolve(dir.path(), "not base64!").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("Show"), dir.path().join("link")).unwrap();
            assert!(super::resolve(dir.path(), &enc("link/a.mkv")).is_err());
        }
    }
}
