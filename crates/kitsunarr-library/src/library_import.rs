//! Sonarr's library import: folders of a root folder that no title uses
//! (`RootFolderService.GetUnmappedFolders`), and the checks an existing folder passes before a
//! title is added with it as its folder.

use std::collections::HashSet;
use std::path::{Component, Path};

use kitsunarr_db::{Database, root_folders, settings};
use kitsunarr_domain::MediaKind;
use kitsunarr_domain::api::library::{UnmappedFolder, folder_search_term};
use kitsunarr_domain::api::settings::{MediaManagementSettings, NamingSettings};

use crate::{LibraryError, LibraryResult};

/// Folders Sonarr never offers (`RootFolderService.SpecialFolders`).
const SPECIAL_FOLDERS: &[&str] = &[
    "$recycle.bin",
    "system volume information",
    "recycler",
    "lost+found",
    ".appledb",
    ".appledesktop",
    ".appledouble",
    "@eadir",
    ".grab",
];

/// Every series and film folder in the library, normalised.
async fn library_paths(db: &Database) -> LibraryResult<Vec<String>> {
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT path FROM series UNION ALL SELECT path FROM movies")
            .fetch_all(db.pool())
            .await
            .map_err(kitsunarr_db::DbError::from)?;
    Ok(paths
        .iter()
        .map(|p| root_folders::normalize_path(p))
        .collect())
}

fn is_inside(child: &str, parent: &str) -> bool {
    child.len() > parent.len()
        && child.starts_with(parent)
        && matches!(child.as_bytes()[parent.len()], b'/' | b'\\')
}

/// The unmapped folders of a root folder, by name. Series (or, in a film root folder, film)
/// folders sit as deep as the folder format has separators, as in Sonarr. A folder holding a
/// library title deeper down (a Tanukarr collection folder) is not offered, nor is the recycle
/// bin. A root folder that cannot be read has none.
pub async fn unmapped_folders(db: &Database, root_id: i64) -> LibraryResult<Vec<UnmappedFolder>> {
    let root = root_folders::list(db.pool())
        .await?
        .into_iter()
        .find(|r| r.id == root_id)
        .ok_or(LibraryError::Db(kitsunarr_db::DbError::NotFound))?;
    let root_path = root_folders::normalize_path(&root.path);
    let naming: NamingSettings = settings::get_or_default(db.pool(), settings::KEY_NAMING).await?;
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT).await?;
    let format = if root.media_kind.as_deref() == Some(MediaKind::Movie.as_str()) {
        &naming.movie_folder_format
    } else {
        &naming.series_folder_format
    };
    let depth = format.matches(['/', '\\']).count();
    let used = library_paths(db).await?;
    let recycle = media
        .recycle_bin
        .as_deref()
        .map(root_folders::normalize_path)
        .filter(|p| !p.is_empty());
    let root_for_scan = root_path.clone();
    let folders = tokio::task::spawn_blocking(move || {
        let mut level = vec![std::path::PathBuf::from(&root_for_scan)];
        for _ in 0..=depth {
            level = level
                .iter()
                .filter_map(|dir| std::fs::read_dir(dir).ok())
                .flat_map(|entries| entries.filter_map(Result::ok))
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
        }
        level
    })
    .await
    .unwrap_or_default();
    let used_set: HashSet<&str> = used.iter().map(String::as_str).collect();
    let mut result: Vec<UnmappedFolder> = folders
        .into_iter()
        .filter_map(|p| {
            let path = root_folders::normalize_path(&p.to_string_lossy());
            let name = p.file_name()?.to_string_lossy().to_string();
            if SPECIAL_FOLDERS.contains(&name.to_lowercase().as_str())
                || used_set.contains(path.as_str())
                || used.iter().any(|u| is_inside(u, &path))
                || recycle
                    .as_deref()
                    .is_some_and(|r| r == path || is_inside(&path, r) || is_inside(r, &path))
            {
                return None;
            }
            let relative_path = path
                .strip_prefix(&root_path)
                .unwrap_or(&path)
                .trim_start_matches(['/', '\\'])
                .to_string();
            let (search_term, year) = folder_search_term(&name);
            Some(UnmappedFolder {
                name,
                path,
                relative_path,
                search_term,
                year,
            })
        })
        .collect();
    result.sort_by_key(|f| (f.name.to_lowercase(), f.path.clone()));
    Ok(result)
}

/// Checks a folder chosen as a new title's folder: an existing directory strictly inside the
/// root folder, without `..`, that no other title uses or contains. Returns it normalised.
pub async fn validate_existing_folder(
    db: &Database,
    root: &str,
    path: &str,
) -> LibraryResult<String> {
    let invalid = |m: &str| LibraryError::invalid("path", m);
    let path = root_folders::normalize_path(path);
    let candidate = Path::new(&path);
    if !candidate.is_absolute()
        || candidate
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid("Choose an absolute folder path."));
    }
    if !is_inside(&path, root) {
        return Err(invalid("The folder must be inside the chosen root folder."));
    }
    let is_dir = tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_dir());
    if !is_dir {
        return Err(invalid("The folder does not exist."));
    }
    let used = library_paths(db).await?;
    if used
        .iter()
        .any(|u| *u == path || is_inside(u, &path) || is_inside(&path, u))
    {
        return Err(invalid("Another title already uses this folder."));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::is_inside;

    #[test]
    fn inside_needs_a_separator() {
        assert!(is_inside("/tv/Show", "/tv"));
        assert!(!is_inside("/tvshows/Show", "/tv"));
        assert!(!is_inside("/tv", "/tv"));
    }
}
