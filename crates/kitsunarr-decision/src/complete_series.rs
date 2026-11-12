//! Tanukarr's `CompleteSeriesReleaseParser`: in a complete-series search, a release is read
//! against the searched series only. It must start with one of the series' titles and then
//! name a complete run ("Complete Series", "S01-S05", "Seasons 1-3", an anime absolute range),
//! or, for a single-season show, nothing but quality tokens.
//!
//! Not ported: matching under a collection title (anime groups are not implemented yet).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use kitsunarr_domain::SeriesType;
use kitsunarr_parser::netregex::NetRegex;
use kitsunarr_parser::{
    ParsedEpisodeInfo, parse_languages, parse_quality, parse_release_group, parse_title,
    remove_diacritics,
};

use crate::model::Series;

static SEASON_RANGE: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"\b(?:s|seasons?|series)\s*(?<first>\d{1,3})\s*(?:-|–|—|to)\s*(?:s|seasons?|series)?\s*(?<last>\d{1,3})(?![a-z0-9])",
        true,
    )
});
static SEASON: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"\b(?:s|seasons?|series)\s*(?<season>\d{1,3})(?![a-z0-9])",
        true,
    )
});
static COMPLETE: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"^(?:the\s+)?(?:complete(?:\s+(?:series|collection))?|(?:entire|full)\s+series|all\s+seasons|box\s*set)\b",
        true,
    )
});
static BARE_QUALITY: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"^(?:bd|bdrip|blu-?ray|brrip|bdremux|remux|web(?:-?dl|-?rip)?|hdtv|dvd(?:rip)?|\d{3,4}p|x26[45]|h\s?26[45]|hevc|avc|hi10p?|10-?bit|dual[\s-]?audio|multi[\s-]?audio)\b",
        true,
    )
});
static EPISODE: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"\bs\d+\s*e\d+|\b\d+x\d+\b|\b(?:episodes?|parts?|volumes?|cours?)(?=\b|\d)",
        true,
    )
});
static ABSOLUTE_RANGE: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"^\s*(?<first>\d{1,4})\s*(?:-|~|–|—|to)\s*(?<last>\d{1,4})(?![^\W_])",
        true,
    )
});
static LEADING_GROUP: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"^\[[^\]]+\]\s*", false));
static APOSTROPHES: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"['`’]", false));
static SEPARATORS: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"[._\[\]()]+", false));
static YEAR: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"^(?<year>(?:19|20)\d{2})(?:\s+|$)", false));
static BEGINNING_THE: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"^the\s", true));
static SPECIAL_CHARACTER: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"['.`´‘’]", true));
static NON_WORD: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"[\W]", true));
static PLUSES: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"\+{2,}", false));
static SEASONS_OR_COMPLETE: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"\b(?:seasons?|series|complete)\b|^\d+\s*-\s*\d+", true));
static SEASON_WORD_START: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"^(?:seasons?|series)\s+\d+", true));
static SEASON_WORD: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"\bseason\b", true));

/// `SearchCriteriaBase.GetCleanSceneTitle`.
pub fn clean_scene_title(title: &str) -> String {
    let t = BEGINNING_THE.replace(title, "");
    let t = t.replace('&', "and");
    let t = SPECIAL_CHARACTER.replace(&t, "");
    let t = NON_WORD.replace(&t, "+");
    let t = PLUSES.replace(&t, "+");
    remove_diacritics(&t).trim_matches(['+', ' ']).to_string()
}

fn escape(word: &str) -> String {
    word.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c.to_string()
            } else {
                format!("\\{c}")
            }
        })
        .collect()
}

/// The seasons a complete-series search wants: every regular season of the series.
pub fn expected_seasons(series: &Series) -> Vec<i32> {
    series
        .episodes
        .iter()
        .map(|e| e.season_number)
        .filter(|s| *s > 0)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// `CompleteSeriesReleaseParser.Parse`. `scene_titles` are the series' titles and aliases;
/// `include_season_packs` (interactive searches) also accepts packs of some of its seasons.
pub fn parse(
    title: &str,
    series: &Series,
    scene_titles: &[String],
    include_season_packs: bool,
) -> Option<ParsedEpisodeInfo> {
    let expected = expected_seasons(series);
    if expected.is_empty() {
        return None;
    }
    let cleaned = LEADING_GROUP.replace(title, "");
    let cleaned = APOSTROPHES.replace(&cleaned, "").replace('&', "and");
    let cleaned = remove_diacritics(&cleaned);
    let chars: Vec<char> = cleaned.chars().collect();

    let mut titles: Vec<&String> = scene_titles
        .iter()
        .filter(|t| !t.trim().is_empty())
        .collect();
    titles.sort_by_key(|t| std::cmp::Reverse(t.chars().count()));
    let mut found: Option<(String, String)> = None;
    for alias in titles {
        let clean = clean_scene_title(alias);
        if clean.is_empty() {
            continue;
        }
        let words: Vec<String> = clean.split('+').map(escape).collect();
        let pattern = format!(
            r"^(?:the[\W_]+)?{}(?=$|[\W_])(?<suffix>.*)$",
            words.join(r"[\W_]+")
        );
        let re = NetRegex::compile(&pattern, true);
        if let Some(m) = re.find(&cleaned) {
            let start = m
                .captures("suffix")
                .first()
                .map_or(chars.len(), |c| c.start);
            let matched: String = chars[..start].iter().collect();
            found = Some((m.value("suffix"), matched.trim().to_string()));
            break;
        }
    }
    let (suffix, mut matched_title) = found?;

    let mut description = SEPARATORS.replace(&suffix, " ");
    description = description.trim_matches([' ', '-']).to_string();
    if let Some(m) = YEAR.find(&description) {
        let year: i32 = m.value("year").parse().ok()?;
        if series.year != year {
            return None;
        }
        matched_title.push_str(&format!(" ({year})"));
        let len = m.value_all().chars().count();
        description = description
            .chars()
            .skip(len)
            .collect::<String>()
            .trim_matches([' ', '-'])
            .to_string();
    }
    if EPISODE.is_match(&description) {
        return None;
    }
    let first_word = description.split(' ').next().unwrap_or_default();
    let anime = series.series_type == SeriesType::Anime;
    // A single-season show's only pack, named with nothing but quality tokens.
    let bare_single_season = BARE_QUALITY.is_match(&description)
        && !SEASONS_OR_COMPLETE.is_match(&description)
        && !SEASON.is_match(&description)
        && expected.len() == 1;
    // A longer, different series title must not match a shorter alias prefix.
    if !bare_single_season
        && !COMPLETE.is_match(&description)
        && !SEASON.is_match(first_word)
        && !SEASON_WORD_START.is_match(&description)
        && !(anime && ABSOLUTE_RANGE.is_match(&description))
    {
        return None;
    }
    let after_label = match COMPLETE.find(&description) {
        Some(m) => description
            .chars()
            .skip(m.value_all().chars().count())
            .collect::<String>(),
        None => description.clone(),
    };
    let absolute = if anime {
        ABSOLUTE_RANGE.find(&after_label).map(|m| {
            (
                m.value("first").parse::<i32>().unwrap_or(0),
                m.value("last").parse::<i32>().unwrap_or(0),
            )
        })
    } else {
        None
    };
    if let Some((first, last)) = absolute {
        let regular = series.episodes.iter().filter(|e| e.season_number > 0);
        let mut all_covered = true;
        for e in regular {
            if !e
                .absolute_episode_number
                .is_some_and(|a| a >= first && a <= last)
            {
                all_covered = false;
            }
        }
        if first <= 0 || last < first || !all_covered {
            return None;
        }
    }
    let mut seasons: BTreeSet<i32> = SEASON
        .find_all(&description)
        .iter()
        .filter_map(|m| m.value("season").parse().ok())
        .collect();
    for m in SEASON_RANGE.find_all(&description) {
        let first: i32 = m.value("first").parse().ok()?;
        let last: i32 = m.value("last").parse().ok()?;
        if first > last {
            return None;
        }
        seasons.extend(first..=last);
    }
    // Explicit season coverage takes precedence over a misleading COMPLETE label.
    let covered: Vec<i32> = if !seasons.is_empty() {
        if seasons.iter().any(|s| !expected.contains(s))
            || (!include_season_packs && !expected.iter().all(|s| seasons.contains(s)))
        {
            return None;
        }
        seasons.into_iter().collect()
    } else if absolute.is_some() {
        expected.clone()
    } else if !bare_single_season
        && (!COMPLETE.is_match(&description) || SEASON_WORD.is_match(&description))
    {
        return None;
    } else {
        expected.clone()
    };
    let mut parsed = parse_title(&format!("{matched_title} S{:02}", covered[0]))?;
    parsed.release_title = title.to_string();
    parsed.release_tokens = suffix.clone();
    parsed.quality = parse_quality(title);
    parsed.languages = parse_languages(&suffix);
    parsed.release_group = parse_release_group(title);
    parsed.season_numbers = covered;
    parsed.full_season = true;
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Episode;

    fn series(seasons: &[(i32, i32)], anime: bool) -> Series {
        let mut episodes = Vec::new();
        let mut id = 0;
        let mut absolute = 0;
        for &(season, count) in seasons {
            for n in 1..=count {
                id += 1;
                if season > 0 {
                    absolute += 1;
                }
                episodes.push(Episode {
                    id,
                    season_number: season,
                    episode_number: n,
                    absolute_episode_number: (season > 0).then_some(absolute),
                    scene_season_number: None,
                    scene_episode_number: None,
                    title: None,
                    air_date: None,
                    air_date_utc: None,
                    runtime: 24,
                    monitored: true,
                    file: None,
                });
            }
        }
        let mut s = crate::tests::series();
        s.title = "Show".into();
        s.year = 2015;
        s.series_type = if anime {
            SeriesType::Anime
        } else {
            SeriesType::Standard
        };
        s.episodes = episodes;
        s
    }

    fn seasons(title: &str, s: &Series, packs: bool) -> Option<Vec<i32>> {
        let titles = vec!["The Expanse".to_string(), "Show".to_string()];
        parse(title, s, &titles, packs).map(|p| p.season_numbers)
    }

    /// Cases after Tanukarr's `CompleteSeriesReleaseParserFixture`.
    #[test]
    fn complete_runs_are_recognised() {
        let s = series(&[(0, 1), (1, 10), (2, 10), (3, 10)], false);
        assert_eq!(
            seasons("Show.S01-S03.1080p.BluRay.x264-GRP", &s, false),
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            seasons("Show Complete Series 1080p WEB-DL", &s, false),
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            seasons("[Group] Show - The Complete Series [1080p]", &s, false),
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            seasons("Show Seasons 1-3 720p", &s, false),
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            seasons("Show (2015) - Season 1-3 1080p", &s, false),
            Some(vec![1, 2, 3])
        );
        // Wrong year, episodes, partial coverage in automatic mode, unknown seasons.
        assert_eq!(seasons("Show (2016) Complete Series", &s, false), None);
        assert_eq!(seasons("Show.S01E01.720p.HDTV", &s, false), None);
        assert_eq!(seasons("Show.S01-S02.1080p", &s, false), None);
        assert_eq!(seasons("Show.S01-S02.1080p", &s, true), Some(vec![1, 2]));
        assert_eq!(seasons("Show.S01-S04.1080p", &s, true), None);
        // A longer, different title does not match the shorter alias.
        assert_eq!(seasons("Show Me More S01-S03", &s, false), None);
        // "Complete" naming a season is not the complete series.
        assert_eq!(seasons("Show Complete Season 1080p", &s, false), None);
        // Reversed ranges are refused.
        assert_eq!(seasons("Show S03-S01 1080p", &s, true), None);
        let p = parse(
            "Show.S01-S03.1080p.BluRay.x264-GRP",
            &s,
            &["Show".into()],
            false,
        )
        .unwrap();
        assert!(p.full_season);
        assert_eq!(p.release_group.as_deref(), Some("GRP"));
        assert_eq!(p.release_title, "Show.S01-S03.1080p.BluRay.x264-GRP");
    }

    #[test]
    fn single_season_shows_and_anime_ranges() {
        let single = series(&[(1, 12)], false);
        assert_eq!(
            seasons("Show 1080p BluRay x265", &single, false),
            Some(vec![1])
        );
        let multi = series(&[(1, 12), (2, 12)], false);
        assert_eq!(seasons("Show 1080p BluRay x265", &multi, false), None);
        let anime = series(&[(1, 12), (2, 12)], true);
        assert_eq!(
            seasons("[SubsPlease] Show 01-24 (1080p)", &anime, false),
            Some(vec![1, 2])
        );
        assert_eq!(
            seasons("[SubsPlease] Show 01-12 (1080p)", &anime, false),
            None
        );
        assert_eq!(
            seasons("Show Complete 1-24 1080p", &anime, false),
            Some(vec![1, 2])
        );
        // Absolute ranges only count for anime.
        assert_eq!(seasons("Show 01-24 1080p", &multi, false), None);
    }
}
