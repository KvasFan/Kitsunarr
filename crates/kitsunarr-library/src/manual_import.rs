//! Manual import (Sonarr's `ManualImportService`): video files in any folder, matched to library
//! series and episodes for review, then imported with the user's choices.
//!
//! Each chosen file is imported through the download import pipeline as its own "download",
//! with a synthetic grab carrying the chosen episodes (the persistent manual mapping), so naming,
//! upgrade rules, history and the recycle bin behave as for downloads.

use std::path::{Path, PathBuf};

use kitsunarr_db::Database;
use kitsunarr_db::history::GrabRecord;
use kitsunarr_db::library::series;
use kitsunarr_domain::api::library_tools::{ManualImportFile, ManualImportItem};
use kitsunarr_domain::text::clean_title;
use kitsunarr_parser::{parse_path, parse_quality, parse_release_group};

use crate::import::{self, Download};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The library series a parsed title names: by title, then by alternate title.
async fn find_series(db: &Database, title: &str) -> Result<Option<i64>, String> {
    let wanted = clean_title(title);
    if wanted.is_empty() {
        return Ok(None);
    }
    let titles: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, title FROM series UNION ALL SELECT series_id, title FROM series_alternate_titles",
    )
    .fetch_all(db.pool())
    .await
    .map_err(err)?;
    Ok(titles
        .into_iter()
        .find(|(_, t)| clean_title(t) == wanted)
        .map(|(id, _)| id))
}

/// Lists the folder's video files (or the one file) with the series and episodes they match.
pub async fn scan(
    db: &Database,
    folder: &Path,
    series_id: Option<i64>,
) -> Result<Vec<ManualImportItem>, String> {
    if !folder.exists() {
        return Err(format!("{} does not exist.", folder.display()));
    }
    let files: Vec<PathBuf> = if folder.is_dir() {
        import::video_files(folder)
    } else {
        vec![folder.to_path_buf()]
    };
    let base = if folder.is_dir() {
        folder.to_path_buf()
    } else {
        folder.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut out = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size = std::fs::metadata(&path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        let parsed = parse_path(&path.to_string_lossy());
        let mut rejections = Vec::new();
        let sid = match series_id {
            Some(id) => Some(id),
            None => match &parsed {
                Some(p) => find_series(db, &p.series_title).await?,
                None => None,
            },
        };
        let mut item = ManualImportItem {
            path: path.display().to_string(),
            relative_path: path
                .strip_prefix(&base)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned(),
            name: name.clone(),
            size,
            series_id: sid,
            series_title: None,
            season_number: parsed
                .as_ref()
                .and_then(|p| p.season_numbers.first().copied()),
            episode_ids: Vec::new(),
            episodes: Vec::new(),
            quality: parse_quality(&name).quality.name.to_string(),
            release_group: parse_release_group(&name),
            rejections: Vec::new(),
        };
        match (sid, &parsed) {
            (None, _) => rejections.push("Unknown series".to_string()),
            (Some(id), parsed) => {
                let detail = series::get_detail(db, id).await.map_err(err)?;
                item.series_title = Some(detail.item.title.clone());
                let episodes = series::list_episodes(db, id, None).await.map_err(err)?;
                let model = crate::decision_input::series_model(db, &detail, &episodes)
                    .await
                    .map_err(err)?;
                match parsed {
                    None => rejections.push("Unable to parse the file name".to_string()),
                    Some(p) => {
                        let mapped = kitsunarr_decision::map_episodes(p, &model);
                        if mapped.is_empty() {
                            rejections.push("No matching episodes".to_string());
                        }
                        for e in mapped {
                            item.episode_ids.push(e.id);
                            item.episodes
                                .push(format!("S{:02}E{:02}", e.season_number, e.episode_number));
                            if e.file.is_some() {
                                rejections.push(format!(
                                    "S{:02}E{:02} already has a file",
                                    e.season_number, e.episode_number
                                ));
                            }
                        }
                    }
                }
            }
        }
        item.rejections = rejections;
        out.push(item);
    }
    Ok(out)
}

/// Imports the chosen files (each with its series and episodes), moving or copying them.
pub async fn import(
    db: &Database,
    files: &[ManualImportFile],
    move_files: bool,
) -> Vec<(String, Result<import::Outcome, String>)> {
    let mut out = Vec::new();
    for f in files {
        let result = async {
            if f.episode_ids.is_empty() {
                return Err("Choose the file's episodes.".to_string());
            }
            for id in &f.episode_ids {
                if series::series_of_episode(db, *id).await.map_err(err)? != Some(f.series_id) {
                    return Err("The chosen episodes must belong to the chosen series.".into());
                }
            }
            let path = PathBuf::from(&f.path);
            if !path.is_file() {
                return Err(format!("{} is not a file.", path.display()));
            }
            let download_id = format!("manual-{}", uuid::Uuid::new_v4().simple());
            let data = serde_json::json!({
                "manualEpisodeMapping": true,
                "manualEpisodeIds": f.episode_ids,
                "releaseSource": "ManualImport",
            })
            .to_string();
            let title = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let grabs: Vec<GrabRecord> = f
                .episode_ids
                .iter()
                .map(|e| GrabRecord {
                    download_id: download_id.clone(),
                    date: kitsunarr_db::now(),
                    source_title: title.clone(),
                    series_id: Some(f.series_id),
                    episode_id: Some(*e),
                    quality_id: None,
                    revision: None,
                    languages: None,
                    data: data.clone(),
                })
                .collect();
            let d = Download {
                download_id: &download_id,
                title: &title,
                output_path: &path,
                series_id: f.series_id,
                grabs: &grabs,
                client_name: "Manual Import",
                client_implementation: "ManualImport",
                move_files,
            };
            import::import_download(db, &d).await
        }
        .await;
        out.push((f.path.clone(), result));
    }
    out
}
