//! Native movie watched state and Jellyfin application against a disposable SQLite library.
use kitsunarr_db::{Database, library::movies};
use kitsunarr_domain::api::library::LibraryQuery;
use kitsunarr_library::jellyfin::{self, RemoteItem, WatchedSettings};

async fn db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    for id in 1..=2 {
        sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (?,?,'film','film',?,'tmdbMovie',?,1,'/data',?,'2026-11-15T00:00:00Z')")
            .bind(id).bind(format!("Film {id}")).bind(format!("film-{id}")).bind(id.to_string()).bind(format!("/data/Film {id}")).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO movie_files (id,movie_id,relative_path,added_at) VALUES (?,?,'feature.mkv','2026-11-15T00:00:00Z')").bind(id).bind(id).execute(db.pool()).await.unwrap();
    }
    (dir, db)
}
fn settings() -> WatchedSettings {
    WatchedSettings {
        connection_id: 1,
        host: "localhost".into(),
        port: 8096,
        use_ssl: false,
        url_base: "".into(),
        api_key: "key".into(),
        library_id: "library".into(),
        user_id: "user".into(),
        interval_minutes: 5,
        unmonitor_watched: true,
        remonitor_unwatched: true,
        jellyfin_prefix: Some("/jf".into()),
        local_prefix: Some("/data".into()),
        tags: vec![],
    }
}
fn item(id: i64, played: bool) -> RemoteItem {
    RemoteItem {
        path: format!("/jf/Film {id}/feature.mkv"),
        played,
        last_played: played.then(|| "2026-11-14T22:00:00Z".into()),
    }
}
async fn state(db: &Database, id: i64) -> (bool, bool, Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT watched,monitored,watched_at,jellyfin_unmonitor_owner FROM movies WHERE id=?",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn manual_updates_are_idempotent_deduplicated_and_leave_monitoring_unchanged() {
    let (_dir, db) = db().await;
    assert_eq!(
        movies::set_watched(&db, &[1, 1, 999], true).await.unwrap(),
        1
    );
    let first = state(&db, 1).await;
    assert!(first.0);
    assert!(first.1);
    assert!(first.2.is_some());
    movies::set_watched(&db, &[1], true).await.unwrap();
    assert_eq!(state(&db, 1).await.2, first.2);
    movies::set_watched(&db, &[1], false).await.unwrap();
    assert_eq!(state(&db, 1).await, (false, true, None, None));
    movies::set_watched(&db, &[1], true).await.unwrap();
    let all = movies::list(
        &db,
        &LibraryQuery {
            watched: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(all.total_records, 1);
    assert_eq!(all.records[0].id, 1);
    assert!(all.records[0].watched);
    let unseen = movies::list(
        &db,
        &LibraryQuery {
            watched: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(unseen.total_records, 1);
    assert_eq!(unseen.records[0].id, 2);
    sqlx::query("UPDATE movies SET watched=1,watched_at=CASE id WHEN 1 THEN '2026-11-15T12:00:00Z' ELSE '2026-11-15T10:00:00Z' END")
        .execute(db.pool())
        .await
        .unwrap();
    let sorted = movies::list(
        &db,
        &LibraryQuery {
            sort_key: Some("watchedAt".into()),
            sort_direction: Some(kitsunarr_domain::SortDirection::Asc),
            page_size: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(sorted.total_records, 2);
    assert_eq!(sorted.records.len(), 1);
    assert_eq!(sorted.records[0].id, 2);
}

#[tokio::test]
async fn jellyfin_sets_movie_progress_and_remonitors_only_what_it_unmonitored() {
    let (_dir, db) = db().await;
    let s = settings();
    let changed = jellyfin::apply(&db, &s, &[item(1, true)], true)
        .await
        .unwrap();
    assert_eq!(
        (
            changed.matched_files,
            changed.updated_movies,
            changed.updated_episodes
        ),
        (1, 1, 0)
    );
    assert!(changed.matched_movies.contains(&1));
    let first = state(&db, 1).await;
    assert_eq!((first.0, first.1), (true, false));
    assert_eq!(first.2.as_deref(), Some("2026-11-14T22:00:00Z"));
    assert_eq!(first.3.as_deref(), Some(s.fingerprint().as_str()));
    assert_eq!(
        jellyfin::apply(&db, &s, &[item(1, true)], false)
            .await
            .unwrap()
            .updated_movies,
        0
    );
    jellyfin::apply(&db, &s, &[item(1, false)], false)
        .await
        .unwrap();
    assert_eq!(state(&db, 1).await, (false, true, None, None));
    jellyfin::apply(&db, &s, &[item(1, true)], false)
        .await
        .unwrap();
    movies::set_monitored(&db, 1, false).await.unwrap();
    jellyfin::apply(&db, &s, &[item(1, false)], false)
        .await
        .unwrap();
    assert_eq!(
        state(&db, 1).await,
        (false, false, None, None),
        "an explicit unmonitor revokes automatic ownership"
    );
}

#[tokio::test]
async fn manual_monitoring_and_tag_scope_are_respected() {
    let (_dir, db) = db().await;
    let mut s = settings();
    s.tags = vec![10];
    sqlx::query("INSERT INTO tags (id,label) VALUES (10,'watched')")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO movie_tags (movie_id,tag_id) VALUES (1,10)")
        .execute(db.pool())
        .await
        .unwrap();
    movies::set_monitored(&db, 1, false).await.unwrap();
    jellyfin::apply(&db, &s, &[item(1, true), item(2, true)], true)
        .await
        .unwrap();
    let first = state(&db, 1).await;
    assert!(first.0);
    assert!(!first.1);
    assert!(
        first.3.is_none(),
        "a prior manual unmonitor is never claimed"
    );
    assert!(!state(&db, 2).await.0);
    jellyfin::apply(&db, &s, &[item(1, false)], false)
        .await
        .unwrap();
    assert!(!state(&db, 1).await.1);
    s.tags.clear();
    jellyfin::apply(&db, &s, &[item(2, true)], true)
        .await
        .unwrap();
    movies::bulk_edit(
        &db,
        &kitsunarr_domain::api::library::BulkEdit {
            ids: vec![2],
            monitored: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    jellyfin::apply(&db, &s, &[item(2, false)], false)
        .await
        .unwrap();
    assert!(
        !state(&db, 2).await.1,
        "bulk unmonitoring also revokes automatic ownership"
    );
}

#[tokio::test]
async fn source_changes_cannot_remonitor_a_previous_connections_choice() {
    let (_dir, db) = db().await;
    let s = settings();
    jellyfin::apply(&db, &s, &[item(1, true)], true)
        .await
        .unwrap();
    let mut other = s.clone();
    other.connection_id = 2;
    jellyfin::apply(&db, &other, &[item(1, false)], false)
        .await
        .unwrap();
    assert!(!state(&db, 1).await.1);
    let mut disabled = settings();
    disabled.remonitor_unwatched = false;
    jellyfin::apply(&db, &disabled, &[item(2, true)], true)
        .await
        .unwrap();
    jellyfin::apply(&db, &disabled, &[item(2, false)], false)
        .await
        .unwrap();
    assert!(!state(&db, 2).await.1);
}

async fn series_file(db: &Database, path: &str) {
    sqlx::query("INSERT INTO series (id,media_kind,title,sort_title,clean_title,title_slug,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (1,'series','Show','show','show','show','tvMaze','1',1,'/data',?,'2026-11-15T00:00:00Z')").bind(path).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO seasons (series_id,season_number) VALUES (1,1)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO episode_files (id,series_id,season_number,relative_path,added_at) VALUES (1,1,1,'feature.mkv','2026-11-15T00:00:00Z')").execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO episodes (series_id,season_number,episode_number,episode_file_id) VALUES (1,1,1,1)").execute(db.pool()).await.unwrap();
}
#[tokio::test]
async fn shared_paths_are_ambiguous_across_movies_and_series() {
    let (_dir, db) = db().await;
    series_file(&db, "/data/Film 1").await;
    let summary = jellyfin::apply(&db, &settings(), &[item(1, true)], true)
        .await
        .unwrap();
    assert_eq!(summary.matched_files, 0);
    assert!(!state(&db, 1).await.0);
    let played: bool = sqlx::query_scalar("SELECT watched FROM episodes")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(!played);
}

#[tokio::test]
async fn conflicting_snapshots_and_database_errors_never_apply_partial_movie_progress() {
    let (_dir, db) = db().await;
    let s = settings();
    assert!(
        jellyfin::apply(
            &db,
            &s,
            &[item(1, true), item(2, true), item(2, false)],
            true
        )
        .await
        .is_err()
    );
    assert!(!state(&db, 1).await.0);
    series_file(&db, "/data/Show").await;
    sqlx::query("CREATE TRIGGER reject_watch BEFORE UPDATE OF watched ON movies WHEN NEW.id=2 BEGIN SELECT RAISE(ABORT,'simulated failure'); END").execute(db.pool()).await.unwrap();
    let remote = RemoteItem {
        path: "/jf/Show/feature.mkv".into(),
        played: true,
        last_played: None,
    };
    assert!(
        jellyfin::apply(&db, &s, &[item(1, true), item(2, true), remote], true)
            .await
            .is_err()
    );
    assert!(!state(&db, 1).await.0);
    assert!(!state(&db, 2).await.0);
    let episode: bool = sqlx::query_scalar("SELECT watched FROM episodes")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(!episode);
}

#[tokio::test]
async fn removed_or_renamed_feature_paths_do_not_receive_old_watched_flags() {
    let (_dir, db) = db().await;
    sqlx::query("UPDATE movie_files SET relative_path='renamed.mkv' WHERE movie_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM movie_files WHERE movie_id=2")
        .execute(db.pool())
        .await
        .unwrap();
    let result = jellyfin::apply(&db, &settings(), &[item(1, true), item(2, true)], true)
        .await
        .unwrap();
    assert_eq!(result.matched_files, 0);
    assert!(!state(&db, 1).await.0);
    assert!(!state(&db, 2).await.0);
}

#[tokio::test]
async fn watched_batch_failure_rolls_back_every_manual_change() {
    let (_dir, db) = db().await;
    sqlx::query("CREATE TRIGGER reject_watch BEFORE UPDATE OF watched ON movies WHEN NEW.id=2 BEGIN SELECT RAISE(ABORT,'simulated failure'); END").execute(db.pool()).await.unwrap();
    assert!(movies::set_watched(&db, &[1, 2], true).await.is_err());
    assert!(!state(&db, 1).await.0);
    assert!(!state(&db, 2).await.0);
}

#[tokio::test]
async fn a_real_jellyfin_http_movie_snapshot_reaches_native_movie_progress() {
    use axum::{Json, routing::get};
    use serde_json::json;
    let (_dir, db) = db().await;
    let app=axum::Router::new()
        .route("/Users",get(||async{Json(json!([{"Id":"user"}]))}))
        .route("/Library/VirtualFolders",get(||async{Json(json!([{"ItemId":"library"}]))}))
        .route("/Users/user/Items/library",get(||async{Json(json!({"Id":"library"}))}))
        .route("/Users/user/Items",get(||async{Json(json!({"Items":[{"Id":"film","Type":"Movie","Path":"/jf/Film 1/feature.mkv","UserData":{"Played":true,"LastPlayedDate":"2026-11-14T22:00:00Z"}}],"TotalRecordCount":1}))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = json!({"host":"127.0.0.1","port":port,"apiKey":"key","libraryId":"library","watchedUserId":"user","enableWatchedSync":true,"unmonitorWatchedEpisodes":true,"remonitorUnwatchedEpisodes":true,"jellyfinPathPrefix":"/jf","kitsunarrPathPrefix":"/data"});
    sqlx::query("INSERT INTO notifications (id,name,implementation,settings,created_at) VALUES (1,'Local Jellyfin','Jellyfin',?,'2026-11-15T00:00:00Z')").bind(config.to_string()).execute(db.pool()).await.unwrap();
    let summary = jellyfin::sync(&db, &reqwest::Client::new(), true)
        .await
        .unwrap();
    assert!(summary.contains("1 movies updated"), "{summary}");
    assert_eq!(
        (state(&db, 1).await.0, state(&db, 1).await.1),
        (true, false)
    );
    assert!(!state(&db, 2).await.0);
}
