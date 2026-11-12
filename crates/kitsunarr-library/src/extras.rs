//! Extra files imported with episodes: Sonarr's `ExtraService`, `SubtitleService` and
//! `OtherExtraService`.
//!
//! With "Import extra files" on, files next to an imported episode whose extension is in the
//! list are brought along: subtitles named for the video, in a subfolder named for it, or
//! numbering the same episode (or every subtitle when the folder holds a single video), named
//! after the episode file with their language (`Episode.en.forced.srt`); other extras named for
//! the video take the episode file's name. They are recorded so upgrades remove them.

use std::path::{Path, PathBuf};

use kitsunarr_db::Database;
use kitsunarr_domain::api::settings::MediaManagementSettings;
use kitsunarr_parser::{
    ParsedEpisodeInfo, iso_for, parse_path, parse_subtitle_language_information, parse_title,
};

/// `SubtitleFileExtensions.Extensions`.
pub const SUBTITLE_EXTENSIONS: &[&str] = &[
    ".aqt", ".ass", ".idx", ".jss", ".psb", ".rt", ".smi", ".srt", ".ssa", ".sub", ".txt", ".utf",
    ".utf8", ".utf-8", ".vtt",
];

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn walk(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && recursive {
            walk(&e.path(), true, out);
        } else if t.is_file() {
            out.push(e.path());
        }
    }
}

/// What an import passes for one imported episode file.
pub struct ExtraImport<'a> {
    pub series_id: i64,
    pub series_path: &'a Path,
    pub season_number: i32,
    pub episode_file_id: i64,
    /// The episode file's path in the library.
    pub episode_file: &'a Path,
    /// The imported video in the download.
    pub source: &'a Path,
    pub source_info: Option<&'a ParsedEpisodeInfo>,
    /// Torrents keep seeding: extras are linked or copied, not moved.
    pub read_only: bool,
}

/// `ExtraService.ImportExtraFiles`; returns how many extras were imported.
pub async fn import_extras(
    db: &Database,
    media: &MediaManagementSettings,
    x: &ExtraImport<'_>,
) -> usize {
    if !media.import_extra_files {
        return 0;
    }
    let wanted: Vec<String> = media
        .extra_file_extensions
        .split(',')
        .map(|e| format!(".{}", e.trim().trim_start_matches('.').to_lowercase()))
        .filter(|e| e.len() > 1)
        .collect();
    let Some(folder) = x.source.parent() else {
        return 0;
    };
    let mut files = Vec::new();
    walk(folder, true, &mut files);
    let video_stem = stem(x.source).to_lowercase();
    let mut subtitles = Vec::new();
    let mut others = Vec::new();
    for f in files {
        let ext = extension(&f);
        if !wanted.contains(&ext) || f == x.source {
            continue;
        }
        if SUBTITLE_EXTENSIONS.contains(&ext.as_str()) {
            subtitles.push(f);
        } else if f.parent() == Some(folder) && stem(&f).to_lowercase().starts_with(&video_stem) {
            others.push(f);
        }
    }
    // `SubtitleService.ImportFiles`: matching subtitles.
    let mut matching: Vec<PathBuf> = Vec::new();
    for f in &subtitles {
        if stem(f).to_lowercase().starts_with(&video_stem) {
            matching.push(f.clone());
            continue;
        }
        let sub_folder = f
            .parent()
            .map(|p| stem(p).to_lowercase())
            .unwrap_or_default();
        if f.parent() != Some(folder) && f.starts_with(folder) && sub_folder == video_stem {
            matching.push(f.clone());
            continue;
        }
        let info = parse_path(&f.to_string_lossy()).or_else(|| {
            f.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| parse_title(&n.to_string_lossy()))
        });
        if let (Some(info), Some(src)) = (info, x.source_info)
            && !info.episode_numbers.is_empty()
            && info.season_numbers == src.season_numbers
            && info.episode_numbers == src.episode_numbers
        {
            matching.push(f.clone());
        }
    }
    // Any subtitle when the folder holds this one video.
    if matching.is_empty() && !subtitles.is_empty() {
        let mut videos = Vec::new();
        walk(folder, true, &mut videos);
        videos.retain(|v| kitsunarr_parser::extensions::is_video_extension(&extension(v)));
        if videos.len() <= 2 {
            videos.retain(|v| {
                !stem(v)
                    .to_lowercase()
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|w| w == "sample")
            });
            if videos.len() == 1 {
                matching = subtitles.clone();
            }
        }
    }
    let target_dir = x
        .episode_file
        .parent()
        .unwrap_or(x.series_path)
        .to_path_buf();
    let target_stem = stem(x.episode_file);
    let now = kitsunarr_db::now();
    let mut imported = 0;
    // Grouped by language, tags and title: copies of a group are numbered.
    let infos: Vec<(PathBuf, kitsunarr_parser::SubtitleTitleInfo)> = matching
        .into_iter()
        .map(|f| {
            let info = parse_subtitle_language_information(&f.to_string_lossy());
            (f, info)
        })
        .collect();
    let key = |i: &kitsunarr_parser::SubtitleTitleInfo| {
        format!(
            "{}|{}|{}",
            i.language.id,
            i.language_tags.join("."),
            i.title.clone().unwrap_or_default()
        )
    };
    let mut copies: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (_, info) in &infos {
        *copies.entry(key(info)).or_default() += 1;
    }
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (path, info) in &infos {
        let k = key(info);
        let multiple = copies[&k] > 1;
        let copy = {
            let n = seen.entry(k).or_default();
            *n += 1;
            *n
        };
        // `GetSuffix`.
        let mut suffix = String::new();
        match &info.title {
            Some(t) => {
                suffix.push('.');
                suffix.push_str(t);
                if multiple {
                    suffix.push_str(&format!(" - {copy}"));
                }
            }
            None if multiple => suffix.push_str(&format!(".{copy}")),
            None => {}
        }
        if info.language.id != 0
            && let Some(iso) = iso_for(info.language)
        {
            suffix.push('.');
            suffix.push_str(iso.two_letter);
        }
        if !info.language_tags.is_empty() {
            suffix.push('.');
            suffix.push_str(&info.language_tags.join("."));
        }
        let ext = extension(path);
        let dest = target_dir.join(format!("{target_stem}{suffix}{ext}"));
        if transfer(media, path, &dest, x.read_only).is_err() {
            tracing::warn!(path = %path.display(), "failed to import the subtitle file");
            continue;
        }
        record(
            db,
            x,
            &dest,
            &ext,
            "subtitle",
            Some(info.language.id),
            &info.language_tags,
            info.title.as_deref(),
            &now,
        )
        .await;
        imported += 1;
    }
    // `OtherExtraService.ImportFiles`.
    for path in &others {
        let ext = extension(path);
        let dest = target_dir.join(format!("{target_stem}{ext}"));
        if dest.exists() || transfer(media, path, &dest, x.read_only).is_err() {
            continue;
        }
        record(db, x, &dest, &ext, "other", None, &[], None, &now).await;
        imported += 1;
    }
    imported
}

fn transfer(
    media: &MediaManagementSettings,
    from: &Path,
    to: &Path,
    read_only: bool,
) -> std::io::Result<()> {
    if to.exists() {
        std::fs::remove_file(to)?;
    }
    if read_only {
        if media.use_hardlinks && std::fs::hard_link(from, to).is_ok() {
            return Ok(());
        }
        std::fs::copy(from, to).map(|_| ())
    } else if std::fs::rename(from, to).is_ok() {
        Ok(())
    } else {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)
    }
}

#[allow(clippy::too_many_arguments)]
async fn record(
    db: &Database,
    x: &ExtraImport<'_>,
    dest: &Path,
    ext: &str,
    kind: &str,
    language: Option<i32>,
    tags: &[String],
    title: Option<&str>,
    now: &str,
) {
    let relative = dest
        .strip_prefix(x.series_path)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| dest.to_string_lossy().into_owned());
    let result = sqlx::query(
        "INSERT INTO extra_files (series_id, episode_file_id, season_number, relative_path, extension, kind, language, language_tags, title, added_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(x.series_id)
    .bind(x.episode_file_id)
    .bind(x.season_number)
    .bind(relative)
    .bind(ext)
    .bind(kind)
    .bind(language)
    .bind(serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()))
    .bind(title)
    .bind(now)
    .execute(db.pool())
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "the extra file could not be recorded");
    }
}

/// The extras of an episode file being replaced or deleted: removed (to the recycle bin when
/// set) with their records (Sonarr's `ExtraFileService` on `EpisodeFileDeletedEvent`).
pub async fn remove_for_file(
    db: &Database,
    media: &MediaManagementSettings,
    series_path: &Path,
    root: &Path,
    episode_file_id: i64,
) {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, relative_path FROM extra_files WHERE episode_file_id = ?")
            .bind(episode_file_id)
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
    for (id, relative) in rows {
        let path = series_path.join(&relative);
        if path.is_file()
            && crate::import::is_within(&path, series_path)
            && let Err(e) = crate::import::remove_or_recycle(&path, media, root)
        {
            tracing::warn!(path = %path.display(), error = %e, "could not remove an extra file");
        }
        let _ = sqlx::query("DELETE FROM extra_files WHERE id = ?")
            .bind(id)
            .execute(db.pool())
            .await;
    }
}

/// Renames an episode file's extras after the file (`ExtraFileRenamer`): each keeps what
/// followed the old file name (`.en.forced.srt`).
pub async fn rename_for_file(
    db: &Database,
    series_path: &Path,
    episode_file_id: i64,
    old_file: &Path,
    new_file: &Path,
) {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, relative_path FROM extra_files WHERE episode_file_id = ?")
            .bind(episode_file_id)
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
    let (old_stem, new_stem) = (stem(old_file), stem(new_file));
    let dir = new_file.parent().unwrap_or(series_path).to_path_buf();
    for (id, rel) in rows {
        let old = series_path.join(&rel);
        let name = old
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(rest) = name.strip_prefix(&old_stem) else {
            continue;
        };
        let dest = dir.join(format!("{new_stem}{rest}"));
        if dest == old || !old.is_file() || std::fs::rename(&old, &dest).is_err() {
            continue;
        }
        let relative = dest
            .strip_prefix(series_path)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let _ = sqlx::query("UPDATE extra_files SET relative_path = ? WHERE id = ?")
            .bind(relative)
            .bind(id)
            .execute(db.pool())
            .await;
    }
}
