//! Media information read from video files with ffprobe: Sonarr's `MediaInfoModel`,
//! `VideoFileInfoReader` and `MediaInfoFormatter`.
//!
//! The model is stored in `episode_files.media_info` / `movie_files.media_info` as Sonarr stores
//! it (camelCase keys, enum names in camelCase, `runTime` as a .NET `TimeSpan` string), so media
//! information migrated from Sonarr or Tanukarr is read as it is.

pub mod format;
pub mod reader;

use kitsunarr_domain::QualitySource;
use serde::{Deserialize, Serialize};

/// `VideoFileInfoReader.CURRENT_MEDIA_INFO_SCHEMA_REVISION`.
pub const CURRENT_SCHEMA_REVISION: i32 = 14;
/// `VideoFileInfoReader.MINIMUM_MEDIA_INFO_SCHEMA_REVISION`: older records are read again.
pub const MINIMUM_SCHEMA_REVISION: i32 = 14;

/// `HdrFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HdrFormat {
    #[default]
    None,
    UnknownHdr,
    Pq10,
    Hdr10,
    Hdr10Plus,
    Hlg10,
    DolbyVision,
    DolbyVisionHdr10,
    DolbyVisionSdr,
    DolbyVisionHlg,
    DolbyVisionHdr10Plus,
}

/// `MediaInfoAudioStreamModel`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioStream {
    pub language: Option<String>,
    pub format: Option<String>,
    pub codec_id: Option<String>,
    pub profile: Option<String>,
    pub bitrate: i64,
    pub channels: i32,
    pub channel_positions: Option<String>,
    pub title: Option<String>,
}

/// `MediaInfoSubtitleStreamModel`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubtitleStream {
    pub language: Option<String>,
    pub format: Option<String>,
    pub title: Option<String>,
    pub forced: Option<bool>,
    pub hearing_impaired: Option<bool>,
}

/// `MediaInfoModel`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_stream_data: Option<String>,
    pub schema_revision: i32,
    pub container_format: Option<String>,
    pub video_format: Option<String>,
    #[serde(rename = "videoCodecID", alias = "videoCodecId")]
    pub video_codec_id: Option<String>,
    pub video_profile: Option<String>,
    pub video_bitrate: i64,
    pub video_bit_depth: i32,
    pub video_colour_primaries: Option<String>,
    pub video_transfer_characteristics: Option<String>,
    pub video_hdr_format: HdrFormat,
    pub height: i32,
    pub width: i32,
    /// Seconds (stored as a .NET `TimeSpan` string, `hh:mm:ss.fffffff`).
    #[serde(with = "timespan")]
    pub run_time: f64,
    pub video_fps: f64,
    pub audio_streams: Vec<AudioStream>,
    pub subtitle_streams: Vec<SubtitleStream>,
    pub scan_type: Option<String>,
    pub title: Option<String>,
}

impl MediaInfo {
    /// `PrimaryAudioStream`.
    pub fn primary_audio(&self) -> Option<&AudioStream> {
        self.audio_streams.first()
    }

    /// `AugmentQualityFromMediaInfo`: the resolution from the picture size, and the source
    /// when the file's title tag names one. `None` without a picture size.
    pub fn quality_augment(&self) -> Option<(QualitySource, i32)> {
        let source = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(kitsunarr_parser::parse_quality_name)
            .filter(|q| {
                q.quality.source != QualitySource::Unknown
                    && q.source_detection == kitsunarr_parser::DetectionSource::Name
            })
            .map_or(QualitySource::Unknown, |q| q.quality.source);
        let (w, h) = (self.width, self.height);
        let resolution = if w >= 3200 || h >= 2100 {
            2160
        } else if w >= 1800 || h >= 1000 {
            1080
        } else if w >= 1200 || h >= 700 {
            720
        } else if w >= 1000 || h >= 560 {
            576
        } else if w > 0 && h > 0 {
            480
        } else {
            return None;
        };
        Some((source, resolution))
    }

    /// `DetectSample.IsSample` with a runtime: shorter than the minimum for the expected
    /// episode length (`expected_minutes`, 45 when unknown) is a sample.
    pub fn is_sample(&self, expected_minutes: i32) -> bool {
        let expected = if expected_minutes <= 0 {
            45
        } else {
            expected_minutes
        };
        // `GetMinimumAllowedRuntime`.
        let minimum = if expected <= 3 {
            15.0
        } else if expected <= 10 {
            90.0
        } else if expected <= 30 {
            300.0
        } else {
            600.0
        };
        self.run_time / 60.0 == 0.0 || self.run_time < minimum
    }

    /// Reads a stored record (Sonarr's or Kitsunarr's); `None` when it is not one.
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(v.clone()).ok()
    }
}

/// .NET `TimeSpan` strings (`[-][d.]hh:mm:ss[.fffffff]`), also accepting plain seconds.
mod timespan {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(secs: &f64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format(*secs))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(match v {
            serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
            serde_json::Value::String(s) => parse(&s).unwrap_or(0.0),
            _ => 0.0,
        })
    }

    pub fn format(secs: f64) -> String {
        let secs = secs.max(0.0);
        let ticks = (secs * 10_000_000.0).round() as u64;
        let (whole, frac) = (ticks / 10_000_000, ticks % 10_000_000);
        let (days, h, m, s) = (
            whole / 86_400,
            whole / 3600 % 24,
            whole / 60 % 60,
            whole % 60,
        );
        let mut out = String::new();
        if days > 0 {
            out.push_str(&format!("{days}."));
        }
        out.push_str(&format!("{h:02}:{m:02}:{s:02}"));
        if frac > 0 {
            out.push_str(&format!(".{frac:07}"));
        }
        out
    }

    pub fn parse(s: &str) -> Option<f64> {
        let s = s.trim();
        let colon = s.find(':')?;
        // A day count comes before a dot in the first part (`1.01:00:00`).
        let (days, rest) = match s[..colon].find('.') {
            Some(i) => (s[..i].parse::<f64>().ok()?, &s[i + 1..]),
            None => (0.0, s),
        };
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() != 3 {
            return None;
        }
        let h: f64 = parts[0].parse().ok()?;
        let m: f64 = parts[1].parse().ok()?;
        let sec: f64 = parts[2].parse().ok()?;
        Some(days * 86_400.0 + h * 3600.0 + m * 60.0 + sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timespans_round_trip_like_dotnet() {
        assert_eq!(timespan::format(1420.123), "00:23:40.1230000");
        assert_eq!(timespan::format(60.0), "00:01:00");
        assert_eq!(timespan::format(90_000.5), "1.01:00:00.5000000");
        assert_eq!(timespan::parse("00:23:40.1230000"), Some(1420.123));
        assert_eq!(timespan::parse("1.01:00:00.5000000"), Some(90_000.5));
        assert_eq!(timespan::parse("x"), None);
    }

    #[test]
    fn sonarr_records_are_read() {
        let v = serde_json::json!({
            "schemaRevision": 14, "containerFormat": "matroska,webm", "videoFormat": "hevc",
            "videoCodecID": "", "videoProfile": "Main 10", "videoBitrate": 0, "videoBitDepth": 10,
            "videoColourPrimaries": "bt2020", "videoTransferCharacteristics": "smpte2084",
            "videoHdrFormat": "hdr10", "height": 2160, "width": 3840, "runTime": "00:58:12.3450000",
            "videoFps": 23.976, "audioStreams": [{"language": "eng", "format": "eac3", "channels": 6,
            "channelPositions": "5.1(side)"}], "subtitleStreams": [{"language": "eng", "format": "subrip"}],
            "scanType": "Progressive"
        });
        let m = MediaInfo::from_json(&v).unwrap();
        assert_eq!(m.video_hdr_format, HdrFormat::Hdr10);
        assert_eq!(m.run_time, 3492.345);
        assert_eq!(m.primary_audio().unwrap().channels, 6);
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(back["runTime"], "00:58:12.3450000");
        assert_eq!(back["videoHdrFormat"], "hdr10");
        assert_eq!(back["videoCodecID"], "");
    }
}
