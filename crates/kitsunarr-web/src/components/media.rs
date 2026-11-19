//! Library presentation (Tanukarr's series index): poster cards, the table and the overview
//! list of series, anime, movies and anime groups, with the index columns chosen per browser.

use std::collections::HashSet;

use dioxus::prelude::*;
use kitsunarr_domain::api::library::{
    LibraryEntry, LibraryGroup, LibraryKey, MovieListItem, SeriesListItem,
};
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

/// Tanukarr's series index columns (`seriesOptionsStore`), in its order, shown by default and
/// sortable as there. "Watched" is Kitsunarr's movie watched state.
pub const SERIES_COLUMNS: &[Column] = &[
    col("status", "Status", true, Some("status")),
    col("seriesType", "Type", false, Some("seriesType")),
    col("network", "Network", true, Some("network")),
    col(
        "qualityProfile",
        "Quality profile",
        true,
        Some("qualityProfile"),
    ),
    col("nextAiring", "Next airing", true, Some("nextAiring")),
    col(
        "previousAiring",
        "Previous airing",
        false,
        Some("previousAiring"),
    ),
    col("added", "Added", false, Some("added")),
    col("seasonCount", "Seasons", true, Some("seasonCount")),
    col("seasonFolder", "Season folder", false, Some("seasonFolder")),
    col("episodeProgress", "Episodes", true, Some("episodeProgress")),
    col("episodeCount", "Episode count", false, Some("episodeCount")),
    col("year", "Year", false, Some("year")),
    col("path", "Path", false, Some("path")),
    col("sizeOnDisk", "Size on disk", false, Some("sizeOnDisk")),
    col(
        "averageSize",
        "Average episode size",
        false,
        Some("averageSize"),
    ),
    col("genres", "Genres", false, None),
    col("ratings", "Rating", false, Some("ratings")),
    col("certification", "Certification", false, None),
    col("releaseGroups", "Release groups", false, None),
    col("tags", "Tags", false, Some("tags")),
    col("watched", "Watched", false, None),
];

/// Tanukarr's sort menu (`SeriesIndexSortMenu`): sort key and label.
pub const SORT_KEYS: &[(&str, &str)] = &[
    ("status", "Monitored/Status"),
    ("title", "Title"),
    ("network", "Network"),
    ("qualityProfile", "Quality profile"),
    ("nextAiring", "Next airing"),
    ("previousAiring", "Previous airing"),
    ("added", "Added"),
    ("seasonCount", "Seasons"),
    ("episodeProgress", "Episodes"),
    ("episodeCount", "Episode count"),
    ("path", "Path"),
    ("sizeOnDisk", "Size on disk"),
    ("averageSize", "Average episode size"),
    ("tags", "Tags"),
    ("ratings", "Rating"),
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

/// The chosen columns of a library, kept per browser.
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

/// Tanukarr's sort menu: the chosen key shows its direction; choosing it again reverses it.
#[component]
pub fn SortMenu(sort_key: String, descending: bool, on_sort: EventHandler<String>) -> Element {
    rsx! {
        details { class: "combine sort-menu",
            summary { class: "btn sm", "aria-label": "Sort by", "Sort by" }
            fieldset { class: "menu stack tight",
                for (key, label) in SORT_KEYS.iter() {
                    {
                        let on = sort_key == *key;
                        rsx! {
                            button {
                                key: "{key}",
                                class: if on { "btn sm ghost sort-item on" } else { "btn sm ghost sort-item" },
                                "aria-pressed": if on { "true" } else { "false" },
                                onclick: move |_| on_sort.call(key.to_string()),
                                span { class: "grow", "{label}" }
                                if on {
                                    span { class: "sort-dir", "aria-hidden": "true", if descending { "▼" } else { "▲" } }
                                }
                            }
                        }
                    }
                }
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

fn rating(v: Option<f64>) -> String {
    v.map(|r| format!("{r:.1}")).unwrap_or_else(|| "—".into())
}

fn status_badge(status: &str) -> (&'static str, String) {
    match status {
        "continuing" => ("success", "Continuing".into()),
        "ended" => ("info", "Ended".into()),
        "upcoming" | "announced" => ("warning", "Upcoming".into()),
        "deleted" => ("danger", "Deleted".into()),
        "inCinemas" => ("success", "In cinemas".into()),
        "released" => ("info", "Released".into()),
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
        "episodeCount" => item.episode_count.to_string(),
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
        "ratings" => rating(x.rating),
        "certification" => x.certification.clone().unwrap_or_else(|| "—".into()),
        "releaseGroups" => list_or_dash(&x.release_groups),
        "tags" => tag_labels(&item.tags, tags),
        "watched" => "—".into(),
        _ => String::new(),
    }
}

/// The cinema release when it is before (`past`) or after now.
fn release(item: &MovieListItem, past: bool) -> String {
    let now = js_sys::Date::new_0()
        .to_iso_string()
        .as_string()
        .unwrap_or_default();
    match item.index.in_cinemas.as_deref() {
        Some(d) if (d <= now.as_str()) == past => format_date(d),
        _ => "—".into(),
    }
}

/// A movie's value of a series column (Tanukarr's movies are one-episode series).
pub fn movie_value(item: &MovieListItem, key: &str, tags: &[TagItem]) -> String {
    let x = &item.index;
    match key {
        "status" => status_badge(&item.status.to_string()).1,
        "seriesType" => "Movie".into(),
        "network" => item.studio.clone().unwrap_or_else(|| "—".into()),
        "qualityProfile" => item.quality_profile_name.clone(),
        "nextAiring" => release(item, false),
        "previousAiring" => release(item, true),
        "added" => format_date(&item.added_at),
        "seasonCount" | "seasonFolder" => "—".into(),
        "episodeProgress" => format!("{} / 1", i32::from(item.has_file)),
        "episodeCount" => "1".into(),
        "year" => item
            .year
            .map(|y| y.to_string())
            .unwrap_or_else(|| "—".into()),
        "path" => item.path.clone(),
        "sizeOnDisk" | "averageSize" => format_bytes(item.size_on_disk),
        "genres" => list_or_dash(&x.genres),
        "ratings" => rating(x.rating),
        "certification" => x.certification.clone().unwrap_or_else(|| "—".into()),
        "releaseGroups" => list_or_dash(&x.release_groups),
        "tags" => tag_labels(&item.tags, tags),
        "watched" => if item.watched { "Watched" } else { "Unwatched" }.into(),
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
                "status" | "episodeProgress" | "year" | "watched"
            )
        })
        .cloned()
        .collect()
}

/// The titles an entry stands for (a group: its members the filters keep).
pub fn entry_keys(entry: &LibraryEntry) -> Vec<LibraryKey> {
    match entry {
        LibraryEntry::Series(s) => vec![LibraryKey {
            movie: false,
            id: s.id,
        }],
        LibraryEntry::Movie(m) => vec![LibraryKey {
            movie: true,
            id: m.id,
        }],
        LibraryEntry::Group(g) => g.members.clone(),
    }
}

fn entry_title(entry: &LibraryEntry) -> &str {
    match entry {
        LibraryEntry::Series(s) => &s.title,
        LibraryEntry::Movie(m) => &m.title,
        LibraryEntry::Group(g) => &g.title,
    }
}

fn entry_dom_key(entry: &LibraryEntry) -> String {
    match entry {
        LibraryEntry::Series(s) => format!("s{}", s.id),
        LibraryEntry::Movie(m) => format!("m{}", m.id),
        LibraryEntry::Group(g) => format!("g{}", g.collection_id),
    }
}

fn is_selected(keys: &[LibraryKey], selected: &HashSet<LibraryKey>) -> bool {
    !keys.is_empty() && keys.iter().all(|k| selected.contains(k))
}

/// Selects the titles of an entry (a group: all its members, as Tanukarr's group checkbox).
#[component]
fn SelectBox(
    keys: Vec<LibraryKey>,
    title: String,
    selected: Signal<HashSet<LibraryKey>>,
) -> Element {
    let on = is_selected(&keys, &selected.read());
    rsx! {
        input {
            r#type: "checkbox",
            "aria-label": "Select {title}",
            checked: on,
            onchange: move |e: FormEvent| {
                let mut sel = selected.write();
                for k in &keys {
                    if e.checked() { sel.insert(*k); } else { sel.remove(k); }
                }
            },
        }
    }
}

/// Tanukarr's group counts: "{seasons} seasons · {movies} movies", specials when there are some.
fn group_counts(g: &LibraryGroup) -> String {
    if g.special_count > 0 {
        format!(
            "{} seasons · {} movies · {} specials",
            g.season_count, g.movie_count, g.special_count
        )
    } else {
        format!("{} seasons · {} movies", g.season_count, g.movie_count)
    }
}

fn group_episodes(g: &LibraryGroup) -> String {
    format!("{}/{} episodes", g.episode_file_count, g.episode_count)
}

#[component]
pub fn SeriesCard(
    item: SeriesListItem,
    selected: bool,
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

/// Tanukarr's grouped entry (`AnimeGroupSummary`): the group's poster and title, its seasons,
/// movies and episodes; it opens the collection page.
#[component]
pub fn GroupCard(group: LibraryGroup, #[props(default)] selected: bool) -> Element {
    let id = group.collection_id;
    let pct = percent(group.episode_file_count, group.episode_count);
    rsx! {
        Link { to: Route::CollectionDetailPage { id }, class: if selected { "media-card group-card selected" } else { "media-card group-card" },
            "data-anime-group": "{id}",
            Poster { url: group.poster_url.clone(), title: group.title.clone(), monitored: true }
            div { class: "meta",
                span { class: "t ellipsis", title: "{group.title}", "{group.title}" }
                span { class: "xs muted ellipsis", {group_counts(&group)} }
                span { class: "xs muted", {group_episodes(&group)} }
                if let Some(n) = group.single_season {
                    span { class: "xs accent", "Season {n}" }
                }
                Progress { percent: pct, tone: None }
            }
        }
    }
}

/// The poster view: one card per entry, with Tanukarr's select checkbox in select mode.
#[component]
pub fn LibraryPosters(
    entries: Vec<LibraryEntry>,
    selected: Signal<HashSet<LibraryKey>>,
    select_mode: bool,
    columns: Vec<String>,
    tags: Vec<TagItem>,
    first_index: u64,
) -> Element {
    rsx! {
        div { class: "media-grid",
            for (i, entry) in entries.into_iter().enumerate() {
                {
                    let keys = entry_keys(&entry);
                    let on = is_selected(&keys, &selected.read());
                    let title = entry_title(&entry).to_string();
                    rsx! {
                        div { key: "{entry_dom_key(&entry)}", class: "card-cell", id: "library-entry-{first_index + i as u64}",
                            match entry {
                                LibraryEntry::Series(item) => rsx! { SeriesCard { item, selected: on, columns: columns.clone(), tags: tags.clone() } },
                                LibraryEntry::Movie(item) => rsx! { MovieCard { item, selected: on, columns: columns.clone(), tags: tags.clone() } },
                                LibraryEntry::Group(group) => rsx! { GroupCard { group, selected: on } },
                            }
                            if select_mode {
                                label { class: "card-select", SelectBox { keys, title, selected } }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The table view: sortable headers (choosing the sorted column again reverses it), one row
/// per entry; a group row shows its title and counts as Tanukarr's.
#[component]
#[allow(clippy::too_many_arguments)]
pub fn LibraryTable(
    entries: Vec<LibraryEntry>,
    selected: Signal<HashSet<LibraryKey>>,
    select_mode: bool,
    sort_key: String,
    descending: bool,
    on_sort: EventHandler<String>,
    columns: Vec<String>,
    tags: Vec<TagItem>,
    first_index: u64,
) -> Element {
    let page_keys: Vec<LibraryKey> = entries.iter().flat_map(entry_keys).collect();
    let all_selected = is_selected(&page_keys, &selected.read());
    // Tanukarr's status column comes before the title.
    let status_first = columns.iter().any(|c| c == "status");
    let rest: Vec<String> = columns.iter().filter(|c| *c != "status").cloned().collect();
    let span = rest.len() + 1 + usize::from(status_first);
    let header = |key: &'static str, label: &'static str, class: &'static str| {
        let active = sort_key == key;
        let dir = if !active {
            "none"
        } else if descending {
            "descending"
        } else {
            "ascending"
        };
        rsx! {
            th {
                key: "{key}",
                class: "sortable {class}",
                "aria-sort": dir,
                onclick: move |_| on_sort.call(key.to_string()),
                "{label}"
                if active {
                    span { class: "sort-dir", "aria-hidden": "true", if descending { " ▼" } else { " ▲" } }
                }
            }
        }
    };
    let column_header = |key: &str| {
        let c = SERIES_COLUMNS.iter().find(|c| c.key == key)?;
        let class = if matches!(
            c.key,
            "episodeProgress" | "seasonCount" | "sizeOnDisk" | "averageSize" | "episodeCount"
        ) {
            "num"
        } else {
            ""
        };
        Some(match c.sort {
            Some(sort) => header(sort, c.label, class),
            None => rsx! { th { key: "{c.key}", class, "{c.label}" } },
        })
    };
    rsx! {
        div { class: "table-wrap",
            table { class: "table library-table",
                thead {
                    tr {
                        if select_mode {
                            th { class: "shrink",
                                input {
                                    r#type: "checkbox",
                                    "aria-label": "Select all",
                                    checked: all_selected,
                                    onchange: move |e: FormEvent| {
                                        let mut sel = selected.write();
                                        for k in &page_keys { if e.checked() { sel.insert(*k); } else { sel.remove(k); } }
                                    },
                                }
                            }
                        }
                        if status_first { {column_header("status")} }
                        {header("title", "Title", "")}
                        for c in rest.iter() {
                            {column_header(c)}
                        }
                    }
                }
                tbody {
                    for (i, entry) in entries.into_iter().enumerate() {
                        LibraryRow {
                            key: "{entry_dom_key(&entry)}",
                            entry,
                            index: first_index + i as u64,
                            selected,
                            select_mode,
                            status_first,
                            columns: rest.clone(),
                            span,
                            tags: tags.clone(),
                        }
                    }
                }
            }
        }
    }
}

fn cell_class(key: &str) -> &'static str {
    match key {
        "episodeProgress" | "seasonCount" | "sizeOnDisk" | "averageSize" | "episodeCount" => "num",
        "path" => "mono small",
        "added" | "nextAiring" | "previousAiring" | "network" => "muted",
        _ => "",
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn LibraryRow(
    entry: LibraryEntry,
    index: u64,
    selected: Signal<HashSet<LibraryKey>>,
    select_mode: bool,
    status_first: bool,
    columns: Vec<String>,
    span: usize,
    tags: Vec<TagItem>,
) -> Element {
    let keys = entry_keys(&entry);
    let on = is_selected(&keys, &selected.read());
    let title = entry_title(&entry).to_string();
    let select = rsx! {
        if select_mode {
            td { SelectBox { keys: keys.clone(), title: title.clone(), selected } }
        }
    };
    match entry {
        LibraryEntry::Group(g) => rsx! {
            tr { class: if on { "selected group-row" } else { "group-row" }, id: "library-entry-{index}", "data-anime-group": "{g.collection_id}",
                {select}
                td { colspan: "{span}",
                    div { class: "row tight",
                        Link { to: Route::CollectionDetailPage { id: g.collection_id }, class: "strong", "{g.title}" }
                        span { class: "xs muted", {group_counts(&g)} }
                        span { class: "xs muted", {group_episodes(&g)} }
                        if let Some(n) = g.single_season {
                            span { class: "xs accent", "Season {n}" }
                        }
                    }
                }
            }
        },
        LibraryEntry::Series(item) => {
            let (tone, label) = status_badge(&item.status.to_string());
            let id = item.id;
            rsx! {
                tr { class: if on { "selected" } else { "" }, id: "library-entry-{index}",
                    {select}
                    if status_first {
                        td { Badge { text: label.clone(), tone: if tone.is_empty() { None } else { Some(tone) } } }
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
                        td { key: "{c}", class: cell_class(c), {series_value(&item, c, &tags)} }
                    }
                }
            }
        }
        LibraryEntry::Movie(item) => {
            let (tone, label) = status_badge(&item.status.to_string());
            let id = item.id;
            rsx! {
                tr { class: if on { "selected" } else { "" }, id: "library-entry-{index}",
                    {select}
                    if status_first {
                        td { Badge { text: label.clone(), tone: if tone.is_empty() { None } else { Some(tone) } } }
                    }
                    td {
                        div { class: "row tight",
                            if !item.monitored { Badge { text: "unmonitored" } }
                            Link { to: Route::MovieDetailPage { id }, "{item.title}" }
                            if !columns.iter().any(|c| c == "year") {
                                span { class: "faint xs", {item.year.map(|y| y.to_string()).unwrap_or_default()} }
                            }
                        }
                    }
                    for c in columns.iter() {
                        if c == "watched" {
                            td { key: "{c}", span { title: item.watched_at.as_deref().map(crate::util::format_datetime).unwrap_or_default(), {movie_value(&item, c, &tags)} } }
                        } else {
                            td { key: "{c}", class: cell_class(c), {movie_value(&item, c, &tags)} }
                        }
                    }
                }
            }
        }
    }
}

/// What an overview row shows: link, poster, monitored, year, overview, progress, details.
type OverviewParts = (
    Route,
    Option<String>,
    bool,
    Option<i32>,
    Option<String>,
    Option<f64>,
    Vec<(&'static str, String)>,
);

/// Tanukarr's overview view: poster, overview and the chosen details, one entry per row.
#[component]
pub fn LibraryOverview(
    entries: Vec<LibraryEntry>,
    selected: Signal<HashSet<LibraryKey>>,
    select_mode: bool,
    columns: Vec<String>,
    tags: Vec<TagItem>,
    first_index: u64,
) -> Element {
    rsx! {
        div { class: "overview-list", role: "list",
            for (i, entry) in entries.into_iter().enumerate() {
                {
                    let keys = entry_keys(&entry);
                    let on = is_selected(&keys, &selected.read());
                    let title = entry_title(&entry).to_string();
                    let (route, poster, monitored, year, overview, pct, details): OverviewParts = match &entry {
                        LibraryEntry::Series(item) => (
                            Route::SeriesDetailPage { id: item.id },
                            item.poster_url.clone(),
                            item.monitored,
                            item.year,
                            item.index.overview.clone(),
                            Some(percent(item.episode_file_count, item.episode_count)),
                            columns.iter().map(|k| (label_of(SERIES_COLUMNS, k), series_value(item, k, &tags))).filter(|(_, v)| !v.is_empty()).collect(),
                        ),
                        LibraryEntry::Movie(item) => (
                            Route::MovieDetailPage { id: item.id },
                            item.poster_url.clone(),
                            item.monitored,
                            item.year,
                            item.index.overview.clone(),
                            None,
                            columns.iter().map(|k| (label_of(SERIES_COLUMNS, k), movie_value(item, k, &tags))).filter(|(_, v)| !v.is_empty()).collect(),
                        ),
                        LibraryEntry::Group(g) => (
                            Route::CollectionDetailPage { id: g.collection_id },
                            g.poster_url.clone(),
                            true,
                            None,
                            None,
                            Some(percent(g.episode_file_count, g.episode_count)),
                            Vec::new(),
                        ),
                    };
                    let group = match &entry { LibraryEntry::Group(g) => Some(g.clone()), _ => None };
                    rsx! {
                        article { key: "{entry_dom_key(&entry)}", id: "library-entry-{first_index + i as u64}",
                            class: if on { "overview-item selected" } else { "overview-item" }, role: "listitem",
                            Link { to: route.clone(), class: "overview-poster", "aria-hidden": "true", tabindex: "-1",
                                Poster { url: poster, title: title.clone(), monitored }
                            }
                            div { class: "overview-body",
                                div { class: "row tight",
                                    if select_mode { SelectBox { keys, title: title.clone(), selected } }
                                    Link { to: route, class: "strong", "{title}" }
                                    if let Some(y) = year { span { class: "faint xs", "{y}" } }
                                    if !monitored { Badge { text: "unmonitored" } }
                                }
                                if let Some(p) = pct { Progress { percent: p, tone: None } }
                                if let Some(g) = group {
                                    div { class: "small muted", {group_counts(&g)} }
                                    div { class: "small muted", {group_episodes(&g)} }
                                    if let Some(n) = g.single_season {
                                        div { class: "small accent", "Season {n}" }
                                    }
                                }
                                if let Some(o) = overview.filter(|o| !o.trim().is_empty()) {
                                    p { class: "overview small", "{o}" }
                                }
                                if !details.is_empty() {
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
}
