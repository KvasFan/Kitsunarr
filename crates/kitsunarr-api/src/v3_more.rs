//! More of Sonarr's `/api/v3` for mobile clients and dashboards (nzb360, LunaSea, Helmarr,
//! Homepage): interactive search and grab (`/release`), queue removal, disk space, `/parse`
//! and episode-file deletion. Shapes follow Sonarr's resources.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use kitsunarr_db::library::series;
use kitsunarr_domain::api::queue::QueueRemoval;
use kitsunarr_domain::api::search::{GrabRequest, ReleaseQuery};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::v3::{episode_json, quality_json, series_json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/release", get(search).post(grab))
        .route("/queue/{id}", delete(remove_queue_item))
        .route("/queue/bulk", delete(remove_queue_items))
        .route("/diskspace", get(disk_space))
        .route("/parse", get(parse))
        .route("/episodefile/{id}", delete(delete_episode_file))
        .route("/episodefile/bulk", delete(delete_episode_files))
}

/// A queue record's id: stable for a download (Sonarr's ids are too), unlike its position.
pub(crate) fn queue_id(download_id: &str) -> i64 {
    // FNV-1a over the lowercased id, kept positive and non-zero.
    let mut hash: u32 = 0x811c_9dc5;
    for b in download_id.to_lowercase().bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    i64::from(hash & 0x7fff_ffff).max(1)
}

// ---- release ------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseParams {
    series_id: Option<i64>,
    season_number: Option<i32>,
    episode_id: Option<i64>,
}

fn protocol_name(p: kitsunarr_domain::DownloadProtocol) -> &'static str {
    match p {
        kitsunarr_domain::DownloadProtocol::Usenet => "usenet",
        kitsunarr_domain::DownloadProtocol::Torrent => "torrent",
        _ => "unknown",
    }
}

/// `GET /release`: an interactive search for an episode (`episodeId`) or a season
/// (`seriesId` and `seasonNumber`), as `ReleaseResource`s.
async fn search(
    State(state): State<AppState>,
    Query(p): Query<ReleaseParams>,
) -> ApiResult<Json<Vec<Value>>> {
    let (series_id, season_number, episode_id) = match (p.episode_id, p.series_id) {
        (Some(e), _) => {
            let sid = series::series_of_episode(&state.db, e)
                .await?
                .ok_or_else(|| ApiError::NotFound("The episode does not exist.".into()))?;
            (sid, None, Some(e))
        }
        (None, Some(s)) => (
            s,
            Some(p.season_number.ok_or_else(|| {
                ApiError::field("seasonNumber", "A season search needs a season number.")
            })?),
            None,
        ),
        (None, None) => {
            return Err(ApiError::field(
                "episodeId",
                "Search for an episode (episodeId) or a season (seriesId and seasonNumber).",
            ));
        }
    };
    let result = crate::routes::search::search_releases(
        State(state.clone()),
        Query(ReleaseQuery {
            series_id,
            season_number,
            episode_id,
            complete_series: false,
            season_packs_only: false,
        }),
    )
    .await?
    .0;
    let now = chrono::Utc::now();
    Ok(Json(
        result
            .releases
            .iter()
            .map(|r| {
                let published = chrono::DateTime::parse_from_rfc3339(&r.publish_date)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or(now);
                let age = now - published;
                let quality = kitsunarr_parser::parse_quality(&r.title);
                json!({
                    "guid": r.guid,
                    "quality": {
                        "quality": quality_json(quality.quality.id),
                        "revision": {"version": quality.revision.version, "real": quality.revision.real, "isRepack": quality.revision.is_repack},
                    },
                    "qualityWeight": r.quality_weight,
                    "age": age.num_days(),
                    "ageHours": age.num_minutes() as f64 / 60.0,
                    "ageMinutes": age.num_seconds() as f64 / 60.0,
                    "size": r.size,
                    "indexerId": r.indexer_id,
                    "indexer": r.indexer,
                    "releaseGroup": r.release_group,
                    "title": r.title,
                    "fullSeason": r.full_season,
                    "sceneSource": false,
                    "seasonNumber": r.season_numbers.first().copied().unwrap_or(0),
                    "languages": r.languages.iter().map(|l| json!({"name": l})).collect::<Vec<_>>(),
                    "seriesTitle": r.series_title,
                    "episodeNumbers": r.episode_numbers,
                    "absoluteEpisodeNumbers": r.absolute_episode_numbers,
                    "mappedEpisodeInfo": r.mapped_episodes,
                    "approved": r.approved,
                    "temporarilyRejected": false,
                    "rejected": !r.approved,
                    "rejections": r.rejections,
                    "publishDate": r.publish_date,
                    "infoUrl": r.info_url,
                    "downloadAllowed": r.matches_series,
                    "special": r.special,
                    "customFormats": r.custom_formats.iter().map(|n| json!({"name": n})).collect::<Vec<_>>(),
                    "customFormatScore": r.custom_format_score,
                    "protocol": protocol_name(r.protocol),
                    "seeders": r.seeders,
                    "leechers": match (r.peers, r.seeders) {
                        (Some(p), Some(s)) => Some(p - s),
                        _ => None,
                    },
                    "seriesId": r.matches_series.then_some(series_id),
                    "indexerFlags": 0,
                })
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrabBody {
    guid: String,
    indexer_id: i64,
    #[serde(default)]
    episode_ids: Vec<i64>,
}

/// `POST /release`: grabs a release of a recent search.
async fn grab(State(state): State<AppState>, Json(b): Json<GrabBody>) -> ApiResult<Json<Value>> {
    let result = crate::routes::grab::grab_release(
        State(state),
        Json(GrabRequest {
            guid: b.guid.clone(),
            indexer_id: b.indexer_id,
            episode_ids: b.episode_ids,
        }),
    )
    .await?
    .0;
    Ok(Json(json!({
        "guid": b.guid,
        "indexerId": b.indexer_id,
        "downloadClient": result.download_client,
        "downloadId": result.download_id,
        "message": result.message,
        "approved": true,
    })))
}

// ---- queue --------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RemovalParams {
    remove_from_client: Option<bool>,
    blocklist: Option<bool>,
    skip_redownload: Option<bool>,
}

impl RemovalParams {
    /// Sonarr's defaults: removed from the client, not blocklisted.
    fn removal(&self) -> QueueRemoval {
        QueueRemoval {
            remove_from_client: self.remove_from_client.unwrap_or(true),
            blocklist: self.blocklist.unwrap_or(false),
            skip_redownload: self.skip_redownload.unwrap_or(false),
        }
    }
}

async fn download_id_of(state: &AppState, id: i64) -> ApiResult<String> {
    let queue = crate::routes::queue::get_queue(State(state.clone()))
        .await?
        .0;
    queue
        .items
        .into_iter()
        .find(|i| queue_id(&i.download_id) == id)
        .map(|i| i.download_id)
        .ok_or_else(|| ApiError::NotFound("The queue item does not exist.".into()))
}

async fn remove(state: &AppState, id: i64, p: &RemovalParams) -> ApiResult<()> {
    let download_id = download_id_of(state, id).await?;
    let _ = crate::routes::queue::remove_from_queue(
        State(state.clone()),
        Path(download_id),
        Query(p.removal()),
    )
    .await?;
    Ok(())
}

/// `DELETE /queue/{id}` (`removeFromClient`, `blocklist`, `skipRedownload`).
async fn remove_queue_item(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(p): Query<RemovalParams>,
) -> ApiResult<StatusCode> {
    remove(&state, id, &p).await?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct Ids {
    ids: Vec<i64>,
}

/// `DELETE /queue/bulk` with `{"ids": [...]}`.
async fn remove_queue_items(
    State(state): State<AppState>,
    Query(p): Query<RemovalParams>,
    Json(b): Json<Ids>,
) -> ApiResult<StatusCode> {
    for id in b.ids {
        remove(&state, id, &p).await?;
    }
    Ok(StatusCode::OK)
}

// ---- disk space ---------------------------------------------------------------------------

/// `GET /diskspace`: the root folders' disks.
async fn disk_space(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let folders = crate::routes::settings::list_root_folders(State(state))
        .await?
        .0;
    Ok(Json(
        folders
            .into_iter()
            .filter(|f| f.accessible)
            .map(|f| {
                let label = std::path::Path::new(&f.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| f.path.clone());
                json!({
                    "path": f.path,
                    "label": label,
                    "freeSpace": f.free_space,
                    "totalSpace": f.total_space,
                })
            })
            .collect(),
    ))
}

// ---- parse --------------------------------------------------------------------------------

#[derive(Deserialize)]
struct ParseParams {
    title: Option<String>,
    path: Option<String>,
}

/// `GET /parse?title=`: what the parser reads from a release name, with the library series
/// and episodes it maps to.
async fn parse(
    State(state): State<AppState>,
    Query(p): Query<ParseParams>,
) -> ApiResult<Json<Value>> {
    let title = p
        .title
        .or(p.path)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ApiError::field("title", "Give a title to parse."))?;
    let Some(parsed) = kitsunarr_parser::parse_title(&title) else {
        return Ok(Json(json!({"title": title})));
    };
    let series_id = kitsunarr_library::manual_import::series_named(&state.db, &title)
        .await
        .map_err(ApiError::internal)?;
    let mut out = json!({
        "title": title,
        "parsedEpisodeInfo": {
            "releaseTitle": parsed.release_title,
            "seriesTitle": parsed.series_title,
            "seriesTitleInfo": {"title": parsed.series_title},
            "quality": {
                "quality": quality_json(parsed.quality.quality.id),
                "revision": {"version": parsed.quality.revision.version, "real": parsed.quality.revision.real, "isRepack": parsed.quality.revision.is_repack},
            },
            "seasonNumber": parsed.season_numbers.first().copied().unwrap_or(0),
            "seasonNumbers": parsed.season_numbers,
            "episodeNumbers": parsed.episode_numbers,
            "absoluteEpisodeNumbers": parsed.absolute_episode_numbers,
            "airDate": parsed.air_date,
            "languages": parsed.languages.iter().map(|l| json!({"id": l.id, "name": l.name})).collect::<Vec<_>>(),
            "fullSeason": parsed.full_season,
            "isPartialSeason": parsed.is_partial_season,
            "isMultiSeason": parsed.is_multi_season(),
            "isSeasonExtra": parsed.is_season_extra,
            "special": parsed.special,
            "releaseGroup": parsed.release_group,
            "releaseHash": parsed.release_hash,
            "isDaily": parsed.is_daily(),
            "isAbsoluteNumbering": parsed.is_absolute_numbering(),
            "isPossibleSpecialEpisode": parsed.is_possible_special_episode(),
        },
        "episodes": [],
    });
    if let Some(id) = series_id {
        let detail = series::get_detail(&state.db, id).await?;
        let episodes = series::list_episodes(&state.db, id, None).await?;
        let model =
            kitsunarr_library::decision_input::series_model(&state.db, &detail, &episodes).await?;
        let mapped: Vec<i64> = kitsunarr_decision::map_episodes(&parsed, &model)
            .iter()
            .map(|e| e.id)
            .collect();
        out["series"] = series_json(&detail);
        out["episodes"] = episodes
            .iter()
            .filter(|e| mapped.contains(&e.id))
            .map(episode_json)
            .collect();
    }
    Ok(Json(out))
}

// ---- episode files ------------------------------------------------------------------------

async fn delete_file(state: &AppState, file_id: i64) -> ApiResult<()> {
    let series_id: Option<i64> =
        kitsunarr_db::sqlx::query_scalar("SELECT series_id FROM episode_files WHERE id = ?")
            .bind(file_id)
            .fetch_optional(state.db.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    let series_id =
        series_id.ok_or_else(|| ApiError::NotFound("The episode file does not exist.".into()))?;
    crate::routes::library::delete_episode_file(State(state.clone()), Path((series_id, file_id)))
        .await?;
    Ok(())
}

/// `DELETE /episodefile/{id}`: the file goes to the recycle bin when one is set.
async fn delete_episode_file(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    delete_file(&state, id).await?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileIds {
    episode_file_ids: Vec<i64>,
}

/// `DELETE /episodefile/bulk` with `{"episodeFileIds": [...]}`.
async fn delete_episode_files(
    State(state): State<AppState>,
    Json(b): Json<FileIds>,
) -> ApiResult<StatusCode> {
    for id in b.episode_file_ids {
        delete_file(&state, id).await?;
    }
    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    #[test]
    fn queue_ids_are_stable_and_positive() {
        let a = super::queue_id("SABnzbd_nzo_abc");
        assert_eq!(a, super::queue_id("sabnzbd_NZO_ABC"));
        assert!(a > 0);
        assert_ne!(a, super::queue_id("sabnzbd_nzo_abd"));
    }
}
