//! Episode details and Sonarr's episode file editor ("Manage episodes" on the series page).

use axum::Json;
use axum::extract::{Path, State};
use kitsunarr_db::library::series::{self, EpisodeFileEdit, FileRecord};
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::library::{
    AudioStreamSummary, BulkDeleteResult, BulkEditResult, EpisodeDetail, EpisodeFileDetail,
    EpisodeFileEditRequest, EpisodeFileIds, EpisodeItem, MediaInfoSummary,
};
use kitsunarr_domain::quality::{QualityModel, Revision};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

/// A stored file's media information as Sonarr's file details format it.
pub(crate) fn media_info_summary(raw: Option<&str>, scene_name: &str) -> Option<MediaInfoSummary> {
    use kitsunarr_library::media_info::{MediaInfo, format};
    let info = serde_json::from_str::<MediaInfo>(raw?).ok()?;
    let secs = info.run_time.max(0.0) as i64;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    Some(MediaInfoSummary {
        video_codec: format::format_video_codec(&info, scene_name),
        video_dynamic_range: format::format_video_dynamic_range(&info),
        resolution: format!("{}x{}", info.width, info.height),
        run_time: if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        },
        video_bitrate: info.video_bitrate,
        video_bit_depth: info.video_bit_depth,
        video_fps: (info.video_fps * 1000.0).round() / 1000.0,
        scan_type: info.scan_type.clone(),
        audio: info
            .audio_streams
            .iter()
            .map(|a| AudioStreamSummary {
                language: a.language.clone(),
                codec: format::format_audio_codec(Some(a), scene_name),
                channels: format::format_audio_channels(Some(a)),
            })
            .collect(),
        subtitles: info
            .subtitle_streams
            .iter()
            .filter_map(|s| s.language.clone().or_else(|| s.format.clone()))
            .collect(),
    })
}

fn file_detail(
    series_id: i64,
    series_path: &str,
    f: &FileRecord,
    episodes: &[EpisodeItem],
) -> EpisodeFileDetail {
    let quality_id = f.quality_id as i32;
    let mine: Vec<&EpisodeItem> = episodes
        .iter()
        .filter(|e| e.file.as_ref().is_some_and(|x| x.id == f.id))
        .collect();
    EpisodeFileDetail {
        id: f.id,
        series_id,
        season_number: mine.first().map_or(0, |e| e.season_number),
        episodes: mine
            .iter()
            .map(|e| format!("S{:02}E{:02}", e.season_number, e.episode_number))
            .collect(),
        relative_path: f.relative_path.clone(),
        path: std::path::Path::new(series_path)
            .join(&f.relative_path)
            .display()
            .to_string(),
        size: f.size,
        quality: QualityModel {
            quality_id,
            revision: Revision {
                version: f.revision_version as i32,
                real: f.revision_real as i32,
                is_repack: f.is_repack,
            },
        },
        quality_name: mine
            .first()
            .and_then(|e| e.file.as_ref())
            .map(|x| x.quality_name.clone())
            .unwrap_or_else(|| {
                kitsunarr_domain::Quality::by_id(quality_id)
                    .unwrap_or_else(kitsunarr_domain::Quality::unknown)
                    .name
                    .to_string()
            }),
        languages: f.language_ids(),
        release_group: f.release_group.clone(),
        scene_name: f.scene_name.clone(),
        release_type: kitsunarr_library::manual_import::release_type_api_name(
            kitsunarr_library::decision_input::release_type(f.release_type),
        )
        .to_string(),
        added_at: f.added_at.clone(),
        media_info: media_info_summary(
            f.media_info.as_deref(),
            f.scene_name.as_deref().unwrap_or_default(),
        ),
    }
}

/// Sonarr's episode details: the episode, its overview and its file with media information.
#[utoipa::path(get, path = "/episodes/{id}", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = EpisodeDetail), (status = 404)))]
pub async fn get_episode(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<EpisodeDetail>> {
    let (series_id, overview) = series::episode_overview(&state.db, id).await?;
    let detail = series::get_list_item(&state.db, series_id).await?;
    let episodes = series::list_episodes(&state.db, series_id, None).await?;
    let episode = episodes
        .iter()
        .find(|e| e.id == id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("The episode does not exist.".into()))?;
    let file = match &episode.file {
        Some(f) => series::file_records(&state.db, series_id)
            .await?
            .iter()
            .find(|r| r.id == f.id)
            .map(|r| file_detail(series_id, &detail.path, r, &episodes)),
        None => None,
    };
    Ok(Json(EpisodeDetail {
        episode,
        series_title: detail.title,
        overview: overview.filter(|o| !o.trim().is_empty()),
        file,
    }))
}

/// A series' episode files, for Sonarr's episode file editor.
#[utoipa::path(get, path = "/series/{id}/files", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = Vec<EpisodeFileDetail>), (status = 404)))]
pub async fn list_episode_files(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<EpisodeFileDetail>>> {
    let detail = series::get_list_item(&state.db, id).await?;
    let episodes = series::list_episodes(&state.db, id, None).await?;
    let mut files: Vec<EpisodeFileDetail> = series::file_records(&state.db, id)
        .await?
        .iter()
        .map(|f| file_detail(id, &detail.path, f, &episodes))
        .collect();
    files.sort_by(|a, b| {
        (a.season_number, &a.episodes, &a.relative_path).cmp(&(
            b.season_number,
            &b.episodes,
            &b.relative_path,
        ))
    });
    Ok(Json(files))
}

fn release_type_id(name: &str) -> ApiResult<i64> {
    let t = kitsunarr_library::decision_input::release_type_named(name);
    if t == kitsunarr_parser::ReleaseType::Unknown && !name.eq_ignore_ascii_case("unknown") {
        return Err(ApiError::field(
            "releaseType",
            "Choose unknown, singleEpisode, multiEpisode or seasonPack.",
        ));
    }
    Ok(kitsunarr_library::decision_input::release_type_id(t))
}

/// Sonarr's episode file editor (`PUT /episodefile/bulk` from "Manage episodes"): the same
/// quality, languages, release group or release type for several files. Files that do not
/// exist are skipped.
#[utoipa::path(put, path = "/episodefiles/editor", tag = "library", request_body = EpisodeFileEditRequest,
    responses((status = 200, body = BulkEditResult), (status = 422, body = ApiErrorBody)))]
pub async fn edit_episode_files(
    State(state): State<AppState>,
    Json(req): Json<EpisodeFileEditRequest>,
) -> ApiResult<Json<BulkEditResult>> {
    if let Some(q) = req.quality
        && kitsunarr_domain::Quality::by_id(q.quality_id).is_none()
    {
        return Err(ApiError::field("quality", "Choose a known quality."));
    }
    if let Some(langs) = &req.languages
        && (langs.is_empty()
            || langs
                .iter()
                .any(|l| kitsunarr_domain::Language::by_id(*l).is_none()))
    {
        return Err(ApiError::field("languages", "Choose known languages."));
    }
    let edit = EpisodeFileEdit {
        quality: req.quality.map(|q| {
            (
                i64::from(q.quality_id),
                i64::from(q.revision.version.max(1)),
                i64::from(q.revision.real),
                q.revision.is_repack,
            )
        }),
        languages: req.languages.clone(),
        scene_name: None,
        release_group: req
            .release_group
            .as_deref()
            .map(str::trim)
            .map(str::to_string),
        indexer_flags: None,
        release_type: req
            .release_type
            .as_deref()
            .map(release_type_id)
            .transpose()?,
    };
    let mut updated = 0;
    let mut touched = Vec::new();
    for id in &req.episode_file_ids {
        match series::edit_episode_file(&state.db, *id, &edit).await {
            Ok(()) => updated += 1,
            Err(kitsunarr_db::DbError::NotFound) => continue,
            Err(e) => return Err(e.into()),
        }
        if let Some(s) = series::series_of_file(&state.db, *id).await?
            && !touched.contains(&s)
        {
            touched.push(s);
        }
    }
    if !touched.is_empty() {
        state.events.publish(ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: touched,
            deleted: false,
        });
    }
    Ok(Json(BulkEditResult { updated }))
}

/// Deletes several episode files (Sonarr's `DELETE /episodefile/bulk`), each to the recycle
/// bin when one is set; a failure keeps that file and does not stop the others.
#[utoipa::path(post, path = "/episodefiles/delete", tag = "library", request_body = EpisodeFileIds,
    responses((status = 200, body = BulkDeleteResult)))]
pub async fn delete_episode_files(
    State(state): State<AppState>,
    Json(req): Json<EpisodeFileIds>,
) -> ApiResult<Json<BulkDeleteResult>> {
    let mut out = BulkDeleteResult::default();
    for id in &req.episode_file_ids {
        let Some(series_id) = series::series_of_file(&state.db, *id).await? else {
            out.failures
                .push(format!("File {id}: the episode file does not exist."));
            continue;
        };
        match crate::routes::library::delete_episode_file(
            State(state.clone()),
            Path((series_id, *id)),
        )
        .await
        {
            Ok(_) => out.deleted += 1,
            Err(e) => out.failures.push(format!("File {id}: {}", e.message())),
        }
    }
    Ok(Json(out))
}
