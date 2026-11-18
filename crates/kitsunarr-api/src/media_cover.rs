//! Sonarr's media cover cache (`MediaCoverService`): a title's poster, fanart and banner are
//! downloaded once into `<data dir>/MediaCover/{id}/`, and the smaller sizes Sonarr serves
//! (`poster-500`, `poster-250`, `fanart-360`, `fanart-180`, `banner-70`, `banner-35`; the number
//! is the height) are made from that copy. A cached image is fetched again when the catalogue's
//! address for it changes.

use std::path::{Path, PathBuf};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

/// Sonarr's resized heights per cover type.
const HEIGHTS: &[(&str, &[u32])] = &[
    ("poster", &[500, 250]),
    ("fanart", &[360, 180]),
    ("banner", &[70, 35]),
];

/// A requested cover file: `poster.jpg`, `poster-250.jpg`, `fanart-360.jpg`...
#[derive(Debug, PartialEq, Eq)]
pub struct CoverRequest {
    pub cover_type: String,
    pub height: Option<u32>,
}

impl CoverRequest {
    pub fn parse(filename: &str) -> Option<Self> {
        let stem = filename.rsplit_once('.').map_or(filename, |(s, _)| s);
        let (cover_type, height) = match stem.split_once('-') {
            Some((t, h)) => (t, Some(h.parse::<u32>().ok()?)),
            None => (stem, None),
        };
        let cover_type = cover_type.to_ascii_lowercase();
        let sizes = HEIGHTS
            .iter()
            .find(|(t, _)| *t == cover_type)
            .map(|(_, s)| *s);
        match (sizes, height) {
            (Some(s), Some(h)) if s.contains(&h) => {}
            (Some(_), None) => {}
            // Other cover types (clearlogo, screenshot) are served at their original size.
            (None, None) => {}
            _ => return None,
        }
        Some(Self { cover_type, height })
    }
}

fn folder(state: &AppState, id: i64, movie: bool) -> PathBuf {
    let base = state.config.data_dir.join("MediaCover");
    if movie {
        base.join(format!("movie-{id}"))
    } else {
        base.join(id.to_string())
    }
}

/// The cached file for a cover (fetching or resizing it first when needed), and its content type.
pub async fn cover_file(
    state: &AppState,
    id: i64,
    movie: bool,
    remote_url: &str,
    req: &CoverRequest,
) -> ApiResult<(PathBuf, &'static str)> {
    let dir = folder(state, id, movie);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(ApiError::internal)?;
    let original = dir.join(format!("{}.img", req.cover_type));
    let source = dir.join(format!("{}.url", req.cover_type));
    let current = tokio::fs::read_to_string(&source).await.unwrap_or_default();
    if current != remote_url || !original.exists() {
        let bytes = state
            .http
            .get(remote_url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| ApiError::BadGateway(format!("The image could not be downloaded: {e}")))?
            .bytes()
            .await
            .map_err(|e| ApiError::BadGateway(format!("The image could not be downloaded: {e}")))?;
        write_atomic(&original, &bytes).await?;
        tokio::fs::write(&source, remote_url)
            .await
            .map_err(ApiError::internal)?;
        // Sizes made from the previous image are stale.
        for (t, heights) in HEIGHTS {
            if *t == req.cover_type {
                for h in *heights {
                    let _ = tokio::fs::remove_file(dir.join(format!("{t}-{h}.jpg"))).await;
                }
            }
        }
    }
    let Some(height) = req.height else {
        let bytes = tokio::fs::read(&original)
            .await
            .map_err(ApiError::internal)?;
        return Ok((original, content_type(&bytes)));
    };
    let resized = dir.join(format!("{}-{height}.jpg", req.cover_type));
    if !resized.exists() {
        let input = original.clone();
        let output = resized.clone();
        tokio::task::spawn_blocking(move || resize(&input, &output, height))
            .await
            .map_err(ApiError::internal)??;
    }
    Ok((resized, "image/jpeg"))
}

fn content_type(bytes: &[u8]) -> &'static str {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::WebP) => "image/webp",
        _ => "image/jpeg",
    }
}

/// `ImageResizer.Resize`: the image scaled to the height, keeping its proportions, as JPEG.
fn resize(input: &Path, output: &Path, height: u32) -> ApiResult<()> {
    let img = image::ImageReader::open(input)
        .and_then(|r| r.with_guessed_format())
        .map_err(ApiError::internal)?
        .decode()
        .map_err(|e| ApiError::BadGateway(format!("The image cannot be read: {e}")))?;
    let scaled = if img.height() > height {
        img.resize(u32::MAX, height, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let tmp = output.with_extension("tmp");
    scaled
        .to_rgb8()
        .save_with_format(&tmp, image::ImageFormat::Jpeg)
        .map_err(ApiError::internal)?;
    std::fs::rename(&tmp, output).map_err(ApiError::internal)?;
    Ok(())
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> ApiResult<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes)
        .await
        .map_err(ApiError::internal)?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(ApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_names_follow_sonarr() {
        assert_eq!(
            CoverRequest::parse("poster-250.jpg"),
            Some(CoverRequest {
                cover_type: "poster".into(),
                height: Some(250)
            })
        );
        assert_eq!(CoverRequest::parse("fanart.jpg").unwrap().height, None);
        assert!(CoverRequest::parse("poster-123.jpg").is_none());
        assert!(CoverRequest::parse("banner-x.jpg").is_none());
        assert_eq!(
            CoverRequest::parse("clearlogo.png").unwrap().cover_type,
            "clearlogo"
        );
    }

    #[test]
    fn resizing_keeps_proportions() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("poster.img");
        image::RgbImage::from_pixel(680, 1000, image::Rgb([200, 10, 10]))
            .save_with_format(&input, image::ImageFormat::Png)
            .unwrap();
        let output = dir.path().join("poster-250.jpg");
        resize(&input, &output, 250).unwrap();
        let out = image::open(&output).unwrap();
        assert_eq!((out.width(), out.height()), (170, 250));
    }
}
