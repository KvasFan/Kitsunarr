//! Completed download handling: Sonarr's `DownloadMonitoringService` and
//! `CompletedDownloadService`, for the download clients Kitsunarr implements (qBittorrent).
//!
//! Each refresh reads the clients, imports completed downloads Kitsunarr grabbed and has not
//! imported yet, and keeps a per-download state for the queue. Downloads are never removed from
//! the client.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kitsunarr_db::Database;
use kitsunarr_db::history;
use kitsunarr_db::providers;
use kitsunarr_db::rules::{self, RemotePathMapping};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_downloaders::{ClientSettings, Clients, ItemStatus};

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
    pub clients: Clients,
    tracked: Arc<Mutex<HashMap<String, Tracked>>>,
    /// Downloads (upper-case ids) seen in a client and not imported, plus grabs since the last
    /// refresh: the queue the decisions compare releases with.
    active: Arc<Mutex<HashSet<String>>>,
    running: Arc<tokio::sync::Mutex<()>>,
}

impl Monitor {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            clients: Clients::new(http),
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

    /// Records a grab: the download is in the queue until a refresh sees it imported or gone.
    pub fn note_grabbed(&self, download_id: &str) {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(download_id.to_ascii_uppercase());
    }

    /// The grabbed releases still in the queue for a series (`QueueService.GetQueue`, as
    /// `QueueSpecification` reads it).
    pub async fn queued_releases(
        &self,
        db: &Database,
        series_id: i64,
    ) -> Result<Vec<kitsunarr_decision::QueuedRelease>, kitsunarr_db::DbError> {
        let grabs = history::grabs_for(db, &self.active_ids()).await?;
        Ok(queued_from_grabs(&grabs, series_id))
    }

    /// Every episode in the queue (`GetQueuedEpisodeIds`), whatever its series.
    pub async fn queued_episode_ids(
        &self,
        db: &Database,
    ) -> Result<HashSet<i64>, kitsunarr_db::DbError> {
        Ok(history::grabs_for(db, &self.active_ids())
            .await?
            .into_iter()
            .filter_map(|g| g.episode_id)
            .collect())
    }

    /// Queued downloads (upper-case ids) not known to be imported.
    fn active_ids(&self) -> Vec<String> {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|id| {
                !matches!(
                    self.tracked(id),
                    Some(Tracked {
                        state: State::Imported,
                        ..
                    })
                )
            })
            .cloned()
            .collect()
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
        let mut seen = HashSet::new();
        for client in &clients {
            let Ok(settings) =
                ClientSettings::from_provider(&client.implementation, &client.settings)
            else {
                failed_clients.push(client.name.clone());
                continue;
            };
            let items = match self.clients.items(&settings).await {
                Ok(items) => items,
                Err(e) => {
                    tracing::warn!(client = %client.name, error = %e, "cannot read the download client");
                    failed_clients.push(client.name.clone());
                    continue;
                }
            };
            seen.extend(items.iter().map(|i| i.download_id.to_ascii_uppercase()));
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
                let local = PathBuf::from(remap(&mappings, settings.host(), output));
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
                        .clients
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
        {
            let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
            if failed_clients.is_empty() {
                // Downloads gone from the clients leave the queue.
                active.retain(|id| seen.contains(id));
            }
            active.extend(seen);
            let tracked = self.tracked.lock().unwrap_or_else(|e| e.into_inner());
            active.retain(|id| {
                !matches!(
                    tracked.get(id),
                    Some(Tracked {
                        state: State::Imported,
                        ..
                    })
                )
            });
        }
        let mut summary = format!("{imported} downloads imported, {blocked} need attention");
        if !failed_clients.is_empty() {
            summary.push_str(&format!("; could not read: {}", failed_clients.join(", ")));
        }
        Ok(summary)
    }
}

/// One queued release per download of the series, from its grab history.
pub fn queued_from_grabs(
    grabs: &[history::GrabRecord],
    series_id: i64,
) -> Vec<kitsunarr_decision::QueuedRelease> {
    let mut out: Vec<(String, kitsunarr_decision::QueuedRelease)> = Vec::new();
    for g in grabs.iter().filter(|g| g.series_id == Some(series_id)) {
        let key = g.download_id.to_ascii_uppercase();
        if let Some((_, q)) = out.iter_mut().find(|(k, _)| *k == key) {
            if let Some(e) = g.episode_id
                && !q.episode_ids.contains(&e)
            {
                q.episode_ids.push(e);
            }
            continue;
        }
        let data: serde_json::Value = serde_json::from_str(&g.data).unwrap_or_default();
        let text = |k: &str| {
            data.get(k).and_then(|v| match v {
                serde_json::Value::String(s) => s.parse::<i64>().ok(),
                other => other.as_i64(),
            })
        };
        let languages: Vec<i32> = g
            .languages
            .as_deref()
            .and_then(|l| serde_json::from_str(l).ok())
            .unwrap_or_default();
        out.push((
            key,
            kitsunarr_decision::QueuedRelease {
                series_id,
                episode_ids: g.episode_id.into_iter().collect(),
                title: g.source_title.clone(),
                quality: kitsunarr_domain::quality::QualityModel {
                    quality_id: g.quality_id.unwrap_or(0) as i32,
                    revision: g
                        .revision
                        .as_deref()
                        .and_then(|r| serde_json::from_str(r).ok())
                        .unwrap_or_default(),
                },
                size: text("size").unwrap_or(0),
                languages: languages
                    .into_iter()
                    .filter_map(kitsunarr_domain::Language::by_id)
                    .collect(),
                indexer_flags: text("indexerFlags").unwrap_or(0) as u32,
            },
        ));
    }
    out.into_iter().map(|(_, q)| q).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grabs_become_one_queued_release_per_download() {
        let grab = |id: &str, episode: i64| history::GrabRecord {
            download_id: id.into(),
            date: "2026-11-10T10:00:00Z".into(),
            source_title: "Show.S01E01E02.720p.HDTV.x264-GRP".into(),
            series_id: Some(3),
            episode_id: Some(episode),
            quality_id: Some(4),
            revision: Some(r#"{"version":2,"real":0,"isRepack":false}"#.into()),
            languages: Some("[1]".into()),
            data: r#"{"size":"1500","indexerFlags":"8"}"#.into(),
        };
        let mut other = grab("ffff", 9);
        other.series_id = Some(4);
        let q = queued_from_grabs(&[grab("abcd", 1), grab("ABCD", 2), other], 3);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].episode_ids, vec![1, 2]);
        assert_eq!(q[0].quality.quality_id, 4);
        assert_eq!(q[0].quality.revision.version, 2);
        assert_eq!(q[0].size, 1500);
        assert_eq!(q[0].indexer_flags, 8);
        assert_eq!(q[0].languages.len(), 1);
    }

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
