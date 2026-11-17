//! Custom filters: saved filters of the web interface's lists (Sonarr's `CustomFilter`; the
//! same records back `/api/v3/customfilter` and `/api/v5/customfilter`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kitsunarr_db::custom_filters;
use kitsunarr_domain::api::settings::CustomFilter;

use crate::AppState;
use crate::error::ApiResult;

fn changed(state: &AppState) {
    crate::routes::settings::settings_changed(state, "customFilters");
}

#[utoipa::path(get, path = "/customfilters", tag = "settings",
    responses((status = 200, body = Vec<CustomFilter>)))]
pub async fn list_custom_filters(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<CustomFilter>>> {
    Ok(Json(custom_filters::list(state.db.pool()).await?))
}

#[utoipa::path(post, path = "/customfilters", tag = "settings", request_body = CustomFilter,
    responses((status = 201, body = CustomFilter), (status = 400, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn create_custom_filter(
    State(state): State<AppState>,
    Json(mut body): Json<CustomFilter>,
) -> ApiResult<(StatusCode, Json<CustomFilter>)> {
    body.id = custom_filters::create(state.db.pool(), &body).await?;
    changed(&state);
    Ok((
        StatusCode::CREATED,
        Json(custom_filters::get(state.db.pool(), body.id).await?),
    ))
}

#[utoipa::path(put, path = "/customfilters/{id}", tag = "settings", params(("id" = i64, Path)),
    request_body = CustomFilter,
    responses((status = 200, body = CustomFilter), (status = 404, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn update_custom_filter(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut body): Json<CustomFilter>,
) -> ApiResult<Json<CustomFilter>> {
    body.id = id;
    custom_filters::update(state.db.pool(), &body).await?;
    changed(&state);
    Ok(Json(custom_filters::get(state.db.pool(), id).await?))
}

#[utoipa::path(delete, path = "/customfilters/{id}", tag = "settings", params(("id" = i64, Path)),
    responses((status = 204), (status = 404, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn delete_custom_filter(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    custom_filters::delete(state.db.pool(), id).await?;
    changed(&state);
    Ok(StatusCode::NO_CONTENT)
}
