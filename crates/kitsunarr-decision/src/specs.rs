//! The decision specifications (`DecisionEngine/Specifications`) that apply to searches and for
//! which Kitsunarr has the data. Each returns `None` to accept or the rejection.

use std::sync::LazyLock;

use kitsunarr_domain::quality::QualityModel;
use kitsunarr_domain::{DownloadProtocol, SeriesType};
use kitsunarr_parser::ParsedEpisodeInfo;
use kitsunarr_parser::netregex::NetRegex;

use crate::custom_format::{CompiledFormat, CustomFormatInput, evaluate};
use crate::model::{
    Blocklisted, DelayProfile, Episode, EpisodeFile, Rejection, RejectionReason as R, Release,
    ReleaseProfile, SearchCriteria, SearchKind, Series, Settings,
};
use crate::profile::{
    ProfileView, ProperDownloadType, UpgradeRejectReason, is_revision_upgrade, quality_model_label,
    revision_label,
};

const MB: i64 = 1024 * 1024;

/// A release mapped to the searched series, with everything the specifications read.
pub(crate) struct Subject<'a> {
    pub release: &'a Release,
    pub parsed: &'a ParsedEpisodeInfo,
    pub episodes: Vec<&'a Episode>,
    /// `MappedSeasonNumber`.
    pub mapped_season: Option<i32>,
    /// Tanukarr's `NumberingRejection`: the release's numbering is ambiguous.
    pub numbering_rejection: Option<String>,
    pub custom_formats: Vec<&'a CompiledFormat>,
    pub score: i32,
}

pub(crate) struct Env<'a> {
    pub series: &'a Series,
    pub settings: &'a Settings,
    pub formats: &'a [CompiledFormat],
    pub criteria: &'a SearchCriteria,
}

impl Env<'_> {
    fn profile(&self) -> ProfileView<'_> {
        ProfileView(&self.series.profile)
    }

    /// `ReleaseProfileService.EnabledForTags(series.Tags, indexerId)`.
    fn release_profiles(&self, indexer_id: i64) -> Vec<&ReleaseProfile> {
        self.settings
            .release_profiles
            .iter()
            .filter(|r| r.tags.is_empty() || r.tags.iter().any(|t| self.series.tags.contains(t)))
            .filter(|r| r.enabled)
            .filter(|r| r.indexer_ids.is_empty() || r.indexer_ids.contains(&indexer_id))
            .collect()
    }

    /// `DelayProfileService.BestForTags(series.Tags)`.
    pub(crate) fn delay_profile(&self) -> Option<&DelayProfile> {
        self.settings
            .delay_profiles
            .iter()
            .filter(|d| d.tags.is_empty() || d.tags.iter().any(|t| self.series.tags.contains(t)))
            .min_by_key(|d| d.order)
    }

    /// `ParseCustomFormat(EpisodeFile)` and its score.
    /// The custom format score of a grabbed release (`ParseCustomFormat(RemoteEpisode)` for the
    /// queue, `ParseCustomFormat(EpisodeHistory)` for history): parsed from its title.
    fn grab_score(&self, grab: GrabInput<'_>) -> i32 {
        let parsed = kitsunarr_parser::parse_title(grab.title);
        let input = CustomFormatInput {
            release_title: grab.title,
            file_name: None,
            release_group: parsed.as_ref().and_then(|p| p.release_group.as_deref()),
            quality: grab.quality.quality(),
            size: grab.size,
            indexer_flags: grab.indexer_flags,
            languages: grab.languages,
            release_type: grab.release_type.unwrap_or_else(|| {
                parsed
                    .as_ref()
                    .map_or(kitsunarr_parser::ReleaseType::Unknown, |p| p.release_type())
            }),
            original_language: self.series.original_language,
        };
        let formats = evaluate(self.formats, &input);
        self.profile().format_score(formats.iter().map(|f| &f.id))
    }

    fn file_formats(&self, file: &EpisodeFile) -> (Vec<&CompiledFormat>, i32) {
        let input = CustomFormatInput {
            release_title: &file.release_title,
            file_name: Some(&file.file_name),
            release_group: file.release_group.as_deref(),
            quality: file.quality.quality(),
            size: file.size,
            indexer_flags: file.indexer_flags,
            languages: &file.languages,
            release_type: file.release_type,
            original_language: self.series.original_language,
        };
        let formats = evaluate(self.formats, &input);
        let score = self.profile().format_score(formats.iter().map(|f| &f.id));
        (formats, score)
    }
}

/// A grabbed release, as `Env::grab_score` reads it.
#[derive(Clone, Copy)]
struct GrabInput<'a> {
    title: &'a str,
    quality: &'a QualityModel,
    size: i64,
    indexer_flags: u32,
    languages: &'a [&'static kitsunarr_domain::Language],
    /// Stored with the grab (history); `None`: from the parsed title (queue).
    release_type: Option<kitsunarr_parser::ReleaseType>,
}

fn model(p: &ParsedEpisodeInfo) -> QualityModel {
    QualityModel {
        quality_id: p.quality.quality.id,
        revision: p.quality.revision,
    }
}

/// `long.SizeSuffix()`: `"1.4 GB"`, `"1,023.5 MB"` (`{0:n1}`, invariant culture).
pub(crate) fn size_suffix(bytes: i64) -> String {
    const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    if bytes == 0 {
        return "0 B".into();
    }
    if bytes < 0 {
        return format!("-{}", size_suffix(bytes.checked_abs().unwrap_or(i64::MAX)));
    }
    let mag = ((bytes as f64).ln() / 1024f64.ln()) as usize;
    let adjusted = bytes as f64 / 1024f64.powi(mag as i32);
    let fixed = format!("{adjusted:.1}");
    let (int, frac) = fixed.split_once('.').unwrap_or((&fixed, "0"));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped}.{frac} {}", UNITS[mag.min(6)])
}

/// .NET `{0:0.#}`-style formatting with up to `decimals` digits, trailing zeros removed.
fn format_number(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn episode_files<'a>(s: &Subject<'a>) -> impl Iterator<Item = &'a EpisodeFile> {
    s.episodes.iter().filter_map(|e| e.file.as_ref())
}

/// Every specification, in a stable (alphabetical) order.
pub(crate) fn evaluate_all(env: &Env<'_>, s: &Subject<'_>) -> Vec<Rejection> {
    let specs: [fn(&Env<'_>, &Subject<'_>) -> Option<Rejection>; 34] = [
        acceptable_size,
        anime_version_upgrade,
        blocklist,
        complete_season_upgrade,
        custom_format_allowed_by_profile,
        delay,
        episode_numbering,
        episode_requested,
        external_ids,
        free_space,
        full_season,
        history,
        indexer_tag,
        maximum_size,
        minimum_age,
        monitored_episode,
        multi_season,
        not_sample,
        proper,
        protocol,
        quality_allowed_by_profile,
        queue,
        raw_disk,
        release_restrictions,
        repack,
        retention,
        same_episodes_grab,
        season_match,
        series_spec,
        single_episode_search_match,
        split_episode,
        torrent_seeding,
        upgrade_allowed,
        upgrade_disk,
    ];
    specs.iter().filter_map(|spec| spec(env, s)).collect()
}

/// Tanukarr's `EpisodeNumberingSpecification` (permanent): an unresolved numbering must not
/// reach the delay queue.
pub(crate) fn episode_numbering(_env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    s.numbering_rejection
        .as_ref()
        .map(|m| Rejection::new(R::AmbiguousNumbering, m.clone()))
}

/// `BlocklistSpecification` (`BlocklistService.Blocklisted`, `ReleaseComparer`): torrents by
/// info hash, else by title and indexer; Usenet by title, indexer and publish date. Titles match
/// when the blocklisted title contains the release's, ignoring case (upstream's `Contains`).
pub(crate) fn blocklist(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let r = s.release;
    let same_indexer = |b: &Blocklisted| {
        b.indexer
            .as_deref()
            .is_none_or(|i| i.trim().is_empty() || i.eq_ignore_ascii_case(&r.indexer))
    };
    let title = r.title.to_lowercase();
    let mut entries = env
        .settings
        .blocklist
        .iter()
        .filter(|b| b.series_id == env.series.id);
    let hash = r.info_hash.as_deref().filter(|h| !h.trim().is_empty());
    let hit = match (r.protocol, hash) {
        (DownloadProtocol::Torrent, Some(h)) => entries.any(|b| {
            b.torrent_info_hash
                .as_deref()
                .is_some_and(|x| x.eq_ignore_ascii_case(h))
        }),
        (DownloadProtocol::Torrent, None) => entries.any(|b| {
            b.protocol == DownloadProtocol::Torrent
                && b.title.to_lowercase().contains(&title)
                && same_indexer(b)
        }),
        _ => entries.any(|b| {
            b.protocol == DownloadProtocol::Usenet
                && b.title.to_lowercase().contains(&title)
                && same_indexer(b)
                && b.published_date == Some(r.publish_date)
        }),
    };
    hit.then(|| Rejection::new(R::Blocklisted, "Release is blocklisted".to_string()))
}

/// Tanukarr's `ExternalIdsSpecification` (Library tools "match external ids", off by
/// default): a release whose TheTVDB id or IMDb id names another series is the wrong series.
pub(crate) fn external_ids(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if !env.settings.match_external_ids {
        return None;
    }
    let series = env.series;
    if s.release.tvdb_id > 0 && series.tvdb_id > 0 && s.release.tvdb_id != series.tvdb_id {
        return Some(Rejection::new(
            R::WrongSeries,
            format!(
                "Release TheTVDB id {} does not match the series' {}",
                s.release.tvdb_id, series.tvdb_id
            ),
        ));
    }
    // IMDb ids compared as numbers (`tt` and leading zeros removed).
    let imdb = |v: Option<&str>| -> Option<u64> {
        let v = v?.trim().to_lowercase();
        v.trim_start_matches("tt")
            .trim_start_matches('0')
            .parse()
            .ok()
    };
    if let (Some(a), Some(b)) = (
        imdb(s.release.imdb_id.as_deref()),
        imdb(series.imdb_id.as_deref()),
    ) && a != b
    {
        return Some(Rejection::new(
            R::WrongSeries,
            "Release IMDb id does not match the series' IMDb id",
        ));
    }
    None
}

/// `AcceptableSizeSpecification`, with Kitsunarr's quality definitions providing the sizes
/// (as Sonarr v4 does; v5 moved them into quality profiles).
pub(crate) fn acceptable_size(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if s.parsed.special || s.release.size == 0 {
        return None;
    }
    let series = env.series;
    let mut series_runtime = series.runtime;
    if series_runtime == 0 {
        let first_season = series
            .episodes
            .iter()
            .filter(|e| e.season_number > 0)
            .map(|e| e.season_number)
            .min();
        let pilot = first_season.and_then(|season| {
            series
                .episodes
                .iter()
                .filter(|e| e.season_number == season)
                .min_by_key(|e| e.episode_number)
        });
        if let (Some(pilot), Some(first)) = (pilot, s.episodes.first())
            && first.season_number == pilot.season_number
        {
            let grace_end =
                pilot.air_date_utc.unwrap_or(s.release.publish_date) + chrono::Duration::hours(24);
            if s.episodes
                .iter()
                .all(|e| e.air_date_utc.is_none_or(|a| a < grace_end))
            {
                series_runtime = 45;
            }
        }
    }
    let runtime: i64 = s
        .episodes
        .iter()
        .map(|e| {
            i64::from(if e.runtime > 0 {
                e.runtime
            } else {
                series_runtime
            })
        })
        .sum();
    if runtime == 0 {
        return Some(Rejection::new(
            R::UnknownRuntime,
            "Runtime of all episodes is 0, unable to validate size until it is available",
        ));
    }
    let (min_limit, _, max_limit) = size_limits(
        env.settings,
        &env.series.profile,
        s.parsed.quality.quality.id,
    )?;
    let runtime_message = if s.episodes.len() == 1 {
        format!("{runtime}min")
    } else {
        format!("{} episodes totalling {runtime}min", s.episodes.len())
    };
    // Tanukarr: the limits apply to the codec-normalised size.
    let (family, factor, size) = crate::codec::normalize(
        &env.series.profile,
        &s.release.title,
        s.parsed,
        s.release.size,
    );
    let shown = if factor == 1.0 {
        size_suffix(s.release.size)
    } else {
        format!(
            "{} actual, {} codec-normalized ({family} ×{factor:.2})",
            size_suffix(s.release.size),
            size_suffix(size)
        )
    };
    if let Some(min) = min_limit {
        let min_size = megabytes(min) * runtime;
        if size < min_size {
            return Some(Rejection::new(
                R::BelowMinimumSize,
                format!(
                    "{shown} is smaller than minimum allowed {} (for {runtime_message})",
                    size_suffix(min_size)
                ),
            ));
        }
    }
    if let Some(max) = max_limit.filter(|m| *m != 0.0) {
        let max_size = megabytes(max) * runtime;
        if size > max_size {
            return Some(Rejection::new(
                R::AboveMaximumSize,
                format!(
                    "{shown} is larger than maximum allowed {} (for {runtime_message})",
                    size_suffix(max_size)
                ),
            ));
        }
    }
    None
}

/// A quality's (minimum, preferred, maximum) size in MiB per minute: the profile's own limits
/// when it has an entry for the quality, else the global quality definition.
pub(crate) fn size_limits(
    settings: &Settings,
    profile: &kitsunarr_domain::api::settings::QualityProfile,
    quality_id: i32,
) -> Option<(Option<f64>, Option<f64>, Option<f64>)> {
    if let Some(l) = profile
        .size_limits
        .iter()
        .find(|l| l.quality_id == quality_id)
    {
        return Some((
            l.min_size.map(|v| v.0),
            l.preferred_size.map(|v| v.0),
            l.max_size.map(|v| v.0),
        ));
    }
    settings
        .quality_definitions
        .iter()
        .find(|d| d.quality_id == quality_id)
        .map(|d| {
            (
                d.min_size.map(|v| v.0),
                d.preferred_size.map(|v| v.0),
                d.max_size.map(|v| v.0),
            )
        })
}

/// `double.Megabytes()`: `Convert.ToInt64(mb * 1024 * 1024)`.
pub(crate) fn megabytes(mb: f64) -> i64 {
    (mb * MB as f64).round_ties_even() as i64
}

pub(crate) fn anime_version_upgrade(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if env.series.series_type != SeriesType::Anime
        || env.settings.propers == ProperDownloadType::DoNotPrefer
    {
        return None;
    }
    let group = s.parsed.release_group.as_deref().unwrap_or_default();
    for file in episode_files(s) {
        if is_revision_upgrade(&file.quality, &model(s.parsed)) {
            let file_group = file.release_group.as_deref().unwrap_or_default();
            if file_group.trim().is_empty() {
                return Some(Rejection::new(
                    R::UnknownReleaseGroup,
                    "Existing release group is unknown",
                ));
            }
            if group.trim().is_empty() {
                return Some(Rejection::new(
                    R::UnknownReleaseGroup,
                    "Release group is unknown",
                ));
            }
            if file_group != group {
                return Some(Rejection::new(
                    R::ReleaseGroupDoesNotMatch,
                    format!("{group} does not match existing release group {file_group}"),
                ));
            }
        }
    }
    None
}

fn format_names(formats: &[&CompiledFormat]) -> String {
    formats
        .iter()
        .map(|f| f.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn custom_format_allowed_by_profile(
    env: &Env<'_>,
    s: &Subject<'_>,
) -> Option<Rejection> {
    let min = env.series.profile.min_format_score;
    (s.score < min).then(|| {
        Rejection::new(
            R::CustomFormatMinimumScore,
            format!(
                "Custom Formats {} have score {} below Series profile minimum {min}",
                format_names(&s.custom_formats),
                s.score
            ),
        )
    })
}

/// `Search/EpisodeRequestedSpecification`.
pub(crate) fn episode_requested(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if env.criteria.is_rss() {
        return None;
    }
    if s.episodes
        .iter()
        .any(|e| env.criteria.episode_ids.contains(&e.id))
    {
        return None;
    }
    let mut eps: Vec<&&Episode> = s.episodes.iter().collect();
    eps.sort_by_key(|e| (e.season_number, e.episode_number));
    let message = match (eps.first(), eps.last()) {
        (Some(first), Some(last)) if eps.len() > 1 => format!(
            "Episode wasn't requested: {}x{}-{}",
            first.season_number, first.episode_number, last.episode_number
        ),
        (Some(first), _) => format!(
            "Episode wasn't requested: {}x{}",
            first.season_number, first.episode_number
        ),
        _ => "Episode wasn't requested".into(),
    };
    Some(Rejection::new(R::WrongEpisode, message))
}

/// `FullSeasonSpecification` (release profiles cannot allow incomplete seasons in Kitsunarr).
pub(crate) fn full_season(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let cutoff = env.settings.now + chrono::Duration::hours(24);
    (s.parsed.full_season
        && s.episodes
            .iter()
            .any(|e| e.air_date_utc.is_none_or(|a| a > cutoff)))
    .then(|| {
        Rejection::new(
            R::FullSeasonNotAired,
            "Full season release rejected. All episodes haven't aired yet.",
        )
    })
}

/// `RssSync/IndexerTagSpecification`.
pub(crate) fn indexer_tag(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let tags = &s.release.indexer_tags;
    (!tags.is_empty() && !tags.iter().any(|t| env.series.tags.contains(t))).then(|| {
        Rejection::new(
            R::NoMatchingTag,
            "Series tags do not match any of the indexer tags",
        )
    })
}

/// `FreeSpaceSpecification`: importing the release must leave the minimum free space.
pub(crate) fn free_space(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let free = env.settings.free_space?;
    let remaining = free.available - s.release.size;
    if remaining <= 0 {
        return Some(Rejection::new(
            R::MinimumFreeSpace,
            "Importing after download will exceed available disk space",
        ));
    }
    (remaining < free.minimum).then(|| {
        Rejection::new(
            R::MinimumFreeSpace,
            format!(
                "Not enough free space ({}) to import after download: {}. (Settings: Media Management: Minimum Free Space)",
                size_suffix(free.minimum),
                size_suffix(remaining)
            ),
        )
    })
}

pub(crate) fn maximum_size(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let maximum = env.settings.maximum_size_mb * MB;
    (maximum != 0 && s.release.size != 0 && s.release.size > maximum).then(|| {
        Rejection::new(
            R::MaximumSizeExceeded,
            format!(
                "{} is too big, maximum size is {} (Settings->Indexers->Maximum Size)",
                size_suffix(s.release.size),
                size_suffix(maximum)
            ),
        )
    })
}

pub(crate) fn minimum_age(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if s.release.protocol != DownloadProtocol::Usenet || env.settings.minimum_age_minutes == 0 {
        return None;
    }
    let age = (env.settings.now - s.release.publish_date).num_milliseconds() as f64 / 60_000.0;
    (age < env.settings.minimum_age_minutes as f64).then(|| {
        Rejection::new(
            R::MinimumAge,
            format!(
                "Only {} minutes old, minimum age is {} minutes",
                format_number((age * 10.0).round_ties_even() / 10.0, 1),
                env.settings.minimum_age_minutes
            ),
        )
    })
}

/// Tanukarr's `MultiSeasonSpecification`: multi-season packs are accepted when enabled
/// (Settings › Indexers › Options) and when they map to episodes.
pub(crate) fn multi_season(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if !s.parsed.is_multi_season() {
        return None;
    }
    if !env.settings.enable_multi_season_releases {
        return Some(Rejection::new(
            R::MultiSeason,
            "Multi-season releases are not enabled",
        ));
    }
    s.episodes.is_empty().then(|| {
        Rejection::new(
            R::MultiSeason,
            "Multi-season release does not match any wanted episodes",
        )
    })
}

/// Tanukarr's `CompleteSeasonUpgradeSpecification` (RSS only): a release for seasons whose
/// monitored, aired episodes all have files is not grabbed from the feeds, so RSS never
/// upgrades complete seasons; searches keep the upgrade rules.
pub(crate) fn complete_season_upgrade(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if !env.criteria.is_rss()
        || s.episodes.is_empty()
        || s.episodes.iter().any(|e| e.monitored && e.file.is_none())
    {
        return None;
    }
    let now = env.settings.now;
    let seasons: std::collections::HashSet<i32> =
        s.episodes.iter().map(|e| e.season_number).collect();
    let open = env.series.episodes.iter().any(|e| {
        seasons.contains(&e.season_number)
            && e.monitored
            && e.file.is_none()
            && e.air_date_utc.is_some_and(|a| a <= now)
    });
    (!open).then(|| {
        Rejection::new(
            R::DiskNotUpgrade,
            "Automatic feed upgrades are disabled for fully downloaded seasons",
        )
    })
}

pub(crate) fn not_sample(_: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    (s.release.title.to_lowercase().contains("sample") && s.release.size < 70 * MB)
        .then(|| Rejection::new(R::Sample, "Sample"))
}

pub(crate) fn protocol(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let profile = env.delay_profile()?;
    match s.release.protocol {
        DownloadProtocol::Usenet if !profile.enable_usenet => Some(Rejection::new(
            R::ProtocolDisabled,
            "Usenet is not enabled for this series",
        )),
        DownloadProtocol::Torrent if !profile.enable_torrent => Some(Rejection::new(
            R::ProtocolDisabled,
            "Torrent is not enabled for this series",
        )),
        _ => None,
    }
}

pub(crate) fn quality_allowed_by_profile(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let quality = s.parsed.quality.quality;
    (!env.profile().allowed(quality.id)).then(|| {
        Rejection::new(
            R::QualityNotWanted,
            format!("{} is not wanted in profile", quality.name),
        )
    })
}

static DISC: LazyLock<[NetRegex; 3]> = LazyLock::new(|| {
    [
        NetRegex::compile(
            r"(?:dis[ck])(?:[-_. ]\d+[-_. ])(?:(?:(?:480|720|1080|2160)[ip]|)[-_. ])?(?:Blu\-?ray)",
            true,
        ),
        NetRegex::compile(
            r"(?:(?:480|720|1080|2160)[ip]|)[-_. ](?:full)[-_. ](?:Blu\-?ray)",
            true,
        ),
        NetRegex::compile(r"(?:\d?x?M?DVD-?[R59])(?:[ ._]|$)", true),
    ]
});

/// `RawDiskSpecification` (indexers here never report a container, so only the title counts).
pub(crate) fn raw_disk(_: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    DISC.iter()
        .any(|r| r.is_match(&s.release.title))
        .then(|| Rejection::new(R::Raw, "Raw Bluray/DVD release"))
}

/// A release-profile term: `/pattern/modifiers` is a regular expression, anything else a
/// case-insensitive substring (`TermMatcherService`, `PerlRegexFactory`).
enum Term {
    Text(String),
    Regex(NetRegex),
    Invalid(String),
}

fn term(t: &str) -> Term {
    static PERL: LazyLock<NetRegex> =
        LazyLock::new(|| NetRegex::compile(r"/(?<pattern>.*)/(?<modifiers>[a-z]*)", false));
    let Some(m) = PERL.find(t) else {
        return Term::Text(t.to_lowercase());
    };
    let pattern = m.value("pattern");
    let modifiers = m.value("modifiers");
    let mut ignore_case = false;
    for c in modifiers.chars() {
        match c {
            'i' => ignore_case = true,
            // Titles are one line, and captures are not used: these change nothing here.
            'm' | 's' | 'n' => {}
            'x' => {
                return Term::Invalid(format!(
                    "the x (ignore pattern whitespace) modifier of {t} is not supported yet"
                ));
            }
            other => {
                return Term::Invalid(format!(
                    "Unknown or unsupported perl regex modifier: {other}"
                ));
            }
        }
    }
    match NetRegex::new(&pattern, ignore_case) {
        Ok(r) => Term::Regex(r),
        Err(e) => Term::Invalid(format!("{t} is not a valid regular expression: {e}")),
    }
}

/// `ReleaseRestrictionsSpecification`.
pub(crate) fn release_restrictions(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let title = &s.release.title;
    let lower = title.to_lowercase();
    let profiles = env.release_profiles(s.release.indexer_id);
    let found = |terms: &[String]| -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for t in terms {
            let hit = match term(t) {
                Term::Text(x) => lower.contains(&x),
                Term::Regex(r) => r.is_match(title),
                Term::Invalid(e) => return Err(e),
            };
            if hit {
                out.push(t.clone());
            }
        }
        Ok(out)
    };
    for r in profiles.iter().filter(|r| !r.required.is_empty()) {
        match found(&r.required) {
            Err(e) => {
                return Some(Rejection::new(
                    R::DecisionError,
                    format!("ReleaseRestrictionsSpecification: {e}"),
                ));
            }
            Ok(hits) if hits.is_empty() => {
                return Some(Rejection::new(
                    R::MustContainMissing,
                    format!(
                        "Does not contain one of the required terms: {}",
                        r.required.join(", ")
                    ),
                ));
            }
            Ok(_) => {}
        }
    }
    for r in profiles.iter().filter(|r| !r.ignored.is_empty()) {
        match found(&r.ignored) {
            Err(e) => {
                return Some(Rejection::new(
                    R::DecisionError,
                    format!("ReleaseRestrictionsSpecification: {e}"),
                ));
            }
            Ok(hits) if !hits.is_empty() => {
                return Some(Rejection::new(
                    R::MustNotContainPresent,
                    format!("Contains these ignored terms: {}", hits.join(", ")),
                ));
            }
            Ok(_) => {}
        }
    }
    None
}

pub(crate) fn repack(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let propers = env.settings.propers;
    if !s.parsed.quality.revision.is_repack || propers == ProperDownloadType::DoNotPrefer {
        return None;
    }
    let group = s.parsed.release_group.as_deref().unwrap_or_default();
    for file in episode_files(s) {
        if !is_revision_upgrade(&file.quality, &model(s.parsed)) {
            continue;
        }
        if propers == ProperDownloadType::DoNotUpgrade {
            return Some(Rejection::new(
                R::RepackDisabled,
                "Repack downloading is disabled",
            ));
        }
        let file_group = file.release_group.as_deref().unwrap_or_default();
        if file_group.trim().is_empty() {
            return Some(Rejection::new(
                R::RepackUnknownReleaseGroup,
                "Unable to determine release group for the existing file",
            ));
        }
        if group.trim().is_empty() {
            return Some(Rejection::new(
                R::RepackUnknownReleaseGroup,
                "Unable to determine release group for this release",
            ));
        }
        if file_group.to_lowercase() != group.to_lowercase() {
            return Some(Rejection::new(
                R::RepackReleaseGroupDoesNotMatch,
                format!(
                    "Release is a repack for a different release group. Release Group: {group}. File release group: {file_group}"
                ),
            ));
        }
    }
    None
}

pub(crate) fn retention(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let days = (env.settings.now - s.release.publish_date).num_days();
    (s.release.protocol == DownloadProtocol::Usenet
        && env.settings.retention_days > 0
        && days > env.settings.retention_days)
        .then(|| Rejection::new(R::MaximumAge, "Older than configured retention"))
}

/// `SameEpisodesGrabSpecification`: files must not hold episodes the release lacks.
pub(crate) fn same_episodes_grab(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let ids: Vec<i64> = s.episodes.iter().map(|e| e.id).collect();
    let mut file_ids: Vec<i64> = episode_files(s).map(|f| f.id).collect();
    file_ids.dedup();
    let more = file_ids.iter().any(|fid| {
        env.series
            .episodes
            .iter()
            .any(|e| e.file.as_ref().is_some_and(|f| f.id == *fid) && !ids.contains(&e.id))
    });
    more.then(|| {
        Rejection::new(
            R::ExistingFileHasMoreEpisodes,
            "Episode file on disk contains more episodes than this release contains",
        )
    })
}

fn season_matches(s: &Subject<'_>, season: i32) -> bool {
    if s.parsed.season_numbers.is_empty() {
        s.mapped_season == Some(season)
    } else {
        s.parsed.season_numbers.contains(&season)
    }
}

/// `Search/SeasonMatchSpecification`.
pub(crate) fn season_match(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    match env.criteria.kind {
        SearchKind::Season { season } if !season_matches(s, season) => {
            Some(Rejection::new(R::WrongSeason, "Wrong season"))
        }
        _ => None,
    }
}

/// `Search/SeriesSpecification`: releases reach the specifications only once mapped to the
/// searched series (others are rejected when mapped), so this always accepts here.
pub(crate) fn series_spec(_: &Env<'_>, _: &Subject<'_>) -> Option<Rejection> {
    None
}

/// `Search/SingleEpisodeSearchMatchSpecification`.
pub(crate) fn single_episode_search_match(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    match env.criteria.kind {
        SearchKind::SingleEpisode { season, episode } => {
            if !season_matches(s, season) {
                return Some(Rejection::new(R::WrongSeason, "Wrong season"));
            }
            if s.parsed.episode_numbers.is_empty() {
                return Some(Rejection::new(R::FullSeason, "Full season pack"));
            }
            (!s.parsed.episode_numbers.contains(&episode))
                .then(|| Rejection::new(R::WrongEpisode, "Wrong episode"))
        }
        SearchKind::AnimeEpisode { season_search } => (s.parsed.full_season && !season_search)
            .then(|| Rejection::new(R::FullSeason, "Full season pack")),
        _ => None,
    }
}

pub(crate) fn split_episode(_: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    s.parsed
        .is_split_episode
        .then(|| Rejection::new(R::SplitEpisode, "Split episode releases are not supported"))
}

pub(crate) fn torrent_seeding(_: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if s.release.protocol != DownloadProtocol::Torrent || s.release.indexer_id == 0 {
        return None;
    }
    let (Some(seeders), Some(minimum)) = (s.release.seeders, s.release.minimum_seeders) else {
        return None;
    };
    (seeders < minimum).then(|| {
        Rejection::new(
            R::MinimumSeeders,
            format!("Not enough seeders: {seeders}. Minimum seeders: {minimum}"),
        )
    })
}

pub(crate) fn upgrade_allowed(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let profile = env.profile();
    for file in episode_files(s) {
        let (_, file_score) = env.file_formats(file);
        if !profile.is_upgrade_allowed(&file.quality, file_score, &model(s.parsed), s.score) {
            return Some(Rejection::new(
                R::QualityUpgradesDisabled,
                "Existing file and the Quality profile does not allow upgrades",
            ));
        }
    }
    None
}

/// `UpgradeDiskSpecification` with the default season-pack rule (every episode must be missing
/// or upgradable).
pub(crate) fn upgrade_disk(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let profile = env.profile();
    let new = model(s.parsed);
    let propers = env.settings.propers;
    if s.parsed.full_season {
        let total = s.episodes.len();
        if total == 0 {
            return None;
        }
        let mut upgraded = s.episodes.iter().filter(|e| e.file.is_none()).count();
        let existing: Vec<&EpisodeFile> = episode_files(s).collect();
        if existing.is_empty() {
            return None;
        }
        for file in existing {
            let (_, file_score) = env.file_formats(file);
            if !profile.cutoff_not_met(&file.quality, file_score, Some(&new)) {
                continue;
            }
            if profile
                .is_upgradable(&file.quality, file_score, &new, s.score, propers)
                .is_ok()
            {
                upgraded += 1;
            }
        }
        let percentage = upgraded as f64 / total as f64 * 100.0;
        if percentage >= 100.0 {
            return None;
        }
        return Some(Rejection::new(
            R::DiskCutoffMet,
            format!(
                "Season pack does not meet the upgrade criteria. Upgradable: {upgraded}/{total} ({}%), Mode: All, Threshold: 100%",
                format_number(percentage, 2)
            ),
        ));
    }
    for file in episode_files(s) {
        let (formats, file_score) = env.file_formats(file);
        if !profile.cutoff_not_met(&file.quality, file_score, Some(&new)) {
            let cutoff = profile.effective_cutoff();
            return Some(Rejection::new(
                R::DiskCutoffMet,
                format!(
                    "Existing file meets cutoff: {}",
                    profile.item_label(profile.index_of(cutoff, false).index)
                ),
            ));
        }
        let reason = match profile.is_upgradable(&file.quality, file_score, &new, s.score, propers)
        {
            Ok(()) => continue,
            Err(r) => r,
        };
        let p = &env.series.profile;
        let (code, message) = match reason {
            UpgradeRejectReason::BetterQuality => (
                R::DiskHigherPreference,
                format!(
                    "Existing file on disk is of equal or higher preference: {}",
                    quality_model_label(&file.quality)
                ),
            ),
            UpgradeRejectReason::BetterRevision => (
                R::DiskHigherRevision,
                format!(
                    "Existing file on disk is of equal or higher revision: {}",
                    revision_label(&file.quality.revision)
                ),
            ),
            UpgradeRejectReason::QualityCutoff => (
                R::DiskCutoffMet,
                format!(
                    "Existing file on disk meets quality cutoff: {}",
                    profile.item_label(profile.index_of(p.cutoff, false).index)
                ),
            ),
            UpgradeRejectReason::CustomFormatCutoff => (
                R::DiskCustomFormatCutoffMet,
                format!(
                    "Existing file on disk meets Custom Format cutoff: {}",
                    p.cutoff_format_score
                ),
            ),
            UpgradeRejectReason::CustomFormatScore => (
                R::DiskCustomFormatScore,
                format!(
                    "Existing file on disk has a equal or higher Custom Format score: {}",
                    profile.format_score(formats.iter().map(|f| &f.id))
                ),
            ),
            UpgradeRejectReason::MinCustomFormatScore => (
                R::DiskCustomFormatScoreIncrement,
                format!(
                    "Existing file on disk has Custom Format score within Custom Format score increment: {}",
                    p.min_upgrade_format_score
                ),
            ),
            UpgradeRejectReason::UpgradesNotAllowed => (
                R::DiskUpgradesNotAllowed,
                format!(
                    "Existing file on disk and Quality Profile '{}' does not allow upgrades",
                    p.name
                ),
            ),
        };
        return Some(Rejection::new(code, message));
    }
    None
}

/// `RssSync/MonitoredEpisodeSpecification`.
pub(crate) fn monitored_episode(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if !env.criteria.monitored_episodes_only {
        return None;
    }
    if !env.series.monitored {
        return Some(Rejection::new(
            R::SeriesNotMonitored,
            "Series is not monitored",
        ));
    }
    let monitored = s.episodes.iter().filter(|e| e.monitored).count();
    if monitored == s.episodes.len() {
        return None;
    }
    Some(if s.episodes.len() == 1 {
        Rejection::new(R::EpisodeNotMonitored, "Episode is not monitored")
    } else {
        Rejection::new(
            R::EpisodeNotMonitored,
            "One or more episodes is not monitored",
        )
    })
}

/// `QueueSpecification`: an episode already downloading at the same or better preference.
pub(crate) fn queue(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    let profile = env.profile();
    let ids: Vec<i64> = s.episodes.iter().map(|e| e.id).collect();
    let new = model(s.parsed);
    let p = &env.series.profile;
    for q in
        env.settings.queue.iter().filter(|q| {
            q.series_id == env.series.id && q.episode_ids.iter().any(|e| ids.contains(e))
        })
    {
        let queued_score = env.grab_score(GrabInput {
            title: &q.title,
            quality: &q.quality,
            size: q.size,
            indexer_flags: q.indexer_flags,
            languages: &q.languages,
            release_type: None,
        });
        let label = quality_model_label(&q.quality);
        if !profile.cutoff_not_met(&q.quality, queued_score, Some(&new)) {
            return Some(Rejection::new(
                R::QueueCutoffMet,
                format!("Release in queue already meets cutoff: {label}"),
            ));
        }
        let reason = profile
            .is_upgradable(
                &q.quality,
                queued_score,
                &new,
                s.score,
                env.settings.propers,
            )
            .err();
        let rejection = match reason {
            None => None,
            Some(UpgradeRejectReason::BetterQuality) => Some((
                R::QueueHigherPreference,
                format!("Release in queue is of equal or higher preference: {label}"),
            )),
            Some(UpgradeRejectReason::BetterRevision) => Some((
                R::QueueHigherRevision,
                format!(
                    "Release in queue is of equal or higher revision: {}",
                    revision_label(&q.quality.revision)
                ),
            )),
            Some(UpgradeRejectReason::QualityCutoff) => Some((
                R::QueueCutoffMet,
                format!(
                    "Release in queue meets quality cutoff: {}",
                    profile.item_label(profile.index_of(p.cutoff, false).index)
                ),
            )),
            Some(UpgradeRejectReason::CustomFormatCutoff) => Some((
                R::QueueCustomFormatCutoffMet,
                format!(
                    "Release in queue meets Custom Format cutoff: {}",
                    p.cutoff_format_score
                ),
            )),
            Some(UpgradeRejectReason::CustomFormatScore) => Some((
                R::QueueCustomFormatScore,
                format!(
                    "Release in queue has an equal or higher Custom Format score: {queued_score}"
                ),
            )),
            Some(UpgradeRejectReason::MinCustomFormatScore) => Some((
                R::QueueCustomFormatScoreIncrement,
                format!(
                    "Release in queue has Custom Format score within Custom Format score increment: {}",
                    p.min_upgrade_format_score
                ),
            )),
            Some(UpgradeRejectReason::UpgradesNotAllowed) => Some((
                R::QueueUpgradesNotAllowed,
                format!(
                    "Release in queue and Quality Profile '{}' does not allow upgrades",
                    p.name
                ),
            )),
        };
        if let Some((code, message)) = rejection {
            return Some(Rejection::new(code, message));
        }
        if is_revision_upgrade(&q.quality, &new)
            && env.settings.propers == ProperDownloadType::DoNotUpgrade
        {
            return Some(Rejection::new(
                R::QueuePropersDisabled,
                "Proper downloading is disabled",
            ));
        }
    }
    None
}

/// `RssSync/HistorySpecification`: an episode's most recent event is a grab from the last 12
/// hours at the same or better preference. (Completed download handling is always on in
/// Kitsunarr, so older grabs do not count.)
pub(crate) fn history(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if !env.criteria.is_rss() {
        return None;
    }
    let profile = env.profile();
    let p = &env.series.profile;
    let new = model(s.parsed);
    let since = env.settings.now - chrono::Duration::hours(12);
    for e in &s.episodes {
        let Some(g) = env
            .settings
            .last_grabs
            .iter()
            .find(|g| g.episode_id == e.id && g.date > since)
        else {
            continue;
        };
        let score = env.grab_score(GrabInput {
            title: &g.title,
            quality: &g.quality,
            size: g.size,
            indexer_flags: g.indexer_flags,
            languages: &g.languages,
            release_type: Some(g.release_type),
        });
        let label = quality_model_label(&g.quality);
        if !profile.cutoff_not_met(&g.quality, score, Some(&new)) {
            return Some(Rejection::new(
                R::HistoryRecentCutoffMet,
                format!("Recent grab event in history already meets cutoff: {label}"),
            ));
        }
        let rejection = match profile
            .is_upgradable(&g.quality, score, &new, s.score, env.settings.propers)
            .err()
        {
            None => continue,
            Some(UpgradeRejectReason::BetterQuality) => (
                R::HistoryHigherPreference,
                format!("Recent grab event in history is of equal or higher preference: {label}"),
            ),
            Some(UpgradeRejectReason::BetterRevision) => (
                R::HistoryHigherRevision,
                format!(
                    "Recent grab event in history is of equal or higher revision: {}",
                    revision_label(&g.quality.revision)
                ),
            ),
            Some(UpgradeRejectReason::QualityCutoff) => (
                R::HistoryCutoffMet,
                format!(
                    "Recent grab event in history meets quality cutoff: {}",
                    profile.item_label(profile.index_of(p.cutoff, false).index)
                ),
            ),
            Some(UpgradeRejectReason::CustomFormatCutoff) => (
                R::HistoryCustomFormatCutoffMet,
                format!(
                    "Recent grab event in history meets Custom Format cutoff: {}",
                    p.cutoff_format_score
                ),
            ),
            Some(UpgradeRejectReason::CustomFormatScore) => (
                R::HistoryCustomFormatScore,
                format!(
                    "Recent grab event in history has an equal or higher Custom Format score: {score}"
                ),
            ),
            Some(UpgradeRejectReason::MinCustomFormatScore) => (
                R::HistoryCustomFormatScoreIncrement,
                format!(
                    "Recent grab event in history has Custom Format score within Custom Format score increment: {}",
                    p.min_upgrade_format_score
                ),
            ),
            Some(UpgradeRejectReason::UpgradesNotAllowed) => (
                R::HistoryUpgradesNotAllowed,
                format!(
                    "Recent grab event in history and Quality Profile '{}' does not allow upgrades",
                    p.name
                ),
            ),
        };
        return Some(Rejection::new(rejection.0, rejection.1));
    }
    None
}

/// `RssSync/DelaySpecification`: the delay profile's waiting period, for RSS sync and the
/// searches the server starts itself; user-started searches skip it.
pub(crate) fn delay(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if !env.criteria.is_rss() && env.criteria.user_invoked {
        return None;
    }
    let d = env.delay_profile()?;
    let delay = match s.release.protocol {
        DownloadProtocol::Usenet => d.usenet_delay,
        DownloadProtocol::Torrent => d.torrent_delay,
        DownloadProtocol::Unknown => 0,
    };
    if delay == 0 {
        return None;
    }
    let preferred = s.release.protocol == d.preferred_protocol;
    let profile = env.profile();
    let new = model(s.parsed);
    if preferred && env.settings.propers == ProperDownloadType::PreferAndUpgrade {
        for file in episode_files(s) {
            if profile.compare_qualities(new.quality_id, file.quality.quality_id)
                == std::cmp::Ordering::Equal
                && crate::profile::compare_revision(&new.revision, &file.quality.revision)
                    == std::cmp::Ordering::Greater
            {
                return None;
            }
        }
    }
    if d.bypass_if_highest_quality
        && preferred
        && let Some(best) = profile.last_allowed_quality()
        && profile.compare_qualities(new.quality_id, best) != std::cmp::Ordering::Less
    {
        return None;
    }
    if d.bypass_if_above_custom_format_score
        && preferred
        && s.score >= d.minimum_custom_format_score
    {
        return None;
    }
    let age = |published: chrono::DateTime<chrono::Utc>| {
        (env.settings.now - published).num_milliseconds() as f64 / 60_000.0
    };
    // `OldestPendingRelease`: a release for these episodes already held back longer than the
    // delay lets this one through.
    let oldest = env
        .settings
        .pending
        .iter()
        .filter(|p| {
            p.series_id == env.series.id
                && p.episode_ids
                    .iter()
                    .any(|id| s.episodes.iter().any(|e| e.id == *id))
        })
        .map(|p| p.publish_date)
        .min();
    if oldest.is_some_and(|p| age(p) > delay as f64) {
        return None;
    }
    // `Release.AgeMinutes`.
    let age_minutes = age(s.release.publish_date);
    (age_minutes < delay as f64)
        .then(|| Rejection::new(R::MinimumAgeDelay, "Waiting for better quality release"))
}

/// `RssSync/ProperSpecification`: propers and repacks for files on disk, per the "propers and
/// repacks" setting; not for files added more than a week ago.
pub(crate) fn proper(env: &Env<'_>, s: &Subject<'_>) -> Option<Rejection> {
    if !env.criteria.is_rss() || env.settings.propers == ProperDownloadType::DoNotPrefer {
        return None;
    }
    let new = model(s.parsed);
    // `DateTime.Today.AddDays(-7)`.
    let week_ago = (env.settings.now.date_naive() - chrono::Days::new(7))
        .and_hms_opt(0, 0, 0)
        .map(|d| d.and_utc());
    for file in episode_files(s) {
        if is_revision_upgrade(&file.quality, &new) {
            if env.settings.propers == ProperDownloadType::DoNotUpgrade {
                return Some(Rejection::new(
                    R::PropersDisabled,
                    "Proper downloading is disabled",
                ));
            }
            if let (Some(added), Some(limit)) = (file.date_added, week_ago)
                && added < limit
            {
                return Some(Rejection::new(R::ProperForOldFile, "Proper for old file"));
            }
        }
    }
    None
}
