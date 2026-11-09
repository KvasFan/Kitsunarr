//! Web UI assets embedded at compile time (see `build.rs`).

use kitsunarr_api::{AssetSource, EmbeddedAsset};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$KITSUNARR_WEB_DIST_RESOLVED"]
struct WebAssets;

pub struct Embedded;

impl AssetSource for Embedded {
    fn get(&self, path: &str) -> Option<EmbeddedAsset> {
        let file = WebAssets::get(path)?;
        Some(EmbeddedAsset {
            mime: file.metadata.mimetype().to_string(),
            etag: hex::encode(&file.metadata.sha256_hash()[..16]),
            data: file.data,
        })
    }
}

impl Embedded {
    pub fn has_ui() -> bool {
        WebAssets::get("index.html").is_some()
    }
}
