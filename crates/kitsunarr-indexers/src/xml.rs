//! A minimal namespace-aware XML tree for RSS feeds and capability documents.
//!
//! Feeds are small; a tree is simpler to query than streaming events. Like Sonarr (which
//! pre-cleans feeds with `XmlCleaner`), parsing is lenient: undeclared HTML entities such as
//! `&nbsp;` are decoded when known and otherwise kept literally, and the DTD is ignored.

use quick_xml::events::Event;

#[derive(Debug, Clone, Default)]
pub struct Element {
    /// Namespace URI (empty for none).
    pub ns: String,
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    /// Attribute by local name (case-sensitive, like `XElement.Attribute`).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// First child with this local name and no namespace.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children
            .iter()
            .find(|c| c.name == name && c.ns.is_empty())
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children
            .iter()
            .filter(move |c| c.name == name && c.ns.is_empty())
    }

    /// Children in a namespace with this local name.
    pub fn children_ns<'a>(
        &'a self,
        ns: &'a str,
        name: &'a str,
    ) -> impl Iterator<Item = &'a Element> {
        self.children
            .iter()
            .filter(move |c| c.name == name && c.ns == ns)
    }

    /// Text of the first child with this name (`XElement.TryGetValue`).
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.as_str())
    }

    /// All descendants with this local name, depth first (`Descendants(name)`).
    pub fn descendants<'a>(&'a self, name: &'a str, out: &mut Vec<&'a Element>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.descendants(name, out);
        }
    }
}

/// Character entities HTML feeds use without declaring them (subset of `WebUtility.HtmlDecode`).
pub(crate) fn html_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "ndash" => '–',
        "mdash" => '—',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "middot" => '·',
        "deg" => '°',
        "eacute" => 'é',
        "egrave" => 'è',
        "aacute" => 'á',
        "agrave" => 'à',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "ntilde" => 'ñ',
        "uuml" => 'ü',
        "ouml" => 'ö',
        "auml" => 'ä',
        "szlig" => 'ß',
        "ccedil" => 'ç',
        _ => return None,
    })
}

/// Decodes `&name;`, `&#N;` and `&#xN;` (`WebUtility.HtmlDecode` for the common cases).
pub fn html_decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let decoded = after.find(';').filter(|&j| j > 0 && j <= 10).and_then(|j| {
            let name = &after[..j];
            let c = if let Some(num) = name.strip_prefix('#') {
                if let Some(hex) = num.strip_prefix(['x', 'X']) {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else {
                    num.parse().ok().and_then(char::from_u32)
                }
            } else {
                html_entity(name)
            };
            c.map(|c| (c, j))
        });
        match decoded {
            Some((c, j)) => {
                out.push(c);
                rest = &after[j + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn split_qname(qname: &str) -> (Option<&str>, &str) {
    match qname.split_once(':') {
        Some((p, l)) => (Some(p), l),
        None => (None, qname),
    }
}

struct Frame {
    element: Element,
    prefix: Option<String>,
    scope: Vec<(Option<String>, String)>,
}

fn open(e: &quick_xml::events::BytesStart<'_>) -> Frame {
    let name = e.name();
    let qname: &str = AsRef::<str>::as_ref(&name);
    let (prefix, local) = split_qname(qname);
    let mut scope = Vec::new();
    let mut attrs = Vec::new();
    for attr in e.attributes().flatten() {
        let key = AsRef::<str>::as_ref(&attr.key).to_string();
        let value = match attr.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
            Ok(v) => v.into_owned(),
            Err(_) => html_decode(&attr.value),
        };
        if key == "xmlns" {
            scope.push((None, value));
        } else if let Some(p) = key.strip_prefix("xmlns:") {
            scope.push((Some(p.to_string()), value));
        } else {
            let (_, local_key) = split_qname(&key);
            attrs.push((local_key.to_string(), value));
        }
    }
    Frame {
        element: Element {
            name: local.to_string(),
            attrs,
            ..Default::default()
        },
        prefix: prefix.map(str::to_string),
        scope,
    }
}

fn resolve(prefix: &Option<String>, own: &[(Option<String>, String)], scopes: &[Frame]) -> String {
    own.iter()
        .rev()
        .chain(scopes.iter().rev().flat_map(|f| f.scope.iter().rev()))
        .find(|(p, _)| p == prefix)
        .map(|(_, uri)| uri.clone())
        .unwrap_or_default()
}

/// Parses a document into its root element.
pub fn parse(xml: &str) -> Result<Element, String> {
    let mut reader = quick_xml::Reader::from_str(xml.trim_start_matches('\u{feff}'));
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Element> = None;
    let finish = |mut frame: Frame, stack: &mut Vec<Frame>, root: &mut Option<Element>| {
        frame.element.ns = resolve(&frame.prefix, &frame.scope, stack);
        match stack.last_mut() {
            Some(parent) => parent.element.children.push(frame.element),
            None => {
                if root.is_none() {
                    *root = Some(frame.element);
                }
            }
        }
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let frame = open(&e);
                stack.push(frame);
            }
            Ok(Event::Empty(e)) => {
                let frame = open(&e);
                finish(frame, &mut stack, &mut root);
            }
            Ok(Event::End(_)) => {
                if let Some(frame) = stack.pop() {
                    finish(frame, &mut stack, &mut root);
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(top) = stack.last_mut() {
                    let raw: &str = &t;
                    top.element.text.push_str(raw);
                }
            }
            Ok(Event::CData(c)) => {
                if let Some(top) = stack.last_mut() {
                    let raw: &str = &c;
                    top.element.text.push_str(raw);
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some(top) = stack.last_mut() {
                    let name: &str = &r;
                    let decoded = html_decode(&format!("&{name};"));
                    top.element.text.push_str(&decoded);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("invalid XML at {}: {e}", reader.buffer_position())),
        }
    }
    root.ok_or_else(|| "empty XML document".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namespaces_entities_and_cdata() {
        let doc = parse(
            r#"<?xml version="1.0"?><rss xmlns:nz="urn:x"><channel><item><title>A &amp; B&nbsp;C</title>
               <nz:attr name="size" value="5"/><description><![CDATA[<b>x</b>]]></description></item></channel></rss>"#,
        )
        .unwrap();
        let item = doc.child("channel").unwrap().child("item").unwrap();
        assert_eq!(item.child_text("title"), Some("A & B\u{a0}C"));
        assert_eq!(
            item.children_ns("urn:x", "attr")
                .next()
                .unwrap()
                .attr("value"),
            Some("5")
        );
        assert_eq!(item.child_text("description"), Some("<b>x</b>"));
        assert_eq!(
            html_decode("x &#39;y&#x27; &unknown; &"),
            "x 'y' &unknown; &"
        );
    }
}
