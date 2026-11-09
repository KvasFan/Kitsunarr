//! System pages: status and health, scheduled tasks, backups and logs.

use dioxus::prelude::*;
use kitsunarr_domain::api::system::{HealthLevel, LogEntry};

use crate::components::common::{Badge, Empty, ErrorBox, Loading};
use crate::state::{use_app, use_client};
use crate::util::{format_bytes, format_datetime, relative};

#[component]
pub fn SystemStatusPage() -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let status = use_resource(move || {
        let c = c0.clone();
        async move { c.system_status().await }
    });
    let health = use_resource(move || {
        let _ = (app.health_rev)();
        let _ = (app.settings_rev)();
        let c = client.clone();
        async move { c.health().await }
    });
    rsx! {
        div { class: "page-head", h1 { "Status & health" } }
        div { class: "two-col",
            div { class: "card",
                h3 { "Health" }
                match &*health.read() {
                    None => rsx! { Loading {} },
                    Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                    Some(Ok(list)) if list.is_empty() => rsx! { p { "✓ No issues detected." } },
                    Some(Ok(list)) => rsx! {
                        for (i, h) in list.iter().enumerate() {
                            div { key: "{i}", class: match h.level { HealthLevel::Error => "alert danger", HealthLevel::Warning => "alert warning", _ => "alert info" },
                                div {
                                    strong { class: "small", "{h.source}" }
                                    div { "{h.message}" }
                                }
                            }
                        }
                    },
                }
            }
            div { class: "card",
                h3 { "About" }
                match &*status.read() {
                    None => rsx! { Loading {} },
                    Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                    Some(Ok(s)) => rsx! {
                        dl { class: "kv",
                            dt { "Version" } dd { "{s.version}" if let Some(r) = &s.git_revision { span { class: "faint", " ({r})" } } }
                            dt { "Instance" } dd { "{s.instance_name}" }
                            dt { "Started" } dd { {format_datetime(&s.start_time)} " (" {relative(&s.start_time)} ")" }
                            dt { "Platform" } dd { "{s.os}/{s.arch}" if s.is_container { " · container" } }
                            dt { "Data directory" } dd { class: "mono", "{s.data_directory}" }
                            dt { "URL base" } dd { if s.url_base.is_empty() { "/" } else { "{s.url_base}" } }
                            dt { "Database schema" } dd { "{s.database_schema_version}" }
                            dt { "SQLite" } dd { "{s.sqlite_version}" }
                        }
                        p { class: "small faint", style: "margin-top: var(--sp-4)",
                            "Kitsunarr is free software (GPL-3.0-or-later). "
                            a { href: "https://github.com/KvasFan/Kitsunarr", target: "_blank", rel: "noopener", "Source code" }
                            " · API: "
                            a { href: "{crate::util::url_base()}/api/v1/openapi.json", target: "_blank", "OpenAPI document" }
                        }
                    },
                }
            }
        }
    }
}

#[component]
pub fn SystemTasks() -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut tasks = use_resource(move || {
        let _ = (app.tasks_rev)();
        let c = c0.clone();
        async move { c.tasks().await }
    });
    let c1 = client.clone();
    let runs = use_resource(move || {
        let _ = (app.tasks_rev)();
        let c = c1.clone();
        async move { c.task_runs().await }
    });
    let run = move |name: String| {
        let c = client.clone();
        spawn(async move {
            match c.run_task(&name).await {
                Ok(()) => {
                    app.success(format!("{name} started."));
                    tasks.restart();
                }
                Err(e) => app.error(&e),
            }
        });
    };
    rsx! {
        div { class: "page-head", h1 { "Scheduled tasks" } }
        match &*tasks.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) => rsx! {
                div { class: "table-wrap", style: "margin-bottom: var(--sp-5)",
                    table { class: "table",
                        thead { tr { th { "Task" } th { "Interval" } th { "Last run" } th { "Duration" } th { "Next run" } th { "" } } }
                        tbody {
                            for t in list.iter() {
                                tr { key: "{t.name}",
                                    td { strong { "{t.name}" } div { class: "xs muted", "{t.description}" } }
                                    td { {format_interval(t.interval_minutes)} }
                                    td {
                                        {t.last_run_at.as_deref().map(relative).unwrap_or_else(|| "never".into())}
                                        if let Some(s) = &t.last_status { " " Badge { text: s.clone(), tone: if s == "completed" { Some("success") } else { Some("danger") } } }
                                    }
                                    td { {t.last_duration_ms.map(|ms| format!("{:.1} s", ms as f64 / 1000.0)).unwrap_or_default()} }
                                    td { {t.next_run_at.as_deref().map(relative).unwrap_or_default()} }
                                    td { class: "shrink",
                                        button { class: "btn sm", disabled: t.running, onclick: { let n = t.name.clone(); let run = run.clone(); move |_| run(n.clone()) },
                                            if t.running { "Running…" } else { "Run now" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
        h2 { "Recent runs" }
        match &*runs.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "No runs yet" } },
            Some(Ok(list)) => rsx! {
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Task" } th { "Trigger" } th { "Status" } th { "Started" } th { "Message" } } }
                        tbody {
                            for r in list.iter().take(50) {
                                tr { key: "{r.id}",
                                    td { "{r.task_name}" }
                                    td { class: "muted", "{r.trigger}" }
                                    td { Badge { text: r.status.clone(), tone: match r.status.as_str() { "completed" => Some("success"), "running" => Some("info"), _ => Some("danger") } } }
                                    td { class: "small", {format_datetime(&r.started_at)} }
                                    td { class: "small muted", {r.message.clone().unwrap_or_default()} }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}

fn format_interval(minutes: i64) -> String {
    match minutes {
        m if m % (24 * 60 * 7) == 0 => format!("{} week(s)", m / (24 * 60 * 7)),
        m if m % (24 * 60) == 0 => format!("{} day(s)", m / (24 * 60)),
        m if m % 60 == 0 => format!("{} h", m / 60),
        m => format!("{m} min"),
    }
}

#[component]
pub fn SystemBackups() -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut backups = use_resource(move || {
        let c = c0.clone();
        async move { c.backups().await }
    });
    let mut busy = use_signal(|| false);
    let create = move |_| {
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            match c.create_backup().await {
                Ok(b) => {
                    app.success(format!("Backup {} created.", b.name));
                    backups.restart();
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Backups" }
            button { class: "btn primary", disabled: busy(), onclick: create, if busy() { "Creating…" } else { "Back up now" } }
        }
        p { class: "muted small",
            "Consistent copies of the database. Scheduled backups run weekly (the last eight are kept); every migration also creates one first. To restore, stop Kitsunarr and run "
            code { "kitsunarr restore <file>" }
            "."
        }
        match &*backups.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "No backups yet" } },
            Some(Ok(list)) => rsx! {
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "File" } th { class: "num", "Size" } th { "Created" } } }
                        tbody {
                            for b in list.iter() {
                                tr { key: "{b.name}",
                                    td { class: "mono small", "{b.name}" }
                                    td { class: "num", {format_bytes(b.size as i64)} }
                                    td { {format_datetime(&b.created_at)} }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}

#[component]
pub fn SystemLogs() -> Element {
    let client = use_client();
    let mut level = use_signal(|| "info".to_string());
    let mut tick = use_signal(|| 0u64);
    let mut auto = use_signal(|| true);
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(5000).await;
            if auto() {
                tick += 1;
            }
        }
    });
    let logs = use_resource(move || {
        let _ = tick();
        let lvl = level();
        let c = client.clone();
        async move {
            c.get_query::<Vec<LogEntry>, _>("/system/logs", &[("level", lvl)])
                .await
        }
    });
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Logs" }
            label { class: "check small", input { r#type: "checkbox", checked: auto(), onchange: move |e: FormEvent| auto.set(e.checked()) } "Auto-refresh" }
            select { class: "select", "aria-label": "Minimum level", value: "{level}", onchange: move |e: FormEvent| level.set(e.value()),
                option { value: "error", "Errors" } option { value: "warn", "Warnings" } option { value: "info", "Info" } option { value: "debug", "Debug" }
            }
        }
        p { class: "muted small", "The most recent entries kept in memory. Full logs go to the process output (container logs or journal)." }
        match &*logs.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(list)) if list.is_empty() => rsx! { Empty { title: "No log entries at this level" } },
            Some(Ok(list)) => rsx! {
                div { class: "table-wrap",
                    table { class: "table",
                        thead { tr { th { "Time" } th { "Level" } th { class: "hide-mobile", "Component" } th { "Message" } } }
                        tbody {
                            for (i, l) in list.iter().enumerate() {
                                tr { key: "{i}",
                                    td { class: "small mono", style: "white-space: nowrap", {format_datetime(&l.time)} }
                                    td { Badge { text: l.level.to_lowercase(), tone: match l.level.as_str() { "ERROR" => Some("danger"), "WARN" => Some("warning"), "DEBUG" | "TRACE" => None, _ => Some("info") } } }
                                    td { class: "small muted hide-mobile", "{l.target}" }
                                    td { class: "small mono", style: "white-space: pre-wrap; overflow-wrap: anywhere", "{l.message}" }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}
