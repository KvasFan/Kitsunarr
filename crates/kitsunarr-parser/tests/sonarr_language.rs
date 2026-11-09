//! Sonarr's language tests (`LanguageParserFixture`, `IsoLanguagesFixture`), replayed against
//! the port.

mod common;

use common::{Fixture, Outcome, expect_eq, s};
use kitsunarr_domain::Language;
use kitsunarr_parser::{
    find_iso, parse_languages, parse_subtitle_language, parse_subtitle_language_information,
};

/// Sonarr `Language` identifier → name in Kitsunarr's catalogue.
fn language(identifier: &str) -> &'static Language {
    let name = match identifier {
        "PortugueseBrazil" => "Portuguese (Brazil)",
        "SpanishLatino" => "Spanish (Latino)",
        other => other,
    };
    Language::by_name(name).unwrap_or_else(|| panic!("unknown language {identifier}"))
}

fn names(langs: &[&'static Language]) -> Vec<&'static str> {
    langs.iter().map(|l| l.name).collect()
}

/// `result.Should().Contain(...)` for every expected language.
fn contains(title: &str, expected: &[&str]) -> Result<(), String> {
    let got = parse_languages(title);
    for e in expected {
        let want = language(e);
        if !got.contains(&want) {
            return Err(format!("missing {}; got {:?}", want.name, names(&got)));
        }
    }
    Ok(())
}

/// `result.Should().BeEquivalentTo(...)`: same languages, any order.
fn equivalent(title: &str, expected: &[&str]) -> Result<(), String> {
    let got = parse_languages(title);
    let mut got_names = names(&got);
    let mut want: Vec<&str> = expected.iter().map(|e| language(e).name).collect();
    got_names.sort_unstable();
    want.sort_unstable();
    expect_eq("languages", got_names, want)
}

/// Methods asserting on `Parser.ParseTitle(title).Languages`, which uses the release tokens
/// after the episode marker; they need the title parser.
const NEEDS_TITLE_PARSER: &[&str] = &[
    "should_add_original_language_to_german_release_with_dl_tag",
    "should_not_add_original_language_to_german_release_when_title_contains_web_dl",
    "should_add_original_language_and_english_to_german_release_with_ml_tag",
    "should_parse_original_title_from_release_name",
    "should_parse_english_and_ukranian",
    "should_parse_turkish_and_ukranian",
    "should_parse_azerbaijani",
    "should_parse_uzbek",
];

#[test]
fn language_parser_fixture() {
    Fixture::load("LanguageParserFixture").run(|method, a| {
        if NEEDS_TITLE_PARSER.contains(&method) {
            return Some(Outcome::Pending(
                "asserts on Parser.ParseTitle (title parser not ported yet)",
            ));
        }
        let title = s(&a[0]);
        if let Some(rest) = method.strip_prefix("should_parse_language_") {
            let expected: Option<&[&str]> = match rest {
                "unknown" => Some(&["Unknown"]),
                "french_english" => Some(&["French", "English"]),
                "portuguese_brazil" => Some(&["PortugueseBrazil"]),
                "spanish_latino" => Some(&["SpanishLatino"]),
                "russian_and_georgian" => {
                    return Some(equivalent(title, &["Russian", "Georgian"]).into());
                }
                "english_french_german_italian_portuguese_spanish" => {
                    return Some(
                        equivalent(
                            title,
                            &[
                                "English",
                                "French",
                                "German",
                                "Italian",
                                "Portuguese",
                                "Spanish",
                            ],
                        )
                        .into(),
                    );
                }
                _ => None,
            };
            let single;
            let expected = match expected {
                Some(e) => e,
                None => {
                    let mut c = rest.chars();
                    let first = c.next()?.to_uppercase().collect::<String>();
                    single = [format!("{first}{}", c.as_str())];
                    return Some(contains(title, &[single[0].as_str()]).into());
                }
            };
            return Some(contains(title, expected).into());
        }
        let result = match method {
            "should_parse_subtitle_language_unknown" => {
                expect_eq("language", parse_subtitle_language(title).name, "Unknown")
            }
            "should_parse_subtitle_language_english" => {
                expect_eq("language", parse_subtitle_language(title).name, "English")
            }
            "should_parse_hindi_and_english" => equivalent(title, &["Hindi", "English"]),
            "should_parse_spanish_and_catalan" => equivalent(title, &["Spanish", "Catalan"]),
            "should_parse_english_spanish_and_catalan" => {
                equivalent(title, &["English", "Spanish", "Catalan"])
            }
            "should_parse_title_and_tags" => {
                let info = parse_subtitle_language_information(title);
                let mut tags = info.language_tags.clone();
                let mut want: Vec<String> = a[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| s(t).to_string())
                    .collect();
                tags.sort();
                want.sort();
                // FluentAssertions' string BeEquivalentTo ignores case.
                let title_ok = match (info.title.as_deref(), a[2].as_str()) {
                    (Some(g), Some(w)) => g.eq_ignore_ascii_case(w),
                    (None, None) => true,
                    _ => false,
                };
                expect_eq("tags", tags, want)
                    .and(if title_ok {
                        Ok(())
                    } else {
                        Err(format!("title: got {:?}, want {}", info.title, a[2]))
                    })
                    .and(expect_eq(
                        "language",
                        info.language.name,
                        Language::by_name(s(&a[3])).map_or("?", |l| l.name),
                    ))
            }
            "should_not_parse_false_title" => {
                let info = parse_subtitle_language_information(title);
                expect_eq("language", info.language.name, "Unknown")
                    .and(expect_eq("tags", info.language_tags.len(), 0))
                    .and(expect_eq("raw title", info.raw_title, None))
            }
            _ => return None,
        };
        Some(result.into())
    });
}

#[test]
fn iso_languages_fixture() {
    Fixture::load("IsoLanguagesFixture").run(|method, a| {
        let found = find_iso(s(&a[0])).map(|i| i.language().name);
        let want = match method {
            "should_return_iso_language_for_English" => Some("English"),
            "unknown_or_invalid_code_should_return_null" | "should_not_return_portuguese" => None,
            "should_return_portuguese" => Some("Portuguese"),
            "should_lookup_cze_via_alternate_iso_code_mapping" => Some("Czech"),
            "should_return_urdu" => Some("Urdu"),
            "should_return_romansh" => Some("Romansh"),
            "should_return_georgian" => Some("Georgian"),
            "should_return_irish" => Some("Irish"),
            _ => return None,
        };
        Some(expect_eq("language", found, want).into())
    });
}
