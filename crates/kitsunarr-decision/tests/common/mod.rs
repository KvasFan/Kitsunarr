//! Builders shared by the decision tests (Sonarr's test helpers, ported).

#![allow(dead_code)]

use kitsunarr_domain::api::settings::{
    CustomFormat, CustomFormatSpecification, FormatScore, QualityProfile, QualityProfileItem,
};

pub const UNKNOWN: i32 = 0;
pub const SDTV: i32 = 1;
pub const DVD: i32 = 2;
pub const WEBDL1080P: i32 = 3;
pub const HDTV720P: i32 = 4;
pub const WEBDL720P: i32 = 5;
pub const BLURAY720P: i32 = 6;
pub const BLURAY1080P: i32 = 7;
pub const WEBDL480P: i32 = 8;
pub const HDTV1080P: i32 = 9;
pub const RAWHD: i32 = 10;
pub const HDTV2160P: i32 = 16;
pub const WEBDL2160P: i32 = 18;
pub const BLURAY2160P: i32 = 19;

/// `QualityFixture.GetDefaultQualities(allowed)`: the listed qualities, the allowed ones moved
/// to the end in the given order.
pub fn default_items(allowed: &[i32]) -> Vec<QualityProfileItem> {
    let qualities = [
        UNKNOWN,
        SDTV,
        WEBDL480P,
        DVD,
        HDTV720P,
        HDTV1080P,
        HDTV2160P,
        RAWHD,
        WEBDL720P,
        WEBDL1080P,
        WEBDL2160P,
        BLURAY720P,
        BLURAY1080P,
        BLURAY2160P,
    ];
    let allowed: Vec<i32> = if allowed.is_empty() {
        qualities.to_vec()
    } else {
        allowed.to_vec()
    };
    qualities
        .iter()
        .filter(|q| !allowed.contains(q))
        .chain(allowed.iter())
        .map(|q| QualityProfileItem {
            quality_id: Some(*q),
            group: None,
            allowed: allowed.contains(q),
        })
        .collect()
}

/// `new QualityProfile { Items = … }` with C#'s defaults for everything else.
pub fn profile(items: Vec<QualityProfileItem>) -> QualityProfile {
    QualityProfile {
        id: 1,
        name: "Profile".into(),
        upgrade_allowed: false,
        cutoff: 0,
        items,
        min_format_score: 0,
        cutoff_format_score: 0,
        min_upgrade_format_score: 0,
        format_items: Vec::new(),
        size_limits: Vec::new(),
        codec_multipliers: Default::default(),
    }
}

pub fn scores(pairs: &[(i64, i32)]) -> Vec<FormatScore> {
    pairs
        .iter()
        .map(|(format_id, score)| FormatScore {
            format_id: *format_id,
            score: *score,
        })
        .collect()
}

pub fn spec(implementation: &str, fields: serde_json::Value) -> CustomFormatSpecification {
    CustomFormatSpecification {
        name: implementation.into(),
        implementation: implementation.into(),
        negate: false,
        required: false,
        fields: fields.as_object().cloned().unwrap_or_default(),
    }
}

pub fn format(id: i64, name: &str, specifications: Vec<CustomFormatSpecification>) -> CustomFormat {
    CustomFormat {
        id,
        name: name.into(),
        include_when_renaming: false,
        specifications,
    }
}
