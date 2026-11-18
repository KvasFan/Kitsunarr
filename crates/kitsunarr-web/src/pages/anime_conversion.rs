//! Converting TheTVDB anime into Tanukarr-style AniList group members: the plan (a dry run) is
//! shown first, and only a confirmation carries it out.

use dioxus::prelude::*;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::library::{AnimeConversionPlan, SeriesDetail};

use crate::components::common::{Badge, ErrorBox, Loading, Modal};
use crate::route::Route;
use crate::state::{ToastLevel, use_app, use_client};

/// Whether the series can be offered the conversion: a TheTVDB series with AniList IDs.
pub fn eligible(d: &SeriesDetail) -> bool {
    d.item.metadata_provider == MetadataProvider::Tvdb
        && d.external_ids
            .iter()
            .any(|x| x.kind == kitsunarr_domain::ExternalIdKind::AniList)
}

/// One series' plan: its members, the files to move, what is left out and why it may not
/// convert.
#[component]
fn PlanView(plan: AnimeConversionPlan) -> Element {
    rsx! {
        if let Some(g) = plan.group_title.as_ref() { p { "Group: {g}" } }
        if !plan.problems.is_empty() {
            div { class: "alert danger", role: "alert",
                strong { "This series cannot be converted:" }
                ul { for (i, problem) in plan.problems.iter().enumerate() { li { key: "{i}", class: "small", "{problem}" } } }
            }
        }
        if !plan.members.is_empty() {
            table { class: "table",
                thead {
                    tr {
                        th { "Season" }
                        th { "Title" }
                        th { "Folder" }
                        th { "TheTVDB episodes" }
                        th { "Files" }
                        th { "Monitored" }
                    }
                }
                tbody {
                    for m in plan.members.iter() {
                        tr { key: "{m.anilist_id}",
                            td { "{m.ordinal}" }
                            td {
                                "{m.title} "
                                if m.existing_series_id.is_some() { Badge { text: "already in the library".to_string(), tone: "info" } }
                            }
                            td { class: "mono small", "{m.path}" }
                            td { class: "mono small", "{m.source_episodes}" }
                            td { "{m.files}" }
                            td { "{m.monitored_episodes}" }
                        }
                    }
                }
            }
        }
        if !plan.moves.is_empty() {
            details {
                summary { "Files to move ({plan.moves.len()})" }
                ul { class: "small mono",
                    for (i, mv) in plan.moves.iter().enumerate() { li { key: "{i}", "{mv.from} → {mv.to}" } }
                }
            }
        }
        if !plan.skipped.is_empty() {
            details {
                summary { "Left out ({plan.skipped.len()})" }
                ul { for (i, s) in plan.skipped.iter().enumerate() { li { key: "{i}", class: "small", "{s}" } } }
            }
        }
        if !plan.warnings.is_empty() {
            div { class: "alert warning", role: "status",
                strong { "Warnings" }
                ul { for (i, w) in plan.warnings.iter().enumerate() { li { key: "{i}", class: "small", "{w}" } } }
            }
        }
    }
}

/// The series detail page's "Convert to AniList group": the plan, then the conversion.
#[component]
pub fn AnimeConversionDialog(series_id: i64, on_close: EventHandler<()>) -> Element {
    let mut app = use_app();
    let client = use_client();
    let nav = navigator();
    let mut busy = use_signal(|| false);
    let mut outcome = use_signal(|| None::<AnimeConversionPlan>);
    let c0 = client.clone();
    let plan = use_resource(move || {
        let c = c0.clone();
        async move { c.convert_to_anime_group(series_id, true).await }
    });
    let convert = move |_| {
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            match c.convert_to_anime_group(series_id, false).await {
                Ok(done) if done.applied => {
                    app.library_rev += 1;
                    app.success(format!(
                        "Converted into {} AniList series.",
                        done.members.len()
                    ));
                    on_close.call(());
                    nav.push(Route::AnimeLibrary {});
                }
                Ok(done) => outcome.set(Some(done)),
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let shown = match (outcome(), &*plan.read()) {
        (Some(done), _) => Some(Ok(done)),
        (None, Some(Ok(p))) => Some(Ok(p.clone())),
        (None, Some(Err(e))) => Some(Err(e.to_string())),
        (None, None) => None,
    };
    let ready = matches!(&shown, Some(Ok(p)) if p.convertible && p.problems.is_empty());
    rsx! {
        Modal {
            title: "Convert to AniList group".to_string(),
            wide: true,
            on_close: move |_| { if !busy() { on_close.call(()) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || !ready, onclick: convert,
                    if busy() { "Converting…" } else { "Convert" }
                }
            },
            p { class: "help", "Each AniList season you follow (with files or monitored episodes) becomes its own series inside this series' folder, and its files move into the season's folder. Review the plan, then convert." }
            match shown {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e } },
                Some(Ok(p)) => rsx! { PlanView { plan: p } },
            }
        }
    }
}

/// The library's bulk "Convert to AniList groups": every selected series is planned, and the
/// convertible ones are converted on confirmation, each reported on its own.
#[component]
pub fn BulkAnimeConversionDialog(series_ids: Vec<i64>, on_close: EventHandler<bool>) -> Element {
    let mut app = use_app();
    let client = use_client();
    let mut busy = use_signal(|| false);
    let mut results = use_signal(|| None::<Vec<AnimeConversionPlan>>);
    let count = series_ids.len();
    let c0 = client.clone();
    let ids = series_ids.clone();
    let plans = use_resource(move || {
        let (c, ids) = (c0.clone(), ids.clone());
        async move { c.bulk_convert_to_anime_group(ids, true).await }
    });
    let convertible: Vec<i64> = match &*plans.read() {
        Some(Ok(r)) => r
            .results
            .iter()
            .filter(|p| p.convertible)
            .map(|p| p.series_id)
            .collect(),
        _ => Vec::new(),
    };
    let n = convertible.len();
    let convert = move |_| {
        let c = client.clone();
        let ids = convertible.clone();
        busy.set(true);
        spawn(async move {
            match c.bulk_convert_to_anime_group(ids, false).await {
                Ok(r) => {
                    let done = r.results.iter().filter(|p| p.applied).count();
                    let failed = r.results.len() - done;
                    app.library_rev += 1;
                    app.toast(
                        if failed == 0 {
                            ToastLevel::Success
                        } else {
                            ToastLevel::Warning
                        },
                        format!("{done} series converted, {failed} not converted."),
                    );
                    results.set(Some(r.results));
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let applied = results().is_some();
    let rows: Option<Result<Vec<AnimeConversionPlan>, String>> = match (results(), &*plans.read()) {
        (Some(r), _) => Some(Ok(r)),
        (None, Some(Ok(r))) => Some(Ok(r.results.clone())),
        (None, Some(Err(e))) => Some(Err(e.to_string())),
        (None, None) => None,
    };
    rsx! {
        Modal {
            title: format!("Convert {count} series to AniList groups"),
            wide: true,
            on_close: move |_| { if !busy() { on_close.call(applied) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(applied), if applied { "Close" } else { "Cancel" } }
                if !applied {
                    button { class: "btn primary", disabled: busy() || n == 0, onclick: convert,
                        if busy() { "Converting…" } else { "Convert {n} series" }
                    }
                }
            },
            p { class: "help", "In each series, every AniList season you follow becomes its own series inside the series' folder, with its files moved there. Series that cannot be converted are left as they are." }
            match rows {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e } },
                Some(Ok(list)) => rsx! {
                    if !applied && n == 0 {
                        div { class: "alert info", role: "status", "None of the selected series can be converted." }
                    }
                    div { class: "stack",
                        for p in list {
                            details { key: "{p.series_id}",
                                summary {
                                    strong { "{p.title}" }
                                    " · "
                                    if p.applied {
                                        span { "Converted" }
                                    } else if let Some(problem) = p.problems.first() {
                                        span { class: "warning", "Cannot be converted: {problem}" }
                                    } else {
                                        span { "{p.members.len()} seasons, {p.moves.len()} files to move" }
                                    }
                                }
                                PlanView { plan: p.clone() }
                            }
                        }
                    }
                },
            }
        }
    }
}
