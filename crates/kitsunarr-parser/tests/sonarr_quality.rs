//! Sonarr's quality parser tests (`QualityParserFixture`, `ExtendedQualityParserRegex`),
//! replayed against the port.

mod common;

use common::{Fixture, Outcome, b, enum_name, expect_eq, i, quality_name, s};
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
            return Some(verify(s(&a[0]), quality, b(&a[1])).into());
        }
        match method {
            // TestCaseSource: "My series S01E01 [<quality name>]" parses back to that quality.
            "parsing_our_own_quality_enum_name" => {
                let name = quality_name(enum_name(&a[0], "Quality"));
                let title = format!("My series S01E01 [{name}]");
                return Some(expect_eq("quality", parse_quality(&title).quality.name, name).into());
            }
            // TestCaseSource: other-source names with every separator.
            "should_parse_quality_from_other_source" => {
                let want = quality_name(enum_name(&a[1], "Quality"));
                let result = ['-', '.', ' ', '_'].iter().try_for_each(|c| {
                    let title =
                        format!("My series S01E01 {}", s(&a[0]).replace(' ', &c.to_string()));
                    verify(&title, want, false).map_err(|e| format!("{title}: {e}"))
                });
                return Some(result.into());
            }
            _ => {}
        }
        let title = s(&a[0]);
        let result: Result<(), String> = match method {
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
        };
        Some(Outcome::from(result))
    });
}

#[test]
fn extended_quality_parser_regex() {
    Fixture::load("ExtendedQualityParserRegex").run(|method, a| {
        let r = parse_quality(s(&a[0]));
        let result = match method {
            "should_parse_reality_from_title" => {
                expect_eq("real", i64::from(r.revision.real), i(&a[1]))
            }
            "should_parse_version_from_title" => {
                expect_eq("version", i64::from(r.revision.version), i(&a[1]))
            }
            _ => return None,
        };
        Some(result.into())
    });
}
