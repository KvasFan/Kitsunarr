//! Reading Sonarr/Tanukarr backups: a `.zip` produced by the application (containing
//! `sonarr.db`, `config.xml`, `INFO`) or a bare `sonarr.db`.
//!
//! Extraction never uses archive entry names as paths (no zip-slip), enforces size limits
//! while decompressing (zip bombs can lie about sizes), validates the SQLite header, and opens
//! the database read-only and immutable.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Column, ConnectOptions, Row, TypeInfo, ValueRef};

use crate::MigrateError;
use crate::model::{RawSource, SourceFormat};
use crate::values::Rec;

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const MAX_DB_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_SMALL_FILE_BYTES: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

/// Backup contents extracted into a private temporary directory (removed on drop).
pub struct ExtractedBackup {
    pub dir: tempfile::TempDir,
    pub db_path: PathBuf,
    pub config_xml: Option<String>,
    pub info: Option<String>,
}

fn copy_limited(mut reader: impl Read, dest: &Path, limit: u64) -> Result<u64, MigrateError> {
    let mut out = std::fs::File::create(dest)?;
    let written = std::io::copy(&mut reader.by_ref().take(limit + 1), &mut out)?;
    if written > limit {
        return Err(MigrateError::InvalidBackup(format!(
            "archive entry exceeds the {limit}-byte limit"
        )));
    }
    out.flush()?;
    Ok(written)
}

fn read_small(reader: impl Read) -> Result<String, MigrateError> {
    let mut buf = Vec::new();
    reader
        .take(MAX_SMALL_FILE_BYTES + 1)
        .read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_SMALL_FILE_BYTES {
        return Err(MigrateError::InvalidBackup(
            "configuration file is unexpectedly large".into(),
        ));
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn check_sqlite(path: &Path) -> Result<(), MigrateError> {
    let mut header = [0u8; 16];
    let mut f = std::fs::File::open(path)?;
    if f.read_exact(&mut header).is_err() || &header != SQLITE_MAGIC {
        return Err(MigrateError::InvalidBackup(
            "the database file is not a SQLite database".into(),
        ));
    }
    Ok(())
}

/// Extracts a backup archive (or copies a bare database) into a new temporary directory.
pub fn extract(upload: &Path, work_root: &Path) -> Result<ExtractedBackup, MigrateError> {
    std::fs::create_dir_all(work_root)?;
    let dir = tempfile::Builder::new()
        .prefix("kitsunarr-migration-")
        .tempdir_in(work_root)?;
    let db_path = dir.path().join("source.db");

    let mut magic = [0u8; 16];
    let n = std::fs::File::open(upload)?.read(&mut magic)?;
    if n == 16 && &magic == SQLITE_MAGIC {
        copy_limited(std::fs::File::open(upload)?, &db_path, MAX_DB_BYTES)?;
        return Ok(ExtractedBackup {
            dir,
            db_path,
            config_xml: None,
            info: None,
        });
    }
    if n < 4 || &magic[..4] != b"PK\x03\x04" {
        return Err(MigrateError::InvalidBackup(
            "expected a Sonarr/Tanukarr backup .zip or a sonarr.db file".into(),
        ));
    }

    let mut archive = zip::ZipArchive::new(std::fs::File::open(upload)?)
        .map_err(|e| MigrateError::InvalidBackup(format!("unreadable zip archive: {e}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(MigrateError::InvalidBackup(
            "archive has too many entries".into(),
        ));
    }
    let (mut found_db, mut config_xml, mut info) = (false, None, None);
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| MigrateError::InvalidBackup(format!("corrupt archive entry: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|e| MigrateError::InvalidBackup(e.to_string()))?
            .to_string();
        // Only the final path component is considered, and only known names are extracted.
        let base = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match base.as_str() {
            "sonarr.db" | "nzbdrone.db" if !found_db => {
                copy_limited(entry, &db_path, MAX_DB_BYTES)?;
                found_db = true;
            }
            "config.xml" => config_xml = Some(read_small(entry)?),
            "info" => info = Some(read_small(entry)?),
            _ => {}
        }
    }
    if !found_db {
        return Err(MigrateError::InvalidBackup(
            "the archive contains no sonarr.db (PostgreSQL installations do not include the database in backups)".into(),
        ));
    }
    check_sqlite(&db_path)?;
    Ok(ExtractedBackup {
        dir,
        db_path,
        config_xml,
        info,
    })
}

/// Simple `<Config><Key>value</Key>...</Config>` reader.
pub fn parse_config_xml(xml: &str) -> HashMap<String, String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = HashMap::new();
    let mut current: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => current = Some(AsRef::<str>::as_ref(&e.name()).to_string()),
            Ok(Event::Text(t)) => {
                if let Some(key) = current.as_ref() {
                    let raw: &str = &t;
                    let text = quick_xml::escape::unescape(raw)
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| raw.to_string());
                    out.insert(key.clone(), text.trim().to_string());
                }
            }
            Ok(Event::End(_)) => current = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out.remove("Config");
    out
}

/// Metadata about the source schema.
#[derive(Debug, Clone)]
pub struct DbMeta {
    pub max_version: Option<i64>,
    pub migrations: Vec<(i64, String)>,
    pub tables: Vec<String>,
    pub is_tanukarr: bool,
}

/// Tables read from a source database: (output name, candidate table names oldest-last).
const TABLES: &[(&str, &[&str])] = &[
    ("series", &["Series"]),
    ("episodes", &["Episodes"]),
    ("episodefiles", &["EpisodeFiles"]),
    ("qualityprofiles", &["QualityProfiles", "Profiles"]),
    ("qualitydefinitions", &["QualityDefinitions"]),
    ("customformats", &["CustomFormats"]),
    ("delayprofiles", &["DelayProfiles"]),
    ("releaseprofiles", &["ReleaseProfiles", "Restrictions"]),
    ("indexers", &["Indexers"]),
    ("downloadclients", &["DownloadClients"]),
    ("notifications", &["Notifications"]),
    ("importlists", &["ImportLists"]),
    ("importlistexclusions", &["ImportListExclusions"]),
    ("tags", &["Tags"]),
    ("rootfolders", &["RootFolders"]),
    ("namingconfig", &["NamingConfig"]),
    ("remotepathmappings", &["RemotePathMappings"]),
    ("history", &["History"]),
    ("scenemappings", &["SceneMappings"]),
];

fn row_to_rec(row: &sqlx::sqlite::SqliteRow) -> Rec {
    let mut map = Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        let value = match row.try_get_raw(i) {
            Ok(raw) if raw.is_null() => Value::Null,
            Ok(raw) => match raw.type_info().name() {
                "INTEGER" => row
                    .try_get::<i64, _>(i)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "REAL" => row
                    .try_get::<f64, _>(i)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "BLOB" => Value::Null,
                _ => row
                    .try_get::<String, _>(i)
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            },
            Err(_) => Value::Null,
        };
        map.insert(col.name().to_string(), value);
    }
    Rec::from_map(map)
}

/// Reads every relevant table of a source database (read-only, immutable).
pub async fn read_database(path: &Path) -> Result<(RawSource, DbMeta), MigrateError> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(true);
    let mut conn = opts
        .connect()
        .await
        .map_err(|e| MigrateError::InvalidBackup(format!("cannot open the database: {e}")))?;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&mut conn)
            .await
            .map_err(|e| MigrateError::InvalidBackup(format!("cannot read the schema: {e}")))?;
    if !tables.iter().any(|t| t == "Series") {
        return Err(MigrateError::InvalidBackup(
            "the database has no Series table; is this a Sonarr or Tanukarr database?".into(),
        ));
    }
    let migrations: Vec<(i64, String)> = if tables.iter().any(|t| t == "VersionInfo") {
        sqlx::query_as(
            "SELECT Version, COALESCE(Description, '') FROM VersionInfo ORDER BY Version",
        )
        .fetch_all(&mut conn)
        .await
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    let series_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('Series')")
            .fetch_all(&mut conn)
            .await
            .unwrap_or_default();
    let is_tanukarr = series_columns
        .iter()
        .any(|c| c == "MetadataSource" || c == "ForeignId");

    let mut out: HashMap<String, Vec<Rec>> = HashMap::new();
    for (name, candidates) in TABLES {
        let Some(table) = candidates.iter().find(|c| tables.iter().any(|t| t == *c)) else {
            continue;
        };
        // Table names come from the static list above.
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT * FROM \"{table}\"")))
            .fetch_all(&mut conn)
            .await
            .map_err(|e| MigrateError::InvalidBackup(format!("cannot read table {table}: {e}")))?;
        out.insert((*name).to_string(), rows.iter().map(row_to_rec).collect());
    }
    if tables.iter().any(|t| t == "Config") {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as("SELECT Key, Value FROM Config")
            .fetch_all(&mut conn)
            .await
            .unwrap_or_default();
        let cfg: Map<String, Value> = rows
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_ascii_lowercase(),
                    v.map(Value::String).unwrap_or(Value::Null),
                )
            })
            .collect();
        out.insert("config".into(), vec![Rec(cfg)]);
    }
    let meta = DbMeta {
        max_version: migrations.last().map(|m| m.0),
        migrations,
        tables,
        is_tanukarr,
    };
    Ok((
        RawSource {
            format: SourceFormat::Database,
            tables: out,
        },
        meta,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_xml_parsing() {
        let cfg = parse_config_xml(
            "<Config>\n  <Port>8989</Port>\n  <ApiKey>abc123</ApiKey>\n  <UrlBase></UrlBase>\n  <InstanceName>Sonarr</InstanceName>\n</Config>",
        );
        assert_eq!(cfg.get("ApiKey").map(String::as_str), Some("abc123"));
        assert_eq!(cfg.get("InstanceName").map(String::as_str), Some("Sonarr"));
        assert!(!cfg.contains_key("Config"));
    }

    #[test]
    fn rejects_non_backups_and_traversal_names_are_harmless() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.zip");
        std::fs::write(&junk, b"not a zip at all").unwrap();
        assert!(matches!(
            extract(&junk, dir.path()),
            Err(MigrateError::InvalidBackup(_))
        ));

        // A zip whose database entry uses a traversal path is extracted under a fixed name.
        let evil = dir.path().join("evil.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&evil).unwrap());
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            w.start_file("../../../../tmp/sonarr.db", opts).unwrap();
            w.write_all(b"SQLite format 3\0rest-of-a-fake-db").unwrap();
            w.finish().unwrap();
        }
        let out = extract(&evil, dir.path()).unwrap();
        assert!(out.db_path.starts_with(out.dir.path()));
        assert_eq!(out.db_path.file_name().unwrap(), "source.db");
    }
}
