//! Sonarr's quality parser tests (`QualityParserFixture`, `ExtendedQualityParserRegex`),
//! replayed against the port.

mod common;

use common::{Fixture, b, expect_eq, i, s};
use kitsunarr_parser::{DetectionSource, parse_quality};

/// Quality expected by each `should_parse_*_quality(title, proper)` method.
fn expected_quality(method: &str) -> Option<&'static str> {
    Some(match method {
        "should_parse_sdtv_quality" => "SDTV",
        "should_parse_dvd_quality" => "DVD",
        "should_parse_webdl480p_quality" => "WEBDL-480p",
        "should_parse_bluray480p_quality" => "Bluray-480p",
        "should_parse_webrip480p_quality" => "WEBRip-480p",
        "should_parse_bluray576p_quality" => "Bluray-576p",
        "should_parse_hdtv720p_quality" => "HDTV-720p",
        "should_parse_hdtv1080p_quality" => "HDTV-1080p",
        "should_parse_hdtv2160p_quality" => "HDTV-2160p",
        "should_parse_webdl720p_quality" => "WEBDL-720p",
        "should_parse_webrip720p_quality" => "WEBRip-720p",
        "should_parse_webdl1080p_quality" => "WEBDL-1080p",
        "should_parse_webrip1080p_quality" => "WEBRip-1080p",
        "should_parse_webdl2160p_quality" => "WEBDL-2160p",
        "should_parse_webrip2160p_quality" => "WEBRip-2160p",
        "should_parse_bluray720p_quality" => "Bluray-720p",
        "should_parse_bluray1080p_quality" => "Bluray-1080p",
        "should_parse_bluray1080p_remux_quality" => "Bluray-1080p Remux",
        "should_parse_bluray2160p_quality" => "Bluray-2160p",
        "should_parse_bluray2160p_remux_quality" => "Bluray-2160p Remux",
        "should_parse_raw_quality" => "Raw-HD",
        "quality_parse" => "Unknown",
        _ => return None,
    })
}

/// `ParseAndVerifyQuality`: quality and `version == (proper ? 2 : 1)`.
fn verify(title: &str, quality: &str, proper: bool) -> Result<(), String> {
    let r = parse_quality(title);
    expect_eq("quality", r.quality.name, quality)?;
    expect_eq("version", r.revision.version, if proper { 2 } else { 1 })
}

fn detection(
    title: &str,
    source: DetectionSource,
    resolution: DetectionSource,
) -> Result<(), String> {
    let r = parse_quality(title);
    expect_eq("source detection", r.source_detection, source)?;
    expect_eq("resolution detection", r.resolution_detection, resolution)
}

#[test]
fn quality_parser_fixture() {
    use DetectionSource::{Extension, Name, Unknown};
    Fixture::load("QualityParserFixture").run(|method, a| {
        if let Some(quality) = expected_quality(method) {
            return Some(verify(s(&a[0]), quality, b(&a[1])));
        }
        let title = s(&a[0]);
        Some(match method {
            "should_parse_full_quality_from_name" => detection(title, Name, Name),
            "should_parse_resolution_from_name" => detection(title, Unknown, Name),
            "should_parse_source_from_name" => detection(title, Name, Unknown),
            "should_parse_quality_from_extension" => detection(title, Extension, Extension),
            "should_parse_resolution_from_name_and_source_from_extension" => {
                detection(title, Extension, Name)
            }
            "should_be_able_to_parse_repack" => {
                let r = parse_quality(title);
                expect_eq("version", i64::from(r.revision.version), i(&a[2])).and(expect_eq(
                    "repack",
                    r.revision.is_repack,
                    b(&a[1]),
                ))
            }
            "should_be_able_to_parse_anime_version" => expect_eq(
                "version",
                i64::from(parse_quality(title).revision.version),
                i(&a[1]),
            ),
            _ => return None,
        })
    });
}

/// `parsing_our_own_quality_enum_name` (a `TestCaseSource`, not in the extracted corpus).
#[test]
fn parses_its_own_quality_names() {
    for name in [
        "SDTV",
        "DVD",
        "WEBDL-480p",
        "Bluray-480p",
        "Bluray-576p",
        "HDTV-720p",
        "HDTV-1080p",
        "HDTV-2160p",
        "WEBDL-720p",
        "WEBDL-1080p",
        "WEBDL-2160p",
        "Bluray-720p",
        "Bluray-1080p",
        "Bluray-2160p",
        "Bluray-1080p Remux",
        "Bluray-2160p Remux",
    ] {
        let title = format!("My series S01E01 [{name}]");
        assert_eq!(parse_quality(&title).quality.name, name, "{title}");
    }
}

/// `should_parse_quality_from_other_source` (a `TestCaseSource`).
#[test]
fn parses_quality_from_other_sources() {
    for (text, quality) in [
        ("SD TV", "SDTV"),
        ("SD DVD", "DVD"),
        ("480p WEB-DL", "WEBDL-480p"),
        ("HD TV", "HDTV-720p"),
        ("1080p HD TV", "HDTV-1080p"),
        ("2160p HD TV", "HDTV-2160p"),
        ("720p WEB-DL", "WEBDL-720p"),
        ("1080p WEB-DL", "WEBDL-1080p"),
        ("2160p WEB-DL", "WEBDL-2160p"),
        ("720p BluRay", "Bluray-720p"),
        ("1080p BluRay", "Bluray-1080p"),
        ("2160p BluRay", "Bluray-2160p"),
        ("1080p Remux", "Bluray-1080p Remux"),
        ("2160p Remux", "Bluray-2160p Remux"),
    ] {
        for c in ['-', '.', ' ', '_'] {
            let title = format!("My series S01E01 {}", text.replace(' ', &c.to_string()));
            verify(&title, quality, false).unwrap_or_else(|e| panic!("{title}: {e}"));
        }
    }
}

#[test]
fn extended_quality_parser_regex() {
    Fixture::load("ExtendedQualityParserRegex").run(|method, a| {
        let r = parse_quality(s(&a[0]));
        Some(match method {
            "should_parse_reality_from_title" => {
                expect_eq("real", i64::from(r.revision.real), i(&a[1]))
            }
            "should_parse_version_from_title" => {
                expect_eq("version", i64::from(r.revision.version), i(&a[1]))
            }
            _ => return None,
        })
    });
}
