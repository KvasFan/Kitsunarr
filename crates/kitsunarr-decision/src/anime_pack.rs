//! Tanukarr's `AnimePackMatcher`: in an anime season search, `[Group] Title 01-12 Complete`
//! (or `Batch`) is the whole season when it names exactly the season's episode count, all of
//! which have aired. It applies when the regular parser reads neither a season nor episodes.

use std::sync::LazyLock;

use kitsunarr_parser::netregex::NetRegex;
use kitsunarr_parser::{ParsedEpisodeInfo, parse_languages, parse_quality, parse_release_group};

use crate::model::Series;

static GROUP: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"^\s*\[[^\]]+\]\s*", false));
static RANGE: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"^0*1\s+0*(?<end>\d+)\s+(?:complete|batch)\b", false));
static EXTRA: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(r"\b(?:ova|oad|specials?|movies?|part|season|s\d+)\b", false)
});

/// Lower case, every run of non-alphanumerics a single space.
fn normalize(value: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in value.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim().to_string()
}

/// `AnimePackMatcher.Match` for `season` of `series`.
pub fn match_pack(
    title: &str,
    series: &Series,
    season: i32,
    season_titles: &[String],
) -> Option<ParsedEpisodeInfo> {
    if season <= 0 {
        return None;
    }
    let mut episodes: Vec<_> = series
        .episodes
        .iter()
        .filter(|e| e.season_number == season)
        .collect();
    episodes.sort_by_key(|e| e.episode_number);
    let now = chrono::Utc::now();
    if episodes.is_empty()
        || episodes
            .iter()
            .enumerate()
            .any(|(i, e)| e.episode_number != i as i32 + 1)
        || episodes
            .iter()
            .any(|e| e.air_date_utc.is_none_or(|a| a > now))
    {
        return None;
    }
    let normalized = normalize(&GROUP.replace(title, ""));
    let mut aliases: Vec<String> = season_titles.to_vec();
    let regular_seasons: std::collections::BTreeSet<i32> = series
        .episodes
        .iter()
        .map(|e| e.season_number)
        .filter(|s| *s > 0)
        .collect();
    if regular_seasons.len() == 1 {
        aliases.extend(series.alternate_titles.iter().cloned());
        aliases.push(series.title.clone());
    }
    aliases.retain(|a| !a.trim().is_empty());
    aliases.sort_by_key(|a| std::cmp::Reverse(a.chars().count()));
    for alias in aliases {
        let prefix = normalize(&alias);
        let Some(suffix) = normalized.strip_prefix(&format!("{prefix} ")) else {
            continue;
        };
        let Some(m) = RANGE.find(suffix) else {
            continue;
        };
        if m.value("end").parse::<usize>().ok() != Some(episodes.len()) || EXTRA.is_match(suffix) {
            continue;
        }
        return Some(ParsedEpisodeInfo {
            series_title: series.title.clone(),
            release_title: title.to_string(),
            season_numbers: vec![season],
            full_season: true,
            quality: parse_quality(title),
            languages: parse_languages(title),
            release_group: parse_release_group(title),
            ..Default::default()
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Episode;

    fn anime(count: i32, aired: bool) -> Series {
        let mut s = crate::tests::series();
        s.title = "Sousou no Frieren".into();
        s.episodes = (1..=count)
            .map(|n| Episode {
                id: n as i64,
                season_number: 1,
                episode_number: n,
                absolute_episode_number: Some(n),
                scene_season_number: None,
                scene_episode_number: None,
                scene_absolute_episode_number: None,
                title: None,
                air_date: None,
                air_date_utc: Some(if aired {
                    chrono::Utc::now() - chrono::Duration::days(30)
                } else {
                    chrono::Utc::now() + chrono::Duration::days(30)
                }),
                runtime: 24,
                monitored: true,
                file: None,
            })
            .collect();
        s
    }

    /// Cases after Tanukarr's `AnimePackMatcherFixture`.
    #[test]
    fn full_season_batches_match() {
        let s = anime(28, true);
        let m = |t: &str| match_pack(t, &s, 1, &[]).map(|p| p.season_numbers);
        assert_eq!(
            m("[SubsPlease] Sousou no Frieren 01-28 Complete [1080p]"),
            Some(vec![1])
        );
        assert_eq!(
            m("[Grp] Sousou no Frieren (01-28) (Batch) (1080p)"),
            Some(vec![1])
        );
        assert_eq!(m("[Grp] Sousou no Frieren 01-12 Complete [1080p]"), None);
        assert_eq!(m("[Grp] Sousou no Frieren 01-28 [1080p]"), None);
        assert_eq!(m("[Grp] Sousou no Frieren OVA 01-28 Complete"), None);
        assert_eq!(m("[Grp] Other Show 01-28 Complete"), None);
        let p = match_pack(
            "[SubsPlease] Sousou no Frieren 01-28 Complete [1080p]",
            &s,
            1,
            &[],
        )
        .unwrap();
        assert!(p.full_season);
        assert_eq!(p.release_group.as_deref(), Some("SubsPlease"));
        // Not every episode has aired.
        assert_eq!(
            match_pack(
                "[Grp] Sousou no Frieren 01-28 Complete",
                &anime(28, false),
                1,
                &[]
            ),
            None
        );
    }
}
