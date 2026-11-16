//! Sonarr's `MoveSeriesService` for series and films: a title's folder changes, with its
//! contents moved there (a rename on one file system, else a copy then removal) or, without
//! "move files", only the recorded folder. Episode, film and retained-version files are stored
//! relative to the title's folder, so their records follow without changes.
//!
//! Kitsunarr is stricter than upstream about the destination: it must lie inside a root
//! folder, must not overlap another title's folder, and must be missing or empty when files
//! are moved (upstream merges into an existing folder).

use std::path::{Component, Path, PathBuf};

use kitsunarr_db::{Database, DbError, root_folders};
use kitsunarr_domain::api::library::MoveResult;

use crate::{LibraryError, LibraryResult};

/// A series or a film.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Title {
    Series(i64),
    Movie(i64),
}

impl Title {
    fn table(self) -> &'static str {
        match self {
            Title::Series(_) => "series",
            Title::Movie(_) => "movies",
        }
    }
    fn id(self) -> i64 {
        match self {
            Title::Series(id) | Title::Movie(id) => id,
        }
    }
}

fn invalid(m: impl Into<String>) -> LibraryError {
    LibraryError::Invalid {
        field: "path",
        message: m.into(),
    }
}

fn inside(child: &str, parent: &str) -> bool {
    child.len() > parent.len()
        && child.starts_with(parent)
        && matches!(child.as_bytes()[parent.len()], b'/' | b'\\')
}

struct Current {
    title: String,
    path: String,
    root: String,
    files: i64,
}

async fn current(db: &Database, title: Title) -> LibraryResult<Current> {
    let (sql, files_sql) = match title {
        Title::Series(_) => (
            "SELECT title, path, root_folder_path FROM series WHERE id = ?",
            "SELECT count(*) FROM episode_files WHERE series_id = ?",
        ),
        Title::Movie(_) => (
            "SELECT title, path, root_folder_path FROM movies WHERE id = ?",
            "SELECT count(*) FROM movie_files WHERE movie_id = ?",
        ),
    };
    let (name, path, root): (String, String, String) = sqlx::query_as(sql)
        .bind(title.id())
        .fetch_optional(db.pool())
        .await
        .map_err(DbError::from)?
        .ok_or(LibraryError::Db(DbError::NotFound))?;
    let files: i64 = sqlx::query_scalar(files_sql)
        .bind(title.id())
        .fetch_one(db.pool())
        .await
        .map_err(DbError::from)?;
    Ok(Current {
        title: name,
        path: root_folders::normalize_path(&path),
        root: root_folders::normalize_path(&root),
        files,
    })
}

/// The root folder holding `path` (the deepest one), checked against the other titles.
async fn check_destination(db: &Database, title: Title, path: &str) -> LibraryResult<String> {
    let p = Path::new(path);
    if !p.is_absolute()
        || p.components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid("Choose an absolute folder path."));
    }
    let roots = root_folders::list(db.pool()).await?;
    let root = roots
        .iter()
        .map(|r| root_folders::normalize_path(&r.path))
        .filter(|r| inside(path, r))
        .max_by_key(String::len)
        .ok_or_else(|| invalid("The folder must be inside one of the root folders."))?;
    let others: Vec<String> = sqlx::query_scalar(
        "SELECT path FROM series WHERE NOT (?1 = 'series' AND id = ?2) \
         UNION ALL SELECT path FROM movies WHERE NOT (?1 = 'movies' AND id = ?2)",
    )
    .bind(title.table())
    .bind(title.id())
    .fetch_all(db.pool())
    .await
    .map_err(DbError::from)?;
    if others
        .iter()
        .map(|o| root_folders::normalize_path(o))
        .any(|o| o == path || inside(&o, path) || inside(path, &o))
    {
        return Err(invalid("Another title already uses this folder."));
    }
    Ok(root)
}

async fn record(db: &Database, title: Title, path: &str, root: &str) -> LibraryResult<()> {
    let sql = match title {
        Title::Series(_) => "UPDATE series SET path = ?, root_folder_path = ? WHERE id = ?",
        Title::Movie(_) => "UPDATE movies SET path = ?, root_folder_path = ? WHERE id = ?",
    };
    sqlx::query(sql)
        .bind(path)
        .bind(root)
        .bind(title.id())
        .execute(db.pool())
        .await
        .map_err(|e| DbError::from_write(e, "path"))?;
    Ok(())
}

/// Moves a folder: a rename, else (across file systems) a copy then removal of the source. A
/// failed copy removes what it wrote and leaves the source as it was.
fn transfer(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    if destination.is_dir() {
        // Checked empty by the caller.
        std::fs::remove_dir(destination).map_err(|e| e.to_string())?;
    }
    match std::fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            if let Err(e) = crate::files::copy_dir(source, destination) {
                let _ = std::fs::remove_dir_all(destination);
                return Err(format!("Copying the folder failed: {e}"));
            }
            std::fs::remove_dir_all(source).map_err(|e| {
                format!("The folder was copied but the original could not be removed: {e}")
            })
        }
        Err(e) => Err(format!("Moving the folder failed: {e}")),
    }
}

/// Changes a title's folder, moving its contents when `move_files` is set (Sonarr's
/// `MoveSingleSeries`): an unchanged folder only records it; a missing source with an existing
/// destination counts as already moved; a missing source and destination is refused when the
/// title has files, else recorded.
pub async fn move_title(
    db: &Database,
    title: Title,
    destination: &str,
    move_files: bool,
) -> LibraryResult<MoveResult> {
    let destination = root_folders::normalize_path(destination);
    let now = current(db, title).await?;
    let root = check_destination(db, title, &destination).await?;
    let result = |moved: bool, message: String| MoveResult {
        id: title.id(),
        path: destination.clone(),
        root_folder_path: root.clone(),
        moved,
        message,
    };
    if destination == now.path {
        record(db, title, &destination, &root).await?;
        return Ok(result(
            false,
            format!("{} is already in {destination}.", now.title),
        ));
    }
    if !move_files {
        record(db, title, &destination, &root).await?;
        return Ok(result(
            false,
            format!(
                "{} now uses {destination}; no files were moved. Scan the disk to pick up its files.",
                now.title
            ),
        ));
    }
    if inside(&destination, &now.path) || inside(&now.path, &destination) {
        return Err(invalid(
            "The new folder cannot be inside the current one, or hold it.",
        ));
    }
    let (source, target) = (PathBuf::from(&now.path), PathBuf::from(&destination));
    let (source_exists, target_state) = {
        let (s, t) = (source.clone(), target.clone());
        tokio::task::spawn_blocking(move || {
            let state = match std::fs::read_dir(&t) {
                Ok(mut entries) => Some(entries.next().is_none()),
                Err(_) => None,
            };
            (s.is_dir(), state)
        })
        .await
        .map_err(|e| invalid(e.to_string()))?
    };
    if !source_exists {
        if target_state.is_some() {
            record(db, title, &destination, &root).await?;
            return Ok(result(
                false,
                format!("{} was already in {destination}.", now.title),
            ));
        }
        if now.files > 0 {
            return Err(invalid(
                "Neither folder exists although the title has files; check the drive. The current folder was kept.",
            ));
        }
        record(db, title, &destination, &root).await?;
        return Ok(result(
            false,
            format!("{} now uses {destination} (nothing to move).", now.title),
        ));
    }
    if target_state == Some(false) {
        return Err(invalid("The new folder already exists and is not empty."));
    }
    tokio::task::spawn_blocking(move || transfer(&source, &target))
        .await
        .map_err(|e| invalid(e.to_string()))?
        .map_err(invalid)?;
    if let Err(e) = record(db, title, &destination, &root).await {
        // Put the folder back so the record and the disk agree.
        let (s, t) = (PathBuf::from(&destination), PathBuf::from(&now.path));
        let _ = tokio::task::spawn_blocking(move || transfer(&s, &t)).await;
        return Err(e);
    }
    tracing::info!(title = %now.title, from = %now.path, to = %destination, "title folder moved");
    Ok(result(
        true,
        format!("{} moved to {destination}.", now.title),
    ))
}

/// The folder a series gets in another root folder: its path below its current root folder
/// (keeping a group's `{group}/{member}` layout), else its folder name.
pub async fn destination_in_root(db: &Database, title: Title, root: &str) -> LibraryResult<String> {
    let now = current(db, title).await?;
    let root = root_folders::normalize_path(root);
    let relative = if inside(&now.path, &now.root) {
        now.path[now.root.len() + 1..].to_string()
    } else {
        Path::new(&now.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or(now.title)
    };
    Ok(format!("{root}/{relative}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_copy_is_not_needed_for_a_rename() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a/Show");
        std::fs::create_dir_all(source.join("Season 01")).unwrap();
        std::fs::write(source.join("Season 01/e.mkv"), b"x").unwrap();
        let target = dir.path().join("b/nested/Show");
        transfer(&source, &target).unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(target.join("Season 01/e.mkv")).unwrap(), b"x");
        // An empty destination is replaced.
        let empty = dir.path().join("c/Show");
        std::fs::create_dir_all(&empty).unwrap();
        transfer(&target, &empty).unwrap();
        assert!(empty.join("Season 01/e.mkv").is_file());
    }
}
