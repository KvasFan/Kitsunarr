//! Real-time events pushed to browsers over Server-Sent Events (`GET /api/v1/events`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerEvent {
    /// A series or movie was created, updated or deleted; clients should refetch.
    #[serde(rename_all = "camelCase")]
    LibraryChanged {
        media_type: String,
        ids: Vec<i64>,
        deleted: bool,
    },
    /// A background task changed state or reported progress.
    #[serde(rename_all = "camelCase")]
    Task {
        run_id: i64,
        task_name: String,
        status: String,
        progress: Option<f64>,
        message: Option<String>,
    },
    /// Progress of a migration session (connect, plan, apply, validate).
    #[serde(rename_all = "camelCase")]
    Migration {
        session_id: String,
        stage: String,
        done: u64,
        total: u64,
        message: String,
    },
    /// Health check results changed.
    HealthChanged,
    /// Settings changed (tags, profiles, providers); clients should refetch caches.
    #[serde(rename_all = "camelCase")]
    SettingsChanged { section: String },
    /// A short message suitable for a toast notification.
    #[serde(rename_all = "camelCase")]
    Notice { level: String, message: String },
}

impl ServerEvent {
    /// SSE `event:` name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::LibraryChanged { .. } => "libraryChanged",
            Self::Task { .. } => "task",
            Self::Migration { .. } => "migration",
            Self::HealthChanged => "healthChanged",
            Self::SettingsChanged { .. } => "settingsChanged",
            Self::Notice { .. } => "notice",
        }
    }
}
