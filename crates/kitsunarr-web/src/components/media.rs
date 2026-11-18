//! Library presentation: poster cards, tables and the overview list, with Tanukarr's index
//! columns chosen per browser.

use std::collections::HashSet;

use dioxus::prelude::*;
use kitsunarr_domain::api::library::{MovieListItem, SeriesListItem};
use kitsunarr_domain::api::settings::TagItem;

use crate::components::common::{Badge, Progress};
use crate::route::Route;
use crate::util::{format_bytes, format_date, initials, load_pref, percent, save_pref};

/// A column of Tanukarr's library index: key, label, shown by default, sort key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    pub key: &'static str,
    pub label: &'static str,
    pub default: bool,
    pub sort: Option<&'static str>,
}

const fn col(
    key: &'static str,
    label: &'static str,
    default: bool,
    sort: Option<&'static str>,
) -> Column {
    Column {
        key,
        label,
        default,
        sort,
    }
}

/// Tanukarr's series index columns (`seriesOptionsStore`).
pub const SERIES_COLUMNS: &[Column] = &[
    col("status", "Status", true, Some("status")),
    col("seriesType", "Type", false, None),
    col("network", "Network", true, Some("network")),
    col("qualityProfile", "Quality profile", true, None),
    col("nextAiring", "Next airing", true, Some("nextAiring")),
    col("previousAiring", "Previous airing", false, None),
    col("added", "Added", true, Some("added")),
    col("seasonCount", "Seasons", true, None),
    col("seasonFolder", "Season folder", false, None),
    col("episodeProgress", "Episodes", true, None),
    col("year", "Year", false, Some("year")),
    col("path", "Path", false, None),
    col("sizeOnDisk", "Size on disk", true, Some("size")),
    col("averageSize", "Average episode size", false, None),
    col("genres", "Genres", false, None),
    col("ratings", "Rating", false, None),
    col("certification", "Certification", false, None),
    col("releaseGroups", "Release groups", false, None),
    col("tags", "Tags", false, None),
];

/// The movie library's columns.
pub const MOVIE_COLUMNS: &[Column] = &[
    col("year", "Year", true, None),
    col("studio", "Studio", false, None),
    col("qualityProfile", "Quality profile", true, None),
    col("file", "File", true, None),
    col("fileQuality", "File quality", false, None),
    col("watched", "Watched", true, None),
    col("sizeOnDisk", "Size on disk", true, None),
    col("added", "Added", true, None),
    col("inCinemas", "In cinemas", false, None),
    col("runtime", "Runtime", false, None),
    col("genres", "Genres", false, None),
    col("ratings", "Rating", false, None),
    col("certification", "Certification", false, None),
    col("releaseGroups", "Release group", false, None),
    col("path", "Path", false, None),
    col("tags", "Tags", false, None),
];

/// The view of a library page: `posters`, `table` or `overview` (kept per browser).
pub fn use_view() -> Signal<String> {
    let view = use_signal(|| match load_pref("kitsunarr-view").as_deref() {
        Some("table") => "table".to_string(),
        Some("overview") => "overview".to_string(),
        _ => "posters".to_string(),
    });
    use_effect(move || save_pref("kitsunarr-view", &view()));
    view
}

/// The chosen columns of a library (`series`, `anime`, `movies`), kept per browser.
pub fn use_columns(library: &'static str, all: &'static [Column]) -> Signal<Vec<String>> {
    let key = format!("library.{library}.columns");
    let k0 = key.clone();
    let columns = use_signal(move || -> Vec<String> {
        match load_pref(&k0) {
            Some(saved) => saved
                .split(',')
                .filter(|c| all.iter().any(|x| x.key == *c))
                .map(str::to_string)
                .collect(),
            None => all
                .iter()
                .filter(|c| c.default)
                .map(|c| c.key.to_string())
                .collect(),
        }
    });
    use_effect(move || save_pref(&key, &columns().join(",")));
    columns
}

/// Posters, table or overview.
#[component]
pub fn ViewSwitch(view: Signal<String>) -> Element {
    rsx! {
        div { class: "seg", role: "group", "aria-label": "View",
            for (v, label) in [("posters", "Posters"), ("table", "Table"), ("overview", "Overview")] {
                button { key: "{v}", class: if view() == v { "on" } else { "" }, "aria-pressed": if view() == v { "true" } else { "false" },
                    onclick: move |_| view.set(v.to_string()), "{label}" }
            }
        }
    }
}

/// Tanukarr's column chooser (table options), also deciding what posters and the overview show.
#[component]
pub fn ColumnChooser(all: &'static [Column], columns: Signal<Vec<String>>) -> Element {
    rsx! {
        details { class: "combine column-chooser",
            summary { class: "btn sm", "Columns" }
            fieldset { class: "menu stack tight",
                legend { class: "small muted", "Shown in the table, the overview and under posters" }
                for c in all.iter() {
                    {
                        let key = c.key;
                        let on = columns().iter().any(|x| x == key);
                        rsx! {
                            label { key: "{key}", class: "check small",
                                input { r#type: "checkbox", checked: on, "aria-label": "Column {c.label}",
                                    onchange: move |e: FormEvent| {
                                        let mut list = columns.write();
                                        list.retain(|x| x != key);
                                        if e.checked() {
                                            // Keep the catalogue order.
                                            let mut next: Vec<String> = all.iter().filter(|x| x.key == key || list.iter().any(|l| l == x.key)).map(|x| x.key.to_string()).collect();
                                            std::mem::swap(&mut *list, &mut next);
                                        }
                                    } }
                                "{c.label}"
                            }
                        }
                    }
                }
                button { class: "btn sm ghost", onclick: move |_| columns.set(all.iter().filter(|c| c.default).map(|c| c.key.to_string()).collect()), "Reset columns" }
            }
        }
    }
}

#[component]
pub fn Poster(url: Option<String>, title: String, monitored: bool) -> Element {
    rsx! {
        div { class: "poster",
            if let Some(src) = url {
                img { src: "{src}", alt: "", loading: "lazy", referrerpolicy: "no-referrer" }
            } else {
                span { class: "initials", "aria-hidden": "true", {initials(&title)} }
            }
            if !monitored { div { class: "unmonitored", title: "Unmonitored" } }
        }
    }
}

fn tag_labels(ids: &[i64], tags: &[TagItem]) -> String {
    ids.iter()
        .filter_map(|id| tags.iter().find(|t| t.id == *id).map(|t| t.label.clone()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn date_or_dash(v: Option<&str>) -> String {
    v.map(format_date).unwrap_or_else(|| "—".into())
}

fn list_or_dash(v: &[String]) -> String {
    if v.is_empty() {
        "—".into()
    } else {
        v.join(", ")
    }
}

fn status_badge(status: &str) -> (&'static str, String) {
    match status {
        "continuing" => ("success", "Continuing".into()),
        "ended" => ("info", "Ended".into()),
        "upcoming" => ("warning", "Upcoming".into()),
        "deleted" => ("danger", "Deleted".into()),
        other => ("", other.to_string()),
    }
}

/// A series column's value as text.
pub fn series_value(item: &SeriesListItem, key: &str, tags: &[TagItem]) -> String {
    let x = &item.index;
    match key {
        "status" => status_badge(&item.status.to_string()).1,
        "seriesType" => match item.series_type.as_str() {
            "daily" => "Daily".into(),
            "anime" => "Anime".into(),
            _ => "Standard".into(),
        },
        "network" => item.network.clone().unwrap_or_else(|| "—".into()),
        "qualityProfile" => item.quality_profile_name.clone(),
        "nextAiring" => date_or_dash(item.next_airing.as_deref()),
        "previousAiring" => date_or_dash(x.previous_airing.as_deref()),
        "added" => format_date(&item.added_at),
        "seasonCount" => item.season_count.to_string(),
        "seasonFolder" => match x.season_folder {
            Some(true) => "Yes".into(),
            Some(false) => "No".into(),
            None => "—".into(),
        },
        "episodeProgress" => format!("{} / {}", item.episode_file_count, item.episode_count),
        "year" => item
            .year
            .map(|y| y.to_string())
            .unwrap_or_else(|| "—".into()),
        "path" => item.path.clone(),
        "sizeOnDisk" => format_bytes(item.size_on_disk),
        "averageSize" => {
            if item.episode_file_count > 0 {
                format_bytes(item.size_on_disk / item.episode_file_count)
            } else {
                "—".into()
            }
        }
        "genres" => list_or_dash(&x.genres),
        "ratings" => x
            .rating
            .map(|r| format!("{r:.1}"))
            .unwrap_or_else(|| "—".into()),
        "certification" => x.certification.clone().unwrap_or_else(|| "—".into()),
        "releaseGroups" => list_or_dash(&x.release_groups),
        "tags" => tag_labels(&item.tags, tags),
        _ => String::new(),
    }
}

/// A movie column's value as text.
pub fn movie_value(item: &MovieListItem, key: &str, tags: &[TagItem]) -> String {
    let x = &item.index;
    match key {
        "year" => item
            .year
            .map(|y| y.to_string())
            .unwrap_or_else(|| "—".into()),
        "studio" => item.studio.clone().unwrap_or_else(|| "—".into()),
        "qualityProfile" => item.quality_profile_name.clone(),
        "file" => if item.has_file { "on disk" } else { "missing" }.into(),
        "fileQuality" => x.file_quality.clone().unwrap_or_else(|| "—".into()),
        "watched" => if item.watched { "watched" } else { "unwatched" }.into(),
        "sizeOnDisk" => format_bytes(item.size_on_disk),
        "added" => format_date(&item.added_at),
        "inCinemas" => date_or_dash(x.in_cinemas.as_deref()),
        "runtime" => x
            .runtime
            .filter(|r| *r > 0)
            .map(|r| format!("{r} min"))
            .unwrap_or_else(|| "—".into()),
        "genres" => list_or_dash(&x.genres),
        "ratings" => x
            .rating
            .map(|r| format!("{r:.1}"))
            .unwrap_or_else(|| "—".into()),
        "certification" => x.certification.clone().unwrap_or_else(|| "—".into()),
        "releaseGroups" => list_or_dash(&x.release_groups),
        "path" => item.path.clone(),
        "tags" => tag_labels(&item.tags, tags),
        _ => String::new(),
    }
}

fn label_of(all: &[Column], key: &str) -> &'static str {
    all.iter()
        .find(|c| c.key == key)
        .map(|c| c.label)
        .unwrap_or("")
}

/// The chosen columns a poster card lists under the title (the ones the card does not show).
fn poster_fields(columns: &[String]) -> Vec<String> {
    columns
        .iter()
        .filter(|c| {
            !matches!(
                c.as_str(),
                "status" | "episodeProgress" | "year" | "file" | "watched"
            )
        })
        .cloned()
        .collect()
}

#[component]
pub fn SeriesCard(
    item: SeriesListItem,
    selected: bool,
    /// The library shows anime groups once ("Group anime seasons").
    #[props(default)]
    grouped: bool,
    /// Chosen columns listed under the title.
    #[props(default)]
    columns: Vec<String>,
    #[props(default)] tags: Vec<TagItem>,
) -> Element {
    let pct = percent(item.episode_file_count, item.episode_count);
    let tone = if item.episode_count > 0 && item.episode_file_count >= item.episode_count {
        Some("success")
    } else {
        None
    };
    let id = item.id;
    let extra: Vec<String> = poster_fields(&columns)
        .iter()
        .map(|k| series_value(&item, k, &tags))
        .filter(|v| !v.is_empty() && v != "—")
        .collect();
    rsx! {
        Link { to: Route::SeriesDetailPage { id }, class: if selected { "media-card selected" } else { "media-card" },
            Poster { url: item.poster_url.clone(), title: item.title.clone(), monitored: item.monitored }
            div { class: "meta",
                span { class: "t ellipsis", title: "{item.title}", "{item.title}" }
                span { class: "xs muted",
                    {item.year.map(|y| y.to_string()).unwrap_or_default()}
                    " · {item.episode_file_count}/{item.episode_count} ep"
                }
                if let Some(g) = item.anime_group.as_ref().filter(|g| grouped && g.series_count > 1) {
                    span { class: "xs accent ellipsis", title: "Anime group {g.title}", "Group of {g.series_count} · {g.title}" }
                }
                for (i, v) in extra.iter().enumerate() {
                    span { key: "{i}", class: "xs muted ellipsis poster-field", title: "{v}", "{v}" }
                }
                Progress { percent: pct, tone }
            }
        }
    }
}

#[component]
pub fn MovieCard(
    item: MovieListItem,
    #[props(default)] selected: bool,
    #[props(default)] columns: Vec<String>,
    #[props(default)] tags: Vec<TagItem>,
) -> Element {
    let id = item.id;
    let extra: Vec<String> = poster_fields(&columns)
        .iter()
        .map(|k| movie_value(&item, k, &tags))
        .filter(|v| !v.is_empty() && v != "—")
        .collect();
    rsx! {
        Link { to: Route::MovieDetailPage { id }, class: if selected {"media-card selected"}else{"media-card"},
            Poster { url: item.poster_url.clone(), title: item.title.clone(), monitored: item.monitored }
            div { class: "meta",
                span { class: "t ellipsis", title: "{item.title}", "{item.title}" }
                span { class: "xs muted",
                    {item.year.map(|y| y.to_string()).unwrap_or_default()}
                    if item.has_file { " · on disk" } else { " · missing" }
                }
                for (i, v) in extra.iter().enumerate() {
                    span { key: "{i}", class: "xs muted ellipsis poster-field", title: "{v}", "{v}" }
                }
                if item.watched {Badge {text:"watched",tone:"success"}}
            }
        }
    }
}

/// Sortable, selectable series table with the chosen columns.
#[component]
pub fn SeriesTable(
    items: Vec<SeriesListItem>,
    selected: Signal<HashSet<i64>>,
    sort_key: String,
    on_sort: EventHandler<String>,
    #[props(default)] columns: Option<Vec<String>>,
    #[props(default)] tags: Vec<TagItem>,
) -> Element {
    let columns = columns.unwrap_or_else(|| {
        SERIES_COLUMNS
            .iter()
            .filter(|c| c.default)
            .map(|c| c.key.to_string())
            .collect()
    });
    let all_ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let all_selected = !all_ids.is_empty() && all_ids.iter().all(|id| selected.read().contains(id));
    let header = move |key: &'static str, label: &'static str| {
        let active = sort_key == key;
        rsx! {
            th {
                class: "sortable",
                "aria-sort": if active { "ascending" } else { "none" },
                onclick: move |_| on_sort.call(key.to_string()),
                "{label}"
                if active { " ▾" }
            }
        }
    };
    rsx! {
        div { class: "table-wrap",
            table { class: "table",
                thead {
                    tr {
                        th { class: "shrink",
                            input {
                                r#type: "checkbox",
                                "aria-label": "Select all",
                                checked: all_selected,
                                onchange: move |e: FormEvent| {
                                    let mut sel = selected.write();
                                    if e.checked() { sel.extend(all_ids.iter().copied()); } else { for id in &all_ids { sel.remove(id); } }
                                },
                            }
                        }
                        {header("title", "Title")}
                        for c in columns.iter().filter_map(|k| SERIES_COLUMNS.iter().find(|c| c.key == k)) {
                            if let Some(sort) = c.sort {
                                {header(sort, c.label)}
                            } else {
                                th { key: "{c.key}", class: if c.key == "episodeProgress" || c.key == "seasonCount" || c.key == "sizeOnDisk" { "num" } else { "" }, "{c.label}" }
                            }
                        }
                    }
                }
                tbody {
                    for item in items {
                        SeriesRow { key: "{item.id}", item: item.clone(), selected, columns: columns.clone(), tags: tags.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn SeriesRow(
    item: SeriesListItem,
    selected: Signal<HashSet<i64>>,
    columns: Vec<String>,
    tags: Vec<TagItem>,
) -> Element {
    let id = item.id;
    let is_selected = selected.read().contains(&id);
    let (tone, label) = status_badge(&item.status.to_string());
    rsx! {
        tr { class: if is_selected { "selected" } else { "" },
            td {
                input {
                    r#type: "checkbox",
                    "aria-label": "Select {item.title}",
                    checked: is_selected,
                    onchange: move |e: FormEvent| { if e.checked() { selected.write().insert(id); } else { selected.write().remove(&id); } },
                }
            }
            td {
                div { class: "row tight",
                    if !item.monitored { Badge { text: "unmonitored" } }
                    Link { to: Route::SeriesDetailPage { id }, "{item.title}" }
                    if !columns.iter().any(|c| c == "year") {
                        span { class: "faint xs", {item.year.map(|y| y.to_string()).unwrap_or_default()} }
                    }
                }
            }
            for c in columns.iter() {
                if c == "status" {
                    td { key: "{c}", Badge { text: label.clone(), tone: if tone.is_empty() { None } else { Some(tone) } } }
                } else {
                    td { key: "{c}",
                        class: match c.as_str() { "episodeProgress" | "seasonCount" | "sizeOnDisk" | "averageSize" => "num", "path" => "mono small", "added" | "nextAiring" | "previousAiring" | "network" => "muted", _ => "" },
                        {series_value(&item, c, &tags)}
                    }
                }
            }
        }
    }
}

#[component]
pub fn MovieTable(
    items: Vec<MovieListItem>,
    mut selected: Signal<HashSet<i64>>,
    #[props(default)] disabled: bool,
    #[props(default)] columns: Option<Vec<String>>,
    #[props(default)] tags: Vec<TagItem>,
) -> Element {
    let columns = columns.unwrap_or_else(|| {
        MOVIE_COLUMNS
            .iter()
            .filter(|c| c.default)
            .map(|c| c.key.to_string())
            .collect()
    });
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let all = !ids.is_empty() && ids.iter().all(|id| selected.read().contains(id));
    rsx! {
        div { class: "table-wrap",
            table { class: "table",
                thead { tr {
                    th { input {r#type:"checkbox",disabled,"aria-label":"Select all movies on this page",checked:all,onchange:move |e:FormEvent|{if e.checked(){selected.write().extend(ids.iter().copied());}else{for id in &ids{selected.write().remove(id);}}}} }
                    th { "Title" }
                    for c in columns.iter() {
                        th { key: "{c}", class: if c == "sizeOnDisk" { "num" } else { "" }, {label_of(MOVIE_COLUMNS, c)} }
                    }
                } }
                tbody {
                    for item in items {
                        tr { key: "{item.id}",
                            td {input {r#type:"checkbox",disabled,"aria-label":"Select {item.title}",checked:selected.read().contains(&item.id),onchange:move |e:FormEvent|{if e.checked(){selected.write().insert(item.id);}else{selected.write().remove(&item.id);}}}}
                            td {
                                div { class: "row tight",
                                    if !item.monitored { Badge { text: "unmonitored" } }
                                    Link { to: Route::MovieDetailPage { id: item.id }, "{item.title}" }
                                }
                            }
                            for c in columns.iter() {
                                match c.as_str() {
                                    "file" => rsx! { td { key: "{c}", if item.has_file { Badge { text: "on disk", tone: "success" } } else { Badge { text: "missing", tone: "warning" } } } },
                                    "watched" => rsx! { td { key: "{c}", span {title:item.watched_at.as_deref().map(crate::util::format_datetime).unwrap_or_default(),if item.watched {"watched"}else{"unwatched"}} } },
                                    _ => rsx! { td { key: "{c}", class: match c.as_str() { "sizeOnDisk" => "num", "path" => "mono small", "added" | "inCinemas" => "muted", _ => "" }, {movie_value(&item, c, &tags)} } },
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Tanukarr's overview view: poster, overview and the chosen details, one title per row.
#[component]
pub fn SeriesOverview(
    items: Vec<SeriesListItem>,
    selected: Signal<HashSet<i64>>,
    columns: Vec<String>,
    tags: Vec<TagItem>,
) -> Element {
    rsx! {
        div { class: "overview-list", role: "list",
            for item in items {
                {
                    let id = item.id;
                    let is_selected = selected.read().contains(&id);
                    let pct = percent(item.episode_file_count, item.episode_count);
                    let details: Vec<(&'static str, String)> = columns.iter()
                        .map(|k| (label_of(SERIES_COLUMNS, k), series_value(&item, k, &tags)))
                        .filter(|(_, v)| !v.is_empty())
                        .collect();
                    rsx! {
                        article { key: "{id}", class: if is_selected { "overview-item selected" } else { "overview-item" }, role: "listitem",
                            Link { to: Route::SeriesDetailPage { id }, class: "overview-poster", "aria-hidden": "true", tabindex: "-1",
                                Poster { url: item.poster_url.clone(), title: item.title.clone(), monitored: item.monitored }
                            }
                            div { class: "overview-body",
                                div { class: "row tight",
                                    input { r#type: "checkbox", "aria-label": "Select {item.title}", checked: is_selected,
                                        onchange: move |e: FormEvent| { if e.checked() { selected.write().insert(id); } else { selected.write().remove(&id); } } }
                                    Link { to: Route::SeriesDetailPage { id }, class: "strong", "{item.title}" }
                                    if let Some(y) = item.year { span { class: "faint xs", "{y}" } }
                                    if !item.monitored { Badge { text: "unmonitored" } }
                                }
                                Progress { percent: pct, tone: None }
                                if let Some(o) = item.index.overview.as_ref().filter(|o| !o.trim().is_empty()) {
                                    p { class: "overview small", "{o}" }
                                }
                                dl { class: "overview-details small",
                                    for (label, value) in details {
                                        div { key: "{label}", dt { "{label}" } dd { "{value}" } }
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

/// The overview view of the movie library.
#[component]
pub fn MovieOverview(
    items: Vec<MovieListItem>,
    selected: Signal<HashSet<i64>>,
    columns: Vec<String>,
    tags: Vec<TagItem>,
    #[props(default)] disabled: bool,
) -> Element {
    rsx! {
        div { class: "overview-list", role: "list",
            for item in items {
                {
                    let id = item.id;
                    let is_selected = selected.read().contains(&id);
                    let details: Vec<(&'static str, String)> = columns.iter()
                        .map(|k| (label_of(MOVIE_COLUMNS, k), movie_value(&item, k, &tags)))
                        .filter(|(_, v)| !v.is_empty())
                        .collect();
                    rsx! {
                        article { key: "{id}", class: if is_selected { "overview-item selected" } else { "overview-item" }, role: "listitem",
                            Link { to: Route::MovieDetailPage { id }, class: "overview-poster", "aria-hidden": "true", tabindex: "-1",
                                Poster { url: item.poster_url.clone(), title: item.title.clone(), monitored: item.monitored }
                            }
                            div { class: "overview-body",
                                div { class: "row tight",
                                    input { r#type: "checkbox", disabled, "aria-label": "Select {item.title}", checked: is_selected,
                                        onchange: move |e: FormEvent| { if e.checked() { selected.write().insert(id); } else { selected.write().remove(&id); } } }
                                    Link { to: Route::MovieDetailPage { id }, class: "strong", "{item.title}" }
                                    if !item.monitored { Badge { text: "unmonitored" } }
                                }
                                if let Some(o) = item.index.overview.as_ref().filter(|o| !o.trim().is_empty()) {
                                    p { class: "overview small", "{o}" }
                                }
                                dl { class: "overview-details small",
                                    for (label, value) in details {
                                        div { key: "{label}", dt { "{label}" } dd { "{value}" } }
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
