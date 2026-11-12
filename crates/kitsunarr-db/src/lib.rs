//! SQLite persistence for Kitsunarr.
//!
//! * One `SqlitePool` in WAL mode with foreign keys enforced on every connection.
//! * Embedded, versioned migrations (`/migrations`), applied at start-up.
//! * Repository modules expose plain async functions taking an executor or the pool.
//!
//! Queries are checked at runtime (not with `query!` macros) so the workspace builds without a
//! live database; every repository function is covered by tests against a real SQLite file.

pub mod auth;
pub mod blocklist;
pub mod companion_keys;
pub mod history;
pub mod import_lists;
pub mod library;
pub mod maintenance;
pub mod migration_runs;
pub mod providers;
pub mod quality;
pub mod root_folders;
pub mod rules;
pub mod seed;
pub mod settings;
pub mod tags;
pub mod tasks;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{SqlitePool, migrate::Migrator};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

pub use sqlx;

/// Embedded schema migrations.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("record not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Invalid(String),
    #[error("stored JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl DbError {
    /// Maps SQLite constraint failures to a friendlier conflict error.
    pub fn from_write(err: sqlx::Error, what: &str) -> Self {
        if let sqlx::Error::Database(db) = &err {
            if db.is_unique_violation() {
                return DbError::Conflict(format!("{what} already exists"));
            }
            if db.is_foreign_key_violation() {
                return DbError::Conflict(format!(
                    "{what} references a record that does not exist or is still in use"
                ));
            }
            if db.is_check_violation() {
                return DbError::Invalid(format!("{what} contains an invalid value"));
            }
        }
        DbError::Sqlx(err)
    }
}

pub type DbResult<T> = Result<T, DbError>;

/// Current UTC time as RFC 3339 with second precision (`2026-11-09T17:34:40Z`).
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Shared database handle.
#[derive(Clone, Debug)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Opens (creating if needed) the database file and applies pending migrations.
    pub async fn open(path: &Path) -> DbResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(15))
            .pragma("temp_store", "memory")
            .pragma("cache_size", "-16000");
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(options)
            .await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    /// In-memory database for tests (single connection so all queries see the same data).
    pub async fn open_in_memory() -> DbResult<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    pub async fn migrate(&self) -> DbResult<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Highest applied migration version.
    pub async fn schema_version(&self) -> DbResult<i64> {
        let v: Option<i64> =
            sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1")
                .fetch_one(&self.pool)
                .await?;
        Ok(v.unwrap_or(0))
    }

    pub async fn sqlite_version(&self) -> DbResult<String> {
        Ok(sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Encodes a value as JSON text for storage.
pub(crate) fn to_json<T: serde::Serialize + ?Sized>(value: &T) -> DbResult<String> {
    Ok(serde_json::to_string(value)?)
}

/// Decodes stored JSON text, mapping errors to [`DbError::Json`].
pub(crate) fn from_json<T: serde::de::DeserializeOwned>(text: &str) -> DbResult<T> {
    Ok(serde_json::from_str(text)?)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub async fn db() -> Database {
        let db = Database::open_in_memory()
            .await
            .expect("in-memory database");
        seed::seed_defaults(&db).await.expect("seed");
        db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opens_file_database_in_wal_mode_with_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("k.db")).await.unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(fk, 1);
        assert!(db.schema_version().await.unwrap() >= 20261109000001);
        // Re-opening is idempotent (migrations already applied).
        db.close().await;
        Database::open(&dir.path().join("k.db")).await.unwrap();
    }
}
