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

#[cfg(test)]
mod tests {
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
