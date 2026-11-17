//! Tanukarr's custom quality presets (`CustomQualityCatalogueFixture`).

use kitsunarr_db::{Database, quality};
use kitsunarr_domain::api::settings::CustomQualityCreate;
use kitsunarr_domain::quality::{Quality, QualitySource};

#[tokio::test]
async fn presets_are_created_added_to_profiles_retired_and_restored() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let create = |title: &str| CustomQualityCreate {
        title: title.into(),
        source: Some(QualitySource::Web),
        resolution: Some(1080),
        codec: "HEVC".into(),
        title_pattern: String::new(),
        copy_from_id: Some(3),
    };
    let d = quality::create_custom(&db, &create("WEB 1080p HEVC"))
        .await
        .unwrap();
    assert!(d.quality_id >= 10_000);
    assert_eq!(Quality::by_id(d.quality_id).unwrap().name, "WEB 1080p HEVC");
    // Titles are unique, criteria are checked.
    assert!(
        quality::create_custom(&db, &create("web 1080p hevc"))
            .await
            .is_err()
    );
    let mut bad = create("Bad");
    bad.codec = "Betamax".into();
    assert!(quality::create_custom(&db, &bad).await.is_err());
    let mut bad = create("Bad");
    bad.resolution = Some(1440);
    assert!(quality::create_custom(&db, &bad).await.is_err());
    let mut bad = create("Bad");
    bad.title_pattern = "(".into();
    assert!(quality::create_custom(&db, &bad).await.is_err());
    // Every profile has it, not allowed.
    let mut profiles = quality::list_profiles(db.pool()).await.unwrap();
    assert!(profiles.iter().all(|p| {
        p.items
            .iter()
            .any(|i| i.quality_id == Some(d.quality_id) && !i.allowed)
    }));
    // Allowed in a profile: retiring is refused.
    let p = &mut profiles[0];
    p.items
        .iter_mut()
        .find(|i| i.quality_id == Some(d.quality_id))
        .unwrap()
        .allowed = true;
    quality::update_profile(db.pool(), p).await.unwrap();
    let err = quality::retire(&db, d.quality_id).await.unwrap_err();
    assert!(err.to_string().contains(&p.name), "{err}");
    assert!(quality::retire(&db, 0).await.is_err());
    p.items
        .iter_mut()
        .find(|i| i.quality_id == Some(d.quality_id))
        .unwrap()
        .allowed = false;
    quality::update_profile(db.pool(), p).await.unwrap();
    quality::retire(&db, d.quality_id).await.unwrap();
    // A retired quality cannot be allowed; omitted presets are kept in a profile.
    let mut again = quality::get_profile(db.pool(), p.id).await.unwrap();
    again
        .items
        .iter_mut()
        .find(|i| i.quality_id == Some(d.quality_id))
        .unwrap()
        .allowed = true;
    assert!(quality::prepare_profile(&db, &mut again).await.is_err());
    let mut omitted = quality::get_profile(db.pool(), p.id).await.unwrap();
    omitted.items.retain(|i| i.quality_id != Some(d.quality_id));
    quality::prepare_profile(&db, &mut omitted).await.unwrap();
    assert!(
        omitted
            .items
            .iter()
            .any(|i| i.quality_id == Some(d.quality_id))
    );
    quality::set_retired(&db, d.quality_id, false)
        .await
        .unwrap();
    let defs = quality::list_definitions(db.pool()).await.unwrap();
    let stored = defs.iter().find(|x| x.quality_id == d.quality_id).unwrap();
    assert!(!stored.is_retired);
    assert_eq!(
        (stored.source, stored.resolution, stored.codec.as_str()),
        (Some(QualitySource::Web), Some(1080), "HEVC")
    );

    // A fresh process registers the stored presets at open.
    drop(db);
    let reopened = Database::open(&dir.path().join("k.db")).await.unwrap();
    assert!(Quality::by_id(d.quality_id).is_some());
    drop(reopened);
}

/// Tanukarr's `QualityDefinitionUpdateService`: titles and flagged sizes, sizes written into
/// every profile's own limit too; Sonarr's reset restores titles and sizes.
#[tokio::test]
async fn definition_sizes_apply_to_profiles_and_reset() {
    use kitsunarr_domain::api::settings::{OrderedF64, QualityDefinitionUpdate, QualitySizeLimit};
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let mut profile = quality::list_profiles(db.pool()).await.unwrap()[0].clone();
    profile.size_limits = vec![QualitySizeLimit {
        quality_id: 7,
        min_size: Some(OrderedF64(4.0)),
        preferred_size: Some(OrderedF64(50.0)),
        max_size: Some(OrderedF64(200.0)),
    }];
    quality::update_profile(db.pool(), &profile).await.unwrap();
    let before = quality::list_definitions(db.pool()).await.unwrap();
    let def = |list: &[kitsunarr_domain::api::settings::QualityDefinition]| {
        list.iter().find(|d| d.quality_id == 7).unwrap().clone()
    };

    quality::update_definitions(
        &db,
        &[QualityDefinitionUpdate {
            quality_id: 7,
            title: Some(" Blu-ray 1080p ".into()),
            preferred_size: Some(OrderedF64(60.0)),
            update_preferred_size: true,
            ..Default::default()
        }],
    )
    .await
    .unwrap();
    let after = def(&quality::list_definitions(db.pool()).await.unwrap());
    assert_eq!(after.title, "Blu-ray 1080p");
    assert_eq!(after.preferred_size, Some(OrderedF64(60.0)));
    assert_eq!(
        after.min_size,
        def(&before).min_size,
        "unflagged sizes stay"
    );
    let limit = quality::get_profile(db.pool(), profile.id)
        .await
        .unwrap()
        .size_limits[0]
        .clone();
    assert_eq!(
        (limit.min_size, limit.preferred_size, limit.max_size),
        (
            Some(OrderedF64(4.0)),
            Some(OrderedF64(60.0)),
            Some(OrderedF64(200.0))
        )
    );

    // Refused batches change nothing.
    for bad in [
        vec![QualityDefinitionUpdate {
            quality_id: 7,
            title: Some("  ".into()),
            ..Default::default()
        }],
        vec![QualityDefinitionUpdate {
            quality_id: 3,
            title: Some("blu-ray 1080P".into()),
            ..Default::default()
        }],
        vec![
            QualityDefinitionUpdate {
                quality_id: 7,
                ..Default::default()
            },
            QualityDefinitionUpdate {
                quality_id: 7,
                ..Default::default()
            },
        ],
        vec![QualityDefinitionUpdate {
            quality_id: 999,
            ..Default::default()
        }],
        // The profile's own minimum would exceed its maximum.
        vec![QualityDefinitionUpdate {
            quality_id: 7,
            title: Some("Changed".into()),
            max_size: Some(OrderedF64(2.0)),
            update_max_size: true,
            ..Default::default()
        }],
    ] {
        assert!(
            quality::update_definitions(&db, &bad).await.is_err(),
            "{bad:?}"
        );
    }
    assert_eq!(
        def(&quality::list_definitions(db.pool()).await.unwrap()).title,
        "Blu-ray 1080p"
    );

    quality::reset_definitions(&db).await.unwrap();
    let reset = def(&quality::list_definitions(db.pool()).await.unwrap());
    assert_eq!(reset.title, def(&before).title);
    assert_eq!(reset.preferred_size, def(&before).preferred_size);
    assert_eq!(
        quality::get_profile(db.pool(), profile.id)
            .await
            .unwrap()
            .size_limits[0]
            .preferred_size,
        Some(OrderedF64(60.0)),
        "profiles keep their own limits"
    );
}
