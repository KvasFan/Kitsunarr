//! Per-download import progress: Tanukarr's `ImportProgressService`.
//!
//! While a completed download is imported, each of its files is tracked (waiting, importing,
//! imported, rejected or failed) against the episodes the grab expected, so the queue can show
//! how far the import is ("12 / 24 episodes imported") and why a file was left out. Progress
//! lives in memory, keyed by download client and download id, and is dropped when the download
//! leaves the client.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use kitsunarr_domain::api::queue::{ImportProgress, ImportProgressItem, ImportProgressState};

/// `ImportProgressKey`: a download client and the download's id (compared case-insensitively).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    client_id: i64,
    download_id: String,
}

impl Key {
    /// `None` without a client or a download id (nothing to track).
    pub fn new(client_id: i64, download_id: &str) -> Option<Self> {
        (client_id > 0 && !download_id.trim().is_empty()).then(|| Self {
            client_id,
            download_id: download_id.to_ascii_uppercase(),
        })
    }
}

/// One import decision for a file (`ImportDecision` with its `LocalEpisode`).
#[derive(Debug, Clone, PartialEq)]
pub struct Decision<'a> {
    pub path: &'a Path,
    /// `(id, season number, episode number)` of each episode the file holds.
    pub episodes: Vec<(i64, i32, i32)>,
    pub size: i64,
    /// Rejection reasons; none for an approved file.
    pub rejections: &'a [String],
}

#[derive(Debug)]
struct Item {
    id: String,
    order: usize,
    episode_ids: Vec<i64>,
    season_number: Option<i32>,
    episode_numbers: Vec<i32>,
    source_path: String,
    size: i64,
    state: ImportProgressState,
    started: Option<String>,
    completed: Option<String>,
    errors: Vec<String>,
}

#[derive(Debug, Default)]
struct Batch {
    expected: HashSet<i64>,
    items: HashMap<String, Item>,
    next_order: usize,
}

/// Shared between the download monitor (which imports) and the queue (which reads).
#[derive(Clone, Default, Debug)]
pub struct Tracker {
    batches: Arc<Mutex<HashMap<Key, Batch>>>,
}

/// A tracker and the download an import reports progress for.
#[derive(Clone, Debug)]
pub struct Scope {
    pub tracker: Tracker,
    pub key: Key,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Tracker {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Batch>> {
        self.batches.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `BeginBatch`: an import of the download starts, expecting these episodes. A different
    /// set of expected episodes starts over.
    pub fn begin_batch(&self, key: &Key, expected: impl IntoIterator<Item = i64>) {
        let expected: HashSet<i64> = expected.into_iter().filter(|id| *id > 0).collect();
        let mut batches = self.lock();
        let batch = batches.entry(key.clone()).or_default();
        if !batch.expected.is_empty() && batch.expected != expected {
            *batch = Batch::default();
        }
        batch.expected.extend(expected);
    }

    /// `RegisterDecisions`: every file of the download, waiting when approved and rejected
    /// with its reasons otherwise. Files already imported keep their state.
    pub fn register(&self, key: &Key, decisions: &[Decision<'_>]) {
        let mut batches = self.lock();
        let batch = batches.entry(key.clone()).or_default();
        if batch.expected.is_empty() {
            batch.expected.extend(
                decisions
                    .iter()
                    .flat_map(|d| d.episodes.iter().map(|e| e.0)),
            );
        }
        for d in decisions {
            let path = path_key(d.path);
            let order = batch.next_order;
            let item = batch.items.entry(path.clone()).or_insert_with(|| Item {
                id: uuid::Uuid::new_v4().simple().to_string(),
                order,
                episode_ids: Vec::new(),
                season_number: None,
                episode_numbers: Vec::new(),
                source_path: path,
                size: 0,
                state: ImportProgressState::Waiting,
                started: None,
                completed: None,
                errors: Vec::new(),
            });
            if item.order == order {
                batch.next_order += 1;
            }
            if item.state == ImportProgressState::Imported {
                continue;
            }
            let mut ids: Vec<i64> = d
                .episodes
                .iter()
                .map(|e| e.0)
                .filter(|id| *id > 0)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            let mut seasons: Vec<i32> = d.episodes.iter().map(|e| e.1).collect();
            seasons.sort_unstable();
            seasons.dedup();
            let mut numbers: Vec<i32> = d.episodes.iter().map(|e| e.2).collect();
            numbers.sort_unstable();
            numbers.dedup();
            let approved = d.rejections.is_empty();
            item.episode_ids = ids;
            item.season_number = (seasons.len() == 1).then(|| seasons[0]);
            item.episode_numbers = numbers;
            item.size = d.size;
            item.state = if approved {
                ImportProgressState::Waiting
            } else {
                ImportProgressState::Rejected
            };
            item.started = None;
            item.completed = (!approved).then(now);
            item.errors = d.rejections.to_vec();
        }
    }

    fn update(&self, key: &Key, path: &Path, f: impl FnOnce(&mut Item)) {
        let mut batches = self.lock();
        if let Some(item) = batches
            .get_mut(key)
            .and_then(|b| b.items.get_mut(&path_key(path)))
        {
            f(item);
        }
    }

    /// `MarkImporting`: the file is being copied, moved or linked.
    pub fn mark_importing(&self, key: &Key, path: &Path) {
        self.update(key, path, |item| {
            item.state = ImportProgressState::Importing;
            item.started = Some(now());
            item.completed = None;
            item.errors.clear();
        });
    }

    /// `MarkImported`.
    pub fn mark_imported(&self, key: &Key, path: &Path) {
        self.update(key, path, |item| {
            item.state = ImportProgressState::Imported;
            item.started.get_or_insert_with(now);
            item.completed = Some(now());
            item.errors.clear();
        });
    }

    /// `MarkFailed`.
    pub fn mark_failed(&self, key: &Key, path: &Path, errors: &[String]) {
        self.update(key, path, |item| {
            item.state = ImportProgressState::Failed;
            item.started.get_or_insert_with(now);
            item.completed = Some(now());
            item.errors = errors
                .iter()
                .filter(|e| !e.trim().is_empty())
                .cloned()
                .collect();
        });
    }

    /// `GetProgress`: the download's progress, `None` before any import started.
    pub fn get(&self, client_id: i64, download_id: &str) -> Option<ImportProgress> {
        let key = Key::new(client_id, download_id)?;
        self.lock().get(&key).map(snapshot)
    }

    /// `Prune`: forgets downloads that are no longer in a client.
    pub fn prune(&self, active: &HashSet<Key>) {
        self.lock().retain(|k, _| active.contains(k));
    }
}

/// `CreateSnapshot`: each expected episode counts once, in the most advanced state any of its
/// files reached (imported, importing, waiting, then failed, then rejected); episodes no file
/// holds yet are waiting.
fn snapshot(batch: &Batch) -> ImportProgress {
    let mut items: Vec<&Item> = batch.items.values().collect();
    items.sort_by_key(|i| i.order);
    let expected: HashSet<i64> = if batch.expected.is_empty() {
        items
            .iter()
            .flat_map(|i| i.episode_ids.iter().copied())
            .collect()
    } else {
        batch.expected.clone()
    };
    let in_state = |candidates: &HashSet<i64>, state: ImportProgressState| -> HashSet<i64> {
        items
            .iter()
            .filter(|i| i.state == state)
            .flat_map(|i| i.episode_ids.iter().copied())
            .filter(|id| candidates.contains(id))
            .collect()
    };
    let imported = in_state(&expected, ImportProgressState::Imported);
    let rest: HashSet<i64> = expected.difference(&imported).copied().collect();
    let importing = in_state(&rest, ImportProgressState::Importing);
    let rest: HashSet<i64> = rest.difference(&importing).copied().collect();
    let waiting = in_state(&rest, ImportProgressState::Waiting);
    let rest: HashSet<i64> = rest.difference(&waiting).copied().collect();
    let failed = in_state(&rest, ImportProgressState::Failed);
    let rest: HashSet<i64> = rest.difference(&failed).copied().collect();
    let rejected = in_state(&rest, ImportProgressState::Rejected);
    let unrepresented = rest.difference(&rejected).count();
    let count = |s: &HashSet<i64>| s.len() as i32;
    ImportProgress {
        total_episodes: count(&expected),
        imported_episodes: count(&imported),
        importing_episodes: count(&importing),
        waiting_episodes: count(&waiting) + unrepresented as i32,
        failed_episodes: count(&failed),
        rejected_episodes: count(&rejected),
        items: items
            .into_iter()
            .map(|i| ImportProgressItem {
                id: i.id.clone(),
                episode_ids: i.episode_ids.clone(),
                season_number: i.season_number,
                episode_numbers: i.episode_numbers.clone(),
                source_path: i.source_path.clone(),
                size: i.size,
                state: i.state,
                started: i.started.clone(),
                completed: i.completed.clone(),
                errors: i.errors.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision<'a>(
        path: &'a str,
        episodes: &[(i64, i32, i32)],
        rejections: &'a [String],
    ) -> Decision<'a> {
        Decision {
            path: Path::new(path),
            episodes: episodes.to_vec(),
            size: 100,
            rejections,
        }
    }

    #[test]
    fn follows_each_file_through_the_import() {
        let tracker = Tracker::default();
        let key = Key::new(3, "abc").unwrap();
        assert!(tracker.get(3, "ABC").is_none());
        tracker.begin_batch(&key, [11, 12, 13, 14]);
        let sample = vec!["Sample".to_string()];
        tracker.register(
            &key,
            &[
                decision("/dl/e01.mkv", &[(11, 1, 1)], &[]),
                decision("/dl/e02-03.mkv", &[(12, 1, 2), (13, 1, 3)], &[]),
                decision("/dl/sample.mkv", &[(14, 1, 4)], &sample),
            ],
        );
        let p = tracker.get(3, "abc").unwrap();
        assert_eq!(
            (p.total_episodes, p.waiting_episodes, p.rejected_episodes),
            (4, 3, 1)
        );
        assert_eq!(p.items[1].episode_numbers, [2, 3]);
        assert_eq!(p.items[1].season_number, Some(1));
        assert_eq!(p.items[2].errors, sample);

        tracker.mark_importing(&key, Path::new("/dl/e01.mkv"));
        let p = tracker.get(3, "abc").unwrap();
        assert_eq!((p.importing_episodes, p.waiting_episodes), (1, 2));
        assert!(p.items[0].started.is_some());

        tracker.mark_imported(&key, Path::new("/dl/e01.mkv"));
        tracker.mark_failed(
            &key,
            Path::new("/dl/e02-03.mkv"),
            &["Failed to import episode".to_string()],
        );
        let p = tracker.get(3, "abc").unwrap();
        assert_eq!(
            (
                p.imported_episodes,
                p.failed_episodes,
                p.rejected_episodes,
                p.waiting_episodes
            ),
            (1, 2, 1, 0)
        );
        assert_eq!(p.items[0].state, ImportProgressState::Imported);

        // A retry keeps imported files and re-registers the others.
        tracker.begin_batch(&key, [11, 12, 13, 14]);
        tracker.register(
            &key,
            &[
                decision("/dl/e01.mkv", &[(11, 1, 1)], &[]),
                decision("/dl/e02-03.mkv", &[(12, 1, 2), (13, 1, 3)], &[]),
            ],
        );
        let p = tracker.get(3, "abc").unwrap();
        assert_eq!((p.imported_episodes, p.waiting_episodes), (1, 2));
        assert_eq!(p.items.len(), 3, "files keep their place");

        tracker.prune(&HashSet::new());
        assert!(tracker.get(3, "abc").is_none());
    }

    #[test]
    fn episodes_without_a_file_wait_and_new_expectations_start_over() {
        let tracker = Tracker::default();
        let key = Key::new(1, "x").unwrap();
        tracker.begin_batch(&key, [1, 2]);
        tracker.register(&key, &[decision("/a.mkv", &[(1, 1, 1)], &[])]);
        tracker.mark_imported(&key, Path::new("/a.mkv"));
        let p = tracker.get(1, "x").unwrap();
        assert_eq!((p.imported_episodes, p.waiting_episodes), (1, 1));
        tracker.begin_batch(&key, [5]);
        let p = tracker.get(1, "x").unwrap();
        assert_eq!((p.total_episodes, p.items.len()), (1, 0));
        assert!(Key::new(0, "x").is_none() && Key::new(1, " ").is_none());
    }
}
