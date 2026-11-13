//! Completed and failed download handling, as the background job and the queue run it: the
//! monitor's refresh, then the searches failed downloads ask for
//! (`RedownloadFailedDownloadService`).

use kitsunarr_db::library::{series, wanted};
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::settings::DownloadClientOptionsSettings;
use kitsunarr_library::failed::Redownload;

use crate::AppState;

/// `RefreshMonitoredDownloads`, then the searches for failed downloads (in the background).
pub async fn refresh(state: &AppState) -> Result<String, String> {
    let summary = state.downloads.refresh(&state.db).await?;
    publish_imports(state);
    run_redownloads(state).await;
    Ok(summary)
}

/// Live updates for what the refresh imported, and the queue.
pub(crate) fn publish_imports(state: &AppState) {
    let imports = state.downloads.take_imports();
    let failures = state.downloads.take_import_failures();
    crate::health::note_imports(state, &failures, !imports.is_empty());
    let mut series: Vec<i64> = Vec::new();
    for i in imports {
        if !series.contains(&i.series_id) {
            series.push(i.series_id);
        }
        state.events.publish(ServerEvent::EpisodeFileChanged {
            series_id: i.series_id,
            episode_file_id: i.episode_file_id,
            episode_ids: i.episode_ids,
            deleted: false,
        });
    }
    if !series.is_empty() {
        state.events.publish(ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: series,
            deleted: false,
        });
    }
    state.events.publish(ServerEvent::QueueChanged);
}

/// Starts the searches the monitor collected, as the download client options allow.
pub(crate) async fn run_redownloads(state: &AppState) {
    let pending = state.downloads.take_redownloads();
    if pending.is_empty() {
        return;
    }
    if crate::library_tools::paused(state).await {
        tracing::info!(count = pending.len(), "{}", crate::library_tools::PAUSED);
        return;
    }
    let options: DownloadClientOptionsSettings =
        kv::get_or_default(state.db.pool(), kv::KEY_DOWNLOAD_CLIENT_OPTIONS)
            .await
            .unwrap_or_default();
    for r in pending {
        if !options.auto_redownload_failed {
            tracing::debug!("auto redownloading failed episodes is disabled");
            continue;
        }
        if r.release_source == "InteractiveSearch"
            && !options.auto_redownload_failed_from_interactive_search
        {
            tracing::debug!(
                "auto redownloading failed episodes from interactive search is disabled"
            );
            continue;
        }
        redownload(state, r);
    }
}

/// `RedownloadFailedDownloadService.Handle`: one episode, or every episode of a season, is
/// searched as such; other multi-episode downloads search each episode.
pub(crate) fn redownload(state: &AppState, r: Redownload) {
    let state = state.clone();
    tokio::spawn(async move {
        // A film is searched for again as a whole.
        if let Some(movie) = r.movie_id {
            crate::routes::movie_search::server_search(&state, movie).await;
            return;
        }
        let targets = match wanted::targets(&state.db, &r.episode_ids).await {
            Ok(t) if !t.is_empty() => t,
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(error = %e, "could not load the failed download's episodes");
                return;
            }
        };
        let season = targets[0].season_number;
        let whole_season = targets.len() > 1
            && targets.iter().all(|t| t.season_number == season)
            && series::list_episodes(&state.db, r.series_id, Some(season as i32))
                .await
                .is_ok_and(|all| all.len() == targets.len());
        let groups = if whole_season {
            vec![targets]
        } else {
            targets.into_iter().map(|t| vec![t]).collect()
        };
        let _guard = state.wanted_search.clone().lock_owned().await;
        let summary = crate::routes::wanted::bulk_search(&state, groups, false, false, false).await;
        tracing::info!(series_id = r.series_id, %summary, "searched again after a failed download");
        state.events.publish(ServerEvent::Notice {
            level: "info".into(),
            message: format!("After a failed download: {summary}"),
        });
    });
}
