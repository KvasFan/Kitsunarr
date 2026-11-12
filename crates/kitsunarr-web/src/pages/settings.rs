//! Settings pages.

use dioxus::prelude::*;
use kitsunarr_domain::api::auth::ChangePasswordRequest;
use kitsunarr_domain::api::settings::{
    CustomFormat, DownloadClientOptionsSettings, GeneralSettings, IndexerOptionsSettings,
    MediaManagementSettings, MetadataSourcesSettings, NamingSettings, ProperDownloadType,
    ProviderItem, ProviderKind, QualityProfile, REDACTED, RootFolderCreate,
};
use kitsunarr_domain::{MediaKind, Quality};
use serde_json::Value;

use crate::components::common::{Badge, Confirm, Empty, ErrorBox, Field, Loading, Modal, Switch};
use crate::state::{use_app, use_client};
use crate::util::format_bytes;

// ---------------------------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------------------------

#[component]
pub fn SettingsGeneral() -> Element {
    let app = use_app();
    let client = use_client();
    let mut form = use_signal(|| None::<GeneralSettings>);
    let mut api_key = use_signal(|| None::<String>);
    let mut reveal = use_signal(|| false);
    let mut confirm_regen = use_signal(|| false);
    let mut current_pw = use_signal(String::new);
    let mut new_pw = use_signal(String::new);

    let c0 = client.clone();
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.general().await {
                Ok(g) => form.set(Some(g)),
                Err(e) => app.error(&e),
            }
            if let Ok(k) = c.api_key().await {
                api_key.set(Some(k.api_key));
            }
        }
    });
    let c1 = client.clone();
    let save = move |_| {
        let Some(g) = form() else { return };
        let c = c1.clone();
        spawn(async move {
            match c.put_general(&g).await {
                Ok(_) => app.success("Settings saved."),
                Err(e) => app.error(&e),
            }
        });
    };
    let c2 = client.clone();
    let regenerate = move |_| {
        confirm_regen.set(false);
        let c = c2.clone();
        spawn(async move {
            match c.regenerate_api_key().await {
                Ok(k) => {
                    api_key.set(Some(k.api_key));
                    reveal.set(true);
                    app.success(
                        "A new API key was generated. Update the applications that use it.",
                    );
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c3 = client.clone();
    let change_password = move |e: FormEvent| {
        e.prevent_default();
        let c = c3.clone();
        let req = ChangePasswordRequest {
            current_password: current_pw(),
            new_password: new_pw(),
        };
        spawn(async move {
            match c.change_password(&req).await {
                Ok(()) => {
                    current_pw.set(String::new());
                    new_pw.set(String::new());
                    app.success("Password changed. Other sessions were signed out.");
                }
                Err(e) => app.error(&e),
            }
        });
    };

    let key = api_key().unwrap_or_default();
    let shown = if reveal() {
        key.clone()
    } else {
        "•".repeat(key.len().min(32))
    };
    rsx! {
        div { class: "page-head", h1 { "General" } }
        div { class: "card",
            h3 { "Instance" }
            match form() {
                None => rsx! { Loading {} },
                Some(g) => rsx! {
                    div { class: "form-grid",
                        Field { label: "Instance name",
                            input { class: "input", value: "{g.instance_name}", oninput: move |e| { if let Some(f) = form.write().as_mut() { f.instance_name = e.value(); } } }
                        }
                        Field { label: "First day of the week",
                            select { class: "select", value: "{g.first_day_of_week}", onchange: move |e: FormEvent| { if let Some(f) = form.write().as_mut() { f.first_day_of_week = e.value().parse().unwrap_or(1); } },
                                option { value: "1", "Monday" } option { value: "0", "Sunday" } option { value: "6", "Saturday" }
                            }
                        }
                        Field { label: "Interface language", help: "More languages can be added to the string tables.".to_string(),
                            select { class: "select", disabled: true, option { "English" } }
                        }
                    }
                    div { class: "row end", button { class: "btn primary", onclick: save, {crate::i18n::t().save} } }
                },
            }
        }
        div { class: "card",
            h3 { "API key" }
            p { class: "muted small", "Use it as the X-Api-Key header (or ?apikey=) from other applications. Compatible with the header Sonarr uses." }
            div { class: "row",
                input { class: "input mono grow", readonly: true, value: "{shown}", "aria-label": "API key" }
                button { class: "btn", onclick: move |_| reveal.toggle(), if reveal() { "Hide" } else { "Show" } }
                button { class: "btn", onclick: move |_| { crate::util::copy_to_clipboard(&key); app.success("Copied."); }, "Copy" }
                button { class: "btn danger", onclick: move |_| confirm_regen.set(true), "Regenerate" }
            }
        }
        if confirm_regen() {
            Confirm {
                title: "Regenerate API key",
                message: "Applications using the current key will stop working until you give them the new one.",
                confirm_label: "Regenerate",
                danger: true,
                on_confirm: regenerate,
                on_cancel: move |_| confirm_regen.set(false),
            }
        }
        form { class: "card", onsubmit: change_password,
            h3 { "Change password" }
            div { class: "form-grid",
                Field { label: "Current password", input { class: "input", r#type: "password", autocomplete: "current-password", value: "{current_pw}", oninput: move |e| current_pw.set(e.value()) } }
                Field { label: "New password", help: "At least 10 characters.".to_string(), input { class: "input", r#type: "password", autocomplete: "new-password", value: "{new_pw}", oninput: move |e| new_pw.set(e.value()) } }
            }
            div { class: "row end", button { class: "btn primary", r#type: "submit", disabled: current_pw().is_empty() || new_pw().is_empty(), "Change password" } }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Media management
// ---------------------------------------------------------------------------------------------

#[component]
pub fn SettingsMedia() -> Element {
    let app = use_app();
    let client = use_client();
    let mut new_path = use_signal(String::new);
    let mut new_kind = use_signal(String::new);
    let mut naming = use_signal(|| None::<NamingSettings>);
    let mut mm = use_signal(|| None::<MediaManagementSettings>);
    let c0 = client.clone();
    let mut roots = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c0.clone();
        async move { c.root_folders().await }
    });
    let c1 = client.clone();
    use_future(move || {
        let c = c1.clone();
        async move {
            match c.naming().await {
                Ok(n) => naming.set(Some(n)),
                Err(e) => app.error(&e),
            }
            match c.media_management().await {
                Ok(m) => mm.set(Some(m)),
                Err(e) => app.error(&e),
            }
        }
    });
    let c2 = client.clone();
    let add_root = move |_| {
        let c = c2.clone();
        let req = RootFolderCreate {
            path: new_path(),
            media_kind: new_kind().parse::<MediaKind>().ok(),
        };
        spawn(async move {
            match c.create_root_folder(&req).await {
                Ok(_) => {
                    new_path.set(String::new());
                    roots.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c3 = client.clone();
    let delete_root = move |id: i64| {
        let c = c3.clone();
        spawn(async move {
            match c.delete_root_folder(id).await {
                Ok(()) => roots.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    let c4 = client.clone();
    let save_naming = move |_| {
        let Some(n) = naming() else { return };
        let c = c4.clone();
        spawn(async move {
            match c.put_naming(&n).await {
                Ok(_) => app.success("Naming saved."),
                Err(e) => app.error(&e),
            }
        });
    };
    let c5 = client.clone();
    let save_mm = move |_| {
        let Some(m) = mm() else { return };
        let c = c5.clone();
        spawn(async move {
            match c.put_media_management(&m).await {
                Ok(_) => app.success("File management settings saved."),
                Err(e) => app.error(&e),
            }
        });
    };

    rsx! {
        div { class: "page-head", h1 { "Media management" } }
        div { class: "card",
            h3 { "Root folders" }
            match &*roots.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) => rsx! {
                    if list.is_empty() { p { class: "muted", "No root folders yet." } }
                    div { class: "table-wrap", style: "margin-bottom: var(--sp-3)",
                        table { class: "table",
                            thead { tr { th { "Path" } th { "Library" } th { "Free space" } th { "" } } }
                            tbody {
                                for r in list.iter() {
                                    tr { key: "{r.id}",
                                        td { class: "mono", "{r.path}" if !r.accessible { " " Badge { text: "not accessible", tone: "danger" } } }
                                        td { {r.media_kind.map(|k| k.to_string()).unwrap_or_else(|| "any".into())} }
                                        td { {r.free_space.map(|f| format!("{} free of {}", format_bytes(f as i64), format_bytes(r.total_space.unwrap_or(0) as i64))).unwrap_or_else(|| "—".into())} }
                                        td { class: "shrink", button { class: "btn sm danger", onclick: { let id = r.id; let d = delete_root.clone(); move |_| d(id) }, "Remove" } }
                                    }
                                }
                            }
                        }
                    }
                },
            }
            div { class: "row",
                input { class: "input mono grow", placeholder: "/media/tv", "aria-label": "New root folder path", value: "{new_path}", oninput: move |e| new_path.set(e.value()) }
                select { class: "select", "aria-label": "Library", onchange: move |e: FormEvent| new_kind.set(e.value()),
                    option { value: "", "Any library" } option { value: "series", "TV series" } option { value: "anime", "Anime" } option { value: "movie", "Movies" }
                }
                button { class: "btn primary", disabled: new_path().trim().is_empty(), onclick: add_root, "Add" }
            }
            p { class: "help small faint", "The folder must already exist; Kitsunarr never creates or removes it." }
        }
        div { class: "card",
            h3 { "Episode and movie naming" }
            match naming() {
                None => rsx! { Loading {} },
                Some(n) => rsx! {
                    Switch { checked: n.rename_episodes, label: "Rename files on import", onchange: move |v| { if let Some(x) = naming.write().as_mut() { x.rename_episodes = v; } } }
                    p { class: "help small faint", "Renaming takes effect once the import pipeline is implemented; formats use Sonarr's tokens." }
                    div { class: "form-grid",
                        NamingInput { label: "Standard episode format", value: n.standard_episode_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.standard_episode_format = v; } } }
                        NamingInput { label: "Daily episode format", value: n.daily_episode_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.daily_episode_format = v; } } }
                        NamingInput { label: "Anime episode format", value: n.anime_episode_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.anime_episode_format = v; } } }
                        NamingInput { label: "Series folder format", value: n.series_folder_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.series_folder_format = v; } } }
                        NamingInput { label: "Season folder format", value: n.season_folder_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.season_folder_format = v; } } }
                        NamingInput { label: "Movie file format", value: n.movie_file_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.movie_file_format = v; } } }
                    }
                    div { class: "row end", button { class: "btn primary", onclick: save_naming, "Save naming" } }
                },
            }
        }
        div { class: "card",
            h3 { "File management" }
            match mm() {
                None => rsx! { Loading {} },
                Some(m) => rsx! {
                    div { class: "stack", style: "margin-bottom: var(--sp-4)",
                        Switch { checked: m.use_hardlinks, label: "Use hardlinks instead of copying", onchange: move |v| { if let Some(x) = mm.write().as_mut() { x.use_hardlinks = v; } } }
                        Switch { checked: m.delete_empty_folders, label: "Delete empty series folders", onchange: move |v| { if let Some(x) = mm.write().as_mut() { x.delete_empty_folders = v; } } }
                        Switch { checked: m.skip_unmonitored_episodes_from_packs, label: "Skip unmonitored episodes when importing packs", onchange: move |v| { if let Some(x) = mm.write().as_mut() { x.skip_unmonitored_episodes_from_packs = v; } } }
                        Switch { checked: m.skip_free_space_check, label: "Skip the free-space check", onchange: move |v| { if let Some(x) = mm.write().as_mut() { x.skip_free_space_check = v; } } }
                        Switch { checked: m.import_extra_files, label: "Import extra files (subtitles…)", onchange: move |v| { if let Some(x) = mm.write().as_mut() { x.import_extra_files = v; } } }
                    }
                    div { class: "form-grid",
                        Field { label: "Recycle bin", help: "Absolute path; empty to delete permanently.".to_string(),
                            input { class: "input mono", value: "{m.recycle_bin.clone().unwrap_or_default()}", oninput: move |e| { if let Some(x) = mm.write().as_mut() { x.recycle_bin = Some(e.value()).filter(|v| !v.is_empty()); } } }
                        }
                        Field { label: "Minimum free space (MiB)",
                            input { class: "input", r#type: "number", min: "0", value: "{m.minimum_free_space_mb}", oninput: move |e| { if let Some(x) = mm.write().as_mut() { x.minimum_free_space_mb = e.value().parse().unwrap_or(0); } } }
                        }
                        Field { label: "Extra file extensions",
                            input { class: "input", value: "{m.extra_file_extensions}", oninput: move |e| { if let Some(x) = mm.write().as_mut() { x.extra_file_extensions = e.value(); } } }
                        }
                        Field { label: "Propers and repacks", help: "Whether releases fixing a bad release are preferred, and upgrade existing files (as in Sonarr).".to_string(),
                            select { class: "select", "aria-label": "Propers and repacks",
                                onchange: move |e: FormEvent| {
                                    if let Some(x) = mm.write().as_mut() {
                                        x.download_propers_and_repacks = match e.value().as_str() {
                                            "doNotUpgrade" => ProperDownloadType::DoNotUpgrade,
                                            "doNotPrefer" => ProperDownloadType::DoNotPrefer,
                                            _ => ProperDownloadType::PreferAndUpgrade,
                                        };
                                    }
                                },
                                option { value: "preferAndUpgrade", selected: m.download_propers_and_repacks == ProperDownloadType::PreferAndUpgrade, "Prefer and upgrade" }
                                option { value: "doNotUpgrade", selected: m.download_propers_and_repacks == ProperDownloadType::DoNotUpgrade, "Prefer, do not upgrade" }
                                option { value: "doNotPrefer", selected: m.download_propers_and_repacks == ProperDownloadType::DoNotPrefer, "Do not prefer" }
                            }
                        }
                    }
                    div { class: "row end", button { class: "btn primary", onclick: save_mm, "Save" } }
                },
            }
        }
    }
}

#[component]
fn NamingInput(label: &'static str, value: String, on_change: EventHandler<String>) -> Element {
    rsx! {
        Field { label: label.to_string(),
            input { class: "input mono", value: "{value}", oninput: move |e| on_change.call(e.value()) }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Quality profiles
// ---------------------------------------------------------------------------------------------

#[component]
pub fn SettingsProfiles() -> Element {
    let app = use_app();
    let client = use_client();
    let mut editing = use_signal(|| None::<QualityProfile>);
    let c0 = client.clone();
    let mut profiles = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c0.clone();
        async move { c.quality_profiles().await }
    });
    let c1 = client.clone();
    let formats = use_resource(move || {
        let c = c1.clone();
        async move { c.custom_formats().await }
    });
    let c2 = client.clone();
    let save = move |p: QualityProfile| {
        let c = c2.clone();
        spawn(async move {
            let result = if p.id == 0 {
                c.create_quality_profile(&p).await
            } else {
                c.update_quality_profile(&p).await
            };
            match result {
                Ok(_) => {
                    editing.set(None);
                    profiles.restart();
                    app.success("Profile saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c3 = client.clone();
    let delete = move |id: i64| {
        let c = c3.clone();
        spawn(async move {
            match c.delete_quality_profile(id).await {
                Ok(()) => {
                    editing.set(None);
                    profiles.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let list = match &*profiles.read() {
        Some(Ok(l)) => l.clone(),
        Some(Err(e)) => return rsx! { ErrorBox { message: e.to_string() } },
        None => return rsx! { Loading {} },
    };
    let template = list.first().cloned();
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Quality profiles" }
            if let Some(t) = template {
                button { class: "btn primary", onclick: move |_| editing.set(Some(QualityProfile { id: 0, name: "New profile".into(), ..t.clone() })), "+ New profile" }
            }
        }
        div { class: "media-grid", style: "grid-template-columns: repeat(auto-fill, minmax(16rem, 1fr))",
            for p in list.iter() {
                button { key: "{p.id}", class: "choice", onclick: { let p = p.clone(); move |_| editing.set(Some(p.clone())) },
                    h3 { "{p.name}" }
                    p { class: "small muted",
                        {allowed_names(p).join(", ")}
                    }
                    div { class: "row tight",
                        if p.upgrade_allowed { Badge { text: "upgrades", tone: "accent" } }
                        Badge { text: format!("cutoff: {}", cutoff_name(p)) }
                        if !p.format_items.is_empty() { Badge { text: format!("{} format scores", p.format_items.len()) } }
                    }
                }
            }
        }
        if let Some(p) = editing() {
            ProfileEditor {
                profile: p,
                formats: match &*formats.read() { Some(Ok(f)) => f.clone(), _ => Vec::new() },
                on_save: save,
                on_delete: delete,
                on_close: move |_| editing.set(None),
            }
        }
    }
}

fn item_name(item: &kitsunarr_domain::api::settings::QualityProfileItem) -> String {
    match (&item.quality_id, &item.group) {
        (Some(q), _) => Quality::by_id(*q).map_or_else(|| format!("#{q}"), |q| q.name.to_string()),
        (None, Some(g)) => g.name.clone(),
        _ => "?".into(),
    }
}

fn allowed_names(p: &QualityProfile) -> Vec<String> {
    p.items
        .iter()
        .rev()
        .filter(|i| i.allowed)
        .map(item_name)
        .collect()
}

fn cutoff_name(p: &QualityProfile) -> String {
    p.items
        .iter()
        .find(|i| {
            i.quality_id == Some(p.cutoff) || i.group.as_ref().is_some_and(|g| g.id == p.cutoff)
        })
        .map(item_name)
        .unwrap_or_else(|| p.cutoff.to_string())
}

#[component]
fn ProfileEditor(
    profile: QualityProfile,
    formats: Vec<CustomFormat>,
    on_save: EventHandler<QualityProfile>,
    on_delete: EventHandler<i64>,
    on_close: EventHandler<()>,
) -> Element {
    let mut p = use_signal(|| profile.clone());
    let mut confirm = use_signal(|| false);
    let id = profile.id;
    let current = p();
    let allowed_ids: Vec<(i32, String)> = current
        .items
        .iter()
        .filter(|i| i.allowed)
        .map(|i| {
            (
                i.quality_id
                    .unwrap_or_else(|| i.group.as_ref().map_or(0, |g| g.id)),
                item_name(i),
            )
        })
        .collect();
    rsx! {
        Modal {
            title: if id == 0 { "New quality profile".to_string() } else { format!("Edit “{}”", profile.name) },
            wide: true,
            on_close: move |_| on_close.call(()),
            footer: rsx! {
                if id != 0 { button { class: "btn danger", onclick: move |_| confirm.set(true), "Delete" } }
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", onclick: move |_| on_save.call(p()), "Save" }
            },
            div { class: "form-grid",
                Field { label: "Name", input { class: "input", value: "{current.name}", oninput: move |e| p.write().name = e.value() } }
                Field { label: "Upgrade until (cutoff)",
                    select { class: "select", value: "{current.cutoff}", onchange: move |e: FormEvent| p.write().cutoff = e.value().parse().unwrap_or(0),
                        for (qid, name) in allowed_ids.iter() { option { value: "{qid}", "{name}" } }
                    }
                }
            }
            Switch { checked: current.upgrade_allowed, label: "Upgrades allowed", onchange: move |v| p.write().upgrade_allowed = v }
            h3 { style: "margin-top: var(--sp-4)", "Qualities" }
            p { class: "help small faint", "Listed best first. Grouped qualities rank equally." }
            div { class: "table-wrap",
                table { class: "table",
                    tbody {
                        for (idx, item) in current.items.iter().enumerate().rev() {
                            tr { key: "{idx}",
                                td { class: "shrink",
                                    input {
                                        r#type: "checkbox",
                                        "aria-label": "Allow {item_name(item)}",
                                        checked: item.allowed,
                                        onchange: move |e: FormEvent| p.write().items[idx].allowed = e.checked(),
                                    }
                                }
                                td {
                                    "{item_name(item)}"
                                    if let Some(g) = &item.group {
                                        span { class: "xs muted", " ({g.quality_ids.iter().filter_map(|q| Quality::by_id(*q)).map(|q| q.name).collect::<Vec<_>>().join(\", \")})" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            h3 { style: "margin-top: var(--sp-4)", "Custom format scores" }
            if formats.is_empty() { p { class: "muted small", "No custom formats defined." } }
            div { class: "form-grid",
                for f in formats.iter() {
                    Field { key: "{f.id}", label: f.name.clone(),
                        input {
                            class: "input",
                            r#type: "number",
                            value: "{current.format_items.iter().find(|s| s.format_id == f.id).map_or(0, |s| s.score)}",
                            oninput: {
                                let fid = f.id;
                                move |e: FormEvent| {
                                    let score: i32 = e.value().parse().unwrap_or(0);
                                    let mut w = p.write();
                                    w.format_items.retain(|s| s.format_id != fid);
                                    if score != 0 { w.format_items.push(kitsunarr_domain::api::settings::FormatScore { format_id: fid, score }); }
                                }
                            },
                        }
                    }
                }
            }
            div { class: "form-grid",
                Field { label: "Minimum format score", input { class: "input", r#type: "number", value: "{current.min_format_score}", oninput: move |e| p.write().min_format_score = e.value().parse().unwrap_or(0) } }
                Field { label: "Upgrade until format score", input { class: "input", r#type: "number", value: "{current.cutoff_format_score}", oninput: move |e| p.write().cutoff_format_score = e.value().parse().unwrap_or(0) } }
            }
        }
        if confirm() {
            Confirm {
                title: "Delete profile",
                message: "Profiles still used by series or movies cannot be deleted.",
                confirm_label: "Delete",
                danger: true,
                on_confirm: move |_| on_delete.call(id),
                on_cancel: move |_| confirm.set(false),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Custom formats
// ---------------------------------------------------------------------------------------------

#[component]
pub fn SettingsFormats() -> Element {
    let app = use_app();
    let client = use_client();
    let mut editing = use_signal(|| None::<(i64, String, bool, String)>);
    let mut editor_error = use_signal(|| None::<String>);
    let c0 = client.clone();
    let mut formats = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c0.clone();
        async move { c.custom_formats().await }
    });
    let c1 = client.clone();
    let save = move |_| {
        let Some((id, name, renaming, json)) = editing() else {
            return;
        };
        let specs = match serde_json::from_str(&json) {
            Ok(s) => s,
            Err(e) => {
                editor_error.set(Some(format!("Specifications must be valid JSON: {e}")));
                return;
            }
        };
        let cf = CustomFormat {
            id,
            name,
            include_when_renaming: renaming,
            specifications: specs,
        };
        let c = c1.clone();
        spawn(async move {
            let result = if id == 0 {
                c.post::<CustomFormat, _>("/customformats", &cf).await
            } else {
                c.put::<CustomFormat, _>(&format!("/customformats/{id}"), &cf)
                    .await
            };
            match result {
                Ok(_) => {
                    editing.set(None);
                    formats.restart();
                    app.success("Custom format saved.");
                }
                Err(e) => editor_error.set(Some(e.to_string())),
            }
        });
    };
    let c2 = client.clone();
    let delete = move |id: i64| {
        let c = c2.clone();
        spawn(async move {
            match c.delete_custom_format(id).await {
                Ok(()) => formats.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    let example = r#"[
  {
    "name": "x265",
    "implementation": "ReleaseTitleSpecification",
    "negate": false,
    "required": true,
    "fields": { "value": "[xh][ ._-]?265|\\bHEVC\\b" }
  }
]"#;
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Custom formats" }
            button { class: "btn primary", onclick: move |_| { editor_error.set(None); editing.set(Some((0, String::new(), false, example.to_string()))) }, "+ New format" }
        }
        p { class: "muted small", "Formats use Sonarr's specification types (ReleaseTitle, ReleaseGroup, Source, Resolution, Language, Size, IndexerFlag, ReleaseType). Scores are assigned per quality profile. Release matching uses them once indexer search is implemented." }
        match &*formats.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "No custom formats" } },
            Some(Ok(list)) => rsx! {
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Name" } th { "Specifications" } th { "" } } }
                        tbody {
                            for f in list.iter() {
                                tr { key: "{f.id}",
                                    td { strong { "{f.name}" } if f.include_when_renaming { " " Badge { text: "in file names" } } }
                                    td { class: "small",
                                        for (i, s) in f.specifications.iter().enumerate() {
                                            span { key: "{i}", class: if s.required { "badge accent" } else { "badge" }, style: "margin: 2px",
                                                if s.negate { "NOT " }
                                                "{s.implementation.trim_end_matches(\"Specification\")}: {s.name}"
                                            }
                                        }
                                    }
                                    td { class: "shrink",
                                        div { class: "row tight",
                                            button { class: "btn sm", onclick: { let f = f.clone(); move |_| { editor_error.set(None); editing.set(Some((f.id, f.name.clone(), f.include_when_renaming, serde_json::to_string_pretty(&f.specifications).unwrap_or_default()))) } }, "Edit" }
                                            button { class: "btn sm danger", onclick: { let id = f.id; let d = delete.clone(); move |_| d(id) }, "Delete" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
        if let Some((id, name, renaming, json)) = editing() {
            Modal {
                title: if id == 0 { "New custom format".to_string() } else { format!("Edit “{name}”") },
                wide: true,
                on_close: move |_| editing.set(None),
                footer: rsx! {
                    button { class: "btn", onclick: move |_| editing.set(None), "Cancel" }
                    button { class: "btn primary", onclick: save, "Save" }
                },
                if let Some(msg) = editor_error() { div { class: "alert danger", "{msg}" } }
                Field { label: "Name", input { class: "input", value: "{name}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.1 = e.value(); } } } }
                label { class: "check small", style: "margin-bottom: var(--sp-3)",
                    input { r#type: "checkbox", checked: renaming, onchange: move |e: FormEvent| { if let Some(x) = editing.write().as_mut() { x.2 = e.checked(); } } }
                    "Include in file names ({{Custom Formats}} token)"
                }
                Field { label: "Specifications (JSON)",
                    textarea { class: "input", rows: "14", spellcheck: false, value: "{json}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.3 = e.value(); } } }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------------------------

#[component]
pub fn SettingsIndexers() -> Element {
    rsx! {
        ProvidersPage { kind: "indexers", title: "Indexers", note: "Torznab and Newznab indexers. Use Test in the editor to check one; each search uses the indexers with that kind of search enabled: RSS, automatic or interactive." }
        IndexerOptions {}
    }
}

/// Sonarr's indexer options; the decision engine applies them to search results.
#[component]
fn IndexerOptions() -> Element {
    let app = use_app();
    let client = use_client();
    let mut opts = use_signal(|| None::<IndexerOptionsSettings>);
    let c1 = client.clone();
    use_future(move || {
        let c = c1.clone();
        async move {
            match c.indexer_options().await {
                Ok(o) => opts.set(Some(o)),
                Err(e) => app.error(&e),
            }
        }
    });
    let c2 = client.clone();
    let save = move |_| {
        let Some(o) = opts() else { return };
        let c = c2.clone();
        spawn(async move {
            match c.put_indexer_options(&o).await {
                Ok(_) => app.success("Indexer options saved."),
                Err(e) => app.error(&e),
            }
        });
    };
    let number = |v: String| v.trim().parse::<i32>().unwrap_or(0);
    rsx! {
        div { class: "card",
            h3 { "Options" }
            match opts() {
                None => rsx! { Loading {} },
                Some(o) => rsx! {
                    div { class: "form-grid",
                        Field { label: "Minimum age (minutes)", help: "Usenet only: newer releases are rejected, giving them time to propagate. 0 turns this off.".to_string(),
                            input { class: "input", r#type: "number", min: "0", "aria-label": "Minimum age", value: "{o.minimum_age}", oninput: move |e| { if let Some(x) = opts.write().as_mut() { x.minimum_age = number(e.value()); } } }
                        }
                        Field { label: "Retention (days)", help: "Usenet only: older releases are rejected. 0 means unlimited.".to_string(),
                            input { class: "input", r#type: "number", min: "0", "aria-label": "Retention", value: "{o.retention}", oninput: move |e| { if let Some(x) = opts.write().as_mut() { x.retention = number(e.value()); } } }
                        }
                        Field { label: "Maximum size (MB)", help: "Larger releases are rejected. 0 means unlimited.".to_string(),
                            input { class: "input", r#type: "number", min: "0", "aria-label": "Maximum size", value: "{o.maximum_size}", oninput: move |e| { if let Some(x) = opts.write().as_mut() { x.maximum_size = number(e.value()); } } }
                        }
                        Field { label: "RSS sync interval (minutes)", help: "0 (off) or 10 to 120. RSS sync also runs from System › Tasks.".to_string(),
                            input { class: "input", r#type: "number", min: "0", max: "120", "aria-label": "RSS sync interval", value: "{o.rss_sync_interval}", oninput: move |e| { if let Some(x) = opts.write().as_mut() { x.rss_sync_interval = number(e.value()); } } }
                        }
                    }
                    Switch { checked: o.enable_fake_release_protection, label: "Fake release protection: inspect torrents before grabbing them and reject executables, password-bait archives and contents with no video",
                        onchange: move |v| { if let Some(x) = opts.write().as_mut() { x.enable_fake_release_protection = v; } } }
                    Switch { checked: o.enable_multi_season_releases, label: "Multi-season releases: accept packs of several seasons (a season search takes the pack for its season)",
                        onchange: move |v| { if let Some(x) = opts.write().as_mut() { x.enable_multi_season_releases = v; } } }
                    div { class: "row end", button { class: "btn primary", onclick: save, "Save options" } }
                },
            }
        }
    }
}

#[component]
pub fn SettingsDownloadClients() -> Element {
    rsx! {
        ProvidersPage { kind: "downloadclients", title: "Download clients", note: "qBittorrent, Deluge, Transmission, SABnzbd and NZBGet work: use Test in the editor; grabs, the queue and imports use them. Each client's \"Remove completed/failed downloads\" options remove downloads (with their files) once the client is done with them." }
        DownloadClientOptions {}
    }
}

/// Sonarr's completed and failed download handling options.
#[component]
fn DownloadClientOptions() -> Element {
    let app = use_app();
    let client = use_client();
    let mut opts = use_signal(|| None::<DownloadClientOptionsSettings>);
    let c1 = client.clone();
    use_future(move || {
        let c = c1.clone();
        async move {
            match c.download_client_options().await {
                Ok(o) => opts.set(Some(o)),
                Err(e) => app.error(&e),
            }
        }
    });
    let c2 = client.clone();
    let save = move |_| {
        let Some(o) = opts() else { return };
        let c = c2.clone();
        spawn(async move {
            match c.put_download_client_options(&o).await {
                Ok(_) => app.success("Download client options saved."),
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        div { class: "card",
            h3 { "Completed and failed download handling" }
            match opts() {
                None => rsx! { Loading {} },
                Some(o) => rsx! {
                    div { class: "stack",
                        Switch { checked: o.enable_completed_download_handling, label: "Import completed downloads",
                            onchange: move |v| { if let Some(x) = opts.write().as_mut() { x.enable_completed_download_handling = v; } } }
                        Switch { checked: o.auto_redownload_failed, label: "Search again when a download fails",
                            onchange: move |v| { if let Some(x) = opts.write().as_mut() { x.auto_redownload_failed = v; } } }
                        Field { label: "Stalled torrent timeout (minutes)", help: "A downloading torrent with no progress for this long is failed, blocklisted and searched again. 0 turns this off; progress is tracked from Kitsunarr's start.".to_string(),
                            input { class: "input", r#type: "number", min: "0", max: "43200", "aria-label": "Stalled torrent timeout", value: "{o.stalled_torrent_timeout}",
                                oninput: move |e| { if let Some(x) = opts.write().as_mut() { x.stalled_torrent_timeout = e.value().trim().parse().unwrap_or(0); } } }
                        }
                        Switch { checked: o.auto_redownload_failed_from_interactive_search, disabled: !o.auto_redownload_failed,
                            label: "Also for releases grabbed from interactive search",
                            onchange: move |v| { if let Some(x) = opts.write().as_mut() { x.auto_redownload_failed_from_interactive_search = v; } } }
                    }
                    div { class: "row end", button { class: "btn primary", onclick: save, "Save options" } }
                },
            }
        }
    }
}

#[component]
pub fn SettingsNotifications() -> Element {
    rsx! { ProvidersPage { kind: "notifications", title: "Notifications", note: "Webhook, Notifiarr, Discord, Slack, Telegram, Gotify, ntfy, Pushover, Pushbullet, Join, Prowl, Simplepush, Signal, Apprise, Emby/Jellyfin, Plex and Kodi are sent: grabs, imports and upgrades, series added or deleted, health issues. Use Test in the editor." } }
}

#[component]
pub fn SettingsImportLists() -> Element {
    rsx! { ProvidersPage { kind: "importlists", title: "Import lists", note: "List synchronisation is not implemented yet; imported list settings are kept." } }
}

#[component]
fn ProvidersPage(kind: &'static str, title: &'static str, note: &'static str) -> Element {
    let app = use_app();
    let client = use_client();
    let mut editing = use_signal(|| None::<ProviderItem>);
    let mut confirm_delete = use_signal(|| None::<ProviderItem>);
    let c0 = client.clone();
    let mut items = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c0.clone();
        async move { c.providers(kind).await }
    });
    let mut adding = use_signal(|| false);
    let c3 = client.clone();
    let templates = use_resource(move || {
        let c = c3.clone();
        async move { c.provider_schema(kind).await }
    });
    let c1 = client.clone();
    let save = move |item: ProviderItem| {
        let c = c1.clone();
        spawn(async move {
            let saved = if item.id == 0 {
                c.create_provider(kind, &item).await
            } else {
                c.update_provider(kind, &item).await
            };
            match saved {
                Ok(_) => {
                    editing.set(None);
                    items.restart();
                    app.success("Saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c2 = client.clone();
    let delete = move |_| {
        let Some(item) = confirm_delete() else { return };
        confirm_delete.set(None);
        let c = c2.clone();
        spawn(async move {
            match c.delete_provider(kind, item.id).await {
                Ok(()) => items.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    let can_add = matches!(&*templates.read(), Some(Ok(t)) if !t.is_empty());
    rsx! {
        div { class: "page-head",
            h1 { "{title}" }
            if can_add {
                button { class: "btn primary", onclick: move |_| adding.set(true), "Add" }
            }
        }
        p { class: "muted small", "{note}" }
        match &*items.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: format!("No {}", title.to_lowercase()), p { if can_add { "Add one, or migrate them from Sonarr/Tanukarr." } else { "They are created by migrating from Sonarr/Tanukarr." } } } },
            Some(Ok(list)) => rsx! {
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Name" } th { "Implementation" } th { "Status" } th { class: "num", "Priority" } th { "" } } }
                        tbody {
                            for p in list.iter() {
                                tr { key: "{p.id}",
                                    td { strong { "{p.name}" } }
                                    td {
                                        "{p.implementation}"
                                        if !p.supported { " " Badge { text: "stored only", tone: "warning" } }
                                    }
                                    td { if p.enabled { Badge { text: "enabled", tone: "success" } } else { Badge { text: "disabled" } } }
                                    td { class: "num", "{p.priority}" }
                                    td { class: "shrink",
                                        div { class: "row tight",
                                            button { class: "btn sm", onclick: { let p = p.clone(); move |_| editing.set(Some(p.clone())) }, "Edit" }
                                            button { class: "btn sm danger", onclick: { let p = p.clone(); move |_| confirm_delete.set(Some(p.clone())) }, {crate::i18n::t().delete} }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
        if adding() {
            Modal {
                title: format!("Add — {}", title.to_lowercase()),
                on_close: move |_| adding.set(false),
                footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| adding.set(false), "Cancel" } },
                match &*templates.read() {
                    Some(Ok(list)) => rsx! {
                        div { class: "stack",
                            for t in list.iter() {
                                button { key: "{t.implementation}", class: "btn", style: "justify-content: flex-start",
                                    onclick: { let t = t.clone(); move |_| { adding.set(false); editing.set(Some(t.clone())); } },
                                    strong { "{t.name}" }
                                    if let Some(p) = t.protocol { span { class: "muted small", " · {p:?}" } }
                                }
                            }
                        }
                    },
                    _ => rsx! { Loading {} },
                }
            }
        }
        if let Some(item) = editing() {
            ProviderEditor { item, on_save: save, on_close: move |_| editing.set(None) }
        }
        if let Some(item) = confirm_delete() {
            Confirm {
                title: "Delete",
                message: format!("Delete “{}”?", item.name),
                confirm_label: "Delete",
                danger: true,
                on_confirm: delete,
                on_cancel: move |_| confirm_delete.set(None),
            }
        }
    }
}

fn is_secret(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "password",
        "apikey",
        "token",
        "secret",
        "passkey",
        "cookie",
        "webhookurl",
    ]
    .iter()
    .any(|s| n.contains(s))
}

#[component]
fn ProviderEditor(
    item: ProviderItem,
    on_save: EventHandler<ProviderItem>,
    on_close: EventHandler<()>,
) -> Element {
    let mut draft = use_signal(|| item.clone());
    let mut testing = use_signal(|| false);
    // (ok, message, warnings) from an indexer or download client test.
    let mut test_result = use_signal(|| None::<Result<(bool, String, Vec<String>), String>>);
    let client = crate::state::use_client();
    let testable = match item.kind {
        ProviderKind::Indexer => matches!(item.implementation.as_str(), "Newznab" | "Torznab"),
        ProviderKind::DownloadClient => {
            matches!(
                item.implementation.as_str(),
                "QBittorrent" | "Deluge" | "Transmission" | "Sabnzbd" | "Nzbget"
            )
        }
        ProviderKind::Notification => kitsunarr_domain::api::settings::NOTIFICATION_IMPLEMENTATIONS
            .iter()
            .any(|i| i.eq_ignore_ascii_case(&item.implementation)),
        _ => false,
    };
    let run_test = move |_| {
        let c = client.clone();
        let d = draft();
        testing.set(true);
        test_result.set(None);
        spawn(async move {
            let req = kitsunarr_domain::api::search::IndexerTestRequest {
                id: (d.id > 0).then_some(d.id),
                implementation: d.implementation.clone(),
                settings: d.settings.clone(),
            };
            let outcome = if d.kind == ProviderKind::DownloadClient {
                c.test_download_client(&req)
                    .await
                    .map(|r| (r.ok, r.message, r.warnings))
            } else if d.kind == ProviderKind::Notification {
                c.test_notification(&req)
                    .await
                    .map(|r| (r.ok, r.message, r.warnings))
            } else {
                c.test_indexer(&req)
                    .await
                    .map(|r| (r.ok, r.message, r.warnings))
            };
            test_result.set(Some(outcome.map_err(|e| e.to_string())));
            testing.set(false);
        });
    };
    let d = draft();
    let mut keys: Vec<String> = d.settings.keys().cloned().collect();
    keys.sort();
    let mut option_keys: Vec<String> = d
        .options
        .iter()
        .filter(|(_, v)| v.is_boolean())
        .map(|(k, _)| k.clone())
        .collect();
    option_keys.sort();
    rsx! {
        Modal {
            title: if item.id == 0 { format!("Add {}", item.implementation) } else { format!("{} ({})", item.name, item.implementation) },
            wide: true,
            on_close: move |_| on_close.call(()),
            footer: rsx! {
                if testable {
                    button { class: "btn", disabled: testing(), onclick: run_test,
                        if testing() { "Testing…" } else { "Test" }
                    }
                }
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", onclick: move |_| on_save.call(draft()), "Save" }
            },
            if let Some(result) = test_result() {
                match result {
                    Ok((true, message, warnings)) => rsx! {
                        div { class: "alert success", role: "status",
                            div { class: "grow",
                                "{message}"
                                for w in warnings.iter() { div { key: "{w}", class: "small", "⚠ {w}" } }
                            }
                        }
                    },
                    Ok((false, message, _)) => rsx! { div { class: "alert danger", role: "alert", div { class: "grow", "Test failed: {message}" } } },
                    Err(e) => rsx! { div { class: "alert danger", role: "alert", div { class: "grow", "{e}" } } },
                }
            }
            div { class: "form-grid",
                Field { label: "Name", input { class: "input", value: "{d.name}", oninput: move |e| draft.write().name = e.value() } }
                Field { label: "Priority", input { class: "input", r#type: "number", value: "{d.priority}", oninput: move |e| draft.write().priority = e.value().parse().unwrap_or(1) } }
            }
            // Indexers are enabled through their RSS/search switches; other providers have their own.
            if item.kind != ProviderKind::Indexer {
                Switch { checked: d.enabled, label: "Enabled", onchange: move |v| draft.write().enabled = v }
            }
            if !option_keys.is_empty() {
                div { class: "form-grid", style: "margin-bottom: var(--sp-4)",
                    for k in option_keys {
                        label { key: "{k}", class: "check small",
                            input {
                                r#type: "checkbox",
                                checked: d.options.get(&k).and_then(Value::as_bool).unwrap_or(false),
                                onchange: { let k = k.clone(); move |e: FormEvent| { draft.write().options.insert(k.clone(), Value::Bool(e.checked())); } },
                            }
                            "{field_label(&k)}"
                        }
                    }
                }
            }
            h3 { "Settings" }
            div { class: "form-grid",
                for k in keys {
                    ProviderField { key: "{k}", name: k.clone(), value: d.settings.get(&k).cloned().unwrap_or(Value::Null),
                        on_change: move |(name, v): (String, Value)| { draft.write().settings.insert(name, v); } }
                }
            }
            if item.id != 0 {
                p { class: "help small faint", "Secrets show as {REDACTED}; leave them unchanged to keep the stored value." }
            }
        }
    }
}

/// A readable label for a provider setting or option stored under Sonarr's field name.
fn field_label(key: &str) -> String {
    let known = [
        ("baseUrl", "URL"),
        ("apiPath", "API path"),
        ("apiKey", "API key"),
        ("urlBase", "URL base"),
        ("useSsl", "Use SSL"),
        ("tvCategory", "Category"),
        ("tvImportedCategory", "Post-import category"),
        ("tvDirectory", "Directory"),
        ("recentTvPriority", "Recent priority"),
        ("olderTvPriority", "Older priority"),
        ("animeCategories", "Anime categories"),
        ("animeStandardFormatSearch", "Anime standard format search"),
        ("enableRss", "Enable RSS"),
        ("enableAutomaticSearch", "Enable automatic search"),
        ("enableInteractiveSearch", "Enable interactive search"),
        ("removeCompletedDownloads", "Remove completed downloads"),
        ("removeFailedDownloads", "Remove failed downloads"),
        ("firstAndLast", "First and last pieces first"),
    ];
    if let Some((_, l)) = known.iter().find(|(k, _)| *k == key) {
        return (*l).to_string();
    }
    let mut out = String::new();
    for (i, c) in key.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[component]
fn ProviderField(name: String, value: Value, on_change: EventHandler<(String, Value)>) -> Element {
    let n = name.clone();
    let label = field_label(&name);
    match value {
        Value::Bool(b) => rsx! {
            label { class: "check small", style: "margin-bottom: var(--sp-4)",
                input { r#type: "checkbox", checked: b, onchange: move |e: FormEvent| on_change.call((n.clone(), Value::Bool(e.checked()))) }
                "{label}"
            }
        },
        Value::Number(num) => rsx! {
            Field { label: label.clone(),
                input { class: "input", r#type: "number", value: "{num}", oninput: move |e: FormEvent| {
                    let v = e.value();
                    let parsed = v.parse::<i64>().map(Value::from).or_else(|_| v.parse::<f64>().map(Value::from)).unwrap_or(Value::Null);
                    on_change.call((n.clone(), parsed));
                } }
            }
        },
        Value::String(s) => rsx! {
            Field { label: label.clone(),
                input { class: "input", r#type: if is_secret(&name) { "password" } else { "text" }, autocomplete: "off", value: "{s}", oninput: move |e: FormEvent| on_change.call((n.clone(), Value::String(e.value()))) }
            }
        },
        Value::Null => rsx! {
            Field { label: label.clone(),
                input { class: "input", value: "", placeholder: "(empty)", oninput: move |e: FormEvent| on_change.call((n.clone(), Value::String(e.value()))) }
            }
        },
        other => rsx! {
            Field { label: label.clone(), help: "JSON".to_string(),
                input { class: "input mono", value: "{other}", oninput: move |e: FormEvent| {
                    if let Ok(v) = serde_json::from_str::<Value>(&e.value()) { on_change.call((n.clone(), v)); }
                } }
            }
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Metadata and tags
// ---------------------------------------------------------------------------------------------

#[component]
pub fn SettingsMetadata() -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut catalogues = use_resource(move || {
        let c = c0.clone();
        async move { c.metadata_providers().await }
    });
    let mut keys = use_signal(|| None::<MetadataSourcesSettings>);
    let c1 = client.clone();
    use_future(move || {
        let c = c1.clone();
        async move {
            match c.metadata_sources().await {
                Ok(k) => keys.set(Some(k)),
                Err(e) => app.error(&e),
            }
        }
    });
    let c2 = client.clone();
    let save = move |_| {
        let Some(k) = keys() else { return };
        let c = c2.clone();
        spawn(async move {
            match c.put_metadata_sources(&k).await {
                Ok(saved) => {
                    keys.set(Some(saved));
                    catalogues.restart();
                    app.success("Metadata keys saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let text = |v: &Option<String>| v.clone().unwrap_or_default();
    let value = |v: String| Some(v.trim().to_string()).filter(|v| !v.is_empty());
    rsx! {
        div { class: "page-head", h1 { "Metadata sources" } }
        p { class: "muted small", "Each title belongs to one catalogue, which its refreshes read; every identifier it carries (TheTVDB, TMDB, IMDb, TVmaze, AniList, MyAnimeList) is kept. Titles migrated from Sonarr belong to TheTVDB." }
        match &*catalogues.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) => rsx! {
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Catalogue" } th { "Covers" } th { class: "hide-mobile", "Credit" } th { "Status" } } }
                        tbody {
                            for c in list.iter() {
                                tr { key: "{c.provider}",
                                    td { a { href: "{c.url}", target: "_blank", rel: "noopener", strong { "{c.name}" } } }
                                    td { class: "small", "{c.description}" }
                                    td { class: "hide-mobile small muted", "{c.attribution}" }
                                    td {
                                        if c.available { Badge { text: "available", tone: "success" } }
                                        else { Badge { text: "API key needed", tone: "warning" } }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
        div { class: "card", style: "margin-top: var(--sp-4)",
            h3 { "API keys" }
            match keys() {
                None => rsx! { Loading {} },
                Some(k) => rsx! {
                    div { class: "form-grid",
                        Field { label: "TMDB API key (v3)", help: "Create one at themoviedb.org › Settings › API. Used for TMDB TV series and films.".to_string(),
                            input { class: "input", r#type: "password", autocomplete: "off", "aria-label": "TMDB API key", value: "{text(&k.tmdb_api_key)}",
                                oninput: move |e| { if let Some(x) = keys.write().as_mut() { x.tmdb_api_key = value(e.value()); } } }
                        }
                        Field { label: "OMDb API key", help: "Request one at omdbapi.com. Used for films by IMDb id.".to_string(),
                            input { class: "input", r#type: "password", autocomplete: "off", "aria-label": "OMDb API key", value: "{text(&k.omdb_api_key)}",
                                oninput: move |e| { if let Some(x) = keys.write().as_mut() { x.omdb_api_key = value(e.value()); } } }
                        }
                    }
                    p { class: "help small faint", "Saved keys show as {REDACTED}; leave them unchanged to keep them, or clear the field to remove one." }
                    div { class: "row end", button { class: "btn primary", onclick: save, "Save keys" } }
                },
            }
        }
    }
}

#[component]
pub fn SettingsTags() -> Element {
    let app = use_app();
    let client = use_client();
    let mut label = use_signal(String::new);
    let c0 = client.clone();
    let mut tags = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c0.clone();
        async move { c.tags().await }
    });
    let c1 = client.clone();
    let create = move |e: FormEvent| {
        e.prevent_default();
        let c = c1.clone();
        spawn(async move {
            match c.create_tag(&label()).await {
                Ok(_) => {
                    label.set(String::new());
                    tags.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c2 = client.clone();
    let delete = move |id: i64| {
        let c = c2.clone();
        spawn(async move {
            match c.delete_tag(id).await {
                Ok(()) => tags.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        div { class: "page-head", h1 { "Tags" } }
        form { class: "row", style: "margin-bottom: var(--sp-4)", onsubmit: create,
            input { class: "input", placeholder: "new-tag", "aria-label": "New tag", value: "{label}", oninput: move |e| label.set(e.value()) }
            button { class: "btn primary", r#type: "submit", disabled: label().trim().is_empty(), "Add tag" }
        }
        match &*tags.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "No tags" } },
            Some(Ok(list)) => rsx! {
                div { class: "row",
                    for t in list.iter() {
                        span { key: "{t.id}", class: "badge", style: "min-height: 1.875rem; padding: 0 4px 0 12px",
                            "{t.label}"
                            span { class: "faint", " · {t.usage_count}" }
                            button { class: "btn ghost sm", "aria-label": "Delete tag {t.label}", onclick: { let id = t.id; let d = delete.clone(); move |_| d(id) }, "✕" }
                        }
                    }
                }
            },
        }
    }
}
