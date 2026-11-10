//! Custom format specifications: Sonarr's `CustomFormats/Specifications` fixtures, ported, plus
//! the grouping rules of `SpecificationMatchesGroup`.

mod common;

use common::{format, spec};
use kitsunarr_decision::{CustomFormatInput, compile_formats};
use kitsunarr_domain::Language;
use kitsunarr_domain::quality::Quality;
use kitsunarr_parser::{ReleaseType, parse_title};
use serde_json::json;

fn lang(name: &str) -> &'static Language {
    Language::by_name(name).unwrap()
}

fn input<'a>(languages: &'a [&'static Language]) -> CustomFormatInput<'a> {
    CustomFormatInput {
        release_title: "",
        file_name: Some("Series.Title.S01E01"),
        release_group: None,
        quality: Quality::unknown(),
        size: 100 * 1024 * 1024,
        indexer_flags: 0,
        languages,
        release_type: ReleaseType::Unknown,
        original_language: Some(lang("English")),
    }
}

/// One language specification, evaluated as Sonarr's fixture does.
fn language(value: &str, except: bool, negate: bool, languages: &[&'static Language]) -> bool {
    let mut s = spec(
        "LanguageSpecification",
        json!({"value": lang(value).id, "exceptLanguage": except}),
    );
    s.negate = negate;
    let (formats, problems) = compile_formats(&[format(1, "F", vec![s])]);
    assert!(problems.is_empty());
    formats[0].matches(&input(languages))
}

#[test]
fn single_language_fixture() {
    let french = [lang("French")];
    assert!(
        language("French", false, false, &french),
        "should_match_same_language"
    );
    assert!(
        !language("Spanish", false, false, &french),
        "should_not_match_different_language"
    );
    assert!(
        !language("French", false, true, &french),
        "should_not_match_negated_same_language"
    );
    assert!(
        language("Spanish", false, true, &french),
        "should_match_negated_different_language"
    );
    assert!(
        language("French", true, true, &french),
        "should_match_negated_except_language_if_language_is_only_present_language"
    );
}

#[test]
fn original_language_fixture() {
    let english = [lang("English")];
    let french = [lang("French")];
    assert!(
        language("Original", false, false, &english),
        "should_match_same_single_language"
    );
    assert!(
        !language("Original", false, false, &french),
        "should_not_match_different_single_language"
    );
    assert!(
        !language("Original", false, true, &english),
        "should_not_match_negated_same_single_language"
    );
    assert!(
        language("Original", false, true, &french),
        "should_match_negated_different_single_language"
    );
}

#[test]
fn multi_language_fixture() {
    let both = [lang("English"), lang("French")];
    assert!(
        language("French", false, false, &both),
        "should_match_one_language"
    );
    assert!(
        language("French", true, false, &both),
        "should_match_language_if_other_languages_are_present"
    );
    assert!(
        language("Original", true, false, &both),
        "should_match_language_if_not_original_language_is_present"
    );
    assert!(
        !language("Spanish", false, false, &both),
        "should_not_match_different_language"
    );
    assert!(
        !language("French", false, true, &both),
        "should_not_match_negated_when_one_language_matches"
    );
    assert!(
        language("Spanish", false, true, &both),
        "should_not_match_negated_when_all_languages_do_not_match"
    );
    assert!(
        !language("Spanish", true, true, &both),
        "should_not_match_negate_language_if_other_languages_are_present"
    );
}

fn release_type(value: i64, title: &str) -> bool {
    let parsed = parse_title(title).unwrap();
    let (formats, _) = compile_formats(&[format(
        1,
        "F",
        vec![spec("ReleaseTypeSpecification", json!({"value": value}))],
    )]);
    let languages = [];
    formats[0].matches(&CustomFormatInput {
        release_type: parsed.release_type(),
        ..input(&languages)
    })
}

#[test]
fn release_type_fixture() {
    let (season_pack, multi_season_pack) = (3, 4);
    assert!(release_type(
        multi_season_pack,
        "Series.Title.S01-S03.1080p.BluRay.x264-RlsGrp"
    ));
    assert!(!release_type(
        multi_season_pack,
        "Series.Title.S01.1080p.BluRay.x264-RlsGrp"
    ));
    assert!(!release_type(
        season_pack,
        "Series.Title.S01-S03.1080p.BluRay.x264-RlsGrp"
    ));
    assert!(release_type(
        season_pack,
        "Series.Title.S01.1080p.BluRay.x264-RlsGrp"
    ));
}

/// `SpecificationMatchesGroup.DidMatch`: within one implementation any member may match, a
/// required member must match; every implementation group must match.
#[test]
fn specification_groups_follow_upstream_rules() {
    let title = |pattern: &str, required: bool, negate: bool| {
        let mut s = spec("ReleaseTitleSpecification", json!({"value": pattern}));
        s.required = required;
        s.negate = negate;
        s
    };
    let check = |specs, release_title: &str| {
        let (formats, problems) = compile_formats(&[format(1, "F", specs)]);
        assert!(problems.is_empty(), "{problems:?}");
        let languages = [];
        formats[0].matches(&CustomFormatInput {
            release_title,
            file_name: None,
            ..input(&languages)
        })
    };
    let x265 = "Show.S01E01.1080p.WEB-DL.x265-GRP";
    // Either member of one group.
    assert!(check(
        vec![title("x265", false, false), title("HEVC", false, false)],
        x265
    ));
    // A required member that fails sinks the group.
    assert!(!check(
        vec![title("x265", false, false), title("HEVC", true, false)],
        x265
    ));
    // Negated: satisfied when the pattern is absent.
    assert!(check(vec![title("x264", false, true)], x265));
    // Case-insensitive, .NET syntax (lookbehind of any length).
    assert!(check(
        vec![title(r"(?<=1080p\.web-dl\.)X265", false, false)],
        x265
    ));
    // Two groups must both match.
    let size = spec("SizeSpecification", json!({"min": 0, "max": 1}));
    let mut big = spec("SizeSpecification", json!({"min": 1, "max": 100}));
    big.required = true;
    assert!(check(vec![title("x265", false, false), size], x265));
    assert!(!check(vec![title("x265", false, false), big], x265));
}

#[test]
fn unusable_specifications_are_reported_and_never_match() {
    let (formats, problems) = compile_formats(&[
        format(
            1,
            "Broken",
            vec![spec(
                "ReleaseTitleSpecification",
                json!({"value": "(unclosed"}),
            )],
        ),
        format(
            2,
            "Packs",
            vec![spec("SeasonPackSpecification", json!({"value": 1}))],
        ),
    ]);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems[0].contains("Broken") && problems[1].contains("SeasonPackSpecification"));
    let languages = [];
    assert!(formats.iter().all(|f| !f.matches(&input(&languages))));
}

#[test]
fn source_resolution_flags_and_size_use_sonarr_values() {
    let bluray1080 = Quality::by_name("Bluray-1080p").unwrap();
    let check = |s| {
        let (formats, _) = compile_formats(&[format(1, "F", vec![s])]);
        let languages = [];
        formats[0].matches(&CustomFormatInput {
            quality: bluray1080,
            indexer_flags: 1 | 16,
            size: 5 * 1024 * 1024 * 1024,
            ..input(&languages)
        })
    };
    assert!(check(spec("SourceSpecification", json!({"value": 6}))));
    assert!(!check(spec("SourceSpecification", json!({"value": 7}))));
    assert!(check(spec(
        "ResolutionSpecification",
        json!({"value": 1080})
    )));
    assert!(check(spec(
        "IndexerFlagSpecification",
        json!({"value": 16})
    )));
    assert!(!check(spec(
        "IndexerFlagSpecification",
        json!({"value": 2})
    )));
    // `size > min && size <= max`, in GiB.
    assert!(check(spec(
        "SizeSpecification",
        json!({"min": 4, "max": 5})
    )));
    assert!(!check(spec(
        "SizeSpecification",
        json!({"min": 5, "max": 6})
    )));
}
