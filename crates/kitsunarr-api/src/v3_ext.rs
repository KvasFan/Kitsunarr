//! The rest of Sonarr's provider endpoints (`/api/v3`, also served under `/api/v5`):
//! `…/bulk` (edit or delete several), `…/testall`, `…/action/{name}` (the editor's helpers:
//! account lists, Jellyfin choices, Newznab categories and sign-in) for indexers, download
//! clients, import lists and connections, and Sonarr's metadata consumers (`metadata`), which
//! Kitsunarr keeps as four fixed consumers.

use std::collections::HashMap;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_domain::api::search::IndexerTestRequest;
use kitsunarr_domain::api::settings::{MetadataConsumersSettings, ProviderKind, SignInRequest};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

const IX: ProviderKind = ProviderKind::Indexer;
const DC: ProviderKind = ProviderKind::DownloadClient;
const IL: ProviderKind = ProviderKind::ImportList;
const NO: ProviderKind = ProviderKind::Notification;

/// The routes, for one resource name per provider kind (`notification` in v3, `connection`
/// in v5).
pub fn router(notification: &'static str) -> Router<AppState> {
    let mut r = Router::new();
    for (name, kind) in [
        ("indexer", IX),
        ("downloadclient", DC),
        ("importlist", IL),
        (notification, NO),
    ] {
        if kind != NO {
            r = r.route(
                &format!("/{name}/bulk"),
                put(move |s, b| bulk_update(s, kind, b))
                    .delete(move |s, b| bulk_delete(s, kind, b)),
            );
        }
        r = r
            .route(
                &format!("/{name}/testall"),
                post(move |s| test_all(s, kind)),
            )
            .route(
                &format!("/{name}/action/{{name}}"),
                post(move |s, p, q, b| action(s, kind, p, q, b)),
            );
    }
    r.route("/metadata", get(list_metadata).post(create_metadata))
        .route("/metadata/schema", get(metadata_schema))
        .route("/metadata/test", post(test_metadata))
        .route("/metadata/testall", post(test_all_metadata))
        .route("/metadata/action/{name}", post(metadata_action))
        .route(
            "/metadata/{id}",
            get(get_metadata)
                .put(update_metadata)
                .delete(delete_metadata),
        )
}

/// A provider kind's resource, as the kind's own v3 handlers write it.
pub(crate) fn resource(p: &ProviderRecord) -> Value {
    if p.kind == IX {
        crate::v3::indexer_json(p)
    } else {
        crate::v3::provider_json(p)
    }
}

fn section(kind: ProviderKind) -> &'static str {
    if kind == IX { "indexers" } else { "providers" }
}

// ---- bulk -----------------------------------------------------------------------------------

/// Sonarr's `ApplyTags`: how a bulk edit's tags combine with each provider's.
#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ApplyTags {
    #[default]
    Add,
    Remove,
    Replace,
}

impl ApplyTags {
    /// Sonarr's `ApplyTags` arrives as a name or as the enum's number.
    pub(crate) fn of(v: &Value) -> Self {
        match v {
            Value::Number(n) => match n.as_i64() {
                Some(1) => ApplyTags::Remove,
                Some(2) => ApplyTags::Replace,
                _ => ApplyTags::Add,
            },
            Value::String(s) => match s.to_ascii_lowercase().as_str() {
                "remove" => ApplyTags::Remove,
                "replace" => ApplyTags::Replace,
                _ => ApplyTags::Add,
            },
            _ => ApplyTags::Add,
        }
    }

    pub(crate) fn apply(self, current: &[i64], given: &[i64]) -> Vec<i64> {
        let mut out: Vec<i64> = match self {
            ApplyTags::Add => current.iter().chain(given).copied().collect(),
            ApplyTags::Remove => current
                .iter()
                .filter(|t| !given.contains(t))
                .copied()
                .collect(),
            ApplyTags::Replace => given.to_vec(),
        };
        out.sort_unstable();
        out.dedup();
        out
    }
}

pub(crate) fn ids_of(v: &Value, key: &str) -> Vec<i64> {
    v[key]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// The resource keys a bulk edit may set for each kind, stored as provider options.
fn bulk_options(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        IX => &[
            "enableRss",
            "enableAutomaticSearch",
            "enableInteractiveSearch",
            "seasonSearchMaximumSingleEpisodeAge",
        ],
        DC => &["removeCompletedDownloads", "removeFailedDownloads"],
        IL => &["enableAutomaticAdd", "rootFolderPath", "qualityProfileId"],
        _ => &[],
    }
}

/// Applies a bulk resource to one provider.
fn apply_bulk(kind: ProviderKind, p: &mut ProviderRecord, body: &Value) {
    if let Some(tags) = body["tags"].as_array() {
        let given: Vec<i64> = tags.iter().filter_map(Value::as_i64).collect();
        p.tags = ApplyTags::of(&body["applyTags"]).apply(&p.tags, &given);
    }
    if let Some(e) = body["enable"].as_bool() {
        p.enabled = e;
    }
    if let Some(prio) = body["priority"].as_i64() {
        p.priority = prio.clamp(1, 50) as i32;
    }
    for key in bulk_options(kind) {
        let v = &body[*key];
        if !v.is_null() {
            p.options.insert((*key).to_string(), v.clone());
        }
    }
}

async fn bulk_update(
    State(state): State<AppState>,
    kind: ProviderKind,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Vec<Value>>)> {
    let ids = ids_of(&body, "ids");
    let mut records = Vec::new();
    for id in &ids {
        records.push(providers::get(state.db.pool(), kind, *id).await?);
    }
    let mut tx = state.db.pool().begin().await.map_err(ApiError::internal)?;
    for p in &mut records {
        apply_bulk(kind, p, &body);
        providers::update(&mut *tx, p).await?;
    }
    tx.commit().await.map_err(ApiError::internal)?;
    crate::routes::settings::settings_changed(&state, section(kind));
    let mut out = Vec::new();
    for id in ids {
        out.push(resource(&providers::get(state.db.pool(), kind, id).await?));
    }
    Ok((StatusCode::ACCEPTED, Json(out)))
}

async fn bulk_delete(
    State(state): State<AppState>,
    kind: ProviderKind,
    Json(body): Json<Value>,
) -> ApiResult<StatusCode> {
    let ids = ids_of(&body, "ids");
    for id in &ids {
        providers::get(state.db.pool(), kind, *id).await?;
    }
    let mut tx = state.db.pool().begin().await.map_err(ApiError::internal)?;
    for id in ids {
        providers::delete(&mut *tx, kind, id).await?;
    }
    tx.commit().await.map_err(ApiError::internal)?;
    crate::routes::settings::settings_changed(&state, section(kind));
    Ok(StatusCode::OK)
}

// ---- test all -------------------------------------------------------------------------------

/// Tests a stored provider with its stored settings: `None` when it passes, else the message.
pub(crate) async fn test_stored(state: &AppState, p: &ProviderRecord) -> ApiResult<Option<String>> {
    let req = IndexerTestRequest {
        id: Some(p.id),
        implementation: p.implementation.clone(),
        settings: p.settings.clone(),
    };
    let s = State(state.clone());
    let (ok, message) = match p.kind {
        IX => {
            let r = crate::routes::search::test_indexer(s, Json(req)).await?.0;
            (r.ok, r.message)
        }
        DC => {
            let r = crate::routes::grab::test_download_client(s, Json(req))
                .await?
                .0;
            (r.ok, r.message)
        }
        IL => {
            let r = crate::routes::import_lists::test_import_list(s, Json(req))
                .await?
                .0;
            (r.ok, r.message)
        }
        _ => {
            let r = crate::routes::grab::test_notification(s, Json(req))
                .await?
                .0;
            (r.ok, r.message)
        }
    };
    Ok((!ok).then_some(message))
}

/// `POST …/testall`: every enabled provider of the kind, as Sonarr's `ProviderTestAllResult`
/// list; 400 when one fails.
async fn test_all(
    State(state): State<AppState>,
    kind: ProviderKind,
) -> ApiResult<(StatusCode, Json<Vec<Value>>)> {
    let mut out = Vec::new();
    for p in providers::list(state.db.pool(), kind).await? {
        if !p.enabled {
            continue;
        }
        let failure = match test_stored(&state, &p).await {
            Ok(f) => f,
            Err(e) => Some(e.message()),
        };
        out.push(test_result(p.id, failure));
    }
    let status = if out.iter().any(|r| r["isValid"] == false) {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    Ok((status, Json(out)))
}

pub(crate) fn test_result(id: i64, failure: Option<String>) -> Value {
    json!({
        "id": id,
        "isValid": failure.is_none(),
        "validationFailures": failure.map_or_else(Vec::new, |m| vec![json!({
            "propertyName": "", "errorMessage": m, "severity": "error", "isWarning": false
        })]),
    })
}

// ---- actions --------------------------------------------------------------------------------

/// A provider resource's settings (its `fields`), with the stored secrets of a saved one.
async fn settings_of(
    state: &AppState,
    kind: ProviderKind,
    body: &Value,
) -> ApiResult<Map<String, Value>> {
    let mut settings = Map::new();
    for f in body["fields"].as_array().into_iter().flatten() {
        if let Some(n) = f["name"].as_str().filter(|n| !n.is_empty())
            && !f["value"].is_null()
        {
            settings.insert(n.to_string(), f["value"].clone());
        }
    }
    if let Some(id) = body["id"].as_i64().filter(|i| *i > 0) {
        let stored = providers::get(state.db.pool(), kind, id).await?;
        crate::routes::settings::merge_secrets(&mut settings, &stored.settings);
    }
    Ok(settings)
}

fn options(choices: Vec<kitsunarr_domain::api::settings::ProviderChoice>) -> Value {
    json!({"options": choices.into_iter().enumerate().map(|(i, c)| json!({
        "value": c.value, "name": c.name, "order": i, "hint": c.hint,
    })).collect::<Vec<_>>()})
}

/// `POST …/action/{name}`: the editor helpers of Sonarr's providers (`RequestAction`).
///
/// Sign-in (`startOAuth`, `pollOAuth`, `getOAuthToken`) uses Kitsunarr's own flows (Plex and
/// Simkl PINs, Trakt's device code, MyAnimeList and AniList with the user's application)
/// instead of the Servarr relay: `startOAuth` answers `oauthUrl` with the `code` to enter and
/// a `state`; `getOAuthToken` (or `pollOAuth`) with that `state` answers the settings to save
/// once the user has signed in, else `{"pending": true}`.
async fn action(
    State(state): State<AppState>,
    kind: ProviderKind,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let implementation = body["implementation"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let settings = settings_of(&state, kind, &body).await?;
    let id = body["id"].as_i64().filter(|i| *i > 0);
    match (kind, name.as_str()) {
        (IX, "newznabCategories") => {
            newznab_categories(&state, id, &implementation, settings).await
        }
        (IL, "getCustomLists" | "getLists" | "getAccountLists") => {
            let req = IndexerTestRequest {
                id,
                implementation,
                settings,
            };
            let choices =
                crate::routes::import_lists::import_list_choices(State(state), Json(req)).await?;
            Ok(Json(options(choices.0)))
        }
        (NO, "libraries" | "users") => {
            let req = IndexerTestRequest {
                id,
                implementation,
                settings,
            };
            let choices = crate::routes::grab::notification_choices(
                State(state),
                Path(name.clone()),
                Json(req),
            )
            .await?;
            Ok(Json(options(choices.0)))
        }
        (_, "startOAuth" | "continueOAuth") => {
            let callback = query
                .get("callbackUrl")
                .or_else(|| query.get("callback"))
                .cloned();
            let started = crate::routes::import_lists::sign_in_start(
                State(state),
                Json(SignInRequest {
                    id,
                    implementation,
                    settings,
                    callback,
                    state: None,
                }),
            )
            .await?
            .0;
            Ok(Json(json!({
                "oauthUrl": started.url,
                "code": started.code,
                "state": started.state,
                "interval": started.interval,
                "expiresIn": started.expires_in,
            })))
        }
        (_, "pollOAuth" | "getOAuthToken") => {
            let sign_in = query
                .get("state")
                .cloned()
                .or_else(|| body["state"].as_str().map(str::to_string))
                .ok_or_else(|| ApiError::field("state", "Start signing in first."))?;
            let polled = crate::routes::import_lists::sign_in_poll(
                State(state),
                Json(SignInRequest {
                    id,
                    implementation,
                    settings,
                    callback: None,
                    state: Some(sign_in),
                }),
            )
            .await?
            .0;
            if polled.done {
                Ok(Json(Value::Object(polled.settings)))
            } else {
                Ok(Json(json!({"pending": true})))
            }
        }
        _ => Err(ApiError::BadRequest(format!(
            "{implementation} has no action \"{name}\"."
        ))),
    }
}

/// `newznabCategories`: the indexer's categories from its capabilities, as Sonarr's
/// `NewznabCategoryFieldOptionsConverter` lists them (subcategories under their parent).
async fn newznab_categories(
    state: &AppState,
    id: Option<i64>,
    implementation: &str,
    settings: Map<String, Value>,
) -> ApiResult<Json<Value>> {
    let dialect = kitsunarr_indexers::Dialect::from_implementation(implementation)
        .ok_or_else(|| ApiError::field("implementation", "Not a Newznab or Torznab indexer."))?;
    let settings = kitsunarr_indexers::Settings::from_map(&settings)
        .map_err(|m| ApiError::field("baseUrl", m))?;
    let indexer = kitsunarr_indexers::Indexer {
        id: id.unwrap_or(0),
        name: "Indexer".into(),
        dialect,
        settings,
    };
    let caps = state
        .indexers
        .capabilities(&indexer)
        .await
        .map_err(|e| ApiError::BadGateway(e.to_string()))?;
    let mut out = Vec::new();
    for c in &caps.categories {
        out.push(json!({"value": c.id, "name": c.name, "hint": format!("({})", c.id)}));
        for s in &c.subcategories {
            out.push(json!({
                "value": s.id, "name": s.name, "hint": format!("({})", s.id), "parentValue": c.id,
            }));
        }
    }
    for (i, o) in out.iter_mut().enumerate() {
        o["order"] = json!(i);
    }
    Ok(Json(json!({"options": out})))
}

// ---- metadata consumers ---------------------------------------------------------------------

/// Sonarr's four metadata consumers: (id, implementation, name, settings key).
const CONSUMERS: [(i64, &str, &str, &str); 4] = [
    (1, "XbmcMetadata", "Kodi (XBMC) / Emby", "kodi"),
    (2, "RoksboxMetadata", "Roksbox", "roksbox"),
    (3, "WdtvMetadata", "WDTV", "wdtv"),
    (4, "PlexMetadata", "Plex", "plex"),
];

async fn consumers(state: &AppState) -> ApiResult<Map<String, Value>> {
    let s = crate::routes::settings::get_metadata_consumers(State(state.clone()))
        .await?
        .0;
    match serde_json::to_value(s).map_err(ApiError::internal)? {
        Value::Object(m) => Ok(m),
        _ => Ok(Map::new()),
    }
}

async fn save_consumers(state: &AppState, m: Map<String, Value>) -> ApiResult<()> {
    let s: MetadataConsumersSettings = serde_json::from_value(Value::Object(m))
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let _ = crate::routes::settings::put_metadata_consumers(State(state.clone()), Json(s)).await?;
    Ok(())
}

fn consumer_json(id: i64, settings: &Value) -> Value {
    let (_, implementation, name, _) = CONSUMERS[(id - 1) as usize];
    let fields: Vec<Value> = settings
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| k.as_str() != "enable")
        .enumerate()
        .map(|(i, (k, v))| {
            json!({"order": i, "name": k, "label": k, "value": v, "type": "checkbox", "advanced": false, "privacy": "normal", "isFloat": false})
        })
        .collect();
    json!({
        "id": id,
        "name": name,
        "implementation": implementation,
        "implementationName": name,
        "configContract": format!("{implementation}Settings"),
        "infoLink": format!("https://wiki.servarr.com/sonarr/supported#{}", implementation.to_lowercase()),
        "enable": settings["enable"].as_bool().unwrap_or(false),
        "tags": [],
        "fields": fields,
        "presets": [],
    })
}

fn consumer_id(v: &Value) -> Option<i64> {
    v["id"]
        .as_i64()
        .filter(|i| (1..=4).contains(i))
        .or_else(|| {
            let imp = v["implementation"].as_str()?;
            CONSUMERS
                .iter()
                .find(|c| c.1.eq_ignore_ascii_case(imp))
                .map(|c| c.0)
        })
}

async fn list_metadata(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let m = consumers(&state).await?;
    Ok(Json(
        CONSUMERS
            .iter()
            .map(|(id, _, _, key)| consumer_json(*id, &m[*key]))
            .collect(),
    ))
}

async fn get_metadata(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let c = CONSUMERS
        .iter()
        .find(|c| c.0 == id)
        .ok_or_else(|| ApiError::NotFound(format!("Metadata consumer {id} not found.")))?;
    let m = consumers(&state).await?;
    Ok(Json(consumer_json(id, &m[c.3])))
}

async fn metadata_schema() -> Json<Vec<Value>> {
    let d = serde_json::to_value(MetadataConsumersSettings::default()).unwrap_or_default();
    Json(
        CONSUMERS
            .iter()
            .map(|(id, _, _, key)| {
                let mut v = consumer_json(*id, &d[*key]);
                v["id"] = json!(0);
                v
            })
            .collect(),
    )
}

/// Writes a resource's `enable` and fields into its consumer.
async fn write_metadata(state: &AppState, id: i64, body: &Value) -> ApiResult<Value> {
    let key = CONSUMERS[(id - 1) as usize].3;
    let mut m = consumers(state).await?;
    let mut current = m.get(key).cloned().unwrap_or_else(|| json!({}));
    if let Some(e) = body["enable"].as_bool() {
        current["enable"] = json!(e);
    }
    for f in body["fields"].as_array().into_iter().flatten() {
        if let (Some(n), Some(b)) = (f["name"].as_str(), f["value"].as_bool())
            && current.get(n).is_some()
        {
            current[n] = json!(b);
        }
    }
    m.insert(key.to_string(), current.clone());
    save_consumers(state, m).await?;
    Ok(consumer_json(id, &current))
}

/// `POST /metadata`: Kitsunarr keeps one consumer per kind, so creating one sets that kind's.
async fn create_metadata(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let id = consumer_id(&json!({"implementation": body["implementation"]}))
        .ok_or_else(|| ApiError::field("implementation", "Unknown metadata consumer."))?;
    Ok((
        StatusCode::CREATED,
        Json(write_metadata(&state, id, &body).await?),
    ))
}

async fn update_metadata(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if !(1..=4).contains(&id) {
        return Err(ApiError::NotFound(format!(
            "Metadata consumer {id} not found."
        )));
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(write_metadata(&state, id, &body).await?),
    ))
}

/// `DELETE /metadata/{id}`: turns the consumer off (the four always exist).
async fn delete_metadata(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    if !(1..=4).contains(&id) {
        return Err(ApiError::NotFound(format!(
            "Metadata consumer {id} not found."
        )));
    }
    write_metadata(&state, id, &json!({"enable": false})).await?;
    Ok(StatusCode::OK)
}

/// Metadata consumers write files only, so there is nothing to reach.
async fn test_metadata(Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    consumer_id(&body)
        .ok_or_else(|| ApiError::field("implementation", "Unknown metadata consumer."))?;
    Ok(Json(json!({})))
}

async fn test_all_metadata(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let m = consumers(&state).await?;
    Ok(Json(
        CONSUMERS
            .iter()
            .filter(|c| m[c.3]["enable"].as_bool().unwrap_or(false))
            .map(|c| test_result(c.0, None))
            .collect(),
    ))
}

async fn metadata_action(Path(name): Path<String>) -> ApiResult<Json<Value>> {
    Err(ApiError::BadRequest(format!(
        "Metadata consumers have no action \"{name}\"."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_tags_combines_like_sonarr() {
        assert_eq!(
            ApplyTags::of(&json!("add")).apply(&[1, 2], &[3, 2]),
            [1, 2, 3]
        );
        assert_eq!(ApplyTags::of(&json!("remove")).apply(&[1, 2], &[2]), [1]);
        assert_eq!(ApplyTags::of(&json!(2)).apply(&[1, 2], &[5]), [5]);
        assert_eq!(ApplyTags::of(&Value::Null), ApplyTags::Add);
    }

    #[test]
    fn bulk_edits_set_only_the_kind_options() {
        let mut p = ProviderRecord {
            id: 1,
            kind: IX,
            name: "x".into(),
            implementation: "Newznab".into(),
            protocol: kitsunarr_domain::DownloadProtocol::Usenet,
            enabled: true,
            priority: 25,
            tags: vec![1],
            options: Map::new(),
            settings: Map::new(),
        };
        apply_bulk(
            IX,
            &mut p,
            &json!({"ids": [1], "enableRss": false, "priority": 99, "tags": [4], "applyTags": "replace", "removeCompletedDownloads": true}),
        );
        assert_eq!(p.options["enableRss"], false);
        assert!(!p.options.contains_key("removeCompletedDownloads"));
        assert_eq!(p.priority, 50);
        assert_eq!(p.tags, [4]);
    }

    #[test]
    fn metadata_consumers_have_sonarr_ids() {
        assert_eq!(
            consumer_id(&json!({"implementation": "PlexMetadata"})),
            Some(4)
        );
        assert_eq!(consumer_id(&json!({"id": 2})), Some(2));
        let d = serde_json::to_value(MetadataConsumersSettings::default()).unwrap();
        let k = consumer_json(1, &d["kodi"]);
        assert_eq!(k["configContract"], "XbmcMetadataSettings");
        assert!(
            k["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["name"] == "seriesMetadata")
        );
    }
}
