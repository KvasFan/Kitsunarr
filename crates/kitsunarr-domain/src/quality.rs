//! Quality catalogue.
//!
//! Identifiers, names, weights and default sizes match Sonarr v4 exactly (captured from a
//! running Sonarr 4.0.20 `/api/v3/qualitydefinition`). Keeping the identifiers stable is what
//! lets quality profiles, history and file records migrate without translation tables.

use serde::{Deserialize, Serialize};

/// Where a release was sourced from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum QualitySource {
    Unknown,
    Television,
    TelevisionRaw,
    Web,
    WebRip,
    Dvd,
    Bluray,
    BlurayRaw,
}

impl QualitySource {
    /// Sonarr's numeric `QualitySource` values (used by the `SourceSpecification` custom format).
    pub fn sonarr_id(self) -> i32 {
        match self {
            Self::Unknown => 0,
            Self::Television => 1,
            Self::TelevisionRaw => 2,
            Self::Web => 3,
            Self::WebRip => 4,
            Self::Dvd => 5,
            Self::Bluray => 6,
            Self::BlurayRaw => 7,
        }
    }

    pub fn from_sonarr_id(id: i64) -> Option<Self> {
        Some(match id {
            0 => Self::Unknown,
            1 => Self::Television,
            2 => Self::TelevisionRaw,
            3 => Self::Web,
            4 => Self::WebRip,
            5 => Self::Dvd,
            6 => Self::Bluray,
            7 => Self::BlurayRaw,
            _ => return None,
        })
    }
}

/// A quality from the fixed catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Quality {
    pub id: i32,
    pub name: &'static str,
    pub source: QualitySource,
    pub resolution: i32,
    /// Ordering weight; higher is better. Qualities with equal weight rank equally.
    pub weight: i32,
    /// Default size limits in megabytes per minute of runtime (`None` = unlimited).
    pub default_min_size: f64,
    pub default_preferred_size: f64,
    pub default_max_size: Option<f64>,
}

macro_rules! q {
    ($id:expr, $name:expr, $src:ident, $res:expr, $w:expr, $min:expr, $max:expr) => {
        Quality {
            id: $id,
            name: $name,
            source: QualitySource::$src,
            resolution: $res,
            weight: $w,
            default_min_size: $min,
            default_preferred_size: 95.0,
            default_max_size: $max,
        }
    };
}

/// All qualities, ordered from worst to best (the default order in a new profile).
pub const QUALITIES: &[Quality] = &[
    q!(0, "Unknown", Unknown, 0, 1, 1.0, Some(199.9)),
    q!(1, "SDTV", Television, 480, 2, 2.0, Some(100.0)),
    q!(12, "WEBRip-480p", WebRip, 480, 3, 2.0, Some(100.0)),
    q!(8, "WEBDL-480p", Web, 480, 3, 2.0, Some(100.0)),
    q!(2, "DVD", Dvd, 480, 4, 2.0, Some(100.0)),
    q!(13, "Bluray-480p", Bluray, 480, 5, 2.0, Some(100.0)),
    q!(22, "Bluray-576p", Bluray, 576, 6, 2.0, Some(100.0)),
    q!(4, "HDTV-720p", Television, 720, 7, 3.0, Some(125.0)),
    q!(9, "HDTV-1080p", Television, 1080, 8, 4.0, Some(125.0)),
    q!(10, "Raw-HD", TelevisionRaw, 1080, 9, 4.0, None),
    q!(14, "WEBRip-720p", WebRip, 720, 10, 3.0, Some(130.0)),
    q!(5, "WEBDL-720p", Web, 720, 10, 3.0, Some(130.0)),
    q!(6, "Bluray-720p", Bluray, 720, 11, 4.0, Some(130.0)),
    q!(15, "WEBRip-1080p", WebRip, 1080, 12, 4.0, Some(130.0)),
    q!(3, "WEBDL-1080p", Web, 1080, 12, 5.0, Some(120.0)),
    q!(7, "Bluray-1080p", Bluray, 1080, 13, 4.0, Some(155.0)),
    q!(20, "Bluray-1080p Remux", BlurayRaw, 1080, 14, 35.0, None),
    q!(16, "HDTV-2160p", Television, 2160, 15, 35.0, Some(199.9)),
    q!(17, "WEBRip-2160p", WebRip, 2160, 16, 35.0, None),
    q!(18, "WEBDL-2160p", Web, 2160, 16, 35.0, None),
    q!(19, "Bluray-2160p", Bluray, 2160, 17, 35.0, None),
    q!(21, "Bluray-2160p Remux", BlurayRaw, 2160, 18, 35.0, None),
];

/// The first id of Tanukarr's custom quality presets.
pub const CUSTOM_QUALITY_BASE: i32 = 10_000;
/// Tanukarr's weight for custom presets (ranking follows the profile's order).
const CUSTOM_WEIGHT: i32 = 19;

/// Custom quality presets registered at run time (loaded from the database).
static CUSTOM: std::sync::RwLock<Vec<&'static Quality>> = std::sync::RwLock::new(Vec::new());

impl Quality {
    pub fn by_id(id: i32) -> Option<&'static Quality> {
        QUALITIES.iter().find(|q| q.id == id).or_else(|| {
            (id >= CUSTOM_QUALITY_BASE)
                .then(|| {
                    CUSTOM
                        .read()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .find(|q| q.id == id)
                        .copied()
                })
                .flatten()
        })
    }

    pub fn by_name(name: &str) -> Option<&'static Quality> {
        QUALITIES
            .iter()
            .find(|q| q.name.eq_ignore_ascii_case(name))
            .or_else(|| {
                CUSTOM
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .find(|q| q.name.eq_ignore_ascii_case(name))
                    .copied()
            })
    }

    /// Whether this is one of Tanukarr's custom presets (id 10000 and up).
    pub fn is_custom(&self) -> bool {
        self.id >= CUSTOM_QUALITY_BASE
    }

    /// Registers (or renames) a custom preset (`Quality.Register`). Each registration keeps a
    /// small allocation for the life of the process, as presets are few and rarely edited.
    pub fn register(
        id: i32,
        name: &str,
        source: QualitySource,
        resolution: i32,
    ) -> &'static Quality {
        let mut list = CUSTOM.write().unwrap_or_else(|e| e.into_inner());
        if let Some(q) = list.iter().find(|q| {
            q.id == id && q.name == name && q.source == source && q.resolution == resolution
        }) {
            return q;
        }
        let base = QUALITIES
            .iter()
            .find(|q| q.source == source && q.resolution == resolution);
        let q: &'static Quality = Box::leak(Box::new(Quality {
            id,
            name: Box::leak(name.to_string().into_boxed_str()),
            source,
            resolution,
            weight: CUSTOM_WEIGHT,
            default_min_size: base.map_or(2.0, |b| b.default_min_size),
            default_preferred_size: 95.0,
            default_max_size: base.and_then(|b| b.default_max_size),
        }));
        list.retain(|x| x.id != id);
        list.push(q);
        q
    }

    /// The registered custom presets, by id.
    pub fn custom() -> Vec<&'static Quality> {
        let mut out = CUSTOM.read().unwrap_or_else(|e| e.into_inner()).clone();
        out.sort_by_key(|q| q.id);
        out
    }

    pub fn unknown() -> &'static Quality {
        &QUALITIES[0]
    }

    pub fn all() -> &'static [Quality] {
        QUALITIES
    }
}

/// Revision information attached to a release or file (`v2`, PROPER, REPACK, REAL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub version: i32,
    pub real: i32,
    pub is_repack: bool,
}

impl Default for Revision {
    fn default() -> Self {
        Self {
            version: 1,
            real: 0,
            is_repack: false,
        }
    }
}

/// A quality together with its revision, as stored on files and history entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QualityModel {
    pub quality_id: i32,
    #[serde(default)]
    pub revision: Revision,
}

impl QualityModel {
    pub fn quality(&self) -> &'static Quality {
        Quality::by_id(self.quality_id).unwrap_or_else(Quality::unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalogue_ids_are_unique() {
        let ids: HashSet<_> = QUALITIES.iter().map(|q| q.id).collect();
        assert_eq!(ids.len(), QUALITIES.len());
    }

    #[test]
    fn catalogue_is_ordered_by_weight() {
        for pair in QUALITIES.windows(2) {
            assert!(
                pair[0].weight <= pair[1].weight,
                "{} > {}",
                pair[0].name,
                pair[1].name
            );
        }
    }

    #[test]
    fn lookups_work() {
        assert_eq!(Quality::by_id(7).unwrap().name, "Bluray-1080p");
        assert_eq!(Quality::by_name("webdl-1080p").unwrap().id, 3);
        assert!(Quality::by_id(11).is_none(), "id 11 is unused by Sonarr");
        assert_eq!(
            QualitySource::from_sonarr_id(7),
            Some(QualitySource::BlurayRaw)
        );
    }
}
