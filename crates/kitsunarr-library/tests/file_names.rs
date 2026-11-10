//! Sonarr's `FileNameBuilderFixture` (OrganizerTests/FileNameBuilderTests), ported. Tests about
//! media info are not ported: Kitsunarr does not read media info yet.

use kitsunarr_domain::SeriesType;
use kitsunarr_domain::api::settings::NamingSettings;
use kitsunarr_domain::quality::{QualityModel, Revision};
use kitsunarr_library::file_names::{Episode, File, Series, build_file_name};

struct Fixture {
    series: Series,
    naming: NamingSettings,
    episode: Episode,
    file: File,
}

/// `Setup`: South Park S15E06 "City Sushi" (absolute 100), HDTV-720p by SonarrTest, with
/// Sonarr's default naming and renaming on (the multi-episode style default is PrefixedRange).
fn fixture() -> Fixture {
    Fixture {
        series: Series {
            title: "South Park".into(),
            year: 1,
            tvdb_id: 1,
            ..Default::default()
        },
        naming: NamingSettings {
            rename_episodes: true,
            multi_episode_style: 5,
            ..Default::default()
        },
        episode: Episode {
            season_number: 15,
            episode_number: 6,
            absolute_episode_number: Some(100),
            title: "City Sushi".into(),
            air_date: None,
        },
        file: File {
            quality: QualityModel {
                quality_id: 4,
                revision: Revision::default(),
            },
            quality_title: "HDTV-720p".into(),
            release_group: Some("SonarrTest".into()),
            ..Default::default()
        },
    }
}

impl Fixture {
    fn name(&self) -> String {
        build_file_name(
            std::slice::from_ref(&self.episode),
            &self.series,
            &self.file,
            "",
            &self.naming,
        )
        .unwrap()
    }

    fn with(mut self, format: &str) -> Self {
        self.naming.standard_episode_format = format.into();
        self
    }

    fn proper(mut self) -> Self {
        self.file.quality.revision.version = 2;
        self
    }
}

#[test]
fn series_title_separators_and_casing() {
    for (format, expected) in [
        ("{Series Title}", "South Park"),
        ("{Series_Title}", "South_Park"),
        ("{Series.Title}", "South.Park"),
        ("{Series-Title}", "South-Park"),
        ("{SERIES TITLE}", "SOUTH PARK"),
        ("{sErIES-tItLE}", "South-Park"),
        ("{series title}", "south park"),
    ] {
        assert_eq!(fixture().with(format).name(), expected, "{format}");
    }
}

#[test]
fn should_cleanup_series_title() {
    let mut f = fixture().with("{Series.CleanTitle}");
    f.series.title = "South Park (1997)".into();
    assert_eq!(f.name(), "South.Park.1997");
}

#[test]
fn episode_titles() {
    assert_eq!(fixture().with("{Episode Title}").name(), "City Sushi");
    assert_eq!(fixture().with("{ePisOde-TitLe}").name(), "City-Sushi");
    // should_not_clean_episode_title_if_there_is_only_one
    let mut f = fixture().with("{Episode Title}");
    f.episode.title = "City Sushi (1)".into();
    assert_eq!(f.name(), "City Sushi (1)");
}

#[test]
fn season_and_episode_numbers() {
    for (format, expected) in [
        ("{season}x{episode}", "1x6"),
        ("{season:00}x{episode}", "01x6"),
        ("{season}x{episode:00}", "1x06"),
    ] {
        let mut f = fixture().with(format);
        f.episode.season_number = 1;
        assert_eq!(f.name(), expected, "{format}");
    }
}

#[test]
fn quality_tokens() {
    assert_eq!(fixture().with("{Quality Title}").name(), "HDTV-720p");
    assert_eq!(fixture().with("{Quality Proper}").proper().name(), "Proper");
    let mut real = fixture().with("{Quality Real}");
    real.file.quality.revision.real = 1;
    assert_eq!(real.name(), "REAL");
    let mut anime = fixture().proper();
    anime.series.series_type = SeriesType::Anime;
    anime.naming.anime_episode_format = "{Quality Proper}".into();
    assert_eq!(
        anime.name(),
        "v2",
        "should_replace_quality_proper_with_v2_for_anime_v2"
    );
    assert_eq!(
        fixture().with("{Quality Title} {Quality Proper}").name(),
        "HDTV-720p"
    );
    let bracketed =
        "{Series Title} - S{season:00}E{episode:00} [{Quality Title}] {[Quality Proper]}";
    assert_eq!(
        fixture().with(bracketed).proper().name(),
        "South Park - S15E06 [HDTV-720p] [Proper]"
    );
    assert_eq!(
        fixture().with(bracketed).name(),
        "South Park - S15E06 [HDTV-720p]"
    );
    let full = "{Series Title} - S{season:00}E{episode:00} [{Quality Full}]";
    assert_eq!(
        fixture().with(full).name(),
        "South Park - S15E06 [HDTV-720p]"
    );
    assert_eq!(
        fixture().with(full).proper().name(),
        "South Park - S15E06 [HDTV-720p Proper]"
    );
    let mut f = fixture().with(full);
    f.file.quality.revision.real = 1;
    assert_eq!(f.name(), "South Park - S15E06 [HDTV-720p REAL]");
}

#[test]
fn should_replace_all_contents_in_pattern() {
    let f = fixture()
        .with("{Series Title} - S{season:00}E{episode:00} - {Episode Title} [{Quality Title}]");
    assert_eq!(f.name(), "South Park - S15E06 - City Sushi [HDTV-720p]");
}

#[test]
fn escaping() {
    for (format, expected) in [
        ("Some Escaped {{ String", "Some Escaped { String"),
        ("Some Escaped }} String", "Some Escaped } String"),
        (
            "Some Escaped {{Series Title}} String",
            "Some Escaped {Series Title} String",
        ),
        (
            "Some Escaped {{{Series Title}}} String",
            "Some Escaped {South Park} String",
        ),
    ] {
        assert_eq!(fixture().with(format).name(), expected, "{format}");
    }
    // should_escape_token_in_title
    let mut f = fixture().with("Some Unescaped {Series Title} String");
    f.series.title = "My {Quality Full} Title".into();
    assert_eq!(f.name(), "Some Unescaped My {Quality Full} Title String");
}

#[test]
fn original_names_when_renaming_is_off() {
    let mut f = fixture();
    f.naming.rename_episodes = false;
    f.file.relative_path = Some("30 Rock - S01E01 - Test".into());
    assert_eq!(f.name(), "30 Rock - S01E01 - Test");
    f.file.relative_path = None;
    f.file.path = Some("/Test/Unsorted/Series - S01E01 - Test".into());
    assert_eq!(f.name(), "Series - S01E01 - Test");
    f.file.scene_name = Some("30.Rock.S01E01.xvid-LOL".into());
    f.file.relative_path = Some("30 Rock - S01E01 - Test".into());
    assert_eq!(f.name(), "30.Rock.S01E01.xvid-LOL");
    // should_replace_illegal_characters_when_renaming_is_disabled
    f.file.scene_name = Some("30.Rock.S01E01.xvid:LOL".into());
    assert_eq!(f.name(), "30.Rock.S01E01.xvid-LOL");
}

#[test]
fn daily_series() {
    let mut f = fixture();
    f.naming.daily_episode_format = "{Series Title} - {air-date} - {Episode Title}".into();
    f.series.title = "The Daily Show with Jon Stewart".into();
    f.series.series_type = SeriesType::Daily;
    f.episode = Episode {
        season_number: 1,
        episode_number: 5,
        absolute_episode_number: None,
        title: "Kristen Stewart".into(),
        air_date: Some("2012-12-13".into()),
    };
    assert_eq!(
        f.name(),
        "The Daily Show with Jon Stewart - 2012-12-13 - Kristen Stewart"
    );
    // should_set_airdate_to_unknown_if_not_available
    f.naming.daily_episode_format = "{Series Title} - {Air-Date} - {Episode Title}".into();
    f.episode.air_date = None;
    assert_eq!(
        f.name(),
        "The Daily Show with Jon Stewart - Unknown - Kristen Stewart"
    );
    // should_use_standard_if_series_isDaily_special
    f.naming.standard_episode_format =
        "{Series Title} - S{season:00}E{episode:00} - {Episode Title}".into();
    f.episode.air_date = Some("2012-12-13".into());
    f.episode.season_number = 0;
    assert_eq!(
        f.name(),
        "The Daily Show with Jon Stewart - S00E05 - Kristen Stewart"
    );
}

#[test]
fn release_group_and_hash() {
    assert_eq!(fixture().with("{Release Group}").name(), "SonarrTest");
    let mut f = fixture().with("{Release Group}");
    f.file.release_group = None;
    // Upstream writes "Sonarr" here.
    assert_eq!(f.name(), "Kitsunarr");
    for pattern in [
        "{Episode Title}{-Release Group}",
        "{Episode Title}{ Release Group}",
        "{Episode Title}{ [Release Group]}",
    ] {
        let mut f = fixture().with(pattern);
        f.file.release_group = None;
        assert_eq!(f.name(), "City Sushi", "{pattern}");
    }
    for group in ["0SEC", "2HD", "IMMERSE"] {
        let mut f = fixture().with("{Release Group}");
        f.file.release_group = Some(group.into());
        assert_eq!(f.name(), group);
    }
    let mut f = fixture().with("{Release Hash}");
    f.file.release_hash = Some("ABCDEFGH".into());
    assert_eq!(f.name(), "ABCDEFGH");
    f.file.release_hash = None;
    assert_eq!(f.name(), "");
}

#[test]
fn original_title_and_filename_tokens() {
    let mut f = fixture().with("{Series Title} - {Original Title}");
    f.series.title = "30 Rock".into();
    f.file.scene_name = Some("30.Rock.S01E01.xvid-LOL".into());
    f.file.relative_path = Some("30 Rock - S01E01 - Test".into());
    assert_eq!(f.name(), "30 Rock - 30.Rock.S01E01.xvid-LOL");
    let f = f.with("{Original Title}");
    assert_eq!(f.name(), "30.Rock.S01E01.xvid-LOL");
    let f = f.with("{Original Filename}");
    assert_eq!(f.name(), "30 Rock - S01E01 - Test");
    // should_use_existing_filename_when_scene_name_is_not_available
    let mut f = fixture().with("{Original Title}");
    f.file.relative_path = Some("existing.file.mkv".into());
    assert_eq!(f.name(), "existing.file");
}

fn other(title: &str, series: &str, format: &str, style: i32) -> String {
    let mut f = fixture().with(format);
    f.naming.multi_episode_style = style;
    f.series = Series {
        title: series.into(),
        ..Default::default()
    };
    f.episode = Episode {
        season_number: 6,
        episode_number: 6,
        absolute_episode_number: None,
        title: title.into(),
        air_date: None,
    };
    f.name()
}

#[test]
fn trimming_and_periods() {
    let format = "{Series Title} - S{season:00}E{episode:00} - {Episode Title}";
    assert_eq!(
        other("Part 1.", "30 Rock", format, 3),
        "30 Rock - S06E06 - Part 1"
    );
    assert_eq!(
        other("Part 1?", "30 Rock", format, 3),
        "30 Rock - S06E06 - Part 1"
    );
    let dotted = "{Series.Title}.S{season:00}E{episode:00}.{Episode.Title}";
    assert_eq!(
        other("Part 1", "Chicago P.D.", dotted, 5),
        "Chicago.P.D.S06E06.Part.1"
    );
    assert_eq!(
        other("Part 1", "Chicago P.D..", dotted, 5),
        "Chicago.P.D.S06E06.Part.1"
    );
}

#[test]
fn absolute_numbering() {
    let format = "{Series.Title}.S{season:00}E{episode:00}.{absolute:00}.{Episode.Title}";
    assert_eq!(
        fixture().with(format).name(),
        "South.Park.S15E06.City.Sushi"
    );
    let anime = |format: &str| {
        let mut f = fixture();
        f.series.series_type = SeriesType::Anime;
        f.naming.anime_episode_format = format.into();
        f
    };
    assert_eq!(anime(format).name(), "South.Park.S15E06.100.City.Sushi");
    assert_eq!(
        anime("{Series.Title}.S{season:00}E{episode:00}.{Episode.Title}").name(),
        "South.Park.S15E06.City.Sushi"
    );
    assert_eq!(
        anime("{Series.Title}.{absolute:00}.{Episode.Title}").name(),
        "South.Park.100.City.Sushi"
    );
    assert_eq!(
        anime("{Series.Title}.{season}x{episode:00}.{absolute:000}\\{Series.Title}.S{season:00}E{episode:00}.{absolute:00}.{Episode.Title}").name(),
        "South.Park.15x06.100/South.Park.S15E06.100.City.Sushi"
    );
    assert_eq!(
        anime("{Series Title} Season {season:0000} Episode {episode:0000}\\{Series.Title}.S{season:00}E{episode:00}.{absolute:00}.{Episode.Title}").name(),
        "South Park Season 0015 Episode 0006/South.Park.S15E06.100.City.Sushi"
    );
    // should_use_standard_naming_when_anime_episode_has_no_absolute_number
    let mut f = anime("{Series Title} - {absolute:000} - {Episode Title}");
    f.naming.standard_episode_format =
        "{Series Title} - {season:0}x{episode:00} - {Episode Title}".into();
    f.episode.absolute_episode_number = None;
    assert_eq!(f.name(), "South Park - 15x06 - City Sushi");
    // should_not_require_a_separator_between_tokens
    let f = anime("[{Release Group}]{Series.CleanTitle}.{absolute:000}");
    assert_eq!(f.name(), "[SonarrTest]South.Park.100");
}

#[test]
fn affixes() {
    let f =
        fixture().with("{Series.Title}.S{season:00}E{episode:00}{_Episode.Title_}{Quality.Title}");
    assert_eq!(f.name(), "South.Park.S15E06_City.Sushi_HDTV-720p");
    let mut f = fixture().with("{Series.Title}.S{season:00}E{episode:00}{_Episode.Title_}");
    f.episode.title = String::new();
    assert_eq!(f.name(), "South.Park.S15E06");
}

#[test]
fn separators_between_season_and_episode() {
    assert_eq!(
        fixture()
            .with("{Series.Title}.S{season:00}.E{episode:00}.{Episode.Title}")
            .name(),
        "South.Park.S15.E06.City.Sushi"
    );
    assert_eq!(
        fixture()
            .with("{Series Title} - S{season:00} E{episode:00} - {Episode Title}")
            .name(),
        "South Park - S15 E06 - City Sushi"
    );
    // should_maintain_ellipsis_in_naming_format
    assert_eq!(
        fixture()
            .with("{Series.Title}.S{season:00}.E{episode:00}...{Episode.CleanTitle}")
            .name(),
        "South.Park.S15.E06...City.Sushi"
    );
}

#[test]
fn extra_separators_are_trimmed() {
    for sep in [' ', '-', '.', '_'] {
        let f = fixture().with(&format!("{{Quality{sep}Title}}{sep}{{Quality{sep}Proper}}"));
        assert_eq!(f.name(), "HDTV-720p", "end, {sep:?}");
        let f = fixture().with(&format!(
            "{{Quality{sep}Title}}{sep}{{Quality{sep}Proper}}{sep}{{Episode{sep}Title}}"
        ));
        assert_eq!(
            f.name(),
            format!("HDTV-720p{sep}City{sep}Sushi"),
            "middle, {sep:?}"
        );
    }
}

/// Sonarr's `MultiEpisodeFixture`: S15E06–E08 (absolute 100–102), all titled "City Sushi".
fn multi(style: i32, anime: bool, format: &str, count: usize) -> String {
    let mut f = fixture();
    f.naming.multi_episode_style = style;
    if anime {
        f.series.series_type = SeriesType::Anime;
        f.naming.anime_episode_format = format.into();
    } else {
        f.naming.standard_episode_format = format.into();
    }
    let episodes: Vec<Episode> = (0..count as i32)
        .map(|i| Episode {
            season_number: 15,
            episode_number: 6 + i,
            absolute_episode_number: Some(100 + i),
            title: "City Sushi".into(),
            air_date: None,
        })
        .collect();
    build_file_name(&episodes, &f.series, &f.file, "", &f.naming).unwrap()
}

#[test]
fn multi_episode_fixture() {
    let standard = "{Series Title} - S{season:00}E{episode:00} - {Episode Title}";
    let absolute = "{Series Title} - {absolute:000} - {Episode Title}";
    let (extend, duplicate, repeat, scene, range, prefixed) = (0, 1, 2, 3, 4, 5);
    for (style, anime, format, count, expected) in [
        (
            extend,
            false,
            standard,
            2,
            "South Park - S15E06-07 - City Sushi",
        ),
        (
            duplicate,
            false,
            standard,
            2,
            "South Park - S15E06 - S15E07 - City Sushi",
        ),
        (
            repeat,
            false,
            standard,
            2,
            "South Park - S15E06E07 - City Sushi",
        ),
        (
            scene,
            false,
            standard,
            2,
            "South Park - S15E06-E07 - City Sushi",
        ),
        (
            extend,
            true,
            absolute,
            2,
            "South Park - 100-101 - City Sushi",
        ),
        (
            duplicate,
            true,
            absolute,
            3,
            "South Park - 100 - 101 - 102 - City Sushi",
        ),
        (
            duplicate,
            false,
            "{Series Title} - S{season:00}E{episode:00} - ({Quality Title}, {MediaInfo Full}, {Release Group}) - {Episode Title}",
            2,
            "South Park - S15E06 - S15E07 - (HDTV-720p, , SonarrTest) - City Sushi",
        ),
        (
            range,
            false,
            standard,
            3,
            "South Park - S15E06-08 - City Sushi",
        ),
        (
            range,
            true,
            absolute,
            3,
            "South Park - 100-102 - City Sushi",
        ),
        (
            repeat,
            true,
            absolute,
            3,
            "South Park - 100-101-102 - City Sushi",
        ),
        (
            range,
            false,
            standard,
            1,
            "South Park - S15E06 - City Sushi",
        ),
        (range, true, absolute, 1, "South Park - 100 - City Sushi"),
        (
            duplicate,
            true,
            "{Series Title} - {season}x{episode:00} - [{absolute:000}] - {Episode Title} - {Quality Title}",
            2,
            "South Park - 15x06 - 15x07 - [100-101] - City Sushi - HDTV-720p",
        ),
        (
            prefixed,
            false,
            standard,
            3,
            "South Park - S15E06-E08 - City Sushi",
        ),
        (
            prefixed,
            true,
            absolute,
            3,
            "South Park - 100-102 - City Sushi",
        ),
        (
            prefixed,
            false,
            standard,
            1,
            "South Park - S15E06 - City Sushi",
        ),
        (prefixed, true, absolute, 1, "South Park - 100 - City Sushi"),
        (
            prefixed,
            false,
            "{Series Title} - {season:0}x{episode:00} - {Episode Title}",
            3,
            "South Park - 15x06-x08 - City Sushi",
        ),
        (
            range,
            false,
            "{Series Title} (S{season:00}E{episode:00}) {Episode Title}",
            3,
            "South Park (S15E06-08) City Sushi",
        ),
    ] {
        assert_eq!(
            multi(style, anime, format, count),
            expected,
            "{format} ×{count}"
        );
    }
}

/// Sonarr's `GetSeasonFolderFixture` and `GetSeriesFolderFixture`.
#[test]
fn folder_fixtures() {
    use kitsunarr_library::file_names::{season_folder, series_folder};
    let season = |title: &str, season_title: Option<&str>, format: &str| {
        let naming = NamingSettings {
            season_folder_format: format.into(),
            ..Default::default()
        };
        let series = Series {
            title: title.into(),
            season_titles: season_title
                .map(|t| vec![(1, t.to_string())])
                .unwrap_or_default(),
            ..Default::default()
        };
        season_folder(&series, 1, &naming)
    };
    for (title, format, expected) in [
        (
            "Venture Bros.",
            "{Series.Title}.{season:00}",
            "Venture.Bros.01",
        ),
        (
            "Venture Bros.",
            "{Series Title} Season {season:00}",
            "Venture Bros. Season 01",
        ),
        (
            "Series Title?",
            "{Series Title} Season {season:00}",
            "Series Title! Season 01",
        ),
        (
            "Series Title?",
            "Season {season:00} - {Season Title}",
            "Season 01 - Part 1!",
        ),
        (
            "Series Title?",
            "Season {season:00} - {Season CleanTitle}",
            "Season 01 - Part 1",
        ),
    ] {
        assert_eq!(season(title, Some("Part 1?"), format), expected, "{format}");
    }
    assert_eq!(
        season("Series Title", None, "Season {season:00}{ - SeasonTitle}"),
        "Season 01"
    );
    for format in [
        "Season {season:00} - {Season Title}",
        "Season {season:00} - {Season CleanTitle}",
    ] {
        assert_eq!(season("Series Title", None, format), "Season 01");
    }
    assert_eq!(
        season(
            "Series Title",
            Some("Partial Title"),
            "Season {season:00}{ - SeasonTitle:10}"
        ),
        "Season 01 - Partial..."
    );
    assert_eq!(
        season(
            "Series Title",
            Some("Partial Title"),
            "Season {season:00}{ - SeasonTitle:-8}"
        ),
        "Season 01 - ...Title"
    );
    for (title, format, expected) in [
        ("30 Rock", "{Series Title}", "30 Rock"),
        ("30 Rock", "{Series.Title}", "30.Rock"),
        (
            "24/7 Road to the NHL Winter Classic",
            "{Series Title}",
            "24+7 Road to the NHL Winter Classic",
        ),
        ("Venture Bros.", "{Series.Title}", "Venture.Bros"),
        (".hack", "{Series.Title}", "hack"),
        ("30 Rock", ".{Series.Title}.", "30.Rock"),
    ] {
        let naming = NamingSettings {
            series_folder_format: format.into(),
            ..Default::default()
        };
        let series = Series {
            title: title.into(),
            ..Default::default()
        };
        assert_eq!(
            series_folder(&series, &naming),
            expected,
            "{title} {format}"
        );
    }
}
