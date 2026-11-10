//! Wanted: missing and cutoff-unmet episodes, and searching for them.

use dioxus::prelude::*;
use kitsunarr_domain::api::wanted::{WantedList, WantedQuery, WantedSearchRequest};

use crate::components::common::{Badge, Empty, ErrorBox, Loading, Pagination};
use crate::route::Route;
use crate::state::{ToastLevel, use_app, use_client};
use crate::util::{format_date, relative};

#[component]
pub fn WantedMissing() -> Element {
    rsx! { WantedPage { list: WantedList::Missing } }
}

#[component]
pub fn WantedCutoff() -> Element {
    rsx! { WantedPage { list: WantedList::CutoffUnmet } }
}

#[component]
fn WantedPage(list: WantedList) -> Element {
    let app = use_app();
    let client = use_client();
    let mut page = use_signal(|| 1u32);
    let mut monitored = use_signal(|| true);
    let mut selected = use_signal(Vec::<i64>::new);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        let _ = (monitored(), list);
        if *page.peek() != 1 {
            page.set(1);
        }
        selected.write().clear();
    });
    let c1 = client.clone();
    let data = use_resource(move || {
        let _ = (app.library_rev)();
        let q = WantedQuery {
            page: Some(page()),
            page_size: Some(50),
            sort_direction: None,
            monitored: Some(monitored()),
        };
        let c = c1.clone();
        async move {
            match list {
                WantedList::Missing => c.wanted_missing(&q).await,
                WantedList::CutoffUnmet => c.wanted_cutoff(&q).await,
            }
        }
    });
    let mut search = move |episode_ids: Vec<i64>| {
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            let req = WantedSearchRequest {
                list,
                episode_ids,
                monitored: Some(monitored()),
            };
            match c.search_wanted(&req).await {
                Ok(started) => app.toast(ToastLevel::Info, started.message),
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let mut search_all = search.clone();
    let (title, empty_title, empty_text) = match list {
        WantedList::Missing => (
            "Missing",
            "Nothing missing",
            "Aired monitored episodes without a file appear here.",
        ),
        WantedList::CutoffUnmet => (
            "Cutoff unmet",
            "Every file meets its cutoff",
            "Episodes whose file is below the quality profile's cutoff appear here.",
        ),
    };
    let chosen = selected().len();
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Wanted" }
            nav { class: "row tight", "aria-label": "Wanted lists",
                Link { class: if list == WantedList::Missing { "btn sm primary" } else { "btn sm" }, to: Route::WantedMissing {}, "Missing" }
                Link { class: if list == WantedList::CutoffUnmet { "btn sm primary" } else { "btn sm" }, to: Route::WantedCutoff {}, "Cutoff unmet" }
            }
            select { class: "select", "aria-label": "Monitored filter",
                onchange: move |e: FormEvent| monitored.set(e.value() == "monitored"),
                option { value: "monitored", selected: monitored(), "Monitored" }
                option { value: "unmonitored", selected: !monitored(), "Unmonitored" }
            }
            button {
                class: "btn",
                disabled: busy() || chosen == 0,
                onclick: move |_| search(selected()),
                if chosen == 0 { "Search selected" } else { "Search {chosen} selected" }
            }
            button {
                class: "btn primary",
                disabled: busy(),
                title: "Search every episode of the list, one search per season",
                onclick: move |_| search_all(Vec::new()),
                "Search all"
            }
        }
        h2 { class: "small muted", "{title}" }
        match &*data.read() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
            Some(Ok(p)) if p.records.is_empty() => rsx! { Empty { title: empty_title, p { "{empty_text}" } } },
            Some(Ok(p)) => {
                let all_ids: Vec<i64> = p.records.iter().map(|e| e.episode_id).collect();
                let every = !all_ids.is_empty() && all_ids.iter().all(|id| selected().contains(id));
                rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr {
                                th { input { r#type: "checkbox", "aria-label": "Select every episode on this page", checked: every,
                                    onchange: move |e: FormEvent| {
                                        let mut s = selected.write();
                                        s.retain(|id| !all_ids.contains(id));
                                        if e.checked() { s.extend(all_ids.iter().copied()); }
                                    } } }
                                th { "Series" } th { "Episode" } th { class: "hide-mobile", "Title" } th { "Aired" }
                                if list == WantedList::CutoffUnmet { th { "Quality" } }
                                th { class: "hide-mobile", "Last search" }
                            } }
                            tbody {
                                for e in p.records.iter() {
                                    tr { key: "{e.episode_id}",
                                        td { input { r#type: "checkbox",
                                            "aria-label": "Select {e.series_title} S{e.season_number:02}E{e.episode_number:02}",
                                            checked: selected().contains(&e.episode_id),
                                            onchange: {
                                                let id = e.episode_id;
                                                move |ev: FormEvent| {
                                                    let mut s = selected.write();
                                                    s.retain(|x| *x != id);
                                                    if ev.checked() { s.push(id); }
                                                }
                                            } } }
                                        td { Link { to: Route::SeriesDetailPage { id: e.series_id }, "{e.series_title}" } }
                                        td { class: "mono", "S{e.season_number:02}E{e.episode_number:02}"
                                            if let Some(a) = e.absolute_episode_number { span { class: "muted xs", " ({a})" } } }
                                        td { class: "hide-mobile ellipsis", style: "max-width: 22rem", {e.title.clone().unwrap_or_else(|| "TBA".into())} }
                                        td { class: "small", {e.air_date_utc.as_deref().map(format_date).unwrap_or_default()} }
                                        if list == WantedList::CutoffUnmet {
                                            td { Badge { text: e.quality_name.clone().unwrap_or_default() } }
                                        }
                                        td { class: "hide-mobile small muted", {e.last_search_at.as_deref().map(relative).unwrap_or_else(|| "never".into())} }
                                    }
                                }
                            }
                        }
                    }
                    Pagination { page: p.page, page_size: p.page_size, total: p.total_records, on_page: move |n| page.set(n) }
                }
            }
        }
    }
}
