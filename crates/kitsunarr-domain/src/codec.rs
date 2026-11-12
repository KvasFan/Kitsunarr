//! Tanukarr's codec size factors (`CodecSizeNormalizer`): a release's size is multiplied by its
//! video codec's factor before the size limits and the preferred size apply, so an efficient
//! codec (HEVC, AV1) is judged like the H.264 release it replaces.

/// Codec families with their default factors.
pub const DEFAULTS: &[(&str, f64)] = &[
    ("MPEG1", 0.40),
    ("MPEG2", 0.50),
    ("MPEG4ASP", 0.67),
    ("H263", 0.40),
    ("H264", 1.00),
    ("HEVC", 1.67),
    ("AV1", 2.22),
    ("VVC", 2.50),
    ("VC1", 0.80),
    ("WMV1", 0.60),
    ("WMV2", 0.60),
    ("WMV3", 0.80),
    ("VP6", 0.60),
    ("VP7", 0.60),
    ("VP8", 1.00),
    ("VP9", 1.55),
    ("Theora", 0.50),
    ("Unknown", 1.00),
];

/// The family names (canonical keys of per-profile overrides).
pub const FAMILIES: &[&str] = &[
    "MPEG1", "MPEG2", "MPEG4ASP", "H263", "H264", "HEVC", "AV1", "VVC", "VC1", "WMV1", "WMV2",
    "WMV3", "VP6", "VP7", "VP8", "VP9", "Theora", "Unknown",
];

/// A family's default factor.
pub fn default_factor(family: &str) -> f64 {
    DEFAULTS
        .iter()
        .find(|(f, _)| *f == family)
        .map_or(1.0, |(_, m)| *m)
}

/// A readable label for a family (Tanukarr's `MultiplierCatalog`).
pub fn label(family: &str) -> &'static str {
    match family {
        "MPEG1" => "MPEG-1",
        "MPEG2" => "MPEG-2 / H.262 / x262",
        "MPEG4ASP" => "MPEG-4 ASP / XviD / DivX",
        "H263" => "H.263",
        "H264" => "H.264 / AVC / x264",
        "HEVC" => "H.265 / HEVC / x265",
        "AV1" => "AV1",
        "VVC" => "H.266 / VVC / x266",
        "VC1" => "VC-1",
        "WMV1" => "WMV1",
        "WMV2" => "WMV2",
        "WMV3" => "WMV3",
        "VP6" => "VP6",
        "VP7" => "VP7",
        "VP8" => "VP8",
        "VP9" => "VP9",
        "Theora" => "Theora",
        _ => "Unknown / mixed / lossless",
    }
}

/// Canonical family of a case-insensitive key, as Tanukarr reads stored overrides.
pub fn canonical(key: &str) -> Option<&'static str> {
    FAMILIES
        .iter()
        .copied()
        .find(|f| f.eq_ignore_ascii_case(key))
}
