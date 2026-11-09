//! Built-in background jobs.

use std::path::PathBuf;
use std::time::Duration;

use kitsunarr_db::{auth, maintenance, tasks};
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_scheduler::{Job, job};

/// Scheduled backups kept on disk (manual and pre-migration backups are never pruned).
const KEEP_SCHEDULED_BACKUPS: usize = 8;
const KEEP_TASK_RUNS: i64 = 2000;

pub fn all(backup_dir: PathBuf, tmp_dir: PathBuf) -> Vec<Job> {
    let backup_dir_for_job = backup_dir.clone();
    vec![
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
            "Re-evaluates health checks and notifies connected browsers",
            6 * 60,
            Duration::from_secs(60),
            |ctx| async move {
                ctx.events.publish(ServerEvent::HealthChanged);
                Ok(None)
            },
        ),
    ]
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
