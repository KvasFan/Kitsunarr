//! Sonarr `/api/v3` compatibility for third-party tools (Overseerr/Jellyseerr, Prowlarr
//! indexer sync, Bazarr, mobile clients): the subset they call, mapped onto the native model.
//!
//! Resources use Sonarr's JSON shapes and names. Series are identified for these tools by their
//! TheTVDB id; titles owned by other catalogues are listed with the TheTVDB id they carry (or 0).
//! New series are added from TheTVDB (through SkyHook), as Sonarr does. The reported version is
//! Sonarr's major version 4 so that tools take their v4 code paths; `kitsunarrVersion` carries
//! Kitsunarr's own.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use kitsunarr_db::library::series;
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_domain::api::library::{
    EpisodeItem, LibraryQuery, SeasonMonitoring, SeriesDetail, SeriesUpdate,
};
use kitsunarr_domain::api::metadata::{AddTitleRequest, MonitorOption};
use kitsunarr_domain::api::search::{IndexerTestRequest, ReleaseQuery};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_domain::quality::Quality;
use kitsunarr_domain::{ExternalIdKind, MetadataProvider, SeriesType};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

/// The Sonarr version reported to tools.
const SONARR_VERSION: &str = "4.0.0.0";

/// The Sonarr v5 routes Tanukarr's compression worker calls back on (`series/{id}`,
/// `episodefile`, `command` with `RescanSeries`), answered as v3 answers them: the fields the
/// worker reads (`title`, `path`, `seriesId`, `relativePath`) are the same.
pub fn worker_router() -> Router<AppState> {
    Router::new()
        .route("/series/{id}", get(get_series))
        .route("/episodefile", get(list_episode_files))
        .route("/episodefile/{id}", get(get_episode_file))
        .route("/command", post(command))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/system/status", get(system_status))
        .route("/health", get(health))
        .route("/qualityprofile", get(quality_profiles))
        .route("/languageprofile", get(language_profiles))
        .route("/rootfolder", get(root_folders))
        .route("/tag", get(tags).post(create_tag))
        .route(
            "/series",
            get(list_series).post(add_series).put(update_series_body),
        )
        .route("/series/lookup", get(lookup))
        .route(
            "/series/{id}",
            get(get_series).put(update_series).delete(delete_series),
        )
        .route("/episode", get(list_episodes))
        .route("/episode/{id}", get(get_episode))
        .route("/episode/monitor", put(monitor_episodes))
        .route("/episodefile", get(list_episode_files))
        .route("/episodefile/{id}", get(get_episode_file))
        .route("/command", post(command))
        .route("/command/{id}", get(get_command))
        .route("/queue", get(queue))
        .route("/calendar", get(calendar))
        .route("/indexer", get(list_indexers).post(create_indexer))
        .route("/indexer/schema", get(indexer_schema))
        .route("/indexer/test", post(test_indexer))
        .route(
            "/indexer/{id}",
            get(get_indexer).put(update_indexer).delete(delete_indexer),
        )
        .route("/release/push", post(push_release))
        .route("/wanted/missing", get(wanted_missing))
        .route("/wanted/cutoff", get(wanted_cutoff))
        .route("/history", get(history))
        .route("/history/series", get(history_of_series))
        .route("/history/since", get(history_since))
        .route("/blocklist", get(blocklist))
        .route("/blocklist/{id}", axum::routing::delete(delete_blocklisted))
        .route(
            "/blocklist/bulk",
            axum::routing::delete(delete_blocklisted_bulk),
        )
        .route("/downloadclient", get(download_clients))
        .route("/downloadclient/{id}", get(download_client))
        .route("/notification", get(notifications))
        .route("/notification/{id}", get(notification))
}

// ---- helpers ----------------------------------------------------------------------------

fn ext(detail: &SeriesDetail, kind: ExternalIdKind) -> Option<String> {
    detail
        .external_ids
        .iter()
        .find(|e| e.kind == kind)
        .map(|e| e.value.clone())
}

fn ext_int(detail: &SeriesDetail, kind: ExternalIdKind) -> i64 {
    ext(detail, kind)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

pub(crate) fn quality_json(id: i32) -> Value {
    let q = Quality::by_id(id);
    json!({
        "id": id,
        "name": q.map_or("Unknown", |q| q.name),
        "source": q.map_or("unknown".to_string(), |q| format!("{:?}", q.source).to_lowercase()),
        "resolution": q.map_or(0, |q| q.resolution),
    })
}

fn images(detail: &SeriesDetail) -> Vec<Value> {
    detail
        .images
        .iter()
        .map(|i| json!({"coverType": i.cover_type, "url": i.remote_url, "remoteUrl": i.remote_url}))
        .collect()
}

pub(crate) fn series_json(detail: &SeriesDetail) -> Value {
    let item = &detail.item;
    let percent = |files: i64, total: i64| {
        if total > 0 {
            (files as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    };
    let seasons: Vec<Value> = detail
        .seasons
        .iter()
        .map(|s| {
            json!({
                "seasonNumber": s.season_number,
                "monitored": s.monitored,
                "statistics": {
                    "episodeFileCount": s.episode_file_count,
                    "episodeCount": s.episode_count,
                    "totalEpisodeCount": s.episode_count,
                    "sizeOnDisk": s.size_on_disk,
                    "percentOfEpisodes": percent(s.episode_file_count, s.episode_count),
                }
            })
        })
        .collect();
    let images = images(detail);
    let poster = detail
        .images
        .iter()
        .find(|i| i.cover_type == "poster")
        .map(|i| i.remote_url.clone());
    json!({
        "id": item.id,
        "title": item.title,
        "sortTitle": item.sort_title,
        "status": item.status.as_str(),
        "ended": item.status.as_str() == "ended",
        "overview": detail.overview,
        "network": item.network,
        "images": images,
        "remotePoster": poster,
        "seasons": seasons,
        "year": item.year.unwrap_or(0),
        "path": item.path,
        "qualityProfileId": item.quality_profile_id,
        "languageProfileId": 1,
        "seasonFolder": detail.season_folder,
        "monitored": item.monitored,
        "useSceneNumbering": detail.use_scene_numbering,
        "runtime": detail.runtime.unwrap_or(0),
        "tvdbId": ext_int(detail, ExternalIdKind::Tvdb),
        "tvRageId": ext_int(detail, ExternalIdKind::TvRage),
        "tvMazeId": ext_int(detail, ExternalIdKind::TvMaze),
        "tmdbId": ext_int(detail, ExternalIdKind::Tmdb),
        "imdbId": ext(detail, ExternalIdKind::Imdb),
        "firstAired": detail.first_aired,
        "lastAired": detail.last_aired,
        "seriesType": item.series_type.as_str(),
        "cleanTitle": kitsunarr_domain::text::clean_title(&item.title),
        "titleSlug": item.sort_title.replace(' ', "-"),
        "rootFolderPath": detail.root_folder_path,
        "certification": detail.certification,
        "genres": detail.genres,
        "tags": item.tags,
        "added": item.added_at,
        "alternateTitles": detail.alternate_titles.iter().map(|t| json!({"title": t, "seasonNumber": -1})).collect::<Vec<_>>(),
        "ratings": {"votes": 0, "value": 0},
        "statistics": {
            "seasonCount": item.season_count,
            "episodeFileCount": item.episode_file_count,
            "episodeCount": item.episode_count,
            "totalEpisodeCount": item.episode_count,
            "sizeOnDisk": item.size_on_disk,
            "percentOfEpisodes": percent(item.episode_file_count, item.episode_count),
        },
    })
}

pub(crate) fn episode_json(e: &EpisodeItem) -> Value {
    json!({
        "id": e.id,
        "seriesId": e.series_id,
        "tvdbId": 0,
        "episodeFileId": e.file.as_ref().map_or(0, |f| f.id),
        "seasonNumber": e.season_number,
        "episodeNumber": e.episode_number,
        "title": e.title,
        "airDate": e.air_date,
        "airDateUtc": e.air_date_utc,
        "runtime": e.runtime.unwrap_or(0),
        "hasFile": e.file.is_some(),
        "monitored": e.monitored,
        "absoluteEpisodeNumber": e.absolute_episode_number,
        "sceneSeasonNumber": e.scene_season_number,
        "sceneEpisodeNumber": e.scene_episode_number,
        "unverifiedSceneNumbering": false,
    })
}

fn episode_file_json(detail: &SeriesDetail, f: &series::FileRecord, languages: &[i32]) -> Value {
    let path = std::path::Path::new(&detail.item.path).join(&f.relative_path);
    json!({
        "id": f.id,
        "seriesId": detail.item.id,
        "relativePath": f.relative_path,
        "path": path.display().to_string(),
        "size": f.size,
        "dateAdded": f.added_at,
        "sceneName": f.scene_name,
        "releaseGroup": f.release_group,
        "languages": languages.iter().map(|id| json!({"id": id, "name": kitsunarr_domain::language::Language::by_id(*id).map_or("Unknown", |l| l.name)})).collect::<Vec<_>>(),
        "quality": {
            "quality": quality_json(f.quality_id as i32),
            "revision": {"version": f.revision_version, "real": f.revision_real, "isRepack": f.is_repack},
        },
        "qualityCutoffNotMet": false,
    })
}

async fn detail(state: &AppState, id: i64) -> ApiResult<SeriesDetail> {
    Ok(series::get_detail(&state.db, id).await?)
}

async fn all_series(state: &AppState) -> ApiResult<Vec<SeriesDetail>> {
    let page = series::list(
        &state.db,
        &LibraryQuery {
            page_size: Some(100_000),
            ..Default::default()
        },
    )
    .await?;
    let mut out = Vec::new();
    for s in page.records {
        out.push(detail(state, s.id).await?);
    }
    Ok(out)
}

// ---- system -----------------------------------------------------------------------------

async fn system_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let native = crate::routes::system::status(State(state)).await?.0;
    Ok(Json(json!({
        "appName": native.app_name,
        "instanceName": native.instance_name,
        "version": SONARR_VERSION,
        "kitsunarrVersion": native.version,
        "buildTime": native.start_time,
        "startTime": native.start_time,
        "isDebug": false,
        "isProduction": true,
        "isAdmin": false,
        "isUserInteractive": false,
        "osName": native.os,
        "isDocker": native.is_container,
        "isLinux": native.os == "linux",
        "isWindows": native.os == "windows",
        "isOsx": native.os == "macos",
        "urlBase": native.url_base,
        "appData": native.data_directory,
        "databaseType": "sqLite",
        "authentication": "forms",
        "branch": "main",
    })))
}

async fn health(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let checks = crate::routes::system::health(State(state)).await?.0;
    Ok(Json(
        checks
            .into_iter()
            .map(|c| json!({"source": c.source, "type": format!("{:?}", c.level).to_lowercase(), "message": c.message}))
            .collect(),
    ))
}

// ---- settings -----------------------------------------------------------------------------

async fn quality_profiles(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let formats = kitsunarr_db::quality::list_custom_formats(state.db.pool()).await?;
    Ok(Json(
        kitsunarr_db::quality::list_profiles(state.db.pool())
            .await?
            .iter()
            .map(|p| crate::v3_settings::profile_json(p, &formats))
            .collect(),
    ))
}

/// Sonarr v4 has no language profiles; it answers one placeholder, as here.
async fn language_profiles() -> Json<Vec<Value>> {
    Json(vec![json!({
        "id": 1, "name": "Deprecated", "upgradeAllowed": false,
        "cutoff": {"id": 1, "name": "English"}, "languages": []
    })])
}

async fn root_folders(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let list = crate::routes::settings::list_root_folders(State(state))
        .await?
        .0;
    Ok(Json(
        list.into_iter()
            .map(|r| json!({"id": r.id, "path": r.path, "accessible": r.accessible, "freeSpace": r.free_space, "totalSpace": r.total_space, "unmappedFolders": []}))
            .collect(),
    ))
}

async fn tags(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let list = kitsunarr_db::tags::list(state.db.pool()).await?;
    Ok(Json(
        list.into_iter()
            .map(|t| json!({"id": t.id, "label": t.label}))
            .collect(),
    ))
}

#[derive(Deserialize)]
struct TagBody {
    label: String,
}

async fn create_tag(
    State(state): State<AppState>,
    Json(body): Json<TagBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let label = body.label.trim().to_lowercase();
    if label.is_empty() {
        return Err(ApiError::field("label", "A tag needs a label."));
    }
    let existing = kitsunarr_db::tags::list(state.db.pool())
        .await?
        .into_iter()
        .find(|t| t.label.eq_ignore_ascii_case(&label));
    let id = match existing {
        Some(t) => t.id,
        None => kitsunarr_db::tags::create(state.db.pool(), &label).await?,
    };
    Ok((StatusCode::CREATED, Json(json!({"id": id, "label": label}))))
}

// ---- series -------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesFilter {
    tvdb_id: Option<i64>,
}

async fn list_series(
    State(state): State<AppState>,
    Query(f): Query<SeriesFilter>,
) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        all_series(&state)
            .await?
            .iter()
            .filter(|d| {
                f.tvdb_id
                    .is_none_or(|t| ext_int(d, ExternalIdKind::Tvdb) == t)
            })
            .map(series_json)
            .collect(),
    ))
}

async fn get_series(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    Ok(Json(series_json(&detail(&state, id).await?)))
}

#[derive(Deserialize)]
struct LookupQuery {
    term: String,
}

/// `GET /series/lookup?term=`: `tvdb:ID` reads that series (with its seasons); other terms
/// search TheTVDB. Titles already in the library come back as their library resource (with
/// an id), as in Sonarr.
async fn lookup(
    State(state): State<AppState>,
    Query(q): Query<LookupQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let term = q.term.trim();
    let library = all_series(&state).await?;
    let in_library = |tvdb: i64| {
        library
            .iter()
            .find(|d| tvdb > 0 && ext_int(d, ExternalIdKind::Tvdb) == tvdb)
    };
    if let Some(id) = term.strip_prefix("tvdb:").map(str::trim) {
        let tvdb: i64 = id
            .parse()
            .map_err(|_| ApiError::field("term", "Not a TheTVDB id."))?;
        if let Some(d) = in_library(tvdb) {
            return Ok(Json(vec![series_json(d)]));
        }
        let info = match state
            .metadata
            .series_ordered(MetadataProvider::Tvdb, id, None)
            .await
        {
            Ok(info) => info,
            Err(kitsunarr_metadata::MetadataError::NotFound(_)) => return Ok(Json(Vec::new())),
            Err(e) => return Err(kitsunarr_library::LibraryError::from(e).into()),
        };
        let poster = info
            .images
            .iter()
            .find(|i| i.cover_type == "poster")
            .map(|i| i.remote_url.clone());
        return Ok(Json(vec![json!({
            "title": info.title,
            "sortTitle": kitsunarr_domain::text::sort_title(&info.title),
            "status": info.status.as_str(),
            "overview": info.overview,
            "network": info.network,
            "images": info.images.iter().map(|i| json!({"coverType": i.cover_type, "url": i.remote_url, "remoteUrl": i.remote_url})).collect::<Vec<_>>(),
            "remotePoster": poster,
            "seasons": info.seasons.iter().map(|s| json!({"seasonNumber": s.season_number, "monitored": s.season_number > 0})).collect::<Vec<_>>(),
            "year": info.year.unwrap_or(0),
            "tvdbId": tvdb,
            "imdbId": info.external_ids.iter().find(|e| e.kind == ExternalIdKind::Imdb).map(|e| e.value.clone()),
            "tmdbId": info.external_ids.iter().find(|e| e.kind == ExternalIdKind::Tmdb).and_then(|e| e.value.parse::<i64>().ok()).unwrap_or(0),
            "seriesType": info.series_type.as_str(),
            "runtime": info.runtime.unwrap_or(0),
            "genres": info.genres,
            "firstAired": info.first_aired,
            "monitored": false,
            "seasonFolder": true,
            "added": "0001-01-01T00:00:00Z",
            "ratings": {"votes": 0, "value": 0},
        })]));
    }
    let found = state
        .metadata
        .search(MetadataProvider::Tvdb, term)
        .await
        .map_err(kitsunarr_library::LibraryError::from)?;
    Ok(Json(
        found
            .iter()
            .map(|r| {
                let tvdb: i64 = r.id.parse().unwrap_or(0);
                match in_library(tvdb) {
                    Some(d) => series_json(d),
                    None => json!({
                        "title": r.title,
                        "sortTitle": kitsunarr_domain::text::sort_title(&r.title),
                        "status": r.status.as_str(),
                        "overview": r.overview,
                        "network": r.network,
                        "images": r.poster_url.iter().map(|u| json!({"coverType": "poster", "url": u, "remoteUrl": u})).collect::<Vec<_>>(),
                        "remotePoster": r.poster_url,
                        "seasons": [],
                        "year": r.year.unwrap_or(0),
                        "tvdbId": tvdb,
                        "monitored": false,
                        "added": "0001-01-01T00:00:00Z",
                        "ratings": {"votes": 0, "value": 0},
                    }),
                }
            })
            .collect(),
    ))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct SeasonBody {
    season_number: i32,
    monitored: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct AddOptions {
    search_for_missing_episodes: bool,
    search_for_cutoff_unmet_episodes: bool,
    monitor: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct SeriesBody {
    id: Option<i64>,
    path: Option<String>,
    tvdb_id: i64,
    quality_profile_id: Option<i64>,
    root_folder_path: Option<String>,
    season_folder: Option<bool>,
    monitored: Option<bool>,
    series_type: Option<String>,
    tags: Option<Vec<i64>>,
    seasons: Option<Vec<SeasonBody>>,
    add_options: Option<AddOptions>,
}

fn series_type(s: Option<&str>) -> Option<SeriesType> {
    match s? {
        "anime" => Some(SeriesType::Anime),
        "daily" => Some(SeriesType::Daily),
        "standard" => Some(SeriesType::Standard),
        _ => None,
    }
}

async fn add_series(
    State(state): State<AppState>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Json(mut body): Json<SeriesBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    // Tanukarr's `CompanionAddSeriesPolicy`: companion keys add into a registered root folder,
    // never at a chosen path, and never start searches.
    if let Some(axum::Extension(crate::auth::Principal::Companion { .. })) = &principal {
        if body.path.as_deref().is_some_and(|p| !p.trim().is_empty()) {
            return Err(ApiError::field(
                "path",
                "Companion keys cannot choose a series path.",
            ));
        }
        let roots = kitsunarr_db::root_folders::list(state.db.pool()).await?;
        let wanted = body.root_folder_path.clone().unwrap_or_default();
        let wanted = wanted.trim_end_matches(['/', '\\']);
        let Some(root) = roots
            .iter()
            .find(|r| r.path.trim_end_matches(['/', '\\']) == wanted)
        else {
            return Err(ApiError::field(
                "rootFolderPath",
                "Choose a registered root folder.",
            ));
        };
        body.root_folder_path = Some(root.path.clone());
        if body
            .add_options
            .as_ref()
            .is_some_and(|o| o.search_for_missing_episodes || o.search_for_cutoff_unmet_episodes)
        {
            return Err(ApiError::field(
                "addOptions",
                "Companion keys cannot start searches.",
            ));
        }
    }
    if body.tvdb_id <= 0 {
        return Err(ApiError::field("tvdbId", "A TheTVDB id is required."));
    }
    let root = body
        .root_folder_path
        .clone()
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| ApiError::field("rootFolderPath", "A root folder is required."))?;
    let options = body.add_options.take().unwrap_or_default();
    let monitor = match options.monitor.as_deref() {
        Some("future") => MonitorOption::Future,
        Some("firstSeason") => MonitorOption::FirstSeason,
        Some("latestSeason") | Some("lastSeason") => MonitorOption::LatestSeason,
        Some("none") => MonitorOption::None,
        _ => MonitorOption::All,
    };
    let req = AddTitleRequest {
        provider: MetadataProvider::Tvdb,
        id: body.tvdb_id.to_string(),
        root_folder_path: root,
        quality_profile_id: body.quality_profile_id.unwrap_or(1),
        monitor,
        series_type: series_type(body.series_type.as_deref()),
        season_folder: body.season_folder.unwrap_or(true),
        tags: body.tags.clone().unwrap_or_default(),
        search_for_missing: false,
        episode_ordering: None,
    };
    let (_, Json(added)) = crate::routes::metadata::add(State(state.clone()), Json(req)).await?;
    // The request's season choices (Overseerr monitors the requested seasons only).
    if body.seasons.is_some() || body.monitored.is_some() {
        apply(&state, added.id, &body).await?;
    }
    if options.search_for_missing_episodes {
        crate::routes::wanted::search_missing_of_series(&state, added.id, false);
    }
    Ok((
        StatusCode::CREATED,
        Json(series_json(&detail(&state, added.id).await?)),
    ))
}

async fn apply(state: &AppState, id: i64, body: &SeriesBody) -> ApiResult<()> {
    let update = SeriesUpdate {
        monitored: body.monitored,
        quality_profile_id: body.quality_profile_id,
        series_type: series_type(body.series_type.as_deref()),
        season_folder: body.season_folder,
        tags: body.tags.clone(),
        seasons: body.seasons.as_ref().map(|s| {
            s.iter()
                .map(|s| SeasonMonitoring {
                    season_number: s.season_number,
                    monitored: s.monitored,
                })
                .collect()
        }),
        ..Default::default()
    };
    series::apply_update(&state.db, id, &update).await?;
    state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: vec![id],
            deleted: false,
        });
    Ok(())
}

async fn update_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<SeriesBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    detail(&state, id).await?;
    apply(&state, id, &body).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(series_json(&detail(&state, id).await?)),
    ))
}

/// `PUT /series` with the id in the body (Overseerr).
async fn update_series_body(
    State(state): State<AppState>,
    Json(body): Json<SeriesBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let id = body
        .id
        .ok_or_else(|| ApiError::field("id", "The series id is required."))?;
    update_series(State(state), Path(id), Json(body)).await
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct DeleteQuery {
    delete_files: bool,
}

async fn delete_series(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    crate::routes::library::delete_series(
        State(state),
        Path(id),
        Query(crate::routes::library::DeleteSeriesQuery {
            delete_files: q.delete_files,
        }),
    )
    .await?;
    Ok(StatusCode::OK)
}

// ---- episodes and files -------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeFilter {
    series_id: Option<i64>,
    season_number: Option<i32>,
    #[serde(default)]
    episode_ids: Option<String>,
}

async fn list_episodes(
    State(state): State<AppState>,
    Query(f): Query<EpisodeFilter>,
) -> ApiResult<Json<Vec<Value>>> {
    let Some(series_id) = f.series_id else {
        let ids: Vec<i64> = f
            .episode_ids
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        let mut out = Vec::new();
        for id in ids {
            if let Ok(Json(v)) = get_episode(State(state.clone()), Path(id)).await {
                out.push(v);
            }
        }
        return Ok(Json(out));
    };
    let list = series::list_episodes(&state.db, series_id, f.season_number).await?;
    Ok(Json(list.iter().map(episode_json).collect()))
}

async fn get_episode(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let series_id: i64 = sqlx_series_of_episode(&state, id).await?;
    let list = series::list_episodes(&state.db, series_id, None).await?;
    let e = list
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| ApiError::NotFound("The episode does not exist.".into()))?;
    Ok(Json(episode_json(e)))
}

async fn sqlx_series_of_episode(state: &AppState, id: i64) -> ApiResult<i64> {
    series::series_of_episode(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("The episode does not exist.".into()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitorBody {
    episode_ids: Vec<i64>,
    monitored: bool,
}

async fn monitor_episodes(
    State(state): State<AppState>,
    Json(body): Json<MonitorBody>,
) -> ApiResult<StatusCode> {
    series::set_episodes_monitored(&state.db, &body.episode_ids, body.monitored).await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileFilter {
    series_id: Option<i64>,
}

async fn files_of(state: &AppState, series_id: i64) -> ApiResult<Vec<Value>> {
    let d = detail(state, series_id).await?;
    let episodes = series::list_episodes(&state.db, series_id, None).await?;
    Ok(series::file_records(&state.db, series_id)
        .await?
        .iter()
        .map(|f| {
            let languages = episodes
                .iter()
                .find_map(|e| {
                    e.file
                        .as_ref()
                        .filter(|x| x.id == f.id)
                        .map(|x| x.languages.clone())
                })
                .unwrap_or_default();
            let mut v = episode_file_json(&d, f, &languages);
            if let Some(season) = episodes
                .iter()
                .find(|e| e.file.as_ref().is_some_and(|x| x.id == f.id))
                .map(|e| e.season_number)
            {
                v["seasonNumber"] = json!(season);
            }
            v
        })
        .collect())
}

async fn list_episode_files(
    State(state): State<AppState>,
    Query(f): Query<FileFilter>,
) -> ApiResult<Json<Vec<Value>>> {
    let series_id = f
        .series_id
        .ok_or_else(|| ApiError::field("seriesId", "A series id is required."))?;
    Ok(Json(files_of(&state, series_id).await?))
}

async fn get_episode_file(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let series_id = series::series_of_file(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("The episode file does not exist.".into()))?;
    files_of(&state, series_id)
        .await?
        .into_iter()
        .find(|v| v["id"] == json!(id))
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("The episode file does not exist.".into()))
}

// ---- commands -----------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct CommandBody {
    name: String,
    series_id: Option<i64>,
    season_number: Option<i32>,
    episode_ids: Vec<i64>,
}

fn command_json(id: i64, name: &str, status: &str, message: &str) -> Value {
    let now = kitsunarr_db::now();
    json!({"id": id, "name": name, "commandName": name, "status": status, "message": message,
        "queued": now, "started": now, "ended": now, "trigger": "manual", "body": {"name": name}})
}

/// Commands run in the background (searches, refreshes); they report as started at once.
async fn command(
    State(state): State<AppState>,
    Json(body): Json<CommandBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let id = chrono::Utc::now().timestamp_millis();
    let name = body.name.clone();
    let message = match name.as_str() {
        "SeriesSearch" => {
            let sid = body
                .series_id
                .ok_or_else(|| ApiError::field("seriesId", "A series id is required."))?;
            detail(&state, sid).await?;
            crate::routes::wanted::search_missing_of_series(&state, sid, true);
            "Searching the series' missing monitored episodes"
        }
        "SeasonSearch" | "EpisodeSearch" => {
            let queries: Vec<ReleaseQuery> = if name == "SeasonSearch" {
                let sid = body
                    .series_id
                    .ok_or_else(|| ApiError::field("seriesId", "A series id is required."))?;
                vec![ReleaseQuery {
                    series_id: sid,
                    season_number: body.season_number,
                    episode_id: None,
                    complete_series: false,
                    season_packs_only: false,
                }]
            } else {
                let mut q = Vec::new();
                for e in &body.episode_ids {
                    q.push(ReleaseQuery {
                        series_id: sqlx_series_of_episode(&state, *e).await?,
                        season_number: None,
                        episode_id: Some(*e),
                        complete_series: false,
                        season_packs_only: false,
                    });
                }
                q
            };
            let st = state.clone();
            tokio::spawn(async move {
                for q in queries {
                    if let Err(e) =
                        crate::routes::search::automatic_search(State(st.clone()), Json(q)).await
                    {
                        tracing::warn!(error = %e.message(), "v3 search command failed");
                    }
                }
            });
            "Searching"
        }
        "RefreshSeries" | "RescanSeries" => {
            let targets: Vec<i64> = match body.series_id {
                Some(s) => vec![s],
                None => all_series(&state)
                    .await?
                    .iter()
                    .map(|d| d.item.id)
                    .collect(),
            };
            let st = state.clone();
            let refresh = name == "RefreshSeries";
            tokio::spawn(async move {
                for s in targets {
                    let r = if refresh {
                        crate::routes::metadata::refresh_series(State(st.clone()), Path(s))
                            .await
                            .map(|_| ())
                    } else {
                        crate::routes::library::rescan_series(State(st.clone()), Path(s))
                            .await
                            .map(|_| ())
                    };
                    if let Err(e) = r {
                        tracing::warn!(series = s, error = %e.message(), "v3 command failed");
                    }
                }
            });
            if refresh { "Refreshing" } else { "Rescanning" }
        }
        "MissingEpisodeSearch" | "CutoffUnmetEpisodeSearch" => {
            let list = if name == "MissingEpisodeSearch" {
                kitsunarr_domain::api::wanted::WantedList::Missing
            } else {
                kitsunarr_domain::api::wanted::WantedList::CutoffUnmet
            };
            let _started = crate::routes::wanted::search_wanted(
                State(state.clone()),
                Json(kitsunarr_domain::api::wanted::WantedSearchRequest {
                    list,
                    episode_ids: body.episode_ids.clone(),
                    monitored: Some(true),
                }),
            )
            .await?;
            "Searching wanted episodes"
        }
        "RssSync" => {
            let st = state.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::rss::sync(&st).await {
                    tracing::warn!(error = %e, "v3 RSS sync failed");
                }
            });
            "RSS sync started"
        }
        other => {
            return Err(ApiError::field(
                "name",
                format!("The command {other} is not supported."),
            ));
        }
    };
    Ok((
        StatusCode::CREATED,
        Json(command_json(id, &name, "started", message)),
    ))
}

async fn get_command(Path(id): Path<i64>) -> Json<Value> {
    Json(command_json(id, "Command", "completed", "Completed"))
}

// ---- queue and calendar -------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct QueueQuery {
    page: Option<i64>,
    page_size: Option<i64>,
}

async fn queue(
    State(state): State<AppState>,
    Query(q): Query<QueueQuery>,
) -> ApiResult<Json<Value>> {
    let native = crate::routes::queue::get_queue(State(state.clone()))
        .await?
        .0;
    let ids: Vec<String> = native.items.iter().map(|i| i.download_id.clone()).collect();
    let grabs = kitsunarr_db::history::grabs_for(&state.db, &ids).await?;
    let mut records = Vec::new();
    for i in &native.items {
        let episode_ids: Vec<i64> = grabs
            .iter()
            .filter(|g| g.download_id.eq_ignore_ascii_case(&i.download_id))
            .filter_map(|g| g.episode_id)
            .collect();
        let base = json!({
            "id": crate::v3_more::queue_id(&i.download_id),
            "seriesId": i.series_id,
            "title": i.title,
            "size": i.size,
            "sizeleft": i.size_left,
            "timeleft": i.time_left.map(|s| format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)),
            "estimatedCompletionTime": i.time_left.map(|s| (chrono::Utc::now() + chrono::Duration::seconds(s)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            "status": i.status,
            "trackedDownloadStatus": if i.messages.is_empty() { "ok" } else { "warning" },
            "trackedDownloadState": i.tracked_state,
            "statusMessages": i.messages.iter().map(|m| json!({"title": i.title, "messages": [m]})).collect::<Vec<_>>(),
            "downloadId": i.download_id,
            "protocol": format!("{:?}", i.protocol).to_lowercase(),
            "downloadClient": i.download_client,
            "indexer": i.indexer,
            "outputPath": i.output_path,
        });
        if episode_ids.is_empty() {
            records.push(base.clone());
        }
        // One record per episode, as Sonarr's queue does.
        for e in episode_ids {
            let mut r = base.clone();
            r["episodeId"] = json!(e);
            records.push(r);
        }
    }
    let total = records.len() as i64;
    let page = q.page.unwrap_or(1).max(1);
    let size = q.page_size.unwrap_or(total.max(1)).max(1);
    let records: Vec<Value> = records
        .into_iter()
        .skip(((page - 1) * size) as usize)
        .take(size as usize)
        .collect();
    Ok(Json(
        json!({"page": page, "pageSize": size, "sortKey": "timeleft", "sortDirection": "ascending", "totalRecords": total, "records": records}),
    ))
}

#[derive(Deserialize)]
struct CalendarQuery {
    start: Option<String>,
    end: Option<String>,
}

async fn calendar(
    State(state): State<AppState>,
    Query(q): Query<CalendarQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let today = chrono::Utc::now().date_naive();
    let start = q.start.unwrap_or_else(|| today.to_string());
    let end = q
        .end
        .unwrap_or_else(|| (today + chrono::Duration::days(2)).to_string());
    let list = series::calendar(&state.db, &start, &end, true).await?;
    Ok(Json(
        list.iter()
            .map(|e| {
                json!({
                    "id": e.episode_id,
                    "seriesId": e.series_id,
                    "seasonNumber": e.season_number,
                    "episodeNumber": e.episode_number,
                    "absoluteEpisodeNumber": e.absolute_episode_number,
                    "title": e.title,
                    "airDateUtc": e.air_date_utc,
                    "airDate": e.air_date_utc.get(..10),
                    "monitored": e.monitored,
                    "hasFile": e.has_file,
                    "series": {"id": e.series_id, "title": e.series_title},
                })
            })
            .collect(),
    ))
}

// ---- indexers (Prowlarr sync) -------------------------------------------------------------

fn indexer_json(p: &ProviderRecord) -> Value {
    let settings = crate::routes::settings::redact(&p.settings);
    let fields: Vec<Value> = settings
        .iter()
        .map(|(k, v)| json!({"name": k, "value": v}))
        .collect();
    let opt = |k: &str, d: bool| p.options.get(k).and_then(Value::as_bool).unwrap_or(d);
    json!({
        "id": p.id,
        "name": p.name,
        "implementation": p.implementation,
        "implementationName": p.implementation,
        "configContract": format!("{}Settings", p.implementation),
        "protocol": if providers::protocol_for(&p.implementation)
            == kitsunarr_domain::DownloadProtocol::Usenet
        {
            "usenet"
        } else {
            "torrent"
        },
        "priority": p.priority,
        "enableRss": opt("enableRss", true),
        "enableAutomaticSearch": opt("enableAutomaticSearch", true),
        "enableInteractiveSearch": opt("enableInteractiveSearch", true),
        "supportsRss": true,
        "supportsSearch": kitsunarr_indexers::Dialect::from_implementation(&p.implementation)
            .is_none_or(|d| d.supports_search()),
        "downloadClientId": p.options.get("downloadClientId").cloned().unwrap_or(json!(0)),
        "seasonSearchMaximumSingleEpisodeAge": p.options.get("seasonSearchMaximumSingleEpisodeAge").cloned().unwrap_or(json!(0)),
        "tags": p.tags,
        "fields": fields,
    })
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct IndexerBody {
    name: String,
    implementation: String,
    priority: Option<i32>,
    enable_rss: Option<bool>,
    enable_automatic_search: Option<bool>,
    enable_interactive_search: Option<bool>,
    download_client_id: Option<i64>,
    season_search_maximum_single_episode_age: Option<i64>,
    tags: Vec<i64>,
    fields: Vec<Field>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Field {
    name: String,
    value: Value,
}

fn record(id: i64, b: &IndexerBody) -> ApiResult<ProviderRecord> {
    if kitsunarr_indexers::Dialect::from_implementation(&b.implementation).is_none() {
        return Err(ApiError::field(
            "implementation",
            format!("{} indexers are not supported.", b.implementation),
        ));
    }
    let mut settings = Map::new();
    for f in &b.fields {
        if !f.name.is_empty() {
            settings.insert(f.name.clone(), f.value.clone());
        }
    }
    let mut options = Map::new();
    options.insert("enableRss".into(), json!(b.enable_rss.unwrap_or(true)));
    options.insert(
        "enableAutomaticSearch".into(),
        json!(b.enable_automatic_search.unwrap_or(true)),
    );
    options.insert(
        "enableInteractiveSearch".into(),
        json!(b.enable_interactive_search.unwrap_or(true)),
    );
    options.insert(
        "downloadClientId".into(),
        json!(b.download_client_id.unwrap_or(0)),
    );
    options.insert(
        "seasonSearchMaximumSingleEpisodeAge".into(),
        json!(b.season_search_maximum_single_episode_age.unwrap_or(0)),
    );
    Ok(ProviderRecord {
        id,
        kind: ProviderKind::Indexer,
        name: b.name.clone(),
        implementation: b.implementation.clone(),
        protocol: providers::protocol_for(&b.implementation),
        enabled: true,
        priority: b.priority.unwrap_or(25),
        tags: b.tags.clone(),
        options,
        settings,
    })
}

async fn list_indexers(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        providers::list(state.db.pool(), ProviderKind::Indexer)
            .await?
            .iter()
            .map(indexer_json)
            .collect(),
    ))
}

async fn get_indexer(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    Ok(Json(indexer_json(
        &providers::get(state.db.pool(), ProviderKind::Indexer, id).await?,
    )))
}

async fn create_indexer(
    State(state): State<AppState>,
    Json(body): Json<IndexerBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let rec = record(0, &body)?;
    let id = providers::create(state.db.pool(), &rec).await?;
    crate::routes::settings::settings_changed(&state, "indexers");
    Ok((
        StatusCode::CREATED,
        Json(indexer_json(
            &providers::get(state.db.pool(), ProviderKind::Indexer, id).await?,
        )),
    ))
}

async fn update_indexer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<IndexerBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let stored = providers::get(state.db.pool(), ProviderKind::Indexer, id).await?;
    let mut rec = record(id, &body)?;
    crate::routes::settings::merge_secrets(&mut rec.settings, &stored.settings);
    rec.enabled = stored.enabled;
    providers::update(state.db.pool(), &rec).await?;
    crate::routes::settings::settings_changed(&state, "indexers");
    Ok((
        StatusCode::ACCEPTED,
        Json(indexer_json(
            &providers::get(state.db.pool(), ProviderKind::Indexer, id).await?,
        )),
    ))
}

async fn delete_indexer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    providers::delete(state.db.pool(), ProviderKind::Indexer, id).await?;
    crate::routes::settings::settings_changed(&state, "indexers");
    Ok(StatusCode::OK)
}

async fn indexer_schema() -> Json<Vec<Value>> {
    Json(
        crate::routes::schema::templates(ProviderKind::Indexer)
            .iter()
            .map(|t| {
                let rec = ProviderRecord {
                    id: 0,
                    kind: ProviderKind::Indexer,
                    name: t.name.clone(),
                    implementation: t.implementation.clone(),
                    protocol: providers::protocol_for(&t.implementation),
                    enabled: true,
                    priority: t.priority,
                    tags: Vec::new(),
                    options: t.options.clone(),
                    settings: t.settings.clone(),
                };
                indexer_json(&rec)
            })
            .collect(),
    )
}

/// `POST /indexer/test`: 200 when the indexer answers, else 400 with Sonarr's validation list.
async fn test_indexer(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let parsed: IndexerBody = serde_json::from_value(body.clone()).unwrap_or_default();
    let mut settings = Map::new();
    for f in &parsed.fields {
        settings.insert(f.name.clone(), f.value.clone());
    }
    let req = IndexerTestRequest {
        id: body.get("id").and_then(Value::as_i64).filter(|i| *i > 0),
        implementation: parsed.implementation.clone(),
        settings,
    };
    let result = crate::routes::search::test_indexer(State(state), Json(req))
        .await?
        .0;
    if result.ok {
        Ok((StatusCode::OK, Json(json!({}))))
    } else {
        Ok((
            StatusCode::BAD_REQUEST,
            Json(
                json!([{"propertyName": "", "errorMessage": result.message, "severity": "error"}]),
            ),
        ))
    }
}

// ---- release push (Prowlarr, autobrr) -----------------------------------------------------

/// Sonarr's `ReleaseResource` as pushed (`ReleasePushController`).
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PushBody {
    guid: Option<String>,
    title: Option<String>,
    download_url: Option<String>,
    magnet_url: Option<String>,
    info_url: Option<String>,
    info_hash: Option<String>,
    protocol: Option<Value>,
    publish_date: Option<String>,
    indexer: Option<String>,
    indexer_id: Option<i64>,
    size: Option<i64>,
    seeders: Option<i64>,
    leechers: Option<i64>,
    tvdb_id: Option<i64>,
    tv_rage_id: Option<i64>,
    imdb_id: Option<String>,
}

fn push_protocol(b: &PushBody) -> kitsunarr_domain::DownloadProtocol {
    use kitsunarr_domain::DownloadProtocol::{Torrent, Usenet};
    match &b.protocol {
        Some(Value::String(p)) if p.eq_ignore_ascii_case("usenet") => Usenet,
        Some(Value::String(p)) if p.eq_ignore_ascii_case("torrent") => Torrent,
        Some(Value::Number(n)) if n.as_i64() == Some(1) => Usenet,
        Some(Value::Number(n)) if n.as_i64() == Some(2) => Torrent,
        _ if b.magnet_url.is_some()
            || b.download_url.as_deref().is_some_and(|u| {
                u.starts_with("magnet:") || u.to_ascii_lowercase().ends_with(".torrent")
            }) =>
        {
            Torrent
        }
        _ => Usenet,
    }
}

/// `POST /release/push`: one release decided with the RSS rules, then grabbed when approved
/// or kept pending when a delay profile holds it back.
async fn push_release(
    State(state): State<AppState>,
    Json(b): Json<PushBody>,
) -> ApiResult<Json<Vec<Value>>> {
    let title = b
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ApiError::field("title", "The release needs a title."))?;
    let download_url = b.download_url.clone().filter(|u| !u.trim().is_empty());
    let magnet_url = b.magnet_url.clone().filter(|u| !u.trim().is_empty());
    if download_url.is_none() && magnet_url.is_none() {
        return Err(ApiError::field(
            "downloadUrl",
            "The release needs a download URL or a magnet link.",
        ));
    }
    let protocol = push_protocol(&b);
    // The indexer by id, else by name (Sonarr's `ResolveIndexer`).
    let indexers = providers::list(state.db.pool(), ProviderKind::Indexer).await?;
    let indexer = indexers
        .iter()
        .find(|p| b.indexer_id.is_some_and(|id| id > 0 && p.id == id))
        .or_else(|| {
            b.indexer
                .as_deref()
                .and_then(|n| indexers.iter().find(|p| p.name.eq_ignore_ascii_case(n)))
        });
    let (_, extras, _) = crate::routes::search::usable_indexers(&state, "enableRss").await?;
    let publish_date = b
        .publish_date
        .as_deref()
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let guid = b.guid.clone().filter(|g| !g.is_empty()).unwrap_or_else(|| {
        format!(
            "PUSH-{}",
            download_url
                .clone()
                .or(magnet_url.clone())
                .unwrap_or_default()
        )
    });
    let release = kitsunarr_indexers::IndexerRelease {
        indexer_id: indexer.map_or(0, |p| p.id),
        indexer: indexer
            .map(|p| p.name.clone())
            .or(b.indexer.clone())
            .unwrap_or_default(),
        release: kitsunarr_indexers::Release {
            guid,
            title: title.clone(),
            size: b.size.unwrap_or(0),
            download_url: download_url.clone().or(magnet_url.clone()),
            info_url: b.info_url.clone(),
            comment_url: None,
            publish_date,
            protocol,
            tvdb_id: b.tvdb_id.unwrap_or(0),
            tv_rage_id: b.tv_rage_id.unwrap_or(0),
            imdb_id: b.imdb_id.clone(),
            languages: Vec::new(),
            indexer_flags: 0,
            info_hash: b.info_hash.clone(),
            magnet_url,
            seeders: b.seeders,
            peers: match (b.seeders, b.leechers) {
                (Some(s), Some(l)) => Some(s + l),
                _ => None,
            },
        },
    };
    let processed = crate::rss::process(&state, std::slice::from_ref(&release), &extras)
        .await
        .map_err(ApiError::internal)?;
    let o = processed.outcomes.first().cloned().unwrap_or_default();
    let quality = kitsunarr_parser::parse_quality(&title);
    Ok(Json(vec![json!({
        "guid": release.release.guid,
        "title": title,
        "indexer": release.indexer,
        "indexerId": release.indexer_id,
        "protocol": protocol,
        "size": release.release.size,
        "publishDate": publish_date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "downloadUrl": release.release.download_url,
        "quality": quality_json(quality.quality.id),
        "seriesId": o.series_id,
        "approved": o.approved,
        "temporarilyRejected": o.temporarily_rejected,
        "rejected": !o.approved,
        "rejections": o.rejections,
        "downloadAllowed": o.series_id.is_some(),
    })]))
}

// ---- history --------------------------------------------------------------------------------

/// Sonarr's `EpisodeHistoryEventType` numbers.
const HISTORY_EVENTS: [&str; 8] = [
    "unknown",
    "grabbed",
    "seriesFolderImported",
    "downloadFolderImported",
    "downloadFailed",
    "episodeFileDeleted",
    "episodeFileRenamed",
    "downloadIgnored",
];

fn history_json(h: &kitsunarr_domain::api::library::HistoryItem) -> Value {
    let quality = h
        .quality_name
        .as_deref()
        .and_then(Quality::by_name)
        .map_or(0, |q| q.id);
    let data: Map<String, Value> = h
        .data
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    let v = match v {
                        Value::String(_) | Value::Null => v.clone(),
                        other => Value::String(other.to_string()),
                    };
                    (k.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": h.id,
        "episodeId": h.episode_id.unwrap_or(0),
        "seriesId": h.series_id.unwrap_or(0),
        "sourceTitle": h.source_title,
        "languages": [],
        "quality": quality_json(quality),
        "customFormats": [],
        "customFormatScore": 0,
        "qualityCutoffNotMet": false,
        "date": h.date,
        "downloadId": h.download_id,
        "eventType": h.event_type,
        "data": data,
    })
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct HistoryQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    sort_direction: Option<String>,
    event_type: Option<Value>,
    series_id: Option<i64>,
    episode_id: Option<i64>,
    date: Option<String>,
    download_id: Option<String>,
}

fn event_name(v: &Option<Value>) -> Option<String> {
    match v {
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|i| HISTORY_EVENTS.get(i as usize))
            .map(|s| s.to_string()),
        Some(Value::String(s)) => s
            .parse::<usize>()
            .ok()
            .and_then(|i| HISTORY_EVENTS.get(i))
            .map(|s| s.to_string())
            .or_else(|| {
                HISTORY_EVENTS
                    .iter()
                    .find(|e| e.eq_ignore_ascii_case(s))
                    .map(|s| s.to_string())
            }),
        _ => None,
    }
}

/// `GET /history`: paged, newest first.
async fn history(
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Value>> {
    let event = event_name(&q.event_type);
    if q.series_id.is_some() || q.episode_id.is_some() || q.download_id.is_some() {
        let all = filtered_history(&state, &q, event.as_deref()).await?;
        let total = all.len();
        let size = q.page_size.unwrap_or(10).max(1) as usize;
        let page = q.page.unwrap_or(1).max(1) as usize;
        let records: Vec<Value> = all.into_iter().skip((page - 1) * size).take(size).collect();
        return Ok(Json(
            json!({"page": page, "pageSize": size, "sortKey": "date", "sortDirection": "descending", "totalRecords": total, "records": records}),
        ));
    }
    let paging = kitsunarr_domain::paging::PageRequest {
        page: q.page,
        page_size: q.page_size,
        sort_key: None,
        sort_direction: (q.sort_direction.as_deref() == Some("ascending"))
            .then_some(kitsunarr_domain::paging::SortDirection::Asc),
        search: None,
    };
    let p = kitsunarr_db::history::list(&state.db, &paging, event.as_deref()).await?;
    Ok(Json(json!({
        "page": p.page, "pageSize": p.page_size, "sortKey": "date",
        "sortDirection": if paging.sort_direction.is_some() { "ascending" } else { "descending" },
        "totalRecords": p.total_records,
        "records": p.records.iter().map(history_json).collect::<Vec<_>>(),
    })))
}

/// Every history record matching the filters, newest first.
async fn filtered_history(
    state: &AppState,
    q: &HistoryQuery,
    event: Option<&str>,
) -> ApiResult<Vec<Value>> {
    let mut out = Vec::new();
    let mut page = 1;
    loop {
        let paging = kitsunarr_domain::paging::PageRequest {
            page: Some(page),
            page_size: Some(1000),
            ..Default::default()
        };
        let p = kitsunarr_db::history::list(&state.db, &paging, event).await?;
        let done = p.records.len() < 1000;
        for h in &p.records {
            if q.series_id.is_some_and(|s| h.series_id != Some(s))
                || q.episode_id.is_some_and(|e| h.episode_id != Some(e))
                || q.download_id
                    .as_deref()
                    .is_some_and(|d| h.download_id.as_deref() != Some(d))
                || q.date.as_deref().is_some_and(|d| {
                    match (
                        chrono::DateTime::parse_from_rfc3339(d),
                        chrono::DateTime::parse_from_rfc3339(&h.date),
                    ) {
                        (Ok(since), Ok(at)) => at < since,
                        _ => false,
                    }
                })
            {
                continue;
            }
            out.push(history_json(h));
        }
        if done || page >= 100 {
            break;
        }
        page += 1;
    }
    Ok(out)
}

/// `GET /history/series?seriesId=`: the series' records (unpaged).
async fn history_of_series(
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    if q.series_id.is_none() {
        return Err(ApiError::field("seriesId", "seriesId is required."));
    }
    let event = event_name(&q.event_type);
    Ok(Json(filtered_history(&state, &q, event.as_deref()).await?))
}

/// `GET /history/since?date=`: records since a date (unpaged).
async fn history_since(
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    if q.date.is_none() {
        return Err(ApiError::field("date", "date is required."));
    }
    let event = event_name(&q.event_type);
    Ok(Json(filtered_history(&state, &q, event.as_deref()).await?))
}

// ---- blocklist -----------------------------------------------------------------------------

async fn blocklist(
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Value>> {
    let paging = kitsunarr_domain::paging::PageRequest {
        page: q.page,
        page_size: q.page_size,
        ..Default::default()
    };
    let p = kitsunarr_db::blocklist::list(&state.db, &paging).await?;
    let records: Vec<Value> = p
        .records
        .iter()
        .map(|b| {
            let quality = b
                .quality
                .as_deref()
                .and_then(Quality::by_name)
                .map_or(0, |q| q.id);
            json!({
                "id": b.id, "seriesId": b.series_id, "episodeIds": b.episode_ids,
                "sourceTitle": b.source_title, "languages": [], "quality": quality_json(quality),
                "customFormats": [], "date": b.date, "protocol": b.protocol,
                "indexer": b.indexer, "message": b.message,
            })
        })
        .collect();
    Ok(Json(json!({
        "page": p.page, "pageSize": p.page_size, "sortKey": "date", "sortDirection": "descending",
        "totalRecords": p.total_records, "records": records,
    })))
}

async fn delete_blocklisted(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    kitsunarr_db::blocklist::delete(&state.db, &[id]).await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct BulkIds {
    ids: Vec<i64>,
}

async fn delete_blocklisted_bulk(
    State(state): State<AppState>,
    Json(b): Json<BulkIds>,
) -> ApiResult<Json<Value>> {
    kitsunarr_db::blocklist::delete(&state.db, &b.ids).await?;
    Ok(Json(json!({})))
}

// ---- download clients and connections (read-only) ------------------------------------------

fn provider_json(p: &ProviderRecord) -> Value {
    let settings = crate::routes::settings::redact(&p.settings);
    let fields: Vec<Value> = settings
        .iter()
        .map(|(k, v)| json!({"name": k, "value": v}))
        .collect();
    let mut v = json!({
        "id": p.id,
        "name": p.name,
        "implementation": p.implementation,
        "implementationName": p.implementation,
        "configContract": format!("{}Settings", p.implementation),
        "enable": p.enabled,
        "protocol": p.protocol,
        "priority": p.priority,
        "tags": p.tags,
        "fields": fields,
    });
    if p.kind == ProviderKind::Notification
        && let Some(o) = v.as_object_mut()
    {
        for (k, t) in &p.options {
            o.insert(k.clone(), t.clone());
        }
    }
    v
}

async fn download_clients(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        providers::list(state.db.pool(), ProviderKind::DownloadClient)
            .await?
            .iter()
            .map(provider_json)
            .collect(),
    ))
}

async fn download_client(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let p = providers::get(state.db.pool(), ProviderKind::DownloadClient, id).await?;
    Ok(Json(provider_json(&p)))
}

async fn notifications(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        providers::list(state.db.pool(), ProviderKind::Notification)
            .await?
            .iter()
            .map(provider_json)
            .collect(),
    ))
}

async fn notification(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let p = providers::get(state.db.pool(), ProviderKind::Notification, id).await?;
    Ok(Json(provider_json(&p)))
}

// ---- wanted ---------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct V3WantedQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    sort_direction: Option<String>,
    monitored: Option<bool>,
    include_series: Option<bool>,
}

impl V3WantedQuery {
    fn native(&self) -> kitsunarr_domain::api::wanted::WantedQuery {
        use kitsunarr_domain::paging::SortDirection;
        kitsunarr_domain::api::wanted::WantedQuery {
            page: self.page,
            page_size: self.page_size,
            sort_direction: match self.sort_direction.as_deref() {
                Some("ascending" | "asc") => Some(SortDirection::Asc),
                Some("descending" | "desc") => Some(SortDirection::Desc),
                _ => None,
            },
            monitored: self.monitored,
        }
    }
}

async fn wanted_json(
    state: &AppState,
    q: &V3WantedQuery,
    page: kitsunarr_domain::Page<kitsunarr_domain::api::wanted::WantedEpisode>,
) -> ApiResult<Value> {
    let mut series_cache: std::collections::HashMap<i64, Value> = std::collections::HashMap::new();
    let mut records = Vec::new();
    for w in &page.records {
        let mut r = json!({
            "id": w.episode_id,
            "seriesId": w.series_id,
            "tvdbId": 0,
            "seasonNumber": w.season_number,
            "episodeNumber": w.episode_number,
            "absoluteEpisodeNumber": w.absolute_episode_number,
            "title": w.title,
            "airDateUtc": w.air_date_utc,
            "airDate": w.air_date_utc.as_deref().map(|d| d.chars().take(10).collect::<String>()),
            "monitored": w.monitored,
            "hasFile": w.quality_name.is_some(),
            "lastSearchTime": w.last_search_at,
        });
        if q.include_series.unwrap_or(false) {
            if let std::collections::hash_map::Entry::Vacant(slot) = series_cache.entry(w.series_id)
            {
                slot.insert(series_json(&detail(state, w.series_id).await?));
            }
            r["series"] = series_cache[&w.series_id].clone();
        }
        records.push(r);
    }
    Ok(json!({
        "page": page.page, "pageSize": page.page_size, "sortKey": "airDateUtc",
        "sortDirection": if q.sort_direction.as_deref() == Some("ascending") { "ascending" } else { "descending" },
        "totalRecords": page.total_records, "records": records,
    }))
}

/// `GET /wanted/missing` (LunaSea, nzb360).
async fn wanted_missing(
    State(state): State<AppState>,
    Query(q): Query<V3WantedQuery>,
) -> ApiResult<Json<Value>> {
    let Json(page) =
        crate::routes::wanted::missing(State(state.clone()), Query(q.native())).await?;
    Ok(Json(wanted_json(&state, &q, page).await?))
}

/// `GET /wanted/cutoff`.
async fn wanted_cutoff(
    State(state): State<AppState>,
    Query(q): Query<V3WantedQuery>,
) -> ApiResult<Json<Value>> {
    let Json(page) =
        crate::routes::wanted::cutoff_unmet(State(state.clone()), Query(q.native())).await?;
    Ok(Json(wanted_json(&state, &q, page).await?))
}
