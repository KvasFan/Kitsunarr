//! `UpgradeSpecificationFixture` (Sonarr's tests of `UpgradableSpecification.IsUpgradable`).

mod common;

use common::*;
use kitsunarr_decision::ProfileView;
use kitsunarr_decision::profile::{ProperDownloadType, UpgradeRejectReason};
use kitsunarr_domain::quality::{QualityModel, Revision};

fn q(quality_id: i32, version: i32) -> QualityModel {
    QualityModel {
        quality_id,
        revision: Revision {
            version,
            ..Default::default()
        },
    }
}

#[test]
fn is_upgrade_test() {
    use UpgradeRejectReason::*;
    let cases = [
        (SDTV, 1, SDTV, 2, Ok(())),
        (WEBDL720P, 1, WEBDL720P, 2, Ok(())),
        (SDTV, 1, SDTV, 1, Err(CustomFormatScore)),
        (WEBDL720P, 1, HDTV720P, 2, Err(BetterQuality)),
        (WEBDL720P, 1, HDTV720P, 2, Err(BetterQuality)),
        (WEBDL720P, 1, WEBDL720P, 1, Err(CustomFormatScore)),
        (WEBDL1080P, 1, WEBDL1080P, 1, Err(CustomFormatScore)),
    ];
    for (current, cv, new, nv, expected) in cases {
        let mut p = profile(default_items(&[]));
        p.upgrade_allowed = true;
        let got = ProfileView(&p).is_upgradable(
            &q(current, cv),
            0,
            &q(new, nv),
            0,
            ProperDownloadType::PreferAndUpgrade,
        );
        assert_eq!(got, expected, "{current} v{cv} -> {new} v{nv}");
    }
}

#[test]
fn propers_settings() {
    let p = profile(default_items(&[]));
    // should_return_true_if_proper_and_download_propers_is_do_not_download
    assert_eq!(
        ProfileView(&p).is_upgradable(
            &q(DVD, 1),
            0,
            &q(DVD, 2),
            0,
            ProperDownloadType::DoNotUpgrade
        ),
        Ok(())
    );
    // should_return_false_if_proper_and_autoDownloadPropers_is_do_not_prefer
    assert_eq!(
        ProfileView(&p).is_upgradable(
            &q(DVD, 1),
            0,
            &q(DVD, 2),
            0,
            ProperDownloadType::DoNotPrefer
        ),
        Err(UpgradeRejectReason::UpgradesNotAllowed)
    );
    // should_return_false_if_release_and_existing_file_are_the_same (default propers setting)
    assert_eq!(
        ProfileView(&p).is_upgradable(
            &q(HDTV720P, 1),
            0,
            &q(HDTV720P, 1),
            0,
            ProperDownloadType::default()
        ),
        Err(UpgradeRejectReason::UpgradesNotAllowed)
    );
}

#[test]
fn cutoff() {
    let mut p = profile(default_items(&[]));
    p.upgrade_allowed = true;
    p.cutoff = HDTV1080P;
    let propers = ProperDownloadType::default();
    // should_return_true_if_release_has_higher_quality_and_cutoff_is_not_already_met
    assert_eq!(
        ProfileView(&p).is_upgradable(&q(HDTV720P, 1), 0, &q(HDTV1080P, 1), 0, propers),
        Ok(())
    );
    // should_return_false_if_release_has_higher_quality_and_cutoff_is_already_met
    p.cutoff = HDTV720P;
    assert_eq!(
        ProfileView(&p).is_upgradable(&q(HDTV720P, 1), 0, &q(HDTV1080P, 1), 0, propers),
        Err(UpgradeRejectReason::QualityCutoff)
    );
}

#[test]
fn minimum_custom_format_score_increment() {
    let mut p = profile(default_items(&[]));
    p.upgrade_allowed = true;
    p.cutoff_format_score = 100;
    p.format_items = scores(&[(1, 10), (2, 20)]);
    let view = ProfileView(&p);
    let (one, two) = (view.format_score(&[1]), view.format_score(&[2]));
    // should_return_false_if_minimum_custom_score_is_not_met
    p.min_upgrade_format_score = 11;
    assert_eq!(
        ProfileView(&p).is_upgradable(
            &q(DVD, 1),
            one,
            &q(DVD, 1),
            two,
            ProperDownloadType::default()
        ),
        Err(UpgradeRejectReason::MinCustomFormatScore)
    );
    // should_return_true_if_minimum_custom_score_is_met
    p.min_upgrade_format_score = 10;
    assert_eq!(
        ProfileView(&p).is_upgradable(
            &q(DVD, 1),
            one,
            &q(DVD, 1),
            two,
            ProperDownloadType::default()
        ),
        Ok(())
    );
}
