//! Differential oracle from the unmodified FileNameBuilder in Tanukarr's container image.
//! Configuration, quality metadata and supplied custom formats are synthetic fixtures.
use kitsunarr_domain::SeriesType;
use kitsunarr_domain::api::settings::NamingSettings;
use kitsunarr_domain::quality::{QualityModel, Revision};
use kitsunarr_library::file_names::{self, Episode, File, Series};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Case {
    kind: String,
    pattern: String,
    style: i32,
    count: i32,
    title: String,
    version: i32,
    expected: String,
}

#[test]
fn names_match_tanukarr_binary_oracle() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/naming/tanukarr-0.1.5.json"
    ))
    .unwrap();
    let cases: Vec<Case> = serde_json::from_value(data["cases"].clone()).unwrap();
    let mut differences = Vec::new();
    for case in &cases {
        let naming = NamingSettings {
            rename_episodes: true,
            multi_episode_style: case.style,
            standard_episode_format: case.pattern.clone(),
            daily_episode_format: case.pattern.clone(),
            anime_episode_format: case.pattern.clone(),
            movie_file_format: case.pattern.clone(),
            ..Default::default()
        };
        let series = Series {
            title: case.title.clone(),
            year: 2010,
            series_type: match case.kind.as_str() {
                "anime" => SeriesType::Anime,
                "daily" => SeriesType::Daily,
                _ => SeriesType::Standard,
            },
            ..Default::default()
        };
        let episodes: Vec<_> = (1..=case.count)
            .map(|n| Episode {
                season_number: 1,
                episode_number: n,
                absolute_episode_number: Some(100 + n),
                title: if case.count == 1 {
                    "City Sushi".into()
                } else {
                    format!("The Story ({n})")
                },
                air_date: Some("2010-01-02".into()),
            })
            .collect();
        let file = File {
            id: 1,
            relative_path: Some("original.mkv".into()),
            scene_name: Some("South.Park.S01E01.720p.HDTV.x264-GRP".into()),
            release_group: Some("GRP".into()),
            quality: QualityModel {
                quality_id: 4,
                revision: Revision {
                    version: case.version,
                    ..Default::default()
                },
            },
            quality_title: "HDTV-720p".into(),
            custom_formats: vec![
                ("INTERNAL".into(), true),
                ("AMZN".into(), true),
                ("NAME WITH SPACES".into(), true),
                ("Excluded".into(), false),
            ],
            ..Default::default()
        };
        let got = if case.kind == "movie" {
            file_names::build_movie_file_name(&series, &file, ".mkv", &naming).unwrap()
        } else {
            file_names::build_file_name(&episodes, &series, &file, ".mkv", &naming).unwrap()
        };
        if got != case.expected {
            differences.push(format!("{case:?}: got {got:?}"));
        }
    }
    assert!(
        differences.is_empty(),
        "{} / {} differences; first cases: {:#?}",
        differences.len(),
        cases.len(),
        differences.iter().take(12).collect::<Vec<_>>()
    );
    assert_eq!(cases.len(), 302);
}

#[test]
fn custom_format_regexes_match_tanukarr_binary_oracle() {
    use kitsunarr_decision::{CustomFormatInput, compile_formats, custom_format::evaluate};
    use kitsunarr_domain::api::settings::{CustomFormat, CustomFormatSpecification};
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/naming/tanukarr-0.1.5.json"
    ))
    .unwrap();
    let cases = data["regexCases"].as_array().unwrap();
    for case in cases {
        let cf = CustomFormat {
            id: 1,
            name: "Matched".into(),
            include_when_renaming: true,
            specifications: vec![CustomFormatSpecification {
                name: "Regex".into(),
                implementation: "ReleaseTitleSpecification".into(),
                negate: case["negate"].as_bool().unwrap(),
                required: true,
                fields: serde_json::json!({"value": case["pattern"]})
                    .as_object()
                    .unwrap()
                    .clone(),
            }],
        };
        let (formats, problems) = compile_formats(&[cf]);
        assert!(problems.is_empty(), "{problems:?}");
        let input = CustomFormatInput {
            release_title: case["title"].as_str().unwrap(),
            file_name: case["filename"].as_str(),
            release_group: None,
            quality: kitsunarr_domain::Quality::by_id(4).unwrap(),
            size: 0,
            indexer_flags: 0,
            languages: &[],
            release_type: kitsunarr_parser::ReleaseType::Unknown,
            original_language: None,
            episode_titles: &[],
        };
        assert_eq!(
            !evaluate(&formats, &input).is_empty(),
            case["matched"].as_bool().unwrap(),
            "{case}"
        );
    }
    assert_eq!(cases.len(), 72);
}
