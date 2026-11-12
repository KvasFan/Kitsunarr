//! Kitsunarr shared domain model.
//!
//! This crate is deliberately free of I/O so it compiles both for the server and for the
//! browser (WebAssembly). It holds:
//!
//! * catalogues that must stay stable across releases and migrations (qualities, languages),
//! * domain enums shared by every subsystem (media kinds, metadata providers, monitoring),
//! * the typed REST contract (`api`) used by the Axum server and the Dioxus client.

pub mod api;
pub mod codec;
pub mod language;
pub mod media;
pub mod naming;
pub mod paging;
pub mod quality;
pub mod text;

pub use language::Language;
pub use media::*;
pub use paging::{Page, PageRequest, SortDirection};
pub use quality::{Quality, QualitySource};
