//! A title added from AniList joins its franchise group in the background (AniList mock).

use axum::Json;
use axum::routing::post;
use kitsunarr_db::Database;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::metadata::{AddTitleRequest, MonitorOption};
use kitsunarr_metadata::Metadata;
use serde_json::{Value, json};

fn media(id: i64) -> Value {
    json!({"id": id, "idMal": id + 100, "format": "TV", "status": "FINISHED", "episodes": 2, "duration": 24,
        "seasonYear": 2020, "countryOfOrigin": "JP",
        "title": {"romaji": format!("Romaji {id}"), "english": format!("Franchise part {id}"), "native": "x"},
        "synonyms": [], "startDate": {"year": 2020, "month": 1, "day": 1}, "endDate": {"year": 2020, "month": 3, "day": 1},
        "coverImage": {"extraLarge": "https://img/x.jpg"}, "genres": [], "averageScore": 80, "popularity": 1,
        "description": "x", "studios": {"nodes": []}, "nextAiringEpisode": null})
}

async fn graphql(Json(body): Json<Value>) -> Json<Value> {
    let q = body["query"].as_str().unwrap_or_default();
    let id = body["variables"]["id"].as_i64().unwrap_or(0);
    if q.contains("airingSchedule") {
        return Json(
            json!({"data": {"Media": {"airingSchedule": {"pageInfo": {"hasNextPage": false}, "nodes": []}}}}),
        );
    }
    if q.contains("relations") {
        let edge = |kind: &str, n: i64| json!({"relationType": kind, "node": {"id": n, "type": "ANIME", "format": "TV", "title": {"english": format!("Franchise part {n}")}}});
        let edges = match id {
            1 => vec![edge("SEQUEL", 2)],
            2 => vec![edge("PREQUEL", 1)],
            _ => vec![],
        };
        return Json(
            json!({"data": {"Media": {"id": id, "format": "TV", "title": {"english": format!("Franchise part {id}")}, "relations": {"edges": edges}}}}),
        );
    }
    Json(json!({"data": {"Media": media(id)}}))
}

#[tokio::test]
async fn an_added_sequel_joins_its_franchise_group() {
    let app = axum::Router::new().route("/graphql", post(graphql));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let metadata = Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            anilist: format!("http://{addr}/graphql"),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    kitsunarr_db::root_folders::create(db.pool(), "/media/anime", None)
        .await
        .unwrap();
    let added = kitsunarr_library::add_title(
        &db,
        &metadata,
        &AddTitleRequest {
            provider: MetadataProvider::AniList,
            id: "2".into(),
            root_folder_path: "/media/anime".into(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
        },
    )
    .await
    .unwrap();
    let mut member = None;
    for _ in 0..100 {
        member = sqlx::query_as::<_, (String, String, String, i64)>(
            "SELECT c.title, c.metadata_id, m.label, m.position FROM collection_members m \
             JOIN collections c ON c.id = m.collection_id WHERE c.kind = 'animeGroup' AND m.series_id = ?",
        )
        .bind(added.id)
        .fetch_optional(db.pool())
        .await
        .unwrap();
        if member.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        member,
        Some(("Franchise part 1".into(), "1".into(), "Season 2".into(), 2))
    );
}

async fn tmdb_movie(axum::extract::Path(id): axum::extract::Path<i64>) -> Json<Value> {
    Json(
        json!({"id": id, "title": format!("Film {id}"), "release_date": "2003-05-01", "runtime": 120,
        "status": "Released", "original_language": "en",
        "belongs_to_collection": {"id": 77, "name": "The Saga Collection"}}),
    )
}

async fn tmdb_collection() -> Json<Value> {
    Json(
        json!({"id": 77, "name": "The Saga Collection", "overview": "Three films.", "parts": [
            {"id": 1, "title": "Film 1", "release_date": "2001-01-01"},
            {"id": 2, "title": "Film 2", "release_date": "2003-05-01"},
            {"id": 3, "title": "Film 3", "release_date": "2002-01-01"}
        ]}),
    )
}

#[tokio::test]
async fn an_added_tmdb_film_joins_its_collection_in_release_order() {
    let app = axum::Router::new()
        .route("/movie/{id}", axum::routing::get(tmdb_movie))
        .route("/collection/{id}", axum::routing::get(tmdb_collection));
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
    kitsunarr_db::root_folders::create(db.pool(), "/media/movies", None)
        .await
        .unwrap();
    let added = kitsunarr_library::add_title(
        &db,
        &metadata,
        &AddTitleRequest {
            provider: MetadataProvider::TmdbMovie,
            id: "2".into(),
            root_folder_path: "/media/movies".into(),
            quality_profile_id: 1,
            monitor: MonitorOption::All,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(added.media_type, "movie");
    let mut member = None;
    for _ in 0..100 {
        member = sqlx::query_as::<_, (String, String, String, i64)>(
            "SELECT c.title, c.kind, m.label, m.position FROM collection_members m \
             JOIN collections c ON c.id = m.collection_id WHERE m.movie_id = ?",
        )
        .bind(added.id)
        .fetch_optional(db.pool())
        .await
        .unwrap();
        if member.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        member,
        Some((
            "The Saga Collection".into(),
            "provider".into(),
            "Film 03".into(),
            3
        ))
    );
}
