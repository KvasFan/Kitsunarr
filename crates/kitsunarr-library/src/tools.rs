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
    Ok(LibraryCheck {
        checked_at: kitsunarr_db::now(),
        series_checked: all.len() as i64,
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
    let naming_series = file_names::Series {
        season_titles: detail
            .seasons
            .iter()
            .filter_map(|s| s.title.clone().map(|t| (s.season_number, t)))
            .collect(),
        ..file_names::Series::from_parts(
            &detail.item.title,
            detail.item.year,
            detail.item.series_type,
            &detail.external_ids,
        )
    };
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
