//! Migration wizard: Sonarr / Tanukarr → Kitsunarr.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use dioxus::web::WebFileExt;
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::migration::{
    ApiSourceRequest, ConflictPolicy, MIGRATION_ENTITY_GROUPS, MigrationOptions, MigrationPreview,
    MigrationReport, MigrationSession, PathMapping, SessionStatus, SourceApp, SourceMethod,
    WarningSeverity,
};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::components::common::{Badge, Confirm, ErrorBox, Field, Loading, Modal, Progress};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::format_datetime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Source,
    Loading,
    Configure,
    Preview,
    Applying,
    Done,
}

impl Step {
    fn index(self) -> usize {
        match self {
            Step::Source => 0,
            Step::Loading | Step::Configure => 1,
            Step::Preview => 2,
            Step::Applying | Step::Done => 3,
        }
    }
}

const STEP_LABELS: [&str; 4] = ["Choose source", "Configure", "Preview", "Apply"];

fn group_label(g: &str) -> &'static str {
    match g {
        "tags" => "Tags",
        "rootFolders" => "Root folders",
        "qualityDefinitions" => "Quality sizes",
        "customFormats" => "Custom formats",
        "qualityProfiles" => "Quality profiles",
        "delayProfiles" => "Delay profiles",
        "releaseProfiles" => "Release profiles",
        "indexers" => "Indexers",
        "downloadClients" => "Download clients",
        "notifications" => "Notifications",
        "importLists" => "Import lists",
        "importListExclusions" => "List exclusions",
        "remotePathMappings" => "Remote path mappings",
        "settings" => "Naming & file settings",
        "series" => "Series & anime",
        "movies" => "Movies (Tanukarr)",
        "history" => "History",
        _ => "Other",
    }
}

/// Streams a `File` to the backup endpoint with `fetch` (no copy into WebAssembly memory).
async fn upload_backup(file: web_sys::File) -> Result<MigrationSession, String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&file);
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    let headers = web_sys::Headers::new().map_err(js)?;
    headers
        .set("Content-Type", "application/octet-stream")
        .map_err(js)?;
    init.set_headers(&headers);
    let url = format!(
        "{}/api/v1/migration/sessions/backup",
        crate::util::api_root()
    );
    let request = web_sys::Request::new_with_str_and_init(&url, &init).map_err(js)?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| "The upload failed (network error).".to_string())?
        .dyn_into()
        .map_err(js)?;
    let text = JsFuture::from(response.text().map_err(js)?)
        .await
        .map_err(js)?
        .as_string()
        .unwrap_or_default();
    if !response.ok() {
        return Err(serde_json::from_str::<ApiErrorBody>(&text)
            .map(|b| b.message)
            .unwrap_or_else(|_| format!("Upload failed (HTTP {}).", response.status())));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[component]
pub fn ImportWizard() -> Element {
    let app = use_app();
    let client = use_client();
    let mut step = use_signal(|| Step::Source);
    let mut use_backup = use_signal(|| true);
    let mut url = use_signal(String::new);
    let mut api_key = use_signal(String::new);
    let mut allow_private = use_signal(|| true);
    let mut session = use_signal(|| None::<MigrationSession>);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut mappings = use_signal(Vec::<(String, String)>::new);
    let mut groups = use_signal(|| {
        MIGRATION_ENTITY_GROUPS
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<String>>()
    });
    let mut update_existing = use_signal(|| false);
    let mut include_history = use_signal(|| true);
    let mut verify_paths = use_signal(|| true);
    let mut preview = use_signal(|| None::<MigrationPreview>);
    let mut report = use_signal(|| None::<MigrationReport>);
    let mut confirm_apply = use_signal(|| false);
    let mut runs_rev = use_signal(|| 0u64);

    let options = move || MigrationOptions {
        path_mappings: mappings
            .read()
            .iter()
            .filter(|(s, t)| !s.trim().is_empty() && !t.trim().is_empty())
            .map(|(s, t)| PathMapping {
                source_prefix: s.trim().into(),
                target_prefix: t.trim().into(),
            })
            .collect(),
        entities: if groups.read().len() == MIGRATION_ENTITY_GROUPS.len() {
            Vec::new()
        } else {
            groups.read().iter().cloned().collect()
        },
        conflict_policy: if update_existing() {
            ConflictPolicy::Update
        } else {
            ConflictPolicy::Skip
        },
        include_history: include_history(),
        verify_paths: verify_paths(),
    };

    // Polls a session until it leaves the given state.
    let poll_client = client.clone();
    let wait_for = use_callback(move |(id, leaving): (String, SessionStatus)| {
        let client = poll_client.clone();
        spawn(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(800).await;
                match client.migration_session(&id).await {
                    Ok(s) if s.status == leaving => session.set(Some(s)),
                    Ok(s) => {
                        let status = s.status;
                        let run_id = s.run_id;
                        let failure = s.error.clone();
                        if status == SessionStatus::Ready && leaving == SessionStatus::Loading {
                            let roots = s.source_root_folders.clone();
                            if mappings.read().is_empty() {
                                mappings.set(roots.into_iter().map(|r| (r.clone(), r)).collect());
                            }
                        }
                        session.set(Some(s));
                        match (leaving, status) {
                            (SessionStatus::Loading, SessionStatus::Ready) => {
                                step.set(Step::Configure)
                            }
                            (SessionStatus::Loading, _) => {
                                error.set(failure);
                                step.set(Step::Source);
                            }
                            (SessionStatus::Applying, SessionStatus::Completed) => {
                                if let Some(run) = run_id {
                                    match client.migration_report(run).await {
                                        Ok(r) => report.set(Some(r)),
                                        Err(e) => error.set(Some(e.to_string())),
                                    }
                                }
                                runs_rev += 1;
                                step.set(Step::Done);
                                app.success("Migration completed.");
                            }
                            (SessionStatus::Applying, _) => {
                                error.set(failure.or(Some(
                                    "The migration did not complete; nothing was changed.".into(),
                                )));
                                runs_rev += 1;
                                step.set(Step::Preview);
                            }
                            _ => {}
                        }
                        break;
                    }
                    Err(e) => {
                        error.set(Some(e.to_string()));
                        break;
                    }
                }
            }
        });
    });

    let connect_client = client.clone();
    let connect_api = move |_| {
        let client = connect_client.clone();
        error.set(None);
        busy.set(true);
        spawn(async move {
            let req = ApiSourceRequest {
                url: url(),
                api_key: api_key(),
                allow_private_network: allow_private(),
            };
            match client.migration_from_api(&req).await {
                Ok(s) => {
                    let id = s.id.clone();
                    session.set(Some(s));
                    step.set(Step::Loading);
                    wait_for.call((id, SessionStatus::Loading));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    let on_file = move |e: FormEvent| {
        let Some(file) = e.files().into_iter().next().and_then(|f| f.get_web_file()) else {
            return;
        };
        error.set(None);
        busy.set(true);
        spawn(async move {
            match upload_backup(file).await {
                Ok(s) => {
                    let id = s.id.clone();
                    session.set(Some(s));
                    step.set(Step::Loading);
                    wait_for.call((id, SessionStatus::Loading));
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    let plan_client = client.clone();
    let run_preview = use_callback(move |()| {
        let Some(id) = session.read().as_ref().map(|s| s.id.clone()) else {
            return;
        };
        let client = plan_client.clone();
        let opts = options();
        error.set(None);
        busy.set(true);
        spawn(async move {
            match client.migration_plan(&id, &opts).await {
                Ok(p) => {
                    preview.set(Some(p));
                    step.set(Step::Preview);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    });

    let apply_client = client.clone();
    let apply = move |_| {
        confirm_apply.set(false);
        let Some(id) = session.read().as_ref().map(|s| s.id.clone()) else {
            return;
        };
        let client = apply_client.clone();
        let opts = options();
        error.set(None);
        spawn(async move {
            match client.migration_apply(&id, &opts).await {
                Ok(s) => {
                    session.set(Some(s));
                    step.set(Step::Applying);
                    wait_for.call((id, SessionStatus::Applying));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let reset_client = client.clone();
    let reset = move |_| {
        if let Some(s) = session.read().as_ref() {
            let (client, id) = (reset_client.clone(), s.id.clone());
            spawn(async move {
                let _ = client.discard_migration(&id).await;
            });
        }
        session.set(None);
        preview.set(None);
        report.set(None);
        mappings.set(Vec::new());
        error.set(None);
        step.set(Step::Source);
    };

    let current = step();
    let progress = app.migration.read().clone();
    let session_now = session.read().clone();

    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Import / Migration" }
            if current != Step::Source { button { class: "btn", onclick: reset, "Start over" } }
        }
        p { class: "muted",
            "Bring your Sonarr (v3/v4) or Tanukarr library and settings into Kitsunarr. The source is only read, never changed, and "
            strong { "no media files are moved, renamed or deleted." }
        }
        ol { class: "steps", "aria-label": "Progress",
            for (i, label) in STEP_LABELS.iter().enumerate() {
                li {
                    class: if i == current.index() { "step active" } else if i < current.index() { "step done" } else { "step" },
                    "aria-current": if i == current.index() { "step" } else { "false" },
                    span { class: "n", if i < current.index() { "✓" } else { "{i + 1}" } }
                    "{label}"
                }
            }
        }
        if let Some(msg) = error() { ErrorBox { message: msg } }

        match current {
            Step::Source => rsx! {
                div { class: "choice-grid", style: "margin-bottom: var(--sp-5)",
                    button { class: if use_backup() { "choice on" } else { "choice" }, onclick: move |_| use_backup.set(true),
                        h3 { "Upload a backup" }
                        p { class: "muted small", "Recommended. In Sonarr/Tanukarr open System → Backup, create a backup and download the .zip (or use sonarr.db). Includes indexer and download-client passwords." }
                    }
                    button { class: if !use_backup() { "choice on" } else { "choice" }, onclick: move |_| use_backup.set(false),
                        h3 { "Connect to a running instance" }
                        p { class: "muted small", "Reads everything through the API. Passwords and API keys of providers are hidden by the source and must be re-entered afterwards." }
                    }
                }
                if use_backup() {
                    div { class: "card",
                        label { class: "dropzone", r#for: "backup-file", style: "display: block; cursor: pointer",
                            if busy() {
                                Loading { label: "Uploading…" }
                            } else {
                                strong { "Choose a backup file" }
                                p { class: "small", "sonarr_backup_*.zip or sonarr.db" }
                            }
                            input { id: "backup-file", class: "sr-only", r#type: "file", accept: ".zip,.db", disabled: busy(), onchange: on_file }
                        }
                    }
                } else {
                    div { class: "card",
                        div { class: "form-grid",
                            Field { label: "Address", help: "Including the URL base, e.g. http://192.168.1.10:8989 or https://host/sonarr".to_string(),
                                input { class: "input", r#type: "url", placeholder: "http://sonarr:8989", value: "{url}", oninput: move |e| url.set(e.value()) }
                            }
                            Field { label: "API key", help: "Settings → General → Security in Sonarr/Tanukarr.".to_string(),
                                input { class: "input mono", r#type: "password", autocomplete: "off", value: "{api_key}", oninput: move |e| api_key.set(e.value()) }
                            }
                        }
                        label { class: "check small", style: "margin-bottom: var(--sp-4)",
                            input { r#type: "checkbox", checked: allow_private(), onchange: move |e: FormEvent| allow_private.set(e.checked()) }
                            "The instance is on my local network (private or loopback address)"
                        }
                        div { class: "row end",
                            button { class: "btn primary", disabled: busy() || url().trim().is_empty() || api_key().trim().is_empty(), onclick: connect_api,
                                if busy() { "Connecting…" } else { "Connect" }
                            }
                        }
                    }
                }
            },
            Step::Loading => rsx! {
                div { class: "card",
                    h3 { "Reading the source" }
                    p { class: "muted", {session_now.as_ref().and_then(|s| s.progress_message.clone()).unwrap_or_else(|| "Working…".into())} }
                    if progress.total > 0 && progress.stage == "load" {
                        Progress { percent: crate::util::percent(progress.done as i64, progress.total as i64) }
                        p { class: "small muted", "{progress.message}" }
                    } else {
                        Progress { percent: 30.0, indeterminate: true }
                    }
                }
            },
            Step::Configure => rsx! {
                if let Some(s) = session_now.clone() { SourceSummary { session: s } }
                div { class: "card",
                    div { class: "card-head", h3 { class: "grow", "Path mappings" }
                        button { class: "btn sm", onclick: move |_| mappings.write().push((String::new(), String::new())), "+ Add mapping" }
                    }
                    p { class: "muted small", "If Kitsunarr sees your media under different paths than the source (for example different Docker volumes), map each source folder to its Kitsunarr location. Paths are only rewritten in the database." }
                    for (i, (src, dst)) in mappings.read().clone().into_iter().enumerate() {
                        div { key: "{i}", class: "mapping",
                            input { class: "input mono", "aria-label": "Source path", placeholder: "/tv", value: "{src}", oninput: move |e| mappings.write()[i].0 = e.value() }
                            span { class: "arrow muted", "→" }
                            input { class: "input mono", "aria-label": "Kitsunarr path", placeholder: "/media/tv", value: "{dst}", oninput: move |e| mappings.write()[i].1 = e.value() }
                            button { class: "btn ghost icon", "aria-label": "Remove mapping", onclick: move |_| { mappings.write().remove(i); }, "✕" }
                        }
                    }
                }
                div { class: "card",
                    h3 { "What to import" }
                    div { class: "form-grid", style: "margin-bottom: var(--sp-3)",
                        for g in MIGRATION_ENTITY_GROUPS.iter() {
                            label { key: "{g}", class: "check small",
                                input {
                                    r#type: "checkbox",
                                    checked: groups.read().contains(*g),
                                    onchange: move |e: FormEvent| { if e.checked() { groups.write().insert(g.to_string()); } else { groups.write().remove(*g); } },
                                }
                                {group_label(g)}
                            }
                        }
                    }
                    div { class: "stack small",
                        label { class: "check", input { r#type: "checkbox", checked: verify_paths(), onchange: move |e: FormEvent| verify_paths.set(e.checked()) } "Check that the mapped series folders exist (read-only)" }
                        label { class: "check", input { r#type: "checkbox", checked: include_history(), onchange: move |e: FormEvent| include_history.set(e.checked()) } "Import history" }
                        label { class: "check", input { r#type: "checkbox", checked: update_existing(), onchange: move |e: FormEvent| update_existing.set(e.checked()) } "Overwrite records that already exist in Kitsunarr (otherwise they are kept)" }
                    }
                }
                div { class: "row end",
                    button { class: "btn primary", disabled: busy(), onclick: move |_| run_preview.call(()), if busy() { "Preparing preview…" } else { "Preview migration" } }
                }
            },
            Step::Preview => rsx! {
                if let Some(p) = preview() {
                    PreviewView { preview: p }
                }
                div { class: "row end", style: "margin-top: var(--sp-4)",
                    button { class: "btn", onclick: move |_| step.set(Step::Configure), "‹ Back" }
                    button { class: "btn", disabled: busy(), onclick: move |_| run_preview.call(()), "Refresh preview" }
                    button {
                        class: "btn primary",
                        disabled: busy() || preview.read().as_ref().is_none_or(|p| !p.blocking.is_empty()),
                        onclick: move |_| confirm_apply.set(true),
                        "Apply migration"
                    }
                }
                if confirm_apply() {
                    Confirm {
                        title: "Apply migration",
                        message: "Kitsunarr will back up its own database, import the previewed data in one transaction and validate it before committing. The source and your media files are not modified.",
                        confirm_label: "Apply",
                        on_confirm: apply,
                        on_cancel: move |_| confirm_apply.set(false),
                    }
                }
            },
            Step::Applying => rsx! {
                div { class: "card",
                    h3 { "Applying migration" }
                    p { class: "muted", {if progress.message.is_empty() { "Working…".to_string() } else { progress.message.clone() }} }
                    Progress { percent: 30.0, indeterminate: true }
                }
            },
            Step::Done => rsx! {
                if let Some(r) = report() { ReportView { report: r } }
                div { class: "row", style: "margin-top: var(--sp-4)",
                    Link { to: Route::SeriesLibrary {}, class: "btn primary", "Open the library" }
                    Link { to: Route::Dashboard {}, class: "btn", "Dashboard" }
                }
            },
        }
        PreviousRuns { rev: runs_rev() }
    }
}

#[component]
fn SourceSummary(session: MigrationSession) -> Element {
    let Some(src) = session.source.clone() else {
        return rsx! {};
    };
    let app_name = match src.app {
        SourceApp::Sonarr => "Sonarr",
        SourceApp::Tanukarr => "Tanukarr",
    };
    let version = src
        .product_version
        .clone()
        .unwrap_or_else(|| src.app_version.clone());
    rsx! {
        div { class: "card",
            div { class: "card-head",
                h3 { class: "grow", "{app_name} {version}" }
                Badge { text: if src.method == SourceMethod::Backup { "backup".to_string() } else { "API".to_string() }, tone: "info" }
                if let Some(s) = src.schema_version { Badge { text: format!("schema {s}") } }
                if src.secrets_available { Badge { text: "secrets included", tone: "success" } } else { Badge { text: "secrets masked", tone: "warning" } }
            }
            if let Some(name) = src.instance_name.clone() { p { class: "muted small", "Instance: {name}" } }
            for (i, note) in src.notes.iter().enumerate() { div { key: "{i}", class: "alert info small", "{note}" } }
            div { class: "grid-stats", style: "margin: 0",
                for c in session.inventory.iter().filter(|c| c.count > 0) {
                    div { key: "{c.entity}", class: "stat", div { class: "value", style: "font-size: var(--fs-xl)", "{c.count}" } div { class: "label", "{c.entity}" } }
                }
            }
        }
    }
}

#[component]
fn PreviewView(preview: MigrationPreview) -> Element {
    let mut severity = use_signal(|| None::<WarningSeverity>);
    let counts = |s: WarningSeverity| preview.warnings.iter().filter(|w| w.severity == s).count();
    let (errors, warnings, infos) = (
        counts(WarningSeverity::Error),
        counts(WarningSeverity::Warning),
        counts(WarningSeverity::Info),
    );
    let shown: Vec<_> = preview
        .warnings
        .iter()
        .filter(|w| severity().is_none_or(|s| w.severity == s))
        .cloned()
        .collect();
    rsx! {
        for (i, b) in preview.blocking.iter().enumerate() { div { key: "{i}", class: "alert danger", strong { "Blocking: " } "{b}" } }
        if preview.blocking.is_empty() {
            div { class: "alert success", "The dry run completed without blocking problems. Nothing has been written yet." }
        }
        div { class: "card",
            h3 { "Changes" }
            div { class: "table-wrap",
                table { class: "table",
                    thead { tr { th { "Entity" } th { class: "num", "Create" } th { class: "num", "Update" } th { class: "num", "Keep / skip" } th { class: "num", "Not imported" } } }
                    tbody {
                        for s in preview.summary.iter() {
                            tr { key: "{s.entity}",
                                td { "{s.entity}" }
                                td { class: "num", "{s.create}" }
                                td { class: "num", "{s.update}" }
                                td { class: "num muted", "{s.skip}" }
                                td { class: if s.unsupported > 0 { "num" } else { "num muted" }, "{s.unsupported}" }
                            }
                        }
                    }
                }
            }
        }
        div { class: "card",
            div { class: "card-head",
                h3 { class: "grow", "Notes ({preview.warnings.len()})" }
                div { class: "seg",
                    button { class: if severity().is_none() { "on" } else { "" }, onclick: move |_| severity.set(None), "All" }
                    button { class: if severity() == Some(WarningSeverity::Error) { "on" } else { "" }, onclick: move |_| severity.set(Some(WarningSeverity::Error)), "Errors {errors}" }
                    button { class: if severity() == Some(WarningSeverity::Warning) { "on" } else { "" }, onclick: move |_| severity.set(Some(WarningSeverity::Warning)), "Warnings {warnings}" }
                    button { class: if severity() == Some(WarningSeverity::Info) { "on" } else { "" }, onclick: move |_| severity.set(Some(WarningSeverity::Info)), "Info {infos}" }
                }
            }
            if shown.is_empty() { p { class: "muted", "No notes." } }
            for (i, w) in shown.iter().enumerate().take(300) {
                div { key: "{i}", class: match w.severity { WarningSeverity::Error => "alert danger small", WarningSeverity::Warning => "alert warning small", WarningSeverity::Info => "alert info small" },
                    div {
                        strong { "{w.entity}" }
                        if let Some(t) = &w.title { " · {t}" }
                        div { "{w.message}" }
                    }
                }
            }
        }
        if !preview.paths.is_empty() {
            div { class: "card",
                h3 { "Media paths" }
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Title" } th { class: "hide-mobile", "Source path" } th { "Kitsunarr path" } th { "Found" } } }
                        tbody {
                            for (i, p) in preview.paths.iter().enumerate() {
                                tr { key: "{i}",
                                    td { "{p.title}" }
                                    td { class: "hide-mobile mono small", "{p.source_path}" }
                                    td { class: "mono small", "{p.target_path}" }
                                    td {
                                        match p.exists {
                                            Some(true) => rsx! { Badge { text: "yes", tone: "success" } },
                                            Some(false) => rsx! { Badge { text: "missing", tone: "danger" } },
                                            None => rsx! { Badge { text: "not checked" } },
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

#[component]
fn ReportView(report: MigrationReport) -> Element {
    let run = report.run.clone();
    let passed = report.validation.iter().all(|c| c.passed);
    rsx! {
        div { class: if run.status == "completed" { "alert success" } else { "alert danger" },
            div {
                strong { "Run #{run.id}: {run.status}" }
                div { class: "small", "Started {format_datetime(&run.started_at)}" if let Some(f) = &run.finished_at { " · finished {format_datetime(f)}" } }
                if let Some(b) = &run.backup_path { div { class: "small mono", "Database backup taken before the import: {b}" } }
            }
        }
        div { class: "card",
            div { class: "card-head", h3 { class: "grow", "Validation" } if passed { Badge { text: "all checks passed", tone: "success" } } else { Badge { text: "failed", tone: "danger" } } }
            div { class: "table-wrap",
                table { class: "table",
                    thead { tr { th { "Check" } th { "Expected" } th { "Actual" } th { "" } } }
                    tbody {
                        for c in report.validation.iter() {
                            tr { key: "{c.name}", td { "{c.name}" } td { "{c.expected}" } td { "{c.actual}" } td { if c.passed { "✓" } else { "✗" } } }
                        }
                    }
                }
            }
        }
        div { class: "card",
            h3 { "Imported" }
            div { class: "grid-stats", style: "margin: 0",
                for s in run.summary.iter().filter(|s| s.create + s.update > 0) {
                    div { key: "{s.entity}", class: "stat", div { class: "value", style: "font-size: var(--fs-xl)", "{s.create + s.update}" } div { class: "label", "{s.entity}" } }
                }
            }
        }
        if !report.warnings.is_empty() {
            div { class: "card",
                h3 { "Notes ({report.warnings.len()})" }
                for (i, w) in report.warnings.iter().enumerate().take(300) {
                    div { key: "{i}", class: "small", style: "padding: 4px 0; border-bottom: 1px solid var(--border)",
                        Badge { text: format!("{:?}", w.severity).to_lowercase(), tone: match w.severity { WarningSeverity::Error => Some("danger"), WarningSeverity::Warning => Some("warning"), WarningSeverity::Info => Some("info") } }
                        " {w.entity}"
                        if let Some(t) = &w.title { " · {t}" }
                        ": {w.message}"
                    }
                }
            }
        }
    }
}

#[component]
fn PreviousRuns(rev: u64) -> Element {
    let client = use_client();
    let mut open = use_signal(|| None::<MigrationReport>);
    let c1 = client.clone();
    let runs = use_resource(move || {
        let _ = rev;
        let c = c1.clone();
        async move { c.migration_runs().await }
    });
    let show = move |id: i64| {
        let c = client.clone();
        spawn(async move {
            if let Ok(r) = c.migration_report(id).await {
                open.set(Some(r));
            }
        });
    };
    rsx! {
        div { class: "section", style: "margin-top: var(--sp-6)",
            h2 { "Previous migrations" }
            match &*runs.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok(list)) if list.is_empty() => rsx! { p { class: "muted", "No migrations have been run yet." } },
                Some(Ok(list)) => rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr { th { "#" } th { "Date" } th { "Source" } th { "Status" } th { class: "num", "Notes" } th { "" } } }
                            tbody {
                                for r in list.iter() {
                                    tr { key: "{r.id}",
                                        td { "{r.id}" }
                                        td { class: "small", {format_datetime(&r.started_at)} }
                                        td { class: "small", "{r.source_app:?} {r.source_version} via {r.source_method:?}" }
                                        td { Badge { text: r.status.clone(), tone: if r.status == "completed" { Some("success") } else { Some("danger") } } }
                                        td { class: "num", "{r.warning_count}" }
                                        td { button { class: "btn sm", onclick: { let id = r.id; let show = show.clone(); move |_| show(id) }, "Report" } }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some(r) = open() {
            Modal { title: format!("Migration report #{}", r.run.id), wide: true, on_close: move |_| open.set(None),
                ReportView { report: r }
            }
        }
    }
}
