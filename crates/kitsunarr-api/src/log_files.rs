//! Text log files under `<data dir>/logs`, as Sonarr writes them: `kitsunarr.txt`, rolled to
//! `kitsunarr.0.txt`, `kitsunarr.1.txt`… when it reaches 1 MiB, keeping 50 archives. They are
//! listed and read through Sonarr's `log/file` endpoints.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The active file's name.
pub const FILE_NAME: &str = "kitsunarr.txt";
/// Size at which the active file is rolled.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Rolled files kept.
pub const ARCHIVES: usize = 50;

/// The log folder of a data directory.
pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

struct Inner {
    dir: PathBuf,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
    archives: usize,
}

impl Inner {
    fn open(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            std::fs::create_dir_all(&self.dir)?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join(FILE_NAME))?;
            self.size = file.metadata().map(|m| m.len()).unwrap_or(0);
            self.file = Some(file);
        }
        Ok(self.file.as_mut().expect("opened above"))
    }

    /// `kitsunarr.txt` → `kitsunarr.0.txt`, older archives shift up, the oldest is dropped.
    fn roll(&mut self) -> io::Result<()> {
        self.file = None;
        let archive = |n: usize| self.dir.join(format!("kitsunarr.{n}.txt"));
        let _ = std::fs::remove_file(archive(self.archives.saturating_sub(1)));
        for n in (0..self.archives.saturating_sub(1)).rev() {
            if archive(n).exists() {
                std::fs::rename(archive(n), archive(n + 1))?;
            }
        }
        if self.archives > 0 {
            std::fs::rename(self.dir.join(FILE_NAME), archive(0))?;
        } else {
            std::fs::remove_file(self.dir.join(FILE_NAME))?;
        }
        self.size = 0;
        Ok(())
    }
}

/// A size-rolled log file, usable as a `tracing_subscriber` writer (`MakeWriter`).
#[derive(Clone)]
pub struct RollingFile(Arc<Mutex<Inner>>);

impl RollingFile {
    pub fn new(dir: PathBuf) -> Self {
        Self::with_limits(dir, MAX_BYTES, ARCHIVES)
    }

    pub fn with_limits(dir: PathBuf, max_bytes: u64, archives: usize) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            dir,
            file: None,
            size: 0,
            max_bytes,
            archives,
        })))
    }
}

impl Write for RollingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.open()?;
        if inner.size > 0 && inner.size + buf.len() as u64 > inner.max_bytes {
            inner.roll()?;
        }
        let file = inner.open()?;
        file.write_all(buf)?;
        inner.size += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match inner.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RollingFile {
    type Writer = RollingFile;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Whether a name is a log file name Sonarr serves (`[-.a-zA-Z0-9]+\.txt`).
pub fn valid_name(name: &str) -> bool {
    name.len() > 4
        && name.ends_with(".txt")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !name.contains("..")
}

/// The log files of a folder, newest first: (name, last write).
pub fn list(dir: &Path) -> Vec<(String, std::time::SystemTime)> {
    let mut out: Vec<(String, std::time::SystemTime)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let meta = e.metadata().ok()?;
            (meta.is_file() && valid_name(&name))
                .then(|| (name, meta.modified().unwrap_or(std::time::UNIX_EPOCH)))
        })
        .collect();
    out.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_roll_at_the_size_limit_and_keep_the_archive_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = RollingFile::with_limits(dir.path().to_path_buf(), 10, 2);
        for line in ["aaaaaaaa\n", "bbbbbbbb\n", "cccccccc\n", "dddddddd\n"] {
            w.write_all(line.as_bytes()).unwrap();
        }
        w.flush().unwrap();
        let read = |n: &str| std::fs::read_to_string(dir.path().join(n)).unwrap();
        assert_eq!(read(FILE_NAME), "dddddddd\n");
        assert_eq!(read("kitsunarr.0.txt"), "cccccccc\n");
        assert_eq!(read("kitsunarr.1.txt"), "bbbbbbbb\n");
        assert!(!dir.path().join("kitsunarr.2.txt").exists());
        assert_eq!(list(dir.path()).len(), 3);
        assert!(valid_name("kitsunarr.0.txt"));
        assert!(!valid_name("../etc.txt") && !valid_name("x.log") && !valid_name("a/b.txt"));
    }
}
