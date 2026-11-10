//! Migration wizard endpoints (Sonarr / Tanukarr → Kitsunarr).
//!
//! A *session* holds a read-only snapshot of one source. Loading and applying run as
//! background tasks reporting progress over SSE (`migration` events); planning is a
//! synchronous dry run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use futures::StreamExt;
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::migration::{
    ApiSourceRequest, EntityCount, MigrationOptions, MigrationPreview, MigrationReport,
    MigrationRunItem, MigrationSession, MigrationWarning, SessionStatus, SourceApp, SourceMethod,
};
use kitsunarr_migrate::{LoadedSource, MigrateError, Mode};
use tokio::io::AsyncWriteExt;

use crate::AppState;
use crate::error::{ApiError, ApiResult};

const SESSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);

struct Session {
    created: Instant,
    status: SessionStatus,
    source: Option<Arc<LoadedSource>>,
    progress: Option<String>,
    error: Option<String>,
    run_id: Option<i64>,
}

impl Session {
    fn to_dto(&self, id: &str) -> MigrationSession {
        let (source, inventory, roots) = match &self.source {
            Some(s) => (
                Some(s.detected.clone()),
                s.snapshot
                    .inventory()
                    .into_iter()
                    .map(|(entity, count)| EntityCount { entity, count })
                    .collect(),
                s.snapshot.root_paths(),
            ),
            None => (None, Vec::new(), Vec::new()),
        };
        MigrationSession {
            id: id.to_string(),
            status: self.status,
            source,
            inventory,
            source_root_folders: roots,
            progress_message: self.progress.clone(),
            error: self.error.clone(),
            run_id: self.run_id,
        }
    }
}

/// In-memory migration sessions (snapshots are discarded after six hours).
#[derive(Clone, Default)]
pub struct MigrationSessions(Arc<Mutex<HashMap<String, Session>>>);

impl MigrationSessions {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn create(&self) -> String {
        let id = kitsunarr_auth::generate_api_key();
        let mut map = self.lock();
        map.retain(|_, s| s.created.elapsed() < SESSION_TTL || s.status == SessionStatus::Applying);
        map.insert(
            id.clone(),
            Session {
                created: Instant::now(),
                status: SessionStatus::Loading,
                source: None,
                progress: Some("Starting".into()),
                error: None,
                run_id: None,
            },
        );
        id
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Session)) {
        if let Some(s) = self.lock().get_mut(id) {
            f(s);
        }
    }

    fn dto(&self, id: &str) -> Option<MigrationSession> {
        self.lock().get(id).map(|s| s.to_dto(id))
    }

    fn ready_source(&self, id: &str) -> ApiResult<Arc<LoadedSource>> {
        let map = self.lock();
        let s = map
            .get(id)
            .ok_or_else(|| ApiError::NotFound("Migration session not found or expired.".into()))?;
        match (&s.status, &s.source) {
            (SessionStatus::Applying, _) => Err(ApiError::Conflict(
                "This migration is being applied.".into(),
            )),
            (_, Some(src)) => Ok(src.clone()),
            (SessionStatus::Failed, None) => Err(ApiError::Conflict(
                s.error
                    .clone()
                    .unwrap_or_else(|| "Loading the source failed.".into()),
            )),
            _ => Err(ApiError::Conflict("The source is still being read.".into())),
        }
    }
}

fn progress_event(state: &AppState, id: &str, stage: &str, done: u64, total: u64, message: String) {
    state
        .migrations
        .update(id, |s| s.progress = Some(message.clone()));
    state.events.publish(ServerEvent::Migration {
        session_id: id.into(),
        stage: stage.into(),
        done,
        total,
        message,
    });
}

fn migrate_error(e: MigrateError) -> ApiError {
    match e {
        MigrateError::InvalidSource(m)
        | MigrateError::InvalidBackup(m)
        | MigrateError::InvalidOptions(m) => ApiError::BadRequest(m),
        MigrateError::Unreachable(m) => ApiError::BadGateway(m),
        MigrateError::ValidationFailed { reason, checks } => {
            let failed: Vec<String> = checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| format!("{} (expected {}, got {})", c.name, c.expected, c.actual))
                .collect();
            ApiError::Conflict(if failed.is_empty() {
                reason
            } else {
                format!("{reason}: {}", failed.join("; "))
            })
        }
        other => ApiError::Internal(other.to_string()),
    }
}

fn user_agent(state: &AppState) -> String {
    format!("Kitsunarr/{} (migration)", state.build.version)
}

/// Starts reading a running Sonarr/Tanukarr instance (`/api/v3`).
#[utoipa::path(post, path = "/migration/sessions", tag = "migration", request_body = ApiSourceRequest,
    responses((status = 202, body = MigrationSession)))]
pub async fn create_api_session(
    State(state): State<AppState>,
    Json(req): Json<ApiSourceRequest>,
) -> ApiResult<(StatusCode, Json<MigrationSession>)> {
    if req.url.trim().is_empty() {
        return Err(ApiError::field(
            "url",
            "Enter the address of the Sonarr or Tanukarr instance.",
        ));
    }
    let id = state.migrations.create();
    let (task_state, task_id) = (state.clone(), id.clone());
    tokio::spawn(async move {
        let ua = user_agent(&task_state);
        let progress = {
            let (s, i) = (task_state.clone(), task_id.clone());
            move |msg: String, done: u64, total: u64| {
                progress_event(&s, &i, "load", done, total, msg)
            }
        };
        let result = kitsunarr_migrate::load_from_api(&req, &ua, &progress).await;
        finish_loading(&task_state, &task_id, result);
    });
    let dto = state
        .migrations
        .dto(&id)
        .ok_or_else(|| ApiError::internal("session vanished"))?;
    Ok((StatusCode::ACCEPTED, Json(dto)))
}

fn finish_loading(state: &AppState, id: &str, result: Result<LoadedSource, MigrateError>) {
    match result {
        Ok(src) => {
            tracing::info!(
                app = src.detected.app.as_str(),
                series = src.snapshot.series.len(),
                "migration source loaded"
            );
            state.migrations.update(id, |s| {
                s.status = SessionStatus::Ready;
                s.progress = Some("Source read".into());
                s.source = Some(Arc::new(src));
            });
            progress_event(state, id, "ready", 1, 1, "Source read".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "migration source could not be read");
            let message = e.to_string();
            state.migrations.update(id, |s| {
                s.status = SessionStatus::Failed;
                s.error = Some(message.clone());
            });
            progress_event(state, id, "failed", 1, 1, message);
        }
    }
}

/// Uploads a Sonarr/Tanukarr backup (`.zip` or `sonarr.db`) as the raw request body.
#[utoipa::path(post, path = "/migration/sessions/backup", tag = "migration",
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "Backup archive or database file"),
    responses((status = 202, body = MigrationSession), (status = 413)))]
pub async fn upload_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<MigrationSession>)> {
    let limit = state.config.server.max_backup_upload_bytes;
    if let Some(len) = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        && len > limit
    {
        return Err(ApiError::PayloadTooLarge(format!(
            "The backup is larger than the {} MiB limit.",
            limit / 1024 / 1024
        )));
    }
    let work = state.config.data_dir.join("tmp").join("migration");
    tokio::fs::create_dir_all(&work)
        .await
        .map_err(ApiError::internal)?;
    let upload = tempfile::Builder::new()
        .prefix("upload-")
        .tempfile_in(&work)
        .map_err(ApiError::internal)?;
    let (std_file, upload_path) = upload.into_parts();
    let mut file = tokio::fs::File::from_std(std_file);
    let mut stream = body.into_data_stream();
    let mut written = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::BadRequest(format!("Upload interrupted: {e}")))?;
        written += chunk.len();
        if written > limit {
            return Err(ApiError::PayloadTooLarge(format!(
                "The backup is larger than the {} MiB limit.",
                limit / 1024 / 1024
            )));
        }
        file.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    file.flush().await.map_err(ApiError::internal)?;
    drop(file);
    if written == 0 {
        return Err(ApiError::BadRequest("The uploaded file is empty.".into()));
    }

    let id = state.migrations.create();
    let (task_state, task_id) = (state.clone(), id.clone());
    tokio::spawn(async move {
        progress_event(
            &task_state,
            &task_id,
            "load",
            0,
            1,
            "Reading the backup".into(),
        );
        let result = kitsunarr_migrate::load_from_backup(&upload_path, &work).await;
        drop(upload_path); // deletes the uploaded copy
        finish_loading(&task_state, &task_id, result);
    });
    let dto = state
        .migrations
        .dto(&id)
        .ok_or_else(|| ApiError::internal("session vanished"))?;
    Ok((StatusCode::ACCEPTED, Json(dto)))
}

#[utoipa::path(get, path = "/migration/sessions/{id}", tag = "migration", params(("id" = String, Path)),
    responses((status = 200, body = MigrationSession), (status = 404)))]
pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<MigrationSession>> {
    state
        .migrations
        .dto(&id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Migration session not found or expired.".into()))
}

#[utoipa::path(delete, path = "/migration/sessions/{id}", tag = "migration", params(("id" = String, Path)), responses((status = 204)))]
pub async fn discard_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let mut map = state.migrations.lock();
    match map.get(&id) {
        Some(s) if s.status == SessionStatus::Applying => Err(ApiError::Conflict(
            "This migration is being applied.".into(),
        )),
        Some(_) => {
            map.remove(&id);
            Ok(StatusCode::NO_CONTENT)
        }
        None => Err(ApiError::NotFound(
            "Migration session not found or expired.".into(),
        )),
    }
}

/// Dry run: performs the whole import in a transaction that is rolled back.
#[utoipa::path(post, path = "/migration/sessions/{id}/plan", tag = "migration", params(("id" = String, Path)),
    request_body = MigrationOptions, responses((status = 200, body = MigrationPreview)))]
pub async fn plan(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(options): Json<MigrationOptions>,
) -> ApiResult<Json<MigrationPreview>> {
    let source = state.migrations.ready_source(&id)?;
    let progress = {
        let (s, i) = (state.clone(), id.clone());
        move |stage: &str, done: u64, total: u64| {
            progress_event(&s, &i, stage, done, total, format!("Planning: {stage}"))
        }
    };
    let outcome = kitsunarr_migrate::run(&state.db, &source, &options, Mode::DryRun, &progress)
        .await
        .map_err(migrate_error)?;
    Ok(Json(MigrationPreview {
        session_id: id,
        source: source.detected.clone(),
        summary: outcome.summary,
        warnings: outcome.warnings,
        paths: outcome.paths,
        blocking: outcome.blocking,
    }))
}

/// Applies the migration in the background; poll the session or listen for `migration` events.
#[utoipa::path(post, path = "/migration/sessions/{id}/apply", tag = "migration", params(("id" = String, Path)),
    request_body = MigrationOptions, responses((status = 202, body = MigrationSession), (status = 409)))]
pub async fn apply(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(options): Json<MigrationOptions>,
) -> ApiResult<(StatusCode, Json<MigrationSession>)> {
    let source = state.migrations.ready_source(&id)?;
    state.migrations.update(&id, |s| {
        s.status = SessionStatus::Applying;
        s.error = None;
        s.progress = Some("Backing up the Kitsunarr database".into());
    });
    let (task_state, task_id) = (state.clone(), id.clone());
    tokio::spawn(async move {
        let progress = {
            let (s, i) = (task_state.clone(), task_id.clone());
            move |stage: &str, done: u64, total: u64| {
                progress_event(&s, &i, stage, done, total, format!("Importing: {stage}"))
            }
        };
        let mode = Mode::Apply {
            backup_dir: task_state.config.backup_dir(),
        };
        match kitsunarr_migrate::run(&task_state.db, &source, &options, mode, &progress).await {
            Ok(outcome) => {
                task_state.migrations.update(&task_id, |s| {
                    s.status = SessionStatus::Completed;
                    s.run_id = outcome.run_id;
                    s.progress = Some("Migration completed".into());
                });
                progress_event(
                    &task_state,
                    &task_id,
                    "completed",
                    1,
                    1,
                    "Migration completed".into(),
                );
                task_state.events.publish(ServerEvent::LibraryChanged {
                    media_type: "all".into(),
                    ids: vec![],
                    deleted: false,
                });
                task_state.events.publish(ServerEvent::SettingsChanged {
                    section: "all".into(),
                });
                // The migrated RSS interval schedules RSS sync.
                if let Err(e) = crate::rss::sync_interval(&task_state.db).await {
                    tracing::warn!(error = %e, "could not apply the RSS sync interval");
                }
            }
            Err(e) => {
                let message = match migrate_error(e) {
                    ApiError::Internal(m) => format!("Migration failed: {m}"),
                    other => other.to_string(),
                };
                tracing::warn!(%message, "migration failed; nothing was committed");
                task_state.migrations.update(&task_id, |s| {
                    s.status = SessionStatus::Ready;
                    s.error = Some(message.clone());
                });
                progress_event(&task_state, &task_id, "failed", 1, 1, message);
            }
        }
    });
    let dto = state
        .migrations
        .dto(&id)
        .ok_or_else(|| ApiError::internal("session vanished"))?;
    Ok((StatusCode::ACCEPTED, Json(dto)))
}

fn run_item(r: &kitsunarr_db::migration_runs::MigrationRun, warnings: u64) -> MigrationRunItem {
    MigrationRunItem {
        id: r.id,
        source_app: if r.source_app == "tanukarr" {
            SourceApp::Tanukarr
        } else {
            SourceApp::Sonarr
        },
        source_method: if r.source_method == "backup" {
            SourceMethod::Backup
        } else {
            SourceMethod::Api
        },
        source_version: r.source_version.clone(),
        source_instance: r.source_instance.clone(),
        status: r.status.clone(),
        started_at: r.started_at.clone(),
        finished_at: r.finished_at.clone(),
        summary: r
            .summary
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default(),
        warning_count: warnings,
        backup_path: r.backup_path.clone(),
    }
}

fn report_parts(
    r: &kitsunarr_db::migration_runs::MigrationRun,
) -> (
    Vec<MigrationWarning>,
    Vec<kitsunarr_domain::api::migration::ValidationCheck>,
) {
    let report: serde_json::Value = r
        .report
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    (
        serde_json::from_value(report.get("warnings").cloned().unwrap_or_default())
            .unwrap_or_default(),
        serde_json::from_value(report.get("validation").cloned().unwrap_or_default())
            .unwrap_or_default(),
    )
}

#[utoipa::path(get, path = "/migration/runs", tag = "migration", responses((status = 200, body = Vec<MigrationRunItem>)))]
pub async fn runs(State(state): State<AppState>) -> ApiResult<Json<Vec<MigrationRunItem>>> {
    let runs = kitsunarr_db::migration_runs::list(state.db.pool()).await?;
    Ok(Json(
        runs.iter()
            .map(|r| run_item(r, report_parts(r).0.len() as u64))
            .collect(),
    ))
}

#[utoipa::path(get, path = "/migration/runs/{id}", tag = "migration", params(("id" = i64, Path)),
    responses((status = 200, body = MigrationReport), (status = 404)))]
pub async fn run_report(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<MigrationReport>> {
    let run = kitsunarr_db::migration_runs::get(state.db.pool(), id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Migration run not found.".into()))?;
    let (mut warnings, validation) = report_parts(&run);
    if let Some(error) = &run.error {
        warnings.insert(
            0,
            MigrationWarning {
                severity: kitsunarr_domain::api::migration::WarningSeverity::Error,
                entity: "run".into(),
                source_id: None,
                title: None,
                message: error.clone(),
            },
        );
    }
    let unmapped = kitsunarr_db::migration_runs::unmapped_count(state.db.pool(), id).await?;
    if unmapped > 0 {
        warnings.push(MigrationWarning {
            severity: kitsunarr_domain::api::migration::WarningSeverity::Info,
            entity: "report".into(),
            source_id: None,
            title: None,
            message: format!("{unmapped} source values without a Kitsunarr equivalent were preserved in the migration record."),
        });
    }
    Ok(Json(MigrationReport {
        run: run_item(&run, warnings.len() as u64),
        warnings,
        validation,
    }))
}
