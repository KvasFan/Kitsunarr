//! Episode parsing (`Parser/Parser.cs`): series title, seasons, episodes, absolute and daily
//! numbering, packs, specials, and the title normalisations used for matching.
//!
//! The patterns are Sonarr's, verbatim (`title_patterns.rs`, generated), run on `netregex` for
//! .NET semantics. The control flow below follows `Parser.cs` statement by statement; where
//! upstream relies on an exception, the port returns an error that ends parsing the same way.

use std::sync::LazyLock;

use chrono::{Local, NaiveDate, TimeDelta};
use unicode_normalization::UnicodeNormalization;

use crate::common::{
    CLEAN_TORRENT_SUFFIX, PATH_PRE_SUBSTITUTION, PRE_SUBSTITUTION, WEBSITE_POSTFIX, WEBSITE_PREFIX,
};
use crate::extensions::{path_extension, remove_file_extension};
use crate::language::parse_languages;
use crate::model::{ParsedEpisodeInfo, SeriesTitleInfo};
use crate::netregex::{NetMatch, NetRegex, digit_value, is_letter_or_digit, simple_lower};
use crate::quality::{parse_quality, parse_quality_name};
use crate::re::contains_ci;
use crate::release_group::parse_release_group;
use crate::title_patterns::{self as tp, Pattern};

fn compile(p: &Pattern) -> NetRegex {
    NetRegex::compile(p.pattern, p.ignore_case)
}

fn single(list: &[Pattern]) -> NetRegex {
    compile(&list[0])
}

macro_rules! patterns {
    ($($name:ident = $field:ident;)*) => {
        $(static $name: LazyLock<NetRegex> = LazyLock::new(|| single(tp::$field));)*
    };
}

static REPORT_TITLE: LazyLock<Vec<NetRegex>> =
    LazyLock::new(|| tp::REPORT_TITLE_REGEX.iter().map(compile).collect());
static SPECIAL_EPISODE_TITLE: LazyLock<Vec<NetRegex>> = LazyLock::new(|| {
    tp::SPECIAL_EPISODE_TITLE_REGEX
        .iter()
        .map(compile)
        .collect()
});
static REJECT_HASHED: LazyLock<Vec<NetRegex>> = LazyLock::new(|| {
    tp::REJECT_HASHED_RELEASES_REGEXES
        .iter()
        .map(compile)
        .collect()
});
static SEASON_FOLDERS: LazyLock<Vec<NetRegex>> =
    LazyLock::new(|| tp::SEASON_FOLDER_REGEXES.iter().map(compile).collect());

patterns! {
    REVERSED_TITLE = REVERSED_TITLE_REGEX;
    ANIME_SEASON_RELEASE_METADATA = ANIME_SEASON_RELEASE_METADATA_REGEX;
    ANIME_SPECIAL_MARKER = ANIME_SPECIAL_MARKER_REGEX;
    NORMALIZE = NORMALIZE_REGEX;
    PERCENT = PERCENT_REGEX;
    SIMPLE_TITLE = SIMPLE_TITLE_REGEX;
    SIX_DIGIT_AIR_DATE = SIX_DIGIT_AIR_DATE_REGEX;
    CLEAN_QUALITY_BRACKETS = CLEAN_QUALITY_BRACKETS_REGEX;
    YEAR_IN_TITLE = YEAR_IN_TITLE_REGEX;
    TITLE_COMPONENTS = TITLE_COMPONENTS_REGEX;
    PART = PART_REGEX;
    PUNCTUATION = PUNCTUATION_REGEX;
    ARTICLE_WORD = ARTICLE_WORD_REGEX;
    DUPLICATE_SPACES = DUPLICATE_SPACES_REGEX;
    SEASON_FOLDER = SEASON_FOLDER_REGEX;
    SIMPLE_EPISODE_NUMBER = SIMPLE_EPISODE_NUMBER_REGEX;
    REQUEST_INFO = REQUEST_INFO_REGEX;
    MULTI = MULTI_REGEX;
}

static IMDB: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"^(\d{1,10}|(tt)\d{1,10})$", false));

const NUMBERS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

const SHORT_MONTHS: &[(&str, u32)] = &[
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

/// Why parsing stopped (upstream: an exception caught by `ParseTitle`).
#[derive(Debug)]
struct Abort(String);

type Parsed<T> = Result<T, Abort>;

fn lower(s: &str) -> String {
    s.chars().map(simple_lower).collect()
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `Convert.ToInt32(string)`.
fn to_int32(s: &str) -> Parsed<i32> {
    s.trim()
        .parse()
        .map_err(|_| Abort(format!("{s:?} isn't an integer")))
}

fn convert_to_numerals(s: &str) -> String {
    s.chars()
        .map(|c| match digit_value(c) {
            Some(v) => char::from_digit(v, 10).unwrap_or(c),
            None => c,
        })
        .collect()
}

/// `ParseNumber`: digits in any script, or a number word (`one` … `nine`).
fn parse_number(value: &str) -> Parsed<i32> {
    let normalized = convert_to_numerals(&value.nfkc().collect::<String>());
    if let Ok(n) = normalized.trim().parse::<i32>() {
        return Ok(n);
    }
    let lowered = lower(value);
    NUMBERS
        .iter()
        .position(|n| *n == lowered)
        .map(|i| i as i32)
        .ok_or_else(|| Abort(format!("{value} isn't a number")))
}

/// `ParseDecimal`.
fn parse_decimal(value: &str) -> Parsed<f64> {
    let normalized = convert_to_numerals(&value.nfkc().collect::<String>());
    normalized
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Abort(format!("{value} isn't a number")))
}

fn validate_before_parsing(title: &str) -> bool {
    let lowered = lower(title);
    if lowered.contains("password") && lowered.contains("yenc") {
        return false;
    }
    if !title.chars().any(is_letter_or_digit) {
        return false;
    }
    let without_extension = remove_file_extension(title);
    if REJECT_HASHED.iter().any(|r| r.is_match(&without_extension)) {
        tracing::debug!(title, "rejected hashed release title");
        return false;
    }
    if SEASON_FOLDERS
        .iter()
        .any(|r| r.is_match(&without_extension))
    {
        tracing::debug!(title, "rejected season folder release title");
        return false;
    }
    true
}

/// Releases whose name was posted reversed (`p027.TVDH.50E10S.eltiT`).
fn unreverse(title: &str) -> String {
    if !REVERSED_TITLE.is_match(title) {
        return title.to_string();
    }
    let without: Vec<char> = remove_file_extension(title).chars().collect();
    let rest: String = title.chars().skip(without.len()).collect();
    let reversed: String = without.iter().rev().collect();
    tracing::debug!(title, "reversed name detected");
    reversed + &rest
}

/// `Parser.ParseTitle`: everything a release or file name says about its episodes.
pub fn parse_title(title: &str) -> Option<ParsedEpisodeInfo> {
    if !validate_before_parsing(title) {
        return None;
    }
    let title = unreverse(title);
    let mut release_title = remove_file_extension(&title)
        .replace('【', "[")
        .replace('】', "]");
    for replace in PRE_SUBSTITUTION.iter() {
        replace.try_replace(&mut release_title);
    }

    let mut simple = SIMPLE_TITLE.replace(&release_title, "");
    simple = WEBSITE_PREFIX.replace(&simple);
    simple = WEBSITE_POSTFIX.replace(&simple);
    simple = CLEAN_TORRENT_SUFFIX.replace(&simple);
    simple = CLEAN_QUALITY_BRACKETS.replace_with(&simple, |m| {
        let value = m.value_all();
        if parse_quality_name(&value).quality.id != 0 {
            String::new()
        } else {
            value
        }
    });

    if let Some(m) = SIX_DIGIT_AIR_DATE.find(&simple) {
        let (year, month, day) = (m.value("airyear"), m.value("airmonth"), m.value("airday"));
        if month != "00" || day != "00" {
            let fixed = format!("20{year}.{month}.{day}");
            let airdate = m.value("airdate");
            simple = simple.replace(&airdate, &fixed);
        }
    }

    for (regex, pattern) in REPORT_TITLE.iter().zip(tp::REPORT_TITLE_REGEX) {
        let matches = regex.find_all(&simple);
        if matches.is_empty() {
            continue;
        }
        tracing::trace!(pattern = pattern.comment, "release title pattern matched");
        match parse_match_collection(&matches, &release_title) {
            Err(Abort(reason)) => {
                tracing::debug!(title = %title, reason, "unable to parse");
                return None;
            }
            Ok(None) => continue,
            Ok(Some(mut result)) => {
                if result.full_season && contains_ci(&result.release_tokens, "Special") {
                    result.full_season = false;
                    result.special = true;
                }
                result.languages = parse_languages(&result.release_tokens);
                result.quality = parse_quality(&title);
                result.release_group = parse_release_group(&release_title);
                if let Some(subgroup) = matches[0].get("subgroup").filter(|s| !s.trim().is_empty())
                {
                    result.release_group = Some(subgroup);
                }
                result.release_hash = release_hash(&matches[0]);
                return Some(result);
            }
        }
    }
    tracing::debug!(title = %title, "unable to parse");
    None
}

fn release_hash(m: &NetMatch<'_>) -> String {
    match m.get("hash") {
        Some(hash) => {
            let value = hash.trim_matches(['[', ']', '(', ')']);
            if value == "1280x720" {
                String::new()
            } else {
                value.to_string()
            }
        }
        None => String::new(),
    }
}

fn parse_match_collection(
    matches: &[NetMatch<'_>],
    release_title: &str,
) -> Parsed<Option<ParsedEpisodeInfo>> {
    let first_match = &matches[0];
    let series_name = first_match.value("title").replace(['.', '_'], " ");
    let series_name = REQUEST_INFO
        .replace(&series_name, "")
        .trim_matches(' ')
        .to_string();
    let air_year: i32 = first_match.value("airyear").trim().parse().unwrap_or(0);
    let mut last_index = first_match.end_index("title");

    let mut result;
    if air_year < 1900 {
        result = ParsedEpisodeInfo {
            release_title: release_title.to_string(),
            season_numbers: vec![0],
            ..Default::default()
        };
        for m in matches {
            let episodes = m.captures("episode");
            let absolutes = m.captures("absoluteepisode");

            // Allows a list of 0 episodes (handled as a full season release).
            if let (Some(first), Some(last)) = (episodes.first(), episodes.last()) {
                let first_number = parse_number(&first.value)?;
                let last_number = parse_number(&last.value)?;
                if first_number > last_number {
                    return Ok(None);
                }
                result.episode_numbers = (first_number..=last_number).collect();
                last_index = last_index.max(last.end);
                if m.success("special") {
                    result.special = true;
                }
                if m.success("splitepisode") {
                    result.is_split_episode = true;
                }
            }

            if let (Some(first), Some(last)) = (absolutes.first(), absolutes.last()) {
                let first_number = parse_decimal(&first.value)?;
                let last_number = parse_decimal(&last.value)?;
                if first_number > last_number {
                    return Ok(None);
                }
                if first_number.fract() != 0.0 || last_number.fract() != 0.0 {
                    if absolutes.len() != 1 {
                        // Multiple matches are not allowed for specials.
                        return Ok(None);
                    }
                    result.special_absolute_episode_numbers = vec![first_number];
                    result.special = true;
                    last_index = last_index.max(first.end);
                } else {
                    // `Enumerable.Range((int)first, (int)count)`
                    let start = first_number as i32;
                    let count = (last_number - first_number + 1.0) as i32;
                    result.absolute_episode_numbers =
                        (start..start.saturating_add(count)).collect();
                    if m.success("special") {
                        result.special = true;
                    }
                    last_index = last_index.max(last.end);
                }
            }

            if episodes.is_empty() && absolutes.is_empty() {
                // "Extras" and "SUBPACK" releases are flagged so they can be filtered out.
                if !first_match.value("extras").trim().is_empty() {
                    result.is_season_extra = true;
                }
                // Partial season packs carry a season part.
                let season_part = first_match.value("seasonpart");
                if !season_part.trim().is_empty() {
                    result.season_part = to_int32(&season_part)?;
                    result.is_partial_season = true;
                } else if first_match.success("special") {
                    result.special = true;
                } else {
                    result.full_season = true;
                }
            }

            if episodes.len() == 2
                && first_match.success("episodecount")
                && episodes.last().map(|c| c.value.as_str())
                    == Some(first_match.value("episodecount").as_str())
            {
                result.episode_numbers = Vec::new();
                result.full_season = true;
            }
        }

        let mut seasons = Vec::new();
        for capture in first_match.captures("season") {
            if let Ok(season) = capture.value.parse::<i32>() {
                seasons.push(season);
                last_index = last_index.max(capture.end);
            }
        }
        // Tanukarr: two seasons are a range only with a dash separator ("S01-S04") or
        // "Complete Series"; "S01 S04" names seasons 1 and 4.
        let range = seasons.len() == 2
            && seasons[0] <= seasons[1]
            && (first_match.value("seasonseparator").contains('-')
                || first_match.success("completeseries"));
        seasons.sort_unstable();
        seasons.dedup();
        match seasons.len() {
            2 if range => result.season_numbers = (seasons[0]..=seasons[1]).collect(),
            0 => {
                if result.absolute_episode_numbers.is_empty() && !result.episode_numbers.is_empty()
                {
                    // No season and not absolute: a mini-series, season 1.
                    result.season_numbers = vec![1];
                    result.is_mini_series = true;
                }
            }
            _ => result.season_numbers = seasons,
        }
    } else {
        // A daily show.
        let (mut month, mut day);
        if first_match.success("ambiguousairmonth") && first_match.success("ambiguousairday") {
            let ambiguous_month = to_int32(&first_match.value("ambiguousairmonth"))?;
            let ambiguous_day = to_int32(&first_match.value("ambiguousairday"))?;
            if ambiguous_day <= 12 && ambiguous_month <= 12 {
                return Err(Abort(format!(
                    "ambiguous date, cannot validate month and day with {ambiguous_month} and {ambiguous_day}"
                )));
            }
            month = ambiguous_month;
            day = ambiguous_day;
        } else if first_match.success("shortairmonth") {
            let short = lower(&first_match.value("shortairmonth"));
            let Some((_, m)) = SHORT_MONTHS.iter().find(|(name, _)| *name == short) else {
                return Err(Abort(format!("unable to determine air month from {short}")));
            };
            month = *m as i32;
            day = to_int32(&first_match.value("airday"))?;
        } else {
            month = to_int32(&first_match.value("airmonth"))?;
            day = to_int32(&first_match.value("airday"))?;
        }
        // Swap day and month if the month is bigger than 12 (scene fail).
        if month > 12 {
            std::mem::swap(&mut month, &mut day);
        }
        let air_date = u32::try_from(month)
            .ok()
            .zip(u32::try_from(day).ok())
            .and_then(|(m, d)| NaiveDate::from_ymd_opt(air_year, m, d))
            .ok_or_else(|| Abort(format!("invalid date {air_year}-{month}-{day}")))?;
        // Early releases are fine; dates too far in the future are most likely parse errors.
        if air_date > Local::now().date_naive() + TimeDelta::days(14) {
            return Err(Abort(format!("invalid date {air_date}")));
        }
        // Before 1970 only with a title year (the Plex DVR format).
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
        if air_date < epoch && first_match.value("titleyear").trim().is_empty() {
            return Err(Abort(format!("invalid date {air_date}")));
        }
        for group in ["airyear", "airmonth", "airday"] {
            last_index = last_index.max(first_match.end_index(group));
        }
        result = ParsedEpisodeInfo {
            release_title: release_title.to_string(),
            season_numbers: vec![0],
            air_date: Some(air_date.format("%Y-%m-%d").to_string()),
            ..Default::default()
        };
        if first_match.success("part") {
            result.daily_part = Some(to_int32(&first_match.value("part"))?);
        }
    }

    // Upstream indexes the release title with positions found in the simplified title.
    result.release_tokens = if last_index < char_len(release_title) {
        release_title.chars().skip(last_index).collect()
    } else {
        release_title.to_string()
    };
    result.series_title_info = series_title_info(&series_name, first_match)?;
    result.series_title = series_name;
    Ok(Some(result))
}

fn series_title_info(title: &str, first_match: &NetMatch<'_>) -> Parsed<SeriesTitleInfo> {
    let mut info = SeriesTitleInfo {
        title: title.to_string(),
        title_without_year: title.to_string(),
        year: 0,
        all_titles: None,
    };
    if let Some(m) = YEAR_IN_TITLE.find(title) {
        info.title_without_year = m.value("title");
        info.year = to_int32(&m.value("year"))?;
    }
    if let Some(components) = TITLE_COMPONENTS.find(&info.title_without_year) {
        info.all_titles = Some(
            components
                .captures("title")
                .into_iter()
                .map(|c| c.value)
                .collect(),
        );
    } else {
        let titles = first_match.captures("title");
        if titles.len() > 1 {
            info.all_titles = Some(
                titles
                    .into_iter()
                    .map(|c| c.value.replace(['.', '_'], " "))
                    .collect(),
            );
        }
    }
    Ok(info)
}

/// `Parser.ParsePath`: parses a file path, falling back to the folder names.
pub fn parse_path(path: &str) -> Option<ParsedEpisodeInfo> {
    let (directory, file_name) = match path.rfind('/') {
        Some(i) => {
            let dir = &path[..i];
            let dir_name = match dir.rfind('/') {
                Some(j) => &dir[j + 1..],
                None => dir,
            };
            (
                Some(if dir_name.is_empty() { "/" } else { dir_name }),
                &path[i + 1..],
            )
        }
        None => (None, path),
    };
    let mut file_name = file_name.to_string();
    for replace in PATH_PRE_SUBSTITUTION.iter() {
        replace.try_replace(&mut file_name);
    }

    let result = parse_title(&file_name);

    // Parse the folder and file separately, combining them when both parse.
    if let (Some(episode_match), Some(directory)) =
        (SIMPLE_EPISODE_NUMBER.find(&file_name), directory)
    {
        let use_folder = result
            .as_ref()
            .is_none_or(|r| r.is_mini_series || !r.absolute_episode_numbers.is_empty());
        if use_folder
            && let Some(season_match) = SEASON_FOLDER.find(directory)
            && season_match.success("season")
        {
            let episodes = episode_match.captures("episode");
            let first = parse_number(&episodes.first()?.value).ok()?;
            let last = parse_number(&episodes.last()?.value).ok()?;
            let mut path_title = format!("S{}E{first:02}", season_match.value("season"));
            if first != last {
                path_title.push_str(&format!("-E{last:02}"));
            }
            if let Some(remaining) = episode_match.get("remaining") {
                path_title.push(' ');
                path_title.push_str(&remaining);
            }
            return parse_title(&path_title);
        }
    }

    let mut result = result;
    let stem = file_name_without_extension(&file_name);
    if result.is_none()
        && let Ok(number) = stem.trim().parse::<i32>()
        && let Some(directory) = directory
    {
        result = parse_title(directory).and_then(|mut r| {
            if r.absolute_episode_numbers.contains(&number) {
                r.absolute_episode_numbers = vec![number];
                Some(r)
            } else if r.episode_numbers.contains(&number) {
                r.episode_numbers = vec![number];
                Some(r)
            } else {
                None
            }
        });
    }
    if let Some(directory) = directory {
        if result.is_none() {
            result = parse_title(&format!("{directory} {file_name}"));
        }
        if result.is_none() {
            result = parse_title(&format!("{directory}{}", path_extension(&file_name)));
        }
    }
    result
}

fn file_name_without_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) => &name[..i],
        None => name,
    }
}

/// `Parser.SimplifyTitle`.
pub fn simplify_title(title: &str) -> String {
    if !validate_before_parsing(title) {
        return title.to_string();
    }
    let title = unreverse(title);
    let simple = WEBSITE_PREFIX.replace(&title);
    let simple = WEBSITE_POSTFIX.replace(&simple);
    CLEAN_TORRENT_SUFFIX.replace(&simple)
}

/// `Parser.ParseSeasonFolder`: the season number of a season folder name.
pub fn parse_season_folder(folder_name: &str) -> Option<i32> {
    if let Some(m) = SEASON_FOLDER.find(folder_name)
        && m.success("season")
    {
        return parse_number(&m.value("season")).ok();
    }
    let info = parse_title(folder_name)?;
    (info.full_season && info.season_numbers.len() == 1).then(|| info.season_numbers[0])
}

/// `Parser.ParseSeasonTitle`: an anime season pack named after the season's own title.
pub fn parse_season_title(title: &str) -> Option<ParsedEpisodeInfo> {
    let release_title = remove_file_extension(title);
    if ANIME_SPECIAL_MARKER.is_match(&release_title) {
        return None;
    }
    let series_title = ANIME_SEASON_RELEASE_METADATA.replace(&release_title, " ");
    let series_title = DUPLICATE_SPACES.replace(&series_title, " ");
    let series_title = series_title.trim().to_string();
    if series_title.is_empty() || series_title.contains(['[', ']', '(', ')', '{', '}']) {
        return None;
    }
    Some(ParsedEpisodeInfo {
        series_title_info: SeriesTitleInfo {
            title: series_title.clone(),
            title_without_year: series_title.clone(),
            year: 0,
            all_titles: None,
        },
        series_title,
        full_season: true,
        is_season_title: true,
        languages: parse_languages(&release_title),
        quality: parse_quality(title),
        release_group: parse_release_group(&release_title),
        release_title,
        ..Default::default()
    })
}

/// `Parser.ParseSeriesName`.
pub fn parse_series_name(title: &str) -> String {
    match parse_title(title) {
        Some(r) => r.series_title,
        None => clean_series_title(title),
    }
}

/// `Parser.CleanSeriesTitle`: the form titles are compared in (lower case, no punctuation, no
/// common words, no diacritics).
pub fn clean_series_title(title: &str) -> String {
    if title.trim().is_empty() || title.trim().parse::<i64>().is_ok() {
        return title.to_string();
    }
    // `%` becomes "percent" (the "3%" case).
    let title = PERCENT.replace(title, "percent");
    remove_diacritics(&lower(&NORMALIZE.replace(&title, "")))
}

/// Sonarr's `RemoveDiacritics` (Diacritical.Net plus Sonarr's additions): a per-character table.
pub fn remove_diacritics(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match crate::diacritics::TABLE.binary_search_by_key(&c, |(k, _)| *k) {
            Ok(i) => out.push_str(crate::diacritics::TABLE[i].1),
            Err(_) => out.push(c),
        }
    }
    out
}

/// `Parser.NormalizeEpisodeTitle`.
pub fn normalize_episode_title(title: &str) -> String {
    let mut title = SPECIAL_EPISODE_TITLE
        .iter()
        .find_map(|r| r.find(title))
        .map_or_else(|| title.to_string(), |m| m.value("episodetitle"));
    title = PART.replace(&title, "");
    title = PUNCTUATION.replace(&title, " ");
    title = DUPLICATE_SPACES.replace(&title, " ");
    lower(title.trim())
}

/// `Parser.NormalizeTitle`.
pub fn normalize_title(title: &str) -> String {
    let title = PUNCTUATION.replace(title, "");
    let title = ARTICLE_WORD.replace(&title, "");
    let title = DUPLICATE_SPACES.replace(&title, " ");
    lower(title.trim())
}

/// `Parser.NormalizeImdbId`: `tt` plus at least seven digits, or `None`.
pub fn normalize_imdb_id(imdb_id: &str) -> Option<String> {
    if !IMDB.is_match(imdb_id) || char_len(imdb_id) <= 2 {
        return None;
    }
    Some(format!("tt{:0>7}", imdb_id.replace("tt", "")))
}

/// `Parser.HasMultipleLanguages`.
pub fn has_multiple_languages(title: &str) -> bool {
    MULTI.is_match(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(REPORT_TITLE.len(), 110);
        let _ = (&*SPECIAL_EPISODE_TITLE, &*REJECT_HASHED, &*SEASON_FOLDERS);
        let _ = (
            &*REVERSED_TITLE,
            &*ANIME_SEASON_RELEASE_METADATA,
            &*ANIME_SPECIAL_MARKER,
        );
        let _ = (&*NORMALIZE, &*PERCENT, &*SIMPLE_TITLE, &*SIX_DIGIT_AIR_DATE);
        let _ = (
            &*CLEAN_QUALITY_BRACKETS,
            &*YEAR_IN_TITLE,
            &*TITLE_COMPONENTS,
            &*PART,
        );
        let _ = (
            &*PUNCTUATION,
            &*ARTICLE_WORD,
            &*DUPLICATE_SPACES,
            &*SEASON_FOLDER,
        );
        let _ = (&*SIMPLE_EPISODE_NUMBER, &*REQUEST_INFO, &*MULTI, &*IMDB);
    }

    #[test]
    fn parses_common_release_names() {
        let r = parse_title("Series.Title.S01E05E06.720p.HDTV.x264-DIMENSION").unwrap();
        assert_eq!(r.series_title, "Series Title");
        assert_eq!(
            (r.season_numbers.clone(), r.episode_numbers.clone()),
            (vec![1], vec![5, 6])
        );
        assert_eq!(r.quality.quality.name, "HDTV-720p");
        assert_eq!(r.release_group.as_deref(), Some("DIMENSION"));
        let daily = parse_title("Series Title 2018 04 28 720p HDTV").unwrap();
        assert_eq!(daily.air_date.as_deref(), Some("2018-04-28"));
        let anime = parse_title("[SubsPlease] Series Title - 12 (1080p) [ABCD1234].mkv").unwrap();
        assert_eq!(anime.absolute_episode_numbers, vec![12]);
        assert_eq!(anime.release_group.as_deref(), Some("SubsPlease"));
        assert_eq!(anime.release_hash, "ABCD1234");
    }

    #[test]
    fn cleans_titles_like_sonarr() {
        assert_eq!(clean_series_title("Ríkið"), "rikid");
        assert_eq!(clean_series_title("Parler à"), "parlera");
        assert_eq!(normalize_imdb_id("tt1234").as_deref(), Some("tt0001234"));
    }
}
