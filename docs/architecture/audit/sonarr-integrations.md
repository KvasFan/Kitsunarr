> Audit notes produced during Kitsunarr's source analysis (read-only review of the GPL-3.0 Sonarr and Tanukarr sources, November 2026). Line references point at the audited revisions; see `../source-audit.md` for the summary.

# Sonarr v4 — External Integrations, Automation & HTTP API Audit

Source audited (read-only): `ref/sonarr` at commit `c018578e4b79ee048575c02d7a8aa2b929e4b837`
(2026-11-02, "Fixed: Don't log Postgres password during database migration"), branch `develop`, tag **`v5.0.0.2941`**
(`.github/workflows/build_v5.yml`: `SONARR_MAJOR_VERSION: 5`, `VERSION: 5.0.0`; `package.json` still says `4.0.0`).

> **Important version caveat.** The checked-out tree is the Sonarr **v5 development line**, not a v4 release. It still ships the full
> `/api/v3` surface (used by the v4 UI and all third-party tools) but `GET /api` now answers `{"current":"v5","deprecated":["v3"]}` and a
> `src/Sonarr.Api.V5` controller set exists. Newer features visible here (qBittorrent API-key auth, OIDC authentication, AllowedHosts,
> password-reset token, `OnImportComplete`, ImportList/Indexer v5 details…) may not exist in a given v4 release. Behaviour that is unchanged
> between v4 and this tree cannot be confirmed from this checkout alone; where v4 behaviour is quoted from memory it is marked **UNVERIFIED**.
> This audit focuses on `/api/v3`.

All paths below are relative to `ref/sonarr/src/` unless stated otherwise.
`Core/` = `NzbDrone.Core/`, `Common/` = `NzbDrone.Common/`, `ApiV3/` = `Sonarr.Api.V3/`, `Http/` = `Sonarr.Http/`, `Host/` = `NzbDrone.Host/`.

Items not confirmed directly in source are marked **UNVERIFIED**.

---

## 1. Newznab & Torznab

Files: `Core/Indexers/Newznab/*`, `Core/Indexers/Torznab/*`, `Core/Indexers/HttpIndexerBase.cs`, `Core/Indexers/IndexerBase.cs`,
`Core/Indexers/RssParser.cs`, `Core/Indexers/TorrentRssParser.cs`, `Core/Indexers/IndexerStatusService.cs`,
`Core/ThingiProvider/Status/*`, `Core/Indexers/RssSyncService.cs`, `Core/Indexers/FetchAndParseRssService.cs`.

Torznab **reuses** `NewznabRequestGenerator` and `NewznabCapabilitiesProvider` (`Torznab.cs`), only the parser differs
(`TorznabRssParser`). `TorznabSettings : NewznabSettings`.

### 1.1 Settings (`NewznabSettings.cs`, `TorznabSettings.cs`, `SeedCriteriaSettings.cs`)

| Field (JSON `name` in provider `fields`) | Order | Default | Notes |
|---|---|---|---|
| `baseUrl` | 0 | — | validated as root URL |
| `apiPath` | 1 | `/api` | advanced; validated as URL base |
| `apiKey` | 2 | — | privacy=apiKey; required only for hosts in allow-list (`nzbs.org`, `nzb.life`, `dognzb.cr`, `nzbplanet.net`, `nzbid.org`, `nzbndx.com`, `nzbindex.in`); Torznab allow-list empty |
| `categories` | 3 | `[5030, 5040]` | select, options via `RequestAction("newznabCategories")` |
| `animeCategories` | 4 | `[]` | select |
| `animeStandardFormatSearch` | 5 | false | also search anime with S/E numbers |
| `additionalParameters` | 6 | — | advanced; must match `(&.+?\=.+?)+`; appended raw to every URL |
| `multiLanguages` | 7 | `[]` | languages applied when title has multi-lang marker and indexer gives none |
| `failDownloads` | 8 | Newznab/Torznab default `[0,1]` (Executables, PotentiallyDangerous) | enum `FailDownloads {Executables=0, PotentiallyDangerous=1, UserDefinedExtensions=2}` |
| `minimumSeeders` (Torznab) | 9 | 1 (`IndexerDefaults.MINIMUM_SEEDERS`) | |
| `seedCriteria.seedRatio` (Torznab) | 10.0 | null | double |
| `seedCriteria.seedTime` | 10.1 | null | minutes |
| `seedCriteria.seasonPackSeedGoal` | 10.2 | 0 | `0=UseStandardSeedGoal, 1=UseSeasonPackSeedGoal` |
| `seedCriteria.seasonPackSeedRatio` | 10.3 | null | |
| `seedCriteria.seasonPackSeedTime` | 10.4 | null | minutes |
| `rejectBlocklistedTorrentHashesWhileGrabbing` (Torznab) | 11 | false | |
| `rejectTorrentFilesWithBlockedExtensionsWhileGrabbing` (Torznab) | 12 | false | |

Validation: at least one of `categories`/`animeCategories` non-empty. Torznab test additionally warns if
`apiPath`/`baseUrl` contains `/torznab/all` or `/api/v2.0/indexers/all/results/torznab` (Jackett "all" aggregate).

Newznab presets (`Newznab.cs DefaultDefinitions`, all disabled by default): DOGnzb `https://api.dognzb.cr`, DrunkenSlug
`https://drunkenslug.com`, Nzb.life `https://api.nzb.life`, NZBCat `https://nzb.cat`, NZBFinder.ws `https://nzbfinder.ws`
(cats 5030,5040,5045), NZBgeek `https://api.nzbgeek.info`, nzbplanet.net `https://api.nzbplanet.net`, SimplyNZBs
`https://simplynzbs.com`, Tabula Rasa `https://www.tabula-rasa.pw` (apiPath `/api/v1/api`).

Indexer definition (DB/API) fields (`IndexerDefinition.cs`): `enableRss`, `enableAutomaticSearch`, `enableInteractiveSearch`,
`downloadClientId` (0 = any), `priority` (default **25**, 1..50), `seasonSearchMaximumSingleEpisodeAge` (days),
plus non-persisted `protocol`, `supportsRss`, `supportsSearch`. `Enable` = any of the three enable flags.

### 1.2 Capabilities (`NewznabCapabilitiesProvider.cs`, `NewznabCapabilities.cs`)

Request: `GET {baseUrl.TrimEnd('/')}{apiPath.TrimEnd('/')}?t=caps[&apikey=KEY]` (Accept: RSS, auto-redirect on).
Cached **7 days** keyed by the serialized settings JSON. On a non-XML/non-ApiKey failure the defaults are used
("using the defaults instead till Sonarr restarts"); `XmlException`/`ApiKeyException`/HTTP errors are re-thrown.
Errors in caps go through the same `<error code=… description=…>` check as feeds (`NewznabRssParser.CheckError`).

Defaults if caps unavailable:
- `DefaultPageSize=100`, `MaxPageSize=100`
- `SupportedSearchParameters = ["q"]`
- `SupportedTvSearchParameters = ["q","rid","season","ep"]`
- `SupportsAggregateIdSearch=false`, `TextSearchEngine="sphinx"`, `TvTextSearchEngine="sphinx"`

Parsing of `<caps>`:

| XML | Effect |
|---|---|
| `<limits default=".." max="..">` | `DefaultPageSize`, `MaxPageSize` (int.Parse — will throw if missing) |
| `<searching><search available="yes" supportedParams="q,.." searchEngine="raw\|sphinx">` | if missing or `available!="yes"` → `SupportedSearchParameters=null`; else split `supportedParams` on `,` (keeps default `["q"]` if attr missing); `searchEngine` → `TextSearchEngine` |
| `<searching><tv-search available="yes" supportedParams=".." searchEngine="..">` | missing/not yes → `SupportedTvSearchParameters=null`; if `supportedParams` present → split **and `SupportsAggregateIdSearch=true`**; `searchEngine` → `TvTextSearchEngine` |
| `<categories><category id name description><subcat id name description/>` | category tree |

Page size used = `Math.Clamp(DefaultPageSize, Math.Min(100, MaxPageSize), MaxPageSize)`; 100 if caps fail (`Newznab.GetProviderPageSize`).

Category picker (`NewznabCategoryFieldOptionsConverter.cs`): hides top-level ids 1000, 3000, 4000, 6000, 7000; sorts
0 and 2000 last; fallback list when caps fail: 5000 TV with 5070 Anime, 5080 Documentary, 5020 Foreign, 5040 HD, 5045 UHD,
5050 Other, 5030 SD, 5060 Sport, 5010 WEB-DL. Options shape `{value, name, hint:"(id)", parentValue}`.

Test (`Newznab.TestCapabilities`/`Torznab.TestCapabilities`): passes if `search` supports `q`, OR `tv-search` supports any of
`q|tvdbid|rid` AND both `season` and `ep`. Before that, `HttpIndexerBase.TestConnection` fetches the first RSS page and fails
if 0 results ("IndexerValidationNoResultsInConfiguredCategories").

### 1.3 Capability predicates used by the request generator (`NewznabRequestGenerator.cs`)

| Predicate | Condition |
|---|---|
| SupportsSearch (anime/special) | `search` supported and contains `q` |
| SupportsTvQuerySearch | `tv-search` contains `q` |
| SupportsTvTitleSearch | `tv-search` contains `title` |
| SupportsTvTextSearches | QuerySearch OR TitleSearch |
| SupportsTvdbSearch / Imdb / TvRage / TvMaze / Tmdb | `tv-search` contains `tvdbid` / `imdbid` / `rid` / `tvmazeid` / `tmdbid` |
| SupportsTvIdSearches | any of the five id predicates |
| SupportsSeasonSearch | `tv-search` contains `season` |
| SupportsEpisodeSearch | `tv-search` contains `season` AND `ep` |
| SupportsAggregatedIdSearch | caps `tv-search` had a `supportedParams` attribute |

### 1.4 URL construction

```
{BaseUrl.TrimEnd('/')}{ApiPath.TrimEnd('/')}?t={tvsearch|search}&cat={comma-joined distinct ids}&extended=1{AdditionalParameters}[&apikey={ApiKey}]
   + (PageSize==0 ? "" : "&offset={page*PageSize}&limit={PageSize}")
   + {criteria parameters}
```
- If the category list is empty, **no request** is generated for that chain.
- `MaxPages = 30` pages per pageable request (generator default), page size as in 1.2 (`PageSize=100` default).
- Every request: Accept = RSS (`HttpAccept.Rss`), rate limit ≥ **2 s** per indexer (`HttpIndexerBase.RateLimit`), with
  `RateLimitKey = indexer id` (the rate limiter keys on URL host + key: `Common/Http/HttpClient.cs` `WaitAndPulseAsync(host, key, rateLimit)`).
- Title escaping: `NewsnabifyTitle(t)` = replace `+` with space then `Uri.EscapeDataString` (so clean title `The+Show` → `The%20Show`).
  `title=` param uses `Uri.EscapeDataString(sceneTitle)` (raw scene title).
- Season number: `NewznabifySeasonNumber(0) == "00"` (workaround for NNTMux), else decimal.
- Clean title (`SearchCriteriaBase.GetCleanSceneTitle`): strip leading `the `, `&`→`and`, remove `'.` `` ` `` `´‘’`, non-word → `+`,
  collapse `++`, remove diacritics, trim `+`/space. `CleanSceneTitles` = distinct cleaned titles; `AllSceneTitles` = raw ∪ clean.
  Which list is used for `q=` title queries depends on the caps `searchEngine` attribute: for `t=tvsearch&q=` (TITLE requests) the
  `<tv-search searchEngine>` value, for anime/season `t=search`/`t=tvsearch` title queries the `<search searchEngine>` value;
  `"raw"` → `AllSceneTitles`, anything else (default `"sphinx"`) → `CleanSceneTitles`.
- IDs are sent verbatim: `tvdbid`/`rid`/`tvmazeid`/`tmdbid` integers, `imdbid` as stored on the series (`tt0903747`, i.e. **with** `tt`).

### 1.5 Requests per criteria type

Notation: `ID(cats, P)` = "AddTvIdPageableRequests": if SupportsAggregatedIdSearch and (tvdb||rage||maze||tmdb present & supported)
→ one `t=tvsearch` request with all supported ids concatenated in order `&tvdbid=..&imdbid=..&rid=..&tvmazeid=..&tmdbid=..` + P
(note: imdb alone does not trigger the aggregate branch). Otherwise **first available** of tvdbid → imdbid → rid → tvmazeid → tmdbid
(single id) + P. An id is "present" when series.TvdbId>0 / ImdbId non-empty / TvRageId>0 / TvMazeId>0 / TmdbId>0.

`TITLE(P)` = "AddTitlePageableRequests": if tv-search supports `title` → one `t=tvsearch&title={escaped}`+P per **raw** `SceneTitles`;
else if supports `q` → one `t=tvsearch&q={newznabified}`+P per title (`AllSceneTitles` when `<tv-search searchEngine="raw">`, else
`CleanSceneTitles`). **Always uses `Settings.Categories`** (standard cats, even for anime series — a quirk).

Search mode (`SearchMode` from scene mapping: `Default=0, SearchID=1, SearchTitle=2`, flags): tier 1 has ID requests if
`SearchID` or `Default`, plus TITLE if `SearchTitle`; tier 2 (only if `Default`) has TITLE. Tier 2 is only executed if tier 1 returned
no releases at all (`HttpIndexerBase.FetchReleases` breaks on first tier yielding releases).

| Criteria | Preconditions (else no requests) | Categories | Tier 1 | Tier 2 |
|---|---|---|---|---|
| **SingleEpisode** | SupportsEpisodeSearch AND (TvText OR TvId) | Anime series → `AnimeCategories`, else `Categories` (ID requests only) | `ID(cats, "&season={S\|00}&ep={E}")` [+ TITLE same P if SearchTitle] | TITLE(`&season={S}&ep={E}`) |
| **Season** | SupportsSeasonSearch AND (TvText OR TvId) | `Categories` | `ID(cats,"&season={S}")` | TITLE(`&season={S}`) |
| **DailyEpisode** | SupportsEpisodeSearch AND (TvText OR TvId) | `Categories` | `ID(cats,"&season={yyyy}&ep={MM}/{dd}")` (literal `/`, not escaped) | TITLE same |
| **DailySeason** | SupportsEpisodeSearch (sic) AND (TvText OR TvId) | `Categories` | `ID(cats,"&season={Year}")` | TITLE same |
| **AnimeEpisode** | SupportsSearch (`search` supports `q`) | `AnimeCategories` | (single tier) `ID(animeCats, "&q={Abs:00}")` (t=tvsearch with id + absolute number as q); if `AnimeStandardFormatSearch` && S>0 && E>0 && SupportsEpisodeSearch: `ID(animeCats,"&season={S}&ep={E}")`; then per title: `t=search&q={title}+{Abs:00}` (note literal `+` separator), and if standard-format enabled: `t=tvsearch&q={title}&season={S}&ep={E}` | — |
| **AnimeSeason** | SupportsSearch AND SeasonNumber>0 | `AnimeCategories` | if `AnimeStandardFormatSearch`: `ID(animeCats,"&season={S}")`; per title: if title is a *season* scene title (alias that already identifies the season) → `t=search&q={title}`; else if standard-format → `t=tvsearch&q={title}&season={S}` | — |
| **SpecialEpisode** | SupportsSearch | Anime → AnimeCategories else Categories | per `EpisodeQueryTitles` ("{cleanSeriesTitle} {cleanEpisodeTitle}", `+`→space then `HttpUtility.UrlEncode`, i.e. spaces become `+`): `t=search&q={query}` | — |

Note: abs. number format `{n:00}` = zero-padded to 2 digits. Special episodes are additionally searched by S/E via SingleEpisode
criteria (`IndexerSearch/ReleaseSearchService.SearchSpecial`).

**RSS (`GetRecentRequests`)**: if caps `tv-search` available → `t=tvsearch&cat={Categories ∪ AnimeCategories}` paged (no query);
else if `search` available → `t=search&cat={AnimeCategories}` paged; else nothing.

### 1.6 Paging / fetch loop (`HttpIndexerBase.FetchReleases`)

- `MaxNumResultsPerQuery = 1000`; for non-RSS fetches stop a pageable request once ≥1000 releases collected.
- A page is "full" when `PageSize != 0 && page.Count >= PageSize`; stop paging on first non-full page.
- Releases filtered by `IsValidRelease` (non-empty title and download URL), then `CleanupReleases`: `DistinctBy(Guid)`;
  multi-language fill-in; set `IndexerId`, `Indexer` (name), `DownloadProtocol`, `IndexerPriority`, `SeasonSearchMaximumSingleEpisodeAge`.
- Tier logic: stop after the first tier that produced any release.

**RSS "last release" bookkeeping** (only when `isRecent`):
- `lastReleaseInfo = IndexerStatus.LastRssSyncReleaseInfo` (persisted JSON in `IndexerStatus` table).
- For each fetched page with items: if no last release → `fullyUpdated=true`, stop (only first page is fetched on first sync).
  If the oldest pubDate on page < last release pubDate OR any item's `DownloadUrl == last.DownloadUrl` → fully updated, stop.
  If ≥1000 collected and oldest older than 24h → stop (gap).
- After the loop: if not fully updated and a last release existed → warn "rss sync didn't cover the period between {last.PublishDate} and {oldest} UTC. Search may be required."
- Store newest release (max PublishDate) as new `LastRssSyncReleaseInfo` (`IndexerStatusService.UpdateRssSyncStatus`).
- `RecordSuccess` at the end of a successful fetch.

### 1.7 Response parsing

Common RSS (`RssParser.cs`): items = `/rss/channel/item` (root → `channel` → `item`). Content is pre-cleaned
(`XmlCleaner.ReplaceEntities`, `ReplaceUnicode`), DTD ignored.

HTTP status: indexer requests do not suppress HTTP errors, so any status ≥ 400 throws in `Common/Http/HttpClient.cs` **before** parsing
(`429` → `TooManyRequestsException` with `Retry-After`, others → `HttpException`); an `<error>` XML body sent with a 4xx status is therefore
not inspected. Caps failures are not cached (every call retries); the page-size lookup falls back to 100.

PreProcess (`RssParser.PreProcess`, for non-throwing statuses): status != 200 and not 5xx → `IndexerException("unexpected StatusCode")`;
`Content-Type` contains `text/html` while request Accept didn't → `IndexerException("Indexer responded with html content…")`.
Newznab/Torznab override: if HTTP error **and** content type is not XML → base check; then parse XML and check `<error>`.

| ReleaseInfo field | Source (Newznab) | Source (Torznab) |
|---|---|---|
| Guid | `<guid>` text (fallback random GUID) | same |
| Title | `<title>` (HTML-decoded; fallback `Unknown`) | same |
| PublishDate | `newznab:attr usenetdate` else `<pubDate>` (required: missing → `UnsupportedFeedException`) | `<pubDate>` |
| DownloadUrl | `<enclosure url>` whose `type` is preferred mime (`application/x-nzb`); if none matches → null | enclosure of type `application/x-bittorrent` or `application/x-bittorrent;x-scheme-handler/magnet`; if result isn't an absolute URI → raw first `<enclosure url>` |
| InfoUrl | `<comments>` with trailing `#comments` trimmed | same |
| CommentUrl | `<comments>` | same |
| Size | attr `size` else enclosure `length` | attr `size` else enclosure `length` |
| Languages | all attrs `language` (comma-split, ISO name lookup) else `<language>` elements | attrs `language` (no comma-split) else `<language>` |
| TvdbId | attr `tvdbid` | attr `tvdbid` |
| TvRageId | attr `rageid` | attr `rageid` |
| ImdbId | attr `imdb` (int) → `tt{0000000}` (7-digit pad) | attr `imdb` |
| InfoHash | — | attr `infohash` (Torznab override; does not fall back to magnet parsing) |
| MagnetUrl | — | attr `magneturl` |
| Seeders | — | attr `seeders` (int.Parse) |
| Peers | — | attr `peers`; else `seeders + leechers` attrs; else null |
| IndexerFlags | see below | see below |

Attribute lookup: elements `{ns}attr` with `name` compared case-insensitively, value from `value` attribute; first match wins.
Namespaces: Newznab `http://www.newznab.com/DTD/2010/feeds/attributes/`; Torznab `http://torznab.com/schemas/2015/feed`.
URLs resolved relative to the request URL (`ParseUrl`).

Indexer flags enum (`Core/Parser/Model/IndexerFlags.cs`, `[Flags]`): `Freeleech=1, Halfleech=2, DoubleUpload=4, Internal=8,
Scene=16, Freeleech75=32, Freeleech25=64, Nuked=128, Subtitles=256`.

| Flag | Newznab rule | Torznab rule |
|---|---|---|
| Scene | attr `prematch=="1"` or `haspretime=="1"` | any attr `tag` == `scene` (ci) |
| Nuked | attr `nuked=="1"` | — |
| Subtitles | attr `subs` non-empty | — |
| Halfleech | — | `downloadvolumefactor == 0.5` |
| Freeleech25 | — | `downloadvolumefactor == 0.75` |
| Freeleech75 | — | `downloadvolumefactor == 0.25` |
| Freeleech | — | `downloadvolumefactor == 0.0` |
| DoubleUpload | — | `uploadvolumefactor == 2.0` |
| Internal | — | any attr `tag` == `internal` (ci) |
(`downloadvolumefactor`/`uploadvolumefactor` default 1 when absent; float parse.)

PostProcess sanity: Newznab feed containing only torrent enclosure types → warning "did you intend to add a Torznab indexer?" and
**all releases discarded**; Torznab feed with only nzb enclosures → "did you intend to add a Newznab indexer?" and discarded.

### 1.8 Error responses (`NewznabRssParser.CheckError`, `TorznabRssParser.PreProcess`)

`<error code="N" description="..."/>` anywhere in the document:

| Condition | Exception | Effect on status |
|---|---|---|
| `100 <= code <= 199` | `ApiKeyException` (Newznab: description; Torznab: "Invalid API key") | `RecordFailure` (escalating) |
| URL has no `apikey=` AND (desc == "Missing parameter" OR (Newznab only) desc contains "apikey") | `ApiKeyException("Indexer requires an API key")` | RecordFailure |
| desc == "Request limit reached" | `RequestLimitReachedException` | RecordFailure with minimum backoff **1 h** (RetryAfter is zero for this one) |
| any other | `NewznabException(desc)` / `TorznabException("Torznab error detected: {desc}")` | RecordFailure |

(Standard Newznab codes 100–199 = auth/account errors, 200–299 = parameter/function errors, 300+ = other; only the 1xx range is special-cased.)

HTTP-level handling in `FetchReleases`:

| Exception | Status update |
|---|---|
| `WebException` NameResolutionFailure/ConnectFailure | `RecordConnectionFailure` (no escalation) |
| other `WebException` | `RecordFailure` |
| `TooManyRequestsException` (HTTP 429) | `RecordFailure(id, Retry-After or 1h)`; Retry-After parsed as seconds or HTTP-date |
| `HttpException` (other non-2xx) | `RecordFailure` |
| `RequestLimitReachedException` | `RecordFailure(id, RetryAfter or 1h)` |
| `ApiKeyException`, `CloudFlareCaptchaException`, `TaskCanceledException` (timeout), `IndexerException`, any other | `RecordFailure` |

### 1.9 Provider status backoff (`Core/ThingiProvider/Status/*`)

`EscalationBackOff.Periods` (seconds) indexed by escalation level:
`[0, 60, 300, 900, 1800, 3600, 10800, 21600, 43200, 86400]` → 0, 1 min, 5 min, 15 min, 30 min, 1 h, 3 h, 6 h, 12 h, 24 h.

Status row (`ProviderStatusBase`): `providerId, initialFailure, mostRecentFailure, escalationLevel, disabledTill`
(+ `lastRssSyncReleaseInfo` for `IndexerStatus`). `IsDisabled() = disabledTill > now`.

`RecordFailure(id, minimumBackOff, escalate)`:
1. `mostRecentFailure = now`. If `escalationLevel == 0`: `initialFailure = now`, level=1, **no further escalation this call**.
2. `inStartupGracePeriod = appStart + 15 min > now`; `inGracePeriod = initialFailure + MinimumTimeSinceInitialFailure > now`.
3. If escalate && !inGrace && !inStartupGrace → level = min(max, level+1).
4. If minimumBackOff>0: raise level until `Periods[level] >= minimumBackOff` (or max).
5. If !inGrace || minimumBackOff>0 → `disabledTill = now + Periods[level]`.
6. If in startup grace and no minimumBackOff → cap `disabledTill` at now + 5 min (`Periods[2]`).

`RecordSuccess`: if level>0 → level-- (one step only) and `disabledTill = null`. `RecordConnectionFailure` = RecordFailure without escalation.

| Provider kind | MinimumTimeSinceInitialFailure | MaximumEscalationLevel |
|---|---|---|
| Indexer (`IndexerStatusService`) | 0 | 9 (24 h) |
| Import list (`ImportListStatusService`) | 0 | 9 |
| Download client (`DownloadClientStatusService`) | 5 min | 5 (1 h) |
| Notification (`NotificationStatusService`) | 5 min | 5 (1 h) |

So for indexers: 1st failure → disabled 1 min; 2nd → 5 min; 3rd → 15 min; … up to 24 h (outside the 15-minute startup window).
Blocked indexers are filtered out of RSS/automatic/interactive lists (`IndexerFactory.FilterBlockedIndexers`). Testing an existing
indexer records success/failure. Status rows are deleted when a provider is deleted.

### 1.10 RSS sync flow (`RssSyncService.cs`, `FetchAndParseRssService.cs`)

`RssSyncCommand` (SendUpdatesToClient, long-running) → `FetchAndParseRssService.Fetch()`: all RSS-enabled, non-blocked indexers fetched
**in parallel** (`Task.WhenAll`), exceptions per indexer swallowed → combined list + `PendingReleaseService.GetPending()` →
`DownloadDecisionMaker.GetRssDecision` → `ProcessDownloadDecisions.ProcessDecisions` (grab / pending) → `RssSyncCompleteEvent`.
Progress messages: "Starting RSS Sync", "RSS Sync Completed. Reports found: N, Reports grabbed: M[, Reports pending: K]".
Default interval: config `RssSyncInterval` = **15 min** (0 disables; see §5).

### 1.11 Downloading the NZB/torrent from the indexer

`HttpIndexerBase.GetDownloadRequest(link)` = plain GET. Usenet (`Core/Download/UsenetClientBase.cs`): GET with
`RateLimitKey=indexerId`, auto-redirect on, retry policy (see 2.0); 404/410 → `ReleaseUnavailableException`; 429 logs
"API Grab Limit reached"; NZB validated (`NzbValidationService`) then `AddFromNzbFile`. Filename = `CleanFileName(title)+".nzb"`.
Torrent (`Core/Download/TorrentClientBase.cs`): see §2.1.

Outgoing HTTP User-Agent (`Common/Http/UserAgentBuilder.cs`): `Sonarr/{version} ({osName} {osVersion})`
(simplified variant `Sonarr/{major.minor}`). Basic credentials (`BasicNetworkCredential`) are sent **pre-emptively** as
`Authorization: Basic base64(user:pass)` (`Common/Http/Dispatchers/ManagedHttpDispatcher.cs`).

---

## 2. Download clients

Files: `Core/Download/DownloadClientBase.cs`, `TorrentClientBase.cs`, `UsenetClientBase.cs`, `DownloadClientItem.cs`,
`DownloadItemStatus.cs`, `DownloadClientInfo.cs`, `Core/Download/Clients/**`.

### 2.0 Common model

`DownloadClientItem` (`DownloadClientItem.cs`):

| Field | Meaning |
|---|---|
| `DownloadClientInfo` | `{Protocol, Type (client Name), Id, Name (definition name), RemoveCompletedDownloads, HasPostImportCategory}` |
| `DownloadId` | torrent infohash **UPPERCASE** / SAB nzo_id / NZBGet `drone` param |
| `Category`, `Title`, `TotalSize`, `RemainingSize`, `RemainingTime` (nullable), `SeedRatio` (nullable) | |
| `OutputPath` | `OsPath`, already remote→local remapped |
| `Message`, `Status` | `DownloadItemStatus { Queued=0, Paused=1, Downloading=2, Completed=3, Failed=4, Warning=5 }` |
| `IsEncrypted` | (SAB: title prefix `ENCRYPTED /`) → failed-download handling |
| `CanMoveFiles`, `CanBeRemoved` | gate import mode (move vs copy) and removal |

`DownloadClientInfo` (from `GetStatus()`): `IsLocalhost`, `SortingMode` (SAB only: "TV"/"Movie"/"Date"), `RemovesCompletedDownloads`,
`OutputRootFolders` (list of local paths) — consumed by health checks.

Download client definition (`DownloadClientDefinition.cs`): `enable`, `protocol`, `priority` (default 1, API validates 1..50),
`removeCompletedDownloads` (default **true**), `removeFailedDownloads` (default **true**), `tags`. Client selection
(`DownloadClientProvider.GetDownloadClient`): filter by protocol / indexer's `downloadClientId` / tags / blocked status; take the lowest
priority value group; round-robin by id within the group (remembers last used id per protocol).

Retry policy for fetching NZB/torrent files *from indexers* (`DownloadClientBase.RetryStrategy`, Polly): retries on 5xx, 408, or
`HttpException` with 5xx; delay 3 s exponential with jitter; max 2 retries.

Torrent add flow (`TorrentClientBase.Download`):
1. magnet = release `DownloadUrl` if it starts with `magnet:`; else torrentUrl = DownloadUrl. `TorrentInfo.MagnetUrl` overrides magnet.
2. If client `PreferTorrentFile` (base default false; only `TorrentBlackhole` overrides it to true): torrent first, magnet fallback. Otherwise magnet first;
   `NotSupportedException` → fallback to torrent file if available, else `ReleaseDownloadException("Magnet not supported…")`.
3. Torrent file GET: `Accept: application/x-bittorrent`, `AllowAutoRedirect=false`, `RateLimitKey=indexerId`. 301/302/303 →
   if `Location` starts with `magnet:` → magnet path; else follow manually (recursive). No Location → error. 404/410 →
   `ReleaseUnavailableException`; 429 logs "API Grab Limit reached".
4. Hash from torrent file (`ITorrentFileInfoReader`) or magnet (`MagnetLink.Parse(...).InfoHashes.V1OrV2.ToHex()`).
5. `EnsureReleaseIsNotBlocklisted`: if release had no InfoHash, set it; if not interactive search and indexer setting
   `rejectBlocklistedTorrentHashesWhileGrabbing` and blocklist contains hash for this series → `ReleaseBlockedException`.
6. `EnsureTorrentDoesNotContainRejectedFiles` (if indexer `rejectTorrentFilesWithBlockedExtensionsWhileGrabbing`): inspects file list;
   dangerous/executable/user-rejected extensions → blocklist (source `TorrentFileValidation`) + `ReleaseBlockedException`.
7. Client `AddFromTorrentFile/AddFromMagnetLink`, returns hash (debug log if client returns a different hash).

Seed configuration passed to clients (`RemoteEpisode.SeedConfiguration`, from `Core/Indexers/SeedConfigProvider.cs`): from indexer
`seedCriteria`; for full-season packs with `seasonPackSeedGoal=1` use `seasonPackSeedRatio/Time`; `SeedTime` = minutes → TimeSpan.

"Recent" priority: `RemoteEpisode.IsRecentEpisode()` = any episode with `AirDateUtc >= today-14d` → use `RecentTvPriority` else `OlderTvPriority`.

### 2.1 qBittorrent (`Clients/QBittorrent/*`)

Settings (`QBittorrentSettings.cs`): `host`(localhost), `port`(8080), `useSsl`, `urlBase`, `apiKey` (NEW: Bearer token; username/password
must then be empty), `username`, `password`, `tvCategory`("tv-sonarr"), `tvImportedCategory`, `recentTvPriority`/`olderTvPriority`
(`QBittorrentPriority {Last=0, First=1}`), `initialState` (`{Start=0, ForceStart=1, Stop=2}`), `sequentialOrder`, `firstAndLast`,
`contentLayout` (`{Default=0, Original=1, Subfolder=2}`), `addSeriesTags`, `seedTimeType` (`{Total=0, Inactive=1}`).
Category regex `^([^\\\/](\/?[^\\\/])*)?$` (subcategories with `/` allowed).

**Version / proxy selection** (`QBittorrentProxySelector.cs`): cached 10 min per `host_port`.
1. v2 probe: `GET /api/v2/app/webapiVersion` (no cookie login; Bearer/Basic headers still attached; `SuppressHttpError`): 404 → not v2;
   **403 → v2** (auth needed); 401 → error;
   other HTTP error → error; else v2. API version = `Version.Parse(GET /api/v2/app/webapiVersion)` (authenticated).
2. v1 probe: `GET /version/api` → API version `"1." + body`. v1 (qBittorrent < 4.1) endpoints: `/login`, `/query/preferences`,
   `/query/torrents?label=&category=`, `/query/propertiesGeneral/{hash}`, `/query/propertiesFiles/{hash}`, `/command/download`,
   `/command/upload`, `/command/delete|deletePerm`, `/command/setCategory` (fallback `/command/setLabel`), `/command/addCategory`,
   `/command/topPrio`, `/command/setForceStart`, `/version/qbittorrent`.

**Auth (v2)** (`QBittorrentProxyV2.cs`):
- If `apiKey` set: header `Authorization: Bearer {apiKey}` on every request; 401/403 → `DownloadClientAuthenticationException`.
- Else if username or password set: Basic credentials are *also* attached (pre-emptive Basic header) and cookie login is used:
  `POST /api/v2/auth/login` form `username`, `password`. Body must be empty or `Ok.` (`Fails.` = bad login); 401/403 → auth exception.
  Cookies (SID) cached per `baseUrl:username:password`. Requests are sent with cached cookies; on **403** response → re-login once and retry.
  If username **or** password is blank no cookie login is attempted (works with qBit's "bypass authentication for localhost/whitelisted
  subnets"); a 403 in that case → `DownloadClientAuthenticationException`.

**Endpoints (v2)**:

| Purpose | Method + path | Params |
|---|---|---|
| API version | GET `/api/v2/app/webapiVersion` | → e.g. `2.11.2` |
| App version | GET `/api/v2/app/version` | (leading `v` trimmed) |
| Preferences | GET `/api/v2/app/preferences` | reads `save_path, max_ratio_enabled, max_ratio, max_seeding_time_enabled, max_seeding_time, max_inactive_seeding_time_enabled, max_inactive_seeding_time, max_ratio_act, queueing_enabled, dht` |
| List torrents | GET `/api/v2/torrents/info` | `category={tvCategory}` if set |
| Torrent properties | GET `/api/v2/torrents/properties?hash={lowerhash}` | reads `save_path`, `seeding_time`; also used as "is loaded" probe |
| Torrent files | GET `/api/v2/torrents/files?hash=` | reads `name` |
| Add | POST `/api/v2/torrents/add` (multipart) | `urls={magnet}` or file part `torrents` (filename `{title}.torrent`); `category`; `stopped` (API ≥ 2.11.0) or `paused` = true/false when initialState Start/Stop; `sequentialDownload=true`; `firstLastPiecePrio=true`; `contentLayout=Original\|Subfolder`; share limits (API ≥ 2.8.1): `ratioLimit`, `seedingTimeLimit` or `inactiveSeedingTimeLimit` (API ≥ 2.9.2 and seedTimeType=Inactive), values in minutes; response `Fails.` → error; **409 Conflict → `DownloadClientRejectedReleaseException`** |
| Remove | POST `/api/v2/torrents/delete` | `hashes`, `deleteFiles=true` when deleting data |
| Set category | POST `/api/v2/torrents/setCategory` | `hashes`, `category` |
| Create category | POST `/api/v2/torrents/createCategory` | `category` |
| List categories | GET `/api/v2/torrents/categories` | map name → `{name, savePath}` |
| Share limits (post-add, API < 2.8.1) | POST `/api/v2/torrents/setShareLimits` | `hashes`, `ratioLimit` (-2 = global), `seedingTimeLimit`/`inactiveSeedingTimeLimit` (-2); 404 ignored |
| Move to top | POST `/api/v2/torrents/topPrio` | `hashes`; 409 (queueing disabled) ignored |
| Force start | POST `/api/v2/torrents/setForceStart` | `hashes`, `value=true` |
| Add tags | POST `/api/v2/torrents/addTags` | `hashes`, `tags` (comma-joined series tag labels) |

Post-add steps (only if needed: share limits on old API, move-to-top, force start, series tags): wait for torrent to appear via
`/torrents/properties` up to 10×100 ms; magnet without `&tr=` when `dht=false` → `NotSupportedException` (falls back to .torrent).
Hash sent lowercase; `DownloadId` stored uppercase.

**State → status mapping** (`QBittorrent.GetItems`):

| qBit `state` | DownloadItemStatus | Message |
|---|---|---|
| `error` | Warning | "qBittorrent is reporting an error" (localized) |
| `stoppedDL`, `pausedDL` | Paused | |
| `queuedDL`, `checkingDL`, `checkingUP`, `checkingResumeData` | Queued | |
| `pausedUP`, `stoppedUP`, `uploading`, `stalledUP`, `queuedUP`, `forcedUP` | Completed (RemainingTime=0) | |
| `stalledDL` | Warning | stalled, no connections |
| `missingFiles` | Warning | missing files |
| `metaDL`, `forcedMetaDL` | Queued if DHT on (msg "downloading metadata"), else Warning (DHT disabled) | |
| `forcedDL`, `moving`, `downloading` | Downloading | |
| other | Downloading | "Unknown download state: X" |

Item fields: `DownloadId = hash.ToUpper()`, `Category = category ?: label`, `Title=name`, `TotalSize=size`,
`RemainingSize = size*(1-progress)`, `RemainingTime` = `eta` seconds unless `eta<0`, `>365d`, or `== 8640000` (null),
`SeedRatio=ratio`. Output path (Completed only, API ≥ 2.6.1): API ≥ 2.11.2 and `root_path` non-empty and ≠ `save_path` → `root_path`;
else if `content_path` ≠ `save_path` → `content_path`; else Warning ("unable to determine path…"). For older APIs `GetImportItem` builds
`properties.save_path + first path component of files[0].name`. All paths remote→local remapped with `Settings.Host`.

**Seed limit / removal**: `CanMoveFiles = CanBeRemoved = RemoveCompletedDownloads && state in {pausedUP, stoppedUP} && HasReachedSeedLimit`.
`HasReachedSeedLimit`: torrent `ratio_limit >= 0` → `ratio_limit - ratio <= 0.001`; `ratio_limit == -2` and global `max_ratio_enabled` →
`max_ratio - ratio <= 0.001`; OR seeding time limit reached: per-torrent `seeding_time_limit` (min) ≥0 or -2→global `max_seeding_time`
(when enabled), compare against `seeding_time` from `/torrents/properties` (cached 5 min with smart skip); OR inactive limit: per-torrent
`inactive_seeding_time_limit` or global `max_inactive_seeding_time`: `now - last_activity > limit*60`.

`GetStatus`: output root = preferences `save_path`; if category set and API ≥ 2.0 and category has `savePath` → that path
(relative ones joined to save_path; `//` prefix converted to UNC `\\`). `RemovesCompletedDownloads` =
`(max_ratio_enabled || (max_seeding_time_enabled && max_seeding_time < 20160 min)) && max_ratio_act in {1 Remove, 3 DeleteFiles}`.

Post-import: `MarkItemAsImported` → `setCategory(tvImportedCategory)` when set and different from `tvCategory`.

**Test**: connection/version (API < 1.5 → error "requires 3.2.4"; < 1.6 with category → error; category empty → warning);
removes-at-ratio config → error; categories exist else `createCategory` (API ≥ 2.0); priority options require `queueing_enabled`;
inactive seed time requires API ≥ 2.9.2 (warning); `GET torrents/info` must succeed.

### 2.2 Transmission (`Clients/Transmission/*`) — also Vuze (`Clients/Vuze/Vuze.cs`)

Settings: `host`(localhost), `port`(9091), `useSsl`, `urlBase`("/transmission/"), `username`, `password`, `tvCategory`("tv-sonarr" for new;
legacy deserialization defaults null), `tvImportedCategory`, `tvDirectory`, `recentTvPriority`/`olderTvPriority` (`{Last=0, First=1}`),
`addPaused`. Category regex `^\.?[-a-z]*$` (ci).

Endpoint: `POST {scheme}://{host}:{port}{urlBase}rpc` with JSON `{"method": "...", "arguments": {...}}`, `Content-Type: application/json`,
Accept JSON, pre-emptive Basic auth (always attached), no redirects.

**Session-id dance** (`TransmissionProxy.AuthenticateClient`): cached per `baseUrl:password`. If none: send an empty POST to `/rpc`
(errors suppressed): `409` → read `X-Transmission-Session-Id` header (missing → error); `301` → error "redirected to {Location}";
`403` → error "add IP to RPC whitelist"; anything else → `DownloadClientAuthenticationException`. Every request carries the header;
if a request returns **409** → re-auth and resend once; **401** → auth exception. Body `result != "success"` → `TransmissionException(result)`.

| RPC method | Arguments used |
|---|---|
| `session-get` | (none) → reads `rpc-version`, `version`, `download-dir`, `seedRatioLimit`, `seedRatioLimited`, `idle-seeding-limit`, `idle-seeding-limit-enabled` |
| `torrent-get` | `fields`: `id, hashString, name, downloadDir, totalSize, leftUntilDone, isFinished, eta, status, secondsDownloading, secondsSeeding, errorString, uploadedEver, downloadedEver, seedRatioLimit, seedRatioMode, seedIdleLimit, seedIdleMode, fileCount, file-count, labels`; optional `ids:[hash]` |
| `torrent-add` | `filename` (magnet) or `metainfo` (base64 .torrent), `paused` (= addPaused), `download-dir` (if computed), `labels: [tvCategory]` (if category set) |
| `torrent-set` (seed) | `ids:[hash]`, `seedRatioLimit` + `seedRatioMode=1` if ratio; `seedIdleLimit` (minutes) + `seedIdleMode=1` if seed time; skipped if neither |
| `torrent-set` (labels) | `ids:[hash]`, `labels:[...]` |
| `torrent-remove` | `ids:[hash]`, `delete-local-data: bool` |
| `queue-move-top` | `ids:[hash]` |
(`session-stats` exists but unused.)

Version: `session-get.version` cached 6 h; `SupportsLabels = version >= 4.0` (Transmission); Vuze never. Version regex
`(?<!\(|(\d|\.)+)(\d|\.)+(?!\)|(\d|\.)+)`; Transmission test requires ≥ 2.40; Vuze requires `rpc-version >= 14`.

Category handling: with labels support, torrents are filtered by `labels` containing `tvCategory` (ci); torrents without labels
(legacy 3.x) match if a path component of `downloadDir` equals the category. Without labels support, category is a **subdirectory**:
`download-dir = {session download-dir}/{tvCategory}` (unless `tvDirectory` set; then category+directory is invalid). Torrents outside
the computed download directory are ignored.

**Status mapping** (`TransmissionBase.GetItems`), `status` enum `{Stopped=0, CheckWait=1, Check=2, Queued=3, Downloading=4, SeedingWait=5, Seeding=6}`:

| Condition (in order) | Status |
|---|---|
| `errorString` non-empty | Warning (message `"{clientName} is reporting an error: {message}"`) |
| `totalSize == 0` | Queued (magnet metadata pending) |
| `leftUntilDone == 0` and status ∈ {Stopped, Seeding, SeedingWait} | Completed |
| `isFinished` and status ∉ {Check, CheckWait} | Completed |
| status == Queued | Queued |
| else | Downloading |
(note: Transmission "paused" (Stopped) unfinished torrents are reported as Downloading — there is no Paused mapping.)

Item: `DownloadId = hashString.ToUpper()`, `Category = tvCategory`, `OutputPath = remap(downloadDir) + name.Replace(":", "_")`
(Vuze: job folder only if `fileCount > 1` or folder name == torrent name), `TotalSize=totalSize`, `RemainingSize=leftUntilDone`,
`SeedRatio = downloadedEver<=0 ? 0 : uploadedEver/downloadedEver`, `RemainingTime = eta` seconds (if ≥0).
`CanBeRemoved = RemoveCompletedDownloads && HasReachedSeedLimit`; `CanMoveFiles = CanBeRemoved && status == Stopped`.
`HasReachedSeedLimit`: `seedRatioMode==1` → stopped && ratio ≥ `seedRatioLimit`; mode 0 (global) → stopped && session `seedRatioLimited` &&
ratio ≥ session `seedRatioLimit`; `seedIdleMode==1` → (stopped||seeding) && `secondsSeeding > seedIdleLimit*60`; idle mode 0 → stopped &&
session `idle-seeding-limit-enabled`.

`GetStatus`: output root = computed download dir ?? session `download-dir`. Post-import (labels supported only): set labels =
existing ∪ {tvImportedCategory} minus tvCategory. `RemoveItem` sends lowercase hash (Vuze sends as-is).

### 2.3 Deluge (`Clients/Deluge/*`)

Settings: `host`(localhost), `port`(8112), `useSsl`, `urlBase`, `password`("deluge"), `tvCategory`("tv-sonarr", regex `^[-a-z0-9]*$`),
`tvImportedCategory`, `recentTvPriority`/`olderTvPriority` (`{Last=0, First=1}`), `addPaused`, `downloadDirectory`, `completedDirectory`.

Transport: Deluge **Web UI** JSON-RPC: `POST {base}/json` with `{"jsonrpc":"2.0","method":M,"params":[...],"id":N}`
(`Common/Http/JsonRpcRequestBuilder.cs`; `byte[]` params are base64-encoded), Accept `application/json-rpc, application/json`,
request timeout 15 s.

Auth (`DelugeProxy.AuthenticateClient`): cookies cached per `baseUrl:password`. Login = `auth.login(password)` → result must be `true`
else auth exception; store the returned cookies (Deluge's session cookie); then **ConnectDaemon**: `web.connected()`; if false → `web.get_hosts()` →
pick host whose entry `[1] == "127.0.0.1"` → `web.connect(hostId)`; none → "Failed to connect to Deluge daemon".
Errors: response `error.code` 1 or 2 → re-login and retry once (then auth exception); HTTP 408 → treated as code 2 (reconnect);
other codes → `DelugeException(message, code)`.

| Method | Params | Use |
|---|---|---|
| `system.listMethods` | — | detect `daemon.get_version` and `label.*` (label plugin enabled) |
| `daemon.get_version` (fallback `daemon.info`) | — | test |
| `core.get_config` | — | `download_location`, `move_completed`, `move_completed_path` |
| `web.update_ui` | `[fields, filter]` fields = `hash,name,state,progress,eta,message,is_finished,save_path,total_size,total_done,time_added,active_time,ratio,is_auto_managed,stop_at_ratio,remove_at_ratio,stop_ratio`; filter `{}` or `{"label": tvCategory}` | list torrents → `result.torrents` map |
| `core.add_torrent_magnet` | `[magnet, options]` | returns hash |
| `core.add_torrent_file` | `[filename, base64, options]` | returns hash |
| options | `add_paused`, `remove_at_ratio=false`, `download_location` (if set), `move_completed_path` + `move_completed=true` (if completedDirectory) | |
| `core.set_torrent_options` | `[[hash], {"stop_ratio": r, "stop_at_ratio": 1}]` (ratio only; seed time is **not** supported) | seed config |
| `label.set_torrent` | `[hash, label]` | category / post-import category |
| `label.get_labels` / `label.add` / `label.get_options` | `[]` / `[label]` / `[tvCategory]` (reads `apply_move_completed`, `move_completed`, `move_completed_path`) | test & status |
| `core.queue_top` | `[[hash]]` | priority First |
| `core.remove_torrent` | `[hash, removeData]` | removal |
| `web.disconnect` | — | reconnect when torrents come back without hash/name |

Status mapping: `state=="Error"` → Warning; `is_finished && state != "Checking"` → Completed; `Queued` → Queued; `Paused` → Paused;
else Downloading. `DownloadId = hash.ToUpper()`, `OutputPath = remap(save_path) + name`, `RemainingSize = total_size - total_done`,
`RemainingTime = eta` s (overflow → max), `SeedRatio = ratio`. `CanMoveFiles = CanBeRemoved = RemoveCompletedDownloads && is_auto_managed &&
stop_at_ratio && ratio >= stop_ratio && state == "Paused"`. `GetStatus` root: `completedDirectory` → `downloadDirectory` → label move-completed path
(if `apply_move_completed && move_completed`) → global `move_completed_path` (if `move_completed`) → `download_location`.
Test: connection → label plugin (any `label.*` method) when categories set; auto-creates labels; `web.update_ui` works.

### 2.4 SABnzbd (`Clients/Sabnzbd/*`)

Settings: `host`(localhost), `port`(8080), `useSsl`, `urlBase`, `apiKey`, `username`, `password`, `tvCategory`("tv"),
`recentTvPriority`/`olderTvPriority` (`SabnzbdPriority {Default=-100, Paused=-2, Low=-1, Normal=0, High=1, Force=2}`; default -100).

Every call: `GET {base}/api?mode={mode}&...&apikey={key}&output=json` (or `ma_username`/`ma_password` instead of apikey when no key).
Error detection: JSON `{"status": false, "error": "..."}` → exception; plain-text bodies starting with `error` → error
(e.g. "API Key Incorrect", "API Key Required" mapped to test errors).

| mode | Extra params | Parsed fields |
|---|---|---|
| `version` | — | `version` |
| `get_config` | — | `config.misc.{complete_dir, tv_categories, enable_tv_sorting, movie_categories, enable_movie_sorting, date_categories (string or array), enable_date_sorting, pre_check, history_retention, history_retention_option, history_retention_number}`, `config.categories[].{name, dir, pp, script, priority}`, `config.sorters[].{name, is_active, sort_cats, sort_type, …}` |
| `fullstatus` | `skip_dashboard=1` | `status.completedir` (SAB ≥ 2.0) |
| `queue` | `start=0&limit=0[&category=tv]` | `queue.paused`, `queue.my_home` (<2.0), `queue.slots[].{status, index, timeleft ("H:MM:SS" or "D:H:MM:SS"), mb, filename, priority (string name), cat, mbleft, percentage, nzo_id}` |
| `history` | `start=0&limit={DownloadClientHistoryLimit=60}[&category=tv]` | `history.slots[].{fail_message, bytes, category, nzb_name, download_time, storage, status, nzo_id, name}` |
| `addfile` (POST multipart) | query `cat`, `priority`; file field `name` (filename `{title}.nzb`, type `application/x-nzb`) | `{status, nzo_ids:[...]}`; empty ids → `DownloadClientRejectedReleaseException` |
| `queue&name=delete` | `del_files=0\|1&value={nzo_id}` | remove queued |
| `history&name=delete` | `del_files=0\|1&value={nzo_id}&archive={failed?0:1}` | remove history (completed items archived, failed deleted permanently) |
| `retry` | `value={id}` | `{status, nzo_id}` (proxy method exists but has **no callers**; NZBGet `HistoryRedownload` likewise unused) |

Status strings (`SabnzbdDownloadStatus`): `Grabbing, Queued, Paused, Checking, Downloading, QuickCheck, Verifying, Repairing, Fetching,
Extracting, Moving, Running, Completed, Failed, Deleted, Propagating`.

Queue mapping: skip `Deleted`; Paused if (queue paused and item priority ≠ Force) or item `Paused` (RemainingTime null);
`Queued|Grabbing|Propagating` → Queued; else Downloading. `TotalSize = mb*1024*1024`, `RemainingSize = mbleft*1024*1024`,
`RemainingTime = timeleft`; title prefix `ENCRYPTED /` stripped and `IsEncrypted=true`. `CanBeRemoved = CanMoveFiles = true`.

History mapping: skip `Deleted`; `Failed` → Failed, except `fail_message == "Unpacking failed, write error or disk is full?"` → Warning;
`Completed` → Completed; anything else (Verifying, Moving, Extracting, Running…) → Downloading. `TotalSize=bytes`, `Message=fail_message`,
`OutputPath = remap(storage)`, then walk up parents: if an ancestor folder name == item `name`, use that ancestor (job folder).

Items are kept only if `category == tvCategory` (or category `*` when tvCategory empty). `RemoveItem`: if item still in queue →
queue delete; else (deleteData && Completed → Sonarr deletes the files itself) history delete.

`GetStatus`: categories' full path = `complete_dir` (if relative: SAB ≥2.0 `fullstatus.completedir`, else `queue.my_home + complete_dir`) +
`category.dir` (trailing `*` stripped); root = category matching tvCategory, else `*`. `SortingMode` "TV"/"Movie"/"Date" from misc flags
or active sorters for this category (empty category list means all categories). `RemovesCompletedDownloads`: by
`history_retention_option` (`all`→false; `number-*`/`all-*`→true; `days-*`→number<14) else legacy `history_retention` (`Nd` → N<14; else != "0").
Version parse regex `(\d+)\.(\d+)\.(\d+|x)`; `develop` → treated as 3.0.0 with warning; min 0.7.0. Test also errors when category dir ends
with `*` (job folders disabled), category missing, TV/movie/date sorting enabled for the category, or `pre_check` on SAB < 1.1.

### 2.5 NZBGet (`Clients/Nzbget/*`)

Settings: `host`(localhost), `port`(6789), `useSsl`, `urlBase`, `username`, `password`, `tvCategory`("tv"),
`recentTvPriority`/`olderTvPriority` (`NzbgetPriority {VeryLow=-100, Low=-50, Normal=0, High=50, VeryHigh=100, Force=900}`), `addPaused`.

Transport: JSON-RPC `POST {base}/jsonrpc`, body `{"jsonrpc":"2.0","method":M,"params":[...],"id":N}`, pre-emptive Basic auth.
HTTP 401 → auth exception; `error != null` → exception.

| Method | Params | Notes |
|---|---|---|
| `version` | — | cached 1 day; test requires ≥ 12.0 |
| `append` (v16+) | `[nzbFilename, base64Nzb, category, priority, addToTop=false, addPaused, dupeKey="", dupeScore=0, dupeMode="all", ["drone", "<32-hex guid>"]]` (last param is a **flat** 2-element string array) | returns NZBID (>0 ok). **DownloadId = the generated `drone` GUID** |
| `append` (v13–15) | `[name, base64, category, priority, false, false, "", 0, "all"]` then `editqueue("GroupSetParameter", 0, "drone=<guid>", nzbId)` | |
| `append` (v12) | `[name, category, priority, false, base64]` then find by name & set param | |
| `status` | — | `DownloadPaused`, `DownloadRate`, `RemainingSizeLo/Hi`, … |
| `listgroups` | — | queue: `NZBID, NZBName, Category, FileSizeLo/Hi, RemainingSizeLo/Hi, PausedSizeLo/Hi, ActiveDownloads, Parameters[{Name,Value}]`, `FirstID/LastID` |
| `history` | — | `ID (NZBID), Name, Category, FileSizeLo/Hi, ParStatus, UnpackStatus, MoveStatus, ScriptStatus, DeleteStatus, MarkStatus, DestDir, FinalDir, Parameters` (first `DownloadClientHistoryLimit`=60 entries) |
| `config` | — | list of `{Name, Value}` → `Category{i}.Name/.DestDir/.Unpack/.DefScript/.Aliases`, `MainDir`, `DestDir`, `AppendCategoryDir`, `KeepHistory` |
| `editqueue` | `[command, offset=0, editText, id]` | `GroupFinalDelete` (queue removal), `HistoryDelete` (history removal), `HistoryRedownload` (retry) |

64-bit sizes = `(Hi << 32) | Lo`. DownloadId = value of parameter `drone` if present else NZBID as string (IDs shorter than 10 chars that
parse as int are treated as NZBID on removal).

Queue mapping: Paused if global `DownloadPaused` or (`remaining == paused` && remaining ≠ 0); else Queued if `ActiveDownloads == 0 && remaining != 0`,
else Downloading; `RemainingSize = remaining - paused`; `RemainingTime` = cumulative `(sumRemainingBefore + remaining)/DownloadRate`.

History mapping (start Completed, RemainingTime 0):
1. `DeleteStatus == "MANUAL"`: if `MarkStatus == "BAD"` → Failed (kept), else **item skipped**.
2. `ParStatus ∉ {SUCCESS, NONE}` → Failed.
3. `UnpackStatus == "SPACE"` → Warning; else `UnpackStatus ∉ {SUCCESS, NONE}` → Failed.
4. `MoveStatus ∉ {SUCCESS, NONE}` → Warning.
5. `ScriptStatus ∉ {SUCCESS, NONE}` → Failed.
6. `DeleteStatus` non-empty and ∉ {SUCCESS, NONE}: ∈ {HEALTH, DUPE, SCAN, COPY, BAD} → Failed, else Warning.
Later rules override earlier ones. `OutputPath = remap(FinalDir ?: DestDir)`; Message = `"PAR Status: {ParStatus} - Unpack Status: {UnpackStatus} - Move Status: {MoveStatus} - Script Status: {ScriptStatus} - Delete Status: {DeleteStatus} - Mark Status: {MarkStatus}"`.
Only items with `Category == tvCategory` returned. `RemoveItem` with deleteData deletes files locally first.
Category dest dir: `Category{i}.DestDir` else `DestDir` (with `${MainDir}` substituted) + category name if `AppendCategoryDir` = yes (default).
Test: version, category exists, `KeepHistory` must be > 0 and ≤ 25000.

### 2.6 Other implementations (names as shown in UI / `implementation`)

| implementation (class) | Display name | Protocol |
|---|---|---|
| `Aria2` | Aria2 | torrent (XML-RPC) |
| `TorrentBlackhole` | Torrent Blackhole | torrent (watch folder) |
| `UsenetBlackhole` | Usenet Blackhole | usenet |
| `TorrentDownloadStation` / `UsenetDownloadStation` | Download Station (Synology) | both |
| `Flood` | Flood | torrent |
| `TorrentFreeboxDownload` | Freebox Download | torrent |
| `Hadouken` | Hadouken | torrent |
| `NzbVortex` | NZBVortex | usenet |
| `Pneumatic` | Pneumatic | usenet (.strm) |
| `RQBit` | RQBit | torrent |
| `RTorrent` | rTorrent | torrent (XML-RPC) |
| `TriblerDownloadClient` | Tribler | torrent |
| `UTorrent` | uTorrent | torrent |
| `Vuze` | Vuze | torrent (Transmission RPC) |
| `QBittorrent`, `Transmission`, `Deluge`, `Sabnzbd`, `Nzbget` | (above) | |

---

## 3. Download tracking, completion, failure, removal, blocklist

Files: `Core/Download/TrackedDownloads/*`, `Core/Download/DownloadProcessingService.cs`, `CompletedDownloadService.cs`,
`FailedDownloadService.cs`, `DownloadEventHub.cs`, `RedownloadFailedDownloadService.cs`, `RejectedImportService.cs`,
`Core/Blocklisting/BlocklistService.cs`, `Core/RemotePathMappings/RemotePathMappingService.cs`,
`Core/MediaFiles/DownloadedEpisodesImportService.cs`, `Core/MediaFiles/DownloadedEpisodesCommandService.cs`.

### 3.1 Cadence

- `RefreshMonitoredDownloadsCommand`: scheduled every **1 minute**, priority **High** (`Jobs/TaskManager.cs`); also pushed (High) through a
  **5 s debouncer** on `EpisodeGrabbedEvent`, `EpisodeImportedEvent`, `ManualInteractionRequiredEvent`, download-client provider updated/deleted.
- The scheduler timer ticks every **30 s** (`Jobs/Scheduler.cs`), so effective latency ≤ 1.5 min.
- `CheckForFinishedDownloadCommand` = deprecated alias (warns "third party app used the deprecated CheckForFinishedDownload command").
- After a refresh, `ProcessMonitoredDownloadsCommand` (High; `RequiresDiskAccess`, `IsLongRunning`) does imports / failures / removals.

### 3.2 Refresh (`DownloadMonitoringService.Refresh`)

For each enabled, non-blocked download client (`DownloadClientFactory.DownloadHandlingEnabled`):
1. `GetItems()`; success → `DownloadClientStatusService.RecordSuccess`, and `StopTrackingMissing` (drop cached tracked downloads of this client
   whose id is no longer reported). Exception → `RecordFailure` (client backoff, max 1 h; see §1.9) and warning.
2. For each item: `TrackedDownloadService.TrackDownload(def, item)`; if the tracked state is `Downloading` or `ImportBlocked` →
   `FailedDownloadService.Check` then `CompletedDownloadService.Check`.
3. Keep only trackable: state ∉ {Imported, Failed, Ignored}; and if Completed Download Handling disabled, items with client status Completed
   are not tracked. `UpdateTrackable` flips `IsTrackable=false` on cached ones not seen. Publish `TrackedDownloadRefreshedEvent`
   (drives SignalR `queue` / `queue/status` updates), push `ProcessMonitoredDownloadsCommand`.

### 3.3 TrackDownload (`TrackedDownloadService.TrackDownload`)

- Cache keyed by `DownloadId`. If an existing entry exists and its state ≠ Downloading → only refresh `DownloadItem` and return.
- New entry: `State` from latest **DownloadHistory** event for the id (`DownloadImported`→Imported, `DownloadFailed`→Failed,
  `DownloadIgnored`→Ignored, else Downloading).
- Parse title → `RemoteEpisode` (if already imported, map with the series from download history).
- Look up **EpisodeHistory** by downloadId (newest first): `Indexer` and `Added` from the `Grabbed` event (`data.indexer`, date). If parse failed or
  no series/episodes → parse the grabbed history `SourceTitle` (or as special) and map with history `SeriesId` and grabbed EpisodeIds.
  Release gets `Indexer`, `Title`, `IndexerFlags` (history `data.indexerFlags`), `IndexerId` (download history).
- Aggregation + custom format calculation. Exceptions: multiple series → warn "Unable to import automatically, found multiple series: …";
  other → warn "Unable to parse episodes from title".
- `TrackedDownloadState { Downloading, ImportBlocked, ImportPending, Importing, Imported, FailedPending, Failed, Ignored }`;
  `TrackedDownloadStatus { Ok, Warning, Error }`; `StatusMessages[] {title, messages[]}`; `HasNotifiedManualInteractionRequired`.
- Cache invalidation/re-mapping on series add/edit/bulk edit/delete and episode info refresh; on `EpisodeGrabbedEvent` an entry in a
  terminal state (Imported/Failed/Ignored) with the same id is evicted (re-grab of same hash).

### 3.4 CompletedDownloadService

`Check(td)` — only if client status == **Completed**:
1. `ImportItem = client.GetImportItem(item, previous)` (qBit legacy path building etc.).
2. Only if state ∈ {Downloading, ImportBlocked}.
3. No `Grabbed` history AND empty category → warn **"Download wasn't grabbed by Sonarr and not in a category, Skipping."** (stays Downloading).
4. `ValidatePath`: empty output path → warn **"Download doesn't contain intermediate path, Skipping."**; path not valid for local OS (e.g. Windows
   path on Linux) → warn **"[{path}] is not a valid local path. You may need a Remote Path Mapping…"**.
5. Series = parse title → series; else series from grab history; none → warn "Series title mismatch; automatic import is not possible…" and
   **ImportBlocked**. If found only via history and the grab was matched by **ID** (history `seriesMatchType == Id`) and not from interactive search →
   warn "Found matching series via grab history, but release was matched to series by ID. Automatic import is not possible…" + ImportBlocked.
6. Else → `ImportPending`.

`Import(td)` (from ProcessMonitoredDownloads, only when CDH enabled and state == ImportPending):
1. Refresh ImportItem, ValidatePath; `RemoteEpisode == null` → warn "Unable to parse download, automatic import is not possible." + ImportBlocked.
2. State `Importing`; `DownloadedEpisodesImportService.ProcessPath(outputPath, ImportMode.Auto, series, importItem)`.
3. `VerifyImport`: all expected episodes imported (count of imported episodes ≥ max(1, expected)) → **Imported** + `DownloadCompletedEvent`;
   or all expected episodes' **latest** history event is `DownloadFolderImported` → Imported (files looked up from DB) + `DownloadCompletedEvent`.
4. Otherwise state back to ImportPending and:
   - no results: if the client's file list (`GetFiles`) shows rejected extensions → `RejectedImportService`; else warn **"No files found are eligible for import in {path}"**;
   - exactly one result & Rejected → `RejectedImportService.Process` (dangerous/executable/user-extension rejection + indexer `failDownloads` contains
     the matching option → `td.Fail()` → **FailedPending** (CanBeRemoved forced true); else warn) → ImportBlocked unless FailedPending;
   - else warn "One or more episodes expected in this release were not imported or missing from the release" + per-file errors → **ImportBlocked**.
- `SetStateToImportBlocked` publishes `ManualInteractionRequiredEvent` once per tracked download (→ notification `OnManualInteractionRequired`).

ImportMode Auto resolves to **Move** when `CanMoveFiles` (or no client item), else **Copy** (`DownloadedEpisodesImportService.ProcessFolder`).
After a Move with ≥1 imported file the source folder is deleted if remaining video files are all samples and no `.rar` > 10 MB remains.
Folder path that is an existing series folder → rejection `SeriesFolder` "Import path is mapped to a series folder". Folder names have
`_UNPACK_`/`_FAILED_` stripped before series lookup. Files starting with `._` rejected. Dangerous / executable / user-rejected / unsupported extensions
rejected per file. For non-client (manual scan) imports, locked files → "Locked file, try again later". Empty result for folder → checks for rejected
extensions or archives ("Found archive file, might need to be extracted").

External trigger `DownloadedEpisodesScanCommand {path, downloadClientId, importMode}` (`DownloadedEpisodesCommandService.cs`) — public contract for
third-party scripts ("Properties used by third-party apps, do not modify"): path must exist; if `downloadClientId` matches a tracked download →
import with its series and `VerifyImport`; otherwise a normal path import. Command result `Unsuccessful` when nothing imported.

### 3.5 FailedDownloadService

- `Check(td)` (state Downloading/ImportBlocked): if `IsEncrypted` or client status **Failed**: no `Grabbed` history → warn
  "Download is encrypted and wasn't grabbed by Sonarr…" / "Download has failed wasn't grabbed by Sonarr, skipping automatic download handling";
  else → **FailedPending**.
- `ProcessFailed(td)` (FailedPending): message = "Encrypted download detected" | client `Message` | "Failed download detected"; state **Failed**;
  publish `DownloadFailedEvent {seriesId, episodeIds, quality, sourceTitle, downloadClient, downloadId, message, source="Sonarr Failed Download Handling", data (grab history data), trackedDownload, languages, skipRedownload, releaseSource}`.
- Manual: `MarkAsFailed(historyId | trackedDownload, message="Manually marked as failed", source, skipRedownload)` (queue/history API).
- Failed Download Handling has no global toggle in this version (only per-client `removeFailedDownloads` and the redownload options); `Warning` status never triggers it.

`DownloadFailedEvent` consumers: `BlocklistService` (insert blocklist row), `DownloadEventHub` (remove from client if `CanBeRemoved` and definition
`removeFailedDownloads`), `RedownloadFailedDownloadService` (if not skipRedownload, config `AutoRedownloadFailed` (default true), and for interactive-search
grabs `AutoRedownloadFailedFromInteractiveSearch` (default true): 1 episode → `EpisodeSearch`; whole season → `SeasonSearch`; else `EpisodeSearch` for the
rest), history (`DownloadFailed` event). There is **no** download-failed notification trigger in Sonarr (`Core/Notifications/INotification.cs` has no OnDownloadFailure).

### 3.6 Removal (`DownloadEventHub`, `DownloadProcessingService.RemoveCompletedDownloads`)

- `DownloadCompletedEvent` → first `client.MarkItemAsImported(item)` (post-import category; `NotSupportedException` ignored), then if
  `item.CanBeRemoved` && status ≠ Downloading && definition `removeCompletedDownloads` → `client.RemoveItem(item, deleteData: true)` and stop tracking.
- After each processing pass, every tracked download with state Imported and `CanBeRemoved` → `DownloadCanBeRemovedEvent` → removal if
  `removeCompletedDownloads` (handles torrents that reach their seed goal later).
- `DownloadFailedEvent` → remove (deleteData true) if `CanBeRemoved` and `removeFailedDownloads`.
- For torrents `CanBeRemoved` is only true once the seed goal is reached (client-specific rules in §2), for usenet it is always true.

### 3.7 Blocklist (`Core/Blocklisting/*`)

Row: `seriesId, episodeIds, sourceTitle, quality, date, publishedDate, size, protocol, indexer, indexerFlags, releaseType, message, source,
torrentInfoHash, languages`. Created on `DownloadFailedEvent` (publishedDate/size/indexer/protocol/indexerFlags/releaseType/torrentInfoHash from grab
history `data`; for tracked torrents the hash is the downloadId) and by torrent file validation. Matching (`Blocklisted`):
torrent with infohash → rows with same hash (ci); torrent without hash → rows with same title & torrent protocol & (row indexer empty or same indexer);
usenet → same title & usenet protocol & same indexer (or empty) & **identical publishedDate**. Deleted on series delete; `ClearBlocklistCommand` purges all.

### 3.8 Remote path mappings (`RemotePathMappingService`)

Model `{id, host, remotePath, localPath}`. `RemapRemoteToLocal(host, remotePath)`: first mapping whose `host` equals the client's configured host
(case-insensitive, **string equality with the client Settings.Host**) and whose `remotePath` contains the path → `localPath + (path - remotePath)`.
A reverse `RemapLocalToRemote` exists but has no callers in Core. Applied by every client to output paths and output root folders.

---

## 4. Import pipeline

Files: `Core/MediaFiles/EpisodeImport/ImportDecisionMaker.cs`, `EpisodeImport/Specifications/*`, `EpisodeImport/ImportApprovedEpisodes.cs`,
`Core/MediaFiles/UpgradeMediaFileService.cs`, `EpisodeFileMovingService.cs`, `RecycleBinProvider.cs`, `MediaFileAttributeService.cs`,
`DiskScanService.cs`, `MediaInfo/*`, `EpisodeImport/DetectSample.cs`, `EpisodeImport/Aggregation/AggregationService.cs`,
`Core/Extras/**`, `Common/Disk/DiskTransferService.cs`, `NzbDrone.Mono/Disk/DiskProvider.cs`.

### 4.1 ImportDecisionMaker

`GetImportDecisions(videoFiles, series, downloadClientItem, downloadClientItemInfo, folderInfo, sceneSource, filterExistingFiles=true)`:
1. Optionally drop files already in DB for the series (`FilterExistingFiles`).
2. If `sceneSource` (download folder), count non-sample files (runtime-based) → `OtherVideoFiles` flag.
3. Per file: `LocalEpisode {Series, DownloadClientEpisodeInfo (parsed client title), DownloadItem, FolderEpisodeInfo (parsed folder), Path,
   SceneSource, ExistingFile = file is inside series folder}`; `FileEpisodeInfo = Parser.ParsePath(path)`; size; release type
   (client → folder → file); `AggregationService.Augment` (fails if none of the three parse results exist → "Unable to parse file"; reads media info
   via ffprobe for new files, or for existing files when `EnableMediaInfo`; runs aggregators: episodes, language, quality, release group,
   release hash, release info (grab history), subtitle info).
4. No episodes → rejection `PartialSeason` "Partial season packs are not supported" / `SeasonExtra` "Extras are not supported" /
   `InvalidSeasonOrEpisode` "Invalid season or episode".
5. Indexer flags from tracked download; custom formats computed; then **all specifications** evaluated (exceptions → `DecisionError`).

Import specifications (`IImportDecisionEngineSpecification`, all evaluated, rejections accumulated):

| Spec | Rejects when (reason) |
|---|---|
| `AbsoluteEpisodeNumberSpecification` | Anime series, naming format needs absolute number, and an episode aired < 1 day ago (or no air date) lacks an absolute number (`MissingAbsoluteEpisodeNumber`) |
| `AlreadyImportedSpecification` | (client imports only) episode has a file and history shows `DownloadFolderImported` for this downloadId newer than the last `Grabbed` of it (`EpisodeAlreadyImported`) |
| `EpisodeTitleSpecification` | new file, config `EpisodeTitleRequired` ≠ Never, naming uses episode title, episode aired within last 48 h (or no date) with empty title (`TitleMissing`) or `TBA` (`TitleTba`); `BulkSeasonReleases` mode skips when <4 episodes share the air date |
| `FreeSpaceSpecification` | not skipped by `SkipFreeSpaceCheckWhenImporting`; new file; free space on series' parent (root) < file size + `MinimumFreeSpaceWhenImporting` MB (default 100) (`MinimumFreeSpace`) |
| `FullSeasonSpecification` | file name parsed as full season (`FullSeason`) |
| `HasAudioTrackSpecification` | media info present and zero audio streams (`NoAudio`) |
| `MatchesFolderSpecification` | new file; folder parse maps to episodes and file maps to episodes not in the folder set (`EpisodeUnexpected`) |
| `MatchesGrabSpecification` | new file; grab history (release info) has episode ids and file episodes aren't among them (`EpisodeNotFoundInRelease`) |
| `MultiSeasonPackSpecification` | new file in a multi-season pack whose file name has no season, or absolute-only numbering mapping to a different season than its season folder (`MultiSeason`) |
| `NotSampleSpecification` | new file is a sample (`Sample`) or undeterminable (`SampleIndeterminate`) — runtime thresholds: expected ≤3 min → <15 s; ≤10 → <90 s; ≤30 → <300 s; else <600 s; runtime 0 → sample; `.flv`/`.strm`/specials never samples; expected runtime = Σ episode runtimes (series runtime fallback, 45 if 0) |
| `NotUnpackingSpecification` | new file under a folder whose name starts with any of `DownloadClientWorkingFolders` (default `_UNPACK_\|_FAILED_`); on Windows only if modified < 5 min ago (`Unpacking`) |
| `SameEpisodesImportSpecification` | existing file on disk covers more episodes than this file (`ExistingFileHasMoreEpisodes`) |
| `SplitEpisodeSpecification` | file parsed as split episode (`SplitEpisode`) |
| `UnverifiedSceneNumberingSpecification` | new file; episode has unverified XEM scene numbering (`UnverifiedSceneMapping`) |
| `UpgradeSpecification` | vs. existing files: lower quality (`NotQualityUpgrade`); same quality with lower revision when propers preferred (`NotRevisionUpgrade`); same quality with lower custom-format score (`NotCustomFormatUpgrade` / `NotCustomFormatUpgradeAfterRename`) |

`ImportRejectionReason` enum (API exposes as string): `Unknown, FileLocked, UnknownSeries, DangerousFile, ExecutableFile, UserRejectedExtension,
ArchiveFile, SeriesFolder, InvalidFilePath, UnsupportedExtension, PartialSeason, SeasonExtra, InvalidSeasonOrEpisode, UnableToParse, Error,
DecisionError, NoEpisodes, MissingAbsoluteEpisodeNumber, EpisodeAlreadyImported, TitleMissing, TitleTba, MinimumFreeSpace, FullSeason, NoAudio,
EpisodeUnexpected, EpisodeNotFoundInRelease, Sample, SampleIndeterminate, Unpacking, ExistingFileHasMoreEpisodes, SplitEpisode,
UnverifiedSceneMapping, NotQualityUpgrade, NotRevisionUpgrade, NotCustomFormatUpgrade, NotCustomFormatUpgradeAfterRename, MultiSeason`.
`ImportResultType { Imported, Rejected, Skipped }`, `ImportMode { Auto=0, Move=1, Copy=2 }`.

Extension lists (`Core/MediaFiles/MediaFileExtensions.cs`, `FileExtensions.cs`): video = `.webm .m4v .3gp .nsv .ty .strm .rm .rmvb .m3u .ifo .mov
.qt .divx .xvid .bivx .nrg .pva .wmv .asf .asx .ogm .ogv .m2v .avi .bin .dat .mpg .mpeg .mp4 .avc .vp3 .svq3 .nuv .viv .dv .fli .flv .wpl .img .iso
.vob .mkv .ts .wtv .m2ts`; archives = `.7z .bz2 .gz .r00 .rar .tar .tb2 .tbz2 .tgz`; dangerous = `.arj .lnk .lzh .ps1 .scr .vbs .zipx`;
executables = `.bat .cmd .exe .sh`; user-rejected from config `UserRejectedExtensions`.

### 4.2 ImportApprovedEpisodes

1. Approved decisions grouped per series, ordered by quality (profile order) desc then size desc; then iterate ordered by lowest episode number,
   size desc. A decision whose episodes were already imported in this batch → Skipped "Episode has already been imported".
2. Build `EpisodeFile` (size; `IndexerFlags` and `ReleaseType` from the latest `Grabbed` history `data.indexerFlags` / `data.releaseType`).
3. `copyOnly`: Auto → `downloadClientItem.CanMoveFiles == false`; Move → false; Copy → true.
4. New download: `OriginalFilePath` = path relative to client output dir; `UpgradeMediaFileService.UpgradeEpisodeFile`:
   - if existing files and the **root folder** (parent of series folder) doesn't exist → `RootFolderNotFoundException`
     ("Failed to import episode, Root folder missing.");
   - each existing file → `RecycleBinProvider.DeleteFile(path, subfolder relative to root)` then DB delete (reason `Upgrade`); old files recorded
     (`DeletedEpisodeFile {episodeFile, recycleBinPath}`) for notifications;
   - then `EpisodeFileMovingService.CopyEpisodeFile` or `MoveEpisodeFile`.
   Existing-file rescan (not new download): DB rows with same relative path deleted (`ManualOverride`).
5. `MediaFileService.Add`; extras imported (`ExtraService.ImportEpisode`) unless script import handled it; publish `EpisodeImportedEvent`
   (→ history `DownloadFolderImported`, notifications OnDownload/OnUpgrade, media server updates).
6. Errors: `DestinationAlreadyExistsException` → "Failed to import episode, Destination already exists." + queue `RescanSeries`;
   `RecycleBinException` → "…unable to move existing file to the Recycle Bin."; `UnauthorizedAccessException` → "…due to permission issues.";
   others → "Failed to import episode". Rejected decisions appended as `Rejected` results with their messages.

### 4.3 EpisodeFileMovingService / DiskTransferService

- Destination path from naming (`BuildFilePath(episodes, series, file, ext, null, customFormats)`).
- `EnsureEpisodeFolder`: best root folder for the series path must be configured (else `RootFolderNotFoundException` "is not a subdirectory of a defined
  root folder") and **exist on disk**; creates series / season / episode folders (recursively creating parents), applies folder permissions,
  publishes `EpisodeFolderCreatedEvent`.
- Copy: if `CopyUsingHardlinks` (default **true**) → `TransferMode.HardLinkOrCopy` (try hardlink; symlinked sources never hardlinked; failure (e.g. EXDEV)
  → warn "Hardlinking … failed, fallback to copy."); else `Copy`. Move: `TransferMode.Move`.
- `DiskTransferService.TransferFile`: resolves real parent paths; same path → error; case-only rename → move via `{src}.backup~`; same parent dir →
  verified move; destination inside source → error; existing destination → `DestinationAlreadyExistsException` (unless overwrite); copy = copy then
  size verification (retry once after 3 s; mismatch → rollback delete + IOException "data loss may have occurred"); move = `rename` + verify
  (rollback on failure); move onto **CIFS** target on a different mount → verified copy + delete source.
- Script import hook (`MediaFiles/ScriptImportDecider.cs`, config `UseScriptImport` + `ScriptImportPath`): runs `ScriptImportPath "<source>" "<dest>"`
  with `Sonarr_*` env vars (`Sonarr_SourcePath, Sonarr_DestinationPath, Sonarr_TransferMode, Sonarr_Series_*, Sonarr_EpisodeFile_*, Sonarr_Deleted*`…);
  non-zero exit → `ScriptImportException`; stdout lines parsed with `^(?:\[(MediaFile|ExtraFile)\]\s?<path>|\[PreventExtraImport\]|\[MoveStatus\]\s?(DeferMove|MoveComplete|RenameRequested))$`;
  `DeferMove` (default) → Sonarr performs the normal transfer; `MoveComplete` → script did it; `RenameRequested` → script moved it and Sonarr renames afterwards;
  `[MediaFile] path` overrides the resulting file path, `[ExtraFile]` lines list extra files to import.
- After transfer: file date per `FileDate` setting (`None`/`LocalAirDate`/`UtcAirDate`), Windows-only folder last-write time, then
  `SetFilePermissions`.
- Permissions (Linux/macOS, only when `SetPermissionsLinux`=true): `chmod` with `ChmodFolder` (default `755`) for folders; files get the same mask with all
  execute bits removed (e.g. 755 → 644); non-access bits preserved unless a 4-digit mask is given; `chown` group = `ChownGroup` (name or gid; empty = unchanged).
  Windows: inherit parent ACLs.

### 4.4 Recycle bin (`RecycleBinProvider.cs`)

- Config `RecycleBin` (path, empty = permanent delete), `RecycleBinCleanupDays` (default **7**, 0 = never auto-clean).
- `DeleteFile(path, subfolder)`: target `{bin}/{subfolder}/{name}`; collisions → `{name}_2.ext`, `_3`, …; move via verified transfer; set
  last-write time = now (age clock for cleanup). Failure → `RecycleBinException`.
- `DeleteFolder(path)` (series delete): move folder to `{bin}/{folderName}`, touch all files.
- `CleanUpRecycleBinCommand` (daily): delete files whose last-write + days < now, then remove empty subfolders. `Empty()` (API/UI) deletes everything.

### 4.5 Free-space & root-folder safety

| Check | Where | Rule |
|---|---|---|
| Grab-time free space | `Core/DecisionEngine/Specifications/FreeSpaceSpecification.cs` | free space at series path (5 s cache) − release size ≤ 0 → reject; < `MinimumFreeSpaceWhenImporting` (100 MB) → reject; skipped if `SkipFreeSpaceCheckWhenGrabbing`; unknown free space → accept |
| Import-time free space | import `FreeSpaceSpecification` | see 4.1 |
| Root folder exists | `UpgradeMediaFileService`, `EpisodeFileMovingService.EnsureEpisodeFolder` | throws `RootFolderNotFoundException` |
| Rescan | `DiskScanService.Scan` | missing series folder AND missing root folder → `SeriesScanSkippedEvent(RootFolderDoesNotExist)`; root folder empty → `RootFolderIsEmpty` skip (protects against unmounted drives deleting DB records) |

### 4.6 Extra files (`Core/Extras/*`)

- Only when config `ImportExtraFiles` (default false). Wanted extensions from `ExtraFileExtensions` (default `"srt"`, comma list, dots optional).
- Looks in the source file's folder (recursive when a folder parse exists). Managers tried in order: subtitles (`.aqt .ass .idx .jss .psb .rt .smi .srt
  .ssa .sub .txt .utf .utf8 .vtt`), metadata, other (anything else, e.g. `.nfo`). Matching: file name starts with the video file name, or parses to
  the same season/episode numbers; only one `.nfo` taken.
- New name = `{episode file name}{suffix}{ext}`; subtitles suffix = `[.{title}[ - {copy}]][.{copy}][.{iso639-1}][.{tags e.g. forced/sdh/cc}]`.
- Transfer: Move normally; when the import is copy-only → hardlink-or-copy (if hardlinks enabled) else copy; overwrite=true.
- Existing `.nfo` "other" files that conflict with metadata consumers are renamed with `-orig` suffix (`OtherExtraFileRenamer`).
- Metadata consumers (`Extras/Metadata/Consumers`): Kodi/XBMC (`XbmcMetadata`, nfo + images), Plex (`PlexMetadata`), Roksbox, WDTV.

### 4.7 MediaInfo (FFprobe) (`MediaFiles/MediaInfo/*`)

- Bundled ffprobe via FFMpegCore (`GlobalFFOptions.BinaryFolder = app dir`).
- `FFProbe.Analyse(file, "-probesize 50000000")`; if primary audio channel layout empty → retry with `-probesize 150000000 -analyzeduration 150000000`.
  HDR: if transfer is PQ-like → `FFProbe.GetFrames(file, "-read_intervals \"%+#1\" -select_streams v:{idx}")` to inspect side data (Dolby Vision/HDR10+).
- Stored model (`MediaInfoModel`, schema revision **14**; files below min revision 14 re-analysed on series scan): container, video format/codec id/profile/
  bitrate/bit depth/colour primaries/transfer/HDR format, width/height, fps, scan type (Interlaced when field order tt/bb/tb/bt), runtime (best of
  audio/video/format durations), audio streams `{language ("und"), format, codecId, profile, bitrate, channels, channelPositions, title}`, subtitle
  streams with language `{language, format, title, forced, hearingImpaired}`, raw stream data, title.
- Used for: sample detection (runtime), `HasAudioTrack` spec, quality/language aggregation, naming tokens, API `mediaInfo` resource.
- Config `EnableMediaInfo` (default true) controls analysis of existing library files; new imports are always analysed.

### 4.8 Rescan (`DiskScanService.Scan`, `RescanSeriesCommand`)

- Root-folder guard (4.5). Missing series folder: create it if `CreateEmptySeriesFolders` and not `DeleteEmptyFolders`; then clean DB files and finish.
- Video files = all files (recursive) with a video extension, filtered by:
  - `ExcludedSubFoldersRegex` `(?:\\|\/|^)(?:@eadir|\.@__thumb|plex versions|\.[^\\/]+)(?:\\|\/)` (ci) on the relative path (hidden dirs, Synology, Plex);
  - `ExcludedFilesRegex` `^\.(_|unmanic|DS_Store$)|^Thumbs\.db$` (ci) on file name;
  - extras: `ExcludedExtrasSubFolderRegex` `(?:\\|\/|^)(?:extras|extrafanart|behind the scenes|deleted scenes|featurettes|interviews|other|scenes|samples|shorts|trailers|theme[-_. ]music|backdrops)(?:\\|\/)`
    and `ExcludedExtraFilesRegex` `(-(trailer|other|behindthescenes|deleted|featurette|interview|scene|short)\.[^.]+$)`.
- `MediaFileTableCleanupService.Clean` removes DB rows for files no longer on disk; unmapped files → import decisions (non-scene source,
  `newDownload=false`) and import; changed sizes → media info refresh; `DeleteEmptyFolders` removes empty subfolders/series folder;
  non-video files passed to `SeriesScannedEvent` (extras/metadata import, media info update).
- Triggers: `RescanSeriesCommand {seriesId?}` (manual/API), after `RefreshSeries` according to `RescanAfterRefresh` (`Always` default / `AfterManual` /
  `Never`; new series always rescanned). Refresh-all only refreshes metadata for a series when `ShouldRefresh`: last sync > 30 days, an aired
  episode titled `TBA`, else not if synced < 6 h, else if not ended, or last episode aired < 30 days ago.

---

## 5. Scheduler & command queue

Files: `Core/Jobs/TaskManager.cs`, `Core/Jobs/Scheduler.cs`, `Core/Jobs/ScheduledTask.cs`, `Core/Messaging/Commands/*`,
`Core/ProgressMessaging/*`.

### 5.1 Default scheduled tasks (`TaskManager.Handle(ApplicationStartedEvent)`)

Interval in **minutes**; task identity = command type full name; `Priority` default `Low` unless noted. Name = class name minus `Command`.

| Command (name) | Interval | Priority | What it does |
|---|---|---|---|
| `RefreshMonitoredDownloads` | 1 | **High** | poll download clients, track/check downloads (§3) |
| `MessagingCleanup` | 5 | Low | `CommandQueueManager.CleanCommands()`: drop in-memory commands ended > 5 min ago; DB `Commands` rows ended > 1 day ago deleted (`CommandRepository.Trim`) |
| `ClearExpiredCache` | 5 | Low | evict expired in-memory cache entries |
| `ApplicationUpdateCheck` | 360 (6 h) | Low | check `services.sonarr.tv` for updates; if allowed (auto-update on or manual trigger, not docker/external mechanism, no major bump unless requested) queue `ApplicationUpdate` |
| `UpdateSceneMapping` | 180 (3 h) | Low | refresh scene-name/XEM mappings |
| `CheckHealth` | 360 (6 h) | Low | run all health checks (most checks also run on events / startup) |
| `RefreshSeries` | 720 (12 h) | Low | refresh metadata (Skyhook/TVDB) for series where `ShouldRefresh` (§4.8), then rescan per `RescanAfterRefresh` |
| `Housekeeping` | 1440 (24 h) | Low | ~30 housekeepers in `Core/Housekeeping/Housekeepers/` (orphan cleanup for episodes/files/history/blocklist/status rows/extra & metadata files/pending releases, unused tags, quality-profile format items, bad media covers, future-dated status times, trim log DB, temp update files, clean titles…) then `VACUUM` main DB. `IsExclusive` |
| `CleanUpRecycleBin` | 1440 | Low | recycle-bin age cleanup (§4.4) |
| `ImportListSync` | 5 | Low | sync all import lists whose own `MinRefreshInterval` has elapsed since `LastInfoSync` |
| `Backup` | `BackupInterval` days (config, default **7**, clamped 1..7) × 1440 | Low | zip `config.xml` + `sonarr.db` → `{BackupFolder (default "Backups" under AppData)}/scheduled/sonarr_backup_v{version}_{yyyy.MM.dd_HH.mm.ss}.zip`; delete backups older than `BackupRetention` days (default **28**) in that folder |
| `RssSync` | `RssSyncInterval` (default **15**; 1..9 → 10; <0 → 0; **0 = disabled**) | Low | §1.10 |

Notes:
- There is **no** scheduled `DownloadedEpisodesScan` task in this tree (the old "drone factory" folder scan is gone; only the external
  `DownloadedEpisodesScanCommand` API trigger remains).
- Task rows (`ScheduledTasks` table: `TypeName, Interval, LastExecution, LastStartTime, Priority`) are reconciled on startup: unknown rows deleted,
  intervals/priorities overwritten from defaults, new tasks get `LastExecution = now` (so they first run one interval after first start).
- `ConfigSavedEvent` recomputes RssSync and Backup intervals.
- `Scheduler` timer: every **30 s** pushes each task where `Interval > 0 && LastExecution + Interval < now` with `CommandTrigger.Scheduled`.
- On `CommandExecutedEvent`, if the command body's `UpdateScheduledTask` is true, `LastExecution = now`, `LastStartTime = command.StartedAt`.
  `UpdateScheduledTask` overrides: Backup only when scheduled; RefreshSeries only when `SeriesIds` empty; ImportListSync only when no `DefinitionId`.
- API `GET /api/v3/system/task` exposes them (§8).

### 5.2 Command model (`CommandModel` → table `Commands`)

| Field | Type / values |
|---|---|
| `id` | int |
| `name` | command name (class name without `Command`, e.g. `RssSync`, `RefreshSeries`, `DownloadedEpisodesScan`) |
| `body` | polymorphic JSON of the command (properties + base: `sendUpdatesToClient, updateScheduledTask, completionMessage, requiresDiskAccess, isExclusive, isLongRunning, name, lastExecutionTime, lastStartTime, trigger, suppressMessages, clientUserAgent`) |
| `priority` | `CommandPriority { Low=-1, Normal=0, High=1 }` |
| `status` | `CommandStatus { Queued=0, Started=1, Completed=2, Failed=3, Aborted=4, Cancelled=5, Orphaned=6 }` |
| `result` | `CommandResult { Unknown=0, Successful=1, Unsuccessful=2, Indeterminate=3 }` |
| `queuedAt, startedAt, endedAt` | UTC |
| `duration` | TimeSpan |
| `exception` | string (full exception) |
| `trigger` | `CommandTrigger { Unspecified=0, Manual=1, Scheduled=2 }` |
| `message` | last progress message / completion message / "Failed" |

Command flags (base `Command`): `SendUpdatesToClient` (SignalR broadcasts), `UpdateScheduledTask` (default true), `CompletionMessage`
(default null → last message), `RequiresDiskAccess`, `IsExclusive`, `IsLongRunning`.

| Command | Flags |
|---|---|
| RssSync | SendUpdatesToClient, IsLongRunning |
| RefreshSeries `{seriesId?, seriesIds[], isNewSeries}` | SendUpdates, IsLongRunning, CompletionMessage "Completed" |
| ProcessMonitoredDownloads | RequiresDiskAccess, IsLongRunning |
| DownloadedEpisodesScan `{path, downloadClientId, importMode}` | RequiresDiskAccess, IsLongRunning |
| ManualImport, RenameFiles, RenameSeries, DeleteSeriesFiles, MoveSeries, BulkMoveSeries | SendUpdates, RequiresDiskAccess |
| Housekeeping | IsExclusive |
| ApplicationUpdate | SendUpdates, IsExclusive |
| Backup, ClearBlocklist, ResetApiKey, ImportListSync, EpisodeSearch, SeasonSearch, SeriesSearch, MissingEpisodeSearch, CutoffUnmetEpisodeSearch, RescanSeries, ClearLog, DeleteLogFiles, DeleteUpdateLogFiles, ApplicationUpdateCheck | SendUpdates |
| RefreshMonitoredDownloads, CheckForFinishedDownload, CheckHealth, ClearExpiredCache, MessagingCleanup, UpdateSceneMapping, CleanUpRecycleBin, ResetQualityDefinitions, Restart, Shutdown | (none) |

### 5.3 Queue semantics (`CommandQueueManager.cs`, `CommandQueue.cs`, `CommandExecutor.cs`)

- **Dedup on push**: if a Queued/Started command with the same name and equal body exists → return the existing one (no new row).
  Equality (`CommandEqualityComparer`) compares public properties declared on the subclass (not base `Command`), collections as sets.
  Quirk: the comparer returns `true` as soon as it meets a property that is null on both sides (early return).
- Push inserts a DB row (status Queued) and adds to the in-memory queue. `PushMany` likewise (priority Normal).
- **3 worker threads** (`THREAD_LIMIT = 3`). Selection (`CommandQueue.TryGet`), under a lock:
  - candidates = Queued commands; if any started command `RequiresDiskAccess` → exclude candidates that require disk access;
    else if any started command `IsLongRunning` → exclude `IsExclusive` candidates;
  - order by `priority` desc, then `queuedAt` asc;
  - if any started command is exclusive → wait; if the chosen candidate is exclusive and anything is running → wait.
- Execution: mark Started (DB), broadcast (if SendUpdatesToClient), run `IExecute<T>.Execute`; success → Completed (result Successful unless reported),
  message = `CompletionMessage ?? last message`; exception → message "Failed", status Failed, `exception` stored. Always broadcast + `CommandExecutedEvent`.
- `ICommandResultReporter.Report(Unsuccessful|Indeterminate)` lets a handler mark a completed command as not fully successful.
- Progress messages: `logger.ProgressInfo/ProgressDebug/ProgressTrace(...)` (log events with a `Status` property) while a command with
  `SendUpdatesToClient` runs set `command.message` and publish `CommandUpdatedEvent` → SignalR `command` message (§9.6).
- Cancel (`DELETE /api/v3/command/{id}`): only removes if still Queued; otherwise **409 Conflict** "Unable to cancel task".
- Startup: rows with status Started → `Orphaned` (EndedAt=now); rows still Queued are re-queued.
- `CommandTrigger`: API-created commands are `Manual`; scheduler `Scheduled`; internal `Unspecified`.

---

## 6. Notifications ("Connections")

Files: `Core/Notifications/INotification.cs`, `NotificationBase.cs`, `NotificationDefinition.cs`, `NotificationService.cs`,
`NotificationStatusService.cs`, `Core/Notifications/Webhook/*`, provider folders.

### 6.1 Event set / definition flags

Definition (API resource) booleans: `onGrab, onDownload, onUpgrade, onImportComplete, onRename, onSeriesAdd, onSeriesDelete,
onEpisodeFileDelete, onEpisodeFileDeleteForUpgrade, onHealthIssue, includeHealthWarnings, onHealthRestored, onApplicationUpdate,
onManualInteractionRequired` + read-only `supportsOn*` for each (computed by whether the provider overrides the method;
`supportsOnUpgrade = supportsOnDownload`, `supportsOnEpisodeFileDeleteForUpgrade = supportsOnEpisodeFileDelete`). `tags` filter series.

| Trigger (internal event) | Method | Rules |
|---|---|---|
| `EpisodeGrabbedEvent` | `OnGrab(GrabMessage)` | series tag filter |
| `EpisodeImportedEvent` (only `NewDownload`) | `OnDownload(DownloadMessage)` | if the import replaced files (`OldFiles` non-empty) only sent when `onUpgrade` |
| `DownloadCompletedEvent` (tracked download fully imported) and `UntrackedDownloadCompletedEvent` | `OnImportComplete(ImportCompleteMessage)` | once per download (all files) |
| `SeriesRenamedEvent` | `OnRename(series, renamedFiles)` | |
| `SeriesAddCompletedEvent` | `OnSeriesAdd` | |
| `SeriesDeletedEvent` | `OnSeriesDelete` (per series; `deletedFiles` flag) | |
| `EpisodeFileDeletedEvent` | `OnEpisodeFileDelete` | skipped for reason `Upgrade` unless `onEpisodeFileDeleteForUpgrade`; skipped if the file has no episodes |
| `HealthCheckFailedEvent` | `OnHealthIssue` | not during startup grace period; `Error` always, `Warning` only if `includeHealthWarnings`; no tag filter |
| `HealthCheckRestoredEvent` | `OnHealthRestored(previousCheck)` | same level rules |
| `UpdateInstalledEvent` | `OnApplicationUpdate` | message `"Sonarr updated from {prev} to {new}"` |
| `ManualInteractionRequiredEvent` | `OnManualInteractionRequired` | (import blocked; see §3.4) |
| `DeleteCompletedEvent`, `DownloadsProcessedEvent`, `RenameCompletedEvent`, `HealthCheckCompleteEvent` | `ProcessQueue()` | flushes batched media-server updates (Plex, Emby/Jellyfin, Kodi…) |

Every send → `NotificationStatusService.RecordSuccess/RecordFailure` (backoff max level 5 = 1 h, 5 min initial grace — §1.9).
Message text (`NotificationService.GetMessage`): `"{Series} - {S}x{EE}[x{EE}…] - {Title1 + Title2} [{Quality}]"`; daily
`"{Series} - {AirDate} - {Title} [{Quality}]"`; multi-season `"{Series} - {seasons} [{Quality}]"`; quality suffix `" Proper"` (or `" v{n}"` for anime) when revision > 1.

### 6.2 Implementations (`implementation` / display name / supported events)

| Class | Name | Grab | Download/Upgrade | ImportComplete | Rename | EpFileDelete | SeriesAdd | SeriesDelete | Health/Restored | AppUpdate | ManualInteraction |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `Apprise` | Apprise | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `CustomScript` | Custom Script | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Discord` | Discord | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Email` | Email | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Gotify` | Gotify | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Join` | Join | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `MailGun` | Mailgun | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `MediaBrowser` | Emby / Jellyfin | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – |
| `Notifiarr` | Notifiarr | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Ntfy` | ntfy.sh | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `PlexServer` | Plex Media Server | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ (only if files deleted) | – | – | – |
| `Prowl` | Prowl | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `PushBullet` | Pushbullet | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Pushcut` | Pushcut | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Pushover` | Pushover | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `SendGrid` | SendGrid | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Signal` | Signal | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Simplepush` | Simplepush | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Slack` | Slack | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `SynologyIndexer` | Synology Indexer | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – | – | – |
| `Telegram` | Telegram | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Trakt` | Trakt | – | ✓ | ✓ | – | ✓ | ✓ | ✓ | – | – | – |
| `Twitter` | Twitter | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Webhook` | Webhook | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `Xbmc` | Kodi | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |

### 6.3 Webhook (`Core/Notifications/Webhook/*`)

Settings: `url`, `method` (`1=POST` default, `2=PUT`), `username`, `password` (→ pre-emptive Basic auth when either set), `headers`
(key/value list added verbatim). Request: `Content-Type: application/json`, `Accept: application/json`, body = Newtonsoft JSON with:
**camelCase property names, null values omitted**, enums as **camelCase strings** (except `eventType`, which is PascalCase), dates ISO-8601 UTC
(Newtonsoft round-trip format, fractional seconds kept, e.g. `2024-05-01T12:00:00.123Z`), pretty-printed (indented),
`Language` serialized as object `{"id": n, "name": "English"}`, `Version` as string. Non-2xx → test failure / RecordFailure.

`eventType` values (`WebhookEventType`, PascalCase): `Test, Grab, Download, Rename, SeriesAdd, SeriesDelete, EpisodeFileDelete, Health,
ApplicationUpdate, HealthRestored, ManualInteractionRequired`. **Note:** On Import Complete payloads also use `"eventType": "Download"`
(distinguishable by `episodeFiles`/`fileCount`).

Common to all payloads: `eventType`, `instanceName` (config.xml `InstanceName`, default `"Sonarr"`), `applicationUrl` (config `ApplicationUrl`;
not set — hence omitted — for Health/HealthRestored/ApplicationUpdate payloads; otherwise emitted, as `""` when unconfigured since only nulls are dropped).

Shared objects:

| Object | Fields |
|---|---|
| `series` (`WebhookSeries`) | `id, title, titleSlug, path, tvdbId, tvMazeId, tmdbId, imdbId, malIds[], aniListIds[], type` (`standard\|daily\|anime`), `year, genres[], images[], tags[]` (tag labels, sorted), `originalLanguage {id,name}, originalCountry` |
| `images[]` | `coverType` (`unknown\|poster\|banner\|fanart\|screenshot\|headshot\|clearlogo`), `url` (local `/MediaCover/...` URL), `remoteUrl` |
| `episodes[]` (`WebhookEpisode`) | `id, episodeNumber, seasonNumber, title, overview, airDate ("yyyy-MM-dd"), airDateUtc, seriesId, tvdbId, finaleType` |
| `episodeFile` (`WebhookEpisodeFile`) | `id, relativePath, path, quality (name), qualityVersion, releaseGroup, sceneName, size, dateAdded, languages[{id,name}], mediaInfo, sourcePath, recycleBinPath` |
| `mediaInfo` | `audioChannels (decimal e.g. 5.1), audioCodec, audioLanguages[], height, width, subtitles[], videoCodec, videoDynamicRange, videoDynamicRangeType` |
| `release` (grab, `WebhookRelease`) | `quality, qualityVersion, releaseGroup, releaseTitle, indexer, size, customFormatScore, customFormats[] (names), languages[], indexerFlags[] (names)` |
| `release` (import/manual, `WebhookGrabbedRelease`) | `releaseTitle, indexer, size, indexerFlags[], releaseType` (`unknown\|singleEpisode\|multiEpisode\|seasonPack`) |
| `customFormatInfo` | `customFormats: [{id, name}], customFormatScore` |
| `downloadInfo` (`WebhookDownloadClientItem`) | `quality, qualityVersion, title, indexer (never set → omitted), size` |
| `downloadStatusMessages[]` | `{title, messages[]}` |
| `renamedEpisodeFiles[]` | episodeFile fields + `previousRelativePath, previousPath` |

Payloads per event:

| eventType | Payload class | Fields (besides common) |
|---|---|---|
| `Grab` | `WebhookGrabPayload` | `series, episodes[], release, downloadClient (definition name), downloadClientType (client type name), downloadId, customFormatInfo` |
| `Download` (OnDownload/OnUpgrade) | `WebhookImportPayload` | `series, episodes[], episodeFile (+sourcePath), isUpgrade, downloadClient, downloadClientType, downloadId, deletedFiles[] (episodeFile objects with path & recycleBinPath; only when upgrade), customFormatInfo, release` |
| `Download` (OnImportComplete) | `WebhookImportCompletePayload` | `series, episodes[], episodeFiles[], downloadClient, downloadClientType, downloadId, release, fileCount, sourcePath, destinationPath` |
| `Rename` | `WebhookRenamePayload` | `series, renamedEpisodeFiles[]` |
| `SeriesAdd` | `WebhookSeriesAddPayload` | `series` |
| `SeriesDelete` | `WebhookSeriesDeletePayload` | `series, deletedFiles (bool)` |
| `EpisodeFileDelete` | `WebhookEpisodeDeletePayload` | `series, episodes[], episodeFile, deleteReason` (`missingFromDisk\|manual\|upgrade\|noLinkedEpisodes\|manualOverride`) |
| `Health` / `HealthRestored` | `WebhookHealthPayload` | `level` (`ok\|notice\|warning\|error`), `message, type` (health check class name, e.g. `IndexerStatusCheck`), `wikiUrl` |
| `ApplicationUpdate` | `WebhookApplicationUpdatePayload` | `message, previousVersion, newVersion` |
| `ManualInteractionRequired` | `WebhookManualInteractionPayload` | `series, episodes[], downloadInfo, downloadClient, downloadClientType, downloadId, downloadStatus` (PascalCase string `Ok`/`Warning`/`Error`), `downloadStatusMessages[], customFormatInfo, release` |
| `Test` | `WebhookGrabPayload` | `series {id:1, title:"Test Title", path:"C:\\testpath", tvdbId:1234, tags:["test-tag"]}`, `episodes [{id:123, episodeNumber:1, seasonNumber:1, title:"Test title"}]` |

(`downloadStatus` is a string set with `TrackedDownloadStatus.ToString()` → `"Ok"|"Warning"|"Error"`.)
Notifiarr also builds on `WebhookBase` (same payload builders) and POSTs them to `https://notifiarr.com/api/v1/notification/sonarr` with header `X-API-Key: {apiKey}` (`Core/Notifications/Notifiarr/NotifiarrProxy.cs`).

Custom Script passes event data as environment variables prefixed `Sonarr_` (e.g. `Sonarr_EventType`, `Sonarr_Series_Id`, `Sonarr_EpisodeFile_Path`,
`Sonarr_Download_Id`, `Sonarr_Release_Title`, `Sonarr_Health_Issue_Message`, …; full list in `Core/Notifications/CustomScript/CustomScript.cs`).

### 6.4 Request basics of common providers

| Provider | Request |
|---|---|
| Discord | `POST {webHookUrl}` JSON `{content, username, avatar_url, embeds:[{description, title, text, color, url, author{name, icon_url}, thumbnail{url}, image{url}, timestamp, fields:[{name, value, inline}]}]}`; settings `webHookUrl, username, avatar, author, grabFields[], importFields[], manualInteractionFields[]` (field enums in `DiscordFieldType.cs`) |
| Telegram | `POST https://api.telegram.org/bot{botToken}/sendmessage` JSON `{chat_id, parse_mode:"HTML", text:"<b>{title}</b>\n{message}\n<a href=…>links</a>", disable_notification, message_thread_id (topicId), link_preview_options:{is_disabled, url}}`; settings `botToken, chatId, topicId, sendSilently, includeAppNameInTitle, includeInstanceNameInTitle, metadataLinks[], linkPreview` |
| Pushover | `POST https://api.pushover.net/1/messages.json` form: `token, user, device (comma list), title, message, priority (-2..2), [encrypted=1 + AES-encrypted title/message when encryptionKey], [retry, expire when priority=2], [ttl>0], [sound]` |
| Email | SMTP via MailKit: `server, port, useEncryption (0 Preferred=Auto/STARTTLS when available, 1 Always = SSL-on-connect if port 465 else STARTTLS, 2 Never), username, password, from, to[], cc[], bcc[]`; plain-text body (HTML variant exists) |
| ntfy | for each topic: `POST {serverUrl or https://ntfy.sh}/{topic}?title=…&message=…&priority=N[&tags=a,b][&click=url]` (params in **query string**, empty body); auth `Authorization: Bearer {accessToken}` or Basic user/pass; custom headers |
| Gotify | settings `server, appToken, priority, includeSeriesPoster, metadataLinks[], preferredMetadataLink`; `POST {server}/message?token={appToken}` JSON `{title, message, priority, extras:{"client::display":{contentType:"text/markdown"\|"text/plain"}, "client::notification":{bigImageUrl, click:{url}}}}` |

### 6.5 Media server library updates

**Plex Media Server** (`Plex/Server/*`): all requests GET to `{http(s)}://{host}:{port}{urlBase}/…` with `Accept: application/json` and query params
`X-Plex-Client-Identifier={config PlexClientIdentifier GUID}`, `X-Plex-Product=Sonarr`, `X-Plex-Platform=Windows`, `X-Plex-Platform-Version=7`,
`X-Plex-Device-Name=Sonarr`, `X-Plex-Version={version}`, `X-Plex-Token={authToken}`.
- `GET /identity` → version (cached 2 h; PMS 1.3.0 rejected).
- `GET /library/sections` → sections with `type == "show"` and their `Location[].path` (legacy `_children` format also handled).
- `GET /library/sections/{id}/refresh?path={location}/{seriesRelativePath}` — partial scan; picks section locations equal to the
  (optionally `mapFrom`→`mapTo` remapped) root folder; if none match, refreshes the series path under **every** TV section location.
- Updates are queued per host and flushed on `ProcessQueue` (§6.1). Plex.tv OAuth (`PlexTvService`) is used to obtain the token; `Ping(token)` before queuing.

**Emby / Jellyfin** (`MediaBrowser/*`): base `{http(s)}://{host}:{port}{urlBase}`; headers `X-MediaBrowser-Token: {apiKey}` and
`Authorization: MediaBrowser Token="{apiKey}"`.
- Test: `GET /System/Configuration`.
- Notify (if `notify`): `POST /Notifications/Admin` JSON `{Name, Description, ImageUrl}`.
- Find series paths: `GET /Items?recursive=true&includeItemTypes=Series&fields=Path,ProviderIds&years={year}` — match by ProviderIds
  (Tvdb, Imdb, TvMaze, Tmdb, TvRage) else exact name.
- Update: `POST /Library/Media/Updated` JSON `{"Updates":[{"Path": path, "UpdateType": "Created"|"Modified"|"Deleted"|null}]}` for each matched path
  plus the (mapFrom→mapTo remapped) Sonarr series path. Created = download/import/series add; Modified = rename; Deleted = file/series delete;
  null when several update types queued for the series.

---

## 7. Health checks

Files: `Core/HealthCheck/*`, `Core/HealthCheck/Checks/*`.

Model (`HealthCheck.cs`): `source` (check class), `type` = `HealthCheckResult { Ok=0, Notice=1, Warning=2, Error=3 }`, `reason`
(`HealthCheckReason` enum), `message`, `wikiUrl` = `https://wiki.servarr.com/sonarr/system#<fragment>`.
Engine (`HealthCheckService.cs`): all checks run on startup (`CheckOnStartup` default true) and on the 6-hourly `CheckHealth` command
(`CheckOnSchedule` default true); `[CheckOn(EventType[, condition])]` re-runs a check (5 s debounce) when that event fires
(`FailedOnly`/`SuccessfulOnly` = only if the check is currently failing / passing). **15-minute startup grace period**: Failed/Restored events
during it are flagged and notifications suppressed. Changes publish `HealthCheckFailedEvent` / `HealthCheckRestoredEvent` / `HealthCheckCompleteEvent`
(→ SignalR `health` broadcast). `ServerSideNotificationService` adds the first message from `GET https://services.sonarr.tv/v1/notification?version=&os=&arch=&branch=` (`Core/HealthCheck/ServerSideNotificationService.cs`).

| Check class | Result | Condition |
|---|---|---|
| `AllowedHostsCheck` | Warning | no explicit allowed hosts configured (config `AllowedHosts` empty or contains `*`) **and** `AuthenticationRequired != Enabled` (runs on start & config.xml save) |
| `ApiKeyValidationCheck` | Warning | API key shorter than **20** chars |
| `AppDataLocationCheck` | Warning | AppData folder is inside / equal to the startup (binary) folder |
| `DownloadClientCheck` | Warning / Error | no enabled download client (Warning); any client's `GetItems()` throws (Error "Unable to communicate with {client}") |
| `DownloadClientRemovesCompletedDownloadsCheck` | Warning | a client's `GetStatus().RemovesCompletedDownloads` (qBit ratio-remove, SAB history retention…) |
| `DownloadClientRootFolderCheck` | Warning | a client output root folder equals/is inside a Sonarr root folder ("downloads in root folder") |
| `DownloadClientSortingCheck` | Warning | client reports a `SortingMode` (SABnzbd TV/Movie/Date sorting for the category) |
| `DownloadClientStatusCheck` | Warning / Error | some / all enabled clients in backoff (`DisabledTill > now`) |
| `ImportListRootFolderCheck` | Error | import list root folder empty/invalid/not a configured root folder/missing on disk |
| `ImportListStatusCheck` | Warning / Error | some / all import lists in backoff |
| `ImportMechanismCheck` | Warning | `EnableCompletedDownloadHandling` never explicitly set (migration helper) — messages vary for remote / SABnzbd-only / NZBGet-only clients |
| `IndexerDownloadClientCheck` | Warning | enabled indexer points to a `downloadClientId` that isn't an enabled client |
| `IndexerJackettAllCheck` | Warning | enabled Torznab indexer uses Jackett `/torznab/all/api` or `/api/v2.0/indexers/all/results/torznab` |
| `IndexerLongTermStatusCheck` | Warning / Error | indexers in backoff whose `initialFailure` is older than **6 h** (some / all) |
| `IndexerStatusCheck` | Warning / Error | indexers in backoff whose `initialFailure` is within the last 6 h (some / all) |
| `IndexerRssCheck` | Error / Warning | no RSS-enabled indexers (Error); all RSS indexers temporarily blocked (Warning) |
| `IndexerSearchCheck` | Warning | no automatic-search indexers; no interactive-search indexers; all automatic-search indexers blocked |
| `MountCheck` | Error | a series path lives on a read-only mount |
| `NotificationStatusCheck` | Warning / Error | some / all notifications in backoff |
| `OidcConfigurationCheck` | Error | auth method `Oidc` and settings missing, authority not HTTPS ("not secure"), or discovery document unreachable |
| `PackageGlobalMessageCheck` | Notice/Warning/Error | package_info `PackageGlobalMessage` set (prefix `Error:`/`Warn:` sets level) |
| `PasswordResetTokenCheck` | Warning / Notice | a password-reset token is configured: shorter than minimum → Warning, else Notice (token active) |
| `ProxyCheck` | Error | proxy enabled and hostname doesn't resolve, or a request through the proxy to `https://services.sonarr.tv/v1/ping` returns 400 or throws |
| `RecyclingBinCheck` | Error | recycle bin configured but not writable (checked after failed imports) |
| `RemotePathMappingCheck` | Error | (CDH on) client output folder is not a valid local path (wrong OS / docker path / missing local folder / permissions) — with ~15 specific messages; also evaluated on `EpisodeImportFailedEvent` for the failed path |
| `RemovedSeriesCheck` | Error | series whose TVDB status is `deleted` (removed from TheTVDB) |
| `RootFolderCheck` | Error / Warning | root folder of any series missing or invalid (Error); root folder exists but is empty (Warning) |
| `SystemTimeCheck` | Error | system clock differs from `https://services.sonarr.tv/v1/time` (`dateTimeUtc`) by ≥ 1 day (request errors ignored) |
| `UpdateCheck` | Error | auto-update with built-in mechanism (not docker) but app is macOS-translocated, or startup folder / `UI` folder not writable |

---

## 8. HTTP API v3

Files: `Sonarr.Api.V3/**` (controllers + resources), `Sonarr.Http/**` (REST base, auth, middleware, ClientSchema, frontend),
`NzbDrone.Host/Startup.cs`.

### 8.0 Conventions

- Base path: `{UrlBase}/api/v3/…` (`[V3ApiController("resource")]` → `api/v3/{resource}`; default resource = controller name lower-cased).
  Routing is **case-insensitive** and generated URLs are lower-case (`AddRouting(LowercaseUrls = true)`).
- JSON via System.Text.Json (`Common/Serializer/System.Text.Json/STJson.cs`): **camelCase** properties (STJ naming, so `UILanguage` →
  `uiLanguage`, `SQLite` → `sqLite`), **enums as camelCase strings** (integers also accepted on input), **nulls omitted**, case-insensitive
  property matching on input, trailing commas allowed, `TimeSpan` via `TimeSpan.ToString()` (`"hh:mm:ss"`, `"d.hh:mm:ss"`, fractional `".fffffff"` when present),
  `Version` as string, `DateTime` always written as UTC **without fractional seconds**: `"yyyy-MM-ddTHH:mm:ssZ"` (`STJUtcConverter`; parsed with `DateTime.Parse` → UTC).
  `RestResource.id` is omitted when 0 (except resources that override with `JsonIgnore(Never)`: customformat, autotagging, language).
- Every `/api/*` response carries header **`X-Application-Version: {version}`** (`Sonarr.Http/Middleware/VersionMiddleware.cs`).
- During startup every request gets **503** (`{"errorMessage":"Sonarr is starting up, please try again later"}` for API).
- If UrlBase is set and the request lacks it → **307** redirect to `{urlBase}{path}{query}`.
- `ReturnHttpNotAcceptable = true` (unsupported `Accept` → 406).
- POST returns **201 Created** with the full resource (Location → `GET …/{id}`); PUT returns **202 Accepted** with the resource;
  DELETE returns 200 (empty or `{}`).
- Errors (`Sonarr.Http/ErrorManagement/SonarrErrorPipeline.cs`): FluentValidation failure → **400** with a JSON **array** of
  `{propertyName, errorMessage, attemptedValue, severity, errorCode, …}` (FluentValidation `ValidationFailure`, plus Sonarr's
  `isWarning`, `infoLink`, `detailedDescription` for `NzbDroneValidationFailure`); `ModelNotFoundException` → 404; `ModelConflictException` /
  SQLite "constraint failed" on POST/PUT → 409; `InvalidOperationException` → 400; `NzbDroneClientException`/`ApiException` → their status; other → 500.
  Non-validation error body: `{"message": "...", "description": "<exception text>", "content": …}`.
- PUT `{id}`: if body `id` is 0 the route id is used. Deprecated endpoints/controllers respond with header `Deprecation: true`.
- Paging (`Sonarr.Http/PagingResource.cs`): query `page` (default 1), `pageSize` (default 10), `sortKey`, `sortDirection`
  (`default|ascending|descending`); response `{page, pageSize, sortKey, sortDirection, totalRecords, records[]}`.

### 8.1 Non-API routes

| Method | Path | Auth | Notes |
|---|---|---|---|
| GET, HEAD | `/ping` | anonymous | `{"status":"OK"}`; 500 `{"status":"Error"}` if DB unreachable (`Sonarr.Http/Ping/PingController.cs`) |
| GET | `/api` | API | `{"current":"v5","deprecated":["v3"]}` in this tree (`Sonarr.Http/ApiInfoController.cs`); released v4 is believed to answer `{"current":"v3","deprecated":[]}` (**UNVERIFIED**) |
| GET | `/initialize.json` | UI policy | `{apiRoot:"{urlBase}/api/v3", apiKey, release, version, instanceName, theme, branch, analytics, userHash, urlBase, isProduction}` — this is how the web UI obtains the **API key** |
| GET | `/login` | anonymous | login page HTML |
| POST | `/login?returnUrl=` | anonymous | form fields `username`, `password`, `rememberMe`; success → 302 to returnUrl + Forms cookie; failure → 302 `~/login?returnUrl=…&loginFailed=true[&lockoutUntil={unixms}]` |
| GET | `/login/sso?returnUrl=&rememberMe=` | anonymous | OIDC challenge (only when auth method = Oidc) |
| GET | `/logout` | anonymous | clears cookies, 302 to `{urlBase}/` (OIDC: provider sign-out → `/loggedout`) |
| GET | `/loggedout` | anonymous | page |
| POST | `/reset-password/request`, `/reset-password` | anonymous | password reset with token (form `token, username, password, passwordConfirmation`) |
| GET | `/feed/v3/calendar/Sonarr.ics` | API key (`?apikey=`) | iCal; query `pastDays=7`, `futureDays=28`, `tags` (comma list of tag labels), `unmonitored=false`, `premieresOnly=false`, `asAllDay=false`; UID `NzbDrone_episode_{id}`, summary `"{Series} - {S}x{EE:00} - {Title}"` (daily: `"{Series} - {Title}"`), categories = network; `text/calendar`; 204 if nothing |
| WS/HTTP | `/signalr/messages` | `SignalR` scheme (API key in `X-Api-Key` or `?access_token=`) | SignalR hub (§9.6) |
| GET | `/MediaCover/{seriesId}/{file}`, `/MediaCoverProxy/…`, `/content/*`, `/favicon.ico`, `/robots.txt`, `/backup/{type}/{file}.zip`, `/logfile/*.txt`, `/updatelogfile/*.txt` | UI | static mappers (`Sonarr.Http/Frontend/Mappers/*`) |
| GET | any other non-`api/`, non-`feed/` path | UI | SPA `index.html` |
| GET | `/docs/{v3\|v5}/openapi.json` | — | **debug builds only** |

### 8.2 `/api/v3` route list

Legend: `{id}` = `{id:int}`; RestController-derived controllers expose `GET …/{id}`.

| Resource | Routes |
|---|---|
| `autotagging` | GET, GET `/{id}`, POST, PUT `/{id}`, DELETE `/{id}`, GET `/schema` |
| `blocklist` | GET (paged; `seriesIds[]`, `protocols[]`), DELETE `/{id}`, DELETE `/bulk` `{ids[]}` |
| `calendar` | GET (`start`, `end` (default today..+2d), `unmonitored`, `includeSeries`, `includeEpisodeFile`, `includeEpisodeImages`, `tags`), GET `/{id}` |
| `command` | GET (all in-memory commands), GET `/{id}`, POST `{name, …command props}`, DELETE `/{id}` (cancel queued) |
| `config/downloadclient`, `config/importlist`, `config/indexer`, `config/mediamanagement`, `config/ui` | GET, GET `/{id}`, PUT `/{id}` (id is always 1) |
| `config/host` | GET, GET `/{id}`, PUT `/{id}` |
| `config/naming` | GET, GET `/{id}`, PUT `/{id}`, GET `/examples` (query = naming resource fields) |
| `customfilter` | GET, GET `/{id}`, POST, PUT `/{id}`, DELETE `/{id}` |
| `customformat` | GET, GET `/{id}`, POST, PUT `/{id}`, PUT `/bulk`, DELETE `/{id}`, DELETE `/bulk`, GET `/schema` |
| `delayprofile` | GET, GET `/{id}`, POST, PUT `/{id}`, DELETE `/{id}`, PUT `/reorder/{id}?after={id}` |
| `diskspace` | GET |
| `downloadclient`, `indexer`, `notification`, `importlist`, `metadata` (provider controllers) | GET, GET `/{id}`, POST `?forceSave=`, PUT `/{id}?forceSave=`, PUT `/bulk`, DELETE `/{id}`, DELETE `/bulk`, GET `/schema`, POST `/test?forceTest=`, POST `/testall`, POST `/action/{name}` |
| `episode` | GET (`seriesId`[+`seasonNumber`] or `episodeIds[]` or `episodeFileId`; `includeSeries`, `includeEpisodeFile`, `includeImages`; else 400 "seriesId or episodeIds must be provided"), GET `/{id}`, PUT `/{id}` (monitored), PUT `/monitor` `{episodeIds[], monitored}` |
| `episodefile` | GET (`seriesId` or `episodeFileIds[]`), GET `/{id}`, PUT `/{id}`, PUT `/editor` `{episodeFileIds, quality, languages, …}`, PUT `/bulk` `[resources]`, DELETE `/{id}`, DELETE `/bulk` `{episodeFileIds[]}` |
| `filesystem` | GET (`path`, `includeFiles`, `allowFoldersWithoutTrailingSlashes`), GET `/type?path=`, GET `/mediafiles?path=` |
| `health` | GET |
| `history` | GET (paged; `includeSeries`, `includeEpisode`, `eventType[]` (int), `episodeId`, `downloadId`, `seriesIds[]`, `languages[]`, `quality[]`; sort keys `date`, `series.sortTitle`), GET `/since?date=&eventType=`, GET `/series?seriesId=&seasonNumber=&eventType=`, POST `/failed/{id}` |
| `importlistexclusion` | GET, GET `/paged`, GET `/{id}`, POST, PUT `/{id}`, DELETE `/{id}`, DELETE `/bulk` |
| `indexerflag` | GET |
| `language` | GET, GET `/{id}` |
| `languageprofile` (deprecated) | GET, GET `/{id}`, POST, PUT `/{id}`, DELETE `/{id}`; `languageprofile/schema` GET |
| `localization` | GET, GET `/language` |
| `log` | GET (paged; `level`) ; `log/file` GET, GET `/{filename}.txt` ; `log/file/update` GET, GET `/{filename}.txt` |
| `manualimport` | GET (`folder`, `downloadId`, `seriesId`, `seasonNumber`, `filterExistingFiles=true`), POST (reprocess items) |
| `mediacover` | GET `/{seriesId}/{filename}.(jpg\|png\|gif)` |
| `parse` | GET `?title=&path=` |
| `qualitydefinition` | GET, GET `/{id}`, PUT `/{id}`, PUT `/update` (bulk), GET `/limits` → `{min, max}` |
| `qualityprofile` | GET, GET `/{id}`, POST, PUT `/{id}`, DELETE `/{id}`; `qualityprofile/schema` GET |
| `queue` | GET (paged; `includeUnknownSeriesItems`, `includeSeries`, `includeEpisode`, `seriesIds[]`, `protocol`, `languages[]`, `quality[]`, `status[]`), GET `/{id}`, DELETE `/{id}?removeFromClient=true&blocklist=false&skipRedownload=false&changeCategory=false`, DELETE `/bulk` `{ids[]}` (+same query), POST `/grab/{id}`, POST `/grab/bulk` |
| `queue/details` | GET (`seriesId`, `episodeIds[]`, `includeSeries`, `includeEpisode`), GET `/{id}` |
| `queue/status` | GET |
| `release` | GET (`seriesId`, `episodeId`, `seasonNumber` → interactive search), POST `{guid, indexerId, …}` (grab) |
| `release/push` | POST (ReleaseResource: `title, downloadUrl/magnetUrl, protocol, publishDate, indexer, …`) |
| `releaseprofile` | GET, GET `/{id}`, POST, PUT `/{id}`, DELETE `/{id}` |
| `remotepathmapping` | GET, GET `/{id}`, POST, PUT `/{id}`, DELETE `/{id}` |
| `rename` | GET `?seriesId=&seasonNumber=`, GET `/bulk?seriesIds=` |
| `rootfolder` | GET, GET `/{id}`, POST `{path}`, DELETE `/{id}` |
| `seasonpass` | POST |
| `series` | GET (`tvdbId`, `includeSeasonImages`), GET `/{id}?includeSeasonImages=`, POST, PUT `/{id}?moveFiles=`, DELETE `/{id}?deleteFiles=&addImportListExclusion=`, GET `/{id}/folder` |
| `series/editor` | PUT, DELETE |
| `series/import` | POST `[series]` |
| `series/lookup` | GET `?term=&language=` (`tvdb:12345` / `tvdbid:12345` prefixes look up by TVDB id — `Core/MetadataSource/SkyHook/SkyHookProxy.cs`) |
| `system` | GET `/status`, GET `/routes`, GET `/routes/duplicate`, POST `/shutdown`, POST `/restart` |
| `system/backup` | GET, DELETE `/{id}`, POST `/restore/{id}`, POST `/restore/upload` (multipart) |
| `system/task` | GET, GET `/{id}` |
| `tag` | GET, GET `/{id}`, POST `{label}`, PUT `/{id}`, DELETE `/{id}` |
| `tag/detail` | GET, GET `/{id}` |
| `update` | GET |
| `wanted/cutoff` | GET (paged; `includeSeries`, `includeEpisodeFile`, `includeImages`, `monitored=true`), GET `/{id}` |
| `wanted/missing` | GET (paged; `includeSeries`, `includeImages`, `monitored=true`), GET `/{id}` |

Commands via `POST /api/v3/command` (`Commands/CommandController.cs`): body `{"name": "<CommandName>", ...properties}`; name matched
case-insensitively against command class names minus `Command` (e.g. `RssSync`, `RefreshSeries {seriesId|seriesIds}`, `RescanSeries {seriesId}`,
`EpisodeSearch {episodeIds}`, `SeasonSearch {seriesId, seasonNumber}`, `SeriesSearch {seriesId}`, `MissingEpisodeSearch`, `CutoffUnmetEpisodeSearch`,
`DownloadedEpisodesScan {path, downloadClientId, importMode}`, `RefreshMonitoredDownloads`, `RenameFiles {seriesId, files}`, `RenameSeries {seriesIds}`,
`ManualImport {files, importMode}`, `Backup`, `ApplicationUpdate`, `ApplicationUpdateCheck`, `CheckHealth`, `ClearBlocklist`, `ClearLog`, `DeleteLogFiles`,
`DeleteUpdateLogFiles`, `ImportListSync`, `Housekeeping`, `MessagingCleanup`, `ResetApiKey`, `ResetQualityDefinitions`, `Restart`, `Shutdown`, …).
API commands are pushed with priority Normal (ManualImport: High), trigger Manual, `sendUpdatesToClient` forced true; response 201 + `CommandResource`.
Unknown name → exception (500). CommandResource: `id, name, commandName, message, body, priority, status, result, queued, started, ended,
duration, exception, trigger, clientUserAgent, lastExecutionTime`.

### 8.3 `GET /api/v3/system/status` (`System/SystemController.cs`, `SystemResource.cs`)

| Field | Source / example |
|---|---|
| `appName` | `BuildInfo.AppName` = **`"Sonarr"`** (hard-coded constant) |
| `instanceName` | config.xml `InstanceName` (default `"Sonarr"`) |
| `version` | assembly version, e.g. `"5.0.0.2941"` here (`"4.0.x.y"` on v4) |
| `buildTime` | build date (UTC) |
| `isDebug`, `isProduction`, `isAdmin`, `isUserInteractive` | bools |
| `startupPath`, `appData` | paths |
| `osName`, `osVersion` | e.g. `"ubuntu"`, `"22.04"` |
| `isNetCore` | always `true` |
| `isLinux`, `isOsx`, `isWindows`, `isDocker`, `isContainerized` | bools |
| `mode` | `console\|service\|tray` |
| `branch` | config `Branch` (default `"main"`) |
| `authentication` | `none\|basic\|forms\|external\|oidc` (`basic` can't occur in config — coerced to `forms`) |
| `databaseType` | `sqLite` or `postgreSQL` |
| `databaseVersion` | e.g. `"3.46.1"` (SQLite) / PG server version |
| `migrationVersion` | int (DB schema migration number) |
| `urlBase` | e.g. `""` or `"/sonarr"` |
| `runtimeVersion`, `runtimeName` | e.g. `"8.0.x"`, `".NET"` |
| `startTime` | UTC |
| `packageVersion`, `packageAuthor`, `packageUpdateMechanism` (`builtIn\|script\|external\|apt\|docker`), `packageUpdateMechanismMessage` | from `package_info` file (docker images set these) |
| `sqliteVersion` | declared but never set → omitted |

**Telling Sonarr from a fork / sibling**: `appName` is a compile-time constant (`Common/EnvironmentInfo/BuildInfo.cs`) — `"Sonarr"` for Sonarr;
siblings report their own (`Radarr`, `Lidarr`, `Readarr`, `Prowlarr`, `Whisparr` for the Sonarr-derived Whisparr — **UNVERIFIED** for forks). Combine with:
the `X-Application-Version` header, `version` major (3 = legacy v3 / 4 = v4 / 5 = this tree), `GET /api` (`current`), `/initialize.json`
`apiRoot`, the presence of Sonarr-only routes (`/api/v3/series`, `/api/v3/episode`) and `packageAuthor`/`branch`. A Kitsunarr compatibility layer
that wants Sonarr clients (Overseerr/Jellyseerr, Prowlarr, Notifiarr, LunaSea, nzb360, Homepage…) to accept it should report `appName: "Sonarr"`
and a 4.x `version`, but `instanceName` may be customized; tools that only check `/api/v3/system/status` success will accept any shape (**UNVERIFIED** per tool).

### 8.4 Resource field lists (JSON names)

Common enums: `seriesType`: `standard|daily|anime`; series `status`: `continuing|ended|upcoming|deleted`; `monitorNewItems`: `all|none`;
`protocol`: `unknown|usenet|torrent`; `Language` = `{id, name}`; `QualityModel` = `{quality:{id, name, source, resolution}, revision:{version, real, isRepack}}`;
`MediaCover` = `{coverType, url, remoteUrl}`; `Ratings` = `{votes, value}`.

**series** (`Series/SeriesResource.cs`): `id, title, alternateTitles[{title, seasonNumber, sceneSeasonNumber, sceneOrigin, comment}], sortTitle,
status, ended, profileName, overview, nextAiring, previousAiring, network, airTime, images[], language, originalLanguage, remotePoster, seasonType,
seasons[{seasonNumber, monitored, title, statistics{nextAiring, previousAiring, episodeFileCount, episodeCount, totalEpisodeCount, sizeOnDisk,
releaseGroups[], percentOfEpisodes}, images[]}], seasonTypes[{name, type, seasonNumbers[], episodeCount}], translations[{language, title, overview}],
year, path, pendingPath, qualityProfileId, seasonFolder, monitored, monitorNewItems, useSceneNumbering, runtime, tvdbId, tvRageId, tvMazeId, tmdbId,
firstAired, lastAired, seriesType, cleanTitle, originalTitle, cleanOriginalTitle, imdbId, titleSlug, rootFolderPath, folder,
folders[{language, folder}], certification, genres[], tags[], added, addOptions{monitor, ignoreEpisodesWithFiles, ignoreEpisodesWithoutFiles,
searchForMissingEpisodes, searchForCutoffUnmetEpisodes}, ratings, statistics{seasonCount, episodeFileCount, episodeCount, totalEpisodeCount,
sizeOnDisk, releaseGroups[], percentOfEpisodes}, episodesChanged, languageProfileId (deprecated, always 1)`.
(`addOptions.monitor` = `unknown|all|future|missing|existing|firstSeason|lastSeason|latestSeason|pilot|recent|monitorSpecials|unmonitorSpecials|none|skip`.
`percentOfEpisodes` is a computed decimal in both statistics resources.)

**episode** (`Episodes/EpisodeResource.cs`): `id, seriesId, tvdbId, episodeFileId, seasonNumber, episodeNumber, title, airDate ("yyyy-MM-dd"),
airDateUtc, lastSearchTime, runtime, finaleType, overview, episodeFile (when requested), hasFile, monitored, absoluteEpisodeNumber,
sceneAbsoluteEpisodeNumber, sceneEpisodeNumber, sceneSeasonNumber, unverifiedSceneNumbering, series (when requested), images[], grabbed (omitted when false)`.

**episodefile** (`EpisodeFiles/EpisodeFileResource.cs`): `id, seriesId, seasonNumber, relativePath, path, size, dateAdded, sceneName, releaseGroup,
languages[], quality, customFormats[], customFormatScore, indexerFlags (int bitmask), releaseType, mediaInfo{audioBitrate, audioChannels,
audioCodec, audioLanguages ("eng/jpn"), audioStreamCount, videoBitDepth, videoBitrate, videoCodec, videoFps, videoDynamicRange,
videoDynamicRangeType, resolution ("1920x1080"), runTime ("hh:mm:ss"), scanType, subtitles ("eng/spa")}, qualityCutoffNotMet`.

**qualityprofile**: `id, name, upgradeAllowed, cutoff (quality or group id), items[{id?, name?, quality?, items[], allowed, minSize?, maxSize?,
preferredSize?}] (groups have id ≥ 1000 and name+items, single qualities have quality), minFormatScore, cutoffFormatScore, minUpgradeFormatScore,
formatItems[{format (customformat id), name, score}]`.

**qualitydefinition**: `id, quality{id, name, source, resolution}, title, weight, minSize, maxSize, preferredSize` (MB/min; null = unlimited).
`/limits` → `{min, max}`.

**customformat**: `id, name, includeCustomFormatWhenRenaming, specifications[{name, implementation, implementationName, infoLink, negate,
required, fields[] (provider-field format, usually one field `value`), presets[]}]`.

**delayprofile**: `id, enableUsenet, enableTorrent, preferredProtocol, usenetDelay, torrentDelay (minutes), bypassIfHighestQuality,
bypassIfAboveCustomFormatScore, minimumCustomFormatScore, order, tags[]` (default profile id 1 has no tags and the highest order).

**releaseprofile**: `id, name, enabled, required (string[] — on input also a comma-separated string), ignored (same), airDateRestriction,
airDateGracePeriod, allowSeasonPackWithoutAllEpisodesAired, indexerId (0 = any), tags[], excludedTags[]`.

**indexer** (provider): `id, name, fields[], implementationName, implementation (e.g. "Newznab","Torznab"), configContract ("NewznabSettings",
"TorznabSettings"…), infoLink, message{message, type}, tags[], presets[] (schema only), enableRss, enableAutomaticSearch,
enableInteractiveSearch, supportsRss, supportsSearch, protocol, priority, seasonSearchMaximumSingleEpisodeAge, downloadClientId`.

**downloadclient** (provider): provider fields + `enable, protocol, priority, removeCompletedDownloads, removeFailedDownloads`.

**notification** (provider): provider fields + `link, onGrab, onDownload, onUpgrade, onImportComplete, onRename, onSeriesAdd, onSeriesDelete,
onEpisodeFileDelete, onEpisodeFileDeleteForUpgrade, onHealthIssue, includeHealthWarnings, onHealthRestored, onApplicationUpdate,
onManualInteractionRequired, supportsOn… (13 flags), testCommand`.

**importlist** (provider): provider fields + `enableAutomaticAdd, searchForMissingEpisodes, shouldMonitor (MonitorTypes), monitorNewItems,
rootFolderPath, qualityProfileId, seriesType, language, seasonFolder, listType (program|plex|trakt|simkl|tmdb|other|advanced), listOrder,
minRefreshInterval (TimeSpan "hh:mm:ss"), tagExisting (none|add|sync)`.

**importlistexclusion**: `id, tvdbId, title`. **tag**: `id, label`. **tag/detail**: `id, label, delayProfileIds[], importListIds[], notificationIds[],
restrictionIds[], excludedReleaseProfileIds[], indexerIds[], downloadClientIds[], autoTagIds[], seriesIds[]`.

**rootfolder**: `id, path, accessible, freeSpace, totalSpace, unmappedFolders[{name, path, relativePath}]`.
**remotepathmapping**: `id, host, remotePath, localPath`. **language**: `id (always present), name, nameLower`.
**autotagging**: `id, name, removeTagsAutomatically, tags[], specifications[{name, implementation, implementationName, negate, required, fields[]}]`.

**history** (`History/HistoryResource.cs`): `id, episodeId, seriesId, sourceTitle, languages[], quality, customFormats[], customFormatScore,
qualityCutoffNotMet, date, downloadId, eventType (unknown|grabbed|seriesFolderImported|downloadFolderImported|downloadFailed|episodeFileDeleted|
episodeFileRenamed|downloadIgnored), data{string→string}, episode?, series?`. Grab `data` keys (stored PascalCase in `Core/History/HistoryService.cs`,
emitted camelCase by the STJ dictionary-key policy): `indexer, nzbInfoUrl, releaseGroup, age, ageHours, ageMinutes, publishedDate, downloadClient,
downloadClientName, size, downloadUrl, guid, tvdbId, tvRageId, imdbId, protocol ("1"/"2"), customFormatScore, seriesMatchType, releaseSource,
indexerFlags, releaseType, releaseHash, torrentInfoHash`. Event type filter `eventType[]` uses **integers**
(`Grabbed=1, SeriesFolderImported=2, DownloadFolderImported=3, DownloadFailed=4, EpisodeFileDeleted=5, EpisodeFileRenamed=6, DownloadIgnored=7`).

**config/naming**: `id, renameEpisodes, replaceIllegalCharacters, colonReplacementFormat (int: 0 Delete, 1 Dash, 2 SpaceDash, 3 SpaceDashSpace, 4 Smart, 5 Custom),
customColonReplacementFormat, multiEpisodeStyle (int: 0 Extend, 1 Duplicate, 2 Repeat, 3 Scene, 4 Range, 5 PrefixedRange),
standardEpisodeFormat, dailyEpisodeFormat, animeEpisodeFormat, seriesFolderFormat, seasonFolderFormat, specialsFolderFormat`.

**config/mediamanagement**: `id, autoUnmonitorPreviouslyDownloadedEpisodes, recycleBin, recycleBinCleanupDays, downloadPropersAndRepacks
(preferAndUpgrade|doNotUpgrade|doNotPrefer), createEmptySeriesFolders, deleteEmptyFolders, fileDate (none|localAirDate|utcAirDate),
rescanAfterRefresh (always|afterManual|never), setPermissionsLinux, chmodFolder, chownGroup, episodeTitleRequired (always|bulkSeasonReleases|never),
skipFreeSpaceCheckWhenGrabbing, skipFreeSpaceCheckWhenImporting, minimumFreeSpaceWhenImporting (MB), copyUsingHardlinks, useScriptImport,
scriptImportPath, importExtraFiles, extraFileExtensions, enableMediaInfo, userRejectedExtensions, seasonPackUpgrade (all|threshold|any),
seasonPackUpgradeThreshold (double)`.

**config/host**: `id (1), bindAddress ("*"), port (8989), sslPort (9898), enableSsl, launchBrowser, authenticationMethod, authenticationRequired
(enabled|disabledForLocalAddresses|disabledForLocalhost), allowedHosts, analyticsEnabled, username, password ("********" if set), passwordConfirmation (""),
oidcAuthority, oidcClientId, oidcClientSecret ("********"), oidcUserIdentifier, oidcScopes ("openid profile email"), logLevel, logSizeLimit,
consoleLogLevel, branch, apiKey (plaintext), sslCertPath, sslKeyPath, sslCertPassword ("********"), urlBase, trustedNetworks, instanceName,
applicationUrl, updateAutomatically, updateMechanism, updateScriptPath, proxyEnabled, proxyType (http|socks4|socks5), proxyHostname, proxyPort,
proxyUsername, proxyPassword ("********"), proxyBypassFilter, proxyBypassLocalAddresses, certificateValidation (enabled|disabledForLocalAddresses|disabled),
backupFolder, backupInterval, backupRetention, trustCgnatIpAddresses`.

**config/indexer**: `id, minimumAge (min), retention (days), maximumSize (MB), rssSyncInterval (min)`.
**config/downloadclient**: `id, downloadClientWorkingFolders ("_UNPACK_|_FAILED_"), enableCompletedDownloadHandling, autoRedownloadFailed,
autoRedownloadFailedFromInteractiveSearch`.
**config/ui**: `id, firstDayOfWeek, calendarWeekColumnHeader, shortDateFormat, longDateFormat, timeFormat, timeZone, showRelativeDates,
enableColorImpairedMode, theme, uiLanguage`.
**config/importlist**: `id, listSyncLevel (disabled|logOnly|keepAndUnmonitor|keepAndTag), listSyncTag`.

Other useful resources: **queue** `{id, seriesId, episodeId, seasonNumber, series?, episode?, languages, quality, customFormats, customFormatScore,
size, title, estimatedCompletionTime, added, status (unknown|queued|paused|downloading|completed|failed|warning|delay|downloadClientUnavailable|fallback),
trackedDownloadStatus (ok|warning|error), trackedDownloadState (downloading|importBlocked|importPending|importing|imported|failedPending|failed|ignored),
statusMessages[{title, messages[]}], errorMessage, downloadId, protocol, downloadClient, downloadClientHasPostImportCategory, indexer, outputPath,
episodeHasFile, sizeleft, timeleft}`; **queue/status** `{totalCount, count, unknownCount, errors, warnings, unknownErrors, unknownWarnings}`;
**health** `{id?, source (check class name), type (ok|notice|warning|error), message, wikiUrl}`; **system/task** `{id, name, taskName, interval,
lastExecution, lastStartTime, nextExecution, lastDuration}`; **indexerflag** `{id, name, nameLower}`.

### 8.5 Provider `fields` array & `/schema`

Provider settings are not exposed as an object but as `fields` (`Sonarr.Http/ClientSchema/Field.cs`, `SchemaBuilder.cs`):

```json
{ "order": 0, "name": "baseUrl", "label": "URL", "unit": null, "helpText": "...", "helpTextWarning": null, "helpLink": null,
  "value": "https://indexer.example", "type": "textbox", "advanced": false,
  "selectOptions": [{"value": 0, "name": "Started", "order": 0, "hint": "(0)"}],
  "selectOptionsProviderAction": "newznabCategories", "section": null, "hidden": "hidden|hiddenIfNotSet",
  "privacy": "normal|password|apiKey|userName", "placeholder": null, "isFloat": false }
```
- `name` = camelCase property name; nested settings objects are flattened with dot prefix (`seedCriteria.seedRatio`, `seedCriteria.seasonPackSeedTime`).
- `order` is **renumbered 0..n** after flattening (attribute orders are only used for sorting).
- `type` = `FieldType` enum, first char lower-cased: `textbox, number, password, checkbox, select, path, filePath, tag, action, url, captcha, oAuth,
  device, tagSelect, rootFolder, qualityProfile, seriesTag, keyValueList`.
- `selectOptions` built from enums (value = int, name = `[FieldOption]` label or enum name, obsolete members hidden) or a converter class;
  dynamic options via `POST /api/v3/{provider}/action/{selectOptionsProviderAction}` with the provider resource as body (returns `{options:[…]}`).
- `isFloat` true for `number` fields backed by `double`.
- Secrets: fields with privacy `apiKey` or `password` are returned as **`"********"`** when non-empty (`PRIVATE_VALUE`); on PUT/POST a value of
  `"********"` means "keep existing". **Consequence for migration: indexer API keys, download-client passwords/API keys, notification tokens cannot
  be read through this API version** — they must be re-entered or read from the DB/backup. (Older v4 releases returned them in clear text —
  **UNVERIFIED**.) `config/host.apiKey` is still returned in clear.
- Input (`ReadFromSchema`): only fields whose `name` matches a mapping are read; missing fields → default values; value converted to the
  property type (ints, bools, arrays from JSON arrays).
- `GET /api/v3/{provider}/schema`: one template resource per implementation (default settings in `fields`, `implementation`, `configContract`,
  `infoLink`, `presets[]` with prefilled definitions, e.g. Newznab DOGnzb/NZBgeek…). Create = take a template, fill `fields[].value`, set `name`,
  enable flags, POST.
- Validation on POST/PUT: provider `Validate()` + `Test()` when enabled (warnings fail unless `?forceSave=true`); errors → 400 array with
  `propertyName` = field name (e.g. `"ApiKey"`, `"Host"`), `isWarning`, `detailedDescription`, `infoLink`.
- `POST /test` returns `"{}"`/200 when OK, 400 with failures otherwise; `POST /testall` returns `[{id, isValid, validationFailures[]}]` (400 if any invalid).
- Bulk resources (`…BulkResource`): `{ids[], tags[], applyTags (add|remove|replace), …nullable per-provider props}` (indexer: `enableRss,
  enableAutomaticSearch, enableInteractiveSearch, priority, seasonSearchMaximumSingleEpisodeAge`).

---

## 9. Security model

Files: `Sonarr.Http/Authentication/*`, `NzbDrone.Host/Startup.cs`, `NzbDrone.Host/ForwardedHeadersConfigurator.cs`,
`NzbDrone.Host/ConfigureHostFilteringOptions.cs`, `Core/Configuration/ConfigFileProvider.cs`, `Core/Authentication/*`,
`NzbDrone.SignalR/*`, `frontend/src/Components/SignalRListener.tsx`.

### 9.1 Authentication methods (`Core/Authentication/AuthenticationType.cs`)

| Value | Meaning in this tree |
|---|---|
| `None` (0) | `NoAuthenticationHandler` (anonymous principal). **Cannot be set via config.xml** — `ConfigFileProvider.AuthenticationMethod` rewrites `None`/`Basic` to `Forms`; only reachable through the options/env override (`AuthOptions.Method`, configuration section `Sonarr:Auth` → environment variable `Sonarr__Auth__Method=None`; `NzbDrone.Host/Bootstrap.cs` binds env vars without prefix). Other `Sonarr:Auth` keys: `ApiKey, InitialUsername, InitialPassword, ResetToken, Enabled, Method, Required, TrustCgnatIpAddresses, Oidc*`. |
| `Basic` (1) | `[Obsolete]`; no Basic scheme is registered — always coerced to `Forms`. (v4 releases still offered HTTP Basic — **UNVERIFIED**.) |
| `Forms` (2) | Cookie `"{InstanceName sanitized}Auth"` (e.g. `SonarrAuth`), 7-day sliding expiry, `rememberMe` → persistent cookie; login page `/login`, `returnUrl` param. Users table: single user, password PBKDF2-HMAC-SHA512, **220,000** iterations, 16-byte salt, 32-byte hash (legacy unsalted SHA-256 hashes upgraded on login). Lockout: **5** failed attempts per remote IP → **5 min** lock. Password reset (Forms only): `POST /reset-password/request` generates a 20-char token that is written to the **log file** (warn `Auth-ResetRequested … token: …`), valid 15 min, one active token per process, 1 request/min/IP; alternatively a pre-configured `Sonarr__Auth__ResetToken` (≥ 20 chars); `POST /reset-password` with `token, username, password, passwordConfirmation`. |
| `External` (3) | No auth in Sonarr (reverse proxy is expected to authenticate); same `NoAuthenticationHandler`. |
| `Oidc` (4) | OpenID Connect code flow (`response_type=code`, `response_mode=query`), cookie `"{Instance}OIDC"`; config `OidcAuthority` (must be https), `OidcClientId`, `OidcClientSecret`, `OidcUserIdentifier` (single allowed user: verified email or `sub`), `OidcScopes` (default `openid profile email`). Falls back to Forms if not fully configured (`EffectiveAuthenticationMethod`). |

`AuthenticationRequired` (`AuthenticationRequiredType`): `Enabled` (0, default), `DisabledForLocalAddresses` (1), `DisabledForLocalhost` (2, new).
Applied **only to the UI policy** (`UiAuthorizationHandler`): local = loopback, 10/8, 172.16/12, 192.168/16, 169.254/16, IPv6 link-local/
unique-local/site-local, plus 100.64/10 (CGNAT) when `TrustCgnatIpAddresses`; bypass is refused if an `X-Forwarded-For` header is still present
(i.e. the request came through an untrusted proxy). `DisabledForLocalhost` = loopback only.

### 9.2 Authorization policies

| Policy | Scheme(s) | Applies to |
|---|---|---|
| Fallback `"API"` | `ApiKeyAuthenticationHandler` (scheme `API`) + `RequireAuthenticatedUser` | everything not `[AllowAnonymous]` and without another policy: **all `/api/*` (v3, v5, `/api`), `/feed/*`** |
| `"UI"` | the effective auth method's scheme (`Forms`, cookie for OIDC, `None`, `External`) + `BypassableDenyAnonymous` (local bypass) | `StaticResourceController` (SPA, MediaCover, logs, backups), `/initialize.json` |
| `"SignalR"` | API-key scheme `SignalR` | `/signalr/messages` |
| `[AllowAnonymous]` | — | `/ping`, `/login*`, `/logout`, `/loggedout`, `/reset-password*`, `/content/*` |

**Key consequence:** API endpoints accept **only the API key**, never the UI cookie. The browser UI gets the key from `/initialize.json`
(cookie-protected) and then sends `X-Api-Key` on every call. With `AuthenticationRequired=DisabledForLocalAddresses`, anyone on the LAN can
open `/initialize.json` and obtain the API key.

### 9.3 API key

- Stored in config.xml `ApiKey` (or `Sonarr__Auth__ApiKey`); generated as `Guid.NewGuid().ToString("N")`-style 32 lowercase hex chars when empty; `ResetApiKeyCommand` regenerates. Health warning if < 20 chars.
- Accepted from (`ApiKeyAuthenticationHandler.ParseApiKey`, first match wins): query **`apikey`** → header **`X-Api-Key`** →
  `Authorization: Bearer {key}`. For SignalR the query name is **`access_token`** (header `X-Api-Key` also works).
- Compared with `CryptographicOperations.FixedTimeEquals`. Missing/invalid key → `NoResult` → **401** (challenge writes bare 401, no body);
  forbidden → 403. No rate limiting on API key failures.

### 9.4 CSRF / CORS / hosts / proxies

- **No anti-forgery tokens** anywhere (no `Antiforgery` usage). Forms login is a plain form POST; API calls are protected by the header/query key.
- **CORS**: every `[V3ApiController]`/`[V5ApiController]` carries policy `ApiCorsPolicy` = `AllowAnyOrigin().AllowAnyMethod().AllowAnyHeader()`
  (no credentials). `/content/*` uses `AllowGet` (any origin, GET/OPTIONS).
- **Host filtering** (`ConfigureHostFilteringOptions.cs`): config `AllowedHosts` (comma/semicolon list, `*.domain` wildcards); empty → any host;
  otherwise the list + `localhost`, `127.0.0.1`, `[::1]`, machine name/DNS host name; empty Host header allowed. Bind addresses (`0.0.0.0`, `[::]`) ignored.
- **Forwarded headers** (`ForwardedHeadersConfigurator.cs`): `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host` honoured from loopback
  (ASP.NET default) plus `TrustedNetworks` CIDRs (config, comma list); unlimited forward chain.
- **URL base**: config `UrlBase` (stored without slashes, exposed with leading `/`); `UsePathBase`; requests without the base are 307-redirected.
- **TLS**: optional Kestrel HTTPS on `SslPort` (9898) with `SslCertPath`/`SslKeyPath`/`SslCertPassword`.
- Outgoing certificate validation for clients/indexers: config `CertificateValidation` (`enabled|disabledForLocalAddresses|disabled`).

### 9.5 Secrets in API responses

Provider fields with privacy `password`/`apiKey` → `"********"`; `config/host` masks `password`, `oidcClientSecret`, `sslCertPassword`,
`proxyPassword` but returns `apiKey` in clear (§8.5).

### 9.6 SignalR real-time messages

- Hub: ASP.NET Core SignalR at `{urlBase}/signalr/messages` (JSON hub protocol, STJ camelCase settings). Client:
  `negotiate` then WebSocket (fallbacks SSE/long-poll) with `?access_token={apiKey}`; automatic reconnect.
- Single server→client method: **`receiveMessage`** with one argument:
  `{"name": "<resource>", "body": {"resource": {...resource JSON...}, "action": "created|updated|deleted|sync"}, "version": 3}`
  (`resource` omitted for `sync`; generic model deletes send a resource-less `deleted` **and** a `deleted` whose resource is `new TResource{id}`
  — i.e. `id` plus default-valued non-nullable fields; `version` = API version of the emitting controller: **3** for `/api/v3` controllers,
  5 for v5 — both are broadcast to all clients; the v5 UI in this tree drops `version < 5`; a v4-era UI/client consumes the v3 messages —
  whether v4 releases already sent the `version` property is **UNVERIFIED**).
- On connect the hub broadcasts `{"name":"version","body":{"version":"5.0.0.2941"}}` (to **all** clients).
- Message names emitted by v3 controllers (`RestControllerWithSignalR`):

| name | Emitted on | action / body |
|---|---|---|
| `version` | client connect | `{version}` |
| `command` | command progress/status changes (100 ms debounce) | `updated` + CommandResource; `sync` after `MessagingCleanup` completes |
| `series` | series updated/edited/refreshed/renamed, file imported/deleted, covers updated, bulk edit | `updated` + SeriesResource; `deleted` + SeriesResource |
| `episode` | episode grabbed/imported/file deleted | `updated` + EpisodeResource (grabbed flag on grab) |
| `episodefile` | file added / deleted | `updated` / `deleted` |
| `calendar`, `wanted/missing`, `wanted/cutoff` | same episode events (EpisodeControllerWithSignalR subclasses) | `updated` + EpisodeResource |
| `queue`, `queue/details` | queue/pending releases changed | `sync` |
| `queue/status` | queue changed | `updated` + QueueStatusResource |
| `health` | health checks completed | `sync` |
| `system/task` | any command executed | `sync` |
| `qualitydefinition` | after `ResetQualityDefinitions` | `sync` |
| `tag` | tags / auto-tags updated | `sync` |
| `rootfolder` | root folder model events | created/updated/deleted |
| `indexer`, `downloadclient`, `notification`, `importlist`, `metadata` | provider added/updated/deleted | `created`/`updated` + resource; `deleted` + `{id}` |
| (any RestControllerWithSignalR model) | generic `ModelEvent<T>` | created/updated/deleted/sync |

(The v5 UI additionally handles `connection` — v5's name for notifications.)

---

## 10. Key findings & tricky details for the Rust implementation

1. **Version caveat** — the audited tree is Sonarr v5-dev (`v5.0.0.2941`); `/api/v3` is intact but `/api` reports v5 current. Validate v4-specific
   behaviours (Basic auth, plaintext provider secrets in API, SignalR `version` field) against a real v4 build before relying on them.
2. **Newznab caps drive everything**: `supportedParams` presence on `<tv-search>` silently enables *aggregate id search* (all ids in one request);
   season 0 is sent as `season=00`; daily episodes as `season=yyyy&ep=MM/dd` (unescaped slash); anime absolute numbers zero-padded to 2 and sent both as
   `tvsearch&q=NN` (with ids) and `search&q={title}+NN`; title queries always use the *standard* categories; tier-2 title searches only run if tier 1
   returned nothing.
3. **RSS paging stop rule** uses the persisted `LastRssSyncReleaseInfo` (pubDate or identical download URL), first sync = 1 page, max 30 pages / 1000
   results; the newest release is persisted after every successful sync.
4. **Backoff**: `[0,1m,5m,15m,30m,1h,3h,6h,12h,24h]`, first failure always level 1, success only steps down one level, 15-min startup grace caps at
   5 min, `Request limit reached`/429 force ≥ 1 h (or `Retry-After`); download clients/notifications capped at 1 h with a 5-min initial grace.
5. **qBittorrent**: probe `/api/v2/app/webapiVersion` (403 ⇒ v2), cookie login with `Ok.`/`Fails.` body semantics and re-login on 403, new
   Bearer API key mode; `stopped` vs `paused` param switch at WebAPI 2.11.0; share limits on add only from 2.8.1; `inactiveSeedingTimeLimit` from 2.9.2;
   output path from `root_path` (≥2.11.2) → `content_path`; removal only when `pausedUP/stoppedUP` **and** seed limit reached (seeding time needs an
   extra `/torrents/properties` call).
6. **Transmission**: empty POST → 409 → `X-Transmission-Session-Id`; labels only on ≥ 4.0 (else category = sub-directory); no Paused state mapping;
   `seedIdleLimit` is (ab)used as seed time.
7. **Deluge**: Web JSON-RPC needs `auth.login` + `web.connected`/`web.get_hosts`/`web.connect` (picks the `127.0.0.1` daemon); error codes 1/2 ⇒ re-auth;
   seed time unsupported, only `stop_ratio`.
8. **SABnzbd**: everything is `GET /api?mode=…&output=json&apikey=…`; add is `mode=addfile` multipart field `name`; queue `timeleft` is `H:MM:SS` or
   `D:H:MM:SS`; history `storage` must be walked up to the job folder; history removal uses `archive=0` only for failed items.
9. **NZBGet**: 64-bit sizes split in Hi/Lo; Sonarr tracks downloads by its own `drone` post-processing parameter (32-hex GUID), not the NZBID; history
   status precedence (Par→Unpack→Move→Script→Delete; `DeleteStatus=MANUAL` hides non-BAD items).
10. **Tracked downloads** live only in memory (state rebuilt from History/DownloadHistory tables after restart); import is gated by grab history
    (or category), path validity (remote path mappings) and `VerifyImport` (count or history-based).
11. **Webhook** JSON is Newtonsoft (camelCase, nulls dropped, indented) with **PascalCase `eventType`**; Import-Complete reuses `eventType: "Download"`;
    `applicationUrl` is `""` when unset.
12. **API**: STJ camelCase (`uiLanguage`, `sqLite`), nulls dropped, dates `yyyy-MM-ddTHH:mm:ssZ`; only the **API key** authorizes `/api` (query
    `apikey` > header `X-Api-Key` > `Authorization: Bearer`); the UI fetches it from `/initialize.json`; POST→201, PUT→202; validation errors are a
    JSON array; provider secrets come back as `********` (unreadable for migration — use DB/backup instead), `config/host.apiKey` is plaintext.
13. **SignalR**: hub `/signalr/messages?access_token=…`, single client method `receiveMessage` with `{name, body:{resource, action}, version}`; resource
    names = REST resource paths (`series`, `queue/status`, `system/task`, …).
