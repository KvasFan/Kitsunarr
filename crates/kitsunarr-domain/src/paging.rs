//! Server-side pagination contract.

use serde::{Deserialize, Serialize};

pub const DEFAULT_PAGE_SIZE: u32 = 50;
pub const MAX_PAGE_SIZE: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

/// Query parameters accepted by every paginated list endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
#[serde(rename_all = "camelCase")]
pub struct PageRequest {
    /// 1-based page number.
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub sort_key: Option<String>,
    pub sort_direction: Option<SortDirection>,
    /// Free-text filter (title search).
    pub search: Option<String>,
}

impl PageRequest {
    pub fn page(&self) -> u32 {
        self.page.unwrap_or(1).max(1)
    }

    pub fn page_size(&self) -> u32 {
        self.page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE)
    }

    pub fn offset(&self) -> u64 {
        u64::from(self.page() - 1) * u64::from(self.page_size())
    }

    pub fn direction(&self) -> SortDirection {
        self.sort_direction.unwrap_or_default()
    }

    /// Trimmed, non-empty search term.
    pub fn search_term(&self) -> Option<&str> {
        self.search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub page: u32,
    pub page_size: u32,
    pub total_records: u64,
    pub records: Vec<T>,
}

impl<T> Page<T> {
    pub fn total_pages(&self) -> u64 {
        if self.page_size == 0 {
            return 0;
        }
        self.total_records.div_ceil(u64::from(self.page_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_request_clamps() {
        let r = PageRequest {
            page: Some(0),
            page_size: Some(10_000),
            ..Default::default()
        };
        assert_eq!(r.page(), 1);
        assert_eq!(r.page_size(), MAX_PAGE_SIZE);
        let r = PageRequest {
            page: Some(3),
            page_size: Some(20),
            ..Default::default()
        };
        assert_eq!(r.offset(), 40);
        let p: Page<()> = Page {
            page: 1,
            page_size: 20,
            total_records: 41,
            records: vec![],
        };
        assert_eq!(p.total_pages(), 3);
    }
}
