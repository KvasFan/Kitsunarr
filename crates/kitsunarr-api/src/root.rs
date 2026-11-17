//! Sonarr's routes outside its API folders: `/ping` (anonymous health probe), `/api` (the API
//! versions), `/content/{path}` (the interface's static files) and the interface itself at `/`
//! and `/{path}` (the same handler as the router's fallback).

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};

use crate::AppState;

/// Anonymous routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ping", get(ping).head(ping))
        .route("/content/{*path}", get(content))
        .route("/", get(crate::assets::serve))
        .route("/{path}", get(crate::assets::serve))
}

/// `GET /api`, behind the Sonarr API's authentication.
pub fn api_info_router() -> Router<AppState> {
    Router::new().route("/api", get(api_info))
}

/// Sonarr's `PingController`: `{"status": "OK"}` while the database answers, else 500 with
/// `{"status": "Error"}`.
async fn ping(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let ok = kitsunarr_db::sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(state.db.pool())
        .await
        .is_ok();
    if ok {
        (StatusCode::OK, Json(json!({"status": "OK"})))
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "Error"})),
        )
    }
}

/// Sonarr 4's `ApiInfoController` (the version Kitsunarr reports): v3 is current.
async fn api_info() -> Json<Value> {
    Json(json!({"current": "v3", "deprecated": []}))
}

/// `GET /content/{path}`: one of the interface's static files (no index fallback).
async fn content(
    State(state): State<AppState>,
    Path(path): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if path.split('/').any(|s| s == ".." || s.is_empty()) || path.contains('\\') {
        return StatusCode::NOT_FOUND.into_response();
    }
    for candidate in [path.clone(), format!("assets/{path}")] {
        if state.assets.get(&candidate).is_some() {
            let uri: Uri = match format!("/{candidate}").parse() {
                Ok(u) => u,
                Err(_) => return StatusCode::NOT_FOUND.into_response(),
            };
            return crate::assets::serve(State(state), method, uri, headers).await;
        }
    }
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain")],
        "Not found",
    )
        .into_response()
}
