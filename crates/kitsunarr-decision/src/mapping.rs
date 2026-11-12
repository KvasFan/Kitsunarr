//! Mapping a parsed release to a series and its episodes (`ParsingService.Map`, `GetEpisodes`,
//! `ParseSpecialEpisodeTitle`, `EpisodeService.FindEpisodeByTitle`), restricted to what a search
//! needs: the searched series' episodes are known; other library titles are known by name and
//! identifiers only.
//!
//! Kitsunarr has no scene-mapping service (TheXEM, Sonarr's scene-name lists). A series'
//! alternate titles stand in for its scene mappings, without season remapping.

use kitsunarr_domain::SeriesType;
use kitsunarr_parser::{
    ParsedEpisodeInfo, SeriesTitleInfo, clean_series_title, normalize_episode_title,
    parse_languages, parse_quality, parse_release_group,
};

use crate::model::{Episode, LibrarySeries, Release, SearchCriteria, SearchKind, Series};

/// The series a release was mapped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mapped {
    /// The searched series.
    Searched,
    /// Another library series.
    Other(i64),
}

fn clean(title: &str) -> String {
    clean_series_title(title).to_lowercase()
}

/// A library title for lookups (the searched series included).
struct Candidate<'a> {
    mapped: Mapped,
    title: &'a str,
    year: i32,
    alternate_titles: &'a [String],
    tvdb_id: i64,
    tv_rage_id: i64,
    imdb_id: Option<&'a str>,
}

pub(crate) struct Lookup<'a> {
    candidates: Vec<Candidate<'a>>,
}

impl<'a> Lookup<'a> {
    pub(crate) fn new(series: &'a Series, library: &'a [LibrarySeries]) -> Self {
        let mut candidates = vec![Candidate {
            mapped: Mapped::Searched,
            title: &series.title,
            year: series.year,
            alternate_titles: &series.alternate_titles,
            tvdb_id: series.tvdb_id,
            tv_rage_id: series.tv_rage_id,
            imdb_id: series.imdb_id.as_deref(),
        }];
        candidates.extend(
            library
                .iter()
                .filter(|l| l.id != series.id)
                .map(|l| Candidate {
                    mapped: Mapped::Other(l.id),
                    title: &l.title,
                    year: l.year,
                    alternate_titles: &l.alternate_titles,
                    tvdb_id: l.tvdb_id,
                    tv_rage_id: l.tv_rage_id,
                    imdb_id: l.imdb_id.as_deref(),
                }),
        );
        Self { candidates }
    }

    /// Every library title, none of them searched (RSS sync).
    pub(crate) fn library(library: &'a [LibrarySeries]) -> Self {
        Self {
            candidates: library
                .iter()
                .map(|l| Candidate {
                    mapped: Mapped::Other(l.id),
                    title: &l.title,
                    year: l.year,
                    alternate_titles: &l.alternate_titles,
                    tvdb_id: l.tvdb_id,
                    tv_rage_id: l.tv_rage_id,
                    imdb_id: l.imdb_id.as_deref(),
                })
                .collect(),
        }
    }

    /// `FindByTitle(title)` / `FindByTitle(title, year)`: clean-title equality.
    fn by_title(&self, title: &str, year: Option<i32>) -> Option<Mapped> {
        let wanted = clean(title);
        self.candidates
            .iter()
            .find(|c| clean(c.title) == wanted && year.is_none_or(|y| c.year == y))
            .map(|c| c.mapped)
    }

    /// `FindByTitleInexact`: titles contained in the release title, leftmost then longest.
    fn by_title_inexact(&self, title: &str) -> Option<Mapped> {
        let release = clean(title);
        self.candidates
            .iter()
            .filter_map(|c| {
                let t = clean(c.title);
                if t.is_empty() {
                    return None;
                }
                release
                    .find(&t)
                    .map(|position| (position, t.len(), c.mapped))
            })
            .min_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|m| m.2)
    }

    /// Scene mappings: the alternate titles.
    fn by_alias(&self, title: &str) -> Option<Mapped> {
        let wanted = clean(title);
        self.candidates
            .iter()
            .find(|c| c.alternate_titles.iter().any(|a| clean(a) == wanted))
            .map(|c| c.mapped)
    }

    fn by_id(&self, f: impl Fn(&Candidate<'_>) -> bool) -> Option<Mapped> {
        self.candidates.iter().find(|c| f(c)).map(|c| c.mapped)
    }

    /// `GetSeriesByAllTitles`: every title must resolve to the same series.
    fn by_all_titles(&self, info: &SeriesTitleInfo) -> Option<Mapped> {
        let titles = info.all_titles.as_ref()?;
        let mut found = None;
        for title in titles {
            let m = if info.year > 0 {
                self.by_title(title, Some(info.year))
                    .or_else(|| self.by_title(&format!("{title} {}", info.year), None))
            } else {
                self.by_title(title, None)
            };
            match (found, m) {
                (_, None) => continue,
                (None, Some(m)) => found = Some(m),
                (Some(f), Some(m)) if f == m => {}
                _ => return None,
            }
        }
        found
    }

    /// `FindSeries` with search criteria.
    pub(crate) fn find_series(
        &self,
        series: &Series,
        parsed: &ParsedEpisodeInfo,
        release: &Release,
    ) -> Option<Mapped> {
        if let Some(m) = self.by_alias(&parsed.series_title) {
            return Some(m);
        }
        if clean(&series.title) == clean(&parsed.series_title) {
            return Some(Mapped::Searched);
        }
        let (tvdb, tv_rage, imdb) = (
            release.tvdb_id,
            release.tv_rage_id,
            release.imdb_id.as_deref().filter(|i| !i.trim().is_empty()),
        );
        if tvdb > 0 && tvdb == series.tvdb_id {
            return Some(Mapped::Searched);
        }
        if tv_rage > 0 && tv_rage == series.tv_rage_id && tvdb <= 0 {
            return Some(Mapped::Searched);
        }
        if let Some(imdb) = imdb
            && series.imdb_id.as_deref() == Some(imdb)
            && tvdb <= 0
        {
            return Some(Mapped::Searched);
        }
        self.find_by_titles_and_ids(parsed, release)
    }

    /// `FindSeries` without search criteria (RSS sync): scene mappings, then titles and ids.
    pub(crate) fn find_series_unsearched(
        &self,
        parsed: &ParsedEpisodeInfo,
        release: &Release,
    ) -> Option<Mapped> {
        self.by_alias(&parsed.series_title)
            .or_else(|| self.find_by_titles_and_ids(parsed, release))
    }

    /// The part of `FindSeries` shared with and without search criteria.
    fn find_by_titles_and_ids(
        &self,
        parsed: &ParsedEpisodeInfo,
        release: &Release,
    ) -> Option<Mapped> {
        let (tvdb, tv_rage, imdb) = (
            release.tvdb_id,
            release.tv_rage_id,
            release.imdb_id.as_deref().filter(|i| !i.trim().is_empty()),
        );
        let info = &parsed.series_title_info;
        self.by_title(&parsed.series_title, None)
            .or_else(|| self.by_all_titles(info))
            .or_else(|| {
                (info.year > 0)
                    .then(|| self.by_title(&info.title_without_year, Some(info.year)))
                    .flatten()
            })
            .or_else(|| {
                (tvdb > 0)
                    .then(|| self.by_id(|c| c.tvdb_id == tvdb))
                    .flatten()
            })
            .or_else(|| {
                (tv_rage > 0 && tvdb <= 0)
                    .then(|| self.by_id(|c| c.tv_rage_id == tv_rage))
                    .flatten()
            })
            .or_else(|| {
                imdb.filter(|_| tvdb <= 0)
                    .and_then(|i| self.by_id(|c| c.imdb_id == Some(i)))
            })
    }

    /// The series `ParseSpecialEpisodeTitle(…, tvdbId, tvRageId, imdbId)` looks in without
    /// search criteria: by ids, else `GetSeries(title)`.
    pub(crate) fn special_series(&self, release: &Release, title: &str) -> Option<Mapped> {
        let by_ids = (release.tvdb_id > 0)
            .then(|| self.by_id(|c| c.tvdb_id == release.tvdb_id))
            .flatten()
            .or_else(|| {
                (release.tv_rage_id > 0)
                    .then(|| self.by_id(|c| c.tv_rage_id == release.tv_rage_id))
                    .flatten()
            })
            .or_else(|| {
                release
                    .imdb_id
                    .as_deref()
                    .filter(|i| !i.trim().is_empty())
                    .and_then(|i| self.by_id(|c| c.imdb_id == Some(i)))
            });
        by_ids.or_else(|| self.series_by_title(release, title))
    }

    /// `GetSeries(title)`: parsed titles, then the inexact title, then the release's ids.
    fn series_by_title(&self, release: &Release, title: &str) -> Option<Mapped> {
        let found = match kitsunarr_parser::parse_title(title) {
            None => self.by_title(title, None),
            Some(p) => self
                .by_alias(&p.series_title)
                .or_else(|| self.by_title(&p.series_title, None))
                .or_else(|| self.by_all_titles(&p.series_title_info))
                .or_else(|| {
                    self.by_title(
                        &p.series_title_info.title_without_year,
                        Some(p.series_title_info.year),
                    )
                }),
        };
        found.or_else(|| self.by_title_inexact(title)).or_else(|| {
            let ids = (
                release.tvdb_id,
                release.tv_rage_id,
                release.imdb_id.as_deref(),
            );
            self.by_id(|c| {
                (ids.0 > 0 && c.tvdb_id == ids.0)
                    || (ids.1 > 0 && c.tv_rage_id == ids.1)
                    || ids
                        .2
                        .is_some_and(|i| !i.trim().is_empty() && c.imdb_id == Some(i))
            })
        })
    }

    /// The series `ParseSpecialEpisodeTitle` would use, when it is the searched one.
    pub(crate) fn special_series_is_searched(
        &self,
        series: &Series,
        release: &Release,
        title: &str,
    ) -> bool {
        if (release.tvdb_id != 0 && release.tvdb_id == series.tvdb_id)
            || (release.tv_rage_id != 0 && release.tv_rage_id == series.tv_rage_id)
            || release
                .imdb_id
                .as_deref()
                .is_some_and(|i| !i.trim().is_empty() && series.imdb_id.as_deref() == Some(i))
        {
            return true;
        }
        // `GetSeries(title)`.
        self.series_by_title(release, title) == Some(Mapped::Searched)
    }
}

/// `string.IndexOf(value, CurrentCultureIgnoreCase)` as a character position.
pub(crate) fn index_of_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    let h: Vec<char> = haystack.to_lowercase().chars().collect();
    let n: Vec<char> = needle.to_lowercase().chars().collect();
    if n.is_empty() {
        return Some(0);
    }
    h.windows(n.len()).position(|w| w == n.as_slice())
}

/// `EpisodeService.FindEpisodeByTitle(seriesId, 0, releaseTitle)`.
fn find_special_by_title<'e>(series: &'e Series, release_title: &str) -> Option<&'e Episode> {
    let normalized = normalize_episode_title(release_title);
    let clean_normalized = clean_series_title(&normalized);
    let mut matches: Vec<(usize, usize, usize, &Episode)> = Vec::new();
    for (order, e) in series
        .episodes
        .iter()
        .filter(|e| e.season_number == 0)
        .enumerate()
    {
        let title = e.title.as_deref().unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let normalized_title = normalize_episode_title(title);
        let length = normalized_title.chars().count();
        for position in [
            index_of_ignore_case(&normalized, &normalized_title),
            index_of_ignore_case(&clean_normalized, &clean_series_title(&normalized_title)),
        ]
        .into_iter()
        .flatten()
        {
            matches.push((position, length, order, e));
        }
    }
    // OrderBy(Position).ThenByDescending(Length), stable.
    matches.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    matches.first().map(|m| m.3)
}

/// `ParseSpecialEpisodeTitle(parsedEpisodeInfo, releaseTitle, series)`.
pub(crate) fn parse_special_episode_title(
    parsed: Option<&ParsedEpisodeInfo>,
    release_title: &str,
    series: &Series,
) -> Option<ParsedEpisodeInfo> {
    if let Some(p) = parsed
        && p.is_possible_scene_season_special()
        && series.use_scene_numbering
        && series.episodes.iter().any(|e| {
            e.scene_season_number == p.season_numbers.first().copied()
                && e.scene_episode_number == Some(0)
        })
    {
        return Some(p.clone());
    }
    let episode = find_special_by_title(series, release_title)?;
    Some(ParsedEpisodeInfo {
        release_title: release_title.to_string(),
        series_title: series.title.clone(),
        series_title_info: SeriesTitleInfo {
            title: series.title.clone(),
            ..Default::default()
        },
        season_numbers: vec![episode.season_number],
        episode_numbers: vec![episode.episode_number],
        full_season: false,
        quality: parse_quality(release_title),
        release_group: parse_release_group(release_title),
        languages: parse_languages(release_title),
        special: true,
        ..Default::default()
    })
}

/// `GetEpisodes` for the searched series (scene source, no scene mapping).
pub(crate) fn episodes<'e>(
    parsed: &ParsedEpisodeInfo,
    series: &'e Series,
    criteria: &SearchCriteria,
) -> Vec<&'e Episode> {
    let Some(&season) = parsed.season_numbers.first() else {
        return Vec::new();
    };
    // `ValidateForSeriesType`.
    if parsed.is_daily() && series.series_type == SeriesType::Standard {
        return Vec::new();
    }
    let all = &series.episodes;
    if parsed.full_season {
        // Tanukarr: a multi-season pack found by a season search stands for that season only.
        if parsed.is_multi_season()
            && !series.use_scene_numbering
            && let SearchKind::Season { season: wanted } = criteria.kind
            && parsed.season_numbers.contains(&wanted)
        {
            return all.iter().filter(|e| e.season_number == wanted).collect();
        }
        if parsed.is_multi_season() {
            let mut out: Vec<&Episode> = Vec::new();
            for s in &parsed.season_numbers {
                let mut found: Vec<&Episode> = if series.use_scene_numbering {
                    all.iter()
                        .filter(|e| e.scene_season_number == Some(*s))
                        .collect()
                } else {
                    Vec::new()
                };
                if found.is_empty() {
                    found = all.iter().filter(|e| e.season_number == *s).collect();
                }
                for e in found {
                    if !out.iter().any(|o| o.id == e.id) {
                        out.push(e);
                    }
                }
            }
            return out;
        }
        if series.use_scene_numbering {
            let by_scene: Vec<&Episode> = all
                .iter()
                .filter(|e| e.scene_season_number == Some(season))
                .collect();
            if !by_scene.is_empty() {
                return by_scene;
            }
        }
        return all.iter().filter(|e| e.season_number == season).collect();
    }
    if parsed.is_daily() {
        let date = parsed.air_date.as_deref().unwrap_or_default();
        let requested = all
            .iter()
            .filter(|e| criteria.episode_ids.contains(&e.id) && e.air_date.as_deref() == Some(date))
            .collect::<Vec<_>>();
        if requested.len() == 1 {
            return requested;
        }
        let mut on_date: Vec<&Episode> = all
            .iter()
            .filter(|e| e.air_date.as_deref() == Some(date))
            .collect();
        on_date.sort_by_key(|e| (e.season_number, e.episode_number));
        let part = parsed.daily_part.unwrap_or(1).max(1) as usize;
        return on_date.get(part - 1).map(|e| vec![*e]).unwrap_or_default();
    }
    if parsed.is_absolute_numbering() {
        let mut out = Vec::new();
        for &absolute in &parsed.absolute_episode_numbers {
            let mut found: Vec<&Episode> = Vec::new();
            if parsed.special {
                found.extend(
                    all.iter()
                        .find(|e| e.season_number == 0 && e.episode_number == absolute),
                );
            } else if season > 1 && parsed.episode_numbers.is_empty() {
                found = all
                    .iter()
                    .filter(|e| {
                        e.scene_season_number == Some(season)
                            && e.scene_episode_number == Some(absolute)
                    })
                    .collect();
                if found.is_empty() {
                    found.extend(
                        all.iter()
                            .find(|e| e.season_number == season && e.episode_number == absolute),
                    );
                }
            }
            if found.is_empty() {
                found.extend(
                    all.iter()
                        .find(|e| e.absolute_episode_number == Some(absolute)),
                );
            }
            out.extend(found);
        }
        return out;
    }
    if parsed.is_possible_scene_season_special()
        && let Some(special) =
            parse_special_episode_title(Some(parsed), &parsed.release_title, series)
    {
        let s = special.season_numbers.first().copied().unwrap_or(0);
        return standard(&special, series, s, false);
    }
    if parsed.special && season != 0 {
        return Vec::new();
    }
    standard(parsed, series, season, true)
}

/// `GetStandardEpisodes`.
fn standard<'e>(
    parsed: &ParsedEpisodeInfo,
    series: &'e Series,
    season: i32,
    scene_source: bool,
) -> Vec<&'e Episode> {
    let all = &series.episodes;
    let mut out = Vec::new();
    for &number in &parsed.episode_numbers {
        if series.use_scene_numbering && scene_source {
            let parsed_season = parsed.season_numbers.first().copied().unwrap_or(-1);
            let mut found: Vec<&Episode> = all
                .iter()
                .filter(|e| {
                    e.scene_season_number == Some(parsed_season)
                        && e.scene_episode_number == Some(number)
                })
                .collect();
            if found.is_empty() {
                found = all
                    .iter()
                    .filter(|e| {
                        e.scene_season_number == Some(season)
                            && e.scene_episode_number == Some(number)
                    })
                    .collect();
            }
            if !found.is_empty() {
                out.extend(found);
                continue;
            }
        }
        if let Some(e) = all
            .iter()
            .find(|e| e.season_number == season && e.episode_number == number)
        {
            out.push(e);
        }
    }
    out
}
