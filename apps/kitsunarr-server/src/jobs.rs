//! Built-in background jobs.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use kitsunarr_db::{auth, maintenance, tasks};
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_metadata::Metadata;
use kitsunarr_scheduler::{Job, JobContext, JobResult, job};

/// Scheduled backups kept on disk (manual and pre-migration backups are never pruned).
const KEEP_SCHEDULED_BACKUPS: usize = 8;
const KEEP_TASK_RUNS: i64 = 2000;

pub fn all(
    backup_dir: PathBuf,
    tmp_dir: PathBuf,
    metadata: Metadata,
    downloads: kitsunarr_library::downloads::Monitor,
    api: Arc<OnceLock<kitsunarr_api::AppState>>,
) -> Vec<Job> {
    let backup_dir_for_job = backup_dir.clone();
    let api_for_downloads = api.clone();
    let api_for_health = api.clone();
    let api_for_watched = api.clone();
    let api_for_lists = api.clone();
    vec![
        job(
            kitsunarr_api::rss::TASK,
            "Reads the newest releases of the indexers with RSS enabled and grabs the wanted ones",
            15,
            Duration::from_secs(30 * 60),
            move |_ctx| {
                let api = api.clone();
                async move {
                    match api.get() {
                        Some(state) => kitsunarr_api::rss::sync(state).await.map(Some),
                        None => Ok(Some("Skipped: the server is still starting".into())),
                    }
                }
            },
        ),
        job(
            "RefreshMonitoredDownloads",
            "Reads the download clients and imports completed downloads Kitsunarr grabbed",
            1,
            // Copying a large file (when hard links are not possible) can take a while.
            Duration::from_secs(2 * 3600),
            move |ctx| {
                let downloads = downloads.clone();
                let api = api_for_downloads.clone();
                async move {
                    match api.get() {
                        Some(state) => kitsunarr_api::downloads::refresh(state).await.map(Some),
                        // Before the API is up: import only; searches wait for the next run.
                        None => downloads.refresh(&ctx.db).await.map(Some),
                    }
                }
            },
        ),
        job(
            kitsunarr_api::import_lists::TASK,
            "Reads the import lists that are due and adds their new titles",
            15,
            Duration::from_secs(2 * 3600),
            move |_ctx| {
                let api = api_for_lists.clone();
                async move {
                    match api.get() {
                        Some(state) => kitsunarr_api::import_lists::sync(state, None, false)
                            .await
                            .map(Some),
                        None => Ok(None),
                    }
                }
            },
        ),
        job(
            "ProcessPendingJellyfinRefresh",
            "Refreshes the Jellyfin libraries that have imports pending (on each connection's refresh mode)",
            1,
            Duration::from_secs(15 * 60),
            {
                let http = reqwest::Client::new();
                move |ctx| {
                    let http = http.clone();
                    async move {
                        kitsunarr_library::jellyfin_refresh::process_pending(&ctx.db, &http)
                            .await
                            .map(Some)
                    }
                }
            },
        ),
        job(
            "SyncJellyfinWatched",
            "Reads which episodes a Jellyfin user has watched (on the connection's own interval)",
            1,
            Duration::from_secs(15 * 60),
            move |ctx| {
                let api = api_for_watched.clone();
                async move {
                    match api.get() {
                        Some(state) => {
                            kitsunarr_library::jellyfin::sync(&ctx.db, &state.http, false)
                                .await
                                .map(Some)
                        }
                        None => Ok(None),
                    }
                }
            },
        ),
        job(
            "CleanUpRecycleBin",
            "Deletes files kept in the recycle bin longer than the media management setting allows",
            24 * 60,
            Duration::from_secs(3600),
            |ctx| async move {
                kitsunarr_library::tools::clean_recycle_bin(&ctx.db)
                    .await
                    .map(Some)
            },
        ),
        job(
            "RescanFolders",
            "Scans every series folder: forgets files that are gone and adds files found there",
            12 * 60,
            Duration::from_secs(6 * 3600),
            |ctx| async move { rescan_folders(&ctx).await },
        ),
        job(
            "RefreshMetadata",
            "Refreshes titles from their metadata catalogues (continuing ones every 6 hours, ended ones monthly)",
            12 * 60,
            Duration::from_secs(6 * 3600),
            move |ctx| {
                let metadata = metadata.clone();
                async move { refresh_metadata(&ctx, &metadata).await }
            },
        ),
        job(
            "Housekeeping",
            "Purges expired sessions, prunes task history and checks database integrity",
            24 * 60,
            Duration::from_secs(600),
            move |ctx| {
                let tmp_dir = tmp_dir.clone();
                async move {
                    let sessions = auth::purge_expired_sessions(ctx.db.pool())
                        .await
                        .map_err(|e| e.to_string())?;
                    let runs = tasks::prune_runs(ctx.db.pool(), KEEP_TASK_RUNS)
                        .await
                        .map_err(|e| e.to_string())?;
                    ctx.progress(0.4, "Checking database integrity").await;
                    let report = maintenance::integrity_check(&ctx.db)
                        .await
                        .map_err(|e| e.to_string())?;
                    if !report.ok() {
                        tracing::error!(?report, "database integrity check reported problems");
                    }
                    let stale = remove_stale_files(&tmp_dir, Duration::from_secs(24 * 3600));
                    Ok(Some(format!(
                        "{sessions} expired sessions removed, {runs} old task runs pruned, {stale} stale temporary files removed, integrity {}",
                        if report.ok() { "ok" } else { "PROBLEMS FOUND" }
                    )))
                }
            },
        ),
        job(
            "CleanupSessions",
            "Removes expired login sessions",
            60,
            Duration::from_secs(60),
            |ctx| async move {
                let n = auth::purge_expired_sessions(ctx.db.pool())
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Some(format!("{n} expired sessions removed")))
            },
        ),
        job(
            "Backup",
            "Creates a consistent database backup and keeps the most recent ones",
            7 * 24 * 60,
            Duration::from_secs(1800),
            move |ctx| {
                let dir = backup_dir_for_job.clone();
                async move {
                    let dest = maintenance::unique_backup_path(&dir, "scheduled");
                    maintenance::backup_to(&ctx.db, &dest)
                        .await
                        .map_err(|e| e.to_string())?;
                    let pruned = prune_scheduled_backups(&dir, KEEP_SCHEDULED_BACKUPS);
                    Ok(Some(format!(
                        "{} written, {pruned} old scheduled backups removed",
                        dest.display()
                    )))
                }
            },
        ),
        job(
            "HealthCheck",
            "Re-evaluates health checks, notifies connected browsers and sends health notifications",
            6 * 60,
            Duration::from_secs(60),
            move |ctx| {
                let api = api_for_health.clone();
                async move {
                    ctx.events.publish(ServerEvent::HealthChanged);
                    match api.get() {
                        Some(state) => {
                            kitsunarr_api::routes::system::check_health_and_notify(state)
                                .await
                                .map(Some)
                                .map_err(|e| e.message())
                        }
                        None => Ok(None),
                    }
                }
            },
        ),
    ]
}

/// Refreshes every title that is due. One failing title does not stop the others; the run
/// fails only when every refresh failed (typically: the catalogue is unreachable).
async fn refresh_metadata(ctx: &JobContext, metadata: &Metadata) -> JobResult {
    let (series, movies) =
        kitsunarr_library::due_for_refresh(&ctx.db, metadata, chrono::Utc::now())
            .await
            .map_err(|e| e.to_string())?;
    let total = series.len() + movies.len();
    if total == 0 {
        return Ok(Some("Nothing is due for a refresh".into()));
    }
    let (mut refreshed_series, mut refreshed_movies, mut failed) = (Vec::new(), Vec::new(), 0usize);
    let work = series
        .iter()
        .map(|id| ("series", *id))
        .chain(movies.iter().map(|id| ("movie", *id)));
    for (done, (kind, id)) in work.enumerate() {
        if ctx.cancel.is_cancelled() {
            break;
        }
        ctx.progress(
            done as f64 / total as f64,
            &format!("Refreshing {} of {total}", done + 1),
        )
        .await;
        let result = if kind == "series" {
            kitsunarr_library::refresh_series(&ctx.db, metadata, id)
                .await
                .map(|_| ())
        } else {
            kitsunarr_library::refresh_movie(&ctx.db, metadata, id).await
        };
        match result {
            Ok(()) if kind == "series" => refreshed_series.push(id),
            Ok(()) => refreshed_movies.push(id),
            Err(e) => {
                failed += 1;
                tracing::warn!(kind, id, error = %e, "metadata refresh failed");
            }
        }
    }
    let refreshed = refreshed_series.len() + refreshed_movies.len();
    for (media_type, ids) in [("series", refreshed_series), ("movie", refreshed_movies)] {
        if !ids.is_empty() {
            ctx.events.publish(ServerEvent::LibraryChanged {
                media_type: media_type.into(),
                ids,
                deleted: false,
            });
        }
    }
    if refreshed == 0 && failed > 0 {
        return Err(format!("All {failed} refreshes failed; see the logs"));
    }
    Ok(Some(format!(
        "{refreshed} titles refreshed, {failed} failed"
    )))
}

fn remove_stale_files(dir: &std::path::Path, age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|e| e > age);
        if old {
            let path = entry.path();
            let ok = if path.is_dir() {
                std::fs::remove_dir_all(&path).is_ok()
            } else {
                std::fs::remove_file(&path).is_ok()
            };
            removed += usize::from(ok);
        }
    }
    removed
}

/// Deletes the oldest `*_scheduled.db` backups beyond `keep`. Other backups are untouched.
fn prune_scheduled_backups(dir: &std::path::Path, keep: usize) -> usize {
    let Ok(list) = maintenance::list_backups(dir) else {
        return 0;
    };
    list.into_iter()
        .filter(|(name, _, _)| name.ends_with("_scheduled.db") || name.contains("_scheduled-"))
        .skip(keep)
        .filter(|(name, _, _)| std::fs::remove_file(dir.join(name)).is_ok())
        .count()
}

/// `RescanSeries` for every series. A series whose root folder is missing or empty is skipped
/// (a disconnected drive must not empty the library); the run fails only when every scan did.
async fn rescan_folders(ctx: &JobContext) -> JobResult {
    let ids: Vec<i64> =
        kitsunarr_db::sqlx::query_scalar("SELECT id FROM series ORDER BY sort_title")
            .fetch_all(ctx.db.pool())
            .await
            .map_err(|e| e.to_string())?;
    let (mut added, mut removed, mut failed) = (0usize, 0usize, 0usize);
    for (done, id) in ids.iter().enumerate() {
        match kitsunarr_library::files::scan_series(&ctx.db, *id).await {
            Ok(s) => {
                added += s.added;
                removed += s.removed;
            }
            Err(e) => {
                tracing::warn!(series_id = id, error = %e, "folder scan skipped");
                failed += 1;
            }
        }
        ctx.progress(
            (done + 1) as f64 / ids.len().max(1) as f64,
            &format!("Scanned {} of {} series", done + 1, ids.len()),
        )
        .await;
    }
    if failed > 0 && failed == ids.len() {
        return Err(format!("every folder scan failed ({failed})"));
    }
    if added + removed > 0 {
        ctx.events.publish(ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: Vec::new(),
            deleted: false,
        });
    }
    Ok(Some(format!(
        "Scanned {} series: {added} files added, {removed} missing files removed, {failed} skipped",
        ids.len()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pruning_only_touches_scheduled_backups() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(
                dir.path().join(format!(
                    "kitsunarr_2026-01-0{}_00-00-00-000_scheduled.db",
                    i + 1
                )),
                b"x",
            )
            .unwrap();
            std::thread::sleep(Duration::from_millis(15));
        }
        std::fs::write(
            dir.path()
                .join("kitsunarr_2026-01-01_00-00-00-000_pre-migration.db"),
            b"x",
        )
        .unwrap();
        assert_eq!(prune_scheduled_backups(dir.path(), 2), 3);
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 3);
        assert!(
            dir.path()
                .join("kitsunarr_2026-01-01_00-00-00-000_pre-migration.db")
                .exists()
        );
    }
}
