//! Library tools settings and recent searches.

use axum::Json;
use axum::extract::State;
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::settings::{LibraryToolsSettings, SearchActivity};

use crate::AppState;
use crate::error::ApiResult;

#[utoipa::path(get, path = "/settings/librarytools", tag = "settings",
    responses((status = 200, body = LibraryToolsSettings)))]
pub async fn get_settings(State(state): State<AppState>) -> Json<LibraryToolsSettings> {
    Json(crate::library_tools::settings(&state).await)
}

#[utoipa::path(put, path = "/settings/librarytools", tag = "settings",
    request_body = LibraryToolsSettings, responses((status = 200, body = LibraryToolsSettings)))]
pub async fn put_settings(
    State(state): State<AppState>,
    Json(body): Json<LibraryToolsSettings>,
) -> ApiResult<Json<LibraryToolsSettings>> {
    kv::set(state.db.pool(), kv::KEY_LIBRARY_TOOLS, &body).await?;
    crate::routes::settings::settings_changed(&state, "libraryTools");
    Ok(Json(body))
}

/// The last 100 searches: what was found and accepted, the common rejections, failing indexers.
#[utoipa::path(get, path = "/librarytools/searches", tag = "system",
    responses((status = 200, body = Vec<SearchActivity>)))]
pub async fn searches(State(state): State<AppState>) -> Json<Vec<SearchActivity>> {
    Json(crate::library_tools::recent_searches(&state).await)
}

/// Per root folder: the downloads in progress for its series and whether their imports fit.
#[utoipa::path(get, path = "/librarytools/storage", tag = "system",
    responses((status = 200, body = Vec<kitsunarr_domain::api::library_tools::StorageOutlook>)))]
pub async fn storage(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library_tools::StorageOutlook>>> {
    use kitsunarr_domain::api::library_tools::StorageOutlook;
    let tools = crate::library_tools::settings(&state).await;
    let media: kitsunarr_domain::api::settings::MediaManagementSettings =
        kv::get_or_default(state.db.pool(), kv::KEY_MEDIA_MANAGEMENT).await?;
    let queue = crate::routes::queue::get_queue(State(state.clone()))
        .await?
        .0;
    let series_paths: Vec<(i64, String)> = kitsunarr_db::library::series::paths(&state.db).await?;
    let movie_paths: Vec<(i64, String)> =
        kitsunarr_db::sqlx::query_as("SELECT id,path FROM movies")
            .fetch_all(state.db.pool())
            .await
            .map_err(kitsunarr_db::DbError::from)?;
    let roots = kitsunarr_db::root_folders::list(state.db.pool())
        .await?
        .into_iter()
        .map(|r| r.path)
        .collect::<Vec<_>>();
    let out: Vec<StorageOutlook> = kitsunarr_library::tools::storage_outlook(
        &roots,
        &series_paths,
        &movie_paths,
        &queue.items,
        tools.hardlink_only,
        media.use_hardlinks,
    );
    Ok(Json(out))
}

/// A read-only check: missing series folders, file records outside their folder, duplicate
/// records, files missing on disk.
#[utoipa::path(post, path = "/librarytools/audit", tag = "system",
    responses((status = 200, body = kitsunarr_domain::api::library_tools::LibraryCheck)))]
pub async fn audit(
    State(state): State<AppState>,
) -> ApiResult<Json<kitsunarr_domain::api::library_tools::LibraryCheck>> {
    Ok(Json(
        kitsunarr_library::tools::check_library(&state.db)
            .await
            .map_err(crate::error::ApiError::internal)?,
    ))
}

/// The media and subtitle files of the recycle bin.
#[utoipa::path(get, path = "/librarytools/recycle", tag = "system",
    responses((status = 200, body = kitsunarr_domain::api::library_tools::RecycleBin)))]
pub async fn recycle(
    State(state): State<AppState>,
) -> ApiResult<Json<kitsunarr_domain::api::library_tools::RecycleBin>> {
    Ok(Json(
        kitsunarr_library::tools::browse_recycle_bin(&state.db)
            .await
            .map_err(crate::error::ApiError::internal)?,
    ))
}

/// Moves a recycled file back into a series' season folder (never overwriting) and rescans the
/// series.
#[utoipa::path(post, path = "/librarytools/recycle/restore", tag = "system",
    request_body = kitsunarr_domain::api::library_tools::RecycleRestoreRequest,
    responses((status = 200, body = String), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn restore(
    State(state): State<AppState>,
    Json(body): Json<kitsunarr_domain::api::library_tools::RecycleRestoreRequest>,
) -> ApiResult<Json<String>> {
    crate::routes::compression::ensure_idle(body.series_id).await?;
    let message = kitsunarr_library::tools::restore_recycled(
        &state.db,
        &body.token,
        body.series_id,
        body.season_number,
    )
    .await
    .map_err(|m| crate::error::ApiError::field("token", m))?;
    state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: vec![body.series_id],
            deleted: false,
        });
    Ok(Json(message))
}

/// A series' renaming mode and season folder titles.
#[utoipa::path(get, path = "/series/{id}/naming", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = kitsunarr_domain::api::library_tools::SeriesNamingOptions), (status = 404, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn get_series_naming(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> ApiResult<Json<kitsunarr_domain::api::library_tools::SeriesNamingOptions>> {
    Ok(Json(
        kitsunarr_db::library::series::naming_options(&state.db, id).await?,
    ))
}

#[utoipa::path(put, path = "/series/{id}/naming", tag = "library", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::library_tools::SeriesNamingOptions,
    responses((status = 200, body = kitsunarr_domain::api::library_tools::SeriesNamingOptions), (status = 400, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn put_series_naming(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(body): Json<kitsunarr_domain::api::library_tools::SeriesNamingOptions>,
) -> ApiResult<Json<kitsunarr_domain::api::library_tools::SeriesNamingOptions>> {
    kitsunarr_db::library::series::set_naming_options(&state.db, id, &body).await?;
    Ok(Json(
        kitsunarr_db::library::series::naming_options(&state.db, id).await?,
    ))
}

fn key_item(
    k: kitsunarr_db::companion_keys::CompanionKey,
) -> kitsunarr_domain::api::library_tools::CompanionKeyItem {
    kitsunarr_domain::api::library_tools::CompanionKeyItem {
        id: k.id,
        name: k.name,
        permission: k.permission,
        created: k.created_at,
        expires: k.expires_at,
    }
}

/// Companion app keys: scoped, expiring keys for the Sonarr v3 API (reading the library, or
/// also adding series).
#[utoipa::path(get, path = "/librarytools/keys", tag = "system",
    responses((status = 200, body = Vec<kitsunarr_domain::api::library_tools::CompanionKeyItem>)))]
pub async fn list_keys(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library_tools::CompanionKeyItem>>> {
    Ok(Json(
        kitsunarr_db::companion_keys::list(state.db.pool())
            .await?
            .into_iter()
            .map(key_item)
            .collect(),
    ))
}

#[utoipa::path(post, path = "/librarytools/keys", tag = "system",
    request_body = kitsunarr_domain::api::library_tools::CompanionKeyCreate,
    responses((status = 201, body = kitsunarr_domain::api::library_tools::CompanionKeyCreated), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn create_key(
    State(state): State<AppState>,
    Json(body): Json<kitsunarr_domain::api::library_tools::CompanionKeyCreate>,
) -> ApiResult<(
    axum::http::StatusCode,
    Json<kitsunarr_domain::api::library_tools::CompanionKeyCreated>,
)> {
    use crate::error::ApiError;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::field(
            "name",
            "A name of 1 to 80 characters is required.",
        ));
    }
    if !matches!(body.permission.as_str(), "read-library" | "add-series") {
        return Err(ApiError::field(
            "permission",
            "Choose read-library or add-series.",
        ));
    }
    if !(1..=365).contains(&body.days) {
        return Err(ApiError::field("days", "Keys are valid for 1 to 365 days."));
    }
    let token = kitsunarr_auth::generate_companion_token();
    let expires = (chrono::Utc::now() + chrono::Duration::days(body.days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let key = kitsunarr_db::companion_keys::create(
        state.db.pool(),
        &kitsunarr_auth::generate_api_key(),
        name,
        &kitsunarr_auth::hash_companion_token(&token),
        &body.permission,
        &expires,
    )
    .await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(kitsunarr_domain::api::library_tools::CompanionKeyCreated {
            key: key_item(key),
            token,
        }),
    ))
}

#[utoipa::path(delete, path = "/librarytools/keys/{id}", tag = "system", params(("id" = String, Path)),
    responses((status = 204)))]
pub async fn delete_key(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    kitsunarr_db::companion_keys::delete(state.db.pool(), &id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Files upgrades replaced in a series with "keep versions" on, newest first.
#[utoipa::path(get, path = "/series/{id}/versions", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = Vec<kitsunarr_domain::api::library_tools::RetainedVersion>)))]
pub async fn versions(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library_tools::RetainedVersion>>> {
    kitsunarr_db::library::series::get_list_item(&state.db, id).await?;
    Ok(Json(
        kitsunarr_library::versions::list(&state.db, id)
            .await
            .map_err(crate::error::ApiError::internal)?,
    ))
}

/// Makes a kept version the current file again (the current file is kept as a version).
#[utoipa::path(post, path = "/series/{id}/versions/{version}/activate", tag = "library",
    params(("id" = i64, Path), ("version" = i64, Path)),
    responses((status = 200, body = String), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn activate_version(
    State(state): State<AppState>,
    axum::extract::Path((id, version)): axum::extract::Path<(i64, i64)>,
) -> ApiResult<Json<String>> {
    crate::routes::compression::ensure_idle(id).await?;
    let message = kitsunarr_library::versions::activate(&state.db, id, version)
        .await
        .map_err(|m| crate::error::ApiError::field("version", m))?;
    state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: vec![id],
            deleted: false,
        });
    Ok(Json(message))
}

#[utoipa::path(delete, path = "/series/{id}/versions/{version}", tag = "library",
    params(("id" = i64, Path), ("version" = i64, Path)), responses((status = 204)))]
pub async fn delete_version(
    State(state): State<AppState>,
    axum::extract::Path((id, version)): axum::extract::Path<(i64, i64)>,
) -> ApiResult<axum::http::StatusCode> {
    kitsunarr_library::versions::delete(&state.db, id, version)
        .await
        .map_err(|m| crate::error::ApiError::field("version", m))?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ManualImportQuery {
    /// A folder (or one file) on the server.
    pub folder: String,
    /// Match every file to this series.
    pub series_id: Option<i64>,
}

/// Manual import: the video files of a folder with the series and episodes they match.
#[utoipa::path(get, path = "/manualimport", tag = "library", params(ManualImportQuery),
    responses((status = 200, body = Vec<kitsunarr_domain::api::library_tools::ManualImportItem>), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn manual_import_scan(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ManualImportQuery>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library_tools::ManualImportItem>>> {
    let folder = std::path::PathBuf::from(q.folder.trim());
    if !folder.is_absolute() {
        return Err(crate::error::ApiError::field(
            "folder",
            "Enter an absolute path.",
        ));
    }
    Ok(Json(
        kitsunarr_library::manual_import::scan(&state.db, &folder, q.series_id)
            .await
            .map_err(|m| crate::error::ApiError::field("folder", m))?,
    ))
}

/// Imports the chosen files with their series and episodes (copying, or moving them).
#[utoipa::path(post, path = "/manualimport", tag = "library",
    request_body = kitsunarr_domain::api::library_tools::ManualImportRequest,
    responses((status = 200, body = Vec<kitsunarr_domain::api::library_tools::ManualImportResult>)))]
pub async fn manual_import(
    State(state): State<AppState>,
    Json(req): Json<kitsunarr_domain::api::library_tools::ManualImportRequest>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library_tools::ManualImportResult>>> {
    use kitsunarr_domain::api::library_tools::ManualImportResult;
    let move_files = req.import_mode.eq_ignore_ascii_case("move");
    let outcomes = kitsunarr_library::manual_import::import(
        &state.db,
        &state.downloads.notifier,
        &req.files,
        move_files,
    )
    .await;
    let mut series_ids: Vec<i64> = req.files.iter().map(|f| f.series_id).collect();
    series_ids.sort_unstable();
    series_ids.dedup();
    state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: series_ids,
            deleted: false,
        });
    Ok(Json(
        outcomes
            .into_iter()
            .map(|(path, r)| match r {
                Ok(outcome) => {
                    let mut messages: Vec<String> = outcome.blocked.into_iter().collect();
                    messages.extend(outcome.results.iter().flat_map(|f| f.errors.clone()));
                    ManualImportResult {
                        path,
                        imported: outcome.results.iter().any(|f| f.imported),
                        messages,
                    }
                }
                Err(m) => ManualImportResult {
                    path,
                    imported: false,
                    messages: vec![m],
                },
            })
            .collect(),
    ))
}

#[utoipa::path(post, path="/librarytools/recycle/restore-movie",tag="system",
    request_body=kitsunarr_domain::api::library_tools::MovieRecycleRestoreRequest,
    responses((status=200,body=String),(status=404),(status=422,body=kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn restore_movie(
    State(state): State<AppState>,
    Json(body): Json<kitsunarr_domain::api::library_tools::MovieRecycleRestoreRequest>,
) -> ApiResult<Json<String>> {
    kitsunarr_db::library::movies::get_detail(&state.db, body.movie_id).await?;
    let message = kitsunarr_library::tools::restore_movie(&state.db, &body.token, body.movie_id)
        .await
        .map_err(|m| crate::error::ApiError::field("token", m))?;
    state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "movie".into(),
            ids: vec![body.movie_id],
            deleted: false,
        });
    Ok(Json(message))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct MovieImportQuery {
    pub folder: String,
    pub movie_id: Option<i64>,
}

#[utoipa::path(get,path="/manualimport/movies",tag="library",params(MovieImportQuery),
    responses((status=200,body=Vec<kitsunarr_domain::api::library_tools::MovieImportItem>),(status=422,body=kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn movie_import_scan(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<MovieImportQuery>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library_tools::MovieImportItem>>> {
    Ok(Json(
        kitsunarr_library::movie_manual_import::scan(
            &state.db,
            std::path::Path::new(query.folder.trim()),
            query.movie_id,
        )
        .await
        .map_err(|m| crate::error::ApiError::field("folder", m))?,
    ))
}

#[utoipa::path(post,path="/manualimport/movies",tag="library",request_body=kitsunarr_domain::api::library_tools::MovieImportRequest,
    responses((status=200,body=Vec<kitsunarr_domain::api::library_tools::ManualImportResult>),(status=422,body=kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn movie_import(
    State(state): State<AppState>,
    Json(body): Json<kitsunarr_domain::api::library_tools::MovieImportRequest>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library_tools::ManualImportResult>>> {
    if body.files.is_empty() || body.files.len() > 200 {
        return Err(crate::error::ApiError::field(
            "files",
            "Choose between 1 and 200 movie files.",
        ));
    }
    if !matches!(body.import_mode.as_str(), "" | "copy" | "move") {
        return Err(crate::error::ApiError::field(
            "importMode",
            "Choose copy or move.",
        ));
    }
    let results = kitsunarr_library::movie_manual_import::import_notifying(
        &state.db,
        &body.files,
        body.import_mode == "move",
        &state.downloads.notifier,
    )
    .await;
    let mut ids = body
        .files
        .iter()
        .zip(&results)
        .filter(|(_, r)| r.imported)
        .map(|(f, _)| f.movie_id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    if !ids.is_empty() {
        state
            .events
            .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
                media_type: "movie".into(),
                ids,
                deleted: false,
            });
    }
    Ok(Json(results))
}
