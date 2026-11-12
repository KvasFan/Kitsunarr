//! Per-show compression: Tanukarr's `SeriesCompressionController`, a thin proxy to an optional
//! host worker that re-encodes a series' files (Tanukarr's `docker/compression/worker.py`).
//!
//! Kitsunarr never runs ffmpeg for this. Requests are forwarded as HTTP over the worker's Unix
//! socket (`KITSUNARR_COMPRESSION_SOCKET`, default `/compression/worker.sock`; the socket's file
//! permissions are the access control). Without the socket, the status says the worker is not
//! connected and starting or cancelling answers 503. While a job runs, kept versions cannot be
//! made current and recycled files cannot be restored into the series (`HasActiveJob`).

use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

const DEFAULT_SOCKET: &str = "/compression/worker.sock";
const TIMEOUT: Duration = Duration::from_secs(30);
/// The worker refuses larger bodies.
const MAX_BODY: usize = 4096;

fn socket() -> PathBuf {
    std::env::var("KITSUNARR_COMPRESSION_SOCKET")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| PathBuf::from(DEFAULT_SOCKET), PathBuf::from)
}

/// One HTTP/1.1 request over the worker's socket: `(status, JSON body)`, or `None` when the
/// worker cannot be reached.
async fn call(method: &str, path: &str, body: Option<&Value>) -> Option<(u16, Value)> {
    call_at(&socket(), method, path, body).await
}

#[cfg(unix)]
async fn call_at(
    socket: &std::path::Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Option<(u16, Value)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if !socket.exists() {
        return None;
    }
    let exchange = async {
        let mut stream = tokio::net::UnixStream::connect(socket).await.ok()?;
        let payload = body.map(Value::to_string).unwrap_or_default();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: compression\r\nConnection: close\r\nAccept: application/json\r\n"
        );
        if body.is_some() {
            request.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                payload.len()
            ));
        }
        request.push_str("\r\n");
        request.push_str(&payload);
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.ok()?;
        parse_response(&raw)
    };
    tokio::time::timeout(TIMEOUT, exchange).await.ok().flatten()
}

#[cfg(not(unix))]
async fn call_at(
    _socket: &std::path::Path,
    _method: &str,
    _path: &str,
    _body: Option<&Value>,
) -> Option<(u16, Value)> {
    None
}

/// The status and JSON body of a `Connection: close` HTTP/1.1 response (with a
/// `Content-Length` or read to the end).
fn parse_response(raw: &[u8]) -> Option<(u16, Value)> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let status: u16 = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let mut body = &raw[split + 4..];
    if let Some(len) = head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse::<usize>().ok())
            .flatten()
    }) {
        body = &body[..len.min(body.len())];
    }
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body).unwrap_or(Value::Null)
    };
    Some((status, value))
}

fn not_connected() -> Value {
    json!({"available": false, "job": null, "modes": [], "message": "Compression worker is not connected."})
}

fn unavailable() -> ApiError {
    ApiError::Unavailable("The compression worker is not connected.".into())
}

/// What the worker answered: its body on success, else its message as an error.
fn answer(status: u16, body: Value) -> ApiResult<Json<Value>> {
    if (200..300).contains(&status) {
        return Ok(Json(body));
    }
    let message = body
        .get("message")
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("The compression worker refused the request.")
        .to_string();
    Err(match status {
        400 | 422 => ApiError::BadRequest(message),
        404 => ApiError::NotFound(message),
        409 => ApiError::Conflict(message),
        _ => ApiError::Unavailable(message),
    })
}

/// `HasActiveJob`: a queued, running or cancelling job for the series. An error when the
/// worker answers but not as expected (its state is unknown).
pub async fn has_active_job(series_id: i64) -> ApiResult<bool> {
    active_job_at(&socket(), series_id).await
}

async fn active_job_at(socket: &std::path::Path, series_id: i64) -> ApiResult<bool> {
    match call_at(socket, "GET", &format!("/series/{series_id}"), None).await {
        None => Ok(false),
        Some((200, body)) => Ok(matches!(
            body["job"]["status"].as_str(),
            Some("queued" | "running" | "cancelling")
        )),
        Some((404, _)) => Ok(false),
        Some(_) => Err(ApiError::Conflict(
            "The compression worker's state for this series is unknown; try again.".into(),
        )),
    }
}

/// Refuses an operation on a series' files while it is being compressed.
pub async fn ensure_idle(series_id: i64) -> ApiResult<()> {
    if has_active_job(series_id).await? {
        return Err(ApiError::Conflict(
            "The series is being compressed; wait for the job to finish or cancel it.".into(),
        ));
    }
    Ok(())
}

#[utoipa::path(get, path = "/series/compression/capabilities", tag = "library",
    responses((status = 200, body = Object, description = "The worker's CPU, hardware and modes")))]
pub async fn capabilities() -> ApiResult<Json<Value>> {
    match call("GET", "/capabilities", None).await {
        Some((status, body)) => answer(status, body),
        None => Ok(Json(not_connected())),
    }
}

#[utoipa::path(get, path = "/series/{id}/compression", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = Object, description = "`{available, job, modes, message}`")))]
pub async fn status(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    kitsunarr_db::library::series::get_list_item(&state.db, id).await?;
    match call("GET", &format!("/series/{id}"), None).await {
        Some((status, body)) => answer(status, body),
        None => Ok(Json(not_connected())),
    }
}

#[utoipa::path(post, path = "/series/{id}/compression", tag = "library", params(("id" = i64, Path)),
    request_body = Object,
    responses((status = 200, body = Object), (status = 409), (status = 503)))]
pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    kitsunarr_db::library::series::get_list_item(&state.db, id).await?;
    let mode = body["mode"]
        .as_str()
        .filter(|m| !m.trim().is_empty())
        .ok_or_else(|| ApiError::field("mode", "Choose a compression mode."))?;
    let min_size = body["minSizeMb"].as_i64().unwrap_or(0);
    if !(0..=100_000).contains(&min_size) {
        return Err(ApiError::field(
            "minSizeMb",
            "Use a minimum size between 0 and 100000 MB.",
        ));
    }
    let request = json!({"mode": mode, "minSizeMb": min_size});
    if request.to_string().len() > MAX_BODY {
        return Err(ApiError::field("mode", "The mode name is too long."));
    }
    match call("POST", &format!("/series/{id}"), Some(&request)).await {
        Some((status, body)) => answer(status, body),
        None => Err(unavailable()),
    }
}

#[utoipa::path(delete, path = "/series/{id}/compression", tag = "library", params(("id" = i64, Path)),
    responses((status = 200, body = Object), (status = 503)))]
pub async fn cancel(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    kitsunarr_db::library::series::get_list_item(&state.db, id).await?;
    match call("DELETE", &format!("/series/{id}"), None).await {
        Some((status, body)) => answer(status, body),
        None => Err(unavailable()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_are_parsed() {
        let raw = b"HTTP/1.0 200 OK\r\nServer: x\r\nContent-Type: application/json\r\nContent-Length: 16\r\n\r\n{\"available\":true}";
        let (status, body) = parse_response(raw).unwrap();
        assert_eq!(status, 200);
        // The length cuts the body as sent.
        assert_eq!(body, Value::Null);
        let raw = b"HTTP/1.0 409 Conflict\r\nContent-Length: 18\r\n\r\n{\"message\":\"busy\"}";
        let (status, body) = parse_response(raw).unwrap();
        assert_eq!((status, body["message"].as_str()), (409, Some("busy")));
        assert!(parse_response(b"garbage").is_none());
    }

    #[test]
    fn worker_answers_become_api_errors() {
        assert!(answer(200, json!({"ok": true})).is_ok());
        assert!(matches!(
            answer(409, json!({"message": "busy"})),
            Err(ApiError::Conflict(m)) if m == "busy"
        ));
        assert!(matches!(
            answer(503, json!({})),
            Err(ApiError::Unavailable(_))
        ));
    }

    /// The proxy against a stand-in worker on a Unix socket.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_worker_is_reached_over_its_socket() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.starts_with("GET /series/5 ") {
                    json!({"available": true, "job": {"status": "running"}, "modes": []})
                } else if req.starts_with("POST /series/5 ") {
                    assert!(req.contains("\"minSizeMb\":100"), "{req}");
                    json!({"job": {"status": "queued"}})
                } else {
                    json!({"available": true, "job": null, "modes": []})
                };
                let text = body.to_string();
                let resp = format!(
                    "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{text}",
                    text.len()
                );
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        assert!(active_job_at(&path, 5).await.unwrap());
        assert!(!active_job_at(&path, 6).await.unwrap());
        let (status, body) = call_at(
            &path,
            "POST",
            "/series/5",
            Some(&json!({"mode": "software-fast", "minSizeMb": 100})),
        )
        .await
        .unwrap();
        assert_eq!(
            (status, body["job"]["status"].as_str()),
            (200, Some("queued"))
        );
        let none = dir.path().join("none");
        assert!(!active_job_at(&none, 5).await.unwrap(), "no worker: no job");
        assert!(call_at(&none, "GET", "/capabilities", None).await.is_none());
    }
}
