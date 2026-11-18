//! Edit dialogs of the series and movie pages (Sonarr's `EditSeriesModal`): monitoring,
//! quality profile, series type, season folders, folder and tags in one place.

use dioxus::prelude::*;
use kitsunarr_domain::api::library::{MovieDetail, MovieUpdate, SeriesDetail, SeriesUpdate};
use kitsunarr_domain::api::metadata::FolderPreviewQuery;
use kitsunarr_domain::api::settings::{QualityProfile, RootFolderItem, TagItem};
use kitsunarr_domain::{MetadataProvider, MinimumAvailability, MonitorNewItems, SeriesType};

use crate::components::common::{ErrorBox, Field, Loading, Modal, Switch};
use crate::state::{use_app, use_client};

/// A title's folder split into its root folder and the folder below it.
pub(crate) fn split_path(path: &str, root: &str) -> (String, String) {
    let root = root.trim_end_matches(['/', '\\']);
    match path
        .strip_prefix(root)
        .and_then(|rest| rest.strip_prefix(['/', '\\']))
    {
        Some(rest) if !root.is_empty() => (root.to_string(), rest.to_string()),
        _ => {
            let at = path.rfind(['/', '\\']).unwrap_or(0);
            (
                path[..at].to_string(),
                path[at..].trim_start_matches(['/', '\\']).to_string(),
            )
        }
    }
}

/// The folder a root folder and a relative folder make.
pub(crate) fn join_path(root: &str, folder: &str) -> String {
    format!(
        "{}/{}",
        root.trim_end_matches(['/', '\\']),
        folder.trim_matches(['/', '\\'])
    )
}

/// Tag chips: every tag, the chosen ones pressed.
#[component]
pub fn TagChips(all: Vec<TagItem>, selected: Vec<i64>, on_toggle: EventHandler<i64>) -> Element {
    if all.is_empty() {
        return rsx! { p { class: "help small", "No tags yet: create them in Settings › Tags." } };
    }
    rsx! {
        div { class: "chips", role: "group", "aria-label": "Tags",
            for t in all.iter() {
                button {
                    key: "{t.id}",
                    r#type: "button",
                    class: if selected.contains(&t.id) { "chip on" } else { "chip" },
                    "aria-pressed": "{selected.contains(&t.id)}",
                    onclick: {
                        let id = t.id;
                        move |_| on_toggle.call(id)
                    },
                    "{t.label}"
                }
            }
        }
    }
}

fn toggle(mut list: Signal<Vec<i64>>, id: i64) {
    let mut v = list.write();
    if let Some(i) = v.iter().position(|x| *x == id) {
        v.remove(i);
    } else {
        v.push(id);
    }
}

type Choices = (Vec<QualityProfile>, Vec<TagItem>, Vec<RootFolderItem>);

fn use_choices() -> Resource<Result<Choices, kitsunarr_client::ClientError>> {
    let client = use_client();
    use_resource(move || {
        let c = client.clone();
        async move {
            let profiles = c.quality_profiles().await?;
            let tags = c.tags().await?;
            let roots = c.root_folders().await?;
            Ok((profiles, tags, roots))
        }
    })
}

/// The folder part of the Edit dialogs: root folder, folder name (with the naming format's
/// suggestion) and whether to move the files when it changes.
#[component]
fn FolderFields(
    roots: Vec<RootFolderItem>,
    root: Signal<String>,
    folder: Signal<String>,
    move_files: Signal<bool>,
    changed: bool,
    disabled: bool,
    title: String,
    year: Option<i32>,
    movie: bool,
) -> Element {
    let app = use_app();
    let client = use_client();
    let mut root = root;
    let mut folder = folder;
    let mut move_files = move_files;
    let mut suggesting = use_signal(|| false);
    let suggest = move |_| {
        let c = client.clone();
        let q = FolderPreviewQuery {
            title: title.clone(),
            year,
            movie,
        };
        suggesting.set(true);
        spawn(async move {
            match c.folder_preview(&q).await {
                Ok(name) => folder.set(name),
                Err(e) => app.error(&e),
            }
            suggesting.set(false);
        });
    };
    rsx! {
        Field { label: "Root folder",
            select { class: "select", "aria-label": "Root folder", disabled,
                onchange: move |e: FormEvent| root.set(e.value()),
                if !roots.iter().any(|r| r.path.trim_end_matches('/') == root().trim_end_matches('/')) {
                    option { value: "{root}", selected: true, "{root}" }
                }
                for r in roots.iter() {
                    option { key: "{r.id}", value: "{r.path}", selected: r.path.trim_end_matches('/') == root().trim_end_matches('/'), "{r.path}" }
                }
            }
        }
        Field { label: "Folder", help: "Below the root folder.".to_string(),
            div { class: "row tight",
                input { class: "input grow", "aria-label": "Folder", value: "{folder}", disabled,
                    oninput: move |e| folder.set(e.value()) }
                button { class: "btn", r#type: "button", disabled: disabled || suggesting(),
                    title: "Use the folder name the naming settings give",
                    onclick: suggest,
                    "Use naming format"
                }
            }
        }
        p { class: "path-preview mono", {join_path(&root(), &folder())} }
        if changed {
            Switch { checked: move_files(), disabled, label: "Move the files to the new folder", onchange: move |v| move_files.set(v) }
            p { class: "help small",
                if move_files() {
                    "The folder is renamed, or copied then removed on another disk. The new folder must be missing or empty."
                } else {
                    "Only the recorded folder changes; use this when the files are already there, then scan the disk."
                }
            }
        }
    }
}

/// The series Edit dialog.
#[component]
pub fn EditSeriesDialog(detail: SeriesDetail, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let choices = use_choices();
    let d = detail.clone();
    let mut monitored = use_signal(|| d.item.monitored);
    let mut monitor_new = use_signal(|| d.monitor_new_items);
    let mut season_folder = use_signal(|| d.season_folder);
    let mut profile = use_signal(|| d.item.quality_profile_id);
    let mut series_type = use_signal(|| d.item.series_type);
    let mut language = use_signal(|| d.metadata_language.unwrap_or(1));
    let mut grouping = use_signal(|| d.automatic_grouping);
    let tags = use_signal(|| d.item.tags.clone());
    let (root0, folder0) = split_path(&d.item.path, &d.root_folder_path);
    let root = use_signal(move || root0.clone());
    let folder = use_signal(move || folder0.clone());
    let move_files = use_signal(|| true);
    let mut busy = use_signal(|| false);
    let mut failure = use_signal(|| None::<String>);
    let target = join_path(&root(), &folder());
    let path_changed = target.trim_end_matches('/') != d.item.path.trim_end_matches(['/', '\\']);
    let folder_invalid = matches!(folder().trim(), "" | "." | "..");
    let id = d.item.id;
    let is_tvdb = d.item.metadata_provider == MetadataProvider::Tvdb;
    let groupable = matches!(
        d.item.metadata_provider,
        MetadataProvider::AniList | MetadataProvider::MyAnimeList
    );
    let save = {
        let d = d.clone();
        let target = target.clone();
        move |_| {
            let mut tag_list = tags();
            tag_list.sort_unstable();
            let mut before = d.item.tags.clone();
            before.sort_unstable();
            let u = SeriesUpdate {
                monitored: (monitored() != d.item.monitored).then_some(monitored()),
                quality_profile_id: (profile() != d.item.quality_profile_id).then_some(profile()),
                series_type: (series_type() != d.item.series_type).then_some(series_type()),
                season_folder: (season_folder() != d.season_folder).then_some(season_folder()),
                monitor_new_items: (monitor_new() != d.monitor_new_items).then_some(monitor_new()),
                tags: (tag_list != before).then_some(tag_list),
                seasons: None,
                metadata_language: (is_tvdb && language() != d.metadata_language.unwrap_or(1))
                    .then_some(language()),
                automatic_grouping: (groupable && grouping() != d.automatic_grouping)
                    .then_some(grouping()),
                path: path_changed.then(|| target.clone()),
                move_files: path_changed && move_files(),
            };
            let c = client.clone();
            busy.set(true);
            failure.set(None);
            spawn(async move {
                match c.update_series(id, &u).await {
                    Ok(_) => {
                        app.success("Series saved.");
                        on_close.call(());
                    }
                    Err(e) => failure.set(Some(e.to_string())),
                }
                busy.set(false);
            });
        }
    };
    let body = match &*choices.read() {
        None => rsx! { Loading {} },
        Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
        Some(Ok((profiles, all_tags, roots))) => rsx! {
            Field { label: "Monitored", help: "Kitsunarr searches for and downloads the monitored episodes of a monitored series.".to_string(),
                Switch { checked: monitored(), disabled: busy(), label: "Monitored", onchange: move |v| monitored.set(v) }
            }
            Field { label: "Monitor new seasons", help: "Whether seasons a refresh discovers are monitored.".to_string(),
                select { class: "select", "aria-label": "Monitor new seasons", disabled: busy(),
                    value: monitor_new().as_str(),
                    onchange: move |e: FormEvent| {
                        if let Ok(m) = e.value().parse::<MonitorNewItems>() {
                            monitor_new.set(m);
                        }
                    },
                    option { value: "all", "All seasons" }
                    option { value: "none", "None" }
                }
            }
            Field { label: "Season folders", help: "Sort episodes into season folders.".to_string(),
                Switch { checked: season_folder(), disabled: busy(), label: "Use season folders", onchange: move |v| season_folder.set(v) }
            }
            Field { label: "Quality profile",
                select { class: "select", "aria-label": "Quality profile", disabled: busy(),
                    onchange: move |e: FormEvent| {
                        if let Ok(p) = e.value().parse() {
                            profile.set(p);
                        }
                    },
                    for p in profiles.iter() {
                        option { key: "{p.id}", value: "{p.id}", selected: p.id == profile(), "{p.name}" }
                    }
                }
            }
            Field { label: "Series type", help: "Decides how episodes are numbered when searching and importing.".to_string(),
                select { class: "select", "aria-label": "Series type", disabled: busy(),
                    value: series_type().as_str(),
                    onchange: move |e: FormEvent| {
                        if let Ok(t) = e.value().parse::<SeriesType>() {
                            series_type.set(t);
                        }
                    },
                    for (t, label) in super::add::SERIES_TYPES.iter() { option { key: "{t}", value: t.as_str(), "{label}" } }
                }
            }
            if is_tvdb {
                Field { label: "Metadata language", help: "TheTVDB titles in this language; changing it refreshes the series.".to_string(),
                    select { class: "select", "aria-label": "Metadata language", disabled: busy(),
                        value: "{language}",
                        onchange: move |e: FormEvent| {
                            if let Ok(l) = e.value().parse::<i32>() {
                                language.set(l);
                            }
                        },
                        for l in kitsunarr_domain::Language::all().iter().filter(|l| l.id > 0) {
                            option { key: "{l.id}", value: "{l.id}", "{l.name}" }
                        }
                    }
                }
            }
            if groupable {
                Field { label: "Grouping", help: "Disabled removes the series from its anime group and keeps it out of automatic grouping.".to_string(),
                    select { class: "select", "aria-label": "Grouping", disabled: busy(),
                        value: if grouping() { "automatic" } else { "disabled" },
                        onchange: move |e: FormEvent| grouping.set(e.value() != "disabled"),
                        option { value: "automatic", "Automatic (official relations)" }
                        option { value: "disabled", "Disabled" }
                    }
                }
            }
            FolderFields {
                roots: roots.clone(), root, folder, move_files,
                changed: path_changed, disabled: busy(),
                title: d.item.title.clone(), year: d.item.year, movie: false,
            }
            Field { label: "Tags",
                TagChips { all: all_tags.clone(), selected: tags(), on_toggle: move |t| toggle(tags, t) }
            }
            if folder_invalid { div { class: "alert danger", role: "alert", "Enter a folder name." } }
            if let Some(f) = failure() { div { class: "alert danger", role: "alert", "{f}" } }
        },
    };
    rsx! {
        Modal {
            title: format!("Edit — {}", d.item.title),
            on_close: move |_| { if !busy() { on_close.call(()) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || folder_invalid, onclick: save,
                    if busy() { "Saving…" } else { "Save" }
                }
            },
            {body}
        }
    }
}

const AVAILABILITY: &[(MinimumAvailability, &str)] = &[
    (MinimumAvailability::Announced, "Announced"),
    (MinimumAvailability::InCinemas, "In cinemas"),
    (MinimumAvailability::Released, "Released"),
];

/// The movie Edit dialog.
#[component]
pub fn EditMovieDialog(detail: MovieDetail, on_close: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let choices = use_choices();
    let d = detail.clone();
    let mut monitored = use_signal(|| d.item.monitored);
    let mut availability = use_signal(|| d.item.minimum_availability);
    let mut profile = use_signal(|| d.item.quality_profile_id);
    let mut grouping = use_signal(|| d.automatic_grouping);
    let tags = use_signal(|| d.item.tags.clone());
    let (root0, folder0) = split_path(&d.item.path, &d.root_folder_path);
    let root = use_signal(move || root0.clone());
    let folder = use_signal(move || folder0.clone());
    let move_files = use_signal(|| true);
    let mut busy = use_signal(|| false);
    let mut failure = use_signal(|| None::<String>);
    let target = join_path(&root(), &folder());
    let path_changed = target.trim_end_matches('/') != d.item.path.trim_end_matches(['/', '\\']);
    let folder_invalid = matches!(folder().trim(), "" | "." | "..");
    let id = d.item.id;
    let groupable = matches!(
        d.item.metadata_provider,
        MetadataProvider::TmdbMovie | MetadataProvider::AniList | MetadataProvider::MyAnimeList
    );
    let save = {
        let d = d.clone();
        let target = target.clone();
        move |_| {
            let mut tag_list = tags();
            tag_list.sort_unstable();
            let mut before = d.item.tags.clone();
            before.sort_unstable();
            let u = MovieUpdate {
                monitored: (monitored() != d.item.monitored).then_some(monitored()),
                quality_profile_id: (profile() != d.item.quality_profile_id).then_some(profile()),
                tags: (tag_list != before).then_some(tag_list),
                minimum_availability: (availability() != d.item.minimum_availability)
                    .then_some(availability()),
                automatic_grouping: (groupable && grouping() != d.automatic_grouping)
                    .then_some(grouping()),
                path: path_changed.then(|| target.clone()),
                move_files: path_changed && move_files(),
            };
            let c = client.clone();
            busy.set(true);
            failure.set(None);
            spawn(async move {
                match c.update_movie(id, &u).await {
                    Ok(_) => {
                        app.success("Movie saved.");
                        on_close.call(());
                    }
                    Err(e) => failure.set(Some(e.to_string())),
                }
                busy.set(false);
            });
        }
    };
    let body = match &*choices.read() {
        None => rsx! { Loading {} },
        Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
        Some(Ok((profiles, all_tags, roots))) => rsx! {
            Field { label: "Monitored", help: "Kitsunarr searches for and downloads a monitored movie.".to_string(),
                Switch { checked: monitored(), disabled: busy(), label: "Monitored", onchange: move |v| monitored.set(v) }
            }
            Field { label: "Minimum availability", help: "When the movie counts as available for automatic searches.".to_string(),
                select { class: "select", "aria-label": "Minimum availability", disabled: busy(),
                    value: availability().as_str(),
                    onchange: move |e: FormEvent| {
                        if let Ok(a) = e.value().parse::<MinimumAvailability>() {
                            availability.set(a);
                        }
                    },
                    for (a, label) in AVAILABILITY.iter() { option { key: "{a}", value: a.as_str(), "{label}" } }
                }
            }
            Field { label: "Quality profile",
                select { class: "select", "aria-label": "Quality profile", disabled: busy(),
                    onchange: move |e: FormEvent| {
                        if let Ok(p) = e.value().parse() {
                            profile.set(p);
                        }
                    },
                    for p in profiles.iter() {
                        option { key: "{p.id}", value: "{p.id}", selected: p.id == profile(), "{p.name}" }
                    }
                }
            }
            if groupable {
                Field { label: "Grouping", help: "Disabled removes the film from its collection and keeps it out of automatic grouping.".to_string(),
                    select { class: "select", "aria-label": "Grouping", disabled: busy(),
                        value: if grouping() { "automatic" } else { "disabled" },
                        onchange: move |e: FormEvent| grouping.set(e.value() != "disabled"),
                        option { value: "automatic", "Automatic (official relations)" }
                        option { value: "disabled", "Disabled" }
                    }
                }
            }
            FolderFields {
                roots: roots.clone(), root, folder, move_files,
                changed: path_changed, disabled: busy(),
                title: d.item.title.clone(), year: d.item.year, movie: true,
            }
            Field { label: "Tags",
                TagChips { all: all_tags.clone(), selected: tags(), on_toggle: move |t| toggle(tags, t) }
            }
            if folder_invalid { div { class: "alert danger", role: "alert", "Enter a folder name." } }
            if let Some(f) = failure() { div { class: "alert danger", role: "alert", "{f}" } }
        },
    };
    rsx! {
        Modal {
            title: format!("Edit — {}", d.item.title),
            on_close: move |_| { if !busy() { on_close.call(()) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: busy() || folder_invalid, onclick: save,
                    if busy() { "Saving…" } else { "Save" }
                }
            },
            {body}
        }
    }
}

/// The library's bulk editor (Sonarr's series editor "Edit"): the chosen settings for every
/// selected title; "No change" leaves a setting as each title has it.
#[component]
pub fn BulkEditDialog(movie: bool, ids: Vec<i64>, on_close: EventHandler<bool>) -> Element {
    use kitsunarr_domain::api::library::BulkEdit;
    let app = use_app();
    let client = use_client();
    let choices = use_choices();
    let mut monitored = use_signal(String::new);
    let mut monitor_new = use_signal(String::new);
    let mut profile = use_signal(String::new);
    let mut series_type = use_signal(String::new);
    let mut season_folder = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let count = ids.len();
    let unchanged = monitored().is_empty()
        && monitor_new().is_empty()
        && profile().is_empty()
        && series_type().is_empty()
        && season_folder().is_empty();
    let save = move |_| {
        let edit = BulkEdit {
            ids: ids.clone(),
            monitored: monitored().parse().ok(),
            quality_profile_id: profile().parse().ok(),
            monitor_new_items: monitor_new().parse().ok(),
            series_type: series_type().parse().ok(),
            season_folder: season_folder().parse().ok(),
            ..Default::default()
        };
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            let result = if movie {
                c.bulk_edit_movies(&edit).await
            } else {
                c.bulk_edit_series(&edit).await
            };
            busy.set(false);
            match result {
                Ok(r) => {
                    app.success(format!("Updated {} items.", r.updated));
                    on_close.call(true);
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let profiles = match &*choices.read() {
        Some(Ok((profiles, _, _))) => profiles.clone(),
        _ => Vec::new(),
    };
    rsx! {
        Modal {
            title: format!("Edit {count} selected"),
            on_close: move |_| { if !busy() { on_close.call(false) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(false), "Cancel" }
                button { class: "btn primary", disabled: busy() || unchanged, onclick: save,
                    if busy() { "Saving…" } else { "Apply changes" }
                }
            },
            Field { label: "Monitored",
                select { class: "select", "aria-label": "Monitored", value: monitored(), disabled: busy(),
                    onchange: move |e: FormEvent| monitored.set(e.value()),
                    option { value: "", "No change" }
                    option { value: "true", "Monitored" }
                    option { value: "false", "Unmonitored" }
                }
            }
            if !movie {
                Field { label: "Monitor new seasons",
                    select { class: "select", "aria-label": "Monitor new seasons", value: monitor_new(), disabled: busy(),
                        onchange: move |e: FormEvent| monitor_new.set(e.value()),
                        option { value: "", "No change" }
                        option { value: "all", "All seasons" }
                        option { value: "none", "None" }
                    }
                }
            }
            Field { label: "Quality profile",
                select { class: "select", "aria-label": "Quality profile", value: profile(), disabled: busy(),
                    onchange: move |e: FormEvent| profile.set(e.value()),
                    option { value: "", "No change" }
                    for p in profiles.iter() { option { key: "{p.id}", value: "{p.id}", "{p.name}" } }
                }
            }
            if !movie {
                Field { label: "Series type",
                    select { class: "select", "aria-label": "Series type", value: series_type(), disabled: busy(),
                        onchange: move |e: FormEvent| series_type.set(e.value()),
                        option { value: "", "No change" }
                        for (t, label) in super::add::SERIES_TYPES.iter() { option { key: "{t}", value: t.as_str(), "{label}" } }
                    }
                }
                Field { label: "Season folders",
                    select { class: "select", "aria-label": "Season folders", value: season_folder(), disabled: busy(),
                        onchange: move |e: FormEvent| season_folder.set(e.value()),
                        option { value: "", "No change" }
                        option { value: "true", "Use season folders" }
                        option { value: "false", "No season folders" }
                    }
                }
            }
        }
    }
}

/// Sonarr's tags editor for selected titles: add, remove or replace their tags.
#[component]
pub fn BulkTagsDialog(movie: bool, ids: Vec<i64>, on_close: EventHandler<bool>) -> Element {
    use kitsunarr_domain::api::library::BulkEdit;
    let app = use_app();
    let client = use_client();
    let choices = use_choices();
    let chosen = use_signal(Vec::<i64>::new);
    let mut mode = use_signal(|| "add".to_string());
    let mut busy = use_signal(|| false);
    let count = ids.len();
    let save = move |_| {
        let tags = chosen();
        let mut edit = BulkEdit {
            ids: ids.clone(),
            ..Default::default()
        };
        match mode().as_str() {
            "remove" => edit.remove_tags = tags,
            "replace" => edit.replace_tags = Some(tags),
            _ => edit.add_tags = tags,
        }
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            let result = if movie {
                c.bulk_edit_movies(&edit).await
            } else {
                c.bulk_edit_series(&edit).await
            };
            busy.set(false);
            match result {
                Ok(r) => {
                    app.success(format!("Tags updated on {} items.", r.updated));
                    on_close.call(true);
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let nothing = chosen().is_empty() && mode() != "replace";
    let help = match mode().as_str() {
        "remove" => "Removes the chosen tags from every selected title.",
        "replace" => "Every selected title gets exactly the chosen tags (none removes all).",
        _ => "Adds the chosen tags to every selected title.",
    };
    rsx! {
        Modal {
            title: format!("Tags — {count} selected"),
            on_close: move |_| { if !busy() { on_close.call(false) } },
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", disabled: busy(), onclick: move |_| on_close.call(false), "Cancel" }
                button { class: "btn primary", disabled: busy() || nothing, onclick: save,
                    if busy() { "Saving…" } else { "Apply tags" }
                }
            },
            Field { label: "Apply tags",
                select { class: "select", "aria-label": "Apply tags", value: mode(), disabled: busy(),
                    onchange: move |e: FormEvent| mode.set(e.value()),
                    option { value: "add", "Add" }
                    option { value: "remove", "Remove" }
                    option { value: "replace", "Replace" }
                }
            }
            p { class: "help small", "{help}" }
            match &*choices.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.to_string() } },
                Some(Ok((_, all_tags, _))) => rsx! {
                    Field { label: "Tags",
                        TagChips { all: all_tags.clone(), selected: chosen(), on_toggle: move |t| toggle(chosen, t) }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{join_path, split_path};

    #[test]
    fn folders_split_below_their_root() {
        assert_eq!(
            split_path("/tv/Breaking Bad (2008)", "/tv"),
            ("/tv".to_string(), "Breaking Bad (2008)".to_string())
        );
        assert_eq!(
            split_path("/tv/Anime/Show", "/tv/"),
            ("/tv".to_string(), "Anime/Show".to_string())
        );
        // A folder outside its recorded root folder: its parent stands in.
        assert_eq!(
            split_path("/elsewhere/Show", "/tv"),
            ("/elsewhere".to_string(), "Show".to_string())
        );
        assert_eq!(join_path("/tv/", "/Show/"), "/tv/Show");
    }
}
