//! Catalogue changes against local TMDB, TVmaze and OMDb mocks. Never contacts live services.

use axum::{
    Json,
    extract::{Path, State},
    routing::get,
};
use kitsunarr_db::Database;
use kitsunarr_domain::{
    MetadataProvider,
    api::metadata::{AddTitleRequest, MetadataSourceRequest, MonitorOption, SourceEpisodeMapping},
};
use kitsunarr_library::{
    add_title,
    metadata_source::{apply, preview},
};
use kitsunarr_metadata::{Endpoints, Metadata};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Mock {
    episodes: Arc<Mutex<Value>>,
}

async fn tmdb_show() -> Json<Value> {
    Json(
        json!({"id":100,"name":"A Show","first_air_date":"2020-01-01","status":"Ended","seasons":[{"season_number":1}],"external_ids":{"tvdb_id":800,"imdb_id":"tt0000100"}}),
    )
}
async fn season() -> Json<Value> {
    Json(json!({"episodes":[
        {"id":1001,"season_number":1,"episode_number":1,"name":"Alpha","air_date":"2020-01-01"},
        {"id":1002,"season_number":1,"episode_number":2,"name":"Beta","air_date":"2020-01-08"}
    ]}))
}
async fn tvmaze_show(Path(id): Path<i64>) -> Json<Value> {
    Json(
        json!({"id":id,"name":"A Show Updated","externals":{"thetvdb":800,"imdb":"tt0000100"},"status":"Ended","summary":"New overview"}),
    )
}
async fn episodes(State(m): State<Mock>) -> Json<Value> {
    Json(m.episodes.lock().unwrap().clone())
}
async fn movie() -> Json<Value> {
    Json(
        json!({"id":60,"title":"A Film","imdb_id":"tt0000060","release_date":"2020-01-01","runtime":90}),
    )
}
async fn omdb() -> Json<Value> {
    Json(
        json!({"Title":"A Film Updated","Type":"movie","imdbID":"tt0000060","Year":"2020","Released":"01 Jan 2020","Runtime":"91 min","Response":"True"}),
    )
}

async fn setup() -> (tempfile::TempDir, Database, Metadata, Mock) {
    let mock = Mock {
        episodes: Arc::new(Mutex::new(json!([
            {"id":2001,"season":1,"number":1,"name":"Beta","airdate":"2020-01-08"},
            {"id":2002,"season":1,"number":2,"name":"Alpha","airdate":"2020-01-01"},
            {"id":2003,"season":1,"number":3,"name":"Gamma","airdate":"2020-01-15"}
        ]))),
    };
    let router = axum::Router::new()
        .route("/tv/100", get(tmdb_show))
        .route("/tv/100/season/1", get(season))
        .route("/shows/{id}", get(tvmaze_show))
        .route("/shows/{id}/episodes", get(episodes))
        .route("/movie/60", get(movie))
        .route("/omdb/", get(omdb))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let metadata = Metadata::with(
        reqwest::Client::new(),
        &Endpoints {
            tmdb: base.clone(),
            tvmaze: base.clone(),
            omdb: format!("{base}/omdb"),
            ..Endpoints::offline()
        },
    );
    metadata.set_api_keys(Some("key".into()), Some("key".into()));
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    kitsunarr_db::root_folders::create(db.pool(), "/media", None)
        .await
        .unwrap();
    (dir, db, metadata, mock)
}
async fn add(db: &Database, metadata: &Metadata, movie: bool) -> i64 {
    add_title(
        db,
        metadata,
        &AddTitleRequest {
            provider: if movie {
                MetadataProvider::TmdbMovie
            } else {
                MetadataProvider::TmdbTv
            },
            id: if movie { "60" } else { "100" }.into(),
            root_folder_path: "/media".into(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
            path: None,
        },
    )
    .await
    .unwrap()
    .id
}
fn request(movie: bool) -> MetadataSourceRequest {
    MetadataSourceRequest {
        provider: if movie {
            MetadataProvider::Omdb
        } else {
            MetadataProvider::TvMaze
        },
        id: if movie { "tt0000060" } else { "200" }.into(),
        confirm_identity: false,
        mappings: vec![],
        revision: None,
    }
}
async fn owner(db: &Database, movie: bool, id: i64) -> String {
    sqlx::query_scalar(if movie {
        "SELECT metadata_provider FROM movies WHERE id = ?"
    } else {
        "SELECT metadata_provider FROM series WHERE id = ?"
    })
    .bind(id)
    .fetch_one(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn series_switch_preserves_episode_ids_files_history_and_user_choices() {
    let (_dir, db, metadata, _) = setup().await;
    let id = add(&db, &metadata, false).await;
    sqlx::query("INSERT INTO episode_files (id,series_id,season_number,relative_path,added_at) VALUES (1,?,1,'alpha.mkv','2026-01-01')").bind(id).execute(db.pool()).await.unwrap();
    sqlx::query("UPDATE episodes SET monitored = 0, episode_file_id = 1 WHERE series_id = ? AND episode_number = 1").bind(id).execute(db.pool()).await.unwrap();
    let before: Vec<(i64, Option<String>, bool, Option<i64>)> = sqlx::query_as(
        "SELECT id,title,monitored,episode_file_id FROM episodes WHERE series_id = ? ORDER BY id",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO history (event_type,date,source_title,series_id,episode_id) SELECT 'grabbed','2026-01-01','A Show Alpha',series_id,id FROM episodes WHERE series_id = ? AND episode_number = 1").bind(id).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO series_alternate_titles (series_id,title,clean_title,season_number,source) VALUES (?,'Old Scene Alias','oldscenealias',1,'xem')").bind(id).execute(db.pool()).await.unwrap();
    let original_path: String = sqlx::query_scalar("SELECT path FROM series WHERE id = ?")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    let mut req = request(false);
    let p = preview(&db, &metadata, false, id, &req).await.unwrap();
    assert!(p.identity_verified);
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert_eq!((p.matched_episodes, p.new_episodes), (2, 1));
    assert_eq!(owner(&db, false, id).await, "tmdb", "preview is read-only");
    req.revision = Some(p.revision);
    let result = apply(&db, &metadata, false, id, &req).await.unwrap();
    assert_eq!(result.added_episodes, 1);
    assert_eq!(owner(&db, false, id).await, "tvMaze");
    let after:Vec<(i64,Option<String>,bool,Option<i64>)>=sqlx::query_as("SELECT id,title,monitored,episode_file_id FROM episodes WHERE series_id = ? ORDER BY id LIMIT 2").bind(id).fetch_all(db.pool()).await.unwrap();
    assert_eq!(
        after, before,
        "ids, file associations and monitoring survive renumbering"
    );
    let history_episode: i64 =
        sqlx::query_scalar("SELECT episode_id FROM history WHERE series_id = ?")
            .bind(id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(history_episode, before[0].0);

    let alpha:(i32,String)=sqlx::query_as("SELECT episode_number,provider_episode_id FROM episodes WHERE series_id = ? AND title = 'Alpha'").bind(id).fetch_one(db.pool()).await.unwrap();
    assert_eq!(alpha, (2, "2002".into()));
    let path: String = sqlx::query_scalar("SELECT path FROM series WHERE id = ?")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(path, original_path);
    let old_id:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM series_external_ids WHERE series_id = ? AND kind = 'tmdb' AND external_id = '100')").bind(id).fetch_one(db.pool()).await.unwrap();
    assert!(old_id);
    let aliases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM series_alternate_titles WHERE series_id = ? AND source = 'xem'",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(aliases, 0);
}

#[tokio::test]
async fn stale_episode_file_changes_and_stale_catalogue_previews_are_rejected() {
    let (_dir, db, metadata, mock) = setup().await;
    let id = add(&db, &metadata, false).await;
    let mut req = request(false);
    req.revision = Some(
        preview(&db, &metadata, false, id, &req)
            .await
            .unwrap()
            .revision,
    );
    sqlx::query("UPDATE episodes SET monitored = 0 WHERE series_id = ?")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        apply(&db, &metadata, false, id, &req)
            .await
            .unwrap_err()
            .to_string()
            .contains("Preview")
    );
    req.revision = Some(
        preview(&db, &metadata, false, id, &req)
            .await
            .unwrap()
            .revision,
    );
    mock.episodes.lock().unwrap()[0]["name"] = json!("Changed");
    assert!(
        apply(&db, &metadata, false, id, &req)
            .await
            .unwrap_err()
            .to_string()
            .contains("Preview")
    );
    assert_eq!(owner(&db, false, id).await, "tmdb");
}

#[tokio::test]
async fn ambiguous_numbers_require_explicit_matches_and_duplicates_are_refused() {
    let (_dir, db, metadata, mock) = setup().await;
    let id = add(&db, &metadata, false).await;
    for e in mock.episodes.lock().unwrap().as_array_mut().unwrap() {
        e["name"] = json!("Unknown");
        e["airdate"] = Value::Null;
    }
    let mut req = request(false);
    let p = preview(&db, &metadata, false, id, &req).await.unwrap();
    assert!(!p.blockers.is_empty());
    req.revision = Some(p.revision);
    assert!(apply(&db, &metadata, false, id, &req).await.is_err());
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM episodes WHERE series_id = ? ORDER BY episode_number")
            .bind(id)
            .fetch_all(db.pool())
            .await
            .unwrap();
    req.mappings = vec![
        SourceEpisodeMapping {
            episode_id: ids[0],
            season_number: 1,
            episode_number: 2,
        },
        SourceEpisodeMapping {
            episode_id: ids[1],
            season_number: 1,
            episode_number: 2,
        },
    ];
    assert!(
        !preview(&db, &metadata, false, id, &req)
            .await
            .unwrap()
            .blockers
            .is_empty()
    );
    req.mappings[1].episode_number = 1;
    let p = preview(&db, &metadata, false, id, &req).await.unwrap();
    assert!(p.blockers.is_empty());
    req.revision = Some(p.revision);
    apply(&db, &metadata, false, id, &req).await.unwrap();
}

#[tokio::test]
async fn orphaned_episodes_are_retained_and_wrong_media_types_are_rejected() {
    let (_dir, db, metadata, mock) = setup().await;
    let id = add(&db, &metadata, false).await;
    sqlx::query(
        "UPDATE episodes SET episode_number = 9 WHERE series_id = ? AND episode_number = 1",
    )
    .bind(id)
    .execute(db.pool())
    .await
    .unwrap();
    mock.episodes
        .lock()
        .unwrap()
        .as_array_mut()
        .unwrap()
        .retain(|e| e["name"] != "Alpha");
    let mut req = request(false);
    let p = preview(&db, &metadata, false, id, &req).await.unwrap();
    assert_eq!(p.retained_episodes, 1);
    assert!(p.blockers.is_empty());
    req.revision = Some(p.revision);
    apply(&db, &metadata, false, id, &req).await.unwrap();
    let orphan: Option<String> = sqlx::query_scalar(
        "SELECT provider_episode_id FROM episodes WHERE series_id = ? AND episode_number = 9",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(orphan.is_none());
    mock.episodes.lock().unwrap().as_array_mut().unwrap().push(json!({"id":2009,"season":1,"number":9,"name":"A different episode","airdate":"2020-02-01"}));
    let err = kitsunarr_library::refresh_series(&db, &metadata, id)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unmatched episode"));
    let title: String =
        sqlx::query_scalar("SELECT title FROM episodes WHERE series_id = ? AND episode_number = 9")
            .bind(id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(title, "Alpha");
    assert!(
        !preview(&db, &metadata, false, id, &request(false))
            .await
            .unwrap()
            .blockers
            .is_empty(),
        "same-owner previews allow retained episodes to be resolved"
    );

    assert!(
        preview(&db, &metadata, false, id, &request(true))
            .await
            .unwrap_err()
            .to_string()
            .contains("media type")
    );
}

#[tokio::test]
async fn movies_keep_their_file_path_and_settings_and_reject_duplicates() {
    let (_dir, db, metadata, _) = setup().await;
    let id = add(&db, &metadata, true).await;
    sqlx::query("INSERT INTO movie_files (movie_id,relative_path,added_at) VALUES (?,'film.mkv','2026-01-01')").bind(id).execute(db.pool()).await.unwrap();
    sqlx::query("UPDATE movies SET monitored = 0 WHERE id = ?")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    kitsunarr_db::library::movies::set_watched(&db, &[id], true)
        .await
        .unwrap();
    kitsunarr_library::refresh_movie(&db, &metadata, id)
        .await
        .unwrap();
    let before = kitsunarr_db::library::movies::get_detail(&db, id)
        .await
        .unwrap();
    let mut req = request(true);
    let p = preview(&db, &metadata, true, id, &req).await.unwrap();
    assert!(p.blockers.is_empty());
    assert!(p.identity_verified);
    req.revision = Some(p.revision);
    apply(&db, &metadata, true, id, &req).await.unwrap();
    let after = kitsunarr_db::library::movies::get_detail(&db, id)
        .await
        .unwrap();
    assert!(after.item.watched);
    assert_eq!(before.item.watched_at, after.item.watched_at);
    assert_eq!(before.file, after.file);
    assert_eq!(before.item.path, after.item.path);
    assert_eq!(before.item.monitored, after.item.monitored);
    assert_eq!(after.item.title, "A Film Updated");
    assert!(
        preview(&db, &metadata, true, id, &req).await.is_err(),
        "same ownership refused"
    );
}

#[tokio::test]
async fn ownership_conflicts_and_missing_previews_never_modify_library() {
    let (_dir, db, metadata, _) = setup().await;
    let id = add(&db, &metadata, false).await;
    assert!(
        apply(&db, &metadata, false, id, &request(false))
            .await
            .is_err()
    );
    sqlx::query("INSERT INTO series (media_kind,title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES ('series','Other','other','other','other','tvMaze','200',1,'/media','/media/other','2026-01-01')").execute(db.pool()).await.unwrap();
    let mut req = request(false);
    let p = preview(&db, &metadata, false, id, &req).await.unwrap();
    assert!(!p.blockers.is_empty());
    req.revision = Some(p.revision);
    assert!(apply(&db, &metadata, false, id, &req).await.is_err());
    assert_eq!(owner(&db, false, id).await, "tmdb");
}

#[tokio::test]
async fn unverified_identity_needs_confirmation_and_database_failure_rolls_back() {
    let (_dir, db, metadata, _) = setup().await;
    let id = add(&db, &metadata, false).await;
    sqlx::query("DELETE FROM series_external_ids WHERE series_id = ?")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    let mut req = request(false);
    let p = preview(&db, &metadata, false, id, &req).await.unwrap();
    assert!(!p.identity_verified);
    assert!(!p.blockers.is_empty());
    req.confirm_identity = true;
    let p = preview(&db, &metadata, false, id, &req).await.unwrap();
    assert!(p.blockers.is_empty());
    req.revision = Some(p.revision);
    sqlx::query("CREATE TRIGGER fail_merge BEFORE UPDATE OF title ON series BEGIN SELECT RAISE(ABORT, 'simulated failure'); END").execute(db.pool()).await.unwrap();
    assert!(apply(&db, &metadata, false, id, &req).await.is_err());
    assert_eq!(owner(&db, false, id).await, "tmdb");
    let rows:Vec<(i32,String)>=sqlx::query_as("SELECT episode_number,provider_episode_id FROM episodes WHERE series_id = ? ORDER BY episode_number").bind(id).fetch_all(db.pool()).await.unwrap();
    assert_eq!(rows, vec![(1, "1001".into()), (2, "1002".into())]);
}
