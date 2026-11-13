//! Sonarr's FileList (`FileListFixture`), HDBits (`HDBitsFixture`) and BroadcasTheNet
//! (`BroadcastheNetFixture`, `BroadcastheNetRequestGeneratorFixture`) tests on their recorded
//! responses, and the fetch client against a local mock of those APIs and of a Torrent RSS feed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use chrono::{NaiveDate, TimeZone, Utc};
use kitsunarr_domain::DownloadProtocol;
use kitsunarr_indexers::feed::flags;
use kitsunarr_indexers::request::{Method, search_mode};
use kitsunarr_indexers::{
    Capabilities, Chain, Criteria, Dialect, Generator, Indexer, IndexerClient, IndexerError,
    LastRelease, Search, SeriesIds, Settings, broadcasthenet, filelist, hdbits,
};
use serde_json::Value;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../tests/fixtures/indexers/sonarr/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

// ---- FileListFixture -------------------------------------------------------------------------

fn filelist_settings(base_url: &str) -> Settings {
    Settings {
        base_url: base_url.into(),
        username: Some("someuser".into()),
        passkey: Some("somepass".into()),
        categories: vec![23, 21, 27],
        ..Settings::default()
    }
}

#[test]
fn should_parse_recent_feed_from_filelist() {
    let releases = filelist::parse(
        &fixture("filelist_recent_feed.json"),
        &filelist_settings("https://filelist.io"),
    )
    .unwrap();
    assert_eq!(releases.len(), 2);
    let r = &releases[0];
    assert_eq!(r.title, "Mankind.Divided.2019.S01E01.1080p.WEB-DL");
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some("https://filelist.io/download.php?id=1234&passkey=somepass")
    );
    assert_eq!(
        r.info_url.as_deref(),
        Some("https://filelist.io/details.php?id=1234")
    );
    assert_eq!(r.comment_url, None);
    assert_eq!(
        r.publish_date,
        Utc.with_ymd_and_hms(2019, 1, 22, 22, 20, 19).unwrap()
    );
    assert_eq!(r.size, 830512414);
    assert_eq!(
        (r.info_hash.as_deref(), r.magnet_url.as_deref()),
        (None, None)
    );
    assert_eq!((r.seeders, r.peers), (Some(12), Some(2 + 12)));
    assert_eq!(r.imdb_id.as_deref(), Some("tt1232322"));
    assert_eq!(r.guid, "FileList-1234");
    assert_eq!(r.indexer_flags, 0);
}

fn criteria(tvdb: i64, imdb: Option<&str>) -> Criteria {
    Criteria {
        ids: SeriesIds {
            tvdb,
            imdb: imdb.map(str::to_string),
            ..SeriesIds::default()
        },
        scene_titles: vec!["Series Title".into(), "Other Title".into()],
        search_mode: search_mode::DEFAULT,
        anime: false,
    }
}

fn generator<'a>(
    settings: &'a Settings,
    caps: &'a Capabilities,
    dialect: Dialect,
) -> Generator<'a> {
    Generator {
        settings,
        caps,
        page_size: 100,
        dialect,
    }
}

fn tier_urls(chain: &Chain) -> Vec<Vec<String>> {
    chain
        .tiers
        .iter()
        .map(|t| t.iter().flat_map(|p| p.pages()).collect())
        .collect()
}

#[test]
fn filelist_requests() {
    let mut settings = filelist_settings("https://filelist.io/");
    settings.anime_categories = vec![24];
    let caps = Capabilities::default();
    let g = generator(&settings, &caps, Dialect::FileList);
    let recent = g.recent();
    let request = &recent.all().next().unwrap().requests()[0];
    assert_eq!(
        request.url,
        "https://filelist.io/api.php?action=latest-torrents&category=23,21,27,24"
    );
    assert_eq!(
        request.basic_auth,
        Some(("someuser".into(), "somepass".into()))
    );
    assert_eq!(request.accept, "application/json");

    let single = g.search(
        &criteria(1, Some("tt1232322")),
        &Search::SingleEpisode {
            season: 1,
            episode: 2,
        },
    );
    let base = "https://filelist.io/api.php?action=search-torrents&category=23,21,27";
    assert_eq!(
        tier_urls(&single),
        [
            vec![format!(
                "{base}&type=imdb&query=tt1232322&season=1&episode=2"
            )],
            vec![
                format!("{base}&type=name&query=Series%20Title&season=1&episode=2"),
                format!("{base}&type=name&query=Other%20Title&season=1&episode=2"),
            ],
        ]
    );

    let anime = g.search(
        &criteria(1, Some("tt1")),
        &Search::AnimeEpisode {
            absolute: 13,
            season: 2,
            episode: 1,
        },
    );
    let anime_base = "https://filelist.io/api.php?action=search-torrents&category=24";
    assert_eq!(
        tier_urls(&anime),
        [
            vec![format!(
                "{anime_base}&type=imdb&query=tt1&season=0&episode=13"
            )],
            vec![format!(
                "{anime_base}&type=imdb&query=tt1&season=2&episode=1"
            )],
            vec![
                format!("{anime_base}&type=name&query=Series%20Title&season=0&episode=13"),
                format!("{anime_base}&type=name&query=Other%20Title&season=0&episode=13"),
            ],
            vec![
                format!("{anime_base}&type=name&query=Series%20Title&season=2&episode=1"),
                format!("{anime_base}&type=name&query=Other%20Title&season=2&episode=1"),
            ],
        ]
    );
    // Without categories nothing is requested; daily searches are not supported.
    let none = Settings {
        categories: vec![],
        ..settings.clone()
    };
    let g = generator(&none, &caps, Dialect::FileList);
    assert!(
        g.search(&criteria(1, Some("tt1")), &Search::Season { season: 1 })
            .all()
            .all(|p| p.pages().is_empty())
    );
    let g = generator(&settings, &caps, Dialect::FileList);
    assert_eq!(
        g.search(
            &criteria(1, Some("tt1")),
            &Search::DailyEpisode {
                date: NaiveDate::from_ymd_opt(2020, 1, 2).unwrap(),
                episodes: Vec::new(),
            }
        )
        .all()
        .count(),
        0
    );
}

#[test]
fn filelist_settings_validation() {
    let ok = filelist_settings("https://filelist.io");
    assert_eq!(ok.validate(Dialect::FileList), Ok(()));
    let no_passkey = Settings {
        passkey: None,
        ..ok.clone()
    };
    assert!(no_passkey.validate(Dialect::FileList).is_err());
    let no_categories = Settings {
        categories: vec![],
        ..ok
    };
    assert!(no_categories.validate(Dialect::FileList).is_err());
}

// ---- HDBitsFixture ---------------------------------------------------------------------------

fn hdbits_settings(base_url: &str) -> Settings {
    Settings {
        base_url: base_url.into(),
        api_key: Some("fakekey".into()),
        username: Some("user".into()),
        categories: vec![2, 3],
        ..Settings::default()
    }
}

#[test]
fn should_parse_recent_feed_from_hdbits() {
    for file in [
        "hdbits_recent_feed_long_ids.json",
        "hdbits_recent_feed_string_ids.json",
    ] {
        let releases =
            hdbits::parse(&fixture(file), &hdbits_settings("https://hdbits.org")).unwrap();
        assert_eq!(releases.len(), 2, "{file}");
        let r = &releases[0];
        assert_eq!(r.guid, "HDBits-257142");
        assert_eq!(r.title, "Supernatural S10E17 1080p WEB-DL DD5.1 H.264-ECI");
        assert_eq!(r.protocol, DownloadProtocol::Torrent);
        assert_eq!(
            r.download_url.as_deref(),
            Some("https://hdbits.org/download.php?id=257142&passkey=fakekey")
        );
        assert_eq!(
            r.info_url.as_deref(),
            Some("https://hdbits.org/details.php?id=257142")
        );
        assert_eq!(
            r.publish_date,
            Utc.with_ymd_and_hms(2015, 4, 4, 20, 30, 46).unwrap()
        );
        assert_eq!(r.size, 1718009717);
        assert_eq!(
            r.info_hash.as_deref(),
            Some("EABC50AEF9F53CEDED84ADF14144D3368E586F3A")
        );
        assert_eq!(r.magnet_url, None);
        assert_eq!((r.seeders, r.peers), (Some(46), Some(47)));
        assert_eq!(r.tvdb_id, 78901);
    }
}

#[test]
fn should_warn_on_wrong_passkey() {
    let body = r#"{"status":5,"message":"Invalid authentication credentials"}"#;
    let err = hdbits::parse(body, &hdbits_settings("https://hdbits.org")).unwrap_err();
    assert_eq!(
        err,
        IndexerError::Indexer(
            "HDBits API request returned status code AuthFailed: Invalid authentication credentials"
                .into()
        )
    );
}

fn body(request: &kitsunarr_indexers::request::Request) -> Value {
    serde_json::from_str(request.body.as_deref().unwrap()).unwrap()
}

#[test]
fn hdbits_requests() {
    let settings = hdbits_settings("https://hdbits.org/");
    let caps = Capabilities::default();
    let g = generator(&settings, &caps, Dialect::HdBits);
    let recent = g.recent().all().next().unwrap().requests();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].url, "https://hdbits.org/api/torrents");
    assert_eq!(recent[0].method, Method::Post);
    assert_eq!(
        body(&recent[0]),
        serde_json::json!({
            "username": "user", "passkey": "fakekey",
            "category": [2, 3], "codec": [], "medium": [], "limit": 100
        })
    );
    let single = g.search(
        &criteria(78901, None),
        &Search::SingleEpisode {
            season: 10,
            episode: 17,
        },
    );
    let q = body(&single.all().next().unwrap().requests()[0]);
    assert_eq!(
        q["tvdb"],
        serde_json::json!({"id": 78901, "season": 10, "episode": 17})
    );
    let daily = g.search(
        &criteria(78901, None),
        &Search::DailyEpisode {
            date: NaiveDate::from_ymd_opt(2015, 4, 4).unwrap(),
            episodes: Vec::new(),
        },
    );
    let q = body(&daily.all().next().unwrap().requests()[0]);
    assert_eq!(q["search"], "2015-04-04");
    assert_eq!(q["tvdb"], serde_json::json!({"id": 78901}));
    // Searches need a TheTVDB id; upstream's anime-episode search requests nothing.
    assert_eq!(
        g.search(&criteria(-5, None), &Search::Season { season: 1 })
            .all()
            .count(),
        0
    );
    assert_eq!(
        g.search(
            &criteria(78901, None),
            &Search::AnimeEpisode {
                absolute: 1,
                season: 1,
                episode: 1
            }
        )
        .all()
        .count(),
        0
    );
}

// ---- BroadcastheNetFixture -------------------------------------------------------------------

#[test]
fn should_parse_recent_feed_from_broadcasthenet() {
    let releases = broadcasthenet::parse(
        &fixture("broadcasthenet_recent_feed.json"),
        "https://api.broadcasthe.net/",
    )
    .unwrap();
    assert_eq!(releases.len(), 2);
    let r = &releases[0];
    assert_eq!(r.guid, "BTN-123");
    assert_eq!(r.title, "Jimmy.Kimmel.2014.09.15.Jane.Fonda.HDTV.x264-aAF");
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "https://broadcasthe.net/torrents.php?action=download&id=123&authkey=123&torrent_pass=123"
        )
    );
    assert_eq!(
        r.info_url.as_deref(),
        Some("https://broadcasthe.net/torrents.php?id=237457&torrentid=123")
    );
    assert_eq!(r.comment_url, None);
    assert_eq!(
        r.publish_date,
        Utc.with_ymd_and_hms(2014, 9, 16, 21, 15, 33).unwrap()
    );
    assert_eq!(r.size, 505099926);
    assert_eq!(r.info_hash.as_deref(), Some("123"));
    assert_eq!((r.tvdb_id, r.tv_rage_id), (71998, 4055));
    assert_eq!(r.imdb_id.as_deref(), Some("tt0320037"));
    assert_eq!(r.magnet_url, None);
    assert_eq!((r.seeders, r.peers), (Some(40), Some(40 + 9)));
    assert_eq!(
        r.indexer_flags,
        flags::FREELEECH | flags::SCENE | flags::SUBTITLES
    );
    assert_eq!(
        releases[1].indexer_flags,
        flags::FREELEECH | flags::INTERNAL
    );
}

#[test]
fn should_replace_https_http_as_needed() {
    let feed = fixture("broadcasthenet_recent_feed.json").replace("http:", "https:");
    let releases = broadcasthenet::parse(&feed, "http://api.broadcasthe.net/").unwrap();
    assert_eq!(releases.len(), 2);
    assert_eq!(
        releases[0].download_url.as_deref(),
        Some(
            "http://broadcasthe.net/torrents.php?action=download&id=123&authkey=123&torrent_pass=123"
        )
    );
    assert_eq!(
        releases[0].info_url.as_deref(),
        Some("http://broadcasthe.net/torrents.php?id=237457&torrentid=123")
    );
}

#[test]
fn broadcasthenet_api_errors() {
    assert!(matches!(
        broadcasthenet::parse(r#"{"id":"1","error":{"code":-32001,"message":"Invalid API Key"}}"#, "https://x/"),
        Err(IndexerError::Indexer(m)) if m.contains("Invalid API Key")
    ));
    assert_eq!(
        broadcasthenet::parse(r#"{"id":"1","result":{"results":"0"}}"#, "https://x/"),
        Ok(Vec::new())
    );
}

// ---- BroadcastheNetRequestGeneratorFixture ---------------------------------------------------

fn btn_settings(base_url: &str) -> Settings {
    Settings {
        base_url: base_url.into(),
        api_key: Some("abc".into()),
        ..Settings::default()
    }
}

/// The query (second JSON-RPC parameter) of each pageable request's first page, per tier.
fn queried(chain: &Chain, tier: usize) -> Vec<Value> {
    chain.tiers[tier]
        .iter()
        .map(|p| body(&p.requests()[0])["params"][1].clone())
        .collect()
}

fn btn_search(tvdb: i64, search: &Search) -> Chain {
    let settings = btn_settings("https://api.broadcasthe.net/");
    let caps = Capabilities::default();
    generator(&settings, &caps, Dialect::BroadcastheNet).search(&criteria(tvdb, None), search)
}

fn single(season: i32, episode: i32) -> Search {
    Search::SingleEpisode { season, episode }
}

#[test]
fn should_wildcard_episode_numbers_below_one_hundred() {
    for (episode, expected) in [
        (1, "S01%E%01%"),
        (9, "S01%E%09%"),
        (97, "S01%E%97%"),
        (99, "S01%E%99%"),
    ] {
        let chain = btn_search(433335, &single(1, episode));
        assert_eq!(chain.tiers.len(), 1);
        let names = queried(&chain, 0);
        assert_eq!(names.len(), 1);
        assert_eq!(names[0]["name"], expected);
        assert_eq!(names[0]["category"], "Episode");
        assert_eq!(names[0]["tvdb"], "433335");
    }
}

#[test]
fn should_not_wildcard_episode_numbers_over_ninety_nine() {
    for (episode, expected) in [(100, "S01%E100%"), (101, "S01%E101%"), (999, "S01%E999%")] {
        let chain = btn_search(433335, &single(1, episode));
        assert_eq!(chain.tiers.len(), 1);
        assert_eq!(queried(&chain, 0)[0]["name"], expected);
    }
}

#[test]
fn should_not_search_without_a_tvdb_or_tvrage_id() {
    assert_eq!(btn_search(0, &single(1, 99)).all().count(), 0);
}

#[test]
fn should_not_change_season_search() {
    let chain = btn_search(433335, &Search::Season { season: 1 });
    assert_eq!(chain.tiers.len(), 1);
    let names = queried(&chain, 0);
    assert_eq!(names.len(), 2);
    assert_eq!(names[0]["name"], "Season 1%");
    assert_eq!(names[0]["category"], "Season");
    // The season wildcard already matches three-digit episode numbers.
    assert_eq!(names[1]["name"], "S01E%");
}

#[test]
fn should_search_for_three_digit_episode_number_for_anime() {
    for (episode, expected) in [(99, "S01E%99"), (100, "S01E100")] {
        let chain = btn_search(
            433335,
            &Search::AnimeEpisode {
                absolute: episode,
                season: 1,
                episode,
            },
        );
        assert_eq!(chain.tiers.len(), 1);
        let names = queried(&chain, 0);
        assert_eq!(names[0]["name"], expected);
        assert_eq!(names[1]["name"], "Season 1%");
    }
}

#[test]
fn broadcasthenet_pages_and_specials() {
    let chain = btn_search(433335, &single(1, 2));
    let requests = chain.all().next().unwrap().requests();
    assert_eq!(requests.len(), 10, "ten pages");
    for (page, r) in requests.iter().enumerate() {
        let b = body(r);
        assert_eq!(b["method"], "getTorrents");
        assert_eq!(b["params"][0], "abc");
        assert_eq!(b["params"][2], 100);
        assert_eq!(b["params"][3], page * 100);
        assert_eq!(r.url, "https://api.broadcasthe.net/");
    }
    let special = btn_search(
        433335,
        &Search::Special {
            episode_query_titles: vec!["Series Title The Pilot".into()],
            episode_titles: vec!["The Pilot".into()],
        },
    );
    assert_eq!(queried(&special, 0)[0]["name"], "%Pilot%");
    // The TVRage id is the fallback.
    let settings = btn_settings("https://api.broadcasthe.net/");
    let caps = Capabilities::default();
    let mut c = criteria(0, None);
    c.ids.tv_rage = 4055;
    let chain = generator(&settings, &caps, Dialect::BroadcastheNet).search(&c, &single(1, 2));
    assert_eq!(queried(&chain, 0)[0]["tvrage"], "4055");
    // RSS: the last day's torrents.
    let recent = generator(&settings, &caps, Dialect::BroadcastheNet).recent();
    assert_eq!(recent.tiers.len(), 1);
    assert_eq!(
        queried(&recent, 0)[0],
        serde_json::json!({"age": "<=86400"})
    );
    // With the last torrent seen, from 100 before it first.
    let recent = broadcasthenet::recent(&settings, Some(1234));
    assert_eq!(recent.tiers.len(), 2);
    assert_eq!(queried(&recent, 0)[0], serde_json::json!({"id": ">=1134"}));
    assert_eq!(broadcasthenet::torrent_id("BTN-1234"), Some(1234));
}

// ---- The client against a mock of the APIs ---------------------------------------------------

#[derive(Clone, Default)]
struct Mock {
    log: Arc<Mutex<Vec<String>>>,
    /// HTTP status the BroadcasTheNet mock answers with (200: the recorded feed).
    btn_status: Arc<Mutex<u16>>,
}

async fn btn(State(m): State<Mock>, headers: HeaderMap, body: String) -> axum::response::Response {
    assert_eq!(headers["content-type"], "application/json");
    let rpc: Value = serde_json::from_str(&body).unwrap();
    m.log
        .lock()
        .unwrap()
        .push(format!("btn {}", rpc["params"][1]));
    let status = *m.btn_status.lock().unwrap();
    if status != 200 {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    (
        [("content-type", "application/json")],
        fixture("broadcasthenet_recent_feed.json"),
    )
        .into_response()
}

async fn filelist_api(State(m): State<Mock>, headers: HeaderMap) -> axum::response::Response {
    // "someuser:somepass"
    assert_eq!(headers["authorization"], "Basic c29tZXVzZXI6c29tZXBhc3M=");
    m.log.lock().unwrap().push("filelist".into());
    (
        [("content-type", "application/json; charset=utf-8")],
        fixture("filelist_recent_feed.json"),
    )
        .into_response()
}

async fn hdbits_api(State(m): State<Mock>, body: String) -> axum::response::Response {
    let q: Value = serde_json::from_str(&body).unwrap();
    m.log
        .lock()
        .unwrap()
        .push(format!("hdbits {}", q["passkey"]));
    if q["passkey"] == "wrong" {
        return (
            [("content-type", "application/json")],
            r#"{"status":5,"message":"Invalid authentication credentials"}"#,
        )
            .into_response();
    }
    (
        [("content-type", "application/json")],
        fixture("hdbits_recent_feed_long_ids.json"),
    )
        .into_response()
}

async fn rss(State(m): State<Mock>, headers: HeaderMap) -> axum::response::Response {
    assert_eq!(headers["cookie"], "uid=1; pass=abc");
    m.log.lock().unwrap().push("rss".into());
    (
        [("content-type", "application/rss+xml")],
        fixture("torrent_rss_immortal_seed.xml"),
    )
        .into_response()
}

async fn start() -> (Mock, String, IndexerClient) {
    let mock = Mock::default();
    *mock.btn_status.lock().unwrap() = 200;
    let app = axum::Router::new()
        .route("/btn/", post(btn))
        .route("/filelist/api.php", get(filelist_api))
        .route("/hdbits/api/torrents", post(hdbits_api))
        .route("/rss", get(rss))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = IndexerClient::new(reqwest::Client::new()).with_rate_limit(Duration::ZERO);
    (mock, format!("http://{addr}"), client)
}

fn indexer(dialect: Dialect, settings: Settings) -> Indexer {
    Indexer {
        id: 3,
        name: "Mock".into(),
        dialect,
        settings,
    }
}

#[tokio::test]
async fn broadcasthenet_through_the_client() {
    let (mock, base, client) = start().await;
    let btn = indexer(
        Dialect::BroadcastheNet,
        btn_settings(&format!("{base}/btn/")),
    );
    let report = client.test(&btn).await.unwrap();
    assert_eq!(report.releases_found, 2);
    let caps = client.capabilities(&btn).await.unwrap();
    let chain = client.generator(&btn, &caps).recent();
    let releases = client.fetch(&btn, &chain, false).await.unwrap();
    // The API's links take the request's scheme (http here).
    assert_eq!(
        releases[0].release.download_url.as_deref(),
        Some(
            "http://broadcasthe.net/torrents.php?action=download&id=123&authkey=123&torrent_pass=123"
        )
    );
    // An RSS sync after "BTN-1234" asks for the torrents from 100 before it.
    let last = LastRelease {
        publish_date: Utc.with_ymd_and_hms(2014, 9, 16, 21, 0, 0).unwrap(),
        download_url: None,
        guid: "BTN-1234".into(),
        title: "x".into(),
    };
    mock.log.lock().unwrap().clear();
    let fetched = client
        .fetch_recent(&btn, &chain, Some(&last))
        .await
        .unwrap();
    assert_eq!(fetched.releases.len(), 2);
    assert_eq!(
        mock.log.lock().unwrap().as_slice(),
        [r#"btn {"id":">=1134"}"#],
        "a short page ends the paging; the id tier found releases"
    );

    // should_back_off_on_bad_request / _and_report_api_key_invalid / _on_unknown_method /
    // _api_limit_reached
    for (status, expected) in [
        (400, "indexer"),
        (401, "apikey"),
        (404, "indexer"),
        (503, "limit"),
    ] {
        *mock.btn_status.lock().unwrap() = status;
        let err = client.fetch(&btn, &chain, true).await.unwrap_err();
        let kind = match err {
            IndexerError::Indexer(_) => "indexer",
            IndexerError::ApiKey(_) => "apikey",
            IndexerError::RequestLimit { .. } => "limit",
            other => panic!("{status}: {other:?}"),
        };
        assert_eq!(kind, expected, "{status}");
    }
}

#[tokio::test]
async fn filelist_and_hdbits_through_the_client() {
    let (mock, base, client) = start().await;
    let fl = indexer(
        Dialect::FileList,
        filelist_settings(&format!("{base}/filelist")),
    );
    assert_eq!(client.test(&fl).await.unwrap().releases_found, 2);

    let hd = indexer(Dialect::HdBits, hdbits_settings(&format!("{base}/hdbits")));
    assert_eq!(client.test(&hd).await.unwrap().releases_found, 2);
    let mut wrong = hdbits_settings(&format!("{base}/hdbits"));
    wrong.api_key = Some("wrong".into());
    assert!(matches!(
        client.test(&indexer(Dialect::HdBits, wrong)).await,
        Err(IndexerError::Indexer(m)) if m.contains("AuthFailed")
    ));
    assert_eq!(
        mock.log.lock().unwrap().as_slice(),
        ["filelist", r#"hdbits "fakekey""#, r#"hdbits "wrong""#]
    );
}

#[tokio::test]
async fn torrent_rss_detects_the_layout_once() {
    let (mock, base, client) = start().await;
    let rss = indexer(
        Dialect::TorrentRss,
        Settings {
            base_url: format!("{base}/rss/"),
            cookie: Some("uid=1; pass=abc".into()),
            ..Settings::default()
        },
    );
    let caps = client.capabilities(&rss).await.unwrap();
    let chain = client.generator(&rss, &caps).recent();
    let releases = client.fetch(&rss, &chain, false).await.unwrap();
    assert_eq!(releases.len(), 50);
    let r = &releases[0].release;
    assert_eq!((r.size, r.seeders, r.peers), (984078090, Some(6), Some(8)));
    assert_eq!(client.fetch(&rss, &chain, false).await.unwrap().len(), 50);
    assert_eq!(
        mock.log.lock().unwrap().len(),
        2,
        "no extra detection request"
    );
    // RSS-only: nothing to search.
    let search = client
        .generator(&rss, &caps)
        .search(&criteria(1, None), &Search::Season { season: 1 });
    assert_eq!(search.all().count(), 0);
}

/// `BroadcastheNetRequestGenerator`: daily searches by date, then a tier by season and episode.
#[test]
fn daily_searches_add_a_season_and_episode_tier() {
    let chain = btn_search(
        433335,
        &Search::DailyEpisode {
            date: NaiveDate::from_ymd_opt(2023, 4, 2).unwrap(),
            episodes: vec![(2023, 41)],
        },
    );
    assert_eq!(chain.tiers.len(), 2);
    assert_eq!(queried(&chain, 0)[0]["name"], "2023.04.02");
    assert_eq!(queried(&chain, 1)[0]["name"], "S2023E41");
    let season = btn_search(
        433335,
        &Search::DailySeason {
            year: 2023,
            episodes: vec![(2023, 40), (2023, 41)],
        },
    );
    assert_eq!(queried(&season, 0)[0]["name"], "2023%");
    let names: Vec<String> = queried(&season, 1)
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["S2023E40", "S2023E41"]);
}
