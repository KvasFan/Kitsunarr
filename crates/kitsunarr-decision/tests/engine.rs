//! End-to-end decisions on real release names: parsing, mapping to the series and episodes,
//! custom formats, specifications and ranking together.

mod common;

use chrono::{TimeZone, Utc};
use common::*;
use kitsunarr_decision::*;
use kitsunarr_domain::{DownloadProtocol, SeriesType};
use serde_json::json;

fn episode(id: i64, season: i32, number: i32) -> Episode {
    Episode {
        id,
        season_number: season,
        episode_number: number,
        absolute_episode_number: None,
        scene_season_number: None,
        scene_episode_number: None,
        title: None,
        air_date: Some(format!("2008-01-{:02}", 19 + number)),
        air_date_utc: Some(Utc.with_ymd_and_hms(2008, 1, 20, 2, 0, 0).unwrap()),
        runtime: 47,
        monitored: true,
        file: None,
    }
}

fn breaking_bad() -> Series {
    let mut episodes: Vec<Episode> = (1..=7).map(|n| episode(100 + n as i64, 1, n)).collect();
    let mut minisode = episode(200, 0, 3);
    minisode.title = Some("Good Cop Bad Cop".into());
    episodes.push(minisode);
    let mut p = profile(default_items(&[]));
    p.upgrade_allowed = true;
    p.cutoff = BLURAY1080P;
    p.format_items = scores(&[(1, 50)]);
    Series {
        id: 1,
        title: "Breaking Bad".into(),
        year: 2008,
        series_type: SeriesType::Standard,
        tvdb_id: 81189,
        tv_rage_id: 18164,
        imdb_id: Some("tt0903747".into()),
        alternate_titles: vec!["BrBa".into()],
        runtime: 47,
        original_language: kitsunarr_domain::Language::by_name("English"),
        use_scene_numbering: false,
        tags: Vec::new(),
        profile: p,
        episodes,
    }
}

fn library() -> Vec<LibrarySeries> {
    vec![LibrarySeries {
        id: 2,
        title: "White Collar".into(),
        year: 2009,
        alternate_titles: Vec::new(),
        tvdb_id: 108611,
        tv_rage_id: 0,
        imdb_id: None,
    }]
}

fn settings() -> Settings {
    Settings {
        quality_definitions: Vec::new(),
        propers: ProperDownloadType::PreferAndUpgrade,
        release_profiles: Vec::new(),
        delay_profiles: Vec::new(),
        minimum_age_minutes: 0,
        retention_days: 0,
        maximum_size_mb: 0,
        now: Utc::now(),
    }
}

fn release(title: &str) -> Release {
    Release {
        guid: title.into(),
        title: title.into(),
        size: 1500 * 1024 * 1024,
        publish_date: Utc::now(),
        protocol: DownloadProtocol::Usenet,
        indexer_id: 1,
        indexer_priority: 25,
        indexer_tags: Vec::new(),
        indexer_flags: 0,
        languages: Vec::new(),
        seeders: None,
        peers: None,
        tvdb_id: 0,
        tv_rage_id: 0,
        imdb_id: None,
        minimum_seeders: None,
        multi_languages: Vec::new(),
    }
}

/// Title, approved, rejection messages, episode ids, custom format score.
type Outcome = (String, bool, Vec<String>, Vec<i64>, i32);

/// Decides and returns the outcomes best first.
fn decide(series: &Series, criteria: SearchCriteria, releases: &[Release]) -> Vec<Outcome> {
    let (formats, problems) = compile_formats(&[format(
        1,
        "x265",
        vec![spec(
            "ReleaseTitleSpecification",
            json!({"value": r"[xh][ ._-]?265|\bHEVC\b"}),
        )],
    )]);
    assert!(problems.is_empty());
    let library = library();
    let settings = settings();
    let engine = Engine {
        series,
        library: &library,
        formats: &formats,
        settings: &settings,
    };
    engine
        .decide(releases, &criteria)
        .into_iter()
        .map(|d| {
            (
                releases[d.release].title.clone(),
                d.approved(),
                d.rejections.iter().map(|r| r.message.clone()).collect(),
                d.episodes.iter().map(|e| e.id).collect(),
                d.custom_format_score,
            )
        })
        .collect()
}

fn reasons<'a>(decided: &'a [Outcome], title: &str) -> &'a [String] {
    &decided.iter().find(|d| d.0 == title).unwrap().2
}

#[test]
fn single_episode_search() {
    let series = breaking_bad();
    let titles = [
        "Breaking.Bad.S01E02.720p.HDTV.x264-CTU",
        "Breaking.Bad.S01E01.720p.HDTV.x264-CTU",
        "White.Collar.S03E05.720p.HDTV.X264-DIMENSION",
        "Some.Unknown.Show.S01E01.720p.HDTV.x264-GRP",
        "Breaking.Bad.S01.720p.BluRay.x264-GRP",
        "complete gibberish",
        "{complete gibberish}",
        "BrBa.S01E01.1080p.WEB-DL.DD5.1.H.265-GRP",
        "Breaking.Bad.S01E01.Pilot.1080p.BluRay.x264-GRP",
    ];
    let releases: Vec<Release> = titles.iter().map(|t| release(t)).collect();
    let decided = decide(
        &series,
        SearchCriteria {
            kind: SearchKind::SingleEpisode {
                season: 1,
                episode: 1,
            },
            episode_ids: vec![101],
        },
        &releases,
    );
    // Ranking ignores rejections (as upstream): best quality first, by this profile's order
    // (`GetDefaultQualities` puts Bluray-720p above WEBDL-1080p).
    let order: Vec<&str> = decided.iter().map(|d| d.0.as_str()).collect();
    assert_eq!(
        &order[..3],
        [
            "Breaking.Bad.S01E01.Pilot.1080p.BluRay.x264-GRP",
            "Breaking.Bad.S01.720p.BluRay.x264-GRP",
            "BrBa.S01E01.1080p.WEB-DL.DD5.1.H.265-GRP",
        ]
    );
    let approved: Vec<&str> = decided
        .iter()
        .filter(|d| d.1)
        .map(|d| d.0.as_str())
        .collect();
    assert_eq!(
        approved,
        [
            "Breaking.Bad.S01E01.Pilot.1080p.BluRay.x264-GRP",
            "BrBa.S01E01.1080p.WEB-DL.DD5.1.H.265-GRP",
            "Breaking.Bad.S01E01.720p.HDTV.x264-CTU",
        ]
    );
    // The alias maps to the series; the x265 custom format scores 50.
    let alias = decided.iter().find(|d| d.0.starts_with("BrBa")).unwrap();
    assert_eq!((alias.3.clone(), alias.4), (vec![101], 50));
    assert_eq!(
        reasons(&decided, "Breaking.Bad.S01E02.720p.HDTV.x264-CTU"),
        ["Episode wasn't requested: 1x2", "Wrong episode"]
    );
    assert_eq!(
        reasons(&decided, "White.Collar.S03E05.720p.HDTV.X264-DIMENSION"),
        ["Wrong series"]
    );
    assert_eq!(
        reasons(&decided, "Some.Unknown.Show.S01E01.720p.HDTV.x264-GRP"),
        ["Unknown Series"]
    );
    // Text without brackets parses as a season title (`ParseSeasonTitle`), so it is an unknown
    // series; with brackets it cannot be parsed at all.
    assert_eq!(reasons(&decided, "complete gibberish"), ["Unknown Series"]);
    assert_eq!(
        reasons(&decided, "{complete gibberish}"),
        ["Unable to parse release"]
    );
    assert_eq!(
        reasons(&decided, "Breaking.Bad.S01.720p.BluRay.x264-GRP"),
        ["Full season pack"]
    );
    // Other series and unparsed releases come last.
    assert_eq!(order[order.len() - 1], "{complete gibberish}");
}

#[test]
fn season_search_and_specials() {
    let series = breaking_bad();
    let releases: Vec<Release> = [
        "Breaking.Bad.S01.720p.BluRay.x264-GRP",
        "Breaking.Bad.S02.720p.BluRay.x264-GRP",
        "Breaking.Bad.Minisode.Good.Cop.Bad.Cop.720p.WEB-DL.AAC2.0.H.264-GRP",
    ]
    .iter()
    .map(|t| release(t))
    .collect();
    let decided = decide(
        &series,
        SearchCriteria {
            kind: SearchKind::Season { season: 1 },
            episode_ids: (101..=107).collect(),
        },
        &releases,
    );
    let pack = decided.iter().find(|d| d.0.contains("S01.")).unwrap();
    assert!(pack.1, "{:?}", pack.2);
    assert_eq!(pack.3, (101..=107).collect::<Vec<i64>>());
    // Season 2 has no episodes in the library.
    assert_eq!(
        reasons(&decided, "Breaking.Bad.S02.720p.BluRay.x264-GRP"),
        ["Unable to identify correct episode(s) using release name and scene mappings"]
    );
    // The special is recognised by its title (`ParseSpecialEpisodeTitle`), then not requested.
    let special = decided.iter().find(|d| d.0.contains("Minisode")).unwrap();
    assert_eq!(special.3, vec![200]);
    assert!(
        special
            .2
            .iter()
            .any(|r| r == "Episode wasn't requested: 0x3"),
        "{:?}",
        special.2
    );
}

#[test]
fn anime_absolute_and_daily_numbering() {
    let mut anime = breaking_bad();
    anime.title = "Frieren".into();
    anime.alternate_titles = vec!["Sousou no Frieren".into()];
    anime.series_type = SeriesType::Anime;
    for (i, e) in anime.episodes.iter_mut().enumerate() {
        e.absolute_episode_number = (e.season_number > 0).then_some(i as i32 + 1);
    }
    let releases = [release(
        "[SubsPlease] Sousou no Frieren - 03 (1080p) [ABCD1234].mkv",
    )];
    let decided = decide(
        &anime,
        SearchCriteria {
            kind: SearchKind::AnimeEpisode {
                season_search: false,
            },
            episode_ids: vec![103],
        },
        &releases,
    );
    assert_eq!(decided[0].3, vec![103], "{:?}", decided[0].2);

    let mut daily = breaking_bad();
    daily.title = "The Daily Show".into();
    daily.series_type = SeriesType::Daily;
    let releases = [
        release("The.Daily.Show.2008.01.22.720p.HDTV.x264-GRP"),
        release("The.Daily.Show.2008.02.22.720p.HDTV.x264-GRP"),
    ];
    let decided = decide(
        &daily,
        SearchCriteria {
            kind: SearchKind::DailyEpisode,
            episode_ids: vec![103],
        },
        &releases,
    );
    let mapped = decided.iter().find(|d| d.0.contains("2008.01.22")).unwrap();
    assert!(mapped.1, "{:?}", mapped.2);
    assert_eq!(mapped.3, vec![103]);
    assert_eq!(
        reasons(&decided, "The.Daily.Show.2008.02.22.720p.HDTV.x264-GRP"),
        ["Unable to identify correct episode(s) using release name and scene mappings"]
    );
}

#[test]
fn release_ids_map_titles_the_library_does_not_know() {
    let series = breaking_bad();
    let mut r = release("Breaking.Bad.Remastered.S01E01.720p.HDTV.x264-GRP");
    let criteria = || SearchCriteria {
        kind: SearchKind::SingleEpisode {
            season: 1,
            episode: 1,
        },
        episode_ids: vec![101],
    };
    assert_eq!(
        decide(&series, criteria(), &[r.clone()])[0].2,
        ["Unknown Series"]
    );
    r.tvdb_id = 81189;
    let decided = decide(&series, criteria(), &[r]);
    assert!(decided[0].1, "{:?}", decided[0].2);
}

#[test]
fn languages_fall_back_to_the_original_language() {
    let series = breaking_bad();
    let (formats, _) = compile_formats(&[format(
        7,
        "English",
        vec![spec("LanguageSpecification", json!({"value": 1}))],
    )]);
    let library = library();
    let settings = settings();
    let criteria = SearchCriteria {
        kind: SearchKind::SingleEpisode {
            season: 1,
            episode: 1,
        },
        episode_ids: vec![101],
    };
    let engine = Engine {
        series: &series,
        library: &library,
        formats: &formats,
        settings: &settings,
    };
    let releases = [
        release("Breaking.Bad.S01E01.720p.HDTV.x264-CTU"),
        release("Breaking.Bad.S01E01.FRENCH.720p.HDTV.x264-GRP"),
    ];
    let decided = engine.decide(&releases, &criteria);
    let names = |i: usize| -> Vec<&str> {
        let d = decided.iter().find(|d| d.release == i).unwrap();
        d.languages.iter().map(|l| l.name).collect()
    };
    assert_eq!(names(0), ["English"]);
    assert_eq!(names(1), ["French"]);
    let formats_of = |i: usize| {
        decided
            .iter()
            .find(|d| d.release == i)
            .unwrap()
            .custom_formats
            .clone()
    };
    assert_eq!(formats_of(0), ["English"]);
    assert!(formats_of(1).is_empty());
}
