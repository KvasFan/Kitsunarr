//! Ports of Sonarr's decision-engine unit tests (`NzbDrone.Core.Test/DecisionEngineTests`).
//! Each test names the upstream fixture and test it mirrors.

use chrono::{Duration, Utc};
use kitsunarr_domain::api::settings::{OrderedF64, QualityDefinition, QualityProfileItem};
use kitsunarr_domain::quality::{Quality, QualityModel, Revision};
use kitsunarr_domain::{DownloadProtocol, SeriesType};
use kitsunarr_parser::{ParsedEpisodeInfo, ParsedQuality, ReleaseType};

use crate::compare::{Comparer, prioritize};
use crate::model::*;
use crate::profile::ProperDownloadType;
use crate::specs::{self, Env, Subject};

const MB: i64 = 1024 * 1024;
const HDTV720P: i32 = 4;
const SDTV: i32 = 1;
const WEBDL1080P: i32 = 3;
const BLURAY1080P: i32 = 7;

fn items() -> Vec<QualityProfileItem> {
    // `QualityFixture.GetDefaultQualities()` with every quality allowed.
    [0, 1, 8, 2, 4, 9, 16, 10, 5, 3, 18, 6, 7, 19]
        .iter()
        .map(|q| QualityProfileItem {
            quality_id: Some(*q),
            group: None,
            allowed: true,
        })
        .collect()
}

pub(crate) fn series() -> Series {
    Series {
        monitored: true,
        id: 1,
        title: "Series Title".into(),
        year: 0,
        series_type: SeriesType::Standard,
        tvdb_id: 0,
        tv_rage_id: 0,
        imdb_id: None,
        alternate_titles: Vec::new(),
        runtime: 60,
        original_language: None,
        use_scene_numbering: false,
        tags: Vec::new(),
        profile: kitsunarr_domain::api::settings::QualityProfile {
            id: 1,
            name: "Profile".into(),
            upgrade_allowed: false,
            cutoff: 0,
            items: items(),
            min_format_score: 0,
            cutoff_format_score: 0,
            min_upgrade_format_score: 0,
            format_items: Vec::new(),
        },
        episodes: Vec::new(),
    }
}

fn settings() -> Settings {
    Settings {
        queue: Vec::new(),
        last_grabs: Vec::new(),
        blocklist: Vec::new(),
        enable_multi_season_releases: true,
        quality_definitions: Vec::new(),
        propers: ProperDownloadType::PreferAndUpgrade,
        release_profiles: Vec::new(),
        delay_profiles: vec![delay(DownloadProtocol::Usenet)],
        minimum_age_minutes: 0,
        retention_days: 0,
        maximum_size_mb: 0,
        now: Utc::now(),
    }
}

fn delay(preferred_protocol: DownloadProtocol) -> DelayProfile {
    DelayProfile {
        enable_usenet: true,
        enable_torrent: true,
        preferred_protocol,
        usenet_delay: 0,
        torrent_delay: 0,
        bypass_if_highest_quality: false,
        bypass_if_above_custom_format_score: false,
        minimum_custom_format_score: 0,
        order: 1,
        tags: Vec::new(),
    }
}

fn release(title: &str) -> Release {
    Release {
        guid: title.into(),
        title: title.into(),
        size: 0,
        publish_date: Utc::now(),
        protocol: DownloadProtocol::Usenet,
        indexer_id: 1,
        indexer_priority: 25,
        indexer_tags: Vec::new(),
        indexer_flags: 0,
        languages: Vec::new(),
        seeders: None,
        peers: None,
        tvdb_id: 0,
        tv_rage_id: 0,
        imdb_id: None,
        minimum_seeders: None,
        multi_languages: Vec::new(),
        indexer: "Indexer".into(),
        info_hash: None,
    }
}

fn quality(id: i32, version: i32, real: i32) -> ParsedQuality {
    ParsedQuality {
        quality: Quality::by_id(id).unwrap(),
        revision: Revision {
            version,
            real,
            is_repack: false,
        },
        ..ParsedQuality::unknown()
    }
}

fn episode(id: i64) -> MappedEpisode {
    MappedEpisode {
        id,
        season_number: 1,
        episode_number: id as i32,
        runtime: 0,
    }
}

/// `GivenRemoteEpisode` + `new DownloadDecision(remoteEpisode)`.
fn decision(index: usize, episodes: Vec<MappedEpisode>, q: ParsedQuality) -> Decision {
    Decision {
        release: index,
        parsed: Some(ParsedEpisodeInfo::default()),
        quality: q,
        languages: Vec::new(),
        series_id: Some(1),
        episodes,
        custom_formats: Vec::new(),
        custom_format_score: 0,
        rejections: Vec::new(),
    }
}

/// `PrioritizeDecisions`, returning the release indices best first.
fn rank(
    series: &Series,
    settings: &Settings,
    releases: &[Release],
    decisions: Vec<Decision>,
) -> Vec<usize> {
    let preferred = settings
        .delay_profiles
        .first()
        .map(|d| d.preferred_protocol);
    let comparer = Comparer {
        series,
        settings,
        releases,
        preferred_protocol: preferred,
    };
    prioritize(&comparer, decisions)
        .into_iter()
        .map(|d| d.release)
        .collect()
}

/// Two single-episode HDTV-720p usenet releases published now, sizes 0.
fn pair() -> (Series, Settings, Vec<Release>) {
    (series(), settings(), vec![release("a"), release("b")])
}

mod prioritize_download_decision_fixture {
    use super::*;

    #[test]
    fn should_put_reals_before_non_reals() {
        let (s, st, r) = pair();
        let d = vec![
            decision(0, vec![episode(1)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 1)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    #[test]
    fn should_put_propers_before_non_propers() {
        let (s, st, r) = pair();
        let d = vec![
            decision(0, vec![episode(1)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 2, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    #[test]
    fn should_put_higher_quality_before_lower() {
        let (s, st, r) = pair();
        let d = vec![
            decision(0, vec![episode(1)], quality(SDTV, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    #[test]
    fn should_order_by_lowest_number_of_episodes() {
        let (s, st, r) = pair();
        let d = vec![
            decision(0, vec![episode(2)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    #[test]
    fn should_order_by_lowest_number_of_episodes_with_multiple_episodes() {
        let (s, st, r) = pair();
        let d = vec![
            decision(0, vec![episode(2), episode(3)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1), episode(2)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    fn aged(title: &str, size_mb: i64, age_days: i64) -> Release {
        Release {
            size: size_mb * MB,
            publish_date: Utc::now() - Duration::days(age_days),
            ..release(title)
        }
    }

    #[test]
    fn should_order_by_age_then_largest_rounded_to_200mb() {
        let (s, st, _) = pair();
        let r = vec![
            aged("sd", 100, 1),
            aged("hd small old", 1200, 1000),
            aged("hd small young", 1250, 10),
            aged("hd large young", 3000, 1),
        ];
        let d = vec![
            decision(0, vec![episode(1)], quality(SDTV, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
            decision(2, vec![episode(1)], quality(HDTV720P, 1, 0)),
            decision(3, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 3);
    }

    /// `GivenPreferredSize`: Kitsunarr reads preferred sizes from the quality definitions.
    fn preferred(st: &mut Settings, mb_per_minute: f64) {
        st.quality_definitions = Quality::all()
            .iter()
            .map(|q| QualityDefinition {
                quality_id: q.id,
                title: q.name.into(),
                weight: q.weight,
                min_size: None,
                preferred_size: Some(OrderedF64(mb_per_minute)),
                max_size: None,
            })
            .collect();
    }

    #[test]
    fn should_order_by_closest_to_preferred_size_if_both_under() {
        let (s, mut st, _) = pair();
        preferred(&mut st, 200.0);
        let r = vec![aged("small", 1200, 1), aged("large", 10000, 1)];
        let d = vec![
            decision(0, vec![episode(1)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    #[test]
    fn should_order_by_closest_to_preferred_size_if_preferred_is_in_between() {
        let (s, mut st, _) = pair();
        preferred(&mut st, 46.0);
        let r = vec![
            aged("1", 500, 1),
            aged("2", 2000, 1),
            aged("3", 3000, 1),
            aged("4", 5000, 1),
        ];
        let d = (0..4)
            .map(|i| decision(i, vec![episode(1)], quality(HDTV720P, 1, 0)))
            .collect();
        assert_eq!(rank(&s, &st, &r, d)[0], 2);
    }

    #[test]
    fn should_order_by_closest_to_preferred_size_of_the_whole_release() {
        let (s, mut st, _) = pair();
        preferred(&mut st, 46.0);
        let episodes: Vec<MappedEpisode> = (1..=10)
            .map(|i| MappedEpisode {
                runtime: 60,
                ..episode(i)
            })
            .collect();
        let r = vec![aged("small", 12000, 1), aged("large", 26000, 1)];
        let d = vec![
            decision(0, episodes.clone(), quality(HDTV720P, 1, 0)),
            decision(1, episodes, quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    #[test]
    fn should_order_by_youngest() {
        let (s, st, _) = pair();
        let r = vec![aged("old", 0, 10), aged("young", 0, 5)];
        let d = vec![
            decision(0, vec![episode(1)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    #[test]
    fn should_not_throw_if_no_episodes_are_found() {
        let (s, st, _) = pair();
        let r = vec![aged("a", 500, 0), aged("b", 500, 0)];
        let d = vec![
            decision(0, vec![], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d).len(), 2);
    }

    fn protocols(preferred: DownloadProtocol) -> usize {
        let (s, mut st, mut r) = pair();
        st.delay_profiles = vec![delay(preferred)];
        r[0].protocol = DownloadProtocol::Torrent;
        r[1].protocol = DownloadProtocol::Usenet;
        let d = vec![
            decision(0, vec![episode(1)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        rank(&s, &st, &r, d)[0]
    }

    #[test]
    fn should_put_usenet_above_torrent_when_usenet_is_preferred() {
        assert_eq!(protocols(DownloadProtocol::Usenet), 1);
    }

    #[test]
    fn should_put_torrent_above_usenet_when_torrent_is_preferred() {
        assert_eq!(protocols(DownloadProtocol::Torrent), 0);
    }

    #[test]
    fn should_prefer_season_pack_above_single_episode() {
        let (s, st, r) = pair();
        let mut pack = decision(0, vec![episode(1), episode(2)], quality(HDTV720P, 1, 0));
        pack.parsed.as_mut().unwrap().full_season = true;
        let d = vec![pack, decision(1, vec![episode(1)], quality(HDTV720P, 1, 0))];
        assert_eq!(rank(&s, &st, &r, d)[0], 0);
    }

    #[test]
    fn should_prefer_multiepisode_over_single_episode_for_anime() {
        let (mut s, st, r) = pair();
        s.series_type = SeriesType::Anime;
        let d = vec![
            decision(0, vec![episode(1), episode(2)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 0);
    }

    #[test]
    fn should_prefer_single_episode_over_multi_episode_for_non_anime() {
        let (s, st, r) = pair();
        let d = vec![
            decision(0, vec![episode(1), episode(2)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    fn torrents(
        a: (Option<i64>, Option<i64>, i64),
        b: (Option<i64>, Option<i64>, i64),
        qualities: (i32, i32),
    ) -> usize {
        let (s, st, _) = pair();
        let torrent = |(seeders, peers, size_mb): (Option<i64>, Option<i64>, i64)| Release {
            protocol: DownloadProtocol::Torrent,
            seeders,
            peers,
            size: size_mb * MB,
            ..release("t")
        };
        let r = vec![torrent(a), torrent(b)];
        let d = vec![
            decision(0, vec![episode(1)], quality(qualities.0, 1, 0)),
            decision(1, vec![episode(1)], quality(qualities.1, 1, 0)),
        ];
        rank(&s, &st, &r, d)[0]
    }

    #[test]
    fn should_prefer_releases_with_more_seeders() {
        let hd = (HDTV720P, HDTV720P);
        assert_eq!(torrents((Some(10), None, 0), (Some(100), None, 0), hd), 1);
    }

    #[test]
    fn should_prefer_releases_with_more_peers_given_equal_number_of_seeds() {
        let hd = (HDTV720P, HDTV720P);
        assert_eq!(
            torrents((Some(10), Some(10), 0), (Some(10), Some(100), 0), hd),
            1
        );
    }

    #[test]
    fn should_prefer_releases_with_more_peers_no_seeds() {
        let hd = (HDTV720P, HDTV720P);
        assert_eq!(
            torrents((Some(0), Some(10), 0), (Some(0), Some(100), 0), hd),
            1
        );
    }

    #[test]
    fn should_prefer_first_release_if_peers_and_size_are_too_similar() {
        let hd = (HDTV720P, HDTV720P);
        assert_eq!(
            torrents((Some(1000), Some(10), 250), (Some(1100), Some(10), 200), hd),
            0
        );
    }

    #[test]
    fn should_prefer_first_release_if_age_and_size_are_too_similar() {
        let (s, st, _) = pair();
        let r = vec![aged("a", 200, 100), aged("b", 250, 150)];
        let d = vec![
            decision(0, vec![episode(1)], quality(HDTV720P, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 0);
    }

    #[test]
    fn should_prefer_quality_over_the_number_of_peers() {
        let q = (BLURAY1080P, SDTV);
        assert_eq!(
            torrents((Some(100), Some(10), 0), (Some(1100), Some(10), 0), q),
            0
        );
    }

    #[test]
    fn should_put_higher_quality_before_lower_always() {
        let (s, st, r) = pair();
        let d = vec![
            decision(0, vec![episode(1)], quality(SDTV, 1, 0)),
            decision(1, vec![episode(1)], quality(HDTV720P, 1, 0)),
        ];
        assert_eq!(rank(&s, &st, &r, d)[0], 1);
    }

    fn scored(propers: ProperDownloadType, first: ParsedQuality, second: ParsedQuality) -> usize {
        let (s, mut st, r) = pair();
        st.propers = propers;
        let mut a = decision(0, vec![episode(1)], first);
        a.custom_format_score = 10;
        let b = decision(1, vec![episode(1)], second);
        rank(&s, &st, &r, vec![a, b])[0]
    }

    #[test]
    fn should_prefer_higher_score_over_lower_score() {
        let q = quality(WEBDL1080P, 1, 0);
        assert_eq!(scored(ProperDownloadType::PreferAndUpgrade, q, q), 0);
    }

    #[test]
    fn should_prefer_proper_over_score_when_download_propers_is_prefer_and_upgrade() {
        let p = ProperDownloadType::PreferAndUpgrade;
        assert_eq!(
            scored(p, quality(WEBDL1080P, 1, 0), quality(WEBDL1080P, 2, 0)),
            1
        );
    }

    #[test]
    fn should_prefer_proper_over_score_when_download_propers_is_do_not_upgrade() {
        let p = ProperDownloadType::DoNotUpgrade;
        assert_eq!(
            scored(p, quality(WEBDL1080P, 1, 0), quality(WEBDL1080P, 2, 0)),
            1
        );
    }

    #[test]
    fn should_prefer_score_over_proper_when_download_propers_is_do_not_prefer() {
        let p = ProperDownloadType::DoNotPrefer;
        assert_eq!(
            scored(p, quality(WEBDL1080P, 1, 0), quality(WEBDL1080P, 2, 0)),
            0
        );
    }

    #[test]
    fn should_prefer_score_over_real_when_download_propers_is_do_not_prefer() {
        let p = ProperDownloadType::DoNotPrefer;
        assert_eq!(
            scored(p, quality(WEBDL1080P, 1, 0), quality(WEBDL1080P, 1, 1)),
            0
        );
    }

    fn priorities(entries: &[(i32, i32)]) -> Vec<usize> {
        let (s, st, _) = pair();
        let r: Vec<Release> = entries
            .iter()
            .map(|(_, p)| Release {
                indexer_priority: *p,
                ..release("r")
            })
            .collect();
        let d = entries
            .iter()
            .enumerate()
            .map(|(i, (q, _))| decision(i, vec![episode(1)], quality(*q, 1, 0)))
            .collect();
        rank(&s, &st, &r, d)
    }

    #[test]
    fn sort_download_decisions_based_on_indexer_priority() {
        let got = priorities(&[(WEBDL1080P, 25), (WEBDL1080P, 50), (WEBDL1080P, 1)]);
        assert_eq!(got, vec![2, 0, 1]);
    }

    #[test]
    fn ensure_download_decisions_indexer_priority_is_not_perfered_over_quality() {
        let got = priorities(&[
            (HDTV720P, 25),
            (WEBDL1080P, 50),
            (SDTV, 1),
            (WEBDL1080P, 25),
        ]);
        assert_eq!(got, vec![3, 1, 0, 2]);
    }
}

/// A subject for the specifications: release, parse result and episodes.
struct Case {
    series: Series,
    settings: Settings,
    release: Release,
    parsed: ParsedEpisodeInfo,
    episodes: Vec<Episode>,
    criteria: SearchCriteria,
    /// The release's custom format score.
    score: i32,
}

fn ep(id: i64, season: i32, number: i32, runtime: i32) -> Episode {
    Episode {
        id,
        season_number: season,
        episode_number: number,
        absolute_episode_number: None,
        scene_season_number: None,
        scene_episode_number: None,
        title: None,
        air_date: None,
        air_date_utc: None,
        runtime,
        monitored: true,
        file: None,
    }
}

impl Case {
    fn new() -> Self {
        Self {
            series: series(),
            settings: settings(),
            release: release("Series.Title.S01E01.720p.HDTV.x264-GRP"),
            parsed: ParsedEpisodeInfo {
                quality: quality(SDTV, 2, 0),
                season_numbers: vec![1],
                episode_numbers: vec![1],
                ..Default::default()
            },
            episodes: vec![ep(5, 1, 1, 30)],
            criteria: SearchCriteria {
                monitored_episodes_only: false,
                kind: SearchKind::SingleEpisode {
                    season: 1,
                    episode: 1,
                },
                episode_ids: vec![5],
            },
            score: 0,
        }
    }

    fn check(&self, spec: fn(&Env<'_>, &Subject<'_>) -> Option<Rejection>) -> Option<Rejection> {
        let env = Env {
            series: &self.series,
            settings: &self.settings,
            formats: &[],
            criteria: &self.criteria,
        };
        let subject = Subject {
            release: &self.release,
            parsed: &self.parsed,
            episodes: self.episodes.iter().collect(),
            mapped_season: self.parsed.season_numbers.first().copied(),
            custom_formats: Vec::new(),
            score: self.score,
        };
        spec(&env, &subject)
    }

    fn accepted(&self, spec: fn(&Env<'_>, &Subject<'_>) -> Option<Rejection>) -> bool {
        self.check(spec).is_none()
    }
}

/// `AcceptableSizeSpecificationFixture` (Sonarr v4: sizes from the quality definition, here
/// SDTV with min 2 and max 10 MB per minute).
mod acceptable_size_specification_fixture {
    use super::*;

    fn case(runtime: i32, size_mb: i64, episodes: usize) -> Case {
        let mut c = Case::new();
        c.series.runtime = runtime;
        c.settings.quality_definitions = vec![QualityDefinition {
            quality_id: SDTV,
            title: "SDTV".into(),
            weight: 2,
            min_size: Some(OrderedF64(2.0)),
            preferred_size: None,
            max_size: Some(OrderedF64(10.0)),
        }];
        c.release.size = size_mb * MB;
        c.episodes = (0..episodes)
            .map(|i| ep(5 + i as i64, 1, 1 + i as i32, runtime))
            .collect();
        c
    }

    #[test]
    fn single_episode() {
        for (runtime, size, expected) in [
            (30, 50, false),
            (30, 250, true),
            (30, 500, false),
            (60, 100, false),
            (60, 500, true),
            (60, 1000, false),
        ] {
            let c = case(runtime, size, 1);
            assert_eq!(
                c.accepted(specs::acceptable_size),
                expected,
                "{runtime} {size}"
            );
        }
    }

    #[test]
    fn multi_episode() {
        for (runtime, size, expected) in [
            (30, 50 * 2, false),
            (30, 250 * 2, true),
            (30, 500 * 2, false),
            (60, 100 * 2, false),
            (60, 500 * 2, true),
            (60, 1000 * 2, false),
        ] {
            let c = case(runtime, size, 2);
            assert_eq!(
                c.accepted(specs::acceptable_size),
                expected,
                "{runtime} {size}"
            );
        }
    }

    #[test]
    fn multiset_episode() {
        for (runtime, size, expected) in [
            (30, 50 * 6, false),
            (30, 250 * 6, true),
            (30, 500 * 6, false),
            (60, 100 * 6, false),
            (60, 500 * 6, true),
            (60, 1000 * 6, false),
        ] {
            let c = case(runtime, size, 6);
            assert_eq!(
                c.accepted(specs::acceptable_size),
                expected,
                "{runtime} {size}"
            );
        }
    }

    #[test]
    fn should_return_true_if_size_is_zero() {
        let c = case(30, 0, 1);
        assert!(c.accepted(specs::acceptable_size));
    }

    #[test]
    fn should_return_true_if_unlimited() {
        for (runtime, bytes) in [(30, 18_457_280_000i64), (60, 36_857_280_000)] {
            let mut c = case(runtime, 0, 1);
            c.release.size = bytes;
            c.settings.quality_definitions[0].max_size = None;
            assert!(c.accepted(specs::acceptable_size));
        }
    }

    #[test]
    fn should_reject_when_runtime_is_unknown_and_explain_sizes() {
        let mut c = case(0, 500, 1);
        c.episodes[0].runtime = 0;
        // The episode is not in the first season, so the 45-minute fallback does not apply.
        c.episodes[0].season_number = 2;
        c.series.episodes = vec![ep(1, 1, 1, 0), ep(5, 2, 1, 0)];
        let r = c.check(specs::acceptable_size).unwrap();
        assert_eq!(r.reason, RejectionReason::UnknownRuntime);
        let c = case(30, 50, 1);
        assert_eq!(
            c.check(specs::acceptable_size).unwrap().message,
            "50.0 MB is smaller than minimum allowed 60.0 MB (for 30min)"
        );
        let c = case(30, 500 * 2, 2);
        assert_eq!(
            c.check(specs::acceptable_size).unwrap().message,
            "1,000.0 MB is larger than maximum allowed 600.0 MB (for 2 episodes totalling 60min)"
        );
    }
}

/// `RawDiskSpecificationFixture`.
#[test]
fn raw_disk_specification_fixture() {
    for (title, raw) in [
        (
            "Series Title S02 Disc 1 1080i Blu-ray DTS-HD MA 2.0 AVC-TrollHD",
            true,
        ),
        (
            "Series Title S03 Disc 1 1080p Blu-ray LPCM 2.0 AVC-TrollHD",
            true,
        ),
        ("SERIES TITLE S02 1080P FULL BLURAY AVC DTS-HD MA 5 1", true),
        (
            "Series.Title.S06.2016.DISC.3.BluRay.1080p.AVC.Atmos.TrueHD7.1-MTeam",
            true,
        ),
        (
            "Series Title S05 Disc 1 BluRay 1080p AVC Atmos TrueHD 7 1-MTeam",
            true,
        ),
        (
            "Someone.the.Entertainer.Presents.S01.NTSC.3xDVD9.MPEG-2.DD2.0",
            true,
        ),
        (
            "Series.Title.S00.The.Christmas.Special.2011.PAL.DVD5.DD2.0",
            true,
        ),
        ("Series.of.Desire.2000.S1_D01.NTSC.DVD5", true),
        ("Series Title EP50 USLT NTSC DVDRemux DD2.0", false),
        ("Series.Title.S01.NTSC.DVDRip.DD2.0.x264-PLAiD", false),
        ("Series.Title.S01E01.720p.HDTV.x264-GRP", false),
    ] {
        let mut c = Case::new();
        c.release.title = title.into();
        assert_eq!(c.accepted(specs::raw_disk), !raw, "{title}");
    }
}

/// `ReleaseRestrictionsSpecificationFixture`.
mod release_restrictions_specification_fixture {
    use super::*;

    fn case(required: &[&str], ignored: &[&str]) -> Case {
        let mut c = Case::new();
        c.release.title = "Dexter.S08E01.EDITED.WEBRip.x264-KYR".into();
        c.settings.release_profiles = vec![ReleaseProfile {
            enabled: true,
            required: required.iter().map(|s| s.to_string()).collect(),
            ignored: ignored.iter().map(|s| s.to_string()).collect(),
            indexer_ids: Vec::new(),
            tags: Vec::new(),
        }];
        c
    }

    #[test]
    fn should_be_true_when_restrictions_are_empty() {
        assert!(case(&[], &[]).accepted(specs::release_restrictions));
    }

    #[test]
    fn required_terms() {
        // should_be_true_when_one_of_the_required_terms_is_present (and case-insensitive)
        for terms in [
            &["EdiTED"][..],
            &["webrip"],
            &["X264"],
            &["X264", "NOTTHERE"],
        ] {
            assert!(
                case(terms, &[]).accepted(specs::release_restrictions),
                "{terms:?}"
            );
        }
        assert!(!case(&["doesnt", "exist"], &[]).accepted(specs::release_restrictions));
    }

    #[test]
    fn ignored_terms() {
        // should_be_false_when_one_of_the_ignored_terms_is_present
        for terms in [
            &["EdiTED"][..],
            &["webrip"],
            &["X264"],
            &["X264", "NOTTHERE"],
        ] {
            assert!(
                !case(&[], terms).accepted(specs::release_restrictions),
                "{terms:?}"
            );
        }
        assert!(case(&[], &["ignored"]).accepted(specs::release_restrictions));
        let r = case(&[], &["EdiTED", "x264"])
            .check(specs::release_restrictions)
            .unwrap();
        assert_eq!(r.message, "Contains these ignored terms: EdiTED, x264");
    }

    #[test]
    fn regex_terms() {
        for (pattern, expected) in [
            ("/WEB/", true),
            ("/WEB\\b/", false),
            ("/WEb/", false),
            ("/\\.WEB/", true),
            // Not upstream rows: the `i` modifier and alternation.
            ("/WEb/i", true),
            ("/edited|kyr/i", true),
        ] {
            assert_eq!(
                case(&[pattern], &[]).accepted(specs::release_restrictions),
                expected,
                "{pattern}"
            );
        }
        let r = case(&["/x/q"], &[])
            .check(specs::release_restrictions)
            .unwrap();
        assert_eq!(r.reason, RejectionReason::DecisionError);
    }

    #[test]
    fn profiles_apply_by_tag_and_indexer() {
        let mut c = case(&["missing"], &[]);
        c.settings.release_profiles[0].tags = vec![7];
        assert!(
            c.accepted(specs::release_restrictions),
            "tag not on the series"
        );
        c.series.tags = vec![7];
        assert!(!c.accepted(specs::release_restrictions));
        c.settings.release_profiles[0].indexer_ids = vec![2];
        assert!(c.accepted(specs::release_restrictions), "another indexer");
    }
}

#[test]
fn usenet_age_and_size_limits() {
    // `RetentionSpecificationFixture`, `MinimumAgeSpecificationFixture`,
    // `MaximumSizeSpecificationFixture`.
    let mut c = Case::new();
    c.release.publish_date = Utc::now() - Duration::days(100);
    assert!(c.accepted(specs::retention), "unlimited retention");
    c.settings.retention_days = 1000;
    assert!(c.accepted(specs::retention));
    c.settings.retention_days = 50;
    assert!(!c.accepted(specs::retention));
    c.release.protocol = DownloadProtocol::Torrent;
    assert!(c.accepted(specs::retention), "torrents have no retention");

    let mut c = Case::new();
    c.settings.minimum_age_minutes = 30;
    c.release.publish_date = Utc::now() - Duration::minutes(10);
    assert!(!c.accepted(specs::minimum_age));
    c.release.publish_date = Utc::now() - Duration::minutes(60);
    assert!(c.accepted(specs::minimum_age));

    let mut c = Case::new();
    c.release.size = 2000 * MB;
    assert!(c.accepted(specs::maximum_size), "0 = unlimited");
    c.settings.maximum_size_mb = 1000;
    assert!(!c.accepted(specs::maximum_size));
    c.release.size = 0;
    assert!(c.accepted(specs::maximum_size), "unknown size");
}

#[test]
fn protocol_seeders_and_tags() {
    // `ProtocolSpecificationFixture`.
    let mut c = Case::new();
    c.settings.delay_profiles[0].enable_usenet = false;
    assert_eq!(
        c.check(specs::protocol).unwrap().message,
        "Usenet is not enabled for this series"
    );
    c.release.protocol = DownloadProtocol::Torrent;
    assert!(c.accepted(specs::protocol));
    // `TorrentSeedingSpecificationFixture`.
    c.release.minimum_seeders = Some(5);
    c.release.seeders = Some(4);
    assert!(!c.accepted(specs::torrent_seeding));
    c.release.seeders = Some(5);
    assert!(c.accepted(specs::torrent_seeding));
    c.release.seeders = None;
    assert!(c.accepted(specs::torrent_seeding), "unknown seeders");
    // `IndexerTagSpecificationFixture`.
    c.release.indexer_tags = vec![3];
    assert!(!c.accepted(specs::indexer_tag));
    c.series.tags = vec![3, 4];
    assert!(c.accepted(specs::indexer_tag));
}

/// `Search/*` fixtures.
#[test]
fn search_match_specifications() {
    let mut c = Case::new();
    // StandardEpisodeSearch: season and episode must match the request.
    assert!(c.accepted(specs::single_episode_search_match));
    c.parsed.episode_numbers = vec![2];
    assert_eq!(
        c.check(specs::single_episode_search_match).unwrap().message,
        "Wrong episode"
    );
    c.parsed.season_numbers = vec![2];
    assert_eq!(
        c.check(specs::single_episode_search_match).unwrap().message,
        "Wrong season"
    );
    c.parsed.season_numbers = vec![1];
    c.parsed.episode_numbers = vec![];
    assert_eq!(
        c.check(specs::single_episode_search_match).unwrap().message,
        "Full season pack"
    );
    // AnimeSearchFixture: season packs only for season searches.
    c.criteria.kind = SearchKind::AnimeEpisode {
        season_search: false,
    };
    c.parsed.full_season = true;
    assert!(!c.accepted(specs::single_episode_search_match));
    c.criteria.kind = SearchKind::AnimeEpisode {
        season_search: true,
    };
    assert!(c.accepted(specs::single_episode_search_match));
    // SeasonMatchSpecificationFixture.
    c.criteria.kind = SearchKind::Season { season: 2 };
    assert!(!c.accepted(specs::season_match));
    c.parsed.season_numbers = vec![1, 2];
    assert!(c.accepted(specs::season_match));
    // EpisodeRequestedSpecification.
    let mut c = Case::new();
    c.criteria.episode_ids = vec![99];
    assert_eq!(
        c.check(specs::episode_requested).unwrap().message,
        "Episode wasn't requested: 1x1"
    );
}

#[test]
fn quality_and_format_score_against_profile() {
    // `QualityAllowedByProfileSpecificationFixture`.
    let mut c = Case::new();
    for item in &mut c.series.profile.items {
        item.allowed = item.quality_id != Some(SDTV);
    }
    assert_eq!(
        c.check(specs::quality_allowed_by_profile).unwrap().message,
        "SDTV is not wanted in profile"
    );
    c.parsed.quality = quality(HDTV720P, 1, 0);
    assert!(c.accepted(specs::quality_allowed_by_profile));
    // `CustomFormatAllowedByProfileSpecificationFixture`: score 0 against a minimum of 1.
    c.series.profile.min_format_score = 1;
    assert_eq!(
        c.check(specs::custom_format_allowed_by_profile)
            .unwrap()
            .message,
        "Custom Formats  have score 0 below Series profile minimum 1"
    );
}

/// `UpgradeDiskSpecificationFixture` (the common cases).
mod upgrade_disk_specification_fixture {
    use super::*;

    fn with_file(file_quality: i32, file_version: i32) -> Case {
        let mut c = Case::new();
        c.series.profile.upgrade_allowed = true;
        c.series.profile.cutoff = BLURAY1080P;
        c.episodes[0].file = Some(EpisodeFile {
            id: 9,
            quality: QualityModel {
                quality_id: file_quality,
                revision: Revision {
                    version: file_version,
                    ..Default::default()
                },
            },
            release_title: "Series.Title.S01E01".into(),
            file_name: "Series.Title.S01E01.mkv".into(),
            release_group: Some("GRP".into()),
            languages: Vec::new(),
            size: 0,
            indexer_flags: 0,
            release_type: ReleaseType::SingleEpisode,
            date_added: None,
        });
        c.series.episodes = c.episodes.clone();
        c
    }

    #[test]
    fn should_return_true_if_episode_has_no_existing_file() {
        assert!(Case::new().accepted(specs::upgrade_disk));
    }

    #[test]
    fn should_be_upgradable_if_only_episode_is_upgradable() {
        let mut c = with_file(SDTV, 1);
        c.parsed.quality = quality(HDTV720P, 1, 0);
        assert!(c.accepted(specs::upgrade_disk));
    }

    #[test]
    fn should_not_be_upgradable_if_qualities_are_the_same() {
        let mut c = with_file(WEBDL1080P, 1);
        c.parsed.quality = quality(WEBDL1080P, 1, 0);
        let r = c.check(specs::upgrade_disk).unwrap();
        assert_eq!(r.reason, RejectionReason::DiskCustomFormatScore);
    }

    #[test]
    fn should_not_be_upgradable_if_file_is_better() {
        let mut c = with_file(WEBDL1080P, 1);
        c.parsed.quality = quality(SDTV, 1, 0);
        let r = c.check(specs::upgrade_disk).unwrap();
        assert_eq!(
            r.message,
            "Existing file on disk is of equal or higher preference: WEBDL-1080p v1"
        );
    }

    #[test]
    fn should_reject_when_cutoff_is_met() {
        let mut c = with_file(BLURAY1080P, 1);
        c.parsed.quality = quality(BLURAY1080P, 1, 0);
        let r = c.check(specs::upgrade_disk).unwrap();
        assert_eq!(r.message, "Existing file meets cutoff: Bluray-1080p");
    }

    #[test]
    fn should_be_upgradable_for_a_proper_of_the_file_quality() {
        let mut c = with_file(WEBDL1080P, 1);
        c.parsed.quality = quality(WEBDL1080P, 2, 0);
        assert!(c.accepted(specs::upgrade_disk));
        // `RepackSpecificationFixture`: a repack must come from the file's group.
        c.parsed.quality.revision.is_repack = true;
        c.parsed.release_group = Some("OTHER".into());
        assert_eq!(
            c.check(specs::repack).unwrap().reason,
            RejectionReason::RepackReleaseGroupDoesNotMatch
        );
        c.parsed.release_group = Some("grp".into());
        assert!(c.accepted(specs::repack));
        c.settings.propers = ProperDownloadType::DoNotUpgrade;
        assert_eq!(
            c.check(specs::repack).unwrap().reason,
            RejectionReason::RepackDisabled
        );
    }

    #[test]
    fn season_packs_need_every_episode_missing_or_upgradable() {
        let mut c = with_file(BLURAY1080P, 1);
        c.parsed.full_season = true;
        c.parsed.episode_numbers = vec![];
        c.parsed.quality = quality(HDTV720P, 1, 0);
        c.episodes.push(ep(6, 1, 2, 30));
        c.series.episodes = c.episodes.clone();
        assert_eq!(
            c.check(specs::upgrade_disk).unwrap().message,
            "Season pack does not meet the upgrade criteria. Upgradable: 1/2 (50%), Mode: All, Threshold: 100%"
        );
        c.episodes[0].file = None;
        assert!(c.accepted(specs::upgrade_disk));
    }
}

#[test]
fn not_sample_multi_season_split_and_full_season() {
    let mut c = Case::new();
    c.release.title = "Series.Title.S01E01.sample.mkv".into();
    c.release.size = 10 * MB;
    assert_eq!(c.check(specs::not_sample).unwrap().message, "Sample");
    c.release.size = 100 * MB;
    assert!(c.accepted(specs::not_sample));
    // Tanukarr's `MultiSeasonSpecificationFixture`: accepted when enabled and mapped.
    c.parsed.season_numbers = vec![1, 2];
    c.episodes = vec![ep(1, 1, 1, 30)];
    assert!(c.accepted(specs::multi_season));
    c.settings.enable_multi_season_releases = false;
    assert_eq!(
        c.check(specs::multi_season).unwrap().message,
        "Multi-season releases are not enabled"
    );
    c.settings.enable_multi_season_releases = true;
    c.episodes.clear();
    assert_eq!(
        c.check(specs::multi_season).unwrap().message,
        "Multi-season release does not match any wanted episodes"
    );
    c.parsed.is_split_episode = true;
    assert!(!c.accepted(specs::split_episode));
    // `FullSeasonSpecificationFixture`: every episode must have aired a day ago or earlier.
    let mut c = Case::new();
    c.parsed.full_season = true;
    c.episodes = vec![ep(1, 1, 1, 30), ep(2, 1, 2, 30)];
    c.episodes[0].air_date_utc = Some(Utc::now() - Duration::days(7));
    c.episodes[1].air_date_utc = Some(Utc::now() + Duration::days(2));
    assert!(!c.accepted(specs::full_season));
    c.episodes[1].air_date_utc = Some(Utc::now() - Duration::days(1));
    assert!(c.accepted(specs::full_season));
    c.episodes[1].air_date_utc = None;
    assert!(!c.accepted(specs::full_season));
}

/// `MonitoredEpisodeSpecificationFixture`.
#[test]
fn monitored_episode_specification_fixture() {
    let mut c = Case::new();
    c.criteria.monitored_episodes_only = true;
    c.episodes = vec![ep(1, 1, 1, 30), ep(2, 1, 2, 30)];
    assert!(c.accepted(specs::monitored_episode), "all monitored");
    c.series.monitored = false;
    assert_eq!(
        c.check(specs::monitored_episode).unwrap().message,
        "Series is not monitored"
    );
    c.series.monitored = true;
    c.episodes[1].monitored = false;
    assert_eq!(
        c.check(specs::monitored_episode).unwrap().message,
        "One or more episodes is not monitored"
    );
    c.episodes[0].monitored = false;
    assert!(!c.accepted(specs::monitored_episode), "both unmonitored");
    c.episodes.truncate(1);
    assert_eq!(
        c.check(specs::monitored_episode).unwrap().message,
        "Episode is not monitored"
    );
    // Searches that are not limited to monitored episodes (single episode, interactive).
    c.criteria.monitored_episodes_only = false;
    c.series.monitored = false;
    assert!(c.accepted(specs::monitored_episode));
}

/// `QueueSpecificationFixture` (the subject is a DVD release of episode 5).
mod queue_specification_fixture {
    use super::*;

    fn case(queued: &[(i64, Vec<i64>, i32, i32)]) -> Case {
        let mut c = Case::new();
        c.series.profile.upgrade_allowed = true;
        c.series.profile.cutoff = BLURAY1080P;
        c.parsed.quality = quality(2, 1, 0);
        c.settings.queue = queued
            .iter()
            .map(|(series_id, episodes, q, version)| QueuedRelease {
                series_id: *series_id,
                episode_ids: episodes.clone(),
                title: "Series.Title.S01E01.GRP".into(),
                quality: QualityModel {
                    quality_id: *q,
                    revision: Revision {
                        version: *version,
                        ..Default::default()
                    },
                },
                size: 0,
                languages: Vec::new(),
                indexer_flags: 0,
            })
            .collect();
        c
    }

    const DVD: i32 = 2;

    #[test]
    fn queue_matching() {
        assert!(case(&[]).accepted(specs::queue), "empty queue");
        assert!(
            case(&[(2, vec![5], DVD, 1)]).accepted(specs::queue),
            "other series"
        );
        assert!(
            case(&[(1, vec![6], DVD, 1)]).accepted(specs::queue),
            "other episode"
        );
        assert!(
            case(&[(1, vec![5], SDTV, 1)]).accepted(specs::queue),
            "lower quality queued"
        );
        assert_eq!(
            case(&[(1, vec![5], DVD, 1)])
                .check(specs::queue)
                .unwrap()
                .reason,
            RejectionReason::QueueCustomFormatScore,
            "everything the same"
        );
        assert_eq!(
            case(&[(1, vec![5], HDTV720P, 1)])
                .check(specs::queue)
                .unwrap()
                .message,
            "Release in queue is of equal or higher preference: HDTV-720p v1"
        );
        // A multi-episode download holding one of the episodes.
        assert!(!case(&[(1, vec![4, 5], HDTV720P, 1)]).accepted(specs::queue));
    }

    #[test]
    fn cutoff_and_upgrades() {
        let mut c = case(&[(1, vec![5], HDTV720P, 1)]);
        c.series.profile.cutoff = DVD;
        assert_eq!(
            c.check(specs::queue).unwrap().message,
            "Release in queue already meets cutoff: HDTV-720p v1"
        );
        let mut c = case(&[(1, vec![5], BLURAY1080P, 1)]);
        c.series.profile.upgrade_allowed = false;
        assert!(!c.accepted(specs::queue));
        // should_return_false_if_same_quality_non_proper_in_queue_and_download_propers_is_do_not_upgrade
        let mut c = case(&[(1, vec![5], HDTV720P, 1)]);
        c.parsed.quality = quality(HDTV720P, 2, 0);
        c.series.profile.cutoff = HDTV720P;
        c.settings.propers = ProperDownloadType::DoNotUpgrade;
        assert!(!c.accepted(specs::queue));
    }
}

fn file_of(quality_id: i32, version: i32) -> EpisodeFile {
    EpisodeFile {
        id: 1,
        quality: QualityModel {
            quality_id,
            revision: Revision {
                version,
                ..Default::default()
            },
        },
        release_title: "Series.Title.S01E01.720p.HDTV.x264-Sonarr".into(),
        file_name: "Series.Title.S01E01.mkv".into(),
        release_group: None,
        languages: Vec::new(),
        size: 0,
        indexer_flags: 0,
        release_type: ReleaseType::SingleEpisode,
        date_added: Some(Utc::now()),
    }
}

const DVD: i32 = 2;
const WEBDL720P: i32 = 5;
const BLURAY720P: i32 = 6;
const HDTV1080P: i32 = 9;

/// `RssSync/DelaySpecificationFixture`.
mod delay_specification_fixture {
    use super::*;

    fn case() -> Case {
        let mut c = Case::new();
        c.criteria = SearchCriteria::rss();
        c.series.profile.items = [HDTV720P, WEBDL720P, BLURAY720P]
            .iter()
            .map(|q| QualityProfileItem {
                quality_id: Some(*q),
                group: None,
                allowed: true,
            })
            .collect();
        c.series.profile.cutoff = WEBDL720P;
        c.release.protocol = DownloadProtocol::Usenet;
        // `new ReleaseInfo()`: no publish date (year 1).
        c.release.publish_date = chrono::DateTime::<Utc>::MIN_UTC;
        c.parsed.quality = quality(SDTV, 1, 0);
        c
    }

    fn delayed(c: &mut Case, minutes: i64) {
        c.settings.delay_profiles[0].usenet_delay = minutes;
    }

    #[test]
    fn should_be_true_when_user_invoked_search() {
        let mut c = case();
        delayed(&mut c, 720);
        c.release.publish_date = Utc::now();
        c.criteria = Case::new().criteria;
        assert!(c.accepted(specs::delay));
    }

    #[test]
    fn should_be_true_when_profile_does_not_have_a_delay() {
        let mut c = case();
        delayed(&mut c, 0);
        assert!(c.accepted(specs::delay));
    }

    #[test]
    fn should_be_false_when_quality_and_language_is_last_allowed_in_profile_and_bypass_disabled() {
        let mut c = case();
        c.release.publish_date = Utc::now();
        c.parsed.quality = quality(BLURAY720P, 1, 0);
        delayed(&mut c, 720);
        assert_eq!(
            c.check(specs::delay).unwrap().message,
            "Waiting for better quality release"
        );
    }

    #[test]
    fn should_be_true_when_quality_and_language_is_last_allowed_in_profile_and_bypass_enabled() {
        let mut c = case();
        delayed(&mut c, 720);
        c.settings.delay_profiles[0].bypass_if_highest_quality = true;
        c.release.publish_date = Utc::now();
        c.parsed.quality = quality(BLURAY720P, 1, 0);
        assert!(c.accepted(specs::delay));
    }

    #[test]
    fn should_be_true_when_release_is_older_than_delay() {
        let mut c = case();
        c.parsed.quality = quality(HDTV720P, 1, 0);
        c.release.publish_date = Utc::now() - Duration::hours(10);
        delayed(&mut c, 60);
        assert!(c.accepted(specs::delay));
    }

    #[test]
    fn should_be_false_when_release_is_younger_than_delay() {
        let mut c = case();
        c.release.publish_date = Utc::now();
        delayed(&mut c, 720);
        assert!(!c.accepted(specs::delay));
    }

    fn revision_case(propers: ProperDownloadType, new: ParsedQuality) -> Case {
        let mut c = case();
        c.settings.propers = propers;
        c.parsed.quality = new;
        c.release.publish_date = Utc::now();
        c.episodes[0].file = Some(file_of(HDTV720P, 1));
        delayed(&mut c, 720);
        c
    }

    #[test]
    fn should_be_true_when_release_is_a_proper_for_existing_episode() {
        let c = revision_case(
            ProperDownloadType::PreferAndUpgrade,
            quality(HDTV720P, 2, 0),
        );
        assert!(c.accepted(specs::delay));
    }

    #[test]
    fn should_be_true_when_release_is_a_real_for_existing_episode() {
        let c = revision_case(
            ProperDownloadType::PreferAndUpgrade,
            quality(HDTV720P, 1, 1),
        );
        assert!(c.accepted(specs::delay));
    }

    #[test]
    fn should_be_false_when_repacks_are_not_preferred() {
        let c = revision_case(ProperDownloadType::DoNotUpgrade, quality(HDTV720P, 2, 0));
        assert!(!c.accepted(specs::delay));
    }

    #[test]
    fn should_be_false_when_release_is_proper_for_existing_episode_of_different_quality() {
        let c = revision_case(
            ProperDownloadType::PreferAndUpgrade,
            quality(WEBDL720P, 2, 0),
        );
        assert!(!c.accepted(specs::delay));
    }

    fn scored(score: i32, bypass: bool) -> Case {
        let mut c = case();
        c.release.publish_date = Utc::now();
        c.score = score;
        delayed(&mut c, 720);
        c.settings.delay_profiles[0].bypass_if_above_custom_format_score = bypass;
        c.settings.delay_profiles[0].minimum_custom_format_score = 50;
        c
    }

    #[test]
    fn should_be_false_when_custom_format_score_is_above_minimum_but_bypass_disabled() {
        assert!(!scored(100, false).accepted(specs::delay));
    }

    #[test]
    fn should_be_false_when_custom_format_score_is_above_minimum_and_bypass_enabled_but_under_minimum()
     {
        assert!(!scored(5, true).accepted(specs::delay));
    }

    #[test]
    fn should_be_true_when_custom_format_score_is_above_minimum_and_bypass_enabled() {
        assert!(scored(100, true).accepted(specs::delay));
    }
}

/// `RssSync/HistorySpecificationFixture` (completed download handling enabled, as it always is
/// in Kitsunarr).
mod history_specification_fixture {
    use super::*;

    fn case(multi: bool) -> Case {
        let mut c = Case::new();
        c.criteria = SearchCriteria::rss();
        c.series.profile.upgrade_allowed = true;
        c.series.profile.cutoff = BLURAY1080P;
        c.parsed.quality = quality(DVD, 2, 0);
        c.episodes = if multi {
            vec![ep(1, 12, 3, 30), ep(2, 12, 4, 30), ep(3, 12, 5, 30)]
        } else {
            vec![ep(1, 12, 3, 30)]
        };
        c
    }

    fn grabbed(c: &mut Case, episode_id: i64, quality_id: i32, version: i32, hours_ago: i64) {
        c.settings.last_grabs.retain(|g| g.episode_id != episode_id);
        c.settings.last_grabs.push(HistoryGrab {
            episode_id,
            date: Utc::now() - Duration::hours(hours_ago),
            title: "Series.Title.S12E03.x264-GRP".into(),
            quality: QualityModel {
                quality_id,
                revision: Revision {
                    version,
                    ..Default::default()
                },
            },
            size: 0,
            languages: Vec::new(),
            indexer_flags: 0,
            release_type: ReleaseType::SingleEpisode,
        });
    }

    const UPGRADABLE: (i32, i32) = (SDTV, 1);
    const NOT_UPGRADABLE: (i32, i32) = (HDTV1080P, 2);

    #[test]
    fn should_return_true_if_it_is_a_search() {
        let mut c = case(true);
        grabbed(&mut c, 1, NOT_UPGRADABLE.0, NOT_UPGRADABLE.1, 0);
        c.criteria = SearchCriteria {
            monitored_episodes_only: false,
            kind: SearchKind::Season { season: 12 },
            episode_ids: vec![1, 2, 3],
        };
        assert!(c.accepted(specs::history));
    }

    #[test]
    fn should_return_true_if_latest_history_item_is_null_or_not_a_grab() {
        // Kitsunarr only receives episodes whose most recent event is a grab.
        assert!(case(true).accepted(specs::history));
    }

    #[test]
    fn should_return_true_if_latest_history_item_is_older_than_twelve_hours() {
        let mut c = case(true);
        grabbed(&mut c, 1, NOT_UPGRADABLE.0, NOT_UPGRADABLE.1, 13);
        assert!(c.accepted(specs::history));
    }

    #[test]
    fn should_be_upgradable_if_only_episode_is_upgradable() {
        let mut c = case(false);
        grabbed(&mut c, 1, UPGRADABLE.0, UPGRADABLE.1, 0);
        assert!(c.accepted(specs::history));
    }

    #[test]
    fn should_be_upgradable_if_both_episodes_are_upgradable() {
        let mut c = case(true);
        grabbed(&mut c, 1, UPGRADABLE.0, UPGRADABLE.1, 0);
        grabbed(&mut c, 2, UPGRADABLE.0, UPGRADABLE.1, 0);
        assert!(c.accepted(specs::history));
    }

    #[test]
    fn should_not_be_upgradable_if_both_episodes_are_not_upgradable() {
        let mut c = case(true);
        grabbed(&mut c, 1, NOT_UPGRADABLE.0, NOT_UPGRADABLE.1, 0);
        grabbed(&mut c, 2, NOT_UPGRADABLE.0, NOT_UPGRADABLE.1, 0);
        assert_eq!(
            c.check(specs::history).unwrap().message,
            "Recent grab event in history is of equal or higher preference: HDTV-1080p v2"
        );
    }

    #[test]
    fn should_be_not_upgradable_if_only_first_episodes_is_upgradable() {
        // Upstream's second setup replaces the first for the same episode.
        let mut c = case(true);
        grabbed(&mut c, 1, UPGRADABLE.0, UPGRADABLE.1, 0);
        grabbed(&mut c, 1, NOT_UPGRADABLE.0, NOT_UPGRADABLE.1, 0);
        assert!(!c.accepted(specs::history));
    }

    #[test]
    fn should_be_not_upgradable_if_only_second_episodes_is_upgradable() {
        let mut c = case(true);
        grabbed(&mut c, 1, NOT_UPGRADABLE.0, NOT_UPGRADABLE.1, 0);
        grabbed(&mut c, 2, UPGRADABLE.0, UPGRADABLE.1, 0);
        assert!(!c.accepted(specs::history));
    }

    #[test]
    fn should_not_be_upgradable_if_episode_is_of_same_quality_as_existing() {
        let mut c = case(false);
        c.series.profile.upgrade_allowed = false;
        c.parsed.quality = quality(WEBDL1080P, 1, 0);
        grabbed(&mut c, 1, WEBDL1080P, 1, 0);
        assert!(!c.accepted(specs::history));
    }

    #[test]
    fn should_not_be_upgradable_if_cutoff_already_met() {
        let mut c = case(false);
        c.series.profile.upgrade_allowed = false;
        c.series.profile.cutoff = WEBDL1080P;
        c.parsed.quality = quality(WEBDL1080P, 1, 0);
        grabbed(&mut c, 1, WEBDL1080P, 1, 0);
        assert_eq!(
            c.check(specs::history).unwrap().message,
            "Recent grab event in history already meets cutoff: WEBDL-1080p v1"
        );
    }

    #[test]
    fn should_return_false_if_latest_history_item_is_only_one_hour_old() {
        let mut c = case(true);
        grabbed(&mut c, 1, NOT_UPGRADABLE.0, NOT_UPGRADABLE.1, 1);
        assert!(!c.accepted(specs::history));
    }
}

/// `RssSync/ProperSpecificationFixture`.
mod proper_specification_fixture {
    use super::*;

    fn case(multi: bool) -> Case {
        let mut c = Case::new();
        c.criteria = SearchCriteria::rss();
        c.series.profile.cutoff = BLURAY1080P;
        c.parsed.quality = quality(DVD, 2, 0);
        let mut first = ep(1, 1, 1, 30);
        first.file = Some(file_of(BLURAY1080P, 1));
        let mut second = ep(2, 1, 2, 30);
        second.file = Some(file_of(BLURAY1080P, 1));
        c.episodes = if multi {
            vec![first, second, ep(3, 1, 3, 30)]
        } else {
            vec![first, ep(3, 1, 3, 30)]
        };
        c
    }

    fn file(c: &mut Case, index: usize) -> &mut EpisodeFile {
        c.episodes[index].file.as_mut().unwrap()
    }

    fn today_minus(days: u64) -> chrono::DateTime<Utc> {
        (Utc::now().date_naive() - chrono::Days::new(days))
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
    }

    #[test]
    fn should_return_false_when_episode_file_was_added_more_than_7_days_ago() {
        let mut c = case(false);
        file(&mut c, 0).quality.quality_id = DVD;
        file(&mut c, 0).date_added = Some(today_minus(30));
        assert_eq!(
            c.check(specs::proper).unwrap().message,
            "Proper for old file"
        );
    }

    #[test]
    fn should_return_false_when_first_episode_file_was_added_more_than_7_days_ago() {
        let mut c = case(true);
        file(&mut c, 0).quality.quality_id = DVD;
        file(&mut c, 1).quality.quality_id = DVD;
        file(&mut c, 0).date_added = Some(today_minus(30));
        assert!(!c.accepted(specs::proper));
    }

    #[test]
    fn should_return_false_when_second_episode_file_was_added_more_than_7_days_ago() {
        let mut c = case(true);
        file(&mut c, 0).quality.quality_id = DVD;
        file(&mut c, 1).quality.quality_id = DVD;
        file(&mut c, 1).date_added = Some(today_minus(30));
        assert!(!c.accepted(specs::proper));
    }

    #[test]
    fn should_return_true_when_episode_file_was_added_more_than_7_days_ago_but_proper_is_for_better_quality()
     {
        let mut c = case(false);
        file(&mut c, 0).quality = QualityModel {
            quality_id: SDTV,
            revision: Revision::default(),
        };
        file(&mut c, 0).date_added = Some(today_minus(30));
        assert!(c.accepted(specs::proper));
    }

    #[test]
    fn should_return_true_when_episode_file_was_added_more_than_7_days_ago_but_is_for_search() {
        let mut c = case(false);
        file(&mut c, 0).quality.quality_id = DVD;
        file(&mut c, 0).date_added = Some(today_minus(30));
        c.criteria = Case::new().criteria;
        assert!(c.accepted(specs::proper));
    }

    #[test]
    fn should_return_false_when_proper_but_auto_download_propers_is_false() {
        let mut c = case(false);
        c.settings.propers = ProperDownloadType::DoNotUpgrade;
        file(&mut c, 0).quality.quality_id = DVD;
        file(&mut c, 0).date_added = Some(today_minus(0));
        assert_eq!(
            c.check(specs::proper).unwrap().message,
            "Proper downloading is disabled"
        );
    }

    #[test]
    fn should_return_true_when_episode_file_was_added_today() {
        let mut c = case(false);
        c.settings.propers = ProperDownloadType::PreferAndUpgrade;
        file(&mut c, 0).quality.quality_id = DVD;
        file(&mut c, 0).date_added = Some(today_minus(0));
        assert!(c.accepted(specs::proper));
    }

    #[test]
    fn should_return_true_when_propers_are_not_preferred() {
        let mut c = case(false);
        c.settings.propers = ProperDownloadType::DoNotPrefer;
        file(&mut c, 0).quality.quality_id = DVD;
        file(&mut c, 0).date_added = Some(today_minus(0));
        assert!(c.accepted(specs::proper));
    }
}

/// RSS decisions skip the search-only rules: nothing was requested.
#[test]
fn rss_skips_episode_requested() {
    let mut c = Case::new();
    c.criteria = SearchCriteria::rss();
    c.episodes = vec![ep(9, 1, 1, 30)];
    assert!(c.accepted(specs::episode_requested));
    c.criteria = Case::new().criteria;
    assert!(!c.accepted(specs::episode_requested));
}

/// Tanukarr's `CompleteSeasonUpgradeSpecificationFixture`.
#[test]
fn complete_season_upgrade_specification_fixture() {
    let mut c = Case::new();
    c.criteria = SearchCriteria::rss();
    c.episodes[0].file = Some(file_of(HDTV720P, 1));
    c.series.episodes = c.episodes.clone();
    // RSS, every monitored aired episode of the season has a file: rejected.
    assert_eq!(
        c.check(specs::complete_season_upgrade).unwrap().message,
        "Automatic feed upgrades are disabled for fully downloaded seasons"
    );
    // Another episode of the season is monitored, aired and missing: accepted.
    let mut missing = ep(6, 1, 2, 30);
    missing.air_date_utc = Some(Utc::now() - Duration::days(1));
    c.series.episodes.push(missing.clone());
    assert!(c.accepted(specs::complete_season_upgrade));
    // Not aired yet: it does not count.
    c.series.episodes[1].air_date_utc = Some(Utc::now() + Duration::days(1));
    assert!(!c.accepted(specs::complete_season_upgrade));
    // An episode of the release itself missing: accepted.
    c.episodes[0].file = None;
    assert!(c.accepted(specs::complete_season_upgrade));
    // Searches keep their upgrade rules.
    c.episodes[0].file = Some(file_of(HDTV720P, 1));
    c.criteria = Case::new().criteria;
    assert!(c.accepted(specs::complete_season_upgrade));
}
