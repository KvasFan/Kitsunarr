//! Episode, season and series folder naming: a port of Sonarr's `FileNameBuilder` (revision
//! `c018578e4`). Patterns use Sonarr's tokens and run on the parser's .NET-semantics regex
//! engine.
//!
//! Differences from upstream:
//! - an empty release group renders as `Kitsunarr` (upstream: `Sonarr`);
//! - media info tokens render empty: Kitsunarr does not read media info yet (upstream does the
//!   same for files without media info).

use std::collections::HashMap;
use std::sync::LazyLock;

use kitsunarr_domain::SeriesType;
use kitsunarr_domain::api::settings::NamingSettings;
use kitsunarr_domain::quality::QualityModel;
use kitsunarr_parser::netregex::{NetMatch, NetRegex};
use kitsunarr_parser::remove_diacritics;

/// `LongPathSupport` on Linux.
const MAX_FILE_NAME_LENGTH: usize = 255;
const MAX_FILE_PATH_LENGTH: usize = 4096;

fn re(pattern: &str, ignore_case: bool) -> NetRegex {
    NetRegex::compile(pattern, ignore_case)
}

static TITLE: LazyLock<NetRegex> = LazyLock::new(|| {
    re(
        r"(?<escaped>\{\{|\}\})|\{(?<prefix>[- ._\[(]*)(?<token>(?:[a-z0-9]+)(?:(?<separator>[- ._]+)(?:[a-z0-9]+))*)(?::(?<customFormat>[ ,a-z0-9+-]+(?<![- ])))?(?<suffix>[- ._)\]]*)\}",
        true,
    )
});
static EPISODE: LazyLock<NetRegex> = LazyLock::new(|| re(r"(?<episode>\{episode(?:\:0+)?})", true));
static SEASON: LazyLock<NetRegex> = LazyLock::new(|| re(r"(?<season>\{season(?:\:0+)?})", true));
static ABSOLUTE: LazyLock<NetRegex> =
    LazyLock::new(|| re(r"(?<absolute>\{absolute(?:\:0+)?})", true));
static SEASON_EPISODE_PATTERN: LazyLock<NetRegex> = LazyLock::new(|| {
    re(
        r"(?<separator>(?<=})[- ._]+?)?(?<seasonEpisode>s?{season(?:\:0+)?}(?<episodeSeparator>[- ._]?[ex])(?<episode>{episode(?:\:0+)?}))(?<separator>[- ._]+?(?={))?",
        true,
    )
});
static ABSOLUTE_EPISODE_PATTERN: LazyLock<NetRegex> = LazyLock::new(|| {
    re(
        r"(?<separator>(?<=})[- ._]+?)?(?<absolute>{absolute(?:\:0+)?})(?<separator>[- ._]+?(?={))?",
        true,
    )
});
static AIR_DATE: LazyLock<NetRegex> = LazyLock::new(|| re(r"\{Air(\s|\W|_)Date\}", true));
static FILE_NAME_CLEANUP: LazyLock<NetRegex> = LazyLock::new(|| re(r"([- ._])(\1)+", false));
static TRIM_SEPARATORS: LazyLock<NetRegex> = LazyLock::new(|| re(r"[- ._]+$", false));
static SCENIFY_REMOVE: LazyLock<NetRegex> = LazyLock::new(|| {
    re(
        r#"(?<=\s)(,|<|>|\/|\\|;|:|'|"|\||`|’|~|!|\?|@|$|%|^|\*|-|_|=){1}(?=\s)|('|`|’|:|\?|,)(?=(?:(?:s|m|t|ve|ll|d|re)\s)|\s|$)|(\(|\)|\[|\]|\{|\})"#,
        true,
    )
});
static SCENIFY_REPLACE: LazyLock<NetRegex> = LazyLock::new(|| re(r"[\/]", true));
static MULTI_PART_CLEANUP: LazyLock<NetRegex> =
    LazyLock::new(|| re(r"(?:\:?\s?(?:\(\d+\)|(Part|Pt\.?)\s?\d+))$", true));
static TITLE_PREFIX: LazyLock<NetRegex> = LazyLock::new(|| {
    re(
        r"^(?<article>The|An|A) (?<rest>.*?)(?<tail>(?: *\([^)]+\))*)$",
        true,
    )
});
static YEAR: LazyLock<NetRegex> = LazyLock::new(|| re(r"\(\d{4}\)$", true));
static RESERVED_DEVICE_NAMES: LazyLock<NetRegex> =
    LazyLock::new(|| re(r"^(?:aux|com[1-9]|con|lpt[1-9]|nul|prn)\.", true));
static SIMPLE_TOKEN: LazyLock<NetRegex> = LazyLock::new(|| re(r"\s|_|\W", true));

const EPISODE_TITLE_TRIM: [char; 3] = [' ', '.', '?'];

/// What the names are built from.
#[derive(Debug, Clone)]
pub struct Series {
    pub title: String,
    pub year: i32,
    pub series_type: SeriesType,
    pub imdb_id: Option<String>,
    pub tvdb_id: i64,
    pub tvmaze_id: i64,
    pub tmdb_id: i64,
    /// Season titles by season number.
    pub season_titles: Vec<(i32, String)>,
    /// Tanukarr's anime group the series belongs to (`{Group Title}`, `{Group Season}`…).
    pub group: Option<GroupMembership>,
}

/// A series' place in its franchise or manual group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupMembership {
    pub title: String,
    /// The ordinal of a season member.
    pub season: Option<i32>,
    /// The ordinal of a movie member.
    pub movie: Option<i32>,
}

/// `FileNameBuilder.GetMemberArcTitle`: the subtitle after the group title (`Group: Arc`), or
/// the member's whole title.
pub fn arc_title(member: &str, group: &str) -> String {
    if member.to_lowercase().starts_with(&group.to_lowercase())
        && let Some(rest) = member.get(group.len()..)
    {
        let rest = rest.trim_start();
        if let Some(sub) = rest
            .strip_prefix(':')
            .or_else(|| rest.strip_prefix('-'))
            .or_else(|| rest.strip_prefix('–'))
            .or_else(|| rest.strip_prefix('—'))
        {
            let sub = sub.trim();
            if !sub.is_empty() {
                return sub.to_string();
            }
        }
    }
    member.to_string()
}

impl Default for Series {
    fn default() -> Self {
        Self {
            title: String::new(),
            year: 0,
            series_type: SeriesType::Standard,
            imdb_id: None,
            tvdb_id: 0,
            tvmaze_id: 0,
            tmdb_id: 0,
            season_titles: Vec::new(),
            group: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Episode {
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub title: String,
    /// `yyyy-MM-dd`.
    pub air_date: Option<String>,
}

#[derive(Debug, Clone)]
pub struct File {
    /// 0 for a file not in the library yet.
    pub id: i64,
    pub scene_name: Option<String>,
    pub relative_path: Option<String>,
    /// The full path (used when there is no relative path).
    pub path: Option<String>,
    pub release_group: Option<String>,
    pub release_hash: Option<String>,
    pub quality: QualityModel,
    /// The quality definition's title.
    pub quality_title: String,
    /// Matched custom formats: (name, include when renaming).
    pub custom_formats: Vec<(String, bool)>,
    /// Media information, for the `{MediaInfo …}` tokens.
    pub media_info: Option<crate::media_info::MediaInfo>,
}

impl Default for File {
    fn default() -> Self {
        Self {
            id: 0,
            scene_name: None,
            relative_path: None,
            path: None,
            release_group: None,
            release_hash: None,
            quality: QualityModel {
                quality_id: 0,
                revision: Default::default(),
            },
            quality_title: String::new(),
            custom_formats: Vec::new(),
            media_info: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct NamingError(pub String);

/// `FileNameBuilderTokenEqualityComparer`: tokens compare without spaces, punctuation or case.
fn simplify(token: &str) -> String {
    SIMPLE_TOKEN.replace(token, "").to_lowercase()
}

struct TokenMatch {
    prefix: String,
    separator: String,
    suffix: String,
    token: String,
    custom_format: Option<String>,
}

impl TokenMatch {
    fn default_value(&self, value: &str) -> String {
        if self.prefix.is_empty() && self.suffix.is_empty() {
            value.to_string()
        } else {
            String::new()
        }
    }
}

type Handler<'a> = Box<dyn Fn(&TokenMatch) -> Option<String> + 'a>;

#[derive(Default)]
struct Handlers<'a>(HashMap<String, Handler<'a>>);

impl<'a> Handlers<'a> {
    fn set(&mut self, token: &str, f: impl Fn(&TokenMatch) -> Option<String> + 'a) {
        self.0.insert(simplify(token), Box::new(f));
    }
    fn text(&mut self, token: &str, value: String) {
        self.set(token, move |_| Some(value.clone()));
    }
}

fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

/// UTF-8 byte length (`GetByteCount`).
fn bytes(s: &str) -> usize {
    s.len()
}

/// `string.Truncate(maxLength)`: at most `max` UTF-8 bytes, on a character boundary.
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if i + c.len_utf8() > max {
            break;
        }
        end = i + c.len_utf8();
    }
    s[..end].to_string()
}

fn max_length(formatter: Option<&str>) -> i64 {
    formatter
        .and_then(|f| f.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// `Truncate(input, formatter)`: a positive length keeps the start, a negative one the end.
fn truncate(input: &str, formatter: Option<&str>) -> String {
    if is_blank(input) {
        return String::new();
    }
    let max = max_length(formatter);
    if max == 0 || input.chars().count() as i64 <= max.abs() {
        return input.to_string();
    }
    if max < 0 {
        let reversed: String = input.chars().rev().collect();
        let cut = truncate_bytes(&reversed, (max.unsigned_abs() as usize).saturating_sub(3));
        let cut = cut.trim_end_matches([' ', '.']);
        let restored: String = cut.chars().rev().collect();
        return format!("{{ellipsis}}{restored}");
    }
    let cut = truncate_bytes(input, (max as usize).saturating_sub(3));
    format!("{}{{ellipsis}}", cut.trim_end_matches([' ', '.']))
}

/// `int.ToString(format)` for the formats patterns use: zero padding (`0`, `00`, `000`…).
fn format_number(value: i32, format: Option<&str>) -> String {
    match format {
        Some(f) if !f.is_empty() && f.chars().all(|c| c == '0') => {
            let width = f.len();
            if value < 0 {
                format!("-{:0width$}", -value)
            } else {
                format!("{value:0width$}")
            }
        }
        _ => value.to_string(),
    }
}

/// `CleanTitle`.
pub fn clean_title(title: &str) -> String {
    let title = title.replace('&', "and");
    let title = SCENIFY_REPLACE.replace(&title, " ");
    let title = SCENIFY_REMOVE.replace(&title, "");
    remove_diacritics(&title)
}

/// `TitleThe`: "The Office" → "Office, The".
pub fn title_the(title: &str) -> String {
    TITLE_PREFIX.replace_with(title, |m| {
        format!(
            "{}, {}{}",
            m.value("rest"),
            m.value("article"),
            m.value("tail")
        )
    })
}

/// `CleanTitleThe`.
pub fn clean_title_the(title: &str) -> String {
    match TITLE_PREFIX.find(title) {
        Some(m) => format!(
            "{}, {}{}",
            clean_title(&m.value("rest")).trim(),
            m.value("article"),
            clean_title(&m.value("tail"))
        ),
        None => clean_title(title),
    }
}

/// `TitleYear`.
pub fn title_year(title: &str, year: i32) -> String {
    if year == 0 || YEAR.is_match(title) {
        title.to_string()
    } else {
        format!("{title} ({year})")
    }
}

/// `CleanTitleTheYear`.
pub fn clean_title_the_year(title: &str, year: i32) -> String {
    if year == 0 {
        return clean_title_the(title);
    }
    if let Some(m) = YEAR.find(title) {
        let (start, _) = m.span();
        let before: String = title.chars().take(start).collect();
        let value = m.value_all();
        let digits: String = value.chars().skip(1).take(4).collect();
        return format!("{} {digits}", clean_title_the(before.trim()));
    }
    format!("{} {year}", clean_title_the(title))
}

/// `TitleWithoutYear`.
pub fn title_without_year(title: &str) -> String {
    YEAR.replace(title, "")
}

/// `TitleFirstCharacter`.
pub fn title_first_character(title: &str) -> String {
    let chars: Vec<char> = title.chars().collect();
    for c in chars.iter().take(2) {
        if c.is_alphanumeric() {
            let upper: String = c.to_uppercase().collect();
            return remove_diacritics(&upper)
                .chars()
                .next()
                .map(String::from)
                .unwrap_or_default();
        }
    }
    "_".into()
}

/// `CleanFileName(name, namingConfig)`.
pub fn clean_file_name_with(name: &str, naming: &NamingSettings) -> String {
    let mut result = if naming.replace_illegal_characters {
        match naming.colon_replacement_format {
            4 => name.replace(": ", " - ").replace(':', "-"),
            1 => name.replace(':', "-"),
            2 => name.replace(':', " -"),
            3 => name.replace(':', " - "),
            5 => name.replace(':', &naming.custom_colon_replacement_format),
            _ => name.replace(':', ""),
        }
    } else {
        name.replace(':', "")
    };
    for (bad, good) in [
        ("\\", "+"),
        ("/", "+"),
        ("<", ""),
        (">", ""),
        ("?", "!"),
        ("*", "-"),
        ("|", ""),
        ("\"", ""),
    ] {
        result = result.replace(
            bad,
            if naming.replace_illegal_characters {
                good
            } else {
                ""
            },
        );
    }
    result
        .trim_start_matches([' ', '.'])
        .trim_end_matches(' ')
        .to_string()
}

/// `CleanFileName(name)` with the default naming settings.
pub fn clean_file_name(name: &str) -> String {
    clean_file_name_with(name, &NamingSettings::default())
}

/// `CleanFolderName`.
pub fn clean_folder_name(name: &str) -> String {
    collapse_separators(name)
        .trim_matches([' ', '.'])
        .to_string()
}

fn collapse_separators(s: &str) -> String {
    FILE_NAME_CLEANUP.replace_with(s, |m| m.value_all().chars().take(1).collect())
}

fn reserved_device_names(s: &str) -> String {
    RESERVED_DEVICE_NAMES.replace_with(s, |m| m.value_all().replace('.', "_"))
}

fn replace_token(
    m: &NetMatch<'_>,
    handlers: &Handlers<'_>,
    naming: &NamingSettings,
    escape: bool,
) -> String {
    if m.success("escaped") {
        let v = m.value_all();
        if escape {
            return v;
        }
        return if v == "{{" { "{".into() } else { "}".into() };
    }
    let tm = TokenMatch {
        prefix: m.value("prefix"),
        separator: m.value("separator"),
        suffix: m.value("suffix"),
        token: m.value("token"),
        custom_format: Some(m.value("customFormat")).filter(|c| !is_blank(c)),
    };
    let replacement = match handlers.0.get(&simplify(&tm.token)) {
        Some(h) => h(&tm),
        None => Some(String::new()),
    };
    let Some(mut text) = replacement else {
        return m.value_all();
    };
    text = text.trim().to_string();
    let letters: Vec<char> = tm.token.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.iter().all(|c| c.is_lowercase()) {
        text = text.to_lowercase();
    } else if letters.iter().all(|c| c.is_uppercase()) {
        text = text.to_uppercase();
    }
    if !is_blank(&tm.separator) {
        text = text.replace(' ', &tm.separator);
    }
    text = clean_file_name_with(&text, naming);
    if !is_blank(&text) {
        text = format!("{}{}{}", tm.prefix, text, tm.suffix);
    }
    if escape {
        text = text.replace('{', "{{").replace('}', "}}");
    }
    text
}

fn replace_tokens(
    pattern: &str,
    handlers: &Handlers<'_>,
    naming: &NamingSettings,
    escape: bool,
) -> String {
    TITLE.replace_with(pattern, |m| replace_token(m, handlers, naming, escape))
}

fn add_series_tokens<'a>(h: &mut Handlers<'a>, series: &'a Series, title: &'a str) {
    let year = series.year;
    h.set("{Series Title}", move |m| {
        Some(truncate(title, m.custom_format.as_deref()))
    });
    h.set("{Series CleanTitle}", move |m| {
        Some(truncate(&clean_title(title), m.custom_format.as_deref()))
    });
    h.set("{Series TitleYear}", move |m| {
        Some(truncate(
            &title_year(title, year),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series CleanTitleYear}", move |m| {
        Some(truncate(
            &clean_title(&title_year(title, year)),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series TitleWithoutYear}", move |m| {
        Some(truncate(
            &title_without_year(title),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series CleanTitleWithoutYear}", move |m| {
        Some(truncate(
            &clean_title(&title_without_year(title)),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series TitleThe}", move |m| {
        Some(truncate(&title_the(title), m.custom_format.as_deref()))
    });
    h.set("{Series CleanTitleThe}", move |m| {
        Some(truncate(
            &clean_title_the(title),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series TitleTheYear}", move |m| {
        Some(truncate(
            &title_year(&title_the(title), year),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series CleanTitleTheYear}", move |m| {
        Some(truncate(
            &clean_title_the_year(title, year),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series TitleTheWithoutYear}", move |m| {
        Some(truncate(
            &title_without_year(&title_the(title)),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series CleanTitleTheWithoutYear}", move |m| {
        Some(truncate(
            &clean_title_the(&title_without_year(title)),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Series TitleFirstCharacter}", move |m| {
        Some(truncate(
            &title_first_character(&title_the(title)),
            m.custom_format.as_deref(),
        ))
    });
    h.text("{Series Year}", year.to_string());
}

/// Tanukarr's group tokens; empty for an ungrouped series or another kind of member.
fn add_group_tokens<'a>(h: &mut Handlers<'a>, series: &'a Series) {
    let group = series.group.as_ref();
    h.set("{Group Title}", move |m| {
        Some(truncate(
            group.map_or("", |g| g.title.as_str()),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Group Arc Title}", move |m| {
        Some(truncate(
            &group.map_or_else(String::new, |g| arc_title(&series.title, &g.title)),
            m.custom_format.as_deref(),
        ))
    });
    h.set("{Group Season}", move |m| {
        Some(group.and_then(|g| g.season).map_or_else(String::new, |n| {
            format_number(n, m.custom_format.as_deref())
        }))
    });
    h.set("{Group Movie}", move |m| {
        Some(group.and_then(|g| g.movie).map_or_else(String::new, |n| {
            format_number(n, m.custom_format.as_deref())
        }))
    });
}

fn add_id_tokens<'a>(h: &mut Handlers<'a>, series: &'a Series) {
    h.text("{ImdbId}", series.imdb_id.clone().unwrap_or_default());
    h.text("{TvdbId}", series.tvdb_id.to_string());
    h.text(
        "{TvMazeId}",
        if series.tvmaze_id > 0 {
            series.tvmaze_id.to_string()
        } else {
            String::new()
        },
    );
    h.text(
        "{TmdbId}",
        if series.tmdb_id > 0 {
            series.tmdb_id.to_string()
        } else {
            String::new()
        },
    );
}

fn add_season_number_token(h: &mut Handlers<'_>, season: i32) {
    h.set("{Season}", move |m| {
        Some(format_number(season, m.custom_format.as_deref()))
    });
}

fn add_season_title_token<'a>(h: &mut Handlers<'a>, title: Option<&'a str>) {
    h.set("{Season Title}", move |m| {
        Some(
            title
                .filter(|t| !is_blank(t))
                .map_or(String::new(), |t| truncate(t, m.custom_format.as_deref())),
        )
    });
    h.set("{Season CleanTitle}", move |m| {
        Some(title.filter(|t| !is_blank(t)).map_or(String::new(), |t| {
            truncate(&clean_title(t), m.custom_format.as_deref())
        }))
    });
}

/// `ReplaceNumberToken`.
fn replace_number_token(token: &str, value: i32) -> String {
    let inner = token.trim_matches(['{', '}']);
    match inner.split_once(':') {
        None => value.to_string(),
        Some((_, f)) => format_number(value, Some(f)),
    }
}

fn replace_season_tokens(pattern: &str, season: i32) -> String {
    SEASON.replace_with(pattern, |m| {
        replace_number_token(&m.value("season"), season)
    })
}

fn format_number_tokens(base: &str, format: &str, episodes: &[&Episode]) -> String {
    let mut pattern = String::new();
    for (i, e) in episodes.iter().enumerate() {
        let p = if i == 0 { base } else { format };
        pattern.push_str(&EPISODE.replace_with(p, |m| {
            replace_number_token(&m.value("episode"), e.episode_number)
        }));
    }
    replace_season_tokens(&pattern, episodes[0].season_number)
}

fn format_absolute_number_tokens(base: &str, format: &str, episodes: &[&Episode]) -> String {
    let mut pattern = String::new();
    for (i, e) in episodes.iter().enumerate() {
        let p = if i == 0 { base } else { format };
        pattern.push_str(&ABSOLUTE.replace_with(p, |m| {
            replace_number_token(&m.value("absolute"), e.absolute_episode_number.unwrap_or(0))
        }));
    }
    replace_season_tokens(&pattern, episodes[0].season_number)
}

fn first_and_last<'e>(episodes: &[&'e Episode]) -> Vec<&'e Episode> {
    let mut eps = vec![episodes[0]];
    if episodes.len() > 1 {
        eps.push(episodes[episodes.len() - 1]);
    }
    eps
}

struct EpisodeFormat {
    separator: String,
    episode_separator: String,
    episode_pattern: String,
    season_episode_pattern: String,
}

fn add_season_episode_numbering_tokens<'a>(
    mut pattern: String,
    h: &mut Handlers<'a>,
    episodes: &[&'a Episode],
    naming: &NamingSettings,
) -> String {
    let mut formats: Vec<EpisodeFormat> = Vec::new();
    for m in SEASON_EPISODE_PATTERN.find_all(&pattern) {
        let f = EpisodeFormat {
            separator: m.value("separator"),
            episode_separator: m.value("episodeSeparator"),
            episode_pattern: m.value("episode"),
            season_episode_pattern: m.value("seasonEpisode"),
        };
        if !formats
            .iter()
            .any(|x| x.season_episode_pattern == f.season_episode_pattern)
        {
            formats.push(f);
        }
    }
    for (index, f) in formats.iter().enumerate() {
        let base = f.season_episode_pattern.as_str();
        let value = match naming.multi_episode_style {
            1 => format_number_tokens(base, &format!("{}{}", f.separator, base), episodes),
            2 => format_number_tokens(
                base,
                &format!("{}{}", f.episode_separator, f.episode_pattern),
                episodes,
            ),
            3 => format_number_tokens(
                base,
                &format!("-{}{}", f.episode_separator, f.episode_pattern),
                episodes,
            ),
            4 => format_number_tokens(
                base,
                &format!("-{}", f.episode_pattern),
                &first_and_last(episodes),
            ),
            5 => format_number_tokens(
                base,
                &format!("-{}{}", f.episode_separator, f.episode_pattern),
                &first_and_last(episodes),
            ),
            _ => format_number_tokens(base, &format!("-{}", f.episode_pattern), episodes),
        };
        let token = format!("{{Season Episode{}}}", index + 1);
        pattern = pattern.replace(base, &token);
        h.text(&token, value);
    }
    add_season_number_token(h, episodes[0].season_number);
    let (first, last) = (
        episodes[0].episode_number,
        episodes[episodes.len() - 1].episode_number,
    );
    let many = episodes.len() > 1;
    h.set("{Episode}", move |m| {
        let f = m.custom_format.as_deref();
        Some(if many {
            format!("{}-{}", format_number(first, f), format_number(last, f))
        } else {
            format_number(first, f)
        })
    });
    pattern
}

fn add_absolute_numbering_tokens<'a>(
    mut pattern: String,
    h: &mut Handlers<'a>,
    series: &Series,
    episodes: &[&'a Episode],
    naming: &NamingSettings,
) -> String {
    let mut formats: Vec<(String, String)> = Vec::new();
    for m in ABSOLUTE_EPISODE_PATTERN.find_all(&pattern) {
        let separator = Some(m.value("separator"))
            .filter(|s| !is_blank(s))
            .unwrap_or_else(|| "-".into());
        let absolute = m.value("absolute");
        if !formats.iter().any(|(_, a)| *a == absolute) {
            formats.push((separator, absolute));
        }
    }
    let mut index = 1;
    for (separator, base) in formats {
        if series.series_type != SeriesType::Anime
            || episodes.iter().any(|e| e.absolute_episode_number.is_none())
        {
            pattern = pattern.replace(&base, "");
            continue;
        }
        let value = match naming.multi_episode_style {
            1 => format_absolute_number_tokens(&base, &format!("{separator}{base}"), episodes),
            2 => {
                let s = if is_blank(separator.trim()) {
                    " ".to_string()
                } else {
                    separator.trim().to_string()
                };
                format_absolute_number_tokens(&base, &format!("{s}{base}"), episodes)
            }
            4 | 5 => {
                format_absolute_number_tokens(&base, &format!("-{base}"), &first_and_last(episodes))
            }
            _ => format_absolute_number_tokens(&base, &format!("-{base}"), episodes),
        };
        let token = format!("{{Absolute Pattern{index}}}");
        index += 1;
        pattern = pattern.replace(&base, &token);
        h.text(&token, value);
    }
    pattern
}

fn add_episode_tokens(h: &mut Handlers<'_>, episodes: &[&Episode]) {
    let date = episodes[0]
        .air_date
        .clone()
        .filter(|d| !is_blank(d))
        .map(|d| d.replace('-', " "))
        .unwrap_or_else(|| "Unknown".into());
    h.text("{Air Date}", date);
}

fn add_episode_title_placeholders(h: &mut Handlers<'_>) {
    h.set("{Episode Title}", |_| None);
    h.set("{Episode CleanTitle}", |_| None);
}

/// `CleanupEpisodeTitle`: drops "(1)", "Part 2"… from multi-part titles.
fn cleanup_episode_title(title: &str) -> String {
    MULTI_PART_CLEANUP.replace(title, "").trim().to_string()
}

fn episode_titles(episodes: &[&Episode]) -> Vec<String> {
    let trimmed = |e: &&Episode| e.title.trim_end_matches(EPISODE_TITLE_TRIM).to_string();
    if episodes.len() == 1 {
        return vec![trimmed(&episodes[0])];
    }
    let mut titles: Vec<String> = Vec::new();
    for t in episodes
        .iter()
        .map(trimmed)
        .map(|t| cleanup_episode_title(&t))
    {
        if !titles.contains(&t) {
            titles.push(t);
        }
    }
    if titles.iter().all(|t| is_blank(t)) {
        titles.clear();
        for t in episodes.iter().map(trimmed) {
            if !titles.contains(&t) {
                titles.push(t);
            }
        }
    }
    titles
}

/// `GetEpisodeTitle`: joined titles within `max_length` bytes, shortened with an ellipsis.
fn episode_title(
    titles: &[String],
    separator: &str,
    max_length: i64,
    formatter: Option<&str>,
) -> String {
    let mut max = max_length;
    let formatter_max = max_length_of(formatter);
    if formatter_max > 0 {
        max = max.min(formatter_max);
    }
    let separator = format!(" {} ", separator.trim());
    let joined = titles.join(&separator);
    if bytes(&joined) as i64 <= max {
        return joined;
    }
    let first = &titles[0];
    let first_len = bytes(first) as i64;
    if titles.len() >= 2 {
        let last = &titles[titles.len() - 1];
        if first_len + bytes(last) as i64 + 3 <= max {
            return format!("{}{{ellipsis}}{last}", first.trim_end_matches([' ', '.']));
        }
    }
    if titles.len() > 1 && first_len + 3 <= max {
        return format!("{}{{ellipsis}}", first.trim_end_matches([' ', '.']));
    }
    if titles.len() == 1 && first_len <= max {
        return first.clone();
    }
    let cut = truncate_bytes(first, (max - 3).max(0) as usize);
    format!("{}{{ellipsis}}", cut.trim_end_matches([' ', '.']))
}

fn max_length_of(formatter: Option<&str>) -> i64 {
    max_length(formatter)
}

fn add_episode_title_tokens<'a>(h: &mut Handlers<'a>, episodes: &[&'a Episode], max: i64) {
    let titles = episode_titles(episodes);
    let clean: Vec<String> = titles.iter().map(|t| clean_title(t)).collect();
    h.set("{Episode Title}", move |m| {
        Some(episode_title(&titles, "+", max, m.custom_format.as_deref()))
    });
    h.set("{Episode CleanTitle}", move |m| {
        Some(episode_title(
            &clean,
            "and",
            max,
            m.custom_format.as_deref(),
        ))
    });
}

fn original_file_name(file: &File, fallback: bool) -> String {
    if !fallback {
        return String::new();
    }
    let path = file
        .relative_path
        .as_deref()
        .filter(|p| !is_blank(p))
        .or(file.path.as_deref())
        .unwrap_or_default();
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `GetOriginalTitle`.
fn original_title(file: &File, fallback: bool) -> String {
    match file.scene_name.as_deref().filter(|s| !is_blank(s)) {
        Some(scene) => clean_file_name(scene),
        None => clean_file_name(&original_file_name(file, fallback)),
    }
}

fn add_episode_file_tokens<'a>(h: &mut Handlers<'a>, file: &'a File, fallback: bool) {
    h.text("{Original Title}", original_title(file, fallback));
    h.text("{Original Filename}", original_file_name(file, fallback));
    h.set("{Release Group}", move |m| {
        Some(
            match file.release_group.as_deref().filter(|g| !is_blank(g)) {
                Some(g) => truncate(g, m.custom_format.as_deref()),
                None => m.default_value("Kitsunarr"),
            },
        )
    });
    h.text(
        "{Release Hash}",
        file.release_hash.clone().unwrap_or_default(),
    );
}

fn add_quality_tokens(h: &mut Handlers<'_>, series: &Series, file: &File) {
    let proper = if file.quality.revision.version > 1 {
        if series.series_type == SeriesType::Anime {
            format!("v{}", file.quality.revision.version)
        } else {
            "Proper".into()
        }
    } else {
        String::new()
    };
    let real = if file.quality.revision.real > 0 {
        "REAL".to_string()
    } else {
        String::new()
    };
    h.text(
        "{Quality Full}",
        format!("{} {proper} {real}", file.quality_title),
    );
    h.text("{Quality Title}", file.quality_title.clone());
    h.text("{Quality Proper}", proper);
    h.text("{Quality Real}", real);
}

/// `GetLanguagesToken`: stream languages as upper-case two-letter codes (ISO 639-2 codes
/// mapped through Kitsunarr's ISO table; unknown codes kept), `und` dropped, filtered by the
/// token's format (`-DE` excludes, `EN+DE` keeps those, a trailing `+` marks the rest with
/// `--`), English alone omitted when asked, and bracketed.
fn languages_token(
    languages: &[String],
    filter: Option<&str>,
    skip_english_only: bool,
    quoted: bool,
) -> String {
    let mut tokens: Vec<String> = Vec::new();
    for l in languages.iter().map(|l| l.trim()) {
        if l.is_empty() || l == "und" {
            continue;
        }
        let code = kitsunarr_parser::find_iso(&l.to_lowercase())
            .map_or_else(|| l.to_string(), |iso| iso.two_letter.to_uppercase());
        if !tokens.contains(&code) {
            tokens.push(code);
        }
    }
    let filter = filter.filter(|f| !is_blank(f));
    let mut filtered: Vec<String> = match filter {
        Some(f) if f.starts_with('-') => {
            let excluded: Vec<&str> = f.split('-').collect();
            tokens
                .iter()
                .filter(|t| !excluded.contains(&t.as_str()))
                .cloned()
                .collect()
        }
        Some(f) => f
            .split('+')
            .filter(|w| tokens.iter().any(|t| t == w))
            .map(str::to_string)
            .fold(Vec::new(), |mut acc, w| {
                if !acc.contains(&w) {
                    acc.push(w);
                }
                acc
            }),
        None => tokens.clone(),
    };
    if filter.is_some_and(|f| f.ends_with('+')) && filtered.len() != tokens.len() {
        filtered.push("--".into());
    }
    if skip_english_only && filtered.len() == 1 && filtered[0] == "EN" {
        return String::new();
    }
    let joined = filtered.join("+");
    if quoted && !joined.is_empty() {
        format!("[{joined}]")
    } else {
        joined
    }
}

/// `AddMediaInfoTokens`: nothing without media information.
fn add_media_info_tokens<'a>(h: &mut Handlers<'a>, file: &'a File, fallback: bool) {
    use crate::media_info::format;
    let Some(info) = file.media_info.as_ref() else {
        return;
    };
    // `GetSceneOrFileName`.
    let scene_name = file
        .scene_name
        .clone()
        .filter(|s| !is_blank(s))
        .unwrap_or_else(|| original_file_name(file, fallback));
    let video = format::format_video_codec(info, &scene_name);
    let audio = format::format_audio_codec(info.primary_audio(), &scene_name);
    let channels = format::format_audio_channels(info.primary_audio());
    let audio_languages: Vec<String> = info
        .audio_streams
        .iter()
        .map(|a| a.language.clone().unwrap_or_default())
        .collect();
    let subtitles: Vec<String> = info
        .subtitle_streams
        .iter()
        .map(|s| s.language.clone().unwrap_or_default())
        .collect();
    let bit_depth = if info.video_bit_depth > 0 {
        info.video_bit_depth
    } else {
        8
    };
    h.text("{MediaInfo Video}", video.clone());
    h.text("{MediaInfo VideoCodec}", video.clone());
    h.text("{MediaInfo VideoBitDepth}", bit_depth.to_string());
    h.text("{MediaInfo Audio}", audio.clone());
    h.text("{MediaInfo AudioCodec}", audio.clone());
    h.text(
        "{MediaInfo AudioChannels}",
        if channels > 0.0 {
            format!("{channels:.1}")
        } else {
            String::new()
        },
    );
    let (al, al2) = (audio_languages.clone(), audio_languages.clone());
    h.set("{MediaInfo AudioLanguages}", move |m| {
        Some(languages_token(&al, m.custom_format.as_deref(), true, true))
    });
    h.set("{MediaInfo AudioLanguagesAll}", move |m| {
        Some(languages_token(
            &al2,
            m.custom_format.as_deref(),
            false,
            true,
        ))
    });
    let (sl, sl2) = (subtitles.clone(), subtitles.clone());
    h.set("{MediaInfo SubtitleLanguages}", move |m| {
        Some(languages_token(
            &sl,
            m.custom_format.as_deref(),
            false,
            true,
        ))
    });
    h.set("{MediaInfo SubtitleLanguagesAll}", move |m| {
        Some(languages_token(
            &sl2,
            m.custom_format.as_deref(),
            false,
            true,
        ))
    });
    h.text("{MediaInfo Simple}", format!("{video} {audio}"));
    let (v2, a2) = (video, audio);
    h.set("{MediaInfo Full}", move |m| {
        let f = m.custom_format.as_deref();
        Some(format!(
            "{v2} {a2}{} {}",
            languages_token(&audio_languages, f, true, true),
            languages_token(&subtitles, f, false, true)
        ))
    });
    h.text(
        "{MediaInfo VideoDynamicRange}",
        format::format_video_dynamic_range(info),
    );
    h.text(
        "{MediaInfo VideoDynamicRangeType}",
        format::format_video_dynamic_range_type(info),
    );
}

fn add_custom_formats<'a>(h: &mut Handlers<'a>, file: &'a File) {
    h.set("{Custom Formats}", move |m| {
        let included: Vec<&str> = file
            .custom_formats
            .iter()
            .filter(|(_, include)| *include)
            .map(|(n, _)| n.as_str())
            .collect();
        let filtered: Vec<&str> = match m.custom_format.as_deref().filter(|f| !is_blank(f)) {
            Some(f) if f.starts_with('-') => {
                let excluded: Vec<&str> = f[1..].split(',').collect();
                included
                    .into_iter()
                    .filter(|n| !excluded.contains(n))
                    .collect()
            }
            Some(f) => {
                let wanted: Vec<&str> = f.split(',').collect();
                included
                    .into_iter()
                    .filter(|n| wanted.contains(n))
                    .collect()
            }
            None => included,
        };
        Some(filtered.join(" "))
    });
    h.set("{Custom Format}", move |m| {
        let Some(name) = m.custom_format.as_deref().filter(|f| !is_blank(f)) else {
            return Some(String::new());
        };
        Some(
            file.custom_formats
                .iter()
                .find(|(n, include)| *include && n == name)
                .map(|(n, _)| n.clone())
                .unwrap_or_default(),
        )
    });
}

/// `GetPatternHasEpisodeIdentifier`.
fn has_episode_identifier(pattern: &str) -> bool {
    SEASON_EPISODE_PATTERN.is_match(pattern)
        || ABSOLUTE_EPISODE_PATTERN.is_match(pattern)
        || AIR_DATE.is_match(pattern)
}

/// `GetLengthWithoutEpisodeTitle`.
fn length_without_episode_title(pattern: &str, naming: &NamingSettings) -> usize {
    let mut h = Handlers::default();
    h.text("{Episode Title}", String::new());
    h.text("{Episode CleanTitle}", String::new());
    h.text("{ellipsis}", "...".into());
    bytes(&replace_tokens(pattern, &h, naming, false))
}

/// `RequiresAbsoluteEpisodeNumber`.
pub fn requires_absolute_episode_number(naming: &NamingSettings) -> bool {
    ABSOLUTE.is_match(&naming.anime_episode_format)
}

/// `RequiresEpisodeTitle`.
pub fn requires_episode_title(
    naming: &NamingSettings,
    series: &Series,
    episodes: &[Episode],
) -> bool {
    if !naming.rename_episodes {
        return false;
    }
    let pattern = match series.series_type {
        SeriesType::Daily => &naming.daily_episode_format,
        SeriesType::Anime if episodes.iter().all(|e| e.absolute_episode_number.is_some()) => {
            &naming.anime_episode_format
        }
        _ => &naming.standard_episode_format,
    };
    TITLE.find_all(pattern).iter().any(|m| {
        let t = simplify(&m.value("token"));
        t == simplify("{Episode Title}") || t == simplify("{Episode CleanTitle}")
    })
}

/// `BuildFileName`: the file name (with `extension`, e.g. `.mkv`) for these episodes.
pub fn build_file_name(
    episodes: &[Episode],
    series: &Series,
    file: &File,
    extension: &str,
    naming: &NamingSettings,
) -> Result<String, NamingError> {
    build_file_name_within(
        episodes,
        series,
        file,
        extension,
        naming,
        MAX_FILE_PATH_LENGTH,
    )
}

fn build_file_name_within(
    episodes: &[Episode],
    series: &Series,
    file: &File,
    extension: &str,
    naming: &NamingSettings,
    max_path: usize,
) -> Result<String, NamingError> {
    if !naming.rename_episodes {
        return Ok(format!("{}{extension}", original_title(file, true)));
    }
    for (format, kind, name) in [
        (
            &naming.standard_episode_format,
            SeriesType::Standard,
            "Standard",
        ),
        (&naming.daily_episode_format, SeriesType::Daily, "Daily"),
        (&naming.anime_episode_format, SeriesType::Anime, "Anime"),
    ] {
        if is_blank(format) && series.series_type == kind {
            return Err(NamingError(format!(
                "{name} episode format cannot be empty"
            )));
        }
    }
    if episodes.is_empty() {
        return Err(NamingError("no episodes to name".into()));
    }
    let mut sorted: Vec<&Episode> = episodes.iter().collect();
    sorted.sort_by_key(|e| (e.season_number, e.episode_number));
    let mut pattern = naming.standard_episode_format.as_str();
    if series.series_type == SeriesType::Daily && sorted[0].season_number > 0 {
        pattern = &naming.daily_episode_format;
    }
    if series.series_type == SeriesType::Anime
        && (sorted.iter().all(|e| e.absolute_episode_number.is_some())
            || !requires_absolute_episode_number(naming))
    {
        pattern = &naming.anime_episode_format;
    }
    let parts: Vec<&str> = pattern
        .split(['\\', '/'])
        .filter(|p| !p.is_empty())
        .collect();
    let mut components = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let mut h = Handlers::default();
        let has_identifier = has_episode_identifier(part);
        let mut split =
            add_season_episode_numbering_tokens(part.to_string(), &mut h, &sorted, naming);
        split = add_absolute_numbering_tokens(split, &mut h, series, &sorted, naming);
        split = split.replace("...", "{{ellipsis}}");
        add_series_tokens(&mut h, series, &series.title);
        add_group_tokens(&mut h, series);
        add_id_tokens(&mut h, series);
        add_episode_tokens(&mut h, &sorted);
        add_episode_title_placeholders(&mut h);
        add_episode_file_tokens(&mut h, file, !has_identifier || file.id == 0);
        add_quality_tokens(&mut h, series, file);
        add_custom_formats(&mut h, file);
        add_media_info_tokens(&mut h, file, !has_identifier || file.id == 0);
        let mut component = replace_tokens(&split, &h, naming, true).trim().to_string();
        let mut max_segment = MAX_FILE_NAME_LENGTH.min(max_path) as i64;
        if i == parts.len() - 1 {
            max_segment -= bytes(extension) as i64;
        }
        let max_title = max_segment - length_without_episode_title(&component, naming) as i64;
        add_episode_title_tokens(&mut h, &sorted, max_title);
        component = replace_tokens(&component, &h, naming, false)
            .trim()
            .to_string();
        component = collapse_separators(&component);
        component = TRIM_SEPARATORS.replace(&component, "");
        component = component.replace("{ellipsis}", "...");
        component = reserved_device_names(&component);
        components.push(component);
    }
    Ok(format!("{}{extension}", components.join("/")))
}

/// `GetSeriesFolder`.
pub fn series_folder(series: &Series, naming: &NamingSettings) -> String {
    let mut h = Handlers::default();
    add_series_tokens(&mut h, series, &series.title);
    add_group_tokens(&mut h, series);
    add_id_tokens(&mut h, series);
    let name = replace_tokens(&naming.series_folder_format, &h, naming, false);
    let name = reserved_device_names(&clean_folder_name(&name));
    name.replace("{ellipsis}", "...")
}

/// Tanukarr's `GetAnimeGroupFolder` / `GetAnimeGroupMemberFolder`: a folder of the group's
/// formats (`{Group Title}`, `Season {Group Season:00}`, `{Group Arc Title}`, `Film {Group
/// Movie:00}`, `{Series Title}`) for a grouped series.
pub fn group_folder(format: &str, series: &Series, naming: &NamingSettings) -> String {
    let mut h = Handlers::default();
    add_series_tokens(&mut h, series, &series.title);
    add_group_tokens(&mut h, series);
    add_id_tokens(&mut h, series);
    let name = replace_tokens(format, &h, naming, false);
    let name = clean_folder_name(&name)
        .trim_matches([' ', '.', '-', '_'])
        .to_string();
    reserved_device_names(&name).replace("{ellipsis}", "...")
}

/// Tanukarr's strict story-arc rule: the member's title is the group's title, a separator,
/// and a subtitle ending in "arc", "hen" or "chapter".
pub fn is_story_arc(member: &str, group: &str) -> bool {
    let arc = arc_title(member, group);
    arc != member && {
        let last = arc
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .to_lowercase();
        matches!(last.as_str(), "arc" | "hen" | "chapter")
    }
}

/// `GetSeasonFolder`.
pub fn season_folder(series: &Series, season: i32, naming: &NamingSettings) -> String {
    let title = series
        .season_titles
        .iter()
        .find(|(n, _)| *n == season)
        .map(|(_, t)| t.as_str());
    let mut h = Handlers::default();
    add_series_tokens(&mut h, series, &series.title);
    add_group_tokens(&mut h, series);
    add_id_tokens(&mut h, series);
    add_season_number_token(&mut h, season);
    add_season_title_token(&mut h, title);
    let format = if season == 0 {
        &naming.specials_folder_format
    } else {
        &naming.season_folder_format
    };
    let name = replace_tokens(format, &h, naming, false);
    let name = clean_folder_name(&name)
        .trim_matches([' ', '.', '-', '_'])
        .to_string();
    reserved_device_names(&name).replace("{ellipsis}", "...")
}

/// `BuildSeasonPath` relative to the series folder: the cleaned season folder, or nothing.
pub fn season_path(
    series: &Series,
    season: i32,
    season_folder_on: bool,
    naming: &NamingSettings,
) -> Option<String> {
    season_folder_on.then(|| clean_file_name(&season_folder(series, season, naming)))
}

/// `BuildFilePath` relative to the series folder.
pub fn build_relative_path(
    episodes: &[Episode],
    series: &Series,
    file: &File,
    extension: &str,
    season_folder_on: bool,
    series_path: &str,
    naming: &NamingSettings,
) -> Result<String, NamingError> {
    // `episodes.First().SeasonNumber`, in the order given.
    let season = episodes.first().map(|e| e.season_number).unwrap_or(0);
    let folder = season_path(series, season, season_folder_on, naming);
    let season_dir = match &folder {
        Some(f) => format!("{}/{f}", series_path.trim_end_matches('/')),
        None => series_path.trim_end_matches('/').to_string(),
    };
    let remaining = MAX_FILE_PATH_LENGTH.saturating_sub(bytes(&season_dir) + 1);
    let name = build_file_name_within(episodes, series, file, extension, naming, remaining)?;
    Ok(match folder {
        Some(f) => format!("{f}/{name}"),
        None => name,
    })
}

impl Series {
    /// A series as naming sees it, from its title, year, type and identifiers.
    pub fn from_parts(
        title: &str,
        year: Option<i32>,
        series_type: SeriesType,
        external_ids: &[kitsunarr_domain::ExternalId],
    ) -> Self {
        use kitsunarr_domain::ExternalIdKind as K;
        let id = |kind: K| {
            external_ids
                .iter()
                .find(|e| e.kind == kind)
                .map(|e| e.value.trim().to_string())
        };
        let int = |kind: K| id(kind).and_then(|v| v.parse().ok()).unwrap_or(0);
        Self {
            title: title.to_string(),
            year: year.unwrap_or(0),
            series_type,
            imdb_id: id(K::Imdb).filter(|v| !v.is_empty()),
            tvdb_id: int(K::Tvdb),
            tvmaze_id: int(K::TvMaze),
            tmdb_id: int(K::Tmdb),
            season_titles: Vec::new(),
            group: None,
        }
    }
}
