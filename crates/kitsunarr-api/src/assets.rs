//! Serving the compiled web application.
//!
//! The server binary embeds the Dioxus build output and exposes it through [`AssetSource`].
//! Unknown non-API paths fall back to `index.html` so client-side routes deep-link correctly.

use std::borrow::Cow;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::error::ApiError;

pub struct EmbeddedAsset {
    pub data: Cow<'static, [u8]>,
    pub mime: String,
    /// Strong validator (content hash).
    pub etag: String,
}

pub trait AssetSource: Send + Sync + 'static {
    fn get(&self, path: &str) -> Option<EmbeddedAsset>;
}

/// Used in tests and API-only deployments.
pub struct NoAssets;

impl AssetSource for NoAssets {
    fn get(&self, _path: &str) -> Option<EmbeddedAsset> {
        None
    }
}

const URL_BASE_META: &str = "kitsunarr-url-base";

/// Injects the runtime URL base and rewrites absolute asset links when served below a prefix.
pub fn render_index(html: &str, url_base: &str) -> String {
    let meta = format!(r#"<meta name="{URL_BASE_META}" content="{url_base}">"#);
    let mut out = match html.find("<head>") {
        Some(i) => format!("{}<head>{meta}{}", &html[..i], &html[i + "<head>".len()..]),
        None => format!("{meta}{html}"),
    };
    if !url_base.is_empty() {
        for attr in [
            "src=\"/",
            "href=\"/",
            "from \"/",
            "import(\"/",
            "init(\"/",
            "url(\"/",
            "\"/assets/",
            "'/assets/",
        ] {
            let quote_and_slash = &attr[attr.len() - 2..];
            let prefix = &attr[..attr.len() - 2];
            let replacement = format!("{prefix}{}{url_base}/", &quote_and_slash[..1]);
            out = out.replace(attr, &replacement);
        }
        // Undo accidental double prefixes for links that already contained the base.
        out = out.replace(&format!("{url_base}{url_base}/"), &format!("{url_base}/"));
    }
    out
}

fn is_hashed_asset(path: &str) -> bool {
    // dx emits content-hashed file names under /assets (e.g. `app-dxh1a2b3c.js`).
    path.starts_with("assets/") && path.contains("dxh")
}

pub async fn serve(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return ApiError::NotFound(format!("No API endpoint at /{path}.")).into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let (asset, is_index) = match state.assets.get(path).filter(|_| !path.is_empty()) {
        Some(asset) => (asset, false),
        None => {
            // Paths with a file extension are real files: do not mask a missing asset.
            let last = path.rsplit('/').next().unwrap_or_default();
            if last.contains('.') && !last.ends_with(".html") {
                return StatusCode::NOT_FOUND.into_response();
            }
            match state.assets.get("index.html") {
                Some(index) => (index, true),
                None => return fallback_page().into_response(),
            }
        }
    };

    if is_index {
        let html = render_index(
            &String::from_utf8_lossy(&asset.data),
            &state.config.server.url_base,
        );
        return (
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            ],
            html,
        )
            .into_response();
    }

    let etag = format!("\"{}\"", asset.etag);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, etag)
            .body(Body::empty())
            .unwrap_or_default();
    }
    let cache = if is_hashed_asset(path) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, asset.mime)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, cache)
        .body(if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(asset.data.into_owned())
        })
        .unwrap_or_default()
}

fn fallback_page() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Kitsunarr</title></head><body>\
         <h1>Kitsunarr</h1><p>The web interface was not included in this build. The API is available under \
         <code>/api/v1</code>. Build the frontend with <code>dx build -p kitsunarr-web --release</code> and rebuild the \
         server.</p></body></html>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_rendering_injects_base() {
        let html = r#"<html><head><title>K</title><script type="module">import init from "/assets/app-dxh12.js"; init("/assets/app_bg-dxh12.wasm");</script><link rel="stylesheet" href="/assets/style.css"></head></html>"#;
        let root = render_index(html, "");
        assert!(root.contains(r#"<meta name="kitsunarr-url-base" content="">"#));
        assert!(root.contains("\"/assets/app-dxh12.js\""));
        let nested = render_index(html, "/kitsunarr");
        assert!(
            nested.contains("from \"/kitsunarr/assets/app-dxh12.js\""),
            "{nested}"
        );
        assert!(
            nested.contains("init(\"/kitsunarr/assets/app_bg-dxh12.wasm\")"),
            "{nested}"
        );
        assert!(
            nested.contains("href=\"/kitsunarr/assets/style.css\""),
            "{nested}"
        );
        assert!(!nested.contains("/kitsunarr/kitsunarr/"));
    }
}
