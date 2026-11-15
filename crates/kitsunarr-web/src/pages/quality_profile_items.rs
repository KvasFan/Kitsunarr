//! Drag/drop and keyboard controls for Tanukarr-style quality groups. All edits, cutoff
//! changes and group normalization live in the shared domain model.
use dioxus::prelude::*;
use kitsunarr_domain::Quality;
use kitsunarr_domain::api::settings::QualityProfile;
use kitsunarr_domain::quality_profile::{
    QualityDestination as To, QualityEdit as Edit, QualityNode as Node,
};

fn key(node: Node) -> String {
    match node {
        Node::Quality(id) => format!("q-{id}"),
        Node::Group(id) => format!("g-{id}"),
    }
}
fn quality_name(id: i32) -> String {
    Quality::by_id(id).map_or_else(|| id.to_string(), |q| q.name.to_string())
}

#[component]
pub(super) fn QualityProfileItems(mut profile: Signal<QualityProfile>) -> Element {
    let mut editing = use_signal(|| false);
    let mut dragging = use_signal(|| None::<Node>);
    let mut error = use_signal(|| None::<String>);
    let current = profile();
    let groups: Vec<(i32, String)> = current
        .items
        .iter()
        .filter_map(|i| i.group.as_ref().map(|g| (g.id, g.name.clone())))
        .collect();
    let on_edit = EventHandler::new(move |edit| {
        let result = profile.write().edit_qualities(edit);
        error.set(result.err().map(str::to_string));
    });
    rsx! {
        div { class: "quality-editor", "data-dragging": if dragging().is_some() { "true" } else { "false" },
            div { class: "row",
                h3 { class: "grow", "Qualities" }
                button { class: "btn sm", "aria-pressed": editing(), onclick: move |_| editing.toggle(),
                    if editing() { "Done editing groups" } else { "Edit groups" }
                }
            }
            p { class: "help small faint", "Best first. Drag the handles or use the arrows to reorder. Qualities inside a group rank equally." }
            if editing() {
                p { class: "help small faint", "Create a group from a quality, then drag more qualities into it or use the Group selectors. Cutoff follows a quality into its group; ungrouping uses the group's highest member." }
            }
            if let Some(message) = error() { p { class: "quality-edit-error", role: "alert", "{message}" } }
            if let Some(issue) = current.quality_edit_issue() { p { class: "quality-edit-error", role: "status", "{issue}" } }
            div { class: "quality-list",
                for (index, item) in current.items.iter().enumerate().rev() {
                    if let Some(node) = Node::of(item) {
                        {
                            let label = item.group.as_ref().map_or_else(|| quality_name(item.quality_id.unwrap_or(0)), |g| g.name.clone());
                            let root_key = key(node);
                            rsx! {
                                div { key: "{root_key}", class: "quality-node", "data-quality-root": "{root_key}",
                                    div { class: "quality-insert-target", "aria-label": "Place above {label}",
                                        ondragover: move |e: DragEvent| { if dragging().is_some() { e.prevent_default(); e.stop_propagation(); } },
                                        ondrop: move |e: DragEvent| {
                                            e.prevent_default(); e.stop_propagation();
                                            if let Some(source) = dragging() { on_edit.call(Edit::Move(source,To::After(node))); }
                                            dragging.set(None);
                                        }
                                    }
                                    div { class: if item.group.is_some() { "quality-card quality-group" } else { "quality-card" },
                                        role: "group", "aria-label": "{label}",
                                        QualityLine { node, label: label.clone(), allowed: item.allowed,
                                            parent: None, editing: editing(), groups: groups.clone(),
                                            more: index + 1 < current.items.len(), less: index > 0,
                                            dragging, on_edit }
                                        if let Some(group) = &item.group {
                                            if editing() {
                                                div { class: "quality-members",
                                                    for (member, id) in group.quality_ids.iter().copied().enumerate().rev() {
                                                        QualityLine { key: "{id}", node: Node::Quality(id), label: quality_name(id),
                                                            allowed: item.allowed, parent: Some(group.id), editing: true,
                                                            groups: groups.clone(), more: member + 1 < group.quality_ids.len(), less: member > 0,
                                                            dragging, on_edit }
                                                    }
                                                }
                                                div { class: "quality-group-drop", "aria-label": "Add qualities to {label}",
                                                    ondragover: move |e: DragEvent| { if matches!(dragging(),Some(Node::Quality(_))) { e.prevent_default(); e.stop_propagation(); } },
                                                    ondrop: {
                                                        let id = group.id;
                                                        move |e: DragEvent| {
                                                            e.prevent_default(); e.stop_propagation();
                                                            if let Some(source) = dragging() { on_edit.call(Edit::Move(source,To::IntoGroup(id))); }
                                                            dragging.set(None);
                                                        }
                                                    }, "Drop qualities here"
                                                }
                                            } else {
                                                div { class: "quality-summary small muted", "{group.quality_ids.iter().rev().map(|q| quality_name(*q)).collect::<Vec<_>>().join(\", \")}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if editing() || dragging().is_some() {
                    div { class: "quality-group-drop", "aria-label": "Place at lowest preference",
                        ondragover: move |e: DragEvent| { if dragging().is_some() { e.prevent_default(); e.stop_propagation(); } },
                        ondrop: move |e: DragEvent| {
                            e.prevent_default(); e.stop_propagation();
                            if let Some(source) = dragging() { on_edit.call(Edit::Move(source,To::Lowest)); }
                            dragging.set(None);
                        }, "Drop here to move out of a group or to the lowest preference"
                    }
                }
            }
        }
    }
}

#[component]
fn QualityLine(
    node: Node,
    label: String,
    allowed: bool,
    parent: Option<i32>,
    editing: bool,
    groups: Vec<(i32, String)>,
    more: bool,
    less: bool,
    mut dragging: Signal<Option<Node>>,
    on_edit: EventHandler<Edit>,
) -> Element {
    let node_key = key(node);
    let destination = if let Node::Group(id) = node {
        To::IntoGroup(id)
    } else {
        To::After(node)
    };
    rsx! {
        div { class: "quality-row", "data-quality-key": "{node_key}", "data-quality-label": "{label}",
            ondragover: move |e: DragEvent| { if dragging().is_some() { e.prevent_default(); e.stop_propagation(); } },
            ondrop: move |e: DragEvent| {
                e.prevent_default(); e.stop_propagation();
                if let Some(source) = dragging() {
                    let target = if matches!(source,Node::Group(_)) { To::After(node) } else { destination };
                    on_edit.call(Edit::Move(source,target));
                }
                dragging.set(None);
            },
            button { class: "btn sm ghost quality-drag", draggable: "true", "aria-label": "Drag {label}", title: "Drag to reorder or move into a group",
                ondragstart: move |e: DragEvent| {
                    let transfer = e.data_transfer();
                    let _ = transfer.set_data("text/plain", &key(node));
                    transfer.set_effect_allowed("move");
                    e.stop_propagation(); dragging.set(Some(node));
                },
                ondragend: move |_| dragging.set(None), "⠿"
            }
            if parent.is_none() {
                input { r#type: "checkbox", "aria-label": "Allow {label}", checked: allowed,
                    onchange: move |e: FormEvent| on_edit.call(Edit::Allow(node,e.checked())) }
            }
            if let Node::Group(id) = node {
                if editing {
                    input { class: "input quality-group-name", "aria-label": "Group name", value: "{label}",
                        oninput: move |e: FormEvent| on_edit.call(Edit::RenameGroup(id,e.value())) }
                } else { strong { class: "quality-name", "{label}" } }
            } else { span { class: "quality-name", "{label}" } }
            div { class: "quality-controls",
                if editing {
                    match node {
                        Node::Group(id) => rsx! {
                            button { class: "btn sm ghost", "aria-label": "Ungroup {label}", onclick: move |_| on_edit.call(Edit::Ungroup(id)), "Ungroup" }
                        },
                        Node::Quality(id) => rsx! {
                            select { class: "select quality-group-select", "aria-label": "Group for {label}", value: "{parent.map(|id| id.to_string()).unwrap_or_default()}",
                                onchange: move |e: FormEvent| {
                                    if let Ok(group) = e.value().parse::<i32>() {
                                        on_edit.call(Edit::Move(node,To::IntoGroup(group)));
                                    } else if let Some(group) = parent {
                                        on_edit.call(Edit::Move(node,To::After(Node::Group(group))));
                                    }
                                },
                                option { value: "", selected: parent.is_none(), "No group" }
                                for (id, name) in groups.iter() { option { key: "{id}", value: "{id}", selected: parent == Some(*id), "{name}" } }
                            }
                            if parent.is_none() { button { class: "btn sm ghost", "aria-label": "Group {label}", onclick: move |_| on_edit.call(Edit::CreateGroup(id)), "Group" } }
                        },
                    }
                }
                button { class: "btn sm ghost", disabled: !more, "aria-label": "Prefer {label} more", onclick: move |_| on_edit.call(Edit::Step(node,true)), "↑" }
                button { class: "btn sm ghost", disabled: !less, "aria-label": "Prefer {label} less", onclick: move |_| on_edit.call(Edit::Step(node,false)), "↓" }
            }
        }
    }
}
