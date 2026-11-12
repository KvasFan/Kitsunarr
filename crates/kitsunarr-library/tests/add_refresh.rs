//! Add and refresh against a local mock of TVmaze (replaying a real response) and AniList.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use kitsunarr_db::Database;
use kitsunarr_domain::api::metadata::{AddTitleRequest, MonitorOption};
use kitsunarr_domain::{MetadataProvider, MonitorNewItems};
use kitsunarr_library::{LibraryError, add_title, due_for_refresh, refresh_series};
use kitsunarr_metadata::Metadata;
use serde_json::{Value, json};

#[derive(Clone)]
struct Mock {
    show: Arc<Mutex<Value>>,
}

async fn show(State(m): State<Mock>, Path(_id): Path<String>) -> Json<Value> {
    Json(m.show.lock().unwrap().clone())
}

async fn episodes(State(m): State<Mock>, Path(_id): Path<String>) -> Json<Value> {
    Json(m.show.lock().unwrap()["_embedded"]["episodes"].clone())
}

async fn graphql(Json(body): Json<Value>) -> Json<Value> {
    let q = body["query"].as_str().unwrap_or_default();
    if q.contains("airingSchedule") {
        return Json(
            json!({"data": {"Media": {"airingSchedule": {"pageInfo": {"hasNextPage": false}, "nodes": []}}}}),
        );
    }
    Json(json!({"data": {"Media": {
        "id": 5, "idMal": 5, "format": "MOVIE", "status": "FINISHED", "episodes": 1, "duration": 115, "seasonYear": 2001,
        "countryOfOrigin": "JP", "title": {"romaji": "Cowboy Bebop: Tengoku no Tobira", "english": "Cowboy Bebop: The Movie", "native": "カウボーイビバップ 天国の扉"},
        "synonyms": [], "startDate": {"year": 2001, "month": 9, "day": 1}, "endDate": {"year": 2001, "month": 9, "day": 1},
        "coverImage": {"extraLarge": "https://img/x.jpg"}, "genres": ["Action"], "averageScore": 82, "popularity": 1,
        "description": "Film.", "studios": {"nodes": [{"name": "Sunrise"}]}, "nextAiringEpisode": null
    }}}))
}

async fn setup() -> (tempfile::TempDir, Database, Metadata, Mock) {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/metadata/tvmaze-show-169.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let mock = Mock {
        show: Arc::new(Mutex::new(fixture)),
    };
    let app = axum::Router::new()
        .route("/shows/{id}", get(show))
        .route("/shows/{id}/episodes", get(episodes))
        .route("/graphql", post(graphql))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let metadata = Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: format!("http://{addr}"),
            anilist: format!("http://{addr}/graphql"),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    kitsunarr_db::root_folders::create(db.pool(), "/media/tv", None)
        .await
        .unwrap();
    (dir, db, metadata, mock)
}

fn request(provider: MetadataProvider, id: &str, monitor: MonitorOption) -> AddTitleRequest {
    AddTitleRequest {
        provider,
        id: id.into(),
        root_folder_path: "/media/tv".into(),
        quality_profile_id: 1,
        monitor,
        series_type: None,
        season_folder: true,
        tags: vec![],
        search_for_missing: false,
        episode_ordering: None,
    }
}

async fn count(db: &Database, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(db.pool()).await.unwrap()
}

#[tokio::test]
async fn add_series_then_refresh_preserves_user_choices() {
    let (_dir, db, metadata, mock) = setup().await;
    let added = add_title(
        &db,
        &metadata,
        &request(MetadataProvider::TvMaze, "169", MonitorOption::FirstSeason),
    )
    .await
    .unwrap();
    assert_eq!(added.media_type, "series");
    let path: String = sqlx::query_scalar("SELECT path FROM series")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(path, "/media/tv/Breaking Bad");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM episodes").await, 62);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM episodes WHERE monitored = 1").await,
        7,
        "first season only"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM series_external_ids WHERE kind = 'tvdb' AND external_id = '81189'"
        )
        .await,
        1
    );

    // Adding the same title again (or via another identity) is refused.
    let again = add_title(
        &db,
        &metadata,
        &request(MetadataProvider::TvMaze, "169", MonitorOption::All),
    )
    .await
    .unwrap_err();
    assert!(matches!(again, LibraryError::Exists(_)));

    // The user changes monitoring; the provider later adds an episode to the monitored first
    // season and one to the unmonitored last season, and retitles the show.
    sqlx::query("UPDATE episodes SET monitored = 1 WHERE season_number = 2 AND episode_number = 1")
        .execute(db.pool())
        .await
        .unwrap();
    {
        let mut show = mock.show.lock().unwrap();
        show["name"] = json!("Breaking Bad (Remastered)");
        let episodes = show["_embedded"]["episodes"].as_array_mut().unwrap();
        for (template, id, number) in [(6usize, 999_998, 8), (61, 999_999, 17)] {
            let mut extra = episodes[template].clone();
            extra["id"] = json!(id);
            extra["number"] = json!(number);
            extra["name"] = json!("Bonus");
            episodes.push(extra);
        }
    }
    // A refresh bypasses the provider cache, so the changed response is picked up immediately.
    let r = refresh_series(&db, &metadata, added.id).await.unwrap();
    assert_eq!((r.added_episodes, r.updated_episodes), (2, 62));
    let title: String = sqlx::query_scalar("SELECT title FROM series")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(title, "Breaking Bad (Remastered)");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM episodes WHERE season_number = 2 AND episode_number = 1 AND monitored = 1").await, 1, "user choice kept");
    let monitor_new: String = sqlx::query_scalar("SELECT monitor_new_items FROM series")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(monitor_new, MonitorNewItems::All.as_str());
    let new_monitored: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT season_number, monitored FROM episodes WHERE title = 'Bonus' ORDER BY season_number",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(
        new_monitored,
        vec![(1, true), (5, false)],
        "new episodes are monitored only in monitored seasons (Sonarr semantics)"
    );
}

#[tokio::test]
async fn add_anilist_movie_creates_a_movie() {
    let (_dir, db, metadata, _mock) = setup().await;
    let added = add_title(
        &db,
        &metadata,
        &request(MetadataProvider::AniList, "5", MonitorOption::All),
    )
    .await
    .unwrap();
    assert_eq!(added.media_type, "movie");
    let (title, path, original): (String, String, Option<String>) =
        sqlx::query_as("SELECT title, path, original_title FROM movies")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(title, "Cowboy Bebop: The Movie");
    assert_eq!(
        path, "/media/tv/Cowboy Bebop - The Movie (2001)",
        "movie folder format with smart colon replacement"
    );
    assert_eq!(original.as_deref(), Some("カウボーイビバップ 天国の扉"));
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM movie_external_ids WHERE kind = 'myAnimeList'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn rejects_unknown_root_folders_and_profiles() {
    let (_dir, db, metadata, _mock) = setup().await;
    let mut req = request(MetadataProvider::TvMaze, "169", MonitorOption::All);
    req.root_folder_path = "/elsewhere".into();
    assert!(matches!(
        add_title(&db, &metadata, &req).await,
        Err(LibraryError::Invalid {
            field: "rootFolderPath",
            ..
        })
    ));
    req.root_folder_path = "/media/tv/".into();
    req.quality_profile_id = 999;
    assert!(matches!(
        add_title(&db, &metadata, &req).await,
        Err(LibraryError::Invalid {
            field: "qualityProfileId",
            ..
        })
    ));
    req.quality_profile_id = 1;
    req.tags = vec![42];
    assert!(matches!(
        add_title(&db, &metadata, &req).await,
        Err(LibraryError::Invalid { field: "tags", .. })
    ));
    assert_eq!(count(&db, "SELECT COUNT(*) FROM series").await, 0);
}

#[tokio::test]
async fn refresh_scheduling_follows_status_and_age() {
    let (_dir, db, metadata, _mock) = setup().await;
    let show = add_title(
        &db,
        &metadata,
        &request(MetadataProvider::TvMaze, "169", MonitorOption::All),
    )
    .await
    .unwrap();
    let movie = add_title(
        &db,
        &metadata,
        &request(MetadataProvider::AniList, "5", MonitorOption::All),
    )
    .await
    .unwrap();
    let now = chrono::Utc::now();
    assert_eq!(
        due_for_refresh(&db, &metadata, now).await.unwrap(),
        (vec![], vec![]),
        "just added"
    );

    // Breaking Bad has ended: a week-old refresh is not due, a 40-day-old one is.
    let set = |table: &'static str, id: i64, days: i64| {
        let at = (now - chrono::TimeDelta::days(days))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let db = db.clone();
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET last_refreshed_at = ? WHERE id = ?"
            )))
            .bind(at)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        }
    };
    set("series", show.id, 7).await;
    set("movies", movie.id, 7).await;
    assert_eq!(
        due_for_refresh(&db, &metadata, now).await.unwrap(),
        (vec![], vec![]),
        "ended series and released films wait a month"
    );
    sqlx::query("UPDATE series SET status = 'continuing'")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        due_for_refresh(&db, &metadata, now).await.unwrap().0,
        vec![show.id],
        "continuing series refresh after 6 hours"
    );
    sqlx::query("UPDATE series SET status = 'ended'")
        .execute(db.pool())
        .await
        .unwrap();
    set("series", show.id, 40).await;
    set("movies", movie.id, 40).await;
    assert_eq!(
        due_for_refresh(&db, &metadata, now).await.unwrap(),
        (vec![show.id], vec![movie.id])
    );

    // TheTVDB-owned titles (migrated from Sonarr) refresh through SkyHook.
    sqlx::query("UPDATE series SET metadata_provider = 'tvdb'")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        due_for_refresh(&db, &metadata, now).await.unwrap().0,
        vec![show.id]
    );
    // Titles owned by a catalogue that needs a key nobody entered are skipped.
    sqlx::query("UPDATE series SET metadata_provider = 'tmdb'")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        due_for_refresh(&db, &metadata, now).await.unwrap().0,
        Vec::<i64>::new()
    );
}
