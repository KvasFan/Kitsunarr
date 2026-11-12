//! `VideoFileInfoReader`: media information from ffprobe.
//!
//! `ffprobe` (or `KITSUNARR_FFPROBE`) is run with Sonarr's arguments: a 50 MB probe, again with
//! 150 MB and a longer analysis when the first audio stream's channel layout is unknown, and the
//! first video frame's side data when the transfer function is PQ (to tell HDR10, HDR10+ and
//! Dolby Vision apart). Without ffprobe, or for a file without a video stream, there is no
//! media information. Unlike Sonarr, the raw ffprobe output is not stored.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::{AudioStream, CURRENT_SCHEMA_REVISION, HdrFormat, MediaInfo, SubtitleStream};

const HLG_TRANSFER: &[&str] = &["arib-std-b67"];
const PQ_TRANSFER: &[&str] = &["smpte2084"];
const HDR_PRIMARIES: &[&str] = &["bt2020"];
/// `FFMpegCoreSideDataTypes`.
const DOVI_CONFIGURATION: &str = "DOVI configuration record";
const HDR10_PLUS: &str = "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)";
const MASTERING_DISPLAY: &str = "Mastering display metadata";
const CONTENT_LIGHT_LEVEL: &str = "Content light level metadata";
/// `MediaFileExtensions.DiskExtensions` and `StreamingExtensions`: not probed.
const NOT_PROBED: &[&str] = &[".img", ".iso", ".vob", ".strm"];
const TIMEOUT: Duration = Duration::from_secs(60);

fn binary() -> String {
    std::env::var("KITSUNARR_FFPROBE")
        .ok()
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| "ffprobe".into())
}

/// Whether ffprobe can be run.
pub async fn available() -> bool {
    let run = tokio::process::Command::new(binary())
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status();
    matches!(tokio::time::timeout(Duration::from_secs(10), run).await, Ok(Ok(s)) if s.success())
}

async fn ffprobe(args: &[&str], path: &Path) -> Option<Value> {
    let run = tokio::process::Command::new(binary())
        .args(["-v", "quiet", "-print_format", "json"])
        .args(args)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(TIMEOUT, run).await {
        Ok(Ok(out)) if out.status.success() => serde_json::from_slice(&out.stdout).ok(),
        Ok(Ok(_)) => None,
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "ffprobe could not be run");
            None
        }
        Err(_) => {
            tracing::warn!(path = %path.display(), "ffprobe timed out");
            None
        }
    }
}

/// `GetMediaInfo`: `None` without ffprobe, for disc images, or when the file cannot be read.
pub async fn read(path: &Path) -> Option<MediaInfo> {
    if !path.is_file() {
        return None;
    }
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if NOT_PROBED.contains(&ext.as_str()) {
        return None;
    }
    let streams = ["-show_format", "-show_streams"];
    let mut probe = ffprobe(&[&streams[..], &["-probesize", "50000000"]].concat(), path).await?;
    let layout_known = streams_of(&probe, "audio")
        .first()
        .and_then(|a| str_of(a, "channel_layout"))
        .is_some();
    if !layout_known
        && let Some(again) = ffprobe(
            &[
                &streams[..],
                &["-probesize", "150000000", "-analyzeduration", "150000000"],
            ]
            .concat(),
            path,
        )
        .await
    {
        probe = again;
    }
    // A PQ transfer: the first frame's side data tells the HDR format.
    let frames = match primary_video(&probe) {
        Some((position, v))
            if str_of(v, "color_transfer").is_some_and(|t| PQ_TRANSFER.contains(&t.as_str())) =>
        {
            let select = format!("v:{position}");
            ffprobe(
                &[
                    "-show_frames",
                    "-read_intervals",
                    "%+#1",
                    "-select_streams",
                    &select,
                ],
                path,
            )
            .await
        }
        _ => None,
    };
    // ffprobe reads some non-video data as a stream-less container: no media information
    // (it would otherwise be a zero-length "sample").
    if primary_video(&probe).is_none() {
        tracing::debug!(path = %path.display(), "no video stream: no media info");
        return None;
    }
    let info = from_ffprobe(&probe, frames.as_ref());
    tracing::debug!(path = %path.display(), "media info read");
    Some(info)
}

fn streams_of<'a>(probe: &'a Value, kind: &str) -> Vec<&'a Value> {
    let mut list: Vec<&Value> = probe
        .get("streams")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|s| s.get("codec_type").and_then(Value::as_str) == Some(kind))
                .collect()
        })
        .unwrap_or_default();
    list.sort_by_key(|s| s.get("index").and_then(Value::as_i64).unwrap_or(0));
    list
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn num_of(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A tag, matched without regard to case (Matroska writes `DURATION`, `BPS`, `TITLE`).
fn tag(v: &Value, key: &str) -> Option<String> {
    v.get("tags")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `GetPrimaryVideoStream`, with its position among the video streams: cover art (`mjpeg`,
/// `png`) often comes first.
fn primary_video(probe: &Value) -> Option<(usize, &Value)> {
    let videos = streams_of(probe, "video");
    if videos.len() > 1
        && let Some(found) = videos.iter().enumerate().find(|(_, s)| {
            !matches!(
                str_of(s, "codec_name").as_deref(),
                Some("mjpeg") | Some("png")
            )
        })
    {
        return Some((found.0, found.1));
    }
    videos.first().map(|v| (0, *v))
}

/// A stream's duration: `duration`, else the Matroska `DURATION` tag.
fn duration(v: &Value) -> Option<f64> {
    num_of(v, "duration")
        .or_else(|| tag(v, "DURATION").and_then(|d| super::timespan::parse(&d)))
        .filter(|d| *d > 0.0)
}

/// `GetBitrate`: `bit_rate`, else the `BPS` tag.
fn bitrate(v: &Value) -> i64 {
    num_of(v, "bit_rate")
        .filter(|b| *b > 0.0)
        .map(|b| b as i64)
        .or_else(|| tag(v, "BPS").and_then(|b| b.parse().ok()))
        .unwrap_or(0)
}

/// The smallest component depth of a pixel format (`yuv420p10le` → 10, `p010le` → 10,
/// `rgb48le` → 16); 8 when unknown.
pub fn pixel_bit_depth(format: &str) -> i32 {
    let f = format.trim().to_ascii_lowercase();
    let f = f
        .strip_suffix("le")
        .or_else(|| f.strip_suffix("be"))
        .unwrap_or(&f);
    if let Some(d) = f.strip_prefix('p').and_then(|d| d.parse::<i32>().ok()) {
        // `p010`, `p016`: semi-planar with the depth in the name.
        return d.clamp(8, 16);
    }
    match f {
        "rgb48" | "rgba64" | "bgr48" | "bgra64" | "gray16" | "ya16" => return 16,
        _ => {}
    }
    let digits: String = f
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let before = f.len() - digits.len();
    match digits.parse::<i32>() {
        // A depth follows the plane layout (`yuv420p10`, `gray12`, `gbrp10`); `nv12` and
        // `nv21` name their chroma layout.
        Ok(d)
            if (9..=16).contains(&d)
                && (f[..before].ends_with('p') || f[..before].ends_with("gray")) =>
        {
            d
        }
        _ => 8,
    }
}

fn frame_rate(v: &Value) -> f64 {
    let ratio = |key: &str| {
        let s = str_of(v, key)?;
        let (n, d) = s.split_once('/')?;
        let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
        (d > 0.0 && n > 0.0).then_some(n / d)
    };
    ratio("avg_frame_rate")
        .or_else(|| ratio("r_frame_rate"))
        .unwrap_or(0.0)
}

fn side_data(v: Option<&Value>) -> Vec<Value> {
    v.and_then(|v| v.get("side_data_list"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn find_side_data<'a>(list: &'a [Value], kind: &str) -> Option<&'a Value> {
    list.iter().find(|s| {
        s.get("side_data_type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case(kind))
    })
}

/// `GetHdrFormat`.
pub fn hdr_format(
    bit_depth: i32,
    primaries: Option<&str>,
    transfer: Option<&str>,
    side_data: &[Value],
) -> HdrFormat {
    if bit_depth < 10 {
        return HdrFormat::None;
    }
    if let Some(dovi) = find_side_data(side_data, DOVI_CONFIGURATION) {
        let plus = find_side_data(side_data, HDR10_PLUS).is_some();
        return match dovi
            .get("dv_bl_signal_compatibility_id")
            .and_then(Value::as_i64)
        {
            Some(1) | Some(6) if plus => HdrFormat::DolbyVisionHdr10Plus,
            Some(1) | Some(6) => HdrFormat::DolbyVisionHdr10,
            Some(2) => HdrFormat::DolbyVisionSdr,
            Some(4) => HdrFormat::DolbyVisionHlg,
            _ => HdrFormat::DolbyVision,
        };
    }
    let (Some(primaries), Some(transfer)) = (primaries, transfer) else {
        return HdrFormat::None;
    };
    if !HDR_PRIMARIES.contains(&primaries) {
        return HdrFormat::None;
    }
    if HLG_TRANSFER.contains(&transfer) {
        return HdrFormat::Hlg10;
    }
    if PQ_TRANSFER.contains(&transfer) {
        if find_side_data(side_data, HDR10_PLUS).is_some() {
            return HdrFormat::Hdr10Plus;
        }
        if find_side_data(side_data, MASTERING_DISPLAY).is_some()
            || find_side_data(side_data, CONTENT_LIGHT_LEVEL).is_some()
        {
            return HdrFormat::Hdr10;
        }
        return HdrFormat::Pq10;
    }
    HdrFormat::None
}

/// The model from ffprobe's `-show_format -show_streams` output (and `-show_frames` for the
/// first video frame).
pub fn from_ffprobe(probe: &Value, frames: Option<&Value>) -> MediaInfo {
    let null = Value::Null;
    let format = probe.get("format").unwrap_or(&null);
    let video = primary_video(probe).map(|(_, v)| v);
    let audio = streams_of(probe, "audio");
    let mut m = MediaInfo {
        schema_revision: CURRENT_SCHEMA_REVISION,
        container_format: str_of(format, "format_name"),
        title: tag(format, "title"),
        ..Default::default()
    };
    if let Some(v) = video {
        m.video_format = str_of(v, "codec_name");
        m.video_codec_id = str_of(v, "codec_tag_string");
        m.video_profile = str_of(v, "profile");
        m.video_bitrate = bitrate(v);
        m.video_bit_depth = str_of(v, "pix_fmt").map_or(8, |p| pixel_bit_depth(&p));
        m.video_colour_primaries = str_of(v, "color_primaries");
        m.video_transfer_characteristics = str_of(v, "color_transfer");
        m.height = num_of(v, "height").unwrap_or(0.0) as i32;
        m.width = num_of(v, "width").unwrap_or(0.0) as i32;
        m.video_fps = frame_rate(v);
        m.scan_type = Some(
            match str_of(v, "field_order").as_deref() {
                Some("tt" | "bb" | "tb" | "bt") => "Interlaced",
                _ => "Progressive",
            }
            .into(),
        );
    } else {
        m.video_bit_depth = 8;
    }
    // `GetBestRuntime`: video, else audio, else the container.
    m.run_time = video
        .and_then(duration)
        .or_else(|| audio.first().and_then(|a| duration(a)))
        .or_else(|| num_of(format, "duration"))
        .unwrap_or(0.0);
    let title = |s: &Value| tag(s, "title").or_else(|| tag(s, "name"));
    m.audio_streams = audio
        .iter()
        .map(|s| AudioStream {
            language: Some(tag(s, "language").unwrap_or_else(|| "und".into())),
            format: str_of(s, "codec_name"),
            codec_id: str_of(s, "codec_tag_string"),
            profile: str_of(s, "profile"),
            bitrate: bitrate(s),
            channels: num_of(s, "channels").unwrap_or(0.0) as i32,
            channel_positions: str_of(s, "channel_layout"),
            title: title(s),
        })
        .collect();
    m.subtitle_streams = streams_of(probe, "subtitle")
        .into_iter()
        .filter_map(|s| {
            let language = tag(s, "language")?;
            let disposition = |k: &str| {
                s.get("disposition")
                    .and_then(|d| d.get(k))
                    .and_then(Value::as_i64)
                    .map(|v| v == 1)
            };
            Some(SubtitleStream {
                language: Some(language),
                format: str_of(s, "codec_name"),
                title: title(s),
                forced: disposition("forced"),
                hearing_impaired: disposition("hearing_impaired"),
            })
        })
        .collect();
    let first_frame = frames
        .and_then(|f| f.get("frames"))
        .and_then(Value::as_array)
        .and_then(|f| f.first());
    let mut sides = side_data(video);
    sides.extend(side_data(first_frame));
    m.video_hdr_format = hdr_format(
        m.video_bit_depth,
        m.video_colour_primaries.as_deref(),
        m.video_transfer_characteristics.as_deref(),
        &sides,
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/../../tests/fixtures/mediainfo/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn pixel_formats() {
        for (f, d) in [
            ("yuv420p", 8),
            ("yuv420p10le", 10),
            ("yuv444p12be", 12),
            ("p010le", 10),
            ("nv12", 8),
            ("gray", 8),
            ("gray10le", 10),
            ("rgb48le", 16),
            ("gbrp10le", 10),
            ("", 8),
        ] {
            assert_eq!(pixel_bit_depth(f), d, "{f}");
        }
    }

    #[test]
    fn an_sdr_file_with_two_audio_tracks_and_forced_subtitles() {
        let m = from_ffprobe(&fixture("sdr_two_audio_forced_subs.json"), None);
        assert_eq!(m.container_format.as_deref(), Some("matroska,webm"));
        assert_eq!(m.title.as_deref(), Some("Sample Show"));
        assert_eq!(m.video_format.as_deref(), Some("h264"));
        assert_eq!((m.width, m.height, m.video_bit_depth), (1280, 720, 8));
        assert!((m.video_fps - 23.976).abs() < 0.001, "{}", m.video_fps);
        assert_eq!(m.scan_type.as_deref(), Some("Progressive"));
        assert!((m.run_time - 2.0).abs() < 0.1, "{}", m.run_time);
        assert_eq!(m.video_hdr_format, HdrFormat::None);
        let audio: Vec<_> = m
            .audio_streams
            .iter()
            .map(|a| {
                (
                    a.language.as_deref().unwrap(),
                    a.format.as_deref().unwrap(),
                    a.channels,
                    a.channel_positions.as_deref().unwrap_or(""),
                    a.title.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            audio,
            vec![
                ("jpn", "aac", 6, "5.1", None),
                ("eng", "aac", 2, "stereo", Some("Commentary"))
            ]
        );
        assert_eq!(m.subtitle_streams.len(), 1);
        let s = &m.subtitle_streams[0];
        assert_eq!(
            (s.language.as_deref(), s.format.as_deref(), s.forced),
            (Some("eng"), Some("subrip"), Some(true))
        );
        assert_eq!(s.hearing_impaired, Some(false));
    }

    #[test]
    fn hdr10_needs_the_first_frame() {
        let probe = fixture("hdr10.json");
        let without = from_ffprobe(&probe, None);
        assert_eq!(without.video_bit_depth, 10);
        assert_eq!(without.video_hdr_format, HdrFormat::Pq10);
        let with = from_ffprobe(&probe, Some(&fixture("hdr10_frames.json")));
        assert_eq!(with.video_hdr_format, HdrFormat::Hdr10);
        assert_eq!(with.video_profile.as_deref(), Some("Main 10"));
        assert_eq!(with.video_codec_id.as_deref(), Some("hvc1"));
    }

    /// `GetHdrFormat` as upstream's `VideoFileInfoReaderFixture` cases.
    #[test]
    fn hdr_formats() {
        let side = |kinds: &[&str]| -> Vec<Value> {
            kinds.iter().map(|k| json!({"side_data_type": k})).collect()
        };
        let dovi = |id: i64| json!({"side_data_type": DOVI_CONFIGURATION, "dv_bl_signal_compatibility_id": id});
        let none: Vec<Value> = Vec::new();
        let pq = (Some("bt2020"), Some("smpte2084"));
        assert_eq!(hdr_format(8, pq.0, pq.1, &none), HdrFormat::None);
        assert_eq!(hdr_format(10, pq.0, pq.1, &none), HdrFormat::Pq10);
        assert_eq!(
            hdr_format(10, pq.0, pq.1, &side(&[MASTERING_DISPLAY])),
            HdrFormat::Hdr10
        );
        assert_eq!(
            hdr_format(10, pq.0, pq.1, &side(&[CONTENT_LIGHT_LEVEL])),
            HdrFormat::Hdr10
        );
        assert_eq!(
            hdr_format(10, pq.0, pq.1, &side(&[MASTERING_DISPLAY, HDR10_PLUS])),
            HdrFormat::Hdr10Plus
        );
        assert_eq!(
            hdr_format(10, Some("bt2020"), Some("arib-std-b67"), &none),
            HdrFormat::Hlg10
        );
        assert_eq!(
            hdr_format(10, Some("bt709"), Some("smpte2084"), &none),
            HdrFormat::None
        );
        assert_eq!(
            hdr_format(10, None, None, &[dovi(1)]),
            HdrFormat::DolbyVisionHdr10
        );
        let mut plus = vec![dovi(6)];
        plus.extend(side(&[HDR10_PLUS]));
        assert_eq!(
            hdr_format(10, None, None, &plus),
            HdrFormat::DolbyVisionHdr10Plus
        );
        assert_eq!(
            hdr_format(10, None, None, &[dovi(2)]),
            HdrFormat::DolbyVisionSdr
        );
        assert_eq!(
            hdr_format(10, None, None, &[dovi(4)]),
            HdrFormat::DolbyVisionHlg
        );
        assert_eq!(
            hdr_format(10, None, None, &[dovi(0)]),
            HdrFormat::DolbyVision
        );
    }

    /// The real ffprobe, when installed, reads a file as the recorded output says.
    #[tokio::test]
    async fn reads_a_real_file_when_ffprobe_is_installed() {
        if !available().await {
            eprintln!("ffprobe is not installed: skipped");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-video.mkv");
        std::fs::write(&path, b"plain text").unwrap();
        assert_eq!(read(&path).await, None);
        assert_eq!(read(&dir.path().join("missing.mkv")).await, None);
        let iso = dir.path().join("disc.iso");
        std::fs::write(&iso, b"x").unwrap();
        assert_eq!(read(&iso).await, None);
    }
}
