//! The fetch client against a local mock indexer replaying Sonarr's recorded responses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use kitsunarr_indexers::request::search_mode;
use kitsunarr_indexers::{
    Criteria, Dialect, Indexer, IndexerClient, IndexerError, Search, SeriesIds, Settings,
};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../tests/fixtures/indexers/sonarr/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The first `n` items of a recorded feed.
fn first_items(xml: &str, n: usize) -> String {
    let mut end = xml.find("<item>").unwrap();
    for _ in 0..n {
        end = xml[end..]
            .find("</item>")
            .map(|i| end + i + "</item>".len())
            .unwrap();
    }
    format!("{}</channel></rss>", &xml[..end])
}

#[derive(Clone, Default)]
struct Mock {
    log: Arc<Mutex<Vec<String>>>,
    mode: Arc<Mutex<&'static str>>,
}

async fn api(
    State(m): State<Mock>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> axum::response::Response {
    let mut line: Vec<String> = q
        .iter()
        .filter(|(k, _)| *k != "apikey")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    line.sort();
    m.log.lock().unwrap().push(line.join("&"));
    assert!(
        headers
            .get("accept")
            .is_some_and(|a| a.to_str().unwrap().contains("rss"))
    );
    let xml = |body: String| ([("content-type", "application/rss+xml")], body).into_response();
    match *m.mode.lock().unwrap() {
        "unauthorized" => return xml(fixture("unauthorized.xml")),
        "limited" => {
            return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "60")]).into_response();
        }
        "html" => return ([("content-type", "text/html")], "<html>login</html>").into_response(),
        _ => {}
    }
    if q.get("t").map(String::as_str) == Some("caps") {
        return xml(fixture("newznab_caps.xml"));
    }
    if q.contains_key("rid") {
        return xml(first_items(&fixture("newznab_nzb_su.xml"), 0));
    }
    match q.get("offset").map(String::as_str) {
        Some("0") => xml(fixture("newznab_nzb_su.xml")),
        Some("60") => xml(first_items(&fixture("newznab_language.xml"), 3)),
        _ => xml(first_items(&fixture("newznab_nzb_su.xml"), 0)),
    }
}

async fn start() -> (Mock, Indexer, IndexerClient) {
    let mock = Mock::default();
    *mock.mode.lock().unwrap() = "ok";
    let app = axum::Router::new()
        .route("/api", get(api))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let indexer = Indexer {
        id: 7,
        name: "Mock".into(),
        dialect: Dialect::Newznab,
        settings: Settings {
            base_url: format!("http://{addr}"),
            api_path: "/api".into(),
            api_key: Some("secret".into()),
            categories: vec![5030, 5040],
            anime_categories: vec![],
            anime_standard_format_search: false,
            additional_parameters: None,
            minimum_seeders: None,
            movie_categories: vec![2000],
            ..Settings::default()
        },
    };
    let client = IndexerClient::new(reqwest::Client::new()).with_rate_limit(Duration::ZERO);
    (mock, indexer, client)
}

#[tokio::test]
async fn test_reads_caps_and_the_newest_page() {
    let (mock, indexer, client) = start().await;
    let report = client.test(&indexer).await.unwrap();
    assert_eq!(report.releases_found, 100);
    assert_eq!(
        (
            report.capabilities.default_page_size,
            report.capabilities.max_page_size
        ),
        (25, 60)
    );
    let log = mock.log.lock().unwrap().clone();
    assert_eq!(log[0], "t=caps");
    assert_eq!(
        log[1], "cat=5030,5040&extended=1&limit=60&offset=0&t=tvsearch",
        "RSS reads one page"
    );
    // Capabilities are cached.
    client.test(&indexer).await.unwrap();
    assert_eq!(
        mock.log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| *l == "t=caps")
            .count(),
        1
    );
}

#[tokio::test]
async fn search_falls_back_to_titles_pages_until_a_short_page_and_dedupes() {
    let (mock, indexer, client) = start().await;
    let caps = client.capabilities(&indexer).await.unwrap();
    let criteria = Criteria {
        ids: SeriesIds {
            tv_rage: 10,
            ..Default::default()
        },
        scene_titles: vec!["White Collar".into()],
        search_mode: search_mode::DEFAULT,
        anime: false,
    };
    let chain = client.generator(&indexer, &caps).search(
        &criteria,
        &Search::SingleEpisode {
            season: 3,
            episode: 5,
        },
    );
    let releases = client.fetch(&indexer, &chain, false).await.unwrap();
    // The second (short) page repeats three GUIDs of the first: duplicates are dropped.
    assert_eq!(releases.len(), 100);
    assert!(
        releases
            .iter()
            .all(|r| r.indexer_id == 7 && r.indexer == "Mock")
    );
    let log = mock.log.lock().unwrap().clone();
    assert_eq!(
        log[1..],
        [
            "cat=5030,5040&ep=5&extended=1&limit=60&offset=0&rid=10&season=3&t=tvsearch",
            "cat=5030,5040&ep=5&extended=1&limit=60&offset=0&q=White Collar&season=3&t=tvsearch",
            "cat=5030,5040&ep=5&extended=1&limit=60&offset=60&q=White Collar&season=3&t=tvsearch",
        ]
    );
}

#[tokio::test]
async fn errors_are_classified() {
    let (mock, indexer, client) = start().await;
    let caps = client.capabilities(&indexer).await.unwrap();
    let chain = client.generator(&indexer, &caps).recent();
    *mock.mode.lock().unwrap() = "unauthorized";
    assert!(matches!(
        client.fetch(&indexer, &chain, true).await,
        Err(IndexerError::ApiKey(_))
    ));
    *mock.mode.lock().unwrap() = "limited";
    assert!(matches!(
        client.fetch(&indexer, &chain, true).await,
        Err(IndexerError::RequestLimit {
            retry_after_secs: Some(60)
        })
    ));
    *mock.mode.lock().unwrap() = "html";
    assert!(matches!(
        client.fetch(&indexer, &chain, true).await,
        Err(IndexerError::Unexpected(_))
    ));
}

#[tokio::test]
async fn requests_to_one_indexer_are_spaced() {
    let (_mock, indexer, _) = start().await;
    let client =
        IndexerClient::new(reqwest::Client::new()).with_rate_limit(Duration::from_millis(300));
    let started = std::time::Instant::now();
    client.test(&indexer).await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "caps and RSS are two spaced requests"
    );
}

/// Nyaa: no capabilities request; the RSS page is read once, with the search term.
#[tokio::test]
async fn nyaa_tests_and_searches_through_its_rss_page() {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let app = axum::Router::new()
        .route(
            "/",
            get(
                |State(log): State<Arc<Mutex<Vec<String>>>>,
                 Query(q): Query<HashMap<String, String>>| async move {
                    let mut line: Vec<String> = q.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    line.sort();
                    log.lock().unwrap().push(line.join("&"));
                    (
                        [("content-type", "application/rss+xml")],
                        fixture("nyaa_2021.xml"),
                    )
                },
            ),
        )
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let indexer = Indexer {
        id: 9,
        name: "Nyaa".into(),
        dialect: Dialect::Nyaa,
        settings: Settings {
            base_url: format!("http://{addr}"),
            api_path: "/api".into(),
            api_key: None,
            categories: vec![],
            anime_categories: vec![],
            anime_standard_format_search: false,
            additional_parameters: Some("&cats=1_0&filter=1".into()),
            minimum_seeders: Some(1),
            movie_categories: vec![2000],
            ..Settings::default()
        },
    };
    let client = IndexerClient::new(reqwest::Client::new()).with_rate_limit(Duration::ZERO);
    assert_eq!(client.test(&indexer).await.unwrap().releases_found, 3);
    let caps = client.capabilities(&indexer).await.unwrap();
    let criteria = Criteria {
        ids: SeriesIds::default(),
        scene_titles: vec!["Mahouka+Koukou".into()],
        search_mode: search_mode::DEFAULT,
        anime: true,
    };
    let chain = client.generator(&indexer, &caps).search(
        &criteria,
        &Search::AnimeEpisode {
            absolute: 12,
            season: 1,
            episode: 12,
        },
    );
    let found = client.fetch(&indexer, &chain, false).await.unwrap();
    assert_eq!(found.len(), 3);
    assert_eq!(
        log.lock().unwrap().clone(),
        vec![
            "cats=1_0&filter=1&page=rss".to_string(),
            "cats=1_0&filter=1&page=rss&term=Mahouka Koukou 12".to_string(),
        ]
    );
}

/// Sonarr's RSS bookkeeping: the feed is paged back to the previous sync's newest release.
#[tokio::test]
async fn rss_sync_pages_back_to_the_last_synced_release() {
    let (mock, mut indexer, client) = start().await;
    // Only the TV feed (no film-category request).
    indexer.settings.movie_categories.clear();
    let caps = client.capabilities(&indexer).await.unwrap();
    let chain = client.generator(&indexer, &caps).recent();
    let offsets = |mock: &Mock| -> Vec<String> {
        let log = std::mem::take(&mut *mock.log.lock().unwrap());
        log.iter()
            .filter_map(|l| {
                l.split('&')
                    .find(|p| p.starts_with("offset="))
                    .map(str::to_string)
            })
            .collect()
    };
    offsets(&mock);

    // The first sync reads the newest page and remembers its newest release.
    let first = client.fetch_recent(&indexer, &chain, None).await.unwrap();
    assert_eq!(offsets(&mock), vec!["offset=0"]);
    assert_eq!(first.releases.len(), 100);
    assert_eq!(first.gap, None);
    let newest = first.newest.clone().unwrap();
    assert!(
        first
            .releases
            .iter()
            .all(|r| r.release.publish_date <= newest.publish_date)
    );

    // The previous sync's release is on the newest page: nothing more is read.
    let again = client
        .fetch_recent(&indexer, &chain, Some(&newest))
        .await
        .unwrap();
    assert_eq!(offsets(&mock), vec!["offset=0"]);
    assert_eq!(again.gap, None);

    // Not reached: the next page is read; the feed ends first, leaving a gap.
    let old = kitsunarr_indexers::LastRelease {
        publish_date: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        download_url: Some("http://elsewhere/1.nzb".into()),
        guid: "x".into(),
        title: "x".into(),
    };
    let behind = client
        .fetch_recent(&indexer, &chain, Some(&old))
        .await
        .unwrap();
    assert_eq!(offsets(&mock), vec!["offset=0", "offset=60"]);
    let oldest = behind
        .releases
        .iter()
        .map(|r| r.release.publish_date)
        .min()
        .unwrap();
    assert_eq!(behind.gap, Some((old.publish_date, oldest)));
}
