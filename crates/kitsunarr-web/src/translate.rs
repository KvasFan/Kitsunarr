//! Interface translations (Sonarr's UI languages). The interface is written in English; for
//! another language, a mutation observer rewrites the document's text nodes and its
//! `placeholder` and `title` attributes from that language's dictionary as they appear, so
//! every page is covered without the components knowing. Dioxus updates nodes by reference and
//! never reads their text back, so rewritten text stays until the component changes it (and is
//! then translated again).
//!
//! A dictionary is a TSV of `English<TAB>translation` lines. `{...}` marks a placeholder: an
//! entry with placeholders matches any text with the same fixed parts, and the translation
//! puts each captured value where the same placeholder appears.

use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

/// Languages the interface can be shown in: (code, name in that language).
pub use kitsunarr_domain::api::settings::UI_LANGUAGES as LANGUAGES;

const STORAGE_KEY: &str = "kitsunarr.language";

fn dictionary_source(lang: &str) -> Option<&'static str> {
    match lang {
        "de" => Some(include_str!("../translations/de.tsv")),
        "es" => Some(include_str!("../translations/es.tsv")),
        "fr" => Some(include_str!("../translations/fr.tsv")),
        "it" => Some(include_str!("../translations/it.tsv")),
        "pt_BR" => Some(include_str!("../translations/pt_BR.tsv")),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Text(String),
    Hole(String),
}

/// Splits a dictionary entry into fixed text and placeholders; anonymous placeholders (`{}`)
/// are numbered in order.
fn pieces(s: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut rest = s;
    let mut anonymous = 0;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        if start > 0 {
            out.push(Piece::Text(rest[..start].to_string()));
        }
        let name = &rest[start + 1..start + len];
        let name = if name.is_empty() || name.starts_with(':') {
            anonymous += 1;
            format!("#{anonymous}")
        } else {
            name.to_string()
        };
        out.push(Piece::Hole(name));
        rest = &rest[start + len + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest.to_string()));
    }
    out
}

#[derive(Debug, Default)]
pub struct Dictionary {
    exact: HashMap<String, String>,
    patterns: Vec<(Vec<Piece>, Vec<Piece>)>,
}

impl Dictionary {
    pub fn parse(tsv: &str) -> Self {
        let mut d = Dictionary::default();
        for line in tsv.lines() {
            let Some((en, tr)) = line.split_once('\t') else {
                continue;
            };
            if en.trim().is_empty() || tr.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            if en.contains('{') {
                let p = pieces(en.trim());
                // A pattern needs some fixed text to anchor on.
                if p.iter()
                    .any(|x| matches!(x, Piece::Text(t) if t.trim().len() > 1))
                {
                    d.patterns.push((p, pieces(tr.trim())));
                }
            } else {
                d.exact.insert(en.trim().to_string(), tr.trim().to_string());
            }
        }
        // Longer patterns first: the most specific wins.
        d.patterns.sort_by_key(|(p, _)| {
            std::cmp::Reverse(
                p.iter()
                    .map(|x| match x {
                        Piece::Text(t) => t.len(),
                        Piece::Hole(_) => 0,
                    })
                    .sum::<usize>(),
            )
        });
        d
    }

    fn matches(pattern: &[Piece], text: &str) -> Option<HashMap<String, String>> {
        let mut values = HashMap::new();
        let mut pos = 0usize;
        let mut pending: Option<&str> = None;
        let last = pattern.len().saturating_sub(1);
        for (i, piece) in pattern.iter().enumerate() {
            match piece {
                Piece::Text(t) => {
                    // The final fixed text anchors at the end; others match lazily.
                    let found = if i == last {
                        let at = text.len().checked_sub(t.len())?;
                        if at < pos || !text.ends_with(t.as_str()) {
                            return None;
                        }
                        at - pos
                    } else {
                        text[pos..].find(t.as_str())?
                    };
                    match pending.take() {
                        Some(name) => {
                            if found == 0 {
                                return None;
                            }
                            values.insert(name.to_string(), text[pos..pos + found].to_string());
                        }
                        None if found != 0 => return None,
                        None => {}
                    }
                    pos += found + t.len();
                }
                Piece::Hole(name) => pending = Some(name),
            }
        }
        match pending {
            Some(name) if pos < text.len() => {
                values.insert(name.to_string(), text[pos..].to_string());
            }
            Some(_) => return None,
            None if pos != text.len() => return None,
            None => {}
        }
        Some(values)
    }

    /// The translation of a whole text (surrounding whitespace kept), if any.
    pub fn translate(&self, text: &str) -> Option<String> {
        let core = text.trim();
        if core.is_empty() || !core.chars().any(char::is_alphabetic) {
            return None;
        }
        let lead = &text[..text.len() - text.trim_start().len()];
        let trail = &text[text.trim_end().len()..];
        let translated = match self.exact.get(core) {
            Some(t) => t.clone(),
            None => self.patterns.iter().find_map(|(en, tr)| {
                let values = Self::matches(en, core)?;
                Some(
                    tr.iter()
                        .map(|p| match p {
                            Piece::Text(t) => t.clone(),
                            Piece::Hole(h) => values.get(h).cloned().unwrap_or_default(),
                        })
                        .collect::<String>(),
                )
            })?,
        };
        (translated != core).then(|| format!("{lead}{translated}{trail}"))
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Dictionary>> = const { RefCell::new(None) };
    static OBSERVER: RefCell<Option<web_sys::MutationObserver>> = const { RefCell::new(None) };
}

/// The language saved in this browser (`en` by default).
pub fn stored_language() -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
        .unwrap_or_else(|| "en".into())
}

/// Remembers the language and reloads the page when it changed (English text that was
/// already translated cannot be restored in place).
pub fn set_language(lang: &str) {
    if stored_language() == lang {
        return;
    }
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(STORAGE_KEY, lang);
    }
    if let Some(w) = web_sys::window() {
        let _ = w.location().reload();
    }
}

fn translate_node(node: &web_sys::Node) {
    ACTIVE.with(|d| {
        let d = d.borrow();
        let Some(dict) = d.as_ref() else { return };
        visit(dict, node);
    });
}

fn visit(dict: &Dictionary, node: &web_sys::Node) {
    match node.node_type() {
        web_sys::Node::TEXT_NODE => {
            if let Some(text) = node.node_value()
                && let Some(t) = dict.translate(&text)
            {
                node.set_node_value(Some(&t));
            }
        }
        web_sys::Node::ELEMENT_NODE => {
            if let Some(el) = node.dyn_ref::<web_sys::Element>() {
                let tag = el.tag_name();
                if tag == "SCRIPT"
                    || tag == "STYLE"
                    || tag == "TEXTAREA"
                    || el.class_name().contains("mono")
                {
                    return;
                }
                for attr in ["placeholder", "title"] {
                    if let Some(v) = el.get_attribute(attr)
                        && let Some(t) = dict.translate(&v)
                    {
                        let _ = el.set_attribute(attr, &t);
                    }
                }
            }
            let children = node.child_nodes();
            for i in 0..children.length() {
                if let Some(c) = children.item(i) {
                    visit(dict, &c);
                }
            }
        }
        _ => {}
    }
}

/// Starts translating the document into the stored language (nothing for English).
pub fn install() {
    let lang = stored_language();
    let Some(source) = dictionary_source(&lang) else {
        return;
    };
    ACTIVE.with(|d| *d.borrow_mut() = Some(Dictionary::parse(source)));
    if let Some(html) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = html.set_attribute("lang", &lang.replace('_', "-"));
    }
    let Some(body) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
    else {
        return;
    };
    translate_node(&body);
    let callback = Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(
        move |records: js_sys::Array, _observer: web_sys::MutationObserver| {
            for r in records.iter() {
                let Ok(record) = r.dyn_into::<web_sys::MutationRecord>() else {
                    continue;
                };
                match record.type_().as_str() {
                    "childList" => {
                        let added = record.added_nodes();
                        for i in 0..added.length() {
                            if let Some(n) = added.item(i) {
                                translate_node(&n);
                            }
                        }
                    }
                    _ => {
                        if let Some(n) = record.target() {
                            translate_node(&n);
                        }
                    }
                }
            }
        },
    );
    let Ok(observer) = web_sys::MutationObserver::new(callback.as_ref().unchecked_ref()) else {
        return;
    };
    callback.forget();
    let options = web_sys::MutationObserverInit::new();
    options.set_child_list(true);
    options.set_subtree(true);
    options.set_character_data(true);
    options.set_attributes(true);
    options.set_attribute_filter(&js_sys::Array::of2(&"placeholder".into(), &"title".into()));
    let _ = observer.observe_with_options(&body, &options);
    OBSERVER.with(|o| *o.borrow_mut() = Some(observer));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_italian_dictionary_translates_the_interface() {
        let d = Dictionary::parse(dictionary_source("it").unwrap());
        assert!(d.exact.len() + d.patterns.len() > 900);
        for (en, it) in [
            ("Calendar", "Calendario"),
            ("Activity", "Attività"),
            (
                "Disk scanned: 2 added Not matched: x.mkv",
                "Disco analizzato: 2 added Non abbinati: x.mkv",
            ),
            ("Season 3", "Stagione 3"),
            ("12 GiB free of 100 GiB", "12 GiB liberi su 100 GiB"),
            (
                "Connected to Download Station.",
                "Connected to Download Station.",
            ),
        ] {
            let got = d.translate(en).unwrap_or_else(|| en.to_string());
            assert_eq!(got, it, "{en}");
        }
    }

    #[test]
    fn exact_and_pattern_entries() {
        let d = Dictionary::parse(
            "Calendar\tCalendario\n\
             Connected to {version}.\tConnesso a {version}.\n\
             {n} of {total} series\t{n} serie su {total}\n\
             # comment\tignored\n\
             {x}\tnot a pattern\n",
        );
        assert_eq!(d.translate("Calendar").as_deref(), Some("Calendario"));
        assert_eq!(d.translate("  Calendar ").as_deref(), Some("  Calendario "));
        assert_eq!(
            d.translate("Connected to qBittorrent 5.0.").as_deref(),
            Some("Connesso a qBittorrent 5.0.")
        );
        assert_eq!(
            d.translate("3 of 10 series").as_deref(),
            Some("3 serie su 10")
        );
        assert_eq!(d.translate("Unknown"), None);
        assert_eq!(d.translate("12:30"), None);
        assert_eq!(d.translate("Connected to ."), None, "a hole needs a value");
        let d = Dictionary::parse("{} free of {}\t{} liberi su {}\n");
        assert_eq!(
            d.translate("3 GiB free of 9 GiB").as_deref(),
            Some("3 GiB liberi su 9 GiB")
        );
    }

    #[test]
    fn every_language_has_a_complete_dictionary() {
        let english: Vec<String> = dictionary_source("it")
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once('\t').map(|(en, _)| en.to_string()))
            .collect();
        for (code, _) in LANGUAGES.iter().filter(|(c, _)| *c != "en") {
            let source = dictionary_source(code).unwrap_or_else(|| panic!("{code}"));
            let keys: Vec<String> = source
                .lines()
                .filter(|l| !l.starts_with('#'))
                .filter_map(|l| l.split_once('\t').map(|(en, _)| en.to_string()))
                .collect();
            assert_eq!(keys, english, "{code}: same English entries as Italian");
            let d = Dictionary::parse(source);
            assert_ne!(
                d.translate("Calendar").as_deref(),
                Some("Calendar"),
                "{code}"
            );
            assert!(
                d.translate("Season 3").is_some_and(|t| t.contains('3')),
                "{code}"
            );
        }
    }
}
