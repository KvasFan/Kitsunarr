//! Import lists: test, sync now, status, membership preview, exclusions and options.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kitsunarr_db::import_lists as db_lists;
use kitsunarr_db::providers;
use kitsunarr_db::rules;
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::import_lists::{
    ImportListEntry, ImportListExclusionItem, ImportListStatusItem, ImportListSyncRequest,
    ImportListSyncResult,
};
use kitsunarr_domain::api::search::{DownloadClientTestResult, IndexerTestRequest};
use kitsunarr_domain::api::settings::{ImportListOptionsSettings, ProviderKind};
use serde::Deserialize;

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::import_lists as sync;

/// Reads a list with the editor's settings (stored secrets are reused) without storing it.
/// The lists an account holds, for an import list's include and exclude fields (Tanukarr's
/// `getCustomLists`): AniList custom lists, Trakt lists, Simkl custom lists.
#[utoipa::path(post, path = "/providers/importlists/lists", tag = "settings", request_body = IndexerTestRequest,
    responses((status = 200, body = Vec<kitsunarr_domain::api::settings::ProviderChoice>), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn import_list_choices(
    State(state): State<AppState>,
    Json(mut req): Json<IndexerTestRequest>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::settings::ProviderChoice>>> {
    if let Some(id) = req.id {
        let stored = providers::get(state.db.pool(), ProviderKind::ImportList, id).await?;
        crate::routes::settings::merge_secrets(&mut req.settings, &stored.settings);
    }
    let list =
        kitsunarr_importlists::choices::lists(&state.http, &req.implementation, &req.settings)
            .await
            .map_err(|e| ApiError::field("settings", e.to_string()))?;
    Ok(Json(
        list.into_iter()
            .map(|c| kitsunarr_domain::api::settings::ProviderChoice {
                value: c.value,
                name: c.name,
                hint: None,
            })
            .collect(),
    ))
}

/// Where MyAnimeList sends the browser back after signing in.
pub const SIGN_IN_CALLBACK: &str = "/api/v1/importlists/oauth/callback";

/// Starts signing in to a list service (Simkl, Trakt, Plex, MyAnimeList) with the editor's
/// settings; stored secrets are reused.
#[utoipa::path(post, path = "/providers/importlists/signin/start", tag = "settings",
    request_body = kitsunarr_domain::api::settings::SignInRequest,
    responses((status = 200, body = kitsunarr_domain::api::settings::SignInStarted), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn sign_in_start(
    State(state): State<AppState>,
    Json(mut req): Json<kitsunarr_domain::api::settings::SignInRequest>,
) -> ApiResult<Json<kitsunarr_domain::api::settings::SignInStarted>> {
    if !kitsunarr_importlists::oauth::supported(&req.implementation) {
        return Err(ApiError::field(
            "implementation",
            "This list has no sign-in; enter its token.",
        ));
    }
    if let Some(id) = req.id {
        let stored = providers::get(state.db.pool(), ProviderKind::ImportList, id).await?;
        crate::routes::settings::merge_secrets(&mut req.settings, &stored.settings);
    }
    let callback = req
        .callback
        .clone()
        .filter(|c| {
            (c.starts_with("http://") || c.starts_with("https://")) && c.ends_with(SIGN_IN_CALLBACK)
        })
        .unwrap_or_default();
    if req.implementation == "MyAnimeListImport" && callback.is_empty() {
        return Err(ApiError::field(
            "callback",
            "MyAnimeList needs Kitsunarr's callback address.",
        ));
    }
    let s = kitsunarr_importlists::oauth::start(
        &state.http,
        &req.implementation,
        &req.settings,
        &callback,
    )
    .await
    .map_err(|e| ApiError::field("settings", e.to_string()))?;
    Ok(Json(kitsunarr_domain::api::settings::SignInStarted {
        url: s.url,
        code: s.code,
        state: s.state,
        interval: s.interval,
        expires_in: s.expires_in,
    }))
}

/// Checks a sign-in: when the user has finished, the settings to merge into the list.
#[utoipa::path(post, path = "/providers/importlists/signin/poll", tag = "settings",
    request_body = kitsunarr_domain::api::settings::SignInRequest,
    responses((status = 200, body = kitsunarr_domain::api::settings::SignInPolled), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn sign_in_poll(
    State(state): State<AppState>,
    Json(mut req): Json<kitsunarr_domain::api::settings::SignInRequest>,
) -> ApiResult<Json<kitsunarr_domain::api::settings::SignInPolled>> {
    let sign_in = req
        .state
        .clone()
        .ok_or_else(|| ApiError::field("state", "Start signing in first."))?;
    if let Some(id) = req.id {
        let stored = providers::get(state.db.pool(), ProviderKind::ImportList, id).await?;
        crate::routes::settings::merge_secrets(&mut req.settings, &stored.settings);
    }
    let code = kitsunarr_importlists::oauth::nonce_of(&sign_in).and_then(|n| {
        state
            .oauth_codes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&n)
    });
    let polled = kitsunarr_importlists::oauth::poll(
        &state.http,
        &req.implementation,
        &req.settings,
        &sign_in,
        code.as_deref(),
    )
    .await
    .map_err(|e| ApiError::field("settings", e.to_string()))?;
    Ok(Json(match polled {
        kitsunarr_importlists::oauth::Polled::Pending => {
            kitsunarr_domain::api::settings::SignInPolled {
                done: false,
                settings: Default::default(),
            }
        }
        kitsunarr_importlists::oauth::Polled::Done(settings) => {
            kitsunarr_domain::api::settings::SignInPolled {
                done: true,
                settings,
            }
        }
    }))
}

#[derive(Debug, Deserialize, Default, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(default)]
pub struct SignInCallback {
    pub code: String,
    pub state: String,
}

/// MyAnimeList's redirect after signing in: the code waits for the editor's next poll.
#[utoipa::path(get, path = "/importlists/oauth/callback", tag = "settings", params(SignInCallback),
    responses((status = 200, description = "A page saying the window can be closed")))]
pub async fn sign_in_callback(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<SignInCallback>,
) -> axum::response::Html<&'static str> {
    if !q.code.is_empty() && !q.state.is_empty() && q.state.len() <= 64 && q.code.len() <= 4096 {
        let mut codes = state.oauth_codes.lock().unwrap_or_else(|e| e.into_inner());
        // Unclaimed codes are few; old ones are dropped.
        if codes.len() >= 100 {
            codes.clear();
        }
        codes.insert(q.state, q.code);
    }
    axum::response::Html(
        "<!doctype html><meta charset=utf-8><title>Kitsunarr</title><p>Signed in. You can close this window and return to Kitsunarr.</p>",
    )
}

#[utoipa::path(post, path = "/providers/importlists/test", tag = "settings", request_body = IndexerTestRequest,
    responses((status = 200, body = DownloadClientTestResult), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn test_import_list(
    State(state): State<AppState>,
    Json(mut req): Json<IndexerTestRequest>,
) -> ApiResult<Json<DownloadClientTestResult>> {
    if !kitsunarr_importlists::is_supported(&req.implementation) {
        return Err(ApiError::field(
            "implementation",
            format!("{} lists are not supported yet.", req.implementation),
        ));
    }
    if let Some(id) = req.id {
        let stored = providers::get(state.db.pool(), ProviderKind::ImportList, id).await?;
        crate::routes::settings::merge_secrets(&mut req.settings, &stored.settings);
    }
    let read = async {
        let settings =
            match kitsunarr_importlists::renew(&state.http, &req.implementation, &req.settings)
                .await?
            {
                Some(renewed) => renewed,
                None => req.settings.clone(),
            };
        kitsunarr_importlists::fetch(&state.http, &req.implementation, &settings).await
    };
    Ok(Json(match read.await {
        Ok(items) if items.is_empty() => DownloadClientTestResult {
            ok: true,
            message: "Connected, but the list contains no shows with the current filters.".into(),
            warnings: vec!["The list is empty.".into()],
        },
        Ok(items) => DownloadClientTestResult {
            ok: true,
            message: format!("Read {} shows.", items.len()),
            warnings: Vec::new(),
        },
        Err(e) => DownloadClientTestResult {
            ok: false,
            message: e.to_string(),
            warnings: Vec::new(),
        },
    }))
}

/// Syncs every list, or one, now (reading them even when not due).
#[utoipa::path(post, path = "/importlists/sync", tag = "importlists", request_body = ImportListSyncRequest,
    responses((status = 200, body = ImportListSyncResult)))]
pub async fn sync_now(
    State(state): State<AppState>,
    Json(req): Json<ImportListSyncRequest>,
) -> ApiResult<Json<ImportListSyncResult>> {
    if let Some(id) = req.import_list_id {
        providers::get(state.db.pool(), ProviderKind::ImportList, id).await?;
    }
    let message = sync::sync(&state, req.import_list_id, true)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ImportListSyncResult { message }))
}

#[utoipa::path(get, path = "/importlists/status", tag = "importlists",
    responses((status = 200, body = Vec<ImportListStatusItem>)))]
pub async fn status(State(state): State<AppState>) -> ApiResult<Json<Vec<ImportListStatusItem>>> {
    let items = db_lists::items(state.db.pool(), None).await?;
    let lists = providers::list(state.db.pool(), ProviderKind::ImportList).await?;
    let mut out = Vec::new();
    for l in lists {
        let s = db_lists::status(state.db.pool(), l.id).await?;
        out.push(ImportListStatusItem {
            import_list_id: l.id,
            last_info_sync: s.last_info_sync,
            disabled_till: s.disabled_till,
            last_error: s.last_error,
            item_count: items.iter().filter(|i| i.import_list_id == l.id).count() as i64,
        });
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct EntriesQuery {
    pub import_list_id: Option<i64>,
}

/// The shows the lists contained at their last complete read, matched with the library.
#[utoipa::path(get, path = "/importlists/items", tag = "importlists", params(EntriesQuery),
    responses((status = 200, body = Vec<ImportListEntry>)))]
pub async fn entries(
    State(state): State<AppState>,
    Query(q): Query<EntriesQuery>,
) -> ApiResult<Json<Vec<ImportListEntry>>> {
    let rows = db_lists::items(state.db.pool(), q.import_list_id).await?;
    let library = sync::library_series(&state)
        .await
        .map_err(ApiError::internal)?;
    let exclusions = rules::list_exclusions(state.db.pool()).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let item = sync::from_row(&r);
                let excluded = exclusions.iter().any(|e| {
                    let id = match e.provider.as_str() {
                        "tvdb" => r.tvdb_id,
                        "tmdb" => r.tmdb_id,
                        "aniList" => r.anilist_id,
                        "myAnimeList" => r.mal_id,
                        _ => None,
                    };
                    id.is_some_and(|id| id.to_string() == e.external_id)
                });
                ImportListEntry {
                    import_list_id: r.import_list_id,
                    series_id: library.iter().find(|s| s.matches(&item)).map(|s| s.id),
                    title: r.title,
                    year: r.year,
                    tvdb_id: r.tvdb_id,
                    tmdb_id: r.tmdb_id,
                    imdb_id: r.imdb_id,
                    mal_id: r.mal_id,
                    ani_list_id: r.anilist_id,
                    excluded,
                }
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/importlists/exclusions", tag = "importlists",
    responses((status = 200, body = Vec<ImportListExclusionItem>)))]
pub async fn list_exclusions(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ImportListExclusionItem>>> {
    Ok(Json(
        rules::list_exclusions(state.db.pool())
            .await?
            .into_iter()
            .map(|e| ImportListExclusionItem {
                id: e.id,
                provider: e.provider,
                external_id: e.external_id,
                title: e.title,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/importlists/exclusions", tag = "importlists", request_body = ImportListExclusionItem,
    responses((status = 201, body = ImportListExclusionItem), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn create_exclusion(
    State(state): State<AppState>,
    Json(mut body): Json<ImportListExclusionItem>,
) -> ApiResult<(StatusCode, Json<ImportListExclusionItem>)> {
    if !matches!(
        body.provider.as_str(),
        "tvdb" | "tmdb" | "aniList" | "myAnimeList"
    ) {
        return Err(ApiError::field(
            "provider",
            "Choose TheTVDB, TMDB, AniList or MyAnimeList.",
        ));
    }
    body.external_id = body.external_id.trim().to_string();
    if body.external_id.is_empty() || !body.external_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(ApiError::field(
            "externalId",
            "Enter the numeric catalogue id.",
        ));
    }
    let mut conn = state
        .db
        .pool()
        .acquire()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    body.id = rules::upsert_exclusion(
        &mut conn,
        &body.provider,
        &body.external_id,
        body.title.as_deref().filter(|t| !t.trim().is_empty()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(body)))
}

#[utoipa::path(delete, path = "/importlists/exclusions/{id}", tag = "importlists", params(("id" = i64, Path)),
    responses((status = 204), (status = 404, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn delete_exclusion(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    rules::delete_exclusion(state.db.pool(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/settings/importlistoptions", tag = "settings",
    responses((status = 200, body = ImportListOptionsSettings)))]
pub async fn get_options(
    State(state): State<AppState>,
) -> ApiResult<Json<ImportListOptionsSettings>> {
    Ok(Json(
        kv::get_or_default(state.db.pool(), kv::KEY_IMPORT_LIST_OPTIONS).await?,
    ))
}

#[utoipa::path(put, path = "/settings/importlistoptions", tag = "settings",
    request_body = ImportListOptionsSettings, responses((status = 200, body = ImportListOptionsSettings)))]
pub async fn put_options(
    State(state): State<AppState>,
    Json(body): Json<ImportListOptionsSettings>,
) -> ApiResult<Json<ImportListOptionsSettings>> {
    if body.list_sync_level == kitsunarr_domain::api::settings::ListSyncLevel::KeepAndTag
        && body.list_sync_tag.is_none()
    {
        return Err(ApiError::field(
            "listSyncTag",
            "Choose the tag to add to titles no list contains.",
        ));
    }
    kv::set(state.db.pool(), kv::KEY_IMPORT_LIST_OPTIONS, &body).await?;
    crate::routes::settings::settings_changed(&state, "importListOptions");
    Ok(Json(body))
}

/// Adds chosen shows of a list's last read (1 to 25), with a root folder and profile.
#[utoipa::path(post, path = "/importlists/{id}/add", tag = "importlists", params(("id" = i64, Path)),
    request_body = kitsunarr_domain::api::import_lists::ImportListAddRequest,
    responses((status = 200, body = Vec<kitsunarr_domain::api::import_lists::ImportListAddOutcome>), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn add_items(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<kitsunarr_domain::api::import_lists::ImportListAddRequest>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::import_lists::ImportListAddOutcome>>> {
    Ok(Json(sync::add_selected(&state, id, &req).await?))
}
