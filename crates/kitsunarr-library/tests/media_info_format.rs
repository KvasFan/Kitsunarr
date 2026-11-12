//! Sonarr's `MediaInfoFormatterTests` (`FormatAudioCodecFixture`, `FormatVideoCodecFixture`,
//! `FormatVideoDynamicRangeFixture`, `FormatVideoDynamicRangeTypeFixture`) and the native
//! codec names of `CodecNormalizedSizeFixture`.

use kitsunarr_library::media_info::format::*;
use kitsunarr_library::media_info::{AudioStream, HdrFormat, MediaInfo};

const SCENE_NAME: &str = "My.Series.S01E01-Sonarr";

fn audio(format: &str, codec_id: &str, profile: &str) -> AudioStream {
    AudioStream {
        format: Some(format.into()),
        codec_id: Some(codec_id.into()),
        profile: Some(profile.into()),
        ..Default::default()
    }
}

fn video(format: &str, codec_id: &str) -> MediaInfo {
    MediaInfo {
        video_format: Some(format.into()),
        video_codec_id: Some(codec_id.into()),
        ..Default::default()
    }
}

fn hdr(format: HdrFormat) -> MediaInfo {
    MediaInfo {
        video_hdr_format: format,
        ..Default::default()
    }
}

#[test]
fn should_format_audio_format() {
    let cases: &[([&str; 3], &str, &str)] = &[
        (
            ["mp2", "", ""],
            "droned.s01e03.swedish.720p.hdtv.x264-prince",
            "MP2",
        ),
        (["vorbis", "", ""], "DB Super HDTV", "Vorbis"),
        (["pcm_s16le", "", ""], "DW DVDRip XviD-idTV, ", "PCM"),
        (["truehd", "", ""], "", "TrueHD"),
        (["truehd", "", ""], "TrueHD", "TrueHD"),
        (["truehd", "thd+", ""], "Atmos", "TrueHD Atmos"),
        (["truehd", "thd+", ""], "TrueHD.Atmos.7.1", "TrueHD Atmos"),
        (["truehd", "thd+", ""], "", "TrueHD Atmos"),
        (
            ["truehd", "", "Dolby TrueHD + Dolby Atmos"],
            "Atmos",
            "TrueHD Atmos",
        ),
        (
            ["truehd", "", "Dolby TrueHD + Dolby Atmos"],
            "TrueHD.Atmos.7.1",
            "TrueHD Atmos",
        ),
        (
            ["truehd", "", "Dolby TrueHD + Dolby Atmos"],
            "",
            "TrueHD Atmos",
        ),
        (["wmav1", "", ""], "Droned.wmv", "WMA"),
        (["wmav2", "", ""], "B.N.S04E18.720p.WEB-DL", "WMA"),
        (["opus", "", ""], "Roadkill Ep3x11 - YouTube.webm", "Opus"),
        (["mp3", "", ""], "climbing.mp4", "MP3"),
        (["dts", "", "DTS-HD MA"], "DTS-HD.MA", "DTS-HD MA"),
        (["dts", "", "DTS:X"], "DTS-X", "DTS-X"),
        (["dts", "", "DTS-HD MA + DTS:X"], "DTS-X", "DTS-X"),
        (["dts", "", "DTS-HD MA + DTS:X IMAX"], "DTS-X", "DTS-X"),
        (["dts", "", "DTS-HD MA"], "DTS-HD.MA", "DTS-HD MA"),
        (["dts", "", "DTS-ES"], "DTS-ES", "DTS-ES"),
        (["dts", "", "DTS-ES"], "DTS", "DTS-ES"),
        (["dts", "", "DTS-HD HRA"], "DTSHD-HRA", "DTS-HD HRA"),
        (["dts", "", "DTS"], "DTS", "DTS"),
        (["eac3", "ec+3", ""], "EAC3.Atmos", "EAC3 Atmos"),
        (
            ["eac3", "", "Dolby Digital Plus + Dolby Atmos"],
            "EAC3.Atmos",
            "EAC3 Atmos",
        ),
        (["eac3", "", ""], "DDP5.1", "EAC3"),
        (["ac3", "", ""], "DD5.1", "AC3"),
        (["aac", "", ""], "AAC2.0", "AAC"),
        (["aac", "", "HE-AAC"], "AAC2.0", "HE-AAC"),
        (["aac", "", "xHE-AAC"], "AAC2.0", "xHE-AAC"),
        (["adpcm_ima_qt", "", ""], "Custom?", "PCM"),
        (["adpcm_ms", "", ""], "Custom", "PCM"),
    ];
    assert_eq!(cases.len(), 33);
    for ([format, codec_id, profile], scene_name, expected) in cases {
        let stream = audio(format, codec_id, profile);
        assert_eq!(
            format_audio_codec(Some(&stream), scene_name),
            *expected,
            "{format}/{codec_id}/{profile} in {scene_name:?}"
        );
    }
}

#[test]
fn should_return_audio_format_by_default() {
    let stream = AudioStream {
        format: Some("Other Audio Format".into()),
        codec_id: Some("Other Audio Codec".into()),
        ..Default::default()
    };
    assert_eq!(
        format_audio_codec(Some(&stream), SCENE_NAME),
        "Other Audio Format"
    );
}

#[test]
fn should_return_null_if_audio_stream_is_null() {
    assert_eq!(format_audio_codec(None, SCENE_NAME), "");
}

#[test]
fn should_format_video_format() {
    let cases: &[([&str; 2], &str, &str)] = &[
        (
            ["mpeg2video", ""],
            "Droned.S01E02.1080i.HDTV.DD5.1.MPEG2-NTb",
            "MPEG2",
        ),
        (["mpeg2video", ""], "", "MPEG2"),
        (
            ["mpeg1video", ""],
            "The.Simpsons.S13E04.INTERNAL-ANiVCD.mpg",
            "MPEG",
        ),
        (["vc1", "WVC1"], "B.N.S04E18.720p.WEB-DL", "VC1"),
        (["vc1", "V_MS/VFW/FOURCC/WVC1"], "", "VC1"),
        (
            ["vc1", "WMV3"],
            "It's Always Sunny S07E13 The Gang's RevengeHDTV.XviD-2HD.avi",
            "VC1",
        ),
        (
            ["h264", "V.MPEG4/ISO/AVC"],
            "pd.2015.S03E08.720p.iP.WEBRip.AAC2.0.H264-BTW",
            "h264",
        ),
        (
            ["h264", "V_MPEG4/ISO/AVC"],
            "Resistance.2019.S01E03.1080p.RTE.WEB-DL.AAC2.0.x264-RTN",
            "x264",
        ),
        (["wmv1", "WMV1"], "Droned.wmv", "WMV"),
        (["wmv2", "WMV2"], "Droned.wmv", "WMV"),
        (["mpeg4", "XVID"], "", "XviD"),
        (["mpeg4", "DIVX"], "", "DivX"),
        (["mpeg4", "divx"], "", "DivX"),
        (["mpeg4", "DIV3"], "spsm.dvdrip.divx.avi'.", "DivX"),
        (
            ["msmpeg4", "DIV3"],
            "Exit the Dragon, Enter the Tiger (1976) 360p MPEG Audio.avi",
            "DivX",
        ),
        (
            ["msmpeg4v2", "DIV3"],
            "Exit the Dragon, Enter the Tiger (1976) 360p MPEG Audio.avi",
            "DivX",
        ),
        (
            ["msmpeg4v3", "DIV3"],
            "Exit the Dragon, Enter the Tiger (1976) 360p MPEG Audio.avi",
            "DivX",
        ),
        (
            ["vp6f", "4"],
            "Top Gear - S12E01 - Lorries - SD TV.flv",
            "VP6",
        ),
        (
            ["vp6", "4"],
            "Top Gear - S12E01 - Lorries - SD TV.flv",
            "VP6",
        ),
        (["vp7", "VP70"], "Sweet Seymour.avi", "VP7"),
        (["vp8", "V_VP8"], "Dick.mkv", "VP8"),
        (["vp9", "V_VP9"], "Roadkill Ep3x11 - YouTube.webm", "VP9"),
        (
            ["h264", "x264"],
            "Ghost Advent - S04E05 - Stanley Hotel SDTV.avi",
            "x264",
        ),
        (
            ["hevc", "V_MPEGH/ISO/HEVC"],
            "The BBT S11E12 The Matrimonial Metric 1080p 10bit AMZN WEB-DL",
            "h265",
        ),
        (["mpeg4", "mp4v-20"], "", ""),
        (
            ["mpeg4", "XVID"],
            "American.Chopper.S06E07.Mountain.Creek.Bike.DSR.XviD-KRS",
            "XviD",
        ),
        (["rpza", "V_QUICKTIME"], "Custom", ""),
        (["mpeg4", "FMP4"], "", ""),
        (["mpeg4", "MP42"], "", ""),
        (
            ["mpeg4", "mp43"],
            "Bubble.Guppies.S01E13.480p.WEB-DL.H.264-BTN-Custom",
            "",
        ),
        (["vvc", ""], "", "h266"),
        (
            ["vvc", ""],
            "Droned.S01E02.1080p.WEB-DL.DD5.1.VVC-NTb",
            "VVC",
        ),
        (
            ["vvc", ""],
            "Droned.S01E02.1080p.WEB-DL.DD5.1.H266-NTb",
            "h266",
        ),
        (
            ["vvc", "x266"],
            "Droned.S01E02.1080p.WEB-DL.DD5.1.H266-NTb",
            "x266",
        ),
    ];
    assert_eq!(cases.len(), 34);
    for ([format, codec_id], scene_name, expected) in cases {
        assert_eq!(
            format_video_codec(&video(format, codec_id), scene_name),
            *expected,
            "{format}/{codec_id} in {scene_name:?}"
        );
    }
}

#[test]
fn should_format_video_format_fallbacks() {
    let cases: &[([&str; 2], &str, &str)] = &[
        // Force mediainfo tag
        (["h264", "x264"], "Some.Video.S01E01.h264", "x264"),
        (["hevc", "x265"], "Some.Video.S01E01.h265", "x265"),
        (["vvc", "x266"], "Some.Video.S01E01.h266", "x266"),
        // Not seen in practice, but honor tag if otherwise unknown
        (["h264", ""], "Some.Video.S01E01.x264", "x264"),
        (["hevc", ""], "Some.Video.S01E01.x265", "x265"),
        (["vvc", ""], "Some.Video.S01E01.x266", "x266"),
        // Default value
        (["h264", ""], "Some.Video.S01E01", "h264"),
        (["hevc", ""], "Some.Video.S01E01", "h265"),
        (["vvc", ""], "Some.Video.S01E01", "h266"),
    ];
    for ([format, codec_id], scene_name, expected) in cases {
        assert_eq!(
            format_video_codec(&video(format, codec_id), scene_name),
            *expected,
            "{format}/{codec_id} in {scene_name:?}"
        );
    }
}

#[test]
fn should_return_video_format_by_default() {
    let info = MediaInfo {
        video_format: Some("VideoCodec".into()),
        ..Default::default()
    };
    // Upstream passes a null scene name.
    assert_eq!(format_video_codec(&info, ""), "VideoCodec");
}

#[test]
fn should_return_null_if_video_stream_is_null() {
    // Upstream passes a null model; here, a model without a video format.
    assert_eq!(format_video_codec(&MediaInfo::default(), ""), "");
}

/// `CodecNormalizedSizeFixture.should_accept_codec_names_produced_by_native_media_information`:
/// the names the formatter gives without a scene name (that fixture then checks the codec
/// family the size normalizer reads from them).
#[test]
fn native_codec_names_without_scene_name() {
    for (format, expected) in [
        ("h264", "h264"),
        ("hevc", "h265"),
        ("vvc", "h266"),
        ("av1", "AV1"),
        ("vp9", "VP9"),
        ("mpeg2video", "MPEG2"),
    ] {
        let info = MediaInfo {
            video_format: Some(format.into()),
            ..Default::default()
        };
        assert_eq!(format_video_codec(&info, ""), expected, "{format}");
    }
}

#[test]
fn should_format_video_dynamic_range() {
    for (format, expected) in [
        (HdrFormat::None, ""),
        (HdrFormat::Hlg10, "HDR"),
        (HdrFormat::Pq10, "HDR"),
        (HdrFormat::Hdr10, "HDR"),
        (HdrFormat::Hdr10Plus, "HDR"),
        (HdrFormat::DolbyVision, "HDR"),
    ] {
        assert_eq!(
            format_video_dynamic_range(&hdr(format)),
            expected,
            "{format:?}"
        );
    }
}

#[test]
fn should_format_video_dynamic_range_type() {
    for (format, expected) in [
        (HdrFormat::None, ""),
        (HdrFormat::Hlg10, "HLG"),
        (HdrFormat::Pq10, "PQ"),
        (HdrFormat::Hdr10, "HDR10"),
        (HdrFormat::Hdr10Plus, "HDR10Plus"),
        (HdrFormat::DolbyVision, "DV"),
        (HdrFormat::DolbyVisionHdr10, "DV HDR10"),
        (HdrFormat::DolbyVisionHdr10Plus, "DV HDR10Plus"),
        (HdrFormat::DolbyVisionHlg, "DV HLG"),
        (HdrFormat::DolbyVisionSdr, "DV SDR"),
    ] {
        assert_eq!(
            format_video_dynamic_range_type(&hdr(format)),
            expected,
            "{format:?}"
        );
    }
}

/// Not an upstream fixture (Sonarr has none for `FormatAudioChannels`).
#[test]
fn audio_channels_from_positions_then_channel_count() {
    let stream = |positions: Option<&str>, channels: i32| AudioStream {
        channel_positions: positions.map(Into::into),
        channels,
        ..Default::default()
    };
    assert_eq!(format_audio_channels(None), 0.0);
    assert_eq!(
        format_audio_channels(Some(&stream(Some("5.1(side)"), 6))),
        5.1
    );
    assert_eq!(format_audio_channels(Some(&stream(Some("7.1"), 8))), 7.1);
    assert_eq!(format_audio_channels(Some(&stream(Some("stereo"), 2))), 2.0);
    assert_eq!(
        format_audio_channels(Some(&stream(Some("3/2/0.1"), 6))),
        6.0
    );
    assert_eq!(format_audio_channels(Some(&stream(None, 1))), 1.0);
    assert_eq!(format_audio_channels(Some(&stream(Some("0.0"), 2))), 2.0);
    assert_eq!(format_audio_channels(Some(&stream(None, 0))), 0.0);
}
