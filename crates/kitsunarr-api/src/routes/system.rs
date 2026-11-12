//! System endpoints: ping, status, health, scheduled tasks and backups.

use std::path::Path;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use kitsunarr_db::{maintenance, providers, root_folders, tasks};
use kitsunarr_domain::api::settings::{MediaManagementSettings, ProviderKind};
use kitsunarr_domain::api::system::{
    BackupItem, HealthCheck, HealthLevel, LogEntry, ScheduledTaskItem, SystemStatus, TaskRunItem,
};
use serde::Serialize;

use crate::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};

/// Implemented by the scheduler; lets the API list and trigger background tasks.
pub trait TaskControl: Send + Sync + 'static {
    /// Description of a registered task, if known.
    fn describe(&self, name: &str) -> Option<String>;
    fn is_running(&self, name: &str) -> bool;
    /// Starts a task now. Returns `false` when unknown or already running.
    fn trigger(&self, name: &str) -> bool;
    /// Next scheduled run as RFC 3339, when known.
    fn next_run(&self, name: &str) -> Option<String>;
}

/// No-op implementation used by tests.
pub struct NoTasks;

impl TaskControl for NoTasks {
    fn describe(&self, _: &str) -> Option<String> {
        None
    }
    fn is_running(&self, _: &str) -> bool {
        false
    }
    fn trigger(&self, _: &str) -> bool {
        false
    }
    fn next_run(&self, _: &str) -> Option<String> {
        None
    }
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pong {
    pub status: &'static str,
    pub database: &'static str,
}

/// Liveness/readiness probe (used by the container health check). Public.
#[utoipa::path(get, path = "/ping", tag = "system", responses((status = 200, body = Pong), (status = 503, body = Pong)))]
pub async fn ping(State(state): State<AppState>) -> (StatusCode, Json<Pong>) {
    match sqlx_ping(&state).await {
        true => (
            StatusCode::OK,
            Json(Pong {
                status: "ok",
                database: "ok",
            }),
        ),
        false => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(Pong {
                status: "degraded",
                database: "unavailable",
            }),
        ),
    }
}

async fn sqlx_ping(state: &AppState) -> bool {
    kitsunarr_db::sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(state.db.pool())
        .await
        .is_ok()
}

#[utoipa::path(get, path = "/system/status", tag = "system", responses((status = 200, body = SystemStatus)))]
pub async fn status(State(state): State<AppState>) -> ApiResult<Json<SystemStatus>> {
    let general: kitsunarr_domain::api::settings::GeneralSettings =
        kitsunarr_db::settings::get_or_default(
            state.db.pool(),
            kitsunarr_db::settings::KEY_GENERAL,
        )
        .await?;
    Ok(Json(SystemStatus {
        app_name: "Kitsunarr".into(),
        instance_name: general.instance_name,
        version: state.build.version.into(),
        git_revision: state.build.git_revision.map(Into::into),
        start_time: state
            .started_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        is_container: Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists(),
        url_base: state.config.server.url_base.clone(),
        data_directory: state.config.data_dir.display().to_string(),
        database_schema_version: state.db.schema_version().await?,
        sqlite_version: state.db.sqlite_version().await?,
    }))
}

/// Free and total bytes of the filesystem holding `path`.
pub fn disk_space(path: &str) -> Option<(u64, u64)> {
    let p = Path::new(path);
    if !p.is_dir() {
        return None;
    }
    Some((fs4::available_space(p).ok()?, fs4::total_space(p).ok()?))
}

/// Computes the current health checks.
pub async fn compute_health(state: &AppState) -> ApiResult<Vec<HealthCheck>> {
    let mut out = Vec::new();
    let mm: MediaManagementSettings = kitsunarr_db::settings::get_or_default(
        state.db.pool(),
        kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
    )
    .await?;
    let roots = root_folders::list(state.db.pool()).await?;
    if roots.is_empty() {
        out.push(HealthCheck {
            source: "RootFolderCheck".into(),
            level: HealthLevel::Notice,
            message: "No root folders are configured yet. Add one in Settings → Media Management or import an existing installation.".into(),
        });
    }
    for root in &roots {
        match disk_space(&root.path) {
            None => out.push(HealthCheck {
                source: "RootFolderCheck".into(),
                level: HealthLevel::Error,
                message: format!("Root folder {} is missing or not accessible.", root.path),
            }),
            Some((free, _))
                if !mm.skip_free_space_check
                    && free < (mm.minimum_free_space_mb.max(0) as u64) * 1024 * 1024 =>
            {
                out.push(HealthCheck {
                    source: "DiskSpaceCheck".into(),
                    level: HealthLevel::Warning,
                    message: format!(
                        "Root folder {} has only {} MiB free.",
                        root.path,
                        free / 1024 / 1024
                    ),
                })
            }
            _ => {}
        }
    }
    for (kind, label) in [
        (ProviderKind::Indexer, "indexer"),
        (ProviderKind::DownloadClient, "download client"),
    ] {
        let list = providers::list(state.db.pool(), kind).await?;
        if !list.iter().any(|p| p.enabled) {
            out.push(HealthCheck {
                source: format!("{label}Check"),
                level: HealthLevel::Warning,
                message: format!(
                    "No enabled {label} is configured; automatic downloads are not possible."
                ),
            });
        }
        let unsupported: Vec<_> = list
            .iter()
            .filter(|p| !providers::is_supported(kind, &p.implementation))
            .map(|p| p.name.clone())
            .collect();
        if !unsupported.is_empty() {
            out.push(HealthCheck {
                source: format!("{label}Support"),
                level: HealthLevel::Notice,
                message: format!(
                    "These {label}s were kept from your previous installation but are not implemented natively yet: {}.",
                    unsupported.join(", ")
                ),
            });
        }
    }
    // `DownloadClientStatusCheck`.
    let clients = providers::list(state.db.pool(), ProviderKind::DownloadClient).await?;
    let enabled_clients: Vec<_> = clients.iter().filter(|p| p.enabled).collect();
    let blocked_clients = kitsunarr_db::indexer_status::blocked_of(
        kitsunarr_db::indexer_status::Kind::DownloadClient,
        state.db.pool(),
        chrono::Utc::now(),
    )
    .await?;
    let names: Vec<String> = enabled_clients
        .iter()
        .filter(|p| blocked_clients.iter().any(|b| b.indexer_id == p.id))
        .map(|p| p.name.clone())
        .collect();
    if !names.is_empty() {
        let all = names.len() == enabled_clients.len();
        out.push(HealthCheck {
            source: "DownloadClientStatusCheck".into(),
            level: if all {
                HealthLevel::Error
            } else {
                HealthLevel::Warning
            },
            message: if all {
                "All download clients are unavailable due to failures".into()
            } else {
                format!(
                    "Download clients are unavailable due to failures: {}",
                    names.join(", ")
                )
            },
        });
    }
    // `IndexerStatusCheck`.
    let indexers = providers::list(state.db.pool(), ProviderKind::Indexer).await?;
    let enabled: Vec<_> = indexers.iter().filter(|p| p.enabled).collect();
    let blocked =
        kitsunarr_db::indexer_status::blocked(state.db.pool(), chrono::Utc::now()).await?;
    let names: Vec<String> = enabled
        .iter()
        .filter(|p| blocked.iter().any(|b| b.indexer_id == p.id))
        .map(|p| p.name.clone())
        .collect();
    if !names.is_empty() {
        let all = names.len() == enabled.len();
        out.push(HealthCheck {
            source: "IndexerStatusCheck".into(),
            level: if all {
                HealthLevel::Error
            } else {
                HealthLevel::Warning
            },
            message: if all {
                "All indexers are unavailable due to failures".into()
            } else {
                format!("Indexers unavailable due to failures: {}", names.join(", "))
            },
        });
    }
    // `ProxyCheck`: the proxy's host must resolve (Tanukarr does not ping Sonarr's servers).
    if let Some(proxy) = state.proxy.current() {
        let host = proxy.hostname.trim().to_string();
        let resolves = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::lookup_host((host.as_str(), proxy.port)),
        )
        .await
        .is_ok_and(|r| r.is_ok_and(|mut a| a.next().is_some()));
        if !resolves {
            out.push(HealthCheck {
                source: "ProxyCheck".into(),
                level: HealthLevel::Error,
                message: format!(
                    "Failed to resolve the IP address of the configured proxy host {host}"
                ),
            });
        }
    }
    if let Ok(data_free) = fs4::available_space(&state.config.data_dir)
        && data_free < 200 * 1024 * 1024
    {
        out.push(HealthCheck {
            source: "AppDataSpaceCheck".into(),
            level: HealthLevel::Error,
            message: format!(
                "The data directory has only {} MiB free; the database may fail to write.",
                data_free / 1024 / 1024
            ),
        });
    }
    Ok(out)
}

/// Re-evaluates health and notifies the connections about warnings and errors that appeared
/// (`OnHealthIssue`) or cleared (`OnHealthRestored`) since the previous evaluation. The first
/// evaluation after start-up only records the baseline, so restarts do not repeat old issues.
pub async fn check_health_and_notify(state: &AppState) -> ApiResult<String> {
    let current: Vec<HealthCheck> = compute_health(state)
        .await?
        .into_iter()
        .filter(|h| matches!(h.level, HealthLevel::Warning | HealthLevel::Error))
        .collect();
    let previous = state
        .last_health
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(current.clone());
    let Some(previous) = previous else {
        return Ok(format!("{} health issues", current.len()));
    };
    let key = |h: &HealthCheck| (h.source.clone(), h.message.clone());
    let to_event = |h: &HealthCheck| kitsunarr_notifications::Health {
        level: if h.level == HealthLevel::Error {
            "error".into()
        } else {
            "warning".into()
        },
        message: h.message.clone(),
        source: h.source.clone(),
        wiki_url: None,
    };
    let mut events = Vec::new();
    for h in current
        .iter()
        .filter(|h| !previous.iter().any(|p| key(p) == key(h)))
    {
        events.push(kitsunarr_notifications::Event::HealthIssue(to_event(h)));
    }
    for h in previous
        .iter()
        .filter(|p| !current.iter().any(|c| key(c) == key(p)))
    {
        events.push(kitsunarr_notifications::Event::HealthRestored(to_event(h)));
    }
    let summary = format!(
        "{} health issues; {} notifications to send",
        current.len(),
        events.len()
    );
    for e in events {
        kitsunarr_library::notify::spawn(&state.db, &state.downloads.notifier, e);
    }
    Ok(summary)
}

#[utoipa::path(get, path = "/system/health", tag = "system", responses((status = 200, body = Vec<HealthCheck>)))]
pub async fn health(State(state): State<AppState>) -> ApiResult<Json<Vec<HealthCheck>>> {
    Ok(Json(compute_health(&state).await?))
}

#[utoipa::path(get, path = "/system/tasks", tag = "system", responses((status = 200, body = Vec<ScheduledTaskItem>)))]
pub async fn tasks(State(state): State<AppState>) -> ApiResult<Json<Vec<ScheduledTaskItem>>> {
    let list = tasks::list(state.db.pool()).await?;
    Ok(Json(
        list.into_iter()
            .map(|t| ScheduledTaskItem {
                description: state.tasks.describe(&t.name).unwrap_or_default(),
                running: state.tasks.is_running(&t.name),
                next_run_at: state.tasks.next_run(&t.name),
                name: t.name,
                interval_minutes: t.interval_minutes,
                enabled: t.enabled,
                last_run_at: t.last_run_at,
                last_duration_ms: t.last_duration_ms,
                last_status: t.last_status,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/system/tasks/{name}/run", tag = "system",
    params(("name" = String, Path, description = "Task name")),
    responses((status = 202), (status = 404), (status = 409)))]
pub async fn run_task(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> ApiResult<StatusCode> {
    if state.tasks.describe(&name).is_none() {
        return Err(ApiError::NotFound(format!("Unknown task `{name}`.")));
    }
    if !state.tasks.trigger(&name) {
        return Err(ApiError::Conflict(format!(
            "Task `{name}` is already running."
        )));
    }
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(get, path = "/system/tasks/runs", tag = "system", responses((status = 200, body = Vec<TaskRunItem>)))]
pub async fn task_runs(State(state): State<AppState>) -> ApiResult<Json<Vec<TaskRunItem>>> {
    Ok(Json(tasks::recent_runs(state.db.pool(), 100).await?))
}

#[utoipa::path(get, path = "/system/backups", tag = "system", responses((status = 200, body = Vec<BackupItem>)))]
pub async fn backups(State(state): State<AppState>) -> ApiResult<Json<Vec<BackupItem>>> {
    let list = maintenance::list_backups(&state.config.backup_dir()).map_err(ApiError::internal)?;
    Ok(Json(
        list.into_iter()
            .map(|(name, size, modified)| BackupItem {
                name,
                size,
                created_at: chrono::DateTime::<chrono::Utc>::from(modified)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/system/backups", tag = "system", responses((status = 201, body = BackupItem)))]
pub async fn create_backup(
    State(state): State<AppState>,
    _p: Principal,
) -> ApiResult<(StatusCode, Json<BackupItem>)> {
    let dest = maintenance::unique_backup_path(&state.config.backup_dir(), "manual");
    let path = maintenance::backup_to(&state.db, &dest).await?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok((
        StatusCode::CREATED,
        Json(BackupItem {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size,
            created_at: kitsunarr_db::now(),
        }),
    ))
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogQuery {
    /// Minimum level: error, warn, info (default), debug or trace.
    pub level: Option<String>,
    /// Maximum number of entries (default 500).
    pub limit: Option<usize>,
}

/// Recent log entries kept in memory, newest first.
#[utoipa::path(get, path = "/system/logs", tag = "system", params(LogQuery), responses((status = 200, body = Vec<LogEntry>)))]
pub async fn logs(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<LogQuery>,
) -> Json<Vec<LogEntry>> {
    let level = crate::logs::parse_level(q.level.as_deref());
    Json(state.logs.snapshot(level, q.limit.unwrap_or(500).min(2000)))
}
