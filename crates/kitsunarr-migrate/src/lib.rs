//! Migration from Sonarr and Tanukarr into Kitsunarr.
//!
//! Two read-only sources feed one importer:
//! * [`load_from_api`] — a running instance's `/api/v3` (secrets are masked by the source),
//! * [`load_from_backup`] — a backup `.zip` or bare `sonarr.db` (secrets included).
//!
//! Both produce a [`LoadedSource`]; [`import::run`] performs a dry run or applies it.
//! Nothing here ever writes to the source or touches media files.

pub mod backup;
pub mod import;
pub mod model;
pub mod normalize;
pub mod paths;
pub mod remote;
pub mod values;

use std::path::Path;

use kitsunarr_db::DbError;
use kitsunarr_domain::api::migration::{
    ApiSourceRequest, DetectedSource, SourceApp, SourceMethod, ValidationCheck,
};
use sha2::{Digest, Sha256};

pub use import::{Mode, Outcome, run};
pub use model::SourceSnapshot;

/// Highest source schema versions verified against real fixtures.
pub const TESTED_SONARR_SCHEMA: i64 = 217;
pub const TESTED_TANUKARR_SCHEMA: i64 = 254;

#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error("{0}")]
    InvalidSource(String),
    #[error("{0}")]
    InvalidBackup(String),
    #[error("{0}")]
    Unreachable(String),
    #[error("{0}")]
    InvalidOptions(String),
    #[error("migration validation failed: {reason}")]
    ValidationFailed {
        reason: String,
        checks: Vec<ValidationCheck>,
    },
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Internal(String),
}

/// A fully read source, ready to plan or apply.
#[derive(Debug, Clone)]
pub struct LoadedSource {
    pub detected: DetectedSource,
    pub snapshot: SourceSnapshot,
    /// Reader notes surfaced in the plan (missing endpoints, truncation, ...).
    pub notes: Vec<String>,
}

/// Stable identity of a source installation.
pub fn fingerprint(app: SourceApp, identity: &str) -> String {
    let digest = Sha256::digest(format!("{}:{}", app.as_str(), identity.trim()).as_bytes());
    hex::encode(&digest[..16])
}

fn version_notes(app: SourceApp, schema: Option<i64>) -> Vec<String> {
    let tested = match app {
        SourceApp::Sonarr => TESTED_SONARR_SCHEMA,
        SourceApp::Tanukarr => TESTED_TANUKARR_SCHEMA,
    };
    match schema {
        Some(v) if v > tested => vec![format!(
            "The source database schema ({v}) is newer than the newest version Kitsunarr was tested against ({tested}); \
             review the preview carefully."
        )],
        Some(v) if app == SourceApp::Sonarr && v < 150 => vec![format!(
            "The source database schema ({v}) is from an old Sonarr v3 release; upgrade the source to Sonarr v4 for the best result."
        )],
        _ => Vec::new(),
    }
}

/// Reads a running Sonarr/Tanukarr instance.
pub async fn load_from_api(
    req: &ApiSourceRequest,
    user_agent: &str,
    progress: &(dyn Fn(String, u64, u64) + Send + Sync),
) -> Result<LoadedSource, MigrateError> {
    let client = remote::RemoteSource::connect(req, user_agent).await?;
    let (status, raw, mut notes) = remote::load(&client, progress).await?;
    let snapshot = normalize::normalize(&raw);
    notes.extend(version_notes(status.app, status.migration_version));
    if snapshot
        .providers
        .iter()
        .any(|p| !p.masked_fields.is_empty())
    {
        notes.push("The source API hides passwords and API keys of indexers, download clients and notifications; \
                    migrate from a backup to carry them over, or re-enter them afterwards."
            .into());
    }
    let detected = DetectedSource {
        app: status.app,
        method: SourceMethod::Api,
        app_version: status.version,
        product_version: status.product_version,
        schema_version: status.migration_version,
        instance_name: status.instance_name,
        fingerprint: fingerprint(status.app, &req.api_key),
        secrets_available: false,
        notes: notes.clone(),
    };
    Ok(LoadedSource {
        detected,
        snapshot,
        notes,
    })
}

/// Reads an uploaded backup archive or database file. `work_root` receives a private
/// temporary directory that is removed before returning.
pub async fn load_from_backup(
    upload: &Path,
    work_root: &Path,
) -> Result<LoadedSource, MigrateError> {
    let (upload, root) = (upload.to_path_buf(), work_root.to_path_buf());
    let extracted = tokio::task::spawn_blocking(move || backup::extract(&upload, &root))
        .await
        .map_err(|e| MigrateError::Internal(e.to_string()))??;
    let (raw, meta) = backup::read_database(&extracted.db_path).await?;
    let config = extracted
        .config_xml
        .as_deref()
        .map(backup::parse_config_xml)
        .unwrap_or_default();
    let app = if meta.is_tanukarr {
        SourceApp::Tanukarr
    } else {
        SourceApp::Sonarr
    };
    let info_version = extracted
        .info
        .as_deref()
        .and_then(|i| i.lines().next())
        .map(|l| l.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty());
    let identity = config
        .get("ApiKey")
        .cloned()
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| {
            format!(
                "schema:{}:{}",
                meta.max_version.unwrap_or(0),
                meta.migrations.first().map(|m| m.1.as_str()).unwrap_or("")
            )
        });
    let mut notes = version_notes(app, meta.max_version);
    if extracted.config_xml.is_none() {
        notes.push("No config.xml was found; re-importing the same database is still idempotent, but API and backup \
                    migrations of this instance will not be recognised as the same source."
            .into());
    }
    let snapshot = normalize::normalize(&raw);
    drop(extracted); // removes the temporary copy
    let detected = DetectedSource {
        app,
        method: SourceMethod::Backup,
        app_version: info_version
            .unwrap_or_else(|| format!("schema {}", meta.max_version.unwrap_or(0))),
        product_version: None,
        schema_version: meta.max_version,
        instance_name: config
            .get("InstanceName")
            .cloned()
            .filter(|n| !n.is_empty()),
        fingerprint: fingerprint(app, &identity),
        secrets_available: true,
        notes: notes.clone(),
    };
    Ok(LoadedSource {
        detected,
        snapshot,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_stable_and_app_scoped() {
        let a = fingerprint(SourceApp::Sonarr, "abc");
        assert_eq!(a, fingerprint(SourceApp::Sonarr, " abc "));
        assert_ne!(a, fingerprint(SourceApp::Tanukarr, "abc"));
        assert_eq!(a.len(), 32);
    }
}
