//! Sonarr's `/api/v5` (Tanukarr 0.1.5's current API), built on the v3 resources with explicit
//! transforms so both versions stay consistent: v5 renames `notification` to `connection` and
//! `config/*` to `settings/*` (see `v5_settings`), groups queue items by download, nests release
//! decisions, embeds series, episodes and files only on request (`includeSubresources`), adds
//! per-season statistics, media information streams, library statistics, Tanukarr's library
//! tools and drops language profiles.

use std::collections::{BTreeSet, HashMap};

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, RawQuery, State};
use axum::handler::Handler;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use kitsunarr_db::library::series;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::{v3, v3_admin, v3_extra, v3_more, v3_providers, v3_settings, v3_system};

/// Applies a synchronous transform to a successful JSON response.
async fn json_out(res: Response, f: fn(&mut Value)) -> Response {
    if !res.status().is_success() {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(mut v) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    f(&mut v);
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(
        parts,
        Body::from(serde_json::to_vec(&v).unwrap_or_default()),
    )
}

/// `out!(f)`: a route layer applying `f` to the handler's JSON (each element of a list).
macro_rules! out {
    ($f:path) => {
        axum::middleware::map_response(|r: Response| json_out(r, $f))
    };
}

/// A v3 success status (`202 Accepted` for an update, `200` for a delete) as v5 answers it.
async fn no_content(res: Response) -> Response {
    if res.status().is_success() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        res
    }
}

/// `200 OK` instead of v3's `202 Accepted`.
async fn ok_status(mut res: Response) -> Response {
    if res.status() == StatusCode::ACCEPTED {
        *res.status_mut() = StatusCode::OK;
    }
    res
}

fn each(v: &mut Value, f: fn(&mut Value)) {
    match v {
        Value::Array(items) => items.iter_mut().for_each(f),
        Value::Object(m) if m.contains_key("records") => {
            if let Some(Value::Array(items)) = m.get_mut("records") {
                items.iter_mut().for_each(f);
            }
        }
        other => f(other),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        // System.
        .route(
            "/system/status",
            get(v3::system_status).layer(out!(system_out)),
        )
        .route("/health", get(v3::health).layer(out!(health_out)))
        .route("/system/task", get(v3_admin::list_tasks))
        .route("/system/task/{id}", get(v3_admin::get_task))
        .route("/system/backup", get(v3_admin::list_backups))
        .route("/log", get(v3_admin::logs))
        .route("/diskspace", get(v3_more::disk_space))
        .route("/statistics", get(statistics))
        // Tags.
        .route("/tag", get(v3::tags).post(v3::create_tag))
        .route("/tag/detail", get(v3_admin::tag_details))
        .route("/tag/detail/{id}", get(v3_admin::tag_detail))
        .route(
            "/tag/{id}",
            get(v3_admin::get_tag)
                .put(v3_admin::update_tag)
                .delete(v3_admin::delete_tag),
        )
        // Root folders.
        .route(
            "/rootfolder",
            get(v3::root_folders)
                .post(v3_admin::create_root_folder)
                .layer(out!(root_folder_out)),
        )
        .route(
            "/rootfolder/{id}",
            get(v3_admin::get_root_folder)
                .delete(v3_admin::delete_root_folder)
                .layer(out!(root_folder_out)),
        )
        // Series.
        .route("/series", get(list_series).post(add_series))
        .route("/series/lookup", get(lookup))
        .route("/series/import", post(import_series))
        .route("/series/ordering", get(orderings))
        .route(
            "/series/editor",
            put(series_editor)
                .delete(v3::series_editor_delete.layer(axum::middleware::map_response(no_content))),
        )
        .route(
            "/series/{id}",
            get(get_series)
                .put(update_series)
                .delete(v3::delete_series.layer(axum::middleware::map_response(no_content))),
        )
        .route("/series/{id}/season", put(update_season))
        .route(
            "/series/compression/capabilities",
            get(crate::routes::compression::capabilities),
        )
        .route(
            "/series/{id}/compression",
            get(crate::routes::compression::status)
                .post(crate::routes::compression::start)
                .delete(crate::routes::compression::cancel),
        )
        .route(
            "/seasonpass",
            post(v3::season_pass).layer(axum::middleware::map_response(no_content)),
        )
        // Episodes and files.
        .route("/episode", get(list_episodes))
        .route("/episode/{id}", get(get_episode))
        .route("/episode/monitor", put(monitor_episodes))
        .route("/episodefile", get(list_episode_files))
        .route(
            "/episodefile/{id}",
            get(get_episode_file).delete(
                v3_more::delete_episode_file.layer(axum::middleware::map_response(no_content)),
            ),
        )
        .route(
            "/episodefile/bulk",
            delete(v3_more::delete_episode_files.layer(axum::middleware::map_response(no_content))),
        )
        .route("/rename", get(v3_admin::rename_preview))
        .route("/manualimport", get(manual_import_scan))
        // Calendar, wanted, history, blocklist.
        .route("/calendar", get(calendar))
        .route("/wanted/missing", get(wanted_missing))
        .route("/wanted/cutoff", get(wanted_cutoff))
        .route("/history", get(history))
        .route("/history/since", get(history_since))
        .route("/history/series", get(history_series))
        .route("/history/season", get(history_season))
        .route("/history/episode", get(history_episode))
        .route("/blocklist", get(v3::blocklist).layer(out!(blocklist_out)))
        .route(
            "/blocklist/{id}",
            delete(v3::delete_blocklisted.layer(axum::middleware::map_response(no_content))),
        )
        .route(
            "/blocklist/bulk",
            delete(v3::delete_blocklisted_bulk.layer(axum::middleware::map_response(no_content))),
        )
        // Queue.
        .route("/queue", get(queue))
        .route("/queue/details", get(queue_details))
        .route("/queue/status", get(v3_admin::queue_status))
        .route(
            "/queue/{id}",
            delete(v3_more::remove_queue_item.layer(axum::middleware::map_response(no_content))),
        )
        .route(
            "/queue/bulk",
            delete(v3_more::remove_queue_items.layer(axum::middleware::map_response(no_content))),
        )
        .route(
            "/queue/grab/{id}",
            post(v3_admin::grab_queue_item.layer(axum::middleware::map_response(no_content))),
        )
        .route(
            "/queue/grab/bulk",
            post(v3_admin::grab_queue_items.layer(axum::middleware::map_response(no_content))),
        )
        // Releases.
        .route("/release", get(releases).post(grab_release))
        .route("/release/push", post(push_release))
        .route("/release/push/{id}", get(pushed_release))
        .route("/parse", get(parse))
        // Commands.
        .route("/command", get(v3::list_commands).post(v3::command))
        .route("/command/{id}", get(v3::get_command))
        // Profiles and formats.
        .route(
            "/qualityprofile",
            get(v3::quality_profiles).post(v3_settings::create_profile),
        )
        .route("/qualityprofile/schema", get(v3_settings::profile_schema))
        .route(
            "/qualityprofile/{id}",
            get(v3_settings::get_profile)
                .put(v3_settings::update_profile)
                .delete(v3_settings::delete_profile),
        )
        .route(
            "/qualitydefinition",
            get(v3_settings::list_definitions).put(
                v3_settings::update_definitions.layer(axum::middleware::map_response(ok_status)),
            ),
        )
        .route(
            "/qualitydefinition/{id}",
            get(v3_settings::get_definition).put(v3_settings::update_definition),
        )
        .route(
            "/customformat",
            get(v3_settings::list_formats).post(v3_settings::create_format),
        )
        .route("/customformat/schema", get(v3_settings::format_schema))
        .route(
            "/customformat/{id}",
            get(v3_settings::get_format)
                .put(v3_settings::update_format)
                .delete(v3_settings::delete_format),
        )
        .route(
            "/releaseprofile",
            get(list_release_profiles).post(create_release_profile),
        )
        .route(
            "/releaseprofile/{id}",
            get(get_release_profile).put(update_release_profile).delete(
                v3_settings::delete_release_profile
                    .layer(axum::middleware::map_response(no_content)),
            ),
        )
        .route(
            "/delayprofile",
            get(v3_admin::list_delay).post(v3_admin::create_delay),
        )
        .route(
            "/delayprofile/{id}",
            get(v3_admin::get_delay)
                .put(v3_admin::update_delay)
                .delete(v3_admin::delete_delay),
        )
        .route("/language", get(v3_settings::languages))
        .route(
            "/remotepathmapping",
            get(v3_admin::list_mappings).post(v3_admin::create_mapping),
        )
        .route(
            "/remotepathmapping/{id}",
            get(v3_admin::get_mapping)
                .put(v3_admin::update_mapping.layer(axum::middleware::map_response(ok_status)))
                .delete(v3_admin::delete_mapping),
        )
        .route(
            "/importlistexclusion",
            get(v3_admin::paged_exclusions).post(v3_admin::create_exclusion),
        )
        .route(
            "/importlistexclusion/bulk",
            delete(v3_admin::delete_exclusions.layer(axum::middleware::map_response(no_content))),
        )
        .route(
            "/importlistexclusion/{id}",
            get(v3_admin::get_exclusion)
                .put(v3_admin::update_exclusion)
                .delete(v3_admin::delete_exclusion),
        )
        // Providers (connections are v3's notifications).
        .route("/indexer", get(v3::list_indexers).post(v3::create_indexer))
        .route("/indexer/schema", get(v3::indexer_schema))
        .route("/indexer/test", post(v3::test_indexer))
        .route(
            "/indexer/{id}",
            get(v3::get_indexer)
                .put(v3::update_indexer)
                .delete(v3::delete_indexer),
        )
        .route(
            "/downloadclient",
            get(v3::download_clients).post(v3_providers::create_client),
        )
        .route("/downloadclient/schema", get(v3_providers::client_schema))
        .route("/downloadclient/test", post(v3_providers::test_client))
        .route(
            "/downloadclient/{id}",
            get(v3::download_client)
                .put(v3_providers::update_client)
                .delete(v3_providers::delete_client),
        )
        .route(
            "/importlist",
            get(v3_providers::list_import_lists).post(v3_providers::create_list),
        )
        .route("/importlist/schema", get(v3_providers::list_schema))
        .route("/importlist/test", post(v3_providers::test_list))
        .route(
            "/importlist/{id}",
            get(v3_providers::get_import_list)
                .put(v3_providers::update_list)
                .delete(v3_providers::delete_list),
        )
        .route(
            "/connection",
            get(v3::notifications).post(v3_providers::create_notification),
        )
        .route("/connection/schema", get(v3_providers::notification_schema))
        .route("/connection/test", post(v3_providers::test_notification))
        .route(
            "/connection/{id}",
            get(v3::notification)
                .put(v3_providers::update_notification)
                .delete(v3_providers::delete_notification),
        )
        // Tanukarr's library tools.
        .route(
            "/librarytools",
            get(library_tools_options).put(save_library_tools_options),
        )
        .route(
            "/librarytools/series/{id}",
            get(series_tool_options).put(save_series_tool_options),
        )
        .route("/librarytools/searches", get(tool_searches))
        .route("/librarytools/storage", get(tool_storage))
        .route("/librarytools/audit", post(tool_audit))
        .route("/librarytools/keys", get(tool_keys).post(create_tool_key))
        .route("/librarytools/keys/{id}", delete(revoke_tool_key))
        .route("/librarytools/recycle", get(tool_recycle))
        .route("/librarytools/recycle/restore", post(tool_recycle_restore))
        .route("/librarytools/series/{id}/versions", get(tool_versions))
        .route(
            "/librarytools/series/{id}/versions/{version_id}/activate",
            post(tool_activate_version),
        )
        .route("/librarytools/lists/{id}", get(list_preview))
        .route(
            "/librarytools/lists/{id}/preview",
            delete(clear_list_preview),
        )
        .route("/librarytools/lists/{id}/add", post(add_from_list))
        .merge(crate::v3_ext::router("connection"))
        .merge(v3_extra::shared::<true>())
        .merge(v3_system::shared::<true>())
        .layer(axum::middleware::from_fn(deletes_no_content))
}

/// Sonarr v5 answers every successful delete with `204 No Content` (v3 with `200`).
async fn deletes_no_content(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let delete = req.method() == axum::http::Method::DELETE;
    let res = next.run(req).await;
    if delete && res.status() == StatusCode::OK {
        return StatusCode::NO_CONTENT.into_response();
    }
    res
}

// ---- query helpers --------------------------------------------------------------------------

/// The `includeSubresources` values (repeated or comma-separated), lower case.
pub(crate) fn subresources(raw: Option<&str>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for pair in raw.unwrap_or_default().split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = k.replace("%5B%5D", "[]");
        if k.eq_ignore_ascii_case("includeSubresources")
            || k.eq_ignore_ascii_case("includeSubresources[]")
        {
            for part in v.replace("%2C", ",").split(',') {
                if !part.trim().is_empty() {
                    out.insert(part.trim().to_ascii_lowercase());
                }
            }
        }
    }
    out
}

/// A typed v3 query from the request's query, without the keys v5 repeats.
fn typed<T: DeserializeOwned>(raw: Option<&str>, drop: &[&str]) -> ApiResult<Query<T>> {
    let kept: Vec<&str> = raw
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let k = p.split_once('=').map_or(*p, |(k, _)| k);
            !drop.iter().any(|d| {
                k.eq_ignore_ascii_case(d)
                    || k.eq_ignore_ascii_case(&format!("{d}[]"))
                    || k.eq_ignore_ascii_case(&format!("{d}%5B%5D"))
            })
        })
        .collect();
    let uri: Uri = format!("/?{}", kept.join("&"))
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid query.".into()))?;
    Query::try_from_uri(&uri).map_err(|e| ApiError::BadRequest(e.body_text()))
}

fn query_value(raw: Option<&str>) -> Value {
    v3_extra::query_json(raw)
}

fn ids(raw: Option<&str>, key: &str) -> Vec<i64> {
    v3_extra::query_ids(raw, key)
}

// ---- simple transforms ----------------------------------------------------------------------

/// Sonarr v5's `SystemResource` lost `isContainerized` and `sqliteVersion`.
fn system_out(v: &mut Value) {
    if let Some(m) = v.as_object_mut() {
        m.remove("isContainerized");
        m.remove("sqliteVersion");
    }
}

/// Sonarr's `HealthCheckReason` for a check, from its source (and message).
pub(crate) fn health_reason(source: &str, message: &str) -> &'static str {
    let m = message.to_lowercase();
    match source {
        "IndexerRssCheck" if m.contains("unavailable") => "indexerRssNoIndexersAvailable",
        "IndexerRssCheck" => "indexerRssNoIndexersEnabled",
        "IndexerSearchCheck" if m.contains("interactive") => "indexerSearchNoInteractive",
        "IndexerSearchCheck" if m.contains("unavailable") => "indexerSearchNoAvailableIndexers",
        "IndexerSearchCheck" => "indexerSearchNoAutomatic",
        "IndexerStatusCheck" if m.contains("all") => "indexerStatusAllUnavailable",
        "IndexerStatusCheck" => "indexerStatusUnavailable",
        "IndexerLongTermStatusCheck" if m.contains("all") => "indexerLongTermStatusAllUnavailable",
        "IndexerLongTermStatusCheck" => "indexerLongTermStatusUnavailable",
        "IndexerJackettAllCheck" => "indexerJackettAll",
        "IndexerDownloadClientCheck" => "indexerDownloadClient",
        "ImportMechanismCheck" => "importMechanismHandlingDisabled",
        "ImportListStatusCheck" if m.contains("all") => "importListStatusAllUnavailable",
        "ImportListStatusCheck" => "importListStatusUnavailable",
        "ImportListRootFolderCheck" if m.contains("folders") => {
            "importListRootFolderMultipleMissing"
        }
        "ImportListRootFolderCheck" => "importListRootFolderMissing",
        "RecyclingBinCheck" => "recycleBinUnableToWrite",
        "BackupCheck" => "backupStale",
        "ApiKeyValidationCheck" => "minimumApiKeyLength",
        "RemovedSeriesCheck" if m.contains("series were") || m.contains("are no longer") => {
            "removedSeriesMultiple"
        }
        "RemovedSeriesCheck" => "removedSeriesSingle",
        "MountCheck" => "mountSeries",
        "DownloadClientCheck" if m.contains("unable") => "downloadClientCheckUnableToCommunicate",
        "DownloadClientCheck" => "downloadClientCheckNoneAvailable",
        "DownloadClientStatusCheck" if m.contains("all") => "downloadClientStatusAllClients",
        "DownloadClientStatusCheck" => "downloadClientStatusSingleClient",
        "DownloadClientRemovesCompletedDownloadsCheck" => "downloadClientRemovesCompletedDownloads",
        "DownloadClientSortingCheck" => "downloadClientSorting",
        "DownloadClientRootFolderCheck" => "downloadClientRootFolder",
        "RemotePathMappingCheck" => "remotePathMappingImportEpisodeFailed",
        "NotificationStatusCheck" if m.contains("all") => "notificationStatusAll",
        "NotificationStatusCheck" => "notificationStatusSingle",
        "ProxyCheck" => "proxyFailed",
        _ => "unknown",
    }
}

fn health_out(v: &mut Value) {
    each(v, |h| {
        let reason = health_reason(
            h["source"].as_str().unwrap_or_default(),
            h["message"].as_str().unwrap_or_default(),
        );
        h["reason"] = json!(reason);
        h["wikiUrl"] = json!(format!(
            "https://wiki.servarr.com/sonarr/system#{}",
            h["source"].as_str().unwrap_or_default().to_lowercase()
        ));
    });
}

/// Sonarr v5's `BlocklistResource` adds where the release came from (`source`).
fn blocklist_out(v: &mut Value) {
    each(v, |b| {
        if b.get("source").is_none() {
            b["source"] = b["indexer"].clone();
        }
    });
}

/// Sonarr v5's `RootFolderResource` adds `isEmpty`.
fn root_folder_out(v: &mut Value) {
    each(v, |r| {
        if let Some(path) = r["path"].as_str() {
            let empty = std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_none());
            r["isEmpty"] = json!(empty);
        }
    });
}

// ---- series ---------------------------------------------------------------------------------

#[derive(Default, Clone)]
struct SeasonFacts {
    monitored: i64,
    next_airing: Option<String>,
    previous_airing: Option<String>,
    release_groups: BTreeSet<String>,
    release_types: BTreeSet<i64>,
    qualities: BTreeSet<i64>,
    images: Value,
}

#[derive(Default)]
struct SeriesFacts {
    seasons: HashMap<(i64, i64), SeasonFacts>,
    original_country: HashMap<i64, Option<String>>,
}

/// What v5 adds to series statistics, read for every series at once.
async fn series_facts(state: &AppState, series_id: Option<i64>) -> ApiResult<SeriesFacts> {
    use kitsunarr_db::sqlx;
    let now = kitsunarr_db::now();
    let filter = series_id.unwrap_or(-1);
    type SeasonRow = (i64, i64, i64, Option<String>, Option<String>);
    let rows: Vec<SeasonRow> = sqlx::query_as(
        "SELECT series_id, season_number, \
           COALESCE(SUM(CASE WHEN monitored = 1 THEN 1 ELSE 0 END), 0), \
           MIN(CASE WHEN air_date_utc > ?1 THEN air_date_utc END), \
           MAX(CASE WHEN air_date_utc <= ?1 THEN air_date_utc END) \
         FROM episodes WHERE (?2 = -1 OR series_id = ?2) GROUP BY series_id, season_number",
    )
    .bind(&now)
    .bind(filter)
    .fetch_all(state.db.pool())
    .await
    .map_err(kitsunarr_db::DbError::from)?;
    let mut facts = SeriesFacts::default();
    for (sid, season, monitored, next, previous) in rows {
        let f = facts.seasons.entry((sid, season)).or_default();
        f.monitored = monitored;
        f.next_airing = next;
        f.previous_airing = previous;
    }
    let files: Vec<(i64, i64, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT series_id, season_number, release_group, release_type, quality_id FROM episode_files \
         WHERE (?1 = -1 OR series_id = ?1)",
    )
    .bind(filter)
    .fetch_all(state.db.pool())
    .await
    .map_err(kitsunarr_db::DbError::from)?;
    for (sid, season, group, release_type, quality) in files {
        let f = facts.seasons.entry((sid, season)).or_default();
        if let Some(g) = group.filter(|g| !g.trim().is_empty()) {
            f.release_groups.insert(g);
        }
        f.release_types.insert(release_type);
        f.qualities.insert(quality);
    }
    let seasons: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT series_id, season_number, images FROM seasons WHERE (?1 = -1 OR series_id = ?1)",
    )
    .bind(filter)
    .fetch_all(state.db.pool())
    .await
    .map_err(kitsunarr_db::DbError::from)?;
    for (sid, season, images) in seasons {
        let images: Vec<Value> = serde_json::from_str(&images).unwrap_or_default();
        facts.seasons.entry((sid, season)).or_default().images = json!(images
            .iter()
            .map(|i| json!({"coverType": i["coverType"], "url": i["remoteUrl"], "remoteUrl": i["remoteUrl"]}))
            .collect::<Vec<_>>());
    }
    let countries: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, original_country FROM series WHERE (?1 = -1 OR id = ?1)")
            .bind(filter)
            .fetch_all(state.db.pool())
            .await
            .map_err(kitsunarr_db::DbError::from)?;
    facts.original_country = countries.into_iter().collect();
    Ok(facts)
}

/// Sonarr's `ReleaseType` names.
fn release_type_name(t: i64) -> &'static str {
    match t {
        1 => "singleEpisode",
        2 => "multiEpisode",
        3 => "seasonPack",
        _ => "unknown",
    }
}

fn stats_extras(stats: &mut Value, facts: &[&SeasonFacts]) {
    let monitored: i64 = facts.iter().map(|f| f.monitored).sum();
    let groups: BTreeSet<&String> = facts.iter().flat_map(|f| &f.release_groups).collect();
    let types: BTreeSet<i64> = facts
        .iter()
        .flat_map(|f| f.release_types.iter().copied())
        .collect();
    let qualities: BTreeSet<i64> = facts
        .iter()
        .flat_map(|f| f.qualities.iter().copied())
        .collect();
    stats["monitoredEpisodeCount"] = json!(monitored);
    stats["releaseGroups"] = json!(groups);
    stats["releaseTypes"] = json!(
        types
            .iter()
            .map(|t| release_type_name(*t))
            .collect::<Vec<_>>()
    );
    stats["episodeFileQualities"] = json!(
        qualities
            .iter()
            .map(|q| v3::quality_json(*q as i32))
            .collect::<Vec<_>>()
    );
    stats["nextAiring"] = json!(facts.iter().filter_map(|f| f.next_airing.clone()).min());
    stats["previousAiring"] = json!(facts.iter().filter_map(|f| f.previous_airing.clone()).max());
}

/// Sonarr v5's `SeriesResource` from the v3 one: no language profile, Tanukarr's catalogue
/// fields, the original country and v5's season and series statistics.
fn series_v5(mut s: Value, facts: &SeriesFacts, season_images: bool) -> Value {
    let id = s["id"].as_i64().unwrap_or_default();
    if let Some(m) = s.as_object_mut() {
        m.remove("languageProfileId");
    }
    s["originalCountry"] = json!(facts.original_country.get(&id).cloned().flatten());
    let mut all: Vec<&SeasonFacts> = Vec::new();
    let empty = SeasonFacts::default();
    if let Some(seasons) = s["seasons"].as_array_mut() {
        for season in seasons.iter_mut() {
            let n = season["seasonNumber"].as_i64().unwrap_or_default();
            let f = facts.seasons.get(&(id, n)).unwrap_or(&empty);
            all.push(f);
            stats_extras(&mut season["statistics"], &[f]);
            if season_images {
                season["images"] = if f.images.is_null() {
                    json!([])
                } else {
                    f.images.clone()
                };
            }
        }
    }
    stats_extras(&mut s["statistics"], &all);
    s["nextAiring"] = s["statistics"]["nextAiring"].clone();
    s["previousAiring"] = s["statistics"]["previousAiring"].clone();
    s
}

pub(crate) async fn series_out(
    state: &AppState,
    list: Vec<Value>,
    season_images: bool,
) -> ApiResult<Vec<Value>> {
    let single = (list.len() == 1).then(|| list[0]["id"].as_i64()).flatten();
    let facts = series_facts(state, single).await?;
    Ok(list
        .into_iter()
        .map(|s| {
            if s["id"].as_i64().unwrap_or(0) > 0 {
                series_v5(s, &facts, season_images)
            } else {
                s
            }
        })
        .collect())
}

pub(crate) async fn one_series(state: &AppState, id: i64) -> ApiResult<Value> {
    let Json(s) = v3::get_series(State(state.clone()), Path(id)).await?;
    Ok(series_out(state, vec![s], false).await?.remove(0))
}

async fn list_series(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = typed::<v3::SeriesFilter>(raw.as_deref(), &["includeSubresources"])?;
    let Json(list) = v3::list_series(State(state.clone()), q).await?;
    let images = subresources(raw.as_deref()).contains("seasonimages");
    Ok(Json(series_out(&state, list, images).await?))
}

async fn get_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    let Json(s) = v3::get_series(State(state.clone()), Path(id)).await?;
    let images = subresources(raw.as_deref()).contains("seasonimages");
    Ok(Json(series_out(&state, vec![s], images).await?.remove(0)))
}

async fn add_series(
    State(state): State<AppState>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    body: Json<v3::SeriesBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (status, Json(s)) = v3::add_series(State(state.clone()), principal, body).await?;
    Ok((
        status,
        Json(series_out(&state, vec![s], false).await?.remove(0)),
    ))
}

async fn import_series(
    State(state): State<AppState>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    body: Json<Vec<v3::SeriesBody>>,
) -> ApiResult<Json<Vec<Value>>> {
    let (_, Json(list)) = v3::import_series(State(state.clone()), principal, body).await?;
    Ok(Json(series_out(&state, list, false).await?))
}

async fn update_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    RawQuery(raw): RawQuery,
    body: Json<v3::SeriesBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let q = typed::<v3::MoveFilesQuery>(raw.as_deref(), &[])?;
    let (status, Json(s)) = v3::update_series(State(state.clone()), Path(id), q, body).await?;
    Ok((
        status,
        Json(series_out(&state, vec![s], false).await?.remove(0)),
    ))
}

async fn series_editor(
    State(state): State<AppState>,
    body: Json<v3::SeriesEditorBody>,
) -> ApiResult<Json<Vec<Value>>> {
    let (_, Json(list)) = v3::series_editor(State(state.clone()), body).await?;
    Ok(Json(series_out(&state, list, false).await?))
}

async fn lookup(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = typed::<v3::LookupQuery>(
        raw.as_deref(),
        &["language", "metadataSource", "allSources"],
    )?;
    let Json(list) = v3::lookup(State(state.clone()), q).await?;
    Ok(Json(
        list.into_iter()
            .map(|mut s| {
                if let Some(m) = s.as_object_mut() {
                    m.remove("languageProfileId");
                }
                s
            })
            .collect(),
    ))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeasonBody {
    season_number: i32,
    monitored: bool,
}

/// `PUT /series/{id}/season`: a season's monitoring.
async fn update_season(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(b): Json<SeasonBody>,
) -> ApiResult<Json<Value>> {
    let s = one_series(&state, id).await?;
    if !s["seasons"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|x| x["seasonNumber"] == json!(b.season_number))
    {
        return Err(ApiError::NotFound("The season does not exist.".into()));
    }
    series::season_pass(
        &state.db,
        id,
        None,
        &[kitsunarr_domain::api::library::SeasonMonitoring {
            season_number: b.season_number,
            monitored: b.monitored,
        }],
        None,
    )
    .await?;
    state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: vec![id],
            deleted: false,
        });
    let s = one_series(&state, id).await?;
    Ok(Json(
        s["seasons"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|x| x["seasonNumber"] == json!(b.season_number))
            .cloned()
            .unwrap_or(Value::Null),
    ))
}

/// `GET /series/ordering?tmdbId=`: the episode orderings TMDB offers for a show (the default
/// first; IMDb orderings are not offered).
async fn orderings(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Json<Vec<Value>> {
    let q = query_value(raw.as_deref());
    let mut out = vec![json!({
        "id": "", "name": "Default", "description": "The catalogue's own episode order.",
        "episodeCount": 0, "seasonCount": 0, "isDefault": true, "isAbsolute": false,
    })];
    if let Some(tmdb) = q["tmdbId"].as_i64().filter(|i| *i > 0)
        && let Ok(list) = state
            .metadata
            .orderings(
                kitsunarr_domain::MetadataProvider::TmdbTv,
                &tmdb.to_string(),
            )
            .await
    {
        out.extend(list.into_iter().filter(|o| !o.id.is_empty()).map(|o| {
            json!({
                "id": o.id, "name": o.name, "description": o.description,
                "episodeCount": 0, "seasonCount": 0, "isDefault": false,
                "isAbsolute": o.name.to_lowercase().contains("absolute"),
            })
        }));
    }
    Json(out)
}

// ---- episodes -------------------------------------------------------------------------------

/// Sonarr v5's `EpisodeResource` from the v3 one (`endTime` and `grabDate` are never filled).
pub(crate) fn episode_v5(e: &mut Value) {
    if e.is_object() {
        e["endTime"] = Value::Null;
        e["grabDate"] = Value::Null;
    }
}

/// Embeds the subresources asked for (`series`, `episodefile`, `images`) in v5 episodes.
pub(crate) async fn episodes_out(
    state: &AppState,
    mut list: Vec<Value>,
    include: &BTreeSet<String>,
) -> ApiResult<Vec<Value>> {
    let mut series_cache: HashMap<i64, Value> = HashMap::new();
    let mut files_cache: HashMap<i64, Vec<Value>> = HashMap::new();
    for e in list.iter_mut() {
        episode_v5(e);
        if let Some(m) = e.as_object_mut() {
            m.remove("series");
            m.remove("episodeFile");
            m.remove("images");
        }
        let sid = e["seriesId"].as_i64().unwrap_or_default();
        if include.contains("series") {
            if let std::collections::hash_map::Entry::Vacant(slot) = series_cache.entry(sid) {
                slot.insert(one_series(state, sid).await?);
            }
            e["series"] = series_cache[&sid].clone();
        }
        let file_id = e["episodeFileId"].as_i64().unwrap_or_default();
        if include.contains("episodefile") && file_id > 0 {
            if let std::collections::hash_map::Entry::Vacant(slot) = files_cache.entry(sid) {
                slot.insert(files_v5(state, sid).await?);
            }
            if let Some(f) = files_cache[&sid].iter().find(|f| f["id"] == json!(file_id)) {
                e["episodeFile"] = f.clone();
            }
        }
        if include.contains("images") {
            e["images"] = json!([]);
        }
    }
    Ok(list)
}

async fn list_episodes(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = query_value(raw.as_deref());
    let include = subresources(raw.as_deref());
    let episode_ids = ids(raw.as_deref(), "episodeIds");
    let list: Vec<Value> = if let Some(sid) = q["seriesId"].as_i64() {
        let season = q["seasonNumber"].as_i64().map(|s| s as i32);
        series::list_episodes(&state.db, sid, season)
            .await?
            .iter()
            .map(v3::episode_json)
            .collect()
    } else if !episode_ids.is_empty() {
        let mut out = Vec::new();
        for id in episode_ids {
            if let Ok(Json(e)) = v3::get_episode(State(state.clone()), Path(id)).await {
                out.push(e);
            }
        }
        out
    } else if let Some(file) = q["episodeFileId"].as_i64() {
        match series::series_of_file(&state.db, file).await? {
            Some(sid) => series::list_episodes(&state.db, sid, None)
                .await?
                .iter()
                .filter(|e| e.file.as_ref().is_some_and(|f| f.id == file))
                .map(v3::episode_json)
                .collect(),
            None => Vec::new(),
        }
    } else {
        return Err(ApiError::BadRequest(
            "seriesId or episodeIds must be provided".into(),
        ));
    };
    Ok(Json(episodes_out(&state, list, &include).await?))
}

async fn get_episode(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let e = v3_extra::episode_full(&state, id).await?;
    let all: BTreeSet<String> = ["series", "episodefile", "images"].map(String::from).into();
    Ok(Json(episodes_out(&state, vec![e], &all).await?.remove(0)))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitorBody {
    episode_ids: Vec<i64>,
    monitored: bool,
}

/// `PUT /episode/monitor`: answers the changed episodes (v3 answers no body).
async fn monitor_episodes(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
    Json(b): Json<MonitorBody>,
) -> ApiResult<Json<Vec<Value>>> {
    series::set_episodes_monitored(&state.db, &b.episode_ids, b.monitored).await?;
    let mut out = Vec::new();
    for id in &b.episode_ids {
        if let Ok(Json(e)) = v3::get_episode(State(state.clone()), Path(*id)).await {
            out.push(e);
        }
    }
    let include: BTreeSet<String> = subresources(raw.as_deref())
        .into_iter()
        .filter(|s| s == "images")
        .collect();
    Ok(Json(episodes_out(&state, out, &include).await?))
}

// ---- episode files --------------------------------------------------------------------------

/// Sonarr v5's `MediaInfoResource` from Kitsunarr's stored media information.
pub(crate) fn media_info_v5(raw: Option<&str>, scene_name: &str) -> Value {
    use kitsunarr_library::media_info::{MediaInfo, format};
    let Some(info) = raw.and_then(|r| serde_json::from_str::<MediaInfo>(r).ok()) else {
        return Value::Null;
    };
    let secs = info.run_time.max(0.0) as i64;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let run_time = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    };
    json!({
        "videoBitDepth": info.video_bit_depth,
        "videoBitrate": info.video_bitrate,
        "videoCodec": format::format_video_codec(&info, scene_name),
        "videoFps": (info.video_fps * 1000.0).round() / 1000.0,
        "videoDynamicRange": format::format_video_dynamic_range(&info),
        "videoDynamicRangeType": format::format_video_dynamic_range_type(&info),
        "resolution": format!("{}x{}", info.width, info.height),
        "runTime": run_time,
        "scanType": info.scan_type,
        "audioStreams": info.audio_streams.iter().map(|a| json!({
            "language": a.language,
            "codec": format::format_audio_codec(Some(a), scene_name),
            "codecId": a.codec_id,
            "profile": a.profile,
            "bitrate": a.bitrate,
            "channels": format::format_audio_channels(Some(a)),
            "channelPositions": a.channel_positions,
            "title": a.title,
        })).collect::<Vec<_>>(),
        "subtitleStreams": info.subtitle_streams.iter().map(|s| json!({
            "language": s.language,
            "format": s.format,
            "title": s.title,
            "forced": s.forced,
            "hearingImpaired": s.hearing_impaired,
        })).collect::<Vec<_>>(),
    })
}

/// Sonarr v5's `EpisodeFileResource`s of a series.
pub(crate) async fn files_v5(state: &AppState, series_id: i64) -> ApiResult<Vec<Value>> {
    let records = series::file_records(&state.db, series_id).await?;
    let mut list = v3::files_of(state, series_id).await?;
    for f in list.iter_mut() {
        let id = f["id"].as_i64().unwrap_or_default();
        if let Some(r) = records.iter().find(|r| r.id == id) {
            if f["seasonNumber"].is_null() {
                f["seasonNumber"] = json!(
                    kitsunarr_db::sqlx::query_scalar::<_, i64>(
                        "SELECT season_number FROM episode_files WHERE id = ?"
                    )
                    .bind(id)
                    .fetch_one(state.db.pool())
                    .await
                    .unwrap_or_default()
                );
            }
            f["mediaInfo"] = media_info_v5(
                r.media_info.as_deref(),
                r.scene_name.as_deref().unwrap_or_default(),
            );
            f["indexerFlags"] = json!(r.indexer_flags);
            f["releaseType"] = json!(release_type_name(r.release_type));
        }
        if f.get("customFormats").is_none() {
            f["customFormats"] = json!([]);
            f["customFormatScore"] = json!(0);
        }
    }
    Ok(list)
}

pub(crate) async fn file_v5(state: &AppState, id: i64) -> ApiResult<Value> {
    let sid = series::series_of_file(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("The episode file does not exist.".into()))?;
    files_v5(state, sid)
        .await?
        .into_iter()
        .find(|f| f["id"] == json!(id))
        .ok_or_else(|| ApiError::NotFound("The episode file does not exist.".into()))
}

async fn list_episode_files(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = query_value(raw.as_deref());
    if let Some(sid) = q["seriesId"].as_i64() {
        return Ok(Json(files_v5(&state, sid).await?));
    }
    let wanted = ids(raw.as_deref(), "episodeFileIds");
    if wanted.is_empty() {
        return Err(ApiError::BadRequest(
            "seriesId or episodeFileIds must be provided".into(),
        ));
    }
    let mut out = Vec::new();
    for id in wanted {
        if let Ok(f) = file_v5(&state, id).await {
            out.push(f);
        }
    }
    Ok(Json(out))
}

async fn get_episode_file(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(file_v5(&state, id).await?))
}

/// `GET /manualimport`: v5 adds `seasonNumber`, `downloadIds` and `filterExistingFiles`.
async fn manual_import_scan(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = query_value(raw.as_deref());
    if q["folder"].as_str().is_none_or(|f| f.trim().is_empty()) {
        return Err(ApiError::BadRequest("folder must be provided".into()));
    }
    let typed_q = typed::<v3_admin::ManualImportParams>(
        raw.as_deref(),
        &["downloadIds", "seasonNumber", "filterExistingFiles"],
    )?;
    let Json(mut list) = v3_admin::manual_import_scan(State(state), typed_q).await?;
    if let Some(season) = q["seasonNumber"].as_i64() {
        list.retain(|i| i["seasonNumber"].as_i64().is_none_or(|s| s == season));
    }
    Ok(Json(list))
}

// ---- calendar and wanted ----------------------------------------------------------------------

async fn calendar(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = query_value(raw.as_deref());
    let include = subresources(raw.as_deref());
    let today = chrono::Utc::now().date_naive();
    let start = q["start"]
        .as_str()
        .map_or_else(|| today.to_string(), str::to_string);
    let end = q["end"].as_str().map_or_else(
        || (today + chrono::Duration::days(2)).to_string(),
        str::to_string,
    );
    let include_unmonitored = q["includeUnmonitored"].as_bool().unwrap_or(false);
    let include_specials = q["includeSpecials"].as_bool().unwrap_or(true);
    let tags: Vec<i64> = q["tags"]
        .as_str()
        .map(|t| v3_extra::id_list(Some(t)))
        .or_else(|| q["tags"].as_i64().map(|t| vec![t]))
        .unwrap_or_default();
    let entries = series::calendar(&state.db, &start, &end, include_unmonitored).await?;
    let mut episodes: HashMap<i64, Vec<kitsunarr_domain::api::library::EpisodeItem>> =
        HashMap::new();
    let mut series_tags: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut list = Vec::new();
    for c in entries {
        if !include_specials && c.season_number == 0 {
            continue;
        }
        if !tags.is_empty() {
            if let std::collections::hash_map::Entry::Vacant(slot) = series_tags.entry(c.series_id)
            {
                slot.insert(series::get_list_item(&state.db, c.series_id).await?.tags);
            }
            if !series_tags[&c.series_id].iter().any(|t| tags.contains(t)) {
                continue;
            }
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = episodes.entry(c.series_id) {
            slot.insert(series::list_episodes(&state.db, c.series_id, None).await?);
        }
        if let Some(e) = episodes[&c.series_id].iter().find(|e| e.id == c.episode_id) {
            list.push(v3::episode_json(e));
        }
    }
    Ok(Json(episodes_out(&state, list, &include).await?))
}

/// A v5 wanted page: the v3 page with the v5 filters and subresources.
async fn wanted(state: AppState, raw: Option<String>, cutoff: bool) -> ApiResult<Json<Value>> {
    let q = query_value(raw.as_deref());
    let include = subresources(raw.as_deref());
    let series_ids = ids(raw.as_deref(), "seriesIds");
    let profiles = ids(raw.as_deref(), "qualityProfileIds");
    let series_tags = ids(raw.as_deref(), "seriesTags");
    let series_types: Vec<String> = raw
        .as_deref()
        .unwrap_or_default()
        .split('&')
        .filter_map(|p| p.split_once('='))
        .filter(|(k, _)| k.eq_ignore_ascii_case("seriesType"))
        .flat_map(|(_, v)| {
            v.split(',')
                .map(|s| s.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .collect();
    let include_specials = q["includeSpecials"].as_bool().unwrap_or(true);
    let filtered = !series_ids.is_empty()
        || !profiles.is_empty()
        || !series_tags.is_empty()
        || !series_types.is_empty()
        || !include_specials;
    let mut v3q: v3::V3WantedQuery = typed::<v3::V3WantedQuery>(
        raw.as_deref(),
        &[
            "seriesIds",
            "qualityProfileIds",
            "seriesType",
            "seriesTags",
            "includeSubresources",
        ],
    )?
    .0;
    let (page, size) = (
        q["page"].as_u64().unwrap_or(1).max(1) as usize,
        q["pageSize"].as_u64().unwrap_or(10).max(1) as usize,
    );
    if filtered {
        v3q.page = Some(1);
        v3q.page_size = Some(100_000);
    }
    let native = v3q.native();
    let Json(p) = if cutoff {
        crate::routes::wanted::cutoff_unmet(State(state.clone()), Query(native)).await?
    } else {
        crate::routes::wanted::missing(State(state.clone()), Query(native)).await?
    };
    let mut body = v3::wanted_json(&state, &v3q, p).await?;
    let mut records: Vec<Value> = body["records"].as_array().cloned().unwrap_or_default();
    if filtered {
        let mut info: HashMap<i64, kitsunarr_domain::api::library::SeriesListItem> = HashMap::new();
        let mut kept = Vec::new();
        for r in records {
            let sid = r["seriesId"].as_i64().unwrap_or_default();
            if !include_specials && r["seasonNumber"] == json!(0) {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = info.entry(sid) {
                slot.insert(series::get_list_item(&state.db, sid).await?);
            }
            let s = &info[&sid];
            if (series_ids.is_empty() || series_ids.contains(&sid))
                && (profiles.is_empty() || profiles.contains(&s.quality_profile_id))
                && (series_tags.is_empty() || s.tags.iter().any(|t| series_tags.contains(t)))
                && (series_types.is_empty()
                    || series_types.contains(&s.series_type.as_str().to_string()))
            {
                kept.push(r);
            }
        }
        body["totalRecords"] = json!(kept.len());
        body["page"] = json!(page);
        body["pageSize"] = json!(size);
        records = kept
            .into_iter()
            .skip((page - 1) * size)
            .take(size)
            .collect();
    }
    let records = episodes_out(&state, records, &include).await?;
    body["records"] = json!(records);
    Ok(Json(body))
}

async fn wanted_missing(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    wanted(state, raw, false).await
}

async fn wanted_cutoff(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    wanted(state, raw, true).await
}

// ---- history --------------------------------------------------------------------------------

/// Sonarr v5's `HistoryResource`: `data` values are strings; series and episode on request.
async fn history_out(
    state: &AppState,
    mut list: Vec<Value>,
    include: &BTreeSet<String>,
) -> ApiResult<Vec<Value>> {
    let mut series_cache: HashMap<i64, Value> = HashMap::new();
    for h in list.iter_mut() {
        if let Some(Value::Object(data)) = h.get_mut("data") {
            data.retain(|_, v| !v.is_null());
        }
        let sid = h["seriesId"].as_i64().unwrap_or_default();
        if include.contains("series") && sid > 0 {
            if let std::collections::hash_map::Entry::Vacant(slot) = series_cache.entry(sid) {
                slot.insert(one_series(state, sid).await.unwrap_or(Value::Null));
            }
            h["series"] = series_cache[&sid].clone();
        }
        let eid = h["episodeId"].as_i64().unwrap_or_default();
        if include.contains("episode")
            && eid > 0
            && let Ok(Json(mut e)) = v3::get_episode(State(state.clone()), Path(eid)).await
        {
            episode_v5(&mut e);
            h["episode"] = e;
        }
    }
    Ok(list)
}

const HISTORY_MULTI: &[&str] = &[
    "includeSubresources",
    "seriesIds",
    "languages",
    "quality",
    "eventType",
];

async fn history(State(state): State<AppState>, RawQuery(raw): RawQuery) -> ApiResult<Json<Value>> {
    let include = subresources(raw.as_deref());
    let event_types = ids(raw.as_deref(), "eventType");
    let series_ids = ids(raw.as_deref(), "seriesIds");
    let mut q = typed::<v3::HistoryQuery>(raw.as_deref(), HISTORY_MULTI)?.0;
    let qv = query_value(raw.as_deref());
    let page = qv["page"].as_u64().unwrap_or(1).max(1) as usize;
    let size = qv["pageSize"].as_u64().unwrap_or(10).max(1) as usize;
    let mut body = if event_types.len() <= 1 && series_ids.is_empty() {
        q.event_type = event_types.first().map(|e| json!(e));
        v3::history(State(state.clone()), Query(q)).await?.0
    } else {
        // Several event types or series: filter every record, then page.
        let mut all = Vec::new();
        let names: Vec<Option<String>> = if event_types.is_empty() {
            vec![None]
        } else {
            event_types
                .iter()
                .map(|e| v3::event_name(&Some(json!(e))))
                .collect()
        };
        for name in names {
            all.extend(v3::filtered_history(&state, &q, name.as_deref()).await?);
        }
        all.retain(|h| {
            series_ids.is_empty()
                || series_ids.contains(&h["seriesId"].as_i64().unwrap_or_default())
        });
        all.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()));
        let total = all.len();
        let records: Vec<Value> = all.into_iter().skip((page - 1) * size).take(size).collect();
        json!({"page": page, "pageSize": size, "sortKey": "date", "sortDirection": "descending", "totalRecords": total, "records": records})
    };
    let records = body["records"].as_array().cloned().unwrap_or_default();
    body["records"] = json!(history_out(&state, records, &include).await?);
    Ok(Json(body))
}

async fn history_list(
    state: &AppState,
    raw: Option<&str>,
    keep: impl Fn(&Value) -> bool,
) -> ApiResult<Vec<Value>> {
    let include = subresources(raw);
    let q = typed::<v3::HistoryQuery>(raw, &["includeSubresources"])?.0;
    let event = v3::event_name(&q_event(raw));
    let mut list = v3::filtered_history(state, &q, event.as_deref()).await?;
    list.retain(|h| keep(h));
    history_out(state, list, &include).await
}

fn q_event(raw: Option<&str>) -> Option<Value> {
    let v = query_value(raw)["eventType"].clone();
    (!v.is_null()).then_some(v)
}

async fn history_since(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    if query_value(raw.as_deref())["date"].is_null() {
        return Err(ApiError::field("date", "date is required."));
    }
    Ok(Json(history_list(&state, raw.as_deref(), |_| true).await?))
}

async fn history_series(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    if query_value(raw.as_deref())["seriesId"].is_null() {
        return Err(ApiError::field("seriesId", "seriesId is required."));
    }
    Ok(Json(history_list(&state, raw.as_deref(), |_| true).await?))
}

/// `GET /history/season?seriesId=&seasonNumber=`.
async fn history_season(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = query_value(raw.as_deref());
    let (Some(sid), Some(season)) = (q["seriesId"].as_i64(), q["seasonNumber"].as_i64()) else {
        return Err(ApiError::field(
            "seriesId",
            "seriesId and seasonNumber are required.",
        ));
    };
    let in_season: BTreeSet<i64> = series::list_episodes(&state.db, sid, Some(season as i32))
        .await?
        .iter()
        .map(|e| e.id)
        .collect();
    Ok(Json(
        history_list(&state, raw.as_deref(), |h| {
            in_season.contains(&h["episodeId"].as_i64().unwrap_or_default())
        })
        .await?,
    ))
}

/// `GET /history/episode?episodeId=`.
async fn history_episode(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    if query_value(raw.as_deref())["episodeId"].is_null() {
        return Err(ApiError::field("episodeId", "episodeId is required."));
    }
    Ok(Json(history_list(&state, raw.as_deref(), |_| true).await?))
}

// ---- queue ----------------------------------------------------------------------------------

/// Season and file of episodes, by id.
async fn episode_facts(state: &AppState, ids: &[i64]) -> ApiResult<HashMap<i64, (i64, bool)>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT id, season_number, episode_file_id IS NOT NULL FROM episodes WHERE id IN ({placeholders})"
        );
        let mut q = kitsunarr_db::sqlx::query_as::<_, (i64, i64, bool)>(
            kitsunarr_db::sqlx::AssertSqlSafe(sql),
        );
        for id in chunk {
            q = q.bind(id);
        }
        for (id, season, has_file) in q
            .fetch_all(state.db.pool())
            .await
            .map_err(kitsunarr_db::DbError::from)?
        {
            out.insert(id, (season, has_file));
        }
    }
    Ok(out)
}

/// Sonarr v5's `QueueResource`s: one per download (v3 has one per episode).
pub(crate) async fn queue_v5(
    state: &AppState,
    records: Vec<Value>,
    include: &BTreeSet<String>,
) -> ApiResult<Vec<Value>> {
    let mut grouped: Vec<(Value, Vec<i64>)> = Vec::new();
    for r in records {
        let id = r["id"].clone();
        let episode = r["episodeId"].as_i64();
        match grouped.iter_mut().find(|(g, _)| g["id"] == id) {
            Some((_, eps)) => eps.extend(episode),
            None => grouped.push((r, episode.into_iter().collect())),
        }
    }
    let all: Vec<i64> = grouped
        .iter()
        .flat_map(|(_, e)| e.iter().copied())
        .collect();
    let facts = episode_facts(state, &all).await?;
    // The import state of each download (Tanukarr's `ImportProgressResource`), from the native
    // queue, which tracks it while a download is imported.
    let progress: std::collections::HashMap<String, Value> =
        crate::routes::queue::get_queue(State(state.clone()))
            .await?
            .0
            .items
            .into_iter()
            .filter_map(|i| {
                let p = serde_json::to_value(i.import_progress?).ok()?;
                Some((i.download_id.to_ascii_lowercase(), p))
            })
            .collect();
    let mut out = Vec::new();
    for (mut r, mut eps) in grouped {
        eps.sort_unstable();
        eps.dedup();
        let mut by_season: std::collections::BTreeMap<i64, Vec<i64>> = Default::default();
        for e in &eps {
            if let Some((season, _)) = facts.get(e) {
                by_season.entry(*season).or_default().push(*e);
            }
        }
        let with_files: Vec<i64> = eps
            .iter()
            .copied()
            .filter(|e| facts.get(e).is_some_and(|f| f.1))
            .collect();
        let title = r["title"].as_str().unwrap_or_default().to_string();
        let parsed = kitsunarr_parser::parse_title(&title);
        let quality = kitsunarr_parser::parse_quality(&title);
        if let Some(m) = r.as_object_mut() {
            m.remove("episodeId");
            let size_left = m.remove("sizeleft").unwrap_or(Value::Null);
            let time_left = m.remove("timeleft").unwrap_or(Value::Null);
            m.insert("sizeLeft".into(), size_left);
            m.insert("timeLeft".into(), time_left);
        }
        r["episodeIds"] = json!(eps);
        r["seasonNumbers"] = json!(if by_season.is_empty() {
            parsed
                .as_ref()
                .map(|p| p.season_numbers.clone())
                .unwrap_or_default()
                .into_iter()
                .map(i64::from)
                .collect::<Vec<_>>()
        } else {
            by_season.keys().copied().collect()
        });
        r["episodeIdsBySeason"] = json!(
            by_season
                .iter()
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect::<Map<String, Value>>()
        );
        r["episodeIdsWithFiles"] = json!(with_files);
        r["episodesWithFilesCount"] = json!(with_files.len());
        r["isFullSeason"] = json!(parsed.as_ref().is_some_and(|p| p.full_season));
        r["importProgress"] = r["downloadId"]
            .as_str()
            .and_then(|d| progress.get(&d.to_ascii_lowercase()).cloned())
            .unwrap_or(Value::Null);
        r["languages"] = json!(
            parsed
                .as_ref()
                .map(|p| p
                    .languages
                    .iter()
                    .map(|l| json!({"id": l.id, "name": l.name}))
                    .collect::<Vec<_>>())
                .unwrap_or_default()
        );
        r["quality"] = json!({
            "quality": v3::quality_json(quality.quality.id),
            "revision": {"version": quality.revision.version, "real": quality.revision.real, "isRepack": quality.revision.is_repack},
        });
        r["downloadClientHasPostImportCategory"] = json!(false);
        let sid = r["seriesId"].as_i64();
        if include.contains("series")
            && let Some(sid) = sid
        {
            r["series"] = one_series(state, sid).await.unwrap_or(Value::Null);
        }
        if include.contains("episodes") {
            let mut list = Vec::new();
            for e in &eps {
                if let Ok(Json(mut v)) = v3::get_episode(State(state.clone()), Path(*e)).await {
                    episode_v5(&mut v);
                    list.push(v);
                }
            }
            r["episodes"] = json!(list);
        }
        out.push(r);
    }
    Ok(out)
}

async fn queue(State(state): State<AppState>, RawQuery(raw): RawQuery) -> ApiResult<Json<Value>> {
    let q = query_value(raw.as_deref());
    let include = subresources(raw.as_deref());
    let series_ids = ids(raw.as_deref(), "seriesIds");
    let mut list = queue_v5(&state, v3::queue_records(&state).await?, &include).await?;
    if q["includeUnknownSeriesItems"].as_bool() == Some(false) {
        list.retain(|r| r["seriesId"].as_i64().is_some());
    }
    if !series_ids.is_empty() {
        list.retain(|r| {
            r["seriesId"]
                .as_i64()
                .is_some_and(|s| series_ids.contains(&s))
        });
    }
    if let Some(p) = q["protocol"].as_str() {
        list.retain(|r| {
            r["protocol"]
                .as_str()
                .is_some_and(|x| x.eq_ignore_ascii_case(p))
        });
    }
    let total = list.len();
    let page = q["page"].as_u64().unwrap_or(1).max(1) as usize;
    let size = q["pageSize"].as_u64().unwrap_or(10).max(1) as usize;
    let records: Vec<Value> = list
        .into_iter()
        .skip((page - 1) * size)
        .take(size)
        .collect();
    Ok(Json(json!({
        "page": page, "pageSize": size, "sortKey": "timeleft", "sortDirection": "ascending",
        "totalRecords": total, "records": records,
    })))
}

async fn queue_details(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = query_value(raw.as_deref());
    let include = subresources(raw.as_deref());
    let episodes = ids(raw.as_deref(), "episodeIds");
    let records = v3::queue_records(&state).await?;
    let list = queue_v5(&state, records, &include).await?;
    Ok(Json(
        list.into_iter()
            .filter(|r| {
                q["seriesId"]
                    .as_i64()
                    .is_none_or(|s| r["seriesId"].as_i64() == Some(s))
            })
            .filter(|r| {
                episodes.is_empty()
                    || r["episodeIds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_i64)
                        .any(|e| episodes.contains(&e))
            })
            .collect(),
    ))
}

// ---- releases -------------------------------------------------------------------------------

/// Sonarr v5's nested `ReleaseResource` from a v3 release.
pub(crate) fn release_v5(r: &Value) -> Value {
    let rejections: Vec<Value> = r["rejections"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| json!({"reason": "unknown", "message": m, "type": "permanent"}))
        .collect();
    json!({
        "id": 0,
        "parsedInfo": {
            "releaseTitle": r["title"],
            "seriesTitle": r["seriesTitle"],
            "quality": r["quality"],
            "seasonNumber": r["seasonNumber"],
            "seasonNumbers": if r["seasonNumber"].is_null() { json!([]) } else { json!([r["seasonNumber"]]) },
            "episodeNumbers": r["episodeNumbers"],
            "absoluteEpisodeNumbers": r["absoluteEpisodeNumbers"],
            "airDate": r.get("airDate").cloned().unwrap_or(Value::Null),
            "releaseGroup": r["releaseGroup"],
            "releaseHash": r.get("releaseHash").cloned().unwrap_or(Value::Null),
            "fullSeason": r["fullSeason"],
            "special": r["special"],
            "isDaily": false,
            "isAbsoluteNumbering": r["absoluteEpisodeNumbers"].as_array().is_some_and(|a| !a.is_empty()),
            "isPossibleSpecialEpisode": false,
        },
        "release": {
            "guid": r["guid"], "age": r["age"], "ageHours": r["ageHours"], "ageMinutes": r["ageMinutes"],
            "size": r["size"], "indexerId": r["indexerId"], "indexer": r["indexer"], "title": r["title"],
            "tvdbId": 0, "tvRageId": 0, "imdbId": null, "publishDate": r["publishDate"],
            "commentUrl": null, "downloadUrl": null, "infoUrl": r["infoUrl"], "magnetUrl": null,
            "infoHash": null, "seeders": r["seeders"], "leechers": r["leechers"],
            "protocol": r["protocol"], "indexerFlags": r["indexerFlags"],
        },
        "decision": {
            "approved": r["approved"],
            "temporarilyRejected": r["temporarilyRejected"],
            "rejected": r["rejected"],
            "rejections": rejections,
        },
        "history": null,
        "qualityWeight": r["qualityWeight"],
        "languages": r["languages"],
        "mappedSeasonNumber": r["seasonNumber"],
        "mappedEpisodeNumbers": r["episodeNumbers"],
        "mappedAbsoluteEpisodeNumbers": r["absoluteEpisodeNumbers"],
        "mappedSeriesId": r["seriesId"],
        "mappedEpisodeInfo": r["mappedEpisodeInfo"],
        "episodeRequested": r["approved"],
        "downloadAllowed": r["downloadAllowed"],
        "releaseWeight": 0,
        "customFormats": r["customFormats"],
        "customFormatScore": r["customFormatScore"],
        "formatScores": [],
        "sceneMapping": null,
    })
}

async fn releases(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let q = query_value(raw.as_deref());
    let complete = q["completeSeries"].as_bool().unwrap_or(false);
    let packs = q["seasonPacksOnly"].as_bool().unwrap_or(false);
    let list = if complete || packs {
        let series_id = q["seriesId"]
            .as_i64()
            .ok_or_else(|| ApiError::field("seriesId", "A series search needs a series id."))?;
        v3_more::release_records(
            &state,
            series_id,
            q["seasonNumber"].as_i64().map(|s| s as i32),
            None,
            complete,
            packs,
        )
        .await?
    } else {
        let p = typed::<v3_more::ReleaseParams>(raw.as_deref(), &[])?;
        v3_more::search(State(state.clone()), p).await?.0
    };
    let mut out: Vec<Value> = list.iter().map(release_v5).collect();
    release_scores_and_history(&state, &mut out).await?;
    Ok(Json(out))
}

/// (event type, date, release guid, download id).
type ReleaseEvent = (String, String, Option<String>, Option<String>);

/// Sonarr v5's `formatScores` (each matched custom format with its score in the series'
/// profile) and `history` (when the release was grabbed, and when that download failed).
async fn release_scores_and_history(state: &AppState, list: &mut [Value]) -> ApiResult<()> {
    let formats = kitsunarr_db::quality::list_custom_formats(state.db.pool()).await?;
    let mut profiles: std::collections::HashMap<
        i64,
        kitsunarr_domain::api::settings::QualityProfile,
    > = Default::default();
    // (series id) → [(event, date, guid, download id)]
    let mut histories: std::collections::HashMap<i64, Vec<ReleaseEvent>> = Default::default();
    for r in list.iter_mut() {
        let Some(sid) = r["release"]["mappedSeriesId"]
            .as_i64()
            .or_else(|| r["release"]["seriesId"].as_i64())
            .or_else(|| r["mappedSeriesId"].as_i64())
        else {
            continue;
        };
        if !profiles.contains_key(&sid)
            && let Ok(d) = series::get_detail(&state.db, sid).await
            && let Ok(p) =
                kitsunarr_db::quality::get_profile(state.db.pool(), d.item.quality_profile_id).await
        {
            profiles.insert(sid, p);
        }
        if let Some(profile) = profiles.get(&sid) {
            let view = kitsunarr_decision::profile::ProfileView(profile);
            let scores: Vec<Value> = r["customFormats"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|cf| {
                    let name = cf["name"].as_str()?;
                    let f = formats.iter().find(|f| f.name == name)?;
                    Some(json!({"id": f.id, "name": f.name, "score": view.format_score(&[f.id])}))
                })
                .collect();
            r["formatScores"] = json!(scores);
        }
        if let std::collections::hash_map::Entry::Vacant(e) = histories.entry(sid) {
            e.insert(kitsunarr_db::history::grab_and_failure_events(&state.db, sid).await?);
        }
        let guid = r["release"]["guid"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let rows = &histories[&sid];
        let grabbed = rows.iter().find(|(e, _, g, _)| {
            e == "grabbed" && !guid.is_empty() && g.as_deref() == Some(guid.as_str())
        });
        r["history"] = match grabbed {
            Some((_, date, _, download)) => {
                let failed = download.as_ref().and_then(|d| {
                    rows.iter()
                        .find(|(e, _, _, dl)| e == "downloadFailed" && dl.as_ref() == Some(d))
                        .map(|(_, date, _, _)| date.clone())
                });
                json!({"grabbed": date, "failed": failed})
            }
            None => Value::Null,
        };
    }
    Ok(())
}

/// `POST /release`: Sonarr v5's `ReleaseGrabResource` (`override` picks the episodes).
async fn grab_release(
    State(state): State<AppState>,
    Json(b): Json<Value>,
) -> ApiResult<Json<Value>> {
    let guid = b["guid"]
        .as_str()
        .ok_or_else(|| ApiError::field("guid", "guid is required."))?
        .to_string();
    let indexer_id = b["indexerId"]
        .as_i64()
        .ok_or_else(|| ApiError::field("indexerId", "indexerId is required."))?;
    let episode_ids: Vec<i64> = b["override"]["episodeIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .collect();
    let _ = crate::routes::grab::grab_release(
        State(state),
        Json(kitsunarr_domain::api::search::GrabRequest {
            guid,
            indexer_id,
            episode_ids,
        }),
    )
    .await?;
    Ok(Json(b))
}

async fn push_release(
    State(state): State<AppState>,
    body: Json<v3::PushBody>,
) -> ApiResult<Json<Value>> {
    let Json(list) = v3::push_release(State(state), body).await?;
    let r = list.into_iter().next().unwrap_or(Value::Null);
    let mut v = release_v5(&r);
    v["release"]["downloadUrl"] = r["downloadUrl"].clone();
    v["decision"]["rejections"] = json!(
        r["rejections"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| json!({"reason": "unknown", "message": m, "type": "permanent"}))
            .collect::<Vec<_>>()
    );
    Ok(Json(v))
}

/// Pushed releases are processed at once and not kept, so none can be read back.
async fn pushed_release(Path(_id): Path<i64>) -> ApiError {
    ApiError::NotFound("Pushed releases are not kept.".into())
}

/// `GET /parse`: v5's `ParsedEpisodeInfo` is the parser's whole model.
async fn parse(State(state): State<AppState>, RawQuery(raw): RawQuery) -> ApiResult<Json<Value>> {
    let p = typed::<v3_more::ParseParams>(raw.as_deref(), &[])?;
    let Json(mut v) = v3_more::parse(State(state.clone()), p).await?;
    let title = v["title"].as_str().unwrap_or_default().to_string();
    if let Some(parsed) = kitsunarr_parser::parse_title(&title) {
        let info = &mut v["parsedEpisodeInfo"];
        info["seasonNumber"] = json!(parsed.season_numbers.first());
        info["specialAbsoluteEpisodeNumbers"] = json!(parsed.special_absolute_episode_numbers);
        info["isSplitEpisode"] = json!(parsed.is_split_episode);
        info["isMiniSeries"] = json!(parsed.is_mini_series);
        info["isSeasonTitle"] = json!(parsed.is_season_title);
        info["seasonPart"] = json!(parsed.season_part);
        info["releaseTokens"] = json!(parsed.release_tokens);
        info["dailyPart"] = json!(parsed.daily_part);
        info["isPossibleSceneSeasonSpecial"] = json!(parsed.is_possible_scene_season_special());
        info["releaseType"] = json!(release_type_name(match parsed.release_type() {
            kitsunarr_parser::ReleaseType::SingleEpisode => 1,
            kitsunarr_parser::ReleaseType::MultiEpisode => 2,
            kitsunarr_parser::ReleaseType::SeasonPack => 3,
            _ => 0,
        }));
        info["isCompleteSeries"] = json!(false);
        info["isMovie"] = json!(false);
        v["languages"] = info["languages"].clone();
    }
    if let Some(m) = v.as_object_mut() {
        m.entry("customFormats").or_insert(json!([]));
        m.entry("customFormatScore").or_insert(json!(0));
        m.entry("languages").or_insert(json!([]));
    }
    if let Some(Value::Array(eps)) = v.get_mut("episodes") {
        eps.iter_mut().for_each(episode_v5);
    }
    if v["series"].is_object() {
        let s = v["series"].take();
        v["series"] = series_out(&state, vec![s], false).await?.remove(0);
    }
    Ok(Json(v))
}

// ---- release profiles -------------------------------------------------------------------------

/// Sonarr v5's `ReleaseProfileResource`: several indexers (`indexerIds`).
fn release_profile_v5(r: &kitsunarr_domain::api::settings::ReleaseProfile) -> Value {
    json!({
        "id": r.id, "name": r.name, "enabled": r.enabled, "required": r.required,
        "ignored": r.ignored, "airDateRestriction": r.air_date_restriction,
        "airDateGracePeriod": r.air_date_grace_period,
        "allowSeasonPackWithoutAllEpisodesAired": r.allow_season_pack_without_all_episodes_aired,
        "indexerIds": r.indexer_ids, "tags": r.tags, "excludedTags": r.excluded_tags,
    })
}

/// Sonarr v5's `RuleFor(d => d.AirDateGracePeriod).NotNull()`.
fn check_grace_period_v5(v: &Value) -> ApiResult<()> {
    if v["airDateGracePeriod"].is_null() {
        return Err(ApiError::field(
            "airDateGracePeriod",
            "'Air Date Grace Period' must not be empty.",
        ));
    }
    Ok(())
}

fn release_profile_from_v5(v: &Value, id: i64) -> kitsunarr_domain::api::settings::ReleaseProfile {
    let mut p = v3_settings::release_profile_from(v, id);
    p.indexer_ids = v["indexerIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .filter(|i| *i > 0)
        .collect();
    p
}

async fn list_release_profiles(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        kitsunarr_db::rules::list_release_profiles(state.db.pool())
            .await?
            .iter()
            .map(release_profile_v5)
            .collect(),
    ))
}

async fn get_release_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(release_profile_v5(
        &v3_settings::find_release_profile(&state, id).await?,
    )))
}

async fn create_release_profile(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    check_grace_period_v5(&v)?;
    let (_, Json(saved)) =
        crate::routes::rules::create_release(State(state), Json(release_profile_from_v5(&v, 0)))
            .await?;
    Ok((StatusCode::CREATED, Json(release_profile_v5(&saved))))
}

async fn update_release_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    v3_settings::find_release_profile(&state, id).await?;
    check_grace_period_v5(&v)?;
    let Json(saved) = crate::routes::rules::update_release(
        State(state),
        Path(id),
        Json(release_profile_from_v5(&v, id)),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(release_profile_v5(&saved))))
}

// ---- statistics -------------------------------------------------------------------------------

/// Sonarr v5's `StatisticsFilter` as a `WHERE` condition on `series s` with its values.
fn statistics_filter(raw: Option<&str>) -> (String, Vec<Value>) {
    let q = query_value(raw);
    let not = |k: &str| q[k].as_bool().unwrap_or(false);
    let multi = |k: &str| -> Vec<String> {
        raw.unwrap_or_default()
            .split('&')
            .filter_map(|p| p.split_once('='))
            .filter(|(key, _)| {
                key.eq_ignore_ascii_case(k) || key.eq_ignore_ascii_case(&format!("{k}[]"))
            })
            .flat_map(|(_, v)| {
                let v = v3_extra::query_json(Some(&format!("x={v}")))["x"].clone();
                let text = match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                text.split(',').map(str::to_string).collect::<Vec<_>>()
            })
            .filter(|s| !s.is_empty())
            .collect()
    };
    let mut conditions = Vec::new();
    let mut binds = Vec::new();
    let group = |parts: Vec<String>, negate: bool| {
        let g = format!("({})", parts.join(" OR "));
        if negate { format!("NOT {g}") } else { g }
    };
    let paths = multi("rootFolderPaths");
    if !paths.is_empty() {
        let mut parts = Vec::new();
        for p in paths {
            let sep = if p.contains('\\') { '\\' } else { '/' };
            let prefix = format!("{}{sep}", p.trim_end_matches(['/', '\\']));
            parts.push("SUBSTR(s.path, 1, ?) = ?".to_string());
            binds.push(json!(prefix.chars().count()));
            binds.push(json!(prefix));
        }
        conditions.push(group(parts, not("rootFolderPathsNot")));
    }
    let tags = multi("tagIds");
    if !tags.is_empty() {
        let mut parts = Vec::new();
        for t in tags {
            parts.push(
                "EXISTS (SELECT 1 FROM series_tags st WHERE st.series_id = s.id AND st.tag_id = ?)"
                    .to_string(),
            );
            binds.push(json!(t.parse::<i64>().unwrap_or(-1)));
        }
        conditions.push(group(parts, not("tagIdsNot")));
    }
    let profiles = multi("qualityProfileIds");
    if !profiles.is_empty() {
        let mut parts = Vec::new();
        for p in profiles {
            parts.push("s.quality_profile_id = ?".to_string());
            binds.push(json!(p.parse::<i64>().unwrap_or(-1)));
        }
        conditions.push(group(parts, not("qualityProfileIdsNot")));
    }
    if let Some(m) = q["monitored"].as_bool() {
        conditions.push("s.monitored = ?".into());
        binds.push(json!(m));
    }
    let types = multi("seriesTypes");
    if !types.is_empty() {
        let mut parts = Vec::new();
        for t in types {
            let t = match t.as_str() {
                "0" => "standard".to_string(),
                "1" => "daily".to_string(),
                "2" => "anime".to_string(),
                other => other.to_ascii_lowercase(),
            };
            parts.push("s.series_type = ?".to_string());
            binds.push(json!(t));
        }
        conditions.push(group(parts, not("seriesTypesNot")));
    }
    let condition = if conditions.is_empty() {
        "1 = 1".to_string()
    } else {
        conditions.join(" AND ")
    };
    (condition, binds)
}

type Qa<'q, O> = kitsunarr_db::sqlx::query::QueryAs<
    'q,
    kitsunarr_db::sqlx::Sqlite,
    O,
    kitsunarr_db::sqlx::sqlite::SqliteArguments,
>;

fn bind_all<'q, O>(mut q: Qa<'q, O>, binds: &'q [Value]) -> Qa<'q, O> {
    for b in binds {
        q = match b {
            Value::Number(n) => q.bind(n.as_i64().unwrap_or_default()),
            Value::Bool(v) => q.bind(*v),
            Value::String(s) => q.bind(s.as_str()),
            _ => q.bind(Option::<String>::None),
        };
    }
    q
}

/// `GET /statistics`: Sonarr v5's library statistics (Tanukarr's `StatisticsRepository`).
async fn statistics(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    use kitsunarr_db::sqlx;
    let (cond, binds) = statistics_filter(raw.as_deref());
    let pool = state.db.pool();
    let now = kitsunarr_db::now();
    let map_err = |e: sqlx::Error| ApiError::from(kitsunarr_db::DbError::from(e));
    let series: (i64, i64, i64, i64, i64, i64, i64, i64, i64) = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*), COALESCE(SUM(s.monitored), 0), \
             COALESCE(SUM(s.status = 'continuing'), 0), COALESCE(SUM(s.status = 'ended'), 0), \
             COALESCE(SUM(s.status = 'upcoming'), 0), COALESCE(SUM(s.status = 'deleted'), 0), \
             COALESCE(SUM(s.series_type = 'standard'), 0), COALESCE(SUM(s.series_type = 'daily'), 0), \
             COALESCE(SUM(s.series_type = 'anime'), 0) FROM series s WHERE {cond}"
        ))),
        &binds,
    )
    .fetch_one(pool)
    .await
    .map_err(map_err)?;
    let progress = |group: &str, extra: &str| {
        format!(
            "SELECT SUM(CASE WHEN (e.monitored = 1 AND e.air_date_utc <= '{now}') OR e.episode_file_id IS NOT NULL THEN 1 ELSE 0 END) AS c, \
             SUM(CASE WHEN e.episode_file_id IS NOT NULL THEN 1 ELSE 0 END) AS f \
             FROM episodes e JOIN series s ON s.id = e.series_id WHERE {cond} {extra} GROUP BY {group}"
        )
    };
    let completed_series: (i64,) = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT COALESCE(SUM(CASE WHEN c > 0 AND c = f THEN 1 ELSE 0 END), 0) FROM ({})",
            progress("e.series_id", "")
        ))),
        &binds,
    )
    .fetch_one(pool)
    .await
    .map_err(map_err)?;
    let seasons: (i64, i64) = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN c > 0 AND c = f THEN 1 ELSE 0 END), 0) FROM ({})",
            progress("e.series_id, e.season_number", "AND e.season_number > 0")
        ))),
        &binds,
    )
    .fetch_one(pool)
    .await
    .map_err(map_err)?;
    let episodes: (i64, i64, i64, i64, i64) = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*), COALESCE(SUM(e.monitored), 0), \
             COALESCE(SUM(e.episode_file_id IS NOT NULL), 0), \
             COALESCE(SUM(e.episode_file_id IS NULL AND e.monitored = 1 AND s.monitored = 1 AND e.air_date_utc <= '{now}'), 0), \
             COALESCE(SUM(e.episode_file_id IS NULL AND (e.air_date_utc IS NULL OR e.air_date_utc > '{now}')), 0) \
             FROM episodes e JOIN series s ON s.id = e.series_id WHERE {cond}"
        ))),
        &binds,
    )
    .fetch_one(pool)
    .await
    .map_err(map_err)?;
    let files: (i64, i64) = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*), COALESCE(SUM(f.size), 0) FROM episode_files f JOIN series s ON s.id = f.series_id WHERE {cond}"
        ))),
        &binds,
    )
    .fetch_one(pool)
    .await
    .map_err(map_err)?;
    let profile_series: Vec<(i64, String, i64)> = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT p.id, p.name, COUNT(s.id) FROM quality_profiles p LEFT JOIN series s ON s.quality_profile_id = p.id AND {cond} \
             GROUP BY p.id, p.name ORDER BY p.name"
        ))),
        &binds,
    )
    .fetch_all(pool)
    .await
    .map_err(map_err)?;
    let profile_files: Vec<(i64, i64, i64)> = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT s.quality_profile_id, COUNT(*), COALESCE(SUM(f.size), 0) FROM episode_files f JOIN series s ON s.id = f.series_id \
             WHERE {cond} GROUP BY s.quality_profile_id"
        ))),
        &binds,
    )
    .fetch_all(pool)
    .await
    .map_err(map_err)?;
    let qualities: Vec<(i64, i64, i64)> = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT f.quality_id, COUNT(*), COALESCE(SUM(f.size), 0) FROM episode_files f JOIN series s ON s.id = f.series_id \
             WHERE {cond} GROUP BY f.quality_id ORDER BY f.quality_id"
        ))),
        &binds,
    )
    .fetch_all(pool)
    .await
    .map_err(map_err)?;
    let tag_series: Vec<(i64, String, i64)> = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT t.id, t.label, COUNT(s.id) FROM tags t LEFT JOIN series_tags st ON st.tag_id = t.id \
             LEFT JOIN series s ON s.id = st.series_id AND {cond} GROUP BY t.id, t.label ORDER BY t.label"
        ))),
        &binds,
    )
    .fetch_all(pool)
    .await
    .map_err(map_err)?;
    let tag_files: Vec<(i64, i64, i64)> = bind_all(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT st.tag_id, COUNT(*), COALESCE(SUM(f.size), 0) FROM episode_files f JOIN series s ON s.id = f.series_id \
             JOIN series_tags st ON st.series_id = s.id WHERE {cond} GROUP BY st.tag_id"
        ))),
        &binds,
    )
    .fetch_all(pool)
    .await
    .map_err(map_err)?;
    Ok(Json(json!({
        "id": 0,
        "seriesCount": series.0,
        "monitoredSeriesCount": series.1,
        "completedSeriesCount": completed_series.0,
        "continuingSeriesCount": series.2,
        "endedSeriesCount": series.3,
        "upcomingSeriesCount": series.4,
        "deletedSeriesCount": series.5,
        "standardSeriesCount": series.6,
        "dailySeriesCount": series.7,
        "animeSeriesCount": series.8,
        "seasonCount": seasons.0,
        "completedSeasonCount": seasons.1,
        "totalEpisodeCount": episodes.0,
        "monitoredEpisodeCount": episodes.1,
        "downloadedEpisodeCount": episodes.2,
        "missingEpisodeCount": episodes.3,
        "unairedEpisodeCount": episodes.4,
        "episodeFileCount": files.0,
        "sizeOnDisk": files.1,
        "qualityProfileStatistics": profile_series.iter().map(|(id, name, count)| {
            let f = profile_files.iter().find(|(p, _, _)| p == id);
            json!({"qualityProfileId": id, "name": name, "seriesCount": count,
                   "episodeFileCount": f.map_or(0, |f| f.1), "sizeOnDisk": f.map_or(0, |f| f.2)})
        }).collect::<Vec<_>>(),
        "qualityStatistics": qualities.iter().map(|(q, count, size)| json!({
            "quality": v3::quality_json(*q as i32), "episodeFileCount": count, "sizeOnDisk": size,
        })).collect::<Vec<_>>(),
        "tagStatistics": tag_series.iter().map(|(id, label, count)| {
            let f = tag_files.iter().find(|(t, _, _)| t == id);
            json!({"tagId": id, "label": label, "seriesCount": count,
                   "episodeFileCount": f.map_or(0, |f| f.1), "sizeOnDisk": f.map_or(0, |f| f.2)})
        }).collect::<Vec<_>>(),
    })))
}

// ---- library tools --------------------------------------------------------------------------

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"message": message}))).into_response()
}

/// Tanukarr's `ProOptions` from Kitsunarr's library tools settings and per-series options.
async fn library_tools_options(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let Json(s) = crate::routes::library_tools::get_settings(State(state.clone())).await;
    let mut v = serde_json::to_value(s).map_err(ApiError::internal)?;
    let mut per_series = Map::new();
    for (id, _) in series::paths(&state.db).await? {
        if let Ok(Json(o)) =
            crate::routes::library_tools::get_series_naming(State(state.clone()), Path(id)).await
            && (o.renaming_mode != "default" || o.keep_versions || !o.season_titles.is_empty())
        {
            per_series.insert(id.to_string(), series_options_json(&o));
        }
    }
    v["series"] = Value::Object(per_series);
    Ok(Json(v))
}

async fn save_library_tools_options(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let Json(current) = crate::routes::library_tools::get_settings(State(state.clone())).await;
    let mut v = serde_json::to_value(current).map_err(ApiError::internal)?;
    for k in [
        "automationPaused",
        "hardlinkOnly",
        "matchExternalIds",
        "preferAnimeSeasonPacks",
    ] {
        if let Some(b) = body.get(k).filter(|b| b.is_boolean()) {
            v[k] = b.clone();
        }
    }
    let next = serde_json::from_value(v).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let _ = crate::routes::library_tools::put_settings(State(state.clone()), Json(next)).await?;
    library_tools_options(State(state)).await
}

/// Tanukarr's `ProSeriesOptions`.
fn series_options_json(o: &kitsunarr_domain::api::library_tools::SeriesNamingOptions) -> Value {
    json!({
        "automaticRenaming": o.renaming_mode,
        "keepVersions": o.keep_versions,
        "seasonTitles": o.season_titles.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<Map<String, Value>>(),
    })
}

async fn series_tool_options(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let Json(o) = crate::routes::library_tools::get_series_naming(State(state), Path(id)).await?;
    Ok(Json(series_options_json(&o)))
}

async fn save_series_tool_options(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let mode = body["automaticRenaming"].as_str().unwrap_or_default();
    let titles: Option<std::collections::BTreeMap<i32, String>> =
        body["seasonTitles"].as_object().map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    Some((k.parse().ok()?, v.as_str().unwrap_or_default().to_string()))
                })
                .collect()
        });
    let Some(titles) = titles.filter(|_| matches!(mode, "default" | "manual" | "automatic")) else {
        return Ok(bad_request(
            "Choose a valid naming option and season titles of at most 200 characters.",
        ));
    };
    let options = kitsunarr_domain::api::library_tools::SeriesNamingOptions {
        renaming_mode: mode.to_string(),
        season_titles: titles,
        keep_versions: body["keepVersions"].as_bool().unwrap_or(false),
    };
    match crate::routes::library_tools::put_series_naming(State(state), Path(id), Json(options))
        .await
    {
        Ok(Json(o)) => Ok(Json(series_options_json(&o)).into_response()),
        Err(ApiError::Validation(_)) => Ok(bad_request(
            "Choose a valid naming option and season titles of at most 200 characters.",
        )),
        Err(e) => Err(e),
    }
}

async fn tool_searches(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let Json(s) = crate::routes::library_tools::searches(State(state)).await;
    Ok(Json(serde_json::to_value(s).map_err(ApiError::internal)?))
}

async fn tool_storage(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let Json(list) = crate::routes::library_tools::storage(State(state)).await?;
    Ok(Json(
        list.into_iter()
            .map(|s| {
                let mut v = serde_json::to_value(&s).unwrap_or_default();
                if let Some(m) = v.as_object_mut() {
                    let path = m.remove("rootFolder").unwrap_or(Value::Null);
                    m.insert("path".into(), path);
                    m.entry("error").or_insert(Value::Null);
                }
                v
            })
            .collect(),
    ))
}

async fn tool_audit(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let Json(c) = crate::routes::library_tools::audit(State(state)).await?;
    Ok(Json(json!({
        "checkedAt": c.checked_at,
        "seriesChecked": c.series_checked,
        "filesChecked": c.files_checked,
        "issues": c.issues.iter().filter(|i| i.movie_id.is_none()).map(|i| json!({
            "seriesId": i.series_id, "title": i.series_title, "path": i.path, "reason": i.problem,
        })).collect::<Vec<_>>(),
    })))
}

async fn tool_keys(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let Json(keys) = crate::routes::library_tools::list_keys(State(state)).await?;
    Ok(Json(
        serde_json::to_value(keys).map_err(ApiError::internal)?,
    ))
}

async fn create_tool_key(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let request = kitsunarr_domain::api::library_tools::CompanionKeyCreate {
        name: body["name"].as_str().unwrap_or_default().to_string(),
        permission: body["permission"]
            .as_str()
            .unwrap_or("read-library")
            .to_string(),
        days: body["days"].as_i64().unwrap_or(90),
    };
    match crate::routes::library_tools::create_key(State(state), Json(request)).await {
        Ok((_, Json(created))) => {
            Ok(Json(serde_json::to_value(created).map_err(ApiError::internal)?).into_response())
        }
        Err(ApiError::Validation(f)) => Ok(bad_request(
            &f.first().map_or_else(String::new, |f| f.message.clone()),
        )),
        Err(ApiError::Conflict(m)) => Ok(bad_request(&m)),
        Err(e) => Err(e),
    }
}

async fn revoke_tool_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    match crate::routes::library_tools::delete_key(State(state), Path(id)).await {
        Ok(_) | Err(ApiError::NotFound(_)) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(e),
    }
}

async fn tool_recycle(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let Json(bin) = crate::routes::library_tools::recycle(State(state)).await?;
    Ok(Json(json!({
        "configured": bin.configured,
        "path": bin.path,
        "truncated": bin.truncated,
        "files": bin.files.iter().map(|f| json!({
            "token": f.token, "name": f.relative_path, "size": f.size, "recycledAt": f.recycled_at,
        })).collect::<Vec<_>>(),
    })))
}

async fn tool_recycle_restore(
    State(state): State<AppState>,
    Json(r): Json<kitsunarr_domain::api::library_tools::RecycleRestoreRequest>,
) -> ApiResult<Response> {
    match crate::routes::library_tools::restore(State(state), Json(r)).await {
        Ok(_) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({"commandId": null, "message": "Restore queued safely with other file operations."})),
        )
            .into_response()),
        Err(ApiError::Conflict(m)) => Ok((StatusCode::CONFLICT, Json(json!({"message": m}))).into_response()),
        Err(ApiError::NotFound(m)) => Err(ApiError::NotFound(m)),
        Err(e) => Ok(bad_request(&format!("Restore failed: {}", e.message()))),
    }
}

async fn tool_versions(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<Value>>> {
    let Json(list) = crate::routes::library_tools::versions(State(state), Path(id)).await?;
    Ok(Json(
        list.iter()
            .map(|v| {
                json!({
                    "id": v.id, "created": v.created, "path": v.relative_path, "available": v.available,
                    "metadata": {
                        "size": v.size, "releaseGroup": v.release_group,
                        "quality": {"quality": v3::quality_json(v.quality_id), "revision": {"version": 1, "real": 0, "isRepack": false}},
                    },
                    "episodeIds": v.episode_ids,
                })
            })
            .collect(),
    ))
}

async fn tool_activate_version(
    State(state): State<AppState>,
    Path((id, version)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    match crate::routes::library_tools::activate_version(State(state), Path((id, version))).await {
        Ok(_) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({"commandId": null, "message": "Version change queued safely with other file operations."})),
        )
            .into_response()),
        Err(ApiError::Conflict(m)) => Ok((StatusCode::CONFLICT, Json(json!({"message": m}))).into_response()),
        Err(e) => Err(e),
    }
}

/// A token identifying a list's current items (Tanukarr's preview token).
fn preview_token(entries: &[kitsunarr_domain::api::import_lists::ImportListEntry]) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    for e in entries {
        h.update(e.title.as_bytes());
        h.update(
            format!(
                "{:?}{:?}{:?}{:?}",
                e.tvdb_id, e.tmdb_id, e.ani_list_id, e.mal_id
            )
            .as_bytes(),
        );
    }
    h.finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn list_entries(
    state: &AppState,
    id: i64,
) -> ApiResult<Vec<kitsunarr_domain::api::import_lists::ImportListEntry>> {
    kitsunarr_db::providers::get(
        state.db.pool(),
        kitsunarr_domain::api::settings::ProviderKind::ImportList,
        id,
    )
    .await?;
    let Json(list) = crate::routes::import_lists::entries(
        State(state.clone()),
        Query(crate::routes::import_lists::EntriesQuery {
            import_list_id: Some(id),
        }),
    )
    .await?;
    Ok(list)
}

/// `GET /librarytools/lists/{id}`: the shows of the list's last read, numbered for adding.
async fn list_preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let entries = list_entries(&state, id).await?;
    let status = kitsunarr_db::import_lists::status(state.db.pool(), id).await?;
    Ok(Json(json!({
        "token": preview_token(&entries),
        "partial": status.last_error.is_some(),
        "items": entries.iter().enumerate().map(|(i, e)| {
            let source = if e.tvdb_id.is_some() { Some("tvdb") } else if e.ani_list_id.is_some() { Some("aniList") }
                else if e.mal_id.is_some() { Some("myAnimeList") } else if e.tmdb_id.is_some() { Some("tmdb") } else { None };
            let foreign = match source {
                Some("tvdb") => e.tvdb_id, Some("aniList") => e.ani_list_id, Some("myAnimeList") => e.mal_id,
                Some("tmdb") => e.tmdb_id, _ => None,
            };
            json!({
                "key": i, "title": e.title, "year": e.year, "tvdbId": e.tvdb_id, "tmdbId": e.tmdb_id,
                "aniListId": e.ani_list_id, "malId": e.mal_id, "imdbId": e.imdb_id,
                "metadataSource": source, "foreignId": foreign.map(|f| f.to_string()),
                "alreadyAdded": e.series_id.is_some(), "excluded": e.excluded,
                "supported": source.is_some() || e.imdb_id.is_some(),
            })
        }).collect::<Vec<_>>(),
    })))
}

/// Kitsunarr keeps a list's last read rather than a preview cache: nothing to clear.
async fn clear_list_preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    list_entries(&state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /librarytools/lists/{id}/add`: adds chosen shows of the preview.
async fn add_from_list(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(b): Json<Value>,
) -> ApiResult<Response> {
    let entries = list_entries(&state, id).await?;
    let keys: Vec<usize> = b["keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .map(|k| k as usize)
        .collect();
    let roots = crate::routes::settings::list_root_folders(State(state.clone()))
        .await?
        .0;
    let root = roots
        .iter()
        .find(|r| Some(r.id) == b["rootFolderId"].as_i64());
    let invalid = b["token"].as_str() != Some(preview_token(&entries).as_str())
        || root.is_none()
        || keys.is_empty()
        || keys.len() > 25
        || keys.iter().any(|k| *k >= entries.len());
    if invalid {
        return Ok(bad_request(
            "Refresh the preview, choose a valid folder and profile, and select up to 25 shows at a time.",
        ));
    }
    let request = kitsunarr_domain::api::import_lists::ImportListAddRequest {
        items: keys.clone(),
        root_folder_path: root.map(|r| r.path.clone()).unwrap_or_default(),
        quality_profile_id: b["qualityProfileId"].as_i64().unwrap_or_default(),
        monitored: b["monitored"].as_bool().unwrap_or(true),
    };
    let outcomes = match crate::routes::import_lists::add_items(
        State(state),
        Path(id),
        Json(request),
    )
    .await
    {
        Ok(Json(o)) => o,
        Err(ApiError::Validation(_)) => {
            return Ok(bad_request(
                "Refresh the preview, choose a valid folder and profile, and select up to 25 shows at a time.",
            ));
        }
        Err(e) => return Err(e),
    };
    Ok(Json(
        keys.iter()
            .zip(outcomes.iter())
            .map(|(k, o)| {
                let status = match o.outcome.as_str() {
                    "added" => "Added".to_string(),
                    "existing" | "alreadyAdded" => "Already added".to_string(),
                    "excluded" => "Excluded from import lists".to_string(),
                    _ => "Could not add: check the metadata provider and selected folder/profile."
                        .to_string(),
                };
                json!({"key": k, "title": o.title, "seriesId": o.series_id, "status": status})
            })
            .collect::<Vec<_>>(),
    )
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subresources_are_read_repeated_or_comma_separated() {
        let s = subresources(Some(
            "seriesId=1&includeSubresources=series&includeSubresources=episodeFile%2CImages",
        ));
        assert_eq!(
            s.into_iter().collect::<Vec<_>>(),
            ["episodefile", "images", "series"]
        );
        assert!(subresources(None).is_empty());
        let q: Query<v3::SeriesFilter> = typed(
            Some("tvdbId=81189&includeSubresources=a&includeSubresources=b"),
            &["includeSubresources"],
        )
        .unwrap();
        assert_eq!(q.0.tvdb_id, Some(81189));
    }

    #[test]
    fn releases_nest_their_parts_like_sonarr_v5() {
        let v3 = json!({
            "guid": "g", "title": "Show.S01E02.1080p.WEB-DL-GRP", "seasonNumber": 1, "episodeNumbers": [2],
            "absoluteEpisodeNumbers": [], "approved": false, "rejected": true, "temporarilyRejected": false,
            "rejections": ["Not wanted"], "indexerId": 3, "indexer": "Nyaa", "protocol": "torrent",
            "seriesId": 7, "quality": {"quality": {"id": 3}}, "size": 10, "customFormatScore": 5,
        });
        let v5 = release_v5(&v3);
        assert_eq!(v5["release"]["guid"], "g");
        assert_eq!(v5["release"]["indexerId"], 3);
        assert_eq!(v5["parsedInfo"]["seasonNumbers"], json!([1]));
        assert_eq!(v5["decision"]["approved"], false);
        assert_eq!(v5["decision"]["rejections"][0]["message"], "Not wanted");
        assert_eq!(v5["mappedSeriesId"], 7);
        assert_eq!(v5["customFormatScore"], 5);
        assert!(v5.get("guid").is_none(), "v5 has no flat release fields");
    }

    #[test]
    fn series_gain_v5_statistics_and_lose_language_profiles() {
        let mut facts = SeriesFacts::default();
        facts.seasons.insert(
            (1, 1),
            SeasonFacts {
                monitored: 3,
                release_groups: ["GRP".to_string()].into(),
                release_types: [1].into(),
                qualities: [4].into(),
                images: json!([{"coverType": "poster"}]),
                next_airing: Some("2030-01-01T00:00:00Z".into()),
                ..Default::default()
            },
        );
        facts.original_country.insert(1, Some("us".into()));
        let s = series_v5(
            json!({"id": 1, "languageProfileId": 1, "statistics": {},
                   "seasons": [{"seasonNumber": 1, "statistics": {}}, {"seasonNumber": 2, "statistics": {}}]}),
            &facts,
            true,
        );
        assert!(s.get("languageProfileId").is_none());
        assert_eq!(s["originalCountry"], "us");
        assert_eq!(s["statistics"]["monitoredEpisodeCount"], 3);
        assert_eq!(s["statistics"]["releaseTypes"], json!(["singleEpisode"]));
        assert_eq!(
            s["seasons"][0]["statistics"]["releaseGroups"],
            json!(["GRP"])
        );
        assert_eq!(
            s["seasons"][0]["statistics"]["episodeFileQualities"][0]["id"],
            4
        );
        assert_eq!(s["seasons"][0]["images"][0]["coverType"], "poster");
        assert_eq!(s["seasons"][1]["images"], json!([]));
        assert_eq!(s["nextAiring"], "2030-01-01T00:00:00Z");
    }

    #[test]
    fn health_checks_carry_sonarr_reasons() {
        assert_eq!(
            health_reason(
                "IndexerRssCheck",
                "No indexers available with RSS sync enabled"
            ),
            "indexerRssNoIndexersEnabled"
        );
        assert_eq!(
            health_reason("IndexerLongTermStatusCheck", "All indexers are unavailable"),
            "indexerLongTermStatusAllUnavailable"
        );
        assert_eq!(health_reason("Something", ""), "unknown");
        let mut v = json!([{"source": "BackupCheck", "type": "warning", "message": "old"}]);
        health_out(&mut v);
        assert_eq!(v[0]["reason"], "backupStale");
        let mut s = json!({"appName": "Sonarr", "isContainerized": true, "sqliteVersion": "3"});
        system_out(&mut s);
        assert_eq!(s, json!({"appName": "Sonarr"}));
    }

    #[test]
    fn media_information_is_shaped_like_sonarr_v5() {
        assert_eq!(media_info_v5(None, ""), Value::Null);
        let raw = json!({
            "schemaRevision": 12, "videoFormat": "hevc", "videoBitDepth": 10, "width": 1920, "height": 1080,
            "runTime": "00:42:05", "videoFps": 23.976,
            "audioStreams": [{"language": "eng", "format": "eac3", "channels": 6, "channelPositions": "5.1", "bitrate": 0}],
            "subtitleStreams": [{"language": "eng", "format": "subrip", "forced": false}],
        })
        .to_string();
        let v = media_info_v5(Some(&raw), "Show.S01E01.1080p.WEB.x265-GRP");
        assert_eq!(v["resolution"], "1920x1080");
        assert_eq!(v["runTime"], "42:05");
        assert_eq!(v["videoBitDepth"], 10);
        assert_eq!(v["audioStreams"][0]["channels"], 5.1);
        assert_eq!(v["subtitleStreams"][0]["language"], "eng");
    }

    #[test]
    fn statistics_filters_become_sql_conditions() {
        let (cond, binds) = statistics_filter(Some(
            "rootFolderPaths=/tv&tagIds=1,2&tagIdsNot=true&monitored=true&seriesTypes=anime",
        ));
        assert!(cond.contains("SUBSTR(s.path, 1, ?) = ?"), "{cond}");
        assert!(cond.contains("NOT (EXISTS"), "{cond}");
        assert!(cond.contains("s.monitored = ?"));
        assert!(cond.contains("s.series_type = ?"));
        assert_eq!(binds[0], json!(4));
        assert_eq!(binds[1], json!("/tv/"));
        assert_eq!(binds.last(), Some(&json!("anime")));
        assert_eq!(statistics_filter(None).0, "1 = 1");
    }
}
