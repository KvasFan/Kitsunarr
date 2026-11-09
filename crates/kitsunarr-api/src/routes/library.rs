//! Library endpoints: series/anime, episodes, movies, collections, calendar, history.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kitsunarr_db::history;
use kitsunarr_db::library::{collections, movies, series, stats};
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::library::{
    BulkEdit, BulkEditResult, CalendarEntry, CalendarQuery, CollectionItem, EpisodeItem,
    EpisodesMonitoredUpdate, HistoryItem, LibraryQuery, LibraryStats, MovieDetail, MovieListItem,
    SeriesDetail, SeriesListItem, SeriesUpdate,
};
use kitsunarr_domain::{Page, PageRequest};
use serde::Deserialize;

use crate::AppState;
use crate::error::{ApiError, ApiResult};

fn changed(state: &AppState, media_type: &str, ids: Vec<i64>, deleted: bool) {
    state.events.publish(ServerEvent::LibraryChanged {
        media_type: media_type.into(),
        ids,
        deleted,
    });
}

#[utoipa::path(get, path = "/library/stats", tag = "library", responses((status = 200, body = LibraryStats)))]
pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<LibraryStats>> {
    Ok(Json(stats::library_stats(&state.db).await?))
}

#[utoipa::path(get, path = "/series", tag = "library", params(LibraryQuery),
    responses((status = 200, body = Page<SeriesListItem>)))]
pub async fn list_series(
    State(state): State<AppState>,
    Query(q): Query<LibraryQuery>,
) -> ApiResult<Json<Page<SeriesListItem>>> {
    Ok(Json(series::list(&state.db, &q).await?))
}

#[utoipa::path(get, path = "/series/{id}", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = SeriesDetail), (status = 404)))]
pub async fn get_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<SeriesDetail>> {
    Ok(Json(series::get_detail(&state.db, id).await?))
}

#[utoipa::path(patch, path = "/series/{id}", tag = "library", params(("id" = i64, Path)), request_body = SeriesUpdate,
    responses((status = 200, body = SeriesDetail)))]
pub async fn update_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(update): Json<SeriesUpdate>,
) -> ApiResult<Json<SeriesDetail>> {
    series::apply_update(&state.db, id, &update).await?;
    changed(&state, "series", vec![id], false);
    Ok(Json(series::get_detail(&state.db, id).await?))
}

#[utoipa::path(delete, path = "/series/{id}", tag = "library", params(("id" = i64, Path)),
    responses((status = 204, description = "Database records removed; files on disk are never deleted by this call")))]
pub async fn delete_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    series::delete(&state.db, id).await?;
    changed(&state, "series", vec![id], true);
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/series/bulk", tag = "library", request_body = BulkEdit, responses((status = 200, body = BulkEditResult)))]
pub async fn bulk_edit_series(
    State(state): State<AppState>,
    Json(edit): Json<BulkEdit>,
) -> ApiResult<Json<BulkEditResult>> {
    if edit.ids.len() > 10_000 {
        return Err(ApiError::BadRequest(
            "At most 10000 items can be edited at once.".into(),
        ));
    }
    let updated = series::bulk_edit(&state.db, &edit).await?;
    changed(&state, "series", edit.ids, false);
    Ok(Json(BulkEditResult { updated }))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeQuery {
    pub season_number: Option<i32>,
}

#[utoipa::path(get, path = "/series/{id}/episodes", tag = "library", params(("id" = i64, Path), EpisodeQuery),
    responses((status = 200, body = Vec<EpisodeItem>)))]
pub async fn list_episodes(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<EpisodeQuery>,
) -> ApiResult<Json<Vec<EpisodeItem>>> {
    Ok(Json(
        series::list_episodes(&state.db, id, q.season_number).await?,
    ))
}

#[utoipa::path(put, path = "/episodes/monitor", tag = "library", request_body = EpisodesMonitoredUpdate,
    responses((status = 200, body = BulkEditResult)))]
pub async fn set_episodes_monitored(
    State(state): State<AppState>,
    Json(body): Json<EpisodesMonitoredUpdate>,
) -> ApiResult<Json<BulkEditResult>> {
    let updated =
        series::set_episodes_monitored(&state.db, &body.episode_ids, body.monitored).await?;
    Ok(Json(BulkEditResult { updated }))
}

#[utoipa::path(get, path = "/movies", tag = "library", params(LibraryQuery), responses((status = 200, body = Page<MovieListItem>)))]
pub async fn list_movies(
    State(state): State<AppState>,
    Query(q): Query<LibraryQuery>,
) -> ApiResult<Json<Page<MovieListItem>>> {
    Ok(Json(movies::list(&state.db, &q).await?))
}

#[utoipa::path(get, path = "/movies/{id}", tag = "library", params(("id" = i64, Path)), responses((status = 200, body = MovieDetail)))]
pub async fn get_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<MovieDetail>> {
    Ok(Json(movies::get_detail(&state.db, id).await?))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MovieUpdate {
    pub monitored: Option<bool>,
    pub quality_profile_id: Option<i64>,
    pub tags: Option<Vec<i64>>,
}

#[utoipa::path(patch, path = "/movies/{id}", tag = "library", params(("id" = i64, Path)), request_body = MovieUpdate,
    responses((status = 200, body = MovieDetail)))]
pub async fn update_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(update): Json<MovieUpdate>,
) -> ApiResult<Json<MovieDetail>> {
    if let Some(m) = update.monitored {
        movies::set_monitored(&state.db, id, m).await?;
    }
    if update.quality_profile_id.is_some() || update.tags.is_some() {
        let current = movies::get_detail(&state.db, id).await?;
        let (add, remove) = match &update.tags {
            Some(tags) => (
                tags.clone(),
                current
                    .item
                    .tags
                    .iter()
                    .copied()
                    .filter(|t| !tags.contains(t))
                    .collect::<Vec<_>>(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        movies::bulk_edit(
            &state.db,
            &BulkEdit {
                ids: vec![id],
                monitored: None,
                quality_profile_id: update.quality_profile_id,
                add_tags: add,
                remove_tags: remove,
            },
        )
        .await?;
    }
    changed(&state, "movie", vec![id], false);
    Ok(Json(movies::get_detail(&state.db, id).await?))
}

#[utoipa::path(delete, path = "/movies/{id}", tag = "library", params(("id" = i64, Path)),
    responses((status = 204, description = "Database records removed; files on disk are never deleted by this call")))]
pub async fn delete_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    movies::delete(&state.db, id).await?;
    changed(&state, "movie", vec![id], true);
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/movies/bulk", tag = "library", request_body = BulkEdit, responses((status = 200, body = BulkEditResult)))]
pub async fn bulk_edit_movies(
    State(state): State<AppState>,
    Json(edit): Json<BulkEdit>,
) -> ApiResult<Json<BulkEditResult>> {
    if edit.ids.len() > 10_000 {
        return Err(ApiError::BadRequest(
            "At most 10000 items can be edited at once.".into(),
        ));
    }
    let updated = movies::bulk_edit(&state.db, &edit).await?;
    changed(&state, "movie", edit.ids, false);
    Ok(Json(BulkEditResult { updated }))
}

#[utoipa::path(get, path = "/collections", tag = "library", responses((status = 200, body = Vec<CollectionItem>)))]
pub async fn list_collections(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<CollectionItem>>> {
    Ok(Json(collections::list(&state.db).await?))
}

fn parse_ts(raw: Option<&str>, field: &str) -> ApiResult<Option<chrono::DateTime<chrono::Utc>>> {
    raw.map(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&chrono::Utc))
            .map_err(|_| ApiError::field(field, "must be an RFC 3339 timestamp"))
    })
    .transpose()
}

#[utoipa::path(get, path = "/calendar", tag = "library", params(CalendarQuery), responses((status = 200, body = Vec<CalendarEntry>)))]
pub async fn calendar(
    State(state): State<AppState>,
    Query(q): Query<CalendarQuery>,
) -> ApiResult<Json<Vec<CalendarEntry>>> {
    let start = parse_ts(q.start.as_deref(), "start")?
        .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::days(1));
    let end = parse_ts(q.end.as_deref(), "end")?.unwrap_or(start + chrono::Duration::days(14));
    if end <= start || end - start > chrono::Duration::days(366) {
        return Err(ApiError::BadRequest(
            "The calendar range must be positive and at most one year.".into(),
        ));
    }
    let fmt =
        |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    Ok(Json(
        series::calendar(&state.db, &fmt(start), &fmt(end), q.unmonitored).await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub sort_direction: Option<kitsunarr_domain::SortDirection>,
    pub event_type: Option<String>,
}

#[utoipa::path(get, path = "/history", tag = "library", params(HistoryQuery), responses((status = 200, body = Page<HistoryItem>)))]
pub async fn history(
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Page<HistoryItem>>> {
    let paging = PageRequest {
        page: q.page,
        page_size: q.page_size,
        sort_direction: q.sort_direction,
        ..Default::default()
    };
    Ok(Json(
        history::list(
            &state.db,
            &paging,
            q.event_type.as_deref().filter(|s| !s.is_empty()),
        )
        .await?,
    ))
}
