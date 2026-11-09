> Audit notes produced during Kitsunarr's source analysis (read-only review of the GPL-3.0 Sonarr and Tanukarr sources, November 2026). Line references point at the audited revisions; see `../source-audit.md` for the summary.

# Tanukarr acquisition / matching / quality / import / integrations audit (for Kitsunarr)

Audit date: 2026-11-09. Read-only source audit; nothing was built or run.
Scope: what Tanukarr changed relative to Sonarr in acquisition, release matching, quality,
import, download handling, Jellyfin, import lists, library tools and authentication, and
whether each change is wired in backend code. Metadata providers / ordering / grouping /
movie identity are covered by a separate audit and are only referenced here where they feed
these features.

## 0. Method and baseline (read this first)

* Trees: `ref/tanukarr` (HEAD `24382f1`, product version file `VERSION` = 0.1.5; the docs also
  use a parallel 1.0.x/1.1.0 numbering for the same lineage) and `ref/sonarr` (upstream
  `develop` at `c018578`, 2026-11-02, shallow single commit).
* **The upstream reference is newer than Tanukarr's base.** Tanukarr's git history is a real
  branch: it forked from upstream `59c22e8` (2026-10-12), stacked all fork commits
  (`390437b` "Preserve Sonarr Pro features..." onward), then merged upstream `57816c1`
  (commit `2d9ab41`) and later cherry-picked ~20 individual upstream fixes. Therefore:
  * `git diff 57816c149 HEAD` (inside `ref/tanukarr`) is the **fork delta** (+ cherry-picks),
    and is what this report uses to attribute changes to Tanukarr.
  * Many entries in `srcdiff.txt` (Tanukarr vs upstream `c018578`) are **upstream changes made
    after the fork base that Tanukarr does not have**. All `Only in sonarr/...` entries are of
    this kind, e.g. upstream's own multi-season-pack import support
    (`EpisodeImport/Specifications/MultiSeasonPackSpecification.cs`,
    `Parser/SeasonNumbersFormatter.cs`, migration `237_pending_releases_season_numbers`,
    `Sonarr.Api.V5/Queue/QueueSeasonResource.cs`), `Download/DownloadClientFile.cs`,
    `QBittorrentSeedTimeType.cs`, SABnzbd response classes, `RejectedFileExtensionService.cs`,
    TMDB import-list token exchange (`TmdbTokenResources.cs`), migration
    `240_series_title_slug_not_unique` (Tanukarr ports it as 254). Several "changed" files in
    `srcdiff.txt` (e.g. Deluge, Transmission, SABnzbd, NZBGet clients; the `Advanced = true`
    field flags on providers) are also upstream drift, not fork work.
* **Migration numbering diverges from upstream.** Upstream: 233 quality_profile_quality_ranks,
  234 add_pending_path_to_series, 236 import_list_tag_existing_type, 237
  pending_releases_season_numbers, 240 series_title_slug_not_unique. Tanukarr: 233-235 are fork
  migrations (metadata source / episode ordering / alternate titles), upstream 233 is
  renumbered **236**, upstream 234 is **238**, upstream 236 is **241**, upstream 240 is **254**,
  and upstream 237 is absent. A database that real Sonarr has migrated past 232 cannot be
  safely opened by Tanukarr (and vice versa). Kitsunarr must not assume Sonarr/Tanukarr schema
  compatibility above 232.
* "Status" vocabulary: **Operational** = implemented in backend and reachable from a normal
  runtime path (registered, scheduled, or called by an API/command); **Partial** = implemented
  but with a reachable gap or a documented behavior that the code does not deliver;
  **UI-only** = surfaced in UI/docs with no effective backend; **Not implemented**.
  "Runtime-verified" is never claimed: no binaries were run in this audit. Tanukarr's own docs
  claim container/runtime checks (docker/tests), which were not re-executed.
* Sections 11-12, 13, 14-15 (including 14.6b, retained versions) were produced by parallel sub-audits and merged verbatim
  below; their claims were spot-checked where they intersect sections 1-10.

## 1. Absolute-numbered releases on standard-ordered series

**Bottom line: the core mechanism is inherited from upstream Sonarr. Tanukarr's own work here
supplies absolute numbers for non-TVDB catalogues and adds several matching guards.**
FORK.md presents "Tensei Shitara Slime Datta Ken - 88 → S04E16" as a fork capability. In both
the fork base and current upstream, `ParsingService.GetEpisodes` sends every
`ParsedEpisodeInfo.IsAbsoluteNumbering` release to `GetAnimeEpisodes(...)`, whatever the
`SeriesType` (base `57816c149` ParsingService.cs:330; upstream `c018578` :292). That method ends
in `_episodeService.FindEpisode(series.Id, absoluteEpisodeNumber)`, a lookup on
`Episodes.AbsoluteEpisodeNumber`. An absolute release therefore resolves to SxxEyy on any series
whose episode rows carry absolute numbers. TheTVDB (via SkyHook) supplies those where TVDB has
them, which is common for anime.

What Tanukarr actually adds (all in `src/NzbDrone.Core/Parser/ParsingService.cs` unless noted):

| Change | Rule (reimplementable) | Evidence |
| --- | --- | --- |
| Absolute numbers for non-TVDB owners | `TmdbProxy.SetAbsoluteNumbering`: episodes with `SeasonNumber > 0`, ordered by (season, episode), numbered 1..N. Specials get none. AniList/Jikan mappers set absolute = native episode number (Jikan: `MalId > 0 ? MalId : number`). Belongs to the metadata audit; the matcher depends on it. | `MetadataSource/Tmdb/TmdbProxy.cs:214-227`, `AniList/AniListEpisodeMapper.cs:50`, `Jikan/JikanProxy.cs:259,279` |
| Native numbering bypasses scene/XEM | `nativeNumbering = series.TvdbId < 0 && series.MetadataSource != Tvdb && !series.UseSceneNumbering && MatchesNativeSeriesTitle(series, parsed)`. When true, `GetAnimeEpisodes` skips `GetSceneSeasonNumber`/`FindEpisodesBySceneNumbering` and goes straight to `FindEpisode(seriesId, absolute)`. `Map()` also sets `sceneMapping = null`, so there are no alias season offsets. `MatchesNativeSeriesTitle`: a parsed year, if present, must equal `series.Year`; `CleanSeriesTitle()` of the parsed title (or title-without-year) must be in {Title} ∪ AlternateTitles (cleaned). | `GetAnimeEpisodes`, `Map`, `MatchesNativeSeriesTitle` |
| Trailing season suffix in series lookup | `FindByTitleWithoutSeasonSuffix`: requires `parsed.SeasonNumber > 0` and no parsed year. Matches `TrailingSeasonRegex = \s+(?<season>\d{1,2})$` on `SeriesTitle`; the number must equal the parsed season. Strips it, then tries `SceneMappingService.FindTvdbId(stripped, releaseTitle, season)` → `FindByTvdbId`, else `FindByTitle(stripped)`. Called last in `GetSeries` and in `FindSeries` (before the TVDB-ID fallback). Example: "Tensei Shitara Slime Datta Ken 4 - S04E16" → series "Tensei Shitara Slime Datta Ken". It deliberately does **not** apply to absolute releases ("… 4 - 88" returns null, per test). | `TrailingSeasonSuffixFixture` |
| New anime regex (trailing subgroup) | Inserted before the generic multi-season regex: `^(?<title>.+?[ ._]S(?<season>\d{1,2})) +\- +(?<absoluteepisode>\d{2,3})(?![\d.])(?:v\d+)? +\[(?<subgroup>(?!\d{3,4}[pi]\])[A-Za-z][^\]]*)\]\s*\[\s*\]`. It requires both the `[Group]` and the (stripped) `[1080p]` bracket pair. Result: SeriesTitle keeps the alias ("Shokugeki no Soma S2"), SeasonNumber=2, absolute=10. | `Parser/Parser.cs:220-227`, `AnimeTrailingGroupSeasonFixture` |
| Scene-vs-aired numbering resolver | `EpisodeNumberingResolver` (new, 370 lines) only acts when `series.UseSceneNumbering && SeasonType == Official`, the release is not absolute/daily/special, its season is > 0, and some episode has scene≠aired numbers. It computes the "aired" and "scene" candidate sets (alias offset `mapping.SeasonNumber - mapping.SceneSeasonNumber`, XEM S1 special case). Evidence used to pick one: a unique episode-title match after the SxxEyy token (titles under 6 characters, "episode …" and "part …" ignored; only technical suffixes such as 1080p/web/x265 allowed after the title), then the scene-mapping origin (`tvdb`→aired, `scene`→scene), then batch inference over a pack's files (≥2 distinct title-verified episodes per season, conflicting votes → Ambiguous). Without evidence it returns `NeedsEvidence` + rejection "Ambiguous scene/library numbering…". Not applied to `CompleteSeriesSearchCriteria`. Feeds `RemoteEpisode.NumberingRejection` → `EpisodeNumberingSpecification` (§8), and `LocalEpisode.NumberingRejection` → import `AmbiguousNumbering`. | `Parser/EpisodeNumberingResolver.cs`, `EpisodeNumberingResolverFixture` (434 lines), `AmericanDadNumberingReplayFixture` |

* Settings/schema/API/UI: none. These paths are always on.
* Status: **Operational**. The absolute-number fallback is inherited. The fork pieces are wired
  through DI-constructed `ParsingService`, `AggregateEpisodes` and `ImportDecisionMaker`.
* Tests: `TrailingSeasonSuffixFixture`, `AnimeTrailingGroupSeasonFixture`,
  `NativeProviderSceneIsolationFixture`, `MultiSeasonSceneMappingFixture`, `GetEpisodesFixture`,
  `EpisodeNumberingResolverFixture`, `AmericanDadNumberingReplayFixture`.
* Caveats:
  * For TMDB-owned series, absolute numbers are a cumulative count over TMDB seasons. Where a
    release group's absolute numbering differs (recaps, split cours, specials counted), the
    mapping is silently wrong. UNVERIFIED against real catalogues.
  * The doc's reverse statement holds: series with an absolute ordering (`tmdb:absolute`) have
    no S04 rows, so `S04E16` maps to no episode.

### 1b. Cross-cutting matching regression: Sonarr scene-mapping feed disabled
* `NzbDrone.Common/Cloud/SonarrCloudRequestPolicy.cs` (new) runs as `PostProcess` on the
  `services.sonarr.tv` request builder and in `HttpClient`. It blocks the hosts
  `services.sonarr.tv` and `sentry.sonarr.tv` and returns a synthetic 403.
* Consequences for release matching:
  * `SceneMappingProxy` (via `ServicesProvider`) no longer downloads Sonarr's curated scene-name
    and season mappings. `SceneMappingService` catches the block and keeps whatever rows already
    exist in the DB. A **fresh install has no "services" scene mappings at all**, only XEM
    (`thexem.info`) mappings for series with a positive TVDB id. Many anime alias titles and
    season-offset mappings that upstream users get automatically are therefore absent.
  * `DailySeriesDataProxy` is blocked as well, so automatic Daily-type detection for new series
    is lost.
  * Also blocked: the `SystemTimeCheck` and `ProxyCheck` health probes, and update checks.
* Documentation: only `docs/integration-2026-10-12.md` ("Disabled shared Sonarr cloud
  services") mentions this. FORK.md/README instead say matching "gained more names to compare
  against exactly", which is true for provider alternate titles but omits the lost scene feed.
* Status: **Operational** (deliberate block).
* Impact on matching: UNVERIFIED in size; it depends on how many releases need the services
  mappings.
* Kitsunarr: decide explicitly whether to consume the Sonarr scene-mapping feed. Its terms of
  use are not covered by this audit.

## 2. Multi-season packs, complete-series packs and complete-series search

### 2a. Context: what upstream already had and has now
* Upstream at the fork base already parsed the full season list:
  * `ParsedEpisodeInfo.SeasonNumbers` (Ion Bazan `368a40a`, 2026-09-11, "Parse full range for
    multi-season packs").
  * Episode mapping across all those seasons (`b0bde26`, 2026-10-11).
  * Its `MultiSeasonSpecification` still rejected every multi-season release.
* Upstream since the base (`c018578`) supports multi-season packs **from interactive grabs
  only**:
  * Automatic grabs are still rejected: "not grabbed automatically. Grab it from interactive
    search to import all seasons".
  * It imports **all** seasons per file via `EpisodeImport/Specifications/MultiSeasonPackSpecification`.
  * It stores `PendingReleases.ParsedEpisodeInfo.seasonNumbers` (migration 237).
  * It adds `QueueSeasonResource` and `IDownloadClient.GetFiles()`.

  Tanukarr has none of this. Its design differs (automatic acceptance, grabbed-episodes-only
  import, copy and retain the source).
* FORK.md's "Upstream rejects `Series.S01-S05` outright … discarded all but the first" describes
  upstream as of ~August 2026 and is stale.

### 2b. Parser changes (`Parser/Parser.cs`)
* The `[SubGroup] Multi-season pack` regex and the generic `Multi-season pack` regex gain named
  groups `seasonseparator` and `completeseries`
  (`^(?<title>.+?)(?<completeseries>Complete Series)?[-_. ]+(?:S|(?:Season|Saison|Series|Stagione)[_. ])(?<season>…)(?:(?<seasonseparator>[-_. ]{1}|[-_. ]{3})(?:S|…)?(?<season>…)){1,}`).
* New range rule in `ParseMatchCollection`. It expands to `Enumerable.Range(min, max-min+1)`
  **only if** there are exactly 2 season captures, they are ascending (`s0 <= s1`), **and**
  either the first match's `seasonseparator` contains `-` or `Complete Series` matched.
  Otherwise the seasons are the distinct sorted values. Examples from the fixtures:
  * `S01-S05`, `S02-05`, `Season 02 - Season 05`, `S01 - S07` → contiguous range.
  * `S01 S04` → **[1,4]**. Upstream expands this to [1,2,3,4].
  * `Complete Series S01 S04` → [1..4].
  * `S03-S03` → [3].
* `ParsedEpisodeInfo` adds `IsCompleteSeries` (set only by `CompleteSeriesReleaseParser`).
  `SeasonNumber` is a computed alias of `SeasonNumbers[0]`. `IsMultiSeason = SeasonNumbers.Count > 1`.

### 2c. Decision: `MultiSeasonSpecification` (permanent rejection type)
1. Not multi-season → accept.
2. Config `EnableMultiSeasonReleases` (DB `Config` key `enablemultiseasonreleases`, **default
   true**) false → reject `MultiSeason` "Multi-season releases are not enabled".
3. `RemoteEpisode.Episodes` null or empty → reject "Multi-season release does not match any
   wanted episodes".
4. Otherwise accept.

The doc says "a Seasons 1-9 pack is not accepted for a Season 3 search". Strictly, the spec
only requires non-empty mapped episodes. In a season search, `ParsingService` scopes the mapping
to the requested season, so an S01–S09 pack **is** accepted for a season-3 search, as a
season-3 grab. It is rejected only when the pack does not contain the requested season.

Exposure:
* API v3 `GET/PUT /api/v3/config/indexer` → `enableMultiSeasonReleases`.
* API v5 `settings/indexer` → `IndexerSettingsResource.EnableMultiSeasonReleases`.
* UI: Settings → Indexers → Options checkbox.

### 2d. Episode mapping for packs (`ParsingService`)
`GetEpisodes(parsed, series, mappedSeason, sceneSource, criteria)`, full-season branch:
* Criteria is `CompleteSeriesSearchCriteria` for the same series and the pack is FullSeason →
  return `criteria.Episodes` whose season is in `parsed.SeasonNumbers`.
* Multi-season pack in a `SeasonSearchCriteria`, series not scene-numbered, and the pack
  contains the requested season → that season's episodes only.
* Otherwise, for each pack season:
  * If not scene source, or `SeriesType != Official` ordering → episodes by season.
  * Else apply that season's own scene mapping via `GetSceneSeasonMapping`: offset
    `mapping.SeasonNumber - mapping.SceneSeasonNumber` when `mapping.SceneSeasonNumber <= season`;
    XEM special case for S1. Then look up by scene season (fallback to aired season).
  * `DistinctBy(Id)`; if the criteria is a season search, filter to that season.

`Map()` also runs the numbering resolver (not for complete-series criteria). For multi-season
plus `SeasonSearchCriteria` it re-filters to the requested season, and sets `MappedSeasonNumber`
when one season remains. v5 `POST /release` additionally calls `RemoteEpisode.ForSeason(searchInfo.seasonNumber)`
(clone filtered to one season) when grabbing a multi-season release found by a season search.

### 2e. `CompleteSeriesReleaseParser` (new, used only inside complete-series contexts)
`Parse(title, CompleteSeriesSearchCriteria c)`:
1. `expectedSeasons` = distinct `c.Episodes.SeasonNumber > 0`. None → null.
2. Normalize the title:
   * strip a leading `[group]`;
   * remove `'`, backtick and `’`;
   * replace `&` with `and`;
   * remove diacritics.
3. Title prefix: for each alias in `c.SceneTitles` (longest first), build
   `^(?:the[\W_]+)?w1[\W_]+w2…(?=$|[\W_])(?<suffix>.*)$` from `GetCleanSceneTitle(alias)` words.
   On a match, keep the suffix and the matched title.
4. Collection fallback, if no alias matched and the series is a grouped collection member: the
   same match on `c.CollectionTitle`, with seasons later translated through `c.CollectionSeasons`
   (collection slot → native season).
5. Year handling. The description is the suffix with `[._\[\]()]` turned into spaces.
   * If it starts with a year `(19|20)\d{2}`, the year must equal `Series.Year`. Via the
     collection title it must instead be ≤ `Series.Year`.
   * The year is appended to the matched title, then stripped from the description.
6. Rejection rules:
   * Reject if the description matches the episode regex
     `\bs\d+\s*e\d+|\b\d+x\d+\b|\b(?:episodes?|parts?|volumes?|cours?)(?=\b|\d)`.
   * Bare single-season pack: the description starts with a quality token (bd, bluray, remux,
     web, hdtv, 1080p, x265, hevc, 10bit, dual-audio, …). It has no seasons/series/complete
     word, no `\d+-\d+`, no season token. Exactly one expected season, equal to the series'
     only regular season.
   * Otherwise the description must start with a "complete" label
     (`^(?:the\s+)?(?:complete(?:\s+(?:series|collection))?|(?:entire|full)\s+series|all\s+seasons|box\s*set)\b`).
     Or its first word is a season token, or it starts with `season(s)/series N`. Or, for
     Anime series only, it starts with an absolute range `^\s*(\d{1,4})\s*(-|~|–|—|to)\s*(\d{1,4})`.
7. Anime absolute range:
   * `first > 0` and `last >= first`.
   * **Every** expected episode must have an absolute number within [first, last].
   * Not allowed via the collection title.
8. Season tokens:
   * `\b(?:s|seasons?|series)\s*(\d{1,3})(?![a-z0-9])`.
   * Ranges `\b(?:s|seasons?|series)\s*(\d{1,3})\s*(-|–|—|to)\s*(?:s|seasons?|series)?\s*(\d{1,3})`
     are unioned in; a reversed range → null.
   * Via collection: there must be ≥1 season and all must be collection slots.
   * Any season not in `expectedSeasons` → null.
   * In automatic mode (`IncludeSeasonPacks=false`), all expected seasons must be covered.
9. Final checks:
   * No seasons, no absolute range, not bare: requires a complete label and **no** word "season".
   * Result: `Parser.ParseTitle("{matchedTitle} S{first:00}")`, then overwrite `ReleaseTitle`,
     `ReleaseTokens = suffix`, quality, languages (from suffix), release group, `SeasonNumbers`,
     `FullSeason = true`, `IsCompleteSeries = (covered == expected)`.

`HasMappedSeasonCoverage` is the grab-time guard: same series, FullSeason, not partial, no
episode/absolute numbers, all seasons > 0, every mapped episode in a listed season, and every
listed season represented.

### 2f. Complete-series search
* Entry point: `ReleaseSearchService.CompleteSeriesSearch(seriesId, userInvoked, interactive)`.
  * Movies go to `MovieSearch`.
  * Episodes = all regular episodes, monitored or not. None → `SearchFailedException`.
  * Criteria: `CompleteSeriesSearchCriteria` with `MonitoredEpisodesOnly = !interactive` and
    `IncludeSeasonPacks = interactive`; `UseCollectionNumbering()`.
  * `SceneTitles = {Title} ∪ scene names ∪ AlternateTitles`.
  * `QueryTitles` = first 3 distinct clean titles.
  * `PackQueryTitles` = per title `"{t} S01-S{last:00}"` (if last > 1) and `"{t} complete"`.
  * `SeasonPackQueryTitles` (interactive only) = `"{t0} S{n:00}"` per season.
  * `CollectionQueryTitles` = `"{collectionTitle} S{slot:00}"`.
* `Dispatch` restricts complete-series criteria to **torrent indexers only**. If none are
  enabled it throws "No torrent indexers are enabled for … search". FORK.md and README do not
  say this; the it/en help text implies it.
* Per-indexer requests:
  * `IIndexer.Fetch(CompleteSeriesSearchCriteria)` (new interface member). `IndexerBase` default
    returns empty; `HttpIndexerBase` uses the generator.
  * `IIndexerRequestGenerator` gets a **default interface method** that converts to
    `SpecialEpisodeSearchCriteria` with `EpisodeQueryTitles = QueryTitles ∪ CollectionQueryTitles`
    (generic text search for every other generator).
  * Newznab/Torznab override it: ID-based TV search, plus `q=` for each QueryTitle, plus `q=`
    for each pack/season-pack/collection query (when `SupportsSearch`). That is up to ~10
    request chains per indexer per series in automatic mode.
  * Nyaa overrides `Fetch`. It runs title queries first, then supplemental queries:
    * pack queries only if no complete-series result;
    * missing single seasons (interactive);
    * collection titles.
    Results are de-duplicated by Guid.
* `DownloadDecisionMaker` with complete-series criteria:
  * Parses with `CompleteSeriesReleaseParser` only and silently skips titles it cannot parse.
  * Rejects `WrongSeries` "Release identifiers conflict with searched series" when the release
    carries a TVDB / TVRage / IMDb id conflicting with the series (for native non-TVDB series
    any TVDB id conflicts).
  * `ParsingService.FindSeries` short-circuits to the searched series for native (non-TVDB)
    series via `MatchesNativeCompleteSeries`.
* Interactive use:
  * `GET /api/v3|v5/release?seriesId=N&completeSeries=true`. Any other parameter → 400.
  * Results are cached for 30 min under key `"{indexerId}_{guid}_complete_{seriesId}"`.
  * Grab: v3 `POST /release` with `completeSeries=true` and `seriesId` only; v5 `searchInfo.completeSeries`.
    `HasMappedSeasonCoverage` is re-verified → 400 "search again" if it fails.
  * UI: "Search complete series" button on the series page (`CompleteSeriesSearchModal`) and in
    collection pages.
* Automatic use (always on; there is **no setting**):
  * `SeriesSearchService` (Search Monitored / series search): if the number of monitored seasons
    with ≥1 monitored, aired, missing, not-queue-covered episode is > 1, or = 1 for a
    single-regular-season series, it runs `CompleteSeriesSearch(auto)` first and processes it.
    `SearchFailedException` (e.g. no torrent indexer) → warn and continue.
  * `EpisodeSearchService` missing searches (`MissingEpisodeSearchCommand`, so also "Search all
    missing"): for every series in the batch with ≥2 distinct missing regular seasons, or a
    single-season series, it tries a complete-series search first. Any exception → warn and
    continue. On a large library this multiplies indexer traffic.
  * `CompleteSeriesSearchCommand { SeriesId, DryRun }` (DryRun logs candidates, no grab) exists
    but has no UI or scheduler trigger. It is reachable only via `POST /api/v3/command`.
* Practical limit of automatic complete-series grabs, inferred from the inherited specs: the
  pack maps to **all** regular episodes, so it is still rejected by:
  * the RSS/Search `MonitoredEpisodeSpecification` ("One or more episodes is not monitored")
    when any regular episode is unmonitored;
  * `FullSeasonSpecification` when an episode has not aired within 24 h;
  * the upgrade/disk specs when existing files already meet the profile.

  In practice it fires for fully monitored, finished series with nothing on disk.

### 2g. Pack-first ranking and "skip complete monitored seasons"
* `DownloadDecisionComparer` inserts `CompareReleaseScope` as the **first** comparer, before
  quality and custom-format score: multi-season full pack = 2 > single-season pack = 1 > anything
  else = 0. It also adds: when both releases are full-season, more mapped episodes wins.
  Consequence: among approved releases, a lower-quality multi-season pack outranks a
  higher-quality season pack or episode. Applies to RSS, automatic and interactive ordering.
  Tests: `PrioritizeDownloadDecisionFixture`.
* `RssSync/CompleteSeasonUpgradeSpecification` (new, always on): for non-search decisions (RSS,
  pushed releases) it rejects `DiskNotUpgrade` "Automatic feed upgrades are disabled for fully
  downloaded seasons". It fires when no episode in the release is (monitored ∧ missing), and none
  of the release's seasons has a monitored, missing, aired (`AirDateUtc <= now`) episode. Effect:
  **RSS never upgrades files in complete seasons.**
* `SeriesSearchService` season loop:
  * Skips any monitored season without a monitored, aired, missing episode, **regardless of
    trigger**. A manual "Search Monitored" on the series no longer upgrades complete seasons.
    FORK.md says "Explicit manual searches retain the normal upgrade rules", which is only true
    for season/episode/cutoff searches.
  * Also skips seasons whose missing episodes are all covered by grabs from this run or by
    protected queue items.
  * Anime seasons: `AnimeSeasonPackSearch` (packs only) first, then per-episode search for aired
    episodes not covered.
* `ReleaseSearchService.SearchAnimeSeason` (all anime season searches) drops upstream's rule
  "only search individual episodes when the season is not fully aired". It now always runs
  per-episode searches too, except when `ProOptions.PreferAnimeSeasonPacks` (Library tools,
  default false) is on and an **approved** full-season decision covers the episode. With the
  default this is **more** indexer traffic than upstream.
* `EpisodeSearchService.SearchForBulkEpisodes`:
  * re-reads episodes before each season group and drops queued, grabbed-this-run and
    has-file (for missing) episodes;
  * orders regular seasons before specials.

### 2h. Queue coverage per actual season
* `Queue/ActiveMultiSeasonCoverage.IsProtected(q)`: state Downloading, ImportPending or
  Importing; `FullSeason`; mapped episodes span ≥2 seasons.
* `QueueSpecification` (non-interactive decisions only) rejects `QueueHigherPreference`
  "Episodes are already covered by an active multi-season download" when a protected item
  overlaps and the candidate is not full-season, or spans ≤1 season, or its episodes are a
  subset of the queued pack.
* v5 `QueueResource`:
  * `SeasonNumbers` is derived from the mapped episodes' seasons (not the single parsed season).
  * Adds `EpisodeIdsBySeason` (season → ids), `EpisodeIdsWithFiles` and `ImportProgress`.
  * Upstream added `QueueSeasonResource` for a similar purpose after the base; Tanukarr lacks it.

### 2i. Import of multi-season / complete downloads
* Source retention:
  * `CompletedDownloadService.Check`: if the client title parses as multi-season, or the tracked
    `RemoteEpisode` is multi-season or complete-series, it forces
    `CanMoveFiles = CanBeRemoved = false` on both DownloadItem and ImportItem.
  * `DownloadProcessingService.RemoveCompletedDownloads` excludes titles that parse as
    multi-season.
* `DownloadedEpisodesImportService.ProcessFolder`:
  * If the tracked download's `ParsedEpisodeInfo` is FullSeason for the same series, it replaces
    the client-title parse.
  * For multi-season / complete-series downloads:
    * `requestedEpisodeIds` = episode ids of `Grabbed` history rows for this downloadId and series.
      Empty → reject `MultiSeason` "…choose the season using interactive import".
    * Video files are filtered to those whose own path parses to exactly one season.
    * Decisions are kept only if all their episodes are in `requestedEpisodeIds` (or have none).
    * **ImportMode forced to Copy.**
  * Net effect: only the grabbed season(s) are imported, and the torrent keeps seeding untouched.
    This contrasts with upstream, which now imports every season from interactively grabbed packs.
* `FullSeasonSpecification` (import) exception: a file parsed as a full season is accepted when
  * the download is multi-season,
  * the file names exactly one season,
  * it maps to more than one episode of that one season,
  * all mapped ids are in the grab's `EpisodeIds`, and
  * `MediaInfo.RunTime ≥ 0.8 × Σ episode runtimes`.

  This handles one-video-per-season packs.
* `ImportDecisionMaker.ReconcileLegacyMultiSeasonGrabMembership` (gaboy): for tracked packs with
  ≥3 **contiguous** seasons, if the grab history's episode ids are a strict subset of the
  currently mapped ids, it unions them in. This repairs grabs recorded before newly listed
  episodes existed.
* `TrackedDownloadService`:
  * Reparses from the `Grabbed` history row (previously the first history row, which could be a
    per-file import row).
  * Falls back to `ParseCompleteSeriesGrab`: rebuilds `CompleteSeriesSearchCriteria` from a single
    grab title or series and the grabbed ids, and runs `CompleteSeriesReleaseParser`.
  * Maps only grabbed ids of the grabbed series.
  * Retries `ImportBlocked` items whose `RemoteEpisode` was null.
  * Drops its cache entry when a reused multi-season/complete pack is grabbed again for the same
    client and series.
* `CompletedDownloadService.VerifyImport`: "all imported" now means expected episode **ids** ⊆
  imported ids (upstream compared counts). `VerifyRetainedEpisodes` is covered in §6/§10.

### 2j. Status / tests
* Status: **Operational** for parsing, decisions, interactive and automatic complete-series
  search, ranking, queue protection and import scoping.
* Caveats:
  * Complete-series search is torrent-only and cannot be configured.
  * Pack-first ranking overrides quality order.
  * Manual series search no longer upgrades complete seasons.
  * `CompleteSeriesSearchCommand` is API-only.
* Tests: `SeasonParserFixture` (modified), `CompleteSeriesReleaseParserFixture` (338 lines),
  `RemoteEpisodeSeasonFixture`, `MultiSeasonSpecificationFixture`,
  `CompleteSeasonUpgradeSpecificationFixture`, `QueueSpecificationFixture`,
  `PrioritizeDownloadDecisionFixture`, `CompleteSeriesSearchServiceFixture`,
  `EpisodeSearchServiceFixture`, `SeriesSearchServiceFixture`, `ReleaseSearchServiceFixture`,
  `NewznabRequestGeneratorFixture`, `NyaaFixture`, `MultiSeasonImportFixture`,
  `MultiSeasonCleanupFixture`, `CompleteSeriesGrabTrackingFixture`,
  `TrackedDownloadGrabHistoryFixture`, `FullSeasonSpecificationFixture`,
  `ImportDecisionMakerFixture`, Api.Test `CompleteSeriesReleaseCacheFixture`,
  `v5/Queue/QueueResourceFixture`. The repo also claims a docker "multi-season packs 13 checks"
  runtime gate (not re-run).

## 3. Interactive season-pack search from native and grouped season panels

* API: `GET /api/v3/release` and `GET /api/v5/release` with `seriesId=N&seasonNumber=S&seasonPacksOnly=true`.
  * Requires a positive seriesId and seasonNumber, no episodeId and no completeSeries; otherwise 400.
  * Calls `ReleaseSearchService.SeasonPackSearch(seriesId, season, userInvoked: true, interactive: true)`.
* Algorithm (`SeasonPackSearch`):
  * Movie → `MovieSearch`. No episodes in the season → empty.
  * For each scene-season mapping of the season's episodes:
    * **Anime series**: `AnimeSeasonSearchCriteria` with `PackSearchOnly = true`,
      `CatalogueEpisodes = season episodes`, and `SceneTitles` plus up to 3 alternate titles.
      Nyaa sends a bare title query when `PackSearchOnly`.
    * **Other series**: a normal `SeasonSearchCriteria`.
  * Results are filtered to `ParsedEpisodeInfo.FullSeason == true`, then de-duplicated.
  * Unlike complete-series search, all protocols are used (Usenet included).
* `AnimePackMatcher.Match` (runs inside `DownloadDecisionMaker` for anime-season criteria, when
  the normal parse fails or yields neither a full season nor episode numbers):
  * Gates:
    * `criteria.SeasonNumber > 0`.
    * The season's catalogue episodes are exactly 1..N contiguous, and **all have aired**.
  * Aliases: the season scene titles, plus series scene titles and the series title when the
    series has exactly one regular season.
  * Text normalization: lowercase, non-alphanumerics → space, leading `[group]` stripped. The
    text must start with an alias followed by a space.
  * The suffix must match `^0*1\s+0*(?<end>\d+)\s+(?:complete|batch)\b` with `end == N`, and
    must not contain `ova|oad|special(s)|movie(s)|part|season|s\d+`.
  * Result: FullSeason for `criteria.SeasonNumber`.
  * Example: `[Grp] Title 01-12 Complete [1080p]` for a 12-episode season.
* UI: `SeasonReleaseSearchButton` ("Search season packs N") renders in
  `Series/Details/SeriesDetailsSeason.tsx` (native seasons) and
  `Series/Grouping/CollectionMemberCard.tsx` (grouped collection seasons, labelled with the
  collection's display season number but querying the native `seasonNumber`). For movie or
  special members it falls back to a normal search.
* Status: **Operational**.
* Tests: `AnimePackMatcherFixture`, `ReleaseSearchServiceFixture`, `NyaaRequestGeneratorFixture`.
  No API-level test for `seasonPacksOnly` was found. UNVERIFIED: the frontend behaviour itself
  (React tests not inspected).

## 4. Fake release filtering (torrent contents inspection)

* Files:
  * `Download/FakeRelease/FakeReleaseInspector.cs`, `FakeReleaseDetection.cs`, `FakeReleaseRejectionReason.cs`;
  * `Exceptions/FakeReleaseException.cs` (derives from `ReleaseBlockedException` → `ReleaseDownloadException`);
  * `MediaFiles/TorrentInfo/TorrentContents.cs`;
  * `TorrentFileInfoReader.GetContentsFromTorrentFile` (MonoTorrent `Torrent.Load`, file path and length);
  * hook in `Download/TorrentClientBase.cs`.
* Setting: `Config` key `enablefakereleaseprotection`, **default true**.
  * API: v3 `config/indexer.enableFakeReleaseProtection`; v5 `settings/indexer`.
  * UI: Settings → Indexers → Options.
* Flow in `TorrentClientBase.Download` when enabled (shared by every torrent client):
  1. **Title check**: reject if the release title's final extension is in the executable set
     (`FakeReleaseInspector.IsDangerousReleaseTitle`).
  2. If the release has a `.torrent` URL, it is **always downloaded and inspected first**, even
     for clients that prefer magnets. On HTTP/WebException failure it falls back to the magnet
     URL **uninspected**, logging a warning. A redirect to `magnet:` is also uninspected (warning).
     Magnet-only releases are not inspected (warning).
  3. After the hash/blocklist check, `EnsureReleaseIsNotFake(contents)` runs before the existing
     rejected-extension check and before `AddFromTorrentFile`. It fails open: if reading the file
     list throws, or the list is empty, the grab proceeds with a warning.
* `Inspect` rules, applied to the file list after removing BEP-47 padding files
  (`(^|[\\/])(\.pad[\\/]|_____padding_file)`), in this order:
  1. `DisguisedExecutable`: a file name matches
     `\.(mkv|mp4|avi|m4v|mpg|mpeg|wmv|mov|flv|ts|webm|iso|srt|sub|nfo|jpg|png)\s*\.(exe|scr|com|pif|bat|cmd|msi|vbs|js|jar|lnk|hta|apk)$` (i).
  2. `ExecutablePayload`: any file whose extension is in {exe, scr, com, pif, bat, cmd, msi, msp,
     vbs, vbe, js, jse, wsf, wsh, ps1, psm1, hta, cpl, msc, reg, jar, apk, lnk, url, gadget, inf,
     dll, sys}.
  3. Compute two flags:
     * `hasArchive`: an extension in {rar, zip, 7z, tar, gz, bz2, xz, z, cab, arj, lzh, ace}, or
       a split-volume name `\.(r\d{2,3}|\d{3}|z\d{2}|part\d+\.rar)$` (i).
     * `hasRealVideo`: an extension in {mkv, mp4, avi, m4v, mpg, mpeg, wmv, mov, flv, ts, m2ts,
       mts, webm, ogm, ogv, divx, vob, rmvb, rm, asf, mk3d, iso, img, m2v, mpv, 3gp, f4v, strm}
       **and** length ≥ 10 MiB.
  4. `PasswordBait`: no real video, an archive is present, and some file name matches
     `(password|passwort|contrase|mot.?de.?passe|how.?to.?(open|watch|play)|read.?me.?first)` (i).
  5. `NoUsableContent`: no real video and no archive.

  Split RAR sets without loose video therefore pass unless a bait file is present.
  "Password-protected archive" is **not** actually detected; it is a filename heuristic, despite
  the UI help text "reject releases containing … password-protected archives".
* On rejection:
  * `FakeReleaseException` is thrown.
  * Automatic paths: `ProcessDownloadDecisions` catches it as a generic exception, logs a Warn
    "Couldn't add release …" and returns `Skipped`, so the next-ranked release for those
    episodes is tried. The indexer is not penalised.
  * **Nothing is persisted**: no blocklist entry, no history row. The same release is re-fetched
    and re-rejected on later RSS syncs and searches. "Rejections are recorded" (FORK.md) means
    logs only.
  * Interactive grabs surface the exception message through the release API.
* qBittorrent's `Download()` override tries torrent **reuse** (§9a) before `base.Download`.
  A release whose info-hash is already loaded in qBittorrent is therefore tagged and accepted
  without inspection.
* Usenet: unaffected.
* Status: **Operational** (torrent protocol only).
* Tests: `Download/FakeReleaseTests/FakeReleaseInspectorFixture` (20 cases). Docs mention
  `docker/tests` runtime checks (not re-run).
* Risks for Kitsunarr to weigh:
  * `.url`, `.lnk` and `.js` in the executable set will reject public-tracker torrents that
    bundle promotional `.url` shortcuts. This is inference; whether such torrents are common
    in the target trackers is UNVERIFIED.
  * Forcing `.torrent` download adds an indexer fetch per grab.
* Two magnet-inspection commits from the gaboy branch were deliberately **not** adopted. Per
  CONTRIBUTORS.md they needed a MonoTorrent build that is not available.

## 5. Season pack trimming (skip unmonitored episodes on import)

* File: `MediaFiles/EpisodeImport/Specifications/MonitoredEpisodeSpecification.cs` (new import
  spec, auto-registered).
* Setting: `Config` key `skipunmonitoredepisodesfrompacks`, **default false**.
  * API: v3 `config/mediamanagement.skipUnmonitoredEpisodesFromPacks`; v5 `settings/mediamanagement`.
  * UI: Settings → Media Management checkbox.
* Rule: accept unless all of the following hold, in which case reject `UnmonitoredEpisode`
  "Episode is not monitored":
  * the setting is on;
  * `downloadClientItem != null` (not a manual import);
  * the series is monitored;
  * the file maps to ≥1 episode;
  * **every** mapped episode is unmonitored.

  It applies to **any** automatic import with a download-client item, not only packs.
  Multi-episode files with one wanted episode are imported.
* Effect on the queue, verified in code and in the fork's own test
  `RetainedEpisodesFixture.should_keep_other_rejections_blocked(UnmonitoredEpisode)`:
  * The grab's expected episodes include the unmonitored ones (a season pack maps the whole
    season).
  * `VerifyImport` therefore never reaches "all imported". `VerifyRetainedEpisodes` refuses
    `UnmonitoredEpisode` rejections.
  * The download stays **ImportBlocked** with the warning "One or more episodes expected in this
    release were not imported…", is re-attempted on every download-monitoring cycle, and is never
    auto-removed or marked imported.
* Manual imports from the queue list the rejection but can still be imported. Manual import
  builds decisions without the spec gate at execution.
* Status: **Partial**. Trimming works, but completed packs with trimmed episodes are left blocked
  in the queue. Docs do not mention this.
* Tests: `MonitoredEpisodeSpecificationFixture`, `RetainedEpisodesFixture`.

## 6. Schema items 237-240 (source release context, pending path, retained versions, subtitle codes)

Migration numbers are Tanukarr's. See §0 for the renumbering against upstream.

### 6a. Migration 237 `preserve_source_release_context`
* Schema: `EpisodeFiles.SourceReleaseSize` (INT64 NULL) and `EpisodeFiles.SourceReleaseTitle`
  (TEXT NULL).
* Backfill (SQLite json_extract and PostgreSQL jsonb variants):
  * Join `History` import rows (`EventType = 3`, DownloadFolderImported) to `Grabbed` rows
    (`EventType = 1`) on `DownloadId + SeriesId + EpisodeId`, with grab date ≤ import date and
    `size` > 0.
  * Keyed by `Data.fileId`. The latest import and latest grab win (`ROW_NUMBER` preference).
  * Writes the grab's `size` and `SourceTitle`.
* Runtime writers:
  * `LocalEpisode.ToEpisodeFile()` (`Release.Size`, `Release.Title`).
  * `ImportApprovedEpisodes`, from the grab history `size` / `Size` and `SourceTitle`.
  * `RetainedVersionService` copies both fields.
* Consumer: `CustomFormatCalculationService.ParseCustomFormat(EpisodeFile)`.
  * It uses `SourceReleaseTitle` in preference to `SceneName`, and `SourceReleaseSize` in
    preference to the file size. Existing-file custom-format scores therefore use the **original
    grabbed release**. For a pack that is the whole pack's size: "a 30 GB pack must not become a
    preferred 1 GB release".
  * Related change: for local imports, CF input uses `Release.Title`/`Release.Size`, and the
    ReleaseType from the grab when it was a SeasonPack.
* Status: **Operational**.
* Test: `237_preserve_source_release_contextFixture`.

### 6b. Migration 238 `add_pending_path_to_series`
* Schema: `Series.PendingPath` (TEXT NULL). Identical to upstream migration 234.
* It entered the fork through KakarottoCake's "approved upgrades" before upstream merged its own.
* Used by `SeriesService` (marks a path move in progress, skips concurrent path updates,
  `ClearStalePendingPaths`) and `MoveSeriesService` (clears it).
* Exposed as `pendingPath` in the v3/v5 `SeriesResource`; UI `SeriesMovePending`.
* Status: **Operational**. This is an upstream feature, renumbered.
* No migration fixture in the fork.

### 6c. Migration 239 `retained_episode_versions`
Covered in detail in §14.6b, from the library-tools sub-audit.
* Table `RetainedVersions`.
* On upgrade, the old file moves to `<series>/Plex Versions/<guid>/`, only when the per-series
  `KeepVersions` option is on.
* Activate = copy back and archive the current file.
* No delete or prune API.

The related **import-side completion rule** lives in `CompletedDownloadService.VerifyRetainedEpisodes`:
* A download counts as Imported when every expected episode is either imported now, already
  imported in history, or rejected only with `EpisodeAlreadyImported` / `NotQualityUpgrade` /
  `NotRevisionUpgrade` / `NotCustomFormatUpgrade`.
* In addition, every expected episode's current library file must exist and be **readable**:
  it is re-read from the DB, and the first byte is read from disk.
* It publishes `DownloadCompletedEvent` listing only the actually imported files.
* "Retained" here means "existing library file kept"; it is unrelated to the `RetainedVersions`
  table.

### 6d. Migration 240 `preserve_subtitle_language_codes`
* Schema: `SubtitleFiles.LanguageCode` (VARCHAR(64) NULL). No backfill. It is filled lazily
  during `MoveFilesAfterRename` from the stored `RelativePath`.
* Parsing (`Parser/LanguageParser.cs`):
  * `SubtitleLanguageRegex` gains a locale group
    `(?<locale>-(?!(?:cc|sdh|forced|foreign|default|psdh)(?:[-_. ]|$))(?:[a-z]{2}|[a-z]{4})(?:-(?:[a-z]{2}|[0-9]{3}))?)?`
    after `iso_code`, case-insensitive. This allows `pt-BR`, `zh-Hant`, `zh-Hant-TW`, `es-419`.
  * `ParseSubtitleLanguageCode`:
    * null if the ISO code is unknown;
    * otherwise the lowercased ISO code plus the original-case locale;
    * otherwise the 2-letter code, mapping 3-letter → `TwoLetterCode`.
  * File names that are only a language code (`en.srt`, `pt-BR.forced.srt`) are parsed as
    `subtitle.<name>`.
  * `ParseSubtitleLanguageInformation` falls back to the basic parse when the ISO code is
    unknown or the "title" contains `SxxEyy`.
* Use:
  * `SubtitleService.ImportFiles` stores `LanguageCode` and `Title`.
  * `ExistingSubtitleImporter` stores `LanguageCode`.
  * The rename suffix uses `languageCode ?? IsoLanguages.Get(language).TwoLetterCode`, so
    `Show.S01E01.pt-BR.srt` keeps `pt-BR` instead of collapsing to `pt`.
* Related, same feature set:
  * Subtitle discovery now searches nested folders. A subfolder named exactly like the video
    file is accepted outright; other subfolders are matched by SxxEyy parsed from file or folder.
  * Non-subtitle extras keep the old search scope.
  * `ExtraService.Handle(SeriesRenamedEvent)` now only moves extras of files actually renamed.
* Status: **Operational**.
* Tests: `Extras/Subtitles/SubtitleServiceFixture` (modified), `ExtraServiceFixture`,
  `ScopedRenameFixture`, `LibraryTools/SubtitleLabelsFixture` (e.g. `pt-BR.forced` → `pt-BR`,
  `eng` → `en`, `fr-cc` → `fr`). No migration fixture.

## 7. Quality: ranks, per-profile sizes, codec factors, custom presets, sliders

The claims in `docs/quality-and-codec-size.md` were checked against the code. Its table, unit
and API statements are **accurate**, with the caveats below.

### 7a. Migration 236 `quality_profile_quality_ranks` (= upstream 233, renumbered)
* Schema:
  * Table `QualityProfileQualityRanks(Id, ProfileId INT, QualityId INT, Score DOUBLE)`, unique on
    (ProfileId, QualityId).
  * JSON expression indexes on `History`, `EpisodeFiles` and `Blocklist`: `Quality.quality`.
* Upstream feature: profile-relative quality ranks for sorting. `QualityProfileRankService` is
  used by History/Blocklist/Episode repositories.
* Fork delta: rank rows are refreshed (`UpdateRanksForProfile`) after a custom preset is added.
* Status: **Operational** (inherited).
* Tests: `236_quality_profile_quality_ranksFixture`, `QualityProfileRankIntegrationFixture`.

### 7b. Per-profile size limits and "Mixed" global editing (`QualityProfileSizeLimit`, `QualityDefinitionUpdateService`)
* Inherited from upstream: size limits live on **quality profile items**
  (`QualityProfileQualityItem.MinSize/PreferredSize/MaxSize`, MiB/minute).
* Fork changes:
  * `QualityProfileSizeLimit.Summarize(profiles)` produces the global Quality Definitions view:
    * per quality id, the value if all profiles agree, otherwise null + `*IsMixed = true`;
    * a stored max of 0 counts as null (unlimited).
  * `QualityDefinitionUpdateService.UpdateMany` (global PUT) builds
    `QualityDefinitionSizeUpdate{UpdateMinSize/UpdateMaxSize/UpdatePreferredSize}`. A field is
    updated only if it was **present in the JSON**, and not (`IsMixed` echoed with null). It
    applies to every profile containing the quality under a global `CatalogueLock`.
  * `QualityProfileService.ApplySizeLimits` merges each profile and validates the **whole batch**
    before mutating anything. It throws `ValidationException` listing every failure.
    * Per-size validation: finite and within `QualityDefinitionLimits.Min = 0` .. `Max = 1000`
      MiB/min, or null.
    * Per-profile consistency: `min ≤ preferred`; `max == 0 || (min ≤ max && preferred ≤ max)`.
    * Then one `QualityProfilesUpdatedEvent` for the batch. `PendingReleaseService` handles it
      by re-mapping pending releases once.
* "Partial size" (`QualityProfilePartialSizeFixture`) = omitted fields preserve stored values;
  explicit null clears.
* API: v3 `PUT /api/v3/qualitydefinition/update`, v5 `PUT /api/v5/qualitydefinition` (bulk), and
  `PUT …/{id}`. Responses include `minSizeIsMixed`, `maxSizeIsMixed`, `preferredSizeIsMixed`.
* UI: MiB/hour fields with a GiB/hour preview; range sliders (`QualityDefinitionSizeSlider.tsx`,
  restored in 0.1.4: each thumb edits only its own limit); per-profile `QualityProfileItemSize`.
  Conversion: UI MiB/h = API MiB/min × 60.
* Status: **Operational**.
* Tests: `QualityDefinitionUpdateFixture`, `QualityProfilePartialSizeFixture`,
  `QualityProfileServiceFixture`, Api.Test `QualityDefinitionSizeResourceFixture`.

### 7c. Codec-normalized size (`Qualities/CodecSizeNormalizer.cs`, `CodecMultiplierOverrides.cs`)
* Decision value: `normalized = Release.Size × multiplier(codec)`, with the profile override
  taking precedence when it is in [0.01, 10]. It is used in exactly two places:
  * `AcceptableSizeSpecification`: min/max per runtime. `runtime = Σ episode runtimes`, with the
    series runtime as fallback. For movie collection packs, `MovieCollectionRuntime` replaces it.
  * `DownloadDecisionComparer.CompareSize`: distance to `preferred × runtime`, truncated to
    200 MiB buckets. Without a preferred size, larger is better.

  Actual bytes are kept for download, disk, indexer `MaximumSizeSpecification` and the
  free-space checks. Rejection messages read
  `"{actual} actual, {normalized} codec-normalized ({codec} ×{m})"`.
* Default multipliers, which match the doc table exactly:

  | Codec | Factor | Codec | Factor | Codec | Factor |
  | --- | ---: | --- | ---: | --- | ---: |
  | MPEG1 | 0.40 | MPEG2 | 0.50 | MPEG4ASP | 0.67 |
  | H263 | 0.40 | H264 | 1.00 | HEVC | 1.67 |
  | AV1 | 2.22 | VVC | 2.50 | VC1 | 0.80 |
  | WMV1 | 0.60 | WMV2 | 0.60 | WMV3 | 0.80 |
  | VP6 | 0.60 | VP7 | 0.60 | VP8 | 1.00 |
  | VP9 | 1.55 | Theora | 0.50 | Unknown | 1.00 |

  The display catalog `MultiplierCatalog` groups these under human labels. Note that
  "VC-1 / WMV3" = 0.80.
* Codec detection:
  1. **Structured** `ReleaseInfo.Codec`. Only BroadcastheNet populates it among indexers; for
     local files it comes from `MediaInfo.VideoCodecID`.
     * Normalize: strip spaces, dots, dashes and underscores; uppercase.
     * Map through a fixed alias table (e.g. `X264|H264|AVC|AVC1|LIBX264 → H264`,
       `X265|H265|HEVC|HVC1|HEV1|LIBX265 → HEVC`, `AV1|AV01|SVTAV1|LIBSVTAV1|LIBAOMAV1 → AV1`;
       lossless/mezzanine and `MIXED/MULTIPLE…` → Unknown).
     * Composite values split on `/ + & | , ;`: same family → that family, else Unknown.
  2. **Title tokens**: `ParsedEpisodeInfo.ReleaseTokens`, else the release title with the series
     title removed and the trailing `-GROUP` / `[GROUP]` removed (keeping `SVT-AV1` /
     `libaom-AV1` tags).
     * Tokens longer than 16384 characters → Unknown.
     * A bounded regex (100 ms timeout) with boundary guards `(?<![\p{L}\p{N}])…(?![\p{L}\p{N}])`
       finds codec tokens, e.g. `x265|h[ ._-]?265|hevc|hvc1|hev1`, `av1|av01|svt[ ._-]?av1|libaom[ ._-]?av1`.
     * Exactly one family → that family; zero or several → Unknown. A timeout → Unknown.
* Per-profile overrides:
  * Stored in `QualityProfiles.CodecMultipliers`, a JSON text column added by migration 251,
    default `{}`. Keys are canonical family names, case-insensitive on read.
  * Validation: keys must be known families, values 0.01–10, at most 18 entries.
  * API:
    * Profile resources expose `codecMultipliers`. Omitted on PUT = keep; `{}` = reset to defaults.
    * Global `GET/PUT /api/v3|v5/qualitydefinition/codecfactors` returns
      `[{codec, factor|null, isMixed, defaultFactor}]`. PUT `{family: factor}` writes only the
      edited families into **every** profile.
    * `GET …/codecs` (display catalog) and `GET …/codecdefaults` are read-only.
  * UI: `QualityCodecSizeFactors.tsx` (global table) and `QualityProfileCodecFactors.tsx` (per profile).
* No bitrate-aware sizing exists. Sizing is still MiB/min × runtime; only the codec multiplier
  was added. Multipliers > 1 make HEVC/AV1 releases **look larger**: they become more likely to
  exceed max and less likely to fall under min.
* Status: **Operational**.
* Tests: `CodecNormalizedSizeFixture`, `AcceptableSizeSpecificationFixture`,
  `PrioritizeDownloadDecisionFixture`, `QualityProfileCodecFixture`.

### 7d. Custom quality presets (migration 251 `custom_quality_presets`)
* Schema:
  * `QualityDefinitions` gains `Source INT NULL`, `Resolution INT NULL`,
    `Codec TEXT NOT NULL DEFAULT ''`, `TitlePattern TEXT NOT NULL DEFAULT ''`,
    `IsRetired BOOL NOT NULL DEFAULT false`.
  * `QualityProfiles.CodecMultipliers TEXT NOT NULL DEFAULT '{}'`.
* Identity:
  * Custom qualities get `Quality.Id ≥ 10000`: `max(10000, max existing id + 1)`, persisted.
  * The process-wide registry `Quality.Register` / `FindById` returns configured qualities.
    An unknown id ≥ 10000 yields a placeholder "Custom quality N".
  * Standard ids are unchanged. The startup sync no longer deletes rows with id ≥ 10000.
  * Weight for custom presets is 19. Comparison falls back to the standard definition with the
    same source and resolution (`QualityModel.CompareTo`).
* Create (`QualityDefinitionService.Add`, under `CatalogueLock`):
  * Title 1–100 characters, unique (case-insensitive).
  * Source required and a defined enum; resolution required and in {0, 480, 576, 720, 1080, 2160}.
  * Codec empty or a known family; TitlePattern ≤ 256 characters and a valid regex.
  * `copyFromId` copies size limits per profile.
  * The new item is appended to **every** profile with `Allowed = false`.
* Retire (DELETE):
  * The Unknown quality (id 0) is refused.
  * Refused while any profile allows the quality or uses it as cutoff; the error lists those
    profiles.
  * Otherwise sets `IsRetired`. Restore: `POST …/{id}/restore`.
  * `QualityAllowedByProfileSpecification` now rejects retired qualities.
* Matching (`Qualities/ProfileQualityMatcher`):
  * Applied in `DownloadDecisionMaker` after aggregation, and in `ImportDecisionMaker` for
    non-existing files.
  * A parsed quality with id ≥ 10000 is kept.
  * Candidates are non-retired definitions **allowed in the series' profile** with equal Source
    and Resolution (definition override or base), matching codec (empty = any) and matching
    `TitlePattern` (case-insensitive, 50 ms timeout; invalid or timeout = no match).
  * Ordering: specificity (codec set + pattern set), then custom before standard, then the
    higher profile index (more preferred).
  * If none matches, the parsed quality is kept, **unless** its own definition is retired or has
    its own codec/pattern/source/resolution constraints that the release fails. Then it becomes
    **Quality.Unknown**. Editing a standard preset to require e.g. HEVC therefore turns
    non-HEVC releases of that quality into Unknown.
* Profile service guards:
  * A profile must keep ≥1 active quality.
  * Retired qualities cannot be enabled.
  * Unknown quality ids are refused.
  * Custom items omitted from a PUT are re-appended.
* API (v3 and v5, `qualitydefinition`):
  * `POST` (with `copyFromId`), `DELETE /{id}` (retire), `POST /{id}/restore`.
  * Resources expose `source`, `resolution`, `codec`, `titlePattern`, `isRetired`.
* UI: `QualityPresetEditor`, `QualityPresetCriteria`, `QualityPresetActions`.
* Status: **Operational**.
* Tests: `251_custom_quality_presetsFixture`, `CustomQualityCatalogueFixture`,
  `ProfileQualityMatcherFixture`, Api.Test `CustomQualityResourceFixture`.

### 7e. Related custom-format changes (not requested but in the acquisition path)
* `CustomFormatSpecificationBase.AlternativeGroup` (int field, order 100). Matching rule:
  * every condition with group ≤ 0 must match (per-type OR, as before);
  * **and**, if any group > 0 exists, at least one alternative group must fully match.

  This gives "OR of AND-groups" custom-format logic.
* `ReleaseTitleSpecification.ExcludeEpisodeTitles`: after the first `S\d{1,3}E\d{1,3}…` token,
  remove each known episode title (longest first; titles longer than 3 characters, word-boundary
  regex, first occurrence) before matching. This stops an episode title such as "The Remux"
  from triggering a CF.
* Tests: `CustomFormats/ReleaseContextAndAlternativesFixture`.

## 8. Decision engine: new and changed specifications

| Spec | New/changed | Rule | Default | Tests |
| --- | --- | --- | --- | --- |
| `EpisodeNumberingSpecification` | new, Permanent | Reject `AmbiguousNumbering` with `RemoteEpisode.NumberingRejection` text (set by `EpisodeNumberingResolver` in `ParsingService.Map`) so ambiguous scene/aired numbering never enters the pending queue | always on | `EpisodeNumberingResolverFixture` (via mapping) |
| `ExternalIdsSpecification` | new, Permanent | When `ProOptions.MatchExternalIds` (Library tools JSON, default **false**): reject `WrongSeries` if `Release.TvdbId > 0 && Series.TvdbId > 0 && different`, or both IMDb ids normalise (strip `tt`, leading zeros, numeric) and differ | off | `LibraryTools/ExternalIdsFixture` |
| `RssSync/CompleteSeasonUpgradeSpecification` | new, Permanent | See §2g. Non-search decisions: no RSS upgrades inside seasons with no monitored, aired, missing episode | always on | `CompleteSeasonUpgradeSpecificationFixture` |
| `MultiSeasonSpecification` | changed | See §2c | `EnableMultiSeasonReleases=true` | `MultiSeasonSpecificationFixture` |
| `AcceptableSizeSpecification` | changed | Codec-normalized size; movie-collection runtime override (§7c) | always | `AcceptableSizeSpecificationFixture`, `CodecNormalizedSizeFixture` |
| `QualityAllowedByProfileSpecification` | changed | Also rejects `Quality.IsRetired` | always | fixture modified |
| `QueueSpecification` | changed | Multi-season queue protection (§2h), non-interactive only | always | `QueueSpecificationFixture` |

Other decision-pipeline changes:
* `DownloadDecisionMaker`:
  * Context-specific parsers:
    * movie series → `MovieReleaseParser` / `MovieCollectionPackParser`;
    * complete-series criteria → `CompleteSeriesReleaseParser` (unparseable results skipped);
    * RSS with no TV parse → `MovieReleaseParser.Parse`;
    * anime season criteria → `AnimePackMatcher` fallback.
  * The complete-series identifier-conflict rejection (§2f).
  * `IProfileQualityMatcher.Apply` before custom-format scoring.
* `DownloadDecisionComparer`: `CompareReleaseScope` first; full-season episode count; normalized
  size (§2g, §7c).
* `DownloadRejectionReason.AmbiguousNumbering` is upstream; the fork reuses it.

No other specification files differ between base and fork. The nine `DecisionEngine` entries
in `srcdiff.txt` are exactly these fork changes; there is no upstream drift in this directory.

## 9. Download handling (`src/NzbDrone.Core/Download`)

**Attribution first:**
* The fork delta (`git diff 57816c149 HEAD -- src/NzbDrone.Core/Download`) is 16 files.
* Among the download clients, **only qBittorrent** is changed by the fork.
* The other differences in `srcdiff.txt` are upstream changes made after the base, which
  Tanukarr **lacks**:
  * Deluge, Flood, Freebox, Hadouken, NzbVortex, NZBGet, RQBit, SABnzbd, Transmission, Tribler,
    rTorrent and uTorrent settings files: upstream removed `Advanced = true` field flags.
  * SABnzbd: `SabnzbdProxy` and the `Sabnzbd/Responses/*History/QueueResponse` classes.
  * `QBittorrentSeedTimeType.cs`.
  * `DownloadClientFile.cs` and `IDownloadClient.GetFiles()` (upstream's per-file listing for its
    multi-season import).
  * `RedownloadFailedDownloadService`: per-season re-search for failed multi-season packs.
  * `DownloadMonitoringService`: refresh on download-client provider update/delete.
  * `DownloadCompletedEvent` carrying `Series` + `Episodes` instead of `SeriesId`, and the
    matching `DownloadHistoryService` change.

### 9a. qBittorrent: reuse of already-loaded torrents (KakarottoCake)
* `ReuseTag = "sonarr-pro-reuse-" + Uri.EscapeDataString(TvCategory ?? "")`. This is internal,
  not a setting.
* `Download()`, `AddFromTorrentFile` and `AddFromMagnetLink` call `TryReuseTorrent(hash)` first.
  The same call is made again on an HTTP 409 Conflict from add.
  * Condition: API ≥ 2.3 and `IsTorrentLoaded(hash)` (any category).
  * Action: add tags [ReuseTag, plus series tag labels if `AddSeriesTags`] via `torrents/addTags`.
    The hash is returned without re-adding, so seeding and save path are untouched.
* Reused torrents (tag present):
  * `GetItems`: `CanMoveFiles = CanBeRemoved = false`.
  * `MarkItemAsImported`: no-op (no post-import category).
  * `RemoveItem`: throws `NotSupportedException` ("Reused torrents are kept…").
* `GetTorrents` no longer passes `category=` to `/api/v2/torrents/info`. It fetches **all**
  torrents and filters in memory to (category == TvCategory) ∨ has ReuseTag. This costs more on
  large clients. `QBittorrentTorrent.Tags` (comma-separated) plus `HasTag()` were added.
* Status: **Operational**.
* Tests: `QBittorrentReuseFixture`, `QBittorrentReuseProxyFixture`, `QBittorrentFixture` (modified).
* Related upstream fix already in the base: `7b54ccd` "Avoid marking grab as failed when
  qBittorrent already has that release".

### 9b. `TorrentClientBase` (all torrent clients)
* Fake-release inspection and forced `.torrent` fetch (§4).
* New fallback: when a `.torrent` fetch fails with 404/410, another HTTP error or a
  `WebException`, and a magnet URL exists, `FallbackToMagnetOrThrow` uses the magnet instead of
  failing the grab. This changes upstream semantics.

### 9c. Stalled torrent recovery (approved upstream PR #8859, not in upstream)
* Setting: `Config` key `stalledtorrenttimeout`, minutes, **default 0 = off**, clamped to
  0..43200.
  * API: v3 `config/downloadclient.stalledTorrentTimeout`; v5 `settings/downloadclient`.
  * UI: Settings → Download Clients → Options.
* Tracking: `TrackedDownload.LastProgressDate` is **in memory**. It is reset to now whenever the
  client item's `Status` or `RemainingSize` changes between polls, and resets on restart (the
  help text says so).
* Rule: `FailedDownloadService.CheckStalled` runs after the normal failure checks. It fires for
  torrent protocol only, item status Downloading or Warning, and
  `LastProgressDate + timeout ≤ now`.
  * No Grabbed history → warn "…wasn't grabbed by Sonarr…", nothing else.
  * Otherwise `IsStalled = true` and state `FailedPending`. `ProcessFailed` then records the
    failure "Download stalled with no progress". This is normal failed-download handling:
    blocklist, then re-search if auto-redownload is enabled.
* Status: **Operational**.
* Tests: `LibraryTools/StalledRecoveryFixture`. It covers only Downloading/Warning being
  recoverable, Paused/Queued/Completed being ignored, and foreign downloads or recent progress
  never failing.

### 9d. Pending-release fixes (`Download/Pending/PendingReleaseService.cs`, v5 `QueueController`)
* `FindPendingQueueItemObsolete` (v3 ids) now searches `GetPendingQueueObsolete()`, the
  per-episode list. It used to search the grouped v5 list, so `DELETE /api/v3/queue/{id}` for a
  pending release always returned 404. Upstream `c018578` **still has this bug** (line 277).
* `RemovePendingQueueItems` / `…Obsolete` now also prune the static in-memory `_pendingReleases`
  and publish `PendingReleasesUpdatedEvent`. The item vanishes immediately instead of at the next
  RSS sync or restart.
* v5 bulk removal: overlapping pending siblings no longer 500.
  * `Remove(pending, …, skipAlreadyRemoved: true)` blocklists, then skips the deletion when an
    earlier sibling removal already deleted it.
  * Removing a pending release also removes other pending releases with the same
    `SeasonNumbers` and `EpisodeNumbers` (inherited).
  * Source: Matt Flowers `ced9105`.
* `PendingReleaseService` also handles the new batch `QualityProfilesUpdatedEvent`, re-mapping
  pending releases once.
* The v5 `removalMethod`/`blocklistMethod` enum parameters (`QueueRemovalMethod`,
  `QueueBlocklistMethod`) are an **upstream cherry-pick** (`049f36f`), not fork design.
* Status: **Operational**.
* Tests: `FindPendingQueueItemObsoleteFixture` (6 cases), plus modified `Remove*Fixture`s.

### 9e. Tracked downloads and completed-download handling
* `TrackedDownloadService.TrackDownload`:
  * maintains `LastProgressDate`;
  * retries ImportBlocked items with a null `RemoteEpisode`;
  * re-parses from the **Grabbed** history row, then movie grab context, then special-episode
    parse, then `ParseCompleteSeriesGrab`;
  * maps only grabbed episode ids of the grabbed series.
* `UpdateCachedItem` (on series refresh/updates) now re-maps from grab history and keeps the
  original `Release`.
* Cache eviction on re-grab of a reused multi-season/complete pack (§2i).
* `CompletedDownloadService`:
  * multi-season/complete source retention (§2i);
  * import-progress batch (§10);
  * id-based `VerifyImport`;
  * movie collection coverage (`VerifyMovieCoverage`; movie audit);
  * `VerifyRetainedEpisodes` (§6c).
* `DownloadProcessingService`: never auto-removes completed items whose title parses as
  multi-season.
* `DownloadService.DownloadReport`:
  * If `ProOptions.AutomationPaused` is set, it throws `ReleaseDownloadException` "Automatic
    downloads are paused in Library tools" for any `ReleaseSource` other than InteractiveSearch
    or UserInvokedSearch.
  * This includes RSS, automatic search, release push and re-search after failure. See §14.
* `MovieGrabContextService` (new): rebuilds the movie / collection-pack context from Grabbed
  history (single series, single source title). Used by tracking, import and verification.
  Belongs to the movie audit.
* `QueueService`:
  * the queue list is now lock-protected and copied on read;
  * adds `DownloadClientId` and `ImportProgress`;
  * handles `ImportProgressUpdatedEvent`;
  * prunes progress for vanished downloads.
* Status: **Operational**.
* Tests: `CompleteSeriesGrabTrackingFixture`, `TrackedDownloadGrabHistoryFixture`,
  `TrackedDownloadServiceFixture`, `RetainedEpisodesFixture`, `MultiSeasonCleanupFixture`,
  `CompletedDownloadServiceTests/ImportFixture` and `ProcessFixture`, `MovieGrabContextServiceFixture`.

### 9f. Indexer-side changes that affect acquisition
* `IIndexer.Fetch(CompleteSeriesSearchCriteria)` and the request generators (§2f).
* Nyaa:
  * `PrepareQuery` = `Uri.EscapeDataString(query).Replace("%20", "+")`. Literal `& # +` are
    preserved; this was the inherited unescaped-query defect.
  * Special-episode query titles have `+` → space before escaping.
  * Pack-only title query for `PackSearchOnly`.
* Newznab/Torznab:
  * New `MovieCategories` setting (field order 13, **default `[2000]`**).
  * The validator accepts TV, anime **or** movie categories.
  * Category options no longer hide 2000 (Movies).
  * `GetRecentRequests` adds a **generic `t=search` RSS request on the movie categories** for
    every indexer with capabilities and `MovieCategories` set. Because the default is `[2000]`,
    existing indexers whose stored settings lack the key start polling movie feeds on every RSS
    sync. This is inferred from the constructor default plus Newtonsoft population; UNVERIFIED
    at runtime.
  * Movie series search uses movie categories (+ anime categories for anime/AniList/MAL owners).
* BroadcastheNet / HDBits: send the TVDB id only when `> 0` (negative placeholder ids for
  non-TVDB series).
* `IndexerFactory.ResolveIndexer`: an unknown indexer name now logs a warning and returns null
  instead of throwing. This is an upstream cherry-pick (`4d25f57`, pushed releases).

## 10. Episode import changes (`MediaFiles/EpisodeImport` and neighbours)

New files:
| File | Purpose |
| --- | --- |
| `ImportProgress.cs`, `ImportProgressService.cs`, `ImportProgressUpdatedEvent.cs` | **In-memory** per-download import progress, keyed by (DownloadClientId, DownloadId). Per file: id, episode ids/numbers, season, source path, size, state ∈ {Waiting, Importing, Imported, Rejected, Failed}, timestamps, errors. Aggregates: total/imported/importing/waiting/failed/rejected episode counts. Fed by `CompletedDownloadService.Import` (`BeginBatch` with expected ids) and `ImportApprovedEpisodes` (`RegisterDecisions`, `MarkImporting/Imported/Failed`). Exposed via `Queue.ImportProgress` → v5 `QueueResource.importProgress`; UI `QueueRow`, `QueueDetailsProvider`, `EpisodeStatus`. Not persisted (lost on restart). All progress calls are wrapped so failures never break imports. Test: `ImportProgressServiceFixture` |
| `MovieFileIdentityService.cs` | Movie imports: persisted episode identity for a physical file (movie audit). Test: `MovieFileIdentityServiceFixture` |
| `Specifications/MonitoredEpisodeSpecification.cs` | Pack trimming (§5) |

Changed behaviour:
* `ImportDecisionMaker`:
  * `InferBatch` numbering per folder (§1).
  * Movie path parsing.
  * **Persistent manual episode overrides** (#6147):
    * When an interactive grab overrides episodes (v3 `shouldOverride` / v5 `override`),
      `RemoteEpisode.ManualEpisodeMapping = true`. `HistoryService` then writes Grabbed-history
      `Data["ManualEpisodeMapping"]="true"` and `Data["ManualEpisodeIds"]=[ids]`.
    * On import, `ManualEpisodeMappingService.Find(localEpisode, downloadId, multipleFiles)`
      (in `LibraryTools/`) reads the latest such grab for the download and series.
      * One file → all chosen episodes.
      * Several files → per-file subset by absolute numbers, or by episode numbers (a
        single-season source may be remapped to a single chosen season). Counts must match
        exactly, otherwise empty.
    * The mapping replaces the parsed episodes and clears numbering rejections. An empty
      result gives the rejection "Your saved episode choice could not be mapped
      unambiguously…".
  * `ProfileQualityMatcher` (§7d).
  * `NumberingRejection` → `AmbiguousNumbering` rejection.
  * Legacy multi-season grab reconciliation (§2i).
* `AggregateEpisodes`:
  * Movies use the persisted identity or the film parser (exactly one native feature required).
  * Scene-numbered series pre-set a fail-safe `NumberingRejection`, then resolve with
    `EpisodeNumberingResolver.ResolveLocal`. The physical file's own parse is preferred over
    pack/folder names. A verified release-group convention can be inherited from the
    download/folder context when the file's raw season belongs to that pack.
* Specifications:
  * `FullSeasonSpecification`: one-file-per-season exception (§2i).
  * `MatchesFolderSpecification`: bypassed for manual mappings; re-resolves folder numbering
    with the verified convention.
  * `MatchesGrabSpecification`: movie collection members allowed via grab context.
  * New `MonitoredEpisodeSpecification`.
  * New rejection reasons `UnmonitoredEpisode` and `AmbiguousNumbering`.
* `DownloadedEpisodesImportService`: multi-season / complete-series scoping and the movie path
  (§2i).
* `ImportApprovedEpisodes`: source-release size/title from grab history (§6a); progress events.
* `ManualImportService`:
  * Manual items are marked `ManualEpisodeMapping`, which clears numbering rejections.
  * Release group `null` now means "parse it"; empty means "clear it" (upstream cherry-pick
    `d58fd46`).
  * Falls back to `DownloadItem` when `ImportItem` is null.
  * Resolves `EpisodeFileId` for existing files.
* `EpisodeFileMovingService` / `UpgradeMediaFileService`:
  * **Hardlink-only imports** (`ProOptions.HardlinkOnly`, default false): copy-mode transfers
    become hardlinks. `VerifyHardlink` probes by creating `.sonarr-hardlink-<guid>` in the
    destination folder and throws before touching existing files.
  * Retained versions (§14.6b).
* `UpdateEpisodeFileService` / `FileDateType.PreserveOriginal = 3`: on upgrade, the new file
  takes the replaced file's date (`LocalEpisode.PreservedFileDate`).
* `RenameEpisodeFileService` (~300 changed lines): manual renaming independent of automatic
  naming, plus collision-safe renames. Per-series `ProSeriesOptions.AutomaticRenaming` and
  `SeasonTitles` are in the Library-tools JSON. Not deep-audited here: **UNVERIFIED**.
  Test: `RenameEpisodeFileServiceFixture` (modified).

Not in Tanukarr, upstream only: `EpisodeImport/RejectedFileExtensionService.cs` and
`MultiSeasonPackSpecification.cs`. Upstream's newer rejected-extension handling and multi-season
import are absent.

* Status: **Operational** for the items above, except pack trimming (**Partial**, §5).
* Tests (fork-added or modified): `AggregateEpisodesFixture`, `ImportDecisionMakerFixture`,
  `ImportApprovedEpisodesFixture`, `ManualImportReprocessFixture`, `ManualNumberingOverrideFixture`,
  `MatchesFolderSpecificationFixture`, `MatchesGrabSpecificationFixture`,
  `FullSeasonSpecificationFixture`, `MonitoredEpisodeSpecificationFixture`,
  `DownloadedEpisodesImportServiceFixture`, `MultiSeasonImportFixture`,
  `ImportedDownloadPreviewFixture`, `UpgradeMediaFileServiceFixture`, `ChangeFileDateForFileFixture`.


> Sections 11-15 below come from the parallel sub-audits, with light cross-checking: the TaskManager change, the cloud block policy, the ExternalIds/StalledRecovery tests and the manual-mapping flow were re-verified. Sub-audit wording is kept.

## 11. Jellyfin integration

Scope note: Tanukarr adds a **new, separate** notification implementation named `Jellyfin`
(`Implementation == "Jellyfin"`, `ConfigContract == "JellyfinSettings"`). The inherited
upstream **Emby / Jellyfin** connection (`Notifications/MediaBrowser/*`, class `MediaBrowser`)
is functionally unchanged (only branding strings and `Advanced = true` on UrlBase / MapPathsFrom /
MapPathsTo; see section 12). Everything below is fork code, authored in commits
`4b44b20` (refresh, 2026-10-12), `2805532` (cold-library retention), `0226d76` (watched sync, "1.0.4")
and `fee14c1` (origin-dispatch authorization/queue isolation).

Implementation files (all under `src/NzbDrone.Core/`):

| File | Role |
| --- | --- |
| `Notifications/Jellyfin/Jellyfin.cs` | `NotificationBase<JellyfinSettings>`; only overrides `OnDownload`, `Test`, `RequestAction("libraries"/"users")` |
| `Notifications/Jellyfin/JellyfinSettings.cs` | 16 settings fields + FluentValidation validator |
| `Notifications/Jellyfin/JellyfinRefreshService.cs` | Durable refresh queue (generations/watermarks), `IExecute<ProcessPendingJellyfinRefreshCommand>`, provider add/update/delete handlers |
| `Notifications/Jellyfin/JellyfinProxy.cs` | Library list, readiness check, selected-library refresh; shared `BuildRequest`/`EnsureSuccess` |
| `Notifications/Jellyfin/JellyfinWatchedService.cs` | Watched-state poller, `IExecute<SyncJellyfinWatchedCommand>`, applies snapshot, hands off to list progress |
| `Notifications/Jellyfin/JellyfinWatchProxy.cs` | Users list + complete paged watched snapshot |
| `Notifications/Jellyfin/JellyfinWatchPath.cs` | Path normalisation + prefix translation |
| `Notifications/Jellyfin/JellyfinRefreshState(.cs/Repository.cs)`, `JellyfinWatchState(.cs/Repository.cs)` | Persistence models |
| `Notifications/Jellyfin/ProcessPendingJellyfinRefreshCommand.cs`, `SyncJellyfinWatchedCommand.cs` (`SendUpdatesToClient => true`) | Scheduled commands |
| `Notifications/Jellyfin/JellyfinRefreshMode.cs`, `JellyfinRefreshClock.cs`, `JellyfinException.cs`, `JellyfinItems.cs`, `JellyfinLibrary.cs`, `JellyfinWatchedItem.cs` | Enum, clock seam, sanitized exception, DTOs (Newtonsoft `[JsonProperty("PascalCase")]`) |
| `Notifications/NotificationDefinition.cs` (modified) | `Enable` also true when `Implementation == "Jellyfin" && EnableWatchedSync` (so a watched-only connection counts as enabled) |
| `Jobs/TaskManager.cs` (modified) | Two new 1-minute scheduled tasks |
| `Tv/Episode.cs`, `Tv/EpisodeRepository.cs` (modified) | `Watched`, `WatchedAtUtc`, `WatchedSource`, `JellyfinUnmonitorOwner`; `SetJellyfinWatched`; manual monitor edits clear ownership |
| `Datastore/TableMapping.cs` (modified) | Registers `JellyfinRefreshStates`, `JellyfinWatchStates` (and `ImportListProgressStates`) |
| `Datastore/Migration/243_persistent_jellyfin_refresh_queue.cs`, `248_jellyfin_watched_state.cs` | Schema |
| `Sonarr.Api.V3/Episodes/EpisodeResource.cs`, `Sonarr.Api.V5/Episodes/EpisodeResource.cs` (modified) | Expose `watched`, `watchedAtUtc` |

### 11.1 Settings (provider fields, stored as JSON in `Notifications.Settings`)

| Order | Field (JSON camelCase) | Type | Default | Validation |
| --- | --- | --- | --- | --- |
| 0 | `host` | string | – | `ValidHost()` |
| 1 | `port` | int | **8096** | 1..65535 |
| 2 | `useSsl` | bool | false | – |
| 3 | `urlBase` (Advanced) | string | – | empty or path: no control/whitespace chars, no `? # : \`, must not start with `//` |
| 4 | `apiKey` (Privacy=ApiKey, masked `********` by inherited `SchemaBuilder`) | string | – | NotEmpty; no control/whitespace, `"`, `,`, `\` (header-injection guard) |
| 5 | `libraryId` (Select, options via `RequestAction("libraries")`) | string GUID | – | parsable non-empty GUID (hyphenated or `N` form) |
| 6 | `refreshMode` | int enum `JellyfinRefreshMode` | **1 = Interval** | defined enum: 0 `EachImport`, 1 `Interval`, 2 `EachImportAndInterval` |
| 7 | `refreshIntervalMinutes` | int | **60** | >= 1 (no upper bound) |
| 8 | `enableWatchedSync` | bool | **false** | – |
| 9 | `watchedUserId` (Select via `RequestAction("users")`) | string GUID | – | required non-empty GUID only when `enableWatchedSync` |
| 10 | `watchedSyncIntervalMinutes` | int | **5** | 1..1440 |
| 11 | `unmonitorWatchedEpisodes` | bool | false | – |
| 12 | `remonitorUnwatchedEpisodes` | bool | false | – |
| 13 | `syncListProgress` | bool | false | – |
| 14 | `jellyfinPathPrefix` (Advanced) | string | – | empty or absolute, no `.`/`..` segments, no control chars |
| 15 | `tanukarrPathPrefix` (Advanced) | string | – | same; both prefixes set or both empty |

Standard notification flags that matter: only `OnDownload` (UI "On Import") and `OnUpgrade` are
supported (`SupportsOnDownload` via reflection on the `OnDownload` override; `SupportsOnUpgrade => SupportsOnDownload`).
Connection `Tags` restrict series (inherited `ShouldHandleSeries` for refresh; explicit tag filter for watched sync).

### 11.2 Library refresh (migration 243)

**Schema (migration 243)** — table `JellyfinRefreshStates`: `Id` PK, `NotificationId INT NOT NULL` (unique index),
`TargetFingerprint VARCHAR(64) NOT NULL`, `StateToken VARCHAR(32) NOT NULL`, `LastRefreshMode INT NOT NULL DEFAULT 1`,
`ImportedGeneration BIGINT NOT NULL DEFAULT 0`, `ImmediateGeneration BIGINT DEFAULT 0`, `TickGeneration BIGINT DEFAULT 0`,
`LastTickUtc DATETIME NOT NULL`. No credentials stored.

**Trigger path.** `NotificationService.Handle(EpisodeImportedEvent)` (inherited, synchronous `IHandle`) returns early unless
`message.NewDownload` (download-client import or manual import from outside the series folder; rescans/existing-file
manual imports do not count), filters by connection tags, skips upgrades (`OldFiles` non-empty) unless `OnUpgrade`,
then calls `Jellyfin.OnDownload` → `JellyfinRefreshService.OnImport(definition)`. One call per imported **file**
(a multi-episode file = one trigger). Grabs, renames, file deletions, series add/delete do **not** trigger a refresh
(Jellyfin only overrides `OnDownload`).

**Active definition** (`IsActive`): saved (`Id > 0`), `Implementation == "Jellyfin"`, `OnDownload == true`, settings type ok.
Inactive → its state row is deleted (on import, tick, provider update/delete).

**Target fingerprint** = SHA-256 of `host.Trim().ToLower() \n port \n useSsl \n urlBase.TrimEnd('/') \n libraryGuid("N")`.
API key is **not** part of it (key rotation keeps pending work). A fingerprint change resets the row: new `StateToken`
(GUID N), all generations = 0, `LastTickUtc = now`, `LastRefreshMode = mode`.

**Algorithm (reimplementable):**
1. `OnImport`: under a per-connection lock, `ImportedGeneration += 1` (checked overflow), persist. If mode has an immediate
   component (`EachImport` or `EachImportAndInterval`) call `TryRefresh(id, processTick:false)` inline (HTTP happens on the
   import thread).
2. Scheduled `ProcessPendingJellyfinRefreshCommand` (every 1 minute, see 11.4) → `ProcessPending()`: delete state rows of
   inactive definitions (re-checked under lock), then `TryRefresh(id, processTick:true)` for each active Jellyfin definition,
   sequentially; one failing server does not stop the others.
3. `TryRefresh`: a non-blocking per-connection "in flight" flag (`ConcurrentDictionary.TryAdd`); if already refreshing,
   return (the running loop will see the new generation). Loop:
   - `tickDue = processTick && intervalMode && (now - LastTickUtc).TotalMinutes >= RefreshIntervalMinutes`
   - `ackImmediate = immediateMode && ImportedGeneration > ImmediateGeneration`
   - `ackTick = tickDue && ImportedGeneration > TickGeneration`
   - idle tick (`tickDue && !ackTick`) → `LastTickUtc = now`, persist, no HTTP.
   - nothing to ack → return.
   - Capture fingerprint + StateToken; `immediateGeneration = ImmediateGeneration + 1` (acknowledges **one** import per
     request); `tickGeneration = ImportedGeneration` (a tick acknowledges all).
   - Call `RefreshLibrary`. Any exception → log a sanitized warning (connection id only), keep watermarks, return
     (retried on the next 1-minute task). Failed tick does not advance `LastTickUtc`, so it is retried every minute.
   - On success, re-read under lock; if fingerprint or StateToken changed during HTTP → do not acknowledge. Else
     `ImmediateGeneration = max(.., immediateGeneration)`; if tick acked: `TickGeneration = max(.., tickGeneration)`,
     `LastTickUtc = now`; if the mode changed mid-flight, set both generations to `max(Immediate, Tick)`. Loop again.
   - Consequence: in immediate modes, N imports (or an N-import backlog after an outage) produce **N sequential refresh
     requests**, not one coalesced request. Interval mode coalesces to one request per due tick.
4. Mode change (detected in `GetState`): both generations set to `max(Immediate, Tick)` (no replay of acknowledged work;
   still-unacknowledged imports stay pending in `ImportedGeneration`), `LastTickUtc = now`. Interval-length change keeps the
   tick origin. Provider add/update → `InitializeDefinition` creates/re-fingerprints state; provider delete → state deleted.

**HTTP (all requests):** base `http(s)://{host}:{port}{urlBase}/`, header `Authorization: MediaBrowser Token="{apiKey}"`,
`AllowAutoRedirect=false`, `SuppressHttpError=true`, `LogHttpError=false`, `LogResponseContent=false`,
`StoreRequestCookie=false`, 30 s timeout. 401/403 → "rejected the API key or denied administrator access"; any non-2xx
(incl. 3xx redirects) → failure. Exception texts are replaced by fixed sanitized messages.

| Purpose | Request | Success criteria |
| --- | --- | --- |
| List libraries (UI select + validation) | `GET /Library/VirtualFolders` | 200, JSON array; entries kept only if `ItemId` is a non-empty GUID; UI option = `{Value: ItemId, Name, Hint: CollectionType}` sorted by name |
| Readiness check (Test **and before every refresh**) | selected library must appear exactly once in VirtualFolders and have non-empty `Locations[]`; then paged `GET /Items?parentId={libId:N}&recursive=true&includeItemTypes=Folder&fields=Path&startIndex={n}&limit=200&enableTotalRecordCount=true` until every `Location` matches a returned `Folder` item `Path` | Path compare: Windows-looking paths (`\` or `X:`) case-insensitive with `/`→`\` and trailing separator trimmed; otherwise ordinal with trailing `/` trimmed. Any location missing → "Initialize every folder of this library with a scan" error; imports remain pending |
| Refresh | `POST /Items/{libId:N}/Refresh?metadataRefreshMode=FullRefresh&imageRefreshMode=Default&replaceAllMetadata=false&replaceAllImages=false` | must return **204** exactly; anything else is a failure (pending kept) |

The server-wide `/Library/Refresh` is never called. Each refresh therefore costs >= 3 requests
(VirtualFolders + >=1 Items page + POST).

**Status: Operational.** Evidence: auto-registered `INotification` implementation; `OnDownload` → `OnImport` wiring
(`Jellyfin.cs:28-31`); scheduled task `ProcessPendingJellyfinRefreshCommand` Interval=1 in `TaskManager.cs`
(`DefaultTasks`); `IExecute<ProcessPendingJellyfinRefreshCommand>` in `JellyfinRefreshService`; table mapped in
`TableMapping.cs`. Docker E2E scripts exist (`docker/tests/jellyfin.py`, `jellyfin-refresh.py`, `ui-jellyfin.cjs`) but
were not executed in this audit.

### 11.3 Watched-state sync (migration 248)

**Schema (migration 248)** — `Episodes` gains `Watched BOOL NOT NULL DEFAULT false`, `WatchedAtUtc DATETIME NULL`,
`WatchedSource VARCHAR(64) NULL`, `JellyfinUnmonitorOwner VARCHAR(64) NULL`. New table `JellyfinWatchStates`:
`Id`, `NotificationId INT NOT NULL` (unique index), `TargetFingerprint VARCHAR(64) NOT NULL`, `LastSyncUtc DATETIME NOT NULL`,
`UnmonitorWatchedEpisodes BOOL NOT NULL DEFAULT false`.

**Active definition** (watched): `Implementation == "Jellyfin"`, saved, `EnableWatchedSync == true` (independent of
`OnDownload`). If more than one active watched definition exists the poll logs a warning and does nothing; `Test()` also
rejects enabling a second one.

**Source fingerprint** (`WatchedSource`, also the unmonitor-ownership token) = SHA-256 of
`notificationId \n host.ToLower() \n port \n useSsl \n urlBase.TrimEnd('/') \n libraryId(no hyphens, lower) \n
watchedUserId(no hyphens, lower) \n Normalize(jellyfinPathPrefix) \n Normalize(tanukarrPathPrefix)`. Because it includes
the notification id and user, a new connection or a user/target change creates a new authority.

**Poll algorithm** (`JellyfinWatchedService.Sync(force)`, global lock):
1. Skip unless forced if `state.TargetFingerprint == fingerprint && (now - LastSyncUtc) < WatchedSyncIntervalMinutes`.
   Scheduled runs are not forced; a manual run of the task (`CommandTrigger.Manual`) is forced.
2. Snapshot (`JellyfinWatchProxy.GetSnapshot`): validate settings; `GET /Users` (array; every id a unique non-empty GUID,
   non-empty `Name`) must contain the selected user; `GET /Library/VirtualFolders` must contain the library;
   `GET /Users/{userId:N}/Items/{libId:N}` must return 200 with `Id == libId` (user-scoped access check); then pages of
   `GET /Users/{userId:N}/Items?parentId={libId:N}&recursive=true&includeItemTypes=Episode,Movie&fields=Path&enableUserData=true&enableTotalRecordCount=true&sortBy=SortName&sortOrder=Ascending&startIndex={n}&limit=200`.
   Rejected (whole snapshot invalid, previous state retained) if: non-200; missing/negative `TotalRecordCount`;
   **TotalRecordCount > 100,000**; page > 200 items; total changes between pages; overrun; duplicate item `Id`; item
   `Type` not Episode/Movie; missing `UserData.Played`; path not normalisable; empty page before reaching total.
3. Re-read the definition after HTTP; abort without writing if it was disabled/changed (fingerprint, the three policy
   booleans, tags) or another watched connection appeared.
4. `ApplySnapshot`: translate each item path (see 11.5); items outside the configured Jellyfin prefix are ignored; if
   two items translate to the same path with different `Played` values → throw (no writes). Build local map
   `Normalize(series.Path.TrimEnd('/','\\') + "/" + episodeFile.RelativePath) → EpisodeFile` over **all** episode files of
   series that pass the tag filter (series with any overlapping tag when the connection has tags); paths that map to more
   than one file are dropped (ambiguous). For each matched file, every episode returned by `GetEpisodeByFileId(file.Id)`
   (same series) is a candidate; an update is queued when `Watched != played`, or source differs, or
   (`played && unmonitor && Monitored`), or (`!played && JellyfinUnmonitorOwner == fingerprint`).
   - `unmonitor = UnmonitorWatchedEpisodes && (applyInitialUnmonitor || !episode.Watched || episode.WatchedSource != fingerprint)`
     i.e. only on a transition to watched, or once for already-watched episodes when the option is newly enabled
     (`applyInitialUnmonitor = setting && (state fingerprint differs || !state.UnmonitorWatchedEpisodes)`).
   - `remonitor = RemonitorUnwatchedEpisodes && series.Monitored && season(SeasonNumber).Monitored`.
   All validation happens before any write. Absent/unmatched remote items never mark anything unwatched.
5. Writes: `EpisodeRepository.SetJellyfinWatched` — one SQL `UPDATE Episodes ... WHERE Id=@id AND EpisodeFileId=@fileId`
   (stale file ids do not match):
   - `Watched = @watched`, `WatchedSource = @owner`
   - `WatchedAtUtc = watched ? (same source ? COALESCE(existing, @at) : @at) : NULL`, where `@at = LastPlayedDate.ToUniversalTime()` or now
   - `Monitored = false` when watched && unmonitor && currently monitored; `= true` when !watched && remonitor &&
     `JellyfinUnmonitorOwner = @owner` && `Series.Monitored`; else unchanged
   - `JellyfinUnmonitorOwner = @owner` when this update unmonitored it; `NULL` when unwatched and owned by @owner; else unchanged
   Then reloads the row and publishes the model-updated event (SignalR refresh). Logs count of updated episodes.
6. Persist `JellyfinWatchStates` (`TargetFingerprint`, `LastSyncUtc = now`, `UnmonitorWatchedEpisodes` = current setting).
7. If `syncListProgress`: for **every** matched series (even unchanged), re-check authorization
   (`IsProgressAuthorized`: same definition, still the only active watched connection, same fingerprint/policy/tags,
   series still in tag scope), collect episode ids with `Watched && WatchedSource == fingerprint`, call
   `IImportListProgressService.SyncProgress(seriesId, watchedIds, canDispatch)` (exceptions logged, local state kept).
   The same `canDispatch` delegate is re-invoked by the progress service before each list/title target.
8. Any exception in steps 2-7 → sanitized warning; `LastSyncUtc` unchanged so the next tick retries.

**Manual monitoring edits relinquish ownership** (fork changes in `EpisodeRepository`): `SetMonitoredFlat`,
`SetMonitored(ids,…)`, season-level `SetMonitoredBySeason`, the generic where-clause monitor update and
`ClearFileId(…, unmonitor:true)` now also set `JellyfinUnmonitorOwner = NULL`; the season/where-clause SQL no longer
has the `AND Monitored != @monitored` filter, so ownership is cleared even when the flag is unchanged.

**What watched state does to Tanukarr data:** sets the four Episode columns; optionally flips `Episodes.Monitored`
(never series/season flags, never files); shown in UI; exposed read-only in API; feeds import-list progress. It does not
affect search/wanted/cutoff except through `Monitored`. Deleting the connection deletes its `JellyfinWatchStates` row but
keeps episode watched flags (and stale ownership tokens, which can never match a new connection's fingerprint).

**Hand-off to the import-list progress queue** (details belong to the import-list section): interface
`IImportListProgressService.SyncProgress(int seriesId, IReadOnlyCollection<int> watchedEpisodeIds, Func<bool> canDispatch)`
(`ImportLists/Progress/ImportListProgressService.cs`). It computes a contiguous watched prefix
(`ImportListProgressGuard.ContiguousProgress`), refuses unless all watched episodes share exactly one non-empty
`WatchedSource`, only considers import lists with `EnableAutomaticAdd`, requires stored import-list item membership
mapped by a writer (`AniList/MyAnimeList/Trakt/Simkl` progress writers, each gated by the list's own
`SyncWatchedProgress` setting), persists `ImportListProgressStates` (migration 249) before I/O, caps dispatch at
**10 targets per minute** (in-memory window), and backs off `5 min × 2^(failures-1)` capped at 360 min (Retry-After up
to 1 day; "unavailable" → 1 day).

**Status: Operational** (opt-in, default off). Evidence: `SyncJellyfinWatchedCommand` Interval=1 in `TaskManager.cs`;
`IExecute<SyncJellyfinWatchedCommand>`; columns mapped (Episode mapping in `TableMapping.cs` ignores only
`SeriesTitle/Series/HasFile/AbsoluteEpisodeNumberAdded/AirDateMovedToPast`); resources map the fields. Remote account
writes depend on the import-list writers (other section) and were not exercised against real provider accounts per the
fork's own docs.

### 11.4 Scheduling

`TaskManager.DefaultTasks` adds `ProcessPendingJellyfinRefreshCommand` (Interval **1** min) and `SyncJellyfinWatchedCommand`
(Interval **1** min). The inherited scheduler timer fires every **30 s** and runs tasks whose
`LastExecution + Interval < now`, so effective cadence is ~1-1.5 min; the per-connection `RefreshIntervalMinutes` and
`WatchedSyncIntervalMinutes` are enforced inside the services. Both tasks run even with no Jellyfin connection (cheap
DB reads). `SyncJellyfinWatchedCommand.SendUpdatesToClient = true`, so each minute's run is broadcast to the UI command
feed. Commands can also be posted by name (`ProcessPendingJellyfinRefresh`, `SyncJellyfinWatched`) via
`POST /api/v3/command` and appear in System → Tasks (inherited mechanics).

Side note outside this scope but in the same `TaskManager.cs` diff: the fork **removed** the scheduled
`ApplicationUpdateCheckCommand` and added a daily `RefreshImdbDatasetCommand`.

### 11.5 Path normalisation and prefix translation (`JellyfinWatchPath`)

- `Normalize(p)`: null if empty or contains control chars. Windows path = starts with `\\` or `X:` followed by `/` or `\`.
  Windows: replace `\`→`/`. Reject non-Windows paths not starting with `/`, and any `.`/`..` segment. Trim trailing `/`
  (empty → `/`). Windows results are upper-cased (case-insensitive compare); POSIX results keep case.
- `Translate(p)`: if no Jellyfin prefix → `Normalize(p)`. Else `src = Normalize(jfPrefix).TrimEnd('/')`,
  `dst = Normalize(tkPrefix).TrimEnd('/')`; require `Normalize(p).StartsWith(src + "/")` (directory boundary: `/library/tv`
  does not match `/library/tv-other`), otherwise the item is ignored; result = `Normalize(rawTanukarrPrefix.TrimEnd('/') +
  rawPathSuffix)` preserving the suffix's original case (Windows server → Linux client).
- Local side uses the same `Normalize` on `series.Path + "/" + relativePath` — note the local side is **not** translated
  and is compared ordinally.

### 11.6 API surface and UI exposure

- Connection CRUD: inherited provider endpoints `/api/v3/notification` and `/api/v5/connection` (schema, test, testall,
  `POST …/action/libraries`, `POST …/action/users`). `Test()` runs the readiness check (no refresh is posted) and, when
  watched sync is enabled, rejects a second watched connection and performs a full watched snapshot read.
- Episode API (`/api/v3/episode`, `/api/v5/episode`): new read-only `watched` (bool) and `watchedAtUtc` (nullable UTC).
  Not settable through episode edits (no controller uses them). `WatchedSource`/`JellyfinUnmonitorOwner` are not exposed.
- UI: no bespoke settings page — fields render through the generic provider form (Settings → Connect → Jellyfin).
  `frontend/src/Settings/useProviderOptions.ts` was changed so the `libraries`/`users` select options are only fetched
  once host/port/apiKey are valid and re-fetched when host/port/useSsl/urlBase/apiKey change. Series details show a
  "Watched" label per episode (`frontend/src/Episode/EpisodeWatchedStatus.tsx`, tooltip "Watched in Jellyfin on {date}")
  and a per-season "{watched}/{total} watched" count (`SeriesDetailsSeason.tsx`). New en.json keys
  `NotificationsJellyfinSettings*`, `EpisodeWatchedAt`, `EpisodeWatchedSource`, `SeasonWatchedCount`, `Watched`.

### 11.7 Tests

Unit (`src/NzbDrone.Core.Test/NotificationTests/Jellyfin/`): `JellyfinFixture` (8), `JellyfinImportDispatcherFixture`
(NewDownload/upgrade/tag gating), `JellyfinProxyFixture` (~20 cases incl. real Newtonsoft deserialization, redirect
rejection, pagination, Windows paths), `JellyfinRefreshServiceFixture` (~28 cases: idle ticks, coalescing, failure
retention, mode switches, key rotation, target change mid-flight, restart persistence), `JellyfinScheduledTaskFixture`
(both 1-minute tasks persisted), `JellyfinSettingsFixture`, `JellyfinWatchedServiceFixture` (~20 cases: ownership,
manual-choice preservation, conflicting paths, settings change mid-request, two connections, tags, origin progress
re-authorization), `JellyfinWatchPathFixture`, `JellyfinWatchProxyFixture`. Migration fixtures:
`243_persistent_jellyfin_refresh_queueFixture`, `248_jellyfin_watched_stateFixture`. Container E2E (not run here):
`docker/tests/jellyfin.py`, `jellyfin-refresh.py`, `jellyfin-watched.py`, `list-progress-mock.py`, `ui-jellyfin.cjs`,
`ui-jellyfin-watched.cjs`.

### 11.8 Doc verification (docs/jellyfin.md, docs/jellyfin-watched.md, README/FORK)

Verified as accurate against code: selected-library-only refresh and exact query string; 204 = accepted, not finished;
readiness check before every refresh; redirects rejected; key masked and not in state table; defaults (Interval mode,
60 min, min 1; watched 5 min, 1..1440); idle ticks send nothing; interval origin resets on enable/target/mode change but
not on interval-length change; key rotation keeps work, target change / disabling On Import / deletion clears it;
NewDownload/OnUpgrade/tag semantics; one-connection rule; tag scoping; unmonitor-on-transition and
"once when newly enabled"; ownership-gated remonitor requiring monitored series+season; manual edits relinquish
ownership; exact-prefix translation; ambiguous mappings skipped; Played flag only; absent items never unwatch;
API exposes `watched`/`watchedAtUtc` only; one-source fingerprint requirement and 10 targets/min for list progress.

Overstatements / gaps:
- `docs/jellyfin.md` l.74 says "This connection does not … synchronize watch status" — stale since the same connection
  definition now carries the watched-sync opt-in (fields 8-15).
- "Every imported media file" mode is literally one refresh request **per imported file**, including replaying an
  N-import backlog as N sequential refreshes (each preceded by the >=2-request readiness check). Docs describe the
  per-import behaviour but not the backlog amplification.
- The immediate refresh runs synchronously inside the import's notification handler (30 s timeout per request, several
  requests); docs do not mention that it can delay import processing.
- Undocumented hard limit: watched snapshots of more than **100,000** Episode+Movie items are always rejected, so sync
  would never complete for such a library.
- Undocumented cost: each poll loads all series and all episode files and does one `GetEpisodeByFileId` query per
  matched file (N+1), plus one `GetEpisodes(seriesId)` per matched series when list progress is on.
- Potential race (UNVERIFIED in practice): other code paths that save whole `Episode` rows (e.g. refresh `UpdateMany`)
  write the in-memory `Watched*`/`JellyfinUnmonitorOwner` values and could revert a concurrent watched update; no
  locking couples them.
- Behavioural consequence not documented (inferred, UNVERIFIED against a live Jellyfin): after an upgrade/rename the new
  file path is a new Jellyfin item (typically unplayed), so the next poll flips the episode to unwatched and, if
  remonitor is on and this connection had unmonitored it, re-monitors it.
- Notifications for multi-season-pack imports via this connection are irrelevant here, but note refresh is not
  triggered by file deletion/rename, so Jellyfin keeps stale items until its own scan.

## 12. Notifications changes

Method: `LC_ALL=C diff -ru` of `src/NzbDrone.Core/Notifications` (excluding the new `Jellyfin/` dir) between the upstream ref
(develop `c018578`, 2026-11-02) and Tanukarr; 34 modified files, ~86 added / ~106 removed lines. Direction was established with
Tanukarr's shallow-history boundary commit `bcab4b0` (2026-08-02, Mark McDowall, a pure upstream tree — it contains no
fork files such as `Notifications/Jellyfin`, `NzbDrone.Core/AniList`, `FORK.md`): anything present in `bcab4b0` and in
Tanukarr HEAD but not in the upstream ref is an **upstream change made after Tanukarr's base that Tanukarr lacks**.
Upstream fixes Tanukarr already absorbed (identical in both trees, so not in the diff) include `1580ff8` Manual
Interaction Required for unknown series, `f382d6e` update all matching Plex sections, `7082970` media-cover caching.

### 12.1 Fork changes (Tanukarr-authored)

| Kind | Files | Change |
| --- | --- | --- |
| Branding (commits `8523e2c` "Tanuki branding", `a62c454` "KvasFan namespace") | `NotificationBase.cs` (10 `*_TITLE_BRANDED` constants), Apprise, Discord, Email, Gotify, Join, Mailgun, MediaBrowserProxy/Service, NotificationService (update message), Ntfy, Prowl, PushBullet, Pushcut, Pushover, SendGrid, Signal, Simplepush, Slack, Telegram, Twitter, Xbmc, XbmcJsonApiProxy | "Sonarr" → "Tanukarr" in titles ("Tanukarr - Episode Grabbed" etc.), test messages, Kodi headers, "Tanukarr updated from X to Y"; logo URLs → `https://raw.githubusercontent.com/KvasFan/Tanukarr/main/Logo/{64,128,256,96-Outline-White}.png`; `https://sonarr.tv` links → `https://github.com/KvasFan/Tanukarr` (Gotify click URL/markdown, Telegram metadata link). Purely cosmetic. Not changed: CustomScript `Sonarr_*` environment variable names, webhook payload schema, instance-name handling — integrations keyed on those remain compatible. |
| Jellyfin enablement | `NotificationDefinition.cs` | `Enable` additionally true for `Implementation == "Jellyfin"` with `EnableWatchedSync` (watched-only connection counts as enabled/listed as active). |
| New provider | `Notifications/Jellyfin/**` | See section 11. |

No new event payload fields (no movie/collection/provider-identity context) were added to any notifier, webhook
(`WebhookSettings` only differs by an Advanced flag, see 12.2), CustomScript, Discord or Telegram payload. The fork's
movie members and collection features are therefore not reflected in notification content beyond the series/episode
fields upstream already sends.

### 12.2 Upstream-only changes missing from Tanukarr (upstream ref is newer)

| Upstream change (absent in Tanukarr) | Files | Evidence |
| --- | --- | --- |
| Multi-season-aware messages via `Parser/SeasonNumbersFormatter` ("Series - Seasons 1-3", "Seasons 1, 3"); `GetFullSeasonMessage(series, episodes, quality)`; `DownloadCompletedEvent` carries `Series`/`Episodes` | `NotificationService.cs`, `Discord/Discord.cs` (`GetTitle`), `Slack/Slack.cs` (`GetTitle`), `Download/DownloadCompletedEvent.cs` | `SeasonNumbersFormatter` exists only in upstream (`grep` finds 0 uses in Tanukarr, 0 in `bcab4b0`); upstream-only test `NotificationTests/NotificationServiceFixture.cs` (`should_show_season_range_when_grabbing_multi_season_pack`, `…imported`, non-contiguous seasons, single season). Tanukarr's `DownloadCompletedEvent(trackedDownload, int seriesId, files, release)` matches `bcab4b0`. |
| Trakt collection audio channels: `MediaInfo` null → `""`, 0 channels → `""`, format with `CultureInfo.InvariantCulture` | `Trakt/Trakt.cs` (`MapAudioChannels`) | Upstream-only `NotificationTests/TraktTests/TraktFixture.cs` (culture-parameterised cases). Tanukarr uses `ToString("0.0")` with the current culture (e.g. `5,1` under a comma-decimal locale) and sends `0.0` for unknown. |
| Un-hiding 18 settings fields (`Advanced = true` removed upstream) | Discord (Author, OnGrab/OnImport/OnManualInteraction fields), Email (CC, BCC), MediaBrowser (UrlBase, MapPathsFrom, MapPathsTo), Ntfy (Headers), Plex Server (UrlBase, MapPathsFrom, MapPathsTo), Pushover (TTL, EncryptionKey), Telegram (IncludeInstanceName), Webhook (Headers), Xbmc (UrlBase) | `bcab4b0` already has these `Advanced = true` flags (e.g. 4 in `DiscordSettings.cs`, 2 in `EmailSettings.cs`). The same pattern spans indexers/download clients (Core-wide: 113 `Advanced = true` in Tanukarr vs 18 upstream) — **not** a fork UX change, contrary to what a naive diff suggests. Stored settings are unaffected; only "Show Advanced" visibility differs. |

Practical consequence for the fork's headline multi-season-pack feature: Tanukarr's notification text for a
`Series.S01-S05` grab is built by the single-season `GetMessage` path — `"{Title} - {firstSeason}x01x02…xNN - {all
episode titles joined by ' + '} [quality]"` (every episode of every season, prefixed with the first season number) — and
the import-complete message for a full multi-season pack is `"{Title} - Season {firstSeason} [quality]"`. Discord/Slack
titles are built the same way and are not truncated (Discord limits embed titles to 256 chars, so such a grab
notification is likely to be rejected — UNVERIFIED, not exercised).

### 12.3 Behavioural fixes

None fork-authored in this directory beyond Jellyfin. The only behavioural deltas are the upstream-only items in 12.2
(multi-season message formatting, Trakt culture/zero-channel fix), which Tanukarr lacks.

### 12.4 Tests

Tanukarr adds only Jellyfin fixtures (section 11.7). It lacks upstream's `NotificationServiceFixture` and `TraktFixture`.
Branding has no dedicated tests.

### Status table (sections 11-12)

| Item | Status | Evidence |
| --- | --- | --- |
| 11a Jellyfin selected-library refresh (modes EachImport / Interval / Both; persistent generations, migration 243) | **Operational** | `Jellyfin.OnDownload` → `JellyfinRefreshService.OnImport`; `ProcessPendingJellyfinRefreshCommand` scheduled every 1 min (`TaskManager.cs`); `JellyfinProxy.RefreshLibrary` POST `/Items/{id}/Refresh` with 204 check; table mapped; ~70 unit cases |
| 11b Readiness check (all library `Locations` indexed as Folder items before refresh) | **Operational** | `JellyfinProxy.TestLibrary` called from `RefreshLibrary` and `Test()` |
| 11c Watched-state sync (one user, path matching, migration 248) | **Operational** (opt-in, default off) | `SyncJellyfinWatchedCommand` every 1 min; `JellyfinWatchedService.Sync`; `EpisodeRepository.SetJellyfinWatched` SQL; resource fields in v3/v5 |
| 11d Unmonitor watched / ownership-gated remonitor | **Operational** (opt-in) | `SetJellyfinWatched` CASE logic; ownership cleared by `SetMonitoredFlat`/`SetMonitoredBySeason`/`SetMonitored(ids)`/`SetMonitoredWhere`/`ClearFileId` |
| 11e Path-prefix translation | **Operational** | `JellyfinWatchPath.Translate` used in `ApplySnapshot`; validator requires both-or-neither |
| 11f Hand-off to list progress queue | **Operational locally; remote writes UNVERIFIED** | `IImportListProgressService.SyncProgress` called per matched series when `syncListProgress`; remote acceptance depends on provider writers/accounts (not exercised; fork docs say the same) |
| 11g UI exposure | **Operational** | Generic provider form + `useProviderOptions.ts` gating; `EpisodeWatchedStatus.tsx`; season watched count |
| 11h Emby / Jellyfin (MediaBrowser) legacy connection | **Unchanged** (branding only) | Diff limited to logo URL/test text + upstream-later Advanced flag removal |
| 12a Notification branding | **Operational** (cosmetic) | String/URL replacements in 23 files |
| 12b New notification payload context (movie/collection) | **Not implemented** | No payload/model changes in diff |
| 12c Multi-season-aware notification text | **Not implemented in Tanukarr** (exists upstream) | `GetMessage`/`GetFullSeasonMessage(series, episodes.First().SeasonNumber, …)`; upstream `SeasonNumbersFormatter` absent |
| 12d Trakt audio-channel culture fix | **Missing** (upstream-only) | Tanukarr `MapAudioChannels` uses current culture |
| 12e Advanced-flag differences | **Upstream-only change**, not fork | Present in pure-upstream `bcab4b0` |


## 13. Import lists

Scope: `src/NzbDrone.Core/ImportLists/**`, `src/NzbDrone.Core/AniList/**`, migrations 241/242/249/250, v3/v5 import-list resources, `Sonarr.Api.V5/LibraryTools/ImportListPreviewController.cs`, and the matching React settings UI. Method: `diff -u` against upstream `develop@c018578` plus `git log` per file in the (shallow) Tanukarr clone to separate fork edits from upstream drift.

**Attribution caveat (important when reading the diff list).** Tanukarr's base is upstream `develop@57816c149` + cherry-picks; the reference upstream is newer. Several import-list diffs are therefore *upstream* changes Tanukarr lacks, not fork edits:

- `ImportListBase.CleanupListItems` — upstream later added `TmdbId` to the dedup key; Tanukarr's file was never touched by a fork commit (only the shallow-boundary commit `bcab4b0`).
- `HttpImportListBase` (`TestConnection` uses `!releases.Any(IsValidItem)` upstream vs `releases.Empty()` here), `Trakt/TraktParser.cs`, `Trakt/User/TraktUserParser.cs`, `Trakt/Popular/TraktPopularParser.cs` — upstream-only drift (STJson `ResponseData` vs `Content`).
- `Only in sonarr: ImportLists/Tmdb/TmdbTokenResources.cs` and `NzbDrone.Core.Test/ImportListTests/Tmdb/` — upstream's own TMDb import lists (added after Tanukarr's base) use an OAuth exchange through `services.sonarr.tv` (`auth/tmdb/request`, `auth/tmdb/access`, then `https://www.themoviedb.org/auth/access`). Tanukarr instead carries **KakarottoCake's** independent TMDB implementation (commit `f8b5d9d`), which takes a user-pasted **TMDB API Read Access Token** (`AuthToken`, visible field, `Privacy=ApiKey`) and optional `AccountId`; `RequestAction` returns `{}` (no OAuth). Verified in `Tmdb/TmdbImportBase.cs`/`TmdbSettingsBase.cs`. Upstream also forces `BaseUrl == https://api.themoviedb.org`; Tanukarr only requires a valid root URL. Tanukarr blocks `services.sonarr.tv` and `sentry.sonarr.tv` in `NzbDrone.Common/Cloud/SonarrCloudRequestPolicy.cs`, so upstream's flow could not work there anyway.
- `Trakt/SmartList/*` exists in both trees but was written independently: Tanukarr (KakarottoCake) stores the setting as `SmartList` (URL **or** slug, parsed by `TraktSmartListSlug`: regex `/(?:lists/smart/view|smart-lists)/(?<slug>[^/?#]+)`, slug `^[\w-]+$`); upstream stores `SmartListId` (slug `^[a-z0-9-]+$`). Both call `/smart-lists/{slug}/items/shows`. **Settings JSON is not compatible** between the two for this provider.
- `ImportListType` enum: Tanukarr `Program, Plex, Trakt, Simkl, Other, Advanced, Tmdb` (Tmdb = 6); upstream inserts `Tmdb` before `Other` (Tmdb = 4). The type is **not persisted** (`TableMapping` ignores `ListType`), but `listOrder` in the API differs, so only the add-list grouping order is affected.

Also unchanged from upstream and still in use: OAuth for **AniList, Trakt and Simkl V1** goes through Servarr's broker `https://auth.servarr.com/v1/{anilist|trakt|simkl}_sonarr/{auth,renew}` (`AniListImportBase.cs`, `TraktImportBase.cs`, `SimklImportBase.cs`). The fork's cloud-block policy does not cover `auth.servarr.com`. Only **MyAnimeList** (direct PKCE) and **Simkl V2** (direct PKCE) avoid a broker, and TMDB uses no OAuth at all.

### 13.1 Sync pipeline and native series identity (`ImportListSyncService`, `ImportListIdentity`)

Files: `ImportLists/ImportListSyncService.cs` (modified), `ImportLists/ImportListIdentity.cs` (new), `ImportLists/FetchAndParseImportListService.cs` (modified), `Tv/AddSeriesService.cs` (bulk add, modified).

**Upstream behaviour being replaced.** Upstream maps every list item to a **TVDB ID** before adding: IMDb → `SearchForNewSeriesByImdbId`, TMDB → `SearchForNewSeriesByTmdbId`, AniList → `SearchForNewSeriesByAniListId`, MAL → `SearchForNewSeriesByMyAnimeListId` (SkyHook mappings). Unmappable AniList/MAL items are rejected. Exclusions, existing-series and pending checks are keyed by `TvdbId`.

**Tanukarr algorithm (`ProcessListItems`):** for each item whose list has `EnableAutomaticAdd`:
1. `existing = allSeries.First(s => ImportListIdentity.Matches(s, item))`.
2. Otherwise `candidate = ImportListIdentity.CreateSeries(item, lookup, list.Language, list.PreferredMetadataSource)`. This makes **no metadata request** unless the item has only an IMDb ID. `InvalidOperationException` means the item is skipped and logged at Debug. Then `existing ??= allSeries.First(s => SameSeries(s, candidate))`. `SameSeries` is true when both TVDB IDs are positive and equal, or when `MetadataSource` matches and `ForeignId` is equal and non-empty.
3. The item is rejected if **any** exclusion matches either the raw item **or** the candidate (`IsExcluded(exclusion, item) || IsExcluded(exclusion, candidate)`).
4. If `existing` is set, tags are queued for an existing series (TagExisting logic, unchanged) and the item is skipped.
5. A pending duplicate (`Matches(pending, item) || SameSeries(pending, candidate)`) only unions its tags.
6. Otherwise the candidate gets the list's Monitored/MonitorNewItems/root/profile/language/season-folder/tags/seasons/AddOptions. Two rules change here: `SeriesType = Anime` is **forced** when the source is AniList or MyAnimeList (the list's SeriesType is ignored), and `SeasonType = Official`.
7. `AddSeriesService.AddSeries(list, ignoreErrors:true)`: on `TooManyRequestsException` the rest of that metadata source is skipped for this run. `HttpException` leaves the entry pending; `ValidationException` is logged. The completion log now reports the actually-added count.

**`ImportListIdentity` rules (reimplement exactly):**
- `GetSource(item, pref)` returns the preferred source if `pref != Automatic` and the item has a **positive** ID for it. Otherwise the first positive ID in the fixed order **TVDB → AniList → TMDB → MAL**, else `null`.
- `CreateSeries` with no source: an IMDb-only item does `SearchForNewSeriesByImdbId(...).First()` (throws if none); anything else throws "does not supply a supported show ID". With a source it builds `Series{MetadataSource, ForeignId=id, TvdbId=item.TvdbId>0?..:0, TmdbId>0?..:0, ImdbId, Title, Year, SeriesType=Anime if AniList/MAL else Standard, SeasonType=Official}` and adds `AniListIds`/`MalIds` when positive. No cross-catalogue ID is guessed. The negative TVDB placeholder is assigned later by add-series code, which the metadata audit covers.
- `Matches(series, item)`: if `item.AniListId>0` and the series is AniList-owned with a ForeignId, the result is **only** `ForeignId == AniListId`. This stops sequels that share a TVDB parent from matching. Otherwise any of: equal positive TVDB; equal positive TMDB; `series.AniListIds ∋ item.AniListId`; `series.MalIds ∋ item.MalId`; equal non-empty IMDb; TMDB-owned with `ForeignId == TmdbId`; MAL-owned with `ForeignId == MalId`.
- `IsExcluded(exclusion, series|item)`: a native exclusion (`MetadataSource != Tvdb` and ForeignId set) matches by `(source, ForeignId)`, i.e. the item's ID for that source. Otherwise it matches only when both TVDB IDs are **positive** and equal.
- `RemoveTagsFromSeriesNotOnLists` and `CleanLibrary` now use `Matches` (`IsSeriesOnList`) instead of upstream's `ListItemIds` hash sets. Cleanup gating (`AllListsSuccessfulWithAPendingClean`, ListSyncLevel) is unchanged from upstream.

**Behavioural consequence (not prominent in docs).** Under the default **Automatic** preference, a TMDB-only item becomes a **TMDB-owned series**, not a TVDB series as upstream would make it. TMDB List/Discover/Person/Account parsers emit only `TmdbId`; MDBList/Trakt/Plex rows emit it when TVDB is missing. An explicit `Tvdb` preference cannot force TVDB because it requires `item.TvdbId > 0`. Users who expect upstream's TVDB numbering for TMDB-sourced lists will get TMDB episode structure instead. Likewise, AniList and MAL list items create AniList/MAL-owned series rather than SkyHook-mapped TVDB series.

**Fetch layer changes (`FetchAndParseImportListService`):**
- Every `Fetch`/`FetchSingleList` runs inside `ImportListSnapshotScope.BeginIfNeeded()` (see 13.8).
- If a provider throws, Tanukarr now also sets `result.AnyFailure = true` and calls `RecordFailure(id)`; upstream only logged.
- Cross-list dedup key changed from `{TvdbId,TmdbId,ImdbId,Title}` to `{ImportListId,TvdbId,ImdbId,TmdbId,AniListId,MalId,Title}`. Per-list membership (for tags) and distinct AniList entries that share titles or TVDB IDs are kept.
- **Completed-fetch resume** (new). The cache key is the list id. The value is `{SHA-256 of the definition (all ImportListDefinition fields incl. Settings/credentials, Tags, ListType, MinRefreshInterval), LastInfoSync}`, held in memory for `MinRefreshInterval`.
  - A witness is written only after a successful fetch whose persisted status has `LastInfoSync`, no `DisabledTill`, no failure since that sync, and no item carrying per-season overrides (`Seasons.Count > 0`).
  - When a supported list is skipped for not being due, its **saved `ImportListItems` rows** are re-fed into processing. This requires the witness to exist, the fingerprint and LastInfoSync to match, and no failure or disable since. The effect is that entries deferred by provider throttling are retried on the next scheduled sync without new HTTP.
  - Supported implementations: `AniListImport`, `MyAnimeListImport`, `TmdbListImport`, `MDBListImport`, `TraktListImport`, `PlexImport`, `SimklUserImport`.
  - The witness is process-local, so a restart drops it.

Status: **Operational**. Wired through the unchanged `ImportListSyncCommand` scheduling. Tests: `ImportListSyncServiceFixture` (39 cases, e.g. `should_add_distinct_anilist_entries_without_mapping_them_to_tvdb`, `should_apply_a_list_provider_preference_to_new_series_only`, `should_preserve_native_global_exclusions_when_a_second_catalog_is_selected`), `ImportListIdentityFixture` (22), and `FetchAndParseImportListFixture` (66, including about 15 `should_resume_*`/`should_not_resume_*` cases).

### 13.2 Metadata source preference for import lists (migration 250)

- Enum `ImportLists/ImportListMetadataSource.cs`: `Automatic=0, Tvdb=1, Tmdb=2, AniList=3, MyAnimeList=4`. The mapping to `MetadataSourceType` (`Tvdb=0, Tmdb=1, AniList=2, MyAnimeList=3, …`) is in `ImportListIdentity.GetPreferredSource`.
- Schema: migration **250** `import_list_metadata_source` adds `ImportLists.PreferredMetadataSource INT NOT NULL DEFAULT 0`. Model: `ImportListDefinition.PreferredMetadataSource`.
- API (v3 and v5): `ImportListResource.preferredMetadataSource` (nullable). On write, null keeps the existing value, else Automatic. The validator rejects undefined enum values ("Select a supported metadata source or Automatic.").
- UI: `frontend/src/Settings/ImportLists/ImportLists/EditImportListModalContent.tsx` adds a "Metadata provider for new series" select (automatic/tvdb/tmdb/aniList/myAnimeList). Default is Automatic.
- Semantics: applied only to **new** candidates in `ProcessListItems`, and only when the item carries a positive ID for that catalogue. If it falls back, a Debug log line is written. Existing series keep their owner and paths.
- **Gap:** the Library Tools preview/add path (`ImportListPreviewController`) calls `GetSource(item)` and `CreateSeries(item, lookup, language)` **without** the preference, so manual "add selected" from a list preview always uses Automatic. It also ignores the list's SeriesType, SeasonFolder (hard-coded true) and Tags.
- Status: **Operational** for scheduled/forced sync; **Partial** for the preview add path. Tests: `250_import_list_metadata_sourceFixture`, `NzbDrone.Api.Test/ImportLists/ImportListMetadataSourceResourceFixture` (10), and the sync fixture cases above.

### 13.3 Exclusion identity (migration 242)

- Schema: migration **242** `preserve_import_list_exclusion_identity` adds `ImportListExclusions.MetadataSource INT NOT NULL DEFAULT 0` and `ForeignId TEXT NULL`. **No backfill.** Legacy rows stay TVDB exclusions; the fixture `should_keep_legacy_exclusions_as_tvdb_exclusions` asserts this. An exclusion created before 242 for a native AniList/MAL/TMDB series stored only that series' **negative placeholder** TVDB ID, and `IsExcluded` requires a positive TVDB ID, so such legacy exclusions are now **inert**.
- Model: `ImportListExclusion.MetadataSource`, `ForeignId`.
- `ImportListExclusionService` changes:
  - `Update`: when an older client sends a negative TvdbId with no ForeignId, the stored MetadataSource/ForeignId are copied back.
  - Series-deleted handler (when "add exclusion" is chosen) runs under a lock and skips series already excluded by `IsExcluded`. It writes `{TvdbId, Title, MetadataSource, ForeignId}`. Because the legacy `TvdbId` column is unique, a non-TVDB series whose placeholder collides gets a fresh placeholder `min(existing TvdbId) - 1` (or -1).
- API v3/v5 `ImportListExclusionResource` gains `metadataSource` and `foreignId`, round-tripped by the mappers. The exclusion controller validators are unchanged upstream ones: `TvdbId` NotEmpty and unique, Title NotEmpty.
- UI: the exclusions table and edit modal (`frontend/src/Settings/ImportLists/ImportListExclusions/*`) do **not** show or edit `metadataSource`/`foreignId`. Native exclusions are created only by deleting a series with "add exclusion".
- Status: **Operational** for exclusions created after 242. Legacy native exclusions are not recovered. Tests: `242_preserve_import_list_exclusion_identityFixture`, `ImportListExclusionServiceFixture` (4), `ImportListSyncServiceFixture.should_honor_an_exclusion_for_a_deleted_native_anilist_entry`.

### 13.4 Tag-existing type (migration 241 = upstream 236)

- Migration **241** `import_list_tag_existing_type` is byte-identical to upstream migration **236** apart from the number. It converts `ImportLists.TagExisting` from boolean (migration 231) to `INT NOT NULL DEFAULT 0`; PostgreSQL uses `USING "TagExisting"::integer`.
- Enum `TagExistingType {None=0, Add=1, Sync=2}` is identical to upstream. The behaviour (Add tags to existing series; Sync also removes tags from series no longer on any list with that tag, via `RemoveTagsFromSeriesNotOnLists`) is upstream commit `be26be8` (Mark McDowall). The fork only replaced the membership test with `ImportListIdentity.Matches`.
- **Compatibility hazard:** FORK.md states the fork numbered it 241 to keep its own 233–240. A database already migrated by upstream Sonarr records 233–240 under different meanings (upstream 233 = quality ranks, 234 = pending path, 236 = this, 237 = pending-release season numbers, 240 = slug). Tanukarr cannot safely open an upstream ≥233 database. Kitsunarr must not assume version-number equivalence.
- Test fixture file `236_import_list_tag_existing_typeFixture.cs` exercises the class that Tanukarr numbers 241.
- Status: **Operational** (upstream feature, renumbered).

### 13.5 AniList import list (custom include/exclude, status filters, full-collection snapshot, rate limiting)

Files: `ImportLists/AniList/AniListAPI.cs` and `List/AniListSettings.cs`, `List/AniListParser.cs`, `List/AniListRequestGenerator.cs`, `List/AniListImport.cs` (modified); `List/AniListAccountSnapshot.cs` and `List/AniListCustomListMembershipSnapshot.cs` (new); `AniListSettingsBase.cs` (tokens marked `Privacy=ApiKey`); `src/NzbDrone.Core/AniList/AniListRequestService.cs` and `AniListRequestDelay.cs` (new). `AniListImportBase.cs` is **unchanged upstream**: OAuth uses client id `13780` via `auth.servarr.com`, renewal happens when `Expires < now+5min`, and `MinRefreshInterval = 12h`.

**Settings** (`AniListSettings`; validator):

| Field | Order | Default | Notes |
| --- | --- | --- | --- |
| `Username` | 1 | — | target user |
| `CustomList` | 2 | `""` | hidden legacy scalar from the fork's 0.1.x; on deserialize or validate it is trimmed and merged into `IncludedCustomLists` (ordinal, no duplicate), then cleared |
| `ImportCurrent` | 3 | true | section "Import List Status" |
| `ImportPlanning` | 4 | true | |
| `ImportCompleted` | 5 | false | |
| `ImportDropped` | 6 | false | |
| `ImportPaused` | 7 | false | |
| `ImportRepeating` | 8 | false | |
| `ImportFinished` | 9 | true | new section "Import Media Release Status" |
| `ImportReleasing` | 10 | true | |
| `ImportUnreleased` | 11 | false | |
| `ImportCancelled` | 12 | false | |
| `ImportHiatus` | 13 | false | |
| `IncludedCustomLists` | 14 | `[]` | Tag field, `IEnumerable<string>` |
| `ExcludedCustomLists` | 15 | `[]` | Tag field, `IEnumerable<string>` |
| `SyncWatchedProgress` | 16 | false | see 13.9 |

Validation: at least one list status; **new** rule of at least one media release status; custom-list names must not be blank. `AccessToken` is required (inherited), so saving requires OAuth.

**Fetch algorithm (production path).** `AniListImport.FetchItems` overrides the base pager entirely. It loads one **complete account snapshot** and filters locally.
- `AniListAccountSnapshot.Load` POSTs GraphQL `QueryCollection` to `https://graphql.anilist.co` for chunk = 1..22 (`MaxCollectionEntries 11000 / CollectionChunkSize 500`): `User(name:$id){id name mediaListOptions{animeList{customLists}}}` + `MediaListCollection(userName:$id, type:ANIME, chunk:$chunk, perChunk:500){ user{id name} hasNextChunk lists{ name isCustomList entries{ id userId status progress updatedAt media{ id idMal type format status episodes title{userPreferred romaji english native} startDate endDate relations{edges{relationType(version:2) node{id type format}}} } } } }`.
- Requests carry `Bearer <AccessToken>` when present, with no redirects and no body logging.
- Hard-fail conditions (the previous list is retained):
  - HTTP ≠ 200 or GraphQL `errors`.
  - User id ≤ 0, user name ≠ configured username (case-insensitive), or collection user ≠ user.
  - `hasNextChunk` null; >128 or blank or duplicate declared custom names; the declared name set changes between chunks.
  - >512 groups or a repeated chunk (SHA-256 of lists JSON); a group with null `isCustomList`/entries; a custom group whose name is not declared.
  - An entry with id ≤ 0, wrong `userId`, `media.type ≠ ANIME`, unknown list or media status, or negative progress/updatedAt.
  - The same media id seen with different entry data across groups ("changed while fetched").
  - ≥11,000 unique entries (truncation boundary); an empty chunk while `hasNextChunk` is true; more than 22 chunks; cumulative payload over **8 MiB**.
- Membership: `customLists` map = every defined name → whether the media appeared in that custom group. When `mediaListOptions.customLists` is null, names are learned from custom groups.

`AniListParser.ParseEntries` (per entry, in order):
1. Normalize membership: use the `customLists` map, else the `customLists(asArray:true)` alias, else an optional resolver.
2. Fail if exclusions or inclusions are configured and membership is unknown.
3. If the entry is on any **excluded** custom list, add it to `ExcludedMediaIds` and skip.
4. Skip if already excluded, the list status is not enabled (`CURRENT/PLANNING/COMPLETED/DROPPED/PAUSED/REPEATING` → the six flags), the media status is not enabled (`FINISHED/RELEASING/NOT_YET_RELEASED/CANCELLED/HIATUS` → the five flags), or included names are configured and the entry is in none of them.
5. Emit `ImportListItemInfo{AniListId=media.id, MalId=idMal??0, Year=startDate.year, Title=userPreferred||romaji}`.

Name matching: exact ordinal first, then a **unique** case-insensitive match. An ambiguous case-insensitive match is an error for excluded names and false for included names. Exclusion veto is applied again over the final list, and dedup is by `AniListId`. Set rule: `(enabled list status ∧ enabled media status ∧ (no included names ∨ ∈ any included)) − (∈ any excluded)`.

Upstream comparison: upstream sends `status_in` in a paged `Page(mediaList)` query, maps only `AniListId`+`Title`, and filters only media status locally. It has no custom lists.

**Dead or legacy code in production:**
- `AniListCustomListMembershipSnapshot.Load` and `AniListRequestGenerator.GetMembershipRequest` are referenced only by `AniListMembershipSnapshotFixture`. Its nested `MembershipUser` type is reused.
- The paged `QueryList` path (`GetListItems`, `AniListParser.ParseResponse`) is unreachable from `Fetch`/`Test`, which are overridden.

**Other additions:**
- `RequestAction("getCustomLists")` posts `CustomListNamesQuery` (`User(name){mediaListOptions{animeList{customLists}}}`) to return name options. Bearer is sent if present. It neither saves nor starts a sync.
- `TestConnection` uses a fresh snapshot scope and warns if zero entries match.

**Shared AniList request coordinator** (`IAniListRequestService`, used by this import list, `MetadataSource/AniList/AniListProxy` and `AniListProgressWriter`):
- One process-wide lock and state.
- Minimum spacing **2.1 s** through `IRateLimitService` key `graphql.anilist.co`.
- `X-RateLimit-Limit` widens the interval to `61 s / limit` (never faster than 2.1 s). `X-RateLimit-Remaining`/`-Reset` track a budget window (default 1 min when Reset is absent) and reserve one unit before each dispatch. Remaining = 0 starts a cooldown.
- On 429: honour `Retry-After` (seconds or date) or `X-RateLimit-Reset`, default **60 s**. Up to **3** attempts within a **2 min** deadline. `ExecuteOptional` makes 1 attempt with a 5 s deadline and a 2 s lock wait.
- Failures surface as a sanitized `TooManyRequestsException` carrying a `Retry-After` header. The import-list catch then sets `RecordFailure(id, RetryAfter or 1h)`.
- Response bodies are never logged.

UI: AniList-specific field groups in `frontend/src/Settings/ImportLists/ImportLists/AniListFieldGroups.tsx` provide a name-loading button (`/importlist/action/getCustomLists`), and `ProviderFieldFormGroup.tsx` special-cases the two tag fields. Docs: `docs/library-tools.md#anilist-lists` and FORK.md "Import lists". These match the code, including defaults (Watching+Planning, Finished+Releasing) and the exclusion veto.

Status: **Operational**, but live private-account sync is UNVERIFIED (FORK.md itself says it was not exercised). Snapshot limits risk: the 8 MiB raw cap counts every occurrence, and entries repeat in each custom group with full media and relations. Very large accounts with many custom lists could fail closed and keep the old list (UNVERIFIED magnitude; no measurement in repo). Tests: `AniListAccountSnapshotFixture` (~40), `AniListFilteringFixture` (~46), `AniListCustomListMembershipFixture` (~25), `AniListSettingsFixture` (~27), `AniListParserFixture` (~18), `AniListImportFixture` (~18), `AniListCustomListOptionsFixture` (~15), `AniListMembershipSnapshotFixture` (~17, dead path), `AniListRequestGeneratorFixture` (2), and `NzbDrone.Core.Test/AniList/AniListRequestServiceFixture` (~32). Counts are `[Test]` + `[TestCase]` lines.

### 13.6 MyAnimeList import list (official API v2, Client ID, direct PKCE OAuth)

Files: `ImportLists/MyAnimeList/MyAnimeListImport.cs`, `MyAnimeListSettings.cs`, `MyAnimeListParser.cs`, `MyAnimeListRequestGenerator.cs`, `MyAnimeListResponses.cs` (modified); `MyAnimeListRequestService.cs` (new). `MyAnimeListStatus` is unchanged: `All=0, Watching=1, Completed=2, OnHold=3, Dropped=4, PlanToWatch=5`.

**Upstream:** OAuth runs entirely through `services.sonarr.tv` (`ISonarrCloudRequestBuilder`: `oauth/myanimelist/authorize|auth|renew`). Settings are a single `ListStatus` plus `MinimumScore`. Because Tanukarr blocks `services.sonarr.tv`, the fork had to replace this flow.

**Settings:**
- `Username` (default `"@me"`, regex `^(@me|[A-Za-z0-9_-]+)$`).
- `ClientId`.
- `ClientSecret` (advanced, password).
- `ListStatuses` (TagSelect; default `[All]`). The legacy `ListStatus` is hidden and migrated in code on deserialize: `_listStatuses ??= [ListStatus]; ListStatus = 0`, with no DB migration.
- `IncludedTags` / `ExcludedTags` (exact, ordinal).
- `ImportFinished` / `ImportAiring` / `ImportUnreleased` (all true).
- `MinimumScore` 0..10.
- `SyncWatchedProgress` (false).
- `BaseUrl` must be https with no userinfo, query or fragment.
- `AccessToken` is required only when `ClientId` is empty or `Username == "@me"`.

**OAuth (direct, PKCE "plain"):**
- `startOAuth` requires `ClientId` and a callback that is absolute http(s), has no userinfo, query or fragment, and has a path ending `/oauth.html`.
- It generates `state` (32 random bytes, hex) and `verifier` (48 random bytes, hex), caches `{ClientId, RedirectUri, Verifier}` for 10 min, and returns `https://myanimelist.net/v1/oauth2/authorize?response_type=code&client_id&redirect_uri&state&code_challenge=<verifier>&code_challenge_method=plain`.
- `getOAuthToken` consumes the state (single use, same ClientId) and POSTs the form `grant_type=authorization_code, code, redirect_uri, code_verifier, client_id[, client_secret]` to `https://myanimelist.net/v1/oauth2/token`.
- The response must have `token_type=Bearer`, `expires_in>0` and a refresh token.
- Renewal happens when `Expires < now+5min`: `grant_type=refresh_token`. Without a ClientId or refresh token the error says "legacy Sonarr-hosted OAuth is unavailable".
- A renewed token is cached in-process for 1 day, keyed by SHA-256 of `{ClientId, ClientSecret, AccessToken, RefreshToken}`. An unsaved Test's renewal can therefore be persisted by the next saved Fetch.

**Fetch:**
- `GET {BaseUrl}/users/{username|@me}/animelist?fields=status,list_status{status,score,tags,is_rewatching}&limit=1000&offset=N`.
- Header is `Authorization: Bearer` if a token exists, else `X-MAL-CLIENT-ID`.
- Pagination must follow `paging.next` on the same scheme, host, port and path, with exactly one `offset` equal to previous offset + count. At most **100 pages**.
- Each page must have `data` with ≤1000 items, each with a positive id, a title, score 0..10 and a known list status.
- Local filters, in order:
  1. If tag filters are configured, tags must be present.
  2. An excluded tag vetoes the item.
  3. `is_rewatching` counts as watching.
  4. The status must be selected, or All.
  5. Release status maps `finished_airing/currently_airing/not_yet_aired` to the three flags. Unknown statuses are accepted only if all three flags are on.
  6. Score ≥ MinimumScore.
  7. If included tags are configured, the item needs one of them.
- Items are `{Title, MalId}`, deduplicated by MalId.

**Request service:** per-host lock; 2 s spacing; up to 3 attempts on 429 honouring Retry-After (default 5 s) within a 30 s deadline; no redirects; no body logging. Any non-200 is an `ImportListException`. A 429 sets `RecordFailure(RetryAfter or 5 min)`.

UI: generic provider fields plus `ProviderListFieldGroups.tsx` grouping. Docs: `docs/library-tools.md#myanimelist-account-lists-and-jikan-metadata` matches the code. The callback must be registered with MAL including the URL base. Jikan is not used for account lists.

Status: **Operational** in code; live MAL account access is UNVERIFIED (no live evidence in repo). Tests: `MyAnimeListFixture` (~50), `MyAnimeListRefreshHandoffFixture` (5), `MyAnimeListRequestServiceFixture` (3).

### 13.7 Simkl (V1 retained; optional AUTH V2 path with custom lists)

Files: `Simkl/User/SimklUserImport.cs`, `SimklUserSettings.cs`, `SimklSettingsBase.cs` (tokens marked ApiKey), `SimklAPI.cs` (adds `Status`) (modified); `SimklV2AuthService.cs`, `SimklV2RequestService.cs`, `SimklV2WatchlistService.cs`, `SimklCustomListReader.cs` (new).

**Switch:** if `ClientIdV2` is empty **and** no custom-list IDs are set, `SimklUserImport` runs the **unchanged upstream V1 path** (OAuth via `auth.servarr.com`). Otherwise it runs V2.

**New settings:**
- `ClientIdV2` (3).
- `ClientSecretV2` (4, advanced, password).
- `IncludedCustomListIds` / `ExcludedCustomListIds` (5/6; Tag fields with `SelectOptionsProviderAction="getCustomLists"`; positive integers).
- `SyncWatchedProgress` (7).
- Hidden `ScopeV2`.

Validation: custom-list IDs require `ClientIdV2`. When V2 is used, BaseUrl must be https with no userinfo, query or fragment.

**V2 auth (PKCE S256):**
- `startOAuth` requires a callback that is https, or http on loopback, ending `/oauth.html`, ≤1024 bytes. State is 32 random bytes and the verifier 48 random bytes, both base64url. The session is cached 10 min.
- Authorize URL: `https://simkl.com/oauth2/authorize?response_type=code&client_id&redirect_uri&state&scope=media:read[+media:write if SyncWatchedProgress]&code_challenge&code_challenge_method=S256`.
- Token exchange: `POST https://api.simkl.com/oauth2/token`. Authorization-code exchange is attempted once. The response must be Bearer, with both tokens, expiry between 0 and 1 year, a `media:read` scope, and `media:write` if progress sync is enabled.
- Then `GET users/settings` and require `account.id > 0`, stored as `AuthUser`.
- Refresh happens when `Expires < now+5min`. The renewed grant is cached 10 min, keyed by a fingerprint.
- Every V2 request adds `client_id`, `app-name=Tanukarr`, `app-version` and `User-Agent: Tanukarr/<ver>`.
- The request service is the same as MAL's: per-host lock, 2 s spacing, 3 attempts on 429, 30 s deadline.

**V2 inventory (incremental, persisted on disk):**
- Read `sync/activities` and validate the `all` stamp plus the per-bucket (`anime` or `tv_shows`) stamps `watching/plantowatch/hold/completed/dropped/removed_from_list`, which must be UTC strings or null.
- If anything changed, read `sync/all-items/{anime|shows}` with `date_from=<previous all>` when a baseline exists.
- If `removed_from_list` changed, read `?extended=simkl_ids_only` and drop rows that are missing.
- An older cursor than the baseline is an error.
- The baseline is stored at `{AppData}/cache/simkl-v2/{sha256(client,accountId,media,api)}.json`, mode 0600, written atomically via a temp file and rename. Writes use optimistic revision checks, are committed only on a successful Fetch (not Test), and the file is capped at 8 MiB.

**Custom lists:**
- `lists/{id}?limit=500&page=N`, max 20 pages and ≤10,000 items. `id`, `media_type` (`anime`/`tv`), name, owner and `counts.items` must match the pagination totals, and members must be unique positive `simkl_id`s.
- Options come from `lists/user/{AuthUser}?limit=500&page=N`, filtered by media type.

**Selection:**
- Start from Watchlist rows whose `status` equals the `ListType` enum name lowercased.
- For anime, keep only `anime_type` TV/ONA/OVA/Special.
- Intersect with included lists, if any, and subtract excluded lists.
- Emit `{Title, TvdbId, TmdbId, MalId, ImdbId}` for rows with at least one ID; Simkl provides no AniList ID.
- Dedupe on `{TvdbId, TmdbId, MalId, ImdbId}`.

The Simkl AUTH V2, `media:read/media:write` scopes and custom-list endpoints are a beta external contract. Correctness is UNVERIFIED against the live API; docs cite `api.simkl.org/guides/custom-lists` and state that no premium-account run was done.

Status: **Operational** in code; external contract UNVERIFIED. Tests: `SimklV2AuthFixture` (~33), `SimklCustomListFixture` (~27), `SimklV2WatchlistFixture` (~18), `SimklUserV2Fixture` (5), `SimklV2RequestServiceFixture` (3).

### 13.8 TMDB List / Trakt List / MDBList include-exclude sets, Plex completeness, and import-scoped snapshots

**Set algebra** (`ImportLists/ImportListSetOperations.cs`, new, internal):
- `Sources(modern, legacy...)` concatenates, trims and de-duplicates the selections (ordinal).
- `Compose(included, excluded, read)`:
  - Throws if there are no included sources ("Select at least one included list").
  - Reads every included source with `applyFilters=true` and every excluded source with `applyFilters=false`, so exclusions apply before optional content filters.
  - Computes identity keys per item: `tv:tvdb:N` (N>0), `tv:tmdb:N` (N>0), and `tv:imdb:ttNNN` (regex `\Att[0-9]+\z`). An item with **no** key fails the whole fetch.
  - Unions keys with **union-find**, so an exclusion sharing any alias with an included item removes it.
  - Returns included items whose root is not excluded, first occurrence only.
- `Fetch(...)` wraps this:
  - Success: assign `ImportListId`/`ImportList`, apply an optional `Take(limit)` (Trakt only), and `RecordSuccess`.
  - 429: `RecordFailure(RetryAfter or 1h)`.
  - DNS or connect failure: `RecordConnectionFailure`.
  - Anything else: `RecordFailure`.
  - All failures log a generic sanitized warning and return `AnyFailure=true` with an empty list (the previous saved list is kept).
- `MaxPages = 1000`.
- `TestResult`: failure → error; zero results → warning.

**TMDB List** (`Tmdb/List/TmdbListImport.cs`, `TmdbListSettings.cs`):
- `IncludedListIds` / `ExcludedListIds` are Tag fields holding positive integer strings. Legacy `ListId`/`AccountListId` are hidden and normalized into `IncludedListIds` on load or validate.
- Read: `GET {BaseUrl}/4/list/{id}?page=N` with `Authorization: Bearer <AuthToken>`. Validation: `page` equals the requested page, `total_pages` is stable and ≤1000, `total_results` is stable, nonterminal pages are non-empty, and the final raw count equals `total_results`. Media types are restricted to tv/movie/person, and tv rows need a positive id.
- `getAccountLists` → `GET 4/account/{AccountId}/lists?page=N`, returning `{name, value=id, hint}`.
- TMDB Discover, Person and Account remain classic paged importers (KakarottoCake versions). Account uses `4/account/{id}/tv/{favorites|rated|recommendations|watchlist}`, which needs a **user** v4 token the user must obtain outside Tanukarr; there is no in-app flow.

**Trakt List** (`Trakt/List/*`):
- `IncludedLists` / `ExcludedLists` hold slugs or IDs matching `\A[A-Za-z0-9][A-Za-z0-9_-]*\z`. Legacy `Listname` is hidden and converted with `ToUrlSlug()` into `IncludedLists`.
- Read: `GET /users/{user}/lists/{slug}/items/show,season,episode?{filters if included}&limit=250&page=N`. Headers `trakt-api-version: 2`, `trakt-api-key`, Bearer.
- Requires `X-Pagination-Page`, `-Page-Count`, `-Item-Count` and `-Limit` to be consistent, with the final raw count equal to the item count. movie/person rows are skipped; others yield `{Title, Year, TvdbId, TmdbId, ImdbId}`.
- The composed result is truncated by `Settings.Limit` (default 100, inherited).
- `getLists` → `/users/{username}/lists` (pagination headers optional on page 1).

**MDBList** (new provider, `MDBList/*`; `ListType=Other`, 6 h refresh):
- Settings: `ApiKey` (required, ApiKey privacy), `IncludedListUrls` / `ExcludedListUrls` (≥1 included), hidden legacy `ListUrl`.
- URL validation: https on the default port, host `mdblist.com` or `www.mdblist.com`, no userinfo, query or fragment, path `^/lists/([a-zA-Z0-9_-]+)/([a-zA-Z0-9_-]+)/?$`.
- Read: `GET https://api.mdblist.com/lists/{u}/{l}/items/show?apikey=…&limit=250` with `cursor=` or `offset=`. The host is hard-coded, so the `BaseUrl` setting is ignored.
- Termination is validated through `pagination.next_cursor`/`has_more`, `X-Has-More`, `X-Total-Items` and repeated-cursor detection. Legacy offset paging advances by the raw count until an empty page arrives.
- Parser: rows whose `mediatype`/`type` is not show/tv are skipped. `tmdb` is taken from `ids.tmdb`, then `tmdb_id`, `tmdbid`, then `id`. The `id` fallback is assumed to be TMDB (UNVERIFIED API semantics). `tvdb` and `imdb` are read similarly.
- There is no `getLists` action; URLs are entered manually.

**Plex Watchlist** (`Plex/PlexImport.cs`, `PlexParser.cs`, `PlexListRequestGenerator.cs`):
- Upstream stopped after `MaxPages = 10` pages of 100. Tanukarr reads the complete watchlist with `MaxPages = 1000`, validating `MediaContainer.offset` against the requested offset, `size` against the row count, and `totalSize` against `X-Plex-Container-Total-Size`/`X-Plex-Container-Start`. Total must be stable, and a short or zero page before the total is an error.
- Only `type=="show"` rows are mapped. Each must have a title and non-empty guids, and a positive tvdb or tmdb or a valid `tt\d+` IMDb; otherwise the whole read fails.
- 429 → `TooManyRequestsException`.
- Named Plex lists are **not** implemented (docs agree).

**Import-scoped snapshots** (`ImportLists/ImportListSnapshotScope.cs`, new):
- An `AsyncLocal` ambient scope. `BeginIfNeeded()` is used by `SyncAll`/`SyncList`/`Fetch`/`FetchSingleList` and provider fetches; `Begin()` (always fresh) is used by every Test.
- `GetOrLoad<T>(sha256Key, loader, measure)` memoizes with `Lazy` (ExecutionAndPublication).
- Retention limits: **64 snapshots**, **8 MiB per snapshot**, **32 MiB total**. An over-limit result is returned but not retained.
- Keys must be 64-hex SHA-256, built by `Fingerprint(parts…)` (length-prefixed SHA-256).
- `AddPayloadBytes` **throws** once a single source read exceeds 8 MiB of raw response, so the list fails and the previous one is kept. This is a hard size limit per selected source.
- `Dispose` clears everything.

Snapshot keys per provider:

| Provider | Key parts |
| --- | --- |
| AniList | `("AniList","complete-anime-collection-v1",BaseUrl,USERNAME,AccessToken)` |
| MAL | `("MyAnimeListAccount-v1", first-page URL, ClientId, AccessToken)` |
| TMDB | `("TmdbSelectedList-v1",BaseUrl,AuthToken,AccountId,listId)` |
| Trakt | `("TraktSelectedList-v1", full URL incl. filters, AccessToken, ClientId)` |
| MDBList | `("MdbSelectedList-v1", list path, ApiKey)` |
| Plex | `("PlexWatchlist-v1", url, PlexClientIdentifier, AccessToken)` |
| Simkl inventory | `("SimklInventory-v1", scope, AccessToken, ScopeV2)` |
| Simkl membership | `("SimklCustomMembership-v1", BaseUrl, ClientIdV2, AuthUser, AccessToken, ScopeV2, ShowType, listId)` |

AniList additionally publishes `AniListPublicRelationSnapshot` into the scope, which `MetadataSource/AniList/AniListProxy.cs:110` reads to avoid relation re-fetches during grouping. Docs (`docs/import-list-snapshots.md`) match these numbers and semantics.

UI: `ProviderListFieldGroups.tsx` groups the source and filter fields and offers a "load lists" button mapped to `TmdbListImport→getAccountLists`, `TraktListImport→getLists`, `SimklUserImport→getCustomLists` (POST `/api/v5/importlist/action/{name}`).

Status: **Operational**. Tests: `ProviderListSetFixture` (~44), `ProviderListSettingsFixture` (~30), `ImportListSnapshotScopeFixture` (10), `Plex/PlexImportPaginationFixture` (~35), `MDBListParserFixture` (2), `TmdbParserFixture` (3), and `Trakt/*` (validation, smart-list and user generators).

### 13.9 Originating-account watched-progress queue (migration 249) and per-provider writers

Files: `ImportLists/Progress/ImportListProgressService.cs`, `ImportListProgressWriter.cs` (interface, `ImportListProgressTarget`, `ImportListProgressGuard`), `ImportListProgressState.cs` and `…Repository.cs`, `AniListProgressWriter.cs`, `MyAnimeListProgressWriter.cs`, `TraktProgressWriter.cs`, `SimklProgressWriter.cs` (all new). Table registered in `Datastore/TableMapping.cs` as `ImportListProgressStates`. Writers are auto-registered and injected as `IEnumerable<IImportListProgressWriter>`.

**Schema** — migration **249** `import_list_progress_queue` creates table `ImportListProgressStates`:

| Column | Type |
| --- | --- |
| `Id` | PK |
| `ImportListId` | int |
| `SeriesId` | int |
| `TargetFingerprint` | varchar(64) |
| `PendingProgress` | int, default 0 |
| `AcknowledgedProgress` | int, default 0 |
| `AcknowledgedIdentityFingerprint` | varchar(64), default '' |
| `FailureCount` | int, default 0 |
| `NextAttemptUtc` | datetime, not null |
| `LastAttemptUtc` | datetime, null |

Index: **unique** `(ImportListId, SeriesId)`.

**Trigger and interface (producer side belongs to the Jellyfin audit).** `JellyfinWatchedService.Sync` (`Notifications/Jellyfin/JellyfinWatchedService.cs`) runs after a successful complete Jellyfin snapshot, and only when the single enabled Jellyfin connection has `SyncListProgress` set (an opt-in, default off). For each matched series it calls `IImportListProgressService.SyncProgress(seriesId, watchedEpisodeIds, canDispatch)`. `watchedEpisodeIds` are the episodes with `Watched && WatchedSource == <connection fingerprint>`. `canDispatch` re-verifies that the connection, its policy and tags are unchanged, and is re-invoked before every remote target. The list-side second opt-in is the provider setting `SyncWatchedProgress` (default false) on AniList, MyAnimeList, Trakt User, Trakt List and Simkl User.

**`SyncProgress` algorithm** (one global lock):
1. Compute `progress = ContiguousProgress(episodes, watched, multipleSeasons: series is TVDB-owned)`:
   - Take native episodes with SeasonNumber > 0, sorted. Return 0 if there are none, if any episode number ≤ 0, if there are duplicates, or if a non-TVDB series has any episode outside season 1.
   - Count watched episodes from S01E01 in strict sequence. For TVDB series the count may roll from season n to season n+1 starting again at E1. Stop at the first gap or unwatched episode. Specials and group/scene numbering are ignored.
2. Return if progress is 0, or if the watched episodes do not share exactly **one** non-empty `WatchedSource`.
3. For each list with `EnableAutomaticAdd` and each writer, find the first stored `ImportListItems` row of that list for which `writer.TryGetTarget(definition, series, item)` succeeds. This requires the list's own saved membership.
4. `Dispatch`:
   - `stateFingerprint = SHA256(target.Fingerprint, watchedSource)`.
   - Reset the row if the fingerprint changed, or if the acknowledged prefix's episode identities changed. The identity fingerprint is a SHA-256 over `id:tvdbId:season:episode:foreignId` of the first N native episodes.
   - Set `Pending = max(Pending, progress)` and **persist**.
   - Skip if `Ack ≥ Pending` or `NextAttemptUtc > now`.
   - Global budget: **10 dispatches per fixed 1-minute window** (the window restarts at the first dispatch after it expires; per service instance).
   - Re-check `canDispatch()`, the current definition (still exists, still EnableAutomaticAdd, same Implementation), and a fresh membership/target with the same fingerprint.
   - Persist `LastAttempt = now` and `NextAttempt = now + 5 min` **before** any I/O, then call `writer.Advance(definition, target{AcknowledgedProgress}, Pending)`.
   - Success (`confirmed > 0`): `Ack = max(Ack, confirmed)`, set the identity fingerprint, `FailureCount = 0`, `NextAttempt = now` if fully confirmed else `now + 1 min`.
   - Failure: `FailureCount = min(n+1, 16)`, wait = `min(360 min, 5 min × 2^(n-1))`. A 429 extends the wait to `Retry-After` (≤1 day). `ImportListProgressUnavailableException` waits **1 day**.
5. Rows are deleted when their import list is deleted (`ProviderDeletedEvent<IImportList>`) or their series is deleted.

**Writers** — what each sends to the remote account, with its guards:

**AniList** (`https://graphql.anilist.co` only; `OfficialRoot` check).
- Target: the series is AniList-owned, `ForeignId == item.AniListId`, and has no `OrderingId`.
- Requires a token with more than 1 min to expiry. There is **no refresh here**; tokens are refreshed only by list sync.
- Steps:
  1. `query{Viewer{id name} User(name:$username){id name}}` — the Viewer id must equal the User id (own account only).
  2. `MediaList(mediaId,userId,type:ANIME){id userId mediaId progress media{episodes type}}` — the entry must already exist (it never creates membership).
  3. If remote progress ≥ pending, return remote (never decrease).
  4. Refuse if progress exceeds `media.episodes`.
  5. `mutation SaveMediaListEntry(id:$entryId, progress:$progress)` — **progress only**. The response must echo id, userId, mediaId and progress ≥ requested.

**MyAnimeList** (`https://api.myanimelist.net/v2`).
- Target: MAL-owned with `ForeignId == item.MalId`, no ordering. The token must be valid.
- Steps:
  1. `GET users/@me` — the name must equal the configured Username unless that is `@me`.
  2. `GET anime/{id}?fields=num_episodes,my_list_status{num_episodes_watched}` — `my_list_status` must exist.
  3. Never decrease, and refuse if progress exceeds `num_episodes`.
  4. `PATCH anime/{id}/my_list_status` with form `num_watched_episodes=N`. The response must confirm ≥ N.

**Trakt** (`https://api.trakt.tv`; Trakt User or Trakt List settings).
- Target: TVDB-owned series with positive `TvdbId == item.TvdbId`, Official season type, no ordering.
- Steps:
  1. `GET users/settings` — `user.ids.slug` must equal the stored `AuthUser`, and the configured Username must match the slug or username.
  2. `GET search/tvdb/{id}?type=show` must return exactly one match.
  3. For episodes `Ack+1 .. Pending`, at most 20 per attempt: `search/tvdb/{episodeTvdbId}?type=episode` (must belong to the same show), then `GET sync/history/episodes/{traktEpisodeId}?limit=1`.
  4. If there is no history, `POST sync/history {"episodes":[{"ids":{"trakt":id},"watched_at":<WatchedAtUtc or now, ISO-8601>}]}`. This requires `added.episodes == 1` and an empty `not_found`. It is never blindly retried, because Trakt does not dedupe.
  5. Return the count of contiguous confirmed episodes.

**Simkl** (`https://api.simkl.com`; V2 only).
- Requires `ClientIdV2`, a valid token and the `media:write` scope.
- Target ID by owner:
  - AniList-owned: `item.AniListId`. Simkl list rows never carry an AniList ID, so this branch is **unreachable for Simkl-imported items**.
  - MAL-owned: `item.MalId`.
  - TVDB-owned: `item.TvdbId` with Official season type.
- Steps:
  1. `GET users/settings` — `account.id` must equal `AuthUser`.
  2. `POST sync/watched [{"ids":{"anilist|mal|tvdb":id},"type":"show|movie"}]` must return exactly one existing entry with a `list` and a `simkl` id.
  3. `POST sync/history?skip_auto_watching=yes {"shows":[{"ids":{"simkl":N},"seasons":[{"number":s,"episodes":[{"number":e}…]}…]}]}` for the next ≤100 episodes, adding `"use_tvdb_anime_seasons":true` for TVDB-owned series. A movie member sends `"movies":[…]` and only with progress == 1.
  4. The write is executed once; the response must be 200/201 with an empty `not_found` and an integer `added.episodes`/`added.movies`. It never sets `allow_rewatch`.

None of the writers send list status, score, rating or custom-list membership. A provider may still apply its own server-side status rules.

UI: the `SyncWatchedProgress` checkboxes render from field definitions, with `ImportListsSyncWatchedProgress*` strings in `Localization/Core/en.json`. The Jellyfin side has `NotificationsJellyfinSettingsSyncListProgress`. There is no page showing queue state; the API does not expose `ImportListProgressStates`.

Status: **Operational, opt-in, default off**. Remote behaviour against live accounts is UNVERIFIED; docs say private-account writes were not exercised. Limitations:
- AniList-via-Simkl is dead code.
- AniList/MAL require single-season native numbering; any non-special episode outside season 1 makes progress 0.
- Expired tokens park the work for 1 day until a list sync refreshes them (MAL/Simkl/AniList/Trakt token refresh happens only in their import paths).

Tests: `ImportListProgressServiceFixture` (~26), `ImportListProgressGuardFixture` (~19), `AniListProgressWriterFixture` (8), `MyAnimeListProgressWriterFixture` (6), `TraktProgressWriterFixture` (9), `SimklProgressWriterFixture` (11). There is **no migration fixture for 249** (fixtures exist for 242, 248 and 250, but none for 249).

### 13.10 List preview / selective add (`Sonarr.Api.V5/LibraryTools/ImportListPreviewController.cs`)

Routes (v5, `[V5ApiController("librarytools/lists")]`):

**`GET /api/v5/librarytools/lists/{id}`**
- Calls `importList.Fetch()` directly. This has side effects: it records list status success/failure, and AniList/MAL may refresh and persist tokens.
- Caches `{Token (GUID), Items, Partial=AnyFailure}` for **10 min** per list id.
- Returns per-item fields `key` (index), `title`, `year`, `tvdbId`, `tmdbId`, `aniListId`, `malId`, `imdbId`, `metadataSource` (from `GetSource(item)`, Automatic only), `foreignId`, `alreadyAdded` (via `Matches`), `excluded` (candidate only, not the raw item), and `supported`.
- An IMDb-only item has no source in preview (`foreignId` null, `excluded` false, `supported` true). Its IMDb metadata lookup happens only on add.

**`DELETE /api/v5/librarytools/lists/{id}/preview`** clears the cached preview.

**`POST /api/v5/librarytools/lists/{id}/add`** with body `{token, keys[1..25], rootFolderId, qualityProfileId, monitored}`.
- Refused if the preview is `Partial`, the token is stale, the root or profile is invalid, or there are more than 25 keys.
- For each key: return "Already added" (by `Matches`), else `CreateSeries` (**ignores `PreferredMetadataSource`**), else "Excluded from import lists" (candidate-only check), else `AddSeries` with `SeasonFolder = true`, `Monitor = All|None`, and no search.
- Errors per item become "Could not add…".

UI: `frontend/src/LibraryTools/ListPreview.tsx`. Status: **Operational**, with the preference and exclusion caveats noted. No dedicated test fixture was found for this controller (`ImportedDownloadPreviewFixture` is a different feature).

### 13.11 API and UI surface summary (import lists only)

- `ImportListResource` (v3 `/api/v3/importlist`, v5 `/api/v5/importlist`): adds `preferredMetadataSource` (`automatic|tvdb|tmdb|aniList|myAnimeList`, nullable on write) alongside upstream's `tagExisting` (`none|add|sync`).
- `ImportListExclusionResource` (v3/v5 `/importlistexclusion`): adds `metadataSource` (`MetadataSourceType`) and `foreignId`.
- Provider actions via `POST /api/{v3|v5}/importlist/action/{name}`:

| Provider | Actions |
| --- | --- |
| AniList | `getCustomLists`; `startOAuth`/`getOAuthToken` (upstream, `auth.servarr.com`) |
| MyAnimeList | `startOAuth`/`getOAuthToken` (direct PKCE) |
| TMDB List | `getAccountLists` |
| Trakt List | `getLists` |
| Simkl User | `getCustomLists`; `startOAuth`/`getOAuthToken` (V2 when `ClientIdV2` is set, else upstream V1) |

- New provider schema fields, all rendered generically from `FieldDefinition` and grouped by `ProviderListFieldGroups.tsx`/`AniListFieldGroups.tsx`:

| Provider | New fields |
| --- | --- |
| AniList | `includedCustomLists`, `excludedCustomLists`, `syncWatchedProgress`, hidden `customList` |
| MAL | `username`, `clientId`, `clientSecret`, `listStatuses`, `includedTags`, `excludedTags`, `importFinished`, `importAiring`, `importUnreleased`, `syncWatchedProgress` |
| Simkl | `clientIdV2`, `clientSecretV2`, `includedCustomListIds`, `excludedCustomListIds`, `syncWatchedProgress`, hidden `scopeV2` |
| TMDB (all) | visible `authToken`, advanced `accountId` |
| TMDB List | `includedListIds`, `excludedListIds` |
| Trakt List | `includedLists`, `excludedLists`, `syncWatchedProgress` |
| Trakt User | `syncWatchedProgress` |
| MDBList (new) | `apiKey`, `includedListUrls`, `excludedListUrls` |

- `/api/v5/librarytools/lists/*` routes as in 13.10.

### 13.12 Docs vs code

Docs that match code: `docs/import-list-snapshots.md` (limits 64 / 8 MiB / 32 MiB, AniList 11,000/500, Automatic order TVDB→AniList→TMDB→MAL, preference needs a positive ID), `docs/library-tools.md` provider matrix (set rule, exclusions read before filters, alias resolution, Plex 1,000-page bound, MAL PKCE), and FORK.md "Import lists" (AniList exclusion veto, statuses, legacy `CustomList` transfer).

Overstatements and omissions:

1. **"Migration 242 preserves provider identities in import-list exclusions"** (FORK.md). It adds columns only. Native-series exclusions created before 242 hold negative placeholder TVDB IDs that can no longer match anything. The UI cannot view or create native exclusions.
2. **"Metadata provider for new series … applies to future additions from that import list"** (`docs/import-list-snapshots.md`, 1.0.5 notes). This is true for sync. The Library Tools preview "add selected" path ignores it, and also ignores the list's series type, season folder and tags.
3. **TMDB and other TMDB-only items become TMDB-owned under Automatic.** Docs present Automatic as "the existing selection order" but do not warn that this differs from upstream, which maps TMDB/IMDb/AniList/MAL items to TVDB. This is a material behaviour change for TV users.
4. **Broker independence is partial.** FORK.md/library-tools stress that MAL avoids "Sonarr's cloud broker". AniList, Trakt and Simkl V1 still use Servarr's `auth.servarr.com` OAuth broker, and AniList saving requires it.
5. **Dead code shipped:** the AniList membership-snapshot recovery and paged `QueryList` path are still compiled and tested but unused in production. FORK.md's "older paged-list plus membership recovery route belongs to the historical release reports" is consistent, but the code was not removed.
6. **External contracts unverified:** Simkl AUTH V2/custom lists (beta, PRO/VIP), MAL/AniList private-account sync and progress writes. The repo's own reports say no live account runs were done. Unit tests use canned JSON.
7. The progress queue has no migration test for 249 and no API/UI visibility of pending or failed state. Failures are only logged as Warn.

### 13.13 Status table

| Sub-feature | Status | Evidence |
| --- | --- | --- |
| Native identity sync (no TVDB remap; `ImportListIdentity`) | Operational | `ImportListSyncService.ProcessListItems` → `ImportListIdentity.CreateSeries/Matches/IsExcluded`; `ImportListSyncServiceFixture`, `ImportListIdentityFixture` |
| Completed-fetch resume (saved membership for not-due lists) | Operational (process-local) | `FetchAndParseImportListService.Resume/RememberCompletedListFetch`; ~15 resume tests |
| Metadata source preference (mig. 250) | Operational (sync) / Partial (preview add ignores it) | `ImportListDefinition.PreferredMetadataSource`, `GetSource`; `ImportListPreviewController` calls `GetSource(item)` without preference |
| Exclusion identity (mig. 242) | Operational for new exclusions; legacy native exclusions inert; no UI | `ImportListExclusionService.AddExclusions`; migration adds columns only |
| Tag-existing type (mig. 241 = upstream 236) | Operational (upstream feature, renumbered) | identical migration body; `TagExistingType` same enum |
| AniList custom include/exclude + list/media status filters | Operational (live private account UNVERIFIED) | `AniListAccountSnapshot.Load` + `AniListParser.ParseEntries`; 200+ AniList test cases |
| AniList OAuth | Operational (inherited, via `auth.servarr.com`) | `AniListImportBase` unchanged |
| AniList shared rate limiter | Operational | `AniListRequestService` (2.1 s, 3 attempts, 2 min, Retry-After/X-RateLimit-*); used by import, metadata, progress |
| MyAnimeList official API + Client ID + direct PKCE | Operational (live UNVERIFIED) | `MyAnimeListImport.OAuthAction/ReadCompleteAccount`; `MyAnimeListFixture` |
| Simkl V2 (auth, incremental inventory, custom lists) | Operational in code; external beta API UNVERIFIED | `SimklV2AuthService`, `SimklV2WatchlistService`, `SimklCustomListReader`; V1 path unchanged |
| TMDB List include/exclude (user Read Access Token) | Operational | `TmdbListImport.FetchComposed` + `ImportListSetOperations.Compose`; no OAuth (upstream's `services.sonarr.tv` flow absent) |
| Trakt List include/exclude | Operational | `TraktListImport.FetchComposed`; pagination-header validation |
| MDBList (new provider) include/exclude | Operational (API field semantics partly UNVERIFIED) | `MDBListImport`, `MDBListParser`; only 2 parser tests |
| Plex complete watchlist | Operational | `PlexImport.ReadWatchlistSnapshot`, `PlexParser.ParsePage`; `PlexImportPaginationFixture` |
| Import-scoped snapshots | Operational | `ImportListSnapshotScope` used by all composed providers, AniList, MAL, Simkl, Plex |
| Progress queue (mig. 249) + writers | Operational, opt-in (two flags), live writes UNVERIFIED; Simkl→AniList branch dead | `ImportListProgressService.Dispatch`; called only from `JellyfinWatchedService.Sync` |
| List preview/selective add (v5 LibraryTools) | Operational (with preference/exclusion caveats) | `ImportListPreviewController`; UI `LibraryTools/ListPreview.tsx` |
| Trakt Smart List | Operational, but settings key `SmartList` ≠ upstream `SmartListId` | KakarottoCake implementation |


## 14. Library tools

Scope note: everything in this section is fork-only. Upstream Sonarr (develop `c018578`) has no `LibraryTools` namespace, no `/api/v5/librarytools` routes and no companion keys. Paths below are relative to `ref/tanukarr/`.

### 14.0 Storage, wiring and UI (common to all tools)

- **Settings storage (no migration):** three JSON blobs in the existing `Config` key/value table, accessed through `src/NzbDrone.Core/Configuration/ConfigService.cs` (keys are lower-cased on write by `SetValue`):
  - `ProOptions` (default `"{}"`): serialized `ProOptions` (System.Text.Json, Web defaults, i.e. camelCase).
  - `ProAccessKeys` (default `"[]"`): list of companion keys (`ProAccessKey`).
  - `ProSearchHistory` (default `"[]"`): last 100 search summaries.
- **Only table added:** `RetainedVersions` (migration 239, see 14.6b).
- **Model** (`src/NzbDrone.Core/LibraryTools/ProOptions.cs`):
  - Global flags: `AutomationPaused`, `HardlinkOnly`, `MatchExternalIds`, `PreferAnimeSeasonPacks`. All are `bool` and default to `false`.
  - `Series: Dictionary<int seriesId, ProSeriesOptions>`, where each entry has:
    - `KeepVersions` (default false);
    - `AutomaticRenaming`: `"default"`, `"manual"` or `"automatic"` (default `"default"`);
    - `SeasonTitles`: `Dictionary<int seasonNumber, string>`.
- **Service:** `ProOptionsService` re-parses the JSON only when the raw string changes. `Read()`/`ForSeries()` return detached copies, so a failed save cannot change live options.
- **API:** v5 only. `[V5ApiController("librarytools")]` routes to `/api/v5/librarytools`. There are no v3 equivalents. Normal authentication applies: the fallback policy uses the "API" key scheme or the forms session's API key, and companion keys are excluded by their allowlist (14.6).
- **UI:**
  - **System → Library tools** (`/system/library`). The sidebar entry is in `frontend/src/Components/Page/Sidebar/PageSidebar.tsx:163`. The page is `frontend/src/LibraryTools/LibraryTools.tsx` plus `SearchActivity.tsx`, `ListPreview.tsx` and `RecycleBrowser.tsx`.
  - **Series details:** `frontend/src/Series/Details/SeriesLibraryTools.tsx` (per-show options and retained versions) and `SeriesCompression.tsx`, both rendered at `SeriesDetails.tsx:875-876`.
  - **Global pause banner:** `frontend/src/LibraryTools/AutomationBanner.tsx`, mounted in `Components/Page/PageContent.tsx:31`. It polls `GET /librarytools` every 60 s.
  - The frontend talks only to `/api/v5` (`frontend/src/Utilities/Fetch/fetchJson.ts:37`).

#### Complete route list (all under `/api/v5`)

| Method + path | Body / result | Guards | File |
| --- | --- | --- | --- |
| `GET /librarytools` | `ProOptions` (global flags + series map) | normal auth | `Sonarr.Api.V5/LibraryTools/LibraryToolsController.cs:53` |
| `PUT /librarytools` | `ProOptions`; **only the 4 global flags are applied**, and the series map in the body is ignored | static lock `SettingsGate` | same, `:56` |
| `GET /librarytools/series/{id}` | `ProSeriesOptions` (404 if series missing) | | `:71` |
| `PUT /librarytools/series/{id}` | `{keepVersions, automaticRenaming, seasonTitles}`. Returns 400 unless `automaticRenaming ∈ {default, manual, automatic}`, `seasonTitles` is non-null, each title is ≤200 chars, and each key is an existing season of the series | `SettingsGate` | `:78` |
| `GET /librarytools/searches` | ≤100 `SearchActivity` rows | | `:97` |
| `GET /librarytools/storage` | per-root estimate (14.8) | | `:100` |
| `POST /librarytools/audit` | `{checkedAt, seriesChecked, filesChecked, issues[]}`; runs synchronously in the request | request cancellation token | `:133` |
| `GET /librarytools/keys` | `[{id, name, permission, created, expires}]`; hash never returned | | `:176` |
| `POST /librarytools/keys` | `{name, permission, days}` → `{key, token}` | see 14.6 | `:179` |
| `DELETE /librarytools/keys/{id}` | 204 (also 204 when the id is unknown) | | `:209` |
| `GET /librarytools/recycle` | `{configured, path, truncated, files:[{token, name, size, recycledAt}]}` | | `LibraryArchiveController.cs:35` |
| `POST /librarytools/recycle/restore` | `{token, seriesId, seasonNumber}` → 202 `{commandId}`; 400 or 409 on refusal | `LibraryOperationGate` | `:62` |
| `GET /librarytools/series/{id}/versions` | `[{id, created, path, available, metadata: EpisodeFile, episodeIds}]` | | `:97` |
| `POST /librarytools/series/{id}/versions/{versionId}/activate` | 202 `{commandId}` or 409 | `LibraryOperationGate` | `:110` |
| `GET /librarytools/lists/{id}` | preview `{token, partial, items[]}` | | `ImportListPreviewController.cs:46` |
| `DELETE /librarytools/lists/{id}/preview` | 204 | | `:82` |
| `POST /librarytools/lists/{id}/add` | `{token, keys[1..25], rootFolderId, qualityProfileId, monitored}` → per-item results | | `:89` |
| `GET /series/compression/capabilities` | proxied to the worker | | `Sonarr.Api.V5/Series/SeriesCompressionController.cs:29` |
| `GET /series/{id}/compression` | proxied | | `:35` |
| `POST /series/{id}/compression` | `{mode, minSizeMb}` proxied | `LibraryOperationGate` | `:42` |
| `DELETE /series/{id}/compression` | proxied | | `:62` |

`LibraryOperationGate` (`Sonarr.Api.V5/LibraryTools/LibraryOperationGate.cs`) is a process-wide `SemaphoreSlim(1,1)`.
- It serializes only the enqueue-time conflict checks of restore, activate and compression-start. The file work itself runs as queued commands.

### 14.1 Automation pause

Implementation:
- `src/NzbDrone.Core/Messaging/Commands/CommandExecutor.cs` (diff vs upstream: lines ~88-93).
- `src/NzbDrone.Core/Download/DownloadService.cs` (start of `DownloadReport`).

The gate applies when `ProOptions.AutomationPaused == true` (default false). It has two parts.
1. **Command gate.**
   - It applies when `command.Trigger != CommandTrigger.Manual` and either `command.Name` ends with `"Search"` (ordinal) or the name is `"RssSync"` or `"ImportListSync"`. `Name` is the class name minus `Command`.
   - The command is then marked **Completed** (not failed) with the message "Automation paused; manual actions remain available.", and its handler never runs.
   - Commands this suppresses:
     - scheduled `RssSync` and `ImportListSync`;
     - every internally queued search: `EpisodeSearch` from `EpisodeRefreshedService` (newly aired or moved episodes), `EpisodeSearch`/`SeasonSearch` from `Download/RedownloadFailedDownloadService.cs:56-79` (automatic re-download after a failed download), `CompleteSeriesSearch`, and others;
     - `SeriesSearch`, `MissingEpisodeSearch` and `CutoffUnmetEpisodeSearch` queued by `Tv/SeriesScannedHandler.cs:69-80` after Add Series with search options. These are pushed with the default `Unspecified` trigger (`CommandQueueManager.Push` overwrites `Trigger`), so **user-initiated "add and search" is also suppressed while paused**. The docs do not mention this.
   - Commands posted through `/api/v3|v5/command` get `Trigger = Manual` and still run.
2. **Grab gate.**
   - `DownloadReport` throws `ReleaseDownloadException("Automatic downloads are paused in Library tools.")` when `remoteEpisode.ReleaseSource` is not `InteractiveSearch` or `UserInvokedSearch`.
   - `ProcessDownloadDecisions.ProcessDecisionInternal` maps that exception to `Skipped`, with a warning log.
   - This blocks grabs from a **manually** triggered RSS sync, from release push (`ReleasePush`, e.g. Prowlarr or autobrr push), from pending or delayed releases, and from automatic searches.
   - Pending releases are normally re-evaluated by RSS sync, so they simply wait.

Not paused: completed-download imports, manual import, refresh/rescan, Jellyfin, import-list progress, the compression worker, and backups. This matches the docs' "manual actions and current downloads continue".

- UI: checkbox "Pause automation" plus the global banner with a **Resume automation** button.
- Status: **Operational.**
- Tests: no unit test covers the CommandExecutor or DownloadService pause gates, and none of `docker/tests` exercises pause. This is **UNVERIFIED by tests**, although the code path is short and wired: `IProOptionsService` is constructor-injected into `CommandExecutor`.

### 14.2 Hardlink-only imports

Implementation:
- `src/NzbDrone.Core/MediaFiles/EpisodeFileMovingService.cs`: `CopyEpisodeFile`, and a new `VerifyHardlink`.
- `src/NzbDrone.Core/MediaFiles/UpgradeMediaFileService.cs`.

Rules, when `HardlinkOnly` is true:
- **Copy-mode imports** are the ones where the client keeps seeding (`copyOnly`).
  - The transfer uses `TransferMode.HardLink` only. There is no `HardLinkOrCopy` fallback, so `DiskTransferService` throws `IOException` if the link fails.
- **Pre-flight check.** Before any existing file is removed in `UpgradeEpisodeFile`:
  - `VerifyHardlink` builds the destination path, creates the folder, and tries `TryCreateHardLink(source, <destDir>/.sonarr-hardlink-<guid>)`.
  - On failure it throws "Hardlink-only import cannot link this download to the library. Existing files have been kept…".
  - On success it deletes the probe.
- **Storage estimate.** `GET /librarytools/storage` reports `importSpaceEstimate = 0` when hardlink-only is on.

- Default: off.
- Status: **Operational.**
- Tests: `UpgradeMediaFileServiceFixture.failed_hardlink_preflight_keeps_existing_file_and_database_record`.

### 14.3 External-ID check (MatchExternalIds) — cross-reference to feature 8

Implementation: new decision spec `src/NzbDrone.Core/DecisionEngine/Specifications/ExternalIdsSpecification.cs`, registered automatically like all `IDownloadDecisionEngineSpecification`.
- Priority Default; rejection is Permanent with reason `WrongSeries`.
- When enabled, it rejects in two cases:
  - **TVDB:** `Release.TvdbId > 0`, `Series.TvdbId > 0`, and they differ. Negative placeholder TVDB IDs are therefore ignored.
  - **IMDb:** both IDs normalize to non-null values that differ. Normalization is trim → lower-case → remove `tt` → trim leading `0`s; the result must parse as a `long`, otherwise it counts as null.
- Default: off.
- Status: **Operational.**
- Tests: `LibraryTools/ExternalIdsFixture`.

### 14.4 Prefer anime season packs

Implementation: `src/NzbDrone.Core/IndexerSearch/ReleaseSearchService.cs`, `SearchAnimeSeason` (around line 581).
- Applies only to **anime season searches that are not interactive**.
- `AnimeSeasonPackSearch` runs first; that is inherited behaviour.
- With `PreferAnimeSeasonPacks`, individual-episode searches are then skipped for episodes already covered by an **accepted** (no rejections) `FullSeason` decision. All other episodes are still searched one by one.
- Default: off.
- Status: **Operational.** No dedicated test found.

### 14.5 Recent searches (diagnostics)

Implementation:
- `src/NzbDrone.Core/LibraryTools/SearchActivityService.cs`.
- Recorded in `ReleaseSearchService.Dispatch`, after `GetSearchDecision`, inside try/catch.
- Per-indexer failures come from a new `DispatchIndexer` wrapper. It collects strings of the form `"<indexer>: search failed (<ExceptionType>)…"`.

Each search is stored as one row and the list is capped at 100 (newest first). Every row has:

- `date`, `seriesId` and `query` (`criteria.ToString()`).
- `found` (decision count) and `accepted` (decisions with no rejections).
- `indexers`: the number of indexers searched.
- `rejections`: the top 12 rejection messages, each with its count.
- `providerFailures`: the distinct failure strings.

This covers automatic and interactive searches alike, and every search writes to the Config table.

- API: `GET /librarytools/searches`.
- UI: "Recent searches" (`SearchActivity.tsx`).
- Status: **Operational.** No unit test.

### 14.6 Companion app keys (scoped API keys)

Storage and creation:
- Implementation: `ProOptionsService.Authenticate` (`src/NzbDrone.Core/LibraryTools/ProOptionsService.cs:140`) and `LibraryToolsController.CreateKey`.
- Stored in Config `ProAccessKeys` as `[{Id (guid "N"), Name (trimmed, 1–80 chars), Hash, Permission, Created, Expires}]`.
- `POST /librarytools/keys` validates the request:
  - `name` must be non-blank and ≤80 chars;
  - `permission` must be `read-library` or `add-series`;
  - `days` must be 1–365 (default 90, matching the UI default);
  - at most **50** keys can exist (400 otherwise).
- Token: `"spr_" + lowercase hex(32 random bytes)`. It is returned once.
- Stored hash: uppercase hex SHA-256 of the UTF-8 token. Comparison is constant-time (`CryptographicOperations.FixedTimeEquals`).
- Revocation (`DELETE`) removes the key.

Enforcement happens in authentication, in `src/Sonarr.Http/Authentication/ApiKeyAuthenticationHandler.cs` (fork diff):
- The provided key is read from query `apikey`, header `X-Api-Key`, or `Authorization: Bearer`. This is the shared `ParseApiKey`. The docs mention only `X-Api-Key`, but the query and Bearer forms also work.
- If the key is not the master key, `Authenticate(token, Request.Method, Request.Path)` is tried. It returns null (and auth fails) unless every check below passes:
  - token starts with `spr_`;
  - a stored key has an allowed permission, a 64-hex hash, and is not expired (`Expires > UtcNow`);
  - the hash matches;
  - the path splits into `["api", "v3"|"v5", endpoint, ...]`.
- **Read** is allowed for `GET` when the endpoint is one of `series`, `episode`, `episodefile`, `calendar`, `tag`, `rootfolder` or `qualityprofile`, and the path is either exactly 3 segments or 4 segments where the 4th is a positive integer. Query strings are unrestricted, e.g. `?seriesId=`.
- **Add** is allowed for `POST /api/{v3|v5}/series` (exactly 3 segments) and only with permission `add-series`.
- Everything else is rejected:
  - lookup subroutes;
  - SignalR, because its path does not start with `api`;
  - commands, settings, logs, librarytools and compression (5 segments);
  - PUT and DELETE;
  - HEAD.
  - Endpoint matching is case-sensitive, so it fails closed.
- On success the principal gets the claims `ApiKey=true`, `CompanionKey=<id>` and `Permission=<perm>`.
- An invalid key now returns `AuthenticateResult.Fail("Invalid API key or insufficient permission.")` instead of upstream's `NoResult()`. The visible effect is the same 401 challenge under the "API" fallback policy (`NzbDrone.Host/Startup.cs:233`).

Add-series policy: when the `CompanionKey` claim is present, `CompanionAddSeriesPolicy.Validate` runs. It is called from `Sonarr.Api.V3/Series/SeriesController.cs:197` and `Sonarr.Api.V5/Series/SeriesController.cs:223`.
- It rejects requests that:
  - send a non-empty `path`;
  - send a `rootFolderPath` that does not match (`PathEquals`) a registered root folder;
  - set `addOptions.searchForMissingEpisodes == true` or `addOptions.searchForCutoffUnmetEpisodes == true`.
- On success it rewrites `RootFolderPath` to the stored root's canonical path.
- Other fields (profile, tags, monitor type, grouping fields) are caller-controlled.

UI: the "Companion app keys" section on the Library tools page lets you create keys, shows the token once, lists keys and revokes them.

- Status: **Operational.**
- Unit tests: `LibraryTools/ProOptionsServiceFixture` and `CompanionAddSeriesPolicyFixture`.
- Runtime test: `docker/tests/functional-security.py::companion_permissions` checks allowed reads, rejected paths (including `..` and `%2f` traversal), rejected commands, PUT and DELETE, read-only POST → 401, add → 400 validation, and revocation. These are recorded scripts; I did not run them.

### 14.6b Retained episode versions (migration 239; feature 6 item)

**Schema.** Migration 239 (`src/NzbDrone.Core/Datastore/Migration/239_retained_episode_versions.cs`) creates table `RetainedVersions`:

| Column | Type |
| --- | --- |
| `Id` | int, PK, identity |
| `SeriesId` | int, not null |
| `RelativePath` | text, not null; relative to the series folder |
| `MetadataJson` | text, not null; serialized `EpisodeFile` snapshot |
| `EpisodeIdsJson` | text, not null; JSON `int[]` |
| `Created` | datetime, not null |

- There is an index on `SeriesId` and no foreign key.
- The table is mapped in `Datastore/TableMapping.cs` and accessed through `LibraryTools/RetainedVersionRepository.cs` (`ForSeries(id)`).

**Opt-in.** Retention is per series: `ProOptions.Series[id].KeepVersions`, default false. It is set with `PUT /api/v5/librarytools/series/{id}` and the checkbox in the series page's "Files, naming and retained versions" panel.

**Capture.** This happens on upgrade only (`src/NzbDrone.Core/MediaFiles/UpgradeMediaFileService.cs`).
- For each existing episode file being replaced that **exists on disk**:
  - with `KeepVersions` on, the file goes to `RetainedVersionService.Preserve(file, series, file.Episodes.Value.Select(e => e.Id))`;
  - otherwise it goes to the recycle bin, as upstream does.
- The original modification time is captured before removal, into `localEpisode.PreservedFileDate`. That is the separate "Preserve original file date" feature (feature 10).
- `Preserve` holds a static lock while it works:
  1. It resolves the source and the destination `<seriesPath>/Plex Versions/<guid N>/<original file name>` through `ArchivePathGuard` (14.9).
  2. It snapshots the `EpisodeFile` fields:
     - `SeasonNumber`, `RelativePath`, `Size`, `DateAdded`, `OriginalFilePath`, `SceneName`;
     - `ReleaseGroup`, `ReleaseHash`, `Quality`, `IndexerFlags`, `MediaInfo`, `Languages`, `ReleaseType`;
     - `SourceReleaseSize` and `SourceReleaseTitle`.
  3. It **inserts the DB row first**, then moves the file with `TransferMode.Move`.
  4. If the move fails, it deletes the row and rethrows.
- The upstream path then deletes the old `EpisodeFile` row as usual.
- `Plex Versions` is already excluded from disk scans upstream, by `DiskScanService.ExcludedSubFoldersRegex` at `MediaFiles/DiskScanService.cs:73`. Retained files are therefore never re-imported. Media servers may still see that folder.

**Not captured:**
- manual deletes;
- rename;
- files that are missing on disk;
- extras such as subtitles, which follow upstream extra-file handling.

**Activation.** `POST /api/v5/librarytools/series/{id}/versions/{versionId}/activate`:
- The controller refuses with 409 if any started command has `RequiresDiskAccess` (imports, moves, renames…) or the compression worker reports an active job for the series.
- Otherwise it queues `ActivateRetainedVersionCommand` (`RequiresDiskAccess`, so it is serialized with other disk commands).

`RetainedVersionService.Activate` then:
1. Refuses on any of these:
   - the version belongs to another series;
   - `series.PendingPath` is set (a move is in progress);
   - the stored episode IDs are not all of this series;
   - they do not share exactly one non-zero `EpisodeFileId`;
   - the active file's episode set differs from the stored set. The message tells the user to use Manual Import.
2. Refuses if the archived file is missing, or if the destination (the version's **old** `RelativePath`) exists and is not the active file.
3. **Copies** the archive to a fresh `Plex Versions/<guid>` staging path.
4. Preserves the currently active file as a new retained version, through the same `Preserve`.
5. Moves staging to the old relative path and updates the existing `EpisodeFile` row (same `Id`) with the stored metadata.
6. On failure it does two things:
   - deletes the installed copy, but only if it is unchanged (same size and mtime as installed);
   - copies the backup back if the active path is now empty.
7. Staging is always cleaned up.

Net effect:
- The selected archive is **kept**: it is copied, not moved.
- The previous active file becomes a new archive.
- Disk use grows with each switch.

**Listing.** `GET /api/v5/librarytools/series/{id}/versions` returns versions newest first, each with `available` (the file exists on disk), `metadata` and `episodeIds`.

Gaps a reimplementation should fix:
- No API or UI deletes or prunes a retained version.
- Rows are not removed when a series is deleted; there is no `SeriesDeletedEvent` handler.
- The folder name `Plex Versions` is hard-coded.
- `LibraryArchiveController` sets `Trigger = Manual` in the object initializer, but `CommandQueueManager.Push` overwrites it with its `trigger` argument, which defaults to `Unspecified`. This also applies to recycle restore. It is cosmetic.

- Default: off, per series.
- Status: **Operational.**
- Tests:
  - `LibraryTools/RetainedVersionFixture`: activation keeps both files; a DB failure restores the original; a pending move blocks the operation.
  - `ArchivePathGuardFixture`.
  - No runtime docker test.

### 14.7 Storage outlook and library check

**Storage outlook** (`GET /librarytools/storage`).
- Per registered root folder, it considers tracked downloads that are:
  - `IsTrackable`;
  - not in state Imported, Ignored or Failed;
  - for a series whose path is inside the root.
- These are de-duplicated by `(client, downloadId)`.
- Returned fields:
  - `freeSpace` (null on IO or permission error);
  - `queuedDownloads`;
  - `downloadRemaining` (Σ `RemainingSize`);
  - `importSpaceEstimate`: Σ `TotalSize`, or 0 when hardlink-only is on. Hardlink-when-possible mode is ignored, so the estimate is conservative.
  - `mayRunOutOfSpace = estimate > free`;
  - an `importMode` text and an explanation text.

**Library check** (`POST /librarytools/audit`). For each series it:
- reports a missing folder and skips the series' files;
- for each registered file, reports the first of these that applies: outside the series folder, duplicate path record, or missing on disk.

It is read-only.

- Status: **Operational.** No tests.

### 14.8 Per-series renaming mode and season folder titles

Implementation: `src/NzbDrone.Core/Organizer/FileNameBuilder.cs` (lines ~155, ~318, ~544) and `Organizer/NamingContext.cs`.

**Automatic renaming.** Unless `NamingContext.ManualRename` is active, the original title is used (no rename) when either:
- the series option is `"manual"`; or
- global `RenameEpisodes` is off and the series option is not `"automatic"`.

So `"automatic"` forces renaming even when renaming is globally off. Manual rename (`RenameEpisodeFileService` and the naming samples) wraps the work in `NamingContext.ForManualRename()`, so Preview Rename still applies the format.

**Season folder title.** `GetSeasonFolder` replaces `{Season Title}` with `SeasonTitles[season]` when it is set; otherwise it uses the provider's season title. `{Season Year}` becomes the year of the earliest `AirDateUtc` in that season. This only happens when the folder format matches `\{season[ ._-]*year`.

- Status: **Operational.**
- Tests: `LibraryTools/NamingOptionsFixture`.

### 14.9 Recycle-bin browser and guarded restore

The fork's "guarded restore" is the **recycle-bin file restore**.
- Database backup restore (`Datastore/DatabaseRestorationService.cs`) is **byte-identical to upstream** (Ion Bazan's "validate restored database and keep the previous DB" change, cherry-picked). Only its fixture gained an expected-error assertion.

**Browse** (`GET /librarytools/recycle`):
- Enumerates the configured `RecycleBin` recursively, skipping reparse points and inaccessible entries.
- Keeps only media extensions (`MediaFileExtensions`) plus `.srt .ass .ssa .sub .idx .vtt`.
- Returns at most 2000 files and sets `truncated` when there are more.
- The `token` is base64(UTF-8 relative path).

**Restore** (`POST /librarytools/recycle/restore`). Under `LibraryOperationGate` the controller:
1. Requires a token of 1–4096 chars and an existing series.
2. Decodes the token and validates it with `ArchivePathGuard.Resolve(RecycleBin, relative)`.
3. Returns 409 if the compression worker reports an active job for the series.
4. Queues `RestoreRecycledFileCommand`, which `RequiresDiskAccess`.

`RecycleRestoreService.Execute` then:
- requires no `PendingPath`, an existing season number, and that the series folder exists;
- re-validates the source and requires an allowed extension and that the file exists;
- targets `BuildSeasonPath(series, season)` when `SeasonFolder` is on, otherwise the series root, keeping the original file name;
- refuses if the target exists ("will not be overwritten");
- moves the file (Move mode) and queues `RescanSeries`.

The restored file is then matched by the normal disk-scan import. It may be rejected or left unregistered if the episode already has a better file; that is inherited behaviour.

**`ArchivePathGuard.Resolve(root, relative)`** (`src/NzbDrone.Core/LibraryTools/ArchivePathGuard.cs`):
- Rejects blank or rooted input.
- Canonicalizes with `GetFullPath`.
- The result must start with `root + separator` and must not equal the root. Comparison is case-insensitive on Windows only.
- Rejects any existing component, walking from the file up to the root, that is a reparse point (symlink or junction).

- Status: **Operational.**
- Tests: `RecycleRestoreFixture` (no overwrite; extension list not mutated; waits for moves) and `ArchivePathGuardFixture`.

### 14.10 Persistent manual episode mapping (grab-time override survives restart)

Implementation:
- `src/NzbDrone.Core/LibraryTools/ManualEpisodeMappingService.cs`.
- The grab side is `Sonarr.Api.V3/Indexers/ReleaseController.cs:129` and `Sonarr.Api.V5/Release/ReleaseController.cs:149`. An interactive grab with an `override` (series id, episode ids, quality, languages) sets `RemoteEpisode.ManualEpisodeMapping`.
- `History/HistoryService.cs:180` stores `Data["ManualEpisodeMapping"]="true"` and `Data["ManualEpisodeIds"]=<json int[]>` on the Grabbed event.
- The import side is `MediaFiles/EpisodeImport/ImportDecisionMaker.cs` (around line 143).

At import, `Find(localEpisode, downloadId, multipleFiles)`:
1. Takes the newest Grabbed history row for that download id and series with the flag set.
2. Loads the episodes and keeps only those of this series. If the count differs from the distinct stored ids, it returns an empty mapping, which becomes the rejection "Your saved episode choice could not be mapped unambiguously… Use Manual Import".
3. With a single file, it maps all selected episodes.
4. With multiple files, it parses each file:
   - Full-season or multi-season file names → empty mapping (rejection).
   - Absolute numbers → keep the selected episodes whose absolute numbers appear. The count must equal the number of distinct absolute numbers.
   - Otherwise, use episode numbers. The season may be remapped only when the grabbed release covers one season, all targets are in one season, and the file season equals the release season. The count must match.

A successful mapping:
- sets `localEpisode.Episodes`;
- sets `ManualEpisodeMapping` (which `MatchesFolderSpecification` skips);
- clears `NumberingRejection` and `VerifiedNumbering`.

- Status: **Operational.**
- Tests: `LibraryTools/ManualEpisodeMappingFixture` (3 cases).

### 14.11 Import-list preview and selective add

Implementation: `Sonarr.Api.V5/LibraryTools/ImportListPreviewController.cs`.

**Preview** (`GET /lists/{id}`):
- Calls the list instance's `Fetch()` directly and caches `{token (guid), items, partial = result.AnyFailure}` per list id for **10 min** (`ICached`).
- Each item gets:
  - `metadataSource` from `ImportListIdentity.GetSource(item)`;
  - `foreignId` (no network lookup when a source ID exists);
  - `alreadyAdded`, via `ImportListIdentity.Matches`;
  - `excluded`, via `IsExcluded` against global exclusions;
  - `supported` (a source exists or an IMDb id is present).

**Add** (`POST /lists/{id}/add`) returns 400 if:
- the preview is partial;
- the token does not match or has expired;
- the root folder id or profile is unknown;
- there are not 1–25 keys, or a key is out of range.

Then, per key, it:
- skips series already added or excluded;
- otherwise builds the series with `CreateSeries` (an IMDb-only item requires a provider lookup) and sets:
  - `RootFolderPath` and `QualityProfileId`;
  - `SeasonFolder = true`;
  - `Monitored = request.monitored` (UI default false);
  - `AddOptions.Monitor = All/None` with both search flags false.

**Divergence:** preview and add call `GetSource(item)` and `CreateSeries(item, lookup, language)` **without** the list's `PreferredMetadataSource` (migration 250). The scheduled sync passes it (`ImportListSyncService.cs:165`). As a result, the Library-tools add path always uses the Automatic provider order.

- Status: **Operational**, with the provider-preference gap above.
- Tests: none for the controller.

### 14.12 Per-show compression (controller + optional host worker)

**Tanukarr side** (`Sonarr.Api.V5/Series/SeriesCompressionController.cs`):
- It is a thin authenticated proxy. It never runs ffmpeg itself.
- It forwards over HTTP on a **Unix domain socket**, at `$SONARR_COMPRESSION_SOCKET` or by default `/compression/worker.sock`, using a static `HttpClient` with a 30 s timeout and base `http://compression`.
- If the socket file is absent or the request fails:
  - GET returns 200 `{"available":false,"job":null,"modes":[],"message":"Compression worker is not connected."}`;
  - POST and DELETE return 503.
- `POST` (start) holds `LibraryOperationGate` and refuses with 409 if a queued or started `ActivateRetainedVersion` or `RestoreRecycledFile` command exists for that series.
- `HasActiveJob(seriesId)` is used by restore and activate:
  - false if there is no socket;
  - an `IOException` (surfaced as 400 or 409) if the worker answers non-200;
  - otherwise true when `job.status ∈ {queued, running, cancelling}`.

**Worker** (`docker/compression/worker.py`, Python 3, host-side, not in the image):
- It is configured by JSON: `socket`, `stateDirectory`, `sonarrConfig` (path to `config.xml`, from which it reads the **master** `ApiKey`), `sonarrUrl`, `mediaRoots[]`, and `scriptIndex`. The `scriptIndex` default is `/home/jellyfin/.ghost_index`, which is deployment-specific.
- The socket is mode 0660. There is no TCP listener, so the socket permissions are the access control.

Socket API:
- `GET /capabilities` returns `{cpu, threads, hardware, modes[]}`.
- `GET`, `POST` and `DELETE /series/<id>` read, start or cancel a job.
- `POST` bodies are limited to 4096 bytes and contain `{mode, minSizeMb: int 0..100000}`.

Calls back to Tanukarr with `X-Api-Key`:
- `GET /api/v5/series/{id}`;
- `GET /api/v5/episodefile?seriesId={id}` and `GET /api/v5/episodefile/{fileId}`;
- `POST /api/v5/command {name: "RescanSeries", seriesId}` after at least one file was compressed.

All of these exist in v5 (`EpisodeFileResource.Path`, `CommandController.StartCommand` deserializes the body into the command type).

Modes:
- `software-fast` and `software-slow`: libx264 CRF 23, preset fast/slow.
- `hardware-fast` and `hardware-slow`: `h264_vaapi` on `/dev/dri/renderD128` with `LIBVA_DRIVER_NAME=i965` (Intel Haswell). Settings are quality 8 / qp 24 / bf 0 and quality 1 / qp 23 / bf 2. They are offered only if a startup lavfi probe succeeds.
- Optional `filesaver` and `phantom` modes read CRF/PRESET/QP/MIN_SIZE_MB from `/usr/local/bin/{filesaver,phantom}` scripts. These are personal-deployment specific.

Job processing:
- At most 10 queued or active shows are accepted, and one show is processed at a time.
- A `fcntl` lock on `<scriptIndex>.lock` prevents running alongside those scripts.
- ffmpeg runs under `nice 15` and `ionice -c2 -n7` with ≤2 threads.
- Before each file, the worker re-checks that the file still belongs to the series and that the series path has not changed. The file must resolve inside the series folder, and that folder must be inside a configured media root without being the root itself. Symlinks are refused.

A file is skipped when:
- its extension is not `.mkv`, `.mp4` or `.m4v`;
- it is smaller than `minSizeMb`;
- its codec is HEVC or AV1;
- it is HDR (PQ/HLG, mastering or DoVi side data) or high bit-depth;
- it has more than one video stream (or cover art);
- it was already processed (state `processed.json` keyed by dev/ino/size/mtime, or a matching script-index line);
- free space is below `size*1.1 + 64 MiB`.

Encoding:
- Input is restricted with `-protocol_whitelist file -format_whitelist matroska,mov`.
- `-map 0`, metadata, chapters and `-c copy` are kept except for the first video stream.

Output validation, all of which must pass:
- output is ≥2% smaller;
- duration within max(1 s, 0.5%);
- codec matches and width/height are equal;
- the per-stream signature (type, non-video codec, disposition, language) and the chapter count are equal;
- a full decode of video and audio with `-xerror` produces no error output.

Commit:
1. Re-check the live series path, the registration, and the source `(dev, ino, size, mtime)`.
2. Copy mode, owner and atime/mtime, then `fsync`.
3. Atomically `os.replace` over the original. Hard-linked torrent originals keep the old inode.
4. Mark the file as processed.

Restarts mark active jobs `interrupted`.

UI: series-page panel `SeriesCompression.tsx` with capabilities and modes, start and cancel, polling every 30 s.

- Default: unavailable unless the worker is installed and the socket is mounted. Neither `docker-compose.yml` nor the image provides it.
- Status: **Operational as an opt-in external component.** The Tanukarr proxy code is complete. The worker is host-specific (Linux, x264, Intel VAAPI i965 for hardware modes).
- Tests:
  - `docker/compression/test_worker.py`: 29 tests. The integration cases are opt-in via `SONARR_COMPRESSION_INTEGRATION=1`.
  - No .NET test for `SeriesCompressionController`.
- The worker relies on a plaintext master API key read from `config.xml`; a reimplementation should prefer a scoped credential.

### 14.13 Related small changes (backup)

- **`BackupCommand.UpdateScheduledTask` override removed.** Upstream returns `Type == Scheduled`; in the fork, manual backups now also reset the backup task timer, as FORK.md describes.
- **New `HealthCheck/Checks/BackupCheck.cs`.** It reports an Error (`BackupStale`) when the newest **scheduled** backup is older than `BackupInterval + 1` days. It is silent when no scheduled backup exists yet.
- Tests: `BackupCheckFixture`, `JobTests/BackupTaskFixture`, and `docker/tests/backup-tasks.py`.

### 14.14 Docs vs code (library tools)

Accurate:
- **Pause.** `docs/library-tools.md` "Pause automation" is accurate for scheduled and automatic work. It omits two things:
  1. Add Series "start search" requests and automatic re-download searches after a failed download are suppressed, because they are queued server-side with a non-Manual trigger. Series-editor searches are posted by the UI as Manual commands and still run.
  2. Release-push grabs from external tools are refused.
- **Companion keys.** The docs' limits (1–365 days, default 90, max 50, hash-only storage, allowlist, add-series restrictions) all match the code. The only difference is that the docs say `X-Api-Key`, while query `apikey` and `Authorization: Bearer` are also accepted.
- **Retained versions and restore.** The claims match: retained in `Plex Versions`, previous active file retained, selected archive kept, different coverage requires Manual Import, no overwrite on restore, wait for compression. Undocumented gaps: no pruning, and orphan rows after series delete.

Inaccurate or unverified:
- **"Guarded restore" (FORK.md).** It is the recycle-bin restore. The DB-restore guard is inherited upstream code.
- **List preview.** It does not honour the per-list metadata-provider preference.
- **"The main Docker image contains network-disabled ffprobe" (FORK.md, Library tools section).** **UNVERIFIED and likely overstated.** The in-app `MediaFiles/MediaInfo/VideoFileInfoReader.cs` is unchanged from upstream, and the Dockerfile only `chmod`s the NuGet-supplied ffprobe. No protocol or demuxer restriction exists for in-app probing. The restriction exists only in the separate compression worker. The phrase "network-disabled" in `docker/tests/README.md:541` refers to a test helper container.

## 15. Authentication, first run, container health

### 15.1 Graphical first-run account creation

**Upstream baseline (develop `c018578`).**
- `InitialUserService` handles `ApplicationStartedEvent`.
- When Forms auth is active and no user exists, it creates user `sonarr` with a generated 20-character password and **logs that password** through the `Auth` logger.
- Forms is the effective default: `ConfigFileProvider.AuthenticationMethod` defaults to Forms and coerces None/Basic to Forms, and `AuthenticationRequired` defaults to Enabled. This part is inherited unchanged.

**Fork behaviour** (`src/NzbDrone.Core/Authentication/InitialUserService.cs`). At startup, in this order:
1. If any user exists, do nothing.
2. Otherwise migrate legacy `<Username>`/`<Password>` from `config.xml` if present.
3. Otherwise, if the auth method is not Forms, do nothing.
4. Otherwise, if `Sonarr:Auth:InitialPassword` is set, create the user with `Sonarr:Auth:InitialUsername` or `"sonarr"`.
   - The values come from the env vars `SONARR__AUTH__INITIALPASSWORD` and `SONARR__AUTH__INITIALUSERNAME` (`Bootstrap.cs:113` binds `Sonarr:Auth`).
5. Otherwise log "No user is configured. Open Tanukarr in your browser to create your account." and create nothing. No password is generated or logged.

**Setup-required condition** (`src/Sonarr.Http/Authentication/AuthenticationService.cs`). `IsInitialSetupRequired` is true when all three hold:
- no `InitialPassword` was configured (captured once at construction);
- `AuthenticationMethod == Forms`;
- `IUserService.FindUser() == null`.

**Login page** (`src/Sonarr.Http/Frontend/Mappers/LoginHtmlMapper.cs`, `frontend/src/login.html`). When setup is required, the mapper:
- replaces `_SETUP_CLASS_` with `setup-required` on `<body>`;
- replaces `_SETUP_TOKEN_` with an HTML-encoded antiforgery request token. `GetAndStoreTokens` also sets the antiforgery cookie.

`login.html` then hides the sign-in panel and shows **Create your account**, a form that posts to `__URL_BASE__/login/setup` with these fields:
- `Username` (maxlength 64);
- `Password` and `PasswordConfirmation` (minlength 12, maxlength 128);
- the hidden `__RequestVerificationToken`.

`?setupInvalid=true` shows an error. Antiforgery is registered via `services.AddAntiforgery()` in `NzbDrone.Host/Startup.cs`.

**Endpoint** `POST /login/setup` (`AuthenticationController.Setup`):
- The controller is `[AllowAnonymous]`; the request size limit is 4096 bytes; the body is a form bound to `ResetPasswordResource{Username, Password, PasswordConfirmation}`.
- Steps:
  1. `IAntiforgery.ValidateRequestAsync`. On failure, return 400.
  2. `CreateInitialUser`:
     - the username must not be blank, its trimmed length must be ≤64, and it must contain no control characters;
     - the password must not be blank, must be 12–128 characters (not trimmed) and must equal the confirmation;
     - if validation fails, redirect to `~/login?setupInvalid=true`.
  3. Under a static `System.Threading.Lock`, re-check `IsInitialSetupRequired`. If it is false, return false, which also redirects to `setupInvalid`.
  4. `IUserService.Add(username.Trim(), password)` and log "Initial account created in the browser".
  5. Run the normal `Login` with `RememberMe` null, so the cookie is not persistent. Redirect to the URL base `/`, or return 401 if sign-in failed.

**Takeover protection:**
- Once any user exists, the endpoint can never create or replace an account.
- Provisioning an initial password via env/config disables browser setup entirely.
- External and OIDC methods also disable it.
- The lock serializes concurrent first-run submissions within one process.
- Antiforgery blocks cross-site submission.

**Not present:**
- There is no rate limit or lockout on `/login/setup`. Upstream's failed-login lockout covers `/login` only.
- There is no setup secret. An unclaimed, network-reachable instance can be claimed by whoever submits first. `docs/docker-installation.md:112` warns about this, and compose binds 127.0.0.1 by default.

- Status: **Operational.**
- Tests:
  - `Sonarr.Http.Test/Authentication/AuthenticationServiceFixture`, 7 new cases: create only when Forms has no account; never replace; disabled when an initial password is provisioned; one account under concurrent setup; not for External or OIDC; input validation matrix; oversize rejection.
  - `NzbDrone.Core.Test/Authentication/InitialUserServiceFixture`: changed to "wait for browser setup", plus password-only env provisioning.
  - Runtime: `docker/tests/first-run.py` and `first-run-browser.cjs`. These check missing token or cookie → 400, invalid input, sign-in, `HttpOnly`, no password in logs, single user row, and survival across restart. They are recorded scripts; I did not run them.

### 15.2 Other Sonarr.Http / host changes in scope

**Fork changes:**
- `ApiKeyAuthenticationHandler`: companion keys and `Fail` instead of `NoResult` (14.6).
- `RestController`: a `null` element inside a request body array now gives 400 "Request body can't contain null resources". Test: `Sonarr.Http.Test/REST/RestControllerNullResourceFixture`.
- `InitializeJsonController`: adds `productName` and `productVersion` to `initialize.json`.
- `StartingUpMiddleware`, `Bootstrap`, `ConsoleApp`, `RemoteAccessAdapter`, `SingleInstancePolicy`: branding only. Startup logs "Starting {ProductName} {ProductVersion} … Sonarr core version …".
- `ConfigFileProvider.InstanceName`:
  - defaults to `BuildInfo.ProductName`;
  - maps blank, `SonarrWeeb` or `Sonarr Pro` to the product name;
  - otherwise accepts any name, whereas upstream forces the name to start or end with "Sonarr".
- `ConfigService`: trace logs no longer print config values. This is a secrets hygiene change.

**Upstream-only deltas (Tanukarr lags, not fork features):**
- Upstream's `LoginResource.RememberMe` is a `bool` and `/login/sso` accepts `rememberMe`. Tanukarr still has `string RememberMe == "on"` and no SSO remember-me.
- Newer package versions (OpenIdConnect 10.0.14 vs 10.0.13, Swashbuckle 10.3.1 vs 10.2.3, ImpromptuInterface 8.1.0 vs 8.0.6).
- Upstream's `WriteIndented=false` for minimal-API JSON in release builds.
- Upstream's `HeaderColor` UI setting.

### 15.3 Container health check

Files:
- `src/NzbDrone.Host/ContainerHealthCheck.cs`.
- `src/NzbDrone.Console/ConsoleApp.cs`. `Main` short-circuits when any argument, after trimming `-` and `/`, equals `healthcheck` (case-insensitive). It sets the exit code and returns before `RuntimePatcher` and the host start.
- `docker/healthcheck.sh`.
- Dockerfile `HEALTHCHECK --interval=30s --timeout=10s --start-period=120s --retries=3 CMD ["/healthcheck.sh"]`.

`healthcheck.sh`:
- Exports `DOTNET_SYSTEM_IO_DISABLEFILELOCKING=1`, so reading `config.xml` takes no shared lock that would block the app's writer.
- If running as root, it execs `gosu ${PUID:-1000}:${PGID:-1000} /app/Sonarr -healthcheck -data=/config`; otherwise it runs that directly.

Algorithm:
1. Build configuration from `config.xml` (optional) and environment variables.
2. **Port:** `Sonarr:Server:Port`, else the XML `Port`, else 8989. It must be 1–65535.
3. **Bind address:** `Sonarr:Server:BindAddress`, else `BindAddress`, else `*`.
   - `*`, `0.0.0.0` or `localhost` → `127.0.0.1`.
   - `::` or `[::]` → `::1`.
   - Anything else must parse as an IP that is loopback or assigned to a local interface; otherwise the check fails, refusing remote probing.
4. **URL base:** `Sonarr:Server:UrlBase` or `UrlBase`, with slashes trimmed.
5. Send `GET http://<addr>:<port>/<urlBase>/ping` with:
   - `Host: localhost`;
   - redirects off and proxy off;
   - a 3 s timeout and a 4096-byte response buffer limit.
6. The container is healthy only when the response is HTTP 200 **and** the JSON `status` is `"OK"`. `/ping` is upstream's anonymous `PingController`, which reads the Config table with a 5 s cache, so the probe also checks the database.
7. Exit code is 0 when healthy and 1 otherwise. Every exception becomes 1, and nothing is printed, so no config or credentials leak into Docker's health log.
8. The SSL port is not probed.

- Status: **Operational.**
- Tests:
  - `NzbDrone.Host.Test/ContainerHealthCheckFixture`, 6 tests: port/url base, option precedence, loopback mapping, remote bind refused, invalid port, ping status matrix.
  - `docker/tests/functional-security.py`: HEALTHCHECK present, passes with `HTTP_PROXY` set and with `config.xml` `flock`ed, fails on a wrong port.

### 15.4 Container hardening (entrypoint and image)

`docker/entrypoint.sh`:
- `PUID` and `PGID` must be numeric and non-zero; otherwise it exits 1.
- umask is `${UMASK:-${UMASK_SET:-002}}`.
- When running as root:
  - it creates the `sonarr` user and group;
  - it runs `chown -R --no-dereference PUID:PGID /config`, touching the config only and never the media;
  - it sets `chmod 0750 /config`;
  - it then `exec gosu PUID:PGID`.
- When not root, it runs as is.

Dockerfile:
- strips setuid/setgid bits under `/usr` and `/app`;
- runs `chmod -R go-w /app`;
- removes `Sonarr.Update` and the Windows service files;
- sets `COMPlus_EnableDiagnostics=0`;
- `EXPOSE 8989`.

`docker-compose.yml` publishes `127.0.0.1:6970→8989` by default.

- Status: **Operational** (packaging).

## Status summary (features 14, 15, 14.6b)

| Item | Status | Evidence |
| --- | --- | --- |
| Settings storage (`ProOptions`/`ProAccessKeys`/`ProSearchHistory` config keys) | Operational | `ConfigService.cs` new properties; `ProOptionsService` |
| Automation pause | Operational (no tests) | `CommandExecutor.cs` gate; `DownloadService.DownloadReport` gate; banner in `PageContent.tsx` |
| Hardlink-only imports | Operational | `EpisodeFileMovingService.CopyEpisodeFile`/`VerifyHardlink`; `UpgradeMediaFileServiceFixture` |
| External-ID check | Operational | `ExternalIdsSpecification`; `ExternalIdsFixture` |
| Anime pack-first | Operational (no tests) | `ReleaseSearchService.SearchAnimeSeason` |
| Recent searches | Operational (no tests) | `ReleaseSearchService.Dispatch` → `SearchActivityService.Record` |
| Storage outlook / library check | Operational (no tests) | `LibraryToolsController.GetStorage`/`Audit` |
| Companion keys | Operational | `ApiKeyAuthenticationHandler` → `ProOptionsService.Authenticate`; `CompanionAddSeriesPolicy` in v3/v5 `SeriesController`; unit + docker tests |
| 14.6b Retained versions (mig. 239) | Operational (no prune/delete; orphans on series delete) | `UpgradeMediaFileService` → `RetainedVersionService.Preserve`; `ActivateRetainedVersionCommand`; `RetainedVersionFixture` |
| Recycle-bin restore ("guarded restore") | Operational | `RecycleRestoreService`, `ArchivePathGuard`; fixtures. DB restore = unchanged upstream |
| Per-series renaming / season titles | Operational | `FileNameBuilder` uses `ForSeries`; `NamingOptionsFixture` |
| Persistent manual episode mapping | Operational | `HistoryService` grab data + `ImportDecisionMaker` → `ManualEpisodeMappingService`; fixture |
| Import-list preview/add | Operational, but ignores per-list provider preference | `ImportListPreviewController` vs `ImportListSyncService.cs:165` |
| Compression proxy + host worker | Operational opt-in (external, host-specific; master-key based) | `SeriesCompressionController` (Unix socket); `docker/compression/worker.py`; `test_worker.py` |
| "Network-disabled ffprobe" claim | UNVERIFIED / overstated | `VideoFileInfoReader` unchanged; no restriction in `src` or Dockerfile |
| First-run browser account creation | Operational | `AuthenticationController.Setup`, `AuthenticationService.CreateInitialUser`, `LoginHtmlMapper`, `login.html`; unit + docker tests |
| Upstream generated-password bootstrap | Removed in fork (replaced by browser setup) | `InitialUserService` diff |
| Container health check | Operational | `ConsoleApp` `-healthcheck` → `ContainerHealthCheck`; Dockerfile HEALTHCHECK; fixture + docker tests |
| Entrypoint/image hardening | Operational | `docker/entrypoint.sh`, Dockerfile |


## 16. Localization and API v5

### 16a. Localization
* **Fork delta**: only 3 files, measured with `git diff 57816c149 HEAD -- src/NzbDrone.Core/Localization`.
  * `Core/en.json`: **+361 keys, 9 changed values, 0 removed**.
  * `Core/it.json`: +3 keys (`SearchCompleteSeries`, `SearchCompleteSeriesHelpText`,
    `SearchCompleteSeriesTooltip`). The Italian help text says the search "shows torrents …",
    consistent with the torrent-only restriction.
  * `LocalizationService.cs`: the `{appName}` token is now `BuildInfo.ProductName` ("Tanukarr")
    instead of the literal "Sonarr".
* The other ~25 locale files listed in `srcdiff.txt` are **upstream Weblate updates made after
  the base**. They are missing from Tanukarr; they are not fork translations. The new features
  are therefore English-only in every other UI language (English fallback), except the three
  Italian strings.
* Added English keys, grouped by key-name heuristic: import lists ~103, anime grouping ~57,
  quality/presets/codecs ~34, Jellyfin ~28, search ~19, MyAnimeList ~16, AniList ~15,
  metadata ~14, collection ~13, Simkl ~11, episode/movie ~16, MDBList ~7, plus fake release,
  multi-season, stalled, format alternatives, watched and others.
* Changed English values:
  * `ImportListsAniListSettingsUsernameHelpText` now covers private-entry authentication.
  * `IndexerTestAllFailed` gains ", try again later".
  * `LibraryImportSeriesDescription` / `LibraryImportSeriesHeader` now cover series, anime and films.
  * `MetadataSourceSettingsLoadError` and `ProviderNotTested` / `ProviderTestFailedStatus` /
    `ProviderTestPassed` / `ProviderTestWarningStatus` change to sentence case.
* Help-text accuracy issues found:
  * `EnableFakeReleaseProtectionHelpText` claims rejection of "password-protected archives". The
    code only uses a password-bait filename heuristic, and only when there is no video (§4).
  * `StalledTorrentTimeoutHelpText` says "treated as failed and blocklisted". That is true only
    via standard failed-download handling (§9c).
* Tests: `Localization/LocalizationServiceFixture` (modified for the product-name token).

### 16b. API v5: upstream, with fork additions
* **v5 is upstream Sonarr's API, not a fork invention.**
  * `Sonarr.Api.V5` exists in upstream `c018578` (Calendar, Episodes, Queue, Release, Settings, …).
  * Upstream's own frontend uses `apiRoot = '/api/v5'`, as does Tanukarr's
    (`frontend/src/Utilities/Fetch/fetchJson.ts:37`).
  * The history contains upstream's "Update workflows for v5 beta" (`fc67367`).
  * Tanukarr keeps v3 and v5; there is no v4.
* **Routes Tanukarr adds** (method, path; all under `/api/v5` unless noted):

| Area | Routes |
| --- | --- |
| Library tools (§14) | `GET/PUT librarytools`; `GET/PUT librarytools/series/{id}`; `GET librarytools/searches`; `GET librarytools/storage`; `POST librarytools/audit`; `GET/POST librarytools/keys`; `DELETE librarytools/keys/{id}`; `GET librarytools/recycle`; `POST librarytools/recycle/restore`; `GET librarytools/series/{id}/versions`; `POST librarytools/series/{id}/versions/{versionId}/activate`; `GET librarytools/lists/{id}`; `DELETE librarytools/lists/{id}/preview`; `POST librarytools/lists/{id}/add` |
| Compression (§14.12) | `GET series/compression/capabilities`; `GET/POST/DELETE series/{id}/compression` |
| Quality (§7) | `GET qualitydefinition/codecs`; `GET qualitydefinition/codecdefaults`; `GET/PUT qualitydefinition/codecfactors`; `POST qualitydefinition`; `DELETE qualitydefinition/{id}` (retire); `POST qualitydefinition/{id}/restore`. The same set exists on **v3** `/api/v3/qualitydefinition/...`. |
| Metadata / grouping (other audit) | `GET series/grouping?metadataSource&foreignId` (also v3); `GET collection/presentation?groupId` (also v3); `GET series/ordering?tmdbId&imdbId` (v5 only); `PUT settings/metadatasource` (alias route on the upstream `settings/metadata` controller); v3 `GET/PUT /api/v3/config/metadatasource` |
| Release (§2, §3) | New query parameters on the existing `GET release`: `completeSeries=true`, `seasonPacksOnly=true`. `POST release` accepts `searchInfo.completeSeries` (v5) or `completeSeries` (v3). Identical parameters exist on v3. |
| Auth (§15) | `POST /login/setup` (Sonarr.Http, not versioned) |

* **Contract changes to existing v3/v5 resources**:
  * `ReleaseResource.completeSeries`; v5 `ReleaseResource.formatScores[{id,name,score}]`.
  * v5 `QueueResource`: `seasonNumbers` from actual episodes, `episodeIdsBySeason`,
    `episodeIdsWithFiles`, `importProgress{…}`.
  * `EpisodeResource.watched`, `watchedAtUtc`.
  * `QualityDefinitionResource`: `source`, `resolution`, `codec`, `titlePattern`, `isRetired`,
    `isCustom`, `copyFromId`, `*IsMixed`, and omitted-vs-null size semantics.
  * `QualityProfileResource.codecMultipliers`.
  * `ImportListResource.preferredMetadataSource`.
  * `ImportListExclusionResource.metadataSource`, `foreignId`.
  * Settings: indexer `enableFakeReleaseProtection`, `enableMultiSeasonReleases`; downloadclient
    `stalledTorrentTimeout`; mediamanagement `skipUnmonitoredEpisodesFromPacks`.
  * `SeriesResource`: provider identity, ordering and grouping fields (other audit).
  * v5 queue removal moved to `removalMethod`/`blocklistMethod` enums. This is an upstream
    cherry-pick (`049f36f`); the old boolean parameters are gone.
* **Companion keys** (§14.6) can reach only `GET` on series, episode, episodefile, calendar, tag,
  rootfolder and qualityprofile (3–4 segment paths), plus `POST series` for `add-series` keys,
  on v3 and v5.
* Status: **Operational** (controllers are discovered by the standard ASP.NET attribute
  routing, with `[V5ApiController]` / `[V3ApiController]`).
* `openapi.json` for v3 and v5 differs from upstream. Whether the committed specs cover every
  fork route is UNVERIFIED (not inspected).

## 17. Where docs overstate or omit (consolidated)

1. **Absolute → SxxEyy** (FORK.md) is presented as a fork feature. The resolver is inherited
   from upstream; the fork supplies absolute numbers for TMDB/AniList/MAL-owned series and adds
   native/scene guards (§1).
2. **"Upstream rejects S01-S05 outright … discarded all but the first"** is stale. At the base,
   upstream parsed and mapped all seasons. Upstream now imports multi-season packs from
   interactive grabs (§2a).
3. **"A Seasons 1-9 pack is not accepted for a Season 3 search"**: it is accepted as a season-3
   grab. Only packs that do not contain the season are rejected (§2c).
4. **"Explicit manual searches retain the normal upgrade rules"**: a manual series-level "Search
   Monitored" skips complete seasons. RSS never upgrades complete seasons. Pack-first ranking
   outranks quality (§2g).
5. Complete-series search is **torrent-only**, and the `CompleteSeriesSearchCommand` is API-only.
   Neither is stated in FORK.md or README (§2f).
6. **Fake-release "rejections are recorded"**: logs only. There is no blocklist entry, so the
   release is re-fetched and re-rejected. "Password-protected archives" are not detected (§4).
7. **Pack trimming**: skipped files leave the download ImportBlocked forever. This is not
   documented (§5).
8. **Scene-mapping feed disabled** (`services.sonarr.tv` blocked). Only an integration report
   mentions it; FORK.md's matching section implies matching only gained names (§1b).
9. Sub-audit findings:
   * Exclusion identity "preserved" (migration 242) is columns only; legacy native exclusions
     are inert.
   * List preview ignores the per-list metadata preference.
   * TMDB-only list items become TMDB-owned series.
   * AniList, Trakt and Simkl V1 still use `auth.servarr.com` (§13).
   * `docs/jellyfin.md` says the connection does not sync watch state, which is stale.
     Backlog refresh amplification and the 100,000-item snapshot cap are undocumented (§11).
   * "Network-disabled ffprobe" is not backed by code (§14).
   * "Guarded restore" is the recycle-bin restore; DB restore is inherited (§14).
   * Automation pause also blocks add-and-search and release push (§14).
10. Migration numbering: FORK.md acknowledges the renumbering, but not that it makes upstream
    databases past 232 incompatible in both directions (§0).

## 18. Status table (all audited items)

| # | Item | Status | Default | Key evidence |
| --- | --- | --- | --- | --- |
| 1 | Absolute releases → SxxEyy on standard-ordered series | Operational (mechanism inherited from upstream; fork adds non-TVDB absolute numbers + native/scene guards) | always on | `ParsingService.GetAnimeEpisodes` → `FindEpisode(seriesId, absolute)`; `TmdbProxy.SetAbsoluteNumbering`; `TrailingSeasonSuffixFixture` |
| 1a | Trailing season-suffix series lookup; trailing-subgroup anime regex | Operational | always on | `FindByTitleWithoutSeasonSuffix`; `Parser.cs:220-227` |
| 1b | Scene/aired numbering resolver (`EpisodeNumberingResolver`) | Operational | always on (scene-numbered series) | `ParsingService.Map`, `AggregateEpisodes`, `EpisodeNumberingSpecification` |
| 1c | Sonarr scene-mapping and daily-series feeds | Disabled by fork (deliberate); matching impact UNVERIFIED | blocked | `SonarrCloudRequestPolicy`; `SceneMappingService` catch |
| 2a | Multi-season pack parsing (explicit-range rule) | Operational | always | `Parser.ParseMatchCollection`; `SeasonParserFixture` |
| 2b | Multi-season acceptance (`MultiSeasonSpecification`) | Operational | `EnableMultiSeasonReleases=true` | `MultiSeasonSpecificationFixture` |
| 2c | Complete-series parser and interactive search | Operational (torrent indexers only) | n/a (user action) | `CompleteSeriesReleaseParser`; `GET release?completeSeries=true`; `CompleteSeriesReleaseCacheFixture` |
| 2d | Automatic pack-first complete-series search | Operational (series search + missing search; strict specs limit real grabs) | always on, no setting | `SeriesSearchService`, `EpisodeSearchService`; fixtures |
| 2e | `CompleteSeriesSearchCommand` (dry-run capable) | Partial (API-only, no UI or scheduler) | n/a | `CompleteSeriesSearchService` |
| 2f | Skip complete monitored seasons (series search + RSS) | Operational | always on | `SeriesSearchService` season loop; `CompleteSeasonUpgradeSpecification` |
| 2g | Pack-scope-first ranking | Operational (overrides quality order) | always on | `DownloadDecisionComparer.CompareReleaseScope` |
| 2h | Queue coverage per actual season | Operational | always on | `ActiveMultiSeasonCoverage`, `QueueSpecification`, v5 `QueueResource` |
| 2i | Multi-season import: grabbed episodes only, copy, keep source | Operational | always on | `DownloadedEpisodesImportService.ProcessFolder`; `MultiSeasonImportFixture` |
| 2j | IndexerSearch definitions (`CompleteSeriesSearchCriteria`, `MovieSearchCriteria`, anime `PackSearchOnly` / `CatalogueEpisodes`) | Operational | — | `IIndexer.Fetch(CompleteSeriesSearchCriteria)`; Newznab/Nyaa overrides |
| 3 | Interactive season-pack search (native + grouped panels) | Operational | user action | `GET release?seasonPacksOnly=true` → `SeasonPackSearch`; `SeasonReleaseSearchButton` in `SeriesDetailsSeason` + `CollectionMemberCard`; `AnimePackMatcher` |
| 4 | Fake release filtering | Operational (torrent only; fail-open; not persisted) | `EnableFakeReleaseProtection=true` | `TorrentClientBase.EnsureReleaseIsNotFake`; `FakeReleaseInspectorFixture` |
| 5 | Season pack trimming | Partial (download left ImportBlocked) | `SkipUnmonitoredEpisodesFromPacks=false` | `MonitoredEpisodeSpecification`; `RetainedEpisodesFixture.should_keep_other_rejections_blocked` |
| 6a | Mig. 237 source release context | Operational | always | `CustomFormatCalculationService.ParseCustomFormat(EpisodeFile)`; migration fixture |
| 6b | Mig. 238 pending path (= upstream 234) | Operational (upstream feature, renumbered) | — | `SeriesService`/`MoveSeriesService` |
| 6c | Mig. 239 retained versions | Operational (no prune/delete; orphan rows) | per-series opt-in | §14.6b; `RetainedVersionFixture` |
| 6d | Mig. 240 subtitle language codes | Operational | always | `LanguageParser.ParseSubtitleLanguageCode`; `SubtitleService`; `SubtitleLabelsFixture` |
| 7a | Mig. 236 quality ranks (= upstream 233) | Operational (inherited) | — | `QualityProfileRankService` |
| 7b | Per-profile sizes, Mixed/partial global edits, sliders | Operational | — | `QualityDefinitionUpdateService`, `ApplySizeLimits`; `QualityProfilePartialSizeFixture` |
| 7c | Codec-normalized size + per-profile codec factors | Operational (heuristic; no bitrate-aware sizing) | defaults per table | `CodecSizeNormalizer`; `codecfactors` routes; `CodecNormalizedSizeFixture` |
| 7d | Custom quality presets (mig. 251) | Operational | presets disabled until enabled in a profile | `QualityDefinitionService.Add/Retire/Restore`; `ProfileQualityMatcher` |
| 7e | CF alternative groups / episode-title exclusion | Operational | per CF | `CustomFormatCalculationService`; `ReleaseTitleSpecification` |
| 8 | New/changed decision specs | Operational (`ExternalIds` off by default) | see §8 | spec files + fixtures |
| 9a | qBittorrent torrent reuse (tags) | Operational | always (API ≥ 2.3) | `QBittorrent.TryReuseTorrent`; reuse fixtures |
| 9b | Torrent-file fetch magnet fallback | Operational | always | `TorrentClientBase.FallbackToMagnetOrThrow` |
| 9c | Stalled torrent recovery | Operational (in-memory timer) | `StalledTorrentTimeout=0` (off) | `FailedDownloadService.CheckStalled`; `StalledRecoveryFixture` |
| 9d | Pending-release removal fixes (v3 404, stale in-memory list, v5 bulk overlap) | Operational | — | `PendingReleaseService`; `FindPendingQueueItemObsoleteFixture` |
| 9e | Tracked-download re-parse from grab history / complete-series recovery | Operational | — | `TrackedDownloadService`; `CompleteSeriesGrabTrackingFixture` |
| 9f | Other clients (Deluge, Transmission, SABnzbd, NZBGet, …) | Unchanged by fork (srcdiff entries are upstream drift Tanukarr lacks) | — | `git diff 57816c149 HEAD` touches only qBittorrent |
| 9g | Newznab/Torznab movie categories + movie RSS request | Operational; RSS side effect on existing indexers UNVERIFIED (inferred from the `[2000]` default) | `MovieCategories=[2000]` | `NewznabRequestGenerator.GetRecentRequests` |
| 10a | Import progress (in-memory) | Operational | always | `ImportProgressService`; v5 `QueueResource.importProgress` |
| 10b | Persistent manual episode overrides | Operational | automatic when override grab | `HistoryService` data + `ManualEpisodeMappingService` |
| 10c | Hardlink-only imports | Operational | `HardlinkOnly=false` | `EpisodeFileMovingService.VerifyHardlink` |
| 10d | Preserve original file date on upgrade | Operational | `FileDateType.PreserveOriginal` option | `UpdateEpisodeFileService` |
| 10e | Renaming changes (manual vs automatic, collision-safe) | UNVERIFIED (not deep-audited) | per series | `RenameEpisodeFileService` (+301 lines) |
| 11a | Jellyfin selected-library refresh (mig. 243) | Operational | refreshMode=Interval, 60 min | §11.2 |
| 11b | Jellyfin watched sync / unmonitor / remonitor (mig. 248) | Operational (opt-in) | off | §11.3 |
| 11c | Hand-off to list progress | Operational locally; remote writes UNVERIFIED | off (two opt-ins) | §11.3, §13.9 |
| 12 | Notifications | Branding only (fork); upstream multi-season message formatting and Trakt fix missing | — | §12 |
| 13a | Import-list native identity (no TVDB remap) | Operational | always | §13.1 |
| 13b | Metadata source preference (mig. 250) | Operational for sync; Partial for preview add | Automatic | §13.2 |
| 13c | Exclusion identity (mig. 242) | Operational for new exclusions; legacy native exclusions inert; no UI | — | §13.3 |
| 13d | Tag-existing type (mig. 241 = upstream 236) | Operational (upstream, renumbered) | None | §13.4 |
| 13e | AniList custom lists, filters, snapshot, rate limiting | Operational; live private sync UNVERIFIED | Watching+Planning, Finished+Releasing | §13.5 |
| 13f | MyAnimeList official API + PKCE | Operational; live UNVERIFIED | — | §13.6 |
| 13g | Simkl V2 / custom lists | Operational in code; external beta API UNVERIFIED | V1 unless ClientIdV2 set | §13.7 |
| 13h | TMDB/Trakt/MDBList include-exclude sets; Plex full watchlist; snapshots | Operational | ≥1 included source required | §13.8 |
| 13i | Progress queue (mig. 249) + writers | Operational opt-in; live writes UNVERIFIED; Simkl→AniList branch dead; no mig. test | off | §13.9 |
| 14a | Library tools options / storage / audit / recent searches | Operational (several without tests) | flags off | §14.0-14.7 |
| 14b | Automation pause | Operational (no tests; broader than documented) | off | §14.1 |
| 14c | Companion keys | Operational | none issued | §14.6 |
| 14d | Recycle-bin "guarded restore" | Operational (DB restore inherited) | — | §14.9 |
| 14e | Compression controller + host worker | Operational as an opt-in external, host-specific component | worker absent | §14.12 |
| 15a | Graphical first-run account creation | Operational (no rate limit or setup secret) | when Forms auth and no user | §15.1 |
| 15b | Container health check | Operational | Docker HEALTHCHECK | §15.3 |
| 16a | Localization | English only (+361 keys); other locales = upstream drift Tanukarr lacks | — | §16a |
| 16b | API v5 | Upstream API; fork adds the routes listed in §16b | — | §16b |
