//! Ranking: `DownloadDecisionComparer` and `DownloadDecisionPriorizationService`.

use std::cmp::Ordering;

use kitsunarr_domain::{DownloadProtocol, SeriesType};

use crate::model::{Decision, Release, Series, Settings};
use crate::profile::{ProfileView, ProperDownloadType, compare_revision};
use crate::specs::megabytes;

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// `long.Round(level)`: towards zero, to a multiple of `level`.
fn round_to(n: i64, level: i64) -> i64 {
    (n / level) * level
}

pub(crate) struct Comparer<'a> {
    pub series: &'a Series,
    pub settings: &'a Settings,
    pub releases: &'a [Release],
    pub preferred_protocol: Option<DownloadProtocol>,
}

impl Comparer<'_> {
    fn release(&self, d: &Decision) -> &Release {
        &self.releases[d.release]
    }

    /// `Compare(x, y)`: `Greater` when `x` is the better release.
    pub(crate) fn compare(&self, x: &Decision, y: &Decision) -> Ordering {
        self.quality(x, y)
            .then_with(|| x.custom_format_score.cmp(&y.custom_format_score))
            .then_with(|| self.protocol(x, y))
            .then_with(|| self.episode_count(x, y))
            .then_with(|| self.episode_number(x, y))
            .then_with(|| {
                // Lower priority values win.
                self.release(y)
                    .indexer_priority
                    .cmp(&self.release(x).indexer_priority)
            })
            .then_with(|| self.peers_if_torrent(x, y))
            .then_with(|| self.age_if_usenet(x, y))
            .then_with(|| self.size(x, y))
    }

    fn quality(&self, x: &Decision, y: &Decision) -> Ordering {
        let profile = ProfileView(&self.series.profile);
        let index = |d: &Decision| profile.index_of(d.quality.quality.id, false);
        let by_index = index(x).compare(&index(y), true);
        if self.settings.propers == ProperDownloadType::DoNotPrefer {
            return by_index;
        }
        by_index.then_with(|| compare_revision(&x.quality.revision, &y.quality.revision))
    }

    fn protocol(&self, x: &Decision, y: &Decision) -> Ordering {
        let Some(preferred) = self.preferred_protocol else {
            return Ordering::Equal;
        };
        let p = |d: &Decision| self.release(d).protocol == preferred;
        p(x).cmp(&p(y))
    }

    fn episode_count(&self, x: &Decision, y: &Decision) -> Ordering {
        let full = |d: &Decision| d.parsed.as_ref().is_some_and(|p| p.full_season);
        let season_pack = full(x).cmp(&full(y));
        if season_pack != Ordering::Equal {
            return season_pack;
        }
        if self.series.series_type == SeriesType::Anime {
            x.episodes.len().cmp(&y.episodes.len())
        } else {
            y.episodes.len().cmp(&x.episodes.len())
        }
    }

    fn episode_number(&self, x: &Decision, y: &Decision) -> Ordering {
        let first = |d: &Decision| {
            d.episodes
                .iter()
                .map(|e| e.episode_number)
                .min()
                .unwrap_or(0)
        };
        first(y).cmp(&first(x))
    }

    fn peers_if_torrent(&self, x: &Decision, y: &Decision) -> Ordering {
        let (rx, ry) = (self.release(x), self.release(y));
        if rx.protocol != DownloadProtocol::Torrent || ry.protocol != DownloadProtocol::Torrent {
            return Ordering::Equal;
        }
        let log = |v: Option<i64>| match v {
            Some(v) if v > 0 => (v as f64).log10().round_ties_even(),
            _ => 0.0,
        };
        cmp_f64(log(rx.seeders), log(ry.seeders))
            .then_with(|| cmp_f64(log(rx.peers), log(ry.peers)))
    }

    fn age_if_usenet(&self, x: &Decision, y: &Decision) -> Ordering {
        let (rx, ry) = (self.release(x), self.release(y));
        if rx.protocol != DownloadProtocol::Usenet || ry.protocol != DownloadProtocol::Usenet {
            return Ordering::Equal;
        }
        let score = |r: &Release| {
            let elapsed = self.settings.now - r.publish_date;
            let hours = elapsed.num_milliseconds() as f64 / 3_600_000.0;
            let days = elapsed.num_days();
            if hours < 1.0 {
                1000.0
            } else if hours <= 24.0 {
                100.0
            } else if days <= 7 {
                10.0
            } else {
                -(days as f64).log10().round_ties_even()
            }
        };
        cmp_f64(score(rx), score(ry))
    }

    /// `GetRuntime`.
    fn runtime(&self, d: &Decision) -> i64 {
        if d.episodes.is_empty() {
            return i64::from(self.series.runtime);
        }
        d.episodes
            .iter()
            .map(|e| {
                i64::from(if e.runtime > 0 {
                    e.runtime
                } else {
                    self.series.runtime
                })
            })
            .sum()
    }

    /// `CompareSize`: closest to the preferred size, else larger (in 200 MB steps). The
    /// preferred size comes from the quality definitions.
    fn size(&self, x: &Decision, y: &Decision) -> Ordering {
        let step = megabytes(200.0);
        let value = |d: &Decision| {
            let size = self.release(d).size;
            let preferred = self
                .settings
                .quality_definitions
                .iter()
                .find(|q| q.quality_id == d.quality.quality.id)
                .and_then(|q| q.preferred_size);
            let runtime = self.runtime(d);
            match preferred {
                Some(p) if runtime > 0 => {
                    let preferred_size = runtime * megabytes(p.0);
                    -round_to(size - preferred_size, step).abs()
                }
                _ => round_to(size, step),
            }
        };
        value(x).cmp(&value(y))
    }
}

/// `PrioritizeDecisions`: the searched series' releases best first; then releases mapped to
/// other library series (kept in arrival order: their profiles are not loaded); then releases
/// that matched no series. Ties keep arrival order.
pub(crate) fn prioritize(comparer: &Comparer<'_>, mut decisions: Vec<Decision>) -> Vec<Decision> {
    let searched = comparer.series.id;
    let rank = |d: &Decision| match d.series_id {
        Some(id) if id == searched => 0,
        Some(_) => 1,
        None => 2,
    };
    decisions.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            if rank(a) == 0 {
                comparer.compare(b, a)
            } else {
                Ordering::Equal
            }
        })
    });
    decisions
}
