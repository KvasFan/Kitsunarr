//! Provider templates (Sonarr's `GET /api/v3/{provider}/schema`): one blank provider per
//! implementation Kitsunarr runs, with upstream's field names and defaults, so new indexers,
//! download clients and notifications can be added from the web UI.

use axum::Json;
use axum::extract::Path;
use kitsunarr_db::providers;
use kitsunarr_domain::api::settings::{ProviderItem, ProviderKind};
use serde_json::{Map, Value, json};

use crate::error::ApiResult;
use crate::routes::settings::parse_kind;

fn map(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn template(
    kind: ProviderKind,
    name: &str,
    implementation: &str,
    options: Value,
    settings: Value,
) -> ProviderItem {
    let protocol = providers::protocol_for(implementation);
    ProviderItem {
        id: 0,
        kind,
        name: name.into(),
        implementation: implementation.into(),
        protocol: (protocol != kitsunarr_domain::DownloadProtocol::Unknown).then_some(protocol),
        enabled: true,
        priority: if kind == ProviderKind::Indexer { 25 } else { 1 },
        tags: Vec::new(),
        supported: providers::is_supported(kind, implementation),
        options: map(options),
        settings: map(settings),
    }
}

/// The templates for a kind of provider.
pub fn templates(kind: ProviderKind) -> Vec<ProviderItem> {
    let indexer_options = json!({
        "enableRss": true, "enableAutomaticSearch": true, "enableInteractiveSearch": true,
        "downloadClientId": 0, "seasonSearchMaximumSingleEpisodeAge": 0
    });
    let client_options = json!({"removeCompletedDownloads": true, "removeFailedDownloads": true});
    match kind {
        ProviderKind::Indexer => vec![
            template(
                kind,
                "Newznab",
                "Newznab",
                indexer_options.clone(),
                json!({
                    "baseUrl": "", "apiPath": "/api", "apiKey": "",
                    "categories": [5030, 5040], "animeCategories": [],
                    "animeStandardFormatSearch": false, "additionalParameters": ""
                }),
            ),
            template(
                kind,
                "Torznab",
                "Torznab",
                indexer_options,
                json!({
                    "baseUrl": "", "apiPath": "/api", "apiKey": "",
                    "categories": [5030, 5040], "animeCategories": [],
                    "animeStandardFormatSearch": false, "additionalParameters": "",
                    "minimumSeeders": 1
                }),
            ),
        ],
        ProviderKind::DownloadClient => vec![
            template(
                kind,
                "qBittorrent",
                "QBittorrent",
                client_options.clone(),
                json!({
                    "host": "localhost", "port": 8080, "useSsl": false, "urlBase": "",
                    "username": "", "password": "", "tvCategory": "tv-kitsunarr", "tvImportedCategory": "",
                    "recentTvPriority": 0, "olderTvPriority": 0, "initialState": 0,
                    "sequentialOrder": false, "firstAndLast": false, "contentLayout": 0,
                    "addSeriesTags": false
                }),
            ),
            template(
                kind,
                "Deluge",
                "Deluge",
                client_options.clone(),
                json!({
                    "host": "localhost", "port": 8112, "useSsl": false, "urlBase": "",
                    "password": "deluge", "tvCategory": "tv-kitsunarr", "tvImportedCategory": "",
                    "recentTvPriority": 0, "olderTvPriority": 0, "addPaused": false,
                    "downloadDirectory": "", "completedDirectory": ""
                }),
            ),
            template(
                kind,
                "Transmission",
                "Transmission",
                client_options.clone(),
                json!({
                    "host": "localhost", "port": 9091, "useSsl": false, "urlBase": "/transmission/",
                    "username": "", "password": "", "tvCategory": "tv-kitsunarr", "tvImportedCategory": "",
                    "tvDirectory": "", "recentTvPriority": 0, "olderTvPriority": 0, "addPaused": false
                }),
            ),
            template(
                kind,
                "SABnzbd",
                "Sabnzbd",
                client_options.clone(),
                json!({
                    "host": "localhost", "port": 8080, "useSsl": false, "urlBase": "",
                    "apiKey": "", "username": "", "password": "", "tvCategory": "tv",
                    "recentTvPriority": -100, "olderTvPriority": -100
                }),
            ),
            template(
                kind,
                "NZBGet",
                "Nzbget",
                client_options,
                json!({
                    "host": "localhost", "port": 6789, "useSsl": false, "urlBase": "",
                    "username": "", "password": "", "tvCategory": "tv",
                    "recentTvPriority": 0, "olderTvPriority": 0, "addPaused": false
                }),
            ),
        ],
        ProviderKind::Notification | ProviderKind::ImportList => Vec::new(),
    }
    .into_iter()
    .filter(|t| t.supported)
    .collect()
}

/// Blank providers of each implementation Kitsunarr runs, with upstream's defaults.
#[utoipa::path(get, path = "/providers/{kind}/schema", tag = "settings",
    params(("kind" = String, Path, description = "indexers | downloadclients | notifications | importlists")),
    responses((status = 200, body = Vec<ProviderItem>)))]
pub async fn provider_schema(Path(kind): Path<String>) -> ApiResult<Json<Vec<ProviderItem>>> {
    Ok(Json(templates(parse_kind(&kind)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every template parses with the settings reader the runtime uses.
    #[test]
    fn templates_parse_as_runtime_settings() {
        for t in templates(ProviderKind::DownloadClient) {
            let mut settings = t.settings.clone();
            // SABnzbd needs credentials to be valid.
            settings.insert("apiKey".into(), "k".into());
            kitsunarr_downloaders::ClientSettings::from_provider(&t.implementation, &settings)
                .unwrap_or_else(|e| panic!("{}: {e}", t.implementation));
        }
        for t in templates(ProviderKind::Indexer) {
            let mut settings = t.settings.clone();
            settings.insert("baseUrl".into(), "http://indexer".into());
            kitsunarr_indexers::request::Settings::from_map(&settings)
                .unwrap_or_else(|e| panic!("{}: {e}", t.implementation));
        }
        assert_eq!(templates(ProviderKind::DownloadClient).len(), 5);
    }
}
