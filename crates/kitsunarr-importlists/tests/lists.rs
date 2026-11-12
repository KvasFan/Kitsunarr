//! List readers against local mocks of AniList (GraphQL), MDBList and Sonarr.

use axum::Json;
use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use kitsunarr_importlists::{ListError, fetch};
use serde_json::{Map, Value, json};

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn settings(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn entry(id: i64, status: &str, media_status: &str, lists: Value) -> Value {
    json!({"status": status, "progress": 0, "customListEntries": lists,
        "media": {"id": id, "idMal": id + 1000, "format": "TV", "status": media_status,
                  "title": {"userPreferred": format!("Show {id}"), "romaji": "x"}, "startDate": {"year": 2024}}})
}

async fn anilist(Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(body["variables"]["id"], "someone");
    let page = body["variables"]["page"].as_i64().unwrap();
    let lists = |fav: bool| json!([{"name": "Favourites", "enabled": fav}, {"name": "Skip", "enabled": !fav}]);
    let entries = if page == 1 {
        vec![
            entry(1, "CURRENT", "RELEASING", lists(true)),
            entry(2, "PLANNING", "NOT_YET_RELEASED", lists(true)),
            entry(3, "COMPLETED", "FINISHED", lists(true)),
        ]
    } else {
        vec![
            entry(4, "PLANNING", "FINISHED", lists(false)),
            entry(5, "CURRENT", "FINISHED", lists(true)),
        ]
    };
    Json(
        json!({"data": {"Page": {"pageInfo": {"currentPage": page, "hasNextPage": page == 1}, "mediaList": entries}}}),
    )
}

#[tokio::test]
async fn anilist_filters_statuses_and_custom_lists_across_pages() {
    let base = serve(axum::Router::new().route("/", post(anilist))).await;
    let s = settings(json!({"baseUrl": base, "username": "someone"}));
    let items = fetch(&reqwest::Client::new(), "AniListImport", &s)
        .await
        .unwrap();
    // Defaults: watching and planning; releasing and finished. 2 is not yet released, 3 completed.
    assert_eq!(
        items
            .iter()
            .map(|i| i.anilist_id.unwrap())
            .collect::<Vec<_>>(),
        vec![1, 4, 5]
    );
    assert_eq!(items[0].mal_id, Some(1001));
    assert_eq!(items[0].title, "Show 1");
    // Excluding the "Skip" custom list drops 4.
    let s =
        settings(json!({"baseUrl": base, "username": "someone", "excludedCustomLists": ["Skip"]}));
    let items = fetch(&reqwest::Client::new(), "AniListImport", &s)
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    let none = settings(
        json!({"baseUrl": base, "username": "someone", "importCurrent": false, "importPlanning": false}),
    );
    assert!(matches!(
        fetch(&reqwest::Client::new(), "AniListImport", &none).await,
        Err(ListError::Settings(_))
    ));
}

async fn mdblist(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<(HeaderMap, Json<Value>), StatusCode> {
    if q["apikey"] != "key" {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let offset: usize = q.get("offset").map_or(0, |o| o.parse().unwrap());
    let all = [
        json!({"title": "A", "mediatype": "show", "ids": {"tvdb": 1, "tmdb": 11, "imdb": "tt0000001"}}),
        json!({"title": "B", "mediatype": "show", "ids": {"tmdb": 12}}),
        json!({"title": "Film", "mediatype": "movie", "ids": {"tmdb": 13}}),
    ];
    let mut h = HeaderMap::new();
    h.insert("X-Total-Items", "3".parse().unwrap());
    let rows: Vec<Value> = all.iter().skip(offset).take(2).cloned().collect();
    h.insert(
        "X-Has-More",
        (offset + rows.len() < 3).to_string().parse().unwrap(),
    );
    Ok((h, Json(Value::Array(rows))))
}

/// No pagination headers: read until an empty page.
async fn mdblist_excluded(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    if q.get("offset").is_some_and(|o| o != "0") {
        return Json(json!([]));
    }
    Json(json!([{"title": "A again", "mediatype": "show", "ids": {"tmdb": 11}}]))
}

#[tokio::test]
async fn mdblist_pages_and_subtracts_excluded_lists() {
    let base = serve(
        axum::Router::new()
            .route("/lists/me/good/items/show", get(mdblist))
            .route("/lists/me/bad/items/show", get(mdblist_excluded)),
    )
    .await;
    let s = settings(json!({"baseUrl": base, "apiKey": "key",
        "includedListUrls": ["https://mdblist.com/lists/me/good"]}));
    let items = fetch(&reqwest::Client::new(), "MDBListImport", &s)
        .await
        .unwrap();
    assert_eq!(
        items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        ["A", "B"]
    );
    let s = settings(json!({"baseUrl": base, "apiKey": "key",
        "includedListUrls": ["https://mdblist.com/lists/me/good"],
        "excludedListUrls": ["https://mdblist.com/lists/me/bad"]}));
    let items = fetch(&reqwest::Client::new(), "MDBListImport", &s)
        .await
        .unwrap();
    assert_eq!(
        items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        ["B"]
    );
    let wrong = settings(
        json!({"baseUrl": base, "apiKey": "nope", "includedListUrls": ["https://mdblist.com/lists/me/good"]}),
    );
    assert!(matches!(
        fetch(&reqwest::Client::new(), "MDBListImport", &wrong).await,
        Err(ListError::Authentication(_))
    ));
}

async fn sonarr(h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if h.get("X-Api-Key").and_then(|v| v.to_str().ok()) != Some("k") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(json!([
        {"title": "One", "tvdbId": 1, "qualityProfileId": 1, "languageProfileId": 1, "tags": [5], "rootFolderPath": "/tv/",
         "seasons": [{"seasonNumber": 1, "monitored": true}, {"seasonNumber": 2, "monitored": false}]},
        {"title": "Two", "tvdbId": 2, "qualityProfileId": 2, "languageProfileId": 1, "tags": [], "rootFolderPath": "/anime/", "seasons": []}
    ])))
}

#[tokio::test]
async fn sonarr_filters_and_syncs_season_monitoring() {
    let base = serve(axum::Router::new().route("/api/v3/series", get(sonarr))).await;
    let s = settings(
        json!({"baseUrl": base, "apiKey": "k", "profileIds": [1], "syncSeasonMonitoring": true}),
    );
    let items = fetch(&reqwest::Client::new(), "SonarrImport", &s)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].seasons, vec![(1, true), (2, false)]);
    let s = settings(json!({"baseUrl": base, "apiKey": "k", "rootFolderPaths": ["/ANIME"]}));
    let items = fetch(&reqwest::Client::new(), "SonarrImport", &s)
        .await
        .unwrap();
    assert_eq!(items[0].title, "Two");
    assert!(items[0].seasons.is_empty());
}
