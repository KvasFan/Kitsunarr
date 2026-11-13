//! XML-RPC (Sonarr's `XmlRpcRequestBuilder` and `XmlExtensions`), for rTorrent and Aria2.

use base64::Engine;
use quick_xml::events::Event;

use crate::DownloadError;

/// A call argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg<'a> {
    Str(&'a str),
    Int(i64),
    Bool(bool),
    Bytes(&'a [u8]),
    List(Vec<Arg<'a>>),
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn value(a: &Arg<'_>) -> String {
    match a {
        Arg::Str(s) => format!("<value><string>{}</string></value>", escape(s)),
        Arg::Int(i) => format!("<value><i4>{i}</i4></value>"),
        Arg::Bool(b) => format!("<value><boolean>{}</boolean></value>", u8::from(*b)),
        Arg::Bytes(b) => format!(
            "<value><base64>{}</base64></value>",
            base64::engine::general_purpose::STANDARD.encode(b)
        ),
        Arg::List(items) => format!(
            "<value><array><data>{}</data></array></value>",
            items.iter().map(value).collect::<String>()
        ),
    }
}

/// `<methodCall>` for a method and its arguments.
pub fn request(method: &str, args: &[Arg<'_>]) -> String {
    let params: String = args
        .iter()
        .map(|a| format!("<param>{}</param>", value(a)))
        .collect();
    format!(
        "<?xml version=\"1.0\"?><methodCall><methodName>{}</methodName><params>{params}</params></methodCall>",
        escape(method)
    )
}

/// A response value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Double(f64),
    Bool(bool),
    Array(Vec<Value>),
    Struct(Vec<(String, Value)>),
}

impl Value {
    /// `GetStringValue`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// `GetIntValue` / `GetLongValue` (numbers written as strings too).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Bool(b) => Some(i64::from(*b)),
            Value::Double(d) => Some(*d as i64),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Value] {
        match self {
            Value::Array(a) => a,
            _ => &[],
        }
    }

    pub fn member(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Struct(m) => m.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
struct Node {
    name: String,
    text: String,
    children: Vec<Node>,
}

fn tree(xml: &str) -> Result<Node, String> {
    let mut reader = quick_xml::Reader::from_str(xml.trim_start_matches('\u{feff}'));
    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => stack.push(Node {
                name: e.local_name().as_ref().to_string(),
                ..Default::default()
            }),
            Ok(Event::Empty(e)) => {
                let node = Node {
                    name: e.local_name().as_ref().to_string(),
                    ..Default::default()
                };
                if let Some(top) = stack.last_mut() {
                    top.children.push(node);
                }
            }
            Ok(Event::End(_)) => {
                if stack.len() > 1 {
                    let node = stack.pop().unwrap_or_default();
                    if let Some(top) = stack.last_mut() {
                        top.children.push(node);
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(top) = stack.last_mut() {
                    let raw: &str = &t;
                    top.text.push_str(raw);
                }
            }
            Ok(Event::CData(c)) => {
                if let Some(top) = stack.last_mut() {
                    let raw: &str = &c;
                    top.text.push_str(raw);
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some(top) = stack.last_mut() {
                    let name: &str = &r;
                    top.text.push_str(match name {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("invalid XML-RPC response: {e}")),
        }
    }
    stack.pop().ok_or_else(|| "empty response".to_string())
}

fn child<'n>(n: &'n Node, name: &str) -> Option<&'n Node> {
    n.children.iter().find(|c| c.name == name)
}

fn to_value(v: &Node) -> Value {
    let Some(typed) = v.children.first() else {
        // An untyped value is a string.
        return Value::Str(v.text.clone());
    };
    match typed.name.as_str() {
        "string" => Value::Str(typed.text.clone()),
        "i4" | "i8" | "int" => Value::Int(typed.text.trim().parse().unwrap_or(0)),
        "boolean" => Value::Bool(typed.text.trim() == "1"),
        "double" => Value::Double(typed.text.trim().parse().unwrap_or(0.0)),
        "array" => Value::Array(
            child(typed, "data")
                .map(|d| {
                    d.children
                        .iter()
                        .filter(|c| c.name == "value")
                        .map(to_value)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "struct" => Value::Struct(
            typed
                .children
                .iter()
                .filter(|m| m.name == "member")
                .filter_map(|m| {
                    Some((
                        child(m, "name")?.text.clone(),
                        child(m, "value").map(to_value)?,
                    ))
                })
                .collect(),
        ),
        _ => Value::Str(typed.text.clone()),
    }
}

/// The response's first parameter, or the fault as an error.
pub fn response(xml: &str, client: &str) -> Result<Value, DownloadError> {
    let doc = tree(xml).map_err(DownloadError::Client)?;
    let root = child(&doc, "methodResponse")
        .ok_or_else(|| DownloadError::Client(format!("{client} did not answer with XML-RPC")))?;
    if let Some(fault) = child(root, "fault").and_then(|f| child(f, "value")) {
        let f = to_value(fault);
        return Err(DownloadError::Client(format!(
            "{client} returned error code {}: {}",
            f.member("faultCode").and_then(Value::as_i64).unwrap_or(0),
            f.member("faultString")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )));
    }
    Ok(child(root, "params")
        .and_then(|p| child(p, "param"))
        .and_then(|p| child(p, "value"))
        .map(to_value)
        .unwrap_or(Value::Str(String::new())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_responses() {
        assert_eq!(
            request(
                "load.raw_start",
                &[
                    Arg::Str(""),
                    Arg::Bytes(b"d4:infoe"),
                    Arg::Str("d.custom1.set=tv & more")
                ]
            ),
            "<?xml version=\"1.0\"?><methodCall><methodName>load.raw_start</methodName><params>\
             <param><value><string></string></value></param>\
             <param><value><base64>ZDQ6aW5mb2U=</base64></value></param>\
             <param><value><string>d.custom1.set=tv &amp; more</string></value></param></params></methodCall>"
        );
        let multi = response(
            r#"<?xml version="1.0"?><methodResponse><params><param><value><array><data>
               <value><array><data><value><string>Show S01E01</string></value><value>ABC</value><value><i8>12</i8></value></data></array></value>
               </data></array></value></param></params></methodResponse>"#,
            "rTorrent",
        )
        .unwrap();
        let row = multi.as_array()[0].as_array();
        assert_eq!(row[0].as_str(), Some("Show S01E01"));
        assert_eq!(row[1].as_str(), Some("ABC"));
        assert_eq!(row[2].as_i64(), Some(12));
        let fault = response(
            "<methodResponse><fault><value><struct><member><name>faultCode</name><value><i4>-501</i4></value></member>\
             <member><name>faultString</name><value><string>Could not find info-hash.</string></value></member></struct></value></fault></methodResponse>",
            "rTorrent",
        )
        .unwrap_err();
        assert_eq!(
            fault.to_string(),
            "rTorrent returned error code -501: Could not find info-hash."
        );
    }
}
