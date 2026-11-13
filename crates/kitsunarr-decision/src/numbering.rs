//! Tanukarr's `EpisodeNumberingResolver`: on a series whose scene numbering differs from
//! TheTVDB's, a release's season/episode numbers can mean either. A distinctive episode title
//! in the release (or files of one pack whose titles agree) decides; a scene mapping with an
//! explicit origin (`tvdb`, `scene`) does too; otherwise the release is ambiguous and rejected
//! rather than grabbed or imported as the wrong episode.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use kitsunarr_parser::ParsedEpisodeInfo;
use regex::Regex;

use crate::model::{Episode, Series};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumberingConvention {
    Aired,
    Scene,
    Ambiguous,
}

/// What Sonarr's `SceneMapping` says about numbering: its origin (`tvdb`, `scene`, `mixed`,
/// `unknown`, …), the season it maps to, the scene season it names, and whether it is
/// TheXEM's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SceneMapping {
    pub origin: Option<String>,
    pub season_number: Option<i32>,
    pub scene_season_number: Option<i32>,
    pub xem: bool,
}

#[derive(Debug, Clone)]
pub struct NumberingResolution<'e> {
    pub episodes: Option<Vec<&'e Episode>>,
    pub convention: NumberingConvention,
    pub from_title: bool,
    pub needs_evidence: bool,
    pub rejection: Option<String>,
}

fn reject<'e>(message: &str) -> NumberingResolution<'e> {
    NumberingResolution {
        episodes: None,
        convention: NumberingConvention::Ambiguous,
        from_title: false,
        needs_evidence: false,
        rejection: Some(message.to_string()),
    }
}

static EPISODE_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\bS\d{1,2}[ ._-]*E\d{1,3}(?:(?:[ ._-]*E|-)\d{1,3})*|\b\d{1,2}x\d{1,3})")
        .expect("valid episode token regex")
});
static WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").expect("valid words regex"));
static TECHNICAL_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d{3,4}p|web|webrip|webdl|web dl|hdtv|bluray|blu ray|bdrip|dvdrip|dsnp|amzn|nf|x264|x265|h264|h265|av1|proper|repack|mkv|mp4|avi)(?: |$)")
        .expect("valid technical suffix regex")
});

fn normalize(text: &str) -> String {
    WORDS
        .replace_all(&text.to_lowercase(), " ")
        .trim()
        .to_string()
}

fn has_different_numbering(e: &Episode) -> bool {
    e.scene_season_number.is_some_and(|s| s != e.season_number)
        || e.scene_episode_number
            .is_some_and(|n| n != e.episode_number)
}

/// `FindTitleMatches`: episodes whose title is what follows the episode token, alone or before
/// technical words. Short and generic titles ("Pilot", "Part 1") are not evidence.
fn title_matches<'e>(release_title: &str, episodes: &'e [Episode]) -> Vec<&'e Episode> {
    let Some(token) = EPISODE_TOKEN.find(release_title) else {
        return Vec::new();
    };
    let suffix = normalize(&release_title[token.end()..]);
    episodes
        .iter()
        .filter(|e| {
            let title = normalize(e.title.as_deref().unwrap_or_default());
            if title.chars().count() < 6
                || title == "episode"
                || title == "unknown"
                || title.starts_with("episode ")
                || title.starts_with("part ")
            {
                return false;
            }
            if suffix == title {
                return true;
            }
            suffix
                .strip_prefix(&format!("{title} "))
                .is_some_and(|rest| TECHNICAL_SUFFIX.is_match(rest))
        })
        .collect()
}

/// Finds the scene mapping of a parse result (`FindSceneMapping`).
pub type MappingLookup<'a> = dyn Fn(&ParsedEpisodeInfo) -> Option<SceneMapping> + 'a;

/// `Resolve`: `None` when the release's numbers mean the same episodes either way (or the
/// series, or the release, is outside scene numbering).
pub fn resolve<'e>(
    series: &'e Series,
    parsed: &ParsedEpisodeInfo,
    mapping: Option<SceneMapping>,
    inferred: Option<NumberingConvention>,
    lookup: &MappingLookup<'_>,
) -> Option<NumberingResolution<'e>> {
    let season = parsed.season_numbers.first().copied();
    // Absolute anime, daily releases and specials keep their own paths.
    if !series.use_scene_numbering
        || parsed.is_absolute_numbering()
        || parsed.is_daily()
        || parsed.special
        || season.is_none_or(|s| s <= 0)
        || parsed.episode_numbers.contains(&0)
    {
        return None;
    }
    let season = season?;
    let all = &series.episodes;
    if !all.iter().any(has_different_numbering) {
        return None;
    }
    let mapping = mapping.or_else(|| lookup(parsed));
    if parsed.full_season && parsed.is_multi_season() {
        return resolve_multi_season(series, parsed, mapping, inferred, lookup);
    }
    let origin = mapping.as_ref().and_then(|m| m.origin.as_deref());

    // Explicit alias offsets of scene mappings are kept.
    let mut offset = match &mapping {
        Some(m) => match (m.season_number, m.scene_season_number) {
            (Some(s), Some(scene)) if s >= 0 && scene <= season => s - scene,
            _ => 0,
        },
        None => 0,
    };
    if let Some(m) = &mapping
        && origin != Some("tvdb")
        && m.xem
        && let Some(scene) = m.scene_season_number.filter(|s| *s >= 0)
        && season == 1
        && scene != 1
    {
        offset = scene - 1;
    }
    let seasons: Vec<i32> = parsed.season_numbers.iter().map(|s| s + offset).collect();
    let aired: Vec<&Episode> = all
        .iter()
        .filter(|e| {
            seasons.contains(&e.season_number)
                && (parsed.full_season || parsed.episode_numbers.contains(&e.episode_number))
        })
        .collect();
    let scene: Vec<&Episode> = all
        .iter()
        .filter(|e| {
            e.scene_season_number.is_some_and(|s| seasons.contains(&s))
                && (parsed.full_season
                    || e.scene_episode_number
                        .is_some_and(|n| parsed.episode_numbers.contains(&n)))
        })
        .collect();

    // No differing mapping at these numbers: normal parsing stands.
    if !aired
        .iter()
        .chain(scene.iter())
        .any(|e| has_different_numbering(e))
    {
        return None;
    }
    // A pack can hold the same episodes in both orders while its files still map differently;
    // only identical single-episode candidates pass.
    if !parsed.full_season {
        let a: HashSet<i64> = aired.iter().map(|e| e.id).collect();
        let s: HashSet<i64> = scene.iter().map(|e| e.id).collect();
        if a == s {
            return None;
        }
    }
    if inferred == Some(NumberingConvention::Ambiguous) {
        return Some(reject(
            "Conflicting episode titles identify different numbering conventions within this season pack",
        ));
    }
    if !parsed.full_season && parsed.episode_numbers.len() == 1 {
        let titles = title_matches(&parsed.release_title, all);
        if let [title] = titles.as_slice() {
            let in_aired = aired.iter().any(|e| e.id == title.id);
            let in_scene = scene.iter().any(|e| e.id == title.id);
            if in_aired != in_scene {
                let convention = if in_aired {
                    NumberingConvention::Aired
                } else {
                    NumberingConvention::Scene
                };
                if inferred.is_some_and(|i| i != convention) {
                    return Some(reject(
                        "Episode title conflicts with the numbering verified for this season pack",
                    ));
                }
                return Some(NumberingResolution {
                    episodes: Some(vec![*title]),
                    convention,
                    from_title: true,
                    needs_evidence: false,
                    rejection: None,
                });
            }
            if !in_aired && !in_scene {
                return Some(reject(
                    "Episode title conflicts with both scene and library episode numbers",
                ));
            }
        } else if titles.len() > 1 {
            return Some(reject(
                "Episode title is not unique; numbering requires manual verification",
            ));
        }
    }

    // Unknown and mixed origins are not evidence (`unknown:tvdb` included).
    let from_mapping = match origin {
        Some("tvdb") => Some(NumberingConvention::Aired),
        Some("scene") => Some(NumberingConvention::Scene),
        _ => None,
    };
    if let Some(selected) = inferred.or(from_mapping) {
        let aired_order = selected == NumberingConvention::Aired;
        let episodes = if aired_order { aired } else { scene };
        let complete = if parsed.full_season {
            seasons.iter().all(|s| {
                episodes.iter().any(|e| {
                    if aired_order {
                        e.season_number == *s
                    } else {
                        e.scene_season_number == Some(*s)
                    }
                })
            })
        } else {
            parsed.episode_numbers.iter().all(|n| {
                episodes.iter().any(|e| {
                    if aired_order {
                        e.episode_number == *n
                    } else {
                        e.scene_episode_number == Some(*n)
                    }
                })
            })
        };
        if !episodes.is_empty() && complete {
            return Some(NumberingResolution {
                episodes: Some(episodes),
                convention: selected,
                from_title: false,
                needs_evidence: false,
                rejection: None,
            });
        }
        return Some(reject(
            "Verified numbering convention does not identify every requested episode",
        ));
    }
    Some(NumberingResolution {
        episodes: None,
        convention: NumberingConvention::Ambiguous,
        from_title: false,
        needs_evidence: true,
        rejection: Some(
            "Ambiguous scene/library numbering: verify episode titles or select episodes manually"
                .into(),
        ),
    })
}

/// `ResolveMultiSeason`: each season of the pack on its own, with its own mapping.
fn resolve_multi_season<'e>(
    series: &'e Series,
    parsed: &ParsedEpisodeInfo,
    mapping: Option<SceneMapping>,
    inferred: Option<NumberingConvention>,
    lookup: &MappingLookup<'_>,
) -> Option<NumberingResolution<'e>> {
    let all = &series.episodes;
    let first = parsed.season_numbers.first().copied();
    let mut conventions: Vec<NumberingConvention> = Vec::new();
    let mut resolved: Vec<&'e Episode> = Vec::new();
    let mut missing_season = false;
    for &season in &parsed.season_numbers {
        let info = ParsedEpisodeInfo {
            series_title: parsed.series_title.clone(),
            release_title: parsed.release_title.clone(),
            season_numbers: vec![season],
            full_season: true,
            ..ParsedEpisodeInfo::default()
        };
        let own = if Some(season) == first {
            mapping.clone()
        } else {
            lookup(&info)
        };
        // A verified origin covers the pack; alias offsets belong to each season's mapping.
        let season_mapping = own.unwrap_or_else(|| SceneMapping {
            origin: mapping.as_ref().and_then(|m| m.origin.clone()),
            ..SceneMapping::default()
        });
        let resolution = resolve(
            series,
            &info,
            Some(season_mapping.clone()),
            inferred,
            lookup,
        );
        if let Some(r) = resolution {
            if r.rejection.is_some() {
                return Some(r);
            }
            conventions.push(r.convention);
            resolved.extend(r.episodes.unwrap_or_default());
            continue;
        }
        let mut mapped = season;
        if let (Some(s), Some(scene)) = (
            season_mapping.season_number,
            season_mapping.scene_season_number,
        ) && s >= 0
            && scene <= season
        {
            mapped += s - scene;
        }
        if season_mapping.origin.as_deref() != Some("tvdb")
            && season_mapping.xem
            && let Some(scene) = season_mapping.scene_season_number.filter(|s| *s >= 0)
            && season == 1
            && scene != 1
        {
            mapped = scene;
        }
        let mut episodes: Vec<&Episode> = if season_mapping.origin.as_deref() == Some("tvdb") {
            all.iter().filter(|e| e.season_number == mapped).collect()
        } else {
            all.iter()
                .filter(|e| e.scene_season_number == Some(mapped))
                .collect()
        };
        if episodes.is_empty() {
            episodes = all.iter().filter(|e| e.season_number == mapped).collect();
        }
        missing_season |= episodes.is_empty();
        resolved.extend(episodes);
    }
    if conventions.is_empty() {
        return None;
    }
    if missing_season {
        return Some(reject(
            "Verified numbering convention does not identify every requested season",
        ));
    }
    let mut seen = HashSet::new();
    resolved.retain(|e| seen.insert(e.id));
    let convention = if conventions.iter().all(|c| *c == conventions[0]) {
        conventions[0]
    } else {
        NumberingConvention::Ambiguous
    };
    Some(NumberingResolution {
        episodes: Some(resolved),
        convention,
        from_title: false,
        needs_evidence: false,
        rejection: None,
    })
}

/// `InferBatch`: the convention the files of one download establish per season, when their
/// titles identify at least two different episodes the same way (or conflict).
pub fn infer_batch(
    series: &Series,
    file_names: &[&str],
    lookup: &MappingLookup<'_>,
) -> HashMap<i32, NumberingConvention> {
    let mut result = HashMap::new();
    if !series.use_scene_numbering {
        return result;
    }
    let mut votes: HashMap<i32, Vec<(NumberingConvention, Vec<i64>)>> = HashMap::new();
    for name in file_names {
        let Some(parsed) = kitsunarr_parser::parse_title(name) else {
            continue;
        };
        let Some(r) = resolve(series, &parsed, None, None, lookup) else {
            continue;
        };
        if !r.from_title {
            continue;
        }
        let Some(&season) = parsed.season_numbers.first() else {
            continue;
        };
        votes.entry(season).or_default().push((
            r.convention,
            r.episodes
                .unwrap_or_default()
                .iter()
                .map(|e| e.id)
                .collect(),
        ));
    }
    for (season, v) in votes {
        if v.iter().any(|(c, _)| *c != v[0].0) {
            result.insert(season, NumberingConvention::Ambiguous);
        } else if v
            .iter()
            .flat_map(|(_, ids)| ids.iter())
            .collect::<HashSet<_>>()
            .len()
            >= 2
        {
            result.insert(season, v[0].0);
        }
    }
    result
}

/// A file being imported, for [`resolve_local`] (`LocalEpisode`).
pub struct LocalContext<'a> {
    pub existing_file: bool,
    pub scene_source: bool,
    /// The grabbed release (`DownloadClientEpisodeInfo`) and the folder
    /// (`FolderEpisodeInfo`).
    pub download_client_info: Option<&'a ParsedEpisodeInfo>,
    pub folder_info: Option<&'a ParsedEpisodeInfo>,
    pub batch: Option<&'a HashMap<i32, NumberingConvention>>,
}

/// `ResolveLocal`: a file of a download inherits the origin its release or folder has for the
/// file's season, and the pack's inferred convention.
pub fn resolve_local<'e>(
    series: &'e Series,
    local: &LocalContext<'_>,
    parsed: &ParsedEpisodeInfo,
    lookup: &MappingLookup<'_>,
) -> Option<NumberingResolution<'e>> {
    let season = parsed.season_numbers.first().copied();
    if local.existing_file || !local.scene_source || !series.use_scene_numbering {
        return None;
    }
    let season = season?;
    let mut mapping = lookup(parsed);
    let explicit = |m: &Option<SceneMapping>| {
        matches!(
            m.as_ref().and_then(|m| m.origin.as_deref()),
            Some("tvdb" | "scene")
        )
    };
    if !explicit(&mapping) {
        // A verified release-group convention covers files that omit the group, when the
        // file's season belongs to that pack.
        for context in [local.download_client_info, local.folder_info]
            .into_iter()
            .flatten()
        {
            if !context.season_numbers.contains(&season) {
                continue;
            }
            let info = ParsedEpisodeInfo {
                season_numbers: vec![season],
                ..context.clone()
            };
            let found = lookup(&info);
            if explicit(&found) {
                mapping = found;
                break;
            }
        }
    }
    let inferred = local.batch.and_then(|b| b.get(&season).copied());
    resolve(series, parsed, mapping, inferred, lookup)
}

/// Kitsunarr's scene mappings: TheXEM's names, each standing for one scene season.
pub fn xem_lookup(series: &Series) -> impl Fn(&ParsedEpisodeInfo) -> Option<SceneMapping> + '_ {
    move |parsed: &ParsedEpisodeInfo| {
        crate::mapping::scene_season_of_title(parsed, series).map(|s| SceneMapping {
            origin: None,
            season_number: None,
            scene_season_number: Some(s),
            xem: true,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Tanukarr's `EpisodeNumberingResolverFixture`.
    use super::*;

    fn episode(id: i64, season: i32, number: i32, ss: i32, sn: i32, title: &str) -> Episode {
        Episode {
            id,
            season_number: season,
            episode_number: number,
            absolute_episode_number: None,
            scene_season_number: Some(ss),
            scene_episode_number: Some(sn),
            scene_absolute_episode_number: None,
            title: Some(title.into()),
            air_date: None,
            air_date_utc: None,
            runtime: 30,
            monitored: true,
            file: None,
        }
    }

    fn series() -> Series {
        let mut s = crate::tests::series();
        s.id = 42;
        s.title = "Example Show".into();
        s.use_scene_numbering = true;
        s.episodes = vec![
            episode(51, 5, 1, 4, 1, "1600 Candles"),
            episode(52, 5, 2, 4, 2, "The One That Got Away"),
            episode(53, 5, 3, 4, 3, "One Little Word"),
            episode(61, 6, 1, 5, 1, "In Country... Club"),
            episode(62, 6, 2, 5, 2, "Moon Over Isla Island"),
            episode(63, 6, 3, 5, 3, "Home Adrone"),
            episode(71, 7, 1, 6, 1, "100 A.D."),
        ];
        s
    }

    fn none(_: &ParsedEpisodeInfo) -> Option<SceneMapping> {
        None
    }

    fn origin(o: &str) -> Option<SceneMapping> {
        Some(SceneMapping {
            origin: Some(o.into()),
            ..SceneMapping::default()
        })
    }

    fn parse(title: &str) -> ParsedEpisodeInfo {
        kitsunarr_parser::parse_title(title).unwrap()
    }

    fn run<'e>(
        s: &'e Series,
        title: &str,
        o: Option<&str>,
        inferred: Option<NumberingConvention>,
    ) -> Option<NumberingResolution<'e>> {
        resolve(s, &parse(title), o.and_then(origin), inferred, &none)
    }

    fn ids(r: &NumberingResolution<'_>) -> Vec<i64> {
        r.episodes.as_ref().unwrap().iter().map(|e| e.id).collect()
    }

    fn rejection(r: Option<NumberingResolution<'_>>) -> String {
        r.unwrap().rejection.unwrap_or_default()
    }

    #[test]
    fn should_resolve_distinctive_title_in_either_numbering() {
        let s = series();
        for (title, id) in [
            ("Example.Show.S05E01.1600.Candles.1080p.WEB.x265-Goki", 51),
            ("Example Show - S05E01 - 1600 Candles.mkv", 51),
            ("Example.Show.5x01.1600.Candles.1080p.mkv", 51),
            ("Example.Show.S05E01.In.Country.Club.720p.HDTV-Group", 61),
            ("Example.Show.S05E02.The.One.That.Got.Away.1080p.mkv", 52),
        ] {
            let r = run(&s, title, None, None).unwrap();
            assert_eq!(r.rejection, None, "{title}");
            assert!(r.from_title, "{title}");
            assert_eq!(ids(&r), [id], "{title}");
        }
    }

    #[test]
    fn title_should_override_a_wrong_group_assumption() {
        let s = series();
        let r = run(
            &s,
            "Example.Show.S05E01.1600.Candles.1080p.mkv",
            Some("scene"),
            None,
        );
        assert_eq!(ids(&r.unwrap()), [51]);
    }

    #[test]
    fn should_block_ambiguous_packs() {
        let s = series();
        for o in [None, Some("unknown"), Some("unknown:tvdb"), Some("mixed")] {
            let r = run(&s, "Example.Show.S05.1080p.x265-Unknown", o, None);
            assert!(rejection(r).contains("Ambiguous"), "{o:?}");
        }
    }

    #[test]
    fn should_preserve_explicit_release_conventions() {
        let s = series();
        for (o, season) in [("tvdb", 5), ("scene", 6)] {
            let r = run(&s, "Example.Show.S05.1080p.x265-Group", Some(o), None).unwrap();
            assert_eq!(r.rejection, None);
            let eps = r.episodes.unwrap();
            assert_eq!(eps.len(), 3);
            assert!(eps.iter().all(|e| e.season_number == season));
        }
    }

    #[test]
    fn should_not_guess_from_only_one_existing_numeric_candidate() {
        let s = series();
        assert!(!rejection(run(&s, "Example.Show.S07.1080p.x265-Unknown", None, None)).is_empty());
    }

    #[test]
    fn should_not_accept_an_incomplete_multi_episode_mapping() {
        let s = series();
        let r = run(&s, "Example.Show.S05E01E04.1080p.mkv", Some("tvdb"), None);
        assert!(!rejection(r).is_empty());
    }

    #[test]
    fn should_resolve_all_seasons_in_a_trusted_multi_season_pack() {
        let s = series();
        let r = run(
            &s,
            "Example.Show.S04-S05.1080p.x265-Group",
            Some("scene"),
            None,
        )
        .unwrap();
        let eps = r.episodes.unwrap();
        let mut seasons: Vec<i32> = eps.iter().map(|e| e.season_number).collect();
        seasons.dedup();
        assert_eq!(seasons, [5, 6]);
        assert_eq!(eps.len(), 6);
    }

    #[test]
    fn should_block_unknown_multi_season_numbering() {
        let s = series();
        let r = run(&s, "Example.Show.S05-S06.1080p.x265-Unknown", None, None);
        assert!(!rejection(r).is_empty());
    }

    #[test]
    fn should_resolve_each_trusted_season_alias_independently() {
        let s = series();
        let parsed = parse("Example.Show.S01-S02.1080p.mkv");
        let lookup = |p: &ParsedEpisodeInfo| {
            (p.season_numbers.first() == Some(&2)).then(|| SceneMapping {
                origin: Some("tvdb".into()),
                season_number: Some(6),
                scene_season_number: Some(2),
                xem: false,
            })
        };
        let first = SceneMapping {
            origin: Some("tvdb".into()),
            season_number: Some(5),
            scene_season_number: Some(1),
            xem: false,
        };
        let r = resolve(&s, &parsed, Some(first), None, &lookup).unwrap();
        assert_eq!(r.rejection, None);
        let mut seasons: Vec<i32> = r
            .episodes
            .unwrap()
            .iter()
            .map(|e| e.season_number)
            .collect();
        seasons.dedup();
        assert_eq!(seasons, [5, 6]);
    }

    #[test]
    fn should_leave_identical_candidate_sets_unchanged() {
        let mut s = series();
        for e in &mut s.episodes {
            e.scene_season_number = Some(e.season_number);
            e.scene_episode_number = Some(e.episode_number);
        }
        assert!(run(&s, "Example.Show.S05.1080p.x265-Unknown", None, None).is_none());
    }

    #[test]
    fn should_leave_unaffected_season_of_a_mapped_show_unchanged() {
        let mut s = series();
        s.episodes.push(episode(11, 1, 1, 1, 1, "Opening Story"));
        assert!(run(&s, "Example.Show.S01E01.1080p.mkv", None, None).is_none());
    }

    #[test]
    fn should_handle_episode_only_mapping_differences() {
        let mut s = series();
        s.episodes = vec![
            episode(11, 1, 1, 1, 2, "First Story"),
            episode(12, 1, 2, 1, 1, "Second Story"),
        ];
        let r = run(&s, "Example.Show.S01E01.Second.Story.1080p.mkv", None, None);
        assert_eq!(ids(&r.unwrap()), [12]);
    }

    #[test]
    fn should_preserve_special_daily_and_absolute_paths() {
        let s = series();
        for t in [
            "Example.Show.S00E01.1080p.mkv",
            "Example.Show.2026.09.30.1080p.mkv",
            "[Group] Example Show - 125 [1080p]",
        ] {
            assert!(run(&s, t, None, None).is_none(), "{t}");
        }
    }

    #[test]
    fn should_not_query_when_scene_numbering_is_off() {
        let mut s = series();
        s.use_scene_numbering = false;
        assert!(run(&s, "Example.Show.S05.1080p.mkv", None, None).is_none());
    }

    #[test]
    fn should_not_use_generic_or_short_titles_as_evidence() {
        for title in ["Pilot", "Part 1", "Episode 1", "400"] {
            let mut s = series();
            s.episodes[0].title = Some(title.into());
            let t = format!("Example.Show.S05E01.{title}.1080p.mkv");
            assert!(!rejection(run(&s, &t, None, None)).is_empty(), "{title}");
        }
    }

    #[test]
    fn should_not_match_title_from_series_name() {
        let s = series();
        assert!(!rejection(run(&s, "1600.Candles.S05E01.1080p.mkv", None, None)).is_empty());
    }

    #[test]
    fn should_not_match_a_title_prefix_of_a_different_story() {
        let s = series();
        let r = run(
            &s,
            "Example.Show.S05E01.1600.Candles.Return.Again.1080p.mkv",
            None,
            None,
        );
        assert!(!rejection(r).is_empty());
    }

    #[test]
    fn should_block_duplicate_titles() {
        let mut s = series();
        s.episodes[3].title = s.episodes[0].title.clone();
        let r = run(
            &s,
            "Example.Show.S05E01.1600.Candles.1080p.mkv",
            Some("scene"),
            None,
        );
        assert!(rejection(r).contains("not unique"));
    }

    #[test]
    fn should_block_title_pointing_to_neither_numeric_interpretation() {
        let s = series();
        let r = run(
            &s,
            "Example.Show.S05E01.Home.Adrone.1080p.mkv",
            Some("scene"),
            None,
        );
        assert!(rejection(r).contains("both"));
    }

    #[test]
    fn two_distinct_titles_should_establish_a_pack_convention() {
        let s = series();
        let batch = infer_batch(
            &s,
            &[
                "Example.Show.S05E01.1600.Candles.1080p.mkv",
                "Example.Show.S05E02.The.One.That.Got.Away.1080p.mkv",
            ],
            &none,
        );
        assert_eq!(batch[&5], NumberingConvention::Aired);
        let r = run(&s, "Example.Show.S05E03.1080p.mkv", None, Some(batch[&5]));
        assert_eq!(ids(&r.unwrap()), [53]);
    }

    #[test]
    fn duplicate_copies_of_one_episode_should_not_establish_a_pack_convention() {
        let s = series();
        let batch = infer_batch(
            &s,
            &[
                "Example.Show.S05E01.1600.Candles.1080p.mkv",
                "Example.Show.S05E01.1600.Candles.720p.mkv",
            ],
            &none,
        );
        assert!(batch.is_empty());
    }

    #[test]
    fn conflicting_titles_should_block_the_season_including_titleless_files() {
        let s = series();
        let batch = infer_batch(
            &s,
            &[
                "Example.Show.S05E01.1600.Candles.1080p.mkv",
                "Example.Show.S05E02.Moon.Over.Isla.Island.1080p.mkv",
            ],
            &none,
        );
        assert_eq!(batch[&5], NumberingConvention::Ambiguous);
        let r = run(
            &s,
            "Example.Show.S05E03.1080p.mkv",
            Some("tvdb"),
            Some(batch[&5]),
        );
        assert!(rejection(r).contains("Conflicting"));
    }

    #[test]
    fn file_title_should_not_silently_conflict_with_inferred_convention() {
        let s = series();
        let r = run(
            &s,
            "Example.Show.S05E01.1600.Candles.1080p.mkv",
            None,
            Some(NumberingConvention::Scene),
        );
        assert!(rejection(r).contains("conflicts"));
    }

    #[test]
    fn library_scans_should_be_unchanged() {
        let s = series();
        let parsed = parse("Example.Show.S05E01.1080p.mkv");
        for (existing, scene) in [(true, true), (false, false)] {
            let local = LocalContext {
                existing_file: existing,
                scene_source: scene,
                download_client_info: None,
                folder_info: None,
                batch: None,
            };
            assert!(resolve_local(&s, &local, &parsed, &none).is_none());
        }
    }

    fn known_pack(
        pack: &ParsedEpisodeInfo,
        mappings: Vec<(i32, SceneMapping)>,
    ) -> impl Fn(&ParsedEpisodeInfo) -> Option<SceneMapping> + '_ {
        move |p: &ParsedEpisodeInfo| {
            if p.release_title != pack.release_title {
                return None;
            }
            let season = p.season_numbers.first()?;
            mappings
                .iter()
                .find(|(s, _)| s == season)
                .map(|(_, m)| m.clone())
        }
    }

    #[test]
    fn should_inherit_explicit_pack_group_for_untagged_files() {
        let s = series();
        let pack = parse("Example.Show.S05.1080p.x265-Known");
        let lookup = known_pack(&pack, vec![(5, origin("tvdb").unwrap())]);
        let local = LocalContext {
            existing_file: false,
            scene_source: true,
            download_client_info: Some(&pack),
            folder_info: None,
            batch: None,
        };
        let r = resolve_local(&s, &local, &parse("Example.Show.S05E01.1080p.mkv"), &lookup);
        assert_eq!(ids(&r.unwrap()), [51]);
    }

    #[test]
    fn should_not_inherit_an_unrelated_pack_season() {
        let s = series();
        let pack = parse("Example.Show.S06.1080p.x265-Known");
        let lookup = known_pack(&pack, vec![(6, origin("tvdb").unwrap())]);
        let local = LocalContext {
            existing_file: false,
            scene_source: true,
            download_client_info: Some(&pack),
            folder_info: None,
            batch: None,
        };
        let r = resolve_local(&s, &local, &parse("Example.Show.S05E01.1080p.mkv"), &lookup);
        assert!(!rejection(r).is_empty());
    }

    #[test]
    fn should_inherit_the_mapping_for_the_files_raw_season_in_a_multi_season_pack() {
        let s = series();
        let pack = parse("Example.Show.S04-S05.1080p.x265-Known");
        let lookup = known_pack(
            &pack,
            vec![
                (
                    4,
                    SceneMapping {
                        origin: Some("tvdb".into()),
                        season_number: Some(5),
                        scene_season_number: Some(4),
                        xem: false,
                    },
                ),
                (
                    5,
                    SceneMapping {
                        origin: Some("scene".into()),
                        season_number: Some(5),
                        scene_season_number: Some(5),
                        xem: false,
                    },
                ),
            ],
        );
        let local = LocalContext {
            existing_file: false,
            scene_source: true,
            download_client_info: Some(&pack),
            folder_info: None,
            batch: None,
        };
        let r =
            resolve_local(&s, &local, &parse("Example.Show.S05E01.1080p.mkv"), &lookup).unwrap();
        assert_eq!(r.rejection, None);
        assert_eq!(ids(&r), [61]);
        assert_eq!(r.convention, NumberingConvention::Scene);
    }

    #[test]
    fn should_reject_pack_with_a_missing_mapped_season() {
        let s = series();
        let r = run(
            &s,
            "Example.Show.S04-S08.1080p.x265-Group",
            Some("scene"),
            None,
        );
        assert!(!rejection(r).is_empty());
    }

    #[test]
    fn batch_inference_should_not_leak_to_a_different_season() {
        let s = series();
        let batch = HashMap::from([(5, NumberingConvention::Aired)]);
        let local = LocalContext {
            existing_file: false,
            scene_source: true,
            download_client_info: None,
            folder_info: None,
            batch: Some(&batch),
        };
        let r = resolve_local(&s, &local, &parse("Example.Show.S06E01.1080p.mkv"), &none);
        assert!(!rejection(r).is_empty());
    }

    #[test]
    fn should_preserve_explicit_season_alias_offsets() {
        let s = series();
        let mapping = SceneMapping {
            origin: Some("tvdb".into()),
            season_number: Some(5),
            scene_season_number: Some(1),
            xem: false,
        };
        let r = resolve(
            &s,
            &parse("Example.Show.S01E01.1080p.mkv"),
            Some(mapping),
            None,
            &none,
        );
        assert_eq!(ids(&r.unwrap()), [51]);
    }

    #[test]
    fn should_preserve_xem_alias_for_first_season() {
        let s = series();
        let mapping = SceneMapping {
            origin: Some("scene".into()),
            season_number: None,
            scene_season_number: Some(5),
            xem: true,
        };
        let r = resolve(
            &s,
            &parse("Example.Show.S01E01.1080p.mkv"),
            Some(mapping),
            None,
            &none,
        );
        assert_eq!(ids(&r.unwrap()), [61]);
    }

    #[test]
    fn equal_pack_coverage_should_not_hide_differing_file_numbering() {
        let mut s = series();
        s.episodes = vec![
            episode(11, 1, 1, 1, 2, "First Story"),
            episode(12, 1, 2, 1, 1, "Second Story"),
        ];
        assert!(!rejection(run(&s, "Example.Show.S01.1080p-Unknown", None, None)).is_empty());
        let r = run(&s, "Example.Show.S01.1080p-Known", Some("tvdb"), None).unwrap();
        assert_eq!(r.episodes.unwrap().len(), 2);
    }
}
