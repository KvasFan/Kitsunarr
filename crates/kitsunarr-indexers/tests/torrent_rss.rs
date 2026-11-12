//! Sonarr's Torrent RSS (`TorrentRssIndexerFixture`, `TorrentRssSettingsDetectorFixture`,
//! `TorrentRssParserFactoryFixture`), IPTorrents, TorrentLeech and Fanzub tests, replayed on
//! its recorded feeds (`tests/fixtures/indexers/sonarr/`).

use std::cell::Cell;

use chrono::{DateTime, TimeZone, Utc};
use kitsunarr_domain::DownloadProtocol;
use kitsunarr_indexers::request::search_mode;
use kitsunarr_indexers::torrent_rss::{
    self, ParserFactory, ParserSettings, RssParser, detect, parser_for,
};
use kitsunarr_indexers::{
    Capabilities, Criteria, Dialect, Generator, IndexerError, Release, Search, SeriesIds, Settings,
    parse_feed,
};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../tests/fixtures/indexers/sonarr/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// `TorrentRssIndexerFixture._indexerUrl`.
const URL: &str = "http://my.indexer.tv/recent";

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

/// `FetchRecent` of a Torrent RSS indexer: the feed's layout is detected, then the feed is
/// parsed with it and invalid releases are dropped.
fn fetch_recent(file: &str, allow_zero_size: bool) -> Vec<Release> {
    let body = fixture(file);
    let settings = detect(&body, URL, allow_zero_size).unwrap();
    parser_for(&settings)
        .parse(&body, URL)
        .unwrap()
        .into_iter()
        .filter(kitsunarr_indexers::feed::is_valid)
        .collect()
}

#[test]
fn should_parse_recent_feed_from_immortal_seed() {
    let releases = fetch_recent("torrent_rss_immortal_seed.xml", false);
    assert_eq!(releases.len(), 50);
    let r = &releases[0];
    assert_eq!(
        r.title,
        "Conan.2015.02.05.Jeff.Bridges.720p.HDTV.X264-CROOKS"
    );
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some("https://immortalseed.me/download.php?type=rss&secret_key=12345678910&id=374534")
    );
    assert_eq!(r.info_url, None);
    assert_eq!(r.comment_url, None);
    assert_eq!(r.publish_date, utc(2015, 2, 6, 13, 32, 26));
    assert_eq!(r.size, 984078090);
    assert_eq!(r.info_hash, None);
    assert_eq!(r.magnet_url, None);
    assert_eq!((r.seeders, r.peers), (Some(6), Some(8)));
}

#[test]
fn should_parse_recent_feed_from_ezrss() {
    let releases = fetch_recent("torrent_rss_ezrss.xml", false);
    assert_eq!(releases.len(), 3);
    let r = &releases[0];
    assert_eq!(
        r.title,
        "S4C I Grombil Cyfandir Pell American Interior [PDTV - MVGROUP]"
    );
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some("http://re.zoink.it/20a4ed4eFC")
    );
    assert_eq!(
        r.info_url.as_deref(),
        Some(
            "http://eztv.it/ep/58439/s4c-i-grombil-cyfandir-pell-american-interior-pdtv-x264-mvgroup/"
        )
    );
    assert_eq!(
        r.comment_url.as_deref(),
        Some("http://eztv.it/forum/discuss/58439/")
    );
    // "Mon, 15 Sep 2014 13:39:00 -0500".
    assert_eq!(r.publish_date, utc(2014, 9, 15, 18, 39, 0));
    assert_eq!(r.size, 796606175);
    assert_eq!(
        r.info_hash.as_deref(),
        Some("20FC4FBFA88272274AC671F857CC15144E9AA83E")
    );
    assert_eq!(
        r.magnet_url.as_deref(),
        Some(
            "magnet:?xt=urn:btih:ED6E7P5IQJZCOSWGOH4FPTAVCRHJVKB6&dn=S4C.I.Grombil.Cyfandir.Pell.American.Interior.PDTV.x264-MVGroup"
        )
    );
    assert_eq!((r.seeders, r.peers), (None, None));
}

#[test]
fn should_parse_recent_feed_from_showrss_info() {
    let releases = fetch_recent("torrent_rss_showrss_info.xml", true);
    assert_eq!(releases.len(), 5);
    let r = &releases[0];
    assert_eq!(r.title, "The Voice 8x25");
    let magnet = "magnet:?xt=urn:btih:96CD620BEDA3EFD7C4D7746EF94549D03A2EB13B&dn=The+Voice+S08E25+WEBRip+x264+WNN&tr=udp://tracker.coppersurfer.tk:6969/announce&tr=udp://tracker.leechers-paradise.org:6969&tr=udp://open.demonii.com:1337";
    assert_eq!(r.download_url.as_deref(), Some(magnet));
    assert_eq!(r.info_url, None);
    assert_eq!(r.comment_url, None);
    assert_eq!(r.publish_date, utc(2015, 5, 15, 8, 30, 1));
    assert_eq!(r.size, 0);
    assert_eq!(
        r.info_hash.as_deref(),
        Some("96CD620BEDA3EFD7C4D7746EF94549D03A2EB13B")
    );
    assert_eq!(r.magnet_url.as_deref(), Some(magnet));
    assert_eq!((r.seeders, r.peers), (None, None));
}

#[test]
fn should_parse_recent_feed_from_doki() {
    let releases = fetch_recent("torrent_rss_doki.xml", true);
    assert_eq!(releases.len(), 5);
    let r = &releases[0];
    assert_eq!(
        r.title,
        "[Doki] PriPara   50 (848x480 h264 AAC) [6F0B49FD] mkv"
    );
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "http://tracker.anime-index.org/download.php?id=82d8ad84403e01a7786130905ca169a3429e657f&f=%5BDoki%5D+PriPara+-+50+%28848x480+h264+AAC%29+%5B6F0B49FD%5D.mkv.torrent"
        )
    );
    assert_eq!(
        (r.info_url.as_deref(), r.comment_url.as_deref()),
        (None, None)
    );
    assert_eq!(r.publish_date, utc(2015, 7, 2, 8, 18, 29));
    assert_eq!(r.size, 0);
    assert_eq!(
        (r.info_hash.as_deref(), r.magnet_url.as_deref()),
        (None, None)
    );
    assert_eq!((r.seeders, r.peers), (None, None));
}

#[test]
fn should_parse_recent_feed_from_extra_torrents() {
    let releases = fetch_recent("torrent_rss_extra_torrents.xml", false);
    assert_eq!(releases.len(), 5);
    let r = &releases[0];
    assert_eq!(
        r.title,
        "One.Piece.E334.D ED.720p.HDTV.x264-W4F-={SPARROW}=-"
    );
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "http://ac.me/download/4722030/One.Piece.E334.D+ED.720p.HDTV.x264-W4F-%3D%7BSPARROW%7D%3D-.torrent"
        )
    );
    assert_eq!(
        (r.info_url.as_deref(), r.comment_url.as_deref()),
        (None, None)
    );
    assert_eq!(r.publish_date, utc(2016, 2, 21, 9, 51, 54));
    assert_eq!(r.size, 562386947);
    assert_eq!(
        (r.info_hash.as_deref(), r.magnet_url.as_deref()),
        (None, None)
    );
    assert_eq!((r.seeders, r.peers), (None, None));
}

#[test]
fn should_parse_recent_feed_from_lime_torrents() {
    let releases = fetch_recent("torrent_rss_lime_torrents.xml", false);
    assert_eq!(releases.len(), 5);
    let r = &releases[0];
    assert_eq!(r.title, "The Expanse 2x04 (720p-HDTV-x264-SVA)[VTV]");
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "http://itorrents.org/torrent/51C578C9823DD58F6EEA287C368ED935843D63AB.torrent?title=The-Expanse-2x04-(720p-HDTV-x264-SVA)[VTV]"
        )
    );
    assert_eq!(r.info_url, None);
    assert_eq!(
        r.comment_url.as_deref(),
        Some(
            "http://www.limetorrents.cc/The-Expanse-2x04-(720p-HDTV-x264-SVA)[VTV]-torrent-8643587.html"
        )
    );
    // "16 Feb 2017 05:24:26 +0300".
    assert_eq!(r.publish_date, utc(2017, 2, 16, 2, 24, 26));
    assert_eq!(r.size, 880496711);
    assert_eq!(
        (r.info_hash.as_deref(), r.magnet_url.as_deref()),
        (None, None)
    );
    assert_eq!((r.seeders, r.peers), (None, None));
}

#[test]
fn should_parse_recent_feed_from_animetosho_without_size() {
    let releases = fetch_recent("torrent_rss_animetosho_no_size.xml", false);
    assert_eq!(releases.len(), 2);
    let r = &releases[0];
    assert_eq!(r.title, "[FFF] Ore Monogatari!! - Vol.01 [BD][720p-AAC]");
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "http://storage.animetosho.org/torrents/85a570f25067f69b3c83b901ce6c00c491345288/%5BFFF%5D%20Ore%20Monogatari%21%21%20-%20Vol.01%20%5BBD%5D%5B720p-AAC%5D.torrent"
        )
    );
    assert_eq!(r.info_url, None);
    assert_eq!(
        r.comment_url.as_deref(),
        Some("https://animetosho.org/view/fff-ore-monogatari-vol-01-bd-720p-aac.1009077")
    );
    assert_eq!(r.publish_date, utc(2016, 8, 2, 13, 48, 4));
    assert_eq!(
        r.size,
        (1.366_f64 * 1024.0 * 1024.0 * 1024.0).round() as i64
    );
    assert_eq!(
        (r.info_hash.as_deref(), r.magnet_url.as_deref()),
        (None, None)
    );
    assert_eq!((r.seeders, r.peers), (None, None));
}

#[test]
fn should_parse_multi_enclosure_from_animetosho() {
    let releases = fetch_recent("torrent_rss_animetosho_no_size.xml", false);
    assert_eq!(releases.len(), 2);
    let r = releases.last().unwrap();
    assert_eq!(r.title, "DAYS - 05 (1280x720 HEVC2 AAC).mkv");
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "http://storage.animetosho.org/torrents/4b58360143d59a55cbd922397a3eaa378165f3ff/DAYS%20-%2005%20%281280x720%20HEVC2%20AAC%29.torrent"
        )
    );
}

#[test]
fn should_parse_recent_feed_from_alpha_ratio() {
    let releases = fetch_recent("torrent_rss_alpha_ratio.xml", false);
    assert_eq!(releases.len(), 2);
    let r = releases.last().unwrap();
    assert_eq!(
        r.title,
        "TvHD 465860 465831 WWE.RAW.2016.11.28.720p.HDTV.x264-KYR"
    );
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "https://alpharatio.cc/torrents.php?action=download&authkey=private_auth_key&torrent_pass=private_torrent_pass&id=465831"
        )
    );
}

#[test]
fn should_parse_recent_feed_from_evolution_world_without_size() {
    let releases = fetch_recent("torrent_rss_evolution_world.xml", true);
    assert_eq!(releases.len(), 2);
    let r = &releases[0];
    assert_eq!(
        r.title,
        "[TVShow --> TVShow Bluray 720p] Fargo S01 Complete Season 1 720p BRRip DD5.1 x264-PSYPHER [SEEDERS (3)/LEECHERS (0)]"
    );
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "http://ew.pw/download.php?id=dea071a7a62a0d662538d46402fb112f30b8c9fa&f=Fargo%20S01%20Complete%20Season%201%20720p%20BRRip%20DD5.1%20x264-PSYPHER.torrent&auth=secret"
        )
    );
    assert_eq!(
        (r.info_url.as_deref(), r.comment_url.as_deref()),
        (None, None)
    );
    assert_eq!(r.publish_date, utc(2017, 8, 13, 22, 21, 43));
    assert_eq!(r.size, 0);
    assert_eq!(
        (r.info_hash.as_deref(), r.magnet_url.as_deref()),
        (None, None)
    );
    assert_eq!((r.seeders, r.peers), (None, None));
}

/// `should_record_indexer_failure_if_unsupported_feed`: the feed fails (the client records the
/// failure for the back-off).
#[test]
fn should_fail_an_unsupported_feed() {
    let body = fixture("torrent_rss_invalid_torrentday_no_pubdate.xml");
    assert!(detect(&body, URL, false).is_err());
    assert!(parse_feed(&body, Dialect::TorrentRss, URL).is_err());
}

// ---- TorrentRssSettingsDetectorFixture --------------------------------------------------------

fn detected(file: &str, allow_zero_size: bool) -> ParserSettings {
    detect(&fixture(file), URL, allow_zero_size).unwrap_or_else(|e| panic!("{file}: {e}"))
}

fn layout(
    url: bool,
    length: bool,
    size_in_description: bool,
    seeders: bool,
    size_element: Option<&str>,
) -> ParserSettings {
    ParserSettings {
        use_eztv_format: false,
        use_enclosure_url: url,
        use_enclosure_length: length,
        parse_size_in_description: size_in_description,
        parse_seeders_in_description: seeders,
        size_element_name: size_element.map(str::to_string),
    }
}

#[test]
fn should_detect_rss_settings() {
    assert_eq!(
        detected("torrent_rss_ezrss.xml", false),
        ParserSettings {
            use_eztv_format: true,
            ..ParserSettings::default()
        }
    );
    let cases: &[(&str, bool, ParserSettings)] = &[
        (
            "torrent_rss_speed_cd.xml",
            false,
            layout(false, false, true, false, None),
        ),
        (
            "torrent_rss_immortal_seed.xml",
            false,
            layout(false, false, true, true, None),
        ),
        (
            "torrent_rss_showrss_info.xml",
            true,
            layout(true, false, false, false, None),
        ),
        (
            "torrent_rss_transmit_the_net.xml",
            false,
            layout(true, false, true, false, None),
        ),
        (
            "torrent_rss_bithdtv.xml",
            false,
            layout(false, false, false, false, Some("size")),
        ),
        (
            "torrent_rss_doki.xml",
            true,
            layout(true, false, false, false, None),
        ),
        (
            "torrent_rss_extra_torrents.xml",
            true,
            layout(true, true, false, false, None),
        ),
        (
            "torrent_rss_lime_torrents.xml",
            true,
            layout(true, true, false, false, None),
        ),
        (
            "torrent_rss_alpha_ratio.xml",
            true,
            layout(false, false, true, false, None),
        ),
        (
            "torrent_rss_animetosho_no_size.xml",
            true,
            layout(true, false, true, false, None),
        ),
    ];
    for (file, allow_zero_size, expected) in cases {
        assert_eq!(&detected(file, *allow_zero_size), expected, "{file}");
    }
}

#[test]
fn should_detect_recent_feed() {
    for file in [
        "iptorrents.xml",
        "nyaa.xml",
        "torznab_hdaccess_net.xml",
        "torznab_tpb.xml",
        "torznab_animetosho.xml",
    ] {
        detected(file, false);
    }
}

fn rejected(file: &str, allow_zero_size: bool) -> String {
    match detect(&fixture(file), URL, allow_zero_size) {
        Err(IndexerError::Indexer(m)) => m,
        other => panic!("{file}: {other:?}"),
    }
}

#[test]
fn should_reject_recent_feed_with_invalid_downloadurl() {
    let m = rejected(
        "torrent_rss_invalid_immortal_seed_invalid_download_url.xml",
        false,
    );
    assert!(m.contains("download url"), "{m}");
}

#[test]
fn should_reject_recent_feed_without_pub_date() {
    let m = rejected("torrent_rss_invalid_torrentday_no_pubdate.xml", false);
    assert!(m.contains("RSS feed must have a pubDate"), "{m}");
}

#[test]
fn should_detect_feed_without_size() {
    for file in [
        "torrentleech.xml",
        "torrent_rss_invalid_eztv_invalid_size.xml",
        "torrent_rss_invalid_immortal_seed_invalid_size.xml",
        "torrent_rss_doki.xml",
    ] {
        let s = detected(file, true);
        assert!(!s.use_enclosure_length, "{file}");
        assert!(!s.parse_size_in_description, "{file}");
        assert_eq!(s.size_element_name, None, "{file}");
    }
}

#[test]
fn should_reject_feed_without_size() {
    for file in [
        "torrent_rss_invalid_eztv_invalid_size.xml",
        "torrent_rss_invalid_immortal_seed_invalid_size.xml",
    ] {
        let m = rejected(file, false);
        assert!(m.contains("content size"), "{file}: {m}");
    }
}

// ---- TorrentRssParserFactoryFixture -----------------------------------------------------------

#[test]
fn factory_returns_the_ezrss_or_generic_parser() {
    let factory = ParserFactory::default();
    let parser = factory
        .get_parser("1", || {
            Ok(ParserSettings {
                use_eztv_format: true,
                ..ParserSettings::default()
            })
        })
        .unwrap();
    assert_eq!(parser, RssParser::ezrss());

    let parser = factory
        .get_parser("2", || {
            Ok(ParserSettings {
                parse_seeders_in_description: true,
                parse_size_in_description: true,
                size_element_name: Some("Hello".into()),
                ..ParserSettings::default()
            })
        })
        .unwrap();
    assert!(parser.torrent && !parser.ezrss);
    assert!(parser.parse_seeders_in_description);
    assert!(parser.parse_size_in_description);
    assert_eq!(parser.size_element.as_deref(), Some("Hello"));
}

#[test]
fn factory_caches_successes_per_settings_but_not_failures() {
    let factory = ParserFactory::default();
    let detections = Cell::new(0);
    let ok = || {
        detections.set(detections.get() + 1);
        Ok(ParserSettings {
            use_enclosure_length: true,
            ..ParserSettings::default()
        })
    };
    let failed = || {
        detections.set(detections.get() + 1);
        Err(IndexerError::Indexer("Could not parse feed".into()))
    };
    let first = Settings {
        base_url: "http://my.indexer.com/".into(),
        ..Settings::default()
    };
    let other_url = Settings {
        base_url: "http://my.other.indexer.com/".into(),
        ..Settings::default()
    };
    let other_settings = Settings {
        allow_zero_size: true,
        ..first.clone()
    };
    let key = |s: &Settings| serde_json::to_string(s).unwrap();

    // should_throw_on_failure, should_not_cache_failure
    assert!(factory.get_parser(&key(&first), failed).is_err());
    let a = factory.get_parser(&key(&first), ok).unwrap();
    assert_eq!(detections.get(), 2);
    // should_cache_settings_for_same_baseurl
    let b = factory.get_parser(&key(&first), ok).unwrap();
    assert_eq!(a, b);
    assert_eq!(detections.get(), 2);
    // should_not_cache_settings_for_different_baseurl / _settings
    factory.get_parser(&key(&other_url), ok).unwrap();
    factory.get_parser(&key(&other_settings), ok).unwrap();
    assert_eq!(detections.get(), 4);
}

#[test]
fn rss_parse_size() {
    assert_eq!(
        torrent_rss::parse_size("Size: 1.3 GB", true),
        Ok(1395864371)
    );
    assert_eq!(torrent_rss::parse_size("1.3 GB", false), Ok(1_300_000_000));
    assert_eq!(torrent_rss::parse_size("700 MiB", false), Ok(734003200));
    assert_eq!(torrent_rss::parse_size("12345", true), Ok(12345));
    assert_eq!(torrent_rss::parse_size("1,024.5 KB", true), Ok(1049088));
    assert_eq!(torrent_rss::parse_size("no size", true), Ok(0));
    // A fraction's digits are not a size of their own.
    assert_eq!(torrent_rss::parse_size("v1.5MB", true), Ok(1572864));
}

#[test]
fn magnet_info_hashes() {
    assert_eq!(
        torrent_rss::magnet_info_hash("magnet:?xt=urn:btih:ED6E7P5IQJZCOSWGOH4FPTAVCRHJVKB6&dn=x")
            .as_deref(),
        Some("20FC4FBFA88272274AC671F857CC15144E9AA83E")
    );
    assert_eq!(
        torrent_rss::magnet_info_hash(
            "magnet:?xt=urn:btih:96cd620beda3efd7c4d7746ef94549d03a2eb13b"
        )
        .as_deref(),
        Some("96CD620BEDA3EFD7C4D7746EF94549D03A2EB13B")
    );
    assert_eq!(torrent_rss::magnet_info_hash("magnet:?dn=x"), None);
    assert_eq!(torrent_rss::magnet_info_hash("http://x/"), None);
}

// ---- IPTorrentsFixture -----------------------------------------------------------------------

fn ipt(url: &str) -> Result<(), String> {
    Settings {
        base_url: url.into(),
        ..Settings::default()
    }
    .validate(Dialect::IpTorrents)
}

#[test]
fn iptorrents_feed_url_validation() {
    // should_validate_old_feed_format / should_validate_new_feed_format
    assert_eq!(
        ipt("https://iptorrents.com/torrents/rss?u=snip;tp=snip;3;80;93;37;download"),
        Ok(())
    );
    assert_eq!(
        ipt("https://iptorrents.com/t.rss?u=USERID;tp=APIKEY;3;80;93;37;download"),
        Ok(())
    );
    // should_not_validate_bad_format
    assert!(ipt("http://fake.com/").is_err());
    // should_not_validate_no_download_format
    assert_eq!(
        ipt("https://iptorrents.com/t.rss?u=USERID;tp=APIKEY;3;80;93;37"),
        Err("Use Direct Download Url (;download)".into())
    );
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

fn criteria(titles: &[&str]) -> Criteria {
    Criteria {
        ids: SeriesIds {
            tvdb: 20,
            ..SeriesIds::default()
        },
        scene_titles: titles.iter().map(|t| t.to_string()).collect(),
        search_mode: search_mode::DEFAULT,
        anime: true,
    }
}

#[test]
fn rss_only_indexers_request_their_feed_and_never_search() {
    let caps = Capabilities::default();
    let feed = |dialect: Dialect, settings: &Settings| {
        let chain = generator(settings, &caps, dialect).recent();
        let requests: Vec<_> = chain.all().flat_map(|p| p.requests()).collect();
        assert_eq!(requests.len(), 1);
        let search = generator(settings, &caps, dialect).search(
            &criteria(&["Show"]),
            &Search::SingleEpisode {
                season: 1,
                episode: 2,
            },
        );
        assert_eq!(search.all().count(), 0, "{dialect:?} is RSS only");
        requests.into_iter().next().unwrap()
    };
    let ipt = Settings {
        base_url: "https://iptorrents.com/t.rss?u=1;tp=2;download".into(),
        ..Settings::default()
    };
    assert_eq!(feed(Dialect::IpTorrents, &ipt).url, ipt.base_url);
    let tl = Settings {
        base_url: "http://rss.torrentleech.org/ ".into(),
        api_key: Some("abc".into()),
        ..Settings::default()
    };
    assert_eq!(
        feed(Dialect::Torrentleech, &tl).url,
        "http://rss.torrentleech.org/abc"
    );
    let rss = Settings {
        base_url: " http://my.indexer.tv/recent/".into(),
        cookie: Some("uid=1; pass=abc".into()),
        ..Settings::default()
    };
    let request = feed(Dialect::TorrentRss, &rss);
    assert_eq!(request.url, "http://my.indexer.tv/recent");
    assert_eq!(request.cookie.as_deref(), Some("uid=1; pass=abc"));
}

#[test]
fn should_parse_recent_feed_from_iptorrents() {
    let releases = parse_feed(
        &fixture("iptorrents.xml"),
        Dialect::IpTorrents,
        "http://fake.com/",
    )
    .unwrap();
    assert_eq!(releases.len(), 5);
    let r = &releases[0];
    assert_eq!(r.title, "24 S03E12 720p WEBRip h264-DRAWER");
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "http://iptorrents.com/download.php/1234/24.S03E12.720p.WEBRip.h264-DRAWER.torrent?torrent_pass=abcd"
        )
    );
    assert_eq!(
        (r.info_url.as_deref(), r.comment_url.as_deref()),
        (None, None)
    );
    assert_eq!(r.publish_date, utc(2014, 5, 12, 19, 6, 34));
    assert_eq!(r.size, 1471026299);
    assert_eq!(
        (r.info_hash.as_deref(), r.magnet_url.as_deref()),
        (None, None)
    );
    assert_eq!((r.seeders, r.peers), (None, None));
}

// ---- TorrentleechFixture ---------------------------------------------------------------------

#[test]
fn should_parse_recent_feed_from_torrentleech() {
    let releases = parse_feed(
        &fixture("torrentleech.xml"),
        Dialect::Torrentleech,
        "http://rss.torrentleech.org/",
    )
    .unwrap();
    assert_eq!(releases.len(), 5);
    let r = &releases[0];
    assert_eq!(r.title, "Classic Car Rescue S02E04 720p HDTV x264-C4TV");
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some(
            "http://www.torrentleech.org/rss/download/513575/1234/Classic.Car.Rescue.S02E04.720p.HDTV.x264-C4TV.torrent"
        )
    );
    assert_eq!(
        r.info_url.as_deref(),
        Some("http://www.torrentleech.org/torrent/513575")
    );
    assert_eq!(
        r.comment_url.as_deref(),
        Some("http://www.torrentleech.org/torrent/513575#comments")
    );
    assert_eq!(r.publish_date, utc(2014, 5, 12, 19, 15, 28));
    assert_eq!(r.size, 0);
    assert_eq!(
        (r.info_hash.as_deref(), r.magnet_url.as_deref()),
        (None, None)
    );
    assert_eq!((r.seeders, r.peers), (Some(1), Some(7 + 1)));
}

// ---- FanzubFixture / FanzubRequestGeneratorFixture --------------------------------------------

#[test]
fn should_parse_recent_feed_from_fanzub() {
    let releases = parse_feed(
        &fixture("fanzub.xml"),
        Dialect::Fanzub,
        "http://fanzub.com/rss/?cat=anime&max=100",
    )
    .unwrap();
    assert_eq!(releases.len(), 3);
    let r = &releases[0];
    assert_eq!(r.title, "[Vivid] Hanayamata - 10 [A33D6606]");
    assert_eq!(r.protocol, DownloadProtocol::Usenet);
    assert_eq!(
        r.download_url.as_deref(),
        Some("http://fanzub.com/nzb/296464/Vivid%20Hanayamata%20-%2010.nzb")
    );
    assert_eq!(
        (r.info_url.as_deref(), r.comment_url.as_deref()),
        (None, None)
    );
    assert_eq!(r.publish_date, utc(2014, 9, 13, 12, 56, 53));
    assert_eq!(r.size, 556246858);
}

fn fanzub_query(standard: bool, search: &Search) -> Option<String> {
    let settings = Settings {
        base_url: "http://127.0.0.1:1234/".into(),
        anime_standard_format_search: standard,
        ..Settings::default()
    };
    let caps = Capabilities::default();
    let chain = generator(&settings, &caps, Dialect::Fanzub)
        .search(&criteria(&["Naruto Shippuuden"]), search);
    let urls: Vec<String> = chain.all().flat_map(|p| p.pages()).collect();
    assert!(urls.len() <= 1, "{urls:?}");
    urls.into_iter().next()
}

#[test]
fn fanzub_searches() {
    let season = Search::Season { season: 1 };
    let anime = Search::AnimeEpisode {
        absolute: 9,
        season: 1,
        episode: 9,
    };
    let anime_season = Search::AnimeSeason {
        season: 3,
        season_scene_titles: Vec::new(),
        pack_search_only: false,
    };
    // should_not_search_season
    assert_eq!(fanzub_query(false, &season), None);
    // should_search_season
    let url = fanzub_query(true, &season).unwrap();
    assert!(
        url.contains(r#"q="Naruto+Shippuuden%20S01"|"Naruto+Shippuuden%20-%20S01""#),
        "{url}"
    );
    assert!(
        url.starts_with("http://127.0.0.1:1234/?cat=anime&max=100&q="),
        "{url}"
    );
    // should_use_only_absolute_numbering_for_anime_search
    let url = fanzub_query(false, &anime).unwrap();
    assert!(
        url.contains(r#"q="Naruto+Shippuuden%2009"|"Naruto+Shippuuden%20-%2009""#),
        "{url}"
    );
    assert!(!url.contains("S01E09"), "{url}");
    // should_also_use_standard_numbering_for_anime_search
    let url = fanzub_query(true, &anime).unwrap();
    assert!(
        url.contains(r#"q="Naruto+Shippuuden%2009"|"Naruto+Shippuuden%20-%2009"|"Naruto+Shippuuden%20S01E09"|"Naruto+Shippuuden%20-%20S01E09""#),
        "{url}"
    );
    // should_search_by_standard_season_number
    let url = fanzub_query(true, &anime_season).unwrap();
    assert!(
        url.contains(r#"q="Naruto+Shippuuden%20S03"|"Naruto+Shippuuden%20-%20S03""#),
        "{url}"
    );
    // Single episodes are not searched.
    assert_eq!(
        fanzub_query(
            true,
            &Search::SingleEpisode {
                season: 1,
                episode: 2
            }
        ),
        None
    );
}

#[test]
fn zz_debug() {
    let p = RssParser {
        use_enclosure_url: true,
        parse_size_in_description: true,
        ..RssParser::torrent()
    };
    let r = p.parse(&fixture("torrent_rss_transmit_the_net.xml"), URL);
    eprintln!("{r:?}");
    let doc = kitsunarr_indexers::xml::parse(&fixture("torrent_rss_transmit_the_net.xml")).unwrap();
    let item = doc.child("channel").unwrap().child("item").unwrap();
    let d = &item.child("description").unwrap().text;
    eprintln!("DESC {:?} {}", torrent_rss::parse_size(d, true), d.len());
    eprintln!("TAIL {:?}", d.chars().skip(2000).collect::<String>());
    for n in [2100usize, 2200, 2300, 2400, 2500, 2533] {
        let t: String = d.chars().take(n).collect();
        eprintln!("N{n} {:?}", torrent_rss::parse_size(&t, true));
    }
}
