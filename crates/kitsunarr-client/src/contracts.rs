//! Resources of native endpoints served by the Sonarr-compatible backend work (custom filters,
//! "Test all"): the client and web UI are written against these shapes, which follow Sonarr's
//! `CustomFilterResource` and `ProviderTestAllResult`.

use serde::{Deserialize, Serialize};

/// Sonarr's `CustomFilterResource`: a saved filter of a list (`type` is the list, such as
/// `series`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomFilter {
    #[serde(default)]
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub filters: Vec<CustomFilterCondition>,
}

/// One condition of a saved filter (`{key, value, type}`), as Sonarr's frontend stores it:
/// `value` is a scalar or a list, `type` the comparison (`equal`, `contains`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomFilterCondition {
    pub key: String,
    pub value: serde_json::Value,
    #[serde(rename = "type")]
    pub kind: String,
}

/// Sonarr's `ProviderTestAllResult`: one provider's test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestAllResult {
    pub id: i64,
    pub is_valid: bool,
    #[serde(default)]
    pub validation_failures: Vec<ProviderValidationFailure>,
}

/// A validation failure of a provider test (`propertyName`, `errorMessage`, `severity`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderValidationFailure {
    #[serde(default)]
    pub property_name: String,
    pub error_message: String,
    /// `error` or `warning`.
    #[serde(default)]
    pub severity: serde_json::Value,
}

impl ProviderValidationFailure {
    /// Sonarr sends the severity as a name (`error`, `warning`) or a number (0 error,
    /// 1 warning).
    pub fn is_warning(&self) -> bool {
        match &self.severity {
            serde_json::Value::String(s) => s.eq_ignore_ascii_case("warning"),
            serde_json::Value::Number(n) => n.as_i64() == Some(1),
            _ => false,
        }
    }
}
