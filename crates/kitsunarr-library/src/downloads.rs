//! Completed download handling: Sonarr's `DownloadMonitoringService` and
//! `CompletedDownloadService`, for the download clients Kitsunarr implements.
//!
//! Each refresh reads the clients, imports completed downloads Kitsunarr grabbed and has not
//! imported yet, handles failed downloads (`FailedDownloadService`: history, blocklist, a search
//! to run again) and keeps a per-download state for the queue. As `DownloadEventHub` does, a
//! download is removed from its client (with its data) only when the client's "remove
//! completed/failed downloads" option is on and the client is done with it (`CanBeRemoved`:
//! Usenet always, torrents once seeding stopped at the seed limit).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kitsunarr_db::Database;
use kitsunarr_db::history;
use kitsunarr_db::providers;
use kitsunarr_db::rules::{self, RemotePathMapping};
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::settings::{DownloadClientOptionsSettings, ProviderKind};
use kitsunarr_downloaders::{ClientSettings, Clients, DownloadItem, ItemStatus};

use crate::failed::{self, Failure, Redownload};
use crate::import::{self, Download};

/// Where an import stands (`TrackedDownloadState`, the states Kitsunarr uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    ImportPending,
    ImportBlocked,
    Imported,
    Failed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImportPending => "importPending",
            Self::ImportBlocked => "importBlocked",
            Self::Imported => "imported",
            Self::Failed => "failed",
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
    /// Sends grab and import notifications.
    pub notifier: kitsunarr_notifications::Notifier,
    tracked: Arc<Mutex<HashMap<String, Tracked>>>,
    /// Downloads (upper-case ids) seen in a client and not imported, plus grabs since the last
    /// refresh: the queue the decisions compare releases with.
    active: Arc<Mutex<HashSet<String>>>,
    running: Arc<tokio::sync::Mutex<()>>,
    /// Searches to run because downloads failed, for the caller (which owns searching).
    redownloads: Arc<Mutex<Vec<Redownload>>>,
    /// Per download (upper-case id): its last status and remaining size, and when either last
    /// changed (`TrackedDownload.LastProgressDate`, in memory as upstream).
    progress: Arc<Mutex<Progress>>,
    /// Downloads (upper-case ids) already announced as needing manual interaction
    /// (`HasNotifiedManualInteractionRequired`).
    notified_manual: Arc<Mutex<HashSet<String>>>,
}

impl Monitor {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            clients: Clients::new(http.clone()),
            notifier: kitsunarr_notifications::Notifier::new(http),
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

    /// The searches failed downloads asked for since the last call.
    pub fn take_redownloads(&self) -> Vec<Redownload> {
        std::mem::take(&mut *self.redownloads.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Records a search to run again (a download marked as failed by hand).
    pub fn push_redownload(&self, r: Redownload) {
        self.redownloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(r);
    }

    /// Stops tracking a download (removed from its client by hand).
    pub fn forget(&self, download_id: &str) {
        let id = download_id.to_ascii_uppercase();
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        self.tracked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    /// Marks a download as failed (by hand or by failed download handling).
    pub fn note_failed(&self, download_id: &str, message: &str) {
        self.set(download_id, State::Failed, vec![message.to_string()]);
    }

    /// Queued downloads (upper-case ids) not known to be imported or failed.
    fn active_ids(&self) -> Vec<String> {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|id| {
                !matches!(
                    self.tracked(id),
                    Some(Tracked {
                        state: State::Imported | State::Failed,
                        ..
                    })
                )
            })
            .cloned()
            .collect()
    }

    /// Builds and sends the "Download" (or upgrade) notification for an imported file.
    async fn notify_import(
        &self,
        db: &Database,
        series_id: i64,
        r: &import::FileResult,
        f: &import::ImportedFile,
        download_id: &str,
        client: &providers::ProviderRecord,
    ) {
        crate::notify::imported(
            db,
            &self.notifier,
            series_id,
            r,
            f,
            download_id,
            &client.name,
            &client.implementation,
        )
        .await;
    }

    /// `OnDownload` for an imported film.
    #[allow(clippy::too_many_arguments)]
    async fn notify_movie_import(
        &self,
        db: &Database,
        series: kitsunarr_notifications::SeriesRef,
        feature: kitsunarr_notifications::EpisodeRef,
        f: &import::ImportedFile,
        item: &DownloadItem,
        client: &providers::ProviderRecord,
        grabs: &[kitsunarr_db::history::GrabRecord],
    ) {
        type Row = (
            String,
            i64,
            i64,
            Option<String>,
            Option<String>,
            i64,
            String,
        );
        let row: Option<Row> =
            sqlx::query_as(
                "SELECT relative_path, quality_id, revision_version, release_group, scene_name, size, added_at \
                 FROM movie_files WHERE id = ?",
            )
            .bind(f.file_id)
            .fetch_optional(db.pool())
            .await
            .ok()
            .flatten();
        let Some((relative, quality, version, group, scene, size, added)) = row else {
            return;
        };
        let grab = grabs.first();
        let data: serde_json::Value = grab
            .and_then(|g| serde_json::from_str(&g.data).ok())
            .unwrap_or_default();
        let text = |k: &str| data.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let file = kitsunarr_notifications::FileRef {
            id: f.file_id,
            path: format!("{}/{relative}", series.path.trim_end_matches('/')),
            relative_path: relative,
            quality: crate::notify::quality_name(quality),
            quality_version: version as i32,
            release_group: group,
            scene_name: scene,
            size,
            date_added: added,
            languages: Vec::new(),
            source_path: Some(f.source_path.clone()),
            recycle_bin_path: None,
        };
        let deleted_files = f
            .replaced
            .iter()
            .map(|rel| kitsunarr_notifications::FileRef {
                relative_path: rel.clone(),
                path: format!("{}/{rel}", series.path.trim_end_matches('/')),
                ..Default::default()
            })
            .collect();
        let event =
            kitsunarr_notifications::Event::Download(Box::new(kitsunarr_notifications::Download {
                series,
                episodes: vec![feature],
                file,
                release: kitsunarr_notifications::GrabbedRef {
                    release_title: grab.map(|g| g.source_title.clone()),
                    indexer: text("indexer"),
                    size: text("size").and_then(|v| v.parse().ok()),
                    indexer_flags: Vec::new(),
                    release_type: text("releaseType"),
                },
                deleted_files,
                download_client: Some(client.name.clone()),
                download_client_type: Some(client.implementation.clone()),
                download_id: Some(item.download_id.clone()),
            }));
        crate::notify::spawn(db, &self.notifier, event);
    }

    /// Blocks a download's import and, the first time, sends "Manual Interaction Required"
    /// (`ManualInteractionRequiredEvent`).
    #[allow(clippy::too_many_arguments)]
    async fn block(
        &self,
        db: &Database,
        item: &DownloadItem,
        client: &providers::ProviderRecord,
        series_id: i64,
        grabs: &[kitsunarr_db::history::GrabRecord],
        messages: Vec<String>,
    ) {
        self.set(&item.download_id, State::ImportBlocked, messages.clone());
        let first = self
            .notified_manual
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(item.download_id.to_ascii_uppercase());
        if !first {
            return;
        }
        let series = crate::notify::series_ref(db, series_id).await;
        let episode_ids: Vec<i64> = grabs.iter().filter_map(|g| g.episode_id).collect();
        let episodes = crate::notify::episode_refs(db, series_id, &episode_ids).await;
        let grab = grabs.first();
        let data: serde_json::Value = grab
            .and_then(|g| serde_json::from_str(&g.data).ok())
            .unwrap_or_default();
        let text = |k: &str| data.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let revision: kitsunarr_domain::quality::Revision = grab
            .and_then(|g| g.revision.as_deref())
            .and_then(|r| serde_json::from_str(r).ok())
            .unwrap_or_default();
        let event = kitsunarr_notifications::Event::ManualInteractionRequired(Box::new(
            kitsunarr_notifications::ManualInteraction {
                series,
                episodes,
                title: item.title.clone(),
                quality: grab
                    .and_then(|g| g.quality_id)
                    .map_or_else(|| "Unknown".into(), crate::notify::quality_name),
                quality_version: revision.version,
                size: item.total_size,
                download_client: Some(client.name.clone()),
                download_client_type: Some(client.implementation.clone()),
                download_id: item.download_id.clone(),
                messages,
                release: kitsunarr_notifications::GrabbedRef {
                    release_title: grab.map(|g| g.source_title.clone()),
                    indexer: text("indexer"),
                    size: text("size").and_then(|v| v.parse().ok()),
                    indexer_flags: Vec::new(),
                    release_type: text("releaseType"),
                },
            },
        ));
        crate::notify::spawn(db, &self.notifier, event);
    }

    /// `DownloadEventHub.RemoveFromDownloadClient`: removes the download and its data.
    async fn remove_from_client(&self, settings: &ClientSettings, item: &DownloadItem) -> bool {
        match self.clients.remove(settings, item, true).await {
            Ok(()) => {
                tracing::info!(title = %item.title, "removed the download from the download client");
                true
            }
            Err(e) => {
                tracing::warn!(title = %item.title, error = %e, "could not remove the download from the download client");
                false
            }
        }
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
        let options: DownloadClientOptionsSettings =
            kv::get_or_default(db.pool(), kv::KEY_DOWNLOAD_CLIENT_OPTIONS)
                .await
                .map_err(|e| e.to_string())?;
        let trimming =
            kv::get_or_default::<kitsunarr_domain::api::settings::MediaManagementSettings>(
                db.pool(),
                kv::KEY_MEDIA_MANAGEMENT,
            )
            .await
            .map_err(|e| e.to_string())?
            .skip_unmonitored_episodes_from_packs;
        let (mut imported, mut blocked, mut failed_clients) = (0usize, 0usize, Vec::new());
        let mut failed_downloads = 0usize;
        let mut seen = HashSet::new();
        for client in &clients {
            let Ok(settings) =
                ClientSettings::from_provider(&client.implementation, &client.settings)
            else {
                failed_clients.push(client.name.clone());
                continue;
            };
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.clients.items(&settings),
            )
            .await
            .unwrap_or_else(|_| {
                Err(kitsunarr_downloaders::DownloadError::Connection(
                    "no answer within 30 seconds".into(),
                ))
            });
            let items = match read {
                Ok(items) => items,
                Err(e) => {
                    tracing::warn!(client = %client.name, error = %e, "cannot read the download client");
                    failed_clients.push(client.name.clone());
                    continue;
                }
            };
            seen.extend(items.iter().map(|i| i.download_id.to_ascii_uppercase()));
            let option = |key: &str| {
                client
                    .options
                    .get(key)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true)
            };
            let (remove_completed, remove_failed) = (
                option("removeCompletedDownloads"),
                option("removeFailedDownloads"),
            );
            let torrent = !settings.is_usenet();

            // Tanukarr's stalled torrent recovery (`CheckStalled`): a downloading torrent with
            // no progress for the configured time fails.
            let timeout = options.stalled_torrent_timeout.clamp(0, 43_200) as u64 * 60;
            let stalled = stalled_downloads(
                &mut self.progress.lock().unwrap_or_else(|e| e.into_inner()),
                &items,
                if torrent { timeout } else { 0 },
                std::time::Instant::now(),
            );

            // `FailedDownloadService.Check` + `ProcessFailed`.
            let failed_items: Vec<&DownloadItem> = items
                .iter()
                .filter(|i| {
                    i.status == ItemStatus::Failed
                        || stalled.contains(&i.download_id.to_ascii_uppercase())
                })
                .collect();
            if !failed_items.is_empty() {
                let ids: Vec<String> = failed_items.iter().map(|i| i.download_id.clone()).collect();
                let grabs = history::grabs_for(db, &ids)
                    .await
                    .map_err(|e| e.to_string())?;
                let handled = history::failed_downloads(db, &ids)
                    .await
                    .map_err(|e| e.to_string())?;
                for item in failed_items {
                    let message = if item.status != ItemStatus::Failed {
                        "Download stalled with no progress".to_string()
                    } else {
                        item.message
                            .clone()
                            .filter(|m| !m.trim().is_empty())
                            .unwrap_or_else(|| "Failed download detected".into())
                    };
                    if handled.contains(&item.download_id.to_ascii_uppercase()) {
                        self.set(&item.download_id, State::Failed, vec![message]);
                        continue;
                    }
                    let mine: Vec<_> = grabs
                        .iter()
                        .filter(|g| g.download_id.eq_ignore_ascii_case(&item.download_id))
                        .cloned()
                        .collect();
                    if mine
                        .iter()
                        .all(|g| g.series_id.is_none() && g.movie_id.is_none())
                    {
                        continue;
                    }
                    let redownload = failed::mark_failed(
                        db,
                        &mine,
                        &Failure {
                            message: &message,
                            source: Some("Kitsunarr Failed Download Handling"),
                            torrent_hash: torrent.then_some(item.download_id.as_str()),
                            skip_redownload: false,
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    failed_downloads += 1;
                    self.set(&item.download_id, State::Failed, vec![message]);
                    if let Some(r) = redownload {
                        self.push_redownload(r);
                    }
                    if remove_failed && item.can_be_removed {
                        self.remove_from_client(&settings, item).await;
                    }
                }
            }

            if !options.enable_completed_download_handling {
                continue;
            }
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
                // Tanukarr: a multi-season pack keeps its files (copied, never moved) and stays
                // in the client, since only the grabbed seasons are imported from it.
                let multi_season = grabs
                    .iter()
                    .find(|g| g.download_id.eq_ignore_ascii_case(&item.download_id))
                    .and_then(|g| kitsunarr_parser::parse_title(&g.source_title))
                    .is_some_and(|p| p.is_multi_season());
                if done.contains(&item.download_id.to_ascii_uppercase()) {
                    self.set(&item.download_id, State::Imported, Vec::new());
                    // `DownloadCanBeRemovedEvent`: seeding finished after the import.
                    if remove_completed && item.can_be_removed && !multi_season {
                        self.remove_from_client(&settings, &item).await;
                    }
                    continue;
                }
                let mine: Vec<_> = grabs
                    .iter()
                    .filter(|g| g.download_id.eq_ignore_ascii_case(&item.download_id))
                    .cloned()
                    .collect();
                // Tanukarr's films: the download becomes the film's file.
                if let Some(movie_id) = mine.iter().find_map(|g| g.movie_id) {
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
                    let outcome = crate::movie_import::import_movie_download(
                        db,
                        &crate::movie_import::MovieDownload {
                            download_id: &item.download_id,
                            title: &item.title,
                            output_path: &local,
                            movie_id,
                            grabs: &mine,
                            client_name: &client.name,
                            client_implementation: &client.implementation,
                            move_files: settings.is_usenet(),
                        },
                    )
                    .await?;
                    if let Some(reason) = outcome.blocked {
                        self.set(&item.download_id, State::ImportBlocked, vec![reason]);
                        blocked += 1;
                    } else if outcome.results.iter().any(|r| r.imported) {
                        // `OnDownload` with the film as a one-episode series.
                        for r in outcome.results.iter().filter(|r| r.imported) {
                            if let (Some(f), Some((series, feature))) = (
                                &r.imported_file,
                                crate::notify::movie_refs(db, movie_id).await,
                            ) {
                                self.notify_movie_import(
                                    db, series, feature, f, &item, client, &mine,
                                )
                                .await;
                            }
                        }
                        if let Err(e) = self
                            .clients
                            .mark_imported(&settings, &item.download_id)
                            .await
                        {
                            tracing::warn!(error = %e, "could not set the post-import category");
                        }
                        self.set(&item.download_id, State::Imported, Vec::new());
                        imported += 1;
                        if remove_completed && item.can_be_removed {
                            self.remove_from_client(&settings, &item).await;
                        }
                    } else if outcome.results.is_empty() {
                        self.set(
                            &item.download_id,
                            State::ImportPending,
                            vec![format!(
                                "No files found are eligible for import in {}",
                                local.display()
                            )],
                        );
                    } else {
                        let messages = outcome
                            .results
                            .iter()
                            .flat_map(|r| r.errors.clone())
                            .collect();
                        self.set(&item.download_id, State::ImportBlocked, messages);
                        blocked += 1;
                    }
                    continue;
                }
                // Not grabbed by Kitsunarr: left alone (the queue explains why).
                let Some(series_id) = mine.iter().find_map(|g| g.series_id) else {
                    continue;
                };
                let Some(output) = item.output_path.as_deref() else {
                    self.block(
                        db,
                        &item,
                        client,
                        series_id,
                        &mine,
                        vec!["Download doesn't contain intermediate path, Skipping.".into()],
                    )
                    .await;
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
                        move_files: settings.is_usenet() && !multi_season,
                    },
                )
                .await?;
                // `EpisodeImportedEvent` → `OnDownload`, one per imported file.
                for r in outcome.results.iter().filter(|r| r.imported) {
                    if let Some(f) = &r.imported_file {
                        self.notify_import(db, series_id, r, f, &item.download_id, client)
                            .await;
                    }
                }
                if let Some(reason) = outcome.blocked {
                    self.block(db, &item, client, series_id, &mine, vec![reason])
                        .await;
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
                // `VerifyImport`: every grabbed episode (at least one) was imported. With pack
                // trimming on, unmonitored episodes are not expected (Tanukarr would leave the
                // download blocked).
                let mut grabbed: std::collections::HashSet<i64> =
                    mine.iter().filter_map(|g| g.episode_id).collect();
                if trimming {
                    let monitored: std::collections::HashSet<i64> =
                        kitsunarr_db::library::series::list_episodes(db, series_id, None)
                            .await
                            .map_err(|e| e.to_string())?
                            .into_iter()
                            .filter(|e| e.monitored)
                            .map(|e| e.id)
                            .collect();
                    grabbed.retain(|id| monitored.contains(id));
                }
                let expected = grabbed.len().max(1);
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
                    // `DownloadCompletedEvent`.
                    if remove_completed && item.can_be_removed && !multi_season {
                        self.remove_from_client(&settings, &item).await;
                    }
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
                    self.block(db, &item, client, series_id, &mine, messages)
                        .await;
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
                        state: State::Imported | State::Failed,
                        ..
                    })
                )
            });
        }
        let mut summary = format!("{imported} downloads imported, {blocked} need attention");
        if failed_downloads > 0 {
            summary.push_str(&format!(", {failed_downloads} failed"));
        }
        if !failed_clients.is_empty() {
            summary.push_str(&format!("; could not read: {}", failed_clients.join(", ")));
        }
        Ok(summary)
    }
}

/// Progress bookkeeping for stalled torrents: each download's last status and remaining size,
/// and when either last changed.
type Progress = HashMap<String, (ItemStatus, i64, std::time::Instant)>;

/// `CheckStalled`: records each item's progress and returns the downloads (upper-case ids) that
/// are downloading (or warning) with no change for `timeout` seconds (0: never).
fn stalled_downloads(
    progress: &mut Progress,
    items: &[DownloadItem],
    timeout: u64,
    now: std::time::Instant,
) -> HashSet<String> {
    let mut stalled = HashSet::new();
    for i in items {
        let id = i.download_id.to_ascii_uppercase();
        let entry = progress
            .entry(id.clone())
            .or_insert((i.status, i.remaining_size, now));
        if entry.0 != i.status || entry.1 != i.remaining_size {
            *entry = (i.status, i.remaining_size, now);
        }
        if timeout > 0
            && matches!(i.status, ItemStatus::Downloading | ItemStatus::Warning)
            && now.duration_since(entry.2).as_secs() >= timeout
        {
            stalled.insert(id);
        }
    }
    stalled
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
            movie_id: None,
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

    /// Tanukarr's `StalledRecoveryFixture`: only downloading or warning torrents with no
    /// progress for the timeout stall; progress or a status change restarts the clock.
    #[test]
    fn stalled_torrents_follow_tanukarr() {
        let item = |id: &str, status, left| DownloadItem {
            download_id: id.into(),
            title: id.into(),
            category: "tv".into(),
            total_size: 100,
            remaining_size: left,
            remaining_seconds: None,
            status,
            message: None,
            output_path: None,
            seed_ratio: 0.0,
            can_be_removed: false,
        };
        let start = std::time::Instant::now();
        let later = start + std::time::Duration::from_secs(600);
        let mut progress = Progress::new();
        let first = [
            item("a", ItemStatus::Downloading, 50),
            item("b", ItemStatus::Paused, 50),
            item("c", ItemStatus::Warning, 50),
            item("d", ItemStatus::Downloading, 50),
        ];
        assert!(stalled_downloads(&mut progress, &first, 300, start).is_empty());
        let second = [
            item("a", ItemStatus::Downloading, 50),
            item("b", ItemStatus::Paused, 50),
            item("c", ItemStatus::Warning, 50),
            item("d", ItemStatus::Downloading, 40),
        ];
        let stalled = stalled_downloads(&mut progress, &second, 300, later);
        let mut ids: Vec<&String> = stalled.iter().collect();
        ids.sort();
        assert_eq!(ids, vec!["A", "C"]);
        // Off.
        assert!(stalled_downloads(&mut progress, &second, 0, later).is_empty());
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
