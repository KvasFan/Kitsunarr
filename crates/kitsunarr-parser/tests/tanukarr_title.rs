//! Tanukarr's parser fixtures (`AnimeTrailingGroupSeasonFixture`, the space-separated
//! multi-season cases of `SeasonParserFixture`).

use kitsunarr_parser::parse_title;

#[test]
fn trailing_subgroup_anime_keeps_the_season_alias() {
    for (title, alias, season, episode, group) in [
        (
            "Shokugeki no Soma S2 - 01 [HorribleSubs][1080p].mkv",
            "Shokugeki no Soma S2",
            2,
            1,
            "HorribleSubs",
        ),
        (
            "Shokugeki no Soma S2 - 10 [HorribleSubs][1080p].mkv",
            "Shokugeki no Soma S2",
            2,
            10,
            "HorribleSubs",
        ),
        (
            "Shokugeki no Soma S2 - 13 [HorribleSubs][1080p].mkv",
            "Shokugeki no Soma S2",
            2,
            13,
            "HorribleSubs",
        ),
        (
            "Another Anime S3 - 08 [OtherGroup] [720p].mkv",
            "Another Anime S3",
            3,
            8,
            "OtherGroup",
        ),
    ] {
        let r = parse_title(title).unwrap_or_else(|| panic!("{title}"));
        assert_eq!(r.series_title, alias, "{title}");
        assert_eq!(r.season_numbers.first().copied(), Some(season), "{title}");
        assert_eq!(r.absolute_episode_numbers, vec![episode], "{title}");
        assert_eq!(r.release_group.as_deref(), Some(group), "{title}");
        assert!(!r.full_season && !r.is_multi_season(), "{title}");
    }
}

#[test]
fn pack_ranges_are_preserved() {
    for (title, seasons) in [
        (
            "Shokugeki no Soma S01-S03 + OVA (Food Wars) [1080p]",
            vec![1, 2, 3],
        ),
        ("Series S02-S10 [OtherGroup][1080p]", (2..=10).collect()),
        ("Series S02 - S10 [OtherGroup][1080p]", (2..=10).collect()),
        ("Series S02-10 [OtherGroup][1080p]", (2..=10).collect()),
        ("Series S02 - 10 1080p WEB-DL", (2..=10).collect()),
        (
            "Series Title Complete Series S01 S04 (1080p BluRay x265)",
            vec![1, 2, 3, 4],
        ),
        ("[Group] Series Title S01 S04 (BD 1080p)", vec![1, 4]),
    ] {
        let r = parse_title(title).unwrap_or_else(|| panic!("{title}"));
        assert!(r.full_season, "{title}");
        assert_eq!(r.season_numbers, seasons, "{title}");
        assert!(r.absolute_episode_numbers.is_empty(), "{title}");
    }
}

#[test]
fn leading_subgroup_season_alias_is_kept() {
    let r = parse_title("[HorribleSubs] Shokugeki no Soma S3 - 10 [1080p].mkv").unwrap();
    assert_eq!(r.series_title, "Shokugeki no Soma S3");
    assert_eq!(r.absolute_episode_numbers, vec![10]);
    assert!(!r.full_season);
}
