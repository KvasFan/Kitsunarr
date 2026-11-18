//! The rest of Sonarr's `/api/v3` that dashboards, mobile clients and sync tools use: tags
//! (`tag/{id}`, `tag/detail`), root folders, delay profiles, remote path mappings, import list
//! exclusions, the queue's status and details and "send now" for pending releases
//! (`queue/status`, `queue/details`, `queue/grab`), scheduled tasks, backups, logs and the
//! rename preview. Every write goes through the native handler (validation, events).

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use kitsunarr_db::rules;
use kitsunarr_domain::api::import_lists::ImportListExclusionItem;
use kitsunarr_domain::api::settings::{DelayProfile, RemotePathMapping, RootFolderCreate};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::routes::{import_lists, rules as rule_routes, settings, system};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tag/detail", get(tag_details))
        .route("/tag/detail/{id}", get(tag_detail))
        .route("/tag/{id}", get(get_tag).put(update_tag).delete(delete_tag))
        .route("/rootfolder", post(create_root_folder))
        .route(
            "/rootfolder/{id}",
            get(get_root_folder).delete(delete_root_folder),
        )
        .route("/delayprofile", get(list_delay).post(create_delay))
        .route(
            "/delayprofile/{id}",
            get(get_delay).put(update_delay).delete(delete_delay),
        )
        .route(
            "/remotepathmapping",
            get(list_mappings).post(create_mapping),
        )
        .route(
            "/remotepathmapping/{id}",
            get(get_mapping).put(update_mapping).delete(delete_mapping),
        )
        .route(
            "/importlistexclusion",
            get(list_exclusions).post(create_exclusion),
        )
        .route("/importlistexclusion/paged", get(paged_exclusions))
        .route("/importlistexclusion/bulk", delete(delete_exclusions))
        .route(
            "/importlistexclusion/{id}",
            get(get_exclusion)
                .put(update_exclusion)
                .delete(delete_exclusion),
        )
        .route("/queue/status", get(queue_status))
        .route("/queue/details", get(queue_details))
        .route("/queue/grab/bulk", post(grab_queue_items))
        .route("/queue/grab/{id}", post(grab_queue_item))
        .route("/system/task", get(list_tasks))
        .route("/system/task/{id}", get(get_task))
        .route("/system/backup", get(list_backups))
        .route("/log", get(logs))
        .route("/rename", get(rename_preview))
        .route("/manualimport", get(manual_import_scan))
}

pub(crate) fn ids(v: &[i64]) -> Value {
    json!(v)
}

// ---- tags -----------------------------------------------------------------------------------

pub(crate) async fn tag_label(state: &AppState, id: i64) -> ApiResult<String> {
    kitsunarr_db::tags::list(state.db.pool())
        .await?
        .into_iter()
        .find(|t| t.id == id)
        .map(|t| t.label)
        .ok_or_else(|| ApiError::NotFound("The tag does not exist.".into()))
}

pub(crate) async fn get_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({"id": id, "label": tag_label(&state, id).await?}),
    ))
}

#[derive(Deserialize)]
pub(crate) struct TagBody {
    label: String,
}

pub(crate) async fn update_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<TagBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    kitsunarr_db::tags::rename(state.db.pool(), id, &body.label)
        .await
        .map_err(|e| match e {
            kitsunarr_db::DbError::Invalid(m) => ApiError::field("label", m),
            other => other.into(),
        })?;
    settings::settings_changed(&state, "tags");
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"id": id, "label": tag_label(&state, id).await?})),
    ))
}

pub(crate) async fn delete_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    settings::delete_tag(State(state), Path(id)).await?;
    Ok(StatusCode::OK)
}

/// Sonarr's `TagDetailsResource` (release profiles are its "restrictions"); films are
/// Kitsunarr's `movieIds`.
pub(crate) async fn detail_json(state: &AppState, id: i64, label: String) -> ApiResult<Value> {
    let u = kitsunarr_db::tags::usage(&state.db, id).await?;
    Ok(json!({
        "id": id,
        "label": label,
        "delayProfileIds": ids(&u.delay_profiles),
        "importListIds": ids(&u.import_lists),
        "notificationIds": ids(&u.notifications),
        "restrictionIds": ids(&u.release_profiles),
        "releaseProfileIds": ids(&u.release_profiles),
        "excludedReleaseProfileIds": ids(&u.excluded_release_profiles),
        "indexerIds": ids(&u.indexers),
        "downloadClientIds": ids(&u.download_clients),
        "autoTagIds": ids(&u.auto_tags),
        "seriesIds": ids(&u.series),
        "movieIds": ids(&u.movies),
    }))
}

pub(crate) async fn tag_details(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let mut out = Vec::new();
    for t in kitsunarr_db::tags::list(state.db.pool()).await? {
        out.push(detail_json(&state, t.id, t.label).await?);
    }
    Ok(Json(out))
}

pub(crate) async fn tag_detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let label = tag_label(&state, id).await?;
    Ok(Json(detail_json(&state, id, label).await?))
}

// ---- root folders ---------------------------------------------------------------------------

pub(crate) async fn root_folder(state: &AppState, id: i64) -> ApiResult<Value> {
    let list = settings::list_root_folders(State(state.clone())).await?.0;
    let r = list
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::NotFound("The root folder does not exist.".into()))?;
    Ok(crate::v3::root_folder_json(state, r).await)
}

pub(crate) async fn get_root_folder(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(root_folder(&state, id).await?))
}

/// Sonarr's `POST /rootfolder` (`{path}`); Kitsunarr's optional `mediaKind` chooses the library
/// section (series by default).
pub(crate) async fn create_root_folder(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let path = body["path"]
        .as_str()
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ApiError::field("path", "A root folder needs a path."))?
        .to_string();
    let media_kind = match body.get("mediaKind").filter(|v| !v.is_null()) {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|_| ApiError::field("mediaKind", "Use series, anime or movie."))?,
        None => kitsunarr_domain::media::MediaKind::Series,
    };
    let (_, Json(item)) = settings::create_root_folder(
        State(state.clone()),
        Json(RootFolderCreate {
            path,
            media_kind: Some(media_kind),
        }),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(crate::v3::root_folder_json(&state, &item).await),
    ))
}

pub(crate) async fn delete_root_folder(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    settings::delete_root_folder(State(state), Path(id)).await?;
    Ok(StatusCode::OK)
}

// ---- delay profiles -------------------------------------------------------------------------

/// Sonarr's `DelayProfileResource`: Kitsunarr's profile with `order` for its sort order.
pub(crate) fn delay_json(d: &DelayProfile) -> Value {
    json!({
        "id": d.id,
        "enableUsenet": d.enable_usenet,
        "enableTorrent": d.enable_torrent,
        "preferredProtocol": d.preferred_protocol,
        "usenetDelay": d.usenet_delay,
        "torrentDelay": d.torrent_delay,
        "bypassIfHighestQuality": d.bypass_if_highest_quality,
        "bypassIfAboveCustomFormatScore": d.bypass_if_above_custom_format_score,
        "minimumCustomFormatScore": d.minimum_custom_format_score,
        "order": d.sort_order,
        "tags": d.tags,
    })
}

/// A delay profile from Sonarr's resource, over `base` for the keys it leaves out.
pub(crate) fn delay_from(v: &Value, base: DelayProfile) -> ApiResult<DelayProfile> {
    let b = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
    let i = |k: &str, d: i64| v.get(k).and_then(Value::as_i64).unwrap_or(d);
    let protocol = match v.get("preferredProtocol") {
        Some(Value::String(s)) => s.to_ascii_lowercase(),
        // Sonarr's `DownloadProtocol`: 1 usenet, 2 torrent.
        Some(Value::Number(n)) if n.as_i64() == Some(2) => "torrent".into(),
        Some(Value::Number(_)) => "usenet".into(),
        _ => base.preferred_protocol.clone(),
    };
    if protocol != "usenet" && protocol != "torrent" {
        return Err(ApiError::field(
            "preferredProtocol",
            "Use usenet or torrent.",
        ));
    }
    Ok(DelayProfile {
        id: base.id,
        enable_usenet: b("enableUsenet", base.enable_usenet),
        enable_torrent: b("enableTorrent", base.enable_torrent),
        preferred_protocol: protocol,
        usenet_delay: i("usenetDelay", base.usenet_delay),
        torrent_delay: i("torrentDelay", base.torrent_delay),
        bypass_if_highest_quality: b("bypassIfHighestQuality", base.bypass_if_highest_quality),
        bypass_if_above_custom_format_score: b(
            "bypassIfAboveCustomFormatScore",
            base.bypass_if_above_custom_format_score,
        ),
        minimum_custom_format_score: i(
            "minimumCustomFormatScore",
            base.minimum_custom_format_score,
        ),
        sort_order: i("order", base.sort_order),
        tags: v
            .get("tags")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or(base.tags),
    })
}

pub(crate) async fn delay_profile(state: &AppState, id: i64) -> ApiResult<DelayProfile> {
    rules::list_delay_profiles(state.db.pool())
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| ApiError::NotFound("The delay profile does not exist.".into()))
}

pub(crate) async fn list_delay(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        rules::list_delay_profiles(state.db.pool())
            .await?
            .iter()
            .map(delay_json)
            .collect(),
    ))
}

pub(crate) async fn get_delay(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(delay_json(&delay_profile(&state, id).await?)))
}

pub(crate) async fn create_delay(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let base = DelayProfile {
        id: 0,
        enable_usenet: true,
        enable_torrent: true,
        preferred_protocol: "usenet".into(),
        usenet_delay: 0,
        torrent_delay: 0,
        bypass_if_highest_quality: true,
        bypass_if_above_custom_format_score: false,
        minimum_custom_format_score: 0,
        // Placed before the default profile by the native handler.
        sort_order: 0,
        tags: Vec::new(),
    };
    let mut d = delay_from(&v, base)?;
    // Sonarr's `order` for a new profile is decided by the server.
    d.sort_order = 0;
    let (_, Json(saved)) = rule_routes::create_delay(State(state), Json(d)).await?;
    Ok((StatusCode::CREATED, Json(delay_json(&saved))))
}

pub(crate) async fn update_delay(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let d = delay_from(&v, delay_profile(&state, id).await?)?;
    let saved = rule_routes::update_delay(State(state), Path(id), Json(d))
        .await?
        .0;
    Ok((StatusCode::ACCEPTED, Json(delay_json(&saved))))
}

pub(crate) async fn delete_delay(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    delay_profile(&state, id).await?;
    rule_routes::delete_delay(State(state), Path(id)).await?;
    Ok(StatusCode::OK)
}

// ---- remote path mappings -------------------------------------------------------------------

pub(crate) fn mapping_from(v: &Value, id: i64) -> ApiResult<RemotePathMapping> {
    let text = |k: &'static str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| ApiError::field(k, "This field is required."))
    };
    Ok(RemotePathMapping {
        id,
        host: text("host")?,
        remote_path: text("remotePath")?,
        local_path: text("localPath")?,
    })
}

pub(crate) async fn mapping(state: &AppState, id: i64) -> ApiResult<RemotePathMapping> {
    rules::list_remote_path_mappings(state.db.pool())
        .await?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| ApiError::NotFound("The remote path mapping does not exist.".into()))
}

pub(crate) async fn list_mappings(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<RemotePathMapping>>> {
    rule_routes::list_mappings(State(state)).await
}

pub(crate) async fn get_mapping(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<RemotePathMapping>> {
    Ok(Json(mapping(&state, id).await?))
}

pub(crate) async fn create_mapping(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<RemotePathMapping>)> {
    rule_routes::create_mapping(State(state), Json(mapping_from(&v, 0)?)).await
}

pub(crate) async fn update_mapping(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<RemotePathMapping>)> {
    mapping(&state, id).await?;
    let saved = rule_routes::update_mapping(State(state), Path(id), Json(mapping_from(&v, id)?))
        .await?
        .0;
    Ok((StatusCode::ACCEPTED, Json(saved)))
}

pub(crate) async fn delete_mapping(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    mapping(&state, id).await?;
    rule_routes::delete_mapping(State(state), Path(id)).await?;
    Ok(StatusCode::OK)
}

// ---- import list exclusions -----------------------------------------------------------------

/// Sonarr's `ImportListExclusionResource` (`tvdbId`, 0 for other catalogues), with
/// Kitsunarr's catalogue and id.
pub(crate) fn exclusion_json(e: &ImportListExclusionItem) -> Value {
    json!({
        "id": e.id,
        "tvdbId": if e.provider == "tvdb" { e.external_id.parse::<i64>().unwrap_or(0) } else { 0 },
        "title": e.title.clone().unwrap_or_default(),
        "provider": e.provider,
        "externalId": e.external_id,
    })
}

/// An exclusion from Sonarr's resource (`tvdbId`) or Kitsunarr's (`provider`, `externalId`).
pub(crate) fn exclusion_from(v: &Value, id: i64) -> ApiResult<ImportListExclusionItem> {
    let (provider, external_id) = match (
        v.get("provider").and_then(Value::as_str),
        v.get("externalId"),
    ) {
        (Some(p), Some(e)) if !e.is_null() => (
            p.to_string(),
            e.as_str().map_or_else(|| e.to_string(), str::to_string),
        ),
        _ => {
            let tvdb = v
                .get("tvdbId")
                .and_then(Value::as_i64)
                .filter(|t| *t > 0)
                .ok_or_else(|| ApiError::field("tvdbId", "Enter the series' TheTVDB id."))?;
            ("tvdb".into(), tvdb.to_string())
        }
    };
    Ok(ImportListExclusionItem {
        id,
        provider,
        external_id,
        title: v
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string),
    })
}

pub(crate) async fn exclusions(state: &AppState) -> ApiResult<Vec<ImportListExclusionItem>> {
    Ok(import_lists::list_exclusions(State(state.clone())).await?.0)
}

pub(crate) async fn exclusion(state: &AppState, id: i64) -> ApiResult<ImportListExclusionItem> {
    exclusions(state)
        .await?
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| ApiError::NotFound("The exclusion does not exist.".into()))
}

pub(crate) async fn list_exclusions(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        exclusions(&state)
            .await?
            .iter()
            .map(exclusion_json)
            .collect(),
    ))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct PageQuery {
    page: Option<i64>,
    page_size: Option<i64>,
}

pub(crate) fn paged(records: Vec<Value>, q: &PageQuery, sort_key: &str, direction: &str) -> Value {
    let total = records.len() as i64;
    let page = q.page.unwrap_or(1).max(1);
    let size = q.page_size.unwrap_or(10).clamp(1, 1000);
    let records: Vec<Value> = records
        .into_iter()
        .skip(((page - 1) * size) as usize)
        .take(size as usize)
        .collect();
    json!({"page": page, "pageSize": size, "sortKey": sort_key, "sortDirection": direction, "totalRecords": total, "records": records})
}

pub(crate) async fn paged_exclusions(
    State(state): State<AppState>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    let records = exclusions(&state)
        .await?
        .iter()
        .map(exclusion_json)
        .collect();
    Ok(Json(paged(records, &q, "title", "ascending")))
}

pub(crate) async fn get_exclusion(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(exclusion_json(&exclusion(&state, id).await?)))
}

pub(crate) async fn create_exclusion(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (_, Json(saved)) =
        import_lists::create_exclusion(State(state), Json(exclusion_from(&v, 0)?)).await?;
    Ok((StatusCode::CREATED, Json(exclusion_json(&saved))))
}

pub(crate) async fn update_exclusion(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    exclusion(&state, id).await?;
    let e = exclusion_from(&v, id)?;
    if !matches!(
        e.provider.as_str(),
        "tvdb" | "tmdb" | "aniList" | "myAnimeList"
    ) || e.external_id.is_empty()
        || !e.external_id.chars().all(|c| c.is_ascii_digit())
    {
        return Err(ApiError::field(
            "externalId",
            "Use a numeric TheTVDB, TMDB, AniList or MyAnimeList id.",
        ));
    }
    rules::update_exclusion(
        state.db.pool(),
        &rules::ImportListExclusion {
            id,
            provider: e.provider,
            external_id: e.external_id,
            title: e.title,
        },
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(exclusion_json(&exclusion(&state, id).await?)),
    ))
}

pub(crate) async fn delete_exclusion(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    import_lists::delete_exclusion(State(state), Path(id)).await?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub(crate) struct BulkIds {
    ids: Vec<i64>,
}

pub(crate) async fn delete_exclusions(
    State(state): State<AppState>,
    Json(b): Json<BulkIds>,
) -> ApiResult<StatusCode> {
    for id in b.ids {
        match rules::delete_exclusion(state.db.pool(), id).await {
            Ok(()) | Err(kitsunarr_db::DbError::NotFound) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(StatusCode::OK)
}

// ---- queue ----------------------------------------------------------------------------------

/// Sonarr's `QueueStatusResource`: downloads (not episode records) and their problems, those
/// of downloads no library title claims counted apart.
pub(crate) async fn queue_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let native = crate::routes::queue::get_queue(State(state)).await?.0;
    let known = |i: &&kitsunarr_domain::api::queue::QueueItem| {
        i.series_id.is_some() || i.movie_id.is_some()
    };
    let error = |i: &&kitsunarr_domain::api::queue::QueueItem| {
        i.status == "failed" || i.tracked_state == "failed"
    };
    let warning = |i: &&kitsunarr_domain::api::queue::QueueItem| {
        !error(i)
            && (i.status == "warning"
                || i.tracked_state == "importBlocked"
                || !i.messages.is_empty() && i.tracked_state != "pending")
    };
    let items = &native.items;
    let count = items.iter().filter(known).count();
    Ok(Json(json!({
        "id": 1,
        "totalCount": items.len(),
        "count": count,
        "unknownCount": items.len() - count,
        "errors": items.iter().filter(known).any(|i| error(&i)),
        "warnings": items.iter().filter(known).any(|i| warning(&i)),
        "unknownErrors": items.iter().filter(|i| !known(i)).any(|i| error(&i)),
        "unknownWarnings": items.iter().filter(|i| !known(i)).any(|i| warning(&i)),
    })))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct DetailsQuery {
    series_id: Option<i64>,
    /// Comma-separated, or repeated.
    episode_ids: Option<String>,
}

/// Sonarr's `GET /queue/details`: every record, unpaged, of a series or of episodes.
pub(crate) async fn queue_details(
    State(state): State<AppState>,
    Query(q): Query<DetailsQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let episodes: Vec<i64> = q
        .episode_ids
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let records = crate::v3::queue_records(&state).await?;
    Ok(Json(
        records
            .into_iter()
            .filter(|r| {
                q.series_id
                    .is_none_or(|s| r["seriesId"].as_i64() == Some(s))
            })
            .filter(|r| {
                episodes.is_empty()
                    || r["episodeId"]
                        .as_i64()
                        .is_some_and(|e| episodes.contains(&e))
            })
            .collect(),
    ))
}

/// The pending release behind a queue record id (`queue/grab` is for pending releases).
pub(crate) async fn pending_download_id(state: &AppState, id: i64) -> ApiResult<String> {
    let native = crate::routes::queue::get_queue(State(state.clone()))
        .await?
        .0;
    let item = native
        .items
        .iter()
        .find(|i| crate::v3_more::queue_id(&i.download_id) == id)
        .ok_or_else(|| ApiError::NotFound("The queue item does not exist.".into()))?;
    if item.tracked_state != "pending" {
        return Err(ApiError::field(
            "id",
            "Only a release a delay profile holds back can be grabbed now.",
        ));
    }
    Ok(item.download_id.clone())
}

pub(crate) async fn grab_queue_item(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let download_id = pending_download_id(&state, id).await?;
    let done = crate::routes::queue::grab_pending(State(state), Path(download_id))
        .await?
        .0;
    Ok(Json(json!({"message": done.message})))
}

pub(crate) async fn grab_queue_items(
    State(state): State<AppState>,
    Json(b): Json<BulkIds>,
) -> ApiResult<Json<Value>> {
    let mut failures = Vec::new();
    for id in b.ids {
        let result = match pending_download_id(&state, id).await {
            Ok(download_id) => {
                crate::routes::queue::grab_pending(State(state.clone()), Path(download_id))
                    .await
                    .map(|_| ())
            }
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            failures.push(format!("{id}: {e}"));
        }
    }
    if failures.is_empty() {
        Ok(Json(json!({})))
    } else {
        Err(ApiError::field("ids", failures.join("; ")))
    }
}

// ---- system ---------------------------------------------------------------------------------

/// Sonarr's `TimeSpan` text (`hh:mm:ss.fff`).
pub(crate) fn time_span(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Sonarr's `TaskResource`; ids follow the task list's order (stable for a version).
pub(crate) async fn tasks_json(state: &AppState) -> ApiResult<Vec<Value>> {
    let list = system::tasks(State(state.clone())).await?.0;
    Ok(list
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            json!({
                "id": i + 1,
                "name": t.description,
                "taskName": t.name,
                "interval": t.interval_minutes,
                "lastExecution": t.last_run_at,
                "lastStartTime": t.last_run_at,
                "nextExecution": t.next_run_at,
                "lastDuration": time_span(t.last_duration_ms.unwrap_or(0)),
                "enabled": t.enabled,
                "running": t.running,
            })
        })
        .collect())
}

pub(crate) async fn list_tasks(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(tasks_json(&state).await?))
}

pub(crate) async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    tasks_json(&state)
        .await?
        .into_iter()
        .find(|t| t["id"].as_i64() == Some(id))
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("The task does not exist.".into()))
}

/// Sonarr's `BackupResource`s (ids as Sonarr derives them from the type and name).
pub(crate) async fn list_backups(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(crate::v3_system::backups_json(&state)?))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct LogParams {
    page: Option<i64>,
    page_size: Option<i64>,
    level: Option<String>,
}

/// Sonarr's paged `LogResource`s (newest first) from Kitsunarr's in-memory log.
pub(crate) async fn logs(
    State(state): State<AppState>,
    Query(p): Query<LogParams>,
) -> ApiResult<Json<Value>> {
    // Sonarr's filter names; `fatal` is Kitsunarr's error.
    let level = p
        .level
        .as_deref()
        .map(|l| match l.to_ascii_lowercase().as_str() {
            "fatal" | "error" => "error".to_string(),
            "warn" | "warning" => "warn".to_string(),
            "info" => "info".to_string(),
            "debug" => "debug".to_string(),
            _ => "trace".to_string(),
        });
    let entries = system::logs(
        State(state),
        Query(system::LogQuery {
            level,
            limit: Some(2000),
        }),
    )
    .await
    .0;
    let mut records: Vec<Value> = entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "id": i + 1,
                "time": e.time,
                "level": e.level.to_ascii_lowercase(),
                "logger": e.target,
                "message": e.message,
            })
        })
        .collect();
    records.sort_by(|a, b| b["time"].as_str().cmp(&a["time"].as_str()));
    let q = PageQuery {
        page: p.page,
        page_size: p.page_size.or(Some(50)),
    };
    Ok(Json(paged(records, &q, "time", "descending")))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RenameParams {
    series_id: i64,
    season_number: Option<i32>,
}

/// Sonarr's `GET /rename`: the files a rename would change (`RenameEpisodeResource`).
pub(crate) async fn rename_preview(
    State(state): State<AppState>,
    Query(p): Query<RenameParams>,
) -> ApiResult<Json<Vec<Value>>> {
    let previews = crate::routes::library::rename_preview(
        State(state),
        Path(p.series_id),
        Query(crate::routes::library::RenameQuery {
            season_number: p.season_number,
        }),
    )
    .await?
    .0;
    Ok(Json(
        previews
            .into_iter()
            .map(|r| {
                json!({
                    "seriesId": p.series_id,
                    "seasonNumber": r.season_number,
                    "episodeNumbers": r.episode_numbers,
                    "episodeFileId": r.episode_file_id,
                    "existingPath": r.existing_path,
                    "newPath": r.new_path,
                })
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManualImportParams {
    folder: String,
    series_id: Option<i64>,
}

/// Sonarr's `GET /manualimport`: a folder's video files with the series and episodes they
/// match (`ManualImportResource`); importing them is the `ManualImport` command.
pub(crate) async fn manual_import_scan(
    State(state): State<AppState>,
    Query(p): Query<ManualImportParams>,
) -> ApiResult<Json<Vec<Value>>> {
    let items = crate::routes::library_tools::manual_import_scan(
        State(state),
        Query(crate::routes::library_tools::ManualImportQuery {
            folder: p.folder,
            series_id: p.series_id,
        }),
    )
    .await?
    .0;
    Ok(Json(
        items
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                let folder_name = std::path::Path::new(&m.path)
                    .parent()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let quality = kitsunarr_parser::parse_quality(&m.name);
                json!({
                    "id": i + 1,
                    "path": m.path,
                    "relativePath": m.relative_path,
                    "folderName": folder_name,
                    "name": m.name,
                    "size": m.size,
                    "series": m.series_id.map(|id| json!({"id": id, "title": m.series_title})),
                    "seasonNumber": m.season_number,
                    "episodes": m.episode_ids.iter().zip(m.episodes.iter()).map(|(id, label)| json!({"id": id, "title": label})).collect::<Vec<_>>(),
                    "episodeIds": m.episode_ids,
                    "quality": {
                        "quality": crate::v3::quality_json(quality.quality.id),
                        "revision": {"version": quality.revision.version, "real": quality.revision.real, "isRepack": quality.revision.is_repack},
                    },
                    "qualityWeight": 0,
                    "releaseGroup": m.release_group,
                    "languages": [],
                    "customFormats": [],
                    "rejections": m.rejections.iter().map(|r| json!({"reason": r, "type": "permanent"})).collect::<Vec<_>>(),
                })
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonarr_resources_map_onto_kitsunarr_rules() {
        let base = DelayProfile {
            id: 3,
            enable_usenet: true,
            enable_torrent: true,
            preferred_protocol: "usenet".into(),
            usenet_delay: 0,
            torrent_delay: 0,
            bypass_if_highest_quality: true,
            bypass_if_above_custom_format_score: false,
            minimum_custom_format_score: 0,
            sort_order: 2,
            tags: vec![1],
        };
        let d = delay_from(
            &json!({"preferredProtocol": 2, "torrentDelay": 120, "order": 1}),
            base.clone(),
        )
        .unwrap();
        assert_eq!(d.preferred_protocol, "torrent");
        assert_eq!(d.torrent_delay, 120);
        assert_eq!(d.sort_order, 1);
        assert_eq!(d.tags, vec![1]);
        assert_eq!(delay_json(&d)["order"], 1);
        assert!(delay_from(&json!({"preferredProtocol": "nzb"}), base).is_err());

        let e = exclusion_from(&json!({"tvdbId": 81189, "title": " Breaking Bad "}), 0).unwrap();
        assert_eq!(
            (e.provider.as_str(), e.external_id.as_str()),
            ("tvdb", "81189")
        );
        assert_eq!(e.title.as_deref(), Some("Breaking Bad"));
        assert_eq!(exclusion_json(&e)["tvdbId"], 81189);
        let a = exclusion_from(&json!({"provider": "aniList", "externalId": 21}), 0).unwrap();
        assert_eq!(exclusion_json(&a)["tvdbId"], 0);
        assert_eq!(a.external_id, "21");
        assert!(exclusion_from(&json!({"title": "x"}), 0).is_err());

        assert_eq!(time_span(3_723_004), "01:02:03.004");
        let p = paged(
            (0..25).map(|i| json!(i)).collect(),
            &PageQuery {
                page: Some(3),
                page_size: Some(10),
            },
            "x",
            "ascending",
        );
        assert_eq!(p["records"].as_array().unwrap().len(), 5);
        assert_eq!(p["totalRecords"], 25);
    }
}
