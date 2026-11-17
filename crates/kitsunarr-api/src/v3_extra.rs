//! The rest of Sonarr's `/api/v3` resources (most also served under `/api/v5`; `V5` picks
//! the version's status codes): auto tagging, custom filters, custom format and episode file
//! bulk edits, delay profile ordering, indexer flags, languages and the deprecated language
//! profile, single-episode reads of the calendar and wanted lists, command cancellation,
//! marking a grab failed, manual import re-evaluation, Tanukarr's anime grouping preview,
//! collection presentation and series folder, its quality presets and codec catalogue, and the
//! v3 metadata source and naming example settings.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use kitsunarr_db::library::series;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::settings::{
    AutoTag, CustomFilter, CustomFormatSpecification, CustomQualityCreate, MetadataSourcesSettings,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::routes::settings as native_settings;

/// The routes both API versions serve.
pub fn shared<const V5: bool>() -> Router<AppState> {
    Router::new()
        .route("/autotagging", get(list_auto_tags).post(create_auto_tag))
        .route("/autotagging/schema", get(auto_tag_schema))
        .route(
            "/autotagging/{id}",
            get(get_auto_tag)
                .put(update_auto_tag)
                .delete(delete_auto_tag::<V5>),
        )
        .route(
            "/customfilter",
            get(list_custom_filters).post(create_custom_filter),
        )
        .route(
            "/customfilter/{id}",
            get(get_custom_filter)
                .put(update_custom_filter)
                .delete(delete_custom_filter::<V5>),
        )
        .route(
            "/customformat/bulk",
            put(bulk_update_formats).delete(bulk_delete_formats::<V5>),
        )
        .route("/delayprofile/reorder/{id}", put(reorder_delay))
        .route("/indexerflag", get(indexer_flags))
        .route("/language/{id}", get(language))
        .route("/calendar/{id}", get(episode_by_id::<V5>))
        .route("/wanted/missing/{id}", get(episode_by_id::<V5>))
        .route("/wanted/cutoff/{id}", get(episode_by_id::<V5>))
        .route("/command/{id}", axum::routing::delete(cancel_command))
        .route("/history/failed/{id}", post(history_failed::<V5>))
        .route("/episode/{id}", put(update_episode::<V5>))
        .route("/episodefile/{id}", put(update_episode_file::<V5>))
        .route("/episodefile/bulk", put(bulk_update_episode_files::<V5>))
        .route("/rename/bulk", get(rename_bulk))
        .route("/manualimport", post(reprocess_manual_import))
        .route("/collection/presentation", get(collection_presentation))
        .route("/series/grouping", get(grouping_preview))
        .route("/series/{id}/folder", get(series_folder))
        .route("/qualitydefinition", post(create_quality))
        .route(
            "/qualitydefinition/{id}",
            axum::routing::delete(retire_quality),
        )
        .route("/qualitydefinition/{id}/restore", post(restore_quality))
        .route("/qualitydefinition/codecs", get(codecs))
        .route("/qualitydefinition/codecdefaults", get(codec_defaults))
}

/// `/api/v3` (with its deprecated and renamed resources).
pub fn router() -> Router<AppState> {
    shared::<false>()
        .route("/episodefile/editor", put(episode_file_editor))
        .route(
            "/languageprofile",
            get(language_profiles).post(save_language_profile),
        )
        .route("/languageprofile/schema", get(language_profile_schema))
        .route(
            "/languageprofile/{id}",
            get(get_language_profile)
                .put(save_language_profile)
                .delete(delete_language_profile),
        )
        .route("/qualitydefinition/limits", get(definition_limits))
        .route(
            "/config/metadatasource",
            get(get_metadata_source).put(put_metadata_source),
        )
        .route(
            "/config/metadatasource/{id}",
            get(get_metadata_source).put(put_metadata_source),
        )
        .route("/config/naming/examples", get(naming_examples))
}

/// Sonarr answers a v3 delete with 200 and an empty body, a v5 one with 204.
fn deleted<const V5: bool>() -> StatusCode {
    if V5 {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::OK
    }
}

// ---- auto tagging ---------------------------------------------------------------------------

/// Sonarr's auto tagging conditions with their fields (name, label, type).
/// A condition field: name, label and Sonarr field type.
type Field = (&'static str, &'static str, &'static str);

const AUTO_TAG_FIELDS: &[(&str, &str, &[Field])] = &[
    (
        "GenreSpecification",
        "Genre",
        &[("value", "Genre(s)", "tag")],
    ),
    ("MonitoredSpecification", "Monitored", &[]),
    (
        "NetworkSpecification",
        "Network",
        &[("value", "Network(s)", "tag")],
    ),
    (
        "OriginalCountrySpecification",
        "Original Country",
        &[("value", "Country", "tag")],
    ),
    (
        "OriginalLanguageSpecification",
        "Original Language",
        &[("value", "Language", "select")],
    ),
    (
        "QualityProfileSpecification",
        "Quality Profile",
        &[("value", "Quality Profile", "qualityProfile")],
    ),
    (
        "RootFolderSpecification",
        "Root Folder",
        &[("value", "Root Folder", "rootFolder")],
    ),
    (
        "SeriesTypeSpecification",
        "Series Type",
        &[("value", "Series Type", "select")],
    ),
    (
        "StatusSpecification",
        "Status",
        &[("status", "Status", "select")],
    ),
    ("TagSpecification", "Tag", &[("value", "Tag", "seriesTag")]),
    (
        "YearSpecification",
        "Year",
        &[
            ("min", "Minimum Year", "number"),
            ("max", "Maximum Year", "number"),
        ],
    ),
];

fn select_options(implementation: &str) -> Value {
    let opts = |list: &[(i64, &str)]| {
        list.iter()
            .enumerate()
            .map(|(i, (v, n))| json!({"value": v, "name": n, "order": i}))
            .collect::<Vec<_>>()
    };
    match implementation {
        "SeriesTypeSpecification" => json!(opts(&[(0, "Standard"), (1, "Daily"), (2, "Anime")])),
        "StatusSpecification" => json!(opts(&[
            (-1, "Deleted"),
            (0, "Continuing"),
            (1, "Ended"),
            (2, "Upcoming"),
        ])),
        "OriginalLanguageSpecification" => json!(
            kitsunarr_domain::language::Language::all()
                .iter()
                .enumerate()
                .map(|(i, l)| json!({"value": l.id, "name": l.name, "order": i}))
                .collect::<Vec<_>>()
        ),
        _ => Value::Null,
    }
}

/// Sonarr's `AutoTaggingSpecificationSchema` of a condition.
fn auto_spec_json(s: &CustomFormatSpecification) -> Value {
    let known = AUTO_TAG_FIELDS
        .iter()
        .find(|(i, _, _)| *i == s.implementation);
    let fields: Vec<Value> = known
        .map(|(_, _, defs)| {
            defs.iter()
                .enumerate()
                .map(|(n, (name, label, kind))| {
                    let mut f = json!({
                        "order": n, "name": name, "label": label,
                        "value": s.fields.get(*name).cloned().unwrap_or(Value::Null),
                        "type": kind, "advanced": false, "privacy": "normal", "isFloat": false,
                    });
                    let options = select_options(&s.implementation);
                    if !options.is_null() {
                        f["selectOptions"] = options;
                    }
                    f
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": 0,
        "name": s.name,
        "implementation": s.implementation,
        "implementationName": known.map_or(s.implementation.as_str(), |(_, n, _)| n),
        "negate": s.negate,
        "required": s.required,
        "fields": fields,
    })
}

fn auto_tag_json(t: &AutoTag) -> Value {
    json!({
        "id": t.id,
        "name": t.name,
        "removeTagsAutomatically": t.remove_tags_automatically,
        "tags": t.tags,
        "specifications": t.specifications.iter().map(auto_spec_json).collect::<Vec<_>>(),
    })
}

/// Fields as Sonarr sends them (`[{name, value}]`) or as an object.
pub(crate) fn field_map(v: &Value) -> Map<String, Value> {
    match v {
        Value::Array(list) => list
            .iter()
            .filter_map(|f| Some((f["name"].as_str()?.to_string(), f["value"].clone())))
            .filter(|(_, v)| !v.is_null())
            .collect(),
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    }
}

fn auto_tag_from(v: &Value, id: i64) -> ApiResult<AutoTag> {
    let mut specifications = Vec::new();
    for s in v["specifications"].as_array().into_iter().flatten() {
        let implementation = s["implementation"].as_str().unwrap_or_default();
        if !AUTO_TAG_FIELDS.iter().any(|(i, _, _)| *i == implementation) {
            return Err(ApiError::BadRequest(format!(
                "{implementation} is not a valid specification implementation"
            )));
        }
        specifications.push(CustomFormatSpecification {
            name: s["name"].as_str().unwrap_or_default().to_string(),
            implementation: implementation.to_string(),
            negate: s["negate"].as_bool().unwrap_or(false),
            required: s["required"].as_bool().unwrap_or(false),
            fields: field_map(&s["fields"]),
        });
    }
    Ok(AutoTag {
        id,
        name: v["name"].as_str().unwrap_or_default().to_string(),
        remove_tags_automatically: v["removeTagsAutomatically"].as_bool().unwrap_or(false),
        specifications,
        tags: v["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .collect(),
    })
}

async fn auto_tags(state: &AppState) -> ApiResult<Vec<AutoTag>> {
    Ok(kitsunarr_db::auto_tags::list(state.db.pool()).await?)
}

async fn auto_tag(state: &AppState, id: i64) -> ApiResult<AutoTag> {
    auto_tags(state)
        .await?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| ApiError::NotFound("The auto tag does not exist.".into()))
}

/// Sonarr's "Must be unique." name rule.
async fn unique_auto_tag_name(state: &AppState, t: &AutoTag) -> ApiResult<()> {
    if auto_tags(state)
        .await?
        .iter()
        .any(|o| o.id != t.id && o.name == t.name.trim())
    {
        return Err(ApiError::field("name", "Must be unique."));
    }
    Ok(())
}

async fn list_auto_tags(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        auto_tags(&state).await?.iter().map(auto_tag_json).collect(),
    ))
}

async fn get_auto_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(auto_tag_json(&auto_tag(&state, id).await?)))
}

async fn create_auto_tag(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let t = auto_tag_from(&v, 0)?;
    unique_auto_tag_name(&state, &t).await?;
    let (_, Json(saved)) = crate::routes::rules::create_auto_tag(State(state), Json(t)).await?;
    Ok((StatusCode::CREATED, Json(auto_tag_json(&saved))))
}

async fn update_auto_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    auto_tag(&state, id).await?;
    let t = auto_tag_from(&v, id)?;
    unique_auto_tag_name(&state, &t).await?;
    let _ = crate::routes::rules::update_auto_tag(State(state.clone()), Path(id), Json(t)).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(auto_tag_json(&auto_tag(&state, id).await?)),
    ))
}

async fn delete_auto_tag<const V5: bool>(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    auto_tag(&state, id).await?;
    crate::routes::rules::delete_auto_tag(State(state), Path(id)).await?;
    Ok(deleted::<V5>())
}

/// `GET /autotagging/schema`: every condition with empty fields.
async fn auto_tag_schema() -> Json<Vec<Value>> {
    Json(
        AUTO_TAG_FIELDS
            .iter()
            .map(|(implementation, _, _)| {
                auto_spec_json(&CustomFormatSpecification {
                    name: String::new(),
                    implementation: (*implementation).to_string(),
                    negate: false,
                    required: false,
                    fields: Map::new(),
                })
            })
            .collect(),
    )
}

// ---- custom filters -------------------------------------------------------------------------

fn custom_filter_from(v: Value, id: i64) -> ApiResult<CustomFilter> {
    let mut f: CustomFilter = serde_json::from_value(v)
        .map_err(|e| ApiError::BadRequest(format!("Invalid custom filter: {e}")))?;
    f.id = id;
    Ok(f)
}

async fn list_custom_filters(State(state): State<AppState>) -> ApiResult<Json<Vec<CustomFilter>>> {
    crate::routes::custom_filters::list_custom_filters(State(state)).await
}

async fn get_custom_filter(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<CustomFilter>> {
    Ok(Json(
        kitsunarr_db::custom_filters::get(state.db.pool(), id).await?,
    ))
}

async fn create_custom_filter(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<CustomFilter>)> {
    let f = custom_filter_from(v, 0)?;
    crate::routes::custom_filters::create_custom_filter(State(state), Json(f)).await
}

async fn update_custom_filter(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<CustomFilter>)> {
    let f = custom_filter_from(v, id)?;
    let Json(saved) =
        crate::routes::custom_filters::update_custom_filter(State(state), Path(id), Json(f))
            .await?;
    Ok((StatusCode::ACCEPTED, Json(saved)))
}

async fn delete_custom_filter<const V5: bool>(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    crate::routes::custom_filters::delete_custom_filter(State(state), Path(id)).await?;
    Ok(deleted::<V5>())
}

// ---- custom format bulk edits -----------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct FormatBulk {
    ids: Vec<i64>,
    include_custom_format_when_renaming: Option<bool>,
}

/// `PUT /customformat/bulk`: "include when renaming" for several formats.
async fn bulk_update_formats(
    State(state): State<AppState>,
    Json(b): Json<FormatBulk>,
) -> ApiResult<(StatusCode, Json<Vec<Value>>)> {
    if b.ids.is_empty() {
        return Err(ApiError::BadRequest("ids must be provided".into()));
    }
    let mut out = Vec::new();
    for id in &b.ids {
        let mut cf = crate::v3_settings::find_format(&state, *id).await?;
        if let Some(include) = b.include_custom_format_when_renaming {
            cf.include_when_renaming = include;
        }
        let Json(saved) =
            native_settings::update_custom_format(State(state.clone()), Path(*id), Json(cf))
                .await?;
        out.push(crate::v3_settings::format_json(&saved));
    }
    Ok((StatusCode::ACCEPTED, Json(out)))
}

async fn bulk_delete_formats<const V5: bool>(
    State(state): State<AppState>,
    Json(b): Json<FormatBulk>,
) -> ApiResult<Response> {
    for id in &b.ids {
        native_settings::delete_custom_format(State(state.clone()), Path(*id)).await?;
    }
    Ok(if V5 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Json(json!({})).into_response()
    })
}

// ---- delay profile order ----------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct AfterQuery {
    after: Option<i64>,
}

/// Sonarr's `DelayProfileService.Reorder` on `(id, order)` pairs; the default profile (the
/// last, `i32::MAX`) keeps its place. Returns the changed orders.
pub(crate) fn reorder(orders: &[(i64, i64)], id: i64, after: Option<i64>) -> Vec<(i64, i64)> {
    let mut all: Vec<(i64, i64)> = orders.to_vec();
    all.sort_by_key(|(_, o)| *o);
    let Some(&(_, moving_order)) = all.iter().find(|(i, _)| *i == id) else {
        return all;
    };
    let after_profile = after.and_then(|a| all.iter().find(|(i, _)| *i == a).copied());
    let after_order = match after_profile {
        None => 0,
        Some((_, o)) if moving_order < o => o - 1,
        Some((_, o)) => o,
    };
    let mut after_count = after_order + 2;
    for (pid, order) in all.iter_mut() {
        if *order >= i64::from(i32::MAX) {
            continue;
        }
        if *pid == id {
            *order = after_order + 1;
        } else if Some(*pid) == after_profile.map(|(i, _)| i) {
            *order = after_order;
        } else if *order > after_order {
            *order = after_count;
            after_count += 1;
        } else if *order > moving_order {
            *order -= 1;
        }
    }
    all
}

async fn reorder_delay(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<AfterQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let profiles = kitsunarr_db::rules::list_delay_profiles(state.db.pool()).await?;
    if !profiles.iter().any(|p| p.id == id) {
        return Err(ApiError::NotFound(
            "The delay profile does not exist.".into(),
        ));
    }
    let orders: Vec<(i64, i64)> = profiles.iter().map(|p| (p.id, p.sort_order)).collect();
    let next = reorder(&orders, id, q.after);
    let mut tx = state
        .db
        .pool()
        .begin()
        .await
        .map_err(kitsunarr_db::DbError::from)?;
    for p in &profiles {
        if let Some((_, order)) = next.iter().find(|(i, _)| *i == p.id)
            && *order != p.sort_order
        {
            let mut changed = p.clone();
            changed.sort_order = *order;
            kitsunarr_db::rules::update_delay_profile(&mut *tx, &changed).await?;
        }
    }
    tx.commit().await.map_err(kitsunarr_db::DbError::from)?;
    native_settings::settings_changed(&state, "delayProfiles");
    Ok(Json(
        kitsunarr_db::rules::list_delay_profiles(state.db.pool())
            .await?
            .iter()
            .map(crate::v3_admin::delay_json)
            .collect(),
    ))
}

// ---- indexer flags and languages ---------------------------------------------------------------

/// Sonarr's `IndexerFlags`.
const INDEXER_FLAGS: &[(i64, &str)] = &[
    (1, "Freeleech"),
    (2, "Halfleech"),
    (4, "DoubleUpload"),
    (8, "Internal"),
    (16, "Scene"),
    (128, "Nuked"),
    (256, "Subtitles"),
];

async fn indexer_flags() -> Json<Vec<Value>> {
    Json(
        INDEXER_FLAGS
            .iter()
            .map(|(id, name)| json!({"id": id, "name": name, "nameLower": name.to_lowercase()}))
            .collect(),
    )
}

async fn language(Path(id): Path<i32>) -> ApiResult<Json<Value>> {
    kitsunarr_domain::language::Language::by_id(id)
        .map(|l| Json(json!({"id": l.id, "name": l.name})))
        .ok_or_else(|| ApiError::NotFound("ID does not match a known language".into()))
}

/// Sonarr v4's only language profile (language profiles are deprecated).
fn language_profile() -> Value {
    json!({
        "id": 1,
        "name": "Deprecated",
        "upgradeAllowed": true,
        "cutoff": {"id": 1, "name": "English"},
        "languages": [{"language": {"id": 1, "name": "English"}, "allowed": true}],
    })
}

pub(crate) async fn language_profiles() -> Json<Vec<Value>> {
    Json(vec![language_profile()])
}

async fn get_language_profile(Path(_id): Path<i64>) -> Json<Value> {
    Json(language_profile())
}

async fn language_profile_schema() -> Json<Value> {
    Json(language_profile())
}

/// Sonarr accepts and ignores writes to the deprecated profile.
async fn save_language_profile(Json(v): Json<Value>) -> (StatusCode, Json<Value>) {
    (StatusCode::ACCEPTED, Json(v))
}

async fn delete_language_profile(Path(_id): Path<i64>) -> StatusCode {
    StatusCode::OK
}

// ---- episodes ---------------------------------------------------------------------------------

/// Sonarr's `EpisodeResource` of one episode with its series and file (`GetResourceById` of
/// the episode, calendar and wanted controllers).
pub(crate) async fn episode_full(state: &AppState, id: i64) -> ApiResult<Value> {
    let Json(mut e) = crate::v3::get_episode(State(state.clone()), Path(id)).await?;
    let series_id = e["seriesId"].as_i64().unwrap_or_default();
    let detail = crate::v3::detail(state, series_id).await?;
    e["series"] = crate::v3::series_json(&detail);
    let file_id = e["episodeFileId"].as_i64().unwrap_or_default();
    if file_id > 0
        && let Some(f) = crate::v3::files_of(state, series_id)
            .await?
            .into_iter()
            .find(|f| f["id"] == json!(file_id))
    {
        e["episodeFile"] = f;
    }
    e["images"] = json!([]);
    Ok(e)
}

async fn episode_by_id<const V5: bool>(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let e = episode_full(&state, id).await?;
    if V5 {
        let all = ["series", "episodefile", "images"].map(String::from).into();
        return Ok(Json(
            crate::v5::episodes_out(&state, vec![e], &all)
                .await?
                .remove(0),
        ));
    }
    Ok(Json(e))
}

#[derive(Deserialize)]
struct MonitoredBody {
    monitored: bool,
}

/// `PUT /episode/{id}`: Sonarr changes only `monitored`.
async fn update_episode<const V5: bool>(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(b): Json<MonitoredBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let series_id = crate::v3::sqlx_series_of_episode(&state, id).await?;
    series::set_episodes_monitored(&state.db, &[id], b.monitored).await?;
    state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: vec![series_id],
            deleted: false,
        });
    let Json(mut e) = crate::v3::get_episode(State(state), Path(id)).await?;
    if V5 {
        crate::v5::episode_v5(&mut e);
    }
    Ok((
        if V5 {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        },
        Json(e),
    ))
}

// ---- episode files ----------------------------------------------------------------------------

/// Sonarr's `ReleaseType` names.
fn release_type(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "unknown" => Some(0),
            "singleepisode" => Some(1),
            "multiepisode" => Some(2),
            "seasonpack" => Some(3),
            _ => None,
        },
        _ => None,
    }
}

/// The edit an `EpisodeFileResource` (or `EpisodeFileListResource`) asks for, as Sonarr applies
/// it: a scene name only when it is a scene title; `Original` is never a file's language.
pub(crate) fn file_edit(v: &Value) -> series::EpisodeFileEdit {
    let quality = v.get("quality").filter(|q| q.is_object()).and_then(|q| {
        let id = q["quality"]["id"].as_i64()?;
        let r = &q["revision"];
        Some((
            id,
            r["version"].as_i64().unwrap_or(1),
            r["real"].as_i64().unwrap_or(0),
            r["isRepack"].as_bool().unwrap_or(false),
        ))
    });
    let languages = v.get("languages").and_then(Value::as_array).map(|l| {
        l.iter()
            .filter_map(|x| x["id"].as_i64().or_else(|| x.as_i64()))
            .filter(|id| *id != -2)
            .map(|id| id as i32)
            .collect()
    });
    series::EpisodeFileEdit {
        quality,
        languages,
        scene_name: v["sceneName"]
            .as_str()
            .filter(|s| kitsunarr_parser::is_scene_title(s))
            .map(str::to_string),
        release_group: v["releaseGroup"].as_str().map(str::to_string),
        indexer_flags: v["indexerFlags"].as_i64(),
        release_type: release_type(&v["releaseType"]),
    }
}

async fn edited_file<const V5: bool>(state: &AppState, id: i64) -> ApiResult<Value> {
    if V5 {
        return crate::v5::file_v5(state, id).await;
    }
    let Json(v) = crate::v3::get_episode_file(State(state.clone()), Path(id)).await?;
    Ok(v)
}

fn files_changed(state: &AppState, series_id: Option<i64>) {
    if let Some(id) = series_id {
        state
            .events
            .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
                media_type: "series".into(),
                ids: vec![id],
                deleted: false,
            });
    }
}

/// `PUT /episodefile/{id}`: quality, scene name and release group.
async fn update_episode_file<const V5: bool>(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let mut edit = file_edit(&v);
    // As Sonarr's `SetQuality`: languages and flags are left alone here.
    edit.languages = None;
    edit.indexer_flags = None;
    edit.release_type = None;
    series::edit_episode_file(&state.db, id, &edit).await?;
    files_changed(&state, series::series_of_file(&state.db, id).await?);
    Ok((
        StatusCode::ACCEPTED,
        Json(edited_file::<V5>(&state, id).await?),
    ))
}

/// `PUT /episodefile/bulk`: each resource's own changes.
async fn bulk_update_episode_files<const V5: bool>(
    State(state): State<AppState>,
    Json(list): Json<Vec<Value>>,
) -> ApiResult<(StatusCode, Json<Vec<Value>>)> {
    let mut out = Vec::new();
    let mut series_id = None;
    for v in &list {
        let Some(id) = v["id"].as_i64() else { continue };
        // Files that do not exist are skipped, as Sonarr's `GetFiles`.
        match series::edit_episode_file(&state.db, id, &file_edit(v)).await {
            Ok(()) => {}
            Err(kitsunarr_db::DbError::NotFound) => continue,
            Err(e) => return Err(e.into()),
        }
        series_id = series::series_of_file(&state.db, id).await?;
        out.push(edited_file::<V5>(&state, id).await?);
    }
    files_changed(&state, series_id);
    Ok((
        if V5 {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        },
        Json(out),
    ))
}

/// `PUT /episodefile/editor` (deprecated): the same changes for several files.
async fn episode_file_editor(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Vec<Value>>)> {
    let ids: Vec<i64> = v["episodeFileIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .collect();
    let mut edit = file_edit(&v);
    edit.indexer_flags = None;
    edit.release_type = None;
    let mut out = Vec::new();
    let mut series_id = None;
    for id in ids {
        match series::edit_episode_file(&state.db, id, &edit).await {
            Ok(()) => {}
            Err(kitsunarr_db::DbError::NotFound) => continue,
            Err(e) => return Err(e.into()),
        }
        series_id = series::series_of_file(&state.db, id).await?;
        out.push(edited_file::<false>(&state, id).await?);
    }
    files_changed(&state, series_id);
    Ok((StatusCode::ACCEPTED, Json(out)))
}

// ---- rename and manual import ---------------------------------------------------------------

pub(crate) fn id_list(raw: Option<&str>) -> Vec<i64> {
    raw.unwrap_or_default()
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect()
}

/// `GET /rename/bulk?seriesIds=1,2`: the renames of several series.
async fn rename_bulk(
    State(state): State<AppState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult<Json<Vec<Value>>> {
    let ids = query_ids(raw.as_deref(), "seriesIds");
    if ids.is_empty() {
        return Err(ApiError::BadRequest("seriesIds must be provided".into()));
    }
    let mut out = Vec::new();
    for id in ids {
        let previews = crate::routes::library::rename_preview(
            State(state.clone()),
            Path(id),
            Query(crate::routes::library::RenameQuery {
                season_number: None,
            }),
        )
        .await?
        .0;
        out.extend(previews.into_iter().map(|r| {
            json!({
                "seriesId": id,
                "seasonNumber": r.season_number,
                "episodeNumbers": r.episode_numbers,
                "episodeFileId": r.episode_file_id,
                "existingPath": r.existing_path,
                "newPath": r.new_path,
            })
        }));
    }
    Ok(Json(out))
}

/// Ids of a query key given as `k=1,2`, `k=1&k=2` or `k[]=1`.
pub(crate) fn query_ids(raw: Option<&str>, key: &str) -> Vec<i64> {
    let mut out = Vec::new();
    for pair in raw.unwrap_or_default().split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = k.replace("%5B%5D", "[]");
        if k.eq_ignore_ascii_case(key) || k.eq_ignore_ascii_case(&format!("{key}[]")) {
            out.extend(id_list(Some(&v.replace("%2C", ","))));
        }
    }
    out
}

/// `POST /manualimport`: Sonarr's `ReprocessItems`, each file evaluated again for the series
/// and episodes chosen for it.
async fn reprocess_manual_import(
    State(state): State<AppState>,
    Json(mut items): Json<Vec<Value>>,
) -> ApiResult<Json<Vec<Value>>> {
    if items.is_empty() {
        return Err(ApiError::BadRequest("items must be provided".into()));
    }
    for item in items.iter_mut() {
        let path = item["path"].as_str().unwrap_or_default().to_string();
        let series_id = item["seriesId"].as_i64().filter(|i| *i > 0);
        let scanned = crate::routes::library_tools::manual_import_scan(
            State(state.clone()),
            Query(crate::routes::library_tools::ManualImportQuery {
                folder: path.clone(),
                series_id,
            }),
        )
        .await
        .map(|j| j.0)
        .unwrap_or_default()
        .into_iter()
        .find(|m| m.path == path);
        let chosen: Vec<i64> = item["episodeIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .collect();
        let episode_ids = if chosen.is_empty() {
            scanned
                .as_ref()
                .map(|m| m.episode_ids.clone())
                .unwrap_or_default()
        } else {
            chosen
        };
        let mut episodes = Vec::new();
        for id in &episode_ids {
            if let Ok(Json(e)) = crate::v3::get_episode(State(state.clone()), Path(*id)).await {
                episodes.push(e);
            }
        }
        if item["seasonNumber"].is_null() {
            item["seasonNumber"] = episodes
                .first()
                .map(|e| e["seasonNumber"].clone())
                .or_else(|| {
                    scanned
                        .as_ref()
                        .and_then(|m| m.season_number)
                        .map(|s| json!(s))
                })
                .unwrap_or(Value::Null);
        }
        item["episodes"] = json!(episodes);
        item["episodeIds"] = Value::Null;
        let mut rejections: Vec<Value> = scanned
            .as_ref()
            .map(|m| {
                m.rejections
                    .iter()
                    .map(|r| json!({"reason": r, "type": "permanent"}))
                    .collect()
            })
            .unwrap_or_default();
        if scanned.is_none() {
            rejections.push(json!({"reason": "The file could not be read.", "type": "permanent"}));
        }
        if series_id.is_none() {
            rejections.push(json!({"reason": "Series must be chosen", "type": "permanent"}));
        } else if episodes.is_empty() {
            rejections.push(json!({"reason": "Episodes must be chosen", "type": "permanent"}));
        }
        item["rejections"] = json!(rejections);
        item["customFormats"] = json!([]);
        item["customFormatScore"] = json!(0);
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if item["quality"]["quality"]["id"].as_i64().unwrap_or(0) == 0 {
            let q = kitsunarr_parser::parse_quality(&name);
            item["quality"] = json!({
                "quality": crate::v3::quality_json(q.quality.id),
                "revision": {"version": q.revision.version, "real": q.revision.real, "isRepack": q.revision.is_repack},
            });
        }
        if item["releaseGroup"]
            .as_str()
            .is_none_or(|g| g.trim().is_empty())
        {
            item["releaseGroup"] = json!(scanned.as_ref().and_then(|m| m.release_group.clone()));
        }
        if item["languages"].as_array().is_none_or(|l| l.len() <= 1)
            && item["languages"][0]["id"].as_i64().unwrap_or(0) == 0
        {
            let languages: Vec<Value> = kitsunarr_parser::parse_title(&name)
                .map(|p| {
                    p.languages
                        .iter()
                        .map(|l| json!({"id": l.id, "name": l.name}))
                        .collect()
                })
                .unwrap_or_default();
            if !languages.is_empty() {
                item["languages"] = json!(languages);
            }
        }
    }
    Ok(Json(items))
}

// ---- commands and history ---------------------------------------------------------------------

/// `DELETE /command/{id}`: Sonarr cancels a command still waiting in its queue; Kitsunarr starts
/// every command at once, so none can be cancelled (Sonarr's 409).
async fn cancel_command(State(state): State<AppState>, Path(id): Path<i64>) -> ApiError {
    let _ = state.commands.get(id);
    ApiError::Conflict("Unable to cancel task".into())
}

/// `POST /history/failed/{id}`: marks a grab as failed (blocklisting the release and searching
/// again as configured).
async fn history_failed<const V5: bool>(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let _ = crate::routes::queue::mark_history_failed(State(state), Path(id)).await?;
    Ok(if V5 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Json(json!({})).into_response()
    })
}

// ---- Tanukarr's grouping, presentation and folders ----------------------------------------

/// A group id as Tanukarr validates it.
fn valid_group_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// The catalogue and id behind a group id (`anilist:1`, `mal:1`, `tmdb:1`).
fn group_source(id: &str) -> Option<(MetadataProvider, &str)> {
    let (prefix, rest) = id.split_once(':')?;
    let provider = match prefix.to_ascii_lowercase().as_str() {
        "anilist" => MetadataProvider::AniList,
        "mal" | "myanimelist" => MetadataProvider::MyAnimeList,
        "tmdb" | "tmdbmovie" => MetadataProvider::TmdbMovie,
        _ => return None,
    };
    Some((provider, rest))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct GroupQuery {
    group_id: String,
}

/// `GET /collection/presentation?groupId=`: an anime group's overview and artwork.
async fn collection_presentation(
    State(state): State<AppState>,
    Query(q): Query<GroupQuery>,
) -> ApiResult<Response> {
    if !valid_group_id(&q.group_id) {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let Some((provider, id)) = group_source(&q.group_id) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let row: Option<(Option<String>, String)> = kitsunarr_db::sqlx::query_as(
        "SELECT c.overview, c.images FROM collections c WHERE c.metadata_provider = ? AND c.metadata_id = ? \
         AND EXISTS (SELECT 1 FROM collection_members m WHERE m.collection_id = c.id AND m.series_id IS NOT NULL)",
    )
    .bind(provider.as_str())
    .bind(id)
    .fetch_optional(state.db.pool())
    .await
    .map_err(kitsunarr_db::DbError::from)?;
    let Some((overview, images)) = row else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let images: Vec<Value> = serde_json::from_str::<Vec<Value>>(&images)
        .unwrap_or_default()
        .into_iter()
        .map(|i| {
            let url = i["remoteUrl"].clone();
            json!({"coverType": i["coverType"], "url": url, "remoteUrl": url})
        })
        .collect();
    Ok(Json(json!({"overview": overview, "images": images})).into_response())
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct GroupingQuery {
    metadata_source: Option<Value>,
    foreign_id: String,
}

/// A Tanukarr `MetadataSourceType` (name or number).
pub(crate) fn metadata_source(v: &Value) -> Option<MetadataProvider> {
    const BY_NUMBER: [MetadataProvider; 7] = [
        MetadataProvider::Tvdb,
        MetadataProvider::TmdbTv,
        MetadataProvider::AniList,
        MetadataProvider::MyAnimeList,
        MetadataProvider::TvMaze,
        MetadataProvider::TmdbMovie,
        MetadataProvider::Omdb,
    ];
    match v {
        Value::Number(n) => n
            .as_u64()
            .and_then(|i| BY_NUMBER.get(usize::try_from(i).ok()?).copied()),
        Value::String(s) => match s.parse::<usize>() {
            Ok(i) => BY_NUMBER.get(i).copied(),
            Err(_) => BY_NUMBER
                .iter()
                .copied()
                .find(|p| p.as_str().eq_ignore_ascii_case(s)),
        },
        _ => None,
    }
}

fn grouping_result(status: &str) -> Value {
    json!({
        "animeGroupingMode": "automatic",
        "status": status,
        "animeGroupId": null,
        "animeGroupTitle": null,
        "animeGroupSeasonNumber": null,
        "animeGroupFolder": null,
        "animeGroupMemberKind": "season",
        "animeGroupKind": "series",
        "animeGroupMovieFolderStyle": "automatic",
        "animeGroupMovieNumber": null,
        "animeGroupSpecialNumber": null,
        "animeGroupMemberFolder": null,
        "isResolved": false,
    })
}

/// `GET /series/grouping?metadataSource=&foreignId=`: the anime group an AniList or
/// MyAnimeList title would join (Tanukarr's grouping preview), with its folders.
async fn grouping_preview(
    State(state): State<AppState>,
    Query(q): Query<GroupingQuery>,
) -> ApiResult<Json<Value>> {
    let source = q.metadata_source.as_ref().and_then(metadata_source);
    let id = q.foreign_id.trim().parse::<i64>().ok().filter(|i| *i > 0);
    let (Some(source), Some(id)) = (source, id) else {
        return Err(ApiError::BadRequest(
            "A valid metadata source and positive catalogue ID are required".into(),
        ));
    };
    if !matches!(
        source,
        MetadataProvider::AniList | MetadataProvider::MyAnimeList
    ) {
        return Ok(Json(grouping_result("unsupported")));
    }
    use kitsunarr_library::grouping::{MemberKind, Resolution, resolve};
    let metadata = state.metadata.clone();
    let resolved = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        if source == MetadataProvider::AniList {
            resolve(id, |n| metadata.anime_relations(n)).await
        } else {
            resolve(id, |n| metadata.mal_relations(n)).await
        }
    })
    .await;
    let g = match resolved {
        Ok(Ok(Resolution::Resolved(g))) => g,
        Ok(Ok(Resolution::Unrelated)) => return Ok(Json(grouping_result("unrelated"))),
        Ok(Ok(Resolution::Unsupported)) => return Ok(Json(grouping_result("unsupported"))),
        Ok(Ok(Resolution::Ambiguous(_))) => return Ok(Json(grouping_result("ambiguous"))),
        Ok(Ok(Resolution::LimitReached)) => return Ok(Json(grouping_result("limitReached"))),
        _ => return Ok(Json(grouping_result("unavailable"))),
    };
    let naming: kitsunarr_domain::api::settings::NamingSettings =
        kitsunarr_db::settings::get_or_default(state.db.pool(), kitsunarr_db::settings::KEY_NAMING)
            .await?;
    let movie = g.member_kind == MemberKind::Movie;
    let member = kitsunarr_library::file_names::Series {
        title: g.title.clone(),
        series_type: kitsunarr_domain::SeriesType::Anime,
        group: Some(kitsunarr_library::file_names::GroupMembership {
            title: g.title.clone(),
            season: (!movie).then_some(g.ordinal),
            movie: movie.then_some(g.ordinal),
        }),
        ..Default::default()
    };
    let group_folder = kitsunarr_library::file_names::group_folder(
        &naming.anime_group_folder_format,
        &member,
        &naming,
    );
    let member_folder = kitsunarr_library::file_names::group_folder(
        if movie {
            &naming.anime_group_movie_folder_format
        } else {
            &naming.anime_group_season_folder_format
        },
        &member,
        &naming,
    );
    let group_id = if source == MetadataProvider::MyAnimeList {
        format!("mal:{}", g.root)
    } else {
        g.id.clone()
    };
    let mut r = grouping_result("resolved");
    r["animeGroupId"] = json!(group_id);
    r["animeGroupTitle"] = json!(g.title);
    r["animeGroupFolder"] = json!(group_folder);
    r["animeGroupMemberFolder"] = json!(member_folder);
    r["animeGroupMemberKind"] = json!(if movie { "movie" } else { "season" });
    if movie {
        r["animeGroupMovieNumber"] = json!(g.ordinal);
    } else {
        r["animeGroupSeasonNumber"] = json!(g.ordinal);
    }
    r["isResolved"] = json!(true);
    Ok(Json(r))
}

/// `GET /series/{id}/folder`: the series' folder name under the naming settings.
async fn series_folder(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let d = crate::v3::detail(&state, id).await?;
    let naming: kitsunarr_domain::api::settings::NamingSettings =
        kitsunarr_db::settings::get_or_default(state.db.pool(), kitsunarr_db::settings::KEY_NAMING)
            .await?;
    let folder = kitsunarr_library::file_names::series_folder(
        &kitsunarr_library::file_names::Series::from_parts(
            &d.item.title,
            d.item.year,
            d.item.series_type,
            &d.external_ids,
        ),
        &naming,
    );
    Ok(Json(json!({"folder": folder})))
}

// ---- quality presets --------------------------------------------------------------------------

/// `POST /qualitydefinition`: a custom quality preset (Tanukarr).
async fn create_quality(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let source = match v.get("source") {
        None | Some(Value::Null) => None,
        Some(s) => Some(
            serde_json::from_value(s.clone())
                .map_err(|_| ApiError::field("source", "Unknown quality source."))?,
        ),
    };
    let create = CustomQualityCreate {
        title: v["title"].as_str().unwrap_or_default().to_string(),
        source,
        resolution: v["resolution"].as_i64().map(|r| r as i32),
        codec: v["codec"].as_str().unwrap_or_default().to_string(),
        title_pattern: v["titlePattern"].as_str().unwrap_or_default().to_string(),
        copy_from_id: v["copyFromId"].as_i64().map(|i| i as i32),
    };
    let (_, Json(d)) =
        native_settings::create_custom_quality(State(state.clone()), Json(create)).await?;
    let Json(saved) = crate::v3_settings::get_definition(State(state), Path(d.quality_id)).await?;
    Ok((StatusCode::CREATED, Json(saved)))
}

/// `DELETE /qualitydefinition/{id}`: retires the preset.
async fn retire_quality(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    native_settings::retire_quality(State(state), Path(id)).await
}

async fn restore_quality(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<Value>> {
    native_settings::restore_quality(State(state.clone()), Path(id)).await?;
    crate::v3_settings::get_definition(State(state), Path(id)).await
}

/// Tanukarr's `CodecSizeNormalizer.MultiplierCatalog`.
const CODEC_CATALOG: &[(&str, &str)] = &[
    ("MPEG-1", "MPEG1"),
    ("MPEG-2 / H.262 / x262", "MPEG2"),
    ("MPEG-4 ASP / XviD / DivX", "MPEG4ASP"),
    ("H.263", "H263"),
    ("H.264 / AVC / x264", "H264"),
    ("H.265 / HEVC / x265", "HEVC"),
    ("AV1", "AV1"),
    ("H.266 / VVC / x266", "VVC"),
    ("VC-1 / WMV3", "VC1"),
    ("WMV1 / WMV2", "WMV1"),
    ("VP6 / VP7", "VP6"),
    ("VP8", "VP8"),
    ("VP9", "VP9"),
    ("Theora", "Theora"),
    ("Unknown / mixed / lossless", "Unknown"),
];

async fn codecs() -> Json<Map<String, Value>> {
    Json(
        CODEC_CATALOG
            .iter()
            .map(|(label, family)| {
                (
                    (*label).to_string(),
                    json!(kitsunarr_domain::codec::default_factor(family)),
                )
            })
            .collect(),
    )
}

/// `CodecSizeNormalizer.DefaultMultipliers` (by family).
async fn codec_defaults() -> Json<Map<String, Value>> {
    Json(
        kitsunarr_domain::codec::DEFAULTS
            .iter()
            .map(|(family, factor)| ((*family).to_string(), json!(factor)))
            .collect(),
    )
}

/// `QualityDefinitionLimits`: sizes are 0 to 1000 MiB per minute.
async fn definition_limits() -> Json<Value> {
    Json(json!({"min": 0, "max": 1000}))
}

// ---- metadata sources (v3 config) -----------------------------------------------------------

/// Every catalogue, in Tanukarr's `MetadataSourceType` order.
pub(crate) const ALL_SOURCES: [MetadataProvider; 7] = [
    MetadataProvider::Tvdb,
    MetadataProvider::TmdbTv,
    MetadataProvider::AniList,
    MetadataProvider::MyAnimeList,
    MetadataProvider::TvMaze,
    MetadataProvider::TmdbMovie,
    MetadataProvider::Omdb,
];

/// Tanukarr's `MetadataSourcePolicy.Order`: the stored order, then the remaining catalogues.
pub(crate) fn source_order(stored: &[MetadataProvider]) -> Vec<MetadataProvider> {
    let mut out: Vec<MetadataProvider> = Vec::new();
    for p in stored.iter().chain(ALL_SOURCES.iter()) {
        if !out.contains(p) {
            out.push(*p);
        }
    }
    out
}

/// Tanukarr's `MetadataSourceConfigResource` (`MetadataSourceSettingsResource` in v5).
pub(crate) fn metadata_source_json(s: &MetadataSourcesSettings, v5: bool) -> Value {
    let configured = |k: &Option<String>| k.as_ref().is_some_and(|k| !k.trim().is_empty());
    let redacted = |k: &Option<String>| {
        if configured(k) { "********" } else { "" }
    };
    let mut out = json!({
        "id": 1,
        "metadataSourceOrder": source_order(&s.source_order),
        "disabledMetadataSources": s.disabled_sources,
        "tmdbApiKeyConfigured": configured(&s.tmdb_api_key),
        "omdbApiKeyConfigured": configured(&s.omdb_api_key),
        "omdbApiKey": redacted(&s.omdb_api_key),
        "tmdbApiKey": redacted(&s.tmdb_api_key),
    });
    if v5 {
        out["preferredMetadataLanguage"] = json!(s.preferred_metadata_language.unwrap_or(1));
    }
    out
}

fn source_list(v: &Value, field: &str) -> ApiResult<Option<Vec<MetadataProvider>>> {
    match v.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| {
                metadata_source(i).ok_or_else(|| {
                    ApiError::field(field, "Choose valid metadata sources only once.")
                })
            })
            .collect::<ApiResult<Vec<_>>>()
            .map(Some),
        Some(_) => Err(ApiError::field(
            field,
            "Choose valid metadata sources only once.",
        )),
    }
}

/// Tanukarr's `MetadataSourcePolicy.Save` and key handling (`********` keeps a key), over the
/// stored settings.
pub(crate) fn apply_metadata_source(
    stored: &MetadataSourcesSettings,
    v: &Value,
) -> ApiResult<MetadataSourcesSettings> {
    let order = source_list(v, "metadataSourceOrder")?;
    let disabled = source_list(v, "disabledMetadataSources")?;
    let effective_order = order
        .clone()
        .unwrap_or_else(|| source_order(&stored.source_order));
    let effective_disabled = disabled
        .clone()
        .unwrap_or_else(|| stored.disabled_sources.clone());
    let distinct = |l: &[MetadataProvider]| l.iter().enumerate().all(|(i, p)| !l[..i].contains(p));
    if effective_order.is_empty() || !distinct(&effective_order) {
        return Err(ApiError::field(
            "metadataSourceOrder",
            "Choose each metadata source only once.",
        ));
    }
    if !distinct(&effective_disabled) {
        return Err(ApiError::field(
            "disabledMetadataSources",
            "Choose valid metadata sources only once.",
        ));
    }
    if ALL_SOURCES.iter().all(|p| effective_disabled.contains(p)) {
        return Err(ApiError::field(
            "disabledMetadataSources",
            "Enable at least one metadata source.",
        ));
    }
    let mut next = stored.clone();
    if order.is_some() {
        next.source_order = source_order(&effective_order);
    }
    if disabled.is_some() {
        next.disabled_sources = effective_disabled;
    }
    for (field, slot) in [
        ("omdbApiKey", &mut next.omdb_api_key),
        ("tmdbApiKey", &mut next.tmdb_api_key),
    ] {
        if let Some(k) = v[field].as_str()
            && k != "********"
        {
            *slot = Some(k.trim().to_string()).filter(|k| !k.is_empty());
        }
    }
    if let Some(l) = v["preferredMetadataLanguage"].as_i64() {
        next.preferred_metadata_language = Some(l as i32);
    }
    Ok(next)
}

async fn stored_sources(state: &AppState) -> ApiResult<MetadataSourcesSettings> {
    Ok(kitsunarr_db::settings::get_or_default(
        state.db.pool(),
        kitsunarr_db::settings::KEY_METADATA_SOURCES,
    )
    .await?)
}

/// Saves through the native handler (validation, keys applied to the catalogue clients).
pub(crate) async fn save_metadata_sources(
    state: &AppState,
    next: MetadataSourcesSettings,
) -> ApiResult<MetadataSourcesSettings> {
    let _ = native_settings::put_metadata_sources(State(state.clone()), Json(next)).await?;
    stored_sources(state).await
}

async fn get_metadata_source(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(metadata_source_json(
        &stored_sources(&state).await?,
        false,
    )))
}

async fn put_metadata_source(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let next = apply_metadata_source(&stored_sources(&state).await?, &v)?;
    let saved = save_metadata_sources(&state, next).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(metadata_source_json(&saved, false)),
    ))
}

// ---- naming examples ----------------------------------------------------------------------

/// Query values as JSON (`true`, numbers, text).
pub(crate) fn query_json(raw: Option<&str>) -> Value {
    let mut out = Map::new();
    for pair in raw.unwrap_or_default().split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let decode = |s: &str| {
            url::form_urlencoded::parse(format!("x={s}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        };
        let v = decode(v);
        let value = match v.as_str() {
            "true" | "True" => json!(true),
            "false" | "False" => json!(false),
            _ => v.parse::<i64>().map_or_else(|_| json!(v), |n| json!(n)),
        };
        out.insert(decode(k), value);
    }
    Value::Object(out)
}

/// Sonarr's `FileNameSampleService` samples under the given naming settings.
pub(crate) fn naming_examples_for(n: &kitsunarr_domain::api::settings::NamingSettings) -> Value {
    use kitsunarr_domain::quality::{QualityModel, Revision};
    use kitsunarr_library::file_names::{self as names, Episode, File, GroupMembership, Series};
    let series = |series_type| Series {
        title: "The Series Title's!".into(),
        year: 2010,
        series_type,
        imdb_id: Some("tt12345".into()),
        tvdb_id: 12345,
        tvmaze_id: 54321,
        tmdb_id: 11223,
        season_titles: vec![(1, "Part 1".into())],
        group: None,
        season_years: vec![(1, 2013)],
    };
    let standard = series(kitsunarr_domain::SeriesType::Standard);
    let daily = series(kitsunarr_domain::SeriesType::Daily);
    let mut anime = series(kitsunarr_domain::SeriesType::Anime);
    anime.group = Some(GroupMembership {
        title: "The Anime Group Title".into(),
        season: Some(2),
        movie: None,
    });
    let episode = |n: i32| Episode {
        season_number: 1,
        episode_number: n,
        absolute_episode_number: Some(n),
        title: format!("Episode Title ({n})"),
        air_date: (n == 1).then(|| "2013-10-30".to_string()),
    };
    let single = vec![episode(1)];
    let multi = vec![episode(1), episode(2), episode(3)];
    let file = |scene: &str| File {
        scene_name: Some(scene.into()),
        relative_path: Some(format!("{scene}.mkv")),
        release_group: Some("RlsGrp".into()),
        quality: QualityModel {
            quality_id: 3,
            revision: Revision {
                version: 2,
                real: 0,
                is_repack: false,
            },
        },
        quality_title: "WEBDL-1080p".into(),
        custom_formats: vec![("Surround Sound".into(), true), ("x264".into(), true)],
        ..Default::default()
    };
    let name = |episodes: &[Episode], s: &Series, f: &File| -> Value {
        names::build_file_name(episodes, s, f, "", n)
            .ok()
            .filter(|x| !x.is_empty())
            .map_or(Value::Null, Value::String)
    };
    let folder = |format: &str, s: &Series| -> Value {
        if format.trim().is_empty() {
            Value::Null
        } else {
            json!(names::group_folder(format, s, n))
        }
    };
    let group = |title: &str, group: &str, movie: bool| Series {
        title: title.into(),
        year: 2010,
        series_type: kitsunarr_domain::SeriesType::Anime,
        group: Some(GroupMembership {
            title: group.into(),
            season: (!movie).then_some(2),
            movie: movie.then_some(2),
        }),
        ..Default::default()
    };
    let movie_group = group("The Movie Title", "The Anime Group Title", true);
    let movie = Series {
        title: "The Movie Title".into(),
        year: 2010,
        imdb_id: Some("tt1234567".into()),
        ..Default::default()
    };
    json!({
        "singleEpisodeExample": name(&single, &standard, &file("The.Series.Title's!.S01E01.1080p.WEBDL.x264-EVOLVE")),
        "multiEpisodeExample": name(&multi, &standard, &file("The.Series.Title's!.S01E01-E03.1080p.WEBDL.x264-EVOLVE")),
        "dailyEpisodeExample": name(&single, &daily, &file("The.Series.Title's!.2013.10.30.1080p.WEBDL.x264-EVOLVE")),
        "animeEpisodeExample": name(&single, &anime, &file("[RlsGroup] The Series Title's! - 001 [1080P]")),
        "animeMultiEpisodeExample": name(&multi, &anime, &file("[RlsGroup] The Series Title's! - 001 - 103 [1080p]")),
        "seriesFolderExample": if n.series_folder_format.trim().is_empty() { Value::Null } else { json!(names::series_folder(&standard, n)) },
        "seasonFolderExample": if n.season_folder_format.trim().is_empty() { Value::Null } else { json!(names::season_folder(&standard, 1, n)) },
        "specialsFolderExample": if n.specials_folder_format.trim().is_empty() { Value::Null } else { json!(names::season_folder(&standard, 0, n)) },
        "animeGroupFolderExample": folder(&n.anime_group_folder_format, &anime),
        "animeGroupSeasonFolderExample": folder(&n.anime_group_season_folder_format, &anime),
        "animeGroupArcFolderExample": folder(&n.anime_group_arc_folder_format, &group("The Anime Group Title: War of Three Titans Arc", "The Anime Group Title", false)),
        "animeGroupMovieFolderExample": folder(&n.anime_group_movie_folder_format, &movie_group),
        "animeGroupMovieSagaFolderExample": folder(&n.anime_group_movie_saga_folder_format, &group("The Movie Title", "The Movie Saga", true)),
        "movieExample": names::build_movie_file_name(&movie, &file("The.Movie.Title.2010.1080p.WEBDL.x264-EVOLVE"), "", n).ok().map_or(Value::Null, Value::String),
        "animeGroupMovieEpisodeExample": name(&single, &movie_group, &file("[RlsGroup] The Series Title's! - 001 [1080P]")),
    })
}

/// `GET /config/naming/examples`: the examples of the settings in the query (the stored ones
/// when the query has no id).
pub(crate) async fn naming_examples(
    State(state): State<AppState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult<Json<Value>> {
    let stored: kitsunarr_domain::api::settings::NamingSettings =
        kitsunarr_db::settings::get_or_default(state.db.pool(), kitsunarr_db::settings::KEY_NAMING)
            .await?;
    let q = query_json(raw.as_deref());
    let naming = if q["id"].as_i64().unwrap_or(0) == 0 {
        stored
    } else {
        crate::v3_config::merged(&stored, &q, crate::v3_config::NAMING)?
    };
    Ok(Json(naming_examples_for(&naming)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_profiles_reorder_like_sonarr() {
        let all = [(1, i64::from(i32::MAX)), (2, 1), (3, 2), (4, 3)];
        // Move 4 to the top.
        let r = reorder(&all, 4, None);
        let order = |id: i64| r.iter().find(|(i, _)| *i == id).unwrap().1;
        assert_eq!((order(4), order(2), order(3)), (1, 2, 3));
        assert_eq!(order(1), i64::from(i32::MAX), "the default stays last");
        // Move 2 after 3.
        let r = reorder(&all, 2, Some(3));
        let order = |id: i64| r.iter().find(|(i, _)| *i == id).unwrap().1;
        assert_eq!((order(3), order(2), order(4)), (1, 2, 3));
    }

    #[test]
    fn metadata_source_policy_validates_and_appends() {
        let stored = MetadataSourcesSettings {
            tmdb_api_key: Some("k".into()),
            ..Default::default()
        };
        let r = metadata_source_json(&stored, false);
        assert_eq!(r["metadataSourceOrder"].as_array().unwrap().len(), 7);
        assert_eq!(r["metadataSourceOrder"][0], "tvdb");
        assert_eq!(r["tmdbApiKey"], "********");
        assert_eq!(r["omdbApiKey"], "");
        let next = apply_metadata_source(
            &stored,
            &json!({"metadataSourceOrder": ["aniList", 0], "disabledMetadataSources": ["omdb"], "tmdbApiKey": "********"}),
        )
        .unwrap();
        assert_eq!(
            next.source_order[..2],
            [MetadataProvider::AniList, MetadataProvider::Tvdb]
        );
        assert_eq!(next.source_order.len(), 7);
        assert_eq!(next.disabled_sources, [MetadataProvider::Omdb]);
        assert_eq!(next.tmdb_api_key.as_deref(), Some("k"));
        assert!(
            apply_metadata_source(&stored, &json!({"metadataSourceOrder": ["tvdb", "tvdb"]}))
                .is_err()
        );
        assert!(
            apply_metadata_source(&stored, &json!({"disabledMetadataSources": ALL_SOURCES}))
                .is_err()
        );
        assert!(apply_metadata_source(&stored, &json!({"metadataSourceOrder": ["nope"]})).is_err());
    }

    #[test]
    fn episode_file_edits_follow_sonarr_rules() {
        let e = file_edit(&json!({
            "quality": {"quality": {"id": 7}, "revision": {"version": 2, "real": 0, "isRepack": true}},
            "languages": [{"id": 1}, {"id": -2}],
            "sceneName": "not a scene name",
            "releaseGroup": "GRP",
            "releaseType": "seasonPack",
        }));
        assert_eq!(e.quality, Some((7, 2, 0, true)));
        assert_eq!(e.languages, Some(vec![1]));
        assert_eq!(e.scene_name, None);
        assert_eq!(e.release_group.as_deref(), Some("GRP"));
        assert_eq!(e.release_type, Some(3));
        let e = file_edit(&json!({"sceneName": "The.Series.S01E01.1080p.WEB-DL.x264-GRP"}));
        assert!(e.scene_name.is_some() && e.quality.is_none() && e.languages.is_none());
    }

    #[test]
    fn naming_examples_use_sonarr_samples() {
        let n = kitsunarr_domain::api::settings::NamingSettings {
            rename_episodes: true,
            ..Default::default()
        };
        let ex = naming_examples_for(&n);
        assert!(
            ex["singleEpisodeExample"]
                .as_str()
                .unwrap()
                .contains("S01E01"),
            "{ex}"
        );
        assert!(
            ex["seriesFolderExample"]
                .as_str()
                .unwrap()
                .contains("The Series Title")
        );
        assert!(ex["dailyEpisodeExample"].as_str().unwrap().contains("2013"));
        let q = query_json(Some(
            "id=1&renameEpisodes=true&standardEpisodeFormat=%7BSeries%20Title%7D+x&multiEpisodeStyle=2",
        ));
        assert_eq!(q["renameEpisodes"], true);
        assert_eq!(q["multiEpisodeStyle"], 2);
        assert_eq!(q["standardEpisodeFormat"], "{Series Title} x");
        assert_eq!(
            query_ids(Some("seriesIds=1,2&seriesIds=3"), "seriesIds"),
            [1, 2, 3]
        );
    }

    #[test]
    fn grouping_inputs_are_checked() {
        assert_eq!(
            metadata_source(&json!("aniList")),
            Some(MetadataProvider::AniList)
        );
        assert_eq!(
            metadata_source(&json!(3)),
            Some(MetadataProvider::MyAnimeList)
        );
        assert_eq!(
            metadata_source(&json!("5")),
            Some(MetadataProvider::TmdbMovie)
        );
        assert_eq!(metadata_source(&json!("x")), None);
        assert!(valid_group_id("anilist:21") && !valid_group_id(":x") && !valid_group_id("a b"));
        assert_eq!(
            group_source("mal:5"),
            Some((MetadataProvider::MyAnimeList, "5"))
        );
    }
}
