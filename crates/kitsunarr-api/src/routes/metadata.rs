//! Catalogue search, adding titles to the library and refreshing their metadata.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::metadata as metadata_api;
use kitsunarr_domain::api::metadata::{
    AddTitle, CatalogueInfo, FolderPreviewQuery, LibraryRef, RefreshResult, SearchItem, SearchQuery,
};
use kitsunarr_domain::api::settings::MetadataSourcesSettings;
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
pub async fn providers(State(state): State<AppState>) -> ApiResult<Json<Vec<CatalogueInfo>>> {
    let settings = source_settings(&state).await?;
    let order = metadata_api::source_order(&settings);
    let mut list: Vec<CatalogueInfo> = Metadata::catalogues()
        .into_iter()
        .map(|mut c| {
            c.available = state.metadata.available(c.provider);
            c.enabled = !settings.disabled_sources.contains(&c.provider);
            c
        })
        .collect();
    // Tanukarr's Settings › Metadata Sources order drives every catalogue list.
    list.sort_by_key(|c| order.iter().position(|p| *p == c.provider));
    Ok(Json(list))
}

/// The catalogue settings (order, disabled catalogues, keys).
async fn source_settings(state: &AppState) -> ApiResult<MetadataSourcesSettings> {
    Ok(kitsunarr_db::settings::get_or_default(
        state.db.pool(),
        kitsunarr_db::settings::KEY_METADATA_SOURCES,
    )
    .await?)
}

/// Tanukarr's `MetadataSearchService.Search`: one enabled catalogue (the first enabled one
/// when none is named), or with `all_sources` every enabled catalogue that can be used now,
/// results merged in the configured order. A catalogue that fails while others answer is
/// skipped (logged); when every one fails the last failure is returned.
pub async fn search_catalogues(
    state: &AppState,
    provider: Option<MetadataProvider>,
    all_sources: bool,
    term: &str,
) -> ApiResult<Vec<kitsunarr_metadata::SearchResult>> {
    let settings = source_settings(state).await?;
    let enabled = metadata_api::enabled_sources(&settings);
    if !all_sources {
        let provider = match provider {
            Some(p) => p,
            None => *enabled
                .first()
                .ok_or_else(|| ApiError::field("provider", DISABLED_SOURCE))?,
        };
        if !enabled.contains(&provider) {
            return Err(ApiError::field("provider", DISABLED_SOURCE));
        }
        ensure_supported("provider", provider)?;
        return Ok(state.metadata.search(provider, term).await?);
    }
    let sources: Vec<MetadataProvider> = enabled
        .into_iter()
        .filter(|p| Metadata::supports(*p) && state.metadata.available(*p))
        .collect();
    if sources.is_empty() {
        return Err(ApiError::field(
            "provider",
            "No enabled metadata source can be searched; enable one or enter its API key in Settings › Metadata sources.",
        ));
    }
    let searches = sources.iter().map(|p| state.metadata.search(*p, term));
    let mut results = Vec::new();
    let mut last_failure = None;
    let mut succeeded = false;
    for (provider, outcome) in sources
        .iter()
        .zip(futures::future::join_all(searches).await)
    {
        match outcome {
            Ok(found) => {
                succeeded = true;
                results.extend(found);
            }
            Err(e) => {
                tracing::warn!(source = %provider, error = %e, "metadata search source is unavailable; continuing with enabled sources");
                last_failure = Some(e);
            }
        }
    }
    match last_failure {
        Some(e) if !succeeded => Err(e.into()),
        _ => Ok(results),
    }
}

const DISABLED_SOURCE: &str = "The selected metadata source is disabled or unknown.";

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
    let results = search_catalogues(&state, q.provider, q.all_sources, term).await?;
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
#[utoipa::path(post, path = "/library/add", tag = "metadata", request_body = AddTitle,
    responses(
        (status = 201, body = LibraryRef),
        (status = 404, body = ApiErrorBody, description = "Not found in the catalogue"),
        (status = 409, body = ApiErrorBody, description = "Already in the library"),
        (status = 422, body = ApiErrorBody),
        (status = 502, body = ApiErrorBody, description = "The catalogue could not be reached")
    ))]
pub async fn add_title(
    State(state): State<AppState>,
    Json(body): Json<AddTitle>,
) -> ApiResult<(StatusCode, Json<LibraryRef>)> {
    add_with(&state, body.title, body.automatic_grouping).await
}

/// [`add_title`] with Sonarr's request alone (automatic grouping on), for the v3 API and import
/// lists.
pub async fn add(
    State(state): State<AppState>,
    Json(req): Json<kitsunarr_domain::api::metadata::AddTitleRequest>,
) -> ApiResult<(StatusCode, Json<LibraryRef>)> {
    add_with(&state, req, true).await
}

async fn add_with(
    state: &AppState,
    req: kitsunarr_domain::api::metadata::AddTitleRequest,
    automatic_grouping: bool,
) -> ApiResult<(StatusCode, Json<LibraryRef>)> {
    let state = state.clone();
    if req.id.trim().is_empty() {
        return Err(ApiError::field("id", "Choose a title to add."));
    }
    ensure_supported("provider", req.provider)?;
    let options = kitsunarr_library::AddOptions { automatic_grouping };
    let added =
        kitsunarr_library::add_title_with(&state.db, &state.metadata, &req, options).await?;
    changed(&state, &added.media_type, added.id);
    let notification_title = match added.media_type.as_str() {
        "series" => kitsunarr_library::notify::series_ref(&state.db, added.id).await,
        "movie" => kitsunarr_library::notify::movie_refs(&state.db, added.id)
            .await
            .map(|(title, _)| title),
        _ => None,
    };
    if let Some(title) = notification_title {
        kitsunarr_library::notify::spawn(
            &state.db,
            &state.downloads.notifier,
            kitsunarr_notifications::Event::SeriesAdd(Box::new(title)),
        );
    }
    if req.search_for_missing && added.media_type == "series" {
        crate::routes::wanted::search_missing_of_series(&state, added.id, false);
    }
    // Tanukarr: a film added with "search for missing" is searched for at once.
    if req.search_for_missing && added.media_type == "movie" {
        let (state, id) = (state.clone(), added.id);
        tokio::spawn(async move {
            if crate::library_tools::paused(&state).await {
                return;
            }
            crate::routes::movie_search::server_search(&state, id).await;
        });
    }
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
    crate::routes::wanted::spawn_recently_aired_search(
        &state,
        id,
        result.recently_aired_episode_ids.clone(),
    );
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

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct OrderingQuery {
    pub provider: kitsunarr_domain::MetadataProvider,
    pub id: String,
}

/// The episode orderings a catalogue title offers before it is added (Tanukarr's ordering
/// choice in the add dialog); empty for catalogues with one order.
#[utoipa::path(get, path = "/metadata/orderings", tag = "metadata", params(OrderingQuery),
    responses((status = 200, body = Vec<kitsunarr_domain::api::library::EpisodeOrderingItem>), (status = 502, body = ApiErrorBody)))]
pub async fn catalogue_orderings(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<OrderingQuery>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library::EpisodeOrderingItem>>> {
    let orderings = state
        .metadata
        .orderings(q.provider, &q.id)
        .await
        .map_err(kitsunarr_library::LibraryError::from)?;
    Ok(Json(
        orderings
            .into_iter()
            .map(|o| kitsunarr_domain::api::library::EpisodeOrderingItem {
                current: o.id.is_empty(),
                id: o.id,
                name: o.name,
                description: o.description,
            })
            .collect(),
    ))
}

/// The episode orderings the series' catalogue offers (TMDB episode groups, TheTVDB season
/// types); empty for catalogues with one order.
#[utoipa::path(get, path = "/series/{id}/orderings", tag = "metadata", params(("id" = i64, Path)),
    responses((status = 200, body = Vec<kitsunarr_domain::api::library::EpisodeOrderingItem>), (status = 404, body = ApiErrorBody), (status = 502, body = ApiErrorBody)))]
pub async fn series_orderings(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::library::EpisodeOrderingItem>>> {
    let detail = kitsunarr_db::library::series::get_detail(&state.db, id).await?;
    let current = detail.episode_ordering.clone().unwrap_or_default();
    let orderings = state
        .metadata
        .orderings(detail.item.metadata_provider, &detail.metadata_id)
        .await
        .map_err(kitsunarr_library::LibraryError::from)?;
    Ok(Json(
        orderings
            .into_iter()
            .map(|o| kitsunarr_domain::api::library::EpisodeOrderingItem {
                // TheTVDB's default season type is stored as `official`.
                current: o.id == current || (o.id.is_empty() && current == "official"),
                id: o.id,
                name: o.name,
                description: o.description,
            })
            .collect(),
    ))
}

/// Switches the series to another episode ordering and refreshes it; episodes keep their files.
#[utoipa::path(put, path = "/series/{id}/ordering", tag = "metadata", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::library::EpisodeOrderingUpdate,
    responses((status = 200, body = RefreshResult), (status = 404, body = ApiErrorBody), (status = 422, body = ApiErrorBody), (status = 502, body = ApiErrorBody)))]
pub async fn set_series_ordering(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<kitsunarr_domain::api::library::EpisodeOrderingUpdate>,
) -> ApiResult<Json<RefreshResult>> {
    let result =
        kitsunarr_library::set_episode_ordering(&state.db, &state.metadata, id, body.ordering)
            .await?;
    changed(&state, "series", id);
    Ok(Json(result))
}

#[utoipa::path(post, path = "/series/{id}/metadata-source/preview", tag = "metadata", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::metadata::MetadataSourceRequest,
    responses((status = 200, body = kitsunarr_domain::api::metadata::MetadataSourcePreview), (status = 404, body = ApiErrorBody), (status = 422, body = ApiErrorBody), (status = 502, body = ApiErrorBody)))]
pub async fn preview_series_source(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<kitsunarr_domain::api::metadata::MetadataSourceRequest>,
) -> ApiResult<Json<kitsunarr_domain::api::metadata::MetadataSourcePreview>> {
    Ok(Json(
        kitsunarr_library::metadata_source::preview(&state.db, &state.metadata, false, id, &req)
            .await?,
    ))
}

#[utoipa::path(put, path = "/series/{id}/metadata-source", tag = "metadata", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::metadata::MetadataSourceRequest,
    responses((status = 200, body = RefreshResult), (status = 404, body = ApiErrorBody), (status = 422, body = ApiErrorBody), (status = 502, body = ApiErrorBody)))]
pub async fn set_series_source(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<kitsunarr_domain::api::metadata::MetadataSourceRequest>,
) -> ApiResult<Json<RefreshResult>> {
    let result =
        kitsunarr_library::metadata_source::apply(&state.db, &state.metadata, false, id, &req)
            .await?;
    changed(&state, "series", id);
    Ok(Json(result))
}

#[utoipa::path(post, path = "/movies/{id}/metadata-source/preview", tag = "metadata", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::metadata::MetadataSourceRequest,
    responses((status = 200, body = kitsunarr_domain::api::metadata::MetadataSourcePreview), (status = 404, body = ApiErrorBody), (status = 422, body = ApiErrorBody), (status = 502, body = ApiErrorBody)))]
pub async fn preview_movie_source(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<kitsunarr_domain::api::metadata::MetadataSourceRequest>,
) -> ApiResult<Json<kitsunarr_domain::api::metadata::MetadataSourcePreview>> {
    Ok(Json(
        kitsunarr_library::metadata_source::preview(&state.db, &state.metadata, true, id, &req)
            .await?,
    ))
}

#[utoipa::path(put, path = "/movies/{id}/metadata-source", tag = "metadata", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::metadata::MetadataSourceRequest,
    responses((status = 200, body = RefreshResult), (status = 404, body = ApiErrorBody), (status = 422, body = ApiErrorBody), (status = 502, body = ApiErrorBody)))]
pub async fn set_movie_source(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<kitsunarr_domain::api::metadata::MetadataSourceRequest>,
) -> ApiResult<Json<RefreshResult>> {
    let result =
        kitsunarr_library::metadata_source::apply(&state.db, &state.metadata, true, id, &req)
            .await?;
    changed(&state, "movie", id);
    Ok(Json(result))
}
