//! Sonarr's system-level endpoints, served alike under `/api/v3` and `/api/v5` (`V5` picks
//! the version's status codes): the file browser (`filesystem`), log files, the interface
//! strings (`localization`), series artwork (`mediacover`), backup deletion and restore,
//! shutdown and restart, the route list and the update list.

use std::path::{Path as FsPath, PathBuf};
use std::sync::OnceLock;

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

/// The routes both API versions serve.
pub fn shared<const V5: bool>() -> Router<AppState> {
    Router::new()
        .route("/filesystem", get(filesystem))
        .route("/filesystem/type", get(filesystem_type))
        .route("/filesystem/mediafiles", get(media_files))
        .route("/log/file", get(log_files))
        .route("/log/file/{filename}", get(log_file))
        .route("/log/file/update", get(update_log_files))
        .route("/log/file/update/{filename}", get(update_log_file))
        .route("/localization", get(localization))
        .route("/localization/language", get(localization_language))
        .route("/localization/{id}", get(localization))
        .route("/mediacover/{series_id}/{filename}", get(media_cover))
        .route("/system/backup/{id}", delete(delete_backup::<V5>))
        .route("/system/backup/restore/{id}", post(restore_backup))
        .route(
            "/system/backup/restore/upload",
            post(restore_upload::<V5>).layer(DefaultBodyLimit::disable()),
        )
        .route("/system/restart", post(restart))
        .route("/system/shutdown", post(shutdown))
        .route("/system/routes", get(routes))
        .route("/system/routes/duplicate", get(duplicate_routes))
        .route("/update", get(updates))
}

/// `/api/v3`.
pub fn router() -> Router<AppState> {
    shared::<false>()
}

// ---- file system ----------------------------------------------------------------------------

/// Sonarr's `SpecialFolders`: system folders the browser hides.
const SPECIAL_FOLDERS: &[&str] = &[
    "boot",
    "bootmgr",
    "cache",
    "msocache",
    "recovery",
    "$recycle.bin",
    "recycler",
    "system volume information",
    "temporary internet files",
    "windows",
    ".fseventd",
    ".spotlight",
    ".trashes",
    ".vol",
    "cachedmessages",
    "caches",
    "trash",
    ".@__thumb",
    "@eadir",
    "#recycle",
];

const SEP: char = std::path::MAIN_SEPARATOR;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct FsQuery {
    path: Option<String>,
    include_files: bool,
    allow_folders_without_trailing_slashes: bool,
}

fn modified(meta: &std::fs::Metadata) -> Value {
    meta.modified()
        .ok()
        .map(|t| {
            json!(
                chrono::DateTime::<chrono::Utc>::from(t)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            )
        })
        .unwrap_or(Value::Null)
}

fn with_separator(mut p: String) -> String {
    if !p.ends_with(SEP) {
        p.push(SEP);
    }
    p
}

/// `DirectoryInfo.Parent` (with a trailing separator); `None` at the root.
fn parent_of(path: &str) -> Value {
    let trimmed = path.trim_end_matches(SEP);
    match FsPath::new(if trimmed.is_empty() { path } else { trimmed }).parent() {
        Some(p) => json!(with_separator(p.display().to_string())),
        None if path == "/" => Value::Null,
        None => json!(""),
    }
}

/// Sonarr's `FileSystemLookupService.LookupContents`.
pub(crate) fn lookup_contents(query: &str, include_files: bool, without_slash: bool) -> Value {
    let empty = || json!({"parent": null, "directories": [], "files": []});
    let mut query = query.to_string();
    if query.trim().is_empty() {
        if cfg!(windows) {
            let drives: Vec<Value> = ('A'..='Z')
                .map(|l| format!("{l}:\\"))
                .filter(|d| FsPath::new(d).is_dir())
                .map(|d| json!({"type": "drive", "name": d.trim_end_matches('\\'), "path": d, "lastModified": null, "size": 0}))
                .collect();
            return json!({"parent": null, "directories": drives, "files": []});
        }
        query = "/".into();
    }
    if without_slash && FsPath::new(&query).is_dir() {
        return result(&query, include_files);
    }
    match query.rfind(SEP) {
        Some(i) => result(&query[..=i], include_files),
        None => empty(),
    }
}

fn result(path: &str, include_files: bool) -> Value {
    let parent = parent_of(path);
    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => return json!({"parent": parent, "directories": [], "files": []}),
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::metadata(e.path()) else {
            continue;
        };
        let full = e.path().display().to_string();
        if meta.is_dir() {
            if SPECIAL_FOLDERS.contains(&name.to_lowercase().as_str()) {
                continue;
            }
            dirs.push((
                name.clone(),
                json!({"type": "folder", "name": name, "path": with_separator(full), "lastModified": modified(&meta), "size": 0}),
            ));
        } else if include_files && meta.is_file() {
            let extension = FsPath::new(&name)
                .extension()
                .map(|x| format!(".{}", x.to_string_lossy()))
                .unwrap_or_default();
            files.push((
                name.clone(),
                json!({"type": "file", "name": name, "path": full, "extension": extension, "size": meta.len(), "lastModified": modified(&meta)}),
            ));
        }
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    json!({
        "parent": parent,
        "directories": dirs.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "files": files.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
    })
}

async fn filesystem(Query(q): Query<FsQuery>) -> Json<Value> {
    let path = q.path.unwrap_or_default();
    Json(
        tokio::task::spawn_blocking(move || {
            lookup_contents(
                &path,
                q.include_files,
                q.allow_folders_without_trailing_slashes,
            )
        })
        .await
        .unwrap_or_else(|_| json!({"parent": null, "directories": [], "files": []})),
    )
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PathQuery {
    path: String,
}

/// `file` for an existing file, else `folder` (Sonarr does not reveal whether it exists).
async fn filesystem_type(Query(q): Query<PathQuery>) -> Json<Value> {
    let file = !q.path.is_empty() && FsPath::new(&q.path).is_file();
    Json(json!({"type": if file { "file" } else { "folder" }}))
}

fn video_files(dir: &FsPath, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            video_files(&p, out);
        } else if p.extension().is_some_and(|x| {
            kitsunarr_parser::extensions::is_video_extension(&format!(".{}", x.to_string_lossy()))
        }) {
            out.push(p);
        }
    }
}

/// The video files below a folder (Sonarr's `DiskScanService.GetVideoFiles`).
async fn media_files(Query(q): Query<PathQuery>) -> Json<Vec<Value>> {
    let root = PathBuf::from(&q.path);
    Json(
        tokio::task::spawn_blocking(move || {
            if q.path.is_empty() || !root.is_dir() {
                return Vec::new();
            }
            let mut files = Vec::new();
            video_files(&root, &mut files);
            files.sort();
            files
                .into_iter()
                .map(|f| {
                    let relative = f
                        .strip_prefix(&root)
                        .map(|r| r.display().to_string())
                        .unwrap_or_default();
                    json!({
                        "path": f.display().to_string(),
                        "relativePath": relative,
                        "name": f.file_name().map(|n| n.to_string_lossy().into_owned()),
                    })
                })
                .collect()
        })
        .await
        .unwrap_or_default(),
    )
}

// ---- log files ----------------------------------------------------------------------------

fn log_list(state: &AppState, dir: &FsPath, resource: &str) -> Vec<Value> {
    let base = &state.config.server.url_base;
    crate::log_files::list(dir)
        .into_iter()
        .enumerate()
        .map(|(i, (name, time))| {
            let contents = format!("{base}/api/v3/log/file/{resource}{name}");
            json!({
                "id": i + 1,
                "filename": name,
                "lastWriteTime": chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "contentsUrl": contents,
                "downloadUrl": contents,
            })
        })
        .collect()
}

fn log_dir(state: &AppState) -> PathBuf {
    crate::log_files::folder(&state.config.data_dir)
}

/// Kitsunarr has no updater, so no update log folder exists.
fn update_log_dir(state: &AppState) -> PathBuf {
    log_dir(state).join("UpdateLogs")
}

async fn log_files(State(state): State<AppState>) -> Json<Vec<Value>> {
    Json(log_list(&state, &log_dir(&state), ""))
}

async fn update_log_files(State(state): State<AppState>) -> Json<Vec<Value>> {
    Json(log_list(&state, &update_log_dir(&state), "update/"))
}

async fn read_log(dir: PathBuf, name: &str) -> Response {
    if !crate::log_files::valid_name(name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(dir.join(name)).await {
        Ok(data) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], data).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn log_file(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    read_log(log_dir(&state), &name).await
}

async fn update_log_file(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    read_log(update_log_dir(&state), &name).await
}

// ---- localization --------------------------------------------------------------------------

macro_rules! strings {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../localization/", $name, ".json")))),*]
    };
}

/// Tanukarr's interface strings, by file name (see COPYRIGHT.md).
const STRINGS: &[(&str, &str)] = strings!(
    "ar", "bg", "bn", "bs", "ca", "cs", "da", "de", "el", "en", "es", "es_MX", "et", "fa", "fi",
    "fr", "he", "hi", "hr", "hu", "id", "is", "it", "ja", "ko", "lt", "lv", "nb_NO", "nl", "pl",
    "pt", "pt_BR", "ro", "ru", "sk", "sr", "sv", "ta", "th", "tr", "uk", "vi", "zh_CN", "zh_Hans",
    "zh_TW",
);

fn strings_of(file: &str) -> Option<Map<String, Value>> {
    STRINGS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(file))
        .and_then(|(_, s)| serde_json::from_str(s).ok())
}

/// Sonarr's `LocalizationService.GetDictionary`: English, then the base language, then the
/// language itself; keys compare without regard to case.
pub(crate) fn localization_strings(code: &str) -> Map<String, Value> {
    let mut out: Map<String, Value> = Map::new();
    let mut merge = |m: Option<Map<String, Value>>| {
        for (k, v) in m.into_iter().flatten() {
            if let Some(existing) = out.keys().find(|e| e.eq_ignore_ascii_case(&k)).cloned() {
                out.remove(&existing);
            }
            out.insert(k, v);
        }
    };
    merge(strings_of("en"));
    let file = code.replace('-', "_");
    if let Some((base, _)) = file.split_once('_') {
        merge(strings_of(base));
    }
    if !file.eq_ignore_ascii_case("en") {
        merge(strings_of(&file));
    }
    out
}

async fn ui_language(state: &AppState) -> String {
    kitsunarr_db::settings::get_or_default::<kitsunarr_domain::api::settings::GeneralSettings>(
        state.db.pool(),
        kitsunarr_db::settings::KEY_GENERAL,
    )
    .await
    .map(|g| g.ui_language)
    .unwrap_or_else(|_| "en".into())
}

/// Sonarr's language identifier (`pt-BR`): the interface language of Kitsunarr's settings.
pub(crate) fn language_identifier(code: &str) -> String {
    match code.split_once('_') {
        Some((l, c)) => format!("{}-{}", l.to_lowercase(), c.to_uppercase()),
        None if code.is_empty() => "en".into(),
        None => code.to_lowercase(),
    }
}

async fn localization(State(state): State<AppState>) -> Json<Value> {
    let code = ui_language(&state).await;
    Json(json!({"id": 0, "strings": localization_strings(&code)}))
}

async fn localization_language(State(state): State<AppState>) -> Json<Value> {
    let code = ui_language(&state).await;
    Json(json!({"identifier": language_identifier(&code)}))
}

// ---- media covers -------------------------------------------------------------------------

/// `GET /mediacover/{seriesId}/{poster|fanart|banner…}[-size].ext`: Kitsunarr keeps no local
/// artwork cache, so this redirects to the catalogue's image.
async fn media_cover(
    State(state): State<AppState>,
    Path((series_id, filename)): Path<(i64, String)>,
) -> ApiResult<Response> {
    let detail = kitsunarr_db::library::series::get_detail(&state.db, series_id)
        .await
        .map_err(|_| ApiError::NotFound("The series does not exist.".into()))?;
    let stem = filename.split(['.', '-']).next().unwrap_or_default();
    detail
        .images
        .iter()
        .find(|i| i.cover_type.eq_ignore_ascii_case(stem) && !i.remote_url.is_empty())
        .map(|i| Redirect::temporary(&i.remote_url).into_response())
        .ok_or_else(|| ApiError::NotFound("No such image.".into()))
}

// ---- backups --------------------------------------------------------------------------------

/// Sonarr's backup type, from Kitsunarr's file name.
pub(crate) fn backup_type(name: &str) -> &'static str {
    if name.contains("_manual") {
        "manual"
    } else if name.contains("update") {
        "update"
    } else {
        "scheduled"
    }
}

/// Sonarr's backup id: `HashConverter.GetHashInt31("backup-{Type}-{name}")`.
pub(crate) fn backup_id(name: &str) -> i64 {
    let kind = match backup_type(name) {
        "manual" => "Manual",
        "update" => "Update",
        _ => "Scheduled",
    };
    let hash = Sha1::digest(format!("backup-{kind}-{name}").as_bytes());
    i64::from(i32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]]) & 0x7fff_ffff)
}

/// Sonarr's `BackupResource`s, newest first.
pub(crate) fn backups_json(state: &AppState) -> ApiResult<Vec<Value>> {
    let list = kitsunarr_db::maintenance::list_backups(&state.config.backup_dir())
        .map_err(ApiError::internal)?;
    Ok(list
        .into_iter()
        .map(|(name, size, time)| {
            let kind = backup_type(&name);
            json!({
                "id": backup_id(&name),
                "name": name,
                "path": format!("/backup/{kind}/{name}"),
                "type": kind,
                "size": size,
                "time": chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            })
        })
        .collect())
}

fn backup_path(state: &AppState, id: i64) -> ApiResult<PathBuf> {
    let list = kitsunarr_db::maintenance::list_backups(&state.config.backup_dir())
        .map_err(ApiError::internal)?;
    list.into_iter()
        .find(|(name, _, _)| backup_id(name) == id)
        .map(|(name, _, _)| state.config.backup_dir().join(name))
        .ok_or_else(|| ApiError::NotFound("The backup does not exist.".into()))
}

async fn delete_backup<const V5: bool>(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let path = backup_path(&state, id)?;
    tokio::fs::remove_file(&path)
        .await
        .map_err(|_| ApiError::NotFound("The backup does not exist.".into()))?;
    Ok(if V5 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Json(json!({})).into_response()
    })
}

/// Checks that a file is a usable Kitsunarr database (opening a scratch copy, which applies
/// pending migrations, and checking its integrity), then stages it for the next start.
pub(crate) async fn stage_restore(state: &AppState, file: &FsPath) -> ApiResult<()> {
    let scratch = tempfile::tempdir().map_err(ApiError::internal)?;
    let probe = scratch.path().join("probe.db");
    tokio::fs::copy(file, &probe)
        .await
        .map_err(|e| ApiError::BadRequest(format!("The backup cannot be read: {e}")))?;
    let db = kitsunarr_db::Database::open(&probe)
        .await
        .map_err(|_| ApiError::BadRequest("The file is not a usable Kitsunarr backup.".into()))?;
    let report = kitsunarr_db::maintenance::integrity_check(&db).await;
    db.close().await;
    if !report.is_ok_and(|r| r.ok()) {
        return Err(ApiError::BadRequest(
            "The backup failed its integrity check.".into(),
        ));
    }
    let staged = kitsunarr_db::maintenance::staged_restore_path(&state.config.data_dir);
    if let Some(dir) = staged.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(ApiError::internal)?;
    }
    tokio::fs::copy(file, &staged)
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(backup = %file.display(), "backup staged; it is restored at the next start");
    Ok(())
}

async fn restore_backup(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let path = backup_path(&state, id)?;
    stage_restore(&state, &path).await?;
    Ok(Json(json!({"restartRequired": true})))
}

/// Sonarr accepts `.zip`, `.db` and `.xml`; Kitsunarr's backups are databases.
const RESTORE_EXTENSIONS: &[&str] = &[".db"];

async fn restore_upload<const V5: bool>(
    State(state): State<AppState>,
    mut form: Multipart,
) -> ApiResult<Response> {
    let field = loop {
        match form
            .next_field()
            .await
            .map_err(|e| ApiError::BadRequest(e.to_string()))?
        {
            Some(f) if f.file_name().is_some() => break f,
            Some(_) => continue,
            None => return Err(ApiError::BadRequest("file must be provided".into())),
        }
    };
    let name = field.file_name().unwrap_or_default().to_string();
    let extension = FsPath::new(&name)
        .extension()
        .map(|x| format!(".{}", x.to_string_lossy()))
        .unwrap_or_default();
    if !RESTORE_EXTENSIONS.contains(&extension.as_str()) {
        let error = format!(
            "Invalid extension, must be one of: {}",
            RESTORE_EXTENSIONS.join(", ")
        );
        return Ok(if V5 {
            (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response()
        } else {
            (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                Json(json!({"message": error})),
            )
                .into_response()
        });
    }
    let limit = state.config.server.max_backup_upload_bytes;
    let tmp = state.config.data_dir.join("tmp");
    tokio::fs::create_dir_all(&tmp)
        .await
        .map_err(ApiError::internal)?;
    let upload = tempfile::Builder::new()
        .prefix("restore-")
        .suffix(".db")
        .tempfile_in(&tmp)
        .map_err(ApiError::internal)?;
    let mut field = field;
    let mut data = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Upload interrupted: {e}")))?
    {
        data.extend_from_slice(&chunk);
        if data.len() > limit {
            return Err(ApiError::PayloadTooLarge(format!(
                "The backup is larger than the {} MiB limit.",
                limit / 1024 / 1024
            )));
        }
    }
    tokio::fs::write(upload.path(), &data)
        .await
        .map_err(ApiError::internal)?;
    stage_restore(&state, upload.path()).await?;
    Ok(Json(json!({"restartRequired": true})).into_response())
}

// ---- lifecycle ----------------------------------------------------------------------------

async fn restart(State(state): State<AppState>) -> Json<Value> {
    state.lifecycle.restart();
    Json(json!({"restarting": true}))
}

async fn shutdown(State(state): State<AppState>) -> Json<Value> {
    state.lifecycle.shutdown();
    Json(json!({"shuttingDown": true}))
}

// ---- routes ---------------------------------------------------------------------------------

/// The Sonarr-compatible routers' sources, read for the route list.
const ROUTER_SOURCES: &[(&str, &str)] = &[
    ("/api/v3", include_str!("v3.rs")),
    ("/api/v3", include_str!("v3_more.rs")),
    ("/api/v3", include_str!("v3_settings.rs")),
    ("/api/v3", include_str!("v3_providers.rs")),
    ("/api/v3", include_str!("v3_config.rs")),
    ("/api/v3", include_str!("v3_admin.rs")),
    ("/api/v3", include_str!("v3_extra.rs")),
    ("/api/v3", include_str!("v3_system.rs")),
    ("/api/v5", include_str!("v5.rs")),
    ("/api/v5", include_str!("v5_settings.rs")),
    ("", include_str!("root.rs")),
    ("/feed/v3", include_str!("ical.rs")),
    ("/feed/v5", include_str!("ical.rs")),
];

/// `(METHOD, path)` pairs of the `.route("…", get(…).post(…))` calls in a router's source.
pub(crate) fn parse_routes(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(i) = rest.find(".route(") {
        rest = &rest[i + ".route(".len()..];
        let trimmed = rest.trim_start();
        let Some(path) = trimmed
            .strip_prefix('"')
            .and_then(|r| r.split_once('"'))
            .map(|(p, _)| p)
        else {
            continue;
        };
        // The method router: up to the call's closing parenthesis.
        let mut depth = 1;
        let mut end = 0;
        for (j, c) in rest.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = j;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &rest[..end];
        for m in ["get", "post", "put", "delete", "patch", "head"] {
            let call = format!("{m}(");
            if body.match_indices(&call).any(|(k, _)| {
                k == 0
                    || !body[..k]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
            }) {
                out.push((m.to_uppercase(), path.to_string()));
            }
        }
    }
    out
}

fn compat_routes() -> &'static [String] {
    static ROUTES: OnceLock<Vec<String>> = OnceLock::new();
    ROUTES.get_or_init(|| {
        let mut lines: Vec<String> = Vec::new();
        for (prefix, source) in ROUTER_SOURCES {
            let source = source.split("#[cfg(test)]").next().unwrap_or_default();
            for (m, p) in parse_routes(source)
                .into_iter()
                .filter(|(_, p)| p.starts_with('/'))
            {
                lines.push(format!("{m} {prefix}{p}"));
            }
        }
        // The provider extras are registered per resource name (`v3_ext::router`).
        for (prefix, names) in [
            (
                "/api/v3",
                ["indexer", "downloadclient", "importlist", "notification"],
            ),
            (
                "/api/v5",
                ["indexer", "downloadclient", "importlist", "connection"],
            ),
        ] {
            for (i, n) in names.iter().enumerate() {
                if i < 3 {
                    lines.push(format!("PUT {prefix}/{n}/bulk"));
                    lines.push(format!("DELETE {prefix}/{n}/bulk"));
                }
                lines.push(format!("POST {prefix}/{n}/testall"));
                lines.push(format!("POST {prefix}/{n}/action/{{name}}"));
            }
            for (m, p) in parse_routes(
                include_str!("v3_ext.rs")
                    .split("#[cfg(test)]")
                    .next()
                    .unwrap_or_default(),
            ) {
                lines.push(format!("{m} {prefix}{p}"));
            }
        }
        lines.sort();
        lines.dedup();
        lines
    })
}

/// `GET /system/routes`: every route, one `METHOD path` per line (Sonarr writes its routing
/// graph here).
async fn routes(State(state): State<AppState>) -> Response {
    let (_, openapi) = crate::api_router(&state);
    let mut lines: Vec<String> = compat_routes().to_vec();
    for (path, item) in &openapi.paths.paths {
        for (m, op) in [
            ("GET", item.get.is_some()),
            ("POST", item.post.is_some()),
            ("PUT", item.put.is_some()),
            ("DELETE", item.delete.is_some()),
            ("PATCH", item.patch.is_some()),
        ] {
            if op {
                lines.push(format!("{m} {path}"));
            }
        }
    }
    lines.sort();
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        lines.join("\n"),
    )
        .into_response()
}

/// Axum refuses duplicate routes when the router is built, so there are none.
async fn duplicate_routes() -> Json<Value> {
    Json(json!({}))
}

// ---- updates --------------------------------------------------------------------------------

/// Kitsunarr has no in-app updater: the running version is the one installed.
async fn updates(State(state): State<AppState>) -> Json<Vec<Value>> {
    Json(vec![json!({
        "version": state.build.version,
        "branch": "main",
        "releaseDate": state.started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "fileName": "",
        "url": "https://github.com/KvasFan/Kitsunarr/releases",
        "installed": true,
        "installedOn": null,
        "installable": false,
        "latest": true,
        "changes": {"new": [], "fixed": []},
        "hash": "",
    })])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_browser_lists_folders_like_sonarr() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Shows")).unwrap();
        std::fs::create_dir(dir.path().join("$RECYCLE.BIN")).unwrap();
        std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();
        let base = with_separator(dir.path().display().to_string());
        let r = lookup_contents(&base, false, false);
        let dirs = r["directories"].as_array().unwrap();
        assert_eq!(dirs.len(), 1, "special folders are hidden: {r}");
        assert_eq!(dirs[0]["type"], "folder");
        assert_eq!(dirs[0]["path"], format!("{base}Shows{SEP}"));
        assert_eq!(r["files"].as_array().unwrap().len(), 0);
        assert_eq!(
            r["parent"],
            json!(with_separator(
                dir.path().parent().unwrap().display().to_string()
            ))
        );
        // Without a trailing separator the parent folder is listed, unless allowed.
        let partial = lookup_contents(&format!("{base}Sho"), true, false);
        assert_eq!(partial["files"][0]["name"], "a.mkv");
        assert_eq!(partial["files"][0]["extension"], ".mkv");
        let exact = lookup_contents(&format!("{base}Shows"), false, true);
        assert_eq!(exact["directories"].as_array().unwrap().len(), 0);
        if cfg!(unix) {
            assert!(lookup_contents("", false, false)["parent"].is_null());
        }
        assert_eq!(
            lookup_contents("nothing", false, false)["directories"],
            json!([])
        );
    }

    #[test]
    fn localization_merges_english_and_the_language() {
        let en = localization_strings("en");
        assert!(en.len() > 1000);
        let de = localization_strings("de");
        assert!(de.len() >= en.len());
        assert_ne!(de.get("Add"), en.get("Add"));
        let br = localization_strings("pt_BR");
        assert!(br.len() >= en.len());
        assert_eq!(language_identifier("pt_BR"), "pt-BR");
        assert_eq!(language_identifier("en"), "en");
    }

    #[test]
    fn backup_ids_follow_sonarr() {
        let a = backup_id("kitsunarr_2026-11-09_17-34-40-123_manual.db");
        assert!(a > 0);
        assert_eq!(a, backup_id("kitsunarr_2026-11-09_17-34-40-123_manual.db"));
        assert_ne!(
            a,
            backup_id("kitsunarr_2026-11-09_17-34-40-123_scheduled.db")
        );
        assert_eq!(backup_type("kitsunarr_x_manual.db"), "manual");
        assert_eq!(backup_type("kitsunarr_x_scheduled.db"), "scheduled");
    }

    #[test]
    fn route_sources_are_parsed() {
        let r = parse_routes(
            r#".route("/a", get(x).post(y)) .route(
                "/b/{id}",
                put(z).delete(remove_thing),
            )"#,
        );
        assert_eq!(
            r,
            vec![
                ("GET".to_string(), "/a".to_string()),
                ("POST".to_string(), "/a".to_string()),
                ("PUT".to_string(), "/b/{id}".to_string()),
                ("DELETE".to_string(), "/b/{id}".to_string()),
            ]
        );
        assert!(
            compat_routes()
                .iter()
                .any(|l| l == "GET /api/v3/series/{id}")
        );
        assert!(
            compat_routes()
                .iter()
                .any(|l| l == "POST /api/v5/connection/testall")
        );
    }
}
