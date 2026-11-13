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

    download_client_checks(state, &clients, dco.enable_completed_download_handling, out).await?;

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

/// Client statuses, kept five minutes so the health page does not wait on every client.
type StatusCache = std::sync::Mutex<
    std::collections::HashMap<
        String,
        (
            std::time::Instant,
            Option<kitsunarr_downloaders::ClientStatus>,
        ),
    >,
>;
static CLIENT_STATUS: std::sync::LazyLock<StatusCache> = std::sync::LazyLock::new(Default::default);

async fn client_status(
    state: &AppState,
    client: &ProviderRecord,
) -> Option<kitsunarr_downloaders::ClientStatus> {
    let key = format!(
        "{}:{}:{}",
        client.id,
        client.implementation,
        serde_json::Value::Object(client.settings.clone())
    );
    if let Some((at, status)) = CLIENT_STATUS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        && at.elapsed() < std::time::Duration::from_secs(300)
    {
        return status.clone();
    }
    let settings = kitsunarr_downloaders::ClientSettings::from_provider(
        &client.implementation,
        &client.settings,
    )
    .ok()?;
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.downloads.clients.status(&settings),
    )
    .await
    .ok()
    .and_then(Result::ok);
    CLIENT_STATUS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, (std::time::Instant::now(), status.clone()));
    status
}

/// Whether a path is valid on this system (Kitsunarr runs on Unix: an absolute Unix path).
fn valid_here(path: &str) -> bool {
    path.starts_with('/') && !path.contains('\\')
}

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|r| {
            r.lines()
                .find_map(|l| l.strip_prefix("ID="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

fn in_docker() -> bool {
    Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup").is_ok_and(|c| c.contains("/docker/"))
}

/// What a client's reported folders say, as Sonarr's checks word it (one problem per check).
#[derive(Debug, Default, PartialEq)]
pub struct ClientFolderProblems {
    pub removes_completed: Option<String>,
    pub sorting: Option<String>,
    pub root_folder: Option<String>,
    pub remote_path: Option<String>,
}

/// Where the folder checks run: completed download handling on, inside a container, and the
/// system's name for messages.
pub struct FolderCheckEnv {
    pub completed_handling: bool,
    pub docker: bool,
    pub os: String,
}

/// `DownloadClientRemovesCompletedDownloadsCheck`, `DownloadClientSortingCheck`,
/// `DownloadClientRootFolderCheck` and `RemotePathMappingCheck` for one client, its status
/// folders already mapped to local paths.
pub fn client_folder_problems(
    name: &str,
    status: &kitsunarr_downloaders::ClientStatus,
    local_folders: &[String],
    root_folders: &[String],
    env: &FolderCheckEnv,
    exists: impl Fn(&str) -> bool,
) -> ClientFolderProblems {
    let (completed_handling, docker, os) = (env.completed_handling, env.docker, env.os.as_str());
    let mut p = ClientFolderProblems::default();
    if status.removes_completed_downloads {
        p.removes_completed = Some(format!(
            "Download client {name} is set to remove completed downloads. This can result in downloads being removed from your client before Kitsunarr can import them."
        ));
    }
    if let Some(mode) = status
        .sorting_mode
        .as_deref()
        .filter(|m| !m.trim().is_empty())
    {
        p.sorting = Some(format!(
            "Download client {name} has {mode} sorting enabled for Kitsunarr's category. You should disable sorting in your download client to avoid import issues."
        ));
    }
    let trim = |x: &str| x.trim_end_matches('/').to_string();
    if let Some(root) = root_folders.iter().find(|r| {
        local_folders.iter().any(|f| {
            let (r, f) = (trim(r), trim(f));
            f == r || f.starts_with(&format!("{r}/"))
        })
    }) {
        p.root_folder = Some(format!(
            "Download client {name} places downloads in the root folder {root}. You should not download to a root folder."
        ));
    }
    if completed_handling {
        for path in local_folders {
            let message = if !valid_here(path) {
                if !status.is_localhost {
                    format!(
                        "Remote download client {name} places downloads in {path} but this is not a valid {os} path. Review your remote path mappings and download client settings."
                    )
                } else if docker {
                    format!(
                        "You are using docker; download client {name} places downloads in {path} but this is not a valid {os} path. Review your remote path mappings and download client settings."
                    )
                } else {
                    format!(
                        "Local download client {name} places downloads in {path} but this is not a valid {os} path. Review your download client settings."
                    )
                }
            } else if !exists(path) {
                if docker {
                    format!(
                        "You are using docker; download client {name} places downloads in {path} but this directory does not appear to exist inside the container. Review your remote path mappings and container volume settings."
                    )
                } else if !status.is_localhost {
                    format!(
                        "Remote download client {name} places downloads in {path} but this directory does not appear to exist. Likely missing or incorrect remote path mapping."
                    )
                } else {
                    format!(
                        "Download client {name} places downloads in {path} but Kitsunarr cannot see this directory. You may need to adjust the folder's permissions."
                    )
                }
            } else {
                continue;
            };
            p.remote_path = Some(message);
            break;
        }
    }
    p
}

/// The checks that need each client's status (`GetStatus`), for enabled clients that are not
/// backing off after failures.
async fn download_client_checks(
    state: &AppState,
    clients: &[ProviderRecord],
    completed_handling: bool,
    out: &mut Vec<HealthCheck>,
) -> ApiResult<()> {
    let pool = state.db.pool();
    let blocked = kitsunarr_db::indexer_status::blocked_of(
        kitsunarr_db::indexer_status::Kind::DownloadClient,
        pool,
        Utc::now(),
    )
    .await?;
    let usable: Vec<&ProviderRecord> = clients
        .iter()
        .filter(|c| {
            c.enabled
                && kitsunarr_downloaders::supported(&c.implementation)
                && !blocked.iter().any(|b| b.indexer_id == c.id)
        })
        .collect();
    if usable.is_empty() {
        import_failure_check(state, completed_handling, out);
        return Ok(());
    }
    let mappings = kitsunarr_db::rules::list_remote_path_mappings(pool).await?;
    let roots: Vec<String> = kitsunarr_db::root_folders::list(pool)
        .await?
        .into_iter()
        .map(|r| r.path)
        .collect();
    let statuses = futures::future::join_all(usable.iter().map(|c| client_status(state, c))).await;
    let env = FolderCheckEnv {
        completed_handling,
        docker: in_docker(),
        os: os_name(),
    };
    let mut found = ClientFolderProblems::default();
    for (client, status) in usable.iter().zip(statuses) {
        let Some(status) = status else { continue };
        let host = kitsunarr_downloaders::ClientSettings::from_provider(
            &client.implementation,
            &client.settings,
        )
        .map(|s| s.host().to_string())
        .unwrap_or_default();
        let local: Vec<String> = status
            .output_root_folders
            .iter()
            .map(|f| kitsunarr_library::downloads::remap(&mappings, &host, f))
            .collect();
        let p = client_folder_problems(&client.name, &status, &local, &roots, &env, |path| {
            Path::new(path).is_dir()
        });
        found.removes_completed = found.removes_completed.or(p.removes_completed);
        found.sorting = found.sorting.or(p.sorting);
        found.root_folder = found.root_folder.or(p.root_folder);
        found.remote_path = found.remote_path.or(p.remote_path);
    }
    for (source, level, message) in [
        (
            "DownloadClientRemovesCompletedDownloadsCheck",
            HealthLevel::Warning,
            found.removes_completed,
        ),
        (
            "DownloadClientSortingCheck",
            HealthLevel::Warning,
            found.sorting,
        ),
        (
            "DownloadClientRootFolderCheck",
            HealthLevel::Warning,
            found.root_folder,
        ),
        (
            "RemotePathMappingCheck",
            HealthLevel::Error,
            found.remote_path,
        ),
    ] {
        if let Some(m) = message {
            out.push(check(source, level, m));
        }
    }
    import_failure_check(state, completed_handling, out);
    Ok(())
}

/// The check a failed import left, unless the folders already gave one.
fn import_failure_check(state: &AppState, completed_handling: bool, out: &mut Vec<HealthCheck>) {
    if completed_handling
        && !out.iter().any(|h| h.source == "RemotePathMappingCheck")
        && let Some(h) = state
            .import_health
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    {
        out.push(h);
    }
}

/// `RemotePathMappingCheck.Check(EpisodeImportFailedEvent)`: what a failed import says about
/// paths and permissions.
pub fn import_failure_message(
    f: &kitsunarr_library::downloads::ImportFailure,
    is_localhost: bool,
    docker: bool,
    os: &str,
    file_exists: impl Fn(&Path) -> bool,
    dir_exists: impl Fn(&str) -> bool,
) -> String {
    let name = &f.client_name;
    if let Some(file) = &f.file {
        let path = file.display();
        return if file_exists(file) {
            format!(
                "Kitsunarr can see but not access downloaded episode {path}. Likely permissions error."
            )
        } else {
            format!("File {path} was removed part way through processing.")
        };
    }
    let path = f.output_path.as_str();
    if path.trim().is_empty() {
        return "Kitsunarr failed to import (an) episode(s). Check your logs for details.".into();
    }
    if !valid_here(path) {
        return if !is_localhost {
            format!(
                "Remote download client {name} reported files in {path} but this is not a valid {os} path. Review your remote path mappings and download client settings."
            )
        } else if docker {
            format!(
                "You are using docker; download client {name} reported files in {path} but this is not a valid {os} path. Review your remote path mappings and download client settings."
            )
        } else {
            format!(
                "Local download client {name} reported files in {path} but this is not a valid {os} path. Review your download client settings."
            )
        };
    }
    if dir_exists(path) || docker {
        return format!(
            "Kitsunarr can see but not access download directory {path}. Likely permissions error."
        );
    }
    if !is_localhost {
        return format!(
            "Remote download client {name} reported files in {path} but this directory does not appear to exist. Likely missing remote path mapping."
        );
    }
    format!(
        "Download client {name} reported files in {path} but Kitsunarr cannot see this directory. You may need to adjust the folder's permissions."
    )
}

/// Records what a refresh's imports said: a failure keeps the check until an import works.
pub fn note_imports(
    state: &AppState,
    failures: &[kitsunarr_library::downloads::ImportFailure],
    imported: bool,
) {
    let mut slot = state
        .import_health
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(f) = failures.last() {
        let message = import_failure_message(
            f,
            kitsunarr_downloaders::is_localhost(&f.client_host),
            in_docker(),
            &os_name(),
            |p| p.exists(),
            |p| Path::new(p).is_dir(),
        );
        *slot = Some(check("RemotePathMappingCheck", HealthLevel::Error, message));
    } else if imported {
        *slot = None;
    }
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

    #[test]
    fn download_client_folders_are_checked_as_sonarr_does() {
        use kitsunarr_downloaders::ClientStatus;
        let env = FolderCheckEnv {
            completed_handling: true,
            docker: false,
            os: "debian".into(),
        };
        let roots = vec!["/media/tv".to_string()];
        let status = ClientStatus {
            is_localhost: false,
            output_root_folders: Vec::new(),
            removes_completed_downloads: true,
            sorting_mode: Some("TV".into()),
        };
        let p = client_folder_problems(
            "SAB",
            &status,
            &["/media/tv/downloads".into()],
            &roots,
            &env,
            |_| true,
        );
        assert!(
            p.removes_completed
                .unwrap()
                .starts_with("Download client SAB is set to remove completed downloads.")
        );
        assert!(p.sorting.unwrap().contains("has TV sorting enabled"));
        assert_eq!(
            p.root_folder.as_deref(),
            Some(
                "Download client SAB places downloads in the root folder /media/tv. You should not download to a root folder."
            )
        );
        assert_eq!(p.remote_path, None);
        // A Windows path from a remote client, and a missing folder.
        let p = client_folder_problems(
            "SAB",
            &status,
            &["D:\\Downloads".into()],
            &roots,
            &env,
            |_| true,
        );
        assert!(p.remote_path.unwrap().starts_with("Remote download client SAB places downloads in D:\\Downloads but this is not a valid debian path."));
        let p =
            client_folder_problems("SAB", &status, &["/downloads".into()], &roots, &env, |_| {
                false
            });
        assert!(
            p.remote_path
                .unwrap()
                .contains("Likely missing or incorrect remote path mapping.")
        );
        let local = ClientStatus {
            is_localhost: true,
            ..ClientStatus::default()
        };
        let p = client_folder_problems("qB", &local, &["/downloads".into()], &roots, &env, |_| {
            false
        });
        assert!(p.remote_path.unwrap().contains("cannot see this directory"));
        // Without completed download handling the folders are not checked.
        let off = FolderCheckEnv {
            completed_handling: false,
            ..env
        };
        let p = client_folder_problems("qB", &local, &["/downloads".into()], &roots, &off, |_| {
            false
        });
        assert_eq!(p, ClientFolderProblems::default());
    }

    #[test]
    fn failed_imports_are_explained_as_sonarr_does() {
        use kitsunarr_library::downloads::ImportFailure;
        let f = |output: &str, file: Option<&str>| ImportFailure {
            client_name: "SAB".into(),
            client_host: "nas".into(),
            output_path: output.into(),
            file: file.map(std::path::PathBuf::from),
        };
        let msg = |f: &ImportFailure, local: bool, docker: bool, exists: bool| {
            import_failure_message(f, local, docker, "debian", |_| exists, |_| exists)
        };
        assert_eq!(
            msg(&f("/dl/x", Some("/dl/x/a.mkv")), false, false, true),
            "Kitsunarr can see but not access downloaded episode /dl/x/a.mkv. Likely permissions error."
        );
        assert_eq!(
            msg(&f("/dl/x", Some("/dl/x/a.mkv")), false, false, false),
            "File /dl/x/a.mkv was removed part way through processing."
        );
        assert!(msg(&f("C:\\dl\\x", None), false, false, false).starts_with("Remote download client SAB reported files in C:\\dl\\x but this is not a valid debian path."));
        assert!(
            msg(&f("/dl/x", None), false, false, false)
                .ends_with("Likely missing remote path mapping.")
        );
        assert!(
            msg(&f("/dl/x", None), true, false, false)
                .ends_with("You may need to adjust the folder's permissions.")
        );
        assert!(
            msg(&f("/dl/x", None), true, false, true)
                .starts_with("Kitsunarr can see but not access download directory /dl/x.")
        );
    }
}
