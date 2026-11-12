//! Tanukarr's fake release protection (`FakeReleaseInspector`): before a torrent is sent to the
//! download client, its title and file list are checked for executables, password-bait archives
//! and contents with nothing to import.

use std::sync::LazyLock;

use regex::Regex;

/// A file listed in a `.torrent`: its path inside the torrent and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFile {
    pub path: String,
    pub length: i64,
}

/// Why a release was judged fake (`FakeReleaseRejectionReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeReason {
    DisguisedExecutable,
    ExecutablePayload,
    NoUsableContent,
    PasswordBait,
}

/// A rejection, with the message shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fake {
    pub reason: FakeReason,
    pub message: String,
}

/// Even a short special is larger: smaller videos do not count as content.
const MINIMUM_VIDEO_SIZE: i64 = 10 * 1024 * 1024;

const EXECUTABLE: &[&str] = &[
    ".exe", ".scr", ".com", ".pif", ".bat", ".cmd", ".msi", ".msp", ".vbs", ".vbe", ".js", ".jse",
    ".wsf", ".wsh", ".ps1", ".psm1", ".hta", ".cpl", ".msc", ".reg", ".jar", ".apk", ".lnk",
    ".url", ".gadget", ".inf", ".dll", ".sys",
];
const VIDEO: &[&str] = &[
    ".mkv", ".mp4", ".avi", ".m4v", ".mpg", ".mpeg", ".wmv", ".mov", ".flv", ".ts", ".m2ts",
    ".mts", ".webm", ".ogm", ".ogv", ".divx", ".vob", ".rmvb", ".rm", ".asf", ".mk3d", ".iso",
    ".img", ".m2v", ".mpv", ".3gp", ".f4v", ".strm",
];
const ARCHIVE: &[&str] = &[
    ".rar", ".zip", ".7z", ".tar", ".gz", ".bz2", ".xz", ".z", ".cab", ".arj", ".lzh", ".ace",
];

static DISGUISED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\.(mkv|mp4|avi|m4v|mpg|mpeg|wmv|mov|flv|ts|webm|iso|srt|sub|nfo|jpg|png)\s*\.(exe|scr|com|pif|bat|cmd|msi|vbs|js|jar|lnk|hta|apk)$").unwrap()
});
static SPLIT_ARCHIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.(r\d{2,3}|\d{3}|z\d{2}|part\d+\.rar)$").unwrap());
static PADDING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(^|[\\/])(\.pad[\\/]|_____padding_file)").unwrap());
static PASSWORD_BAIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(password|passwort|contrase|mot.?de.?passe|how.?to.?(open|watch|play)|read.?me.?first)").unwrap()
});

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn extension(name: &str) -> String {
    name.rfind('.')
        .map(|i| name[i..].to_ascii_lowercase())
        .unwrap_or_default()
}

/// `IsDangerousReleaseTitle`: the release title itself ends in an executable extension.
pub fn dangerous_title(title: &str) -> bool {
    let t = title.trim();
    !t.is_empty() && EXECUTABLE.contains(&extension(file_name(t)).as_str())
}

/// `FakeReleaseInspector.Inspect`. An empty list passes (nothing to judge).
pub fn inspect(files: &[TorrentFile]) -> Option<Fake> {
    let files: Vec<&TorrentFile> = files
        .iter()
        .filter(|f| !PADDING.is_match(&f.path))
        .collect();
    if files.is_empty() {
        return None;
    }
    let fake = |reason, message: String| Some(Fake { reason, message });
    for f in &files {
        let name = file_name(&f.path);
        if DISGUISED.is_match(name) {
            return fake(
                FakeReason::DisguisedExecutable,
                format!("Contains an executable disguised as a media file: '{name}'"),
            );
        }
    }
    for f in &files {
        let name = file_name(&f.path);
        if EXECUTABLE.contains(&extension(name).as_str()) {
            return fake(
                FakeReason::ExecutablePayload,
                format!("Contains an executable or script payload: '{name}'"),
            );
        }
    }
    let has_archive = files.iter().any(|f| {
        let name = file_name(&f.path);
        ARCHIVE.contains(&extension(name).as_str()) || SPLIT_ARCHIVE.is_match(name)
    });
    let has_video = files.iter().any(|f| {
        VIDEO.contains(&extension(file_name(&f.path)).as_str()) && f.length >= MINIMUM_VIDEO_SIZE
    });
    if !has_video
        && has_archive
        && let Some(bait) = files
            .iter()
            .find(|f| PASSWORD_BAIT.is_match(file_name(&f.path)))
    {
        return fake(
            FakeReason::PasswordBait,
            format!(
                "Contains archives with no video alongside '{}', a common password-scam pattern",
                file_name(&bait.path)
            ),
        );
    }
    if !has_video && !has_archive {
        return fake(
            FakeReason::NoUsableContent,
            "Contains no video files and no archives".into(),
        );
    }
    None
}

/// A decoded bencode value (just what file lists need).
enum Bencode<'a> {
    Int(i64),
    Bytes(&'a [u8]),
    List(Vec<Bencode<'a>>),
    Dict(Vec<(&'a [u8], Bencode<'a>)>),
}

fn decode(data: &[u8], pos: usize, depth: usize) -> Result<(Bencode<'_>, usize), String> {
    if depth > 64 {
        return Err("the torrent file is nested too deeply".into());
    }
    match data.get(pos) {
        Some(b'i') => {
            let end = data[pos..]
                .iter()
                .position(|b| *b == b'e')
                .ok_or("unterminated integer")?
                + pos;
            let n = std::str::from_utf8(&data[pos + 1..end])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or("invalid integer")?;
            Ok((Bencode::Int(n), end + 1))
        }
        Some(b'l') => {
            let mut p = pos + 1;
            let mut items = Vec::new();
            while data.get(p) != Some(&b'e') {
                if p >= data.len() {
                    return Err("unterminated list".into());
                }
                let (v, next) = decode(data, p, depth + 1)?;
                items.push(v);
                p = next;
            }
            Ok((Bencode::List(items), p + 1))
        }
        Some(b'd') => {
            let mut p = pos + 1;
            let mut items = Vec::new();
            while data.get(p) != Some(&b'e') {
                if p >= data.len() {
                    return Err("unterminated dictionary".into());
                }
                let (Bencode::Bytes(key), next) = decode(data, p, depth + 1)? else {
                    return Err("dictionary keys must be strings".into());
                };
                let (v, next) = decode(data, next, depth + 1)?;
                items.push((key, v));
                p = next;
            }
            Ok((Bencode::Dict(items), p + 1))
        }
        Some(c) if c.is_ascii_digit() => {
            let colon = data[pos..]
                .iter()
                .position(|b| *b == b':')
                .ok_or("invalid string length")?
                + pos;
            let len: usize = std::str::from_utf8(&data[pos..colon])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or("invalid string length")?;
            let end = colon + 1 + len;
            if end > data.len() {
                return Err("truncated string".into());
            }
            Ok((Bencode::Bytes(&data[colon + 1..end]), end))
        }
        _ => Err(format!("unexpected byte at {pos}")),
    }
}

fn get<'a, 'b>(dict: &'b [(&'a [u8], Bencode<'a>)], key: &[u8]) -> Option<&'b Bencode<'a>> {
    dict.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn text(v: &Bencode<'_>) -> Option<String> {
    match v {
        Bencode::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

/// `TorrentFileInfoReader.GetContentsFromTorrentFile`: every file's path and size.
pub fn torrent_files(data: &[u8]) -> Result<Vec<TorrentFile>, String> {
    let (Bencode::Dict(root), _) = decode(data, 0, 0)? else {
        return Err("not a torrent file".into());
    };
    let Some(Bencode::Dict(info)) = get(&root, b"info") else {
        return Err("the torrent file has no info dictionary".into());
    };
    let name = get(info, b"name.utf-8")
        .or_else(|| get(info, b"name"))
        .and_then(text)
        .unwrap_or_default();
    if let Some(Bencode::List(files)) = get(info, b"files") {
        let mut out = Vec::new();
        for f in files {
            let Bencode::Dict(f) = f else { continue };
            let length = match get(f, b"length") {
                Some(Bencode::Int(n)) => *n,
                _ => 0,
            };
            let parts: Vec<String> = match get(f, b"path.utf-8").or_else(|| get(f, b"path")) {
                Some(Bencode::List(p)) => p.iter().filter_map(text).collect(),
                _ => Vec::new(),
            };
            out.push(TorrentFile {
                path: std::iter::once(name.clone())
                    .chain(parts)
                    .collect::<Vec<_>>()
                    .join("/"),
                length,
            });
        }
        return Ok(out);
    }
    let length = match get(info, b"length") {
        Some(Bencode::Int(n)) => *n,
        _ => 0,
    };
    Ok(vec![TorrentFile { path: name, length }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, mb: i64) -> TorrentFile {
        TorrentFile {
            path: path.into(),
            length: mb * 1024 * 1024,
        }
    }

    /// Cases after Tanukarr's `FakeReleaseInspectorFixture`.
    #[test]
    fn inspector_follows_tanukarr() {
        assert_eq!(inspect(&[]), None);
        assert_eq!(inspect(&[f("Show/Show.S01E01.mkv", 700)]), None);
        assert_eq!(
            inspect(&[f("Show/Show.S01E01.mkv.exe", 1)]).unwrap().reason,
            FakeReason::DisguisedExecutable
        );
        assert_eq!(
            inspect(&[f("Show/Show.S01E01.mkv", 700), f("Show/setup.exe", 1)])
                .unwrap()
                .reason,
            FakeReason::ExecutablePayload
        );
        assert_eq!(
            inspect(&[f("Show/Show.rar", 700), f("Show/PASSWORD.txt", 0)])
                .unwrap()
                .reason,
            FakeReason::PasswordBait
        );
        // Archives without bait pass: their contents cannot be seen.
        assert_eq!(
            inspect(&[
                f("Show/Show.part01.rar", 700),
                f("Show/Show.part02.rar", 700)
            ]),
            None
        );
        assert_eq!(
            inspect(&[f("Show/Show.r00", 50), f("Show/Show.r01", 50)]),
            None
        );
        assert_eq!(
            inspect(&[f("Show/info.txt", 0), f("Show/sample.mkv", 2)])
                .unwrap()
                .reason,
            FakeReason::NoUsableContent
        );
        // Padding files are ignored.
        assert_eq!(
            inspect(&[
                f("Show/Show.mkv", 700),
                f(".pad/123", 0),
                f("Show/_____padding_file_0", 0)
            ]),
            None
        );
        assert!(dangerous_title("Show.S01E01.1080p.mkv.exe"));
        assert!(!dangerous_title("Show.S01E01.1080p.WEB-DL"));
    }

    #[test]
    fn file_lists_are_read_from_torrents() {
        let single = b"d4:infod6:lengthi1024e4:name8:Show.mkv12:piece lengthi16384eee";
        assert_eq!(
            torrent_files(single).unwrap(),
            vec![TorrentFile {
                path: "Show.mkv".into(),
                length: 1024
            }]
        );
        let multi = b"d4:infod5:filesld6:lengthi5e4:pathl3:sub5:a.mkveed6:lengthi7e4:pathl5:b.exeeee4:name4:Showee";
        let files = torrent_files(multi).unwrap();
        assert_eq!(files[0].path, "Show/sub/a.mkv");
        assert_eq!(
            files[1],
            TorrentFile {
                path: "Show/b.exe".into(),
                length: 7
            }
        );
        assert!(torrent_files(b"not bencode").is_err());
    }
}
