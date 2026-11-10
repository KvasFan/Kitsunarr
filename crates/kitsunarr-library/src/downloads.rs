//! Completed download handling: Sonarr's `DownloadMonitoringService` and
//! `CompletedDownloadService`, for the download clients Kitsunarr implements (qBittorrent).
//!
//! Each refresh reads the clients, imports completed downloads Kitsunarr grabbed and has not
//! imported yet, and keeps a per-download state for the queue. Downloads are never removed from
//! the client.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kitsunarr_db::Database;
use kitsunarr_db::history;
use kitsunarr_db::providers;
use kitsunarr_db::rules::{self, RemotePathMapping};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_downloaders::{ItemStatus, QBittorrent, qbittorrent};

use crate::import::{self, Download};

/// Where an import stands (`TrackedDownloadState`, the states Kitsunarr uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    ImportPending,
    ImportBlocked,
    Imported,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImportPending => "importPending",
            Self::ImportBlocked => "importBlocked",
            Self::Imported => "imported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracked {
    pub state: State,
    pub messages: Vec<String>,
}

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

/// Shared by the background job and the queue.
#[derive(Clone, Default)]
pub struct Monitor {
    pub qbittorrent: QBittorrent,
    tracked: Arc<Mutex<HashMap<String, Tracked>>>,
    running: Arc<tokio::sync::Mutex<()>>,
}

impl Monitor {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            qbittorrent: QBittorrent::new(http),
            ..Default::default()
        }
    }

    /// The import state of a download (upper-case id), once a refresh has looked at it.
    pub fn tracked(&self, download_id: &str) -> Option<Tracked> {
        self.tracked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&download_id.to_ascii_uppercase())
            .cloned()
    }

    fn set(&self, id: &str, state: State, messages: Vec<String>) {
        self.tracked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_ascii_uppercase(), Tracked { state, messages });
    }

    /// One `RefreshMonitoredDownloads` pass. Returns a summary for the task history.
    pub async fn refresh(&self, db: &Database) -> Result<String, String> {
        let Ok(_guard) = self.running.try_lock() else {
            return Ok("already running".into());
        };
        let clients: Vec<_> = providers::list(db.pool(), ProviderKind::DownloadClient)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|c| c.enabled && kitsunarr_downloaders::supported(&c.implementation))
            .collect();
        let mappings = rules::list_remote_path_mappings(db.pool())
            .await
            .map_err(|e| e.to_string())?;
        let (mut imported, mut blocked, mut failed_clients) = (0usize, 0usize, Vec::new());
        for client in &clients {
            let Ok(settings) = qbittorrent::Settings::from_map(&client.settings) else {
                failed_clients.push(client.name.clone());
                continue;
            };
            let items = match self.qbittorrent.items(&settings).await {
                Ok(items) => items,
                Err(e) => {
                    tracing::warn!(client = %client.name, error = %e, "cannot read the download client");
                    failed_clients.push(client.name.clone());
                    continue;
                }
            };
            let completed: Vec<_> = items
                .into_iter()
                .filter(|i| i.status == ItemStatus::Completed)
                .collect();
            let ids: Vec<String> = completed.iter().map(|i| i.download_id.clone()).collect();
            let grabs = history::grabs_for(db, &ids)
                .await
                .map_err(|e| e.to_string())?;
            let done = history::imported_downloads(db, &ids)
                .await
                .map_err(|e| e.to_string())?;
            for item in completed {
                if done.contains(&item.download_id) {
                    self.set(&item.download_id, State::Imported, Vec::new());
                    continue;
                }
                let mine: Vec<_> = grabs
                    .iter()
                    .filter(|g| g.download_id.eq_ignore_ascii_case(&item.download_id))
                    .cloned()
                    .collect();
                // Not grabbed by Kitsunarr: left alone (the queue explains why).
                let Some(series_id) = mine.iter().find_map(|g| g.series_id) else {
                    continue;
                };
                let Some(output) = item.output_path.as_deref() else {
                    self.set(
                        &item.download_id,
                        State::ImportBlocked,
                        vec!["Download doesn't contain intermediate path, Skipping.".into()],
                    );
                    blocked += 1;
                    continue;
                };
                let local = PathBuf::from(remap(&mappings, &settings.host, output));
                let outcome = import::import_download(
                    db,
                    &Download {
                        download_id: &item.download_id,
                        title: &item.title,
                        output_path: &local,
                        series_id,
                        grabs: &mine,
                        client_name: &client.name,
                        client_implementation: &client.implementation,
                    },
                )
                .await?;
                if let Some(reason) = outcome.blocked {
                    self.set(&item.download_id, State::ImportBlocked, vec![reason]);
                    blocked += 1;
                    continue;
                }
                if outcome.results.is_empty() {
                    self.set(
                        &item.download_id,
                        State::ImportPending,
                        vec![format!(
                            "No files found are eligible for import in {}",
                            local.display()
                        )],
                    );
                    continue;
                }
                // `VerifyImport`: every grabbed episode (at least one) was imported.
                let expected = mine
                    .iter()
                    .filter_map(|g| g.episode_id)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    .max(1);
                if outcome.imported_episodes() >= expected {
                    if let Err(e) = self
                        .qbittorrent
                        .mark_imported(&settings, &item.download_id)
                        .await
                    {
                        tracing::warn!(error = %e, "could not set the post-import category");
                    }
                    self.set(&item.download_id, State::Imported, Vec::new());
                    imported += 1;
                } else {
                    let mut messages = vec![
                        "One or more episodes expected in this release were not imported or missing from the release".to_string(),
                    ];
                    for r in outcome.results.iter().filter(|r| !r.imported) {
                        let name = r
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        messages.push(format!("{name}: {}", r.errors.join(", ")));
                    }
                    self.set(&item.download_id, State::ImportBlocked, messages);
                    blocked += 1;
                }
            }
        }
        let mut summary = format!("{imported} downloads imported, {blocked} need attention");
        if !failed_clients.is_empty() {
            summary.push_str(&format!("; could not read: {}", failed_clients.join(", ")));
        }
        Ok(summary)
    }
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
        assert_eq!(remap(&m, "qbit", "/downloads2/x"), "/downloads2/x");
    }
}
