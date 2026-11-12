//! Codec detection and codec-normalised sizes: Tanukarr's `CodecSizeNormalizer`.
//!
//! The decision value is the release size times its codec family's factor (the profile's
//! override when set, else the default). It is used by the size limits and by the
//! preferred-size ranking only; the actual size still drives downloads and free-space checks.

use std::sync::LazyLock;

use kitsunarr_domain::api::settings::QualityProfile;
use kitsunarr_domain::codec;
use kitsunarr_parser::ParsedEpisodeInfo;
use regex::Regex;

/// Codec tokens in titles; the letter/number boundary guards are checked by hand (the `regex`
/// crate has no look-around).
static TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?P<MPEG1>mpeg[ ._-]?1(?:video)?)|(?P<MPEG2>x262|h[ ._-]?262|mpeg[ ._-]?2(?:video)?)|(?P<MPEG4ASP>xvid(?:hd)?|divx|mpeg[ ._-]?4[ ._-]?asp|msmpeg4(?:v[123])?)|(?P<H263>h[ ._-]?263)|(?P<H264>x264|h[ ._-]?264|avc1?)|(?P<HEVC>x265|h[ ._-]?265|hevc|hvc1|hev1)|(?P<AV1>av1|av01|svt[ ._-]?av1|libaom[ ._-]?av1)|(?P<VVC>x266|h[ ._-]?266|vvc)|(?P<VC1>vc[ ._-]?1)|(?P<WMV1>wmv1)|(?P<WMV2>wmv2)|(?P<WMV3>wmv3)|(?P<VP6>vp6(?:f|a)?)|(?P<VP7>vp7)|(?P<VP8>vp8)|(?P<VP9>vp9)|(?P<Theora>theora)|(?P<Unknown>ffv1|huffyuv|ffvhuff|rawvideo|utvideo|prores|dnxhd|dnxhr|jpeg[ ._-]?2000|j2k)",
    )
    .expect("valid codec regex")
});

/// `StructuredCodec`: a media-info or indexer codec name.
pub fn structured(value: &str) -> Option<&'static str> {
    if value.trim().is_empty() || value.len() > 128 {
        return None;
    }
    let n: String = value
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '.' | '-' | '_'))
        .collect::<String>()
        .to_uppercase();
    let family = match n.as_str() {
        "MPEG1" | "MPEG1VIDEO" => Some("MPEG1"),
        "X262" | "H262" | "MPEG2" | "MPEG2VIDEO" => Some("MPEG2"),
        "XVID" | "XVIDHD" | "DIVX" | "DIV3" | "DX50" | "MPEG4" | "MPEG4ASP" | "MSMPEG4"
        | "MSMPEG4V1" | "MSMPEG4V2" | "MSMPEG4V3" => Some("MPEG4ASP"),
        "H263" => Some("H263"),
        "X264" | "H264" | "AVC" | "AVC1" | "LIBX264" => Some("H264"),
        "X265" | "H265" | "HEVC" | "HVC1" | "HEV1" | "LIBX265" => Some("HEVC"),
        "AV1" | "AV01" | "SVTAV1" | "LIBSVTAV1" | "LIBAOMAV1" => Some("AV1"),
        "X266" | "H266" | "VVC" | "LIBX266" => Some("VVC"),
        "VC1" => Some("VC1"),
        "WMV1" => Some("WMV1"),
        "WMV2" => Some("WMV2"),
        "WMV3" => Some("WMV3"),
        "VP6" | "VP6F" | "VP6A" => Some("VP6"),
        "VP7" => Some("VP7"),
        "VP8" => Some("VP8"),
        "VP9" => Some("VP9"),
        "THEORA" => Some("Theora"),
        "FFV1" | "HUFFYUV" | "FFVHUFF" | "RAWVIDEO" | "UTVIDEO" | "PRORES" | "DNXHD" | "DNXHR"
        | "JPEG2000" | "J2K" | "MIXED" | "MULTIPLE" | "MULTICODEC" | "MULTICODECS"
        | "MULTIPLECODECS" => Some("Unknown"),
        _ => None,
    };
    if family.is_some() {
        return family;
    }
    let parts: Vec<&str> = value
        .split(['/', '+', '&', '|', ',', ';'])
        .filter(|p| !p.trim().is_empty())
        .collect();
    if parts.len() < 2 {
        return None;
    }
    let mut families = std::collections::HashSet::new();
    for p in parts {
        match structured(p) {
            None | Some("Unknown") => return Some("Unknown"),
            Some(f) => {
                families.insert(f);
            }
        }
    }
    Some(if families.len() == 1 {
        families.into_iter().next().unwrap_or("Unknown")
    } else {
        "Unknown"
    })
}

/// `TitleCodec`: exactly one codec family named in the release tokens, else `Unknown`.
pub fn from_title(title: &str, parsed: &ParsedEpisodeInfo) -> &'static str {
    let mut tokens = if parsed.release_tokens.trim().is_empty() {
        let t = title.replace(['.', '_'], " ");
        let series = parsed.series_title.replace(['.', '_'], " ");
        if series.trim().is_empty() {
            t
        } else {
            replace_ignore_case(&t, &series)
        }
    } else {
        parsed.release_tokens.clone()
    };
    if tokens.len() > 16_384 {
        return "Unknown";
    }
    if let Some(group) = parsed
        .release_group
        .as_deref()
        .filter(|g| !g.trim().is_empty())
    {
        tokens = replace_ignore_case(&tokens, &format!("[{group}]"));
        let lower = tokens.to_lowercase();
        let suffix = format!("-{}", group.to_lowercase());
        let encoder_tag = lower.ends_with("svt-av1") || lower.ends_with("libaom-av1");
        if !encoder_tag && lower.ends_with(&suffix) {
            tokens.truncate(tokens.len() - suffix.len());
        }
    }
    let mut found = std::collections::HashSet::new();
    for caps in TOKENS.captures_iter(&tokens) {
        let m = caps.get(0).expect("whole match");
        let before = tokens[..m.start()].chars().next_back();
        let after = tokens[m.end()..].chars().next();
        if before.is_some_and(char::is_alphanumeric) || after.is_some_and(char::is_alphanumeric) {
            continue;
        }
        if let Some(f) = codec::FAMILIES.iter().find(|f| caps.name(f).is_some()) {
            found.insert(*f);
        }
    }
    if found.len() == 1 {
        found.into_iter().next().unwrap_or("Unknown")
    } else {
        "Unknown"
    }
}

fn replace_ignore_case(haystack: &str, needle: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let (lh, ln) = (haystack.to_lowercase(), needle.to_lowercase());
    // Lower-casing can change byte lengths outside ASCII; fall back to an exact replace then.
    if lh.len() != haystack.len() || ln.len() != needle.len() {
        return haystack.replace(needle, "");
    }
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while let Some(pos) = lh[i..].find(&ln) {
        out.push_str(&haystack[i..i + pos]);
        i += pos + ln.len();
    }
    out.push_str(&haystack[i..]);
    out
}

/// The factor of a family for a profile: the profile's override (0.01 to 10), else the default.
pub fn factor(profile: &QualityProfile, family: &str) -> f64 {
    profile
        .codec_multipliers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(family))
        .map(|(_, v)| v.0)
        .filter(|v| (0.01..=10.0).contains(v))
        .unwrap_or_else(|| codec::default_factor(family))
}

/// `Normalize`: (family, factor, normalised size in bytes).
pub fn normalize(
    profile: &QualityProfile,
    title: &str,
    parsed: &ParsedEpisodeInfo,
    size: i64,
) -> (&'static str, f64, i64) {
    let family = from_title(title, parsed);
    let m = factor(profile, family);
    (family, m, (size as f64 * m).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(title: &str) -> &'static str {
        let parsed = kitsunarr_parser::parse_title(title).unwrap_or_default();
        from_title(title, &parsed)
    }

    /// Cases after Tanukarr's `CodecNormalizedSizeFixture`.
    #[test]
    fn title_codecs() {
        assert_eq!(
            detect("Series.Title.S01E01.1080p.WEB-DL.x265-GROUP"),
            "HEVC"
        );
        assert_eq!(
            detect("Series.Title.S01E01.1080p.BluRay.H.264-GROUP"),
            "H264"
        );
        assert_eq!(
            detect("Series Title S01E01 1080p WEB AV1 Opus-GROUP"),
            "AV1"
        );
        assert_eq!(
            detect("Series.Title.S01E01.720p.HDTV.XviD-GROUP"),
            "MPEG4ASP"
        );
        // Two families named: unknown.
        assert_eq!(
            detect("Series.Title.S01E01.1080p.x264.x265-GROUP"),
            "Unknown"
        );
        // Inside a word: not a codec.
        assert_eq!(
            detect("Series.Title.S01E01.1080p.WEB-DL.DDP5.1.Hevcx-GROUP"),
            "Unknown"
        );
        assert_eq!(detect("Series.Title.S01E01.1080p.WEB-DL"), "Unknown");
    }

    #[test]
    fn structured_codecs() {
        assert_eq!(structured("x265"), Some("HEVC"));
        assert_eq!(structured("AVC"), Some("H264"));
        assert_eq!(structured("libsvtav1"), Some("AV1"));
        assert_eq!(structured("h264 / avc"), Some("H264"));
        assert_eq!(structured("h264 / hevc"), Some("Unknown"));
        assert_eq!(structured("something"), None);
    }

    #[test]
    fn defaults_cover_every_family() {
        assert_eq!(codec::DEFAULTS.len(), codec::FAMILIES.len());
        assert!(
            codec::FAMILIES
                .iter()
                .all(|f| codec::DEFAULTS.iter().any(|(d, _)| d == f))
        );
    }
}
