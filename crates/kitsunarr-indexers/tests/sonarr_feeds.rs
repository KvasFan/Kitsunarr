//! Sonarr's Newznab/Torznab feed and capability tests, replayed on its recorded responses
//! (`tests/fixtures/indexers/sonarr/`, from `src/NzbDrone.Core.Test/Files/Indexers/`).

use chrono::{TimeZone, Utc};
use kitsunarr_domain::DownloadProtocol;
use kitsunarr_indexers::feed::flags;
use kitsunarr_indexers::{Dialect, IndexerError, capabilities_or_default, parse_feed};
use serde_json::Value;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../tests/fixtures/indexers/sonarr/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

const URL: &str = "http://indexer.local/api?t=tvsearch&cat=5030,5040&apikey=xxx";

fn valid(dialect: Dialect, file: &str) -> Vec<kitsunarr_indexers::Release> {
    parse_feed(&fixture(file), dialect, URL)
        .unwrap()
        .into_iter()
        .filter(kitsunarr_indexers::feed::is_valid)
        .collect()
}

#[test]
fn newznab_nzb_su_feed() {
    let releases = valid(Dialect::Newznab, "newznab_nzb_su.xml");
    assert_eq!(releases.len(), 100);
    let r = &releases[0];
    assert_eq!(r.title, "White.Collar.S03E05.720p.HDTV.X264-DIMENSION");
    assert_eq!(r.protocol, DownloadProtocol::Usenet);
    assert_eq!(
        r.download_url.as_deref(),
        Some("http://nzb.su/getnzb/24967ef4c2e26296c65d3bbfa97aa8fe.nzb&i=37292&r=xxx")
    );
    assert_eq!(
        r.info_url.as_deref(),
        Some("http://nzb.su/details/24967ef4c2e26296c65d3bbfa97aa8fe")
    );
    assert_eq!(
        r.comment_url.as_deref(),
        Some("http://nzb.su/details/24967ef4c2e26296c65d3bbfa97aa8fe#comments")
    );
    assert_eq!(
        r.publish_date,
        Utc.with_ymd_and_hms(2012, 2, 27, 16, 9, 39).unwrap()
    );
    assert_eq!(r.size, 1183105773);
}

#[test]
fn newznab_reads_only_the_nzb_items_of_animetosho() {
    let releases = valid(Dialect::Newznab, "torznab_animetosho.xml");
    assert_eq!(releases.len(), 1);
    let r = &releases[0];
    assert_eq!(r.title, "[HorribleSubs] Frame Arms Girl - 07 [720p].mkv");
    assert_eq!(r.protocol, DownloadProtocol::Usenet);
    assert_eq!(
        r.download_url.as_deref(),
        Some("http://storage.localhost/nzb/123452.nzb")
    );
    assert_eq!(
        r.info_url.as_deref(),
        Some("https://localhost/view/horriblesubs-frame-arms-girl-07-720p-mkv.123452")
    );
    assert_eq!(
        r.comment_url.as_deref(),
        Some("https://localhost/view/horriblesubs-frame-arms-girl-07-720p-mkv.123452")
    );
    assert_eq!(
        r.publish_date,
        Utc.with_ymd_and_hms(2017, 5, 15, 19, 15, 56).unwrap()
    );
    assert_eq!(r.size, 473987489);
    assert_eq!((r.tvdb_id, r.tv_rage_id), (0, 0));
}

#[test]
fn newznab_languages() {
    let releases = valid(Dialect::Newznab, "newznab_language.xml");
    assert_eq!(releases.len(), 100);
    let names = |i: usize| {
        let mut n: Vec<&str> = releases[i].languages.iter().map(|l| l.name).collect();
        n.sort_unstable();
        n
    };
    assert_eq!(names(0), ["English", "Japanese"]);
    assert_eq!(names(1), ["English", "Spanish"]);
    assert_eq!(names(2), ["French"]);
}

#[test]
fn newznab_indexer_flags() {
    let doc: Value = serde_json::from_str(&fixture("cases/NewznabFixture.json")).unwrap();
    let cases = doc["methods"]["should_parse_indexer_flags"]["cases"]
        .as_array()
        .unwrap();
    assert_eq!(cases.len(), 11);
    let releases = parse_feed(&fixture("newznab_indexerflags.xml"), Dialect::Newznab, URL).unwrap();
    for case in cases {
        let args = case["args"].as_array().unwrap();
        let guid = args[0].as_str().unwrap();
        let release = releases
            .iter()
            .find(|r| r.guid == guid)
            .unwrap_or_else(|| panic!("no release {guid}"));
        let mut expected = 0;
        for flag in &args[1..] {
            expected |= match flag["enum"].as_str().unwrap() {
                "IndexerFlags.Scene" => flags::SCENE,
                "IndexerFlags.Nuked" => flags::NUKED,
                "IndexerFlags.Subtitles" => flags::SUBTITLES,
                other => panic!("unexpected flag {other}"),
            };
        }
        // Upstream asserts each expected flag is present (`HaveFlag`).
        assert_eq!(release.indexer_flags & expected, expected, "{guid}");
    }
}

#[test]
fn torznab_hdaccess_feed() {
    let releases = valid(Dialect::Torznab, "torznab_hdaccess_net.xml");
    assert_eq!(releases.len(), 5);
    let r = &releases[0];
    assert_eq!(
        r.title,
        "Better Call Saul S01E05 Alpine Shepherd 1080p NF WEBRip DD5.1 x264"
    );
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some("https://hdaccess.net/download.php?torrent=11515&passkey=123456")
    );
    assert_eq!(
        r.info_url.as_deref(),
        Some("https://hdaccess.net/details.php?id=11515&hit=1")
    );
    assert_eq!(
        r.comment_url.as_deref(),
        Some("https://hdaccess.net/details.php?id=11515&hit=1#comments")
    );
    assert_eq!(
        r.publish_date,
        Utc.with_ymd_and_hms(2015, 3, 14, 21, 10, 42).unwrap()
    );
    assert_eq!(r.size, 2538463390);
    assert_eq!((r.tvdb_id, r.tv_rage_id), (273181, 37780));
    assert_eq!(
        r.info_hash.as_deref(),
        Some("63e07ff523710ca268567dad344ce1e0e6b7e8a3")
    );
    assert_eq!((r.seeders, r.peers), (Some(7), Some(7)));
}

#[test]
fn torznab_tpb_feed_with_magnets() {
    let releases = valid(Dialect::Torznab, "torznab_tpb.xml");
    assert_eq!(releases.len(), 5);
    let r = &releases[0];
    assert_eq!(r.title, "Series Title S05E02 HDTV x264-Xclusive [eztv]");
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    let magnet = "magnet:?xt=urn:btih:9fb267cff5ae5603f07a347676ec3bf3e35f75e1&dn=Game+of+Thrones+S05E02+HDTV+x264-Xclusive+%5Beztv%5D&tr=udp:%2F%2Fopen.demonii.com:1337&tr=udp:%2F%2Ftracker.coppersurfer.tk:6969&tr=udp:%2F%2Ftracker.leechers-paradise.org:6969&tr=udp:%2F%2Fexodus.desync.com:6969";
    assert_eq!(r.magnet_url.as_deref(), Some(magnet));
    assert_eq!(r.download_url.as_deref(), Some(magnet));
    assert_eq!(
        r.info_url.as_deref(),
        Some(
            "https://thepiratebay.se/torrent/11811366/Series_Title_S05E02_HDTV_x264-Xclusive_%5Beztv%5D"
        )
    );
    assert_eq!(r.comment_url, r.info_url);
    // "Sat, 11 Apr 2015 21:34:00 -0600" in UTC.
    assert_eq!(
        r.publish_date,
        Utc.with_ymd_and_hms(2015, 4, 12, 3, 34, 0).unwrap()
    );
    assert_eq!(r.size, 388895872);
    assert_eq!(
        r.info_hash.as_deref(),
        Some("9fb267cff5ae5603f07a347676ec3bf3e35f75e1")
    );
    assert_eq!((r.seeders, r.peers), (Some(34128), Some(36724)));
}

#[test]
fn torznab_reads_only_the_torrent_items_of_animetosho() {
    let releases = valid(Dialect::Torznab, "torznab_animetosho.xml");
    assert_eq!(releases.len(), 2);
    let r = &releases[0];
    assert_eq!(
        r.title,
        "[finFAGs]_Frame_Arms_Girl_07_(1280x720_TV_AAC)_[1262B6F7].mkv"
    );
    assert_eq!(
        r.download_url.as_deref(),
        Some("http://storage.localhost/torrents/123451.torrent")
    );
    assert_eq!(
        r.info_url.as_deref(),
        Some(
            "https://localhost/view/finfags-_frame_arms_girl_07_-1280x720_tv_aac-_-1262b6f7-mkv.123451"
        )
    );
    assert_eq!(
        r.publish_date,
        Utc.with_ymd_and_hms(2017, 5, 17, 20, 36, 6).unwrap()
    );
    assert_eq!(r.size, 316477946);
    assert_eq!(
        r.info_hash.as_deref(),
        Some("2d69a861bef5a9f2cdf791b7328e37b7953205e1")
    );
    assert_eq!((r.seeders, r.peers), (None, None));
}

#[test]
fn wrong_dialect_feeds_are_discarded() {
    // A Torznab-only feed read as Newznab (and vice versa) yields nothing.
    assert!(
        parse_feed(&fixture("torznab_hdaccess_net.xml"), Dialect::Newznab, URL)
            .unwrap()
            .is_empty()
    );
    assert!(
        parse_feed(&fixture("newznab_nzb_su.xml"), Dialect::Torznab, URL)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn error_documents() {
    let err = parse_feed(&fixture("unauthorized.xml"), Dialect::Newznab, URL).unwrap_err();
    assert!(matches!(err, IndexerError::ApiKey(_)), "{err:?}");
    let limit = r#"<?xml version="1.0"?><error code="500" description="Request limit reached"/>"#;
    assert!(matches!(
        parse_feed(limit, Dialect::Newznab, URL),
        Err(IndexerError::RequestLimit { .. })
    ));
    let missing = r#"<error code="200" description="Missing parameter"/>"#;
    assert!(matches!(
        parse_feed(missing, Dialect::Torznab, "http://x/api?t=search"),
        Err(IndexerError::ApiKey(m)) if m == "Indexer requires an API key"
    ));
    let other = r#"<error code="201" description="Incorrect parameter"/>"#;
    assert!(
        matches!(parse_feed(other, Dialect::Torznab, URL), Err(IndexerError::Indexer(m)) if m == "Torznab error detected: Incorrect parameter")
    );
    let no_date = r#"<rss><channel><item><title>x</title></item></channel></rss>"#;
    assert!(
        parse_feed(no_date, Dialect::Newznab, URL).is_err(),
        "a missing pubDate fails the feed"
    );
}

#[test]
fn capabilities_provider_fixture() {
    let caps_xml = fixture("newznab_caps.xml");
    let caps = capabilities_or_default(&caps_xml).unwrap();
    assert_eq!((caps.default_page_size, caps.max_page_size), (25, 60));
    assert_eq!(
        (
            caps.text_search_engine.as_str(),
            caps.tv_text_search_engine.as_str()
        ),
        ("sphinx", "sphinx")
    );

    let missing_limits =
        capabilities_or_default(&caps_xml.replace("<limits", "<abclimits")).unwrap();
    assert_eq!(
        (
            missing_limits.default_page_size,
            missing_limits.max_page_size
        ),
        (100, 100)
    );

    assert!(matches!(
        capabilities_or_default(&caps_xml.replace("<limits", "<>")),
        Err(IndexerError::InvalidXml(_))
    ));
    // Unexpected data falls back to the defaults instead of failing.
    assert!(capabilities_or_default(&caps_xml.replace("5030", "asdf")).is_ok());

    let engines = capabilities_or_default(
        &caps_xml
            .replace("<search ", "<search searchEngine=\"raw\" ")
            .replace("<tv-search ", "<tv-search searchEngine=\"raw2\" "),
    )
    .unwrap();
    assert_eq!(
        (
            engines.text_search_engine.as_str(),
            engines.tv_text_search_engine.as_str()
        ),
        ("raw", "raw2")
    );
    assert!(caps.usable_for_tv());
}

/// `NyaaFixture.should_parse_2021_recent_feed_from_Nyaa`.
#[test]
fn nyaa_2021_feed() {
    let releases = valid(Dialect::Nyaa, "nyaa_2021.xml");
    assert_eq!(releases.len(), 3);
    let r = &releases[0];
    assert_eq!(
        r.title,
        "[Foxy-Subs] Mahouka Koukou no Yuutousei - 08 [720p] [3194D881].mkv"
    );
    assert_eq!(r.protocol, DownloadProtocol::Torrent);
    assert_eq!(
        r.download_url.as_deref(),
        Some("https://nyaa.si/download/1424896.torrent")
    );
    assert_eq!(r.info_url.as_deref(), Some("https://nyaa.si/view/1424896"));
    assert_eq!(r.comment_url, None);
    assert_eq!(
        r.publish_date,
        Utc.with_ymd_and_hms(2021, 8, 24, 22, 18, 46).unwrap()
    );
    assert_eq!(r.size, 639_211_930);
    assert_eq!(r.magnet_url, None);
    assert_eq!(
        r.info_hash.as_deref(),
        Some("e8ca5e20eca876339f41c3d9e95ea66c1d7caaee")
    );
    assert_eq!(r.seeders, Some(4));
    assert_eq!(r.peers, Some(3 + 4));
}
