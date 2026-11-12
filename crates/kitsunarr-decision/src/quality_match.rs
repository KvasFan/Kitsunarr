//! Tanukarr's `ProfileQualityMatcher`: a release's parsed quality becomes the most specific
//! quality preset of the series' profile whose criteria it meets (source, resolution, codec
//! family, title pattern).

use kitsunarr_domain::api::settings::{QualityDefinition, QualityProfile, QualityProfileItem};
use kitsunarr_domain::quality::{Quality, QualitySource};
use kitsunarr_parser::ParsedEpisodeInfo;

fn item_ids(item: &QualityProfileItem) -> Vec<i32> {
    match (&item.quality_id, &item.group) {
        (Some(q), _) => vec![*q],
        (None, Some(g)) => g.quality_ids.clone(),
        _ => Vec::new(),
    }
}

/// The definition's source and resolution: its own (custom presets) or its quality's.
fn criteria_of(d: &QualityDefinition) -> Option<(QualitySource, i32)> {
    let q = Quality::by_id(d.quality_id)?;
    Some((
        d.source.unwrap_or(q.source),
        d.resolution.unwrap_or(q.resolution),
    ))
}

fn pattern_matches(pattern: &str, title: &str) -> bool {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .is_ok_and(|r| r.is_match(title))
}

fn meets(
    d: &QualityDefinition,
    source: QualitySource,
    resolution: i32,
    codec: &str,
    title: &str,
) -> bool {
    criteria_of(d) == Some((source, resolution))
        && (d.codec.is_empty() || d.codec.eq_ignore_ascii_case(codec))
        && (d.title_pattern.is_empty() || pattern_matches(&d.title_pattern, title))
}

/// `ProfileQualityMatcher.Match`.
pub fn match_quality(
    parsed_quality: &'static Quality,
    title: &str,
    parsed: &ParsedEpisodeInfo,
    profile: &QualityProfile,
    definitions: &[QualityDefinition],
) -> &'static Quality {
    if parsed_quality.is_custom() {
        return parsed_quality;
    }
    // Nothing to match against without presets or criteria.
    if !definitions.iter().any(|d| {
        d.quality_id >= kitsunarr_domain::quality::CUSTOM_QUALITY_BASE
            || !d.codec.is_empty()
            || !d.title_pattern.is_empty()
            || d.is_retired
    }) {
        return parsed_quality;
    }
    let codec = crate::codec::from_title(title, parsed);
    let (source, resolution) = (parsed_quality.source, parsed_quality.resolution);
    // Allowed qualities of the profile with their index (higher: more preferred).
    let allowed: Vec<(i32, usize)> = profile
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.allowed)
        .flat_map(|(n, i)| item_ids(i).into_iter().map(move |id| (id, n)))
        .collect();
    let best = definitions
        .iter()
        .filter(|d| !d.is_retired)
        .filter_map(|d| {
            allowed
                .iter()
                .find(|(id, _)| *id == d.quality_id)
                .map(|(_, index)| (d, *index))
        })
        .filter(|(d, _)| meets(d, source, resolution, codec, title))
        .max_by_key(|(d, index)| {
            (
                usize::from(!d.codec.is_empty()) + usize::from(!d.title_pattern.is_empty()),
                d.quality_id >= kitsunarr_domain::quality::CUSTOM_QUALITY_BASE,
                *index,
            )
        });
    if let Some(q) = best.and_then(|(d, _)| Quality::by_id(d.quality_id)) {
        return q;
    }
    // No preset matched: the parsed quality stays unless its own definition is retired or
    // has criteria the release fails.
    match definitions
        .iter()
        .find(|d| d.quality_id == parsed_quality.id)
    {
        Some(own)
            if own.is_retired
                || ((!own.codec.is_empty() || !own.title_pattern.is_empty())
                    && !meets(own, source, resolution, codec, title)) =>
        {
            Quality::unknown()
        }
        _ => parsed_quality,
    }
}

#[cfg(test)]
mod tests {
    use kitsunarr_domain::api::settings::QualityDefinition;
    use kitsunarr_domain::quality::{Quality, QualitySource};

    use super::*;

    fn profile(allowed: &[i32]) -> QualityProfile {
        QualityProfile {
            id: 1,
            name: "P".into(),
            upgrade_allowed: true,
            cutoff: allowed[0],
            items: allowed
                .iter()
                .map(|id| QualityProfileItem {
                    quality_id: Some(*id),
                    group: None,
                    allowed: true,
                })
                .collect(),
            min_format_score: 0,
            cutoff_format_score: 0,
            min_upgrade_format_score: 0,
            format_items: Vec::new(),
            size_limits: Vec::new(),
            codec_multipliers: Default::default(),
        }
    }

    fn def(id: i32, codec: &str, pattern: &str) -> QualityDefinition {
        QualityDefinition {
            quality_id: id,
            title: Quality::by_id(id).map_or("custom".into(), |q| q.name.into()),
            codec: codec.into(),
            title_pattern: pattern.into(),
            ..Default::default()
        }
    }

    /// Cases after Tanukarr's `ProfileQualityMatcherFixture`.
    #[test]
    fn presets_match_by_codec_and_pattern() {
        let custom = Quality::register(10_500, "WEBDL-1080p HEVC", QualitySource::Web, 1080);
        let mut hevc = def(10_500, "HEVC", "");
        hevc.source = Some(QualitySource::Web);
        hevc.resolution = Some(1080);
        let webdl = Quality::by_id(3).unwrap();
        let defs = vec![def(3, "", ""), hevc];
        let parse = |t: &str| kitsunarr_parser::parse_title(t).unwrap();
        let title = "Show.S01E01.1080p.WEB-DL.x265-GRP";
        let m = |t: &str, p: &QualityProfile, d: &[QualityDefinition]| {
            match_quality(webdl, t, &parse(t), p, d).id
        };
        // Allowed in the profile: the HEVC release takes the preset.
        assert_eq!(m(title, &profile(&[3, 10_500]), &defs), custom.id);
        // An x264 release keeps WEBDL-1080p.
        assert_eq!(
            m(
                "Show.S01E01.1080p.WEB-DL.x264-GRP",
                &profile(&[3, 10_500]),
                &defs
            ),
            3
        );
        // Not allowed in the profile: no match.
        assert_eq!(m(title, &profile(&[3]), &defs), 3);
        // A standard quality edited to require HEVC turns other releases into Unknown.
        let strict = vec![def(3, "HEVC", "")];
        assert_eq!(
            m("Show.S01E01.1080p.WEB-DL.x264-GRP", &profile(&[3]), &strict),
            0
        );
        // Title patterns, case-insensitive.
        let mut amzn = def(3, "", "");
        amzn.title_pattern = r"\bamzn\b".into();
        assert_eq!(
            m(
                "Show.S01E01.1080p.AMZN.WEB-DL.x264-GRP",
                &profile(&[3]),
                &[amzn.clone()]
            ),
            3
        );
        assert_eq!(m(title, &profile(&[3]), &[amzn]), 0);
        // Retired: Unknown.
        let mut retired = def(3, "", "");
        retired.is_retired = true;
        assert_eq!(m(title, &profile(&[3]), &[retired]), 0);
    }
}
