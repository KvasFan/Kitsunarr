//! Media file extensions and the quality each implies (`MediaFiles/MediaFileExtensions.cs`).

use kitsunarr_domain::Quality;

/// Extension (with the dot, lower case) → Sonarr quality id.
const FILE_EXTENSIONS: &[(&str, i32)] = &[
    // Unknown
    (".webm", 0),
    // SDTV
    (".m4v", 1),
    (".3gp", 1),
    (".nsv", 1),
    (".ty", 1),
    (".strm", 1),
    (".rm", 1),
    (".rmvb", 1),
    (".m3u", 1),
    (".ifo", 1),
    (".mov", 1),
    (".qt", 1),
    (".divx", 1),
    (".xvid", 1),
    (".bivx", 1),
    (".nrg", 1),
    (".pva", 1),
    (".wmv", 1),
    (".asf", 1),
    (".asx", 1),
    (".ogm", 1),
    (".ogv", 1),
    (".m2v", 1),
    (".avi", 1),
    (".bin", 1),
    (".dat", 1),
    (".dvr-ms", 1),
    (".mpg", 1),
    (".mpeg", 1),
    (".mp4", 1),
    (".avc", 1),
    (".vp3", 1),
    (".svq3", 1),
    (".nuv", 1),
    (".viv", 1),
    (".dv", 1),
    (".fli", 1),
    (".flv", 1),
    (".wpl", 1),
    // DVD
    (".img", 2),
    (".iso", 2),
    (".vob", 2),
    // HD
    (".mkv", 4),
    (".ts", 4),
    (".wtv", 4),
    // Bluray
    (".m2ts", 6),
];

/// Disc-image extensions (`DiskExtensions`).
pub const DISK_EXTENSIONS: &[&str] = &[".img", ".iso", ".vob"];

/// Playlist/stream stub extensions (`StreamingExtensions`).
pub const STREAMING_EXTENSIONS: &[&str] = &[".m3u", ".strm"];

/// Whether an extension (with the dot) is a known video extension.
pub fn is_video_extension(extension: &str) -> bool {
    let ext = extension.to_ascii_lowercase();
    FILE_EXTENSIONS.iter().any(|(e, _)| *e == ext)
}

/// `GetQualityForExtension`: the quality implied by a file extension (with the dot).
pub fn quality_for_extension(extension: &str) -> &'static Quality {
    let ext = extension.to_ascii_lowercase();
    FILE_EXTENSIONS
        .iter()
        .find(|(e, _)| *e == ext)
        .and_then(|(_, id)| Quality::by_id(*id))
        .unwrap_or_else(Quality::unknown)
}

/// .NET `Path.GetExtension` on Unix: the extension of the last path segment, including the
/// dot, or empty when there is none (or the name ends with a dot).
pub fn path_extension(path: &str) -> &str {
    let segment_start = path.rfind('/').map_or(0, |i| i + 1);
    match path[segment_start..].rfind('.') {
        Some(i) if segment_start + i + 1 < path.len() => &path[segment_start + i..],
        _ => "",
    }
}

/// Sonarr's `GetPathExtension`: everything from the last dot anywhere in the string, or empty
/// when there is no dot or it is the last character.
pub fn last_extension(path: &str) -> &str {
    match path.rfind('.') {
        Some(i) if i + 1 < path.len() => &path[i..],
        _ => "",
    }
}

/// Usenet-specific extensions (`FileExtensions.UsenetExtensions`).
const USENET_EXTENSIONS: &[&str] = &[".par2", ".nzb"];

static FILE_EXTENSION: std::sync::LazyLock<crate::re::Re> =
    std::sync::LazyLock::new(|| crate::re::Re::ci(r"\.[a-z0-9]{2,4}$"));

/// `FileExtensions.RemoveFileExtension`: strips a trailing media or Usenet extension.
pub fn remove_file_extension(title: &str) -> String {
    match FILE_EXTENSION.captures(title).and_then(|c| c.get(0)) {
        Some(m) => {
            let extension = m.as_str().to_lowercase();
            if is_video_extension(&extension) || USENET_EXTENSIONS.contains(&extension.as_str()) {
                title[..m.start()].to_string()
            } else {
                title.to_string()
            }
        }
        None => title.to_string(),
    }
}

/// `ContainsInvalidPathChars` with .NET's Unix definition (only NUL is invalid).
pub fn contains_invalid_path_chars(text: &str) -> bool {
    text.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(quality_for_extension(".MKV").name, "HDTV-720p");
        assert_eq!(quality_for_extension(".m2ts").name, "Bluray-720p");
        assert_eq!(quality_for_extension(".nfo").name, "Unknown");
        assert_eq!(path_extension("a/b.c/name.mkv"), ".mkv");
        assert_eq!(path_extension("dir.x/name"), "");
        assert_eq!(path_extension("name."), "");
        assert_eq!(last_extension("dir.x/name"), ".x/name");
    }
}
