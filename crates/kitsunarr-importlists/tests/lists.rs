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

fn mal_entry(id: i64, status: &str, release: &str, score: i64, tags: &[&str]) -> Value {
    json!({"node": {"id": id, "title": format!("Anime {id}"), "status": release},
        "list_status": {"status": status, "score": score, "tags": tags, "is_rewatching": false}})
}

async fn mal(
    h: HeaderMap,
    axum::extract::Path(user): axum::extract::Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let auth = h.get("authorization").and_then(|v| v.to_str().ok());
    let client = h.get("x-mal-client-id").and_then(|v| v.to_str().ok());
    match (user.as_str(), auth, client) {
        ("@me", Some("Bearer fresh"), _) | ("someone", None, Some("cid")) => {}
        _ => return Err(StatusCode::UNAUTHORIZED),
    }
    let offset: usize = q["offset"].parse().unwrap();
    // Two pages: the API's limit is honoured by the mock as 2 rows per page.
    let all = [
        mal_entry(1, "watching", "currently_airing", 8, &[]),
        mal_entry(2, "plan_to_watch", "not_yet_aired", 0, &["skip"]),
        mal_entry(3, "completed", "finished_airing", 9, &[]),
        mal_entry(4, "plan_to_watch", "finished_airing", 3, &[]),
    ];
    let rows: Vec<Value> = all.iter().skip(offset).take(2).cloned().collect();
    let next = (offset + 2 < all.len()).then(|| {
        format!(
            "http://x/v2/users/{user}/animelist?offset={}&limit=1000",
            offset + 2
        )
    });
    Ok(Json(json!({"data": rows, "paging": {"next": next}})))
}

async fn mal_token(
    axum::extract::Form(f): axum::extract::Form<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(f["grant_type"], "refresh_token");
    assert_eq!(f["refresh_token"], "old-refresh");
    assert_eq!(f["client_id"], "cid");
    Json(json!({"access_token": "fresh", "refresh_token": "new-refresh", "expires_in": 2_678_400}))
}

#[tokio::test]
async fn myanimelist_reads_public_and_token_lists_and_renews_tokens() {
    let base = serve(
        axum::Router::new()
            .route("/v2/users/{user}/animelist", get(mal))
            .route("/token", post(mal_token)),
    )
    .await;
    let http = reqwest::Client::new();
    // Public list with a Client ID: watching and plan to watch, tag "skip" excluded.
    let s = settings(
        json!({"baseUrl": format!("{base}/v2"), "username": "someone", "clientId": "cid",
        "listStatuses": [1, 5], "excludedTags": ["skip"]}),
    );
    let items = fetch(&http, "MyAnimeListImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.mal_id.unwrap()).collect::<Vec<_>>(),
        vec![1, 4]
    );
    // Minimum score.
    let s = settings(
        json!({"baseUrl": format!("{base}/v2"), "username": "someone", "clientId": "cid", "minimumScore": 5}),
    );
    let items = fetch(&http, "MyAnimeListImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.mal_id.unwrap()).collect::<Vec<_>>(),
        vec![1, 3]
    );
    // An expired token is renewed first; the renewed settings are returned for storing.
    let s = settings(
        json!({"baseUrl": format!("{base}/v2"), "tokenUrl": format!("{base}/token"), "clientId": "cid",
        "accessToken": "stale", "refreshToken": "old-refresh", "expires": "2020-01-01T00:00:00Z"}),
    );
    let renewed = kitsunarr_importlists::renew(&http, "MyAnimeListImport", &s)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renewed["accessToken"], "fresh");
    assert_eq!(renewed["refreshToken"], "new-refresh");
    assert!(
        kitsunarr_importlists::renew(&http, "MyAnimeListImport", &renewed)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fetch(&http, "MyAnimeListImport", &renewed)
            .await
            .unwrap()
            .len(),
        4
    );
    // Without a Client ID nothing can be read.
    let s = settings(json!({"username": "someone"}));
    assert!(matches!(
        fetch(&http, "MyAnimeListImport", &s).await,
        Err(ListError::Settings(_))
    ));
}

async fn tmdb_list(
    h: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    if h.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer read-token") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let page: i64 = q["page"].parse().unwrap();
    let body = match (id.as_str(), page) {
        ("1", 1) => json!({"page": 1, "total_pages": 2, "total_results": 3, "results": [
            {"media_type": "tv", "id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20"},
            {"media_type": "movie", "id": 550, "title": "Fight Club"}]}),
        ("1", 2) => json!({"page": 2, "total_pages": 2, "total_results": 3, "results": [
            {"media_type": "tv", "id": 60059, "name": "Better Call Saul"}]}),
        ("2", 1) => json!({"page": 1, "total_pages": 1, "total_results": 1, "results": [
            {"media_type": "tv", "id": 60059, "name": "Better Call Saul"}]}),
        _ => return Err(StatusCode::NOT_FOUND),
    };
    Ok(Json(body))
}

#[tokio::test]
async fn tmdb_lists_page_and_subtract() {
    let base = serve(axum::Router::new().route("/4/list/{id}", get(tmdb_list))).await;
    let http = reqwest::Client::new();
    let s = settings(json!({"baseUrl": base, "authToken": "read-token", "includedListIds": ["1"]}));
    let items = fetch(&http, "TmdbListImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.tmdb_id.unwrap()).collect::<Vec<_>>(),
        vec![1396, 60059]
    );
    assert_eq!(items[0].year, Some(2008));
    let s = settings(
        json!({"baseUrl": base, "authToken": "read-token", "includedListIds": ["1"], "excludedListIds": ["2"]}),
    );
    let items = fetch(&http, "TmdbListImport", &s).await.unwrap();
    assert_eq!(items.len(), 1);
    let s = settings(json!({"baseUrl": base, "authToken": "read-token", "includedListIds": ["9"]}));
    assert!(matches!(
        fetch(&http, "TmdbListImport", &s).await,
        Err(ListError::Settings(_))
    ));
}

async fn trakt_watchlist(
    h: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<(HeaderMap, Json<Value>), StatusCode> {
    if h.get("trakt-api-key").and_then(|v| v.to_str().ok()) != Some("my-app") {
        return Err(StatusCode::FORBIDDEN);
    }
    let page: i64 = q["page"].parse().unwrap();
    let mut out = HeaderMap::new();
    out.insert("X-Pagination-Page-Count", "2".parse().unwrap());
    let body = if page == 1 {
        json!([{"show": {"title": "Severance", "year": 2022, "ids": {"tvdb": 371980, "imdb": "tt11280740"}}}])
    } else {
        json!([{"show": {"title": "Andor", "year": 2022, "ids": {"tvdb": 393189}}}])
    };
    Ok((out, Json(body)))
}

async fn trakt_watched() -> Json<Value> {
    let ep = |n: i64| json!({"number": n});
    Json(json!([
        {"show": {"title": "Done", "aired_episodes": 2, "ids": {"tvdb": 1}}, "seasons": [{"number": 1, "episodes": [ep(1), ep(2)]}]},
        {"show": {"title": "Half", "aired_episodes": 4, "ids": {"tvdb": 2}}, "seasons": [{"number": 0, "episodes": [ep(1)]}, {"number": 1, "episodes": [ep(1)]}]}
    ]))
}

async fn trakt_popular() -> Json<Value> {
    Json(json!([{"title": "Bare", "year": 2024, "ids": {"tmdb": 99}}]))
}

#[tokio::test]
async fn trakt_watchlists_watched_shows_and_charts() {
    let base = serve(
        axum::Router::new()
            .route("/users/me/watchlist/shows/rank", get(trakt_watchlist))
            .route("/users/me/watched/shows", get(trakt_watched))
            .route("/shows/popular", get(trakt_popular)),
    )
    .await;
    let http = reqwest::Client::new();
    let s = settings(json!({"baseUrl": base, "clientId": "my-app", "username": "me"}));
    let items = fetch(&http, "TraktUserImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        ["Severance", "Andor"]
    );
    assert_eq!(items[0].imdb_id.as_deref(), Some("tt11280740"));
    let s = settings(
        json!({"baseUrl": base, "clientId": "my-app", "username": "me", "traktListType": 1, "traktWatchedListType": 1}),
    );
    let items = fetch(&http, "TraktUserImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        ["Half"]
    );
    let s = settings(json!({"baseUrl": base, "clientId": "my-app", "traktListType": 1}));
    let items = fetch(&http, "TraktPopularImport", &s).await.unwrap();
    assert_eq!(items[0].tmdb_id, Some(99));
    let s = settings(json!({"baseUrl": base, "clientId": "other", "username": "me"}));
    assert!(matches!(
        fetch(&http, "TraktUserImport", &s).await,
        Err(ListError::Authentication(_))
    ));
    let s = settings(json!({"baseUrl": base, "username": "me"}));
    assert!(matches!(
        fetch(&http, "TraktUserImport", &s).await,
        Err(ListError::Settings(_))
    ));
}

async fn simkl(h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if h.get("simkl-api-key").is_none()
        || h.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer tok")
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(json!({"anime": [
        {"anime_type": "tv", "show": {"title": "Frieren", "ids": {"simkl": 11, "mal": "52991", "anilist": 154587}}},
        {"anime_type": "tv", "show": {"title": "Dropped later", "ids": {"simkl": 12, "mal": "5"}}},
        {"anime_type": "movie", "show": {"title": "A film", "ids": {"simkl": 13, "mal": "1"}}}
    ]})))
}

/// A Simkl custom list (Tanukarr's V2 custom lists).
async fn simkl_list(axum::extract::Path(id): axum::extract::Path<i64>) -> Json<Value> {
    assert_eq!(id, 77);
    Json(json!({"id": 77, "media_type": "anime", "items": [{"simkl_id": 12}]}))
}

async fn plex(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    if q.get("X-Plex-Token").map(String::as_str) != Some("plex-token") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let start: i64 = q["X-Plex-Container-Start"].parse().unwrap();
    let rows = if start == 0 {
        json!([{"type": "show", "title": "Severance", "year": 2022, "Guid": [{"id": "imdb://tt11280740"}, {"id": "tvdb://371980"}]},
               {"type": "movie", "title": "Dune", "Guid": [{"id": "tmdb://438631"}]}])
    } else {
        json!([])
    };
    let size = rows.as_array().unwrap().len();
    Ok(Json(
        json!({"MediaContainer": {"offset": start, "size": size, "totalSize": 2, "Metadata": rows}}),
    ))
}

#[tokio::test]
async fn simkl_and_plex_watchlists() {
    let base = serve(
        axum::Router::new()
            .route("/sync/all-items/anime/plantowatch", get(simkl))
            .route("/lists/{id}", get(simkl_list))
            .route("/library/sections/watchlist/all", get(plex)),
    )
    .await;
    let http = reqwest::Client::new();
    let s = settings(
        json!({"baseUrl": base, "clientIdV2": "app", "accessToken": "tok", "showType": 1, "listType": 1}),
    );
    let items = fetch(&http, "SimklUserImport", &s).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(
        (items[0].mal_id, items[0].anilist_id),
        (Some(52991), Some(154587))
    );
    // Excluding a custom list's members; including only them.
    let s = settings(
        json!({"baseUrl": base, "clientIdV2": "app", "accessToken": "tok", "showType": 1, "listType": 1, "excludedCustomListIds": [77]}),
    );
    let items = fetch(&http, "SimklUserImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        vec!["Frieren"]
    );
    let s = settings(
        json!({"baseUrl": base, "clientIdV2": "app", "accessToken": "tok", "showType": 1, "listType": 1, "includedCustomListIds": [77]}),
    );
    let items = fetch(&http, "SimklUserImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        vec!["Dropped later"]
    );
    let s = settings(json!({"baseUrl": base, "accessToken": "plex-token"}));
    let items = fetch(&http, "PlexImport", &s).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].tvdb_id, Some(371980));
    let s = settings(json!({"baseUrl": base, "accessToken": "wrong"}));
    assert!(matches!(
        fetch(&http, "PlexImport", &s).await,
        Err(ListError::Authentication(_))
    ));
}

async fn tmdb_discover(Query(q): Query<std::collections::HashMap<String, String>>) -> Json<Value> {
    assert_eq!(q["with_original_language"], "ja");
    assert_eq!(q["sort_by"], "popularity.desc");
    assert_eq!(q["with_genres"], "16");
    let page: i64 = q["page"].parse().unwrap();
    Json(
        json!({"page": page, "total_pages": 2, "results": [{"id": page * 10, "name": format!("Show {page}")}]}),
    )
}

async fn tmdb_person() -> Json<Value> {
    Json(json!({
        "cast": [{"id": 1, "name": "Acted", "media_type": "tv"}],
        "crew": [{"id": 2, "name": "Directed", "department": "Directing"}, {"id": 3, "name": "Lit", "department": "Lighting"}]
    }))
}

async fn trakt_smart(h: HeaderMap) -> Json<Value> {
    assert_eq!(h.get("trakt-api-key").unwrap(), "app");
    Json(json!([{"show": {"title": "Smart", "ids": {"tvdb": 77}}}]))
}

#[tokio::test]
async fn tmdb_discover_person_and_trakt_smart_lists() {
    let base = serve(
        axum::Router::new()
            .route("/3/discover/tv", get(tmdb_discover))
            .route("/3/person/{id}/tv_credits", get(tmdb_person))
            .route("/smart-lists/{slug}/items/shows", get(trakt_smart)),
    )
    .await;
    let http = reqwest::Client::new();
    let s = settings(
        json!({"baseUrl": base, "authToken": "t", "sortByType": 7, "originalLanguage": "JA", "withGenreTypes": [16]}),
    );
    let items = fetch(&http, "TmdbDiscoverImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.tmdb_id.unwrap()).collect::<Vec<_>>(),
        vec![10, 20]
    );
    let s = settings(
        json!({"baseUrl": base, "authToken": "t", "personId": 5, "includeDepartmentTypes": [4]}),
    );
    let items = fetch(&http, "TmdbPersonImport", &s).await.unwrap();
    assert_eq!(
        items.iter().map(|i| i.tmdb_id.unwrap()).collect::<Vec<_>>(),
        vec![1, 2]
    );
    let s = settings(
        json!({"baseUrl": base, "clientId": "app", "smartList": "https://trakt.tv/lists/smart/view/my-smart_1?x=1"}),
    );
    let items = fetch(&http, "TraktSmartListImport", &s).await.unwrap();
    assert_eq!(items[0].tvdb_id, Some(77));
}

/// Account-list pickers: AniList custom lists, Trakt lists, Simkl custom lists.
#[tokio::test]
async fn account_lists_are_listed_for_the_pickers() {
    async fn anilist() -> Json<Value> {
        Json(
            json!({"data": {"User": {"mediaListOptions": {"animeList": {"customLists": ["Favourites", "Rewatch"]}}}}}),
        )
    }
    async fn trakt() -> Json<Value> {
        Json(json!([{"name": "Weekend", "ids": {"slug": "weekend"}}]))
    }
    async fn simkl_me() -> Json<Value> {
        Json(json!({"account": {"id": 42}}))
    }
    async fn simkl_lists() -> Json<Value> {
        Json(
            json!({"lists": [{"id": 7, "name": "Seasonal", "media_type": "anime"}, {"id": 8, "name": "TV", "media_type": "tv"}]}),
        )
    }
    let base = serve(
        axum::Router::new()
            .route("/graphql", axum::routing::post(anilist))
            .route("/users/anna/lists", get(trakt))
            .route("/users/settings", get(simkl_me))
            .route("/lists/user/42", get(simkl_lists)),
    )
    .await;
    let http = reqwest::Client::new();
    use kitsunarr_importlists::choices::lists;
    let names = |c: Vec<kitsunarr_importlists::choices::Choice>| {
        c.into_iter().map(|c| (c.value, c.name)).collect::<Vec<_>>()
    };
    let s = settings(json!({"baseUrl": format!("{base}/graphql"), "username": "anna"}));
    assert_eq!(
        names(lists(&http, "AniListImport", &s).await.unwrap()),
        vec![
            ("Favourites".into(), "Favourites".into()),
            ("Rewatch".into(), "Rewatch".into())
        ]
    );
    let s = settings(json!({"baseUrl": base, "username": "anna", "clientId": "c"}));
    assert_eq!(
        names(lists(&http, "TraktListImport", &s).await.unwrap()),
        vec![("weekend".into(), "Weekend".into())]
    );
    let s =
        settings(json!({"baseUrl": base, "clientIdV2": "c", "accessToken": "t", "showType": 1}));
    assert_eq!(
        names(lists(&http, "SimklUserImport", &s).await.unwrap()),
        vec![("7".into(), "Seasonal".into())]
    );
}
