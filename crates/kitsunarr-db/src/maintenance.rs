//! Backups and integrity checks.

use std::path::{Path, PathBuf};

use crate::{Database, DbError, DbResult};

/// Writes a consistent copy of the live database with `VACUUM INTO` (safe while running).
pub async fn backup_to(db: &Database, dest: &Path) -> DbResult<PathBuf> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if dest.exists() {
        return Err(DbError::Conflict(format!(
            "{} already exists",
            dest.display()
        )));
    }
    let path = dest
        .to_str()
        .ok_or_else(|| DbError::Invalid("backup path is not valid UTF-8".into()))?;
    sqlx::query("VACUUM INTO ?")
        .bind(path)
        .execute(db.pool())
        .await?;
    Ok(dest.to_path_buf())
}

/// Timestamped backup file name, e.g. `kitsunarr_2026-11-09_17-34-40-123_pre-migration.db`.
pub fn backup_file_name(reason: &str) -> String {
    let ts = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S-%3f");
    let reason: String = reason
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    format!("kitsunarr_{ts}_{reason}.db")
}

/// A backup path in `dir` that does not exist yet.
pub fn unique_backup_path(dir: &Path, reason: &str) -> PathBuf {
    let base = backup_file_name(reason);
    let mut candidate = dir.join(&base);
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(base.replace(".db", &format!("-{n}.db")));
        n += 1;
    }
    candidate
}

/// The scheduled backup task's name.
pub const BACKUP_TASK: &str = "Backup";

/// `GetBackupInterval`: Sonarr's `BackupInterval` (days, clamped to 1-7) as the Backup task's
/// interval in minutes.
pub fn backup_interval_minutes(days: i32) -> i64 {
    i64::from(days.clamp(1, 7)) * 24 * 60
}

/// Applies the backup interval to the Backup task (registering it when it is not yet).
pub async fn set_backup_interval(db: &Database, days: i32) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO scheduled_tasks (name, interval_minutes, last_run_at) VALUES (?, ?, ?) \
         ON CONFLICT (name) DO UPDATE SET interval_minutes = excluded.interval_minutes",
    )
    .bind(BACKUP_TASK)
    .bind(backup_interval_minutes(days))
    .bind(crate::now())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Tanukarr 1.0.4: a successful manual backup is recorded as a Backup task run, so the next
/// scheduled backup is one interval after it (`TaskManager.Handle(CommandExecutedEvent)`).
/// Returns the run id.
pub async fn record_manual_backup(db: &Database, message: &str, duration_ms: i64) -> DbResult<i64> {
    let run = crate::tasks::start_run(db.pool(), BACKUP_TASK, "manual").await?;
    crate::tasks::finish_run(db, run, "completed", Some(message), None, duration_ms).await?;
    Ok(run)
}

/// `CleanupOldBackups`: deletes `*_scheduled` backups in `dir` last written more than
/// `retention_days` ago (Sonarr's `BackupRetention`); manual and pre-migration backups are
/// kept. Returns how many were deleted.
pub fn prune_scheduled_backups(
    dir: &Path,
    retention_days: i32,
    now: std::time::SystemTime,
) -> usize {
    let Ok(list) = list_backups(dir) else {
        return 0;
    };
    let keep =
        std::time::Duration::from_secs(u64::from(retention_days.max(1).unsigned_abs()) * 86_400);
    list.into_iter()
        .filter(|(name, _, _)| name.ends_with("_scheduled.db") || name.contains("_scheduled-"))
        .filter(|(_, _, modified)| now.duration_since(*modified).is_ok_and(|age| age > keep))
        .filter(|(name, _, _)| std::fs::remove_file(dir.join(name)).is_ok())
        .count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    pub integrity: Vec<String>,
    pub foreign_key_violations: i64,
}

impl IntegrityReport {
    pub fn ok(&self) -> bool {
        self.integrity == ["ok"] && self.foreign_key_violations == 0
    }
}

pub async fn integrity_check(db: &Database) -> DbResult<IntegrityReport> {
    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(db.pool())
        .await?;
    let fk: Vec<(String,)> = sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
        .fetch_all(db.pool())
        .await?;
    Ok(IntegrityReport {
        integrity,
        foreign_key_violations: fk.len() as i64,
    })
}

/// Lists backup files (newest first) in `dir`.
pub fn list_backups(dir: &Path) -> std::io::Result<Vec<(String, u64, std::time::SystemTime)>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if meta.is_file() && name.starts_with("kitsunarr_") && name.ends_with(".db") {
            out.push((
                name,
                meta.len(),
                meta.modified().unwrap_or(std::time::UNIX_EPOCH),
            ));
        }
    }
    out.sort_by_key(|entry| std::cmp::Reverse(entry.2));
    Ok(out)
}

/// Where a backup chosen for restoring (Sonarr's `system/backup/restore`) waits until the next
/// start.
pub fn staged_restore_path(data_dir: &Path) -> PathBuf {
    data_dir.join("restore").join("kitsunarr.db")
}

/// Replaces the database with a staged backup, if one is waiting: the current database is kept
/// next to it (`….db.pre-restore-<time>`), whose path is returned. Must run before the database
/// is opened.
pub fn apply_staged_restore(data_dir: &Path, database: &Path) -> std::io::Result<Option<PathBuf>> {
    let staged = staged_restore_path(data_dir);
    if !staged.is_file() {
        return Ok(None);
    }
    let aside = database.with_extension(format!(
        "db.pre-restore-{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    ));
    if database.exists() {
        std::fs::rename(database, &aside)?;
    }
    for ext in ["db-wal", "db-shm"] {
        let _ = std::fs::remove_file(database.with_extension(ext));
    }
    if std::fs::rename(&staged, database).is_err() {
        std::fs::copy(&staged, database)?;
        std::fs::remove_file(&staged)?;
    }
    Ok(Some(aside))
}

#[cfg(test)]
mod tests {
    #[test]
    fn staged_restores_replace_the_database_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("kitsunarr.db");
        std::fs::write(&db, b"old").unwrap();
        std::fs::write(db.with_extension("db-wal"), b"wal").unwrap();
        assert!(apply_staged_restore(dir.path(), &db).unwrap().is_none());
        let staged = staged_restore_path(dir.path());
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"new").unwrap();
        let aside = apply_staged_restore(dir.path(), &db).unwrap().unwrap();
        assert_eq!(std::fs::read(&db).unwrap(), b"new");
        assert_eq!(std::fs::read(&aside).unwrap(), b"old");
        assert!(!staged.exists() && !db.with_extension("db-wal").exists());
        assert!(apply_staged_restore(dir.path(), &db).unwrap().is_none());
    }

    use super::*;

    #[tokio::test]
    async fn backup_and_check() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("live.db")).await.unwrap();
        crate::seed::seed_defaults(&db).await.unwrap();
        let report = integrity_check(&db).await.unwrap();
        assert!(report.ok(), "{report:?}");
        let dest = dir
            .path()
            .join("backups")
            .join(backup_file_name("test run!"));
        backup_to(&db, &dest).await.unwrap();
        assert!(dest.exists());
        assert!(
            dest.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("_testrun.db")
        );
        assert!(backup_to(&db, &dest).await.is_err(), "never overwrites");
        let restored = Database::open(&dest).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quality_profiles")
            .fetch_one(restored.pool())
            .await
            .unwrap();
        assert_eq!(n, 6);
        assert_eq!(list_backups(&dir.path().join("backups")).unwrap().len(), 1);
    }
}
