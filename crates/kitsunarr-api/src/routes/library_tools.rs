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
    let roots = kitsunarr_db::root_folders::list(state.db.pool()).await?;
    let mut out = Vec::new();
    for root in roots {
        let root_path = std::path::Path::new(&root.path);
        let mut seen = std::collections::HashSet::new();
        let (mut count, mut remaining, mut total) = (0i64, 0u64, 0u64);
        for item in &queue.items {
            let in_root = item.series_id.is_some_and(|id| {
                series_paths
                    .iter()
                    .any(|(sid, p)| *sid == id && std::path::Path::new(p).starts_with(root_path))
            });
            if !in_root
                || matches!(
                    item.tracked_state.as_str(),
                    "imported" | "ignored" | "failed"
                )
                || !seen.insert((item.download_client.clone(), item.download_id.clone()))
            {
                continue;
            }
            count += 1;
            remaining += item.size_left.max(0) as u64;
            total += item.size.max(0) as u64;
        }
        let estimate = if tools.hardlink_only { 0 } else { total };
        let free = fs4::available_space(root_path).ok();
        out.push(StorageOutlook {
            root_folder: root.path.clone(),
            free_space: free,
            queued_downloads: count,
            download_remaining: remaining,
            import_space_estimate: estimate,
            may_run_out_of_space: free.is_some_and(|f| estimate > f),
            import_mode: if tools.hardlink_only {
                "Hardlink-only: imports take no extra space".into()
            } else if media.use_hardlinks {
                "Hard link when possible, else copy: the estimate assumes copies".into()
            } else {
                "Copy: imports take the downloads' size again".into()
            },
        });
    }
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
