//! `POST /providers/{kind}/testall`: tests every enabled provider of a kind with its stored
//! settings (Sonarr's "Test All" button), answered as Sonarr's `ProviderTestAllResult` list:
//! `{id, isValid, validationFailures: [{propertyName, errorMessage, severity}]}`, with 400
//! when one fails.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kitsunarr_db::providers;
use serde_json::Value;

use crate::AppState;
use crate::error::ApiResult;

#[utoipa::path(post, path = "/providers/{kind}/testall", tag = "settings",
    params(("kind" = String, Path, description = "indexers | downloadclients | notifications | importlists")),
    responses((status = 200, body = Vec<Object>, description = "Every test passed"),
        (status = 400, body = Vec<Object>, description = "At least one test failed")))]
pub async fn test_all_providers(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> ApiResult<(StatusCode, Json<Vec<Value>>)> {
    let kind = crate::routes::settings::parse_kind(&kind)?;
    let mut out = Vec::new();
    for p in providers::list(state.db.pool(), kind).await? {
        if !p.enabled {
            continue;
        }
        let failure = match crate::v3_ext::test_stored(&state, &p).await {
            Ok(m) => m,
            Err(e) => Some(e.message()),
        };
        out.push(crate::v3_ext::test_result(p.id, failure));
    }
    let status = if out.iter().any(|r| r["isValid"] == false) {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    Ok((status, Json(out)))
}
