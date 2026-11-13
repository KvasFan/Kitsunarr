//! Sonarr's other health checks (`HealthCheck/Checks`), with upstream's messages:
//! `IndexerRssCheck`, `IndexerSearchCheck`, `IndexerLongTermStatusCheck`,
//! `IndexerJackettAllCheck`, `IndexerDownloadClientCheck`, `ImportMechanismCheck`,
//! `ImportListStatusCheck`, `ImportListRootFolderCheck`, `RecyclingBinCheck`, `BackupCheck`,
//! `ApiKeyValidationCheck`, `RemovedSeriesCheck` and `MountCheck`.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_domain::api::settings::{
    DownloadClientOptionsSettings, MediaManagementSettings, ProviderKind,
};
use kitsunarr_domain::api::system::{HealthCheck, HealthLevel};

use crate::AppState;
use crate::error::ApiResult;

fn check(source: &str, level: HealthLevel, message: String) -> HealthCheck {
    HealthCheck {
        source: source.into(),
        level,
        message,
    }
}

fn option(p: &ProviderRecord, key: &str) -> bool {
    p.options
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

/// `IndexerJackettAllCheck`: Torznab indexers pointing at Jackett's aggregate endpoint.
pub fn uses_jackett_all(p: &ProviderRecord) -> bool {
    if p.implementation != "Torznab" {
        return false;
    }
    let field = |k: &str| {
        p.settings
            .get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let (base, api) = (field("baseUrl"), field("apiPath"));
    ["/torznab/all/api", "/api/v2.0/indexers/all/results/torznab"]
        .iter()
        .any(|needle| base.contains(needle) || api.contains(needle))
}

/// The mount (from `/proc/mounts`) holding a path, and whether it is read-only.
pub fn read_only_mount(mounts: &str, path: &Path) -> Option<String> {
    let mut best: Option<(&str, bool)> = None;
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let (Some(_dev), Some(point), Some(_fs), Some(opts)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let point_path = Path::new(point);
        if path.starts_with(point_path) && best.is_none_or(|(b, _)| point.len() > b.len()) {
            best = Some((point, opts.split(',').any(|o| o == "ro")));
        }
    }
    best.filter(|(_, ro)| *ro).map(|(p, _)| p.to_string())
}

fn writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".kitsunarr-health-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn parse(ts: Option<&str>) -> Option<DateTime<Utc>> {
    ts.and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// The checks above, appended to `out`.
pub async fn more_checks(state: &AppState, out: &mut Vec<HealthCheck>) -> ApiResult<()> {
    let pool = state.db.pool();
    let now = Utc::now();
    let indexers: Vec<ProviderRecord> = providers::list(pool, ProviderKind::Indexer)
        .await?
        .into_iter()
        .filter(|p| p.enabled)
        .collect();
    let blocked = kitsunarr_db::indexer_status::blocked(pool, now).await?;
    let available = |p: &&ProviderRecord| !blocked.iter().any(|b| b.indexer_id == p.id);

    // `IndexerRssCheck` and `IndexerSearchCheck` (only once some indexer exists).
    if !indexers.is_empty() {
        let rss: Vec<_> = indexers.iter().filter(|p| option(p, "enableRss")).collect();
        if rss.is_empty() {
            out.push(check("IndexerRssCheck", HealthLevel::Error,
                "No indexers available with RSS sync enabled, Kitsunarr will not grab new releases automatically".into()));
        } else if !rss.iter().any(available) {
            out.push(check(
                "IndexerRssCheck",
                HealthLevel::Warning,
                "All rss-capable indexers are temporarily unavailable due to recent indexer errors"
                    .into(),
            ));
        }
        let automatic: Vec<_> = indexers
            .iter()
            .filter(|p| option(p, "enableAutomaticSearch"))
            .collect();
        if automatic.is_empty() {
            out.push(check("IndexerSearchCheck", HealthLevel::Warning,
                "No indexers available with Automatic Search enabled, Kitsunarr will not provide any automatic search results".into()));
        } else if !indexers
            .iter()
            .any(|p| option(p, "enableInteractiveSearch"))
        {
            out.push(check("IndexerSearchCheck", HealthLevel::Warning,
                "No indexers available with Interactive Search enabled, Kitsunarr will not provide any interactive search results".into()));
        } else if !automatic.iter().any(available) {
            out.push(check("IndexerSearchCheck", HealthLevel::Warning,
                "All search-capable indexers are temporarily unavailable due to recent indexer errors".into()));
        }
    }

    // `IndexerLongTermStatusCheck`: failing for more than six hours.
    let long_term: Vec<&str> = indexers
        .iter()
        .filter(|p| {
            blocked.iter().any(|b| {
                b.indexer_id == p.id
                    && parse(b.initial_failure.as_deref())
                        .is_some_and(|f| f < now - Duration::hours(6))
            })
        })
        .map(|p| p.name.as_str())
        .collect();
    if !long_term.is_empty() {
        out.push(if long_term.len() == indexers.len() {
            check(
                "IndexerLongTermStatusCheck",
                HealthLevel::Error,
                "All indexers are unavailable due to failures for more than 6 hours".into(),
            )
        } else {
            check(
                "IndexerLongTermStatusCheck",
                HealthLevel::Warning,
                format!(
                    "Indexers unavailable due to failures for more than 6 hours: {}",
                    long_term.join(", ")
                ),
            )
        });
    }

    // `IndexerJackettAllCheck`.
    let jackett: Vec<&str> = indexers
        .iter()
        .filter(|p| uses_jackett_all(p))
        .map(|p| p.name.as_str())
        .collect();
    if !jackett.is_empty() {
        out.push(check(
            "IndexerJackettAllCheck",
            HealthLevel::Warning,
            format!(
                "Indexers using the unsupported Jackett 'all' endpoint: {}",
                jackett.join(", ")
            ),
        ));
    }

    // `IndexerDownloadClientCheck`: an indexer tied to a client that no longer exists.
    let clients = providers::list(pool, ProviderKind::DownloadClient).await?;
    let invalid: Vec<&str> = indexers
        .iter()
        .filter(|p| {
            p.options
                .get("downloadClientId")
                .and_then(serde_json::Value::as_i64)
                .is_some_and(|id| id > 0 && !clients.iter().any(|c| c.id == id))
        })
        .map(|p| p.name.as_str())
        .collect();
    if !invalid.is_empty() {
        out.push(check(
            "IndexerDownloadClientCheck",
            HealthLevel::Warning,
            format!(
                "Indexers with invalid download clients: {}.",
                invalid.join(", ")
            ),
        ));
    }

    // `ImportMechanismCheck`.
    let dco: DownloadClientOptionsSettings = kitsunarr_db::settings::get_or_default(
        pool,
        kitsunarr_db::settings::KEY_DOWNLOAD_CLIENT_OPTIONS,
    )
    .await?;
    if !dco.enable_completed_download_handling && clients.iter().any(|c| c.enabled) {
        out.push(check(
            "ImportMechanismCheck",
            HealthLevel::Error,
            "Enable Completed Download Handling".into(),
        ));
    }

    // `ImportListStatusCheck` and `ImportListRootFolderCheck`.
    let lists: Vec<ProviderRecord> = providers::list(pool, ProviderKind::ImportList)
        .await?
        .into_iter()
        .filter(|p| p.enabled)
        .collect();
    let statuses = kitsunarr_db::import_lists::all_status(pool).await?;
    let failing: Vec<&str> = lists
        .iter()
        .filter(|l| {
            statuses
                .iter()
                .any(|s| s.import_list_id == l.id && s.disabled_till().is_some_and(|d| d > now))
        })
        .map(|l| l.name.as_str())
        .collect();
    if !failing.is_empty() {
        out.push(if failing.len() == lists.len() {
            check(
                "ImportListStatusCheck",
                HealthLevel::Error,
                "All lists are unavailable due to failures".into(),
            )
        } else {
            check(
                "ImportListStatusCheck",
                HealthLevel::Warning,
                format!("Lists unavailable due to failures: {}", failing.join(", ")),
            )
        });
    }
    let mut missing_roots: Vec<String> = Vec::new();
    for l in &lists {
        if let Some(root) = l
            .options
            .get("rootFolderPath")
            .and_then(serde_json::Value::as_str)
            .filter(|r| !r.trim().is_empty())
            && !Path::new(root).is_dir()
        {
            missing_roots.push(format!("{root} ({})", l.name));
        }
    }
    if !missing_roots.is_empty() {
        out.push(check(
            "ImportListRootFolderCheck",
            HealthLevel::Warning,
            if missing_roots.len() == 1 {
                format!(
                    "Missing root folder for import list(s): {}",
                    missing_roots[0]
                )
            } else {
                format!(
                    "Multiple root folders are missing for import lists: {}",
                    missing_roots.join(" | ")
                )
            },
        ));
    }

    // `RecyclingBinCheck`.
    let mm: MediaManagementSettings =
        kitsunarr_db::settings::get_or_default(pool, kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT)
            .await?;
    if let Some(bin) = mm.recycle_bin.as_deref().filter(|b| !b.trim().is_empty())
        && !writable(Path::new(bin))
    {
        out.push(check("RecyclingBinCheck", HealthLevel::Error, format!(
            "Unable to write to configured recycling bin folder: {bin}. Ensure this path exists and is writable by the user running Kitsunarr")));
    }

    // `BackupCheck`: the backup folder must be writable (it is created when missing).
    let backups = state.config.backup_dir();
    if (backups.exists() || std::fs::create_dir_all(&backups).is_ok()) && !writable(&backups) {
        out.push(check(
            "BackupCheck",
            HealthLevel::Error,
            format!(
                "Unable to write to the backup folder {}; scheduled backups will fail.",
                backups.display()
            ),
        ));
    }

    // `ApiKeyValidationCheck`.
    let key_len = state.api_key.read().map(|k| k.len()).unwrap_or_default();
    if key_len < 20 {
        out.push(check("ApiKeyValidationCheck", HealthLevel::Warning,
            "Please update your API key to be at least 20 characters long. You can do this via settings or the config file".into()));
    }

    // `RemovedSeriesCheck`: series the catalogue reports as deleted.
    let removed: Vec<(String, String, String)> = kitsunarr_db::sqlx::query_as(
        "SELECT title, metadata_provider, metadata_id FROM series WHERE status = 'deleted' ORDER BY sort_title",
    )
    .fetch_all(pool)
    .await
    .map_err(kitsunarr_db::DbError::from)?;
    if !removed.is_empty() {
        let text = removed
            .iter()
            .map(|(t, p, id)| {
                if p == "tvdb" {
                    format!("{t} (tvdbid {id})")
                } else {
                    format!("{t} ({p} {id})")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let catalogue = if removed.iter().all(|(_, p, _)| p == "tvdb") {
            "TheTVDB"
        } else {
            "its catalogue"
        };
        out.push(check(
            "RemovedSeriesCheck",
            HealthLevel::Error,
            if removed.len() == 1 {
                format!("Series {text} was removed from {catalogue}")
            } else {
                format!("Series {text} were removed from {catalogue}")
            },
        ));
    }

    // `MountCheck`: series folders on read-only mounts.
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        let paths: Vec<String> = kitsunarr_db::sqlx::query_scalar("SELECT path FROM series")
            .fetch_all(pool)
            .await
            .map_err(kitsunarr_db::DbError::from)?;
        let mut ro: Vec<String> = paths
            .iter()
            .filter_map(|p| read_only_mount(&mounts, Path::new(p)))
            .collect();
        ro.sort();
        ro.dedup();
        if !ro.is_empty() {
            out.push(check(
                "MountCheck",
                HealthLevel::Error,
                format!(
                    "Mount containing a series path is mounted read-only: {}",
                    ro.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn torznab(base: &str, api: &str) -> ProviderRecord {
        ProviderRecord {
            id: 1,
            kind: ProviderKind::Indexer,
            name: "J".into(),
            implementation: "Torznab".into(),
            protocol: kitsunarr_domain::DownloadProtocol::Torrent,
            enabled: true,
            priority: 25,
            tags: vec![],
            options: serde_json::Map::new(),
            settings: json!({"baseUrl": base, "apiPath": api})
                .as_object()
                .cloned()
                .unwrap(),
        }
    }

    #[test]
    fn jackett_all_endpoints_are_recognised() {
        // Sonarr's `IndexerJackettAllCheckFixture` cases.
        assert!(uses_jackett_all(&torznab(
            "http://localhost:9117/torznab/all/api",
            "/api"
        )));
        assert!(uses_jackett_all(&torznab(
            "http://localhost:9117/",
            "/api/v2.0/indexers/all/results/torznab"
        )));
        assert!(uses_jackett_all(&torznab(
            "http://localhost:9117/API/V2.0/INDEXERS/ALL/RESULTS/TORZNAB",
            "/api"
        )));
        assert!(!uses_jackett_all(&torznab(
            "http://localhost:9117/torznab/nyaa/api",
            "/api"
        )));
        assert!(!uses_jackett_all(&torznab(
            "http://localhost:9117/api/v2.0/indexers/nyaa/results/torznab",
            "/api"
        )));
    }

    #[test]
    fn read_only_mounts() {
        let mounts = "/dev/sda1 / ext4 rw,relatime 0 0\n\
                      /dev/sdb1 /media ext4 rw 0 0\n\
                      /dev/sdc1 /media/archive ext4 ro,relatime 0 0\n";
        assert_eq!(
            read_only_mount(mounts, Path::new("/media/archive/tv/Show")).as_deref(),
            Some("/media/archive")
        );
        assert_eq!(read_only_mount(mounts, Path::new("/media/tv/Show")), None);
        assert_eq!(read_only_mount(mounts, Path::new("/srv/x")), None);
    }
}
