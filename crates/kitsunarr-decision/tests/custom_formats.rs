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
        episode_titles: &[],
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
fn invalid_regexes_never_match_even_when_negated() {
    for implementation in ["ReleaseTitleSpecification", "ReleaseGroupSpecification"] {
        for negate in [false, true] {
            for required in [false, true] {
                let mut invalid = spec(implementation, json!({"value": "(unclosed"}));
                invalid.negate = negate;
                invalid.required = required;
                let (formats, problems) = compile_formats(&[format(1, "Broken", vec![invalid])]);
                assert_eq!(problems.len(), 1, "{problems:?}");
                assert!(problems[0].contains("not valid"));
                assert!(
                    !formats[0].matches(&CustomFormatInput {
                        release_title: "Series.Title.S01E01.1080p.WEB-DL-GRP",
                        release_group: Some("GRP"),
                        ..input(&[])
                    }),
                    "{implementation}, negate={negate}, required={required}"
                );
            }
        }
    }
}

#[test]
fn invalid_required_regex_cannot_be_bypassed_by_a_valid_alternative() {
    for implementation in ["ReleaseTitleSpecification", "ReleaseGroupSpecification"] {
        let mut invalid = spec(implementation, json!({"value": "[unclosed"}));
        invalid.negate = true;
        invalid.required = true;
        let valid = spec(implementation, json!({"value": "GRP"}));
        let (formats, problems) =
            compile_formats(&[format(1, "Broken", vec![invalid.clone(), valid.clone()])]);
        assert_eq!(problems.len(), 1);
        let input = CustomFormatInput {
            release_title: "Series.Title.S01E01.1080p.WEB-DL-GRP",
            release_group: Some("GRP"),
            ..input(&[])
        };
        assert!(!formats[0].matches(&input), "{implementation}");

        // An optional invalid member must still allow a valid member of its group to match.
        invalid.required = false;
        let (formats, _) = compile_formats(&[format(1, "Alternative", vec![invalid, valid])]);
        assert!(formats[0].matches(&input), "{implementation}");
    }
}

#[test]
fn blank_regexes_keep_their_existing_negation_semantics() {
    for implementation in ["ReleaseTitleSpecification", "ReleaseGroupSpecification"] {
        for pattern in ["", "  "] {
            for negate in [false, true] {
                let mut blank = spec(implementation, json!({"value": pattern}));
                blank.negate = negate;
                let (formats, problems) = compile_formats(&[format(1, "Blank", vec![blank])]);
                assert!(problems.is_empty(), "{problems:?}");
                assert_eq!(formats[0].matches(&input(&[])), negate);
            }
        }
    }
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

/// Tanukarr's alternative groups: group 0 always applies, positive groups are alternatives
/// (`A AND (B OR C)`), each one grouped by implementation as usual.
#[test]
fn alternative_groups_form_or_branches() {
    let cond = |implementation: &str, value: serde_json::Value, group: i64| {
        let mut fields = value;
        fields["alternativeGroup"] = json!(group);
        spec(implementation, fields)
    };
    let check = |specs: Vec<_>, release_title: &str, group: Option<&str>| {
        let (formats, problems) = compile_formats(&[format(1, "F", specs)]);
        assert!(problems.is_empty(), "{problems:?}");
        formats[0].matches(&CustomFormatInput {
            release_title,
            file_name: None,
            release_group: group,
            ..input(&[])
        })
    };
    // 1080p AND (x265 OR group GRP).
    let specs = || {
        vec![
            cond("ReleaseTitleSpecification", json!({"value": "1080p"}), 0),
            cond("ReleaseTitleSpecification", json!({"value": "x265"}), 1),
            cond("ReleaseGroupSpecification", json!({"value": "^GRP$"}), 2),
        ]
    };
    assert!(check(
        specs(),
        "Show.S01E01.1080p.WEB.x265-OTHER",
        Some("OTHER")
    ));
    assert!(check(
        specs(),
        "Show.S01E01.1080p.WEB.x264-GRP",
        Some("GRP")
    ));
    assert!(!check(
        specs(),
        "Show.S01E01.1080p.WEB.x264-OTHER",
        Some("OTHER")
    ));
    // The always-required part still has to match.
    assert!(!check(
        specs(),
        "Show.S01E01.720p.WEB.x265-GRP",
        Some("GRP")
    ));
    // Within one alternative, different implementations must all match, and same
    // implementations are grouped as usual.
    let both = vec![
        cond("ReleaseTitleSpecification", json!({"value": "x265"}), 1),
        cond("ReleaseGroupSpecification", json!({"value": "^GRP$"}), 1),
        cond("ReleaseTitleSpecification", json!({"value": "AV1"}), 2),
    ];
    assert!(!check(
        both.clone(),
        "Show.S01E01.x265-OTHER",
        Some("OTHER")
    ));
    assert!(check(both.clone(), "Show.S01E01.x265-GRP", Some("GRP")));
    assert!(check(both, "Show.S01E01.AV1-OTHER", Some("OTHER")));
    // Without a group 0 any alternative suffices; required keeps its meaning in a group.
    let mut required = cond("ReleaseTitleSpecification", json!({"value": "HDR"}), 1);
    required.required = true;
    let only = vec![
        cond("ReleaseTitleSpecification", json!({"value": "x265"}), 1),
        required,
        cond("ReleaseTitleSpecification", json!({"value": "AV1"}), 2),
    ];
    assert!(!check(only.clone(), "Show.S01E01.x265-GRP", None));
    assert!(check(only.clone(), "Show.S01E01.HDR.x265-GRP", None));
    assert!(check(only, "Show.S01E01.AV1-GRP", None));
    // Migrated values may be strings; negative numbers behave as 0.
    let mut negative = spec("ReleaseTitleSpecification", json!({"value": "1080p"}));
    negative
        .fields
        .insert("AlternativeGroup".into(), json!("-1"));
    let alternative = cond("ReleaseTitleSpecification", json!({"value": "x265"}), 3);
    assert!(!check(
        vec![negative.clone(), alternative.clone()],
        "Show.S01E01.720p.x265",
        None
    ));
    assert!(check(
        vec![negative, alternative],
        "Show.S01E01.1080p.x265",
        None
    ));
}

/// Tanukarr's `ExcludeEpisodeTitles`: words of the episode titles after the episode number do
/// not count (`ReleaseTitleSpecification.WithoutEpisodeTitles`).
#[test]
fn release_titles_can_ignore_episode_titles() {
    use kitsunarr_decision::custom_format::without_episode_titles;
    let titles = vec!["The Amazon Job".to_string(), "Pilot".to_string()];
    assert_eq!(
        without_episode_titles("Show.S01E02.The.Amazon.Job.1080p.WEB-DL-GRP", &titles),
        "Show.S01E02..1080p.WEB-DL-GRP"
    );
    // Before the episode number nothing is removed; short titles never are.
    assert_eq!(
        without_episode_titles("The.Amazon.Job.S01E02.1080p-GRP", &titles),
        "The.Amazon.Job.S01E02.1080p-GRP"
    );
    assert_eq!(
        without_episode_titles("Show.S01E01.Pilot.AMZ-GRP", &["AMZ".into()]),
        "Show.S01E01.Pilot.AMZ-GRP"
    );
    // Whole words only, case-insensitive, any separator; titles without an episode number
    // are left alone.
    assert_eq!(
        without_episode_titles("Show S01E01 pilotx AMZN", &titles),
        "Show S01E01 pilotx AMZN"
    );
    assert_eq!(
        without_episode_titles("Show - S01 E01-E02 - PILOT - AMZN", &titles),
        "Show - S01 E01-E02 -  - AMZN"
    );
    assert_eq!(
        without_episode_titles("Show.2024.1080p.Amazon.Job", &titles),
        "Show.2024.1080p.Amazon.Job"
    );

    let amazon = |exclude: bool| {
        spec(
            "ReleaseTitleSpecification",
            json!({"value": "\\bAmazon\\b", "excludeEpisodeTitles": exclude}),
        )
    };
    let matches = |s, titles: &[String]| {
        let (formats, _) = compile_formats(&[format(1, "AMZN", vec![s])]);
        formats[0].matches(&CustomFormatInput {
            release_title: "Show.S01E02.The.Amazon.Job.1080p.WEB-DL-GRP",
            file_name: Some("Show.S01E02.The.Amazon.Job.1080p.WEB-DL-GRP.mkv"),
            episode_titles: titles,
            ..input(&[])
        })
    };
    assert!(matches(amazon(false), &titles));
    assert!(!matches(amazon(true), &titles));
    assert!(
        matches(amazon(true), &[]),
        "no known titles: nothing removed"
    );
}
