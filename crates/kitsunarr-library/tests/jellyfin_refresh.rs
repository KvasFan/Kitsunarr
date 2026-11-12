//! Jellyfin refresh queue against a Jellyfin mock: readiness, immediate and interval modes,
//! failures that keep imports pending.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use kitsunarr_db::Database;
use kitsunarr_library::jellyfin_refresh::{RefreshSettings, on_import, process_pending};
use serde_json::{Value, json};

const LIBRARY: &str = "8a1d2e3f-0000-4000-8000-000000000002";

#[derive(Clone, Default)]
struct Mock {
    refreshes: Arc<Mutex<usize>>,
    /// Whether Jellyfin has scanned the library folder yet.
    scanned: Arc<Mutex<bool>>,
}

fn auth(h: &HeaderMap) -> Result<(), StatusCode> {
    match h.get("authorization").and_then(|v| v.to_str().ok()) {
        Some("MediaBrowser Token=\"key\"") => Ok(()),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn folders(h: HeaderMap) -> Result<axum::Json<Value>, StatusCode> {
    auth(&h)?;
    Ok(axum::Json(
        json!([{"Name": "Anime", "ItemId": LIBRARY.replace('-', ""), "Locations": ["/jf/anime/"]}]),
    ))
}

async fn items(State(m): State<Mock>, h: HeaderMap) -> Result<axum::Json<Value>, StatusCode> {
    auth(&h)?;
    let rows = if *m.scanned.lock().unwrap() {
        json!([{"Id": "1", "Path": "/jf/anime"}])
    } else {
        json!([])
    };
    let n = rows.as_array().unwrap().len();
    Ok(axum::Json(json!({"Items": rows, "TotalRecordCount": n})))
}

async fn refresh(State(m): State<Mock>, h: HeaderMap) -> Result<StatusCode, StatusCode> {
    auth(&h)?;
    *m.refreshes.lock().unwrap() += 1;
    Ok(StatusCode::NO_CONTENT)
}

/// `id` differs per test: the in-flight guard is process-wide, keyed by connection id.
async fn setup(id: i64, mode: i64) -> (tempfile::TempDir, Database, Mock, RefreshSettings) {
    let mock = Mock::default();
    let app = axum::Router::new()
        .route("/Library/VirtualFolders", get(folders))
        .route("/Items", get(items))
        .route("/Items/{id}/Refresh", post(refresh))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    let settings = json!({"host": "127.0.0.1", "port": port, "apiKey": "key", "libraryId": LIBRARY,
        "refreshMode": mode, "refreshIntervalMinutes": 60});
    sqlx::query("INSERT INTO notifications (id, name, implementation, settings, triggers, created_at) VALUES (?, 'Jellyfin', 'Jellyfin', ?, '{\"onDownload\": true}', '2026-11-01T00:00:00Z')")
        .bind(id)
        .bind(settings.to_string())
        .execute(db.pool())
        .await
        .unwrap();
    let c = kitsunarr_library::notify::connections(&db).await.remove(0);
    assert!(kitsunarr_library::jellyfin_refresh::is_queue_managed(&c));
    let r = RefreshSettings::from_connection(&c).unwrap();
    (dir, db, mock, r)
}

async fn generations(db: &Database) -> (i64, i64, i64) {
    sqlx::query_as("SELECT imported_generation, immediate_generation, tick_generation FROM jellyfin_refresh_states")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn each_import_refreshes_once_the_library_is_ready() {
    let (_dir, db, mock, r) = setup(1, 0).await;
    let http = reqwest::Client::new();
    // Not scanned yet: the import stays pending.
    on_import(&db, &http, &r).await;
    assert_eq!(*mock.refreshes.lock().unwrap(), 0);
    assert_eq!(generations(&db).await, (1, 0, 0));
    // Once Jellyfin knows the folder, the scheduled task catches up.
    *mock.scanned.lock().unwrap() = true;
    on_import(&db, &http, &r).await;
    assert_eq!(
        *mock.refreshes.lock().unwrap(),
        2,
        "one refresh per pending import"
    );
    assert_eq!(generations(&db).await, (2, 2, 0));
    process_pending(&db, &http).await.unwrap();
    assert_eq!(*mock.refreshes.lock().unwrap(), 2);
}

#[tokio::test]
async fn interval_mode_coalesces_imports_into_one_refresh_per_tick() {
    let (_dir, db, mock, r) = setup(2, 1).await;
    *mock.scanned.lock().unwrap() = true;
    let http = reqwest::Client::new();
    for _ in 0..3 {
        on_import(&db, &http, &r).await;
    }
    assert_eq!(*mock.refreshes.lock().unwrap(), 0);
    // The interval has not passed yet.
    process_pending(&db, &http).await.unwrap();
    assert_eq!(*mock.refreshes.lock().unwrap(), 0);
    sqlx::query("UPDATE jellyfin_refresh_states SET last_tick_at = '2020-01-01T00:00:00Z'")
        .execute(db.pool())
        .await
        .unwrap();
    process_pending(&db, &http).await.unwrap();
    assert_eq!(*mock.refreshes.lock().unwrap(), 1);
    assert_eq!(generations(&db).await, (3, 0, 3));
    // Deleting the connection drops its state.
    sqlx::query("DELETE FROM notifications")
        .execute(db.pool())
        .await
        .unwrap();
    process_pending(&db, &http).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jellyfin_refresh_states")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}
