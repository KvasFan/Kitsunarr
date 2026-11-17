//! Library import (Sonarr's Add Series › Library Import): the folders of a root folder that no
//! title uses are matched against a catalogue, then added with their existing folders. Their
//! files are scanned in place before the monitoring choice applies.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use kitsunarr_domain::api::library::UnmappedFolder;
use kitsunarr_domain::api::metadata::{AddTitleRequest, MonitorOption, SearchItem, SearchQuery};
use kitsunarr_domain::{MetadataProvider, SeriesType};

use crate::components::common::{Empty, ErrorBox, Field, Loading};
use crate::route::Route;
use crate::state::{use_app, use_client};

/// One folder's matching and import state.
#[derive(Debug, Clone, PartialEq, Default)]
struct Row {
    /// `None` while the catalogue search is pending.
    matches: Option<Result<Vec<SearchItem>, String>>,
    /// The chosen catalogue entry's id.
    chosen: Option<String>,
    selected: bool,
    outcome: Option<Result<String, String>>,
}

fn titles(n: usize) -> String {
    if n == 1 {
        "1 title".into()
    } else {
        format!("{n} titles")
    }
}

/// The best catalogue match for a folder: the same year when the folder names one, else the
/// first result; titles already in the library are never chosen.
fn best_match(folder: &UnmappedFolder, items: &[SearchItem]) -> Option<String> {
    let free = || items.iter().filter(|i| i.in_library.is_none());
    folder
        .year
        .and_then(|y| free().find(|i| i.year == Some(y)))
        .or_else(|| free().next())
        .map(|i| i.id.clone())
}

#[component]
pub fn LibraryImport() -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let setup = use_resource(move || {
        let c = c0.clone();
        async move {
            let roots = c.root_folders().await?;
            let profiles = c.quality_profiles().await?;
            let catalogues = c.metadata_providers().await?;
            Ok::<_, kitsunarr_client::ClientError>((roots, profiles, catalogues))
        }
    });
    let mut root_id = use_signal(|| None::<i64>);
    let mut provider = use_signal(|| MetadataProvider::Tvdb);
    let mut monitor = use_signal(|| MonitorOption::All);
    let mut profile_id = use_signal(|| None::<i64>);
    let mut series_type = use_signal(|| None::<SeriesType>);
    let mut search_missing = use_signal(|| false);
    let mut rows = use_signal(BTreeMap::<String, Row>::new);
    let mut busy = use_signal(|| false);
    // Each matching run's number; an older run stops when a newer one starts.
    let mut generation = use_signal(|| 0u64);

    let c1 = client.clone();
    let mut folders = use_resource(move || {
        let root = root_id();
        let c = c1.clone();
        async move {
            match root {
                Some(id) => c.unmapped_folders(id).await.map(Some),
                None => Ok(None),
            }
        }
    });
    // Match the folders one after another (catalogues limit request rates).
    let c2 = client.clone();
    use_effect(move || {
        let list = match &*folders.read() {
            Some(Ok(Some(list))) => list.clone(),
            _ => return,
        };
        let catalogue = provider();
        let run = *generation.peek() + 1;
        generation.set(run);
        rows.set(
            list.iter()
                .map(|f| (f.path.clone(), Row::default()))
                .collect(),
        );
        let c = c2.clone();
        spawn(async move {
            for folder in list {
                let query = SearchQuery {
                    provider: Some(catalogue),
                    term: folder.search_term.clone(),
                    all_sources: false,
                };
                let result = if folder.search_term.trim().is_empty() {
                    Ok(Vec::new())
                } else {
                    c.metadata_search(&query).await.map_err(|e| e.to_string())
                };
                // A newer folder list or catalogue replaced this run.
                if *generation.peek() != run {
                    return;
                }
                let chosen = result
                    .as_ref()
                    .ok()
                    .and_then(|items| best_match(&folder, items));
                if let Some(row) = rows.write().get_mut(&folder.path) {
                    row.selected = chosen.is_some();
                    row.chosen = chosen;
                    row.matches = Some(result);
                }
            }
        });
    });

    let read = setup.read();
    let (roots, profiles, catalogues) = match &*read {
        None => return rsx! { div { class: "page-head", h1 { "Library import" } } Loading {} },
        Some(Err(e)) => {
            return rsx! { div { class: "page-head", h1 { "Library import" } } ErrorBox { message: e.to_string() } };
        }
        Some(Ok(v)) => v.clone(),
    };
    drop(read);
    if root_id.peek().is_none()
        && let Some(r) = roots.first()
    {
        root_id.set(Some(r.id));
    }
    if profile_id.peek().is_none()
        && let Some(p) = profiles.first()
    {
        profile_id.set(Some(p.id));
    }
    let root_path = roots
        .iter()
        .find(|r| Some(r.id) == root_id())
        .map(|r| r.path.clone())
        .unwrap_or_default();
    let folder_list: Vec<UnmappedFolder> = match &*folders.read() {
        Some(Ok(Some(list))) => list.clone(),
        _ => Vec::new(),
    };
    let selected_count = rows
        .read()
        .values()
        .filter(|r| r.selected && r.chosen.is_some() && r.outcome.is_none())
        .count();

    let import = move |_| {
        let Some(profile) = profile_id() else { return };
        let todo: Vec<(String, String)> = rows
            .read()
            .iter()
            .filter(|(_, r)| r.selected && r.outcome.is_none())
            .filter_map(|(path, r)| Some((path.clone(), r.chosen.clone()?)))
            .collect();
        let (catalogue, root, choice, kind, search) = (
            provider(),
            root_path.clone(),
            monitor(),
            series_type(),
            search_missing(),
        );
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            let (mut done, mut failed) = (0usize, 0usize);
            for (path, id) in todo {
                let request = AddTitleRequest {
                    provider: catalogue,
                    id,
                    root_folder_path: root.clone(),
                    quality_profile_id: profile,
                    monitor: choice,
                    series_type: kind,
                    season_folder: true,
                    tags: vec![],
                    search_for_missing: search,
                    episode_ordering: None,
                    path: Some(path.clone()),
                };
                let outcome = match c.add_title(&request).await {
                    Ok(r) => {
                        done += 1;
                        Ok(format!("{}:{}", r.media_type, r.id))
                    }
                    Err(e) => {
                        failed += 1;
                        Err(e.to_string())
                    }
                };
                if let Some(row) = rows.write().get_mut(&path) {
                    row.outcome = Some(outcome);
                }
            }
            busy.set(false);
            if failed == 0 {
                app.success(format!("{} imported.", titles(done)));
            } else {
                app.toast(
                    crate::state::ToastLevel::Warning,
                    format!(
                        "{} imported, {failed} failed; see each folder.",
                        titles(done)
                    ),
                );
            }
        });
    };

    rsx! {
        div { class: "page-head",
            h1 { "Library import" }
            div { class: "spacer" }
            Link { to: Route::AddMedia {}, class: "btn", "Search catalogues" }
        }
        p { class: "help",
            "Folders of a root folder that no title uses. Each is matched against the catalogue by its name (or the "
            code { "[tvdbid-…]" }
            ", "
            code { "{{tmdb-…}}" }
            " or "
            code { "[imdbid-…]" }
            " tag in it); imported titles keep their folder, their files are scanned in place, then the monitoring choice applies. Nothing is moved or renamed."
        }
        if roots.is_empty() {
            Empty { title: "No root folder",
                p { "Add a root folder in Settings › Media management first." }
                Link { to: Route::SettingsMedia {}, class: "btn primary", "Media management" }
            }
        } else {
            div { class: "card",
                div { class: "form-grid",
                    Field { label: "Root folder",
                        select { class: "select", "aria-label": "Root folder", value: "{root_id().unwrap_or_default()}",
                            onchange: move |e: FormEvent| root_id.set(e.value().parse().ok()),
                            for r in roots.iter() { option { key: "{r.id}", value: "{r.id}", "{r.path}" } }
                        }
                    }
                    Field { label: "Catalogue",
                        select { class: "select", "aria-label": "Catalogue", value: provider().as_str(),
                            onchange: move |e: FormEvent| {
                                if let Ok(p) = e.value().parse::<MetadataProvider>() {
                                    provider.set(p);
                                }
                            },
                            for c in catalogues.iter() {
                                option { key: "{c.provider}", value: c.provider.as_str(), disabled: !c.available,
                                    if c.available { "{c.name}" } else { "{c.name} (API key needed)" }
                                }
                            }
                        }
                    }
                    Field { label: "Monitor",
                        select { class: "select", "aria-label": "Monitor", value: monitor().as_str(),
                            onchange: move |e: FormEvent| {
                                if let Some(m) = MonitorOption::from_sonarr(&e.value()) {
                                    monitor.set(m);
                                }
                            },
                            for m in super::add::SERIES_MONITOR.iter() { option { key: "{m.as_str()}", value: m.as_str(), "{m.label()}" } }
                        }
                    }
                    Field { label: "Quality profile",
                        select { class: "select", "aria-label": "Quality profile", value: "{profile_id().unwrap_or_default()}",
                            onchange: move |e: FormEvent| profile_id.set(e.value().parse().ok()),
                            for p in profiles.iter() { option { key: "{p.id}", value: "{p.id}", "{p.name}" } }
                        }
                    }
                    Field { label: "Series type",
                        select { class: "select", "aria-label": "Series type",
                            value: series_type().map(|t| t.as_str()).unwrap_or(""),
                            onchange: move |e: FormEvent| series_type.set(e.value().parse::<SeriesType>().ok()),
                            option { value: "", "As the catalogue says" }
                            option { value: "standard", "Standard" }
                            option { value: "daily", "Daily" }
                            option { value: "anime", "Anime / absolute numbering" }
                        }
                    }
                }
                label { class: "check",
                    input { r#type: "checkbox", checked: search_missing(), onchange: move |e: FormEvent| search_missing.set(e.checked()) }
                    "Search for missing episodes once imported"
                }
            }
            match &*folders.read() {
                None => rsx! { Loading { label: "Reading the root folder…" } },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string(), on_retry: move |_| folders.restart() } },
                Some(Ok(_)) if folder_list.is_empty() => rsx! {
                    Empty { title: "Nothing to import",
                        p { "Every folder of this root folder belongs to a title already." }
                    }
                },
                Some(Ok(_)) => rsx! {
                    div { class: "table-wrap", style: "margin-top: var(--sp-3)",
                        table { class: "table", "aria-label": "Unmapped folders",
                            thead { tr {
                                th { class: "check", span { class: "sr-only", "Import" } }
                                th { "Folder" }
                                th { "Catalogue match" }
                                th { "Result" }
                            } }
                            tbody {
                                for folder in folder_list.iter() {
                                    FolderRow { key: "{folder.path}", folder: folder.clone(), rows, busy: busy() }
                                }
                            }
                        }
                    }
                    div { class: "row", style: "margin-top: var(--sp-3)",
                        span { class: "muted", "{selected_count} of {folder_list.len()} folders selected" }
                        div { class: "spacer" }
                        button { class: "btn", disabled: busy(), onclick: move |_| folders.restart(), "Reload folders" }
                        button { class: "btn primary", disabled: busy() || selected_count == 0 || profile_id().is_none(), onclick: import,
                            {if busy() { "Importing…".to_string() } else { format!("Import {}", titles(selected_count)) }}
                        }
                    }
                },
            }
        }
    }
}

#[component]
fn FolderRow(folder: UnmappedFolder, rows: Signal<BTreeMap<String, Row>>, busy: bool) -> Element {
    let row = rows.read().get(&folder.path).cloned().unwrap_or_default();
    let path = folder.path.clone();
    let path2 = folder.path.clone();
    let done = row.outcome.as_ref().is_some_and(Result::is_ok);
    rsx! {
        tr {
            td { class: "check",
                input { r#type: "checkbox", "aria-label": "Import {folder.name}",
                    checked: row.selected,
                    disabled: busy || done || row.chosen.is_none(),
                    onchange: move |e: FormEvent| {
                        if let Some(r) = rows.write().get_mut(&path) {
                            r.selected = e.checked();
                        }
                    },
                }
            }
            td {
                div { "{folder.name}" }
                div { class: "small muted mono", "{folder.relative_path}" }
            }
            td {
                match &row.matches {
                    None => rsx! { span { class: "muted", "Searching “{folder.search_term}”…" } },
                    Some(Err(e)) => rsx! { span { class: "small error-text", "{e}" } },
                    Some(Ok(items)) if items.is_empty() => rsx! { span { class: "muted", "No match for “{folder.search_term}”" } },
                    Some(Ok(items)) => rsx! {
                        select { class: "select", "aria-label": "Match for {folder.name}",
                            disabled: busy || done,
                            value: row.chosen.clone().unwrap_or_default(),
                            onchange: move |e: FormEvent| {
                                if let Some(r) = rows.write().get_mut(&path2) {
                                    let v = e.value();
                                    r.selected = !v.is_empty();
                                    r.chosen = Some(v).filter(|v| !v.is_empty());
                                }
                            },
                            option { value: "", selected: row.chosen.is_none(), "Don't import" }
                            for i in items.iter() {
                                option { key: "{i.id}", value: "{i.id}", disabled: i.in_library.is_some(),
                                    selected: row.chosen.as_deref() == Some(i.id.as_str()),
                                    {format!("{}{}{}", i.title, i.year.map(|y| format!(" ({y})")).unwrap_or_default(), if i.in_library.is_some() { " — in library" } else { "" })}
                                }
                            }
                        }
                    },
                }
            }
            td {
                match &row.outcome {
                    None => rsx! { span { class: "muted", "—" } },
                    Some(Ok(_)) => rsx! { span { class: "badge success", "Imported" } },
                    Some(Err(e)) => rsx! { span { class: "small error-text", "{e}" } },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kitsunarr_domain::SeriesStatus;
    use kitsunarr_domain::api::metadata::EntryKind;

    fn item(id: &str, year: Option<i32>, in_library: bool) -> SearchItem {
        SearchItem {
            provider: MetadataProvider::Tvdb,
            id: id.into(),
            kind: EntryKind::Series,
            title: "T".into(),
            year,
            overview: None,
            network: None,
            status: SeriesStatus::Ended,
            poster_url: None,
            external_ids: vec![],
            in_library: in_library.then(|| kitsunarr_domain::api::metadata::LibraryRef {
                media_type: "series".into(),
                id: 1,
            }),
        }
    }

    #[test]
    fn the_folder_year_picks_the_match() {
        let folder = UnmappedFolder {
            name: "T (2005)".into(),
            path: "/tv/T (2005)".into(),
            relative_path: "T (2005)".into(),
            search_term: "T".into(),
            year: Some(2005),
        };
        let items = [item("1", Some(1963), false), item("2", Some(2005), false)];
        assert_eq!(best_match(&folder, &items).as_deref(), Some("2"));
        let taken = [item("1", Some(2005), true), item("2", Some(1963), false)];
        assert_eq!(best_match(&folder, &taken).as_deref(), Some("2"));
        assert_eq!(best_match(&folder, &[item("1", None, true)]), None);
    }
}
