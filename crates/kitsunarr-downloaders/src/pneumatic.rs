//! Pneumatic (Sonarr's `Pneumatic`): the NZB is saved to a folder Kodi's Pneumatic add-on
//! reads, and a `.strm` file pointing at it is written to the STRM folder; those `.strm`
//! files are the downloads, imported as they are. Full-season releases are refused, and
//! downloads cannot be removed.

use std::path::Path;
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value};

use crate::blackhole::clean_file_name;
use crate::{DownloadError, DownloadItem, ItemStatus};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub nzb_folder: String,
    pub strm_folder: String,
}

impl Settings {
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let folder = |k: &str, label: &str| -> Result<String, String> {
            let f = map
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("the {label} is required"))?;
            if !Path::new(f).is_absolute() {
                return Err(format!("the {label} must be an absolute path"));
            }
            Ok(f.to_string())
        };
        Ok(Self {
            nzb_folder: folder("nzbFolder", "NZB folder")?,
            strm_folder: folder("strmFolder", "STRM folder")?,
        })
    }
}

fn ticks(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64 / 100)
        .unwrap_or(0)
        + 621_355_968_000_000_000
}

/// `GetDownloadClientId`: the file name and its write time.
fn id(file: &Path) -> String {
    format!(
        "PNEUMATIC_{}_{}",
        file.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ticks(file)
    )
    .to_ascii_uppercase()
}

/// `Download`: the NZB and its `.strm` file.
pub fn add(s: &Settings, name: &str, content: &[u8]) -> Result<String, DownloadError> {
    let title = clean_file_name(name.trim_end_matches(".nzb"));
    if kitsunarr_parser::parse_title(&title).is_some_and(|p| p.full_season) {
        return Err(DownloadError::Unsupported(
            "Full season releases are not supported with Pneumatic.".into(),
        ));
    }
    let nzb = Path::new(&s.nzb_folder).join(format!("{title}.nzb"));
    std::fs::write(&nzb, content)
        .map_err(|e| DownloadError::Client(format!("could not write {}: {e}", nzb.display())))?;
    let strm = Path::new(&s.strm_folder).join(format!("{title}.strm"));
    std::fs::write(
        &strm,
        format!(
            "plugin://plugin.program.pneumatic/?mode=strm&type=add_file&nzb={}&nzbname={title}",
            nzb.display()
        ),
    )
    .map_err(|e| DownloadError::Client(format!("could not write {}: {e}", strm.display())))?;
    Ok(id(&strm))
}

/// `GetItems`: the `.strm` files, completed.
pub fn items(s: &Settings) -> Vec<DownloadItem> {
    let mut files: Vec<_> = std::fs::read_dir(&s.strm_folder)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "strm"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
        .iter()
        .map(|f| DownloadItem {
            download_id: id(f),
            title: clean_file_name(
                &f.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            category: String::new(),
            total_size: std::fs::metadata(f).map(|m| m.len() as i64).unwrap_or(0),
            remaining_size: 0,
            remaining_seconds: Some(0),
            status: ItemStatus::Completed,
            message: None,
            output_path: Some(f.display().to_string()),
            seed_ratio: 0.0,
            can_be_removed: true,
            seed_progress: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nzb_and_strm_files() {
        let dir = tempfile::tempdir().unwrap();
        let (nzbs, strms) = (dir.path().join("nzb"), dir.path().join("strm"));
        std::fs::create_dir_all(&nzbs).unwrap();
        std::fs::create_dir_all(&strms).unwrap();
        let s = Settings {
            nzb_folder: nzbs.display().to_string(),
            strm_folder: strms.display().to_string(),
        };
        let id = add(&s, "Show.S01E01.720p.HDTV-GRP.nzb", b"<nzb/>").unwrap();
        assert!(id.starts_with("PNEUMATIC_SHOW.S01E01.720P.HDTV-GRP.STRM_"));
        assert_eq!(
            std::fs::read(nzbs.join("Show.S01E01.720p.HDTV-GRP.nzb")).unwrap(),
            b"<nzb/>"
        );
        let strm = std::fs::read_to_string(strms.join("Show.S01E01.720p.HDTV-GRP.strm")).unwrap();
        assert!(
            strm.starts_with("plugin://plugin.program.pneumatic/?mode=strm&type=add_file&nzb=")
        );
        assert!(strm.ends_with("&nzbname=Show.S01E01.720p.HDTV-GRP"));
        let listed = items(&s);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].download_id, id, "the grab's id tracks the item");
        assert_eq!(listed[0].status, ItemStatus::Completed);
        assert!(matches!(
            add(&s, "Show.S01.720p.HDTV-GRP", b"<nzb/>"),
            Err(DownloadError::Unsupported(_))
        ));
    }
}
