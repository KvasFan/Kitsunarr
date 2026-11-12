//! Language parsing (`Parser/LanguageParser.cs`, `Parser/IsoLanguages.cs`).

use std::sync::LazyLock;

use kitsunarr_domain::Language;
use serde::Serialize;

use crate::common::RegexReplace;
use crate::re::{Re, has};

fn lang(id: i32) -> &'static Language {
    Language::by_id(id).unwrap_or_else(|| panic!("unknown language id {id}"))
}

// Sonarr language ids.
const ORIGINAL: i32 = -2;
const UNKNOWN: i32 = 0;
const ENGLISH: i32 = 1;
const GERMAN: i32 = 4;

/// Languages in Sonarr's `Language.All` order: Unknown, ids 1… ascending, Original last.
fn all_in_sonarr_order() -> Vec<&'static Language> {
    let mut all: Vec<&'static Language> = Language::all().iter().collect();
    all.sort_by_key(|l| match l.id {
        UNKNOWN => i32::MIN,
        ORIGINAL => i32::MAX,
        id => id,
    });
    all
}

static CLEAN_SERIES_TITLE: LazyLock<Vec<RegexReplace>> = LazyLock::new(|| {
    vec![RegexReplace::new(
        r".*?[_. ](S\d{2}(?:E\d{2,4})*[_. ].*)",
        "$1",
        true,
    )]
});

const LANGUAGES_ONLY: &str = r"(?<english>\b(?:ing|eng)\b)|(?<italian>\b(?:ita|italian)\b)|(?<german>(?:swiss)?german\b|videomann|ger[. ]dub|\bger\b)|(?<flemish>flemish)|(?<greek>greek)|(?<french>(?:_|\b)(?:FR|VF|VF2|VFF|VFI|VFQ|TRUEFRENCH|FRENCH|FRE|FRA)(?:_|\b))|(?<russian>\b(?:rus|ru)\b)|(?<hungarian>\b(?:HUNDUB|HUN)\b)|(?<hebrew>\bHebDub\b)|(?<polish>\b(?:PL\W?DUB|DUB\W?PL|LEK\W?PL|PL\W?LEK)\b)|(?<chinese>\[(?:CH[ST]|BIG5|GB)\]|简|繁|字幕|国语音轨[.+])|(?<bulgarian>\bbgaudio\b)|(?<spanish>\b(?:español|castellano|esp|spa(?!\(Latino\)))\b)|(?<ukrainian>\b(?:\dx?)?(?:ukr))|(?<thai>\b(?:THAI)\b)|(?<romanian>\b(?:RoDubbed|ROMANIAN)\b)|(?<catalan>[-,. ]cat[. ](?:DD|subs)|\b(?:catalan|catalán)\b)|(?<latvian>\b(?:lat|lav|lv)\b)|(?<turkish>\b(?:tur)\b)|(?<urdu>\burdu\b)|(?<romansh>\b(?:romansh|rumantsch|romansch)\b)|(?<georgian>\b(?:geo|ka|kat|georgian)\b)|(?<irish>\b(?:irish|gaeilge)\b)|(?<japanese>\(JA\)|JAP|JPN)|(?<portuguese>[_. ]por[_. ])|(?<original>\b(?:orig|original)\b)";

/// `LanguageRegex` embeds the language alternation twice. The second copy sits in a negative
/// lookahead, whose captures .NET discards, so it is made non-capturing here.
static LANGUAGE: LazyLock<Re> = LazyLock::new(|| {
    let uncaptured = Re::new(r"\(\?<[a-z]+>").replace_all_literal(LANGUAGES_ONLY, "(?:");
    Re::ci(&format!(
        "(?:{LANGUAGES_ONLY})(?!(?:[-_. ](?:{uncaptured}))*[-_. ]subs?)"
    ))
});

/// Regex groups → language ids, in the order upstream checks them.
const REGEX_GROUPS: &[(&str, i32)] = &[
    ("english", 1),
    ("italian", 5),
    ("german", 4),
    ("flemish", 19),
    ("greek", 20),
    ("french", 2),
    ("russian", 11),
    // upstream also checks a "dutch" group that the pattern does not define
    ("hungarian", 22),
    ("hebrew", 23),
    ("polish", 12),
    ("chinese", 10),
    ("bulgarian", 28),
    ("ukrainian", 30),
    ("spanish", 3),
    ("thai", 32),
    ("romanian", 35),
    ("catalan", 38),
    ("latvian", 36),
    ("turkish", 17),
    ("urdu", 50),
    ("romansh", 51),
    ("georgian", 52),
    ("irish", 53),
    ("japanese", 8),
    ("portuguese", 18),
    ("original", ORIGINAL),
];

static CASE_SENSITIVE_LANGUAGE: LazyLock<Re> = LazyLock::new(|| {
    Re::new(
        r"(?:(?i)(?<!SUB[\W|_|^]))(?:(?<lithuanian>\bLT\b)|(?<czech>\bCZ\b)|(?<polish>\bPL\b)|(?<bulgarian>\bBG\b)|(?<slovak>\bSK\b)|(?<german>\bDE\b))(?:(?i)(?![\W|_|^]SUB))",
    )
});

static GERMAN_DUAL_LANGUAGE: LazyLock<Re> = LazyLock::new(|| Re::ci(r"(?<!WEB[-_. ]?)\bDL\b"));
static GERMAN_MULTI_LANGUAGE: LazyLock<Re> = LazyLock::new(|| Re::ci(r"\bML\b"));

const TAGS: &str = "forced|foreign|default|cc|psdh|sdh";

// The repeated `tags` groups are wrapped in span groups (`pre`, `post`) and re-scanned, which
// yields .NET's capture collection in the same order.
static SUBTITLE_LANGUAGE: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(&format!(
        ".+?(?<pre>(?:[-_. ](?:{TAGS}))*)[-_. ](?<iso_code>[a-z]{{2,3}})(?<post>(?:[-_. ](?:{TAGS}))*)$"
    ))
});
// The lookahead makes each unit end where the original's repetition unit ended.
static TAG: LazyLock<Re> = LazyLock::new(|| Re::ci(&format!("[-_. ](?<tag>{TAGS})(?=[-_. ]|$)")));

// Same treatment for `SubtitleLanguageTitleRegex`: the two repeated sections become `pre` and
// `post` spans of `.token` units, each a tag or an ISO code.
static SUBTITLE_LANGUAGE_TITLE: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(&format!(
        r".+?(?<pre>(?:\.(?:(?:{TAGS})|(?:[a-z]{{2,3}})))*)[-_. ](?<title>[^.]*)(?<post>(?:\.(?:(?:{TAGS})|(?:[a-z]{{2,3}})))*)$"
    ))
});
static EPISODE_TOKEN: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bS\d+E\d+\b").unwrap());
static TAG_OR_ISO: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(&format!(
        r"\.(?:(?<tag>{TAGS})|(?<iso>[a-z]{{2,3}}))(?=\.|$)"
    ))
});

static SUBTITLE_TITLE: LazyLock<Re> =
    LazyLock::new(|| Re::new(r"^((?<title>.+) - )?(?<copy>(?<!\d+)\d{1,3}(?!\d+))$"));

/// `LanguageParser.ParseLanguages`: languages named in a release title.
pub fn parse_languages(title: &str) -> Vec<&'static Language> {
    let mut title = title.to_string();
    for replace in CLEAN_SERIES_TITLE.iter() {
        if replace.try_replace(&mut title) {
            break;
        }
    }
    let lower = title.to_lowercase();
    let mut languages: Vec<&'static Language> = Vec::new();
    for (words, id) in [
        (&["spanish"][..], 3),
        (&["danish"], 6),
        (&["dutch"], 7),
        (&["japanese"], 8),
        (&["icelandic"], 9),
        (&["mandarin", "cantonese", "chinese"], 10),
        (&["korean"], 21),
        (&["russian"], 11),
        (&["polish"], 12),
        (&["vietnamese"], 13),
        (&["swedish"], 14),
        (&["norwegian"], 15),
        (&["finnish"], 16),
        (&["turkish"], 17),
        (&["portuguese"], 18),
        (&["hungarian"], 22),
        (&["hebrew"], 23),
        (&["arabic"], 26),
        (&["hindi"], 27),
        (&["malayalam"], 29),
        (&["ukrainian"], 30),
        (&["bulgarian"], 28),
        (&["slovak"], 31),
        (&["brazilian", "dublado"], 33),
        (&["latino"], 34),
        (&["latvian"], 36),
        (&["azerbaijani", "azerbaijan"], 47),
        (&["uzbek", "uzbekistan"], 48),
    ] {
        if words.iter().any(|w| lower.contains(w)) {
            languages.push(lang(id));
        }
    }
    languages.extend(regex_languages(&title));
    if lower.contains("english") {
        languages.push(lang(ENGLISH));
    }
    if languages.is_empty() {
        languages.push(lang(UNKNOWN));
    }
    if languages.len() == 1 && languages[0].id == GERMAN {
        if GERMAN_DUAL_LANGUAGE.is_match(&title) {
            languages.push(lang(ORIGINAL));
        } else if GERMAN_MULTI_LANGUAGE.is_match(&title) {
            languages.push(lang(ORIGINAL));
            languages.push(lang(ENGLISH));
        }
    }
    let mut seen = std::collections::HashSet::new();
    languages.retain(|l| seen.insert(l.id));
    languages
}

fn regex_languages(title: &str) -> Vec<&'static Language> {
    let mut languages = Vec::new();
    // Case sensitive; only the first match counts.
    if let Some(c) = CASE_SENSITIVE_LANGUAGE.captures(title) {
        for (group, id) in [
            ("lithuanian", 24),
            ("czech", 25),
            ("polish", 12),
            ("bulgarian", 28),
            ("slovak", 31),
            ("german", GERMAN),
        ] {
            if has(&c, group) {
                languages.push(lang(id));
            }
        }
    }
    for m in LANGUAGE.captures_all(title) {
        for (group, id) in REGEX_GROUPS {
            if has(&m, group) {
                languages.push(lang(*id));
            }
        }
    }
    languages
}

/// .NET `Path.GetFileNameWithoutExtension` on Unix.
fn file_name_without_extension(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) => &name[..i],
        None => name,
    }
}

/// `LanguageParser.ParseSubtitleLanguage`: the language of a subtitle file name.
pub fn parse_subtitle_language(file_name: &str) -> &'static Language {
    let simple = file_name_without_extension(file_name);
    if let Some(c) = SUBTITLE_LANGUAGE.captures(simple) {
        let iso = c.name("iso_code").map(|m| m.as_str()).unwrap_or_default();
        return find_iso(&iso.to_lowercase()).map_or_else(|| lang(UNKNOWN), |i| i.language());
    }
    let lower = simple.to_lowercase();
    all_in_sonarr_order()
        .into_iter()
        .find(|l| lower.ends_with(&l.name.to_lowercase()))
        .unwrap_or_else(|| lang(UNKNOWN))
}

/// `LanguageParser.ParseLanguageTags`: subtitle flags such as `forced` or `sdh`.
pub fn parse_language_tags(file_name: &str) -> Vec<String> {
    let simple = file_name_without_extension(file_name);
    let Some(c) = SUBTITLE_LANGUAGE.captures(simple) else {
        return Vec::new();
    };
    ["pre", "post"]
        .iter()
        .filter_map(|g| c.name(g))
        .flat_map(|span| TAG.captures_all(span.as_str()))
        .filter_map(|t| t.name("tag").map(|m| m.as_str().to_lowercase()))
        .collect()
}

/// What a subtitle file name says about its language, flags and title (`SubtitleTitleInfo`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTitleInfo {
    pub language: &'static Language,
    pub language_tags: Vec<String>,
    pub raw_title: Option<String>,
    pub title: Option<String>,
    pub copy: i32,
    pub title_first: bool,
}

/// `LanguageParser.ParseBasicSubtitle`.
pub fn parse_basic_subtitle(file_name: &str) -> SubtitleTitleInfo {
    SubtitleTitleInfo {
        language: parse_subtitle_language(file_name),
        language_tags: parse_language_tags(file_name),
        raw_title: None,
        title: None,
        copy: 0,
        title_first: false,
    }
}

/// `LanguageParser.ParseSubtitleLanguageInformation`: language, flags and an optional title.
pub fn parse_subtitle_language_information(file_name: &str) -> SubtitleTitleInfo {
    let simple = file_name_without_extension(file_name);
    let Some(c) = SUBTITLE_LANGUAGE_TITLE.captures(simple) else {
        return parse_basic_subtitle(file_name);
    };
    let Some(title) = c.name("title") else {
        return parse_basic_subtitle(file_name);
    };
    let (mut tags1, mut tags2, mut isos) = (Vec::new(), Vec::new(), Vec::new());
    for (span, tags) in [("pre", &mut tags1), ("post", &mut tags2)] {
        let Some(span) = c.name(span) else { continue };
        for unit in TAG_OR_ISO.captures_all(span.as_str()) {
            if let Some(t) = unit.name("tag") {
                tags.push(t.as_str().to_string());
            } else if let Some(iso) = unit.name("iso") {
                isos.push(iso.as_str().to_string());
            }
        }
    }
    if isos.len() != 1 {
        return parse_basic_subtitle(file_name);
    }
    // Tanukarr: an unknown code, or a "title" that is the episode number, is no title.
    let Some(iso) = find_iso(&isos[0].to_lowercase()) else {
        return parse_basic_subtitle(file_name);
    };
    if EPISODE_TOKEN.is_match(title.as_str()) {
        return parse_basic_subtitle(file_name);
    }
    let language = iso.language();
    // Upstream's `Union` compares `Capture` objects by reference, so equal values are kept.
    let language_tags: Vec<String> = tags1
        .iter()
        .chain(tags2.iter())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect();
    let raw_title = title.as_str().to_string();
    let (title, copy) = match SUBTITLE_TITLE.captures(&raw_title) {
        Some(m) => (
            m.name("title").map(|t| t.as_str().to_string()),
            m.name("copy")
                .and_then(|c| c.as_str().parse().ok())
                .unwrap_or(0),
        ),
        None => (Some(raw_title.clone()), 0),
    };
    SubtitleTitleInfo {
        language,
        language_tags,
        raw_title: Some(raw_title),
        title,
        copy,
        title_first: tags1.is_empty(),
    }
}

/// An ISO 639 entry (`IsoLanguage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoLanguage {
    pub two_letter: &'static str,
    pub country: &'static str,
    pub three_letter: &'static str,
    language_id: i32,
}

impl IsoLanguage {
    pub fn language(&self) -> &'static Language {
        lang(self.language_id)
    }
}

const fn iso(
    two: &'static str,
    country: &'static str,
    three: &'static str,
    id: i32,
) -> IsoLanguage {
    IsoLanguage {
        two_letter: two,
        country,
        three_letter: three,
        language_id: id,
    }
}

/// `IsoLanguages.All`, in upstream order (lookups return the first entry that fits).
const ISO_LANGUAGES: &[IsoLanguage] = &[
    iso("en", "", "eng", 1),
    iso("fr", "fr", "fra", 2),
    iso("es", "", "spa", 3),
    iso("de", "de", "deu", 4),
    iso("it", "", "ita", 5),
    iso("da", "", "dan", 6),
    iso("nl", "", "nld", 7),
    iso("ja", "", "jpn", 8),
    iso("is", "", "isl", 9),
    iso("zh", "cn", "zho", 10),
    iso("ru", "", "rus", 11),
    iso("pl", "", "pol", 12),
    iso("vi", "", "vie", 13),
    iso("sv", "", "swe", 14),
    iso("no", "", "nor", 15),
    iso("nb", "", "nob", 15),
    iso("fi", "", "fin", 16),
    iso("tr", "", "tur", 17),
    iso("pt", "pt", "por", 18),
    iso("nl", "", "nld", 19),
    iso("el", "", "ell", 20),
    iso("ko", "", "kor", 21),
    iso("hu", "", "hun", 22),
    iso("he", "", "heb", 23),
    iso("lt", "", "lit", 24),
    iso("cs", "", "ces", 25),
    iso("ar", "", "ara", 26),
    iso("hi", "", "hin", 27),
    iso("bg", "", "bul", 28),
    iso("ml", "", "mal", 29),
    iso("uk", "", "ukr", 30),
    iso("sk", "", "slk", 31),
    iso("th", "th", "tha", 32),
    iso("pt", "br", "por", 33),
    iso("es", "mx", "spa", 34),
    iso("ro", "", "ron", 35),
    iso("lv", "", "lav", 36),
    iso("fa", "", "fas", 37),
    iso("ca", "", "cat", 38),
    iso("hr", "", "hrv", 39),
    iso("sr", "", "srp", 40),
    iso("bs", "", "bos", 41),
    iso("et", "", "est", 42),
    iso("ta", "", "tam", 43),
    iso("id", "", "ind", 44),
    iso("mk", "", "mkd", 45),
    iso("sl", "", "slv", 46),
    iso("az", "", "aze", 47),
    iso("uz", "", "uzb", 48),
    iso("ms", "", "msa", 49),
    iso("ur", "", "urd", 50),
    iso("rm", "", "roh", 51),
    iso("ka", "", "kat", 52),
    iso("ga", "", "gle", 53),
];

/// `IsoLanguages.AlternateIsoCodeMappings` (looked up with the code as given, case-sensitive).
const ALTERNATE_ISO_CODES: &[(&str, i32)] = &[
    ("cze", 25),
    ("dut", 7),
    ("mac", 45),
    ("rum", 35),
    ("yue", 10),
    ("zhtw", 10),
];

/// `FileNameBuilder.Iso639BTMap`: ISO 639-2/B → 639-2/T.
const ISO_639_BT: &[(&str, &str)] = &[
    ("alb", "sqi"),
    ("arm", "hye"),
    ("baq", "eus"),
    ("bur", "mya"),
    ("chi", "zho"),
    ("cze", "ces"),
    ("dut", "nld"),
    ("fre", "fra"),
    ("geo", "kat"),
    ("ger", "deu"),
    ("gre", "ell"),
    ("gsw", "deu"),
    ("ice", "isl"),
    ("mac", "mkd"),
    ("mao", "mri"),
    ("may", "msa"),
    ("per", "fas"),
    ("rum", "ron"),
    ("slo", "slk"),
    ("tib", "bod"),
    ("wel", "cym"),
];

/// `IsoLanguages.Get`: the ISO entry of a language (its two-letter code for file names).
pub fn iso_for(language: &Language) -> Option<IsoLanguage> {
    ISO_LANGUAGES
        .iter()
        .find(|l| l.language_id == language.id)
        .copied()
}

/// `IsoLanguages.Find`: an ISO 639-1 (optionally with a country, `pt-br`) or 639-2 code.
pub fn find_iso(code: &str) -> Option<IsoLanguage> {
    let parts: Vec<&str> = code.split('-').collect();
    let lang_code = parts[0].to_lowercase();
    if lang_code.chars().count() == 2 {
        let mut candidates: Vec<&IsoLanguage> = ISO_LANGUAGES
            .iter()
            .filter(|l| l.two_letter == lang_code)
            .collect();
        if let Some(country) = parts.get(1).map(|c| c.to_lowercase()) {
            if candidates.iter().any(|l| l.country == country) {
                candidates.retain(|l| l.country == country);
            } else {
                candidates.retain(|l| l.country.is_empty());
            }
        }
        return candidates.first().map(|l| **l);
    }
    if let Some((_, id)) = ALTERNATE_ISO_CODES.iter().find(|(c, _)| *c == code) {
        return ISO_LANGUAGES.iter().find(|l| l.language_id == *id).copied();
    }
    if lang_code.chars().count() == 3 {
        let code = ISO_639_BT
            .iter()
            .find(|(b, _)| *b == lang_code)
            .map_or(lang_code.as_str(), |(_, t)| t);
        return ISO_LANGUAGES
            .iter()
            .find(|l| l.three_letter == code)
            .copied();
    }
    None
}
