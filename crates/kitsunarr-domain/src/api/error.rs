use serde::{Deserialize, Serialize};

/// Error body returned by every failing API call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorBody {
    /// Stable machine-readable code, e.g. `not_found`, `validation_failed`, `unauthorized`.
    pub code: String,
    /// Human readable message (safe to display).
    pub message: String,
    /// Field-level validation errors, if any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FieldError {
    pub field: String,
    pub message: String,
}
