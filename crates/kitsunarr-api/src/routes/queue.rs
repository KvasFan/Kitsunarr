//! The download queue (Sonarr's `QueueController`, `TrackedDownloadService`), read live from
//! the download clients.

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use kitsunarr_db::history;
use kitsunarr_db::library::series;
use kitsunarr_db::providers;
use kitsunarr_db::rules::{self, RemotePathMapping};
use kitsunarr_domain::Quality;
use kitsunarr_domain::api::queue::{QueueClientFailure, QueueItem, QueueResult};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_downloaders::{ItemStatus, qbittorrent};

use crate::AppState;
use crate::error::ApiResult;

/// `RemotePathMappingService.RemapRemoteToLocal`: the first mapping for the client's host whose
/// remote path contains the path.
pub fn remap(mappings: &[RemotePathMapping], host: &str, path: &str) -> String {
    let normal = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_string();
    let target = normal(path);
    for m in mappings {
        if !m.host.eq_ignore_ascii_case(host) {
            continue;
        }
        let remote = normal(&m.remote_path);
        let rest = if target == remote {
            Some("")
        } else {
            target
                .strip_prefix(&format!("{remote}/"))
                .filter(|_| !remote.is_empty())
        };
        if let Some(rest) = rest {
            let local = m.local_path.trim_end_matches(['/', '\\']);
            return if rest.is_empty() {
                local.to_string()
            } else {
                format!("{local}/{rest}")
            };
        }
    }
    path.to_string()
}

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
    let mut items = Vec::new();
    let mut failures = Vec::new();
    for client in &clients {
        let settings = match qbittorrent::Settings::from_map(&client.settings) {
            Ok(s) => s,
            Err(m) => {
                failures.push(QueueClientFailure {
                    download_client: client.name.clone(),
                    message: format!("Invalid settings: {m}"),
                });
                continue;
            }
        };
        match state.qbittorrent.items(&settings).await {
            Ok(list) => items.extend(list.into_iter().map(|i| (client, settings.host.clone(), i))),
            Err(e) => failures.push(QueueClientFailure {
                download_client: client.name.clone(),
                message: e.to_string(),
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
        let tracked_state = if grab.is_none() {
            messages.push(
                "Not grabbed by Kitsunarr: it was added to the download client another way, so Kitsunarr does not know which episodes it contains.".into(),
            );
            "unknown"
        } else if item.status == ItemStatus::Completed {
            messages.push(
                "Downloaded. Importing completed downloads into the library is not implemented yet.".into(),
            );
            "importPending"
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
    Ok(Json(QueueResult {
        items: out,
        failures,
        clients: clients.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_are_remapped_by_host_and_prefix() {
        let m = vec![RemotePathMapping {
            id: 1,
            host: "qbit".into(),
            remote_path: "/downloads/".into(),
            local_path: "/mnt/media/downloads/".into(),
        }];
        assert_eq!(
            remap(&m, "QBIT", "/downloads/tv/Show.S01E01"),
            "/mnt/media/downloads/tv/Show.S01E01"
        );
        assert_eq!(remap(&m, "qbit", "/downloads"), "/mnt/media/downloads");
        assert_eq!(remap(&m, "other", "/downloads/x"), "/downloads/x");
        // A prefix that is not a whole path segment does not match.
        assert_eq!(remap(&m, "qbit", "/downloads2/x"), "/downloads2/x");
    }
}
