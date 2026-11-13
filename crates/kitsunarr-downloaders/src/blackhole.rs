//! Torrent and Usenet Blackhole (Sonarr's `Download/Clients/Blackhole`): grabs are written as
//! `.torrent`, `.magnet` or `.nzb` files to a folder another program watches, and finished
//! downloads are read from a watch folder (`ScanWatchFolder`). A download becomes completed
//! once its files have not changed for the grace period (30 seconds); until then it is shown
//! downloading. Blackholes report no download id at grab time, so their downloads are matched
//! to series by title when imported.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{DownloadError, DownloadItem, ItemStatus};

/// `ScanGracePeriod`.
pub const GRACE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct TorrentSettings {
    pub torrent_folder: String,
    pub watch_folder: String,
    pub save_magnet_files: bool,
    pub magnet_file_extension: String,
    /// `ReadOnly`: the watch folder's files are copied, never moved.
    pub read_only: bool,
    /// `ScanGracePeriod` (30 seconds; `scanGracePeriodSeconds` overrides it).
    pub grace: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsenetSettings {
    pub nzb_folder: String,
    pub watch_folder: String,
    pub grace: Duration,
}

fn grace(map: &Map<String, Value>) -> Duration {
    map.get("scanGracePeriodSeconds")
        .and_then(Value::as_u64)
        .map_or(GRACE, Duration::from_secs)
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn folder(map: &Map<String, Value>, key: &str, label: &str) -> Result<String, String> {
    let f = text(map, key).ok_or_else(|| format!("the {label} is required"))?;
    if !Path::new(&f).is_absolute() {
        return Err(format!("the {label} must be an absolute path"));
    }
    Ok(f)
}

impl TorrentSettings {
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        Ok(Self {
            torrent_folder: folder(map, "torrentFolder", "torrent folder")?,
            watch_folder: folder(map, "watchFolder", "watch folder")?,
            save_magnet_files: map
                .get("saveMagnetFiles")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            magnet_file_extension: text(map, "magnetFileExtension")
                .unwrap_or_else(|| ".magnet".into()),
            read_only: map.get("readOnly").and_then(Value::as_bool).unwrap_or(true),
            grace: grace(map),
        })
    }
}

impl UsenetSettings {
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        Ok(Self {
            nzb_folder: folder(map, "nzbFolder", "NZB folder")?,
            watch_folder: folder(map, "watchFolder", "watch folder")?,
            grace: grace(map),
        })
    }
}

/// `SpecialFolders`: system folders a watch folder scan skips.
const SPECIAL_FOLDERS: &[&str] = &[
    "boot",
    "bootmgr",
    "cache",
    "msocache",
    "recovery",
    "$recycle.bin",
    "recycler",
    "system volume information",
    "temporary internet files",
    "windows",
    ".fseventd",
    ".spotlight",
    ".trashes",
    ".vol",
    "cachedmessages",
    "caches",
    "trash",
    ".@__thumb",
    "@eadir",
    "#recycle",
];

/// `FileNameBuilder.CleanFileName` with the default (smart) colon replacement.
pub fn clean_file_name(name: &str) -> String {
    let s: String = name
        .replace(": ", " - ")
        .replace(':', "-")
        .chars()
        .filter_map(|c| match c {
            '\\' | '/' => Some('+'),
            '?' => Some('!'),
            '*' => Some('-'),
            '<' | '>' | '|' | '"' => None,
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    s.trim_start_matches([' ', '.']).trim_end().to_string()
}

/// .NET ticks of a time (the download id's suffix, as upstream).
fn ticks(t: SystemTime) -> i64 {
    let since = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64 / 100)
        .unwrap_or(0);
    since + 621_355_968_000_000_000
}

fn modified(p: &Path) -> SystemTime {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(UNIX_EPOCH)
}

fn created(p: &Path) -> SystemTime {
    std::fs::metadata(p)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .unwrap_or(UNIX_EPOCH)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.is_dir() => stack.push(p),
                Ok(m) if m.is_file() => out.push(p),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

fn file_hash_data(p: &Path) -> String {
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    format!("{}{}{}", p.display(), ticks(modified(p)), size)
}

/// `GetHash`: the paths, write times and sizes of everything in the download.
fn hash(path: &Path, files: &[PathBuf]) -> String {
    let mut data = path.display().to_string();
    if path.is_dir() {
        data.push_str(&ticks(modified(path)).to_string());
        for f in files {
            data.push_str(&hex::encode(Sha256::digest(file_hash_data(f).as_bytes())));
        }
    } else {
        data = file_hash_data(path);
    }
    hex::encode(Sha256::digest(data.as_bytes()))
}

/// A download in the watch folder (`WatchFolderItem`).
#[derive(Debug, Clone, PartialEq)]
pub struct WatchItem {
    pub download_id: String,
    pub title: String,
    pub output_path: PathBuf,
    pub total_size: i64,
    pub hash: String,
    pub last_changed: SystemTime,
    pub status: ItemStatus,
    pub remaining_seconds: Option<i64>,
}

/// One watch folder's last scan: when, and its downloads by id.
type Scan = (Instant, HashMap<String, WatchItem>);

/// `ScanWatchFolder` with its per-folder cache (kept five minutes).
#[derive(Clone, Default)]
pub struct Blackhole {
    cache: Arc<Mutex<HashMap<String, Scan>>>,
}

impl Blackhole {
    /// The downloads in a watch folder: its folders (not the special ones) and its video files.
    pub fn scan(&self, watch_folder: &str, grace: Duration) -> Vec<WatchItem> {
        let previous: HashMap<String, WatchItem> = {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache
                .get(watch_folder)
                .filter(|(at, _)| at.elapsed() < Duration::from_secs(300))
                .map(|(_, m)| m.clone())
                .unwrap_or_default()
        };
        let mut found: Vec<(PathBuf, String, SystemTime)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(watch_folder) {
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') && !SPECIAL_FOLDERS.contains(&name.to_lowercase().as_str())
                {
                    continue;
                }
                match std::fs::symlink_metadata(&p) {
                    Ok(m) if m.is_dir() => {
                        if !SPECIAL_FOLDERS.contains(&name.to_lowercase().as_str()) {
                            found.push((p.clone(), name, created(&p)));
                        }
                    }
                    Ok(m) if m.is_file() => {
                        let ext = p
                            .extension()
                            .map(|x| format!(".{}", x.to_string_lossy()))
                            .unwrap_or_default();
                        if kitsunarr_parser::extensions::is_video_extension(&ext) {
                            found.push((p.clone(), name, modified(&p)));
                        }
                    }
                    _ => {}
                }
            }
        }
        let now = SystemTime::now();
        let mut items = HashMap::new();
        for (path, name, stamp) in found {
            let mut item = WatchItem {
                download_id: format!("{name}_{}", ticks(stamp)),
                title: clean_file_name(&name),
                output_path: path.clone(),
                total_size: 0,
                hash: String::new(),
                last_changed: now,
                status: ItemStatus::Completed,
                remaining_seconds: Some(0),
            };
            let old = previous.get(&item.download_id);
            // `PreCheckWatchItemExpiry`: unchanged for an hour, the old measure is kept.
            let measure = old.is_none_or(|o| {
                now.duration_since(o.last_changed).unwrap_or_default() < Duration::from_secs(3600)
            });
            if measure {
                let files = if path.is_dir() {
                    files_under(&path)
                } else {
                    vec![path.clone()]
                };
                item.total_size = files
                    .iter()
                    .map(|f| std::fs::metadata(f).map(|m| m.len() as i64).unwrap_or(0))
                    .sum();
                item.hash = hash(&path, &files);
                // `UpdateWatchItemExpiry`.
                item.last_changed = match old {
                    Some(o) if o.hash == item.hash => o.last_changed,
                    _ => now,
                };
                let settled = now.duration_since(item.last_changed).unwrap_or_default();
                if settled < grace {
                    item.status = ItemStatus::Downloading;
                    item.remaining_seconds = Some((grace - settled).as_secs() as i64);
                }
            } else if let Some(o) = old {
                item.total_size = o.total_size;
                item.hash = o.hash.clone();
                item.last_changed = o.last_changed;
            }
            items.insert(item.download_id.clone(), item);
        }
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(watch_folder.to_string(), (Instant::now(), items.clone()));
        let mut list: Vec<WatchItem> = items.into_values().collect();
        list.sort_by(|a, b| a.download_id.cmp(&b.download_id));
        list
    }

    /// `GetItems`: the watch folder's downloads, ids prefixed with the client's kind.
    pub fn items(&self, prefix: &str, watch_folder: &str, grace: Duration) -> Vec<DownloadItem> {
        self.scan(watch_folder, grace)
            .into_iter()
            .map(|w| DownloadItem {
                download_id: format!("{prefix}_{}", w.download_id).to_ascii_uppercase(),
                title: w.title,
                category: "kitsunarr".into(),
                total_size: w.total_size,
                remaining_size: if w.status == ItemStatus::Completed {
                    0
                } else {
                    w.total_size
                },
                remaining_seconds: w.remaining_seconds,
                status: w.status,
                message: None,
                output_path: Some(w.output_path.display().to_string()),
                seed_ratio: 0.0,
                // `RemoveCompletedDownloads` decides.
                can_be_removed: true,
            })
            .collect()
    }

    fn write(dir: &str, name: &str, content: &[u8]) -> Result<(), DownloadError> {
        let path = Path::new(dir).join(name);
        std::fs::write(&path, content)
            .map_err(|e| DownloadError::Client(format!("could not write {}: {e}", path.display())))
    }

    /// `AddFromTorrentFile`, `AddFromMagnetLink` (only with "save magnet files").
    pub fn add_torrent(
        &self,
        s: &TorrentSettings,
        title: &str,
        source: &crate::Source<'_>,
    ) -> Result<String, DownloadError> {
        let title = clean_file_name(title);
        match source {
            crate::Source::File { content, .. } => {
                Self::write(&s.torrent_folder, &format!("{title}.torrent"), content)?;
            }
            crate::Source::Magnet(url) => {
                if !s.save_magnet_files {
                    return Err(DownloadError::Unsupported(
                        "Blackhole does not support magnet links.".into(),
                    ));
                }
                let ext = s.magnet_file_extension.trim_start_matches('.');
                Self::write(&s.torrent_folder, &format!("{title}.{ext}"), url.as_bytes())?;
            }
        }
        Ok(String::new())
    }

    /// `AddFromNzbFile`.
    pub fn add_nzb(
        &self,
        s: &UsenetSettings,
        title: &str,
        content: &[u8],
    ) -> Result<String, DownloadError> {
        let title = clean_file_name(title.trim_end_matches(".nzb"));
        Self::write(&s.nzb_folder, &format!("{title}.nzb"), content)?;
        Ok(String::new())
    }

    /// `RemoveItem`: only with its data (`DeleteItemData`).
    pub fn remove(&self, item: &DownloadItem, delete_data: bool) -> Result<(), DownloadError> {
        if !delete_data {
            return Err(DownloadError::Unsupported(
                "Blackhole cannot remove a download without deleting its data as well.".into(),
            ));
        }
        let Some(path) = item.output_path.as_deref().map(Path::new) else {
            return Ok(());
        };
        let result = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else if path.is_file() {
            std::fs::remove_file(path)
        } else {
            Ok(())
        };
        result
            .map_err(|e| DownloadError::Client(format!("could not delete {}: {e}", path.display())))
    }

    /// `TestFolder`: the folder exists and can be written to.
    pub fn test_folder(path: &str, label: &str) -> Result<(), DownloadError> {
        let dir = Path::new(path);
        if !dir.is_dir() {
            return Err(DownloadError::Client(format!(
                "the {label} {path} does not exist"
            )));
        }
        let probe = dir.join(format!(".kitsunarr-write-test-{}", std::process::id()));
        std::fs::write(&probe, b"")
            .and_then(|()| std::fs::remove_file(&probe))
            .map_err(|e| {
                DownloadError::Client(format!("Kitsunarr cannot write to the {label} {path}: {e}"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_folder_items_settle_after_the_grace_period() {
        let dir = tempfile::tempdir().unwrap();
        let watch = dir.path().join("watch");
        std::fs::create_dir_all(watch.join("Droned.S01E01.720p.WEB-DL")).unwrap();
        std::fs::write(
            watch.join("Droned.S01E01.720p.WEB-DL/episode.mkv"),
            b"12345",
        )
        .unwrap();
        std::fs::write(watch.join("Droned.S01E02.mkv"), b"123").unwrap();
        std::fs::write(watch.join("readme.txt"), b"x").unwrap();
        std::fs::create_dir_all(watch.join("@eaDir")).unwrap();
        let b = Blackhole::default();
        let w = watch.display().to_string();
        let items = b.scan(&w, Duration::from_secs(30));
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(items.iter().all(|i| i.status == ItemStatus::Downloading));
        let folder = items
            .iter()
            .find(|i| i.title == "Droned.S01E01.720p.WEB-DL")
            .unwrap();
        assert_eq!(folder.total_size, 5);
        assert!(folder.download_id.starts_with("Droned.S01E01.720p.WEB-DL_"));
        // Without a grace period (or once it passes unchanged) they are completed.
        let settled = b.scan(&w, Duration::ZERO);
        assert!(settled.iter().all(|i| i.status == ItemStatus::Completed));
        // A change restarts the wait.
        std::fs::write(
            watch.join("Droned.S01E01.720p.WEB-DL/episode.mkv"),
            b"1234567",
        )
        .unwrap();
        let changed = b.scan(&w, Duration::from_secs(30));
        let folder = changed.iter().find(|i| i.total_size == 7).unwrap();
        assert_eq!(folder.status, ItemStatus::Downloading);
        let listed = b.items("TorrentBlackhole", &w, GRACE);
        assert!(
            listed
                .iter()
                .all(|i| i.download_id.starts_with("TORRENTBLACKHOLE_"))
        );
        // Removing deletes the data.
        let file = listed
            .iter()
            .find(|i| i.title == "Droned.S01E02.mkv")
            .unwrap();
        assert!(matches!(
            b.remove(file, false),
            Err(DownloadError::Unsupported(_))
        ));
        b.remove(file, true).unwrap();
        assert!(!watch.join("Droned.S01E02.mkv").exists());
    }

    #[test]
    fn grabs_are_written_to_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let s = TorrentSettings {
            torrent_folder: dir.path().display().to_string(),
            watch_folder: dir.path().display().to_string(),
            save_magnet_files: false,
            magnet_file_extension: ".magnet".into(),
            read_only: true,
            grace: GRACE,
        };
        let b = Blackhole::default();
        let id = b
            .add_torrent(
                &s,
                "Show: The Return S01E01?",
                &crate::Source::File {
                    name: "x.torrent",
                    content: b"d4:infoe",
                },
            )
            .unwrap();
        assert!(id.is_empty());
        assert_eq!(
            std::fs::read(dir.path().join("Show - The Return S01E01!.torrent")).unwrap(),
            b"d4:infoe"
        );
        assert!(matches!(
            b.add_torrent(&s, "Show", &crate::Source::Magnet("magnet:?xt=urn:btih:x")),
            Err(DownloadError::Unsupported(_))
        ));
        let magnets = TorrentSettings {
            save_magnet_files: true,
            magnet_file_extension: "magnet".into(),
            ..s
        };
        b.add_torrent(
            &magnets,
            "Show",
            &crate::Source::Magnet("magnet:?xt=urn:btih:x"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("Show.magnet")).unwrap(),
            "magnet:?xt=urn:btih:x"
        );
        let u = UsenetSettings {
            nzb_folder: dir.path().display().to_string(),
            watch_folder: dir.path().display().to_string(),
            grace: GRACE,
        };
        b.add_nzb(&u, "Show.S01E01.nzb", b"<nzb/>").unwrap();
        assert!(dir.path().join("Show.S01E01.nzb").is_file());
        Blackhole::test_folder(&u.nzb_folder, "NZB folder").unwrap();
        assert!(Blackhole::test_folder("/nonexistent/kitsunarr", "NZB folder").is_err());
        let mut map = Map::new();
        map.insert("torrentFolder".into(), "relative".into());
        map.insert("watchFolder".into(), "/w".into());
        assert!(TorrentSettings::from_map(&map).is_err());
    }
}
