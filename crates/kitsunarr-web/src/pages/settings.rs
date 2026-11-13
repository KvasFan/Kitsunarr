//! Settings pages.

use dioxus::prelude::*;
use kitsunarr_domain::api::auth::ChangePasswordRequest;
use kitsunarr_domain::api::settings::{
    CustomFormat, DownloadClientOptionsSettings, GeneralSettings, IndexerOptionsSettings,
    MediaManagementSettings, MetadataSourcesSettings, NamingSettings, ProperDownloadType,
    ProviderItem, ProviderKind, ProxySettings, ProxyType, QualityProfile, REDACTED,
    RootFolderCreate,
};
use kitsunarr_domain::{MediaKind, Quality};
use serde_json::Value;

use crate::components::common::{Badge, Confirm, Empty, ErrorBox, Field, Loading, Modal, Switch};
use crate::state::{ToastLevel, use_app, use_client};
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
                        Field { label: "Certificate validation", help: "When HTTPS certificates of indexers, download clients and services are checked (Sonarr's setting). Self-signed certificates on your network need it relaxed.".to_string(),
                            select { class: "select", "aria-label": "Certificate validation",
                                onchange: move |e: FormEvent| { if let Some(f) = form.write().as_mut() { f.certificate_validation = kitsunarr_domain::api::settings::CertificateValidation::parse(&e.value()).unwrap_or_default(); } },
                                for (v, label) in [("enabled", "Enabled"), ("disabledForLocalAddresses", "Disabled for local addresses"), ("disabled", "Disabled")] {
                                    option { key: "{v}", value: "{v}", selected: kitsunarr_domain::api::settings::CertificateValidation::parse(v) == Some(g.certificate_validation), "{label}" }
                                }
                            }
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
        ProxyCard {}
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

/// Sonarr's Settings › General › Proxy.
#[component]
fn ProxyCard() -> Element {
    let app = use_app();
    let client = use_client();
    let mut form = use_signal(|| None::<ProxySettings>);
    let c0 = client.clone();
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.proxy().await {
                Ok(p) => form.set(Some(p)),
                Err(e) => app.error(&e),
            }
        }
    });
    let save = move |_| {
        let Some(p) = form() else { return };
        let c = client.clone();
        spawn(async move {
            match c.put_proxy(&p).await {
                Ok(saved) => {
                    form.set(Some(saved));
                    app.success("Proxy saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let mut edit = move |f: &dyn Fn(&mut ProxySettings)| {
        if let Some(p) = form.write().as_mut() {
            f(p);
        }
    };
    rsx! {
        div { class: "card",
            h3 { "Proxy" }
            p { class: "muted small", "Send requests to indexers, catalogues, lists, download clients and connections through a proxy." }
            match form() {
                None => rsx! { Loading {} },
                Some(p) => rsx! {
                    Switch { checked: p.enabled, label: "Use a proxy", onchange: move |v| edit(&|x| x.enabled = v) }
                    if p.enabled {
                        div { class: "form-grid",
                            Field { label: "Proxy type",
                                select { class: "select", "aria-label": "Proxy type",
                                    value: match p.proxy_type { ProxyType::Http => "http", ProxyType::Socks4 => "socks4", ProxyType::Socks5 => "socks5" },
                                    onchange: move |e: FormEvent| { let t = ProxyType::parse(&e.value()).unwrap_or_default(); edit(&|x| x.proxy_type = t) },
                                    option { value: "http", "HTTP(S)" }
                                    option { value: "socks4", "SOCKS4" }
                                    option { value: "socks5", "SOCKS5" }
                                }
                            }
                            Field { label: "Hostname",
                                input { class: "input", "aria-label": "Proxy hostname", value: "{p.hostname}", oninput: move |e: FormEvent| { let v = e.value(); edit(&|x| x.hostname = v.clone()) } }
                            }
                            Field { label: "Port",
                                input { class: "input", r#type: "number", min: "1", max: "65535", "aria-label": "Proxy port", value: "{p.port}", oninput: move |e: FormEvent| { let v = e.value().parse().unwrap_or(0); edit(&|x| x.port = v) } }
                            }
                            Field { label: "Username", help: "Only when the proxy needs one.".to_string(),
                                input { class: "input", autocomplete: "off", value: "{p.username}", oninput: move |e: FormEvent| { let v = e.value(); edit(&|x| x.username = v.clone()) } }
                            }
                            Field { label: "Password",
                                input { class: "input", r#type: "password", autocomplete: "new-password", value: "{p.password}", oninput: move |e: FormEvent| { let v = e.value(); edit(&|x| x.password = v.clone()) } }
                            }
                            Field { label: "Ignored addresses", help: "Comma-separated; * is a wildcard (*.lan), and IP ranges like 192.168.1.0/24 work.".to_string(),
                                input { class: "input", "aria-label": "Ignored addresses", value: "{p.bypass_filter}", oninput: move |e: FormEvent| { let v = e.value(); edit(&|x| x.bypass_filter = v.clone()) } }
                            }
                        }
                        Switch { checked: p.bypass_local_addresses, label: "Bypass the proxy for local addresses", onchange: move |v| edit(&|x| x.bypass_local_addresses = v) }
                    }
                    div { class: "row end", button { class: "btn primary", onclick: save, {crate::i18n::t().save} } }
                },
            }
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
                        NamingInput { label: "Group folder format", value: n.anime_group_folder_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.anime_group_folder_format = v; } } }
                        NamingInput { label: "Group season folder format", value: n.anime_group_season_folder_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.anime_group_season_folder_format = v; } } }
                        NamingInput { label: "Group story-arc folder format", value: n.anime_group_arc_folder_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.anime_group_arc_folder_format = v; } } }
                        NamingInput { label: "Group film folder format", value: n.anime_group_movie_folder_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.anime_group_movie_folder_format = v; } } }
                        NamingInput { label: "Collection film folder format", value: n.anime_group_movie_saga_folder_format.clone(), on_change: move |v| { if let Some(x) = naming.write().as_mut() { x.anime_group_movie_saga_folder_format = v; } } }
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
                        Switch { checked: m.enable_media_info, label: "Analyze video files (resolution, runtime and codecs, with ffprobe)", onchange: move |v| { if let Some(x) = mm.write().as_mut() { x.enable_media_info = v; } } }
                        Switch { checked: m.skip_free_space_check, label: "Skip the free-space check", onchange: move |v| { if let Some(x) = mm.write().as_mut() { x.skip_free_space_check = v; } } }
                        Switch { checked: m.import_extra_files, label: "Import extra files (subtitles…)", onchange: move |v| { if let Some(x) = mm.write().as_mut() { x.import_extra_files = v; } } }
                    }
                    div { class: "form-grid",
                        Field { label: "Recycle bin", help: "Absolute path; empty to delete permanently.".to_string(),
                            input { class: "input mono", value: "{m.recycle_bin.clone().unwrap_or_default()}", oninput: move |e| { if let Some(x) = mm.write().as_mut() { x.recycle_bin = Some(e.value()).filter(|v| !v.is_empty()); } } }
                        }
                        Field { label: "Recycle bin cleanup (days)", help: "Files older than this are deleted from the recycle bin daily; 0 keeps them.".to_string(),
                            input { class: "input", r#type: "number", min: "0", value: "{m.recycle_bin_cleanup_days}", oninput: move |e| { if let Some(x) = mm.write().as_mut() { x.recycle_bin_cleanup_days = e.value().parse().unwrap_or(0); } } }
                        }
                        Field { label: "Minimum free space (MiB)",
                            input { class: "input", r#type: "number", min: "0", value: "{m.minimum_free_space_mb}", oninput: move |e| { if let Some(x) = mm.write().as_mut() { x.minimum_free_space_mb = e.value().parse().unwrap_or(0); } } }
                        }
                        Field { label: "Extra file extensions",
                            input { class: "input", value: "{m.extra_file_extensions}", oninput: move |e| { if let Some(x) = mm.write().as_mut() { x.extra_file_extensions = e.value(); } } }
                        }
                        Field { label: "File date", help: "The date imported files get: the episode's air date (local or UTC), or on an upgrade the replaced file's date.".to_string(),
                            select { class: "select", "aria-label": "File date",
                                onchange: move |e: FormEvent| {
                                    if let Some(x) = mm.write().as_mut() {
                                        x.file_date = kitsunarr_domain::api::settings::FileDate::from_sonarr(&e.value()).unwrap_or_default();
                                    }
                                },
                                {
                                    use kitsunarr_domain::api::settings::FileDate as F;
                                    let cur = m.file_date;
                                    rsx! {
                                        option { value: "none", selected: cur == F::None, "Unchanged" }
                                        option { value: "localAirDate", selected: cur == F::LocalAirDate, "Local air date" }
                                        option { value: "utcAirDate", selected: cur == F::UtcAirDate, "UTC air date" }
                                        option { value: "preserveOriginal", selected: cur == F::PreserveOriginal, "Keep the replaced file's date (upgrades)" }
                                    }
                                }
                            }
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
    // Custom quality presets, registered so their names show here.
    let c4 = client.clone();
    let mut definitions = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c4.clone();
        async move {
            let defs = c.quality_definitions().await;
            if let Ok(defs) = &defs {
                register_presets(defs);
            }
            defs
        }
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
        match &*definitions.read() {
            Some(Ok(defs)) => {
                // Remounted when the presets change, so the editor starts from the stored ones.
                let key = defs.iter().map(|d| format!("{}:{}:{}:{}:{}", d.quality_id, d.is_retired, d.codec, d.title_pattern, d.title)).collect::<Vec<_>>().join("|");
                rsx! { QualityPresets { key: "{key}", definitions: defs.clone(), on_change: move |_| { definitions.restart(); profiles.restart(); } } }
            }
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            None => rsx! {},
        }
        GlobalCodecFactors { on_change: move |_| profiles.restart() }
        DelayProfiles {}
        ReleaseProfiles {}
    }
}

/// Tanukarr's global codec factor editor: one factor per codec family across every profile
/// ("mixed" where profiles differ); only edited families are written.
#[component]
fn GlobalCodecFactors(on_change: EventHandler<()>) -> Element {
    use kitsunarr_domain::api::settings::OrderedF64;
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut summary = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c0.clone();
        async move { c.codec_factors().await }
    });
    // Edited families: Some(value) sets, None resets to the default.
    let mut edits = use_signal(std::collections::BTreeMap::<String, Option<OrderedF64>>::new);
    let save = move |_| {
        let c = client.clone();
        let e = edits();
        spawn(async move {
            match c.update_codec_factors(&e).await {
                Ok(_) => {
                    edits.write().clear();
                    summary.restart();
                    app.success("Codec factors saved in every profile.");
                    on_change.call(());
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let rows = match &*summary.read() {
        Some(Ok(r)) => r.clone(),
        Some(Err(e)) => return rsx! { ErrorBox { message: e.to_string() } },
        None => return rsx! {},
    };
    rsx! {
        details { class: "card", style: "margin-top: var(--sp-6)",
            summary { strong { "Codec size factors" } span { class: "muted small", " — all profiles at once" } }
            p { class: "help small faint", "A release's size is multiplied by its codec's factor before the size limits apply. \"Mixed\" means profiles use different factors; a value entered here is written into every profile." }
            div { class: "table-wrap",
                table { class: "table",
                    thead { tr { th { "Codec" } th { class: "num", "Factor" } th { class: "num", "Default" } th { span { class: "sr-only", "Actions" } } } }
                    tbody {
                        for r in rows.iter() {
                            {
                                let codec = r.codec.clone();
                                let codec2 = r.codec.clone();
                                let edited = edits().get(&r.codec).cloned();
                                let shown = match &edited {
                                    Some(Some(v)) => v.0.to_string(),
                                    Some(None) => r.default_factor.0.to_string(),
                                    None => r.factor.map(|f| f.0.to_string()).unwrap_or_default(),
                                };
                                rsx! {
                                    tr { key: "{r.codec}",
                                        td { "{r.label}" if r.is_mixed && edited.is_none() { span { class: "badge", " mixed" } } }
                                        td { class: "num",
                                            input { class: "input", style: "width: 7rem", r#type: "number", step: "0.01", min: "0.01", max: "10",
                                                "aria-label": "Factor for {r.label}", placeholder: if r.is_mixed { "mixed" } else { "" }, value: "{shown}",
                                                oninput: move |e| {
                                                    if let Ok(v) = e.value().parse::<f64>() {
                                                        edits.write().insert(codec.clone(), Some(OrderedF64(v)));
                                                    }
                                                } }
                                        }
                                        td { class: "num small muted", "{r.default_factor.0}" }
                                        td { class: "actions",
                                            button { class: "btn sm ghost", onclick: move |_| { edits.write().insert(codec2.clone(), None); }, "Default" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "row", button { class: "btn primary", disabled: edits().is_empty(), onclick: save, "Save factors" } }
        }
    }
}

/// Registers custom presets in the browser's quality catalogue (names in profiles).
fn register_presets(defs: &[kitsunarr_domain::api::settings::QualityDefinition]) {
    for d in defs
        .iter()
        .filter(|d| d.quality_id >= kitsunarr_domain::quality::CUSTOM_QUALITY_BASE)
    {
        Quality::register(
            d.quality_id,
            &d.title,
            d.source
                .unwrap_or(kitsunarr_domain::quality::QualitySource::Unknown),
            d.resolution.unwrap_or(0),
        );
    }
}

const PRESET_SOURCES: &[(kitsunarr_domain::quality::QualitySource, &str)] = {
    use kitsunarr_domain::quality::QualitySource as S;
    &[
        (S::Television, "Television"),
        (S::TelevisionRaw, "Television (raw)"),
        (S::Web, "Web"),
        (S::WebRip, "WebRip"),
        (S::Dvd, "DVD"),
        (S::Bluray, "Blu-ray"),
        (S::BlurayRaw, "Blu-ray (raw / remux)"),
        (S::Unknown, "Unknown"),
    ]
};

fn source_label(s: Option<kitsunarr_domain::quality::QualitySource>) -> &'static str {
    s.and_then(|s| {
        PRESET_SOURCES
            .iter()
            .find(|(x, _)| *x == s)
            .map(|(_, l)| *l)
    })
    .unwrap_or("—")
}

/// Tanukarr's quality presets: criteria on standard qualities, custom presets (create, retire,
/// restore).
#[component]
fn QualityPresets(
    definitions: Vec<kitsunarr_domain::api::settings::QualityDefinition>,
    on_change: EventHandler<()>,
) -> Element {
    use kitsunarr_domain::api::settings::CustomQualityCreate;
    let app = use_app();
    let client = use_client();
    let mut defs = use_signal(|| definitions.clone());
    let mut new = use_signal(|| CustomQualityCreate {
        source: Some(kitsunarr_domain::quality::QualitySource::Web),
        resolution: Some(1080),
        ..Default::default()
    });
    let c1 = client.clone();
    let save = move |_| {
        let c = c1.clone();
        let list = defs();
        spawn(async move {
            match c.update_quality_definitions(&list).await {
                Ok(_) => {
                    app.success("Quality presets saved.");
                    on_change.call(());
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c2 = client.clone();
    let create = move |_| {
        let c = c2.clone();
        let req = new();
        spawn(async move {
            match c.create_custom_quality(&req).await {
                Ok(d) => {
                    app.success(format!(
                        "{} added to every profile (not allowed yet).",
                        d.title
                    ));
                    new.write().title.clear();
                    on_change.call(());
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c3 = client.clone();
    let toggle_retired = move |(id, retired): (i32, bool)| {
        let c = c3.clone();
        spawn(async move {
            let r = if retired {
                c.restore_quality(id).await
            } else {
                c.retire_quality(id).await
            };
            match r {
                Ok(()) => on_change.call(()),
                Err(e) => app.error(&e),
            }
        });
    };
    let families = kitsunarr_domain::codec::FAMILIES;
    rsx! {
        details { class: "card", style: "margin-top: var(--sp-6)",
            summary { strong { "Quality presets" } span { class: "muted small", " — codec and title criteria, custom qualities (Tanukarr)" } }
            p { class: "help small faint", "A release takes the most specific preset allowed in its series' profile whose source, resolution, codec and title pattern it matches. A standard quality given a codec or pattern turns releases that fail it into Unknown. Custom presets are added to every profile as not allowed." }
            div { class: "table-wrap",
                table { class: "table",
                    thead { tr { th { "Quality" } th { "Source" } th { class: "num", "Resolution" } th { "Codec" } th { "Title pattern" } th { span { class: "sr-only", "Actions" } } } }
                    tbody {
                        for (i, d) in defs().iter().enumerate() {
                            {
                                let custom = d.quality_id >= kitsunarr_domain::quality::CUSTOM_QUALITY_BASE;
                                let q = Quality::by_id(d.quality_id);
                                let source = d.source.or(q.map(|q| q.source));
                                let resolution = d.resolution.or(q.map(|q| q.resolution)).unwrap_or(0);
                                let (id, retired) = (d.quality_id, d.is_retired);
                                let toggle = toggle_retired.clone();
                                rsx! {
                                    tr { key: "{d.quality_id}", class: if d.is_retired { "rejected" } else { "" },
                                        td {
                                            "{d.title}"
                                            if custom { span { class: "badge accent", " custom" } }
                                            if d.is_retired { span { class: "badge", " retired" } }
                                        }
                                        td { class: "small", {source_label(source)} }
                                        td { class: "num small", "{resolution}" }
                                        td {
                                            select { class: "input", "aria-label": "Codec of {d.title}", value: "{d.codec}",
                                                onchange: move |e| { defs.write()[i].codec = e.value(); },
                                                option { value: "", "Any" }
                                                for f in families.iter() { option { key: "{f}", value: "{f}", "{f}" } }
                                            }
                                        }
                                        td { input { class: "input mono", "aria-label": "Title pattern of {d.title}", value: "{d.title_pattern}",
                                            oninput: move |e| { defs.write()[i].title_pattern = e.value(); } } }
                                        td { class: "actions",
                                            if d.quality_id != 0 {
                                                button { class: "btn sm ghost", onclick: move |_| toggle((id, retired)),
                                                    if retired { "Restore" } else { "Retire" } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "row", button { class: "btn primary", onclick: save, "Save criteria" } }
            h3 { "New custom preset" }
            div { class: "form-grid",
                Field { label: "Title",
                    input { class: "input", "aria-label": "Preset title", value: "{new().title}", oninput: move |e| new.write().title = e.value() }
                }
                Field { label: "Source",
                    select { class: "input", value: "{PRESET_SOURCES.iter().position(|(s, _)| Some(*s) == new().source).unwrap_or(0)}",
                        onchange: move |e| { if let Ok(i) = e.value().parse::<usize>() { new.write().source = PRESET_SOURCES.get(i).map(|(s, _)| *s); } },
                        for (i, (_, l)) in PRESET_SOURCES.iter().enumerate() { option { key: "{i}", value: "{i}", "{l}" } }
                    }
                }
                Field { label: "Resolution",
                    select { class: "input", value: "{new().resolution.unwrap_or(0)}",
                        onchange: move |e| new.write().resolution = e.value().parse().ok(),
                        for r in [2160, 1080, 720, 576, 480, 0] { option { key: "{r}", value: "{r}", "{r}" } }
                    }
                }
                Field { label: "Codec",
                    select { class: "input", value: "{new().codec}", onchange: move |e| new.write().codec = e.value(),
                        option { value: "", "Any" }
                        for f in families.iter() { option { key: "{f}", value: "{f}", "{f}" } }
                    }
                }
                Field { label: "Title pattern", help: "A regular expression, case-insensitive; empty matches every title.".to_string(),
                    input { class: "input mono", value: "{new().title_pattern}", oninput: move |e| new.write().title_pattern = e.value() }
                }
                Field { label: "Copy sizes from",
                    select { class: "input", value: "{new().copy_from_id.map(|v| v.to_string()).unwrap_or_default()}",
                        onchange: move |e| new.write().copy_from_id = e.value().parse().ok(),
                        option { value: "", "Defaults" }
                        for d in definitions.iter() { option { key: "{d.quality_id}", value: "{d.quality_id}", "{d.title}" } }
                    }
                }
            }
            div { class: "row", button { class: "btn primary", disabled: new().title.trim().is_empty(), onclick: create, "Add preset" } }
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
            details { style: "margin-top: var(--sp-4)",
                summary { strong { "Size limits" } span { class: "muted small", " — MiB per minute; empty uses Settings › Quality" } }
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Quality" } th { "Minimum" } th { "Preferred" } th { "Maximum" } } }
                        tbody {
                            for qid in profile_quality_ids(&current) {
                                {
                                    let limit = current.size_limits.iter().find(|l| l.quality_id == qid).cloned();
                                    let name = Quality::by_id(qid).map_or_else(|| qid.to_string(), |q| q.name.to_string());
                                    let cell = move |field: u8| {
                                        let value = limit.as_ref().and_then(|l| match field { 0 => l.min_size, 1 => l.preferred_size, _ => l.max_size }).map(|v| v.0.to_string()).unwrap_or_default();
                                        // Tanukarr's sliders: each thumb edits its own limit; the GiB per hour
                                        // preview reads like the Quality settings.
                                        let per_hour = value.parse::<f64>().ok().map(|v| format!("{:.1} GiB/h", v * 60.0 / 1024.0));
                                        rsx! {
                                            td {
                                                div { class: "stack tight",
                                                    input { class: "input", r#type: "number", min: "0", max: "1000", step: "0.1", style: "width: 6rem",
                                                        "aria-label": "{field_name(field)} size of {qid}",
                                                        placeholder: "global", value: "{value}",
                                                        oninput: move |e: FormEvent| set_size_limit(&mut p.write(), qid, field, e.value().trim().parse().ok()) }
                                                    input { r#type: "range", min: "0", max: "1000", step: "0.5", style: "width: 8rem",
                                                        "aria-label": "{field_name(field)} size slider of {qid}",
                                                        value: "{value}",
                                                        oninput: move |e: FormEvent| set_size_limit(&mut p.write(), qid, field, e.value().trim().parse().ok()) }
                                                    if let Some(h) = per_hour { span { class: "xs muted", "{h}" } }
                                                }
                                            }
                                        }
                                    };
                                    rsx! { tr { key: "{qid}", td { "{name}" } {cell(0)} {cell(1)} {cell(2)} } }
                                }
                            }
                        }
                    }
                }
            }
            details { style: "margin-top: var(--sp-2)",
                summary { strong { "Codec size factors" } span { class: "muted small", " — a release's size is multiplied by its codec's factor before the size limits apply" } }
                div { class: "form-grid",
                    for family in kitsunarr_domain::codec::FAMILIES.iter().copied() {
                        Field { key: "{family}", label: kitsunarr_domain::codec::label(family).to_string(),
                            input { class: "input", r#type: "number", min: "0.01", max: "10", step: "0.01",
                                "aria-label": "{family} factor",
                                placeholder: "{kitsunarr_domain::codec::default_factor(family):.2}",
                                value: "{current.codec_multipliers.get(family).map(|v| v.0.to_string()).unwrap_or_default()}",
                                oninput: move |e: FormEvent| {
                                    let mut w = p.write();
                                    match e.value().trim().parse::<f64>() {
                                        Ok(v) => { w.codec_multipliers.insert(family.to_string(), kitsunarr_domain::api::settings::OrderedF64(v)); }
                                        Err(_) => { w.codec_multipliers.remove(family); }
                                    }
                                } }
                        }
                    }
                }
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

/// Every quality of a profile (group members included), best first.
fn profile_quality_ids(p: &QualityProfile) -> Vec<i32> {
    p.items
        .iter()
        .rev()
        .flat_map(|i| match (&i.quality_id, &i.group) {
            (Some(q), _) => vec![*q],
            (None, Some(g)) => g.quality_ids.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn field_name(field: u8) -> &'static str {
    match field {
        0 => "Minimum",
        1 => "Preferred",
        _ => "Maximum",
    }
}

/// Sets one size of a quality; a quality left with no sizes falls back to the global definition.
fn set_size_limit(p: &mut QualityProfile, quality_id: i32, field: u8, value: Option<f64>) {
    use kitsunarr_domain::api::settings::{OrderedF64, QualitySizeLimit};
    let idx = match p
        .size_limits
        .iter()
        .position(|l| l.quality_id == quality_id)
    {
        Some(i) => i,
        None => {
            p.size_limits.push(QualitySizeLimit {
                quality_id,
                min_size: None,
                preferred_size: None,
                max_size: None,
            });
            p.size_limits.len() - 1
        }
    };
    let l = &mut p.size_limits[idx];
    let v = value.map(OrderedF64);
    match field {
        0 => l.min_size = v,
        1 => l.preferred_size = v,
        _ => l.max_size = v,
    }
    if l.min_size.is_none() && l.preferred_size.is_none() && l.max_size.is_none() {
        p.size_limits.remove(idx);
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
        ProvidersPage { kind: "downloadclients", title: "Download clients", note: "Add a client from its template and use Test in the editor; grabs, the queue and imports use every enabled client. A blackhole writes grabs to a folder and imports what appears in its watch folder. Each client's \"Remove completed/failed downloads\" options remove downloads (with their files) once the client is done with them." }
        DownloadClientOptions {}
        RemotePathMappings {}
    }
}

fn split_terms(v: &str) -> Vec<String> {
    v.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn tag_ids(v: &str) -> Vec<i64> {
    v.split(',').filter_map(|t| t.trim().parse().ok()).collect()
}

/// Sonarr's delay profiles: how long to wait for a better release, per protocol.
#[component]
fn DelayProfiles() -> Element {
    use kitsunarr_domain::api::settings::DelayProfile;
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut list = use_resource(move || {
        let c = c0.clone();
        async move { c.delay_profiles().await }
    });
    let mut editing = use_signal(|| None::<DelayProfile>);
    let c1 = client.clone();
    let save = move |_| {
        let Some(d) = editing() else { return };
        let c = c1.clone();
        spawn(async move {
            match c.save_delay_profile(&d).await {
                Ok(_) => {
                    editing.set(None);
                    list.restart();
                    app.success("Delay profile saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let remove = move |id: i64| {
        let c = client.clone();
        spawn(async move {
            match c.delete_delay_profile(id).await {
                Ok(()) => list.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        div { class: "card",
            div { class: "row",
                h3 { class: "grow", "Delay profiles" }
                button { class: "btn", onclick: move |_| editing.set(Some(DelayProfile { id: 0, enable_usenet: true, enable_torrent: true, preferred_protocol: "usenet".into(), usenet_delay: 0, torrent_delay: 0, bypass_if_highest_quality: true, bypass_if_above_custom_format_score: false, minimum_custom_format_score: 0, sort_order: 0, tags: Vec::new() })), "Add" }
            }
            p { class: "muted small", "RSS sync waits this long after a release appears for a better one; delayed releases show in the queue. The first profile whose tags match the series applies; the last one (no tags) is the default." }
            match &*list.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(items)) => rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr { th { "Preferred" } th { class: "num", "Usenet delay" } th { class: "num", "Torrent delay" } th { "Tags" } th { "" } } }
                            tbody {
                                for d in items.iter() {
                                    {
                                        let (dd, id, remove) = (d.clone(), d.id, remove.clone());
                                        let default = d.tags.is_empty() && d.sort_order >= i64::from(i32::MAX);
                                        rsx! {
                                            tr { key: "{d.id}",
                                                td { "{d.preferred_protocol}" }
                                                td { class: "num", if d.enable_usenet { "{d.usenet_delay} min" } else { "off" } }
                                                td { class: "num", if d.enable_torrent { "{d.torrent_delay} min" } else { "off" } }
                                                td { class: "small", if default { "default" } else { {d.tags.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ")} } }
                                                td { class: "shrink", div { class: "row tight",
                                                    button { class: "btn sm", onclick: move |_| editing.set(Some(dd.clone())), "Edit" }
                                                    if !default { button { class: "btn sm danger", onclick: move |_| remove(id), {crate::i18n::t().delete} } }
                                                } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some(d) = editing() {
            Modal { title: "Delay profile".to_string(), on_close: move |_| editing.set(None),
                footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| editing.set(None), "Cancel" } button { class: "btn primary", onclick: save, "Save" } },
                div { class: "stack",
                    Switch { checked: d.enable_usenet, label: "Usenet", onchange: move |v| { if let Some(x) = editing.write().as_mut() { x.enable_usenet = v; } } }
                    Switch { checked: d.enable_torrent, label: "Torrent", onchange: move |v| { if let Some(x) = editing.write().as_mut() { x.enable_torrent = v; } } }
                }
                div { class: "form-grid",
                    Field { label: "Preferred protocol",
                        select { class: "select", "aria-label": "Preferred protocol", onchange: move |e| { if let Some(x) = editing.write().as_mut() { x.preferred_protocol = e.value(); } },
                            option { value: "usenet", selected: d.preferred_protocol == "usenet", "Usenet" }
                            option { value: "torrent", selected: d.preferred_protocol == "torrent", "Torrent" }
                        }
                    }
                    Field { label: "Usenet delay (minutes)", input { class: "input", r#type: "number", min: "0", value: "{d.usenet_delay}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.usenet_delay = e.value().parse().unwrap_or(0); } } } }
                    Field { label: "Torrent delay (minutes)", input { class: "input", r#type: "number", min: "0", value: "{d.torrent_delay}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.torrent_delay = e.value().parse().unwrap_or(0); } } } }
                    Field { label: "Tags (ids, comma separated)", input { class: "input", value: "{d.tags.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(\", \")}", onchange: move |e: FormEvent| { if let Some(x) = editing.write().as_mut() { x.tags = tag_ids(&e.value()); } } } }
                    Field { label: "Minimum custom format score", input { class: "input", r#type: "number", value: "{d.minimum_custom_format_score}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.minimum_custom_format_score = e.value().parse().unwrap_or(0); } } } }
                }
                div { class: "stack",
                    Switch { checked: d.bypass_if_highest_quality, label: "No delay for the profile's best quality in the preferred protocol", onchange: move |v| { if let Some(x) = editing.write().as_mut() { x.bypass_if_highest_quality = v; } } }
                    Switch { checked: d.bypass_if_above_custom_format_score, label: "No delay above the minimum custom format score", onchange: move |v| { if let Some(x) = editing.write().as_mut() { x.bypass_if_above_custom_format_score = v; } } }
                }
            }
        }
    }
}

/// Sonarr's release profiles: terms a release must or must not contain.
#[component]
fn ReleaseProfiles() -> Element {
    use kitsunarr_domain::api::settings::ReleaseProfile;
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut list = use_resource(move || {
        let c = c0.clone();
        async move { c.release_profiles().await }
    });
    let mut editing = use_signal(|| None::<ReleaseProfile>);
    let c1 = client.clone();
    let save = move |_| {
        let Some(r) = editing() else { return };
        let c = c1.clone();
        spawn(async move {
            match c.save_release_profile(&r).await {
                Ok(_) => {
                    editing.set(None);
                    list.restart();
                    app.success("Release profile saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let remove = move |id: i64| {
        let c = client.clone();
        spawn(async move {
            match c.delete_release_profile(id).await {
                Ok(()) => list.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        div { class: "card",
            div { class: "row",
                h3 { class: "grow", "Release profiles" }
                button { class: "btn", onclick: move |_| editing.set(Some(ReleaseProfile { id: 0, name: None, enabled: true, required: Vec::new(), ignored: Vec::new(), indexer_ids: Vec::new(), tags: Vec::new() })), "Add" }
            }
            match &*list.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(items)) if items.is_empty() => rsx! { p { class: "small faint", "No release profiles." } },
                Some(Ok(items)) => rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr { th { "Name" } th { "Must contain" } th { "Must not contain" } th { "" } } }
                            tbody {
                                for r in items.iter() {
                                    {
                                        let (rr, id, remove) = (r.clone(), r.id, remove.clone());
                                        rsx! {
                                            tr { key: "{r.id}",
                                                td { {r.name.clone().unwrap_or_default()} if !r.enabled { " " Badge { text: "disabled".to_string() } } }
                                                td { class: "small", {r.required.join(", ")} }
                                                td { class: "small", {r.ignored.join(", ")} }
                                                td { class: "shrink", div { class: "row tight",
                                                    button { class: "btn sm", onclick: move |_| editing.set(Some(rr.clone())), "Edit" }
                                                    button { class: "btn sm danger", onclick: move |_| remove(id), {crate::i18n::t().delete} }
                                                } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some(r) = editing() {
            Modal { title: "Release profile".to_string(), on_close: move |_| editing.set(None),
                footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| editing.set(None), "Cancel" } button { class: "btn primary", onclick: save, "Save" } },
                Switch { checked: r.enabled, label: "Enabled", onchange: move |v| { if let Some(x) = editing.write().as_mut() { x.enabled = v; } } }
                div { class: "form-grid",
                    Field { label: "Name", input { class: "input", value: "{r.name.clone().unwrap_or_default()}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.name = Some(e.value()).filter(|v| !v.trim().is_empty()); } } } }
                    Field { label: "Must contain (comma separated; /regex/ allowed)", input { class: "input", value: "{r.required.join(\", \")}", onchange: move |e: FormEvent| { if let Some(x) = editing.write().as_mut() { x.required = split_terms(&e.value()); } } } }
                    Field { label: "Must not contain", input { class: "input", value: "{r.ignored.join(\", \")}", onchange: move |e: FormEvent| { if let Some(x) = editing.write().as_mut() { x.ignored = split_terms(&e.value()); } } } }
                    Field { label: "Tags (ids, comma separated)", input { class: "input", value: "{r.tags.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(\", \")}", onchange: move |e: FormEvent| { if let Some(x) = editing.write().as_mut() { x.tags = tag_ids(&e.value()); } } } }
                }
            }
        }
    }
}

/// Sonarr's remote path mappings: where a download client's paths are for Kitsunarr.
#[component]
fn RemotePathMappings() -> Element {
    use kitsunarr_domain::api::settings::RemotePathMapping;
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut list = use_resource(move || {
        let c = c0.clone();
        async move { c.remote_path_mappings().await }
    });
    let mut editing = use_signal(|| None::<RemotePathMapping>);
    let c1 = client.clone();
    let save = move |_| {
        let Some(m) = editing() else { return };
        let c = c1.clone();
        spawn(async move {
            match c.save_remote_path_mapping(&m).await {
                Ok(_) => {
                    editing.set(None);
                    list.restart();
                    app.success("Remote path mapping saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let remove = move |id: i64| {
        let c = client.clone();
        spawn(async move {
            match c.delete_remote_path_mapping(id).await {
                Ok(()) => list.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        div { class: "card",
            div { class: "row",
                h3 { class: "grow", "Remote path mappings" }
                button { class: "btn", onclick: move |_| editing.set(Some(RemotePathMapping { id: 0, host: String::new(), remote_path: String::new(), local_path: String::new() })), "Add" }
            }
            p { class: "muted small", "When a download client reports paths Kitsunarr sees elsewhere (another machine or container), map the client's host and path to the local path." }
            match &*list.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(items)) if items.is_empty() => rsx! { p { class: "small faint", "No mappings." } },
                Some(Ok(items)) => rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr { th { "Host" } th { "Remote path" } th { "Local path" } th { "" } } }
                            tbody {
                                for m in items.iter() {
                                    {
                                        let (mm, id, remove) = (m.clone(), m.id, remove.clone());
                                        rsx! {
                                            tr { key: "{m.id}",
                                                td { "{m.host}" } td { class: "mono small", "{m.remote_path}" } td { class: "mono small", "{m.local_path}" }
                                                td { class: "shrink", div { class: "row tight",
                                                    button { class: "btn sm", onclick: move |_| editing.set(Some(mm.clone())), "Edit" }
                                                    button { class: "btn sm danger", onclick: move |_| remove(id), {crate::i18n::t().delete} }
                                                } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some(m) = editing() {
            Modal { title: "Remote path mapping".to_string(), on_close: move |_| editing.set(None),
                footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| editing.set(None), "Cancel" } button { class: "btn primary", onclick: save, "Save" } },
                div { class: "form-grid",
                    Field { label: "Host (as in the download client's settings)", input { class: "input", value: "{m.host}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.host = e.value(); } } } }
                    Field { label: "Remote path", input { class: "input", value: "{m.remote_path}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.remote_path = e.value(); } } } }
                    Field { label: "Local path", input { class: "input", value: "{m.local_path}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.local_path = e.value(); } } } }
                }
            }
        }
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
    rsx! { ProvidersPage { kind: "notifications", title: "Notifications", note: "Webhook, Notifiarr, Discord, Slack, Telegram, Gotify, ntfy, Pushover, Pushbullet, Join, Prowl, Simplepush, Signal, Apprise, e-mail (SMTP), Mailgun, SendGrid, custom scripts (with Sonarr's environment variables), Emby/Jellyfin, Plex and Kodi are sent: grabs, imports and upgrades, renames, series added or deleted, deleted files, health issues. Use Test in the editor." } }
}

#[component]
pub fn SettingsImportLists() -> Element {
    rsx! {
        ProvidersPage { kind: "importlists", title: "Import lists", note: "AniList, MyAnimeList, Trakt, Simkl, TMDB, MDBList, Plex, another Sonarr and custom JSON lists are read every few hours; new titles are added with each list's root folder, profile and monitoring. Services that need OAuth use your own application's Client ID and a token you paste (tokens migrated from Tanukarr keep working)." }
        ImportListExtras {}
    }
}

/// Jellyfin library and user pickers (Tanukarr's `RequestAction("libraries" | "users")`): read
/// from the server with the editor's host and API key.
/// Tanukarr's account-list pickers: the account's lists, each to include or exclude.
/// Signing in to the list's service instead of pasting a token: the service's page opens (with
/// a code to enter for Simkl and Trakt), and the token is filled in once the user is done.
#[component]
fn SignInButton(mut draft: Signal<ProviderItem>) -> Element {
    let app = use_app();
    let client = use_client();
    let mut waiting = use_signal(|| None::<kitsunarr_domain::api::settings::SignInStarted>);
    let sign_in = move |_| {
        let c = client.clone();
        let d = draft();
        spawn(async move {
            let base = web_sys::window()
                .and_then(|w| w.location().href().ok())
                .and_then(|h| h.split("/settings").next().map(str::to_string))
                .unwrap_or_default();
            let mut req = kitsunarr_domain::api::settings::SignInRequest {
                id: (d.id > 0).then_some(d.id),
                implementation: d.implementation.clone(),
                settings: d.settings.clone(),
                callback: Some(format!(
                    "{}/api/v1/importlists/oauth/callback",
                    base.trim_end_matches('/')
                )),
                state: None,
            };
            let started = match c.sign_in_start(&req).await {
                Ok(s) => s,
                Err(e) => {
                    app.error(&e);
                    return;
                }
            };
            if let Some(w) = web_sys::window() {
                let _ = w.open_with_url_and_target(&started.url, "_blank");
            }
            waiting.set(Some(started.clone()));
            req.state = Some(started.state.clone());
            req.callback = None;
            let deadline = js_sys::Date::now() + started.expires_in as f64 * 1000.0;
            while js_sys::Date::now() < deadline && waiting().is_some() {
                gloo_timers::future::TimeoutFuture::new(started.interval as u32 * 1000).await;
                match c.sign_in_poll(&req).await {
                    Ok(p) if p.done => {
                        draft.write().settings.extend(p.settings);
                        waiting.set(None);
                        app.success("Signed in. Save the list to keep the token.");
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        waiting.set(None);
                        app.error(&e);
                        return;
                    }
                }
            }
            if waiting().is_some() {
                waiting.set(None);
                app.toast(ToastLevel::Error, "The sign-in expired; try again.");
            }
        });
    };
    rsx! {
        div { class: "row", style: "margin-bottom: var(--sp-3)",
            match waiting() {
                None => rsx! {
                    button { class: "btn", r#type: "button", onclick: sign_in, "Sign in" }
                    span { class: "muted small", "Opens the service to authorize Kitsunarr and fills in the token." }
                },
                Some(s) => rsx! {
                    span { class: "small",
                        "Waiting for the sign-in at "
                        a { href: "{s.url}", target: "_blank", rel: "noopener", "{s.url}" }
                        if let Some(code) = &s.code { " — enter the code " strong { class: "mono", "{code}" } }
                    }
                    button { class: "btn sm", r#type: "button", onclick: move |_| waiting.set(None), "Cancel" }
                },
            }
        }
    }
}

#[component]
fn AccountListPickers(mut draft: Signal<ProviderItem>) -> Element {
    let app = use_app();
    let client = use_client();
    let mut lists = use_signal(Vec::<kitsunarr_domain::api::settings::ProviderChoice>::new);
    let implementation = draft().implementation.clone();
    // The setting names and whether values are numbers (Simkl list ids).
    let (include_key, exclude_key, numeric) = match implementation.as_str() {
        "AniListImport" => ("includedCustomLists", "excludedCustomLists", false),
        "TraktListImport" => ("includedLists", "excludedLists", false),
        _ => ("includedCustomListIds", "excludedCustomListIds", true),
    };
    let load = move |_| {
        let c = client.clone();
        let d = draft();
        spawn(async move {
            let req = kitsunarr_domain::api::search::IndexerTestRequest {
                id: (d.id > 0).then_some(d.id),
                implementation: d.implementation.clone(),
                settings: d.settings.clone(),
            };
            match c.import_list_choices(&req).await {
                Ok(list) => lists.set(list),
                Err(e) => app.error(&e),
            }
        });
    };
    let selected = |key: &str| -> Vec<String> {
        draft()
            .settings
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (included, excluded) = (selected(include_key), selected(exclude_key));
    let mut toggle = move |key: &'static str, value: String, on: bool| {
        let mut d = draft.write();
        let mut current: Vec<Value> = d
            .settings
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let matches = |v: &Value| {
            v.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string())
                == value
        };
        current.retain(|v| !matches(v));
        if on {
            current.push(if numeric {
                value
                    .parse::<i64>()
                    .map(Value::from)
                    .unwrap_or(Value::String(value.clone()))
            } else {
                Value::String(value.clone())
            });
        }
        d.settings.insert(key.into(), Value::Array(current));
    };
    rsx! {
        h3 { "Account lists" }
        p { class: "help small faint", "Include only titles on some lists, or leave out the titles of others." }
        div { class: "row", button { class: "btn sm", onclick: load, "Load the account's lists" } }
        if !lists().is_empty() {
            div { class: "table-wrap",
                table { class: "table",
                    thead { tr { th { "List" } th { "Include" } th { "Exclude" } } }
                    tbody {
                        for l in lists() {
                            {
                                let (v1, v2) = (l.value.clone(), l.value.clone());
                                rsx! {
                                    tr { key: "{l.value}",
                                        td { "{l.name}" }
                                        td { input { r#type: "checkbox", "aria-label": "Include {l.name}", checked: included.contains(&l.value),
                                            onchange: move |e: FormEvent| toggle(include_key, v1.clone(), e.checked()) } }
                                        td { input { r#type: "checkbox", "aria-label": "Exclude {l.name}", checked: excluded.contains(&l.value),
                                            onchange: move |e: FormEvent| toggle(exclude_key, v2.clone(), e.checked()) } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn JellyfinPickers(mut draft: Signal<ProviderItem>) -> Element {
    let app = use_app();
    let client = use_client();
    let mut libraries = use_signal(Vec::<kitsunarr_domain::api::settings::ProviderChoice>::new);
    let mut users = use_signal(Vec::<kitsunarr_domain::api::settings::ProviderChoice>::new);
    let load = move |kind: &'static str| {
        let c = client.clone();
        let d = draft();
        spawn(async move {
            let req = kitsunarr_domain::api::search::IndexerTestRequest {
                id: (d.id > 0).then_some(d.id),
                implementation: d.implementation.clone(),
                settings: d.settings.clone(),
            };
            match c.notification_choices(kind, &req).await {
                Ok(list) if kind == "libraries" => libraries.set(list),
                Ok(list) => users.set(list),
                Err(e) => app.error(&e),
            }
        });
    };
    let load_libraries = load.clone();
    let load_users = load;
    let d = draft();
    let current = |k: &str| {
        d.settings
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .replace('-', "")
            .to_lowercase()
    };
    let (library, user) = (current("libraryId"), current("watchedUserId"));
    rsx! {
        h3 { "Jellyfin library and user" }
        div { class: "form-grid",
            Field { label: "Library",
                div { class: "row tight",
                    select { class: "select grow", "aria-label": "Jellyfin library",
                        onchange: move |e| { draft.write().settings.insert("libraryId".into(), Value::String(e.value())); },
                        option { value: "", if libraries().is_empty() { "Load the libraries…" } else { "Choose…" } }
                        for l in libraries() {
                            option { key: "{l.value}", value: "{l.value}", selected: l.value.replace('-', "").to_lowercase() == library,
                                "{l.name}" if let Some(h) = &l.hint { " ({h})" } }
                        }
                    }
                    button { class: "btn sm", onclick: move |_| load_libraries("libraries"), "Load" }
                }
            }
            Field { label: "Watched sync user",
                div { class: "row tight",
                    select { class: "select grow", "aria-label": "Jellyfin user",
                        onchange: move |e| { draft.write().settings.insert("watchedUserId".into(), Value::String(e.value())); },
                        option { value: "", if users().is_empty() { "Load the users…" } else { "Choose…" } }
                        for u in users() {
                            option { key: "{u.value}", value: "{u.value}", selected: u.value.replace('-', "").to_lowercase() == user, "{u.name}" }
                        }
                    }
                    button { class: "btn sm", onclick: move |_| load_users("users"), "Load" }
                }
            }
        }
    }
}

/// Names of a Sonarr enum option stored as a name or as its number.
fn enum_option(v: Option<&Value>, names: &[&'static str]) -> &'static str {
    match v {
        Some(Value::String(s)) => names
            .iter()
            .copied()
            .find(|n| n.eq_ignore_ascii_case(s))
            .unwrap_or(names[0]),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|i| names.get(i as usize).copied())
            .unwrap_or(names[0]),
        _ => names[0],
    }
}

const MONITOR_NAMES: &[&str] = &[
    "unknown",
    "all",
    "future",
    "missing",
    "existing",
    "firstSeason",
    "lastSeason",
    "latestSeason",
    "pilot",
    "recent",
    "monitorSpecials",
    "unmonitorSpecials",
    "none",
    "skip",
];

/// The list behaviour Sonarr keeps beside a list's settings: root folder, profile, monitoring,
/// type, tagging and the preferred catalogue.
#[component]
fn ImportListFields(mut draft: Signal<ProviderItem>) -> Element {
    let client = use_client();
    let c1 = client.clone();
    let roots = use_resource(move || {
        let c = c1.clone();
        async move { c.root_folders().await.unwrap_or_default() }
    });
    let c2 = client.clone();
    let profiles = use_resource(move || {
        let c = c2.clone();
        async move { c.quality_profiles().await.unwrap_or_default() }
    });
    let c3 = client.clone();
    let tags = use_resource(move || {
        let c = c3.clone();
        async move { c.tags().await.unwrap_or_default() }
    });
    let d = draft();
    let o = d.options.clone();
    let mut set = move |k: &'static str, v: Value| {
        draft.write().options.insert(k.into(), v);
    };
    let root = o
        .get("rootFolderPath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let profile = o
        .get("qualityProfileId")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let monitor = match enum_option(o.get("shouldMonitor"), MONITOR_NAMES) {
        "unknown" => "all",
        "latestSeason" => "lastSeason",
        m => m,
    };
    let series_type = enum_option(o.get("seriesType"), &["standard", "daily", "anime"]);
    let tag_existing = enum_option(o.get("tagExisting"), &["none", "add", "sync"]);
    let preferred = enum_option(
        o.get("preferredMetadataSource"),
        &["automatic", "tvdb", "tmdb", "aniList", "myAnimeList"],
    );
    rsx! {
        h3 { "Adding titles" }
        div { class: "form-grid",
            Field { label: "Root folder",
                select { class: "select", "aria-label": "Root folder", value: "{root}",
                    onchange: move |e| set("rootFolderPath", Value::String(e.value())),
                    option { value: "", "Choose…" }
                    for r in roots().unwrap_or_default() {
                        option { key: "{r.id}", value: "{r.path}", selected: r.path == root, "{r.path}" }
                    }
                }
            }
            Field { label: "Quality profile",
                select { class: "select", "aria-label": "Quality profile", value: "{profile}",
                    onchange: move |e| set("qualityProfileId", Value::from(e.value().parse::<i64>().unwrap_or(0))),
                    option { value: "0", "Choose…" }
                    for p in profiles().unwrap_or_default() {
                        option { key: "{p.id}", value: "{p.id}", selected: p.id == profile, "{p.name}" }
                    }
                }
            }
            Field { label: "Monitor",
                select { class: "select", "aria-label": "Monitor", value: "{monitor}",
                    onchange: move |e| set("shouldMonitor", Value::String(e.value())),
                    for (v, l) in [("all", "All episodes"), ("future", "Future episodes"), ("firstSeason", "First season"), ("lastSeason", "Last season"), ("none", "None")] {
                        option { key: "{v}", value: v, selected: v == monitor, "{l}" }
                    }
                }
            }
            Field { label: "Series type", help: "AniList and MyAnimeList titles are always added as anime.".to_string(),
                select { class: "select", "aria-label": "Series type", value: "{series_type}",
                    onchange: move |e| set("seriesType", Value::String(e.value())),
                    for (v, l) in [("standard", "Standard"), ("daily", "Daily"), ("anime", "Anime")] {
                        option { key: "{v}", value: v, selected: v == series_type, "{l}" }
                    }
                }
            }
            Field { label: "Catalogue", help: "The catalogue new titles are added from when the list knows their id there.".to_string(),
                select { class: "select", "aria-label": "Preferred catalogue", value: "{preferred}",
                    onchange: move |e| set("preferredMetadataSource", Value::String(e.value())),
                    for (v, l) in [("automatic", "Automatic"), ("tvdb", "TheTVDB"), ("tmdb", "TMDB"), ("aniList", "AniList"), ("myAnimeList", "MyAnimeList")] {
                        option { key: "{v}", value: v, selected: v == preferred, "{l}" }
                    }
                }
            }
            Field { label: "Tag titles already in the library",
                select { class: "select", "aria-label": "Tag existing titles", value: "{tag_existing}",
                    onchange: move |e| set("tagExisting", Value::String(e.value())),
                    for (v, l) in [("none", "No"), ("add", "Add the list's tags"), ("sync", "Add, and remove when they leave the list")] {
                        option { key: "{v}", value: v, selected: v == tag_existing, "{l}" }
                    }
                }
            }
        }
        if !tags().unwrap_or_default().is_empty() {
            Field { label: "Tags", help: "Added to the titles this list adds.".to_string(),
                div { class: "row wrap",
                    for t in tags().unwrap_or_default() {
                        label { key: "{t.id}", class: "check small",
                            input { r#type: "checkbox", checked: d.tags.contains(&t.id),
                                onchange: move |e: FormEvent| {
                                    let mut w = draft.write();
                                    w.tags.retain(|x| *x != t.id);
                                    if e.checked() { w.tags.push(t.id); }
                                } }
                            "{t.label}"
                        }
                    }
                }
            }
        }
    }
}

/// Clean-library options, per-list status with "Sync now" and the list contents, and exclusions.
#[component]
fn ImportListExtras() -> Element {
    use kitsunarr_domain::api::import_lists::ImportListExclusionItem;
    use kitsunarr_domain::api::settings::{ImportListOptionsSettings, ListSyncLevel};
    let app = use_app();
    let client = use_client();
    let mut opts = use_signal(|| None::<ImportListOptionsSettings>);
    let c0 = client.clone();
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.import_list_options().await {
                Ok(o) => opts.set(Some(o)),
                Err(e) => app.error(&e),
            }
        }
    });
    let c1 = client.clone();
    let tags = use_resource(move || {
        let c = c1.clone();
        async move { c.tags().await.unwrap_or_default() }
    });
    let c2 = client.clone();
    let mut lists = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c2.clone();
        async move {
            (
                c.providers("importlists").await.unwrap_or_default(),
                c.import_list_status().await.unwrap_or_default(),
            )
        }
    });
    let c3 = client.clone();
    let mut exclusions = use_resource(move || {
        let c = c3.clone();
        async move { c.import_list_exclusions().await }
    });
    let mut syncing = use_signal(|| false);
    let mut preview = use_signal(|| None::<(i64, String)>);
    let mut new_excl = use_signal(|| ImportListExclusionItem {
        id: 0,
        provider: "tvdb".into(),
        external_id: String::new(),
        title: None,
    });
    let c4 = client.clone();
    let save = move |_| {
        let Some(o) = opts() else { return };
        let c = c4.clone();
        spawn(async move {
            match c.put_import_list_options(&o).await {
                Ok(_) => app.success("Import list options saved."),
                Err(e) => app.error(&e),
            }
        });
    };
    let c5 = client.clone();
    let sync = move |id: Option<i64>| {
        let c = c5.clone();
        syncing.set(true);
        spawn(async move {
            match c.sync_import_lists(id).await {
                Ok(r) => app.success(&r.message),
                Err(e) => app.error(&e),
            }
            syncing.set(false);
            lists.restart();
        });
    };
    let mut sync_all = sync.clone();
    let c6 = client.clone();
    let add_exclusion = move |_| {
        let c = c6.clone();
        let e = new_excl();
        spawn(async move {
            match c.create_import_list_exclusion(&e).await {
                Ok(_) => {
                    new_excl.write().external_id.clear();
                    new_excl.write().title = None;
                    exclusions.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let c7 = client.clone();
    let delete_exclusion = move |id: i64| {
        let c = c7.clone();
        spawn(async move {
            match c.delete_import_list_exclusion(id).await {
                Ok(()) => exclusions.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    let (list_items, statuses) = lists().unwrap_or_default();
    let supported: Vec<ProviderItem> = list_items.into_iter().filter(|l| l.supported).collect();
    rsx! {
        if !supported.is_empty() {
            div { class: "card",
                div { class: "row",
                    h3 { class: "grow", "Sync" }
                    button { class: "btn", disabled: syncing(), onclick: move |_| sync_all(None),
                        if syncing() { "Syncing…" } else { "Sync all lists now" } }
                }
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "List" } th { "Last read" } th { class: "num", "Shows" } th { "Status" } th { "" } } }
                        tbody {
                            for l in supported.iter() {
                                {
                                    let st = statuses.iter().find(|s| s.import_list_id == l.id).cloned();
                                    let id = l.id;
                                    let name = l.name.clone();
                                    let mut sync_one = sync.clone();
                                    rsx! {
                                        tr { key: "{l.id}",
                                            td { strong { "{l.name}" } }
                                            td { class: "small", {st.as_ref().and_then(|s| s.last_info_sync.clone()).unwrap_or_else(|| "never".into())} }
                                            td { class: "num", {st.as_ref().map(|s| s.item_count.to_string()).unwrap_or_default()} }
                                            td {
                                                match st.as_ref().and_then(|s| s.last_error.clone()) {
                                                    Some(err) => rsx! { span { class: "badge danger", title: "{err}", "failing" } span { class: "small muted", " {err}" } },
                                                    None if st.as_ref().is_some_and(|s| s.last_info_sync.is_some()) => rsx! { Badge { text: "ok", tone: "success" } },
                                                    None => rsx! { Badge { text: "not read yet" } },
                                                }
                                            }
                                            td { class: "shrink",
                                                div { class: "row tight",
                                                    button { class: "btn sm", disabled: syncing(), onclick: move |_| sync_one(Some(id)), "Sync" }
                                                    button { class: "btn sm", onclick: move |_| preview.set(Some((id, name.clone()))), "Shows" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        div { class: "card",
            h3 { "Clean library" }
            match opts() {
                None => rsx! { Loading {} },
                Some(o) => rsx! {
                    div { class: "form-grid",
                        Field { label: "Titles on no list", help: "Applied after every list has been read successfully and some title left a list.".to_string(),
                            select { class: "select", "aria-label": "Clean library level",
                                onchange: move |e| {
                                    let v = match e.value().as_str() {
                                        "logOnly" => ListSyncLevel::LogOnly,
                                        "keepAndUnmonitor" => ListSyncLevel::KeepAndUnmonitor,
                                        "keepAndTag" => ListSyncLevel::KeepAndTag,
                                        _ => ListSyncLevel::Disabled,
                                    };
                                    if let Some(x) = opts.write().as_mut() { x.list_sync_level = v; }
                                },
                                for (v, l, lvl) in [("disabled", "Do nothing", ListSyncLevel::Disabled), ("logOnly", "Log them", ListSyncLevel::LogOnly), ("keepAndUnmonitor", "Unmonitor them", ListSyncLevel::KeepAndUnmonitor), ("keepAndTag", "Tag them", ListSyncLevel::KeepAndTag)] {
                                    option { key: "{v}", value: v, selected: o.list_sync_level == lvl, "{l}" }
                                }
                            }
                        }
                        if o.list_sync_level == ListSyncLevel::KeepAndTag {
                            Field { label: "Tag",
                                select { class: "select", "aria-label": "Clean library tag",
                                    onchange: move |e| { if let Some(x) = opts.write().as_mut() { x.list_sync_tag = e.value().parse().ok(); } },
                                    option { value: "", "Choose…" }
                                    for t in tags().unwrap_or_default() {
                                        option { key: "{t.id}", value: "{t.id}", selected: o.list_sync_tag == Some(t.id), "{t.label}" }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "row end", button { class: "btn primary", onclick: save, "Save options" } }
                },
            }
        }
        div { class: "card",
            h3 { "Exclusions" }
            p { class: "muted small", "Titles import lists never add." }
            div { class: "row wrap",
                select { class: "select", "aria-label": "Exclusion catalogue", style: "width: auto",
                    onchange: move |e| new_excl.write().provider = e.value(),
                    for (v, l) in [("tvdb", "TheTVDB"), ("tmdb", "TMDB"), ("aniList", "AniList"), ("myAnimeList", "MyAnimeList")] {
                        option { key: "{v}", value: v, selected: new_excl().provider == v, "{l}" }
                    }
                }
                input { class: "input", style: "width: 8rem", "aria-label": "Exclusion id", placeholder: "id", value: "{new_excl().external_id}",
                    oninput: move |e| new_excl.write().external_id = e.value() }
                input { class: "input grow", "aria-label": "Exclusion title", placeholder: "title (optional)", value: "{new_excl().title.clone().unwrap_or_default()}",
                    oninput: move |e| new_excl.write().title = Some(e.value()) }
                button { class: "btn", onclick: add_exclusion, "Add exclusion" }
            }
            match &*exclusions.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) if list.is_empty() => rsx! { p { class: "small faint", "No exclusions." } },
                Some(Ok(list)) => rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr { th { "Title" } th { "Catalogue" } th { "Id" } th { "" } } }
                            tbody {
                                for e in list.iter() {
                                    {
                                        let id = e.id;
                                        let del = delete_exclusion.clone();
                                        rsx! {
                                            tr { key: "{e.id}",
                                                td { {e.title.clone().unwrap_or_default()} }
                                                td { "{e.provider}" }
                                                td { class: "mono", "{e.external_id}" }
                                                td { class: "shrink", button { class: "btn sm danger", onclick: move |_| del(id), {crate::i18n::t().delete} } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some((id, name)) = preview() {
            ImportListShows { id, name, on_close: move |_| preview.set(None) }
        }
    }
}

/// The shows a list contained at its last read, whether each is in the library, and Tanukarr's
/// selective add (up to 25 at a time).
#[component]
fn ImportListShows(id: i64, name: String, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut entries = use_resource(move || {
        let c = c0.clone();
        async move { c.import_list_entries(id).await }
    });
    let c1 = client.clone();
    let roots = use_resource(move || {
        let c = c1.clone();
        async move { c.root_folders().await.unwrap_or_default() }
    });
    let c2 = client.clone();
    let profiles = use_resource(move || {
        let c = c2.clone();
        async move { c.quality_profiles().await.unwrap_or_default() }
    });
    let mut chosen = use_signal(Vec::<usize>::new);
    let mut root = use_signal(String::new);
    let mut profile = use_signal(|| 0i64);
    let mut monitored = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let add = move |_| {
        let c = client.clone();
        let req = kitsunarr_domain::api::import_lists::ImportListAddRequest {
            items: chosen(),
            root_folder_path: root(),
            quality_profile_id: profile(),
            monitored: monitored(),
        };
        busy.set(true);
        spawn(async move {
            match c.add_import_list_items(id, &req).await {
                Ok(results) => {
                    let added = results.iter().filter(|r| r.outcome == "added").count();
                    let failed: Vec<String> = results
                        .iter()
                        .filter(|r| r.outcome == "failed")
                        .map(|r| format!("{}: {}", r.title, r.message.clone().unwrap_or_default()))
                        .collect();
                    if failed.is_empty() {
                        app.success(format!("{added} shows added."));
                    } else {
                        app.toast(
                            ToastLevel::Warning,
                            format!("{added} added; {}", failed.join("; ")),
                        );
                    }
                    chosen.set(Vec::new());
                    entries.restart();
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let can_add = !chosen().is_empty()
        && chosen().len() <= 25
        && !root().is_empty()
        && profile() > 0
        && !busy();
    rsx! {
        Modal { title: format!("{name}: shows"), wide: true, on_close: move |_| on_close.call(()),
            footer: rsx! {
                span { class: "small muted", "{chosen().len()} chosen (at most 25)" }
                div { class: "spacer" }
                button { class: "btn", onclick: move |_| on_close.call(()), "Close" }
                button { class: "btn primary", disabled: !can_add, onclick: add, if busy() { "Adding…" } else { "Add selected" } }
            },
            div { class: "row wrap", style: "margin-bottom: var(--sp-3)",
                select { class: "select", style: "width: auto", "aria-label": "Root folder for added shows", onchange: move |e| root.set(e.value()),
                    option { value: "", "Root folder…" }
                    for r in roots().unwrap_or_default() { option { key: "{r.id}", value: "{r.path}", "{r.path}" } }
                }
                select { class: "select", style: "width: auto", "aria-label": "Quality profile for added shows", onchange: move |e| profile.set(e.value().parse().unwrap_or(0)),
                    option { value: "0", "Quality profile…" }
                    for p in profiles().unwrap_or_default() { option { key: "{p.id}", value: "{p.id}", "{p.name}" } }
                }
                label { class: "check small", input { r#type: "checkbox", checked: monitored(), onchange: move |e: FormEvent| monitored.set(e.checked()) } "Monitored" }
            }
            match &*entries.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) if list.is_empty() => rsx! { p { class: "muted", "The list has not been read, or it is empty." } },
                Some(Ok(list)) => rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr { th { "" } th { "Title" } th { "Year" } th { "Ids" } th { "" } } }
                            tbody {
                                for (i, e) in list.iter().enumerate() {
                                    tr { key: "{i}",
                                        td { class: "shrink",
                                            if e.series_id.is_none() && !e.excluded {
                                                input { r#type: "checkbox", "aria-label": "Choose {e.title}", checked: chosen().contains(&i),
                                                    onchange: move |ev: FormEvent| { let mut w = chosen.write(); w.retain(|x| *x != i); if ev.checked() { w.push(i); } } }
                                            }
                                        }
                                        td { "{e.title}" }
                                        td { {e.year.map(|y| y.to_string()).unwrap_or_default()} }
                                        td { class: "small mono",
                                            {[("tvdb", e.tvdb_id.map(|v| v.to_string())), ("tmdb", e.tmdb_id.map(|v| v.to_string())), ("imdb", e.imdb_id.clone()), ("anilist", e.ani_list_id.map(|v| v.to_string())), ("mal", e.mal_id.map(|v| v.to_string()))]
                                                .into_iter().filter_map(|(k, v)| v.map(|v| format!("{k}:{v}"))).collect::<Vec<_>>().join(" ")}
                                        }
                                        td {
                                            if e.excluded { Badge { text: "excluded", tone: "warning" } }
                                            else if let Some(sid) = e.series_id {
                                                Link { to: crate::route::Route::SeriesDetailPage { id: sid }, Badge { text: "in library", tone: "success" } }
                                            } else { Badge { text: "not added" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
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
                                            button { class: "btn sm", onclick: { let p = p.clone(); move |_| {
                                                // Settings added since the provider was saved show with their defaults.
                                                let mut p = p.clone();
                                                if let Some(Ok(ts)) = &*templates.read()
                                                    && let Some(t) = ts.iter().find(|t| t.implementation == p.implementation)
                                                {
                                                    for (k, v) in &t.settings {
                                                        p.settings.entry(k.clone()).or_insert_with(|| v.clone());
                                                    }
                                                    // Event switches added since it was saved are off.
                                                    if p.kind == ProviderKind::Notification {
                                                        for (k, v) in &t.options {
                                                            if v.is_boolean() {
                                                                p.options.entry(k.clone()).or_insert(Value::Bool(false));
                                                            }
                                                        }
                                                    }
                                                }
                                                editing.set(Some(p));
                                            } }, "Edit" }
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
        ProviderKind::Indexer => {
            matches!(
                item.implementation.as_str(),
                "Newznab"
                    | "Torznab"
                    | "Nyaa"
                    | "TorrentRssIndexer"
                    | "IPTorrents"
                    | "Torrentleech"
                    | "Fanzub"
                    | "FileList"
                    | "HDBits"
                    | "BroadcastheNet"
            )
        }
        ProviderKind::DownloadClient => {
            kitsunarr_domain::api::settings::DOWNLOAD_CLIENT_IMPLEMENTATIONS
                .contains(&item.implementation.as_str())
        }
        ProviderKind::Notification => kitsunarr_domain::api::settings::NOTIFICATION_IMPLEMENTATIONS
            .iter()
            .any(|i| i.eq_ignore_ascii_case(&item.implementation)),
        ProviderKind::ImportList => item.supported,
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
            } else if d.kind == ProviderKind::ImportList {
                c.test_import_list(&req)
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
            if item.kind == ProviderKind::ImportList {
                ImportListFields { draft }
            }
            if item.kind == ProviderKind::Notification && item.implementation.eq_ignore_ascii_case("Jellyfin") {
                JellyfinPickers { draft }
            }
            if item.kind == ProviderKind::ImportList
                && matches!(item.implementation.as_str(), "SimklUserImport" | "TraktListImport" | "TraktUserImport" | "TraktPopularImport" | "TraktSmartListImport" | "PlexImport" | "MyAnimeListImport")
            {
                SignInButton { draft }
            }
            if matches!(item.implementation.as_str(), "AniListImport" | "TraktListImport" | "SimklUserImport") {
                AccountListPickers { draft }
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
        ("passkey", "Passkey"),
        ("username", "User name"),
        (
            "cookie",
            "Cookie (sent with the feed request, e.g. uid=1; pass=…)",
        ),
        (
            "allowZeroSize",
            "Allow zero size (the feed has no release sizes)",
        ),
        ("minimumSeeders", "Minimum seeders"),
        ("codecs", "Codecs"),
        ("mediums", "Mediums"),
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
        (
            "seedCriteria.seedRatio",
            "Seed ratio (empty: the client's default)",
        ),
        (
            "seedCriteria.seedTime",
            "Seed time, minutes (empty: the client's default)",
        ),
        (
            "seedCriteria.seasonPackSeedGoal",
            "Season packs: 1 uses the season-pack goals below",
        ),
        ("seedCriteria.seasonPackSeedRatio", "Season-pack seed ratio"),
        (
            "seedCriteria.seasonPackSeedTime",
            "Season-pack seed time, minutes",
        ),
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
            h3 { "API keys and language" }
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
                        Field { label: "Preferred metadata language", help: "TheTVDB titles are searched and added in this language; each series can be changed in its editor.".to_string(),
                            select { class: "select", "aria-label": "Preferred metadata language", value: "{k.preferred_metadata_language.unwrap_or(1)}",
                                onchange: move |e: FormEvent| { if let Some(x) = keys.write().as_mut() { x.preferred_metadata_language = e.value().parse().ok(); } },
                                for l in kitsunarr_domain::Language::all().iter().filter(|l| l.id > 0) {
                                    option { key: "{l.id}", value: "{l.id}", "{l.name}" }
                                }
                            }
                        }
                    }
                    p { class: "help small faint", "Saved keys show as {REDACTED}; leave them unchanged to keep them, or clear the field to remove one." }
                    div { class: "row end", button { class: "btn primary", onclick: save, "Save keys" } }
                },
            }
        }
        MetadataConsumers {}
    }
}

/// Sonarr's Settings › Metadata: files written next to the media for media centres.
#[component]
fn MetadataConsumers() -> Element {
    use kitsunarr_domain::api::settings::MetadataConsumersSettings;
    let app = use_app();
    let client = use_client();
    let mut form = use_signal(|| None::<MetadataConsumersSettings>);
    let c0 = client.clone();
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.metadata_consumers().await {
                Ok(m) => form.set(Some(m)),
                Err(e) => app.error(&e),
            }
        }
    });
    let save = move |_| {
        let Some(m) = form() else { return };
        let c = client.clone();
        spawn(async move {
            match c.put_metadata_consumers(&m).await {
                Ok(saved) => {
                    form.set(Some(saved));
                    app.success("Metadata settings saved. Files are written at the next disk scan, refresh or import.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let Some(m) = form() else {
        return rsx! { div { class: "card", style: "margin-top: var(--sp-4)", Loading {} } };
    };
    macro_rules! flag {
        ($label:expr, $($path:ident).+) => {
            rsx! { Switch { checked: m.$($path).+, label: $label, onchange: move |v| { if let Some(x) = form.write().as_mut() { x.$($path).+ = v; } } } }
        };
    }
    rsx! {
        div { class: "card", style: "margin-top: var(--sp-4)",
            h3 { "Metadata files" }
            p { class: "small faint", "Written in the series folders when series are scanned, refreshed or imported, as Sonarr's metadata consumers write them." }
            div { class: "form-grid",
                div {
                    h4 { "Kodi (XBMC) / Emby" }
                    {flag!("Enabled", kodi.enable)}
                    if m.kodi.enable {
                        {flag!("Series metadata (tvshow.nfo)", kodi.series_metadata)}
                        {flag!("Series metadata episode guide", kodi.series_metadata_episode_guide)}
                        {flag!("Series metadata URL", kodi.series_metadata_url)}
                        {flag!("Episode metadata (<filename>.nfo)", kodi.episode_metadata)}
                        {flag!("Episode image thumbs in the NFO", kodi.episode_image_thumb)}
                        {flag!("Series images (fanart.jpg, poster.jpg, banner.jpg)", kodi.series_images)}
                        {flag!("Season images (season##-poster.jpg)", kodi.season_images)}
                        {flag!("Episode images (<filename>-thumb.jpg)", kodi.episode_images)}
                    }
                }
                div {
                    h4 { "Plex" }
                    {flag!("Enabled", plex.enable)}
                    if m.plex.enable {
                        {flag!("Series .plexmatch file", plex.series_plex_match_file)}
                        {flag!("Episode mappings in .plexmatch", plex.episode_mappings)}
                    }
                }
                div {
                    h4 { "Roksbox" }
                    {flag!("Enabled", roksbox.enable)}
                    if m.roksbox.enable {
                        {flag!("Episode metadata (Season##/filename.xml)", roksbox.episode_metadata)}
                        {flag!("Series images (Series Title.jpg)", roksbox.series_images)}
                        {flag!("Season images (Season ##.jpg)", roksbox.season_images)}
                        {flag!("Episode images (Season##/filename.jpg)", roksbox.episode_images)}
                    }
                }
                div {
                    h4 { "WDTV" }
                    {flag!("Enabled", wdtv.enable)}
                    if m.wdtv.enable {
                        {flag!("Episode metadata (Season##/filename.xml)", wdtv.episode_metadata)}
                        {flag!("Series images (folder.jpg)", wdtv.series_images)}
                        {flag!("Season images (Season##/folder.jpg)", wdtv.season_images)}
                        {flag!("Episode images (Season##/filename.metathumb)", wdtv.episode_images)}
                    }
                }
            }
            div { class: "row end", button { class: "btn primary", onclick: save, "Save metadata files" } }
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
        AutoTagging {}
    }
}

/// Default fields of a new condition of the given implementation.
fn auto_tag_spec(
    implementation: &str,
) -> kitsunarr_domain::api::settings::CustomFormatSpecification {
    let name = kitsunarr_domain::auto_tagging::SPECIFICATIONS
        .iter()
        .find(|(i, _)| *i == implementation)
        .map_or("Condition", |(_, n)| n);
    let fields = match implementation {
        "GenreSpecification" | "NetworkSpecification" | "OriginalCountrySpecification" => {
            serde_json::json!({"value": []})
        }
        "MonitoredSpecification" => serde_json::json!({}),
        "RootFolderSpecification" => serde_json::json!({"value": ""}),
        "StatusSpecification" => serde_json::json!({"status": 0}),
        "YearSpecification" => serde_json::json!({"min": 2000, "max": 2030}),
        "OriginalLanguageSpecification" => serde_json::json!({"value": 1}),
        _ => serde_json::json!({"value": 0}),
    };
    kitsunarr_domain::api::settings::CustomFormatSpecification {
        name: name.to_string(),
        implementation: implementation.to_string(),
        negate: false,
        required: false,
        fields: fields.as_object().cloned().unwrap_or_default(),
    }
}

/// Sonarr's auto tagging (Settings › Tags › Auto Tagging): rules that add tags to titles
/// matching their conditions when a title is added, edited or refreshed.
#[component]
fn AutoTagging() -> Element {
    use kitsunarr_domain::api::settings::AutoTag;
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut list = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c0.clone();
        async move { c.auto_tags().await }
    });
    let c1 = client.clone();
    let lookups = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c1.clone();
        async move {
            let tags = c.tags().await.unwrap_or_default();
            let profiles = c.quality_profiles().await.unwrap_or_default();
            let roots = c.root_folders().await.unwrap_or_default();
            (tags, profiles, roots)
        }
    });
    let mut editing = use_signal(|| None::<AutoTag>);
    let mut new_kind = use_signal(|| "GenreSpecification".to_string());
    let c2 = client.clone();
    let save = move |_| {
        let Some(r) = editing() else { return };
        let c = c2.clone();
        spawn(async move {
            match c.save_auto_tag(&r).await {
                Ok(_) => {
                    editing.set(None);
                    list.restart();
                    app.success("Auto tag saved. It applies when titles are next added, edited or refreshed.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let remove = move |id: i64| {
        let c = client.clone();
        spawn(async move {
            match c.delete_auto_tag(id).await {
                Ok(()) => list.restart(),
                Err(e) => app.error(&e),
            }
        });
    };
    let (tags, profiles, roots) = lookups.read().clone().unwrap_or_default();
    let tag_label = {
        let tags = tags.clone();
        move |id: i64| {
            tags.iter()
                .find(|t| t.id == id)
                .map_or_else(|| format!("#{id}"), |t| t.label.clone())
        }
    };
    rsx! {
        div { class: "card", style: "margin-top: var(--sp-5)",
            div { class: "row",
                h3 { class: "grow", "Auto tagging" }
                button { class: "btn", onclick: move |_| editing.set(Some(AutoTag { id: 0, name: String::new(), remove_tags_automatically: false, specifications: Vec::new(), tags: Vec::new() })), "Add auto tag" }
            }
            p { class: "small faint", "Tags are added to series and films that match every kind of condition when they are added, edited or refreshed." }
            match &*list.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(items)) if items.is_empty() => rsx! { p { class: "small faint", "No auto tags." } },
                Some(Ok(items)) => rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr { th { "Name" } th { "Tags" } th { "Conditions" } th { "" } } }
                            tbody {
                                for r in items.iter() {
                                    {
                                        let (rr, id, remove) = (r.clone(), r.id, remove.clone());
                                        let labels: Vec<String> = r.tags.iter().map(|t| tag_label(*t)).collect();
                                        let conditions: Vec<String> = r.specifications.iter().map(|s| s.name.clone()).collect();
                                        rsx! {
                                            tr { key: "{r.id}",
                                                td { "{r.name}" if r.remove_tags_automatically { " " Badge { text: "removes".to_string() } } }
                                                td { class: "small", {labels.join(", ")} }
                                                td { class: "small", {conditions.join(", ")} }
                                                td { class: "shrink", div { class: "row tight",
                                                    button { class: "btn sm", onclick: move |_| editing.set(Some(rr.clone())), "Edit" }
                                                    button { class: "btn sm danger", onclick: move |_| remove(id), {crate::i18n::t().delete} }
                                                } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some(r) = editing() {
            Modal { title: "Auto tag".to_string(), wide: true, on_close: move |_| editing.set(None),
                footer: rsx! { div { class: "spacer" } button { class: "btn", onclick: move |_| editing.set(None), "Cancel" } button { class: "btn primary", onclick: save, "Save" } },
                div { class: "form-grid",
                    Field { label: "Name", input { class: "input", "aria-label": "Auto tag name", value: "{r.name}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.name = e.value(); } } } }
                    Field { label: "Tags",
                        div { class: "row",
                            for t in tags.iter() {
                                {
                                    let (id, on) = (t.id, r.tags.contains(&t.id));
                                    rsx! {
                                        label { key: "{t.id}", class: "row tight small",
                                            input { r#type: "checkbox", checked: on, "aria-label": "Tag {t.label}",
                                                onchange: move |e: FormEvent| { if let Some(x) = editing.write().as_mut() { x.tags.retain(|v| *v != id); if e.checked() { x.tags.push(id); } } } }
                                            "{t.label}"
                                        }
                                    }
                                }
                            }
                            if tags.is_empty() { span { class: "small faint", "Add a tag above first." } }
                        }
                    }
                }
                Switch { checked: r.remove_tags_automatically, label: "Remove tags automatically (from titles that stop matching)", onchange: move |v| { if let Some(x) = editing.write().as_mut() { x.remove_tags_automatically = v; } } }
                h4 { "Conditions" }
                p { class: "small faint", "Conditions of one kind match when any of them matches (all the required ones must); every kind must match." }
                for (i, spec) in r.specifications.iter().enumerate() {
                    {
                        let spec = spec.clone();
                        let kind_name = kitsunarr_domain::auto_tagging::SPECIFICATIONS.iter().find(|(k, _)| *k == spec.implementation).map_or(spec.implementation.clone(), |(_, n)| n.to_string());
                        let mut set_field = move |k: &'static str, v: Value| { if let Some(x) = editing.write().as_mut() { x.specifications[i].fields.insert(k.to_string(), v); } };
                        let value = spec.fields.get("value").cloned().unwrap_or(Value::Null);
                        let int_value = value.as_i64().unwrap_or(0);
                        rsx! {
                            div { key: "{i}-{spec.implementation}", class: "card", style: "padding: var(--sp-3); margin-bottom: var(--sp-3)",
                                div { class: "row",
                                    Badge { text: kind_name }
                                    input { class: "input grow", "aria-label": "Condition name", value: "{spec.name}", oninput: move |e| { if let Some(x) = editing.write().as_mut() { x.specifications[i].name = e.value(); } } }
                                    button { class: "btn sm danger", "aria-label": "Remove condition", onclick: move |_| { if let Some(x) = editing.write().as_mut() { x.specifications.remove(i); } }, "✕" }
                                }
                                div { class: "row", style: "margin-top: var(--sp-2)",
                                    match spec.implementation.as_str() {
                                        "GenreSpecification" | "NetworkSpecification" | "OriginalCountrySpecification" => {
                                            let text = value.as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                                            rsx! { input { class: "input grow", "aria-label": "Values", placeholder: "comma separated", value: "{text}",
                                                onchange: move |e: FormEvent| set_field("value", Value::from(split_terms(&e.value()))) } }
                                        }
                                        "MonitoredSpecification" => rsx! { span { class: "small faint", "Matches monitored titles." } },
                                        "OriginalLanguageSpecification" => rsx! {
                                            select { class: "select", "aria-label": "Language", value: "{int_value}", onchange: move |e: FormEvent| set_field("value", Value::from(e.value().parse::<i64>().unwrap_or(1))),
                                                for l in kitsunarr_domain::Language::all().iter() { option { key: "{l.id}", value: "{l.id}", selected: i64::from(l.id) == int_value, "{l.name}" } }
                                            }
                                        },
                                        "QualityProfileSpecification" => rsx! {
                                            select { class: "select", "aria-label": "Quality profile", onchange: move |e: FormEvent| set_field("value", Value::from(e.value().parse::<i64>().unwrap_or(0))),
                                                option { value: "0", selected: int_value == 0, "Choose…" }
                                                for p in profiles.iter() { option { key: "{p.id}", value: "{p.id}", selected: p.id == int_value, "{p.name}" } }
                                            }
                                        },
                                        "RootFolderSpecification" => {
                                            let current = value.as_str().unwrap_or_default().to_string();
                                            rsx! {
                                                select { class: "select", "aria-label": "Root folder", onchange: move |e: FormEvent| set_field("value", Value::from(e.value())),
                                                    option { value: "", selected: current.is_empty(), "Choose…" }
                                                    for f in roots.iter() { option { key: "{f.id}", value: "{f.path}", selected: f.path == current, "{f.path}" } }
                                                }
                                            }
                                        }
                                        "SeriesTypeSpecification" => rsx! {
                                            select { class: "select", "aria-label": "Series type", onchange: move |e: FormEvent| set_field("value", Value::from(e.value().parse::<i64>().unwrap_or(0))),
                                                for (v, n) in [(0, "Standard"), (1, "Daily"), (2, "Anime")] { option { key: "{v}", value: "{v}", selected: v == int_value, "{n}" } }
                                            }
                                        },
                                        "StatusSpecification" => {
                                            let status = spec.fields.get("status").and_then(Value::as_i64).unwrap_or(0);
                                            rsx! {
                                                select { class: "select", "aria-label": "Status", onchange: move |e: FormEvent| set_field("status", Value::from(e.value().parse::<i64>().unwrap_or(0))),
                                                    for (v, n) in [(0, "Continuing"), (1, "Ended"), (2, "Upcoming"), (-1, "Deleted")] { option { key: "{v}", value: "{v}", selected: v == status, "{n}" } }
                                                }
                                            }
                                        }
                                        "TagSpecification" => rsx! {
                                            select { class: "select", "aria-label": "Tag", onchange: move |e: FormEvent| set_field("value", Value::from(e.value().parse::<i64>().unwrap_or(0))),
                                                option { value: "0", selected: int_value == 0, "Choose…" }
                                                for t in tags.iter() { option { key: "{t.id}", value: "{t.id}", selected: t.id == int_value, "{t.label}" } }
                                            }
                                        },
                                        "YearSpecification" => {
                                            let min = spec.fields.get("min").and_then(Value::as_i64).unwrap_or(0);
                                            let max = spec.fields.get("max").and_then(Value::as_i64).unwrap_or(0);
                                            rsx! {
                                                input { class: "input", r#type: "number", "aria-label": "Minimum year", value: "{min}", oninput: move |e| set_field("min", Value::from(e.value().parse::<i64>().unwrap_or(0))) }
                                                span { "to" }
                                                input { class: "input", r#type: "number", "aria-label": "Maximum year", value: "{max}", oninput: move |e| set_field("max", Value::from(e.value().parse::<i64>().unwrap_or(0))) }
                                            }
                                        }
                                        _ => rsx! { span { class: "small faint", "Not evaluated by Kitsunarr." } },
                                    }
                                }
                                div { class: "row", style: "margin-top: var(--sp-2)",
                                    Switch { checked: spec.negate, label: "Negate", onchange: move |v| { if let Some(x) = editing.write().as_mut() { x.specifications[i].negate = v; } } }
                                    Switch { checked: spec.required, label: "Required", onchange: move |v| { if let Some(x) = editing.write().as_mut() { x.specifications[i].required = v; } } }
                                }
                            }
                        }
                    }
                }
                div { class: "row",
                    select { class: "select", "aria-label": "Condition type", onchange: move |e: FormEvent| new_kind.set(e.value()),
                        for (k, n) in kitsunarr_domain::auto_tagging::SPECIFICATIONS.iter() { option { key: "{k}", value: "{k}", selected: *k == new_kind(), "{n}" } }
                    }
                    button { class: "btn", onclick: move |_| { let spec = auto_tag_spec(&new_kind()); if let Some(x) = editing.write().as_mut() { x.specifications.push(spec); } }, "Add condition" }
                }
            }
        }
    }
}
