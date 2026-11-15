//! Profile editing follows Tanukarr's EditQualityProfileModalContent/useQualityProfileDnd:
//! stable quality/group identities, worst-to-best storage, and equally ranked group members.
//! UI-independent edits preserve the catalogue and per-quality settings. Draft one-member
//! groups are permitted while editing; they cannot be saved until another quality is added.
use crate::api::settings::{QualityGroup, QualityProfile, QualityProfileItem};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityNode {
    Quality(i32),
    Group(i32),
}

impl QualityNode {
    pub fn of(item: &QualityProfileItem) -> Option<Self> {
        item.quality_id
            .map(Self::Quality)
            .or_else(|| item.group.as_ref().map(|g| Self::Group(g.id)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityDestination {
    /// Before/after refer to persisted order (worst first), including members of a group.
    Before(QualityNode),
    After(QualityNode),
    IntoGroup(i32),
    Lowest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QualityEdit {
    Move(QualityNode, QualityDestination),
    Step(QualityNode, bool),
    CreateGroup(i32),
    RenameGroup(i32, String),
    Ungroup(i32),
    Allow(QualityNode, bool),
}

impl QualityProfile {
    /// Apply atomically: rejected edits leave the original draft unchanged.
    pub fn edit_qualities(&mut self, edit: QualityEdit) -> Result<(), &'static str> {
        let mut next = self.clone();
        next.apply_quality_edit(edit)?;
        next.repair_quality_cutoff();
        *self = next;
        Ok(())
    }

    /// Only structural draft issues: the server also validates qualities, scores and limits.
    pub fn quality_edit_issue(&self) -> Option<&'static str> {
        for item in &self.items {
            if let Some(group) = &item.group {
                if group.quality_ids.len() < 2 {
                    return Some(
                        "Groups need at least two qualities. Move another quality into the group or ungroup it.",
                    );
                }
                if group.name.trim().is_empty() {
                    return Some("Every quality group needs a name.");
                }
            }
        }
        if !self.items.iter().any(|i| i.allowed) {
            return Some("Allow at least one quality before saving.");
        }
        if !self
            .items
            .iter()
            .any(|i| i.allowed && node_id(i) == Some(self.cutoff))
        {
            return Some("Choose an allowed quality or group as the cutoff.");
        }
        None
    }

    fn locate_quality_node(&self, node: QualityNode) -> Option<(usize, Option<usize>)> {
        for (index, item) in self.items.iter().enumerate() {
            if QualityNode::of(item) == Some(node) {
                return Some((index, None));
            }
            if let (QualityNode::Quality(id), Some(group)) = (node, &item.group)
                && let Some(member) = group.quality_ids.iter().position(|q| *q == id)
            {
                return Some((index, Some(member)));
            }
        }
        None
    }

    fn repair_quality_cutoff(&mut self) {
        if !self
            .items
            .iter()
            .any(|i| i.allowed && node_id(i) == Some(self.cutoff))
        {
            self.cutoff = self
                .items
                .iter()
                .rev()
                .find(|i| i.allowed)
                .and_then(node_id)
                .unwrap_or(0);
        }
    }

    fn apply_quality_edit(&mut self, edit: QualityEdit) -> Result<(), &'static str> {
        match edit {
            QualityEdit::CreateGroup(id) => {
                let (index, member) = self
                    .locate_quality_node(QualityNode::Quality(id))
                    .ok_or("Quality not found.")?;
                if member.is_some() {
                    return Err("This quality is already grouped.");
                }
                let mut group_id = self
                    .items
                    .iter()
                    .filter_map(|i| i.group.as_ref().map(|g| g.id))
                    .max()
                    .unwrap_or(1000)
                    .max(1000);
                loop {
                    group_id = group_id
                        .checked_add(1)
                        .ok_or("No group identifiers available.")?;
                    if crate::Quality::by_id(group_id).is_none()
                        && self
                            .locate_quality_node(QualityNode::Quality(group_id))
                            .is_none()
                    {
                        break;
                    }
                }
                self.items[index].quality_id = None;
                self.items[index].group = Some(QualityGroup {
                    id: group_id,
                    name: crate::Quality::by_id(id)
                        .map_or_else(|| "New group".into(), |q| q.name.into()),
                    quality_ids: vec![id],
                });
                if self.cutoff == id {
                    self.cutoff = group_id;
                }
            }
            QualityEdit::RenameGroup(id, name) => {
                let (index, _) = self
                    .locate_quality_node(QualityNode::Group(id))
                    .ok_or("Group not found.")?;
                self.items[index]
                    .group
                    .as_mut()
                    .ok_or("Group not found.")?
                    .name = name;
            }
            QualityEdit::Ungroup(id) => self.split_quality_group(id)?,
            QualityEdit::Allow(node, allowed) => {
                let (index, member) = self.locate_quality_node(node).ok_or("Quality not found.")?;
                if member.is_some() {
                    return Err("Grouped qualities use the group's allowed setting.");
                }
                self.items[index].allowed = allowed;
            }
            QualityEdit::Step(node, more) => {
                let (index, member) = self.locate_quality_node(node).ok_or("Quality not found.")?;
                if let Some(member) = member {
                    let group = self.items[index].group.as_mut().ok_or("Group not found.")?;
                    if let Some(target) = adjacent(member, group.quality_ids.len(), more) {
                        group.quality_ids.swap(member, target);
                    }
                } else if let Some(target) = adjacent(index, self.items.len(), more) {
                    self.items.swap(index, target);
                }
            }
            QualityEdit::Move(node, destination) => self.move_quality_node(node, destination)?,
        }
        Ok(())
    }

    fn split_quality_group(&mut self, id: i32) -> Result<(), &'static str> {
        let (index, _) = self
            .locate_quality_node(QualityNode::Group(id))
            .ok_or("Group not found.")?;
        let item = self.items.remove(index);
        let group = item.group.ok_or("Group not found.")?;
        if self.cutoff == id {
            self.cutoff = group.quality_ids.last().copied().unwrap_or(0);
        }
        self.items.splice(
            index..index,
            group.quality_ids.into_iter().map(|q| QualityProfileItem {
                quality_id: Some(q),
                group: None,
                allowed: item.allowed,
            }),
        );
        Ok(())
    }

    fn move_quality_node(
        &mut self,
        node: QualityNode,
        destination: QualityDestination,
    ) -> Result<(), &'static str> {
        let (source, member) = self.locate_quality_node(node).ok_or("Quality not found.")?;
        // Resolve the destination before removing anything. Groups always remain top-level;
        // a group dropped over a member is reordered relative to that member's whole group.
        let (target_group, target_node, after) = match destination {
            QualityDestination::IntoGroup(id) => {
                self.locate_quality_node(QualityNode::Group(id))
                    .ok_or("Group not found.")?;
                if matches!(node, QualityNode::Group(_)) {
                    return Err("Groups cannot be nested.");
                }
                (Some(id), None, true)
            }
            QualityDestination::Before(target) | QualityDestination::After(target) => {
                if node == target {
                    return Ok(());
                }
                let (index, child) = self
                    .locate_quality_node(target)
                    .ok_or("Drop target not found.")?;
                let root = QualityNode::of(&self.items[index]).ok_or("Invalid drop target.")?;
                if matches!(node, QualityNode::Group(_)) {
                    if node == root {
                        return Ok(());
                    }
                    (
                        None,
                        Some(root),
                        matches!(destination, QualityDestination::After(_)),
                    )
                } else {
                    (
                        child.and_then(|_| self.items[index].group.as_ref().map(|g| g.id)),
                        Some(target),
                        matches!(destination, QualityDestination::After(_)),
                    )
                }
            }
            QualityDestination::Lowest => (None, None, false),
        };
        let source_group = member.and_then(|_| self.items[source].group.as_ref().map(|g| g.id));
        if source_group.is_some()
            && source_group == target_group
            && target_node.is_none()
            && self.items[source]
                .group
                .as_ref()
                .is_some_and(|g| member == g.quality_ids.len().checked_sub(1))
        {
            return Ok(());
        }
        let moving_cutoff = matches!(node, QualityNode::Quality(id) if self.cutoff == id)
            || (source_group == Some(self.cutoff)
                && self.items[source]
                    .group
                    .as_ref()
                    .is_some_and(|g| g.quality_ids.len() == 1));
        let moved = if let Some(member) = member {
            let item = &mut self.items[source];
            let id = item
                .group
                .as_mut()
                .ok_or("Group not found.")?
                .quality_ids
                .remove(member);
            QualityProfileItem {
                quality_id: Some(id),
                group: None,
                allowed: item.allowed,
            }
        } else {
            self.items.remove(source)
        };
        if let Some(id) = target_group {
            let (index, _) = self
                .locate_quality_node(QualityNode::Group(id))
                .ok_or("Group not found.")?;
            let group = self.items[index].group.as_mut().ok_or("Group not found.")?;
            let position = match target_node {
                Some(QualityNode::Quality(q)) => {
                    group
                        .quality_ids
                        .iter()
                        .position(|m| *m == q)
                        .ok_or("Drop target not found.")?
                        + usize::from(after)
                }
                _ => group.quality_ids.len(),
            };
            group.quality_ids.insert(
                position,
                moved.quality_id.ok_or("Groups cannot be nested.")?,
            );
            if moving_cutoff {
                self.cutoff = id;
            }
        } else {
            let position = match target_node {
                Some(target) => {
                    self.locate_quality_node(target)
                        .ok_or("Drop target not found.")?
                        .0
                        + usize::from(after)
                }
                None => 0,
            };
            self.items.insert(position, moved);
            if moving_cutoff && let QualityNode::Quality(id) = node {
                self.cutoff = id;
            }
        }
        if let Some(id) = source_group
            && let Some((index, _)) = self.locate_quality_node(QualityNode::Group(id))
            && self.items[index]
                .group
                .as_ref()
                .is_some_and(|g| g.quality_ids.len() < 2)
        {
            self.split_quality_group(id)?;
        }
        Ok(())
    }
}

fn node_id(item: &QualityProfileItem) -> Option<i32> {
    match QualityNode::of(item)? {
        QualityNode::Quality(id) | QualityNode::Group(id) => Some(id),
    }
}

fn adjacent(index: usize, len: usize, more: bool) -> Option<usize> {
    if more {
        index.checked_add(1).filter(|i| *i < len)
    } else {
        index.checked_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile() -> QualityProfile {
        serde_json::from_value(
            json!({"id":1,"name":"Test","upgradeAllowed":true,"cutoff":3,
            "minFormatScore":0,"cutoffFormatScore":100,"minUpgradeFormatScore":1,
            "formatItems":[{"formatId":5,"score":100}],"codecMultipliers":{"HEVC":1.8},
            "sizeLimits":[{"qualityId":7,"minSize":1,"preferredSize":2,"maxSize":3}],
            "items":[{"qualityId":1,"allowed":false},
                {"group":{"id":1000,"name":"HDTV","qualityIds":[4,9]},"allowed":true},
                {"qualityId":3,"allowed":true},{"qualityId":7,"allowed":true}]}),
        )
        .unwrap()
    }
    fn catalogue(p: &QualityProfile) -> Vec<i32> {
        let mut ids: Vec<_> = p
            .items
            .iter()
            .flat_map(|i| {
                i.quality_id
                    .into_iter()
                    .chain(i.group.iter().flat_map(|g| g.quality_ids.iter().copied()))
            })
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn create_fill_reorder_and_split_group_preserves_settings_and_cutoff() {
        let mut p = profile();
        let before = p.clone();
        p.edit_qualities(QualityEdit::CreateGroup(3)).unwrap();
        let id = p.cutoff;
        assert_eq!(id, 1001);
        assert!(p.quality_edit_issue().is_some());
        p.edit_qualities(QualityEdit::RenameGroup(id, "1080p".into()))
            .unwrap();
        p.edit_qualities(QualityEdit::Move(
            QualityNode::Quality(7),
            QualityDestination::IntoGroup(id),
        ))
        .unwrap();
        assert!(p.quality_edit_issue().is_none());
        assert_eq!(p.items[2].group.as_ref().unwrap().quality_ids, [3, 7]);
        p.edit_qualities(QualityEdit::Step(QualityNode::Quality(3), true))
            .unwrap();
        p.edit_qualities(QualityEdit::Ungroup(id)).unwrap();
        assert_eq!(p.cutoff, 3);
        assert_eq!(catalogue(&p), catalogue(&before));
        assert_eq!(p.size_limits, before.size_limits);
        assert_eq!(p.codec_multipliers, before.codec_multipliers);
        assert_eq!(p.format_items, before.format_items);
    }

    #[test]
    fn removing_member_dissolves_singleton_without_losing_qualities_or_flags() {
        let mut p = profile();
        p.cutoff = 1000;
        p.edit_qualities(QualityEdit::Move(
            QualityNode::Quality(9),
            QualityDestination::After(QualityNode::Quality(7)),
        ))
        .unwrap();
        assert_eq!(p.cutoff, 4);
        assert_eq!(p.items[1].quality_id, Some(4));
        assert!(p.items[1].allowed && p.items.last().unwrap().allowed);
        assert_eq!(catalogue(&p), catalogue(&profile()));
    }

    #[test]
    fn moving_the_only_draft_member_keeps_its_cutoff() {
        let mut p = profile();
        p.edit_qualities(QualityEdit::CreateGroup(3)).unwrap();
        let group = p.cutoff;
        let draft = p.clone();
        p.edit_qualities(QualityEdit::Move(
            QualityNode::Quality(3),
            QualityDestination::IntoGroup(group),
        ))
        .unwrap();
        assert_eq!(p, draft, "dropping onto the same draft group is a no-op");
        p.edit_qualities(QualityEdit::Move(
            QualityNode::Quality(3),
            QualityDestination::After(QualityNode::Group(group)),
        ))
        .unwrap();
        assert_eq!(p.cutoff, 3);
        assert_eq!(catalogue(&p), catalogue(&profile()));
        assert!(p.quality_edit_issue().is_none());
    }

    #[test]
    fn group_reordering_is_atomic_and_groups_cannot_nest() {
        let mut p = profile();
        p.edit_qualities(QualityEdit::CreateGroup(3)).unwrap();
        let id = p.cutoff;
        p.edit_qualities(QualityEdit::Move(
            QualityNode::Quality(7),
            QualityDestination::IntoGroup(id),
        ))
        .unwrap();
        let before = p.clone();
        assert!(
            p.edit_qualities(QualityEdit::Move(
                QualityNode::Group(1000),
                QualityDestination::IntoGroup(id)
            ))
            .is_err()
        );
        assert_eq!(p, before);
        assert!(
            p.edit_qualities(QualityEdit::Move(
                QualityNode::Quality(9),
                QualityDestination::IntoGroup(9999)
            ))
            .is_err()
        );
        assert_eq!(p, before);
        p.edit_qualities(QualityEdit::Move(
            QualityNode::Group(1000),
            QualityDestination::After(QualityNode::Quality(7)),
        ))
        .unwrap();
        assert_eq!(
            QualityNode::of(p.items.last().unwrap()),
            Some(QualityNode::Group(1000))
        );
        assert_eq!(p.cutoff, id);
    }

    #[test]
    fn member_reordering_and_disabled_destination_keep_a_valid_cutoff() {
        let mut p = profile();
        p.edit_qualities(QualityEdit::Move(
            QualityNode::Quality(4),
            QualityDestination::After(QualityNode::Quality(9)),
        ))
        .unwrap();
        assert_eq!(p.items[1].group.as_ref().unwrap().quality_ids, [9, 4]);
        p.edit_qualities(QualityEdit::Allow(QualityNode::Group(1000), false))
            .unwrap();
        p.edit_qualities(QualityEdit::Move(
            QualityNode::Quality(3),
            QualityDestination::IntoGroup(1000),
        ))
        .unwrap();
        assert_eq!(p.cutoff, 7);
        assert!(p.quality_edit_issue().is_none());
        p.edit_qualities(QualityEdit::Allow(QualityNode::Quality(7), false))
            .unwrap();
        assert!(p.quality_edit_issue().is_some());
    }

    #[test]
    fn all_drag_destinations_preserve_the_catalogue_and_reject_atomically() {
        let original = profile();
        let nodes = [
            QualityNode::Quality(1),
            QualityNode::Quality(4),
            QualityNode::Quality(9),
            QualityNode::Quality(3),
            QualityNode::Quality(7),
            QualityNode::Group(1000),
        ];
        let destinations: Vec<_> = nodes
            .into_iter()
            .flat_map(|n| [QualityDestination::Before(n), QualityDestination::After(n)])
            .chain([
                QualityDestination::IntoGroup(1000),
                QualityDestination::Lowest,
            ])
            .collect();
        for node in nodes {
            for &destination in &destinations {
                let mut p = original.clone();
                let result = p.edit_qualities(QualityEdit::Move(node, destination));
                assert_eq!(
                    catalogue(&p),
                    catalogue(&original),
                    "{node:?} -> {destination:?}"
                );
                if result.is_err() {
                    assert_eq!(p, original);
                }
                assert!(
                    p.quality_edit_issue().is_none(),
                    "{node:?} -> {destination:?}"
                );
            }
        }
    }
}
