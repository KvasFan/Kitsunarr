//! Migration (Sonarr / Tanukarr → Kitsunarr) contract.
//!
//! Flow: create a session from an API endpoint or an uploaded backup → the server loads a
//! read-only snapshot → request a plan (dry run) with path mappings → apply the plan →
//! fetch the report. Sources are never written to.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum SourceApp {
    Sonarr,
    Tanukarr,
}

impl SourceApp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sonarr => "sonarr",
            Self::Tanukarr => "tanukarr",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum SourceMethod {
    Api,
    Backup,
}

impl SourceMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Backup => "backup",
        }
    }
}

/// Body of `POST /api/v1/migration/sessions` for API-based migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApiSourceRequest {
    /// Base URL including any URL base, e.g. `http://sonarr:8989` or `https://host/sonarr`.
    pub url: String,
    pub api_key: String,
    /// Allow connecting to hosts on private/loopback networks (the usual case for a home
    /// server). Link-local and metadata addresses are always refused.
    #[serde(default = "default_true")]
    pub allow_private_network: bool,
}

fn default_true() -> bool {
    true
}

/// What was detected about the source installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DetectedSource {
    pub app: SourceApp,
    pub method: SourceMethod,
    /// Application version (`4.0.20.3014`; Tanukarr reports its Sonarr base build here).
    pub app_version: String,
    /// Fork product version (`0.1.5` for Tanukarr).
    pub product_version: Option<String>,
    /// Highest applied FluentMigrator migration.
    pub schema_version: Option<i64>,
    pub instance_name: Option<String>,
    /// Stable identifier of the source used to make repeated migrations idempotent.
    pub fingerprint: String,
    /// Whether provider secrets (API keys, passwords) are available. API sources mask
    /// passwords, so those must be re-entered after an API migration.
    pub secrets_available: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Loading,
    Ready,
    Applying,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EntityCount {
    pub entity: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MigrationSession {
    pub id: String,
    pub status: SessionStatus,
    pub source: Option<DetectedSource>,
    pub inventory: Vec<EntityCount>,
    /// Distinct path prefixes used by the source (root folders), to seed path mappings.
    pub source_root_folders: Vec<String>,
    pub progress_message: Option<String>,
    pub error: Option<String>,
    /// Set once the plan has been applied.
    pub run_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PathMapping {
    /// Path prefix as seen by the source application (e.g. `/tv`).
    pub source_prefix: String,
    /// Same location as seen by Kitsunarr (e.g. `/media/tv`).
    pub target_prefix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Keep the existing Kitsunarr record untouched.
    #[default]
    Skip,
    /// Overwrite mapped fields of the existing record with the source values.
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MigrationOptions {
    #[serde(default)]
    pub path_mappings: Vec<PathMapping>,
    /// Entity groups to import (see [`MIGRATION_ENTITY_GROUPS`]); empty = all.
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub conflict_policy: ConflictPolicy,
    #[serde(default = "default_true")]
    pub include_history: bool,
    /// Check that mapped media paths exist (read-only `stat`); never creates or moves files.
    #[serde(default = "default_true")]
    pub verify_paths: bool,
}

impl Default for MigrationOptions {
    fn default() -> Self {
        Self {
            path_mappings: Vec::new(),
            entities: Vec::new(),
            conflict_policy: ConflictPolicy::Skip,
            include_history: true,
            verify_paths: true,
        }
    }
}

/// Selectable entity groups.
pub const MIGRATION_ENTITY_GROUPS: &[&str] = &[
    "tags",
    "rootFolders",
    "qualityDefinitions",
    "customFormats",
    "qualityProfiles",
    "delayProfiles",
    "releaseProfiles",
    "indexers",
    "downloadClients",
    "notifications",
    "importLists",
    "importListExclusions",
    "remotePathMappings",
    "settings",
    "series",
    "movies",
    "history",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum WarningSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MigrationWarning {
    pub severity: WarningSeverity,
    pub entity: String,
    pub source_id: Option<String>,
    pub title: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EntityPlanSummary {
    pub entity: String,
    pub create: u64,
    pub update: u64,
    pub skip: u64,
    pub unsupported: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PathPreview {
    pub title: String,
    pub source_path: String,
    pub target_path: String,
    /// `None` when verification was disabled.
    pub exists: Option<bool>,
}

/// Dry-run result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MigrationPreview {
    pub session_id: String,
    pub source: DetectedSource,
    pub summary: Vec<EntityPlanSummary>,
    pub warnings: Vec<MigrationWarning>,
    pub paths: Vec<PathPreview>,
    /// Problems that prevent applying (e.g. unmapped paths when verification is on).
    pub blocking: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ValidationCheck {
    pub name: String,
    pub passed: bool,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MigrationRunItem {
    pub id: i64,
    pub source_app: SourceApp,
    pub source_method: SourceMethod,
    pub source_version: String,
    pub source_instance: Option<String>,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub summary: Vec<EntityPlanSummary>,
    pub warning_count: u64,
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub run: MigrationRunItem,
    pub warnings: Vec<MigrationWarning>,
    pub validation: Vec<ValidationCheck>,
}
