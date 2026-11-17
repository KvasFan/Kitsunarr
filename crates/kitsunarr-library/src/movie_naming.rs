//! Movie inputs for Tanukarr's shared FileNameBuilder pipeline, including regex-based
//! custom formats. Imports and explicit rename previews use the same renderer and metadata.
use kitsunarr_db::library::movies::NewMovieFile;
use kitsunarr_db::{Database, quality};
use kitsunarr_domain::api::library::MovieDetail;
use kitsunarr_domain::api::settings::NamingSettings;
use kitsunarr_domain::quality::{QualityModel, Revision};
use kitsunarr_domain::{Language, MetadataProvider, SeriesType};

use crate::file_names::{self, File, Series};

pub(crate) async fn stored_file(
    db: &Database,
    detail: &MovieDetail,
) -> Result<NewMovieFile, String> {
    let f = detail.file.as_ref().ok_or("The movie has no file.")?;
    let (scene_name, version, real, is_repack, media_info): (Option<String>, i64, i64, bool, Option<String>) =
        sqlx::query_as("SELECT scene_name, revision_version, revision_real, is_repack, media_info FROM movie_files WHERE id = ?")
            .bind(f.id).fetch_one(db.pool()).await.map_err(|e| e.to_string())?;
    Ok(NewMovieFile {
        movie_id: detail.item.id,
        relative_path: f.relative_path.clone(),
        size: f.size,
        added_at: f.added_at.clone(),
        scene_name,
        release_group: f.release_group.clone(),
        quality_id: f.quality_id,
        revision_version: version as i32,
        revision_real: real as i32,
        is_repack,
        languages: f.languages.clone(),
        media_info: media_info
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?,
        original_path: None,
        edition: None,
    })
}

pub(crate) async fn file_name(
    db: &Database,
    detail: &MovieDetail,
    input: &NewMovieFile,
    extension: &str,
    naming: &NamingSettings,
) -> Result<String, String> {
    let path = std::path::Path::new(&input.relative_path);
    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    let title = input
        .scene_name
        .clone()
        .unwrap_or_else(|| crate::import::stem(path));
    let quality = QualityModel {
        quality_id: input.quality_id,
        revision: Revision {
            version: input.revision_version,
            real: input.revision_real,
            is_repack: input.is_repack,
        },
    };
    let (formats, _) = kitsunarr_decision::compile_formats(
        &quality::list_custom_formats(db.pool())
            .await
            .map_err(|e| e.to_string())?,
    );
    let languages: Vec<_> = input
        .languages
        .iter()
        .filter_map(|id| Language::by_id(*id))
        .collect();
    let matched = kitsunarr_decision::custom_format::evaluate(
        &formats,
        &kitsunarr_decision::CustomFormatInput {
            release_title: &title,
            file_name: Some(&filename),
            release_group: input.release_group.as_deref(),
            quality: quality.quality(),
            size: input.size,
            indexer_flags: 0,
            languages: &languages,
            release_type: kitsunarr_parser::ReleaseType::Unknown,
            original_language: None,
            episode_titles: &[],
        },
    );
    let quality_title = quality::list_definitions(db.pool())
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|d| d.quality_id == input.quality_id)
        .map_or_else(|| quality.quality().name.to_string(), |d| d.title);
    let series = Series::from_parts(
        &detail.item.title,
        detail.item.year,
        if matches!(
            detail.item.metadata_provider,
            MetadataProvider::AniList | MetadataProvider::MyAnimeList
        ) {
            SeriesType::Anime
        } else {
            SeriesType::Standard
        },
        &detail.external_ids,
    );
    let file = File {
        id: detail
            .file
            .as_ref()
            .filter(|f| f.relative_path == input.relative_path)
            .map_or(0, |f| f.id),
        scene_name: input.scene_name.clone(),
        relative_path: Some(input.relative_path.clone()),
        release_group: input.release_group.clone(),
        quality,
        quality_title,
        custom_formats: matched
            .iter()
            .map(|f| (f.name.clone(), f.include_when_renaming))
            .collect(),
        media_info: input
            .media_info
            .clone()
            .and_then(|v| serde_json::from_value(v).ok()),
        ..Default::default()
    };
    let result = file_names::build_movie_file_name(&series, &file, extension, naming)
        .map_err(|e| e.to_string())?;
    if result.is_empty() || result == extension {
        return Err("The movie naming format produced an empty name.".into());
    }
    Ok(result)
}
