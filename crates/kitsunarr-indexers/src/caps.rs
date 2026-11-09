//! Indexer capabilities (`NewznabCapabilities.cs`, `NewznabCapabilitiesProvider.cs`).

use serde::Serialize;

use crate::error::IndexerError;
use crate::feed::check_error;
use crate::xml;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: i32,
    pub name: String,
    pub description: String,
    pub subcategories: Vec<Category>,
}

/// What an indexer supports. `None` parameter lists mean the search mode is unavailable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub default_page_size: i32,
    pub max_page_size: i32,
    pub search_params: Option<Vec<String>>,
    pub tv_search_params: Option<Vec<String>>,
    pub supports_aggregate_id_search: bool,
    pub text_search_engine: String,
    pub tv_text_search_engine: String,
    pub categories: Vec<Category>,
}

impl Default for Capabilities {
    /// Upstream's defaults, used when an indexer's capabilities cannot be read.
    fn default() -> Self {
        Self {
            default_page_size: 100,
            max_page_size: 100,
            search_params: Some(vec!["q".into()]),
            tv_search_params: Some(vec!["q".into(), "rid".into(), "season".into(), "ep".into()]),
            supports_aggregate_id_search: false,
            text_search_engine: "sphinx".into(),
            tv_text_search_engine: "sphinx".into(),
            categories: Vec::new(),
        }
    }
}

fn has(list: &Option<Vec<String>>, param: &str) -> bool {
    list.as_ref().is_some_and(|l| l.iter().any(|p| p == param))
}

impl Capabilities {
    /// `Math.Clamp(DefaultPageSize, Math.Min(100, MaxPageSize), MaxPageSize)`.
    pub fn page_size(&self) -> i32 {
        let low = self.max_page_size.min(100);
        self.default_page_size
            .max(low)
            .min(self.max_page_size.max(low))
    }

    pub fn supports_search(&self) -> bool {
        has(&self.search_params, "q")
    }
    pub fn supports_tv_query_search(&self) -> bool {
        has(&self.tv_search_params, "q")
    }
    pub fn supports_tv_title_search(&self) -> bool {
        has(&self.tv_search_params, "title")
    }
    pub fn supports_tv_text_searches(&self) -> bool {
        self.supports_tv_query_search() || self.supports_tv_title_search()
    }
    pub fn supports_tv_param(&self, param: &str) -> bool {
        has(&self.tv_search_params, param)
    }
    pub fn supports_tv_id_searches(&self) -> bool {
        ["tvdbid", "imdbid", "rid", "tvmazeid", "tmdbid"]
            .iter()
            .any(|p| self.supports_tv_param(p))
    }
    pub fn supports_season_search(&self) -> bool {
        self.supports_tv_param("season")
    }
    pub fn supports_episode_search(&self) -> bool {
        self.supports_tv_param("season") && self.supports_tv_param("ep")
    }

    /// `TestCapabilities`: usable for TV when free-text search works, or TV search with an
    /// identifier plus season and episode.
    pub fn usable_for_tv(&self) -> bool {
        self.supports_search()
            || (["q", "tvdbid", "rid"]
                .iter()
                .any(|p| self.supports_tv_param(p))
                && self.supports_episode_search())
    }
}

fn int_attr(e: &xml::Element, name: &str) -> Result<i32, IndexerError> {
    e.attr(name)
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| IndexerError::Unexpected(format!("caps: missing or invalid {name}")))
}

fn categories(e: &xml::Element, child: &str) -> Result<Vec<Category>, IndexerError> {
    e.children_named(child)
        .map(|c| {
            Ok(Category {
                id: int_attr(c, "id")?,
                name: c.attr("name").unwrap_or_default().to_string(),
                description: c.attr("description").unwrap_or_default().to_string(),
                subcategories: if child == "category" {
                    categories(c, "subcat")?
                } else {
                    Vec::new()
                },
            })
        })
        .collect()
}

/// An available search mode: its parameter list (when declared) and search engine.
type SearchMode = (Option<Vec<String>>, Option<String>);

fn search_mode(searching: &xml::Element, name: &str) -> Result<Option<SearchMode>, IndexerError> {
    let Some(e) = searching.child(name) else {
        return Ok(None);
    };
    let available = e
        .attr("available")
        .ok_or_else(|| IndexerError::Unexpected(format!("caps: {name} lacks 'available'")))?;
    if available != "yes" {
        return Ok(None);
    }
    let params = e
        .attr("supportedParams")
        .map(|p| p.split(',').map(str::to_string).collect());
    Ok(Some((params, e.attr("searchEngine").map(str::to_string))))
}

/// Parses a `t=caps` response.
pub fn parse_capabilities(body: &str) -> Result<Capabilities, IndexerError> {
    let doc = xml::parse(body).map_err(IndexerError::InvalidXml)?;
    check_error(&doc, true, true)?;
    if doc.name != "caps" {
        return Err(IndexerError::Unexpected(
            "not a capabilities document".into(),
        ));
    }
    let mut caps = Capabilities::default();
    if let Some(limits) = doc.child("limits") {
        caps.default_page_size = int_attr(limits, "default")?;
        caps.max_page_size = int_attr(limits, "max")?;
    }
    if let Some(searching) = doc.child("searching") {
        match search_mode(searching, "search")? {
            None => caps.search_params = None,
            Some((params, engine)) => {
                if let Some(p) = params {
                    caps.search_params = Some(p);
                }
                if let Some(e) = engine {
                    caps.text_search_engine = e;
                }
            }
        }
        match search_mode(searching, "tv-search")? {
            None => caps.tv_search_params = None,
            Some((params, engine)) => {
                if let Some(p) = params {
                    caps.tv_search_params = Some(p);
                    caps.supports_aggregate_id_search = true;
                }
                if let Some(e) = engine {
                    caps.tv_text_search_engine = e;
                }
            }
        }
    }
    if let Some(cats) = doc.child("categories") {
        caps.categories = categories(cats, "category")?;
    }
    Ok(caps)
}

/// `NewznabCapabilitiesProvider`: malformed XML and API key errors are errors; any other
/// problem with the document falls back to the defaults (with a warning), as upstream does.
pub fn capabilities_or_default(body: &str) -> Result<Capabilities, IndexerError> {
    match parse_capabilities(body) {
        Ok(caps) => Ok(caps),
        Err(e @ (IndexerError::InvalidXml(_) | IndexerError::ApiKey(_))) => Err(e),
        Err(e) => {
            tracing::warn!(error = %e, "unable to read indexer capabilities; using the defaults");
            Ok(Capabilities::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_size_follows_upstream_clamp() {
        let size = |default, max| {
            Capabilities {
                default_page_size: default,
                max_page_size: max,
                ..Default::default()
            }
            .page_size()
        };
        // NewznabFixture's three page-size tests.
        assert_eq!(size(25, 30), 30);
        assert_eq!(size(25, 250), 100);
        assert_eq!(size(200, 250), 200);
        assert_eq!(Capabilities::default().page_size(), 100);
    }
}
