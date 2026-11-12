//! `MediaInfoFormatter`: codec, channel and dynamic-range names for naming tokens.

use kitsunarr_parser::extensions::remove_file_extension;

use super::{AudioStream, HdrFormat, MediaInfo};

/// `VideoDynamicRangeHdr`.
const VIDEO_DYNAMIC_RANGE_HDR: &str = "HDR";

/// `FormatAudioChannels`: the channel count from the channel layout (`5.1(side)` → 5.1), else
/// the stream's channel count, else 0.
pub fn format_audio_channels(stream: Option<&AudioStream>) -> f64 {
    let channels = format_audio_channels_from_positions(stream);
    if channels != 0.0 {
        return channels;
    }
    stream.map_or(0.0, |s| f64::from(s.channels))
}

/// `FormatAudioChannelsFromAudioChannelPositions`: `^\d\.\d` of the channel layout, else 0.
fn format_audio_channels_from_positions(stream: Option<&AudioStream>) -> f64 {
    let Some(positions) = stream.and_then(|s| s.channel_positions.as_deref()) else {
        return 0.0;
    };
    match positions.as_bytes() {
        [a, b'.', b, ..] if a.is_ascii_digit() && b.is_ascii_digit() => {
            positions[..3].parse().unwrap_or(0.0)
        }
        _ => 0.0,
    }
}

/// `FormatAudioCodec`. See ffmpeg's `libavcodec/codec_desc.c` for the format names.
pub fn format_audio_codec(stream: Option<&AudioStream>, scene_name: &str) -> String {
    let Some(stream) = stream else {
        return String::new();
    };
    let Some(format) = stream.format.as_deref() else {
        return String::new();
    };
    let audio_format = format.trim();
    let codec_id = stream.codec_id.as_deref().unwrap_or("");
    let profile = stream.profile.as_deref().unwrap_or("");

    if audio_format.is_empty() {
        return String::new();
    }

    if codec_id == "thd+" {
        return "TrueHD Atmos".into();
    }

    if audio_format == "truehd" {
        return match profile {
            "Dolby TrueHD + Dolby Atmos" => "TrueHD Atmos",
            _ => "TrueHD",
        }
        .into();
    }

    if audio_format == "flac" {
        return "FLAC".into();
    }

    if audio_format == "dts" {
        return match profile {
            "DTS:X" | "DTS-HD MA + DTS:X" | "DTS-HD MA + DTS:X IMAX" => "DTS-X",
            "DTS-HD MA" => "DTS-HD MA",
            "DTS-ES" => "DTS-ES",
            "DTS-HD HRA" => "DTS-HD HRA",
            "DTS Express" => "DTS Express",
            "DTS 96/24" => "DTS 96/24",
            _ => "DTS",
        }
        .into();
    }

    if codec_id == "ec+3" {
        return "EAC3 Atmos".into();
    }

    if audio_format == "eac3" {
        return match profile {
            "Dolby Digital Plus + Dolby Atmos" => "EAC3 Atmos",
            _ => "EAC3",
        }
        .into();
    }

    if audio_format == "ac3" {
        return "AC3".into();
    }

    if audio_format == "aac" {
        if codec_id == "A_AAC/MPEG4/LC/SBR" {
            return "HE-AAC".into();
        }
        return match profile {
            "HE-AAC" => "HE-AAC",
            "xHE-AAC" => "xHE-AAC",
            _ => "AAC",
        }
        .into();
    }

    if audio_format == "mp3" {
        return "MP3".into();
    }

    if audio_format == "mp2" {
        return "MP2".into();
    }

    if audio_format == "opus" {
        return "Opus".into();
    }

    if audio_format.starts_with("pcm_") || audio_format.starts_with("adpcm_") {
        return "PCM".into();
    }

    if audio_format == "vorbis" {
        return "Vorbis".into();
    }

    if matches!(audio_format, "wmav1" | "wmav2" | "wmapro") {
        return "WMA".into();
    }

    tracing::debug!(
        format = format,
        codec_id = codec_id,
        "Unknown audio format: '{audio_format}' in '{scene_name}'"
    );

    audio_format.to_string()
}

/// `FormatVideoCodec`. See ffmpeg's `libavcodec/codec_desc.c` for the format names.
pub fn format_video_codec(info: &MediaInfo, scene_name: &str) -> String {
    let Some(format) = info.video_format.as_deref() else {
        return String::new();
    };
    let video_format = format.trim();
    let codec_id = info.video_codec_id.as_deref().unwrap_or("");

    if video_format.is_empty() {
        return String::new();
    }

    if codec_id == "x264" {
        return "x264".into();
    }

    if video_format == "h264" {
        return scene_name_match(scene_name, &["AVC", "x264", "h264"]);
    }

    if codec_id == "x265" {
        return "x265".into();
    }

    if video_format == "hevc" {
        return scene_name_match(scene_name, &["HEVC", "x265", "h265"]);
    }

    if codec_id == "x266" {
        return "x266".into();
    }

    if video_format == "vvc" {
        return scene_name_match(scene_name, &["VVC", "x266", "h266"]);
    }

    if video_format == "mpeg2video" {
        return "MPEG2".into();
    }

    if video_format == "mpeg1video" {
        return "MPEG".into();
    }

    if video_format == "mpeg4" || video_format.contains("msmpeg4") {
        let upper = codec_id.to_uppercase();
        if upper == "XVID" {
            return "XviD".into();
        }
        if codec_id == "DIV3" || codec_id == "DX50" || upper == "DIVX" {
            return "DivX".into();
        }
        return String::new();
    }

    if video_format == "vc1" {
        return "VC1".into();
    }

    if video_format == "av1" {
        return "AV1".into();
    }

    if video_format.contains("vp6") {
        return "VP6".into();
    }

    if matches!(video_format, "vp7" | "vp8" | "vp9") {
        return video_format.to_uppercase();
    }

    if matches!(video_format, "wmv1" | "wmv2" | "wmv3") {
        return "WMV".into();
    }

    if matches!(
        video_format,
        "qtrle" | "rpza" | "rv10" | "rv20" | "rv30" | "rv40" | "cinepak" | "rawvideo" | "msvideo1"
    ) {
        return String::new();
    }

    tracing::debug!(
        container = info.container_format.as_deref().unwrap_or(""),
        format = video_format,
        codec_id = codec_id,
        "Unknown video format: '{video_format}' in '{scene_name}'. Streams: {}",
        info.raw_stream_data.as_deref().unwrap_or("")
    );

    video_format.to_string()
}

/// `GetSceneNameMatch`: the first token found in the scene name (without its file extension,
/// ignoring case); the last token when none is.
fn scene_name_match(scene_name: &str, tokens: &[&str]) -> String {
    let scene_name = if scene_name.trim().is_empty() {
        String::new()
    } else {
        remove_file_extension(scene_name).to_lowercase()
    };

    tokens
        .iter()
        .find(|token| scene_name.contains(&token.to_lowercase()))
        .or(tokens.last())
        .map_or_else(String::new, |token| (*token).to_string())
}

/// `FormatVideoDynamicRange`: `HDR` for any HDR format.
pub fn format_video_dynamic_range(info: &MediaInfo) -> String {
    if info.video_hdr_format != HdrFormat::None {
        VIDEO_DYNAMIC_RANGE_HDR.into()
    } else {
        String::new()
    }
}

/// `FormatVideoDynamicRangeType`.
pub fn format_video_dynamic_range_type(info: &MediaInfo) -> String {
    match info.video_hdr_format {
        HdrFormat::DolbyVision => "DV",
        HdrFormat::DolbyVisionHdr10 => "DV HDR10",
        HdrFormat::DolbyVisionHdr10Plus => "DV HDR10Plus",
        HdrFormat::DolbyVisionHlg => "DV HLG",
        HdrFormat::DolbyVisionSdr => "DV SDR",
        HdrFormat::Hdr10 => "HDR10",
        HdrFormat::Hdr10Plus => "HDR10Plus",
        HdrFormat::Hlg10 => "HLG",
        HdrFormat::Pq10 => "PQ",
        HdrFormat::UnknownHdr => "HDR",
        HdrFormat::None => "",
    }
    .into()
}
