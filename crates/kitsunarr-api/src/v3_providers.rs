//! Sonarr's `/api/v3` download clients, connections and import lists written by tools
//! (`downloadclient`, `notification`, `importlist`: create, update, delete, schema, test), in
//! Sonarr's resource shape (`fields` as `{name, value}`, the notification triggers and the list
//! options as top-level keys) and stored as Kitsunarr's providers through the native handlers
//! (validation, secret redaction).

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_domain::api::search::IndexerTestRequest;
use kitsunarr_domain::api::settings::ProviderKind;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::routes::settings::{self, ProviderInput};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/downloadclient", post(create_client))
        .route("/downloadclient/schema", get(client_schema))
        .route("/downloadclient/test", post(test_client))
        .route(
            "/downloadclient/{id}",
            put(update_client).delete(delete_client),
        )
        .route("/notification", post(create_notification))
        .route("/notification/schema", get(notification_schema))
        .route("/notification/test", post(test_notification))
        .route(
            "/notification/{id}",
            put(update_notification).delete(delete_notification),
        )
        .route("/importlist", get(list_import_lists).post(create_list))
        .route("/importlist/schema", get(list_schema))
        .route("/importlist/test", post(test_list))
        .route(
            "/importlist/{id}",
            get(get_import_list).put(update_list).delete(delete_list),
        )
}

/// Sonarr's `ImportListResource` keys Kitsunarr keeps as the list's options.
pub(crate) const LIST_OPTIONS: &[&str] = &[
    "enableAutomaticAdd",
    "searchForMissingEpisodes",
    "shouldMonitor",
    "monitorNewItems",
    "rootFolderPath",
    "qualityProfileId",
    "seriesType",
    "seasonFolder",
    "tagExisting",
    "preferredMetadataSource",
];

/// The resource's top-level keys that are options of the provider.
pub(crate) fn is_option(kind: ProviderKind, key: &str) -> bool {
    match kind {
        ProviderKind::DownloadClient => {
            matches!(key, "removeCompletedDownloads" | "removeFailedDownloads")
        }
        ProviderKind::Notification => {
            (key.starts_with("on") && key.chars().nth(2).is_some_and(char::is_uppercase))
                || key.starts_with("include")
        }
        ProviderKind::ImportList => LIST_OPTIONS.contains(&key),
        _ => false,
    }
}

/// A Sonarr provider resource as the native provider input.
pub(crate) fn input_from(kind: ProviderKind, v: &Value) -> ApiResult<ProviderInput> {
    let name = v["name"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| ApiError::field("name", "A name is required."))?;
    let implementation = v["implementation"]
        .as_str()
        .filter(|i| !i.trim().is_empty())
        .ok_or_else(|| ApiError::field("implementation", "An implementation is required."))?;
    let mut fields = Map::new();
    for f in v["fields"].as_array().into_iter().flatten() {
        if let Some(n) = f["name"].as_str().filter(|n| !n.is_empty())
            && !f["value"].is_null()
        {
            fields.insert(n.to_string(), f["value"].clone());
        }
    }
    let mut options = Map::new();
    if let Some(o) = v.as_object() {
        for (k, val) in o {
            let usable = if kind == ProviderKind::ImportList {
                !val.is_null()
            } else {
                val.is_boolean()
            };
            if is_option(kind, k) && usable {
                options.insert(k.clone(), val.clone());
            }
        }
    }
    Ok(ProviderInput {
        name: name.trim().to_string(),
        implementation: implementation.to_string(),
        enabled: v["enable"].as_bool().unwrap_or(true),
        priority: v["priority"].as_i64().map(|p| p as i32),
        tags: v["tags"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default(),
        options,
        settings: fields,
    })
}

pub(crate) fn path_of(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::DownloadClient => "downloadclients",
        ProviderKind::ImportList => "importlists",
        _ => "notifications",
    }
}

pub(crate) fn supported(kind: ProviderKind, implementation: &str) -> bool {
    match kind {
        ProviderKind::DownloadClient => kitsunarr_downloaders::supported(implementation),
        ProviderKind::ImportList => kitsunarr_importlists::is_supported(implementation),
        _ => kitsunarr_notifications::supported(implementation),
    }
}

pub(crate) async fn resource(state: &AppState, kind: ProviderKind, id: i64) -> ApiResult<Value> {
    let p = providers::get(state.db.pool(), kind, id).await?;
    Ok(crate::v3::provider_json(&p))
}

pub(crate) async fn create(
    state: AppState,
    kind: ProviderKind,
    v: Value,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let input = input_from(kind, &v)?;
    if !supported(kind, &input.implementation) {
        return Err(ApiError::field(
            "implementation",
            format!("{} is not supported.", input.implementation),
        ));
    }
    let (_, Json(item)) = settings::create_provider(
        State(state.clone()),
        Path(path_of(kind).into()),
        Json(input),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(resource(&state, kind, item.id).await?),
    ))
}

pub(crate) async fn update(
    state: AppState,
    kind: ProviderKind,
    id: i64,
    v: Value,
) -> ApiResult<(StatusCode, Json<Value>)> {
    providers::get(state.db.pool(), kind, id).await?;
    let input = input_from(kind, &v)?;
    let _ = settings::update_provider(
        State(state.clone()),
        Path((path_of(kind).into(), id)),
        Json(input),
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(resource(&state, kind, id).await?),
    ))
}

pub(crate) async fn delete(state: AppState, kind: ProviderKind, id: i64) -> ApiResult<StatusCode> {
    providers::get(state.db.pool(), kind, id).await?;
    settings::delete_provider(State(state), Path((path_of(kind).into(), id))).await?;
    Ok(StatusCode::OK)
}

/// `…/schema`: Kitsunarr's templates for every implementation it supports.
pub(crate) fn schema(kind: ProviderKind) -> Vec<Value> {
    crate::routes::schema::templates(kind)
        .iter()
        .filter(|t| supported(kind, &t.implementation))
        .map(|t| {
            crate::v3::provider_json(&ProviderRecord {
                id: 0,
                kind,
                name: t.name.clone(),
                implementation: t.implementation.clone(),
                protocol: providers::protocol_for(&t.implementation),
                enabled: true,
                priority: t.priority,
                tags: Vec::new(),
                options: t.options.clone(),
                settings: t.settings.clone(),
            })
        })
        .collect()
}

/// `…/test`: 200 when the provider answers, else 400 with Sonarr's validation list.
pub(crate) async fn test(
    state: AppState,
    kind: ProviderKind,
    v: Value,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let input = input_from(kind, &v)?;
    let req = IndexerTestRequest {
        id: v["id"].as_i64().filter(|i| *i > 0),
        implementation: input.implementation,
        settings: input.settings,
    };
    let result = match kind {
        ProviderKind::DownloadClient => {
            crate::routes::grab::test_download_client(State(state), Json(req)).await?
        }
        ProviderKind::ImportList => {
            crate::routes::import_lists::test_import_list(State(state), Json(req)).await?
        }
        _ => crate::routes::grab::test_notification(State(state), Json(req)).await?,
    }
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

pub(crate) const DC: ProviderKind = ProviderKind::DownloadClient;
pub(crate) const NO: ProviderKind = ProviderKind::Notification;
pub(crate) const IL: ProviderKind = ProviderKind::ImportList;

pub(crate) async fn list_import_lists(State(s): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(
        providers::list(s.db.pool(), IL)
            .await?
            .iter()
            .map(crate::v3::provider_json)
            .collect(),
    ))
}
pub(crate) async fn get_import_list(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(resource(&s, IL, id).await?))
}
pub(crate) async fn create_list(
    State(s): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    create(s, IL, v).await
}
pub(crate) async fn update_list(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    update(s, IL, id, v).await
}
pub(crate) async fn delete_list(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    delete(s, IL, id).await
}
pub(crate) async fn list_schema() -> Json<Vec<Value>> {
    Json(schema(IL))
}
pub(crate) async fn test_list(
    State(s): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    test(s, IL, v).await
}

pub(crate) async fn create_client(
    State(s): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    create(s, DC, v).await
}
pub(crate) async fn update_client(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    update(s, DC, id, v).await
}
pub(crate) async fn delete_client(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    delete(s, DC, id).await
}
pub(crate) async fn client_schema() -> Json<Vec<Value>> {
    Json(schema(DC))
}
pub(crate) async fn test_client(
    State(s): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    test(s, DC, v).await
}
pub(crate) async fn create_notification(
    State(s): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    create(s, NO, v).await
}
pub(crate) async fn update_notification(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    update(s, NO, id, v).await
}
pub(crate) async fn delete_notification(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    delete(s, NO, id).await
}
pub(crate) async fn notification_schema() -> Json<Vec<Value>> {
    Json(schema(NO))
}
pub(crate) async fn test_notification(
    State(s): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    test(s, NO, v).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonarr_resources_become_provider_input() {
        let v = json!({
            "name": "Hook", "implementation": "Webhook", "enable": true, "tags": [2],
            "onGrab": true, "onDownload": false, "includeHealthWarnings": true, "id": 0,
            "fields": [{"name": "url", "value": "http://x"}, {"name": "method", "value": 1}, {"name": "username", "value": null}]
        });
        let i = input_from(NO, &v).unwrap();
        assert_eq!(i.settings["url"], "http://x");
        assert!(!i.settings.contains_key("username"));
        assert_eq!(i.options["onGrab"], true);
        assert_eq!(i.options["includeHealthWarnings"], true);
        assert!(!i.options.contains_key("id"));
        assert_eq!(i.tags, [2]);
        let dc = input_from(DC, &json!({"name": "q", "implementation": "QBittorrent", "removeCompletedDownloads": false, "onGrab": true})).unwrap();
        assert_eq!(dc.options.len(), 1);
        assert!(input_from(DC, &json!({"implementation": "QBittorrent"})).is_err());
    }
}
