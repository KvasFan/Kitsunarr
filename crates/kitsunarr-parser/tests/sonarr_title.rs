//! Sonarr's title, path and episode parser tests, replayed against the port.

// Each case body is an immediately invoked closure so it can use `?`.
#![allow(clippy::redundant_closure_call)]

mod common;

use chrono::{Local, TimeDelta};
use common::{
    Fixture, Outcome, check, enum_name, equivalent, expect_eq, i, ints, path_arg, quality_name, s,
};
use kitsunarr_domain::Language;
use kitsunarr_parser::{
    DetectionSource, ParsedEpisodeInfo, ReleaseType, clean_series_title, format_season_numbers,
    is_scene_title, normalize_episode_title, parse_path, parse_quality, parse_release_group,
    parse_season_title, parse_series_name, parse_title,
};
use serde_json::Value;

type R = Result<(), String>;

fn parsed(title: &str) -> Result<ParsedEpisodeInfo, String> {
    parse_title(title).ok_or_else(|| "not parsed".to_string())
}

fn parsed_path(path: &str) -> Result<ParsedEpisodeInfo, String> {
    parse_path(path).ok_or_else(|| "path not parsed".to_string())
}

/// `.Single()`
fn single(what: &str, v: &[i32]) -> Result<i32, String> {
    match v {
        [x] => Ok(*x),
        _ => Err(format!("{what}: expected exactly one, got {v:?}")),
    }
}

/// `.SingleOrDefault()`
fn single_or_default(what: &str, v: &[i32]) -> Result<i32, String> {
    match v {
        [] => Ok(0),
        [x] => Ok(*x),
        _ => Err(format!("{what}: expected at most one, got {v:?}")),
    }
}

fn title_is(r: &ParsedEpisodeInfo, want: &Value) -> R {
    expect_eq("series title", r.series_title.as_str(), s(want))
}

fn seasons_are(r: &ParsedEpisodeInfo, want: &[i32]) -> R {
    expect_eq("seasons", r.season_numbers.as_slice(), want)
}

/// `XmlCleaner.ReplaceUnicode`: drops characters XML cannot carry.
fn replace_unicode(text: &str) -> String {
    text.chars()
        .filter(
            |&c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}'),
        )
        .collect()
}

fn language(identifier: &str) -> &'static Language {
    Language::by_name(identifier).unwrap_or_else(|| panic!("unknown language {identifier}"))
}

fn languages_exactly(r: &ParsedEpisodeInfo, want: &[&str]) -> R {
    let mut got: Vec<&str> = r.languages.iter().map(|l| l.name).collect();
    let mut want: Vec<&str> = want.iter().map(|n| language(n).name).collect();
    got.sort_unstable();
    want.sort_unstable();
    expect_eq("languages", got, want)
}

#[test]
fn absolute_episode_number_parser_fixture() {
    Fixture::load("AbsoluteEpisodeNumberParserFixture").run(|method, a| {
        let result = (|| -> R {
            match method {
                "should_parse_absolute_numbers" => {
                    let r = parsed(s(&a[0]))?;
                    expect_eq(
                        "absolute",
                        single("absolute", &r.absolute_episode_numbers)?,
                        i(&a[2]) as i32,
                    )?;
                    seasons_are(&r, &[i(&a[3]) as i32])?;
                    expect_eq(
                        "episode",
                        single_or_default("episode", &r.episode_numbers)?,
                        i(&a[4]) as i32,
                    )?;
                    title_is(&r, &a[1])?;
                    check(!r.full_season, "full season")
                }
                "should_parse_absolute_specials" => {
                    let r = parsed(s(&a[0]))?;
                    expect_eq(
                        "absolute",
                        single("absolute", &r.absolute_episode_numbers)?,
                        i(&a[2]) as i32,
                    )?;
                    seasons_are(&r, &[0])?;
                    expect_eq(
                        "episode",
                        single_or_default("episode", &r.episode_numbers)?,
                        0,
                    )?;
                    title_is(&r, &a[1])?;
                    check(!r.full_season, "full season")?;
                    check(r.special, "not special")
                }
                "should_parse_absolute_specials_without_absolute_number" => {
                    let r = parsed(s(&a[0]))?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    seasons_are(&r, &[0])?;
                    check(r.episode_numbers.is_empty(), "episodes present")?;
                    title_is(&r, &a[1])?;
                    check(!r.full_season, "full season")?;
                    check(r.special, "not special")
                }
                "should_parse_multi_episode_absolute_numbers" => {
                    let r = parsed(s(&a[0]))?;
                    let want: Vec<i32> = (i(&a[2]) as i32..=i(&a[3]) as i32).collect();
                    equivalent("absolute", &r.absolute_episode_numbers, &want)?;
                    title_is(&r, &a[1])?;
                    check(!r.full_season, "full season")
                }
                "should_parse_anime_season_packs" => {
                    let r = parsed(s(&a[0]))?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    title_is(&r, &a[1])?;
                    check(r.full_season, "not a full season")?;
                    seasons_are(&r, &[i(&a[2]) as i32])
                }
                "should_handle_anime_recap_numbering" => {
                    let r = parsed(s(&a[0]))?;
                    title_is(&r, &a[1])?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    expect_eq(
                        "special absolute",
                        r.special_absolute_episode_numbers.clone(),
                        vec![a[2].as_f64().unwrap_or(f64::NAN)],
                    )?;
                    check(!r.full_season, "full season")
                }
                "should_handle_ambiguously_named_anime_files_in_batch_release" => {
                    let path = format!("/C/Test/{}/{}", s(&a[0]), s(&a[1]));
                    let r = parsed_path(&path)?;
                    expect_eq(
                        "absolute",
                        single("absolute", &r.absolute_episode_numbers)?,
                        i(&a[3]) as i32,
                    )?;
                    seasons_are(&r, &[0])?;
                    check(r.episode_numbers.is_empty(), "episodes present")?;
                    title_is(&r, &a[2])?;
                    check(!r.full_season, "full season")
                }
                "should_parse_absolute_followed_by_standard_as_standard" => {
                    let r = parsed(s(&a[0]))?;
                    expect_eq("episode count", r.episode_numbers.len(), 1)?;
                    seasons_are(&r, &[i(&a[3]) as i32])?;
                    expect_eq("episode", r.episode_numbers[0], i(&a[4]) as i32)?;
                    title_is(&r, &a[1])?;
                    expect_eq(
                        "special absolute",
                        r.special_absolute_episode_numbers.clone(),
                        vec![a[2].as_f64().unwrap_or(f64::NAN)],
                    )?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    check(!r.full_season, "full season")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })();
        outcome(result)
    });
}

/// Maps the closure convention (`\0unhandled`) onto the harness outcome.
fn outcome(result: R) -> Option<Outcome> {
    match result {
        Err(e) if e == "\u{0}unhandled" => None,
        other => Some(other.into()),
    }
}

#[test]
fn anime_metadata_parser_fixture() {
    Fixture::load("AnimeMetadataParserFixture").run(|method, a| {
        outcome((|| -> R {
            let r = parsed(s(&a[0]))?;
            match method {
                "should_parse_releasegroup_and_hash" => {
                    expect_eq("release group", r.release_group.as_deref(), a[1].as_str())?;
                    expect_eq("hash", r.release_hash.as_str(), a[2].as_str().unwrap_or(""))
                }
                "should_parse_release_hashes_with_10b_or_8b" => {
                    expect_eq("hash", r.release_hash.as_str(), a[1].as_str().unwrap_or(""))
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn anime_version_fixture() {
    Fixture::load("AnimeVersionFixture").run(|method, a| {
        outcome(match method {
            "should_be_able_to_parse_repack" => expect_eq(
                "version",
                i64::from(parse_quality(s(&a[0])).revision.version),
                i(&a[1]),
            ),
            _ => Err("\u{0}unhandled".into()),
        })
    });
}

/// Deterministic pseudo-random strings (upstream uses `new Random()`).
fn random_titles(length: usize, count: usize) -> Vec<String> {
    const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15 ^ length as u64;
    (0..count)
        .map(|_| {
            (0..length)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    CHARSET[(state % CHARSET.len() as u64) as usize] as char
                })
                .collect()
        })
        .collect()
}

#[test]
fn crap_parser_fixture() {
    Fixture::load("CrapParserFixture").run(|method, a| {
        outcome(match method {
            "should_not_parse_crap"
            | "should_not_parse_file_name_without_proper_spacing"
            | "should_not_parse_invalid_release_name"
            | "should_not_parse_special_with_part_number" => {
                check(parse_title(s(&a[0])).is_none(), "parsed")
            }
            "should_parse_unknown_formats_without_error" => {
                check(parse_title(s(&a[0])).is_some(), "not parsed")
            }
            "should_not_parse_random" => {
                let parsed: Vec<String> = random_titles(i(&a[0]) as usize, 500)
                    .into_iter()
                    .filter(|t| parse_title(t).is_some())
                    .collect();
                check(
                    parsed.is_empty(),
                    &format!("parsed random titles {parsed:?}"),
                )
            }
            _ => Err("\u{0}unhandled".into()),
        })
    });
}

/// `should_not_parse_md5` (a plain `[Test]`): hash-like names are rejected.
#[test]
fn does_not_parse_hashes() {
    for t in random_titles(32, 100) {
        let hex: String = t
            .chars()
            .map(|c| if c.is_ascii_digit() { c } else { 'A' })
            .collect();
        assert!(parse_title(&hex).is_none(), "{hex}");
    }
}

fn expand(template: &str, year: i32, month: &str, day: &str) -> String {
    template
        .replace("{year}", &year.to_string())
        .replace("{month}", month)
        .replace("{day}", day)
}

#[test]
fn daily_episode_parser_fixture() {
    Fixture::load("DailyEpisodeParserFixture").run(|method, a| {
        outcome((|| -> R {
            let daily =
                |r: &ParsedEpisodeInfo, want: &Value, y: &Value, m: &Value, d: &Value| -> R {
                    title_is(r, want)?;
                    let date = format!("{:04}-{:02}-{:02}", i(y), i(m), i(d));
                    expect_eq("air date", r.air_date.as_deref(), Some(date.as_str()))?;
                    check(r.episode_numbers.is_empty(), "episodes present")?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    check(!r.full_season, "full season")
                };
            match method {
                "should_parse_daily_episode"
                | "should_parse_daily_episode_using_short_month_format" => {
                    let r = parsed(s(&a[0]))?;
                    daily(&r, &a[1], &a[2], &a[3], &a[4])
                }
                "should_parse_daily_episode_with_multiple_parts" => {
                    let r = parsed(s(&a[0]))?;
                    daily(&r, &a[1], &a[2], &a[3], &a[4])?;
                    expect_eq("part", r.daily_part, Some(i(&a[5]) as i32))
                }
                "should_not_accept_ancient_daily_series" => check(
                    parse_title(&expand(s(&a[0]), 1950, "10", "14")).is_none(),
                    "parsed",
                ),
                "should_accept_dates_within_future_limit" => {
                    let date = Local::now().date_naive() + TimeDelta::days(2);
                    let title = expand(
                        s(&a[0]),
                        date.format("%Y").to_string().parse().unwrap_or(0),
                        &date.format("%m").to_string(),
                        &date.format("%d").to_string(),
                    );
                    let r = parsed(&title)?;
                    expect_eq(
                        "air date",
                        r.air_date,
                        Some(date.format("%Y-%m-%d").to_string()),
                    )
                }
                "should_parse_episodes_released_before_their_air_date" => {
                    let date = Local::now().date_naive() + TimeDelta::days(i(&a[0]));
                    let title = format!(
                        "A.Talk.Show.{}.1080p.WEB-DL.H264-Sonarr",
                        date.format("%Y.%m.%d")
                    );
                    expect_eq(
                        "air date",
                        parsed(&title)?.air_date,
                        Some(date.format("%Y-%m-%d").to_string()),
                    )
                }
                "should_not_parse_dates_beyond_future_limit" => {
                    let date = Local::now().date_naive() + TimeDelta::days(i(&a[0]));
                    let title = format!(
                        "A.Talk.Show.{}.1080p.WEB-DL.H264-Sonarr",
                        date.format("%Y.%m.%d")
                    );
                    check(parse_title(&title).is_none(), "parsed")
                }
                "should_not_parse_ambiguous_daily_episode" => {
                    check(parse_title(s(&a[0])).is_none(), "parsed")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

/// `should_parse_daily_episode_into_season_zero` (a plain `[Test]`).
#[test]
fn daily_episodes_are_season_zero() {
    let r = parse_title("A.Late.Talk.Show.2010.10.11.Johnny.Knoxville.iTouch-MW").unwrap();
    assert_eq!(r.season_numbers, vec![0]);
}

#[test]
fn is_possible_special_episode_fixture() {
    Fixture::load("IsPossibleSpecialEpisodeFixture").run(|method, a| {
        outcome((|| -> R {
            let r = parsed(s(&a[0]))?;
            match method {
                "IsPossibleSpecialEpisode_should_be_true"
                | "IsPossibleSpecialEpisode_should_be_true_if_e00_special" => {
                    check(r.is_possible_special_episode(), "not a possible special")
                }
                "IsPossibleSpecialEpisode_should_be_false_for_Special_in_title" => {
                    check(!r.is_possible_special_episode(), "possible special")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

/// The model-only `[Test]`s of `IsPossibleSpecialEpisodeFixture` and `ParsedEpisodeInfoFixture`.
#[test]
fn parsed_episode_info_properties() {
    let info = |series: &str, seasons: Vec<i32>, episodes: Vec<i32>| ParsedEpisodeInfo {
        series_title: series.into(),
        season_numbers: seasons,
        episode_numbers: episodes,
        ..Default::default()
    };
    assert!(!info("", vec![1], vec![7]).is_possible_special_episode());
    assert!(info("", vec![1], vec![]).is_possible_special_episode());
    assert!(info("", vec![2], vec![0]).is_possible_scene_season_special());
    assert!(!info("", vec![0], vec![0]).is_possible_scene_season_special());
    assert!(!info("", vec![], vec![0]).is_possible_scene_season_special());
    assert!(!info("", vec![], vec![7]).is_possible_special_episode());
    assert!(info("", vec![0], vec![7]).is_possible_special_episode());
    assert!(
        info("Series Title", vec![2], vec![5, 6])
            .to_string()
            .contains("S02E05-06")
    );
}

#[test]
fn mini_series_episode_parser_fixture() {
    Fixture::load("MiniSeriesEpisodeParserFixture").run(|method, a| {
        outcome((|| -> R {
            match method {
                "should_parse_mini_series_episode" | "should_parse_korean_series_episode" => {
                    let r = parsed(s(&a[0]))?;
                    expect_eq("episode count", r.episode_numbers.len(), 1)?;
                    seasons_are(&r, &[1])?;
                    expect_eq("episode", r.episode_numbers[0], i(&a[2]) as i32)?;
                    title_is(&r, &a[1])?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    check(!r.full_season, "full season")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn multi_episode_parser_fixture() {
    Fixture::load("MultiEpisodeParserFixture").run(|method, a| {
        outcome((|| -> R {
            match method {
                "should_parse_multiple_episodes" => {
                    let r = parsed(s(&a[0]))?;
                    seasons_are(&r, &[i(&a[2]) as i32])?;
                    equivalent("episodes", &r.episode_numbers, &ints(&a[3]))?;
                    title_is(&r, &a[1])?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    check(!r.full_season, "full season")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn normalize_episode_title_fixture() {
    Fixture::load("NormalizeEpisodeTitleFixture").run(|method, a| {
        outcome(match method {
            "should_normalize_episode_title" => expect_eq(
                "title",
                normalize_episode_title(s(&a[0])).as_str(),
                s(&a[1]),
            ),
            _ => Err("\u{0}unhandled".into()),
        })
    });
}

#[test]
fn normalize_series_title_fixture() {
    Fixture::load("NormalizeSeriesTitleFixture").run(|method, a| {
        outcome((|| -> R {
            let all = |formats: &[&str], want: &dyn Fn(&str) -> String| -> R {
                let word = s(&a[0]);
                for f in formats {
                    let dirty = f.replace("{0}", word);
                    expect_eq(&dirty, clean_series_title(&dirty), want(word))?;
                }
                Ok(())
            };
            match method {
                "should_normalize_series_title"
                | "should_remove_special_characters_and_casing"
                | "should_not_remove_from_the_beginning_of_the_title"
                | "should_replace_percent_sign_with_percent_following_numbers" => expect_eq(
                    "clean title",
                    clean_series_title(s(&a[0])).as_str(),
                    s(&a[1]),
                ),
                "should_remove_common_words_from_middle_of_title" => all(
                    &["word.{0}.word", "word {0} word", "word-{0}-word"],
                    &|_| "wordword".into(),
                ),
                "should_not_remove_common_words_from_end_of_title" => {
                    all(&["word.word.{0}", "word-word-{0}", "word-word {0}"], &|w| {
                        format!("wordword{}", w.to_lowercase())
                    })
                }
                "should_not_remove_common_words_in_the_middle_of_word" => all(
                    &[
                        "word.{0}word",
                        "word {0}word",
                        "word-{0}word",
                        "word{0}.word",
                        "word{0}-word",
                        "word{0}-word",
                    ],
                    &|w| format!("word{}word", w.to_lowercase()),
                ),
                "should_not_clean_word_from_beginning_of_string" => {
                    all(&["{0}.word.word", "{0}-word-word", "{0} word word"], &|w| {
                        format!("{w}wordword")
                    })
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

/// The plain `[Test]`s of `NormalizeSeriesTitleFixture` and `ParserFixture`.
#[test]
fn clean_series_title_special_cases() {
    for f in ["word.{0}.word", "word {0} word", "word-{0}-word"] {
        assert_eq!(clean_series_title(&f.replace("{0}", "a")), "wordword");
    }
    assert_eq!(clean_series_title("Series Title A"), "seriestitlea");
    assert_eq!(clean_series_title("Seri\u{e0}es"), "seriaes");
}

#[test]
fn parsed_episode_info_fixture() {
    Fixture::load("ParsedEpisodeInfoFixture").run(|method, a| {
        outcome((|| -> R {
            match method {
                "should_format_full_season" => {
                    let info = ParsedEpisodeInfo {
                        series_title: "Series Title".into(),
                        season_numbers: ints(&a[0]),
                        full_season: true,
                        ..Default::default()
                    };
                    let text = info.to_string();
                    check(text.contains(s(&a[1])), &format!("{text:?} lacks {}", a[1]))
                }
                "should_get_release_type" => {
                    let want = match enum_name(&a[1], "ReleaseType") {
                        "Unknown" => ReleaseType::Unknown,
                        "SingleEpisode" => ReleaseType::SingleEpisode,
                        "MultiEpisode" => ReleaseType::MultiEpisode,
                        "SeasonPack" => ReleaseType::SeasonPack,
                        "MultiSeasonPack" => ReleaseType::MultiSeasonPack,
                        other => return Err(format!("unknown release type {other}")),
                    };
                    expect_eq("release type", parsed(s(&a[0]))?.release_type(), want)
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn parser_fixture() {
    Fixture::load("ParserFixture").run(|method, a| {
        outcome((|| -> R {
            match method {
                "should_parse_series_name" => expect_eq(
                    "series name",
                    clean_series_title(&parse_series_name(s(&a[0]))),
                    clean_series_title(s(&a[1])),
                ),
                "should_parse_series_title_info" => {
                    let info = parsed(s(&a[0]))?.series_title_info;
                    expect_eq(
                        "title without year",
                        info.title_without_year.as_str(),
                        s(&a[1]),
                    )?;
                    expect_eq("year", i64::from(info.year), a.get(2).map_or(0, i))
                }
                "should_clean_up_invalid_path_characters" => {
                    let _ = parse_title(s(&a[0]));
                    Ok(())
                }
                "should_remove_request_info_from_title" => expect_eq(
                    "series title",
                    parsed(s(&a[0]))?.series_title.as_str(),
                    s(&a[1]),
                ),
                "should_parse_quality_from_extension" => {
                    let q = parsed(s(&a[0]))?.quality;
                    check(q.quality.id != 0, "unknown quality")?;
                    expect_eq(
                        "source detection",
                        q.source_detection,
                        DetectionSource::Extension,
                    )?;
                    expect_eq(
                        "resolution detection",
                        q.resolution_detection,
                        DetectionSource::Extension,
                    )
                }
                "should_parse_releasetitle" => expect_eq(
                    "release title",
                    parsed(s(&a[0]))?.release_title.as_str(),
                    s(&a[1]),
                ),
                "should_parse_multiple_series_titles" => {
                    let info = parsed(s(&a[0]))?.series_title_info;
                    let want: Vec<String> = a[1..].iter().map(|t| s(t).to_string()).collect();
                    equivalent("titles", &info.all_titles.unwrap_or_default(), &want)
                }
                "should_parse_language_after_parsing_title" => {
                    let r = parsed(s(&a[0]))?;
                    languages_exactly(&r, &[s(&a[1])])
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn path_parser_fixture() {
    Fixture::load("PathParserFixture").run(|method, a| {
        outcome((|| -> R {
            let path = common::as_os_agnostic(s(&a[0]));
            let r = parsed_path(&path)?;
            match method {
                "should_parse_from_path" => {
                    expect_eq("episode count", r.episode_numbers.len(), 1)?;
                    seasons_are(&r, &[i(&a[1]) as i32])?;
                    expect_eq("episode", r.episode_numbers[0], i(&a[2]) as i32)?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    check(!r.full_season, "full season")
                }
                "should_parse_multi_episode_from_path" => {
                    title_is(&r, &a[1])?;
                    let episodes = ints(&a[3]);
                    expect_eq("episode count", r.episode_numbers.len(), episodes.len())?;
                    seasons_are(&r, &[i(&a[2]) as i32])?;
                    equivalent("episodes", &r.episode_numbers, &episodes)?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    check(!r.full_season, "full season")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

/// `should_not_include_extension_in_release_group` (a plain `[Test]` in the release group fixture).
#[test]
fn path_release_group_excludes_extension() {
    let r =
        parse_path("/C/Test/Doctor.Series.2005.s01e01.internal.bdrip.x264-archivist.mkv").unwrap();
    assert_eq!(r.release_group.as_deref(), Some("archivist"));
}

#[test]
fn hashed_release_fixture() {
    Fixture::load("HashedReleaseFixture").run(|method, a| {
        outcome((|| -> R {
            match method {
                "should_properly_parse_hashed_releases" => {
                    let r = parsed_path(&path_arg(&a[0]))?;
                    title_is(&r, &a[1])?;
                    expect_eq(
                        "quality",
                        r.quality.quality.name,
                        quality_name(enum_name(&a[2], "Quality")),
                    )?;
                    expect_eq("release group", r.release_group.as_deref(), a[3].as_str())
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn scene_checker_fixture() {
    Fixture::load("SceneCheckerFixture").run(|method, a| {
        outcome(match method {
            "should_return_true_for_scene_names" => check(is_scene_title(s(&a[0])), "not scene"),
            "should_return_false_for_non_scene_names" => check(!is_scene_title(s(&a[0])), "scene"),
            _ => Err("\u{0}unhandled".into()),
        })
    });
}

#[test]
fn season_numbers_formatter_fixture() {
    Fixture::load("SeasonNumbersFormatterFixture").run(|method, a| {
        outcome(match method {
            "should_format_season_numbers" => expect_eq(
                "format",
                format_season_numbers(&ints(&a[0]), false).as_str(),
                s(&a[1]),
            ),
            "should_format_season_numbers_with_number_format" => expect_eq(
                "format",
                format_season_numbers(&ints(&a[0]), true).as_str(),
                s(&a[1]),
            ),
            _ => Err("\u{0}unhandled".into()),
        })
    });
}

#[test]
fn season_parser_fixture() {
    Fixture::load("SeasonParserFixture").run(|method, a| {
        outcome((|| -> R {
            let r = parsed(s(&a[0]))?;
            let pack = |r: &ParsedEpisodeInfo| -> R {
                title_is(r, &a[1])?;
                check(r.episode_numbers.is_empty(), "episodes present")?;
                check(
                    r.absolute_episode_numbers.is_empty(),
                    "absolute numbers present",
                )
            };
            match method {
                "should_parse_full_season_release" => {
                    seasons_are(&r, &[i(&a[2]) as i32])?;
                    pack(&r)?;
                    check(r.full_season, "not a full season")
                }
                "should_parse_season_extras" | "should_parse_season_subpack" => {
                    seasons_are(&r, &[i(&a[2]) as i32])?;
                    pack(&r)?;
                    check(r.full_season, "not a full season")?;
                    check(r.is_season_extra, "not an extra")
                }
                "should_parse_partial_season_release" => {
                    seasons_are(&r, &[i(&a[2]) as i32])?;
                    pack(&r)?;
                    check(!r.full_season, "full season")?;
                    check(r.is_partial_season, "not partial")?;
                    expect_eq("season part", i64::from(r.season_part), i(&a[3]))
                }
                "should_parse_multi_season_release" => {
                    pack(&r)?;
                    check(r.full_season, "not a full season")?;
                    check(!r.is_partial_season, "partial")?;
                    check(r.is_multi_season(), "not multi-season")?;
                    seasons_are(&r, &ints(&a[2]))
                }
                "should_not_be_multi_season_for_a_single_season_release" => {
                    check(!r.is_multi_season(), "multi-season")?;
                    seasons_are(&r, &[i(&a[1]) as i32])
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

/// `should_not_parse_season_folders` (a plain `[Test]`).
#[test]
fn season_folders_are_not_releases() {
    assert!(parse_title("Season 3").is_none());
}

#[test]
fn season_title_parser_fixture() {
    Fixture::load("SeasonTitleParserFixture").run(|method, a| {
        outcome((|| -> R {
            let title = s(&a[0]);
            match method {
                "should_return_the_title_when_there_is_no_season_or_episode" => {
                    let r = parse_season_title(title).ok_or("not parsed")?;
                    title_is(&r, &a[1])?;
                    check(r.full_season && r.is_season_title, "not a season title")?;
                    check(r.season_numbers.is_empty(), "seasons present")?;
                    check(r.episode_numbers.is_empty(), "episodes present")?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )
                }
                "should_populate_the_fields_the_decision_engine_reads" => {
                    let r = parse_season_title(title).ok_or("not parsed")?;
                    check(!r.languages.is_empty(), "no languages")
                }
                "should_not_return_a_title_when_stripping_leaves_metadata"
                | "should_not_return_a_title_for_an_extra" => {
                    check(parse_season_title(title).is_none(), "parsed")
                }
                "should_not_flag_a_release_the_regexes_can_parse" => {
                    check(!parsed(title)?.is_season_title, "flagged as season title")
                }
                "should_leave_parse_title_rejecting_what_it_rejects_today" => {
                    check(parse_title(title).is_none(), "parsed")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn single_episode_parser_fixture() {
    Fixture::load("SingleEpisodeParserFixture").run(|method, a| {
        outcome((|| -> R {
            let r = parsed(s(&a[0]))?;
            let base = |r: &ParsedEpisodeInfo| -> R {
                expect_eq("episode count", r.episode_numbers.len(), 1)?;
                seasons_are(r, &[i(&a[2]) as i32])?;
                expect_eq("episode", r.episode_numbers[0], i(&a[3]) as i32)?;
                title_is(r, &a[1])?;
                check(
                    r.absolute_episode_numbers.is_empty(),
                    "absolute numbers present",
                )?;
                check(!r.full_season, "full season")
            };
            match method {
                "should_parse_single_episode" | "should_parse_japanese_variety_show_format" => {
                    base(&r)
                }
                "should_parse_decimal_number_as_special" => {
                    base(&r)?;
                    check(r.special, "not special")
                }
                "should_parse_split_episode" => {
                    base(&r)?;
                    check(r.is_split_episode, "not split")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn sub_group_parser_fixture() {
    Fixture::load("SubGroupParserFixture").run(|method, a| {
        outcome((|| -> R {
            match method {
                "should_parse_sub_group_from_title_as_release_group" => expect_eq(
                    "release group",
                    parsed(s(&a[0]))?.release_group.as_deref(),
                    a[1].as_str(),
                ),
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn unicode_release_parser_fixture() {
    Fixture::load("UnicodeReleaseParserFixture").run(|method, a| {
        outcome((|| -> R {
            let anime = |title: String, series: &Value, group: &Value, absolute: &Value| -> R {
                let r = parsed(&title)?;
                expect_eq("release group", r.release_group.as_deref(), group.as_str())?;
                expect_eq(
                    "absolute",
                    single("absolute", &r.absolute_episode_numbers)?,
                    i(absolute) as i32,
                )?;
                title_is(&r, series)?;
                check(!r.full_season, "full season")
            };
            match method {
                "should_parse_chinese_anime_releases"
                | "should_parse_chinese_anime_season_episode_releases"
                | "should_parse_unbracketed_chinese_anime_releases"
                | "should_parse_false_positive_chinese_anime_releases" => {
                    anime(replace_unicode(s(&a[0])), &a[1], &a[2], &a[3])
                }
                "should_parse_chinese_multiepisode_releases" => {
                    let r = parsed(&replace_unicode(s(&a[0])))?;
                    expect_eq("release group", r.release_group.as_deref(), a[2].as_str())?;
                    equivalent("absolute", &r.absolute_episode_numbers, &ints(&a[3]))?;
                    title_is(&r, &a[1])?;
                    check(!r.full_season, "full season")
                }
                "should_parse_gm_team_releases_and_files" => {
                    let r = parsed(s(&a[0]))?;
                    expect_eq(
                        "absolute",
                        single("absolute", &r.absolute_episode_numbers)?,
                        i(&a[2]) as i32,
                    )?;
                    title_is(&r, &a[1])?;
                    check(!r.full_season, "full season")?;
                    expect_eq("release group", r.release_group.as_deref(), Some("GM-Team"))
                }
                "should_parse_unicode_digits" => {
                    let r = parsed(s(&a[0]))?;
                    title_is(&r, &a[1])?;
                    equivalent("absolute", &r.absolute_episode_numbers, &[i(&a[2]) as i32])?;
                    check(
                        r.special_absolute_episode_numbers.is_empty(),
                        "special absolute present",
                    )?;
                    check(!r.full_season, "full season")
                }
                "should_parse_full_season_releases" => {
                    let r = parsed(&replace_unicode(s(&a[0])))?;
                    expect_eq("release group", r.release_group.as_deref(), a[2].as_str())?;
                    check(r.episode_numbers.is_empty(), "episodes present")?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    title_is(&r, &a[1])?;
                    check(r.full_season, "not a full season")?;
                    seasons_are(&r, &[i(&a[3]) as i32])
                }
                "should_parse_multi_episode_release" => {
                    let r = parsed(&replace_unicode(s(&a[0])))?;
                    seasons_are(&r, &[i(&a[3]) as i32])?;
                    equivalent("episodes", &r.episode_numbers, &ints(&a[4]))?;
                    title_is(&r, &a[1])?;
                    check(
                        r.absolute_episode_numbers.is_empty(),
                        "absolute numbers present",
                    )?;
                    check(!r.full_season, "full season")
                }
                _ => Err("\u{0}unhandled".into()),
            }
        })())
    });
}

#[test]
fn url_fixture() {
    Fixture::load("UrlFixture").run(|method, a| {
        outcome(match method {
            "should_not_parse_url_in_name" => expect_eq(
                "series name",
                clean_series_title(&parse_series_name(s(&a[0]))),
                clean_series_title(s(&a[1])),
            ),
            "should_not_parse_url_in_group" => expect_eq(
                "release group",
                parse_release_group(s(&a[0])).as_deref(),
                a[1].as_str(),
            ),
            _ => Err("\u{0}unhandled".into()),
        })
    });
}
