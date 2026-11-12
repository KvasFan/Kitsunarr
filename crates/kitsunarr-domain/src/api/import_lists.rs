//! Import lists: sync, membership preview, status and exclusions.

use serde::{Deserialize, Serialize};

/// A list's sync state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ImportListStatusItem {
    pub import_list_id: i64,
    /// The last complete read.
    pub last_info_sync: Option<String>,
    /// Reads are paused until then after failures.
    pub disabled_till: Option<String>,
    pub last_error: Option<String>,
    /// Shows at the last complete read.
    pub item_count: i64,
}

/// A show a list contains (Tanukarr's import list preview).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ImportListEntry {
    pub import_list_id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub mal_id: Option<i64>,
    pub ani_list_id: Option<i64>,
    /// The library series it matches.
    pub series_id: Option<i64>,
    /// An exclusion keeps it out of the library.
    pub excluded: bool,
}

/// Sync every list (or one) now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ImportListSyncRequest {
    pub import_list_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ImportListSyncResult {
    pub message: String,
}

/// A title import lists never add (Sonarr's import list exclusion).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ImportListExclusionItem {
    #[serde(default)]
    pub id: i64,
    /// Catalogue of the identifier: `tvdb`, `tmdb`, `aniList` or `myAnimeList`.
    pub provider: String,
    pub external_id: String,
    #[serde(default)]
    pub title: Option<String>,
}
