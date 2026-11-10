//! Quality profile operations: `QualityProfile.GetIndex`, `QualityIndex`, `QualityModelComparer`,
//! `CalculateCustomFormatScore` and the cutoff and upgrade rules of `UpgradableSpecification`.

use std::cmp::Ordering;

use kitsunarr_domain::api::settings::{QualityProfile, QualityProfileItem};
use kitsunarr_domain::quality::{Quality, QualityModel, Revision};

/// Position of a quality in a profile (`QualityIndex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QualityIndex {
    pub index: usize,
    pub group_index: usize,
}

impl QualityIndex {
    /// `CompareTo(right, respectGroupOrder)`.
    pub fn compare(&self, other: &Self, respect_group_order: bool) -> Ordering {
        let c = self.index.cmp(&other.index);
        if respect_group_order && c == Ordering::Equal {
            self.group_index.cmp(&other.group_index)
        } else {
            c
        }
    }
}

/// `Revision.CompareTo`: `Real` first, then `Version` (`IsRepack` does not count).
pub fn compare_revision(a: &Revision, b: &Revision) -> Ordering {
    a.real.cmp(&b.real).then(a.version.cmp(&b.version))
}

/// `Revision.ToString`.
pub fn revision_label(r: &Revision) -> String {
    if r.real > 0 {
        format!("v{} Real:{}", r.version, r.real)
    } else {
        format!("v{}", r.version)
    }
}

/// `QualityModel.ToString`: `"HDTV-720p v1"`.
pub fn quality_model_label(q: &QualityModel) -> String {
    format!("{} {}", q.quality().name, revision_label(&q.revision))
}

/// A quality profile with Sonarr's lookups.
#[derive(Debug, Clone, Copy)]
pub struct ProfileView<'a>(pub &'a QualityProfile);

fn item_qualities(item: &QualityProfileItem) -> Vec<i32> {
    match (&item.quality_id, &item.group) {
        (Some(q), _) => vec![*q],
        (None, Some(g)) => g.quality_ids.clone(),
        (None, None) => Vec::new(),
    }
}

impl<'a> ProfileView<'a> {
    pub fn items(&self) -> &'a [QualityProfileItem] {
        &self.0.items
    }

    /// `GetIndex(id, respectGroupOrder)`: a quality or group id; unknown ids give index 0.
    pub fn index_of(&self, id: i32, respect_group_order: bool) -> QualityIndex {
        for (i, item) in self.0.items.iter().enumerate() {
            if item.quality_id == Some(id) {
                return QualityIndex {
                    index: i,
                    group_index: 0,
                };
            }
            if let Some(g) = &item.group {
                if g.id > 0 && g.id == id {
                    return QualityIndex {
                        index: i,
                        group_index: 0,
                    };
                }
                if let Some(gi) = g.quality_ids.iter().position(|q| *q == id) {
                    return QualityIndex {
                        index: i,
                        group_index: if respect_group_order { gi } else { 0 },
                    };
                }
            }
        }
        QualityIndex::default()
    }

    /// Whether the item holding a quality is allowed (`Items[GetIndex(q).Index].Allowed`).
    pub fn allowed(&self, quality_id: i32) -> bool {
        self.0
            .items
            .get(self.index_of(quality_id, false).index)
            .is_some_and(|i| i.allowed)
    }

    /// `QualityProfileQualityItem.ToString()` of the item at `index`.
    pub fn item_label(&self, index: usize) -> String {
        let Some(item) = self.0.items.get(index) else {
            return String::new();
        };
        let names: Vec<&str> = item_qualities(item)
            .iter()
            .map(|q| Quality::by_id(*q).unwrap_or_else(Quality::unknown).name)
            .collect();
        match &item.group {
            Some(g) if !g.name.trim().is_empty() => format!("{} ({})", g.name, names.join(", ")),
            _ => names.join(", "),
        }
    }

    /// `FirststAllowedQuality().Id`.
    pub fn first_allowed_quality(&self) -> Option<i32> {
        self.0
            .items
            .iter()
            .find(|i| i.allowed)
            .and_then(|i| item_qualities(i).first().copied())
    }

    /// `QualityModelComparer.Compare(Quality, Quality)` (group order not respected).
    pub fn compare_qualities(&self, a: i32, b: i32) -> Ordering {
        self.index_of(a, false)
            .compare(&self.index_of(b, false), false)
    }

    /// `QualityModelComparer.Compare(QualityModel, QualityModel)`.
    pub fn compare_models(&self, a: &QualityModel, b: &QualityModel) -> Ordering {
        self.compare_qualities(a.quality_id, b.quality_id)
            .then_with(|| compare_revision(&a.revision, &b.revision))
    }

    /// `CalculateCustomFormatScore`.
    pub fn format_score<'f>(&self, format_ids: impl IntoIterator<Item = &'f i64>) -> i32 {
        let ids: Vec<i64> = format_ids.into_iter().copied().collect();
        self.0
            .format_items
            .iter()
            .filter(|f| ids.contains(&f.format_id))
            .map(|f| f.score)
            .sum()
    }

    /// The cutoff in effect: the configured one when upgrades are allowed, else the first
    /// allowed quality.
    pub fn effective_cutoff(&self) -> i32 {
        if self.0.upgrade_allowed {
            self.0.cutoff
        } else {
            self.first_allowed_quality().unwrap_or(self.0.cutoff)
        }
    }
}

/// `IsRevisionUpgrade`: same quality, newer revision.
pub fn is_revision_upgrade(current: &QualityModel, new: &QualityModel) -> bool {
    current.quality_id == new.quality_id
        && compare_revision(&new.revision, &current.revision) == Ordering::Greater
}

/// How propers and repacks are handled (`ProperDownloadTypes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProperDownloadType {
    #[default]
    PreferAndUpgrade,
    DoNotUpgrade,
    DoNotPrefer,
}

/// Why a release is not an upgrade (`UpgradeableRejectReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeRejectReason {
    BetterQuality,
    BetterRevision,
    QualityCutoff,
    CustomFormatScore,
    CustomFormatCutoff,
    MinCustomFormatScore,
    UpgradesNotAllowed,
}

impl<'a> ProfileView<'a> {
    /// `QualityCutoffNotMet`.
    pub fn quality_cutoff_not_met(
        &self,
        current: &QualityModel,
        new: Option<&QualityModel>,
    ) -> bool {
        let cutoff = self.effective_cutoff();
        if self
            .index_of(current.quality_id, false)
            .compare(&self.index_of(cutoff, false), false)
            == Ordering::Less
        {
            return true;
        }
        new.is_some_and(|n| is_revision_upgrade(current, n))
    }

    /// `CustomFormatCutoffNotMet`.
    pub fn custom_format_cutoff_not_met(&self, current_score: i32) -> bool {
        let cutoff = if self.0.upgrade_allowed {
            self.0.cutoff_format_score
        } else {
            self.0.min_format_score
        };
        current_score < cutoff
    }

    /// `CutoffNotMet`.
    pub fn cutoff_not_met(
        &self,
        current: &QualityModel,
        current_score: i32,
        new: Option<&QualityModel>,
    ) -> bool {
        self.quality_cutoff_not_met(current, new)
            || self.custom_format_cutoff_not_met(current_score)
    }

    /// `IsUpgradable`.
    pub fn is_upgradable(
        &self,
        current: &QualityModel,
        current_score: i32,
        new: &QualityModel,
        new_score: i32,
        propers: ProperDownloadType,
    ) -> Result<(), UpgradeRejectReason> {
        let quality_compare = self.compare_qualities(new.quality_id, current.quality_id);
        if quality_compare == Ordering::Greater && self.quality_cutoff_not_met(current, Some(new)) {
            return Ok(());
        }
        if quality_compare == Ordering::Less {
            return Err(UpgradeRejectReason::BetterQuality);
        }
        let revision_compare = compare_revision(&new.revision, &current.revision);
        if propers != ProperDownloadType::DoNotPrefer && revision_compare == Ordering::Greater {
            return Ok(());
        }
        if !self.0.upgrade_allowed {
            return Err(UpgradeRejectReason::UpgradesNotAllowed);
        }
        if propers != ProperDownloadType::DoNotPrefer && revision_compare == Ordering::Less {
            return Err(UpgradeRejectReason::BetterRevision);
        }
        if quality_compare == Ordering::Greater {
            return Err(UpgradeRejectReason::QualityCutoff);
        }
        if new_score <= current_score {
            return Err(UpgradeRejectReason::CustomFormatScore);
        }
        if current_score >= self.0.cutoff_format_score {
            return Err(UpgradeRejectReason::CustomFormatCutoff);
        }
        if new_score < current_score + self.0.min_upgrade_format_score {
            return Err(UpgradeRejectReason::MinCustomFormatScore);
        }
        Ok(())
    }

    /// `IsUpgradeAllowed`.
    pub fn is_upgrade_allowed(
        &self,
        current: &QualityModel,
        current_score: i32,
        new: &QualityModel,
        new_score: i32,
    ) -> bool {
        let quality_upgrade = self.compare_models(new, current) == Ordering::Greater;
        let format_upgrade = new_score > current_score;
        if is_revision_upgrade(current, new) {
            return true;
        }
        if quality_upgrade || format_upgrade {
            return self.0.upgrade_allowed;
        }
        true
    }
}
