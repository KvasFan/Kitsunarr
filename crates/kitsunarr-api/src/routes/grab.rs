//! Grabbing releases (Sonarr's `ReleaseController.Create`, `DownloadService.DownloadReport`,
//! `DownloadClientProvider`) and testing download clients.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use kitsunarr_db::history::{self, NewHistory};
use kitsunarr_db::library::series;
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_db::tags;
use kitsunarr_domain::DownloadProtocol;
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::search::{
    DownloadClientTestResult, GrabRequest, GrabResult, IndexerTestRequest,
};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_domain::quality::QualityModel;
use kitsunarr_downloaders::{AddRequest, ClientSettings, Source};
use kitsunarr_downloaders::{DownloadError, magnet_info_hash, torrent_info_hash};
use kitsunarr_indexers::{Downloaded, IndexerRelease};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

/// Releases are kept for grabbing for 30 minutes after a search, as upstream.
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);

/// A searched release with its decision, kept server-side so download links (which embed
/// indexer API keys) never travel to the browser.
#[derive(Debug, Clone)]
pub struct CachedRelease {
    pub release: IndexerRelease,
    pub series_id: i64,
    pub episode_ids: Vec<i64>,
    pub quality: QualityModel,
    pub languages: Vec<i32>,
    pub custom_format_score: i32,
    pub release_group: Option<String>,
    /// Sonarr's `ReleaseType` name, stored with the grab.
    pub release_type: &'static str,
    /// An episode aired within the last 14 days (`IsRecentEpisode`).
    pub recent: bool,
    /// Sonarr's `ReleaseSourceType`: `Rss`, `Search` or `InteractiveSearch`.
    pub release_source: &'static str,
    pub cached_at: Instant,
}

/// Search results by (indexer id, GUID).
#[derive(Default)]
pub struct ReleaseCache(Mutex<HashMap<(i64, String), CachedRelease>>);

impl ReleaseCache {
    pub fn insert(&self, r: CachedRelease) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, v| v.cached_at.elapsed() < CACHE_TTL);
        map.insert((r.release.indexer_id, r.release.release.guid.clone()), r);
    }

    pub fn get(&self, indexer_id: i64, guid: &str) -> Option<CachedRelease> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(indexer_id, guid.to_string()))
            .filter(|r| r.cached_at.elapsed() < CACHE_TTL)
            .cloned()
    }
}

fn describe(e: &DownloadError) -> String {
    match e {
        DownloadError::Connection(m) => format!("Could not connect to the download client: {m}"),
        DownloadError::Authentication => {
            "The download client rejected the credentials; check the username and password (or API key).".into()
        }
        DownloadError::Http { status } => format!("The download client answered HTTP {status}."),
        other => {
            let m = other.to_string();
            let mut c = m.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>() + ".")
                .unwrap_or_default()
        }
    }
}

/// Test a download client's settings.
#[utoipa::path(post, path = "/providers/downloadclients/test", tag = "search", request_body = IndexerTestRequest,
    responses(
        (status = 200, body = DownloadClientTestResult, description = "Test outcome (ok or not)"),
        (status = 422, body = ApiErrorBody)
    ))]
pub async fn test_download_client(
    State(state): State<AppState>,
    Json(mut req): Json<IndexerTestRequest>,
) -> ApiResult<Json<DownloadClientTestResult>> {
    if !kitsunarr_downloaders::supported(&req.implementation) {
        return Err(ApiError::field(
            "implementation",
            format!(
                "Testing {} download clients is not supported yet.",
                req.implementation
            ),
        ));
    }
    if let Some(id) = req.id {
        let stored = providers::get(state.db.pool(), ProviderKind::DownloadClient, id).await?;
        crate::routes::settings::merge_secrets(&mut req.settings, &stored.settings);
    }
    let settings = ClientSettings::from_provider(&req.implementation, &req.settings)
        .map_err(|m| ApiError::field("host", m))?;
    Ok(Json(match state.downloads.clients.test(&settings).await {
        Ok(report) => DownloadClientTestResult {
            ok: true,
            message: report.message,
            warnings: report.warnings,
        },
        Err(e) => DownloadClientTestResult {
            ok: false,
            message: describe(&e),
            warnings: Vec::new(),
        },
    }))
}

/// `DownloadClientProvider.GetDownloadClient`: clients for the protocol; those sharing a tag
/// with the series, else those without tags; the indexer's own client when it names one;
/// then the best priority, rotating between equals.
async fn choose_client(
    state: &AppState,
    protocol: DownloadProtocol,
    indexer_id: i64,
    series_tags: &[i64],
) -> ApiResult<ProviderRecord> {
    let all: Vec<ProviderRecord> = providers::list(state.db.pool(), ProviderKind::DownloadClient)
        .await?
        .into_iter()
        .filter(|c| c.enabled && c.protocol == protocol)
        .collect();
    let label = match protocol {
        DownloadProtocol::Torrent => "torrent",
        DownloadProtocol::Usenet => "Usenet",
        DownloadProtocol::Unknown => "this",
    };
    if all.is_empty() {
        return Err(ApiError::field(
            "guid",
            format!(
                "No {label} download client is enabled. Add one in Settings › Download clients."
            ),
        ));
    }
    let tagged: Vec<&ProviderRecord> = all
        .iter()
        .filter(|c| c.tags.iter().any(|t| series_tags.contains(t)))
        .collect();
    let mut candidates: Vec<&ProviderRecord> = if tagged.is_empty() {
        all.iter().filter(|c| c.tags.is_empty()).collect()
    } else {
        tagged
    };
    if candidates.is_empty() {
        return Err(ApiError::field(
            "guid",
            "No download client was found without tags or with one of the series' tags.",
        ));
    }
    let indexer = providers::get(state.db.pool(), ProviderKind::Indexer, indexer_id)
        .await
        .ok();
    if let Some(indexer) = &indexer
        && let Some(wanted) = indexer
            .options
            .get("downloadClientId")
            .and_then(|v| v.as_i64())
            .filter(|id| *id > 0)
    {
        candidates.retain(|c| c.id == wanted);
        if candidates.is_empty() {
            return Err(ApiError::field(
                "guid",
                format!(
                    "The download client chosen for the indexer {} is not available.",
                    indexer.name
                ),
            ));
        }
    }
    let best = candidates.iter().map(|c| c.priority).min().unwrap_or(0);
    let mut equals: Vec<&ProviderRecord> = candidates
        .into_iter()
        .filter(|c| c.priority == best)
        .collect();
    equals.sort_by_key(|c| c.id);
    let key = label.to_string();
    let mut last = state
        .last_download_client
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let previous = last.get(&key).copied().unwrap_or(0);
    let chosen = equals
        .iter()
        .find(|c| c.id > previous)
        .or(equals.first())
        .copied()
        .cloned()
        .ok_or_else(|| ApiError::Internal("no download client".into()))?;
    last.insert(key, chosen.id);
    Ok(chosen)
}

/// Grab a release from a recent search: send it to a download client and record the grab.
#[utoipa::path(post, path = "/releases/grab", tag = "search", request_body = GrabRequest,
    responses(
        (status = 200, body = GrabResult),
        (status = 404, body = ApiErrorBody, description = "Not in the search cache any more"),
        (status = 422, body = ApiErrorBody)
    ))]
pub async fn grab_release(
    State(state): State<AppState>,
    Json(req): Json<GrabRequest>,
) -> ApiResult<Json<GrabResult>> {
    let cached = state
        .releases
        .get(req.indexer_id, &req.guid)
        .ok_or_else(|| {
            ApiError::NotFound(
                "The release is no longer in the search cache (results are kept for 30 minutes); search again.".into(),
            )
        })?;
    if cached.episode_ids.is_empty() {
        return Err(ApiError::field(
            "guid",
            "The release could not be matched to episodes of the series, so it cannot be grabbed (choosing episodes by hand is not implemented yet).",
        ));
    }
    let target = pick_client(&state, &cached).await?;
    send(&state, &cached, target).await.map(Json)
}

/// The download client a release goes to, with its parsed settings and the series' tags.
pub(crate) struct Target {
    client: ProviderRecord,
    settings: ClientSettings,
    series_tags: Vec<i64>,
}

/// Chooses the download client for a release (`DownloadClientProvider.GetDownloadClient`).
/// Errors mean no usable client for the release's protocol (`DownloadClientUnavailableException`).
pub(crate) async fn pick_client(state: &AppState, cached: &CachedRelease) -> ApiResult<Target> {
    let release = &cached.release.release;
    let series = series::get_list_item(&state.db, cached.series_id).await?;
    let client = choose_client(
        state,
        release.protocol,
        cached.release.indexer_id,
        &series.tags,
    )
    .await?;
    if !kitsunarr_downloaders::supported(&client.implementation) {
        return Err(ApiError::field(
            "guid",
            format!(
                "The download client {} uses {}, which Kitsunarr cannot send downloads to yet.",
                client.name, client.implementation
            ),
        ));
    }
    let settings = ClientSettings::from_provider(&client.implementation, &client.settings)
        .map_err(|m| {
            ApiError::field(
                "guid",
                format!(
                    "The download client {} has invalid settings: {m}",
                    client.name
                ),
            )
        })?;
    Ok(Target {
        client,
        settings,
        series_tags: series.tags,
    })
}

/// Sends a release to its download client and records the grab (`DownloadService.DownloadReport`).
pub(crate) async fn send(
    state: &AppState,
    cached: &CachedRelease,
    target: Target,
) -> ApiResult<GrabResult> {
    let Target {
        client,
        settings,
        series_tags,
    } = target;
    let release = &cached.release.release;
    let indexer_id = cached.release.indexer_id;
    let failed = |m: String| ApiError::field("guid", m);

    // `UsenetClientBase.Download`: fetch the NZB through the indexer, validate it, send it.
    if settings.is_usenet() {
        let url = release
            .download_url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .ok_or_else(|| failed("The release has no download link.".into()))?;
        let fetched = state
            .indexers
            .download(indexer_id, &url, "application/x-nzb")
            .await
            .map_err(|e| failed(format!("Downloading the NZB failed: {e}")))?;
        let Downloaded::File(bytes) = fetched else {
            return Err(failed(
                "The indexer returned a magnet link for a Usenet release.".into(),
            ));
        };
        let name = format!("{}.nzb", clean_file_name(&release.title));
        let id = state
            .downloads
            .clients
            .add_nzb(&settings, &name, &bytes, cached.recent)
            .await
            .map_err(|e| failed(describe(&e)))?;
        return finish(state, cached, &client, id).await;
    }

    // `TorrentClientBase.Download`: the magnet link first (qBittorrent does not prefer
    // torrent files), else the file behind the download link.
    let download_url = release.download_url.clone().unwrap_or_default();
    let magnet = release
        .magnet_url
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| {
            download_url
                .starts_with("magnet:")
                .then(|| download_url.clone())
        });
    let torrent_url =
        (!download_url.is_empty() && !download_url.starts_with("magnet:")).then_some(download_url);
    let tag_labels: Vec<String> = if settings.add_series_tags() {
        tags::list(state.db.pool())
            .await?
            .into_iter()
            .filter(|t| series_tags.contains(&t.id))
            .map(|t| t.label)
            .collect()
    } else {
        Vec::new()
    };
    let add_magnet = |url: String| {
        let settings = settings.clone();
        let tag_labels = tag_labels.clone();
        let clients = state.downloads.clients.clone();
        let recent = cached.recent;
        async move {
            let hash = magnet_info_hash(&url).ok_or_else(|| {
                DownloadError::Client("the magnet link has no BitTorrent v1 info hash".into())
            })?;
            clients
                .add(
                    &settings,
                    &AddRequest {
                        source: Source::Magnet(&url),
                        hash: &hash,
                        recent,
                        tags: &tag_labels,
                    },
                )
                .await
        }
    };
    // Tanukarr's fake release protection: the title, then the torrent's file list, which is
    // fetched first whenever there is a torrent file (even for clients that prefer magnets).
    let options: kitsunarr_domain::api::settings::IndexerOptionsSettings =
        kitsunarr_db::settings::get_or_default(
            state.db.pool(),
            kitsunarr_db::settings::KEY_INDEXER_OPTIONS,
        )
        .await?;
    if options.enable_fake_release_protection {
        if kitsunarr_downloaders::fake::dangerous_title(&release.title) {
            return Err(failed(
                "Fake release: the release title ends in an executable extension.".into(),
            ));
        }
        if let Some(url) = torrent_url.clone() {
            match state
                .indexers
                .download(indexer_id, &url, "application/x-bittorrent")
                .await
            {
                Ok(Downloaded::File(bytes)) => {
                    match kitsunarr_downloaders::fake::torrent_files(&bytes) {
                        Ok(files) => {
                            if let Some(fake) = kitsunarr_downloaders::fake::inspect(&files) {
                                tracing::warn!(release = %release.title, reason = ?fake.reason, "fake release rejected");
                                return Err(failed(format!("Fake release: {}.", fake.message)));
                            }
                        }
                        Err(e) => {
                            tracing::warn!(release = %release.title, error = %e, "torrent contents unreadable; not inspected");
                        }
                    }
                    let hash = torrent_info_hash(&bytes).map_err(|e| {
                        failed(format!(
                            "The indexer did not return a valid torrent file ({e})."
                        ))
                    })?;
                    let name = format!("{}.torrent", clean_file_name(&release.title));
                    let id = state
                        .downloads
                        .clients
                        .add(
                            &settings,
                            &AddRequest {
                                source: Source::File {
                                    name: &name,
                                    content: &bytes,
                                },
                                hash: &hash,
                                recent: cached.recent,
                                tags: &tag_labels,
                            },
                        )
                        .await
                        .map_err(|e| failed(describe(&e)))?;
                    return finish(state, cached, &client, id).await;
                }
                Ok(Downloaded::Magnet(m)) => {
                    tracing::warn!(release = %release.title, "the torrent link redirects to a magnet; contents not inspected");
                    let id = add_magnet(m).await.map_err(|e| failed(describe(&e)))?;
                    return finish(state, cached, &client, id).await;
                }
                Err(e) if magnet.is_some() => {
                    tracing::warn!(release = %release.title, error = %e, "the torrent file could not be fetched; the magnet is used, not inspected");
                }
                Err(e) => {
                    return Err(failed(format!("Downloading the torrent failed: {e}")));
                }
            }
        } else if magnet.is_some() {
            tracing::warn!(release = %release.title, "magnet-only release; contents not inspected");
        }
    }

    let mut result = Err(DownloadError::Client(
        "the release has no download link".into(),
    ));
    if let Some(m) = magnet {
        result = add_magnet(m).await;
        // A client that cannot take this magnet falls back to the torrent file, if any.
        let fall_back =
            matches!(result, Err(DownloadError::Unsupported(_))) && torrent_url.is_some();
        if !fall_back {
            let id = result.map_err(|e| failed(describe(&e)))?;
            return finish(state, cached, &client, id).await;
        }
    }
    if let Some(url) = torrent_url {
        let fetched = state
            .indexers
            .download(indexer_id, &url, "application/x-bittorrent")
            .await
            .map_err(|e| failed(format!("Downloading the torrent failed: {e}")))?;
        result = match fetched {
            Downloaded::Magnet(m) => add_magnet(m).await,
            Downloaded::File(bytes) => {
                let hash = torrent_info_hash(&bytes).map_err(|e| {
                    failed(format!(
                        "The indexer did not return a valid torrent file ({e})."
                    ))
                })?;
                let name = format!("{}.torrent", clean_file_name(&release.title));
                state
                    .downloads
                    .clients
                    .add(
                        &settings,
                        &AddRequest {
                            source: Source::File {
                                name: &name,
                                content: &bytes,
                            },
                            hash: &hash,
                            recent: cached.recent,
                            tags: &tag_labels,
                        },
                    )
                    .await
            }
        };
    }
    let id = result.map_err(|e| failed(describe(&e)))?;
    finish(state, cached, &client, id).await
}

/// `FileNameBuilder.CleanFileName`, simplified: characters that are invalid in file names.
fn clean_file_name(title: &str) -> String {
    title
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect()
}

/// Records the grab in history (one entry per episode, as upstream).
async fn finish(
    state: &AppState,
    cached: &CachedRelease,
    client: &ProviderRecord,
    download_id: String,
) -> ApiResult<GrabResult> {
    let r = &cached.release;
    let data = serde_json::json!({
        "indexer": r.indexer,
        "releaseGroup": cached.release_group,
        "releaseType": cached.release_type,
        "size": r.release.size.to_string(),
        "guid": r.release.guid,
        "protocol": r.release.protocol,
        "publishedDate": r.release.publish_date.to_rfc3339(),
        "downloadClient": client.implementation,
        "downloadClientName": client.name,
        "customFormatScore": cached.custom_format_score.to_string(),
        "indexerFlags": r.release.indexer_flags.to_string(),
        "nzbInfoUrl": r.release.info_url,
        "tvdbId": r.release.tvdb_id.to_string(),
        "releaseSource": cached.release_source,
        "torrentInfoHash": r.release.info_hash,
    });
    let mut conn = state
        .db
        .pool()
        .acquire()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for episode_id in &cached.episode_ids {
        history::insert(
            &mut conn,
            &NewHistory {
                event_type: "grabbed".into(),
                date: now.clone(),
                source_title: r.release.title.clone(),
                series_id: Some(cached.series_id),
                episode_id: Some(*episode_id),
                movie_id: None,
                quality_id: Some(cached.quality.quality_id),
                revision: serde_json::to_value(cached.quality.revision).ok(),
                languages: cached.languages.clone(),
                custom_format_score: Some(cached.custom_format_score),
                download_id: Some(download_id.clone()),
                data: data.clone(),
            },
        )
        .await?;
    }
    state.downloads.note_grabbed(&download_id);
    notify_grab(state, cached, client, &download_id).await;
    tracing::info!(release = %r.release.title, client = %client.name, %download_id, "release grabbed");
    Ok(GrabResult {
        download_client: client.name.clone(),
        message: format!("Sent \"{}\" to {}.", r.release.title, client.name),
        download_id,
    })
}

/// `OnGrab` notifications.
async fn notify_grab(
    state: &AppState,
    cached: &CachedRelease,
    client: &ProviderRecord,
    download_id: &str,
) {
    use kitsunarr_library::notify;
    let Some(series) = notify::series_ref(&state.db, cached.series_id).await else {
        return;
    };
    let episodes = notify::episode_refs(&state.db, cached.series_id, &cached.episode_ids).await;
    let r = &cached.release;
    let event = kitsunarr_notifications::Event::Grab(Box::new(kitsunarr_notifications::Grab {
        series,
        episodes,
        release: kitsunarr_notifications::ReleaseRef {
            quality: notify::quality_name(i64::from(cached.quality.quality_id)),
            quality_version: cached.quality.revision.version,
            release_group: cached.release_group.clone(),
            release_title: r.release.title.clone(),
            indexer: r.indexer.clone(),
            size: r.release.size,
            custom_format_score: cached.custom_format_score,
            custom_formats: Vec::new(),
            languages: cached
                .languages
                .iter()
                .filter_map(|l| notify::language(*l))
                .collect(),
            indexer_flags: Vec::new(),
        },
        download_client: client.name.clone(),
        download_client_type: client.implementation.clone(),
        download_id: download_id.to_string(),
    }));
    notify::spawn(&state.db, &state.downloads.notifier, event);
}

/// Send a test notification with a connection's settings (stored secrets are reused).
#[utoipa::path(post, path = "/providers/notifications/test", tag = "settings", request_body = IndexerTestRequest,
    responses(
        (status = 200, body = DownloadClientTestResult, description = "Test outcome (ok or not)"),
        (status = 422, body = ApiErrorBody)
    ))]
pub async fn test_notification(
    State(state): State<AppState>,
    Json(mut req): Json<IndexerTestRequest>,
) -> ApiResult<Json<DownloadClientTestResult>> {
    if !kitsunarr_notifications::supported(&req.implementation) {
        return Err(ApiError::field(
            "implementation",
            format!(
                "{} notifications are not supported yet.",
                req.implementation
            ),
        ));
    }
    if let Some(id) = req.id {
        let stored = providers::get(state.db.pool(), ProviderKind::Notification, id).await?;
        crate::routes::settings::merge_secrets(&mut req.settings, &stored.settings);
    }
    let connection = kitsunarr_notifications::Connection {
        id: req.id.unwrap_or(0),
        name: req.implementation.clone(),
        implementation: req.implementation.clone(),
        triggers: Default::default(),
        tags: Vec::new(),
        settings: req.settings,
    };
    let instance = kitsunarr_library::notify::instance(&state.db).await;
    Ok(Json(
        match state
            .downloads
            .notifier
            .send(
                &connection,
                &kitsunarr_notifications::Event::Test,
                &instance,
            )
            .await
        {
            Ok(()) => DownloadClientTestResult {
                ok: true,
                message: "Test notification sent.".into(),
                warnings: Vec::new(),
            },
            Err(e) => DownloadClientTestResult {
                ok: false,
                message: format!("Sending failed: {e}."),
                warnings: Vec::new(),
            },
        },
    ))
}

/// Jellyfin's libraries or users for a connection's settings (stored secrets are reused).
#[utoipa::path(post, path = "/providers/notifications/choices/{kind}", tag = "settings",
    params(("kind" = String, Path, description = "libraries | users")),
    request_body = IndexerTestRequest,
    responses((status = 200, body = Vec<kitsunarr_domain::api::settings::ProviderChoice>), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn notification_choices(
    State(state): State<AppState>,
    axum::extract::Path(kind): axum::extract::Path<String>,
    Json(mut req): Json<IndexerTestRequest>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::settings::ProviderChoice>>> {
    if !req.implementation.eq_ignore_ascii_case("Jellyfin") {
        return Err(ApiError::field(
            "implementation",
            "Only Jellyfin connections offer choices.",
        ));
    }
    if let Some(id) = req.id {
        let stored = providers::get(state.db.pool(), ProviderKind::Notification, id).await?;
        crate::routes::settings::merge_secrets(&mut req.settings, &stored.settings);
    }
    let list = kitsunarr_library::jellyfin::choices(&state.http, &req.settings, &kind)
        .await
        .map_err(|m| ApiError::field("host", m))?;
    Ok(Json(
        list.into_iter()
            .map(|c| kitsunarr_domain::api::settings::ProviderChoice {
                value: c.value,
                name: c.name,
                hint: c.hint,
            })
            .collect(),
    ))
}
