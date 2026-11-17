//! Sonarr v5's `settings/*` resources: v3's `config/*` under new names, each with exactly the
//! fields of its v5 resource (`settings/general` and `settings/update` split v3's host
//! settings; `settings/metadata` and `settings/metadatasource` are Tanukarr's catalogue
//! settings, with the preferred metadata language).

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::ApiResult;
use crate::{v3_config, v3_extra};

/// A v5 resource's fields from the v3 resource (`id` 1; a field v3 does not have is null).
fn pick(v: &Value, keys: &[&str]) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(1));
    for k in keys {
        out.insert((*k).into(), v.get(*k).cloned().unwrap_or(Value::Null));
    }
    Value::Object(out)
}

/// `get`/`put` handlers of a v5 settings resource over v3's.
macro_rules! settings {
    ($get:ident, $put:ident, $v3get:path, $v3put:path, $keys:expr) => {
        async fn $get(State(state): State<AppState>) -> ApiResult<Json<Value>> {
            let Json(v) = $v3get(State(state)).await?;
            Ok(Json(pick(&v, $keys)))
        }

        async fn $put(
            State(state): State<AppState>,
            Json(body): Json<Value>,
        ) -> ApiResult<(StatusCode, Json<Value>)> {
            let (_, Json(v)) = $v3put(State(state), Json(body)).await?;
            Ok((StatusCode::ACCEPTED, Json(pick(&v, $keys))))
        }
    };
}

const DOWNLOAD_CLIENT: &[&str] = &[
    "autoRedownloadFailed",
    "autoRedownloadFailedFromInteractiveSearch",
    "downloadClientWorkingFolders",
    "enableCompletedDownloadHandling",
    "stalledTorrentTimeout",
];
const IMPORT_LIST: &[&str] = &["listSyncLevel", "listSyncTag"];
const INDEXER: &[&str] = &[
    "enableFakeReleaseProtection",
    "enableMultiSeasonReleases",
    "maximumSize",
    "minimumAge",
    "retention",
    "rssSyncInterval",
];
const MEDIA_MANAGEMENT: &[&str] = &[
    "autoUnmonitorPreviouslyDownloadedEpisodes",
    "chmodFolder",
    "chownGroup",
    "copyUsingHardlinks",
    "createEmptySeriesFolders",
    "deleteEmptyFolders",
    "downloadPropersAndRepacks",
    "enableMediaInfo",
    "episodeTitleRequired",
    "extraFileExtensions",
    "fileDate",
    "importExtraFiles",
    "minimumFreeSpaceWhenImporting",
    "recycleBin",
    "recycleBinCleanupDays",
    "rescanAfterRefresh",
    "scriptImportPath",
    "seasonPackUpgrade",
    "seasonPackUpgradeThreshold",
    "setPermissionsLinux",
    "skipFreeSpaceCheckWhenGrabbing",
    "skipFreeSpaceCheckWhenImporting",
    "skipUnmonitoredEpisodesFromPacks",
    "useScriptImport",
    "userRejectedExtensions",
];
const NAMING: &[&str] = &[
    "animeEpisodeFormat",
    "animeGroupArcFolderFormat",
    "animeGroupFolderFormat",
    "animeGroupMovieFolderFormat",
    "animeGroupMovieSagaFolderFormat",
    "animeGroupSeasonFolderFormat",
    "colonReplacementFormat",
    "customColonReplacementFormat",
    "dailyEpisodeFormat",
    "movieFormat",
    "multiEpisodeStyle",
    "renameEpisodes",
    "replaceIllegalCharacters",
    "seasonFolderFormat",
    "seriesFolderFormat",
    "specialsFolderFormat",
    "standardEpisodeFormat",
];
const UI: &[&str] = &[
    "calendarWeekColumnHeader",
    "enableColorImpairedMode",
    "firstDayOfWeek",
    "longDateFormat",
    "shortDateFormat",
    "showRelativeDates",
    "theme",
    "timeFormat",
    "timeZone",
    "uiLanguage",
];
const UPDATE: &[&str] = &[
    "branch",
    "updateAutomatically",
    "updateMechanism",
    "updateScriptPath",
];
const GENERAL: &[&str] = &[
    "allowedHosts",
    "analyticsEnabled",
    "apiKey",
    "applicationUrl",
    "authenticationMethod",
    "authenticationRequired",
    "backupFolder",
    "backupInterval",
    "backupRetention",
    "bindAddress",
    "branch",
    "certificateValidation",
    "consoleLogLevel",
    "enableSsl",
    "instanceName",
    "launchBrowser",
    "logLevel",
    "logSizeLimit",
    "oidcAuthority",
    "oidcClientId",
    "oidcClientSecret",
    "oidcScopes",
    "oidcUserIdentifier",
    "password",
    "passwordConfirmation",
    "port",
    "proxyBypassFilter",
    "proxyBypassLocalAddresses",
    "proxyEnabled",
    "proxyHostname",
    "proxyPassword",
    "proxyPort",
    "proxyType",
    "proxyUsername",
    "sslCertPassword",
    "sslCertPath",
    "sslKeyPath",
    "sslPort",
    "trustedNetworks",
    "updateAutomatically",
    "updateMechanism",
    "updateScriptPath",
    "urlBase",
    "username",
];

settings!(
    get_download_client,
    put_download_client,
    v3_config::get_download_client,
    v3_config::put_download_client,
    DOWNLOAD_CLIENT
);
settings!(
    get_import_list,
    put_import_list,
    v3_config::get_import_list,
    v3_config::put_import_list,
    IMPORT_LIST
);
settings!(
    get_indexer,
    put_indexer,
    v3_config::get_indexer,
    v3_config::put_indexer,
    INDEXER
);
settings!(
    get_media_management,
    put_media_management,
    v3_config::get_media_management,
    v3_config::put_media_management,
    MEDIA_MANAGEMENT
);
settings!(
    get_naming,
    put_naming,
    v3_config::get_naming,
    v3_config::put_naming,
    NAMING
);
settings!(get_ui, put_ui, v3_config::get_ui, v3_config::put_ui, UI);
settings!(
    get_general,
    put_general,
    v3_config::get_host,
    v3_config::put_host,
    GENERAL
);
settings!(
    get_update,
    put_update,
    v3_config::get_host,
    v3_config::put_host,
    UPDATE
);

async fn stored_sources(
    state: &AppState,
) -> ApiResult<kitsunarr_domain::api::settings::MetadataSourcesSettings> {
    Ok(kitsunarr_db::settings::get_or_default(
        state.db.pool(),
        kitsunarr_db::settings::KEY_METADATA_SOURCES,
    )
    .await?)
}

/// Tanukarr's `MetadataSourceSettingsResource`.
async fn get_metadata(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(v3_extra::metadata_source_json(
        &stored_sources(&state).await?,
        true,
    )))
}

async fn put_metadata(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let next = v3_extra::apply_metadata_source(&stored_sources(&state).await?, &body)?;
    let saved = v3_extra::save_metadata_sources(&state, next).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(v3_extra::metadata_source_json(&saved, true)),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/downloadclient", get(get_download_client))
        .route(
            "/settings/downloadclient/{id}",
            get(get_download_client).put(put_download_client),
        )
        .route("/settings/importlist", get(get_import_list))
        .route(
            "/settings/importlist/{id}",
            get(get_import_list).put(put_import_list),
        )
        .route("/settings/indexer", get(get_indexer))
        .route("/settings/indexer/{id}", get(get_indexer).put(put_indexer))
        .route("/settings/mediamanagement", get(get_media_management))
        .route(
            "/settings/mediamanagement/{id}",
            get(get_media_management).put(put_media_management),
        )
        .route("/settings/naming", get(get_naming))
        .route("/settings/naming/examples", get(v3_extra::naming_examples))
        .route("/settings/naming/{id}", get(get_naming).put(put_naming))
        .route("/settings/ui", get(get_ui))
        .route("/settings/ui/{id}", get(get_ui).put(put_ui))
        .route("/settings/general", get(get_general))
        .route("/settings/general/{id}", get(get_general).put(put_general))
        .route("/settings/update", get(get_update))
        .route("/settings/update/{id}", get(get_update).put(put_update))
        .route("/settings/metadata", get(get_metadata))
        .route(
            "/settings/metadata/{id}",
            get(get_metadata).put(put_metadata),
        )
        .route("/settings/metadatasource", get(get_metadata))
        .route(
            "/settings/metadatasource/{id}",
            get(get_metadata).put(put_metadata),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v5_settings_keep_exactly_their_fields() {
        let v3 =
            json!({"id": 1, "listSyncLevel": "disabled", "listSyncTag": 0, "kitsunarrExtra": true});
        assert_eq!(
            pick(&v3, IMPORT_LIST),
            json!({"id": 1, "listSyncLevel": "disabled", "listSyncTag": 0})
        );
        assert_eq!(pick(&json!({}), UPDATE)["branch"], Value::Null);
    }
}
