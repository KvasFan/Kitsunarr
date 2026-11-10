//! REST API contract shared by the server (Axum) and the browser client (Dioxus).
//!
//! All payloads use camelCase JSON. Types derive `utoipa::ToSchema` when the `openapi`
//! feature is enabled so the server can publish an OpenAPI document generated from the
//! exact types the browser deserialises.

pub mod auth;
pub mod error;
pub mod events;
pub mod library;
pub mod metadata;
pub mod migration;
pub mod queue;
pub mod search;
pub mod settings;
pub mod system;

/// Version prefix of the native API.
pub const API_PREFIX: &str = "/api/v1";
/// Header carrying an API key (Sonarr-compatible name).
pub const API_KEY_HEADER: &str = "X-Api-Key";
/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "kitsunarr_session";
