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
        self.alias_candidate(title).map(|c| c.mapped)
    }

    fn alias_candidate(&self, title: &str) -> Option<&Candidate<'a>> {
        let wanted = clean(title);
        self.candidates
            .iter()
            .find(|c| c.alternate_titles.iter().any(|a| clean(a) == wanted))
    }

    /// `GetSeriesAliasTitleAndYear`: the title without its year is a scene name of a series of
    /// that year.
    fn by_alias_and_year(&self, info: &SeriesTitleInfo) -> Option<Mapped> {
        self.alias_candidate(&info.title_without_year)
            .filter(|c| c.year == info.year)
            .map(|c| c.mapped)
    }

    fn by_id(&self, f: impl Fn(&Candidate<'_>) -> bool) -> Option<Mapped> {
        self.candidates.iter().find(|c| f(c)).map(|c| c.mapped)
    }

    /// `GetSeriesByAllTitles`: every title (by name, else by scene mapping) must resolve to the
    /// same series.
    fn by_all_titles(&self, info: &SeriesTitleInfo) -> Option<Mapped> {
        let titles = info.all_titles.as_ref()?;
        let mut found = None;
        for title in titles {
            let m = if info.year > 0 {
                self.by_title(title, Some(info.year))
                    .or_else(|| self.by_title(&format!("{title} {}", info.year), None))
            } else {
                self.by_title(title, None)
            }
            .or_else(|| self.by_alias(title));
            match (found, m) {
                (_, None) => continue,
                (None, Some(m)) => found = Some(m),
                (Some(f), Some(m)) if f == m => {}
                _ => return None,
            }
        }
        found
    }

    /// Tanukarr's `FindByTitleWithoutSeasonSuffix`: "Title 4 - S04E16" names series "Title". Only
    /// for season releases without a parsed year whose title ends in that season's number.
    fn without_season_suffix(parsed: &ParsedEpisodeInfo) -> Option<String> {
        let season = *parsed.season_numbers.first()?;
        if season <= 0 || parsed.series_title_info.year > 0 || parsed.is_absolute_numbering() {
            return None;
        }
        let title = parsed.series_title.trim_end();
        let (head, number) = title.rsplit_once(' ')?;
        let n: i32 = (number.len() <= 2 && number.chars().all(|c| c.is_ascii_digit()))
            .then(|| number.parse().ok())
            .flatten()?;
        (n == season && !head.trim().is_empty()).then(|| head.trim_end().to_string())
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
        if let Some(stripped) = Self::without_season_suffix(parsed)
            && clean(&series.title) == clean(&stripped)
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
                    .then(|| {
                        self.by_title(&info.title_without_year, Some(info.year))
                            .or_else(|| self.by_alias_and_year(info))
                    })
                    .flatten()
            })
            .or_else(|| self.without_season_suffix_series(parsed))
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

    /// `GetSeries(title)`: the title as is when it does not parse, else the parsed title's scene
    /// mapping, the title, every title of a multi-title name, the title without its year, and
    /// the title without a trailing season number.
    pub(crate) fn get_series(&self, title: &str) -> Option<Mapped> {
        match kitsunarr_parser::parse_title(title) {
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
                })
                .or_else(|| self.without_season_suffix_series(&p)),
        }
    }

    /// `FindByTitleWithoutSeasonSuffix`: the stripped title's scene mapping, else the title.
    fn without_season_suffix_series(&self, parsed: &ParsedEpisodeInfo) -> Option<Mapped> {
        Self::without_season_suffix(parsed)
            .and_then(|t| self.by_alias(&t).or_else(|| self.by_title(&t, None)))
    }

    /// `ParseSpecialEpisodeTitle`'s series without search criteria: `GetSeries(title)`, then the
    /// inexact title, then the release's ids.
    fn series_by_title(&self, release: &Release, title: &str) -> Option<Mapped> {
        self.get_series(title)
            .or_else(|| self.by_title_inexact(title))
            .or_else(|| {
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

/// The scene season a scene name of the series stands for (`GetSceneSeasonNumber`): the
/// release's series title is one of TheXEM's names for a single season.
pub(crate) fn scene_season_of_title(parsed: &ParsedEpisodeInfo, series: &Series) -> Option<i32> {
    let wanted = clean(&parsed.series_title);
    if wanted.is_empty() {
        return None;
    }
    series
        .scene_season_titles
        .iter()
        .find(|(t, _)| clean(t) == wanted)
        .map(|(_, s)| *s)
}

/// `RemoteEpisode.MappedSeasonNumber`: a release named with TheXEM's name of a later season
/// (numbered from season 1 under that name, or a season title) belongs to that scene season.
pub(crate) fn mapped_season(parsed: &ParsedEpisodeInfo, series: &Series) -> Option<i32> {
    let parsed_season = parsed.season_numbers.first().copied();
    match scene_season_of_title(parsed, series) {
        Some(scene) if parsed.is_season_title => Some(scene),
        Some(scene) if parsed_season == Some(1) && scene != 1 => Some(scene),
        _ => parsed_season,
    }
}

/// What `ParsingService.Map(parsedEpisodeInfo, series, searchCriteria)` yields for a release
/// already mapped to `series` (`RemoteEpisode`).
pub(crate) struct Remote<'e> {
    pub(crate) episodes: Vec<&'e Episode>,
    pub(crate) mapped_season: Option<i32>,
    pub(crate) numbering_rejection: Option<String>,
}

/// `Map` for a known series: `GetEpisodes`, then Tanukarr's `EpisodeNumberingResolver` (not for
/// complete-series searches), which picks scene or aired numbering by the episode title or the
/// scene mapping's origin.
pub(crate) fn map_remote<'e>(
    parsed: &ParsedEpisodeInfo,
    series: &'e Series,
    criteria: &SearchCriteria,
) -> Remote<'e> {
    let mut remote = Remote {
        episodes: episodes(parsed, series, criteria),
        mapped_season: mapped_season(parsed, series),
        numbering_rejection: None,
    };
    if matches!(criteria.kind, SearchKind::CompleteSeries { .. }) {
        return remote;
    }
    let lookup = crate::numbering::xem_lookup(series);
    if let Some(r) = crate::numbering::resolve(series, parsed, None, None, &lookup) {
        remote.numbering_rejection = r.rejection;
        if let Some(mut resolved) = r.episodes {
            if parsed.is_multi_season()
                && let SearchKind::Season { season } = criteria.kind
            {
                resolved.retain(|e| e.season_number == season);
            }
            let mut seasons: Vec<i32> = resolved.iter().map(|e| e.season_number).collect();
            seasons.dedup();
            if let [season] = seasons.as_slice() {
                remote.mapped_season = Some(*season);
            }
            remote.episodes = resolved;
        }
    }
    remote
}

/// `GetEpisodes` for the searched series (scene source, no scene mapping).
pub(crate) fn episodes<'e>(
    parsed: &ParsedEpisodeInfo,
    series: &'e Series,
    criteria: &SearchCriteria,
) -> Vec<&'e Episode> {
    let Some(season) = mapped_season(parsed, series) else {
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
            // `GetSceneSeasonMapping` for each season: TheXEM's name of another scene season
            // moves season 1 to that scene season.
            let xem = scene_season_of_title(parsed, series);
            let mut out: Vec<&Episode> = Vec::new();
            for &s in &parsed.season_numbers {
                let mapped = match xem {
                    Some(scene) if scene >= 0 && s == 1 && scene != s => scene,
                    _ => s,
                };
                let mut found: Vec<&Episode> = if series.use_scene_numbering {
                    all.iter()
                        .filter(|e| e.scene_season_number == Some(mapped))
                        .collect()
                } else {
                    Vec::new()
                };
                if found.is_empty() {
                    found = all.iter().filter(|e| e.season_number == mapped).collect();
                }
                for e in found {
                    if !out.iter().any(|o| o.id == e.id) {
                        out.push(e);
                    }
                }
            }
            if let SearchKind::Season { season: wanted } = criteria.kind {
                out.retain(|e| e.season_number == wanted);
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
        return find_by_air_date(all, date, parsed.daily_part)
            .map(|e| vec![e])
            .unwrap_or_default();
    }
    if parsed.is_absolute_numbering() {
        let scene_season = scene_season_of_title(parsed, series);
        let mut out = Vec::new();
        for &absolute in &parsed.absolute_episode_numbers {
            let mut found: Vec<&Episode> = Vec::new();
            if parsed.special {
                found.extend(
                    all.iter()
                        .find(|e| e.season_number == 0 && e.episode_number == absolute),
                );
            } else if let Some(scene) = scene_season {
                // Numbered within the scene season the release's name stands for.
                found = all
                    .iter()
                    .filter(|e| {
                        e.scene_season_number == Some(scene)
                            && e.scene_episode_number == Some(absolute)
                    })
                    .collect();
                if found.is_empty() {
                    found.extend(
                        all.iter()
                            .find(|e| e.season_number == scene && e.episode_number == absolute),
                    );
                }
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
            } else {
                // TheXEM's scene absolute numbers; several matches without a scene name are
                // ambiguous and ignored.
                found = all
                    .iter()
                    .filter(|e| e.scene_absolute_episode_number == Some(absolute))
                    .collect();
                if found.len() > 1 {
                    found.clear();
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
        return standard(&special, series, s, false, &criteria.episode_ids);
    }
    if parsed.special && season != 0 {
        return Vec::new();
    }
    standard(parsed, series, season, true, &criteria.episode_ids)
}

/// `EpisodeService.FindEpisode(seriesId, airDate, part)` (`FindOneByAirDate`): the only episode
/// of the day, else specials aside the only one or the `part`-th in season and episode order.
fn find_by_air_date<'e>(all: &'e [Episode], date: &str, part: Option<i32>) -> Option<&'e Episode> {
    let on_date: Vec<&Episode> = all
        .iter()
        .filter(|e| e.air_date.as_deref() == Some(date))
        .collect();
    if on_date.len() <= 1 {
        return on_date.first().copied();
    }
    let mut regular: Vec<&Episode> = on_date
        .into_iter()
        .filter(|e| e.season_number > 0)
        .collect();
    match part {
        None if regular.len() == 1 => Some(regular[0]),
        Some(part) if part as usize <= regular.len() => {
            regular.sort_by_key(|e| (e.season_number, e.episode_number));
            usize::try_from(part - 1)
                .ok()
                .and_then(|i| regular.get(i).copied())
        }
        // Upstream throws: several episodes air that day.
        _ => None,
    }
}

/// `GetStandardEpisodes`.
fn standard<'e>(
    parsed: &ParsedEpisodeInfo,
    series: &'e Series,
    season: i32,
    scene_source: bool,
    requested: &[i64],
) -> Vec<&'e Episode> {
    let all = &series.episodes;
    let mut out = Vec::new();
    for &number in &parsed.episode_numbers {
        if series.use_scene_numbering && scene_source {
            // The searched episodes by the release's own season first, then every episode by
            // the mapped season.
            let parsed_season = parsed.season_numbers.first().copied().unwrap_or(-1);
            let mut found: Vec<&Episode> = all
                .iter()
                .filter(|e| {
                    requested.contains(&e.id)
                        && e.scene_season_number == Some(parsed_season)
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

#[cfg(test)]
mod tests {
    use super::Lookup;

    /// Tanukarr's `TrailingSeasonSuffixFixture`.
    #[test]
    fn season_suffixes_are_stripped_only_for_matching_season_releases() {
        let stripped = |title: &str| {
            let parsed = kitsunarr_parser::parse_title(title).unwrap();
            Lookup::without_season_suffix(&parsed)
        };
        assert_eq!(
            stripped("Tensei Shitara Slime Datta Ken 4 - S04E16 1080p WEB-DL").as_deref(),
            Some("Tensei Shitara Slime Datta Ken")
        );
        // The number must be the release's season; absolute releases never strip.
        assert_eq!(
            stripped("Tensei Shitara Slime Datta Ken 2 - S04E16 1080p"),
            None
        );
        assert_eq!(
            stripped("[Group] Tensei Shitara Slime Datta Ken 4 - 88 [1080p]"),
            None
        );
        // A parsed year keeps the title as is.
        assert_eq!(stripped("Series Title 2019 4 - S04E01"), None);
    }

    /// TheXEM: TheTVDB's 1x07–1x08 are the scene's 2x01–2x02 (absolute 7–8), named
    /// "Series Title Part Two".
    fn xem_series(series_type: kitsunarr_domain::SeriesType) -> crate::Series {
        let mut s = crate::tests::series();
        s.series_type = series_type;
        s.use_scene_numbering = true;
        s.alternate_titles = vec!["Series Title Part Two".into()];
        s.scene_season_titles = vec![("Series Title Part Two".into(), 2)];
        s.episodes = (1..=8)
            .map(|n| crate::Episode {
                id: i64::from(n),
                season_number: 1,
                episode_number: n,
                absolute_episode_number: Some(n),
                scene_season_number: Some(if n > 6 { 2 } else { 1 }),
                scene_episode_number: Some(if n > 6 { n - 6 } else { n }),
                scene_absolute_episode_number: Some(n),
                title: None,
                air_date: None,
                air_date_utc: None,
                runtime: 30,
                monitored: true,
                file: None,
            })
            .collect();
        s
    }

    fn mapped(title: &str, series: &crate::Series) -> Vec<(i32, i32)> {
        let parsed = kitsunarr_parser::parse_title(title).unwrap();
        crate::map_episodes(&parsed, series)
            .iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect()
    }

    #[test]
    fn scene_numbers_and_xem_names_map_to_thetvdb_episodes() {
        let standard = xem_series(kitsunarr_domain::SeriesType::Standard);
        assert_eq!(
            mapped("Series.Title.S02E01.720p.HDTV-GRP", &standard),
            [(1, 7)]
        );
        assert_eq!(
            mapped("Series.Title.S01E02.720p.HDTV-GRP", &standard),
            [(1, 2)]
        );
        // Named for scene season 2 and numbered from 1 under that name (`MappedSeasonNumber`).
        assert_eq!(
            mapped("Series.Title.Part.Two.S01E02.720p.HDTV-GRP", &standard),
            [(1, 8)]
        );
        let anime = xem_series(kitsunarr_domain::SeriesType::Anime);
        // Absolute numbers within the scene season the name stands for.
        assert_eq!(
            mapped("[Group] Series Title Part Two - 01 [1080p]", &anime),
            [(1, 7)]
        );
        // TheXEM's scene absolute numbers.
        assert_eq!(
            mapped("[Group] Series Title - 08 [1080p]", &anime),
            [(1, 8)]
        );
    }

    /// `GetEpisodes`: a multi-season pack found by a season search stands for the searched
    /// season, scene numbering or not.
    #[test]
    fn multi_season_packs_found_by_a_season_search_keep_that_season() {
        // TheTVDB's 1x01 moved to season 2 (scene 2x01).
        let mut series = xem_series(kitsunarr_domain::SeriesType::Standard);
        series.episodes[0].season_number = 2;
        series.episodes[0].scene_season_number = Some(2);
        let parsed = kitsunarr_parser::parse_title("Series.Title.S01-S02.720p.HDTV-GRP").unwrap();
        let criteria = crate::SearchCriteria {
            kind: crate::SearchKind::Season { season: 2 },
            episode_ids: vec![1],
            monitored_episodes_only: false,
            user_invoked: true,
        };
        let ids: Vec<i64> = super::episodes(&parsed, &series, &criteria)
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, [1]);
    }
}
