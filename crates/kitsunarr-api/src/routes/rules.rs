//! Delay profiles, release profiles and remote path mappings (Sonarr's Settings › Profiles and
//! Settings › Download Clients).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kitsunarr_db::rules::{self, RuleTable};
use kitsunarr_domain::api::settings::{DelayProfile, ReleaseProfile, RemotePathMapping};

use crate::AppState;
use crate::error::ApiResult;

fn changed(state: &AppState, section: &str) {
    crate::routes::settings::settings_changed(state, section);
}

#[utoipa::path(get, path = "/delayprofiles", tag = "settings", responses((status = 200, body = Vec<DelayProfile>)))]
pub async fn list_delay(State(state): State<AppState>) -> ApiResult<Json<Vec<DelayProfile>>> {
    Ok(Json(rules::list_delay_profiles(state.db.pool()).await?))
}

#[utoipa::path(post, path = "/delayprofiles", tag = "settings", request_body = DelayProfile,
    responses((status = 201, body = DelayProfile), (status = 400, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn create_delay(
    State(state): State<AppState>,
    Json(mut body): Json<DelayProfile>,
) -> ApiResult<(StatusCode, Json<DelayProfile>)> {
    // New profiles go before the default one (which stays last).
    if body.sort_order <= 0 {
        let max = rules::list_delay_profiles(state.db.pool())
            .await?
            .iter()
            .map(|d| d.sort_order)
            .filter(|o| *o < i64::from(i32::MAX))
            .max()
            .unwrap_or(0);
        body.sort_order = max + 1;
    }
    body.id = rules::create_delay_profile(state.db.pool(), &body).await?;
    changed(&state, "delayProfiles");
    Ok((StatusCode::CREATED, Json(body)))
}

#[utoipa::path(put, path = "/delayprofiles/{id}", tag = "settings", params(("id" = i64, Path)), request_body = DelayProfile,
    responses((status = 200, body = DelayProfile), (status = 400, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn update_delay(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut body): Json<DelayProfile>,
) -> ApiResult<Json<DelayProfile>> {
    body.id = id;
    rules::update_delay_profile(state.db.pool(), &body).await?;
    changed(&state, "delayProfiles");
    Ok(Json(body))
}

#[utoipa::path(delete, path = "/delayprofiles/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 204)))]
pub async fn delete_delay(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    rules::delete_rule(state.db.pool(), RuleTable::DelayProfiles, id).await?;
    changed(&state, "delayProfiles");
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/releaseprofiles", tag = "settings", responses((status = 200, body = Vec<ReleaseProfile>)))]
pub async fn list_release(State(state): State<AppState>) -> ApiResult<Json<Vec<ReleaseProfile>>> {
    Ok(Json(rules::list_release_profiles(state.db.pool()).await?))
}

#[utoipa::path(post, path = "/releaseprofiles", tag = "settings", request_body = ReleaseProfile,
    responses((status = 201, body = ReleaseProfile), (status = 400, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn create_release(
    State(state): State<AppState>,
    Json(mut body): Json<ReleaseProfile>,
) -> ApiResult<(StatusCode, Json<ReleaseProfile>)> {
    if body.required.is_empty() && body.ignored.is_empty() {
        return Err(crate::error::ApiError::field(
            "required",
            "Add required or ignored terms.",
        ));
    }
    body.id = rules::create_release_profile(state.db.pool(), &body).await?;
    changed(&state, "releaseProfiles");
    Ok((StatusCode::CREATED, Json(body)))
}

#[utoipa::path(put, path = "/releaseprofiles/{id}", tag = "settings", params(("id" = i64, Path)), request_body = ReleaseProfile,
    responses((status = 200, body = ReleaseProfile), (status = 400, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn update_release(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut body): Json<ReleaseProfile>,
) -> ApiResult<Json<ReleaseProfile>> {
    body.id = id;
    rules::update_release_profile(state.db.pool(), &body).await?;
    changed(&state, "releaseProfiles");
    Ok(Json(body))
}

#[utoipa::path(delete, path = "/releaseprofiles/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 204)))]
pub async fn delete_release(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    rules::delete_rule(state.db.pool(), RuleTable::ReleaseProfiles, id).await?;
    changed(&state, "releaseProfiles");
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/remotepathmappings", tag = "settings", responses((status = 200, body = Vec<RemotePathMapping>)))]
pub async fn list_mappings(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<RemotePathMapping>>> {
    Ok(Json(
        rules::list_remote_path_mappings(state.db.pool()).await?,
    ))
}

#[utoipa::path(post, path = "/remotepathmappings", tag = "settings", request_body = RemotePathMapping,
    responses((status = 201, body = RemotePathMapping), (status = 400, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn create_mapping(
    State(state): State<AppState>,
    Json(mut body): Json<RemotePathMapping>,
) -> ApiResult<(StatusCode, Json<RemotePathMapping>)> {
    body.id = rules::create_remote_path_mapping(
        state.db.pool(),
        &body.host,
        &body.remote_path,
        &body.local_path,
    )
    .await?;
    changed(&state, "remotePathMappings");
    Ok((StatusCode::CREATED, Json(body)))
}

#[utoipa::path(put, path = "/remotepathmappings/{id}", tag = "settings", params(("id" = i64, Path)), request_body = RemotePathMapping,
    responses((status = 200, body = RemotePathMapping), (status = 400, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn update_mapping(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut body): Json<RemotePathMapping>,
) -> ApiResult<Json<RemotePathMapping>> {
    body.id = id;
    rules::update_remote_path_mapping(state.db.pool(), &body).await?;
    changed(&state, "remotePathMappings");
    Ok(Json(body))
}

#[utoipa::path(delete, path = "/remotepathmappings/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 204)))]
pub async fn delete_mapping(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    rules::delete_rule(state.db.pool(), RuleTable::RemotePathMappings, id).await?;
    changed(&state, "remotePathMappings");
    Ok(StatusCode::NO_CONTENT)
}
