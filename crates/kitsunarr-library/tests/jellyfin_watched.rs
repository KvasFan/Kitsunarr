//! Jellyfin watched sync against a local Jellyfin mock (the endpoints Tanukarr's proxy reads).

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use kitsunarr_db::Database;
use serde_json::{Value, json};

const USER: &str = "5f3c1c2e-0000-4000-8000-000000000001";
const LIBRARY: &str = "8a1d2e3f-0000-4000-8000-000000000002";

#[derive(Clone, Default)]
struct Mock {
    /// (path, played) of every episode in the library.
    items: Arc<Mutex<Vec<(String, bool)>>>,
}

fn authorized(h: &HeaderMap) -> Result<(), StatusCode> {
    match h.get("X-Emby-Token").and_then(|v| v.to_str().ok()) {
        Some("key") => Ok(()),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn users(h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    authorized(&h)?;
    Ok(Json(json!([{"Id": USER.replace('-', ""), "Name": "anna"}])))
}

async fn folders(h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    authorized(&h)?;
    Ok(Json(
        json!([{"Name": "Anime", "ItemId": LIBRARY.replace('-', ""), "Locations": ["/jf/anime"]}]),
    ))
}

async fn library(h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    authorized(&h)?;
    Ok(Json(
        json!({"Id": LIBRARY.replace('-', ""), "Name": "Anime"}),
    ))
}

async fn items(
    State(m): State<Mock>,
    h: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&h)?;
    assert_eq!(q["includeItemTypes"], "Episode,Movie");
    assert_eq!(q["parentId"], LIBRARY.replace('-', ""));
    let start: usize = q["startIndex"].parse().unwrap();
    let limit: usize = q["limit"].parse().unwrap();
    let all = m.items.lock().unwrap().clone();
    let page: Vec<Value> = all
        .iter()
        .enumerate()
        .skip(start)
        .take(limit)
        .map(|(i, (path, played))| {
            json!({"Id": format!("{:032x}", i + 1), "Type": "Episode", "Path": path,
                   "UserData": {"Played": played, "LastPlayedDate": if *played { json!("2026-11-01T20:00:00Z") } else { Value::Null }}})
        })
        .collect();
    Ok(Json(json!({"Items": page, "TotalRecordCount": all.len()})))
}

async fn setup(settings: Value) -> (tempfile::TempDir, Database, Mock, reqwest::Client) {
    let mock = Mock::default();
    let app = axum::Router::new()
        .route("/Users", get(users))
        .route("/Library/VirtualFolders", get(folders))
        .route("/Users/{user}/Items/{lib}", get(library))
        .route("/Users/{user}/Items", get(items))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let mut s = json!({"host": "127.0.0.1", "port": port, "apiKey": "key", "libraryId": LIBRARY,
        "enableWatchedSync": true, "watchedUserId": USER,
        "jellyfinPathPrefix": "/jf/anime", "kitsunarrPathPrefix": "/data/anime"});
    s.as_object_mut()
        .unwrap()
        .extend(settings.as_object().unwrap().clone());
    let pool = db.pool();
    sqlx::query("INSERT INTO notifications (id, name, implementation, settings, created_at) VALUES (1, 'Jellyfin', 'Jellyfin', ?, '2026-11-01T00:00:00Z')")
        .bind(s.to_string())
        .execute(pool)
        .await
        .unwrap();
    let profile: i64 = sqlx::query_scalar("SELECT MIN(id) FROM quality_profiles")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, metadata_provider, metadata_id, \
         quality_profile_id, root_folder_path, path, added_at) VALUES (1, 'anime', 'Frieren', 'frieren', 'frieren', 'frieren', \
         'anilist', '154587', ?, '/data/anime', '/data/anime/Frieren', '2026-11-01T00:00:00Z')",
    )
    .bind(profile)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO seasons (series_id, season_number, monitored) VALUES (1, 1, 1)")
        .execute(pool)
        .await
        .unwrap();
    for n in 1..=3 {
        sqlx::query("INSERT INTO episode_files (id, series_id, season_number, relative_path, added_at) VALUES (?, 1, 1, ?, '2026-11-01T00:00:00Z')")
            .bind(n)
            .bind(format!("Season 01/Frieren - S01E0{n}.mkv"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO episodes (id, series_id, season_number, episode_number, episode_file_id) VALUES (?, 1, 1, ?, ?)")
            .bind(n)
            .bind(n)
            .bind(n)
            .execute(pool)
            .await
            .unwrap();
    }
    (dir, db, mock, reqwest::Client::new())
}

async fn states(db: &Database) -> Vec<(i64, bool, bool, Option<String>)> {
    sqlx::query_as("SELECT id, watched, monitored, watched_at FROM episodes ORDER BY id")
        .fetch_all(db.pool())
        .await
        .unwrap()
}

fn path(n: u32) -> String {
    format!("/jf/anime/Frieren/Season 01/Frieren - S01E0{n}.mkv")
}

#[tokio::test]
async fn marks_watched_unmonitors_and_remonitors_owned_episodes() {
    let (_dir, db, mock, http) = setup(json!({
        "unmonitorWatchedEpisodes": true, "remonitorUnwatchedEpisodes": true
    }))
    .await;
    // Episode 3 is absent from Jellyfin: never treated as unwatched.
    *mock.items.lock().unwrap() = vec![
        (path(1), true),
        (path(2), false),
        ("/elsewhere/x.mkv".into(), true),
    ];
    let summary = kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    assert!(
        summary.contains("3 items read, 2 files matched"),
        "{summary}"
    );
    let s = states(&db).await;
    assert_eq!(s[0], (1, true, false, Some("2026-11-01T20:00:00Z".into())));
    assert!(!s[1].1 && s[1].2);
    assert!(!s[2].1 && s[2].2);

    // Not due again within the interval unless forced.
    assert_eq!(
        kitsunarr_library::jellyfin::sync(&db, &http, false)
            .await
            .unwrap(),
        "Not due"
    );

    // Marked unwatched in Jellyfin: monitored again, since the sync unmonitored it.
    mock.items.lock().unwrap()[0].1 = false;
    kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    let s = states(&db).await;
    assert_eq!(s[0], (1, false, true, None));

    // A manual unmonitor takes ownership away: unwatching no longer re-monitors.
    mock.items.lock().unwrap()[1].1 = true;
    kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    assert!(!states(&db).await[1].2);
    kitsunarr_db::library::series::set_episodes_monitored(&db, &[2], false)
        .await
        .unwrap();
    mock.items.lock().unwrap()[1].1 = false;
    kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    let s = states(&db).await;
    assert!(!s[1].1 && !s[1].2, "{s:?}");
}

#[tokio::test]
async fn an_invalid_snapshot_changes_nothing() {
    let (_dir, db, mock, http) = setup(json!({})).await;
    *mock.items.lock().unwrap() = vec![(path(1), true), ("relative/path.mkv".into(), true)];
    let err = kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap_err();
    assert!(err.contains("invalid watched snapshot"), "{err}");
    assert!(states(&db).await.iter().all(|s| !s.1));
}

#[tokio::test]
async fn a_wrong_key_or_user_is_reported() {
    let (_dir, db, _mock, http) = setup(json!({"apiKey": "wrong"})).await;
    let err = kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap_err();
    assert!(err.contains("401"), "{err}");
    let (_dir, db, _mock, http) = setup(json!({"watchedUserId": "0000"})).await;
    let err = kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap_err();
    assert!(err.contains("user does not exist"), "{err}");
}

#[tokio::test]
async fn libraries_and_users_are_listed_for_the_editor() {
    let (_dir, db, _mock, http) = setup(json!({})).await;
    let c = kitsunarr_library::notify::connections(&db).await.remove(0);
    let libraries = kitsunarr_library::jellyfin::choices(&http, &c.settings, "libraries")
        .await
        .unwrap();
    assert_eq!(libraries.len(), 1);
    assert_eq!(libraries[0].name, "Anime");
    assert_eq!(libraries[0].value, LIBRARY.replace('-', ""));
    let users = kitsunarr_library::jellyfin::choices(&http, &c.settings, "users")
        .await
        .unwrap();
    assert_eq!(users[0].name, "anna");
    let mut wrong = c.settings.clone();
    wrong.insert("apiKey".into(), json!("nope"));
    assert!(
        kitsunarr_library::jellyfin::choices(&http, &wrong, "users")
            .await
            .is_err()
    );
}

/// AniList and MyAnimeList stand-ins for the progress writers.
#[derive(Clone, Default)]
struct Lists {
    on_list: Arc<Mutex<bool>>,
    anilist_progress: Arc<Mutex<i64>>,
    anilist_writes: Arc<Mutex<Vec<i64>>>,
    mal_watched: Arc<Mutex<i64>>,
    mal_writes: Arc<Mutex<Vec<i64>>>,
}

async fn anilist(
    State(l): State<Lists>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    if h.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer token") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let q = body["query"].as_str().unwrap_or_default();
    let vars = &body["variables"];
    if q.contains("Viewer") {
        assert_eq!(vars["name"], "anna");
        return Ok(Json(
            json!({"data": {"Viewer": {"id": 7, "name": "anna"}, "User": {"id": 7, "name": "anna"}}}),
        ));
    }
    if q.contains("SaveMediaListEntry") {
        assert_eq!(vars["id"], 99);
        let p = vars["progress"].as_i64().unwrap();
        *l.anilist_progress.lock().unwrap() = p;
        l.anilist_writes.lock().unwrap().push(p);
        return Ok(Json(
            json!({"data": {"SaveMediaListEntry": {"id": 99, "userId": 7, "mediaId": 154587, "progress": p}}}),
        ));
    }
    assert!(q.contains("MediaList("), "{q}");
    assert_eq!(vars["media"], 154587);
    assert_eq!(vars["user"], 7);
    if !*l.on_list.lock().unwrap() {
        return Ok(Json(json!({"data": {"MediaList": null}})));
    }
    let p = *l.anilist_progress.lock().unwrap();
    Ok(Json(
        json!({"data": {"MediaList": {"id": 99, "userId": 7, "mediaId": 154587, "progress": p, "media": {"episodes": 28, "type": "ANIME"}}}}),
    ))
}

async fn mal_me() -> Json<Value> {
    Json(json!({"id": 1, "name": "anna"}))
}

async fn mal_anime(State(l): State<Lists>) -> Json<Value> {
    let w = *l.mal_watched.lock().unwrap();
    Json(json!({"id": 5, "num_episodes": 12, "my_list_status": {"num_episodes_watched": w}}))
}

async fn mal_patch(
    State(l): State<Lists>,
    axum::Form(f): axum::Form<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let n: i64 = f["num_watched_episodes"].parse().unwrap();
    *l.mal_watched.lock().unwrap() = n;
    l.mal_writes.lock().unwrap().push(n);
    Json(json!({"status": "watching", "num_episodes_watched": n}))
}

async fn lists_mock() -> (Lists, String) {
    let l = Lists::default();
    let app = axum::Router::new()
        .route("/graphql", axum::routing::post(anilist))
        .route("/mal/users/@me", get(mal_me))
        .route("/mal/anime/{id}", get(mal_anime))
        .route(
            "/mal/anime/{id}/my_list_status",
            axum::routing::patch(mal_patch),
        )
        .with_state(l.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (l, base)
}

async fn progress_state(db: &Database) -> Option<(i64, i64, i64, Option<String>)> {
    sqlx::query_as(
        "SELECT pending_progress, acknowledged_progress, failure_count, last_error FROM import_list_progress_states",
    )
    .fetch_optional(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn watched_progress_reaches_the_anilist_list() {
    let (_dir, db, mock, http) = setup(json!({"syncListProgress": true})).await;
    let (lists, base) = lists_mock().await;
    let settings = json!({"accessToken": "token", "username": "anna", "baseUrl": format!("{base}/graphql"),
        "syncWatchedProgress": true});
    sqlx::query("INSERT INTO import_lists (id, name, implementation, settings, created_at) VALUES (1, 'AniList', 'AniListImport', ?, '2026-11-01T00:00:00Z')")
        .bind(settings.to_string())
        .execute(db.pool())
        .await
        .unwrap();
    // A list without its own opt-in receives nothing.
    sqlx::query("INSERT INTO import_lists (id, name, implementation, settings, created_at) VALUES (2, 'Other', 'AniListImport', ?, '2026-11-01T00:00:00Z')")
        .bind(json!({"accessToken": "token", "username": "anna", "baseUrl": format!("{base}/graphql")}).to_string())
        .execute(db.pool())
        .await
        .unwrap();
    for list in [1, 2] {
        sqlx::query("INSERT INTO import_list_items (import_list_id, title, anilist_id) VALUES (?, 'Frieren', 154587)")
            .bind(list)
            .execute(db.pool())
            .await
            .unwrap();
    }
    *mock.items.lock().unwrap() = vec![(path(1), true), (path(2), true), (path(3), false)];

    // Not on the user's list: nothing is created; retried after a day.
    let summary = kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    assert!(!summary.contains("list progress"), "{summary}");
    let (pending, ack, failures, error) = progress_state(&db).await.unwrap();
    assert_eq!((pending, ack, failures), (2, 0, 1));
    assert!(error.unwrap().contains("not on the AniList list"));
    let next: String =
        sqlx::query_scalar("SELECT next_attempt_at FROM import_list_progress_states")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let next = chrono::DateTime::parse_from_rfc3339(&next).unwrap();
    assert!(next > chrono::Utc::now() + chrono::Duration::hours(23));
    assert!(lists.anilist_writes.lock().unwrap().is_empty());

    // On the list and due: the watched prefix is written.
    *lists.on_list.lock().unwrap() = true;
    sqlx::query("UPDATE import_list_progress_states SET next_attempt_at = '2026-01-01T00:00:00Z'")
        .execute(db.pool())
        .await
        .unwrap();
    let summary = kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    assert!(summary.contains("1 list progress updates"), "{summary}");
    assert_eq!(*lists.anilist_writes.lock().unwrap(), vec![2]);
    assert_eq!(progress_state(&db).await.unwrap(), (2, 2, 0, None));

    // Nothing new: nothing sent.
    kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    assert_eq!(lists.anilist_writes.lock().unwrap().len(), 1);

    // The next episode advances it.
    mock.items.lock().unwrap()[2].1 = true;
    kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    assert_eq!(*lists.anilist_writes.lock().unwrap(), vec![2, 3]);

    // Progress never goes down: unwatching episode 1 breaks the prefix; nothing is sent.
    mock.items.lock().unwrap()[0].1 = false;
    kitsunarr_library::jellyfin::sync(&db, &http, true)
        .await
        .unwrap();
    assert_eq!(*lists.anilist_writes.lock().unwrap(), vec![2, 3]);
    assert_eq!(*lists.anilist_progress.lock().unwrap(), 3);

    // MyAnimeList's writer never lowers a count either.
    let mal = json!({"accessToken": "t", "username": "anna", "baseUrl": format!("{base}/mal")});
    let mal = mal.as_object().unwrap();
    let n = kitsunarr_importlists::progress::myanimelist(&http, mal, 5, 4)
        .await
        .unwrap();
    assert_eq!(n, 4);
    let n = kitsunarr_importlists::progress::myanimelist(&http, mal, 5, 2)
        .await
        .unwrap();
    assert_eq!(n, 4);
    assert_eq!(*lists.mal_writes.lock().unwrap(), vec![4]);
    let other = json!({"accessToken": "t", "username": "bob", "baseUrl": format!("{base}/mal")});
    let e = kitsunarr_importlists::progress::myanimelist(&http, other.as_object().unwrap(), 5, 6)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("another account"), "{e}");
}
