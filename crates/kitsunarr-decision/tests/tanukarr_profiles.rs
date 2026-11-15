//! Oracle captured by executing Tanukarr's original C# profile/revision/upgrade classes.
//! See tests/fixtures/generate/quality-profiles, including infrastructure-only stubs.
use kitsunarr_decision::{ProfileView, ProperDownloadType};
use kitsunarr_domain::api::settings::{FormatScore, QualityProfile};
use kitsunarr_domain::quality::{QualityModel, Revision};
use serde_json::json;

#[test]
fn quality_profiles_match_tanukarr_reference_matrix() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/quality-profiles/tanukarr-reference.json"
    ))
    .unwrap();
    let ids: Vec<i32> = serde_json::from_value(fixture["qualityIds"].clone()).unwrap();
    let revisions: Vec<[i32; 4]> = serde_json::from_value(fixture["revisions"].clone()).unwrap();
    let scores: Vec<[i32; 2]> = serde_json::from_value(fixture["scores"].clone()).unwrap();
    let truth = |v| if v { "True" } else { "False" };
    let mut checked = 0;
    for reference in fixture["profiles"].as_array().unwrap() {
        let mut profile: QualityProfile = serde_json::from_value(json!({
            "id": 1, "name": "Reference", "upgradeAllowed": reference["upgrades"],
            "cutoff": reference["cutoff"], "minFormatScore": 0,
            "cutoffFormatScore": 100, "minUpgradeFormatScore": 50, "formatItems": [],
            "items": [
                {"qualityId": 1, "allowed": false},
                {"group": {"id": 1000, "name": "HD group", "qualityIds": [9,3]}, "allowed": true},
                {"qualityId": 7, "allowed": true}
            ]
        }))
        .unwrap();
        let mut row = 0;
        for &current_id in &ids {
            for &new_id in &ids {
                for &[cv, cr, nv, nr] in &revisions {
                    for &[current_score, new_score] in &scores {
                        for propers in [
                            ProperDownloadType::PreferAndUpgrade,
                            ProperDownloadType::DoNotUpgrade,
                            ProperDownloadType::DoNotPrefer,
                        ] {
                            profile.format_items = vec![
                                FormatScore {
                                    format_id: 1,
                                    score: current_score,
                                },
                                FormatScore {
                                    format_id: 2,
                                    score: new_score,
                                },
                            ];
                            let p = ProfileView(&profile);
                            assert_eq!(p.format_score([&1]), current_score);
                            assert_eq!(p.format_score([&2]), new_score);
                            let current = QualityModel {
                                quality_id: current_id,
                                revision: Revision {
                                    version: cv,
                                    real: cr,
                                    is_repack: false,
                                },
                            };
                            let new = QualityModel {
                                quality_id: new_id,
                                revision: Revision {
                                    version: nv,
                                    real: nr,
                                    is_repack: false,
                                },
                            };
                            let reason = p
                                .is_upgradable(&current, current_score, &new, new_score, propers)
                                .err()
                                .map_or("None".into(), |e| format!("{e:?}"));
                            let got = format!(
                                "{}|{}|{}|{}|{}|{}",
                                p.compare_models(&new, &current) as i8,
                                truth(p.quality_cutoff_not_met(&current, Some(&new))),
                                truth(p.cutoff_not_met(&current, current_score, Some(&new))),
                                truth(kitsunarr_decision::profile::is_revision_upgrade(
                                    &current, &new
                                )),
                                truth(p.is_upgrade_allowed(
                                    &current,
                                    current_score,
                                    &new,
                                    new_score
                                )),
                                reason
                            );
                            assert_eq!(
                                got,
                                reference["results"][row].as_str().unwrap(),
                                "upgrades={} cutoff={} {current:?}/{current_score} -> {new:?}/{new_score}, {propers:?}",
                                profile.upgrade_allowed,
                                profile.cutoff
                            );
                            row += 1;
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(row, reference["results"].as_array().unwrap().len());
    }
    assert_eq!(checked, 5376);
}
