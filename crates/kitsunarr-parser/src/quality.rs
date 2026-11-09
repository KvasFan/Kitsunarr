//! Quality parsing (`Parser/QualityParser.cs` and `Qualities/QualityFinder.cs`).
//!
//! A line-by-line port: the same regexes (rewritten only where .NET-only syntax required it,
//! as noted next to each pattern) and the same decision order.

use std::sync::LazyLock;

use kitsunarr_domain::quality::{QUALITIES, Revision};
use kitsunarr_domain::{Quality, QualitySource};
use serde::Serialize;

use crate::extensions::{
    contains_invalid_path_chars, last_extension, path_extension, quality_for_extension,
};
use crate::re::{Re, any_of, contains_ci, first_of, has};

/// Where a part of the quality was determined from (`QualityDetectionSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DetectionSource {
    #[default]
    Unknown,
    Name,
    Extension,
    MediaInfo,
}

/// A parsed quality with its revision and how each part was detected (`QualityModel`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedQuality {
    pub quality: &'static Quality,
    pub revision: Revision,
    pub source_detection: DetectionSource,
    pub resolution_detection: DetectionSource,
    pub revision_detection: DetectionSource,
}

impl ParsedQuality {
    /// `Quality.Unknown`, revision 1, nothing detected.
    pub fn unknown() -> Self {
        Self {
            quality: Quality::unknown(),
            revision: Revision::default(),
            source_detection: DetectionSource::Unknown,
            resolution_detection: DetectionSource::Unknown,
            revision_detection: DetectionSource::Unknown,
        }
    }

    pub fn model(&self) -> kitsunarr_domain::quality::QualityModel {
        kitsunarr_domain::quality::QualityModel {
            quality_id: self.quality.id,
            revision: self.revision,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    R360p,
    R480p,
    R540p,
    R576p,
    R720p,
    R1080p,
    R2160p,
    Unknown,
}

// The .NET original uses IgnorePatternWhitespace; the alternatives are joined without the layout
// whitespace (whitespace inside character classes is literal in .NET and kept). `\/` became `/`.
static SOURCE: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(concat!(
        r"\b(?:",
        r"(?<bluray>BluRay|Blu-Ray|HD-?DVD|BDMux|BD(?!$))|",
        r"(?<webdl>WEB[-_. ]DL(?:mux)?|WEBDL|AmazonHD|AmazonSD|iTunesHD|MaxdomeHD|NetflixU?HD|WebHD|HBOMaxHD|DisneyHD|[. ]WEB[. ](?:[xh][ .]?26[456]|AVC|HEVC|EAC3|DDP?[ .]?5[. ]1)|[. ](?-i:WEB)$|(?:720|1080|2160)p[-. ]WEB[-. ]|[-. ]WEB[-. ](?:720|1080|2160)p|\b\s/\sWEB\s/\s\b|(?:AMZN|ATVP|DP|NF)[. -]WEB[. -](?!Rip))|",
        r"(?<webrip>WebRip|Web-Rip|WEBMux)|",
        r"(?<hdtv>HDTV)|",
        r"(?<bdrip>BDRip|BDLight)|",
        r"(?<brrip>BRRip)|",
        r"(?<dvd>DVD|DVDRip|NTSC|PAL|xvidvd)|",
        r"(?<dsr>WS[-_. ]DSR|DSR)|",
        r"(?<pdtv>PDTV)|",
        r"(?<sdtv>SDTV)|",
        r"(?<tvrip>TVRip)",
        r")(?:\b|$|[ .])",
    ))
});
static RAW_HD: LazyLock<Re> = LazyLock::new(|| Re::ci(r"\b(?<rawhd>RawHD|Raw[-_. ]HD)\b"));
static MPEG2: LazyLock<Re> = LazyLock::new(|| Re::new(r"\b(?<mpeg2>MPEG[-_. ]?2)\b"));
static PROPER: LazyLock<Re> = LazyLock::new(|| Re::ci(r"\b(?<proper>proper)\b"));
static REPACK: LazyLock<Re> = LazyLock::new(|| Re::ci(r"\b(?<repack>repack\d?|rerip\d?)\b"));
// `version` is used five times in the original.
static VERSION: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(
        r"\d[-._ ]?v(?<version1>\d)[-._ ]|\[v(?<version2>\d)\]|repack(?<version3>\d)|rerip(?<version4>\d)|(?:480|576|720|1080|2160)p[._ ]v(?<version5>\d)",
    )
});
const VERSION_GROUPS: &[&str] = &["version1", "version2", "version3", "version4", "version5"];
static REAL: LazyLock<Re> = LazyLock::new(|| Re::new(r"\b(?<real>REAL)\b"));
static RESOLUTION: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(
        r"\b(?:(?<R360p>360p)|(?<R480p>480p|480i|640x480|848x480)|(?<R540p>540p)|(?<R576p>576p)|(?<R720p>720p|1280x720|960p)|(?<R1080p>1080p|1920x1080|1440p|FHD|1080i|4kto1080p)|(?<R2160p>2160p|3840x2160|4k[-_. ](?:UHD|HEVC|BD|H26[56])|(?:UHD|HEVC|BD|H26[56])[-_. ]4k))\b",
    )
});
// `R2160p` is used twice in the original.
static ALTERNATIVE_RESOLUTION: LazyLock<Re> =
    LazyLock::new(|| Re::ci(r"\b(?<R2160p>UHD)\b|(?<R2160p2>\[4K\])"));
static CODEC: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(r"\b(?:(?<x264>x264)|(?<h264>h264)|(?<xvidhd>XvidHD)|(?<xvid>Xvid)|(?<divx>divx))\b")
});
static OTHER_SOURCE: LazyLock<Re> =
    LazyLock::new(|| Re::ci(r"(?<hdtv>HD[-_. ]TV)|(?<sdtv>SD[-_. ]TV)"));
static ANIME_BLURAY: LazyLock<Re> =
    LazyLock::new(|| Re::ci(r"bd(?:720|1080|2160)|(?<=[-_. (\[])bd(?=[-_. )\]])"));
static ANIME_WEBDL: LazyLock<Re> = LazyLock::new(|| Re::ci(r"\[WEB\]|[\[\(]WEB[ .]"));
static HIGH_DEF_PDTV: LazyLock<Re> = LazyLock::new(|| Re::ci(r"hr[-_. ]ws"));
// `remux` is used twice in the original; only `IsMatch` is needed.
static REMUX: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(
        r"(?:[_. ]|\d{4}p-|\bHybrid-)(?:(?:(BD|UHD)[-_. ]?)?Remux)\b|(?:(?:(BD|UHD)[-_. ]?)?Remux[_. ]\d{4}p)",
    )
});

fn q(name: &str) -> &'static Quality {
    Quality::by_name(name).unwrap_or_else(|| panic!("unknown quality {name}"))
}

/// `QualityParser.ParseQuality`: the quality of a release or file name.
pub fn parse_quality(name: &str) -> ParsedQuality {
    if name.trim().is_empty() {
        return ParsedQuality::unknown();
    }
    let name = name.trim();
    let mut result = parse_quality_name(name);
    if result.quality.id == 0 && !contains_invalid_path_chars(name) {
        result.quality = quality_for_extension(path_extension(name));
        result.source_detection = DetectionSource::Extension;
        result.resolution_detection = DetectionSource::Extension;
    }
    result
}

/// `QualityParser.ParseQualityName`: the quality from the name alone.
pub fn parse_quality_name(name: &str) -> ParsedQuality {
    let normalized = name.replace('_', " ");
    let normalized = normalized.trim();
    let mut result = parse_quality_modifiers(name, normalized);
    let set = |quality: &str| q(quality);

    if RAW_HD.is_match(normalized) {
        result.source_detection = DetectionSource::Name;
        result.resolution_detection = DetectionSource::Name;
        result.quality = set("Raw-HD");
        return result;
    }

    let source_matches = SOURCE.captures_all(normalized);
    let source = source_matches.last();
    let resolution = parse_resolution(normalized);
    let codec = CODEC.captures(normalized);
    let codec_has = |g: &str| codec.as_ref().is_some_and(|c| has(c, g));
    let remux = REMUX.is_match(normalized);

    if resolution != Resolution::Unknown {
        result.resolution_detection = DetectionSource::Name;
    }

    if let Some(src) = source {
        result.source_detection = DetectionSource::Name;
        let pick = |quality: &str, mut r: ParsedQuality| {
            r.quality = q(quality);
            r
        };

        if has(src, "bluray") {
            if codec_has("xvid") || codec_has("divx") {
                return pick("Bluray-480p", result);
            }
            return match resolution {
                Resolution::R2160p => pick(
                    if remux {
                        "Bluray-2160p Remux"
                    } else {
                        "Bluray-2160p"
                    },
                    result,
                ),
                Resolution::R1080p => pick(
                    if remux {
                        "Bluray-1080p Remux"
                    } else {
                        "Bluray-1080p"
                    },
                    result,
                ),
                Resolution::R576p => pick("Bluray-576p", result),
                Resolution::R360p | Resolution::R480p | Resolution::R540p => {
                    pick("Bluray-480p", result)
                }
                // A remux without a resolution is treated as 1080p; a 720p remux as Bluray-720p.
                _ if remux && resolution != Resolution::R720p => pick("Bluray-1080p Remux", result),
                _ => pick("Bluray-720p", result),
            };
        }

        if has(src, "webdl") {
            return match resolution {
                Resolution::R2160p => pick("WEBDL-2160p", result),
                Resolution::R1080p => pick("WEBDL-1080p", result),
                Resolution::R720p => pick("WEBDL-720p", result),
                _ if name.contains("[WEBDL]") => pick("WEBDL-720p", result),
                _ => pick("WEBDL-480p", result),
            };
        }

        if has(src, "webrip") {
            return match resolution {
                Resolution::R2160p => pick("WEBRip-2160p", result),
                Resolution::R1080p => pick("WEBRip-1080p", result),
                Resolution::R720p => pick("WEBRip-720p", result),
                _ => pick("WEBRip-480p", result),
            };
        }

        if has(src, "hdtv") {
            if MPEG2.is_match(normalized) {
                return pick("Raw-HD", result);
            }
            return match resolution {
                Resolution::R2160p => pick("HDTV-2160p", result),
                Resolution::R1080p => pick("HDTV-1080p", result),
                Resolution::R720p => pick("HDTV-720p", result),
                _ if name.contains("[HDTV]") => pick("HDTV-720p", result),
                _ => pick("SDTV", result),
            };
        }

        if has(src, "bdrip") || has(src, "brrip") {
            return match resolution {
                Resolution::R720p => pick("Bluray-720p", result),
                Resolution::R1080p => pick("Bluray-1080p", result),
                Resolution::R2160p => pick("Bluray-2160p", result),
                _ => pick("Bluray-480p", result),
            };
        }

        if has(src, "dvd") {
            return pick("DVD", result);
        }

        if has(src, "pdtv") || has(src, "sdtv") || has(src, "dsr") || has(src, "tvrip") {
            if resolution == Resolution::R1080p || contains_ci(normalized, "1080p") {
                return pick("HDTV-1080p", result);
            }
            if resolution == Resolution::R720p || contains_ci(normalized, "720p") {
                return pick("HDTV-720p", result);
            }
            if HIGH_DEF_PDTV.is_match(normalized) {
                result.resolution_detection = DetectionSource::Name;
                return pick("HDTV-720p", result);
            }
            return pick("SDTV", result);
        }
    }

    if source.is_none() && remux && resolution != Resolution::Unknown {
        result.source_detection = DetectionSource::Unknown;
        match resolution {
            Resolution::R480p => {
                result.quality = set("Bluray-480p");
                return result;
            }
            Resolution::R720p => {
                result.quality = set("Bluray-720p");
                return result;
            }
            Resolution::R2160p => {
                result.quality = set("Bluray-2160p Remux");
                return result;
            }
            Resolution::R1080p => {
                result.quality = set("Bluray-1080p Remux");
                return result;
            }
            _ => {}
        }
    }

    let low_resolution = matches!(
        resolution,
        Resolution::R360p | Resolution::R480p | Resolution::R540p | Resolution::R576p
    );

    // Anime Bluray matching
    if ANIME_BLURAY.is_match(normalized) {
        result.source_detection = DetectionSource::Name;
        if low_resolution || contains_ci(normalized, "480p") {
            result.resolution_detection = DetectionSource::Name;
            result.quality = set("DVD");
            return result;
        }
        if resolution == Resolution::R1080p || contains_ci(normalized, "1080p") {
            result.resolution_detection = DetectionSource::Name;
            result.quality = set(if remux {
                "Bluray-1080p Remux"
            } else {
                "Bluray-1080p"
            });
            return result;
        }
        if resolution == Resolution::R2160p || contains_ci(normalized, "2160p") {
            result.resolution_detection = DetectionSource::Name;
            result.quality = set(if remux {
                "Bluray-2160p Remux"
            } else {
                "Bluray-2160p"
            });
            return result;
        }
        if remux && resolution != Resolution::R720p {
            result.quality = set("Bluray-1080p Remux");
            return result;
        }
        result.quality = set("Bluray-720p");
        return result;
    }

    if ANIME_WEBDL.is_match(normalized) {
        result.source_detection = DetectionSource::Name;
        if low_resolution || contains_ci(normalized, "480p") {
            result.resolution_detection = DetectionSource::Name;
            result.quality = set("WEBDL-480p");
            return result;
        }
        if resolution == Resolution::R1080p || contains_ci(normalized, "1080p") {
            result.resolution_detection = DetectionSource::Name;
            result.quality = set("WEBDL-1080p");
            return result;
        }
        if resolution == Resolution::R2160p || contains_ci(normalized, "2160p") {
            result.resolution_detection = DetectionSource::Name;
            result.quality = set("WEBDL-2160p");
            return result;
        }
        result.quality = set("WEBDL-720p");
        return result;
    }

    if resolution != Resolution::Unknown {
        let mut source = QualitySource::Unknown;
        if remux {
            result.source_detection = DetectionSource::Name;
            source = QualitySource::BlurayRaw;
        } else {
            let by_extension = quality_for_extension(last_extension(name));
            if by_extension.id != 0 {
                result.source_detection = DetectionSource::Extension;
                source = by_extension.source;
            }
        }
        let (fallback, height) = match resolution {
            Resolution::R2160p => ("HDTV-2160p", 2160),
            Resolution::R1080p => ("HDTV-1080p", 1080),
            Resolution::R720p => ("HDTV-720p", 720),
            _ => ("SDTV", 480),
        };
        result.resolution_detection = DetectionSource::Name;
        result.quality = if source == QualitySource::Unknown {
            set(fallback)
        } else {
            find_by_source_and_resolution(source, height)
        };
        return result;
    }

    if codec_has("x264") {
        result.quality = set("SDTV");
        return result;
    }

    if normalized.contains("848x480") {
        result.resolution_detection = DetectionSource::Name;
        if normalized.contains("dvd") {
            result.source_detection = DetectionSource::Name;
            result.quality = set("DVD");
        } else if contains_ci(normalized, "bluray") {
            result.source_detection = DetectionSource::Name;
            result.quality = set("Bluray-480p");
        } else {
            result.quality = set("SDTV");
        }
        return result;
    }

    for (pattern, bluray, other) in [
        ("1280x720", "Bluray-720p", "HDTV-720p"),
        ("1920x1080", "Bluray-1080p", "HDTV-1080p"),
    ] {
        if contains_ci(normalized, pattern) {
            result.resolution_detection = DetectionSource::Name;
            if contains_ci(normalized, "bluray") {
                result.source_detection = DetectionSource::Name;
                result.quality = set(bluray);
            } else {
                result.quality = set(other);
            }
            return result;
        }
    }

    for (pattern, quality) in [
        ("bluray720p", "Bluray-720p"),
        ("bluray1080p", "Bluray-1080p"),
        ("bluray2160p", "Bluray-2160p"),
    ] {
        if contains_ci(normalized, pattern) {
            result.source_detection = DetectionSource::Name;
            result.resolution_detection = DetectionSource::Name;
            result.quality = set(quality);
            return result;
        }
    }

    if let Some(other) = other_source_match(normalized) {
        result.source_detection = DetectionSource::Name;
        result.quality = other;
    }
    result
}

fn parse_resolution(name: &str) -> Resolution {
    let m = RESOLUTION.captures(name);
    let alternative = ALTERNATIVE_RESOLUTION.captures(name);
    if let Some(m) = &m {
        for (group, res) in [
            ("R360p", Resolution::R360p),
            ("R480p", Resolution::R480p),
            ("R540p", Resolution::R540p),
            ("R576p", Resolution::R576p),
            ("R720p", Resolution::R720p),
            ("R1080p", Resolution::R1080p),
            ("R2160p", Resolution::R2160p),
        ] {
            if has(m, group) {
                return res;
            }
        }
    }
    if alternative
        .as_ref()
        .is_some_and(|a| any_of(a, &["R2160p", "R2160p2"]))
    {
        return Resolution::R2160p;
    }
    Resolution::Unknown
}

fn other_source_match(name: &str) -> Option<&'static Quality> {
    let m = OTHER_SOURCE.captures(name)?;
    if has(&m, "sdtv") {
        return Some(q("SDTV"));
    }
    if has(&m, "hdtv") {
        return Some(q("HDTV-720p"));
    }
    None
}

fn parse_quality_modifiers(name: &str, normalized: &str) -> ParsedQuality {
    let mut result = ParsedQuality::unknown();
    let version = VERSION
        .captures(normalized)
        .and_then(|c| first_of(&c, VERSION_GROUPS).and_then(|m| m.as_str().parse::<i32>().ok()));
    if let Some(v) = version {
        result.revision.version = v;
        result.revision_detection = DetectionSource::Name;
    }
    if PROPER.is_match(normalized) {
        result.revision.version = version.map_or(2, |v| v + 1);
        result.revision_detection = DetectionSource::Name;
    }
    if REPACK.is_match(normalized) {
        result.revision.version = version.map_or(2, |v| v + 1);
        result.revision.is_repack = true;
        result.revision_detection = DetectionSource::Name;
    }
    // Upstream counts every "REAL" (case-sensitive) in the original name.
    let real = REAL.count(name);
    if real > 0 {
        result.revision.real = i32::try_from(real).unwrap_or(i32::MAX);
        result.revision_detection = DetectionSource::Name;
    }
    result
}

/// `QualityFinder.FindBySourceAndResolution`.
pub fn find_by_source_and_resolution(source: QualitySource, resolution: i32) -> &'static Quality {
    if let Some(exact) = QUALITIES
        .iter()
        .find(|q| q.source == source && q.resolution == resolution)
    {
        return exact;
    }
    // 576p Television/Web releases must not roll up to Bluray-576p.
    if resolution < 720 {
        match source {
            QualitySource::Television => return q("SDTV"),
            QualitySource::Web => return q("WEBDL-480p"),
            QualitySource::WebRip => return q("WEBRip-480p"),
            _ => {}
        }
    }
    let mut candidates: Vec<&'static Quality> = QUALITIES
        .iter()
        .filter(|q| q.resolution == resolution)
        .collect();
    candidates.sort_by_key(|q| q.source.sonarr_id());
    let nearest = candidates
        .into_iter()
        .find(|q| q.source.sonarr_id() >= source.sonarr_id())
        .unwrap_or_else(Quality::unknown);
    tracing::debug!(
        ?source,
        resolution,
        quality = nearest.name,
        "no exact quality; using nearest"
    );
    nearest
}
