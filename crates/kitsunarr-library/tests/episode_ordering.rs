//! Switching a TMDB series to an episode group against a TMDB mock: episodes are renumbered by
//! their TMDB episode id and keep their files.

use axum::Json;
use axum::extract::Path;
use axum::routing::get;
use kitsunarr_db::Database;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::metadata::{AddTitleRequest, MonitorOption};
use kitsunarr_metadata::Metadata;
use serde_json::{Value, json};

async fn show() -> Json<Value> {
    Json(
        json!({"id": 100, "name": "Grouped Show", "first_air_date": "2020-01-01", "status": "Ended",
        "episode_run_time": [24], "original_language": "en", "seasons": [{"season_number": 1}]}),
    )
}

async fn season(Path((_, n)): Path<(String, i32)>) -> Json<Value> {
    assert_eq!(n, 1);
    let eps: Vec<Value> = (1..=4)
        .map(|e| json!({"id": 1000 + e, "season_number": 1, "episode_number": e, "name": format!("Episode {e}"), "air_date": "2020-01-01"}))
        .collect();
    Json(json!({"episodes": eps}))
}

async fn groups() -> Json<Value> {
    Json(
        json!({"results": [{"id": "grp1", "name": "Two cours", "type": 6, "episode_count": 4, "group_count": 2}]}),
    )
}

/// Two "seasons" of two episodes; episode 1004 is not in the group.
async fn group() -> Json<Value> {
    Json(json!({"groups": [
        {"order": 1, "episodes": [{"id": 1001, "order": 0}, {"id": 1002, "order": 1}]},
        {"order": 2, "episodes": [{"id": 1003, "order": 0}]}
    ]}))
}

#[tokio::test]
async fn switching_to_an_episode_group_keeps_files_with_their_episodes() {
    let app = axum::Router::new()
        .route("/tv/{id}", get(show))
        .route("/tv/{id}/season/{n}", get(season))
        .route("/tv/{id}/episode_groups", get(groups))
        .route("/tv/episode_group/{gid}", get(group));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let metadata = Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tmdb: format!("http://{addr}"),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    );
    metadata.set_api_keys(Some("key".into()), None);
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    kitsunarr_db::root_folders::create(db.pool(), "/media/tv", None)
        .await
        .unwrap();
    let added = kitsunarr_library::add_title(
        &db,
        &metadata,
        &AddTitleRequest {
            provider: MetadataProvider::TmdbTv,
            id: "100".into(),
            root_folder_path: "/media/tv".into(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
        },
    )
    .await
    .unwrap();
    // A file on S01E03 and one on S01E04.
    for (file, ep) in [(1, 3), (2, 4)] {
        sqlx::query("INSERT INTO episode_files (id, series_id, season_number, relative_path, added_at) VALUES (?, ?, 1, ?, '2026-11-01T00:00:00Z')")
            .bind(file)
            .bind(added.id)
            .bind(format!("S01E0{ep}.mkv"))
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE episodes SET episode_file_id = ? WHERE series_id = ? AND season_number = 1 AND episode_number = ?")
            .bind(file)
            .bind(added.id)
            .bind(ep)
            .execute(db.pool())
            .await
            .unwrap();
    }

    let orderings = metadata
        .orderings(MetadataProvider::TmdbTv, "100")
        .await
        .unwrap();
    assert_eq!(orderings.len(), 2);
    assert_eq!(orderings[1].id, "grp1");

    kitsunarr_library::set_episode_ordering(&db, &metadata, added.id, Some("grp1".into()))
        .await
        .unwrap();
    let rows: Vec<(i64, i64, String, Option<i64>)> = sqlx::query_as(
        "SELECT season_number, episode_number, provider_episode_id, episode_file_id FROM episodes WHERE series_id = ? ORDER BY season_number, episode_number",
    )
    .bind(added.id)
    .fetch_all(db.pool())
    .await
    .unwrap();
    // TMDB episode 1003 is S02E01 now and keeps its file; 1004 is not in the group but keeps
    // its file at its old free number.
    assert_eq!(
        rows,
        vec![
            (1, 1, "1001".into(), None),
            (1, 2, "1002".into(), None),
            (1, 4, "1004".into(), Some(2)),
            (2, 1, "1003".into(), Some(1)),
        ]
    );
    let ordering: Option<String> =
        sqlx::query_scalar("SELECT episode_ordering FROM series WHERE id = ?")
            .bind(added.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(ordering.as_deref(), Some("grp1"));

    // Back to the default seasons.
    kitsunarr_library::set_episode_ordering(&db, &metadata, added.id, None)
        .await
        .unwrap();
    let files: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT season_number, episode_number, episode_file_id FROM episodes WHERE series_id = ? AND episode_file_id IS NOT NULL ORDER BY episode_number",
    )
    .bind(added.id)
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(files, vec![(1, 3, Some(1)), (1, 4, Some(2))]);
}

/// Tanukarr's ordering choice when adding: the series is numbered by the group from the start.
#[tokio::test]
async fn a_series_can_be_added_in_an_episode_group() {
    let app = axum::Router::new()
        .route("/tv/{id}", get(show))
        .route("/tv/{id}/season/{n}", get(season))
        .route("/tv/{id}/episode_groups", get(groups))
        .route("/tv/episode_group/{gid}", get(group));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let metadata = Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tmdb: format!("http://{addr}"),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    );
    metadata.set_api_keys(Some("key".into()), None);
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    kitsunarr_db::root_folders::create(db.pool(), "/media/tv", None)
        .await
        .unwrap();
    let added = kitsunarr_library::add_title(
        &db,
        &metadata,
        &AddTitleRequest {
            provider: MetadataProvider::TmdbTv,
            id: "100".into(),
            root_folder_path: "/media/tv".into(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: Some("grp1".into()),
        },
    )
    .await
    .unwrap();
    let (ordering, seasons): (Option<String>, i64) = sqlx::query_as(
        "SELECT episode_ordering, (SELECT MAX(season_number) FROM episodes WHERE series_id = s.id) FROM series s WHERE id = ?",
    )
    .bind(added.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!((ordering.as_deref(), seasons), (Some("grp1"), 2));
}
