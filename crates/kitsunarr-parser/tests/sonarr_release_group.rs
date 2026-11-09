//! Sonarr's release group tests (`ReleaseGroupParserFixture`), replayed against the port.

mod common;

use common::{Fixture, Outcome, expect_eq, s};
use kitsunarr_parser::parse_release_group;

#[test]
fn release_group_parser_fixture() {
    Fixture::load("ReleaseGroupParserFixture").run(|method, a| {
        let got = parse_release_group(s(&a[0]));
        let result = match method {
            "should_parse_release_group"
            | "should_parse_exception_release_group"
            | "should_not_include_language_in_release_group"
            | "should_not_include_repost_in_release_group"
            | "should_parse_anime_release_groups" => {
                expect_eq("release group", got.as_deref(), a[1].as_str())
            }
            "should_not_parse_anime_hash_as_release_group"
            | "should_not_parse_episode_title_as_release_group" => {
                expect_eq("release group", got.as_deref(), None)
            }
            _ => return None,
        };
        Some(Outcome::from(result))
    });
}
