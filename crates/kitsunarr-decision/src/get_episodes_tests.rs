//! Sonarr's `ParsingServiceTests/GetEpisodesFixture`: `ParsingService.Map` and `GetEpisodes`
//! (`GetDailyEpisode`, `GetAnimeEpisodes`, `GetStandardEpisodes`, full-season lookups).
//!
//! Upstream verifies which `IEpisodeService` lookups run against mocks. Kitsunarr's mapping is a
//! pure function over the series' episodes, so each test builds episodes that the alternative
//! lookups would find differently and checks which ones come back.
//!
//! Scene mappings: Kitsunarr has no `SceneMappingService`. TheXEM's names for one scene season
//! (`Series.scene_season_titles`, a `SceneMapping` of type `XemService` with a
//! `SceneSeasonNumber` and no `SeasonNumber`) stand in for `GetSceneSeasonNumber` and
//! `FindSceneMapping`. Mappings with a TheTVDB season offset or an origin do not exist here.
//!
//! `GetEpisodes(…, sceneSource: false, …)`: Kitsunarr maps every release as a scene source;
//! without a scene mapping (all such tests here) both give the same episodes.

use chrono::{Local, TimeDelta};
use kitsunarr_domain::{Language, SeriesType};
use kitsunarr_parser::{ParsedEpisodeInfo, parse_title};

use crate::mapping;
use crate::model::{Episode, SearchCriteria, SearchKind, Series};

/// `DateTime.Today.AddDays(days).ToString(Episode.AIR_DATE_FORMAT)`.
fn air_date(days: i64) -> String {
    (Local::now().date_naive() + TimeDelta::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

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

fn aired(mut e: Episode, days: i64) -> Episode {
    e.air_date = Some(air_date(days));
    e
}

fn absolute(mut e: Episode, number: i32) -> Episode {
    e.absolute_episode_number = Some(number);
    e
}

fn scene(mut e: Episode, season: i32, number: i32) -> Episode {
    e.scene_season_number = Some(season);
    e.scene_episode_number = Some(number);
    e
}

fn scene_absolute(mut e: Episode, number: i32) -> Episode {
    e.scene_absolute_episode_number = Some(number);
    e
}

/// `_series`.
fn series(episodes: Vec<Episode>) -> Series {
    let mut s = crate::tests::series();
    s.title = "30 Stone".into();
    s.episodes = episodes;
    s
}

/// `GivenDailySeries`.
fn daily_series(episodes: Vec<Episode>) -> Series {
    let mut s = series(episodes);
    s.series_type = SeriesType::Daily;
    s
}

/// `GivenSceneNumberingSeries`.
fn scene_numbering_series(episodes: Vec<Episode>) -> Series {
    let mut s = series(episodes);
    s.use_scene_numbering = true;
    s
}

/// A TheXEM name for one scene season: `GetSceneSeasonNumber(SeriesTitle, …)` returns
/// `season`.
fn with_scene_season_title(mut s: Series, season: i32) -> Series {
    s.scene_season_titles = vec![("30 Stone".into(), season)];
    s
}

/// `_parsedEpisodeInfo`.
fn parsed_info() -> ParsedEpisodeInfo {
    ParsedEpisodeInfo {
        series_title: "30 Stone".into(),
        season_numbers: vec![1],
        episode_numbers: vec![1],
        absolute_episode_numbers: Vec::new(),
        languages: Language::by_name("English").into_iter().collect(),
        ..Default::default()
    }
}

/// `GivenDailyParseResult`.
fn daily_parsed() -> ParsedEpisodeInfo {
    ParsedEpisodeInfo {
        air_date: Some(air_date(0)),
        ..parsed_info()
    }
}

/// `GivenAbsoluteNumberingSeries`.
fn absolute_parsed() -> ParsedEpisodeInfo {
    ParsedEpisodeInfo {
        absolute_episode_numbers: vec![1],
        ..parsed_info()
    }
}

/// `GivenFullSeason`.
fn full_season(seasons: Vec<i32>) -> ParsedEpisodeInfo {
    ParsedEpisodeInfo {
        full_season: true,
        season_numbers: seasons,
        episode_numbers: Vec::new(),
        ..parsed_info()
    }
}

/// `_singleEpisodeSearchCriteria` for `e`.
fn single_episode(e: &Episode) -> SearchCriteria {
    SearchCriteria {
        kind: SearchKind::SingleEpisode {
            season: e.season_number,
            episode: e.episode_number,
        },
        episode_ids: vec![e.id],
        monitored_episodes_only: false,
        user_invoked: true,
    }
}

fn ids(episodes: Vec<&Episode>) -> Vec<i64> {
    episodes.iter().map(|e| e.id).collect()
}

/// `Map(parsed, series)` / `GetEpisodes(parsed, series, true, null)`.
fn map(parsed: &ParsedEpisodeInfo, series: &Series) -> Vec<i64> {
    ids(crate::map_episodes(parsed, series))
}

/// `Map(…, searchCriteria)` / `GetEpisodes(…, searchCriteria)`.
fn map_searched(
    parsed: &ParsedEpisodeInfo,
    series: &Series,
    criteria: &SearchCriteria,
) -> Vec<i64> {
    ids(mapping::episodes(parsed, series, criteria))
}

#[test]
fn should_match_early_daily_release_to_known_air_date() {
    for days in [2, 7, 14] {
        let date = air_date(days);
        let s = daily_series(vec![aired(episode(1, 1, 1), days)]);
        let parsed = parse_title(&format!("30.Stone.{date}.1080p.WEB-DL.H264-Sonarr"))
            .unwrap_or_else(|| panic!("{date} parses"));
        assert_eq!(map(&parsed, &s), [1], "{days} days ahead");
    }
}

#[test]
fn should_not_match_early_daily_release_without_matching_episode_metadata() {
    let date = air_date(7);
    let s = daily_series(vec![aired(episode(1, 1, 1), 0)]);
    let parsed = parse_title(&format!("30.Stone.{date}.1080p.WEB-DL.H264-Sonarr")).unwrap();
    assert!(map(&parsed, &s).is_empty());
}

#[test]
fn should_get_daily_episode_episode_when_search_criteria_is_null() {
    // `FindEpisode(seriesId, airDate, null)`.
    let s = daily_series(vec![
        aired(episode(1, 1, 1), 0),
        aired(episode(2, 1, 2), -1),
    ]);
    assert_eq!(map(&daily_parsed(), &s), [1]);
}

#[test]
fn should_use_search_criteria_episode_when_it_matches_daily() {
    // Two regular episodes air today: by air date alone (`FindEpisode`) the date is
    // ambiguous, so only the searched episode can be the one mapped.
    let s = daily_series(vec![aired(episode(1, 1, 1), 0), aired(episode(2, 1, 2), 0)]);
    let criteria = single_episode(&s.episodes[1]);
    assert_eq!(map_searched(&daily_parsed(), &s, &criteria), [2]);
    assert!(map(&daily_parsed(), &s).is_empty());
}

#[test]
fn should_fallback_to_daily_episode_lookup_when_search_criteria_episode_doesnt_match() {
    let s = daily_series(vec![
        aired(episode(1, 1, 1), 0),
        aired(episode(2, 1, 2), -5),
    ]);
    let criteria = single_episode(&s.episodes[0]);
    let parsed = ParsedEpisodeInfo {
        air_date: Some(air_date(-5)),
        ..parsed_info()
    };
    assert_eq!(map_searched(&parsed, &s, &criteria), [2]);
}

#[test]
fn should_get_daily_episode_episode_should_lookup_including_daily_part() {
    // `FindEpisode(seriesId, airDate, part)`: specials aside, the part-th episode of the day
    // in season and episode order.
    let s = daily_series(vec![
        aired(episode(1, 0, 1), 0),
        aired(episode(2, 1, 2), 0),
        aired(episode(3, 1, 1), 0),
    ]);
    let part = |n| ParsedEpisodeInfo {
        daily_part: Some(n),
        ..daily_parsed()
    };
    assert_eq!(map(&part(1), &s), [3]);
    assert_eq!(map(&part(2), &s), [2]);
}

#[test]
fn should_use_search_criteria_episode_when_it_matches_absolute() {
    // No scene absolute numbers (`FindEpisodesBySceneNumbering` finds nothing): the absolute
    // number, never the air date.
    let s = series(vec![
        absolute(aired(episode(1, 1, 1), 0), 1),
        absolute(episode(2, 1, 2), 2),
    ]);
    let criteria = single_episode(&s.episodes[0]);
    assert_eq!(map_searched(&absolute_parsed(), &s, &criteria), [1]);
}

#[test]
fn should_use_scene_numbering_when_series_uses_scene_numbering() {
    // `FindEpisodesBySceneNumbering(seriesId, 1, 1)`: scene 1x01 is TheTVDB's 1x02.
    let s = scene_numbering_series(vec![
        scene(episode(1, 1, 1), 1, 2),
        scene(episode(2, 1, 2), 1, 1),
    ]);
    assert_eq!(map(&parsed_info(), &s), [2]);
}

#[test]
fn should_match_search_criteria_by_scene_numbering() {
    // The searched episode matches by scene numbering, so the series-wide scene lookup (which
    // would also find episode 2) is not used.
    let s = scene_numbering_series(vec![
        scene(episode(1, 1, 1), 1, 1),
        scene(episode(2, 1, 5), 1, 1),
    ]);
    let criteria = single_episode(&s.episodes[0]);
    assert_eq!(map_searched(&parsed_info(), &s, &criteria), [1]);
    assert_eq!(map(&parsed_info(), &s), [1, 2]);
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_fallback_to_findEpisode_when_search_criteria_match_fails_for_scene_numbering() {
    let s = scene_numbering_series(vec![
        scene(episode(1, 1, 1), 1, 10),
        scene(episode(2, 1, 2), 1, 1),
    ]);
    let criteria = single_episode(&s.episodes[0]);
    assert_eq!(map_searched(&parsed_info(), &s, &criteria), [2]);
}

#[test]
fn should_find_episode() {
    // `FindEpisode(seriesId, 1, 1)`.
    let s = series(vec![episode(1, 1, 1), episode(2, 1, 2)]);
    assert_eq!(map(&parsed_info(), &s), [1]);
}

#[test]
fn should_match_episode_with_search_criteria() {
    let s = series(vec![episode(1, 1, 1), episode(2, 1, 2)]);
    let criteria = single_episode(&s.episodes[0]);
    assert_eq!(map_searched(&parsed_info(), &s, &criteria), [1]);
}

#[test]
#[allow(non_snake_case)] // Upstream's name.
fn should_fallback_to_findEpisode_when_search_criteria_match_fails() {
    let s = series(vec![episode(1, 1, 10), episode(2, 1, 1)]);
    let criteria = single_episode(&s.episodes[0]);
    assert_eq!(map_searched(&parsed_info(), &s, &criteria), [2]);
}

#[test]
fn should_look_for_episode_in_season_zero_if_absolute_special() {
    // `FindEpisode(seriesId, 0, 1)`, never `FindEpisodesBySceneNumbering(seriesId, 0, 1)`.
    let s = series(vec![
        episode(1, 0, 1),
        absolute(episode(2, 1, 1), 1),
        scene(episode(3, 1, 2), 0, 1),
    ]);
    let parsed = ParsedEpisodeInfo {
        special: true,
        ..absolute_parsed()
    };
    assert_eq!(map(&parsed, &s), [1]);
}

#[test]
fn should_use_scene_numbering_when_scene_season_number_has_value() {
    for season in [0, 1, 2] {
        // Nothing at scene `season`x01: `FindEpisode(seriesId, season, 1)`.
        let s = with_scene_season_title(
            series(vec![
                absolute(episode(1, season, 1), 10),
                absolute(episode(2, 5, 1), 1),
            ]),
            season,
        );
        assert_eq!(map(&absolute_parsed(), &s), [1], "scene season {season}");
    }
}

#[test]
fn should_find_episode_by_season_and_scene_absolute_episode_number() {
    for season in [0, 1, 2] {
        // `FindEpisodesBySceneNumbering(seriesId, season, 1)` finds episode 1.
        let s = with_scene_season_title(
            series(vec![
                scene(episode(1, 3, 7), season, 1),
                episode(2, season, 1),
            ]),
            season,
        );
        assert_eq!(map(&absolute_parsed(), &s), [1], "scene season {season}");
    }
}

#[test]
fn should_find_episode_by_parsed_season_and_absolute_episode_number_when_season_number_is_2_or_higher()
 {
    for season in [2, 20] {
        let s = series(vec![
            scene(episode(1, 1, 5), season, 1),
            episode(2, season, 1),
        ]);
        let parsed = ParsedEpisodeInfo {
            season_numbers: vec![season],
            episode_numbers: Vec::new(),
            ..absolute_parsed()
        };
        assert_eq!(map(&parsed, &s), [1], "season {season}");
    }
}

#[test]
fn should_find_episode_by_parsed_season_and_absolute_episode_number_when_season_number_is_2_or_higher_and_scene_season_number_lookup_failed()
 {
    for season in [2, 20] {
        // Nothing at scene `season`x01: `FindEpisode(seriesId, season, 1)`.
        let s = series(vec![episode(1, season, 1), absolute(episode(2, 1, 1), 1)]);
        let parsed = ParsedEpisodeInfo {
            season_numbers: vec![season],
            episode_numbers: Vec::new(),
            ..absolute_parsed()
        };
        assert_eq!(map(&parsed, &s), [1], "season {season}");
    }
}

#[test]
fn should_not_find_episode_by_parsed_season_and_absolute_episode_number_when_season_number_is_2_or_higher_and_a_episode_number_was_parsed()
 {
    for season in [2, 20] {
        // An episode number was parsed: the scene absolute number
        // (`FindEpisodesBySceneNumbering(seriesId, 1)`), not the parsed season.
        let s = series(vec![
            scene_absolute(episode(1, 1, 1), 1),
            scene(episode(2, 1, 5), season, 1),
            episode(3, season, 1),
            absolute(episode(4, 1, 2), 1),
        ]);
        let parsed = ParsedEpisodeInfo {
            season_numbers: vec![season],
            episode_numbers: vec![1],
            ..absolute_parsed()
        };
        assert_eq!(map(&parsed, &s), [1], "season {season}");
    }
}

#[test]
fn should_return_episodes_when_scene_absolute_episode_number_returns_multiple_results() {
    for season in [0, 1, 2] {
        let mut episodes: Vec<Episode> = (1..=5)
            .map(|n| scene(episode(n, 9, n as i32), season, 1))
            .collect();
        episodes.push(episode(6, season, 1));
        let s = with_scene_season_title(series(episodes), season);
        assert_eq!(
            map(&absolute_parsed(), &s),
            [1, 2, 3, 4, 5],
            "scene season {season}"
        );
    }
}

#[test]
fn should_find_episode_by_season_and_absolute_episode_number_when_scene_absolute_episode_number_returns_no_results()
 {
    for season in [0, 1, 2] {
        // Nothing at scene `season`x01: `FindEpisode(seriesId, season, 1)`.
        let s = with_scene_season_title(
            series(vec![
                scene(episode(1, 7, 1), 7, 1),
                episode(2, season, 1),
                absolute(episode(3, 5, 2), 1),
            ]),
            season,
        );
        assert_eq!(map(&absolute_parsed(), &s), [2], "scene season {season}");
    }
}

// should_use_tvdb_season_number_when_available_and_a_scene_source: not ported. It needs a scene
// mapping with a TheTVDB season (`SeasonNumber = 5` for `SceneSeasonNumber = 1`) shifting the
// mapped season; Kitsunarr's scene names (TheXEM's) carry no TheTVDB season.

// should_not_use_tvdb_season_number_when_available_for_a_different_season_and_a_scene_source:
// not ported, for the same reason (the offset of a mapping for a later scene season).

#[test]
fn should_not_use_tvdb_season_when_not_a_scene_source() {
    // No scene mapping: `FindEpisode(seriesId, 1, 1)`, never season 5.
    let s = series(vec![episode(1, 1, 1), episode(2, 5, 1)]);
    assert_eq!(map(&parsed_info(), &s), [1]);
}

#[test]
fn should_not_use_tvdb_season_when_tvdb_season_number_is_less_than_zero() {
    // A mapping without a TheTVDB season (TheXEM's, for the parsed season 1) leaves the
    // season as parsed.
    let s = with_scene_season_title(series(vec![episode(1, 1, 1), episode(2, 5, 1)]), 1);
    assert_eq!(map(&parsed_info(), &s), [1]);
}

#[test]
fn should_lookup_full_season_by_season_number_if_series_does_not_use_scene_numbering() {
    // `GetEpisodesBySeason`, never `GetEpisodesBySceneSeason`.
    let s = series(vec![
        scene(episode(1, 1, 1), 2, 1),
        scene(episode(2, 2, 1), 1, 1),
    ]);
    assert_eq!(map(&full_season(vec![1]), &s), [1]);
}

#[test]
fn should_lookup_every_season_in_a_multi_season_range() {
    let seasons = [1, 2, 3, 4];
    let s = series(
        seasons
            .iter()
            .map(|&n| episode(i64::from(n) * 100 + 1, n, 1))
            .collect(),
    );
    let mapped = crate::map_episodes(&full_season(seasons.to_vec()), &s);
    let mapped_seasons: Vec<i32> = mapped.iter().map(|e| e.season_number).collect();
    assert_eq!(mapped_seasons, seasons);
}

/// Stargate SG-1's seasons, `sizes[n - 1]` episodes in season `n`.
fn stargate(sizes: &[i32]) -> Series {
    series(
        (1..=sizes.len() as i32)
            .flat_map(|season| {
                (1..=sizes[season as usize - 1])
                    .map(move |n| episode(i64::from(season * 100 + n), season, n))
            })
            .collect(),
    )
}

const STARGATE_SEASON_SIZES: [i32; 10] = [22, 22, 22, 22, 22, 22, 21, 21, 20, 20];

fn distinct_seasons(episodes: &[&Episode]) -> Vec<i32> {
    let mut seasons: Vec<i32> = episodes.iter().map(|e| e.season_number).collect();
    seasons.dedup();
    seasons
}

#[test]
fn should_map_only_stargate_endpoint_seasons_for_discrete_title() {
    let parsed =
        parse_title("Stargate SG 1 (1997) S01 S10 (1080p BluRay x265 10bit AC3 5.1 Celdra) [QxR]")
            .unwrap();
    let s = stargate(&STARGATE_SEASON_SIZES);
    let mapped = crate::map_episodes(&parsed, &s);
    assert_eq!(distinct_seasons(&mapped), [1, 10]);
    assert_eq!(mapped.len(), 42);
}

#[test]
fn should_map_all_stargate_seasons_for_continuous_title() {
    let parsed =
        parse_title("Stargate SG-1 (1997) S01-S10 (1080p BluRay x265 10bit AC3 5.1 Celdra) [QxR]")
            .unwrap();
    let s = stargate(&STARGATE_SEASON_SIZES);
    let mapped = crate::map_episodes(&parsed, &s);
    assert_eq!(distinct_seasons(&mapped), (1..=10).collect::<Vec<_>>());
    assert_eq!(mapped.len(), 214);
}

#[test]
fn should_apply_scene_mapping_to_each_season_in_a_multi_season_range() {
    // Upstream maps season 2 through a TheTVDB-origin mapping to season 5. Kitsunarr has no
    // such mappings; TheXEM's name for scene season 5 does the same for season 1
    // (`GetSceneSeasonMapping`: an `XemService` mapping moves season 1 to its scene season),
    // and every other season is looked up by its own scene season.
    let s = with_scene_season_title(
        scene_numbering_series(vec![
            scene(episode(101, 1, 1), 1, 1),
            scene(episode(201, 2, 1), 2, 1),
            scene(episode(301, 3, 1), 3, 1),
            scene(episode(401, 4, 1), 4, 1),
            scene(episode(501, 6, 1), 5, 1),
        ]),
        5,
    );
    assert_eq!(
        map(&full_season(vec![1, 2, 3, 4]), &s),
        [501, 201, 301, 401]
    );
}

#[test]
fn should_lookup_full_season_by_scene_season_number_if_series_uses_scene_numbering() {
    // `GetEpisodesBySceneSeason`, never `GetEpisodesBySeason`.
    let s = scene_numbering_series(vec![
        scene(episode(1, 1, 1), 2, 1),
        scene(episode(2, 2, 1), 1, 1),
    ]);
    assert_eq!(map(&full_season(vec![1]), &s), [2]);
}

#[test]
fn multi_season_pack_should_map_only_the_season_being_searched() {
    let s = series(vec![episode(1, 1, 1), episode(2, 2, 1), episode(3, 3, 1)]);
    let criteria = SearchCriteria {
        kind: SearchKind::Season { season: 3 },
        episode_ids: vec![3],
        monitored_episodes_only: false,
        user_invoked: true,
    };
    assert_eq!(
        map_searched(&full_season(vec![1, 2, 3]), &s, &criteria),
        [3]
    );
}

#[test]
fn should_fallback_to_lookup_full_season_by_season_number_if_series_uses_scene_numbering_and_no_epsiodes_are_found_by_scene_season_number()
 {
    // Nothing in scene season 1: `GetEpisodesBySeason(seriesId, 1)`.
    let s = scene_numbering_series(vec![episode(1, 1, 1), scene(episode(2, 2, 1), 2, 1)]);
    assert_eq!(map(&full_season(vec![1]), &s), [1]);
}

#[test]
fn should_lookup_every_season_in_a_multi_season_pack() {
    // The series does not use scene numbering: scene seasons are ignored.
    let s = series(vec![
        scene(episode(101, 1, 1), 3, 1),
        scene(episode(102, 1, 2), 3, 2),
        scene(episode(201, 2, 1), 1, 1),
        scene(episode(202, 2, 2), 1, 2),
        scene(episode(203, 2, 3), 1, 3),
        scene(episode(301, 3, 1), 2, 1),
        episode(401, 4, 1),
    ]);
    assert_eq!(
        map(&full_season(vec![1, 2, 3]), &s),
        [101, 102, 201, 202, 203, 301]
    );
}

#[test]
fn should_use_season_zero_when_looking_up_is_partial_special_episode_found_by_title() {
    let mut special = episode(1, 0, 1);
    special.title = Some("My Special Episode".into());
    let s = series(vec![special, episode(2, 1, 0), episode(3, 1, 1)]);
    let parsed = ParsedEpisodeInfo {
        episode_numbers: vec![0],
        release_title: "Series.Title.S01E00.My.Special.Episode.1080p.AMZN.WEB-DL.DDP5.1.H264-TEPES"
            .into(),
        ..parsed_info()
    };
    assert_eq!(map(&parsed, &s), [1]);
}

#[test]
fn should_use_original_parse_result_when_special_episode_lookup_by_title_fails() {
    let mut special = episode(1, 0, 1);
    special.title = Some("Another Episode Entirely".into());
    let s = series(vec![special, episode(2, 1, 0), episode(3, 1, 1)]);
    let parsed = ParsedEpisodeInfo {
        episode_numbers: vec![0],
        release_title: "Series.Title.S01E00.My.Special.Episode.1080p.AMZN.WEB-DL.DDP5.1.H264-TEPES"
            .into(),
        ..parsed_info()
    };
    assert_eq!(map(&parsed, &s), [2]);
}
