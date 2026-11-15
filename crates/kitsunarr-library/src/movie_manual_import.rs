//! Manual film import: Tanukarr's `MovieReleaseParser` and Sonarr's `ManualImportService`,
//! with a reviewed file/movie
//! association. Files are staged before publishing, and file/history writes commit together.

use crate::import::{aggregate_quality_with_media, extension, stem, video_files};
use crate::movie_files as files;
use kitsunarr_db::library::movies;
use kitsunarr_db::{Database, settings};
use kitsunarr_domain::api::library_tools::{ManualImportResult, MovieImportFile, MovieImportItem};
use kitsunarr_domain::api::settings::{MediaManagementSettings, NamingSettings};
use std::path::{Path, PathBuf};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Source paths are chosen explicitly, but a symlink or parent traversal may never turn an
/// import into an operation on a different file. Scans and imports share this validation.
fn source_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("Enter an absolute source path.".into());
    }
    let root = path.ancestors().last().ok_or("Invalid source root")?;
    let relative = path.strip_prefix(root).map_err(err)?;
    files::child(root, relative)
}

pub(crate) fn file_record(
    id: i64,
    path: &Path,
    relative: &str,
    info: Option<&crate::media_info::MediaInfo>,
) -> Result<movies::NewMovieFile, String> {
    let name = stem(path);
    let quality = aggregate_quality_with_media(&[kitsunarr_parser::parse_quality(&name)], info);
    let languages = kitsunarr_parser::parse_languages(&name)
        .iter()
        .map(|l| l.id)
        .collect();
    Ok(movies::NewMovieFile {
        movie_id: id,
        relative_path: relative.into(),
        size: std::fs::metadata(path).map_err(err)?.len() as i64,
        added_at: kitsunarr_db::now(),
        scene_name: Some(name.clone()),
        release_group: kitsunarr_parser::parse_release_group(&name),
        quality_id: quality.quality_id,
        revision_version: quality.revision.version,
        revision_real: quality.revision.real,
        is_repack: quality.revision.is_repack,
        languages,
        media_info: info.and_then(|i| serde_json::to_value(i).ok()),
        original_path: None,
        edition: None,
    })
}

/// Match against library film identities once per scan; ambiguous titles stay unselected.
pub async fn scan(
    db: &Database,
    path: &Path,
    movie_id: Option<i64>,
) -> Result<Vec<MovieImportItem>, String> {
    let path = source_path(path)?;
    if !path.exists() {
        return Err("The source path does not exist.".into());
    }
    type Film = (
        i64,
        String,
        Option<String>,
        Option<i32>,
        String,
        Option<String>,
    );
    let films:Vec<Film>=sqlx::query_as("SELECT id,title,original_title,year,metadata_provider,(SELECT external_id FROM movie_external_ids e WHERE e.movie_id=movies.id AND kind='imdb' LIMIT 1) FROM movies ORDER BY sort_title").fetch_all(db.pool()).await.map_err(err)?;
    if movie_id.is_some_and(|id| !films.iter().any(|m| m.0 == id)) {
        return Err("The chosen movie is not in the library.".into());
    }
    let list = if path.is_dir() {
        video_files(&path)
    } else {
        vec![path.clone()]
    };
    if list.len() > 2000 {
        return Err("Scan at most 2000 video files at once.".into());
    }
    let mut out = Vec::new();
    for file in list {
        let mut rejections = Vec::new();
        if !crate::import::VIDEO_EXTENSIONS.contains(&extension(&file).as_str()) {
            continue;
        }
        source_path(&file)?;
        let name = file
            .file_name()
            .ok_or("Invalid source file")?
            .to_string_lossy()
            .into_owned();
        let mut matching = Vec::new();
        for (id, title, original, year, provider, imdb) in &films {
            let c = kitsunarr_parser::movie::MovieCandidate {
                title,
                original_title: original.as_deref(),
                alternate_titles: &[],
                year: year.unwrap_or(0),
                imdb_id: imdb.as_deref(),
                anime_catalogue: matches!(provider.as_str(), "aniList" | "myAnimeList"),
                collection: None,
            };
            if movie_id == Some(*id)
                || (movie_id.is_none() && kitsunarr_parser::movie::match_movie(&name, &c).is_some())
            {
                matching.push((*id, title.clone()));
            }
        }
        let selected = if matching.len() == 1 {
            matching.pop()
        } else {
            rejections.push(
                if matching.is_empty() {
                    "Unknown movie: choose its library title."
                } else {
                    "Several movies match: choose its library title."
                }
                .into(),
            );
            None
        };
        if stem(&file)
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|s| s.eq_ignore_ascii_case("sample"))
        {
            rejections.push("Sample".into());
        }
        if let Some((id, _)) = &selected {
            let d = movies::get_detail(db, *id).await.map_err(err)?;
            if d.file.is_some() {
                rejections.push(
                    "The movie already has a feature; only an upgrade can replace it.".into(),
                );
            }
            if let Err(message) = files::location(db, &d).await {
                rejections.push(message);
            }
        }
        out.push(MovieImportItem {
            path: file.display().to_string(),
            name,
            size: std::fs::metadata(&file).map_err(err)?.len() as i64,
            movie_id: selected.as_ref().map(|m| m.0),
            movie_title: selected.map(|m| m.1),
            quality: kitsunarr_parser::parse_quality(&stem(&file))
                .quality
                .name
                .into(),
            rejections,
        });
    }
    Ok(out)
}

/// Copy without overwriting, retaining permissions and timestamps. A partial copy is removed.
fn copy_file(
    source: &Path,
    target: &Path,
    hardlinks: bool,
    hardlink_only: bool,
) -> Result<(), String> {
    if hardlink_only {
        return std::fs::hard_link(source, target)
            .map_err(|e| format!("Hardlink-only import cannot link this file ({e})."));
    }
    if hardlinks && std::fs::hard_link(source, target).is_ok() {
        return Ok(());
    }
    let mut input = std::fs::File::open(source).map_err(err)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(err)?;
    let result = (|| -> std::io::Result<()> {
        std::io::copy(&mut input, &mut output)?;
        output.set_permissions(input.metadata()?.permissions())?;
        if let Ok(time) = input.metadata()?.modified() {
            output.set_modified(time)?;
        }
        output.sync_all()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(target);
        return Err(err(e));
    }
    Ok(())
}

pub async fn import(
    db: &Database,
    selected: &[MovieImportFile],
    move_files: bool,
) -> Vec<ManualImportResult> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for file in selected {
        let result = if !seen.insert(file.movie_id) {
            Err("Choose only one feature per movie in an import.".into())
        } else {
            import_one(db, file, move_files).await
        };
        match result {
            Ok(messages) => out.push(ManualImportResult {
                path: file.path.clone(),
                imported: true,
                messages,
            }),
            Err(message) => out.push(ManualImportResult {
                path: file.path.clone(),
                imported: false,
                messages: vec![message],
            }),
        }
    }
    out
}

async fn import_one(
    db: &Database,
    selected: &MovieImportFile,
    move_files: bool,
) -> Result<Vec<String>, String> {
    let source = source_path(Path::new(&selected.path))?;
    if !source.is_file() || !crate::import::VIDEO_EXTENSIONS.contains(&extension(&source).as_str())
    {
        return Err("Choose a video file.".into());
    }
    if stem(&source)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|s| s.eq_ignore_ascii_case("sample"))
    {
        return Err("Sample".into());
    }
    let d = movies::get_detail(db, selected.movie_id)
        .await
        .map_err(err)?;
    let (root, folder) = files::location(db, &d).await?;
    let library_paths: Vec<String> =
        sqlx::query_scalar("SELECT path FROM movies UNION SELECT path FROM series")
            .fetch_all(db.pool())
            .await
            .map_err(err)?;
    if library_paths.iter().any(|p| source.starts_with(p)) {
        return Err(
            "The source is already inside a library title. Use its disk scan instead.".into(),
        );
    }
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    let tools: kitsunarr_domain::api::settings::LibraryToolsSettings =
        settings::get_or_default(db.pool(), settings::KEY_LIBRARY_TOOLS)
            .await
            .map_err(err)?;
    let info = if media.enable_media_info {
        crate::media_info::reader::read(&source).await
    } else {
        None
    };
    if info
        .as_ref()
        .is_some_and(|i| i.is_sample(d.runtime.unwrap_or(0)))
    {
        return Err("Sample".into());
    }
    let mut file = file_record(d.item.id, &source, "", info.as_ref())?;
    let profile = kitsunarr_db::quality::get_profile(db.pool(), d.item.quality_profile_id)
        .await
        .map_err(err)?;
    if let Some(existing) = &d.file {
        let order = kitsunarr_decision::ProfileView(&profile)
            .compare_qualities(file.quality_id, existing.quality_id);
        let (v, r, p): (i32, i32, bool) = sqlx::query_as(
            "SELECT revision_version,revision_real,is_repack FROM movie_files WHERE id=?",
        )
        .bind(existing.id)
        .fetch_one(db.pool())
        .await
        .map_err(err)?;
        let revision = kitsunarr_domain::quality::Revision {
            version: v,
            real: r,
            is_repack: p,
        };
        let incoming = kitsunarr_domain::quality::Revision {
            version: file.revision_version,
            real: file.revision_real,
            is_repack: file.is_repack,
        };
        if order.is_lt()
            || (order.is_eq()
                && !kitsunarr_decision::profile::compare_revision(&incoming, &revision).is_gt())
        {
            return Err("Not an upgrade for the existing movie feature.".into());
        }
    }
    let naming: NamingSettings = settings::get_or_default(db.pool(), settings::KEY_NAMING)
        .await
        .map_err(err)?;
    let mut quality = crate::notify::quality_name(i64::from(file.quality_id));
    if file.revision_version > 1 {
        quality.push_str(" Proper");
    }
    if file.revision_real > 0 {
        quality.push_str(" REAL");
    }
    let base = if naming.rename_episodes {
        kitsunarr_domain::naming::movie_file_name(
            &naming.movie_file_format,
            &d.item.title,
            d.item.year,
            &quality,
            file.release_group.as_deref(),
            naming.colon_replacement_format,
            &naming.custom_colon_replacement_format,
        )
    } else {
        stem(&source)
    };
    if base.is_empty() {
        return Err("The movie naming format produced an empty name.".into());
    }
    file.relative_path = format!("{base}{}", extension(&source));
    file.original_path = Some(source.display().to_string());
    let target = files::child(&folder, Path::new(&file.relative_path))?;
    let mut incoming = vec![(source.clone(), target.clone())];
    for extra in files::sidecars(&source)? {
        let old = stem(&source);
        let name = extra
            .file_name()
            .ok_or("Invalid subtitle")?
            .to_string_lossy();
        let suffix = name.strip_prefix(&old).ok_or("Invalid subtitle")?;
        let dest = folder.join(format!("{base}{suffix}"));
        incoming.push((extra, dest));
    }
    let mut old = Vec::new();
    if let Some(existing) = &d.file {
        let path = files::child(
            &folder,
            Path::new(&existing.relative_path.replace('\\', "/")),
        )?;
        if path.is_file() {
            old.extend(files::sidecars(&path)?);
            old.push(path);
        }
    }
    for (_, dest) in &incoming {
        files::child(&folder, dest.strip_prefix(&folder).map_err(err)?)?;
        if dest.try_exists().map_err(err)? && !old.contains(dest) {
            return Err(format!(
                "{} exists and will not be overwritten.",
                dest.display()
            ));
        }
    }
    std::fs::create_dir_all(&folder).map_err(err)?;
    let stage = folder.join(format!(
        ".kitsunarr-import-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir(&stage).map_err(err)?;
    let staging=async {
        let mut staged=Vec::new();
        for (index,(src,dest)) in incoming.iter().enumerate() {
            let tmp=stage.join(format!("new-{index}"));copy_file(src,&tmp,media.use_hardlinks,tools.hardlink_only && !move_files)?;
            staged.push((tmp,dest.clone()));
        }
        let mut tx=db.pool().begin_with("BEGIN IMMEDIATE").await.map_err(err)?;
        files::unchanged(&mut tx,&d).await?;
        let mut moved=Vec::new();
        let publication=async {
            for (index,old) in old.iter().enumerate() {
                let backup=stage.join(format!("old-{index}"));files::move_file(old,&backup)?;moved.push((old.clone(),backup));
            }
            for (tmp,dest) in &staged {files::move_file(tmp,dest)?;moved.push((tmp.clone(),dest.clone()));}
            movies::upsert_file(&mut tx,&file).await.map_err(err)?;
            files::record(&mut tx,&d,"downloadFolderImported",&stem(&source),serde_json::json!({"releaseSource":"ManualImport","droppedPath":source,"importedPath":target})).await?;
            tx.commit().await.map_err(err)
        }.await;
        if let Err(message)=publication {
            let recovery=files::rollback(&moved);
            if !recovery.is_empty() {return Err(format!("{message}{recovery} Previous files remain in {}.",stage.display()));}
            return Err(message);
        }
        Ok(())
    }.await;
    if let Err(message) = staging {
        // After rollback, backups have returned to their original names. Any remaining old
        // backup is retained for recovery, never purged with the disposable new copies.
        let has_backup = std::fs::read_dir(&stage).is_ok_and(|mut entries| {
            entries.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("old-")))
        });
        if !has_backup {
            let _ = std::fs::remove_dir_all(&stage);
        }
        return Err(message);
    }
    let mut messages = Vec::new();
    for (index, original) in old.iter().enumerate() {
        let backup = stage.join(format!("old-{index}"));
        let result = if let Some(bin) = media
            .recycle_bin
            .as_deref()
            .filter(|b| !b.trim().is_empty())
        {
            (|| -> Result<(), String> {
                let bin = files::recycle_directory(Path::new(bin), &folder)?;
                let parent = original
                    .parent()
                    .ok_or("Invalid old feature")?
                    .strip_prefix(&root)
                    .map_err(err)?;
                let dir = files::child(&bin, parent)?;
                std::fs::create_dir_all(&dir).map_err(err)?;
                let target = files::unused_target(&dir, original)?;
                files::move_file(&backup, &target)?;
                crate::tools::touch(&target);
                Ok(())
            })()
        } else {
            std::fs::remove_file(&backup).map_err(err)
        };
        if let Err(e) = result {
            messages.push(format!(
                "Previous file retained at {}: {e}",
                backup.display()
            ));
        }
    }
    if move_files {
        for (src, _) in &incoming {
            if let Err(e) = std::fs::remove_file(src) {
                messages.push(format!(
                    "Imported, but the source {} was kept: {e}",
                    src.display()
                ));
            }
        }
    }
    let _ = std::fs::remove_dir(&stage);
    crate::metadata_files::after_movie_change(db, d.item.id).await;
    Ok(messages)
}
