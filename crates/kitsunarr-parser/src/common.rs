//! Shared building blocks (`Parser/ParserCommon.cs`, `Parser/RegexReplace.cs`).

use std::sync::LazyLock;

use crate::re::{Captures, Re};

/// A pattern plus a .NET-style replacement template (`RegexReplace`).
///
/// Template syntax as in .NET: `$N` (group number), `${name}`, `$$`. Patterns that reuse a group
/// name in .NET are ported with numbered aliases (`title`, `title__2`, ...); `${title}` then
/// resolves to the alias that participated last in the input, which is what .NET's "last
/// capture" rule yields. `$N` is only allowed in patterns without named groups, where .NET and
/// Rust number groups identically.
pub(crate) struct RegexReplace {
    re: Re,
    template: Vec<Piece>,
    names: Vec<String>,
}

enum Piece {
    Literal(String),
    Number(usize),
    Named(String),
}

impl RegexReplace {
    pub fn new(pattern: &str, template: &str, case_insensitive: bool) -> Self {
        let re = if case_insensitive {
            Re::ci(pattern)
        } else {
            Re::new(pattern)
        };
        let names: Vec<String> = re.group_names();
        let template = parse_template(template);
        assert!(
            names.is_empty() || !template.iter().any(|p| matches!(p, Piece::Number(_))),
            "numbered references with named groups would be numbered differently from .NET: {pattern}"
        );
        Self {
            re,
            template,
            names,
        }
    }

    /// `Regex.Replace`: replaces every match.
    pub fn replace(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut last = 0;
        for c in self.re.captures_all(input) {
            let Some(m) = c.get(0) else { continue };
            out.push_str(&input[last..m.start()]);
            self.expand(&c, &mut out);
            last = m.end();
        }
        out.push_str(&input[last..]);
        out
    }

    /// `TryReplace`: whether the pattern matched; replaces in place either way.
    pub fn try_replace(&self, input: &mut String) -> bool {
        let matched = self.re.is_match(input);
        *input = self.replace(input);
        matched
    }

    fn expand(&self, c: &Captures<'_>, out: &mut String) {
        for piece in &self.template {
            match piece {
                Piece::Literal(s) => out.push_str(s),
                Piece::Number(n) => {
                    if let Some(m) = c.get(*n) {
                        out.push_str(m.as_str());
                    }
                }
                Piece::Named(name) => {
                    let alias = format!("{name}__");
                    let mut known = false;
                    let best = self
                        .names
                        .iter()
                        .filter(|n| *n == name || n.starts_with(&alias))
                        .inspect(|_| known = true)
                        .filter_map(|n| c.name(n))
                        .max_by_key(|m| m.start());
                    match best {
                        Some(m) => out.push_str(m.as_str()),
                        // .NET leaves a reference to an unknown group as literal text.
                        None if !known => {
                            out.push_str("${");
                            out.push_str(name);
                            out.push('}');
                        }
                        None => {}
                    }
                }
            }
        }
    }
}

fn parse_template(template: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut chars = template.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch != '$' {
            literal.push(ch);
            continue;
        }
        let rest = &template[i + 1..];
        if rest.starts_with('$') {
            literal.push('$');
            chars.next();
        } else if let Some(close) = rest.strip_prefix('{').and_then(|r| r.find('}')) {
            let name = &rest[1..=close];
            pieces.push(Piece::Literal(std::mem::take(&mut literal)));
            pieces.push(Piece::Named(name.to_string()));
            for _ in 0..close + 2 {
                chars.next();
            }
        } else if rest.starts_with(|c: char| c.is_ascii_digit()) {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            pieces.push(Piece::Literal(std::mem::take(&mut literal)));
            pieces.push(Piece::Number(digits.parse().unwrap_or(0)));
            for _ in 0..digits.len() {
                chars.next();
            }
        } else {
            literal.push('$');
        }
    }
    pieces.push(Piece::Literal(literal));
    pieces
}

const CJK: &str = r"\x{4E00}-\x{9FCC}";

/// `PreSubstitutionRegex`: normalises Korean and Chinese release layouts before parsing.
pub(crate) static PRE_SUBSTITUTION: LazyLock<Vec<RegexReplace>> = LazyLock::new(|| {
    let p = |s: &str| s.replace("CJK", CJK);
    vec![
        // Korean series without season number, replace with S01Exxx and remove airdate
        RegexReplace::new(
            r"\.E(\d{2,4})\.\d{6}\.(.*-(F1RST|NEXT))$",
            ".S01E$1.$2",
            false,
        ),
        // Some Chinese anime releases contain both English and Chinese titles, remove the Chinese title and replace with normal anime pattern
        RegexReplace::new(
            &p(
                r"^\[(?:(?<subgroup>[^\]]+?)(?:[CJK]+)?)\]\[(?<title>[^\]]+?)(?:\s(?<chinesetitle>[CJK][^\]]*?))\]\[(?:(?:[CJK]+?)?(?<episode>\d{1,4})(?:[CJK]+?)?)\]",
            ),
            "[${subgroup}] ${title} - ${episode} - ",
            false,
        ),
        // Chinese LoliHouse/ZERO/Lilith-Raws/Skymoon-Raws/orion origin releases don't use the expected brackets, normalize using brackets
        RegexReplace::new(
            r"^\[(?<subgroup>[^\]]*?(?:LoliHouse|ZERO|Lilith-Raws|Skymoon-Raws|orion origin)[^\]]*?)\](?<title>[^\[\]]+?)(?: - (?<episode>[0-9-]+)\s*|\[第?(?<episode__2>[0-9]+(?:-[0-9]+)?)话?(?:END|完)?\])\[",
            "[${subgroup}][${title}][${episode}][",
            false,
        ),
        // Most Chinese anime releases contain additional brackets/separators for chinese and non-chinese titles, remove junk first and if it has S0x as season number, convert it to Sx
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:\s?★[^\[ -]+\s?)?\[?(?:(?<chinesetitle>(?=[^\]]*?[CJK])[^\]]*?)(?:\]\[|\s*[_/·]\s*)){0,2}(?<title>[^\[\]]+?)(?:\s(?:S?(?<!\d+)((0)(?<season>\d)|(?<season__2>[1-9]\d))(?!\d+)))\]?(?:\[\d{4}\])?\[第?(?<episode>[0-9]+(?:-[0-9]+)?)(?:话|集)?(?: ?END|完| ?Fin)?\]",
            ),
            "[${subgroup}] ${title} S${season} - ${episode} ",
            false,
        ),
        // Some Chinese releases don't include a separation between Chinese and English titles within the same bracketed group
        RegexReplace::new(
            r"^\[(?<subgroup>[^\]]+)\]\[(?<chinesetitle>(?<![^a-zA-Z0-9])[^a-zA-Z0-9]+)(?<title>[^\]]+?)\](?:\[\d{4}\])?\[第?(?<episode>[0-9]+(?:-[0-9]+)?)(?:话|集)?(?: ?END|完| ?Fin)?\]",
            "[${subgroup}] ${title} - ${episode} ",
            false,
        ),
        // Chinese season packs, that may not actually have the full season so treated as multi-episode
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:\s?★[^\[ -]+\s?)?\[?(?:(?<chinesetitle>(?=[^\]]*?[CJK])[^\]]*?)(?:\]\[|\s*[_/·]\s*)){0,2}(?<title>[^\]]+?)\]?(?:\[\d{4}\])?\[(?:第|全)?(?<episode>[0-9]{1,4}-[0-9]{1,4})(?:话|集)?\](?:\[(?<language>.+?)\+.+?\])[_. ](?<title__2>.+?)S(?<season>[0-9]{1,2})(?<rest>.+?)$",
            ),
            "${title}S${season}E${episode}.${language}${rest}",
            false,
        ),
        // Chinese season packs, that actually have the full season so treated as multi-episode
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:\s?★[^\[ -]+\s?)?\[?(?:(?<chinesetitle>(?=[^\]]*?[CJK])[^\]]*?)(?:\]\[|\s*[_/·]\s*)){0,2}(?<title>[^\]]+?)\]?(?:\[\d{4}\])?\[(?:全)?(?<episode>[0-9]{1,4})(?:话|集)?\](?:\[(?<language>.+?)\+.+?\])[_. ](?<title__2>.+?)S(?<season>[0-9]{1,2})(?<rest>.+?)$",
            ),
            "${title}S${season}.${language}${rest}",
            false,
        ),
        // Most Chinese anime releases contain additional brackets/separators for chinese and non-chinese titles, remove junk and replace with normal anime pattern
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:\s?★[^\[ -]+\s?)?\[?(?:(?<chinesetitle>(?=[^\]]*?[CJK])[^\]]*?)(?:\]\[|\s*[_/·]\s*)){0,2}(?<title>[^\]]+?)\]?(?:\[\d{4}\])?\[第?(?<episode>[0-9]{1,4}(?:-[0-9]{1,4})?)(?:话|集)?(?: ?END|完| ?Fin)?\]",
            ),
            "[${subgroup}] ${title} - ${episode} ",
            false,
        ),
        // Some Chinese anime releases contain both Chinese and English titles, remove the Chinese title first and if it has S0x as season number, convert it to Sx
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:\s)(?:(?<chinesetitle>(?=[^\]]*?[CJK])[^\]]*?)(?:\s/\s))(?<title>[^\[\]]+?)(?:\s(?:S?(?<!\d+)((0)(?<season>\d)|(?<season__2>[1-9]\d))(?!\d+)))(?:[- ]+)(?<episode>[0-9]+(?:-[0-9]+)?)话?(?:END|完)?",
            ),
            "[${subgroup}] ${title} S${season} - ${episode} ",
            false,
        ),
        // Some Chinese anime releases contain both English and Chinese titles, remove the Chinese title and replace with normal anime pattern
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:\s)(?:(?<title>[^\]]+?)(?:\s/\s))(?<chinesetitle>(?=[^\]]*?[CJK])[^\]]*?)(?:[- ]+)(?<episode>[0-9]+(?:-[0-9]+)?(?![a-z]))话?(?:END|完)?",
            ),
            "[${subgroup}] ${title} - ${episode} ",
            false,
        ),
        // Some Chinese anime releases contain both Chinese and English titles, remove the Chinese title and replace with normal anime pattern
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:\s)(?:(?<chinesetitle>(?=[^\]]*?[CJK])[^\]]*?)(?:\s/\s))(?<title>[^\]]+?)(?:[- ]+)(?<episode>[0-9]+(?:-[0-9]+)?(?![a-z]))话?(?:END|完)?",
            ),
            "[${subgroup}] ${title} - ${episode} ",
            false,
        ),
        // GM-Team releases with lots of square brackets
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:(?<chinesubgroup>\[(?=[^\]]*?[CJK])[^\]]*\])+)\[(?<title>[^\]]+?)\](?<junk>\[[^\]]+\])*\[(?<episode>[0-9]+(?:-[0-9]+)?)( END| Fin)?\]",
            ),
            "[${subgroup}] ${title} - ${episode} ",
            false,
        ),
        // Some Chinese anime releases contain both Chinese and English titles separated by | instead of /, remove the Chinese title and replace with normal anime pattern
        RegexReplace::new(
            &p(
                r"^\[(?<subgroup>[^\]]+)\](?:\s)(?:(?<chinesetitle>(?=[^\]]*?[CJK])[^\]]*?)(?:\s\|\s))(?<title>[^\]]+?)(?:[- ]+)(?<episode>[0-9]+(?:-[0-9]+)?(?![a-z]))话?(?:END|完)?",
            ),
            "[${subgroup}] ${title} - ${episode} ",
            false,
        ),
        // Spanish releases with information in brackets
        RegexReplace::new(
            r"^(?<title>.+?(?=[ ._-]\()).+?\((?<year>\d{4})/(?<info>S[^/]+)",
            "${title} (${year}) - ${info} ",
            false,
        ),
    ]
});

/// `PathPreSubstitutionRegex`: season and episode number separated by a space (S02 01).
#[allow(dead_code)] // used by the title parser
pub(crate) static PATH_PRE_SUBSTITUTION: LazyLock<Vec<RegexReplace>> = LazyLock::new(|| {
    vec![RegexReplace::new(
        r"(?<separator>^|[-_. ])S(?<season>(?<!\d+)\d{1,2}(?!\d+)) (?<episode>(?<!\d+)\d{1,2}(?!\d+))(?!\d+)",
        "${separator}S${season}E${episode}",
        true,
    )]
});

pub(crate) static WEBSITE_PREFIX: LazyLock<RegexReplace> = LazyLock::new(|| {
    RegexReplace::new(
        r"^(?:(?:\[|\()\s*)?(?:www\.)?[-a-z0-9-]{1,256}\.(?<!Naruto-Kun\.)(?:[a-z]{2,6}\.[a-z]{2,6}|xn--[a-z0-9-]{4,}|[a-z]{2,})\b(?:\s*(?:\]|\))|[ -]{2,})[ -]*",
        "",
        true,
    )
});

#[allow(dead_code)] // used by the title parser
pub(crate) static WEBSITE_POSTFIX: LazyLock<RegexReplace> = LazyLock::new(|| {
    RegexReplace::new(
        r"(?:\[\s*)?(?:www\.)?[-a-z0-9-]{1,256}\.(?:xn--[a-z0-9-]{4,}|[a-z]{2,6})\b(?:\s*\])$",
        "",
        true,
    )
});

pub(crate) static CLEAN_TORRENT_SUFFIX: LazyLock<RegexReplace> =
    LazyLock::new(|| RegexReplace::new(r"\[(?:ettv|rartv|rarbg|cttv|publichd)\]$", "", true));

/// Applies the pre-substitutions in order, stopping after the first one that matched.
pub(crate) fn pre_substitute(title: &mut String) {
    for replace in PRE_SUBSTITUTION.iter() {
        if replace.try_replace(title) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_follow_dotnet_rules() {
        let r = RegexReplace::new(r"(?<a>x)(?<a__2>y)?", "[${a}|${missing}|$$]", false);
        assert_eq!(r.replace("x xy"), "[x|${missing}|$] [y|${missing}|$]");
        let n = RegexReplace::new(r"(\d)-(\d)", "$2$1", false);
        assert_eq!(n.replace("1-2 3-4"), "21 43");
        let mut s = String::from("none");
        assert!(!n.try_replace(&mut s));
    }

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(PRE_SUBSTITUTION.len(), 14);
        let _ = (
            &*PATH_PRE_SUBSTITUTION,
            &*WEBSITE_PREFIX,
            &*WEBSITE_POSTFIX,
            &*CLEAN_TORRENT_SUFFIX,
        );
    }
}
