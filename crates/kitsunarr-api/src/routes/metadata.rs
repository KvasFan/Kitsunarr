//! Catalogue search, adding titles to the library and refreshing their metadata.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::metadata::{
    AddTitleRequest, CatalogueInfo, FolderPreviewQuery, LibraryRef, RefreshResult, SearchItem,
    SearchQuery,
};
use kitsunarr_metadata::Metadata;

use crate::AppState;
use crate::error::{ApiError, ApiResult};

const MAX_TERM_CHARS: usize = 200;

fn ensure_supported(field: &str, provider: MetadataProvider) -> ApiResult<()> {
    if Metadata::supports(provider) {
        Ok(())
    } else {
        Err(ApiError::field(
            field,
            format!("{} is not supported yet.", provider.display_name()),
        ))
    }
}

fn changed(state: &AppState, media_type: &str, id: i64) {
    state.events.publish(ServerEvent::LibraryChanged {
        media_type: media_type.into(),
        ids: vec![id],
        deleted: false,
    });
}

/// Catalogues available for search, with the attribution their terms require.
#[utoipa::path(get, path = "/metadata/providers", tag = "metadata",
    responses((status = 200, body = Vec<CatalogueInfo>)))]
pub async fn providers() -> Json<Vec<CatalogueInfo>> {
    Json(Metadata::catalogues())
}

/// Searches a catalogue. Each result says whether it is already in the library (matched by
/// catalogue id or any shared external id).
#[utoipa::path(get, path = "/metadata/search", tag = "metadata", params(SearchQuery),
    responses(
        (status = 200, body = Vec<SearchItem>),
        (status = 422, body = ApiErrorBody),
        (status = 502, body = ApiErrorBody, description = "The catalogue could not be reached")
    ))]
pub async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<Vec<SearchItem>>> {
    let term = q.term.trim();
    if term.is_empty() {
        return Err(ApiError::field("term", "Enter a title to search for."));
    }
    if term.chars().count() > MAX_TERM_CHARS {
        return Err(ApiError::field("term", "The search term is too long."));
    }
    ensure_supported("provider", q.provider)?;
    let results = state.metadata.search(q.provider, term).await?;
    let mut items = Vec::with_capacity(results.len());
    for r in results {
        let in_library =
            kitsunarr_library::find_existing(&state.db, r.provider, &r.id, &r.external_ids).await?;
        items.push(SearchItem {
            provider: r.provider,
            id: r.id,
            kind: r.kind,
            title: r.title,
            year: r.year,
            overview: r.overview,
            network: r.network,
            status: r.status,
            poster_url: r.poster_url,
            external_ids: r.external_ids,
            in_library,
        });
    }
    Ok(Json(items))
}

/// The folder name a title would get under a root folder, from the naming settings.
#[utoipa::path(get, path = "/library/folder-preview", tag = "metadata", params(FolderPreviewQuery),
    responses((status = 200, body = String)))]
pub async fn folder_preview(
    State(state): State<AppState>,
    Query(q): Query<FolderPreviewQuery>,
) -> ApiResult<Json<String>> {
    Ok(Json(
        kitsunarr_library::folder_preview(&state.db, &q.title, q.year, q.movie).await?,
    ))
}

/// Adds a catalogue entry to the library. Series and anime get their seasons and episodes;
/// films become movies. Nothing is created on disk: the folder is made when files arrive.
#[utoipa::path(post, path = "/library/add", tag = "metadata", request_body = AddTitleRequest,
    responses(
        (status = 201, body = LibraryRef),
        (status = 404, body = ApiErrorBody, description = "Not found in the catalogue"),
        (status = 409, body = ApiErrorBody, description = "Already in the library"),
        (status = 422, body = ApiErrorBody),
        (status = 502, body = ApiErrorBody, description = "The catalogue could not be reached")
    ))]
pub async fn add(
    State(state): State<AppState>,
    Json(req): Json<AddTitleRequest>,
) -> ApiResult<(StatusCode, Json<LibraryRef>)> {
    if req.id.trim().is_empty() {
        return Err(ApiError::field("id", "Choose a title to add."));
    }
    ensure_supported("provider", req.provider)?;
    let added = kitsunarr_library::add_title(&state.db, &state.metadata, &req).await?;
    changed(&state, &added.media_type, added.id);
    Ok((StatusCode::CREATED, Json(added)))
}

/// Refreshes a series from its catalogue. Monitoring, files and settings are kept; new
/// episodes follow the series' "monitor new items" choice.
#[utoipa::path(post, path = "/series/{id}/refresh", tag = "metadata", params(("id" = i64, Path)),
    responses(
        (status = 200, body = RefreshResult),
        (status = 400, body = ApiErrorBody, description = "The owning catalogue is not supported yet"),
        (status = 404, body = ApiErrorBody),
        (status = 502, body = ApiErrorBody)
    ))]
pub async fn refresh_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<RefreshResult>> {
    let result = kitsunarr_library::refresh_series(&state.db, &state.metadata, id).await?;
    changed(&state, "series", id);
    Ok(Json(result))
}

/// Refreshes a movie from its catalogue.
#[utoipa::path(post, path = "/movies/{id}/refresh", tag = "metadata", params(("id" = i64, Path)),
    responses(
        (status = 204, description = "Refreshed"),
        (status = 400, body = ApiErrorBody, description = "The owning catalogue is not supported yet"),
        (status = 404, body = ApiErrorBody),
        (status = 502, body = ApiErrorBody)
    ))]
pub async fn refresh_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    kitsunarr_library::refresh_movie(&state.db, &state.metadata, id).await?;
    changed(&state, "movie", id);
    Ok(StatusCode::NO_CONTENT)
}
