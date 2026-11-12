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
