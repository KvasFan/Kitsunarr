//! Settings › Media management › Naming (live samples and the token picker), Settings › UI,
//! and Settings › General › Host and Logging (Tanukarr's `Naming`, `NamingModal`,
//! `UISettings` and the host section of `GeneralSettings`).

use dioxus::prelude::*;
use kitsunarr_domain::api::settings::{
    GeneralSettings, LONG_DATE_FORMATS, NamingSettings, SHORT_DATE_FORMATS, TIME_FORMATS,
    WEEK_COLUMN_HEADERS,
};
use kitsunarr_domain::api::views::{HostSettings, NamingExamples};

use crate::components::common::{Field, Loading, Modal, Switch};
use crate::state::{use_app, use_client};

// ---------------------------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------------------------

/// The naming fields, by their API name.
fn get(n: &NamingSettings, key: &str) -> String {
    match key {
        "standardEpisodeFormat" => n.standard_episode_format.clone(),
        "dailyEpisodeFormat" => n.daily_episode_format.clone(),
        "animeEpisodeFormat" => n.anime_episode_format.clone(),
        "seriesFolderFormat" => n.series_folder_format.clone(),
        "seasonFolderFormat" => n.season_folder_format.clone(),
        "specialsFolderFormat" => n.specials_folder_format.clone(),
        "movieFormat" => n.movie_file_format.clone(),
        "movieFolderFormat" => n.movie_folder_format.clone(),
        "animeGroupFolderFormat" => n.anime_group_folder_format.clone(),
        "animeGroupSeasonFolderFormat" => n.anime_group_season_folder_format.clone(),
        "animeGroupArcFolderFormat" => n.anime_group_arc_folder_format.clone(),
        "animeGroupMovieFolderFormat" => n.anime_group_movie_folder_format.clone(),
        "animeGroupMovieSagaFolderFormat" => n.anime_group_movie_saga_folder_format.clone(),
        "customColonReplacementFormat" => n.custom_colon_replacement_format.clone(),
        _ => String::new(),
    }
}

fn set(n: &mut NamingSettings, key: &str, v: String) {
    let slot = match key {
        "standardEpisodeFormat" => &mut n.standard_episode_format,
        "dailyEpisodeFormat" => &mut n.daily_episode_format,
        "animeEpisodeFormat" => &mut n.anime_episode_format,
        "seriesFolderFormat" => &mut n.series_folder_format,
        "seasonFolderFormat" => &mut n.season_folder_format,
        "specialsFolderFormat" => &mut n.specials_folder_format,
        "movieFormat" => &mut n.movie_file_format,
        "movieFolderFormat" => &mut n.movie_folder_format,
        "animeGroupFolderFormat" => &mut n.anime_group_folder_format,
        "animeGroupSeasonFolderFormat" => &mut n.anime_group_season_folder_format,
        "animeGroupArcFolderFormat" => &mut n.anime_group_arc_folder_format,
        "animeGroupMovieFolderFormat" => &mut n.anime_group_movie_folder_format,
        "animeGroupMovieSagaFolderFormat" => &mut n.anime_group_movie_saga_folder_format,
        "customColonReplacementFormat" => &mut n.custom_colon_replacement_format,
        _ => return,
    };
    *slot = v;
}

/// Which token groups the picker offers for a field (Tanukarr's `NamingModalOptions`).
#[derive(Clone, Copy, PartialEq, Default)]
struct TokenFlags {
    season: bool,
    season_title: bool,
    episode: bool,
    daily: bool,
    anime: bool,
    additional: bool,
    group: bool,
}

fn flags_for(key: &str) -> TokenFlags {
    let f = TokenFlags::default();
    match key {
        "standardEpisodeFormat" => TokenFlags {
            season: true,
            episode: true,
            additional: true,
            ..f
        },
        "dailyEpisodeFormat" => TokenFlags {
            season: true,
            episode: true,
            daily: true,
            additional: true,
            ..f
        },
        "animeEpisodeFormat" => TokenFlags {
            season: true,
            episode: true,
            anime: true,
            additional: true,
            ..f
        },
        "movieFormat" | "movieFolderFormat" => TokenFlags {
            additional: key == "movieFormat",
            ..f
        },
        "seasonFolderFormat" | "specialsFolderFormat" => TokenFlags {
            season: true,
            season_title: true,
            ..f
        },
        k if k.starts_with("animeGroup") => TokenFlags { group: true, ..f },
        _ => f,
    }
}

type Tokens = &'static [(&'static str, &'static str)];

const FILE_NAME_PRESETS: Tokens = &[
    (
        "{Series TitleYear} - S{season:00}E{episode:00} - {Episode CleanTitle} {Quality Full}",
        "The Series Title's! (2010) - S01E01 - Episode Title WEBDL-1080p Proper",
    ),
    (
        "{Series TitleYear} - {season:0}x{episode:00} - {Episode CleanTitle} {Quality Full}",
        "The Series Title's! (2010) - 1x01 - Episode Title WEBDL-1080p Proper",
    ),
    (
        "{Series.CleanTitleYear}.S{season:00}E{episode:00}.{Episode.CleanTitle}.{Quality.Full}",
        "The.Series.Title's!.2010.S01E01.Episode.Title.WEBDL-1080p.Proper",
    ),
];
const DAILY_PRESETS: Tokens = &[
    (
        "{Series TitleYear} - {Air-Date} - {Episode CleanTitle} {Quality Full}",
        "The Series Title's! (2010) - 2013-10-30 - Episode Title WEBDL-1080p Proper",
    ),
    (
        "{Series.CleanTitleYear}.{Air.Date}.{Episode.CleanTitle}.{Quality.Full}",
        "The.Series.Title's!.2010.2013.10.30.Episode.Title.WEBDL-1080p.Proper",
    ),
];
const ANIME_PRESETS: Tokens = &[
    (
        "{Series TitleYear} - S{season:00}E{episode:00} - {absolute:000} - {Episode CleanTitle} {Quality Full}",
        "The Series Title's! (2010) - S01E01 - 001 - Episode Title WEBDL-1080p Proper",
    ),
    (
        "{Series TitleYear} - {season:0}x{episode:00} - {absolute:000} - {Episode CleanTitle} {Quality Full}",
        "The Series Title's! (2010) - 1x01 - 001 - Episode Title WEBDL-1080p Proper",
    ),
    (
        "{Series.CleanTitleYear}.S{season:00}E{episode:00}.{absolute:000}.{Episode.CleanTitle}.{Quality.Full}",
        "The.Series.Title's!.2010.S01E01.001.Episode.Title.WEBDL-1080p.Proper",
    ),
];
const SERIES_TOKENS: Tokens = &[
    ("{Series Title}", "The Series Title's!"),
    ("{Series CleanTitle}", "The Series Title's!"),
    ("{Series TitleYear}", "The Series Title's! (2010)"),
    ("{Series CleanTitleYear}", "The Series Title's! 2010"),
    ("{Series TitleWithoutYear}", "The Series Title's!"),
    ("{Series CleanTitleWithoutYear}", "The Series Title's!"),
    ("{Series TitleThe}", "Series Title's!, The"),
    ("{Series CleanTitleThe}", "Series Title's!, The"),
    ("{Series TitleTheYear}", "Series Title's!, The (2010)"),
    ("{Series CleanTitleTheYear}", "Series Title's!, The 2010"),
    ("{Series TitleTheWithoutYear}", "Series Title's!, The"),
    ("{Series CleanTitleTheWithoutYear}", "Series Title's!, The"),
    ("{Series TitleFirstCharacter}", "S"),
    ("{Series Year}", "2010"),
];
const MOVIE_TOKENS: Tokens = &[
    ("{Movie Title}", "The Movie Title"),
    ("{Movie CleanTitle}", "The Movie Title"),
    ("{Release Year}", "2010"),
];
const SERIES_ID_TOKENS: Tokens = &[
    ("{Plex Id}", "{tvdb-12345}"),
    ("{ImdbId}", "tt12345"),
    ("{TvdbId}", "12345"),
    ("{TmdbId}", "11223"),
    ("{TvMazeId}", "54321"),
];
const GROUP_TOKENS: Tokens = &[
    ("{Group Title}", "Generated anime group"),
    ("{Group Arc Title}", "War Of Three Titans Arc"),
    ("{Group Season:00}", "02"),
    ("{Group Movie:00}", "01"),
];
const SEASON_TOKENS: Tokens = &[
    ("{Season Title}", "Stone Wars"),
    ("{Season Year}", "2021"),
    ("{season:0}", "1"),
    ("{season:00}", "01"),
];
const SEASON_TITLE_TOKENS: Tokens = &[
    ("{Season Title}", "Part 1!"),
    ("{Season CleanTitle}", "Part 1"),
];
const EPISODE_TOKENS: Tokens = &[("{episode:0}", "1"), ("{episode:00}", "01")];
const AIR_DATE_TOKENS: Tokens = &[("{Air-Date}", "2016-03-20"), ("{Air Date}", "2016 03 20")];
const ABSOLUTE_TOKENS: Tokens = &[
    ("{absolute:0}", "1"),
    ("{absolute:00}", "01"),
    ("{absolute:000}", "001"),
];
const EPISODE_TITLE_TOKENS: Tokens = &[
    ("{Episode Title}", "Episode's Title"),
    ("{Episode CleanTitle}", "Episodes Title"),
];
const QUALITY_TOKENS: Tokens = &[
    ("{Quality Full}", "WEBDL-1080p Proper"),
    ("{Quality Title}", "WEBDL-1080p"),
];
const MEDIA_INFO_TOKENS: Tokens = &[
    ("{MediaInfo Simple}", "x264 DTS"),
    ("{MediaInfo Full}", "x264 DTS [EN+DE]"),
    ("{MediaInfo AudioCodec}", "DTS"),
    ("{MediaInfo AudioChannels}", "5.1"),
    ("{MediaInfo AudioLanguages}", "[EN+DE]"),
    ("{MediaInfo AudioLanguagesAll}", "[EN]"),
    ("{MediaInfo SubtitleLanguages}", "[DE]"),
    ("{MediaInfo VideoCodec}", "x264"),
    ("{MediaInfo VideoBitDepth}", "10"),
    ("{MediaInfo VideoDynamicRange}", "HDR"),
    ("{MediaInfo VideoDynamicRangeType}", "DV HDR10"),
];
const OTHER_TOKENS: Tokens = &[
    ("{Release Group}", "Rls Grp"),
    ("{Custom Formats}", "iNTERNAL"),
    ("{Custom Format:FormatName}", "Surround Sound"),
];
const ANIME_OTHER_TOKENS: Tokens = &[("{Release Hash}", "ABCDEFGH")];
const ORIGINAL_TOKENS: Tokens = &[
    (
        "{Original Title}",
        "The.Series.Title's!.S01E01.WEBDL.1080p.x264-EVOLVE",
    ),
    (
        "{Original Filename}",
        "the.series.title's!.s01e01.webdl.1080p.x264-EVOLVE",
    ),
];

/// A group of tokens: (legend, tokens, presets replace the whole format).
type Group = (&'static str, Vec<(&'static str, &'static str)>, bool);

/// Tanukarr's `getNamingTokenGroups`, with its per-field filters.
fn token_groups(key: &str) -> Vec<Group> {
    let f = flags_for(key);
    let mut groups: Vec<Group> = Vec::new();
    if f.episode {
        let mut presets = Vec::new();
        if f.daily {
            presets.extend_from_slice(DAILY_PRESETS);
        }
        if f.anime {
            presets.extend_from_slice(ANIME_PRESETS);
        }
        presets.extend_from_slice(FILE_NAME_PRESETS);
        groups.push(("Presets", presets, true));
    }
    match key {
        "movieFormat" => groups.push((
            "Presets",
            vec![(
                "{Movie Title} ({Release Year}) {Quality Full}",
                "The Movie Title (2010) WEBDL-1080p",
            )],
            true,
        )),
        "seriesFolderFormat" => groups.push((
            "Presets",
            vec![(
                "{Series TitleYear} {Plex Id}",
                "The Series Title's! (2010) {tvdb-12345}",
            )],
            true,
        )),
        _ => {}
    }
    if key.starts_with("movie") {
        groups.push(("Movie", MOVIE_TOKENS.to_vec(), false));
    } else {
        groups.push(("Series", SERIES_TOKENS.to_vec(), false));
        groups.push(("Series ID", SERIES_ID_TOKENS.to_vec(), false));
    }
    if f.group || f.episode {
        groups.push(("Anime group", GROUP_TOKENS.to_vec(), false));
    }
    if f.season || f.season_title {
        let mut t = Vec::new();
        if f.season {
            t.extend_from_slice(SEASON_TOKENS);
        }
        if f.season_title {
            t.extend_from_slice(SEASON_TITLE_TOKENS);
        }
        groups.push(("Season", t, false));
    }
    if f.episode {
        groups.push(("Episode", EPISODE_TOKENS.to_vec(), false));
        groups.push(("Air date", AIR_DATE_TOKENS.to_vec(), false));
        if f.anime {
            groups.push(("Absolute episode number", ABSOLUTE_TOKENS.to_vec(), false));
        }
    }
    if f.additional {
        if key != "movieFormat" {
            groups.push(("Episode title", EPISODE_TITLE_TOKENS.to_vec(), false));
        }
        groups.push(("Quality", QUALITY_TOKENS.to_vec(), false));
        groups.push(("Media info", MEDIA_INFO_TOKENS.to_vec(), false));
        let mut other = OTHER_TOKENS.to_vec();
        if f.anime {
            other.extend_from_slice(ANIME_OTHER_TOKENS);
        }
        groups.push(("Other", other, false));
        groups.push(("Original", ORIGINAL_TOKENS.to_vec(), false));
    }
    if f.group {
        // Only the tokens a group folder can use.
        let mut allowed = vec![
            "{Group Title}",
            "{Group Arc Title}",
            "{Series Title}",
            "{Series Year}",
        ];
        if key == "animeGroupSeasonFolderFormat" {
            allowed.push("{Group Season:00}");
        }
        if key == "animeGroupMovieFolderFormat" || key == "animeGroupMovieSagaFolderFormat" {
            allowed.push("{Group Movie:00}");
        }
        groups = groups
            .into_iter()
            .map(|(l, t, p)| {
                (
                    l,
                    t.into_iter()
                        .filter(|(tok, _)| allowed.contains(tok))
                        .collect(),
                    p,
                )
            })
            .filter(|(_, t, _): &Group| !t.is_empty())
            .collect();
    }
    groups
}

/// The token as inserted: spaces become the separator, then the case applies.
fn styled(token: &str, separator: &str, case: &str) -> String {
    let t = token.replace(' ', separator);
    match case {
        "lower" => t.to_lowercase(),
        "upper" => t.to_uppercase(),
        _ => t,
    }
}

/// The caret of an input, or the end of its value.
fn selection(id: &str, len: usize) -> (usize, usize) {
    use wasm_bindgen::JsCast;
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        .and_then(|i| {
            let start = i.selection_start().ok()??;
            let end = i.selection_end().ok()??;
            Some((start as usize, end as usize))
        })
        .unwrap_or((len, len))
}

fn place_caret(id: &str, at: usize) {
    use wasm_bindgen::JsCast;
    if let Some(i) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        let _ = i.focus();
        let _ = i.set_selection_range(at as u32, at as u32);
    }
}

/// Inserts `token` at the UTF-16 selection of `value` (as browsers count).
fn insert_at(value: &str, (start, end): (usize, usize), token: &str) -> (String, usize) {
    let units: Vec<u16> = value.encode_utf16().collect();
    let start = start.min(units.len());
    let end = end.clamp(start, units.len());
    let before = String::from_utf16_lossy(&units[..start]);
    let after = String::from_utf16_lossy(&units[end..]);
    let caret = start + token.encode_utf16().count();
    (format!("{before}{token}{after}"), caret)
}

/// Tanukarr's `NamingModal`: the format with a token picker.
#[component]
fn TokenPicker(
    key_name: String,
    value: String,
    on_change: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut separator = use_signal(|| " ".to_string());
    let mut case = use_signal(|| "title".to_string());
    let input_id = "naming-token-input";
    let episode = flags_for(&key_name).episode || key_name == "movieFormat";
    let groups = token_groups(&key_name);
    let current = value.clone();
    rsx! {
        Modal {
            title: if episode { "File name tokens".to_string() } else { "Folder name tokens".to_string() },
            wide: true,
            on_close: move |_| on_close.call(()),
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn primary", onclick: move |_| on_close.call(()), "Close" }
            },
            div { class: "row wrap",
                input { id: input_id, class: "input mono grow", "aria-label": "Format", value: "{value}",
                    oninput: move |e| on_change.call(e.value()) }
                label { class: "small row tight", "Separator"
                    select { class: "select", "aria-label": "Separator", value: "{separator}", onchange: move |e: FormEvent| separator.set(e.value()),
                        option { value: " ", "Space ( )" }
                        option { value: ".", "Period (.)" }
                        option { value: "_", "Underscore (_)" }
                        option { value: "-", "Dash (-)" }
                    }
                }
                label { class: "small row tight", "Capitalization"
                    select { class: "select", "aria-label": "Capitalization", value: "{case}", onchange: move |e: FormEvent| case.set(e.value()),
                        option { value: "title", "Default case" }
                        option { value: "lower", "Lowercase" }
                        option { value: "upper", "Uppercase" }
                    }
                }
            }
            p { class: "help small", "Click a token to insert it at the cursor; a preset replaces the whole format." }
            for (legend, tokens, preset) in groups {
                section { key: "{legend}", class: "token-group",
                    h4 { "{legend}" }
                    div { class: if preset { "token-grid presets" } else { "token-grid" },
                        for (token, example) in tokens {
                            {
                                let text = styled(token, &separator(), &case());
                                let shown_example = example.replace(' ', &separator());
                                let current = current.clone();
                                rsx! {
                                    button { key: "{token}", class: "token", r#type: "button", title: "{shown_example}",
                                        onclick: move |_| {
                                            if preset {
                                                on_change.call(text.clone());
                                            } else {
                                                let sel = selection(input_id, current.encode_utf16().count());
                                                let (next, caret) = insert_at(&current, sel, &text);
                                                on_change.call(next);
                                                spawn(async move {
                                                    gloo_timers::future::TimeoutFuture::new(0).await;
                                                    place_caret(input_id, caret);
                                                });
                                            }
                                        },
                                        span { class: "mono", "{text}" }
                                        span { class: "xs muted", "{shown_example}" }
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

/// A format input with its samples (or the invalid-format error) and the token picker button.
#[component]
fn FormatField(
    label: &'static str,
    key_name: &'static str,
    value: String,
    #[props(default)] help: Option<String>,
    examples: Vec<String>,
    #[props(default)] invalid: bool,
    on_change: EventHandler<(&'static str, String)>,
    on_tokens: EventHandler<&'static str>,
) -> Element {
    rsx! {
        div { class: "field naming-field",
            label { "{label}" }
            div { class: "row tight",
                input { class: "input mono grow", "aria-label": "{label}", value: "{value}",
                    "aria-invalid": if invalid { "true" } else { "false" },
                    oninput: move |e| on_change.call((key_name, e.value())) }
                button { class: "btn", r#type: "button", title: "Tokens", "aria-label": "Tokens for {label}", onclick: move |_| on_tokens.call(key_name), "?" }
            }
            if let Some(h) = help { span { class: "help", "{h}" } }
            for (i, e) in examples.iter().enumerate() {
                span { key: "{i}", class: "help naming-example", "{e}" }
            }
            if invalid { span { class: "help danger", "Invalid format" } }
        }
    }
}

/// Settings › Media management › Naming.
#[component]
pub fn NamingCard() -> Element {
    let app = use_app();
    let client = use_client();
    let mut naming = use_signal(|| None::<NamingSettings>);
    let mut examples = use_signal(|| None::<NamingExamples>);
    let mut picker = use_signal(|| None::<&'static str>);
    let mut generation = use_signal(|| 0u64);
    let c0 = client.clone();
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.naming().await {
                Ok(n) => naming.set(Some(n)),
                Err(e) => app.error(&e),
            }
        }
    });
    // Samples follow the unsaved settings, 300 ms after the last change.
    let c1 = client.clone();
    use_effect(move || {
        let Some(n) = naming() else { return };
        let ticket = *generation.peek() + 1;
        generation.set(ticket);
        let c = c1.clone();
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(300).await;
            if *generation.peek() != ticket {
                return;
            }
            if let Ok(e) = c.naming_examples(&n).await
                && *generation.peek() == ticket
            {
                examples.set(Some(e));
            }
        });
    });
    let c2 = client.clone();
    let save = move |_| {
        let Some(n) = naming() else { return };
        let c = c2.clone();
        spawn(async move {
            match c.put_naming(&n).await {
                Ok(saved) => {
                    naming.set(Some(saved));
                    app.success("Naming saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let mut edit = move |key: &'static str, v: String| {
        if let Some(n) = naming.write().as_mut() {
            set(n, key, v);
        }
    };
    let on_change = move |(key, v): (&'static str, String)| edit(key, v);
    let on_tokens = move |key: &'static str| picker.set(Some(key));
    let Some(n) = naming() else {
        return rsx! { div { class: "card", h3 { "Episode and movie naming" } Loading {} } };
    };
    let ex = examples();
    let loaded = ex.is_some();
    let ex = ex.unwrap_or_default();
    let lines = |pairs: &[(&str, &Option<String>)]| -> Vec<String> {
        pairs
            .iter()
            .filter_map(|(label, v)| v.as_ref().map(|v| format!("{label}: {v}")))
            .collect()
    };
    let missing = |v: &Option<String>| loaded && v.is_none();
    let group_formats: [(&'static str, &'static str, &Option<String>); 5] = [
        (
            "Group folder format",
            "animeGroupFolderFormat",
            &ex.anime_group_folder_example,
        ),
        (
            "Group season folder format",
            "animeGroupSeasonFolderFormat",
            &ex.anime_group_season_folder_example,
        ),
        (
            "Group story-arc folder format",
            "animeGroupArcFolderFormat",
            &ex.anime_group_arc_folder_example,
        ),
        (
            "Group film folder format",
            "animeGroupMovieFolderFormat",
            &ex.anime_group_movie_folder_example,
        ),
        (
            "Collection film folder format",
            "animeGroupMovieSagaFolderFormat",
            &ex.anime_group_movie_saga_folder_example,
        ),
    ];
    rsx! {
        div { class: "card", id: "naming",
            h3 { "Episode and movie naming" }
            p { class: "help small faint", "Formats use Sonarr's tokens; the examples below follow your changes before you save." }
            div { class: "stack", style: "margin-bottom: var(--sp-4)",
                Switch { checked: n.rename_episodes, label: "Rename episodes", onchange: move |v| { if let Some(x) = naming.write().as_mut() { x.rename_episodes = v; } } }
                Switch { checked: n.replace_illegal_characters, label: "Replace illegal characters (otherwise they are removed)", onchange: move |v| { if let Some(x) = naming.write().as_mut() { x.replace_illegal_characters = v; } } }
            }
            div { class: "form-grid",
                if n.replace_illegal_characters {
                    Field { label: "Colon replacement", help: "How colons in titles are written in file and folder names.".to_string(),
                        select { class: "select", "aria-label": "Colon replacement",
                            onchange: move |e: FormEvent| { if let Some(x) = naming.write().as_mut() { x.colon_replacement_format = e.value().parse().unwrap_or(4); } },
                            for (v, label) in [(0, "Delete"), (1, "Replace with dash"), (2, "Replace with space dash"), (3, "Replace with space dash space"), (4, "Smart replace (dash or space dash)"), (5, "Custom")] {
                                option { key: "{v}", value: "{v}", selected: n.colon_replacement_format == v, "{label}" }
                            }
                        }
                    }
                    if n.colon_replacement_format == 5 {
                        Field { label: "Custom colon replacement", help: "The characters written instead of a colon.".to_string(),
                            input { class: "input mono", "aria-label": "Custom colon replacement", value: "{n.custom_colon_replacement_format}",
                                oninput: move |e| edit("customColonReplacementFormat", e.value()) }
                        }
                    }
                }
                Field { label: "Multi-episode style",
                    select { class: "select", "aria-label": "Multi-episode style",
                        onchange: move |e: FormEvent| { if let Some(x) = naming.write().as_mut() { x.multi_episode_style = e.value().parse().unwrap_or(5); } },
                        for (v, label) in [(0, "Extend (S01E01-02-03)"), (1, "Duplicate (S01E01.S01E02)"), (2, "Repeat (S01E01E02E03)"), (3, "Scene (S01E01-E02-E03)"), (4, "Range (S01E01-03)"), (5, "Prefixed range (S01E01-E03)")] {
                            option { key: "{v}", value: "{v}", selected: n.multi_episode_style == v, "{label}" }
                        }
                    }
                }
            }
            div { class: "form-grid naming-grid",
                if n.rename_episodes {
                    FormatField { label: "Standard episode format", key_name: "standardEpisodeFormat", value: get(&n, "standardEpisodeFormat"),
                        examples: lines(&[("Single episode", &ex.single_episode_example), ("Multi-episode", &ex.multi_episode_example)]),
                        invalid: missing(&ex.single_episode_example) || missing(&ex.multi_episode_example), on_change, on_tokens }
                    FormatField { label: "Daily episode format", key_name: "dailyEpisodeFormat", value: get(&n, "dailyEpisodeFormat"),
                        examples: lines(&[("Example", &ex.daily_episode_example)]), invalid: missing(&ex.daily_episode_example), on_change, on_tokens }
                    FormatField { label: "Anime episode format", key_name: "animeEpisodeFormat", value: get(&n, "animeEpisodeFormat"),
                        examples: lines(&[("Single episode", &ex.anime_episode_example), ("Multi-episode", &ex.anime_multi_episode_example), ("Anime group movie", &ex.anime_group_movie_episode_example)]),
                        invalid: missing(&ex.anime_episode_example) || missing(&ex.anime_multi_episode_example), on_change, on_tokens }
                }
                FormatField { label: "Movie file format", key_name: "movieFormat", value: get(&n, "movieFormat"),
                    help: "Used when films are renamed.".to_string(),
                    examples: lines(&[("Example", &ex.movie_example)]), on_change, on_tokens }
                FormatField { label: "Movie folder format", key_name: "movieFolderFormat", value: get(&n, "movieFolderFormat"),
                    examples: lines(&[("Example", &ex.movie_folder_example)]), invalid: missing(&ex.movie_folder_example), on_change, on_tokens }
                FormatField { label: "Series folder format", key_name: "seriesFolderFormat", value: get(&n, "seriesFolderFormat"),
                    help: "Used when adding a new series or moving one.".to_string(),
                    examples: lines(&[("Example", &ex.series_folder_example)]), invalid: missing(&ex.series_folder_example), on_change, on_tokens }
                FormatField { label: "Season folder format", key_name: "seasonFolderFormat", value: get(&n, "seasonFolderFormat"),
                    examples: lines(&[("Example", &ex.season_folder_example)]), invalid: missing(&ex.season_folder_example), on_change, on_tokens }
                FormatField { label: "Specials folder format", key_name: "specialsFolderFormat", value: get(&n, "specialsFolderFormat"),
                    examples: lines(&[("Example", &ex.specials_folder_example)]), invalid: missing(&ex.specials_folder_example), on_change, on_tokens }
                for (label, key, example) in group_formats {
                    FormatField { key: "{key}", label, key_name: key, value: get(&n, key),
                        help: "Anime groups and collections: titles go to {root}/{group folder}/{member folder}.".to_string(),
                        examples: lines(&[("Example", example)]), invalid: missing(example), on_change, on_tokens }
                }
            }
            div { class: "row end", button { class: "btn primary", onclick: save, "Save naming" } }
        }
        if let Some(key) = picker() {
            TokenPicker { key_name: key.to_string(), value: get(&n, key), on_change: move |v: String| edit(key, v), on_close: move |_| picker.set(None) }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Settings › UI
// ---------------------------------------------------------------------------------------------

/// A sample of a moment format on 25 March 2014, 17:30 (Sonarr's `createDateFormatOption`).
fn sample(format: &str) -> String {
    let at = chrono::NaiveDate::from_ymd_opt(2014, 3, 25)
        .and_then(|d| d.and_hms_opt(17, 30, 0))
        .unwrap_or_default();
    kitsunarr_domain::moment::format(&at, format)
}

/// The time zones the browser knows (`Intl.supportedValuesOf('timeZone')`).
fn time_zones() -> Vec<String> {
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::JsCast;
    let list = (|| {
        let intl = Reflect::get(&js_sys::global(), &"Intl".into()).ok()?;
        let f: Function = Reflect::get(&intl, &"supportedValuesOf".into())
            .ok()?
            .dyn_into()
            .ok()?;
        let v: Array = f.call1(&intl, &"timeZone".into()).ok()?.dyn_into().ok()?;
        Some(v.iter().filter_map(|z| z.as_string()).collect::<Vec<_>>())
    })();
    list.unwrap_or_default()
}

/// Sonarr's Settings › UI: calendar, dates, style.
#[component]
pub fn SettingsUi() -> Element {
    let app = use_app();
    let client = use_client();
    let mut form = use_signal(|| None::<GeneralSettings>);
    let zones = use_hook(time_zones);
    let c0 = client.clone();
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.general().await {
                Ok(g) => form.set(Some(g)),
                Err(e) => app.error(&e),
            }
        }
    });
    let save = move |_| {
        let Some(g) = form() else { return };
        let c = client.clone();
        spawn(async move {
            match c.put_general(&g).await {
                Ok(saved) => {
                    crate::util::apply_ui_settings(&saved);
                    form.set(Some(saved));
                    app.success("Interface settings saved.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let mut edit = move |f: &dyn Fn(&mut GeneralSettings)| {
        if let Some(g) = form.write().as_mut() {
            f(g);
        }
    };
    let Some(g) = form() else {
        return rsx! { div { class: "page-head", h1 { "UI" } } Loading {} };
    };
    let theme = app.theme.read().clone();
    rsx! {
        div { class: "page-head", h1 { "UI" } }
        div { class: "card",
            h3 { "Calendar" }
            div { class: "form-grid",
                Field { label: "First day of the week",
                    select { class: "select", "aria-label": "First day of the week",
                        onchange: move |e: FormEvent| { let v = e.value().parse().unwrap_or(1); edit(&|g| g.first_day_of_week = v) },
                        for (v, label) in [(0u8, "Sunday"), (1, "Monday"), (6, "Saturday")] {
                            option { key: "{v}", value: "{v}", selected: g.first_day_of_week == v, "{label}" }
                        }
                    }
                }
                Field { label: "Week column header", help: "The column headers of the calendar's week view.".to_string(),
                    select { class: "select", "aria-label": "Week column header",
                        onchange: move |e: FormEvent| { let v = e.value(); edit(&|g| g.calendar_week_column_header = v.clone()) },
                        for f in WEEK_COLUMN_HEADERS.iter() {
                            option { key: "{f}", value: "{f}", selected: g.calendar_week_column_header == *f, "{sample(f)}" }
                        }
                    }
                }
            }
        }
        div { class: "card",
            h3 { "Dates" }
            div { class: "form-grid",
                Field { label: "Short date format",
                    select { class: "select", "aria-label": "Short date format",
                        onchange: move |e: FormEvent| { let v = e.value(); edit(&|g| g.short_date_format = v.clone()) },
                        for f in SHORT_DATE_FORMATS.iter() {
                            option { key: "{f}", value: "{f}", selected: g.short_date_format == *f, "{sample(f)}" }
                        }
                    }
                }
                Field { label: "Long date format",
                    select { class: "select", "aria-label": "Long date format",
                        onchange: move |e: FormEvent| { let v = e.value(); edit(&|g| g.long_date_format = v.clone()) },
                        for f in LONG_DATE_FORMATS.iter() {
                            option { key: "{f}", value: "{f}", selected: g.long_date_format == *f, "{sample(f)}" }
                        }
                    }
                }
                Field { label: "Time format",
                    select { class: "select", "aria-label": "Time format",
                        onchange: move |e: FormEvent| { let v = e.value(); edit(&|g| g.time_format = v.clone()) },
                        for (f, label) in TIME_FORMATS.iter().zip(["5pm/5:30pm", "17:00/17:30"]) {
                            option { key: "{f}", value: "{f}", selected: g.time_format == *f, "{label}" }
                        }
                    }
                }
                Field { label: "Time zone", help: "Dates and times are shown in this time zone; empty follows the browser.".to_string(),
                    select { class: "select", "aria-label": "Time zone",
                        onchange: move |e: FormEvent| { let v = e.value(); edit(&|g| g.time_zone = v.clone()) },
                        option { value: "", selected: g.time_zone.is_empty(), "The browser's time zone" }
                        if !g.time_zone.is_empty() && !zones.contains(&g.time_zone) {
                            option { value: "{g.time_zone}", selected: true, "{g.time_zone}" }
                        }
                        for z in zones.iter() {
                            option { key: "{z}", value: "{z}", selected: g.time_zone == *z, "{z}" }
                        }
                    }
                }
            }
            Switch { checked: g.show_relative_dates, label: "Show relative dates (Today, Yesterday, Tomorrow) instead of the date", onchange: move |v| edit(&|g| g.show_relative_dates = v) }
        }
        div { class: "card",
            h3 { "Style" }
            div { class: "form-grid",
                Field { label: "Theme", help: "Kept in this browser; the top bar button switches it too.".to_string(),
                    select { class: "select", "aria-label": "Theme",
                        onchange: move |e: FormEvent| { if e.value() != *app.theme.peek() { app.toggle_theme(); } },
                        option { value: "dark", selected: theme == "dark", "Dark" }
                        option { value: "light", selected: theme == "light", "Light" }
                    }
                }
            }
            Switch { checked: g.enable_color_impaired_mode, label: "Colour-impaired mode (colours easier to tell apart, with patterns)", onchange: move |v| edit(&|g| g.enable_color_impaired_mode = v) }
        }
        div { class: "row end", style: "margin-top: var(--sp-4)", button { class: "btn primary", onclick: save, {crate::i18n::t().save} } }
    }
}

// ---------------------------------------------------------------------------------------------
// Settings › General › Host and Logging
// ---------------------------------------------------------------------------------------------

/// Settings › General's host values (`config.toml`), applied when Kitsunarr restarts.
#[component]
pub fn HostCard() -> Element {
    let app = use_app();
    let client = use_client();
    let mut form = use_signal(|| None::<HostSettings>);
    let mut confirm_restart = use_signal(|| false);
    let c0 = client.clone();
    use_future(move || {
        let c = c0.clone();
        async move {
            match c.host_settings().await {
                Ok(h) => form.set(Some(h)),
                Err(e) => app.error(&e),
            }
        }
    });
    let c1 = client.clone();
    let save = move |_| {
        let Some(h) = form() else { return };
        let c = c1.clone();
        spawn(async move {
            match c.put_host_settings(&h).await {
                Ok(saved) => {
                    form.set(Some(saved));
                    app.success("Saved to the configuration file. Restart Kitsunarr to apply.");
                }
                Err(e) => app.error(&e),
            }
        });
    };
    let restart = move |_| {
        confirm_restart.set(false);
        let c = client.clone();
        spawn(async move {
            match c.restart().await {
                Ok(()) => app.success("Kitsunarr is restarting."),
                Err(e) => app.error(&e),
            }
        });
    };
    let mut edit = move |f: &dyn Fn(&mut HostSettings)| {
        if let Some(h) = form.write().as_mut() {
            f(h);
        }
    };
    let Some(h) = form() else {
        return rsx! { div { class: "card", h3 { "Host and logging" } Loading {} } };
    };
    let locked = |k: &str| h.overridden.iter().any(|o| o == k);
    let note = |k: &str, running: String| -> Option<String> {
        locked(k).then(|| format!("Set by an environment variable or command-line flag: {running}"))
    };
    rsx! {
        div { class: "card", id: "host",
            h3 { "Host and logging" }
            p { class: "muted small", "Saved to {h.config_file}. Changes apply the next time Kitsunarr starts; environment variables and command-line flags still take precedence." }
            if h.restart_required {
                div { class: "alert warning", role: "status",
                    div { class: "grow", "Restart required: the saved values differ from the ones in use." }
                    button { class: "btn sm", onclick: move |_| confirm_restart.set(true), "Restart now" }
                }
            }
            div { class: "form-grid",
                Field { label: "Bind address", help: note("bindAddress", h.running.bind_address.clone()).unwrap_or_else(|| "0.0.0.0 listens on every interface.".into()),
                    input { class: "input mono", "aria-label": "Bind address", disabled: locked("bindAddress"), value: "{h.bind_address}",
                        oninput: move |e: FormEvent| { let v = e.value(); edit(&|x| x.bind_address = v.clone()) } }
                }
                Field { label: "Port", help: note("port", h.running.port.to_string()),
                    input { class: "input", r#type: "number", min: "1", max: "65535", "aria-label": "Port", disabled: locked("port"), value: "{h.port}",
                        oninput: move |e: FormEvent| { let v = e.value().parse().unwrap_or(0); edit(&|x| x.port = v) } }
                }
                Field { label: "URL base", help: note("urlBase", h.running.url_base.clone()).unwrap_or_else(|| "For reverse proxies, e.g. /kitsunarr; empty serves from the root.".into()),
                    input { class: "input mono", "aria-label": "URL base", disabled: locked("urlBase"), value: "{h.url_base}",
                        oninput: move |e: FormEvent| { let v = e.value(); edit(&|x| x.url_base = v.clone()) } }
                }
                Field { label: "Log level", help: note("logLevel", h.running.log_level.clone()).unwrap_or_else(|| "Debug and trace write much more to the log files.".into()),
                    select { class: "select", "aria-label": "Log level", disabled: locked("logLevel"),
                        onchange: move |e: FormEvent| { let v = e.value(); edit(&|x| x.log_level = v.clone()) },
                        if !kitsunarr_config_levels().contains(&h.log_level.as_str()) {
                            option { value: "{h.log_level}", selected: true, "{h.log_level}" }
                        }
                        for (v, label) in [("trace", "Trace"), ("debug", "Debug"), ("info", "Info"), ("warn", "Warn"), ("error", "Error")] {
                            option { key: "{v}", value: "{v}", selected: h.log_level == v, "{label}" }
                        }
                    }
                }
                Field { label: "Backup folder", help: note("backupFolder", h.running.backup_folder.clone()).unwrap_or_else(|| format!("Absolute path; empty uses {}.", h.running.backup_folder)),
                    input { class: "input mono", "aria-label": "Backup folder", disabled: locked("backupFolder"), placeholder: "{h.running.backup_folder}", value: "{h.backup_folder}",
                        oninput: move |e: FormEvent| { let v = e.value(); edit(&|x| x.backup_folder = v.clone()) } }
                }
            }
            p { class: "help small faint", "Kitsunarr serves plain HTTP; put a reverse proxy in front of it for HTTPS." }
            div { class: "row end", button { class: "btn primary", onclick: save, "Save host settings" } }
        }
        if confirm_restart() {
            crate::components::common::Confirm {
                title: "Restart Kitsunarr".to_string(),
                message: "Restart Kitsunarr now? If the address, port or URL base changed, open the new address afterwards.".to_string(),
                confirm_label: "Restart".to_string(),
                on_confirm: restart,
                on_cancel: move |_| confirm_restart.set(false),
            }
        }
    }
}

/// The log levels the select offers.
fn kitsunarr_config_levels() -> [&'static str; 5] {
    ["trace", "debug", "info", "warn", "error"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_insert_at_the_caret_and_follow_style() {
        assert_eq!(
            insert_at("{Series Title} - ", (17, 17), "{season:00}"),
            ("{Series Title} - {season:00}".to_string(), 28)
        );
        assert_eq!(insert_at("ab", (1, 2), "{x}"), ("a{x}".to_string(), 4));
        assert_eq!(styled("{Series Title}", ".", "title"), "{Series.Title}");
        assert_eq!(styled("{Series Title}", "_", "lower"), "{series_title}");
        let groups = token_groups("animeGroupSeasonFolderFormat");
        let all: Vec<&str> = groups
            .iter()
            .flat_map(|(_, t, _)| t.iter().map(|(k, _)| *k))
            .collect();
        assert!(all.contains(&"{Group Season:00}") && !all.contains(&"{Group Movie:00}"));
        assert!(!all.contains(&"{Quality Full}"));
        let daily = token_groups("dailyEpisodeFormat");
        assert_eq!(daily[0].0, "Presets");
        assert!(daily[0].1.iter().any(|(t, _)| t.contains("{Air-Date}")));
        assert_eq!(sample("YYYY-MM-DD"), "2014-03-25");
    }
}
