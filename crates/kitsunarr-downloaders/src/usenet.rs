//! Helpers shared by the Usenet clients: Sonarr's `UsenetClientBase` and `NzbValidationService`,
//! and lenient readers for the loosely typed JSON SABnzbd and NZBGet return.

use serde_json::{Map, Value};

use crate::error::DownloadError;

/// `NzbValidationService.Validate`: the file must be XML whose root is `nzb` with at least one
/// `file` element. An indexer error document (`<error code=".." description=".."/>`) is
/// reported as such.
pub fn validate_nzb(name: &str, content: &[u8]) -> Result<(), DownloadError> {
    use quick_xml::events::Event;
    let invalid = |m: String| DownloadError::Rejected(format!("Invalid NZB: {m} [{name}]"));
    let mut reader = quick_xml::Reader::from_reader(content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut root: Option<String> = None;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| invalid(format!("Unable to parse ({e})")))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(event, Event::Empty(_));
                let local = e.local_name().as_ref().to_string();
                if depth == 0 {
                    if local == "error" {
                        let attr = |k: &str| {
                            e.attributes()
                                .flatten()
                                .find(|a| a.key.local_name().as_ref() == k)
                                .map(|a| a.value.to_string())
                        };
                        if let (Some(code), Some(description)) = (attr("code"), attr("description"))
                        {
                            return Err(DownloadError::Rejected(format!(
                                "Invalid NZB: Contains indexer error: {code} - {description}"
                            )));
                        }
                    }
                    if local != "nzb" {
                        return Err(invalid(format!(
                            "Unexpected root element. Expected 'nzb' found '{local}'"
                        )));
                    }
                    root = Some(local);
                } else if depth == 1 && local == "file" {
                    return Ok(());
                }
                // Empty elements have no end event and hold nothing.
                if !is_empty {
                    depth += 1;
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    match root {
        None => Err(invalid("No Root element".into())),
        Some(_) => Err(invalid("No files".into())),
    }
}

pub(crate) fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(crate) fn port(map: &Map<String, Value>, default: u16) -> Result<u16, String> {
    match map.get("port") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p > 0)
            .ok_or_else(|| "the port must be between 1 and 65535".to_string()),
    }
}

pub(crate) fn host(map: &Map<String, Value>) -> Result<String, String> {
    let host = text(map, "host").unwrap_or_else(|| "localhost".into());
    if host.contains('/') || host.contains(' ') {
        return Err("the host must be a host name or address, without a scheme or path".into());
    }
    Ok(host)
}

/// `HttpRequestBuilder.BuildBaseUrl` plus a relative path.
pub(crate) fn base_url(use_ssl: bool, host: &str, port: u16, url_base: &str, path: &str) -> String {
    let scheme = if use_ssl { "https" } else { "http" };
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let base = url_base.trim_matches('/');
    let mut url = format!("{scheme}://{host}:{port}/");
    if !base.is_empty() {
        url.push_str(base);
        url.push('/');
    }
    url.push_str(path.trim_start_matches('/'));
    url
}

/// A number that may arrive as a JSON number or a string.
pub(crate) fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

/// A boolean that may arrive as `true`, `1` or `"1"`.
pub(crate) fn flag(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        Some(Value::String(s)) => matches!(s.trim(), "1" | "true" | "True"),
        _ => false,
    }
}

pub(crate) fn string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// A list of strings that may arrive as one string (`SabnzbdStringArrayConverter`).
pub(crate) fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().map(|x| string(Some(x))).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nzbs_are_validated_as_upstream() {
        let ok = br#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE nzb PUBLIC "-//newzBin//DTD NZB 1.1//EN" "http://www.newzbin.com/DTD/nzb/nzb-1.1.dtd">
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <head><meta type="name">Show</meta></head>
  <file poster="a@b" date="1" subject="s"><groups><group>a.b</group></groups><segments/></file>
</nzb>"#;
        assert_eq!(validate_nzb("a.nzb", ok), Ok(()));
        let empty = br#"<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><head/></nzb>"#;
        assert!(
            validate_nzb("a.nzb", empty)
                .unwrap_err()
                .to_string()
                .contains("No files")
        );
        let error = br#"<?xml version="1.0"?><error code="300" description="Item not found"/>"#;
        assert_eq!(
            validate_nzb("a.nzb", error).unwrap_err().to_string(),
            "Invalid NZB: Contains indexer error: 300 - Item not found"
        );
        let html = b"<html><body>login</body></html>";
        assert!(
            validate_nzb("a.nzb", html)
                .unwrap_err()
                .to_string()
                .contains("Expected 'nzb' found 'html'")
        );
        assert!(validate_nzb("a.nzb", b"not xml at all").is_err());
        // A nested file element is not a file of the NZB.
        let nested = br#"<nzb><head><file/></head></nzb>"#;
        assert!(validate_nzb("a.nzb", nested).is_err());
    }

    #[test]
    fn lenient_values() {
        assert_eq!(num(Some(&Value::String("12.5".into()))), 12.5);
        assert!(flag(Some(&serde_json::json!(1))));
        assert!(!flag(Some(&serde_json::json!("0"))));
        assert_eq!(strings(Some(&serde_json::json!("tv"))), vec!["tv"]);
        assert_eq!(
            base_url(false, "::1", 8080, "/sab/", "api"),
            "http://[::1]:8080/sab/api"
        );
        assert_eq!(base_url(true, "h", 1, "", "api"), "https://h:1/api");
    }
}
