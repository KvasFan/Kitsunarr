//! Locates the compiled Dioxus web application to embed.
//!
//! Order: `KITSUNARR_WEB_DIST` (absolute path), then the default `dx` release output, then an
//! empty placeholder (the server then serves a notice page and the API only).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=KITSUNARR_WEB_DIST");
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let default_dist = workspace.join("target/dx/kitsunarr-web/release/web/public");

    let dist = match std::env::var("KITSUNARR_WEB_DIST") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => default_dist.clone(),
    };
    let resolved = if dist.join("index.html").is_file() {
        println!("cargo:rerun-if-changed={}", dist.display());
        dist
    } else {
        println!(
            "cargo:warning=web UI not found at {}; building without it (run `dx build -p kitsunarr-web --release` first)",
            dist.display()
        );
        let placeholder =
            PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("web-placeholder");
        std::fs::create_dir_all(&placeholder).expect("create placeholder dir");
        placeholder
    };
    println!(
        "cargo:rustc-env=KITSUNARR_WEB_DIST_RESOLVED={}",
        resolved.display()
    );
    let revision = std::env::var("KITSUNARR_GIT_REVISION").unwrap_or_default();
    println!("cargo:rerun-if-env-changed=KITSUNARR_GIT_REVISION");
    println!("cargo:rustc-env=KITSUNARR_GIT_REVISION={revision}");
}
