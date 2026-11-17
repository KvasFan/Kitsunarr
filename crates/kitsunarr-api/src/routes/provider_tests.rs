//! `POST /providers/{kind}/testall`: tests every enabled provider of a kind with its stored
//! settings (Sonarr's "Test All" button).

use axum::Json;
use axum::extract::{Path, State};
use kitsunarr_db::providers;
use kitsunarr_domain::api::settings::ProviderTestAllItem;

use crate::AppState;
use crate::error::ApiResult;

#[utoipa::path(post, path = "/providers/{kind}/testall", tag = "settings",
    params(("kind" = String, Path, description = "indexers | downloadclients | notifications | importlists")),
    responses((status = 200, body = Vec<ProviderTestAllItem>)))]
pub async fn test_all_providers(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> ApiResult<Json<Vec<ProviderTestAllItem>>> {
    let kind = crate::routes::settings::parse_kind(&kind)?;
    let mut out = Vec::new();
    for p in providers::list(state.db.pool(), kind).await? {
        if !p.enabled {
            continue;
        }
        let message = match crate::v3_ext::test_stored(&state, &p).await {
            Ok(m) => m,
            Err(e) => Some(e.message()),
        };
        out.push(ProviderTestAllItem {
            id: p.id,
            name: p.name.clone(),
            ok: message.is_none(),
            message,
        });
    }
    Ok(Json(out))
}
