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
        ProviderKind::Notification => {
            let on = json!({
                "onGrab": true, "onDownload": true, "onUpgrade": true, "onSeriesAdd": false,
                "onSeriesDelete": false, "onEpisodeFileDelete": false, "onHealthIssue": false,
                "includeHealthWarnings": false, "onHealthRestored": false
            });
            let n = |name: &str, implementation: &str, settings: Value| {
                template(kind, name, implementation, on.clone(), settings)
            };
            vec![
                n("Webhook", "Webhook", json!({"url": "", "method": 1, "username": "", "password": "", "headers": []})),
                n("Discord", "Discord", json!({"webHookUrl": "", "username": "", "avatar": "", "author": "",
                    "grabFields": [0, 1, 2, 3, 5, 6, 7, 8, 9], "importFields": [0, 1, 2, 3, 5, 6, 9, 10, 11, 12]})),
                n("Telegram", "Telegram", json!({"botToken": "", "chatId": "", "topicId": "", "sendSilently": false, "includeAppNameInTitle": false})),
                n("Slack", "Slack", json!({"webHookUrl": "", "username": "", "icon": "", "channel": ""})),
                n("Gotify", "Gotify", json!({"server": "", "appToken": "", "priority": 5, "includeSeriesPoster": false})),
                n("ntfy", "Ntfy", json!({"serverUrl": "https://ntfy.sh", "accessToken": "", "userName": "", "password": "", "priority": 3, "topics": [], "tags": [], "clickUrl": ""})),
                n("Pushover", "Pushover", json!({"apiKey": "", "userKey": "", "devices": [], "priority": 0, "retry": 0, "expire": 0, "sound": ""})),
                n("Pushbullet", "PushBullet", json!({"apiKey": "", "deviceIds": [], "channelTags": [], "senderId": ""})),
                n("Join", "Join", json!({"apiKey": "", "deviceNames": "", "priority": 0})),
                n("Prowl", "Prowl", json!({"apiKey": "", "priority": 0})),
                n("Simplepush", "Simplepush", json!({"key": "", "event": ""})),
                n("Signal", "Signal", json!({"host": "localhost", "port": 8080, "useSsl": false, "senderNumber": "", "receiverId": "", "authUsername": "", "authPassword": ""})),
                n("Apprise", "Apprise", json!({"serverUrl": "", "configurationKey": "", "statelessUrls": "", "notificationType": 0, "tags": [], "authUsername": "", "authPassword": ""})),
                n("Notifiarr", "Notifiarr", json!({"apiKey": ""})),
                n("Emby / Jellyfin", "MediaBrowser", json!({"host": "localhost", "port": 8096, "useSsl": false, "urlBase": "", "apiKey": "", "notify": false, "updateLibrary": true, "mapFrom": "", "mapTo": ""})),
                n("Jellyfin", "Jellyfin", json!({"host": "localhost", "port": 8096, "useSsl": false, "urlBase": "", "apiKey": "", "libraryId": "", "updateLibrary": true, "enableWatchedSync": false, "watchedUserId": "", "watchedSyncIntervalMinutes": 5, "unmonitorWatchedEpisodes": false, "remonitorUnwatchedEpisodes": false, "jellyfinPathPrefix": "", "kitsunarrPathPrefix": ""})),
                n("Plex Media Server", "PlexServer", json!({"host": "localhost", "port": 32400, "useSsl": false, "urlBase": "", "authToken": "", "updateLibrary": true, "mapFrom": "", "mapTo": ""})),
                n("Kodi", "Xbmc", json!({"host": "localhost", "port": 8080, "useSsl": false, "urlBase": "", "username": "", "password": "", "displayTime": 5, "notify": false, "updateLibrary": true, "cleanLibrary": false})),
            ]
        }
        ProviderKind::ImportList => Vec::new(),
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
