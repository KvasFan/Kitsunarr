//! Sonarr's `/api/v3/config/*` resources (naming, media management, indexers, download
//! clients, import lists, interface and host), which sync tools (Recyclarr's media naming and
//! "propers and repacks", Notifiarr, Profilarr) and dashboards read and write. Each resource is
//! Kitsunarr's own settings under Sonarr's names: a `PUT` changes only the keys it sends and
//! goes through the native handler's validation; Sonarr settings Kitsunarr does not have are
//! answered with Sonarr's defaults and ignored when written.

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::settings::{
    DownloadClientOptionsSettings, FileDate, GeneralSettings, ImportListOptionsSettings,
    IndexerOptionsSettings, MediaManagementSettings, NamingSettings, ProperDownloadType,
    ProxySettings, ProxyType,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::routes::{import_lists, settings};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/config/naming", get(get_naming).put(put_naming))
        .route("/config/naming/{id}", get(get_naming).put(put_naming))
        .route(
            "/config/mediamanagement",
            get(get_media_management).put(put_media_management),
        )
        .route(
            "/config/mediamanagement/{id}",
            get(get_media_management).put(put_media_management),
        )
        .route("/config/indexer", get(get_indexer).put(put_indexer))
        .route("/config/indexer/{id}", get(get_indexer).put(put_indexer))
        .route(
            "/config/downloadclient",
            get(get_download_client).put(put_download_client),
        )
        .route(
            "/config/downloadclient/{id}",
            get(get_download_client).put(put_download_client),
        )
        .route(
            "/config/importlist",
            get(get_import_list).put(put_import_list),
        )
        .route(
            "/config/importlist/{id}",
            get(get_import_list).put(put_import_list),
        )
        .route("/config/ui", get(get_ui).put(put_ui))
        .route("/config/ui/{id}", get(get_ui).put(put_ui))
        .route("/config/host", get(get_host).put(put_host))
        .route("/config/host/{id}", get(get_host).put(put_host))
}

/// (Sonarr's key, Kitsunarr's key) of a setting both have.
type Keys = &'static [(&'static str, &'static str)];

/// Kitsunarr's settings as a Sonarr resource (`id` 1, as Sonarr's config resources).
fn resource<T: Serialize>(native: &T, keys: Keys, fixed: Value) -> Value {
    let native = serde_json::to_value(native).unwrap_or_default();
    let mut out = Map::new();
    out.insert("id".into(), json!(1));
    for (sonarr, ours) in keys {
        out.insert((*sonarr).into(), native[*ours].clone());
    }
    if let Value::Object(fixed) = fixed {
        for (k, v) in fixed {
            out.entry(k).or_insert(v);
        }
    }
    Value::Object(out)
}

/// The stored settings with the keys `body` sends (and that both have) replaced.
fn merged<T: Serialize + DeserializeOwned>(stored: &T, body: &Value, keys: Keys) -> ApiResult<T> {
    let mut native = serde_json::to_value(stored).map_err(ApiError::internal)?;
    for (sonarr, ours) in keys {
        if let Some(v) = body.get(*sonarr).filter(|v| !v.is_null()) {
            native[*ours] = v.clone();
        }
    }
    serde_json::from_value(native)
        .map_err(|e| ApiError::BadRequest(format!("Invalid settings: {e}")))
}

fn accepted(v: Value) -> (StatusCode, Json<Value>) {
    (StatusCode::ACCEPTED, Json(v))
}

/// An integer field of a resource, checked against Sonarr's range.
fn int_in(body: &Value, key: &str, range: std::ops::RangeInclusive<i64>) -> ApiResult<()> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(v) if v.as_i64().is_some_and(|n| range.contains(&n)) => Ok(()),
        Some(_) => Err(ApiError::field(
            key,
            format!(
                "Use a whole number from {} to {}.",
                range.start(),
                range.end()
            ),
        )),
    }
}

// ---- naming ---------------------------------------------------------------------------------

const NAMING: Keys = &[
    ("renameEpisodes", "renameEpisodes"),
    ("replaceIllegalCharacters", "replaceIllegalCharacters"),
    ("colonReplacementFormat", "colonReplacementFormat"),
    (
        "customColonReplacementFormat",
        "customColonReplacementFormat",
    ),
    ("multiEpisodeStyle", "multiEpisodeStyle"),
    ("standardEpisodeFormat", "standardEpisodeFormat"),
    ("dailyEpisodeFormat", "dailyEpisodeFormat"),
    ("animeEpisodeFormat", "animeEpisodeFormat"),
    ("seriesFolderFormat", "seriesFolderFormat"),
    ("seasonFolderFormat", "seasonFolderFormat"),
    ("specialsFolderFormat", "specialsFolderFormat"),
    // Tanukarr's film file format; the folder format is Kitsunarr's.
    ("movieFormat", "movieFileFormat"),
    ("movieFolderFormat", "movieFolderFormat"),
    ("animeGroupFolderFormat", "animeGroupFolderFormat"),
    (
        "animeGroupSeasonFolderFormat",
        "animeGroupSeasonFolderFormat",
    ),
    ("animeGroupArcFolderFormat", "animeGroupArcFolderFormat"),
    ("animeGroupMovieFolderFormat", "animeGroupMovieFolderFormat"),
    (
        "animeGroupMovieSagaFolderFormat",
        "animeGroupMovieSagaFolderFormat",
    ),
];

async fn naming(state: &AppState) -> ApiResult<NamingSettings> {
    Ok(kv::get_or_default(state.db.pool(), kv::KEY_NAMING).await?)
}

async fn get_naming(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(resource(&naming(&state).await?, NAMING, json!({}))))
}

async fn put_naming(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    int_in(&body, "multiEpisodeStyle", 0..=5)?;
    int_in(&body, "colonReplacementFormat", 0..=5)?;
    let next = merged(&naming(&state).await?, &body, NAMING)?;
    let saved = settings::put_naming(State(state), Json(next)).await?.0;
    Ok(accepted(resource(&saved, NAMING, json!({}))))
}

// ---- media management -----------------------------------------------------------------------

const MEDIA: Keys = &[
    (
        "autoUnmonitorPreviouslyDownloadedEpisodes",
        "autoUnmonitorPreviouslyDownloadedEpisodes",
    ),
    ("recycleBinCleanupDays", "recycleBinCleanupDays"),
    ("createEmptySeriesFolders", "createEmptySeriesFolders"),
    ("deleteEmptyFolders", "deleteEmptyFolders"),
    ("setPermissionsLinux", "setPermissions"),
    ("chmodFolder", "chmodFolder"),
    ("skipFreeSpaceCheckWhenImporting", "skipFreeSpaceCheck"),
    (
        "skipUnmonitoredEpisodesFromPacks",
        "skipUnmonitoredEpisodesFromPacks",
    ),
    ("minimumFreeSpaceWhenImporting", "minimumFreeSpaceMb"),
    ("copyUsingHardlinks", "useHardlinks"),
    ("importExtraFiles", "importExtraFiles"),
    ("extraFileExtensions", "extraFileExtensions"),
    ("enableMediaInfo", "enableMediaInfo"),
];

/// Sonarr's `ProperDownloadTypes` by name or number.
fn propers(v: &Value) -> Option<ProperDownloadType> {
    match v {
        Value::String(s) => ProperDownloadType::from_sonarr(s),
        Value::Number(n) => match n.as_u64()? {
            0 => Some(ProperDownloadType::PreferAndUpgrade),
            1 => Some(ProperDownloadType::DoNotUpgrade),
            2 => Some(ProperDownloadType::DoNotPrefer),
            _ => None,
        },
        _ => None,
    }
}

fn media_resource(m: &MediaManagementSettings) -> Value {
    resource(
        m,
        MEDIA,
        json!({
            "recycleBin": m.recycle_bin.clone().unwrap_or_default(),
            "downloadPropersAndRepacks": m.download_propers_and_repacks,
            // Tanukarr's "preserve original" has no Sonarr name; it reads as "none".
            "fileDate": match m.file_date {
                FileDate::LocalAirDate => "localAirDate",
                FileDate::UtcAirDate => "utcAirDate",
                FileDate::None | FileDate::PreserveOriginal => "none",
            },
            "kitsunarrFileDate": m.file_date,
            "skipFreeSpaceCheckWhenGrabbing": m.skip_free_space_check,
            "rescanAfterRefresh": "always",
            "chownGroup": "",
            "episodeTitleRequired": "always",
            "useScriptImport": false,
            "scriptImportPath": "",
            "userRejectedExtensions": "",
            "seasonPackUpgrade": "all",
            "seasonPackUpgradeThreshold": 100.0,
        }),
    )
}

async fn media_management(state: &AppState) -> ApiResult<MediaManagementSettings> {
    Ok(kv::get_or_default(state.db.pool(), kv::KEY_MEDIA_MANAGEMENT).await?)
}

async fn get_media_management(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(media_resource(&media_management(&state).await?)))
}

async fn put_media_management(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let mut next = merged(&media_management(&state).await?, &body, MEDIA)?;
    if let Some(bin) = body.get("recycleBin").and_then(Value::as_str) {
        let bin = bin.trim();
        next.recycle_bin = (!bin.is_empty()).then(|| bin.to_string());
    }
    if let Some(v) = body
        .get("downloadPropersAndRepacks")
        .filter(|v| !v.is_null())
    {
        next.download_propers_and_repacks = propers(v).ok_or_else(|| {
            ApiError::field(
                "downloadPropersAndRepacks",
                "Use preferAndUpgrade, doNotUpgrade or doNotPrefer.",
            )
        })?;
    }
    // Kitsunarr's own value first (it keeps "preserve original"), then Sonarr's.
    let date = body
        .get("kitsunarrFileDate")
        .or_else(|| body.get("fileDate"))
        .filter(|v| !v.is_null());
    if let Some(v) = date {
        let text = v.as_str().map_or_else(|| v.to_string(), str::to_string);
        let parsed = FileDate::from_sonarr(&text)
            .ok_or_else(|| ApiError::field("fileDate", "Use none, localAirDate or utcAirDate."))?;
        // Writing Sonarr's "none" back keeps Tanukarr's "preserve original".
        if !(parsed == FileDate::None
            && next.file_date == FileDate::PreserveOriginal
            && body.get("kitsunarrFileDate").is_none())
        {
            next.file_date = parsed;
        }
    }
    let saved = settings::put_media_management(State(state), Json(next))
        .await?
        .0;
    Ok(accepted(media_resource(&saved)))
}

// ---- indexers, download clients and import lists ----------------------------------------------

const INDEXER: Keys = &[
    ("minimumAge", "minimumAge"),
    ("retention", "retention"),
    ("maximumSize", "maximumSize"),
    ("rssSyncInterval", "rssSyncInterval"),
    ("enableMultiSeasonReleases", "enableMultiSeasonReleases"),
    ("enableFakeReleaseProtection", "enableFakeReleaseProtection"),
];

async fn get_indexer(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let s: IndexerOptionsSettings =
        kv::get_or_default(state.db.pool(), kv::KEY_INDEXER_OPTIONS).await?;
    Ok(Json(resource(&s, INDEXER, json!({}))))
}

async fn put_indexer(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let stored: IndexerOptionsSettings =
        kv::get_or_default(state.db.pool(), kv::KEY_INDEXER_OPTIONS).await?;
    let next = merged(&stored, &body, INDEXER)?;
    let saved = settings::put_indexer_options(State(state), Json(next))
        .await?
        .0;
    Ok(accepted(resource(&saved, INDEXER, json!({}))))
}

const DOWNLOAD_CLIENT: Keys = &[
    (
        "enableCompletedDownloadHandling",
        "enableCompletedDownloadHandling",
    ),
    ("autoRedownloadFailed", "autoRedownloadFailed"),
    (
        "autoRedownloadFailedFromInteractiveSearch",
        "autoRedownloadFailedFromInteractiveSearch",
    ),
    ("stalledTorrentTimeout", "stalledTorrentTimeout"),
];

fn download_client_fixed() -> Value {
    json!({"downloadClientWorkingFolders": "_UNPACK_|_FAILED_"})
}

async fn get_download_client(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let s: DownloadClientOptionsSettings =
        kv::get_or_default(state.db.pool(), kv::KEY_DOWNLOAD_CLIENT_OPTIONS).await?;
    Ok(Json(resource(&s, DOWNLOAD_CLIENT, download_client_fixed())))
}

async fn put_download_client(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    int_in(&body, "stalledTorrentTimeout", 0..=43200)?;
    let stored: DownloadClientOptionsSettings =
        kv::get_or_default(state.db.pool(), kv::KEY_DOWNLOAD_CLIENT_OPTIONS).await?;
    let next = merged(&stored, &body, DOWNLOAD_CLIENT)?;
    let saved = settings::put_download_client_options(State(state), Json(next))
        .await?
        .0;
    Ok(accepted(resource(
        &saved,
        DOWNLOAD_CLIENT,
        download_client_fixed(),
    )))
}

fn import_list_resource(s: &ImportListOptionsSettings) -> Value {
    json!({
        "id": 1,
        "listSyncLevel": s.list_sync_level,
        // Sonarr's 0 is "no tag".
        "listSyncTag": s.list_sync_tag.unwrap_or(0),
    })
}

async fn get_import_list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let s = import_lists::get_options(State(state)).await?.0;
    Ok(Json(import_list_resource(&s)))
}

async fn put_import_list(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let mut next = import_lists::get_options(State(state.clone())).await?.0;
    if let Some(level) = body.get("listSyncLevel").filter(|v| !v.is_null()) {
        let names = ["disabled", "logOnly", "keepAndUnmonitor", "keepAndTag"];
        let name = match level {
            Value::String(s) => names.iter().find(|n| n.eq_ignore_ascii_case(s)).copied(),
            Value::Number(n) => n
                .as_u64()
                .and_then(|i| names.get(usize::try_from(i).ok()?))
                .copied(),
            _ => None,
        }
        .ok_or_else(|| {
            ApiError::field(
                "listSyncLevel",
                "Use disabled, logOnly, keepAndUnmonitor or keepAndTag.",
            )
        })?;
        next.list_sync_level = serde_json::from_value(json!(name)).map_err(ApiError::internal)?;
    }
    if let Some(tag) = body.get("listSyncTag").filter(|v| !v.is_null()) {
        next.list_sync_tag = tag.as_i64().filter(|t| *t > 0);
    }
    let saved = import_lists::put_options(State(state), Json(next)).await?.0;
    Ok(accepted(import_list_resource(&saved)))
}

// ---- interface and host ---------------------------------------------------------------------

async fn general(state: &AppState) -> ApiResult<GeneralSettings> {
    Ok(kv::get_or_default(state.db.pool(), kv::KEY_GENERAL).await?)
}

/// Sonarr's interface settings: the first day of the week is Kitsunarr's; the date formats and
/// theme are Sonarr's defaults (Kitsunarr formats dates by the browser's locale).
fn ui_resource(g: &GeneralSettings) -> Value {
    json!({
        "id": 1,
        "firstDayOfWeek": g.first_day_of_week,
        "calendarWeekColumnHeader": "ddd M/D",
        "shortDateFormat": "MMM D YYYY",
        "longDateFormat": "dddd, MMMM D YYYY",
        "timeFormat": "h(:mm)a",
        "timeZone": "",
        "showRelativeDates": true,
        "enableColorImpairedMode": false,
        "theme": "auto",
        "uiLanguage": 1,
        "kitsunarrUiLanguage": g.ui_language,
    })
}

async fn get_ui(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(ui_resource(&general(&state).await?)))
}

async fn put_ui(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    int_in(&body, "firstDayOfWeek", 0..=6)?;
    let mut next = general(&state).await?;
    if let Some(day) = body.get("firstDayOfWeek").and_then(Value::as_u64) {
        next.first_day_of_week = day as u8;
    }
    if let Some(code) = body.get("kitsunarrUiLanguage").and_then(Value::as_str) {
        next.ui_language = code.to_string();
    }
    let saved = settings::put_general(State(state), Json(next)).await?.0;
    Ok(accepted(ui_resource(&saved)))
}

fn proxy_type_name(t: ProxyType) -> &'static str {
    match t {
        ProxyType::Http => "http",
        ProxyType::Socks4 => "socks4",
        ProxyType::Socks5 => "socks5",
    }
}

/// Sonarr's host settings: the instance name, certificate validation and proxy are Kitsunarr's
/// and writable; the listening address, port and URL base come from `config.toml` (read-only
/// here). The API key and passwords are never returned.
fn host_resource(state: &AppState, g: &GeneralSettings, p: &ProxySettings) -> Value {
    let server = &state.config.server;
    json!({
        "id": 1,
        "bindAddress": server.bind_address.to_string(),
        "port": server.port,
        "sslPort": 9898,
        "enableSsl": false,
        "launchBrowser": false,
        "authenticationMethod": "forms",
        "authenticationRequired": "enabled",
        "allowedHosts": "",
        "analyticsEnabled": false,
        "username": "",
        "password": "",
        "passwordConfirmation": "",
        "logLevel": state.config.log.level,
        "logSizeLimit": 1,
        "consoleLogLevel": "",
        "branch": "main",
        "apiKey": "",
        "sslCertPath": "",
        "sslKeyPath": "",
        "sslCertPassword": "",
        "urlBase": server.url_base,
        "trustedNetworks": "",
        "instanceName": g.instance_name,
        "applicationUrl": "",
        "updateAutomatically": false,
        "updateMechanism": "docker",
        "updateScriptPath": "",
        "proxyEnabled": p.enabled,
        "proxyType": proxy_type_name(p.proxy_type),
        "proxyHostname": p.hostname,
        "proxyPort": p.port,
        "proxyUsername": p.username,
        "proxyPassword": if p.password.is_empty() { "" } else { kitsunarr_domain::api::settings::REDACTED },
        "proxyBypassFilter": p.bypass_filter,
        "proxyBypassLocalAddresses": p.bypass_local_addresses,
        "certificateValidation": g.certificate_validation,
        "backupFolder": state.config.backup_dir().display().to_string(),
        "backupInterval": 7,
        "backupRetention": 28,
        "trustCgnatIpAddresses": false,
    })
}

async fn proxy(state: &AppState) -> ApiResult<ProxySettings> {
    Ok(settings::get_proxy(State(state.clone())).await?.0)
}

async fn get_host(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let (g, p) = (general(&state).await?, proxy(&state).await?);
    let resource = host_resource(&state, &g, &p);
    Ok(Json(
        crate::security::with_v3_host_fields(&state, resource).await?,
    ))
}

async fn put_host(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let mut g = general(&state).await?;
    if let Some(name) = body.get("instanceName").and_then(Value::as_str) {
        g.instance_name = name.to_string();
    }
    if let Some(v) = body.get("certificateValidation").filter(|v| !v.is_null()) {
        let text = v.as_str().map_or_else(|| v.to_string(), str::to_string);
        g.certificate_validation = kitsunarr_domain::api::settings::CertificateValidation::parse(
            &text,
        )
        .ok_or_else(|| {
            ApiError::field(
                "certificateValidation",
                "Use enabled, disabledForLocalAddresses or disabled.",
            )
        })?;
    }
    let mut p = proxy(&state).await?;
    let mut proxy_changed = false;
    if let Some(v) = body.get("proxyEnabled").and_then(Value::as_bool) {
        proxy_changed |= v != p.enabled;
        p.enabled = v;
    }
    if let Some(v) = body.get("proxyType").filter(|v| !v.is_null()) {
        let text = v.as_str().map_or_else(|| v.to_string(), str::to_string);
        let t = ProxyType::parse(&text)
            .ok_or_else(|| ApiError::field("proxyType", "Use http, socks4 or socks5."))?;
        proxy_changed |= t != p.proxy_type;
        p.proxy_type = t;
    }
    for (key, field) in [
        ("proxyHostname", &mut p.hostname),
        ("proxyUsername", &mut p.username),
        ("proxyPassword", &mut p.password),
        ("proxyBypassFilter", &mut p.bypass_filter),
    ] {
        if let Some(v) = body.get(key).and_then(Value::as_str)
            && v != field.as_str()
        {
            *field = v.to_string();
            proxy_changed = true;
        }
    }
    if let Some(v) = body.get("proxyPort").and_then(Value::as_u64) {
        let port = u16::try_from(v).map_err(|_| ApiError::field("proxyPort", "Use a port."))?;
        proxy_changed |= port != p.port;
        p.port = port;
    }
    if let Some(v) = body
        .get("proxyBypassLocalAddresses")
        .and_then(Value::as_bool)
    {
        proxy_changed |= v != p.bypass_local_addresses;
        p.bypass_local_addresses = v;
    }
    // Validate the proxy before anything is saved.
    if proxy_changed {
        crate::proxy::validate(&p).map_err(|(field, message)| ApiError::field(field, message))?;
    }
    // Authentication, allowed hosts, OIDC and the account (Settings › General › Security).
    crate::security::apply_v3_host(&state, &body).await?;
    let g = settings::put_general(State(state.clone()), Json(g))
        .await?
        .0;
    let p = if proxy_changed {
        settings::put_proxy(State(state.clone()), Json(p)).await?.0
    } else {
        p
    };
    let resource = host_resource(&state, &g, &p);
    Ok(accepted(
        crate::security::with_v3_host_fields(&state, resource).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_rename_keys_and_merges_keep_the_rest() {
        let naming = NamingSettings::default();
        let r = resource(&naming, NAMING, json!({}));
        assert_eq!(r["id"], 1);
        assert_eq!(r["movieFormat"], naming.movie_file_format);
        assert_eq!(r["standardEpisodeFormat"], naming.standard_episode_format);
        let next = merged(
            &naming,
            &json!({"seasonFolderFormat": "S{season:00}", "movieFormat": "{Movie Title}", "unknown": 1, "renameEpisodes": null}),
            NAMING,
        )
        .unwrap();
        assert_eq!(next.season_folder_format, "S{season:00}");
        assert_eq!(next.movie_file_format, "{Movie Title}");
        assert_eq!(next.rename_episodes, naming.rename_episodes);
        assert_eq!(next.daily_episode_format, naming.daily_episode_format);
        assert!(merged(&naming, &json!({"renameEpisodes": "yes"}), NAMING).is_err());

        let media = MediaManagementSettings::default();
        let r = media_resource(&media);
        assert_eq!(r["copyUsingHardlinks"], media.use_hardlinks);
        assert_eq!(
            r["minimumFreeSpaceWhenImporting"],
            media.minimum_free_space_mb
        );
        assert_eq!(r["downloadPropersAndRepacks"], "preferAndUpgrade");
        assert_eq!(r["recycleBin"], "");
        assert_eq!(
            propers(&json!("doNotPrefer")),
            Some(ProperDownloadType::DoNotPrefer)
        );
        assert_eq!(propers(&json!(1)), Some(ProperDownloadType::DoNotUpgrade));
        assert_eq!(propers(&json!("x")), None);
    }
}
