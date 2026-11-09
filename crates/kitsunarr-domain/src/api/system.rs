use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    pub app_name: String,
    pub instance_name: String,
    pub version: String,
    pub git_revision: Option<String>,
    pub start_time: String,
    pub os: String,
    pub arch: String,
    pub is_container: bool,
    pub url_base: String,
    pub data_directory: String,
    pub database_schema_version: i64,
    pub sqlite_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum HealthLevel {
    Ok,
    Notice,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct HealthCheck {
    pub source: String,
    pub level: HealthLevel,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTaskItem {
    pub name: String,
    pub description: String,
    pub interval_minutes: i64,
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub last_duration_ms: Option<i64>,
    pub next_run_at: Option<String>,
    pub running: bool,
    pub last_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TaskRunItem {
    pub id: i64,
    pub task_name: String,
    pub trigger: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub message: Option<String>,
    pub progress: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub time: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BackupItem {
    pub name: String,
    pub size: u64,
    pub created_at: String,
}
