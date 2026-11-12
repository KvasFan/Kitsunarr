//! Kitsunarr's decision engine: a port of Sonarr's `DecisionEngine` and `CustomFormats`.
//!
//! Given the releases a search found, it maps each to the searched series and its episodes,
//! evaluates custom formats and the decision specifications, and ranks the results with Sonarr's
//! comparer chain. The behaviour follows Sonarr at revision `c018578e4` (the parser port's
//! revision), except where Kitsunarr's data model is Sonarr v4's: size limits come from the
//! quality definitions.
//!
//! What is not evaluated yet, for lack of data: the blocklist, the download queue, history,
//! pending releases, delay profiles' delays (searches bypass them in Sonarr anyway), free disk
//! space, scene mappings and TheXEM numbering.

pub mod complete_series;
pub mod custom_format;
pub mod model;
pub mod profile;

mod compare;
mod mapping;
mod specs;
#[cfg(test)]
mod tests;

use kitsunarr_domain::Language;
use kitsunarr_parser::{
    ParsedEpisodeInfo, has_multiple_languages, normalize_episode_title, parse_languages,
    parse_quality, parse_season_title, parse_title,
};

pub use custom_format::{CompiledFormat, CustomFormatInput, compile_formats};
pub use model::*;
pub use profile::{ProfileView, ProperDownloadType, QualityIndex};

use crate::custom_format::evaluate;
use crate::mapping::{Lookup, Mapped, index_of_ignore_case};
use crate::specs::{Env, Subject};

/// The episodes of `series` a parse result refers to (`ParsingService.GetEpisodes`, with no
/// search criteria; specials matched by title when the parse marks a possible special).
pub fn map_episodes<'s>(parsed: &ParsedEpisodeInfo, series: &'s Series) -> Vec<&'s Episode> {
    let none = SearchCriteria {
        monitored_episodes_only: false,
        kind: SearchKind::Special,
        episode_ids: Vec::new(),
    };
    mapping::episodes(parsed, series, &none)
}

/// RSS sync, first step of `GetRssDecision`: when the title does not parse, or parses as a
/// possible special, the library series whose specials it may name (by the release's ids, else
/// by title). Confirm with [`rss_is_special_of`] once that series' episodes are known.
pub fn rss_special_candidate(release: &Release, library: &[LibrarySeries]) -> Option<i64> {
    let parsed = parse_title(&release.title);
    if !parsed
        .as_ref()
        .is_none_or(ParsedEpisodeInfo::is_possible_special_episode)
    {
        return None;
    }
    match Lookup::library(library).special_series(release, &release.title) {
        Some(Mapped::Other(id)) => Some(id),
        _ => None,
    }
}

/// Whether the release names one of `series`' specials (`ParseSpecialEpisodeTitle`).
pub fn rss_is_special_of(release: &Release, series: &Series) -> bool {
    let parsed = parse_title(&release.title);
    mapping::parse_special_episode_title(parsed.as_ref(), &release.title, series).is_some()
}

/// RSS sync: the library series a release belongs to (`ParsingService.Map` without search
/// criteria), or `None` when it does not parse or names no library title.
pub fn rss_series(release: &Release, library: &[LibrarySeries]) -> Option<i64> {
    let parsed = parse_title(&release.title).or_else(|| parse_season_title(&release.title))?;
    if parsed.series_title.trim().is_empty() {
        return None;
    }
    match Lookup::library(library).find_series_unsearched(&parsed, release) {
        Some(Mapped::Other(id)) => Some(id),
        _ => None,
    }
}

/// Everything a search's decisions depend on.
pub struct Engine<'a> {
    pub series: &'a Series,
    /// The other library titles (the searched series may be included; it is skipped).
    pub library: &'a [LibrarySeries],
    pub formats: &'a [CompiledFormat],
    pub settings: &'a Settings,
}

fn unknown_language() -> &'static Language {
    Language::by_id(0).expect("the language catalogue has Unknown (0)")
}

/// `LINQ Except`: distinct elements of `a` that are not in `b`, in order.
fn except(a: &[&'static Language], b: &[&'static Language]) -> Vec<&'static Language> {
    let mut out: Vec<&'static Language> = Vec::new();
    for l in a {
        if !b.iter().any(|x| x.id == l.id) && !out.iter().any(|x| x.id == l.id) {
            out.push(l);
        }
    }
    out
}

impl Engine<'_> {
    /// Decides on every release of one search and returns the decisions best first
    /// (`GetSearchDecision`, `DeDupeDecisions`, `PrioritizeDecisions`).
    pub fn decide(&self, releases: &[Release], criteria: &SearchCriteria) -> Vec<Decision> {
        let decisions = self.evaluate(releases, criteria);
        self.prioritize(Self::dedupe(decisions, releases), releases)
    }

    /// The decision on each release, in input order. `Decision::release` indexes `releases`.
    pub fn evaluate(&self, releases: &[Release], criteria: &SearchCriteria) -> Vec<Decision> {
        if let SearchKind::CompleteSeries {
            include_season_packs,
        } = criteria.kind
        {
            return self.evaluate_complete_series(releases, criteria, include_season_packs);
        }
        let lookup = Lookup::new(self.series, self.library);
        releases
            .iter()
            .enumerate()
            .map(|(i, r)| self.decide_one(&lookup, criteria, i, r))
            .collect()
    }

    /// Complete-series search: only releases `CompleteSeriesReleaseParser` reads as a run of
    /// this series are decided; the others are left out, as upstream.
    fn evaluate_complete_series(
        &self,
        releases: &[Release],
        criteria: &SearchCriteria,
        include_season_packs: bool,
    ) -> Vec<Decision> {
        let series = self.series;
        let titles: Vec<String> = std::iter::once(series.title.clone())
            .chain(series.alternate_titles.iter().cloned())
            .collect();
        releases
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let parsed =
                    complete_series::parse(&r.title, series, &titles, include_season_packs)?;
                // Release identifiers that name another series.
                let conflicts = (r.tvdb_id > 0 && series.tvdb_id > 0 && r.tvdb_id != series.tvdb_id)
                    || (r.tv_rage_id > 0 && series.tv_rage_id > 0 && r.tv_rage_id != series.tv_rage_id)
                    || matches!((&r.imdb_id, &series.imdb_id), (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() && a != b);
                Some(if conflicts {
                    self.rejected(
                        i,
                        r,
                        Some(parsed),
                        Some(series.id),
                        Rejection::new(
                            RejectionReason::WrongSeries,
                            "Release identifiers conflict with searched series",
                        ),
                    )
                } else {
                    self.decide_mapped(criteria, i, r, parsed)
                })
            })
            .collect()
    }

    /// `DeDupeDecisions`: one decision per GUID, the one with the fewest rejections, then the
    /// best indexer priority (first seen on ties).
    pub fn dedupe(decisions: Vec<Decision>, releases: &[Release]) -> Vec<Decision> {
        let mut out: Vec<Decision> = Vec::new();
        for d in decisions {
            let guid = &releases[d.release].guid;
            match out.iter_mut().find(|o| &releases[o.release].guid == guid) {
                None => out.push(d),
                Some(existing) => {
                    let key =
                        |x: &Decision| (x.rejections.len(), releases[x.release].indexer_priority);
                    if key(&d) < key(existing) {
                        *existing = d;
                    }
                }
            }
        }
        out
    }

    /// `PrioritizeDecisions`: best first. `releases` must be the slice the decisions index.
    pub fn prioritize(&self, decisions: Vec<Decision>, releases: &[Release]) -> Vec<Decision> {
        let comparer = compare::Comparer {
            series: self.series,
            settings: self.settings,
            releases,
            preferred_protocol: self
                .env_for(None)
                .delay_profile()
                .map(|d| d.preferred_protocol),
        };
        compare::prioritize(&comparer, decisions)
    }

    fn env_for<'c>(&'c self, criteria: Option<&'c SearchCriteria>) -> Env<'c> {
        static NONE: SearchCriteria = SearchCriteria {
            monitored_episodes_only: false,
            kind: SearchKind::Special,
            episode_ids: Vec::new(),
        };
        Env {
            series: self.series,
            settings: self.settings,
            formats: self.formats,
            criteria: criteria.unwrap_or(&NONE),
        }
    }

    fn rejected(
        &self,
        index: usize,
        release: &Release,
        parsed: Option<ParsedEpisodeInfo>,
        series_id: Option<i64>,
        rejection: Rejection,
    ) -> Decision {
        let (quality, languages) = match &parsed {
            Some(p) => (p.quality, p.languages.clone()),
            None => (
                parse_quality(&release.title),
                parse_languages(&release.title),
            ),
        };
        Decision {
            release: index,
            parsed,
            quality,
            languages,
            series_id,
            episodes: Vec::new(),
            custom_formats: Vec::new(),
            custom_format_score: 0,
            rejections: vec![rejection],
        }
    }

    /// `GetDecisions` for one release, with search criteria.
    fn decide_one(
        &self,
        lookup: &Lookup<'_>,
        criteria: &SearchCriteria,
        index: usize,
        release: &Release,
    ) -> Decision {
        let series = self.series;
        let mut parsed = parse_title(&release.title);
        if parsed
            .as_ref()
            .is_none_or(ParsedEpisodeInfo::is_possible_special_episode)
            && lookup.special_series_is_searched(series, release, &release.title)
            && let Some(special) =
                mapping::parse_special_episode_title(parsed.as_ref(), &release.title, series)
        {
            parsed = Some(special);
        }
        if parsed.is_none() {
            parsed = parse_season_title(&release.title);
        }
        let parsed = match parsed {
            Some(p) if !p.series_title.trim().is_empty() => p,
            other => {
                return self.rejected(
                    index,
                    release,
                    other,
                    None,
                    Rejection::new(RejectionReason::UnableToParse, "Unable to parse release"),
                );
            }
        };
        match lookup.find_series(series, &parsed, release) {
            None => self.rejected(
                index,
                release,
                Some(parsed),
                None,
                Rejection::new(RejectionReason::UnknownSeries, "Unknown Series"),
            ),
            // `SeriesSpecification`. (Sonarr would also evaluate the other series' episodes
            // and profile; only this rejection is reported here.)
            Some(Mapped::Other(id)) => self.rejected(
                index,
                release,
                Some(parsed),
                Some(id),
                Rejection::new(RejectionReason::WrongSeries, "Wrong series"),
            ),
            Some(Mapped::Searched) => self.decide_mapped(criteria, index, release, parsed),
        }
    }

    fn decide_mapped(
        &self,
        criteria: &SearchCriteria,
        index: usize,
        release: &Release,
        parsed: ParsedEpisodeInfo,
    ) -> Decision {
        let series = self.series;
        let episodes = mapping::episodes(&parsed, series, criteria);
        if episodes.is_empty() {
            return self.rejected(
                index,
                release,
                Some(parsed),
                Some(series.id),
                Rejection::new(
                    RejectionReason::UnknownEpisode,
                    "Unable to identify correct episode(s) using release name and scene mappings",
                ),
            );
        }
        let Some(languages) = self.aggregate_languages(release, &parsed, &episodes) else {
            return self.rejected(
                index,
                release,
                Some(parsed),
                Some(series.id),
                Rejection::new(
                    RejectionReason::DecisionError,
                    "Unexpected error processing release",
                ),
            );
        };
        let input = CustomFormatInput {
            release_title: &parsed.release_title,
            file_name: None,
            release_group: parsed.release_group.as_deref(),
            quality: parsed.quality.quality,
            size: release.size,
            indexer_flags: release.indexer_flags,
            languages: &languages,
            release_type: parsed.release_type(),
            original_language: series.original_language,
        };
        let formats = evaluate(self.formats, &input);
        let score = ProfileView(&series.profile).format_score(formats.iter().map(|f| &f.id));
        let env = self.env_for(Some(criteria));
        let rejections = specs::evaluate_all(
            &env,
            &Subject {
                release,
                parsed: &parsed,
                episodes: episodes.clone(),
                mapped_season: parsed.season_numbers.first().copied(),
                custom_formats: formats.clone(),
                score,
            },
        );
        Decision {
            release: index,
            quality: parsed.quality,
            languages,
            series_id: Some(series.id),
            episodes: episodes
                .iter()
                .map(|e| MappedEpisode {
                    id: e.id,
                    season_number: e.season_number,
                    episode_number: e.episode_number,
                    runtime: e.runtime,
                })
                .collect(),
            custom_formats: formats.iter().map(|f| f.name.clone()).collect(),
            custom_format_score: score,
            rejections,
            parsed: Some(parsed),
        }
    }

    /// `AggregateLanguages`. `None` where Sonarr would fail with an exception.
    fn aggregate_languages(
        &self,
        release: &Release,
        parsed: &ParsedEpisodeInfo,
        episodes: &[&Episode],
    ) -> Option<Vec<&'static Language>> {
        let series = self.series;
        let mut languages: Vec<&'static Language> = parsed.languages.clone();
        if !release.languages.is_empty() {
            languages = release.languages.clone();
        } else {
            let mut tokens: Vec<char> = if parsed.release_tokens.is_empty() {
                parsed.release_title.chars().collect()
            } else {
                parsed.release_tokens.chars().collect()
            };
            let normalized_tokens = normalize_episode_title(&tokens.iter().collect::<String>());
            let mut to_remove: Vec<&'static Language> = Vec::new();
            for e in episodes {
                let title = e.title.as_deref().unwrap_or_default();
                let title_languages = parse_languages(title);
                if title_languages.iter().any(|l| l.id == 0) {
                    continue;
                }
                let normalized_title = normalize_episode_title(title);
                if let Some(i) = index_of_ignore_case(&normalized_tokens, &normalized_title) {
                    let len = normalized_title.chars().count();
                    if i + len > tokens.len() {
                        return None;
                    }
                    tokens.drain(i..i + len);
                    to_remove.extend(title_languages);
                }
            }
            let remaining = parse_languages(&tokens.iter().collect::<String>());
            let to_remove = except(&to_remove, &remaining);
            languages = except(&languages, &to_remove);
        }
        if !release.title.trim().is_empty()
            && !release.multi_languages.is_empty()
            && has_multiple_languages(&release.title)
        {
            let multi: Vec<&'static Language> = release
                .multi_languages
                .iter()
                .filter_map(|id| Language::by_id(*id))
                .collect();
            if languages.is_empty() || (languages.len() == 1 && languages[0].id == 0) {
                languages = multi;
            } else {
                let extra = except(&multi, &languages);
                languages.extend(extra);
            }
        }
        let original = series.original_language.unwrap_or_else(unknown_language);
        if languages.is_empty() || (languages.len() == 1 && languages[0].id == 0) {
            languages = vec![original];
        }
        if let Some(pos) = languages.iter().position(|l| l.id == -2) {
            languages.remove(pos);
            if languages.iter().any(|l| l.id == original.id) {
                languages.push(unknown_language());
            } else {
                languages.push(original);
            }
        }
        Some(languages)
    }
}
