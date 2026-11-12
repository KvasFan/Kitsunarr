//! The download queue (Sonarr's `QueueController`, `TrackedDownloadService`), read live from
//! the download clients.

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use kitsunarr_db::library::series;
use kitsunarr_db::providers;
use kitsunarr_db::rules;
use kitsunarr_db::{blocklist, history};
use kitsunarr_domain::Quality;
use kitsunarr_domain::api::queue::{
    BlocklistItem, BlocklistRemoval, QueueClientFailure, QueueItem, QueueRemoval,
    QueueRemovalResult, QueueResult,
};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_downloaders::{ClientSettings, ItemStatus};
use kitsunarr_library::downloads::remap;

use crate::AppState;
use crate::error::{ApiError, ApiResult};

fn status_name(s: ItemStatus) -> &'static str {
    match s {
        ItemStatus::Queued => "queued",
        ItemStatus::Paused => "paused",
        ItemStatus::Downloading => "downloading",
        ItemStatus::Completed => "completed",
        ItemStatus::Failed => "failed",
        ItemStatus::Warning => "warning",
    }
}

/// The downloads of every enabled download client Kitsunarr can read.
#[utoipa::path(get, path = "/queue", tag = "queue", responses((status = 200, body = QueueResult)))]
pub async fn get_queue(State(state): State<AppState>) -> ApiResult<Json<QueueResult>> {
    let clients: Vec<_> = providers::list(state.db.pool(), ProviderKind::DownloadClient)
        .await?
        .into_iter()
        .filter(|c| c.enabled && kitsunarr_downloaders::supported(&c.implementation))
        .collect();
    let mappings = rules::list_remote_path_mappings(state.db.pool()).await?;
    // Clients are read in parallel, each for at most 15 seconds, so one unreachable client
    // does not hold up the others.
    let reads = clients.iter().map(|client| {
        let state = state.clone();
        async move {
            let settings = ClientSettings::from_provider(&client.implementation, &client.settings)
                .map_err(|m| format!("Invalid settings: {m}"))?;
            let list = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                state.downloads.clients.items(&settings),
            )
            .await
            .map_err(|_| "The download client did not answer within 15 seconds.".to_string())?
            .map_err(|e| e.to_string())?;
            Ok::<_, String>((settings.host().to_string(), list))
        }
    });
    let mut items = Vec::new();
    let mut failures = Vec::new();
    for (client, read) in clients.iter().zip(futures::future::join_all(reads).await) {
        match read {
            Ok((host, list)) => items.extend(list.into_iter().map(|i| (client, host.clone(), i))),
            Err(message) => failures.push(QueueClientFailure {
                download_client: client.name.clone(),
                message,
            }),
        }
    }

    let ids: Vec<String> = items
        .iter()
        .map(|(_, _, i)| i.download_id.clone())
        .collect();
    let grabs = history::grabs_for(&state.db, &ids).await?;
    let mut series_titles: HashMap<i64, String> = HashMap::new();
    let mut episode_labels: HashMap<i64, HashMap<i64, String>> = HashMap::new();
    let mut out = Vec::new();
    for (client, host, item) in items {
        let mine: Vec<&history::GrabRecord> = grabs
            .iter()
            .filter(|g| g.download_id.eq_ignore_ascii_case(&item.download_id))
            .collect();
        let series_id = mine.iter().find_map(|g| g.series_id);
        let movie_id = mine.iter().find_map(|g| g.movie_id);
        let movie_title = match movie_id {
            Some(id) => kitsunarr_db::library::movies::get_detail(&state.db, id)
                .await
                .ok()
                .map(|m| m.item.title),
            None => None,
        };
        let mut series_title = None;
        let mut episodes = Vec::new();
        if let Some(id) = series_id {
            if !series_titles.contains_key(&id)
                && let Ok(s) = series::get_list_item(&state.db, id).await
            {
                series_titles.insert(id, s.title);
            }
            series_title = series_titles.get(&id).cloned();
            if let std::collections::hash_map::Entry::Vacant(slot) = episode_labels.entry(id) {
                let labels = series::list_episodes(&state.db, id, None)
                    .await
                    .map(|list| {
                        list.into_iter()
                            .map(|e| {
                                (
                                    e.id,
                                    format!("S{:02}E{:02}", e.season_number, e.episode_number),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                slot.insert(labels);
            }
            let labels = &episode_labels[&id];
            episodes = mine
                .iter()
                .filter_map(|g| g.episode_id.and_then(|e| labels.get(&e).cloned()))
                .collect();
        }
        let grab = mine.first();
        let data: serde_json::Value = grab
            .and_then(|g| serde_json::from_str(&g.data).ok())
            .unwrap_or_default();
        let mut messages: Vec<String> = item.message.clone().into_iter().collect();
        let tracked = state.downloads.tracked(&item.download_id);
        let tracked_state = if grab.is_none() {
            messages.push(
                "Not grabbed by Kitsunarr: it was added to the download client another way, so Kitsunarr does not know which episodes it contains.".into(),
            );
            "unknown"
        } else if let Some(t) = tracked
            .clone()
            .filter(|t| t.state == kitsunarr_library::downloads::State::Failed)
        {
            messages.extend(t.messages);
            "failed"
        } else if item.status == ItemStatus::Completed {
            match tracked {
                Some(t) => {
                    messages.extend(t.messages);
                    t.state.as_str()
                }
                None => {
                    messages.push(
                        "Downloaded; it is imported at the next check (every minute).".into(),
                    );
                    "importPending"
                }
            }
        } else {
            "downloading"
        };
        out.push(QueueItem {
            download_id: item.download_id.clone(),
            title: item.title.clone(),
            download_client: client.name.clone(),
            protocol: client.protocol,
            series_id,
            series_title,
            movie_id,
            movie_title,
            episodes,
            quality: grab
                .and_then(|g| g.quality_id)
                .and_then(|q| Quality::by_id(q as i32))
                .map(|q| q.name.to_string()),
            indexer: data
                .get("indexer")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            size: item.total_size,
            size_left: item.remaining_size,
            time_left: item.remaining_seconds,
            status: status_name(item.status).into(),
            tracked_state: tracked_state.into(),
            messages,
            output_path: item
                .output_path
                .as_deref()
                .map(|p| remap(&mappings, &host, p)),
            added: grab.map(|g| g.date.clone()),
        });
    }
    // Releases a delay profile holds back (Sonarr shows them in the queue as "Delay").
    for p in kitsunarr_db::pending::list(state.db.pool()).await? {
        let Ok(stored) = serde_json::from_str::<crate::rss::StoredRelease>(&p.release) else {
            continue;
        };
        let r = stored.release();
        let episode_ids: Vec<i64> = serde_json::from_str(&p.episode_ids).unwrap_or_default();
        let labels = series::list_episodes(&state.db, p.series_id, None)
            .await
            .map(|list| {
                list.into_iter()
                    .filter(|e| episode_ids.contains(&e.id))
                    .map(|e| format!("S{:02}E{:02}", e.season_number, e.episode_number))
                    .collect()
            })
            .unwrap_or_default();
        out.push(QueueItem {
            download_id: format!("pending-{}", p.id),
            title: p.title,
            download_client: String::new(),
            protocol: r.release.protocol,
            series_id: Some(p.series_id),
            series_title: series::get_list_item(&state.db, p.series_id)
                .await
                .ok()
                .map(|s| s.title),
            movie_id: None,
            movie_title: None,
            episodes: labels,
            quality: Some(
                kitsunarr_parser::parse_quality(&r.release.title)
                    .quality
                    .name
                    .to_string(),
            ),
            indexer: Some(r.indexer),
            size: r.release.size,
            size_left: r.release.size,
            time_left: None,
            status: "delay".into(),
            tracked_state: "pending".into(),
            messages: vec![
                "Held back by a delay profile: an RSS sync grabs it once the delay is over.".into(),
            ],
            output_path: None,
            added: Some(p.added_at),
        });
    }
    Ok(Json(QueueResult {
        items: out,
        failures,
        clients: clients.len(),
    }))
}

/// Runs one completed-download check now (`RefreshMonitoredDownloads`): imports downloads that
/// finished. Returns the summary.
#[utoipa::path(post, path = "/queue/refresh", tag = "queue", responses((status = 200, body = String)))]
pub async fn refresh_queue(State(state): State<AppState>) -> ApiResult<Json<String>> {
    crate::downloads::refresh(&state)
        .await
        .map(Json)
        .map_err(crate::error::ApiError::Internal)
}

/// Finds a download in the enabled clients Kitsunarr can read: its client record, settings and
/// item.
async fn find_download(
    state: &AppState,
    download_id: &str,
) -> ApiResult<
    Option<(
        providers::ProviderRecord,
        ClientSettings,
        kitsunarr_downloaders::DownloadItem,
    )>,
> {
    let clients: Vec<_> = providers::list(state.db.pool(), ProviderKind::DownloadClient)
        .await?
        .into_iter()
        .filter(|c| c.enabled && kitsunarr_downloaders::supported(&c.implementation))
        .collect();
    for client in clients {
        let Ok(settings) = ClientSettings::from_provider(&client.implementation, &client.settings)
        else {
            continue;
        };
        let Ok(Ok(items)) = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            state.downloads.clients.items(&settings),
        )
        .await
        else {
            continue;
        };
        if let Some(item) = items
            .into_iter()
            .find(|i| i.download_id.eq_ignore_ascii_case(download_id))
        {
            return Ok(Some((client, settings, item)));
        }
    }
    Ok(None)
}

/// Remove a download from the queue (Sonarr's `QueueController.Remove`): optionally remove it
/// (and its data) from the download client, and optionally blocklist the release, which marks
/// the download as failed and, unless skipped, searches for a replacement.
#[utoipa::path(delete, path = "/queue/{download_id}", tag = "queue",
    params(("download_id" = String, Path), QueueRemoval),
    responses(
        (status = 200, body = QueueRemovalResult),
        (status = 404, body = kitsunarr_domain::api::error::ApiErrorBody),
        (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)
    ))]
pub async fn remove_from_queue(
    State(state): State<AppState>,
    axum::extract::Path(download_id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<QueueRemoval>,
) -> ApiResult<Json<QueueRemovalResult>> {
    // A pending release: forgotten (it is grabbed only if a later search finds it again).
    if let Some(id) = download_id
        .strip_prefix("pending-")
        .and_then(|i| i.parse::<i64>().ok())
    {
        let pending = kitsunarr_db::pending::list(state.db.pool()).await?;
        let p = pending
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ApiError::NotFound("The pending release does not exist.".into()))?;
        kitsunarr_db::pending::remove(state.db.pool(), p.indexer_id, &p.guid).await?;
        return Ok(Json(QueueRemovalResult {
            message: format!("{} is no longer pending.", p.title),
        }));
    }
    let found = find_download(&state, &download_id).await?;
    if found.is_none() && !q.blocklist {
        return Err(ApiError::NotFound(
            "The download is not in any download client Kitsunarr can read.".into(),
        ));
    }
    let mut done = Vec::new();
    if q.blocklist {
        let grabs = history::grabs_for(&state.db, std::slice::from_ref(&download_id)).await?;
        if grabs
            .iter()
            .all(|g| g.series_id.is_none() && g.movie_id.is_none())
        {
            return Err(ApiError::field(
                "blocklist",
                "Kitsunarr did not grab this download, so it cannot blocklist the release.",
            ));
        }
        let torrent = found
            .as_ref()
            .is_some_and(|(_, settings, _)| !settings.is_usenet());
        let redownload = kitsunarr_library::failed::mark_failed(
            &state.db,
            &grabs,
            &kitsunarr_library::failed::Failure {
                message: "Manually marked as failed",
                source: None,
                torrent_hash: torrent.then_some(download_id.as_str()),
                skip_redownload: q.skip_redownload,
            },
        )
        .await?;
        state
            .downloads
            .note_failed(&download_id, "Manually marked as failed");
        done.push("blocklisted the release".to_string());
        if let Some(r) = redownload {
            state.downloads.push_redownload(r);
            crate::downloads::run_redownloads(&state).await;
            done.push("searching for a replacement".into());
        }
    }
    if q.remove_from_client
        && let Some((client, settings, item)) = &found
    {
        state
            .downloads
            .clients
            .remove(settings, item, true)
            .await
            .map_err(|e| ApiError::field("removeFromClient", format!("{}: {e}", client.name)))?;
        state.downloads.forget(&download_id);
        done.push(format!("removed it from {}", client.name));
    }
    let title = found
        .as_ref()
        .map(|(_, _, i)| i.title.clone())
        .unwrap_or_else(|| download_id.clone());
    Ok(Json(QueueRemovalResult {
        message: if done.is_empty() {
            format!("Nothing to do for \"{title}\".")
        } else {
            let mut m = format!("\"{title}\": {}.", done.join(", "));
            if let Some(f) = m.get_mut(..1) {
                f.make_ascii_uppercase();
            }
            m
        },
    }))
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct BlocklistQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub sort_direction: Option<kitsunarr_domain::SortDirection>,
}

/// Blocklisted releases, newest first.
#[utoipa::path(get, path = "/blocklist", tag = "queue", params(BlocklistQuery),
    responses((status = 200, body = kitsunarr_domain::Page<BlocklistItem>)))]
pub async fn list_blocklist(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<BlocklistQuery>,
) -> ApiResult<Json<kitsunarr_domain::Page<BlocklistItem>>> {
    let paging = kitsunarr_domain::PageRequest {
        page: q.page,
        page_size: q.page_size,
        sort_direction: q.sort_direction,
        ..Default::default()
    };
    Ok(Json(blocklist::list(&state.db, &paging).await?))
}

/// Remove one release from the blocklist: it may be grabbed again.
#[utoipa::path(delete, path = "/blocklist/{id}", tag = "queue", params(("id" = i64, Path)),
    responses((status = 204), (status = 404, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn delete_blocklist(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> ApiResult<axum::http::StatusCode> {
    if blocklist::delete(&state.db, &[id]).await? == 0 {
        return Err(ApiError::NotFound("No such blocklist entry.".into()));
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Remove several releases from the blocklist (`ids`), or every one (`all`).
#[utoipa::path(post, path = "/blocklist/remove", tag = "queue", request_body = BlocklistRemoval,
    responses((status = 200, body = u64)))]
pub async fn remove_blocklist(
    State(state): State<AppState>,
    Json(req): Json<BlocklistRemoval>,
) -> ApiResult<Json<u64>> {
    Ok(Json(if req.all {
        blocklist::clear(&state.db).await?
    } else {
        blocklist::delete(&state.db, &req.ids).await?
    }))
}

/// Mark a grab as failed (Sonarr's `HistoryController.MarkAsFailed`): blocklists the release
/// and, unless skipped, searches again.
#[utoipa::path(post, path = "/history/{id}/failed", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = QueueRemovalResult), (status = 422, body = kitsunarr_domain::api::error::ApiErrorBody)))]
pub async fn mark_history_failed(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> ApiResult<Json<QueueRemovalResult>> {
    let (event_type, download_id) = history::get(&state.db, id).await?;
    let Some(download_id) = download_id.filter(|d| !d.trim().is_empty()) else {
        return Err(ApiError::field(
            "id",
            "This history entry has no download id, so the download cannot be marked as failed.",
        ));
    };
    if event_type != "grabbed" {
        return Err(ApiError::field("id", "Only grabs can be marked as failed."));
    }
    let grabs = history::grabs_for(&state.db, std::slice::from_ref(&download_id)).await?;
    let torrent = grabs
        .first()
        .and_then(|g| serde_json::from_str::<serde_json::Value>(&g.data).ok())
        .and_then(|d| {
            d.get("protocol")
                .and_then(|p| p.as_str())
                .map(|p| p == "torrent")
        })
        .unwrap_or(false);
    let redownload = kitsunarr_library::failed::mark_failed(
        &state.db,
        &grabs,
        &kitsunarr_library::failed::Failure {
            message: "Manually marked as failed",
            source: None,
            torrent_hash: torrent.then_some(download_id.as_str()),
            skip_redownload: false,
        },
    )
    .await?;
    state
        .downloads
        .note_failed(&download_id, "Manually marked as failed");
    let mut message = "Marked as failed and blocklisted".to_string();
    if let Some(r) = redownload {
        state.downloads.push_redownload(r);
        crate::downloads::run_redownloads(&state).await;
        message.push_str("; searching for a replacement");
    }
    message.push('.');
    Ok(Json(QueueRemovalResult { message }))
}
