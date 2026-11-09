//! Settings endpoints: tags, root folders, qualities, custom formats, providers, naming,
//! media management, general settings and static catalogues.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_db::{quality, root_folders, settings as kv, tags};
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::settings::{
    CustomFormat, GeneralSettings, MediaManagementSettings, NamingSettings, ProviderItem,
    ProviderKind, QualityDefinition, QualityProfile, REDACTED, RootFolderCreate, RootFolderItem,
    TagCreate, TagItem,
};
use kitsunarr_domain::{Language, MediaKind, Quality};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::routes::system::disk_space;

fn settings_changed(state: &AppState, section: &str) {
    state.events.publish(ServerEvent::SettingsChanged {
        section: section.into(),
    });
}

// ---------------------------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------------------------

#[utoipa::path(get, path = "/tags", tag = "settings", responses((status = 200, body = Vec<TagItem>)))]
pub async fn list_tags(State(state): State<AppState>) -> ApiResult<Json<Vec<TagItem>>> {
    Ok(Json(tags::list(state.db.pool()).await?))
}

#[utoipa::path(post, path = "/tags", tag = "settings", request_body = TagCreate, responses((status = 201, body = TagItem)))]
pub async fn create_tag(
    State(state): State<AppState>,
    Json(body): Json<TagCreate>,
) -> ApiResult<(StatusCode, Json<TagItem>)> {
    let id = tags::create(state.db.pool(), &body.label).await?;
    settings_changed(&state, "tags");
    let item = tags::list(state.db.pool())
        .await?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| ApiError::internal("tag vanished"))?;
    Ok((StatusCode::CREATED, Json(item)))
}

#[utoipa::path(delete, path = "/tags/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 204)))]
pub async fn delete_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    tags::delete(&state.db, id).await?;
    settings_changed(&state, "tags");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------------
// Root folders
// ---------------------------------------------------------------------------------------------

fn root_item(r: root_folders::RootFolder) -> RootFolderItem {
    let space = disk_space(&r.path);
    RootFolderItem {
        id: r.id,
        media_kind: r.kind(),
        accessible: space.is_some(),
        free_space: space.map(|s| s.0),
        total_space: space.map(|s| s.1),
        path: r.path,
    }
}

#[utoipa::path(get, path = "/rootfolders", tag = "settings", responses((status = 200, body = Vec<RootFolderItem>)))]
pub async fn list_root_folders(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<RootFolderItem>>> {
    let list = root_folders::list(state.db.pool()).await?;
    let items =
        tokio::task::spawn_blocking(move || list.into_iter().map(root_item).collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    Ok(Json(items))
}

/// Root folders must be absolute paths to existing directories; nothing is created on disk.
#[utoipa::path(post, path = "/rootfolders", tag = "settings", request_body = RootFolderCreate,
    responses((status = 201, body = RootFolderItem), (status = 422)))]
pub async fn create_root_folder(
    State(state): State<AppState>,
    Json(body): Json<RootFolderCreate>,
) -> ApiResult<(StatusCode, Json<RootFolderItem>)> {
    let path = root_folders::normalize_path(&body.path);
    if !std::path::Path::new(&path).is_absolute() {
        return Err(ApiError::field(
            "path",
            "Root folders must be absolute paths.",
        ));
    }
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(ApiError::field(
            "path",
            "Root folders must not contain `..` segments.",
        ));
    }
    let check = path.clone();
    let is_dir = tokio::task::spawn_blocking(move || std::path::Path::new(&check).is_dir())
        .await
        .map_err(ApiError::internal)?;
    if !is_dir {
        return Err(ApiError::field(
            "path",
            "The folder does not exist or is not accessible to Kitsunarr.",
        ));
    }
    let id = root_folders::create(state.db.pool(), &path, body.media_kind).await?;
    settings_changed(&state, "rootFolders");
    let row = root_folders::list(state.db.pool())
        .await?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::internal("missing"))?;
    Ok((StatusCode::CREATED, Json(root_item(row))))
}

#[utoipa::path(delete, path = "/rootfolders/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 204)))]
pub async fn delete_root_folder(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    root_folders::delete(state.db.pool(), id).await?;
    settings_changed(&state, "rootFolders");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------------
// Qualities and custom formats
// ---------------------------------------------------------------------------------------------

#[utoipa::path(get, path = "/qualitydefinitions", tag = "settings", responses((status = 200, body = Vec<QualityDefinition>)))]
pub async fn list_quality_definitions(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<QualityDefinition>>> {
    Ok(Json(quality::list_definitions(state.db.pool()).await?))
}

#[utoipa::path(put, path = "/qualitydefinitions", tag = "settings", request_body = Vec<QualityDefinition>,
    responses((status = 200, body = Vec<QualityDefinition>)))]
pub async fn update_quality_definitions(
    State(state): State<AppState>,
    Json(defs): Json<Vec<QualityDefinition>>,
) -> ApiResult<Json<Vec<QualityDefinition>>> {
    let mut tx = state
        .db
        .pool()
        .begin()
        .await
        .map_err(kitsunarr_db::DbError::from)?;
    for d in &defs {
        quality::upsert_definition(&mut *tx, d).await?;
    }
    tx.commit().await.map_err(kitsunarr_db::DbError::from)?;
    settings_changed(&state, "qualityDefinitions");
    Ok(Json(quality::list_definitions(state.db.pool()).await?))
}

#[utoipa::path(get, path = "/qualityprofiles", tag = "settings", responses((status = 200, body = Vec<QualityProfile>)))]
pub async fn list_quality_profiles(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<QualityProfile>>> {
    Ok(Json(quality::list_profiles(state.db.pool()).await?))
}

#[utoipa::path(get, path = "/qualityprofiles/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 200, body = QualityProfile)))]
pub async fn get_quality_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<QualityProfile>> {
    Ok(Json(quality::get_profile(state.db.pool(), id).await?))
}

#[utoipa::path(post, path = "/qualityprofiles", tag = "settings", request_body = QualityProfile, responses((status = 201, body = QualityProfile)))]
pub async fn create_quality_profile(
    State(state): State<AppState>,
    Json(p): Json<QualityProfile>,
) -> ApiResult<(StatusCode, Json<QualityProfile>)> {
    let id = quality::create_profile(state.db.pool(), &p).await?;
    settings_changed(&state, "qualityProfiles");
    Ok((
        StatusCode::CREATED,
        Json(quality::get_profile(state.db.pool(), id).await?),
    ))
}

#[utoipa::path(put, path = "/qualityprofiles/{id}", tag = "settings", params(("id" = i64, Path)), request_body = QualityProfile,
    responses((status = 200, body = QualityProfile)))]
pub async fn update_quality_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut p): Json<QualityProfile>,
) -> ApiResult<Json<QualityProfile>> {
    p.id = id;
    quality::update_profile(state.db.pool(), &p).await?;
    settings_changed(&state, "qualityProfiles");
    Ok(Json(quality::get_profile(state.db.pool(), id).await?))
}

#[utoipa::path(delete, path = "/qualityprofiles/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 204), (status = 409)))]
pub async fn delete_quality_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    quality::delete_profile(state.db.pool(), id).await?;
    settings_changed(&state, "qualityProfiles");
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/customformats", tag = "settings", responses((status = 200, body = Vec<CustomFormat>)))]
pub async fn list_custom_formats(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<CustomFormat>>> {
    Ok(Json(quality::list_custom_formats(state.db.pool()).await?))
}

#[utoipa::path(post, path = "/customformats", tag = "settings", request_body = CustomFormat, responses((status = 201, body = CustomFormat)))]
pub async fn create_custom_format(
    State(state): State<AppState>,
    Json(cf): Json<CustomFormat>,
) -> ApiResult<(StatusCode, Json<CustomFormat>)> {
    let id = quality::create_custom_format(state.db.pool(), &cf).await?;
    settings_changed(&state, "customFormats");
    let created = quality::list_custom_formats(state.db.pool())
        .await?
        .into_iter()
        .find(|c| c.id == id);
    Ok((
        StatusCode::CREATED,
        Json(created.ok_or_else(|| ApiError::internal("missing"))?),
    ))
}

#[utoipa::path(put, path = "/customformats/{id}", tag = "settings", params(("id" = i64, Path)), request_body = CustomFormat,
    responses((status = 200, body = CustomFormat)))]
pub async fn update_custom_format(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut cf): Json<CustomFormat>,
) -> ApiResult<Json<CustomFormat>> {
    cf.id = id;
    quality::update_custom_format(state.db.pool(), &cf).await?;
    settings_changed(&state, "customFormats");
    Ok(Json(cf))
}

#[utoipa::path(delete, path = "/customformats/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 204)))]
pub async fn delete_custom_format(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    quality::delete_custom_format(&state.db, id).await?;
    settings_changed(&state, "customFormats");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------------
// Providers (indexers, download clients, notifications, import lists)
// ---------------------------------------------------------------------------------------------

/// Field names whose values are secrets and must never be returned to clients.
pub fn is_secret_field(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "password",
        "apikey",
        "api_key",
        "token",
        "secret",
        "passkey",
        "cookie",
        "webhookurl",
        "accesskey",
        "privatekey",
    ]
    .iter()
    .any(|s| n.contains(s))
}

pub fn redact(settings: &Map<String, Value>) -> Map<String, Value> {
    settings
        .iter()
        .map(|(k, v)| {
            let redacted = is_secret_field(k)
                && !matches!(v, Value::Null)
                && v.as_str().is_none_or(|s| !s.is_empty());
            (
                k.clone(),
                if redacted {
                    Value::String(REDACTED.into())
                } else {
                    v.clone()
                },
            )
        })
        .collect()
}

/// Keeps stored secrets when the client sends back the redaction placeholder.
pub(crate) fn merge_secrets(incoming: &mut Map<String, Value>, stored: &Map<String, Value>) {
    for (k, v) in incoming.iter_mut() {
        if v.as_str() == Some(REDACTED)
            && let Some(old) = stored.get(k)
        {
            *v = old.clone();
        }
    }
}

fn parse_kind(raw: &str) -> ApiResult<ProviderKind> {
    match raw {
        "indexers" | "indexer" => Ok(ProviderKind::Indexer),
        "downloadclients" | "downloadclient" => Ok(ProviderKind::DownloadClient),
        "notifications" | "notification" => Ok(ProviderKind::Notification),
        "importlists" | "importlist" => Ok(ProviderKind::ImportList),
        other => Err(ApiError::NotFound(format!(
            "Unknown provider kind `{other}`."
        ))),
    }
}

fn to_api(p: &ProviderRecord) -> ProviderItem {
    let mut item = p.to_item();
    item.settings = redact(&item.settings);
    item
}

#[utoipa::path(get, path = "/providers/{kind}", tag = "settings",
    params(("kind" = String, Path, description = "indexers | downloadclients | notifications | importlists")),
    responses((status = 200, body = Vec<ProviderItem>)))]
pub async fn list_providers(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> ApiResult<Json<Vec<ProviderItem>>> {
    let kind = parse_kind(&kind)?;
    Ok(Json(
        providers::list(state.db.pool(), kind)
            .await?
            .iter()
            .map(to_api)
            .collect(),
    ))
}

#[utoipa::path(get, path = "/providers/{kind}/{id}", tag = "settings", params(("kind" = String, Path), ("id" = i64, Path)),
    responses((status = 200, body = ProviderItem)))]
pub async fn get_provider(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
) -> ApiResult<Json<ProviderItem>> {
    let kind = parse_kind(&kind)?;
    Ok(Json(to_api(
        &providers::get(state.db.pool(), kind, id).await?,
    )))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    pub name: String,
    pub implementation: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub tags: Vec<i64>,
    #[serde(default)]
    pub options: Map<String, Value>,
    #[serde(default)]
    pub settings: Map<String, Value>,
}

fn yes() -> bool {
    true
}

fn record_from(kind: ProviderKind, id: i64, input: ProviderInput) -> ProviderRecord {
    ProviderRecord {
        id,
        kind,
        protocol: providers::protocol_for(&input.implementation),
        priority: input.priority.unwrap_or(match kind {
            ProviderKind::Indexer => 25,
            _ => 1,
        }),
        name: input.name,
        implementation: input.implementation,
        enabled: input.enabled,
        tags: input.tags,
        options: input.options,
        settings: input.settings,
    }
}

#[utoipa::path(post, path = "/providers/{kind}", tag = "settings", params(("kind" = String, Path)), request_body = ProviderInput,
    responses((status = 201, body = ProviderItem)))]
pub async fn create_provider(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Json(input): Json<ProviderInput>,
) -> ApiResult<(StatusCode, Json<ProviderItem>)> {
    let kind = parse_kind(&kind)?;
    if input
        .settings
        .values()
        .any(|v| v.as_str() == Some(REDACTED))
    {
        return Err(ApiError::BadRequest(
            "Enter the secret values; the redaction placeholder is only valid on updates.".into(),
        ));
    }
    let id = providers::create(state.db.pool(), &record_from(kind, 0, input)).await?;
    settings_changed(&state, "providers");
    Ok((
        StatusCode::CREATED,
        Json(to_api(&providers::get(state.db.pool(), kind, id).await?)),
    ))
}

#[utoipa::path(put, path = "/providers/{kind}/{id}", tag = "settings", params(("kind" = String, Path), ("id" = i64, Path)),
    request_body = ProviderInput, responses((status = 200, body = ProviderItem)))]
pub async fn update_provider(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
    Json(input): Json<ProviderInput>,
) -> ApiResult<Json<ProviderItem>> {
    let kind = parse_kind(&kind)?;
    let stored = providers::get(state.db.pool(), kind, id).await?;
    let mut record = record_from(kind, id, input);
    merge_secrets(&mut record.settings, &stored.settings);
    providers::update(state.db.pool(), &record).await?;
    settings_changed(&state, "providers");
    Ok(Json(to_api(
        &providers::get(state.db.pool(), kind, id).await?,
    )))
}

#[utoipa::path(delete, path = "/providers/{kind}/{id}", tag = "settings", params(("kind" = String, Path), ("id" = i64, Path)),
    responses((status = 204)))]
pub async fn delete_provider(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
) -> ApiResult<StatusCode> {
    let kind = parse_kind(&kind)?;
    providers::delete(state.db.pool(), kind, id).await?;
    settings_changed(&state, "providers");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------------
// Key/value settings
// ---------------------------------------------------------------------------------------------

#[utoipa::path(get, path = "/settings/naming", tag = "settings", responses((status = 200, body = NamingSettings)))]
pub async fn get_naming(State(state): State<AppState>) -> ApiResult<Json<NamingSettings>> {
    Ok(Json(
        kv::get_or_default(state.db.pool(), kv::KEY_NAMING).await?,
    ))
}

#[utoipa::path(put, path = "/settings/naming", tag = "settings", request_body = NamingSettings, responses((status = 200, body = NamingSettings)))]
pub async fn put_naming(
    State(state): State<AppState>,
    Json(body): Json<NamingSettings>,
) -> ApiResult<Json<NamingSettings>> {
    for (field, value) in [
        ("standardEpisodeFormat", &body.standard_episode_format),
        ("dailyEpisodeFormat", &body.daily_episode_format),
        ("animeEpisodeFormat", &body.anime_episode_format),
        ("seriesFolderFormat", &body.series_folder_format),
        ("movieFileFormat", &body.movie_file_format),
        ("movieFolderFormat", &body.movie_folder_format),
    ] {
        if value.trim().is_empty() {
            return Err(ApiError::field(field, "Format must not be empty."));
        }
        if value.contains("..") || value.starts_with('/') || value.starts_with('\\') {
            return Err(ApiError::field(
                field,
                "Formats must not contain `..` or start with a path separator.",
            ));
        }
    }
    kv::set(state.db.pool(), kv::KEY_NAMING, &body).await?;
    settings_changed(&state, "naming");
    Ok(Json(body))
}

#[utoipa::path(get, path = "/settings/mediamanagement", tag = "settings", responses((status = 200, body = MediaManagementSettings)))]
pub async fn get_media_management(
    State(state): State<AppState>,
) -> ApiResult<Json<MediaManagementSettings>> {
    Ok(Json(
        kv::get_or_default(state.db.pool(), kv::KEY_MEDIA_MANAGEMENT).await?,
    ))
}

#[utoipa::path(put, path = "/settings/mediamanagement", tag = "settings", request_body = MediaManagementSettings,
    responses((status = 200, body = MediaManagementSettings)))]
pub async fn put_media_management(
    State(state): State<AppState>,
    Json(body): Json<MediaManagementSettings>,
) -> ApiResult<Json<MediaManagementSettings>> {
    if body.chmod_folder.len() != 3 || !body.chmod_folder.chars().all(|c| ('0'..='7').contains(&c))
    {
        return Err(ApiError::field(
            "chmodFolder",
            "Use a three-digit octal mode such as 755.",
        ));
    }
    if let Some(bin) = body.recycle_bin.as_deref().filter(|b| !b.is_empty())
        && !std::path::Path::new(bin).is_absolute()
    {
        return Err(ApiError::field(
            "recycleBin",
            "The recycle bin must be an absolute path.",
        ));
    }
    kv::set(state.db.pool(), kv::KEY_MEDIA_MANAGEMENT, &body).await?;
    settings_changed(&state, "mediaManagement");
    Ok(Json(body))
}

#[utoipa::path(get, path = "/settings/general", tag = "settings", responses((status = 200, body = GeneralSettings)))]
pub async fn get_general(State(state): State<AppState>) -> ApiResult<Json<GeneralSettings>> {
    Ok(Json(
        kv::get_or_default(state.db.pool(), kv::KEY_GENERAL).await?,
    ))
}

#[utoipa::path(put, path = "/settings/general", tag = "settings", request_body = GeneralSettings, responses((status = 200, body = GeneralSettings)))]
pub async fn put_general(
    State(state): State<AppState>,
    Json(body): Json<GeneralSettings>,
) -> ApiResult<Json<GeneralSettings>> {
    if body.instance_name.trim().is_empty() || body.instance_name.chars().count() > 64 {
        return Err(ApiError::field(
            "instanceName",
            "Instance name must be 1-64 characters.",
        ));
    }
    if body.first_day_of_week > 6 {
        return Err(ApiError::field(
            "firstDayOfWeek",
            "Use 0 (Sunday) to 6 (Saturday).",
        ));
    }
    kv::set(state.db.pool(), kv::KEY_GENERAL, &body).await?;
    settings_changed(&state, "general");
    Ok(Json(body))
}

/// Static catalogues the UI needs (qualities, languages, media kinds).
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Catalogues {
    pub qualities: Vec<Quality>,
    pub languages: Vec<Language>,
    pub media_kinds: Vec<MediaKind>,
}

#[utoipa::path(get, path = "/catalogues", tag = "settings", responses((status = 200, body = Catalogues)))]
pub async fn catalogues() -> Json<Catalogues> {
    Json(Catalogues {
        qualities: Quality::all().to_vec(),
        languages: Language::all().to_vec(),
        media_kinds: MediaKind::ALL.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_and_restored() {
        let stored: Map<String, Value> =
            serde_json::from_value(serde_json::json!({"host": "qb", "password": "hunter2", "apiKey": "k", "webHookUrl": "https://discord/x", "username": ""}))
                .unwrap();
        let red = redact(&stored);
        assert_eq!(red["host"], "qb");
        assert_eq!(red["password"], REDACTED);
        assert_eq!(red["apiKey"], REDACTED);
        assert_eq!(red["webHookUrl"], REDACTED);
        assert_eq!(red["username"], "", "empty values stay visible");
        let mut incoming = red.clone();
        incoming.insert("host".into(), "qb2".into());
        merge_secrets(&mut incoming, &stored);
        assert_eq!(incoming["password"], "hunter2");
        assert_eq!(incoming["host"], "qb2");
    }
}
