//! Library endpoints: series/anime, episodes, movies, collections, calendar, history.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kitsunarr_db::history;
use kitsunarr_db::library::{collections, movies, series, stats};
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::library::{
    BulkEdit, BulkEditResult, CalendarEntry, CalendarQuery, CollectionDetail, CollectionEdit,
    CollectionItem, CollectionMemberEdit, EpisodeItem, EpisodesMonitoredUpdate,
    FileOperationResult, HistoryItem, LibraryQuery, LibraryStats, MovieDetail, MovieListItem,
    MovieRenamePreview, RenamePreview, RenameRequest, SeriesDetail, SeriesListItem, SeriesUpdate,
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

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct PathFiltersQuery {
    /// `series` or `anime`; every series when omitted.
    pub kind: Option<kitsunarr_domain::MediaKind>,
}

/// The folders the library can be filtered by (Tanukarr's `SeriesPathFilter` choices): root
/// folders, the folders the series are in, and their mount points.
#[utoipa::path(get, path = "/series/pathfilters", tag = "library", params(PathFiltersQuery),
    responses((status = 200, body = Vec<String>)))]
pub async fn series_path_filters(
    State(state): State<AppState>,
    Query(q): Query<PathFiltersQuery>,
) -> ApiResult<Json<Vec<String>>> {
    Ok(Json(series::path_filters(&state.db, q.kind).await?))
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
    let before = series::get_detail(&state.db, id).await?;
    if let Some(l) = update.metadata_language
        && kitsunarr_library::metadata_language_code(Some(l)) == "eng"
        && l != 1
    {
        return Err(ApiError::field(
            "metadataLanguage",
            "TheTVDB has no titles in this language.",
        ));
    }
    series::apply_update(&state.db, id, &update).await?;
    changed(&state, "series", vec![id], false);
    // `SeriesEditedService`: a new language is read at once.
    if update
        .metadata_language
        .is_some_and(|l| before.metadata_language.unwrap_or(1) != l)
        && before.item.metadata_provider == kitsunarr_domain::MetadataProvider::Tvdb
    {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = kitsunarr_library::refresh_series(&state.db, &state.metadata, id).await
            {
                tracing::warn!(series_id = id, error = %e, "refresh after a language change failed");
            }
            changed(&state, "series", vec![id], false);
        });
    }
    Ok(Json(series::get_detail(&state.db, id).await?))
}

#[derive(Debug, Deserialize, Default, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase", default)]
pub struct DeleteSeriesQuery {
    /// Also delete the series folder (to the recycle bin when one is set).
    pub delete_files: bool,
    /// Series only: keep import lists from adding the series again (Sonarr's
    /// `addImportListExclusion`).
    pub add_import_list_exclusion: bool,
}

/// Excludes a series from import lists by the identity lists match: its own catalogue's id
/// when lists know that catalogue, else its TheTVDB, TMDB, AniList or MyAnimeList id.
pub(crate) async fn exclude_from_import_lists(state: &AppState, id: i64) -> ApiResult<()> {
    use kitsunarr_domain::{ExternalIdKind, MetadataProvider};
    let d = series::get_detail(&state.db, id).await?;
    let own = match d.item.metadata_provider {
        MetadataProvider::Tvdb => Some("tvdb"),
        MetadataProvider::TmdbTv => Some("tmdb"),
        MetadataProvider::AniList => Some("aniList"),
        MetadataProvider::MyAnimeList => Some("myAnimeList"),
        _ => None,
    }
    .map(|p| (p.to_string(), d.metadata_id.clone()));
    let identity = own.or_else(|| {
        [
            ExternalIdKind::Tvdb,
            ExternalIdKind::Tmdb,
            ExternalIdKind::AniList,
            ExternalIdKind::MyAnimeList,
        ]
        .iter()
        .find_map(|k| {
            d.external_ids
                .iter()
                .find(|x| x.kind == *k)
                .map(|x| (k.as_str().to_string(), x.value.clone()))
        })
    });
    let Some((provider, external_id)) = identity else {
        tracing::warn!(
            series_id = id,
            "no identity import lists know; no exclusion added"
        );
        return Ok(());
    };
    let mut conn = state
        .db
        .pool()
        .acquire()
        .await
        .map_err(kitsunarr_db::DbError::from)?;
    kitsunarr_db::rules::upsert_exclusion(&mut conn, &provider, &external_id, Some(&d.item.title))
        .await?;
    Ok(())
}

#[utoipa::path(delete, path = "/series/{id}", tag = "library", params(("id" = i64, Path), DeleteSeriesQuery),
    responses((status = 204, description = "Removed; the folder is deleted only with deleteFiles")))]
pub async fn delete_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<DeleteSeriesQuery>,
) -> ApiResult<StatusCode> {
    let notification = kitsunarr_library::notify::series_ref(&state.db, id).await;
    if q.add_import_list_exclusion {
        exclude_from_import_lists(&state, id).await?;
    }
    if q.delete_files {
        kitsunarr_library::files::delete_series_folder(&state.db, id)
            .await
            .map_err(|m| ApiError::field("deleteFiles", m))?;
    }
    series::delete(&state.db, id).await?;
    changed(&state, "series", vec![id], true);
    if let Some(series) = notification {
        kitsunarr_library::notify::spawn(
            &state.db,
            &state.downloads.notifier,
            kitsunarr_notifications::Event::SeriesDelete {
                series: Box::new(series),
                deleted_files: q.delete_files,
            },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Removes several series, each as `DELETE /series/{id}` does; a failure keeps that series and
/// does not stop the others.
#[utoipa::path(post, path = "/series/delete", tag = "library",
    request_body = kitsunarr_domain::api::library::BulkDeleteRequest,
    responses((status = 200, body = kitsunarr_domain::api::library::BulkDeleteResult)))]
pub async fn bulk_delete_series(
    State(state): State<AppState>,
    Json(req): Json<kitsunarr_domain::api::library::BulkDeleteRequest>,
) -> ApiResult<Json<kitsunarr_domain::api::library::BulkDeleteResult>> {
    if req.ids.len() > 10_000 {
        return Err(ApiError::BadRequest(
            "At most 10000 series can be removed at once.".into(),
        ));
    }
    let mut out = kitsunarr_domain::api::library::BulkDeleteResult::default();
    for id in &req.ids {
        let title = series::get_list_item(&state.db, *id)
            .await
            .map(|s| s.title)
            .unwrap_or_else(|_| format!("Series {id}"));
        match delete_series(
            State(state.clone()),
            Path(*id),
            Query(DeleteSeriesQuery {
                delete_files: req.delete_files,
                add_import_list_exclusion: req.add_import_list_exclusion,
            }),
        )
        .await
        {
            Ok(_) => out.deleted += 1,
            Err(e) => out.failures.push(format!("{title}: {e}")),
        }
    }
    Ok(Json(out))
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

/// Changes a series' folder, moving its contents there unless `moveFiles` is false (Sonarr's
/// path edit with `MoveSeriesService`).
#[utoipa::path(put, path = "/series/{id}/path", tag = "library", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::library::MoveTitleRequest,
    responses((status = 200, body = kitsunarr_domain::api::library::MoveResult), (status = 404), (status = 422, body = ApiErrorBody)))]
pub async fn move_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<kitsunarr_domain::api::library::MoveTitleRequest>,
) -> ApiResult<Json<kitsunarr_domain::api::library::MoveResult>> {
    use kitsunarr_library::move_title::{Title, move_title};
    let result = move_title(&state.db, Title::Series(id), &req.path, req.move_files).await?;
    changed(&state, "series", vec![id], false);
    Ok(Json(result))
}

/// Moves series into another root folder, each keeping its folder below the root folder
/// (Sonarr's series editor root folder change with `moveFiles`). Failures keep their folder and
/// do not stop the others.
#[utoipa::path(post, path = "/series/move", tag = "library",
    request_body = kitsunarr_domain::api::library::BulkMoveRequest,
    responses((status = 200, body = kitsunarr_domain::api::library::BulkMoveResult), (status = 422, body = ApiErrorBody)))]
pub async fn bulk_move_series(
    State(state): State<AppState>,
    Json(req): Json<kitsunarr_domain::api::library::BulkMoveRequest>,
) -> ApiResult<Json<kitsunarr_domain::api::library::BulkMoveResult>> {
    use kitsunarr_library::move_title::{Title, destination_in_root, move_title};
    if req.ids.len() > 1_000 {
        return Err(ApiError::BadRequest(
            "At most 1000 series can be moved at once.".into(),
        ));
    }
    let root = kitsunarr_db::root_folders::normalize_path(&req.root_folder_path);
    let roots = kitsunarr_db::root_folders::list(state.db.pool()).await?;
    if !roots
        .iter()
        .any(|r| kitsunarr_db::root_folders::normalize_path(&r.path) == root)
    {
        return Err(ApiError::field(
            "rootFolderPath",
            "Choose one of the configured root folders.",
        ));
    }
    let mut out = kitsunarr_domain::api::library::BulkMoveResult::default();
    for id in &req.ids {
        let title = Title::Series(*id);
        let outcome = match destination_in_root(&state.db, title, &root).await {
            Ok(path) => move_title(&state.db, title, &path, req.move_files).await,
            Err(e) => Err(e),
        };
        match outcome {
            Ok(r) => out.moved.push(r),
            Err(e) => {
                let name = series::get_list_item(&state.db, *id)
                    .await
                    .map(|s| s.title)
                    .unwrap_or_else(|_| format!("Series {id}"));
                out.failures.push(format!("{name}: {e}"));
            }
        }
    }
    changed(&state, "series", req.ids.clone(), false);
    Ok(Json(out))
}

/// Changes a film's folder, as for series.
#[utoipa::path(put, path = "/movies/{id}/path", tag = "library", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::library::MoveTitleRequest,
    responses((status = 200, body = kitsunarr_domain::api::library::MoveResult), (status = 404), (status = 422, body = ApiErrorBody)))]
pub async fn move_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<kitsunarr_domain::api::library::MoveTitleRequest>,
) -> ApiResult<Json<kitsunarr_domain::api::library::MoveResult>> {
    use kitsunarr_library::move_title::{Title, move_title};
    let result = move_title(&state.db, Title::Movie(id), &req.path, req.move_files).await?;
    changed(&state, "movie", vec![id], false);
    Ok(Json(result))
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
                ..Default::default()
            },
        )
        .await?;
    }
    changed(&state, "movie", vec![id], false);
    Ok(Json(movies::get_detail(&state.db, id).await?))
}

#[utoipa::path(delete, path = "/movies/{id}", tag = "library", params(("id" = i64, Path), DeleteSeriesQuery),
    responses((status = 204, description = "Removed; the folder is deleted only with deleteFiles")))]
pub async fn delete_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<DeleteSeriesQuery>,
) -> ApiResult<StatusCode> {
    let notification = kitsunarr_library::notify::movie_refs(&state.db, id).await;
    if q.delete_files {
        kitsunarr_library::movie_files::delete_folder(&state.db, id)
            .await
            .map_err(|m| ApiError::field("deleteFiles", m))?;
    }
    movies::delete(&state.db, id).await?;
    if let Some((series, _)) = notification {
        kitsunarr_library::notify::spawn(
            &state.db,
            &state.downloads.notifier,
            kitsunarr_notifications::Event::SeriesDelete {
                series: Box::new(series),
                deleted_files: q.delete_files,
            },
        );
    }
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
    if edit.monitor.is_some() || edit.monitor_new_items.is_some() {
        return Err(ApiError::field(
            "monitor",
            "Episode monitoring choices apply to series only.",
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

/// A collection with its members.
#[utoipa::path(get, path = "/collections/{id}", tag = "library",
    responses((status = 200, body = CollectionDetail), (status = 404, body = ApiErrorBody)))]
pub async fn get_collection(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<CollectionDetail>> {
    Ok(Json(collections::detail(&state.db, id).await?))
}

fn collections_changed(state: &AppState) {
    changed(state, "collection", Vec::new(), false);
}

/// Creates a manual collection (Tanukarr's manual group).
#[utoipa::path(post, path = "/collections", tag = "library", request_body = CollectionEdit,
    responses((status = 201, body = CollectionDetail), (status = 422, body = ApiErrorBody)))]
pub async fn create_collection(
    State(state): State<AppState>,
    Json(edit): Json<CollectionEdit>,
) -> ApiResult<(StatusCode, Json<CollectionDetail>)> {
    let id = collections::create_manual(&state.db, &edit).await?;
    collections_changed(&state);
    Ok((
        StatusCode::CREATED,
        Json(collections::detail(&state.db, id).await?),
    ))
}

/// Renames a collection (any kind; its members stay where they are).
#[utoipa::path(put, path = "/collections/{id}", tag = "library", request_body = CollectionEdit,
    responses((status = 200, body = CollectionDetail), (status = 404, body = ApiErrorBody)))]
pub async fn update_collection(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(edit): Json<CollectionEdit>,
) -> ApiResult<Json<CollectionDetail>> {
    collections::rename(&state.db, id, &edit).await?;
    collections_changed(&state);
    Ok(Json(collections::detail(&state.db, id).await?))
}

/// Deletes a collection; its series and movies stay in the library.
#[utoipa::path(delete, path = "/collections/{id}", tag = "library",
    responses((status = 204), (status = 404, body = ApiErrorBody)))]
pub async fn delete_collection(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    collections::delete(&state.db, id).await?;
    collections_changed(&state);
    Ok(StatusCode::NO_CONTENT)
}

/// Adds a series or movie to a collection; joining a franchise or manual group leaves any
/// other one.
#[utoipa::path(post, path = "/collections/{id}/members", tag = "library", request_body = CollectionMemberEdit,
    responses((status = 200, body = CollectionDetail), (status = 404, body = ApiErrorBody), (status = 409, body = ApiErrorBody)))]
pub async fn add_collection_member(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(edit): Json<CollectionMemberEdit>,
) -> ApiResult<Json<CollectionDetail>> {
    collections::add_member_manually(&state.db, id, &edit).await?;
    collections_changed(&state);
    Ok(Json(collections::detail(&state.db, id).await?))
}

/// Changes a member's role, label or position.
#[utoipa::path(put, path = "/collections/{id}/members/{member_id}", tag = "library", request_body = CollectionMemberEdit,
    responses((status = 200, body = CollectionDetail), (status = 404, body = ApiErrorBody)))]
pub async fn update_collection_member(
    State(state): State<AppState>,
    Path((id, member_id)): Path<(i64, i64)>,
    Json(edit): Json<CollectionMemberEdit>,
) -> ApiResult<Json<CollectionDetail>> {
    collections::update_member(&state.db, id, member_id, &edit).await?;
    collections_changed(&state);
    Ok(Json(collections::detail(&state.db, id).await?))
}

/// Tanukarr's "Search collection": each member's missing episodes, or the film, are searched
/// for in the background.
#[utoipa::path(post, path = "/collections/{id}/search", tag = "library",
    responses((status = 202, body = kitsunarr_domain::api::wanted::WantedSearchStarted), (status = 404, body = ApiErrorBody)))]
pub async fn search_collection(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<(
    StatusCode,
    Json<kitsunarr_domain::api::wanted::WantedSearchStarted>,
)> {
    let detail = collections::detail(&state.db, id).await?;
    let mut count = 0usize;
    for m in &detail.members {
        if let Some(series) = m.series_id {
            crate::routes::wanted::search_missing_of_series(&state, series, true);
            count += 1;
        } else if let Some(movie) = m.movie_id {
            let state = state.clone();
            tokio::spawn(async move {
                if crate::library_tools::paused(&state).await {
                    return;
                }
                if let Ok(run) =
                    crate::routes::movie_search::run_movie_search(&state, movie, true).await
                {
                    crate::routes::movie_search::grab_best(&state, &run, movie, "Search").await;
                }
            });
            count += 1;
        }
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(kitsunarr_domain::api::wanted::WantedSearchStarted {
            episodes: 0,
            searches: count,
            message: format!("Searching {count} titles of {}.", detail.title),
        }),
    ))
}

/// Removes a member (the title itself stays in the library).
#[utoipa::path(delete, path = "/collections/{id}/members/{member_id}", tag = "library",
    responses((status = 200, body = CollectionDetail), (status = 404, body = ApiErrorBody)))]
pub async fn remove_collection_member(
    State(state): State<AppState>,
    Path((id, member_id)): Path<(i64, i64)>,
) -> ApiResult<Json<CollectionDetail>> {
    collections::remove_member(&state.db, id, member_id).await?;
    collections_changed(&state);
    Ok(Json(collections::detail(&state.db, id).await?))
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
    pub series_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub download_id: Option<String>,
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
    for (field, id) in [
        ("seriesId", q.series_id),
        ("episodeId", q.episode_id),
        ("movieId", q.movie_id),
    ] {
        if id.is_some_and(|id| id <= 0) {
            return Err(ApiError::field(field, "Choose a positive library id."));
        }
    }
    if q.movie_id.is_some() && (q.series_id.is_some() || q.episode_id.is_some()) {
        return Err(ApiError::field(
            "movieId",
            "Choose either a movie or a series/episode filter.",
        ));
    }
    let download_id = q
        .download_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if download_id.as_ref().is_some_and(|s| s.len() > 256) {
        return Err(ApiError::field(
            "downloadId",
            "Use a download id of at most 256 bytes.",
        ));
    }
    let filter = kitsunarr_domain::api::library::HistoryFilter {
        event_type: q.event_type.filter(|s| !s.is_empty()),
        series_id: q.series_id,
        episode_id: q.episode_id,
        movie_id: q.movie_id,
        download_id,
    };
    Ok(Json(
        history::list_filtered(&state.db, &paging, &filter).await?,
    ))
}

/// Rescan the series folder (Sonarr's `RescanSeries`): forget files that are gone, add the
/// video files the library does not know (in place), update sizes.
#[utoipa::path(post, path = "/series/{id}/rescan", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = FileOperationResult), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn rescan_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<FileOperationResult>> {
    series::get_list_item(&state.db, id).await?;
    let summary = kitsunarr_library::files::scan_series(&state.db, id)
        .await
        .map_err(|m| ApiError::field("id", m))?;
    changed(&state, "series", vec![id], false);
    Ok(Json(FileOperationResult {
        message: format!(
            "{} files added, {} missing files removed, {} sizes updated.",
            summary.added, summary.removed, summary.updated
        ),
        problems: summary.skipped,
    }))
}

#[derive(Debug, Deserialize, Default, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase", default)]
pub struct RenameQuery {
    pub season_number: Option<i32>,
}

/// The series' files whose names differ from the naming settings (Sonarr's `GET /rename`).
#[utoipa::path(get, path = "/series/{id}/rename", tag = "library", params(("id" = i64, Path), RenameQuery),
    responses((status = 200, body = Vec<RenamePreview>)))]
pub async fn rename_preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<RenameQuery>,
) -> ApiResult<Json<Vec<RenamePreview>>> {
    series::get_list_item(&state.db, id).await?;
    Ok(Json(
        kitsunarr_library::files::rename_previews(&state.db, id, q.season_number)
            .await
            .map_err(ApiError::Internal)?,
    ))
}

/// Rename the chosen files (all previewed files when none is given).
#[utoipa::path(post, path = "/series/{id}/rename", tag = "library", params(("id" = i64, Path)),
    request_body = RenameRequest, responses((status = 200, body = FileOperationResult)))]
pub async fn rename_series_files(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<RenameRequest>,
) -> ApiResult<Json<FileOperationResult>> {
    let (renamed, problems) = rename_and_notify(&state, id, &req.episode_file_ids).await?;
    Ok(Json(FileOperationResult {
        message: format!("{renamed} files renamed."),
        problems,
    }))
}

/// Renames a series' chosen files (all when none is chosen) and sends Sonarr's `OnRename`;
/// returns how many were renamed and the problems.
async fn rename_and_notify(
    state: &AppState,
    id: i64,
    file_ids: &[i64],
) -> ApiResult<(usize, Vec<String>)> {
    let req = RenameRequest {
        episode_file_ids: file_ids.to_vec(),
    };
    series::get_list_item(&state.db, id).await?;
    let (renamed, problems) =
        kitsunarr_library::files::rename_files(&state.db, id, &req.episode_file_ids)
            .await
            .map_err(ApiError::Internal)?;
    changed(state, "series", vec![id], false);
    // Sonarr's `OnRename`.
    if !renamed.is_empty()
        && let Some(series) = kitsunarr_library::notify::series_ref(&state.db, id).await
    {
        let base = std::path::PathBuf::from(&series.path);
        let files = renamed
            .iter()
            .map(|(file_id, old, new)| kitsunarr_notifications::RenamedFile {
                id: *file_id,
                previous_relative_path: old.clone(),
                previous_path: base.join(old).display().to_string(),
                relative_path: new.clone(),
                path: base.join(new).display().to_string(),
            })
            .collect();
        kitsunarr_library::notify::spawn(
            &state.db,
            &state.downloads.notifier,
            kitsunarr_notifications::Event::Rename {
                series: Box::new(series),
                files,
            },
        );
    }
    Ok((renamed.len(), problems))
}

/// Renames every file of several series to the naming settings (Sonarr's series editor
/// "Organize"), each as `POST /series/{id}/rename` with no file chosen.
#[utoipa::path(post, path = "/series/rename", tag = "library",
    request_body = kitsunarr_domain::api::library::BulkRenameRequest,
    responses((status = 200, body = FileOperationResult)))]
pub async fn bulk_rename_series(
    State(state): State<AppState>,
    Json(req): Json<kitsunarr_domain::api::library::BulkRenameRequest>,
) -> ApiResult<Json<FileOperationResult>> {
    if req.ids.len() > 1_000 {
        return Err(ApiError::BadRequest(
            "At most 1000 series can be renamed at once.".into(),
        ));
    }
    let (mut renamed, mut problems) = (0usize, Vec::new());
    for id in &req.ids {
        match rename_and_notify(&state, *id, &[]).await {
            Ok((n, found)) => {
                renamed += n;
                let title = series::get_list_item(&state.db, *id)
                    .await
                    .map(|s| s.title)
                    .unwrap_or_default();
                problems.extend(found.into_iter().map(|p| format!("{title}: {p}")));
            }
            Err(e) => problems.push(format!("Series {id}: {}", e.message())),
        }
    }
    Ok(Json(FileOperationResult {
        message: format!("{renamed} files renamed in {} series.", req.ids.len()),
        problems,
    }))
}

/// Delete an episode file (to the recycle bin when one is set).
#[utoipa::path(delete, path = "/series/{id}/files/{file_id}", tag = "library",
    params(("id" = i64, Path), ("file_id" = i64, Path)),
    responses((status = 204), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn delete_episode_file(
    State(state): State<AppState>,
    Path((id, file_id)): Path<(i64, i64)>,
) -> ApiResult<StatusCode> {
    let episode_ids: Vec<i64> =
        kitsunarr_db::sqlx::query_scalar("SELECT id FROM episodes WHERE episode_file_id = ?")
            .bind(file_id)
            .fetch_all(state.db.pool())
            .await
            .unwrap_or_default();
    kitsunarr_library::files::delete_file(&state.db, id, file_id)
        .await
        .map_err(|m| ApiError::field("fileId", m))?;
    state.events.publish(ServerEvent::EpisodeFileChanged {
        series_id: id,
        episode_file_id: file_id,
        episode_ids,
        deleted: true,
    });
    changed(&state, "series", vec![id], false);
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/movies/{id}/rescan", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = FileOperationResult), (status = 404), (status = 422, body = ApiErrorBody)))]
pub async fn rescan_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<FileOperationResult>> {
    movies::get_detail(&state.db, id).await?;
    let summary =
        kitsunarr_library::movie_files::scan_notifying(&state.db, id, &state.downloads.notifier)
            .await
            .map_err(|m| ApiError::field("id", m))?;
    changed(&state, "movie", vec![id], false);
    Ok(Json(FileOperationResult {
        message: format!("{}.", summary.describe()),
        problems: summary.skipped,
    }))
}

#[utoipa::path(get, path = "/movies/{id}/rename", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = Option<MovieRenamePreview>), (status = 404), (status = 422, body = ApiErrorBody)))]
pub async fn movie_rename_preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Option<MovieRenamePreview>>> {
    movies::get_detail(&state.db, id).await?;
    Ok(Json(
        kitsunarr_library::movie_files::rename_preview(&state.db, id)
            .await
            .map_err(|m| ApiError::field("id", m))?,
    ))
}

#[utoipa::path(post, path = "/movies/{id}/rename", tag = "library", params(("id" = i64, Path)), request_body = MovieRenamePreview,
    responses((status = 200, body = FileOperationResult), (status = 404), (status = 422, body = ApiErrorBody)))]
pub async fn rename_movie_file(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(preview): Json<MovieRenamePreview>,
) -> ApiResult<Json<FileOperationResult>> {
    movies::get_detail(&state.db, id).await?;
    kitsunarr_library::movie_files::rename(&state.db, id, &preview)
        .await
        .map_err(|m| ApiError::field("preview", m))?;
    changed(&state, "movie", vec![id], false);
    if let Some((series, _)) = kitsunarr_library::notify::movie_refs(&state.db, id).await {
        let base = std::path::Path::new(&series.path);
        let file = kitsunarr_notifications::RenamedFile {
            id: preview.movie_file_id,
            previous_relative_path: preview.existing_path.clone(),
            previous_path: base.join(&preview.existing_path).display().to_string(),
            relative_path: preview.new_path.clone(),
            path: base.join(&preview.new_path).display().to_string(),
        };
        kitsunarr_library::notify::spawn(
            &state.db,
            &state.downloads.notifier,
            kitsunarr_notifications::Event::Rename {
                series: Box::new(series),
                files: vec![file],
            },
        );
    }
    Ok(Json(FileOperationResult {
        message: "1 file renamed.".into(),
        problems: vec![],
    }))
}

#[utoipa::path(delete, path = "/movies/{id}/files/{file_id}", tag = "library", params(("id" = i64, Path),("file_id" = i64, Path)),
    responses((status = 204), (status = 404), (status = 422, body = ApiErrorBody)))]
pub async fn delete_movie_file(
    State(state): State<AppState>,
    Path((id, file_id)): Path<(i64, i64)>,
) -> ApiResult<StatusCode> {
    movies::get_detail(&state.db, id).await?;
    kitsunarr_library::movie_files::delete_file_notifying(
        &state.db,
        id,
        file_id,
        &state.downloads.notifier,
    )
    .await
    .map_err(|m| ApiError::field("fileId", m))?;
    changed(&state, "movie", vec![id], false);
    Ok(StatusCode::NO_CONTENT)
}

/// Marks a movie watched or unwatched without changing its monitoring choice. When enabled,
/// the next Jellyfin watched sync follows Jellyfin's current state.
#[utoipa::path(put,path="/movies/{id}/watched",tag="library",params(("id"=i64,Path)),
    request_body=kitsunarr_domain::api::library::MovieWatchedUpdate,responses((status=200,body=MovieDetail),(status=404)))]
pub async fn set_movie_watched(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<kitsunarr_domain::api::library::MovieWatchedUpdate>,
) -> ApiResult<Json<MovieDetail>> {
    movies::get_detail(&state.db, id).await?;
    movies::set_watched(&state.db, &[id], body.watched).await?;
    changed(&state, "movie", vec![id], false);
    Ok(Json(movies::get_detail(&state.db, id).await?))
}

#[utoipa::path(put,path="/movies/watched",tag="library",request_body=kitsunarr_domain::api::library::MoviesWatchedUpdate,
    responses((status=200,body=BulkEditResult),(status=422,body=ApiErrorBody)))]
pub async fn set_movies_watched(
    State(state): State<AppState>,
    Json(body): Json<kitsunarr_domain::api::library::MoviesWatchedUpdate>,
) -> ApiResult<Json<BulkEditResult>> {
    if body.ids.is_empty() || body.ids.len() > 10_000 || body.ids.iter().any(|id| *id < 1) {
        return Err(ApiError::field(
            "ids",
            "Choose 1 to 10000 positive movie ids.",
        ));
    }
    let updated = movies::set_watched(&state.db, &body.ids, body.watched).await?;
    changed(&state, "movie", body.ids, false);
    Ok(Json(BulkEditResult { updated }))
}
