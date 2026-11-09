//! A small backtracking regex engine with .NET semantics.
//!
//! Sonarr's episode parser (`Parser.cs`) depends on .NET regex behaviour that no Rust engine
//! offers: capture *collections* (every iteration of a repeated group is kept and read back as
//! a list), group names reused within one pattern (all occurrences feed one group), and
//! lookbehind of any length (matched right to left, as .NET does). Rather than rewriting ~75
//! patterns and risking subtle differences, this engine runs them verbatim.
//!
//! Supported syntax (what the ported patterns use): literals and escapes, `.`, character
//! classes with ranges and `\d \w \s \D \W \S`, `^ $ \b \B \A \z \Z`, groups `( )`, `(?: )`,
//! `(?<name> )`, `(?'name' )`, lookaround `(?= ) (?! ) (?<= ) (?<! )`, atomic `(?> )`, inline
//! options `(?i) (?-i) (?i: ) (?-i: )`, greedy and lazy quantifiers, alternation, and
//! backreferences `\k<name>` / `\N`.
//!
//! Semantics follow .NET: leftmost match, alternatives in order, greedy/lazy backtracking,
//! group numbering (unnamed groups first, then named ones), a failed backreference to a group
//! that has not captured, captures inside negative lookarounds discarded, lookarounds and
//! atomic groups not backtracked into. `\d`, `\w`, `\s` and `\b` use .NET's Unicode
//! definitions. Matching is bounded: a step budget stops pathological backtracking and very
//! long inputs are not matched (both count as "no match").

use std::collections::HashMap;
use std::sync::LazyLock;

/// Inputs longer than this (in characters) never match; it bounds recursion depth.
const MAX_INPUT_CHARS: usize = 2048;
/// Upper bound on matcher steps for one search, like a .NET match timeout.
const STEP_BUDGET: u64 = 20_000_000;

#[derive(Debug)]
pub struct NetRegex {
    root: Node,
    names: HashMap<String, usize>,
    groups: usize,
    anchored: bool,
    /// Starts with a greedy `.*`: when the input has no newline, any match from a later start
    /// can be absorbed by that `.*` from position 0, and .NET reports the leftmost match, so
    /// searching from position 0 only gives the same result.
    leading_dot_star: bool,
    pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Builtin {
    Digit,
    Word,
    Space,
}

#[derive(Debug)]
enum SetItem {
    Range(char, char),
    Class(Builtin, bool),
}

#[derive(Debug)]
struct CharSet {
    negated: bool,
    items: Vec<SetItem>,
}

#[derive(Debug)]
enum Node {
    Empty,
    Char {
        c: char,
        ci: bool,
    },
    Any,
    Set {
        set: CharSet,
        ci: bool,
    },
    Bol,
    Eol,
    StartOfText,
    EndOfText,
    EndOfTextOrNewline,
    WordBoundary {
        negated: bool,
    },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Capture {
        index: usize,
        node: Box<Node>,
    },
    Repeat {
        node: Box<Node>,
        min: u32,
        max: u32,
        greedy: bool,
    },
    Look {
        behind: bool,
        negated: bool,
        node: Box<Node>,
        captures: bool,
    },
    Atomic {
        node: Box<Node>,
        captures: bool,
    },
    BackRef {
        index: usize,
        ci: bool,
    },
}

// ---------------------------------------------------------------------------------------------
// Pattern parser

#[derive(Debug)]
enum GroupRef {
    Unnamed(usize),
    Named(String),
}

/// Parse tree before groups get their final .NET numbers.
#[derive(Debug)]
enum Raw {
    Node(Node),
    Concat(Vec<Raw>),
    Alt(Vec<Raw>),
    Capture(GroupRef, Box<Raw>),
    Repeat(Box<Raw>, u32, u32, bool),
    Look(bool, bool, Box<Raw>),
    Atomic(Box<Raw>),
    BackRefName(String, bool),
    BackRefNumber(usize, bool),
}

struct Parser<'p> {
    chars: Vec<char>,
    pos: usize,
    unnamed: usize,
    named_order: Vec<String>,
    pattern: &'p str,
}

impl Parser<'_> {
    fn err<T>(&self, msg: &str) -> Result<T, String> {
        Err(format!("{msg} at {} in {:?}", self.pos, self.pattern))
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        s.chars()
            .enumerate()
            .all(|(i, c)| self.peek_at(i) == Some(c))
    }

    fn parse_alt(&mut self, ci: &mut bool) -> Result<Raw, String> {
        let mut branches = vec![self.parse_concat(ci)?];
        while self.eat('|') {
            branches.push(self.parse_concat(ci)?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().unwrap_or(Raw::Node(Node::Empty))
        } else {
            Raw::Alt(branches)
        })
    }

    fn parse_concat(&mut self, ci: &mut bool) -> Result<Raw, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            if let Some(atom) = self.parse_atom(ci)? {
                let atom = self.parse_quantifier(atom)?;
                items.push(atom);
            }
        }
        Ok(if items.len() == 1 {
            items.pop().unwrap_or(Raw::Node(Node::Empty))
        } else {
            Raw::Concat(items)
        })
    }

    fn parse_quantifier(&mut self, atom: Raw) -> Result<Raw, String> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, u32::MAX)
            }
            Some('+') => {
                self.pos += 1;
                (1, u32::MAX)
            }
            Some('?') => {
                self.pos += 1;
                (0, 1)
            }
            Some('{') => match self.try_braces() {
                Some(range) => range,
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        let greedy = !self.eat('?');
        if matches!(self.peek(), Some('*' | '+' | '?')) {
            return self.err("nested quantifier");
        }
        Ok(Raw::Repeat(Box::new(atom), min, max, greedy))
    }

    /// `{n}`, `{n,}`, `{n,m}`; anything else leaves `{` as a literal (as .NET does).
    fn try_braces(&mut self) -> Option<(u32, u32)> {
        let start = self.pos;
        self.pos += 1;
        let read = |p: &mut Self| -> Option<u32> {
            let s = p.pos;
            while p.peek().is_some_and(|c| c.is_ascii_digit()) {
                p.pos += 1;
            }
            if p.pos == s {
                None
            } else {
                p.chars[s..p.pos].iter().collect::<String>().parse().ok()
            }
        };
        let result = (|| {
            let min = read(self)?;
            if self.eat('}') {
                return Some((min, min));
            }
            if !self.eat(',') {
                return None;
            }
            if self.eat('}') {
                return Some((min, u32::MAX));
            }
            let max = read(self)?;
            self.eat('}').then_some((min, max))
        })();
        if result.is_none() {
            self.pos = start;
        }
        result
    }

    fn parse_atom(&mut self, ci: &mut bool) -> Result<Option<Raw>, String> {
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        self.pos += 1;
        Ok(Some(match c {
            '(' => return self.parse_group(ci),
            '[' => Raw::Node(Node::Set {
                set: self.parse_set()?,
                ci: *ci,
            }),
            '.' => Raw::Node(Node::Any),
            '^' => Raw::Node(Node::Bol),
            '$' => Raw::Node(Node::Eol),
            '\\' => self.parse_escape(*ci)?,
            c => Raw::Node(Node::Char { c, ci: *ci }),
        }))
    }

    fn parse_group(&mut self, ci: &mut bool) -> Result<Option<Raw>, String> {
        if !self.eat('?') {
            self.unnamed += 1;
            let index = self.unnamed;
            let inner = self.group_body(*ci)?;
            return Ok(Some(Raw::Capture(
                GroupRef::Unnamed(index),
                Box::new(inner),
            )));
        }
        if self.eat(':') {
            return Ok(Some(self.group_body(*ci)?));
        }
        if self.eat('=') {
            return Ok(Some(Raw::Look(
                false,
                false,
                Box::new(self.group_body(*ci)?),
            )));
        }
        if self.eat('!') {
            return Ok(Some(Raw::Look(
                false,
                true,
                Box::new(self.group_body(*ci)?),
            )));
        }
        if self.eat('>') {
            return Ok(Some(Raw::Atomic(Box::new(self.group_body(*ci)?))));
        }
        if self.starts_with("<=") || self.starts_with("<!") {
            let negated = self.peek_at(1) == Some('!');
            self.pos += 2;
            return Ok(Some(Raw::Look(
                true,
                negated,
                Box::new(self.group_body(*ci)?),
            )));
        }
        if matches!(self.peek(), Some('<' | '\'')) {
            let close = if self.peek() == Some('<') { '>' } else { '\'' };
            self.pos += 1;
            let start = self.pos;
            while self.peek().is_some_and(|c| c != close) {
                self.pos += 1;
            }
            let name: String = self.chars[start..self.pos].iter().collect();
            if !self.eat(close) || name.is_empty() {
                return self.err("bad group name");
            }
            if !self.named_order.contains(&name) {
                self.named_order.push(name.clone());
            }
            let inner = self.group_body(*ci)?;
            return Ok(Some(Raw::Capture(GroupRef::Named(name), Box::new(inner))));
        }
        // Inline options: (?imnsx-imnsx) or (?imnsx-imnsx: ... )
        let mut on = true;
        let mut new_ci = *ci;
        loop {
            match self.peek() {
                Some('-') => on = false,
                Some('i') => new_ci = on,
                Some('m' | 'n' | 's' | 'x') => {
                    return self.err("unsupported inline option");
                }
                Some(')') => {
                    self.pos += 1;
                    *ci = new_ci;
                    return Ok(None);
                }
                Some(':') => {
                    self.pos += 1;
                    let mut scoped = new_ci;
                    let inner = self.parse_alt(&mut scoped)?;
                    if !self.eat(')') {
                        return self.err("missing )");
                    }
                    return Ok(Some(inner));
                }
                _ => return self.err("bad group"),
            }
            self.pos += 1;
        }
    }

    fn group_body(&mut self, ci: bool) -> Result<Raw, String> {
        let mut scoped = ci;
        let inner = self.parse_alt(&mut scoped)?;
        if !self.eat(')') {
            return self.err("missing )");
        }
        Ok(inner)
    }

    fn hex(&mut self, digits: usize) -> Result<char, String> {
        let s: String = self
            .chars
            .get(self.pos..self.pos + digits)
            .unwrap_or(&[])
            .iter()
            .collect();
        let v = u32::from_str_radix(&s, 16)
            .map_err(|_| format!("bad hex escape in {:?}", self.pattern))?;
        self.pos += digits;
        char::from_u32(v).ok_or_else(|| format!("bad code point in {:?}", self.pattern))
    }

    /// Escapes that denote a single character (shared by sets and atoms).
    fn char_escape(&mut self, c: char) -> Result<Option<char>, String> {
        Ok(Some(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'f' => '\x0C',
            'v' => '\x0B',
            'e' => '\x1B',
            'a' => '\x07',
            '0' => '\0',
            'u' => self.hex(4)?,
            'x' => self.hex(2)?,
            c if !c.is_alphanumeric() => c,
            _ => return Ok(None),
        }))
    }

    fn parse_escape(&mut self, ci: bool) -> Result<Raw, String> {
        let Some(c) = self.peek() else {
            return self.err("trailing backslash");
        };
        self.pos += 1;
        let class = |b, negated| {
            Raw::Node(Node::Set {
                set: CharSet {
                    negated: false,
                    items: vec![SetItem::Class(b, negated)],
                },
                ci: false,
            })
        };
        Ok(match c {
            'd' => class(Builtin::Digit, false),
            'D' => class(Builtin::Digit, true),
            'w' => class(Builtin::Word, false),
            'W' => class(Builtin::Word, true),
            's' => class(Builtin::Space, false),
            'S' => class(Builtin::Space, true),
            'b' => Raw::Node(Node::WordBoundary { negated: false }),
            'B' => Raw::Node(Node::WordBoundary { negated: true }),
            'A' => Raw::Node(Node::StartOfText),
            'z' => Raw::Node(Node::EndOfText),
            'Z' => Raw::Node(Node::EndOfTextOrNewline),
            'k' => {
                let close = match self.peek() {
                    Some('<') => '>',
                    Some('\'') => '\'',
                    _ => return self.err("bad \\k"),
                };
                self.pos += 1;
                let start = self.pos;
                while self.peek().is_some_and(|c| c != close) {
                    self.pos += 1;
                }
                let name: String = self.chars[start..self.pos].iter().collect();
                if !self.eat(close) {
                    return self.err("bad \\k");
                }
                match name.parse::<usize>() {
                    Ok(n) => Raw::BackRefNumber(n, ci),
                    Err(_) => Raw::BackRefName(name, ci),
                }
            }
            '1'..='9' => {
                let mut n = c.to_digit(10).unwrap_or(0) as usize;
                while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
                    n = n * 10 + d as usize;
                    self.pos += 1;
                }
                Raw::BackRefNumber(n, ci)
            }
            c => match self.char_escape(c)? {
                Some(ch) => Raw::Node(Node::Char { c: ch, ci }),
                None => return self.err("unsupported escape"),
            },
        })
    }

    fn parse_set(&mut self) -> Result<CharSet, String> {
        let negated = self.eat('^');
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return self.err("unterminated class");
            };
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            first = false;
            self.pos += 1;
            let lo = if c == '\\' {
                let Some(e) = self.peek() else {
                    return self.err("trailing backslash");
                };
                self.pos += 1;
                let builtin = match e {
                    'd' => Some((Builtin::Digit, false)),
                    'D' => Some((Builtin::Digit, true)),
                    'w' => Some((Builtin::Word, false)),
                    'W' => Some((Builtin::Word, true)),
                    's' => Some((Builtin::Space, false)),
                    'S' => Some((Builtin::Space, true)),
                    _ => None,
                };
                if let Some((b, n)) = builtin {
                    items.push(SetItem::Class(b, n));
                    continue;
                }
                if e == 'b' {
                    '\x08'
                } else {
                    match self.char_escape(e)? {
                        Some(ch) => ch,
                        None => return self.err("unsupported escape in class"),
                    }
                }
            } else {
                c
            };
            // Range `a-z` (a '-' before ']' is a literal).
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|n| n != ']') {
                self.pos += 1;
                let Some(h) = self.peek() else {
                    return self.err("unterminated class");
                };
                self.pos += 1;
                let hi = if h == '\\' {
                    let Some(e) = self.peek() else {
                        return self.err("trailing backslash");
                    };
                    self.pos += 1;
                    match self.char_escape(e)? {
                        Some(ch) => ch,
                        None => return self.err("unsupported range end"),
                    }
                } else {
                    h
                };
                items.push(SetItem::Range(lo, hi));
            } else {
                items.push(SetItem::Range(lo, lo));
            }
        }
        Ok(CharSet { negated, items })
    }
}

fn has_captures(raw: &Raw) -> bool {
    match raw {
        Raw::Capture(..) => true,
        Raw::Concat(v) | Raw::Alt(v) => v.iter().any(has_captures),
        Raw::Repeat(r, ..) | Raw::Look(_, _, r) | Raw::Atomic(r) => has_captures(r),
        _ => false,
    }
}

fn resolve(raw: Raw, unnamed: usize, names: &HashMap<String, usize>) -> Result<Node, String> {
    Ok(match raw {
        Raw::Node(n) => n,
        Raw::Concat(v) => Node::Concat(
            v.into_iter()
                .map(|r| resolve(r, unnamed, names))
                .collect::<Result<_, _>>()?,
        ),
        Raw::Alt(v) => Node::Alt(
            v.into_iter()
                .map(|r| resolve(r, unnamed, names))
                .collect::<Result<_, _>>()?,
        ),
        Raw::Capture(group, inner) => Node::Capture {
            index: match group {
                GroupRef::Unnamed(i) => i,
                GroupRef::Named(n) => names[&n],
            },
            node: Box::new(resolve(*inner, unnamed, names)?),
        },
        Raw::Repeat(inner, min, max, greedy) => Node::Repeat {
            node: Box::new(resolve(*inner, unnamed, names)?),
            min,
            max,
            greedy,
        },
        Raw::Look(behind, negated, inner) => {
            let captures = has_captures(&inner);
            Node::Look {
                behind,
                negated,
                node: Box::new(resolve(*inner, unnamed, names)?),
                captures,
            }
        }
        Raw::Atomic(inner) => {
            let captures = has_captures(&inner);
            Node::Atomic {
                node: Box::new(resolve(*inner, unnamed, names)?),
                captures,
            }
        }
        Raw::BackRefName(name, ci) => Node::BackRef {
            index: *names
                .get(&name)
                .ok_or_else(|| format!("reference to undefined group {name}"))?,
            ci,
        },
        Raw::BackRefNumber(n, ci) => {
            if n > unnamed + names.len() {
                return Err(format!("reference to undefined group {n}"));
            }
            Node::BackRef { index: n, ci }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Character classes

static UNICODE_DIGIT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\p{Nd}$").unwrap_or_else(|e| panic!("{e}")));
static UNICODE_WORD: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[\p{L}\p{Mn}\p{Nd}\p{Pc}]$").unwrap_or_else(|e| panic!("{e}"))
});
static UNICODE_LETTER_OR_DIGIT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[\p{L}\p{Nd}]$").unwrap_or_else(|e| panic!("{e}")));
static UNICODE_SPACE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[\x{85}\p{Z}]$").unwrap_or_else(|e| panic!("{e}")));

fn unicode_is(re: &regex::Regex, c: char) -> bool {
    let mut buf = [0u8; 4];
    re.is_match(c.encode_utf8(&mut buf))
}

/// .NET `\d`: Unicode decimal digits.
pub(crate) fn is_digit(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        unicode_is(&UNICODE_DIGIT, c)
    }
}

/// .NET `\w`: letters, non-spacing marks, decimal digits and connector punctuation.
pub(crate) fn is_word(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric() || c == '_'
    } else {
        unicode_is(&UNICODE_WORD, c)
    }
}

/// .NET `char.IsLetterOrDigit`.
pub(crate) fn is_letter_or_digit(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric()
    } else {
        unicode_is(&UNICODE_LETTER_OR_DIGIT, c)
    }
}

/// The value of a Unicode decimal digit (`char.GetNumericValue` for `\d` characters). Decimal
/// digit blocks are runs of ten starting at zero.
pub(crate) fn digit_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return c.to_digit(10);
    }
    if !is_digit(c) {
        return None;
    }
    let mut cp = c as u32;
    let mut steps = 0;
    while steps < 100 && char::from_u32(cp.wrapping_sub(1)).is_some_and(is_digit) {
        cp -= 1;
        steps += 1;
    }
    Some(steps % 10)
}

/// .NET `\s`.
fn is_space(c: char) -> bool {
    if c.is_ascii() {
        matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r')
    } else {
        unicode_is(&UNICODE_SPACE, c)
    }
}

/// Word characters for `\b` (.NET also counts ZWJ and ZWNJ).
fn is_boundary_word(c: char) -> bool {
    is_word(c) || c == '\u{200D}' || c == '\u{200C}'
}

fn builtin(b: Builtin, c: char) -> bool {
    match b {
        Builtin::Digit => is_digit(c),
        Builtin::Word => is_word(c),
        Builtin::Space => is_space(c),
    }
}

/// Single-character lower-casing, as .NET's `ToLowerInvariant` does it.
pub(crate) fn simple_lower(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

fn simple_upper(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_uppercase();
    }
    let mut it = c.to_uppercase();
    match (it.next(), it.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

impl CharSet {
    fn contains_exact(&self, c: char) -> bool {
        self.items.iter().any(|item| match *item {
            SetItem::Range(lo, hi) => lo <= c && c <= hi,
            SetItem::Class(b, negated) => builtin(b, c) != negated,
        })
    }

    fn matches(&self, c: char, ci: bool) -> bool {
        let found = self.contains_exact(c)
            || (ci
                && (self.contains_exact(simple_lower(c)) || self.contains_exact(simple_upper(c))));
        found != self.negated
    }
}

fn chars_eq(a: char, b: char, ci: bool) -> bool {
    a == b || (ci && simple_lower(a) == simple_lower(b))
}

// ---------------------------------------------------------------------------------------------
// Matcher

type Span = (usize, usize);

struct Ctx<'t> {
    text: &'t [char],
    caps: Vec<Vec<Span>>,
    steps: u64,
    aborted: bool,
}

type Cont<'a, 't> = &'a mut dyn FnMut(&mut Ctx<'t>, usize) -> bool;

impl<'t> Ctx<'t> {
    fn next(&self, pos: usize, rtl: bool) -> Option<(char, usize)> {
        if rtl {
            (pos > 0).then(|| (self.text[pos - 1], pos - 1))
        } else {
            self.text.get(pos).map(|&c| (c, pos + 1))
        }
    }

    fn marks(&self) -> Vec<usize> {
        self.caps.iter().map(Vec::len).collect()
    }

    fn rewind(&mut self, marks: &[usize]) {
        for (stack, &len) in self.caps.iter_mut().zip(marks) {
            stack.truncate(len);
        }
    }

    fn m(&mut self, node: &Node, pos: usize, rtl: bool, k: Cont<'_, 't>) -> bool {
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            self.aborted = true;
        }
        if self.aborted {
            return false;
        }
        match node {
            Node::Empty => k(self, pos),
            Node::Char { c, ci } => match self.next(pos, rtl) {
                Some((ch, np)) if chars_eq(ch, *c, *ci) => k(self, np),
                _ => false,
            },
            Node::Any => match self.next(pos, rtl) {
                Some((ch, np)) if ch != '\n' => k(self, np),
                _ => false,
            },
            Node::Set { set, ci } => match self.next(pos, rtl) {
                Some((ch, np)) if set.matches(ch, *ci) => k(self, np),
                _ => false,
            },
            Node::Bol | Node::StartOfText => pos == 0 && k(self, pos),
            Node::Eol | Node::EndOfTextOrNewline => {
                let len = self.text.len();
                (pos == len || (pos + 1 == len && self.text[pos] == '\n')) && k(self, pos)
            }
            Node::EndOfText => pos == self.text.len() && k(self, pos),
            Node::WordBoundary { negated } => {
                let before = pos > 0 && is_boundary_word(self.text[pos - 1]);
                let after = pos < self.text.len() && is_boundary_word(self.text[pos]);
                ((before != after) != *negated) && k(self, pos)
            }
            Node::Concat(nodes) => self.seq(nodes, 0, pos, rtl, k),
            Node::Alt(branches) => {
                for b in branches {
                    if self.m(b, pos, rtl, k) {
                        return true;
                    }
                    if self.aborted {
                        return false;
                    }
                }
                false
            }
            Node::Capture { index, node } => {
                let index = *index;
                self.m(node, pos, rtl, &mut |cx: &mut Ctx<'t>, p| {
                    let span = if rtl { (p, pos) } else { (pos, p) };
                    cx.caps[index].push(span);
                    if k(cx, p) {
                        true
                    } else {
                        cx.caps[index].pop();
                        false
                    }
                })
            }
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => self.rep(node, *min, *max, *greedy, 0, pos, rtl, k),
            Node::Look {
                behind,
                negated,
                node,
                captures,
            } => {
                let marks = if *captures { Some(self.marks()) } else { None };
                let found = self.m(node, pos, *behind, &mut |_, _| true);
                if self.aborted {
                    return false;
                }
                if *negated {
                    if let Some(m) = &marks {
                        self.rewind(m);
                    }
                    !found && k(self, pos)
                } else if !found {
                    false
                } else if k(self, pos) {
                    true
                } else {
                    if let Some(m) = &marks {
                        self.rewind(m);
                    }
                    false
                }
            }
            Node::Atomic { node, captures } => {
                let marks = if *captures { Some(self.marks()) } else { None };
                let mut end = None;
                let found = self.m(node, pos, rtl, &mut |_, p| {
                    end = Some(p);
                    true
                });
                let Some(end) = end.filter(|_| found) else {
                    return false;
                };
                if k(self, end) {
                    true
                } else {
                    if let Some(m) = &marks {
                        self.rewind(m);
                    }
                    false
                }
            }
            Node::BackRef { index, ci } => {
                let Some(&(s, e)) = self.caps[*index].last() else {
                    return false;
                };
                let len = e - s;
                let ok = if rtl {
                    pos >= len
                        && (0..len)
                            .all(|i| chars_eq(self.text[pos - len + i], self.text[s + i], *ci))
                } else {
                    pos + len <= self.text.len()
                        && (0..len).all(|i| chars_eq(self.text[pos + i], self.text[s + i], *ci))
                };
                ok && k(self, if rtl { pos - len } else { pos + len })
            }
        }
    }

    fn seq(&mut self, nodes: &[Node], i: usize, pos: usize, rtl: bool, k: Cont<'_, 't>) -> bool {
        if i == nodes.len() {
            return k(self, pos);
        }
        let node = if rtl {
            &nodes[nodes.len() - 1 - i]
        } else {
            &nodes[i]
        };
        self.m(node, pos, rtl, &mut |cx: &mut Ctx<'t>, p| {
            cx.seq(nodes, i + 1, p, rtl, k)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(
        &mut self,
        node: &Node,
        min: u32,
        max: u32,
        greedy: bool,
        count: u32,
        pos: usize,
        rtl: bool,
        k: Cont<'_, 't>,
    ) -> bool {
        let more = count < max;
        let iterate = |cx: &mut Ctx<'t>, k: Cont<'_, 't>| {
            cx.m(node, pos, rtl, &mut |cx: &mut Ctx<'t>, p| {
                // An empty iteration once the minimum is met would loop forever.
                if p == pos && count >= min {
                    return false;
                }
                cx.rep(node, min, max, greedy, count.saturating_add(1), p, rtl, k)
            })
        };
        if greedy {
            if more && iterate(self, k) {
                return true;
            }
            !self.aborted && count >= min && k(self, pos)
        } else {
            if count >= min && k(self, pos) {
                return true;
            }
            !self.aborted && more && iterate(self, k)
        }
    }
}

fn starts_with_dot_star(node: &Node) -> bool {
    match node {
        Node::Concat(v) => v.first().is_some_and(starts_with_dot_star),
        Node::Repeat {
            node,
            min: 0,
            max: u32::MAX,
            greedy: true,
        } => matches!(**node, Node::Any),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Public API

impl NetRegex {
    /// Compiles a .NET pattern. `ignore_case` is `RegexOptions.IgnoreCase`.
    pub fn new(pattern: &str, ignore_case: bool) -> Result<Self, String> {
        let mut p = Parser {
            chars: pattern.chars().collect(),
            pos: 0,
            unnamed: 0,
            named_order: Vec::new(),
            pattern,
        };
        let mut ci = ignore_case;
        let raw = p.parse_alt(&mut ci)?;
        if p.pos != p.chars.len() {
            return p.err("unbalanced )");
        }
        let unnamed = p.unnamed;
        // .NET numbering: unnamed groups first, then named groups in order of appearance.
        let names: HashMap<String, usize> = p
            .named_order
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), unnamed + 1 + i))
            .collect();
        let groups = unnamed + names.len() + 1;
        let root = resolve(raw, unnamed, &names)?;
        let leading_dot_star = starts_with_dot_star(&root);
        let anchored = match &root {
            Node::Bol | Node::StartOfText => true,
            Node::Concat(v) => matches!(v.first(), Some(Node::Bol | Node::StartOfText)),
            _ => false,
        };
        Ok(Self {
            root,
            names,
            groups,
            anchored,
            leading_dot_star,
            pattern: pattern.to_string(),
        })
    }

    /// Like [`Self::new`] for patterns known to be valid (compile-time constants).
    pub fn compile(pattern: &str, ignore_case: bool) -> Self {
        Self::new(pattern, ignore_case).unwrap_or_else(|e| panic!("{e}"))
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    fn search(&self, text: &[char], from: usize) -> Option<Vec<Vec<Span>>> {
        if text.len() > MAX_INPUT_CHARS {
            return None;
        }
        let single_start = self.anchored || (self.leading_dot_star && !text.contains(&'\n'));
        let last = if single_start { from } else { text.len() };
        let mut ctx = Ctx {
            text,
            caps: vec![Vec::new(); self.groups],
            steps: 0,
            aborted: false,
        };
        for start in from..=last {
            if ctx.aborted {
                tracing::debug!(pattern = %self.pattern, "regex step budget exhausted");
                return None;
            }
            for stack in &mut ctx.caps {
                stack.clear();
            }
            let mut end = None;
            if ctx.m(&self.root, start, false, &mut |_, p| {
                end = Some(p);
                true
            }) {
                let mut caps = std::mem::take(&mut ctx.caps);
                caps[0] = vec![(start, end.unwrap_or(start))];
                return Some(caps);
            }
        }
        None
    }

    /// `Regex.Match`: the first match.
    pub fn find(&self, text: &str) -> Option<NetMatch<'_>> {
        let chars: Vec<char> = text.chars().collect();
        let caps = self.search(&chars, 0)?;
        Some(NetMatch {
            regex: self,
            text: chars.into(),
            caps,
        })
    }

    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        self.search(&chars, 0).is_some()
    }

    /// `Regex.Matches`: all non-overlapping matches, left to right.
    pub fn find_all(&self, text: &str) -> Vec<NetMatch<'_>> {
        let chars: std::rc::Rc<[char]> = text.chars().collect::<Vec<_>>().into();
        let mut out = Vec::new();
        let mut from = 0;
        while from <= chars.len() {
            let Some(caps) = self.search(&chars, from) else {
                break;
            };
            let (s, e) = caps[0][0];
            from = if e == s { e + 1 } else { e };
            out.push(NetMatch {
                regex: self,
                text: chars.clone(),
                caps,
            });
        }
        out
    }

    /// `Regex.Replace` with a function deciding each replacement.
    pub fn replace_with(&self, text: &str, mut f: impl FnMut(&NetMatch<'_>) -> String) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for m in self.find_all(text) {
            let (s, e) = m.span();
            out.extend(&chars[last..s]);
            out.push_str(&f(&m));
            last = e;
        }
        out.extend(&chars[last..]);
        out
    }

    /// `Regex.Replace` with a literal replacement.
    pub fn replace(&self, text: &str, replacement: &str) -> String {
        self.replace_with(text, |_| replacement.to_string())
    }
}

/// One capture of a group: char offsets into the input and the captured text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub start: usize,
    pub end: usize,
    pub value: String,
}

/// A successful match (`System.Text.RegularExpressions.Match`). Offsets are in characters.
pub struct NetMatch<'r> {
    regex: &'r NetRegex,
    text: std::rc::Rc<[char]>,
    caps: Vec<Vec<Span>>,
}

impl NetMatch<'_> {
    fn stack(&self, name: &str) -> &[Span] {
        self.regex
            .names
            .get(name)
            .map_or(&[][..], |&i| self.caps[i].as_slice())
    }

    fn text_of(&self, (s, e): Span) -> String {
        self.text[s..e].iter().collect()
    }

    /// Whole-match span.
    pub fn span(&self) -> Span {
        self.caps[0][0]
    }

    pub fn value_all(&self) -> String {
        self.text_of(self.span())
    }

    /// `Groups[name].Success`.
    pub fn success(&self, name: &str) -> bool {
        !self.stack(name).is_empty()
    }

    /// `Groups[name].Value`: the last capture, or empty.
    pub fn value(&self, name: &str) -> String {
        self.stack(name)
            .last()
            .map(|&s| self.text_of(s))
            .unwrap_or_default()
    }

    /// `Groups[name].Value` as `Option` (None when the group did not participate).
    pub fn get(&self, name: &str) -> Option<String> {
        self.stack(name).last().map(|&s| self.text_of(s))
    }

    /// `Groups[name].Captures`, in match order.
    pub fn captures(&self, name: &str) -> Vec<Capture> {
        self.stack(name)
            .iter()
            .map(|&(start, end)| Capture {
                start,
                end,
                value: self.text_of((start, end)),
            })
            .collect()
    }

    /// `Groups[name].EndIndex()`: end of the last capture, 0 when the group did not participate.
    pub fn end_index(&self, name: &str) -> usize {
        self.stack(name).last().map_or(0, |&(_, e)| e)
    }

    /// `Groups[number].Value` for numbered groups.
    pub fn group(&self, number: usize) -> Option<String> {
        self.caps
            .get(number)
            .and_then(|s| s.last())
            .map(|&s| self.text_of(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(p: &str) -> NetRegex {
        NetRegex::compile(p, true)
    }

    #[test]
    fn capture_collections_keep_every_iteration() {
        let r = re(r"^(?<title>.+?)[-_. ]S(?<season>\d{2})(?:E(?<episode>\d{2}))+");
        let m = r.find("Series.Title.S01E05E06E07.720p").unwrap();
        assert_eq!(m.value("title"), "Series.Title");
        let eps: Vec<String> = m.captures("episode").into_iter().map(|c| c.value).collect();
        assert_eq!(eps, ["05", "06", "07"]);
        assert_eq!(m.value("episode"), "07", "Value is the last capture");
        assert_eq!(m.end_index("episode"), 25);
    }

    #[test]
    fn reused_group_names_accumulate() {
        let r = re(r"^(?<title>[一-鿌]+)[_. ](?<title>.+?)\.S(?<season>\d+)");
        let m = r.find("中文.English Title.S02").unwrap();
        let titles: Vec<String> = m.captures("title").into_iter().map(|c| c.value).collect();
        assert_eq!(titles, ["中文", "English Title"]);
    }

    #[test]
    fn variable_length_lookbehind_and_backreference() {
        let r = re(r"(?<!\d+)\d{2}(?!\d+)");
        assert_eq!(r.find("S1234 E05").unwrap().value_all(), "05");
        let d = re(r"^(?<y>\d{4})(?<sep>[-_]?)(?<m>\d\d)\k<sep>(?<d>\d\d)");
        assert!(d.is_match("2018-10-12"));
        assert!(d.is_match("20181012"));
        assert!(!d.is_match("2018-1012"));
        // Backreference inside a lookbehind, as in Sonarr's release group pattern.
        let g = re(r"-(?<g>[a-z]+(?<p2>-[a-z]+)?)(?<!(?:dts-hd)(?:\k<p2>)?)$");
        assert!(g.is_match("title-group"));
        assert!(!g.is_match("title.dts-hd"));
    }

    #[test]
    fn negative_lookaround_discards_captures_and_atomic_does_not_backtrack() {
        let r = re(r"(?!(?<x>a))(?<y>b)");
        let m = r.find("b").unwrap();
        assert!(!m.success("x"));
        let atomic = re(r"^(?>a+)a");
        assert!(!atomic.is_match("aaa"), "the atomic group keeps all a's");
        let plain = re(r"^(?:a+)a");
        assert!(plain.is_match("aaa"));
    }

    #[test]
    fn options_classes_and_quantifiers() {
        assert!(re(r"(?-i:WEB)$").is_match("x WEB"));
        assert!(!re(r"(?-i:WEB)$").is_match("x web"));
        assert!(NetRegex::compile(r"(?:(?i)(?<!SUB[\W|_|^]))\bCZ\b", false).is_match("Title.CZ.x"));
        assert!(
            !NetRegex::compile(r"(?:(?i)(?<!SUB[\W|_|^]))\bCZ\b", false).is_match("Title.sub.CZ")
        );
        assert_eq!(re(r"[^]]+").find("ab]c").unwrap().value_all(), "ab");
        assert_eq!(re(r"x{2,}").find("axxxb").unwrap().value_all(), "xxx");
        assert_eq!(re(r"a{1,2}?").find("aa").unwrap().value_all(), "a");
        assert_eq!(
            re(r"\d+").find("Ep ٣٤").unwrap().value_all(),
            "٣٤",
            "Unicode digits"
        );
        assert_eq!(
            re(r"a{,2}").find("a{,2}").unwrap().value_all(),
            "a{,2}",
            "invalid braces are literal"
        );
        assert!(
            re(r"^abc$").is_match("abc\n"),
            "$ matches before a final newline"
        );
    }

    #[test]
    fn dotnet_group_numbering_and_matches() {
        let r = NetRegex::compile(r"(?<n>x)(y)", false);
        let m = r.find("xy").unwrap();
        assert_eq!(
            m.group(1).as_deref(),
            Some("y"),
            "unnamed groups are numbered first"
        );
        assert_eq!(m.group(2).as_deref(), Some("x"));
        let digits = re(r"\d");
        assert_eq!(digits.find_all("a1b22").len(), 3);
        assert_eq!(
            re("x*").find_all("ab").len(),
            3,
            "empty matches advance one char"
        );
    }
}
