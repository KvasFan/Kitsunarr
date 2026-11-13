//! The rest of Sonarr/Tanukarr's `ParsingServiceTests`: `MapFixture`, `GetSeriesFixture`,
//! `MultiSeasonSceneMappingFixture`, `NativeProviderSceneIsolationFixture` and
//! `MovieRssMappingFixture` (`GetEpisodesFixture` is in `get_episodes_tests`,
//! `TrailingSeasonSuffixFixture` in `mapping::tests`).
//!
//! Upstream verifies which `ISeriesService`, `ISceneMappingService` and `IEpisodeService` calls
//! run against mocks. Kitsunarr's lookups are pure functions over the library, so each test
//! builds a library in which the alternative lookups would find different series (or episodes)
//! and checks which one comes back.
//!
//! - `Map(parsed, tvdbId, tvRageId, imdbId)` (no search criteria) is
//!   `Lookup::library(…).find_series_unsearched`; with search criteria it is
//!   `Lookup::new(series, …).find_series`. The release's ids are the `Release`'s.
//! - `Map(parsed, series)` is `mapping::map_remote` (episodes, mapped season and numbering
//!   rejection); `GetEpisodes(parsed, series, true)` is `map_episodes`.
//! - `GetSeries(title)` is `Lookup::get_series`.
//! - Scene mappings: Kitsunarr has no `SceneMappingService`. A series' alternate titles stand
//!   for scene mappings without seasons (`FindSceneMapping`/`FindTvdbId` resolving to that
//!   series), and TheXEM's names of one scene season (`Series.scene_season_titles`) for
//!   `XemService` mappings with a `SceneSeasonNumber` (`GetSceneSeasonNumber`). Mappings with a
//!   TheTVDB season offset (`SeasonNumber`) or an origin (`SceneOrigin`) do not exist here.

use kitsunarr_domain::{Language, SeriesType};
use kitsunarr_parser::{ParsedEpisodeInfo, SeriesTitleInfo, parse_title};

use crate::mapping::{self, Lookup, Mapped};
use crate::model::{Episode, LibrarySeries, Release, SearchCriteria, SearchKind, Series};

fn episode(id: i64, season: i32, number: i32) -> Episode {
    Episode {
        id,
        season_number: season,
        episode_number: number,
        absolute_episode_number: None,
        scene_season_number: None,
        scene_episode_number: None,
        scene_absolute_episode_number: None,
        title: None,
        air_date: None,
        air_date_utc: None,
        runtime: 30,
        monitored: true,
        file: None,
    }
}

fn ids(episodes: &[&Episode]) -> Vec<i64> {
    episodes.iter().map(|e| e.id).collect()
}

fn sorted(mut ids: Vec<i64>) -> Vec<i64> {
    ids.sort_unstable();
    ids
}

/// `Map(parsed, series)` with no search criteria.
fn no_criteria() -> SearchCriteria {
    SearchCriteria {
        kind: SearchKind::Special,
        episode_ids: Vec::new(),
        monitored_episodes_only: false,
        user_invoked: true,
    }
}

fn map_remote<'s>(parsed: &ParsedEpisodeInfo, series: &'s Series) -> mapping::Remote<'s> {
    mapping::map_remote(parsed, series, &no_criteria())
}

/// A library title.
fn library(id: i64, title: &str) -> LibrarySeries {
    LibrarySeries {
        id,
        title: title.into(),
        year: 0,
        alternate_titles: Vec::new(),
        tvdb_id: 0,
        tv_rage_id: 0,
        imdb_id: None,
    }
}

/// A release with the indexer's ids (`Map(…, tvdbId, tvRageId, imdbId, …)`).
fn release(tvdb_id: i64, tv_rage_id: i64, imdb_id: Option<&str>) -> Release {
    Release {
        tvdb_id,
        tv_rage_id,
        imdb_id: imdb_id.map(Into::into),
        ..crate::tests::release("30.Stone.S01E01.720p.HDTV-GRP")
    }
}

// ---------------------------------------------------------------------------------------------
// MapFixture
// ---------------------------------------------------------------------------------------------

/// `_series`: NBuilder numbers its ids 1.
const SERIES_IMDB: &str = "tt0000001";

/// `_series` as the searched series.
fn map_series() -> Series {
    let mut s = crate::tests::series();
    s.title = "30 Stone".into();
    s.year = 2017;
    s.tvdb_id = 1;
    s.tv_rage_id = 1;
    s.imdb_id = Some(SERIES_IMDB.into());
    s.episodes = vec![episode(1, 1, 1)];
    s
}

/// `_series` as a library title.
fn map_library_series() -> LibrarySeries {
    LibrarySeries {
        year: 2017,
        tvdb_id: 1,
        tv_rage_id: 1,
        imdb_id: Some(SERIES_IMDB.into()),
        ..library(1, "30 Stone")
    }
}

/// `_parsedEpisodeInfo`.
fn parsed_info() -> ParsedEpisodeInfo {
    ParsedEpisodeInfo {
        series_title: "30 Stone".into(),
        series_title_info: SeriesTitleInfo::default(),
        season_numbers: vec![1],
        episode_numbers: vec![1],
        languages: Language::by_name("English").into_iter().collect(),
        ..Default::default()
    }
}

/// `GivenParseResultSeriesDoesntMatchSearchCriteria`.
fn another_name() -> ParsedEpisodeInfo {
    ParsedEpisodeInfo {
        series_title: "Another Name".into(),
        ..parsed_info()
    }
}

/// `Map(parsed, tvdbId, tvRageId, imdbId)`.
fn map_unsearched(
    parsed: &ParsedEpisodeInfo,
    titles: &[LibrarySeries],
    release: &Release,
) -> Option<Mapped> {
    Lookup::library(titles).find_series_unsearched(parsed, release)
}

/// `Map(parsed, tvdbId, tvRageId, imdbId, _singleEpisodeSearchCriteria)`.
fn map_searched(
    parsed: &ParsedEpisodeInfo,
    series: &Series,
    titles: &[LibrarySeries],
    release: &Release,
) -> Option<Mapped> {
    Lookup::new(series, titles).find_series(series, parsed, release)
}

#[test]
fn should_lookup_series_by_name() {
    // The ids name another title: the parsed title wins.
    let titles = [
        map_library_series(),
        LibrarySeries {
            tvdb_id: 2,
            tv_rage_id: 2,
            imdb_id: Some("tt0000002".into()),
            ..library(2, "Other Title")
        },
    ];
    let r = release(2, 2, Some("tt0000002"));
    assert_eq!(
        map_unsearched(&parsed_info(), &titles, &r),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_use_tvdbid_when_series_title_lookup_fails() {
    let titles = [LibrarySeries {
        tvdb_id: 1,
        ..library(1, "Library Title")
    }];
    let r = release(1, 1, Some(SERIES_IMDB));
    assert_eq!(
        map_unsearched(&parsed_info(), &titles, &r),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_use_tvrageid_when_series_title_lookup_fails() {
    let titles = [LibrarySeries {
        tv_rage_id: 1,
        ..library(1, "Library Title")
    }];
    assert_eq!(
        map_unsearched(&parsed_info(), &titles, &release(0, 1, None)),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_not_use_tvrageid_when_scene_naming_exception_exists() {
    // The parsed title is a scene name (alternate title) of series 10; the TVRage id names
    // series 2 and is never used. Upstream's mapping names a TheTVDB id missing from the
    // library, so its result is no series; an alternate title always belongs to a library
    // title here, so that series is the result.
    let titles = [
        LibrarySeries {
            alternate_titles: vec!["30 Stone".into()],
            tvdb_id: 10,
            ..library(10, "Scene Owner")
        },
        LibrarySeries {
            tv_rage_id: 1,
            ..library(2, "Library Title")
        },
    ];
    assert_eq!(
        map_unsearched(&parsed_info(), &titles, &release(0, 1, None)),
        Some(Mapped::Other(10))
    );
}

#[test]
fn should_use_search_criteria_series_title() {
    // Another library title has the same name: the searched series is used without a library
    // title lookup.
    let series = map_series();
    let titles = [library(2, "30 Stone")];
    let r = release(1, 1, Some(SERIES_IMDB));
    assert_eq!(
        map_searched(&parsed_info(), &series, &titles, &r),
        Some(Mapped::Searched)
    );
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_FindByTitle_when_search_criteria_matching_fails() {
    let series = map_series();
    let titles = [library(2, "Another Name")];
    assert_eq!(
        map_searched(&another_name(), &series, &titles, &release(10, 10, None)),
        Some(Mapped::Other(2))
    );
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_FindByTitle_using_year_when_FindByTitle_matching_fails() {
    let series = map_series();
    let parsed = ParsedEpisodeInfo {
        series_title_info: SeriesTitleInfo {
            title: "Series Title 2017".into(),
            title_without_year: "Series Title".into(),
            year: 2017,
            all_titles: None,
        },
        ..another_name()
    };
    let titles = [
        LibrarySeries {
            year: 2016,
            ..library(2, "Series Title")
        },
        LibrarySeries {
            year: 2017,
            ..library(3, "Series Title")
        },
    ];
    assert_eq!(
        map_searched(&parsed, &series, &titles, &release(10, 10, None)),
        Some(Mapped::Other(3))
    );
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_FindByTvdbId_when_search_criteria_and_FindByTitle_matching_fails() {
    let series = map_series();
    let titles = [LibrarySeries {
        tvdb_id: 10,
        ..library(2, "Library Title")
    }];
    assert_eq!(
        map_searched(&another_name(), &series, &titles, &release(10, 10, None)),
        Some(Mapped::Other(2))
    );
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_FindByTvRageId_when_search_criteria_and_FindByTitle_matching_fails() {
    let series = map_series();
    let titles = [LibrarySeries {
        tv_rage_id: 10,
        ..library(2, "Library Title")
    }];
    assert_eq!(
        map_searched(&another_name(), &series, &titles, &release(0, 10, None)),
        Some(Mapped::Other(2))
    );
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_not_FindByTvRageId_when_search_criteria_and_FindByTitle_matching_fails_and_tvdb_id_is_specified()
 {
    let series = map_series();
    let titles = [LibrarySeries {
        tv_rage_id: 10,
        ..library(2, "Library Title")
    }];
    assert_eq!(
        map_searched(&another_name(), &series, &titles, &release(10, 10, None)),
        None
    );
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_FindByImdbId_when_search_criteria_and_FindByTitle_matching_fails() {
    let series = map_series();
    let titles = [LibrarySeries {
        imdb_id: Some("tt12345".into()),
        ..library(2, "Library Title")
    }];
    assert_eq!(
        map_searched(
            &another_name(),
            &series,
            &titles,
            &release(0, 0, Some("tt12345"))
        ),
        Some(Mapped::Other(2))
    );
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_not_FindByImdbId_when_search_criteria_and_FindByTitle_matching_fails_and_tvdb_id_is_specified()
 {
    let series = map_series();
    let titles = [LibrarySeries {
        imdb_id: Some("tt12345".into()),
        ..library(2, "Library Title")
    }];
    assert_eq!(
        map_searched(
            &another_name(),
            &series,
            &titles,
            &release(10, 10, Some("tt12345"))
        ),
        None
    );
}

#[test]
fn should_use_tvdbid_matching_when_alias_is_found() {
    // The parsed title is the searched series' scene name and another library title's name:
    // the scene mapping decides, no title lookup.
    let mut series = map_series();
    series.title = "Series Title".into();
    series.alternate_titles = vec!["30 Stone".into()];
    let titles = [library(2, "30 Stone")];
    let r = release(1, 1, Some(SERIES_IMDB));
    assert_eq!(
        map_searched(&parsed_info(), &series, &titles, &r),
        Some(Mapped::Searched)
    );
}

#[test]
fn should_use_tvrageid_match_from_search_criteria_when_title_match_fails() {
    // The searched series' ids match: no library title lookup, though one has the name.
    let series = map_series();
    let titles = [library(2, "Another Name")];
    let r = release(1, 1, Some(SERIES_IMDB));
    assert_eq!(
        map_searched(&another_name(), &series, &titles, &r),
        Some(Mapped::Searched)
    );
}

#[test]
fn should_use_scene_season_number_from_xem_mapping_if_alias_matches_a_specific_season_number() {
    let mut series = map_series();
    series.scene_season_titles = vec![("30 Stone".into(), 2)];
    let parsed = parsed_info();
    assert_eq!(map_remote(&parsed, &series).mapped_season, Some(2));
}

#[test]
fn should_not_use_scene_season_number_from_xem_mapping_if_alias_matches_a_specific_season_number_but_did_not_parse_season_1()
 {
    // TheXEM's name of scene season 3, and a release numbered from season 2 under that name:
    // the parsed season stands.
    let mut series = map_series();
    series.scene_season_titles = vec![("30 Stone".into(), 3)];
    let parsed = ParsedEpisodeInfo {
        season_numbers: vec![2],
        ..parsed_info()
    };
    assert_eq!(map_remote(&parsed, &series).mapped_season, Some(2));
}

#[test]
fn should_use_tvdbid_matching_when_alias_without_year_is_found() {
    // `GetSeriesAliasTitleAndYear`: the title without its year is a scene name of a series of
    // that year. Upstream also passes the series' ids, which would find it anyway; here the
    // release has none, so only the alias can.
    let titles = [LibrarySeries {
        alternate_titles: vec!["Series Alias".into()],
        ..map_library_series()
    }];
    let parsed = ParsedEpisodeInfo {
        series_title: "Series Alias 2017".into(),
        series_title_info: SeriesTitleInfo {
            title_without_year: "Series Alias".into(),
            year: 2017,
            ..Default::default()
        },
        ..parsed_info()
    };
    assert_eq!(
        map_unsearched(&parsed, &titles, &release(0, 0, None)),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_not_use_tvdbid_matching_when_alias_without_year_is_found_with_wrong_year() {
    let titles = [LibrarySeries {
        alternate_titles: vec!["Series Alias".into()],
        ..map_library_series()
    }];
    let parsed = ParsedEpisodeInfo {
        series_title: "Series Alias 2017".into(),
        series_title_info: SeriesTitleInfo {
            title_without_year: "Series Alias".into(),
            year: 2018,
            ..Default::default()
        },
        ..parsed_info()
    };
    assert_eq!(
        map_unsearched(&parsed, &titles, &release(0, 0, Some(""))),
        None
    );
}

/// `Series Title AKA Series Alias 2017`.
fn aka_parsed() -> ParsedEpisodeInfo {
    ParsedEpisodeInfo {
        series_title: "Series Title AKA Series Alias 2017".into(),
        series_title_info: SeriesTitleInfo {
            all_titles: Some(vec!["Series Title".into(), "Series Alias".into()]),
            year: 2017,
            ..Default::default()
        },
        ..parsed_info()
    }
}

#[test]
fn should_use_year_when_looking_up_by_all_titles_in_release_title() {
    let titles = [
        LibrarySeries {
            year: 2010,
            ..library(2, "Series Title")
        },
        LibrarySeries {
            year: 2017,
            ..library(1, "Series Title")
        },
    ];
    assert_eq!(
        map_unsearched(&aka_parsed(), &titles, &release(0, 0, Some(""))),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_use_title_with_year_when_looking_up_by_all_titles_in_release_title() {
    let titles = [
        LibrarySeries {
            year: 2010,
            ..library(2, "Series Title")
        },
        library(1, "Series Title 2017"),
    ];
    assert_eq!(
        map_unsearched(&aka_parsed(), &titles, &release(0, 0, Some(""))),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_not_have_a_mapped_season_number_when_the_scene_mapping_has_no_season() {
    // A season title without a season, named by a scene name without a season.
    let mut series = map_series();
    series.alternate_titles = vec!["30 Stone".into()];
    let parsed = ParsedEpisodeInfo {
        season_numbers: Vec::new(),
        episode_numbers: Vec::new(),
        is_season_title: true,
        ..parsed_info()
    };
    let titles = [LibrarySeries {
        alternate_titles: vec!["30 Stone".into()],
        ..map_library_series()
    }];
    let r = release(1, 1, Some(SERIES_IMDB));
    assert_eq!(map_unsearched(&parsed, &titles, &r), Some(Mapped::Other(1)));
    assert_eq!(map_remote(&parsed, &series).mapped_season, None);
}

// ---------------------------------------------------------------------------------------------
// GetSeriesFixture
// ---------------------------------------------------------------------------------------------

#[test]
fn should_use_passed_in_title_when_it_cannot_be_parsed() {
    assert!(parse_title("30 Stone").is_none());
    let titles = [library(1, "30 Stone")];
    assert_eq!(
        Lookup::library(&titles).get_series("30 Stone"),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_use_parsed_series_title() {
    let title = "30.Stone.S01E01.720p.hdtv";
    assert_eq!(parse_title(title).unwrap().series_title, "30 Stone");
    let titles = [library(1, "30 Stone")];
    assert_eq!(
        Lookup::library(&titles).get_series(title),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_fallback_to_title_without_year_and_year_when_title_lookup_fails() {
    let titles = [
        LibrarySeries {
            year: 2010,
            ..library(2, "Show")
        },
        LibrarySeries {
            year: 2004,
            ..library(1, "Show")
        },
    ];
    assert_eq!(
        Lookup::library(&titles).get_series("Show.2004.S01E01.720p.hdtv"),
        Some(Mapped::Other(1))
    );
}

#[test]
fn should_parse_concatenated_title() {
    // "Welcome" by title, "Mairimashita" by scene name: both the same series.
    let titles = [LibrarySeries {
        alternate_titles: vec!["Mairimashita".into()],
        tvdb_id: 100,
        ..library(100, "Welcome")
    }];
    let title = "Welcome (Mairimashita).S01E01.720p.WEB-DL-Viva";
    assert_eq!(
        Lookup::library(&titles).get_series(title),
        Some(Mapped::Other(100))
    );
    // `GetSeriesByAllTitles`: when the scene name is another series', the titles conflict.
    let conflicting = [
        library(100, "Welcome"),
        LibrarySeries {
            alternate_titles: vec!["Mairimashita".into()],
            tvdb_id: 200,
            ..library(200, "Another Series")
        },
    ];
    assert_eq!(Lookup::library(&conflicting).get_series(title), None);
}

// ---------------------------------------------------------------------------------------------
// MultiSeasonSceneMappingFixture
// ---------------------------------------------------------------------------------------------
//
// Upstream mocks `IEpisodeNumberingResolver` (it resolves nothing) except where a test sets it
// up, so `Map(_parsed, _series).Episodes` is `GetEpisodes`' (`map_episodes`).

/// `_series`: Food Wars!, anime, scene numbering (identical to TheTVDB's), seasons of 24, 13
/// and 24 episodes; episode ids are `season * 100 + number`.
fn food_wars() -> Series {
    let mut s = crate::tests::series();
    s.id = 55;
    s.title = "Food Wars!".into();
    s.series_type = SeriesType::Anime;
    s.use_scene_numbering = true;
    s.episodes = [24, 13, 24]
        .iter()
        .zip(1..)
        .flat_map(|(&count, season)| {
            (1..=count).map(move |n| {
                let mut e = episode(i64::from(season * 100 + n), season, n);
                e.scene_season_number = Some(season);
                e.scene_episode_number = Some(n);
                e
            })
        })
        .collect();
    s
}

fn food_wars_pack() -> ParsedEpisodeInfo {
    parse_title("Shokugeki no Soma S01-S03 + OVA (Food Wars) [1080p]").unwrap()
}

fn all_ids(series: &Series) -> Vec<i64> {
    series.episodes.iter().map(|e| e.id).collect()
}

#[test]
fn should_resolve_all_61_episodes_when_first_scene_season_matches() {
    let series = food_wars();
    let parsed = food_wars_pack();
    let mapped = sorted(ids(&crate::map_episodes(&parsed, &series)));
    assert_eq!(mapped.len(), 61);
    assert_eq!(mapped, all_ids(&series));
    assert_eq!(parsed.season_numbers, [1, 2, 3]);
}

#[test]
fn should_fallback_to_aired_numbering_for_each_missing_scene_season() {
    // Nothing in scene season 2: its episodes by aired season; seasons 1 and 3 by scene season.
    let mut series = food_wars();
    for e in series.episodes.iter_mut().filter(|e| e.season_number == 2) {
        e.scene_season_number = None;
        e.scene_episode_number = None;
    }
    let mapped = sorted(ids(&crate::map_episodes(&food_wars_pack(), &series)));
    assert_eq!(mapped, all_ids(&series));
}

// should_apply_each_seasons_alias_mapping: not ported. It needs scene mappings with a TheTVDB
// season offset (`SeasonNumber = 2` for `SceneSeasonNumber = 1`, 3 for 2); Kitsunarr's scene
// names (TheXEM's) carry no TheTVDB season and only ever move season 1.

#[test]
fn should_map_trailing_group_anime_episode_to_its_season_alias() {
    let mut series = food_wars();
    series.scene_season_titles = vec![("Shokugeki no Soma S2".into(), 2)];
    let parsed = parse_title("Shokugeki no Soma S2 - 10 [HorribleSubs][1080p].mkv").unwrap();
    assert_eq!(ids(&crate::map_episodes(&parsed, &series)), [210]);
}

#[test]
fn should_preserve_xem_season_one_alias_override() {
    // TheXEM's name "Series" stands for scene season 3: season 1 of the pack is scene season
    // 3, season 2 stays.
    let mut series = food_wars();
    series.scene_season_titles = vec![("Series".into(), 3)];
    let parsed = parse_title("Series S01-S02 1080p").unwrap();
    let expected: Vec<i64> = series
        .episodes
        .iter()
        .filter(|e| e.season_number >= 2)
        .map(|e| e.id)
        .collect();
    assert_eq!(
        sorted(ids(&crate::map_episodes(&parsed, &series))),
        expected
    );
}

// should_preserve_aired_library_scans: not ported. It calls `GetEpisodes(…, sceneSource:
// false)`; Kitsunarr's decision engine maps every release as a scene source and has no aired
// (library scan) variant of `GetEpisodes`.

// should_honor_tvdb_origin_for_an_individual_season_mapping: not ported. It needs a scene
// mapping with a `tvdb` origin; Kitsunarr's scene names (TheXEM's) have no origin.

#[test]
fn should_keep_numbering_ambiguity_rejection() {
    // Upstream mocks the resolver's rejection; here season 2's scene numbering differs from
    // TheTVDB's and the pack has no title or origin to tell them apart.
    let mut series = food_wars();
    for e in series.episodes.iter_mut().filter(|e| e.season_number == 2) {
        e.scene_episode_number = Some(match e.episode_number {
            1 => 2,
            2 => 1,
            n => n,
        });
    }
    let rejection = map_remote(&food_wars_pack(), &series).numbering_rejection;
    assert!(
        rejection
            .as_deref()
            .is_some_and(|r| r.starts_with("Ambiguous")),
        "{rejection:?}"
    );
}

#[test]
fn should_deduplicate_overlapping_scene_seasons() {
    // TheTVDB's 2x13 is the scene's 1x25: found by scene season 1, and again by aired season 2
    // (scene season 2 is otherwise empty).
    let mut series = food_wars();
    for e in series.episodes.iter_mut().filter(|e| e.season_number == 2) {
        (e.scene_season_number, e.scene_episode_number) = if e.episode_number == 13 {
            (Some(1), Some(25))
        } else {
            (None, None)
        };
    }
    let mapped = sorted(ids(&crate::map_episodes(&food_wars_pack(), &series)));
    assert_eq!(mapped.len(), 61);
    assert_eq!(mapped, all_ids(&series));
}

// ---------------------------------------------------------------------------------------------
// NativeProviderSceneIsolationFixture
// ---------------------------------------------------------------------------------------------
//
// Upstream's scene mapping service answers every title with another provider's mapping
// (TheTVDB 7999, season 2 as scene season 1, scene season 2 for any name), which a series from
// a native provider (AniList, MyAnimeList, TMDb; a placeholder TheTVDB id, no scene numbering)
// must ignore. Kitsunarr's scene data belong to each series and only TheTVDB series get
// TheXEM's names (`kitsunarr-library`'s `xem`), so such a series carries none and no other
// provider's mapping can reach it. The metadata source is not part of the decision model; the
// tests check the native episode is mapped for each way of mapping.

/// `_series` and `_episode` (4201, 1x01, absolute 1).
fn native_series() -> Series {
    let mut s = crate::tests::series();
    s.id = 42;
    s.title = "Native Anime Run".into();
    s.tvdb_id = 0;
    s.alternate_titles = vec!["Shokugeki no Soma S2".into()];
    s.series_type = SeriesType::Anime;
    s.use_scene_numbering = false;
    let mut e = episode(4201, 1, 1);
    e.absolute_episode_number = Some(1);
    e.title = Some("Native Episode".into());
    s.episodes = vec![e];
    s
}

#[test]
fn known_native_file_should_keep_its_library_season_despite_foreign_scene_offset() {
    let series = native_series();
    let parsed = parse_title("Native.Anime.Run.S01E01.1080p.WEB.mkv").unwrap();
    for use_map in [false, true] {
        let episodes = if use_map {
            map_remote(&parsed, &series).episodes
        } else {
            crate::map_episodes(&parsed, &series)
        };
        assert_eq!(ids(&episodes), [4201], "use_map {use_map}");
    }
}

#[test]
fn known_native_alias_or_absolute_file_should_keep_the_exact_native_episode() {
    let series = native_series();
    for title in [
        "Shokugeki no Soma S2 S01E01.1080p.WEB.mkv",
        "Shokugeki no Soma S2 - 01 [Group][1080p].mkv",
        "[Group] Native Anime Run - 01 [1080p].mkv",
    ] {
        let remote = map_remote(&parse_title(title).unwrap(), &series);
        assert_eq!(remote.numbering_rejection, None, "{title}");
        assert_eq!(ids(&remote.episodes), [4201], "{title}");
        assert_eq!(remote.episodes[0].season_number, 1, "{title}");
    }
}

// ---------------------------------------------------------------------------------------------
// MovieRssMappingFixture
// ---------------------------------------------------------------------------------------------
//
// Tanukarr keeps films in the series table, so an RSS release the episode parser misreads can
// be mapped to a film by title. Kitsunarr's films are not library series: RSS sync matches a
// release no series claims with a film (`MovieReleaseParser.MatchUnique`, in `kitsunarr-api`)
// and decides it with `Engine::evaluate_movie`, the film as one episode (S01E01). These tests
// check that decision.

/// `_movie` and `_feature` (episode 20, S01E01).
fn movie_decision(title: &str) -> crate::Decision {
    let mut film = crate::tests::series();
    film.id = 10;
    film.title = "Generated Feature One".into();
    film.year = 2019;
    film.episodes = vec![episode(20, 1, 1)];
    let settings = crate::tests::settings();
    let engine = crate::Engine {
        series: &film,
        library: &[],
        formats: &[],
        settings: &settings,
    };
    let candidate = kitsunarr_parser::movie::MovieCandidate {
        title: "Generated Feature One",
        year: 2019,
        ..Default::default()
    };
    let releases = [crate::tests::release(title)];
    engine
        .evaluate_movie(&releases, &SearchCriteria::rss(), &candidate)
        .remove(0)
}

#[test]
fn should_use_the_verified_movie_title_year_when_the_legacy_rss_parser_reads_a_year_as_an_episode()
{
    let d = movie_decision("Generated.Feature.One.2019.720p.WEB-DL.H264-Fixture");
    let episodes: Vec<i64> = d.episodes.iter().map(|e| e.id).collect();
    assert_eq!(episodes, [20]);
    assert_eq!(d.parsed.unwrap().series_title_info.year, 2019);
}

#[test]
fn should_not_let_legacy_rss_episode_coordinates_bypass_ordinary_movie_identity() {
    let d = movie_decision("Generated.Feature.One.S01E01.720p.WEB-DL.H264-Fixture");
    assert!(d.episodes.is_empty());
}

/// Files already in the series folder are mapped with aired numbering (`sceneSource: false`),
/// releases with scene numbering.
#[test]
fn library_files_use_aired_numbering() {
    let mut series = crate::tests::series();
    series.use_scene_numbering = true;
    let mut ep = |id: i64, s: i32, n: i32, ss: i32, sn: i32| {
        let mut e = crate::tests::ep(id, s, n, 30);
        e.scene_season_number = Some(ss);
        e.scene_episode_number = Some(sn);
        series.episodes.push(e);
    };
    ep(1, 1, 7, 2, 1);
    ep(2, 2, 1, 3, 1);
    let parsed = kitsunarr_parser::parse_title("Series.Title.S02E01.720p.HDTV-GRP").unwrap();
    let ids = |s: &crate::Series| -> Vec<i64> {
        crate::map_episodes(&parsed, s)
            .iter()
            .map(|e| e.id)
            .collect()
    };
    assert_eq!(ids(&series), [1], "a release: scene 2x01");
    assert_eq!(
        ids(&crate::aired_numbering(&series)),
        [2],
        "a library file: aired 2x01"
    );
}
