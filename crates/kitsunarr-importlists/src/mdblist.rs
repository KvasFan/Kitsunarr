//! MDBList (`MDBListImport`, Tanukarr): the shows of the included list URLs minus those of the
//! excluded ones, read completely through the MDBList API (offset or cursor pagination).

use serde_json::{Map, Value};

use crate::{
    ListError, ListItem, check_status, compose, connection_error, imdb, positive, strings, text,
};

const MAX_PAGES: usize = 1000;
const PAGE: usize = 250;

/// `MDBListSettings.GetListPath`: `https://mdblist.com/lists/{user}/{list}` → API path.
pub fn list_path(url: &str) -> Option<String> {
    let u = url::Url::parse(url.trim()).ok()?;
    if u.scheme() != "https"
        || u.port().is_some()
        || !u.username().is_empty()
        || u.query().is_some()
        || u.fragment().is_some()
        || !matches!(u.host_str(), Some("mdblist.com" | "www.mdblist.com"))
    {
        return None;
    }
    let re = regex::Regex::new(r"^/lists/([a-zA-Z0-9_-]+)/([a-zA-Z0-9_-]+)/?$").ok()?;
    let c = re.captures(u.path())?;
    Some(format!("lists/{}/{}/items/show", &c[1], &c[2]))
}

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let key = text(s, "apiKey").ok_or(ListError::Settings("The API key is required".into()))?;
    let mut included = strings(s, "includedListUrls");
    if let Some(legacy) = text(s, "listUrl")
        && !included.contains(&legacy)
    {
        included.push(legacy);
    }
    if included.is_empty() {
        return Err(ListError::Settings(
            "Add at least one included MDBList URL".into(),
        ));
    }
    let excluded = strings(s, "excludedListUrls");
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.mdblist.com".into());
    let mut inc = Vec::new();
    for u in &included {
        inc.push(read_list(http, &base, &key, u).await?);
    }
    let mut exc = Vec::new();
    for u in &excluded {
        exc.push(read_list(http, &base, &key, u).await?);
    }
    compose(inc, exc)
}

async fn read_list(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    url: &str,
) -> Result<Vec<ListItem>, ListError> {
    let path = list_path(url).ok_or_else(|| {
        ListError::Settings(format!(
            "{url} is not an MDBList URL such as https://mdblist.com/lists/username/list-name"
        ))
    })?;
    let invalid = |m: &str| ListError::Invalid(format!("MDBList {m}; the previous list is kept"));
    let mut out = Vec::new();
    let mut cursors = std::collections::HashSet::new();
    let (mut cursor, mut offset): (Option<String>, usize) = (None, 0);
    let mut raw = 0usize;
    let mut expected: Option<usize> = None;
    for _ in 0..MAX_PAGES {
        let mut req = http
            .get(format!("{}/{path}", base.trim_end_matches('/')))
            .query(&[("apikey", key), ("limit", &PAGE.to_string())]);
        req = match &cursor {
            Some(c) => req.query(&[("cursor", c)]),
            None => req.query(&[("offset", offset)]),
        };
        let resp = req.send().await.map_err(connection_error)?;
        check_status(&resp)?;
        let header = |n: &str| {
            resp.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let total = header("X-Total-Items");
        let has_more_header = header("X-Has-More");
        let doc: Value = resp
            .json()
            .await
            .map_err(|_| invalid("did not answer JSON"))?;
        let rows = doc
            .as_array()
            .or_else(|| doc["shows"].as_array())
            .or_else(|| doc["items"].as_array())
            .ok_or_else(|| invalid("did not return a show list"))?;
        raw += rows.len();
        for r in rows {
            let kind = r["mediatype"].as_str().or(r["type"].as_str());
            if kind.is_some_and(|t| t != "show" && t != "tv") {
                continue;
            }
            let ids = &r["ids"];
            out.push(ListItem {
                title: r["title"]
                    .as_str()
                    .or(r["name"].as_str())
                    .unwrap_or_default()
                    .to_string(),
                tmdb_id: [&ids["tmdb"], &r["tmdb_id"], &r["tmdbid"], &r["id"]]
                    .into_iter()
                    .find(|v| !v.is_null())
                    .and_then(positive),
                tvdb_id: [&ids["tvdb"], &r["tvdb_id"]]
                    .into_iter()
                    .find(|v| !v.is_null())
                    .and_then(positive),
                imdb_id: imdb(
                    ids["imdb"]
                        .as_str()
                        .or(r["imdb_id"].as_str())
                        .or(r["imdbid"].as_str()),
                ),
                ..Default::default()
            });
        }
        if let Some(t) = total {
            let t: usize = t
                .parse()
                .map_err(|_| invalid("returned inconsistent item counts"))?;
            if expected.is_some_and(|e| e != t) {
                return Err(invalid("returned inconsistent item counts"));
            }
            expected = Some(t);
        }
        let pagination = doc.get("pagination").filter(|p| p.is_object());
        let next = pagination
            .and_then(|p| p.get("next_cursor"))
            .or_else(|| doc.get("next_cursor"))
            .filter(|v| !v.is_null());
        let next_cursor = next.and_then(Value::as_str).filter(|c| !c.is_empty());
        let mut has_more = pagination.and_then(|p| p["has_more"].as_bool());
        if let Some(h) = has_more_header {
            let h: bool = h
                .parse()
                .map_err(|_| invalid("returned inconsistent pagination headers"))?;
            if has_more.is_some_and(|m| m != h) {
                return Err(invalid("returned inconsistent pagination headers"));
            }
            has_more = Some(h);
        }
        if let Some(c) = next_cursor {
            if has_more == Some(false)
                || c.len() > 4096
                || !cursors.insert(c.to_string())
                || rows.is_empty()
            {
                return Err(invalid("returned inconsistent or repeated pagination"));
            }
            cursor = Some(c.to_string());
            continue;
        }
        if cursor.is_some() && has_more == Some(true) {
            return Err(invalid("omitted the cursor needed to complete the list"));
        }
        if has_more == Some(false) || next.is_some() || rows.is_empty() {
            if has_more == Some(true) || expected.is_some_and(|e| e != raw) {
                return Err(invalid("returned an incomplete list"));
            }
            return Ok(out);
        }
        cursor = None;
        offset = raw;
    }
    Err(invalid("pagination could not be completed"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_urls() {
        assert_eq!(
            super::list_path("https://mdblist.com/lists/garycrawfordgc/latest-tv-shows"),
            Some("lists/garycrawfordgc/latest-tv-shows/items/show".into())
        );
        assert_eq!(super::list_path("http://mdblist.com/lists/a/b"), None);
        assert_eq!(super::list_path("https://example.com/lists/a/b"), None);
        assert_eq!(super::list_path("https://mdblist.com/lists/a/b?x=1"), None);
    }
}
