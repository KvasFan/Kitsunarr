# Feature matrix

Status legend:
- **Done**: implemented and verified by automated tests (and, where noted, by a manual run against real data).
- **Partial**: some of the behaviour exists; the gaps are listed.
- **Model**: data model, storage and migration only, with no runtime behaviour yet.
- **Not started**

The "Tests" column names the suites that cover the feature. Simulated tests replay captured responses or use mocks; real-service tests are marked explicitly.

## Required features

| # | Feature | Reference implementation | Kitsunarr module | Status | Tests | Gaps / next steps |
|---|---|---|---|---|---|---|
| 1 | TV series management | Sonarr `Core/Tv`, `Api.V3/Series`, `AddSeriesService`, `RefreshSeriesService` | `kitsunarr-db::library::series`, `kitsunarr-library` (add, refresh), `kitsunarr-api::routes::{library, metadata}`, web `pages::{library, detail, add}` | Partial | db `series::tests`; library `add_refresh`; HTTP `migration_over_http_then_library_operations`, `catalogue_search_add_and_refresh`; browser e2e | Series can be added and refreshed from TVmaze. Adding from TheTVDB (Sonarr's catalogue) and deleting files from disk are not implemented. |
| 2 | Anime management | Sonarr `SeriesTypes.Anime`, Tanukarr AniList/MAL owners | same as 1, `media_kind = anime` | Partial | as 1; library `airing_is_inferred_from_later_episodes` | Anime can be added and refreshed from AniList (one season, absolute number = episode number, as in Tanukarr). Scene/absolute mapping and search logic are not implemented. |
| 3 | Movie management | Tanukarr `MediaType=Movie` | `kitsunarr-db::library::movies`, `kitsunarr-library` | Partial | db `movies::tests`, migrate fixture tests, library `add_anilist_movie_creates_a_movie` | Anime films can be added and refreshed from AniList. Live-action films need TMDB/OMDb. Search and download are not implemented. |
| 4 | Collections | Tanukarr `CollectionTitle`, `AnimeGroup*` | `kitsunarr-db::library::collections` | Partial | db `collections::tests`, Tanukarr fixture test | Editing UI, TMDB collections. |
| 5 | Metadata providers | Sonarr SkyHook; Tanukarr 7 proxies | `kitsunarr-metadata` (TVmaze, AniList), `kitsunarr-library`, scheduled `RefreshMetadata` | Partial | metadata unit tests (recorded TVmaze responses); library `add_refresh` (mock catalogue); HTTP `catalogue_search_add_and_refresh`; browser e2e (mock TVmaze); live `tvmaze_search_and_details` and `anilist_search_details_and_movies` (ignored, real services; passed on 2026-11-09) | 2 of 7 catalogues. Not implemented: TheTVDB, TMDB TV, TMDB Movies, MAL/Jikan, OMDb; id-prefixed search terms (`tvdb:123`); Tanukarr's presentation enrichment. |
| 6 | Multiple metadata identities | Tanukarr `MetadataSource`/`ForeignId`, `MalIds`/`AniListIds` | `series/movie_external_ids`, `(metadata_provider, metadata_id)`, `kitsunarr-library::find_existing` | Partial | migrate fixture tests; HTTP `catalogue_search_add_and_refresh`; browser e2e | Search results and adds are matched against the library by catalogue id and any shared external id: a migrated TheTVDB-owned series shows as "in library" in TVmaze results. Changing a title's owning catalogue is not implemented. |
| 7 | Episode numbering | Sonarr scene/absolute numbering | episode columns (absolute, scene); release names parsed by `kitsunarr-parser` (standard, absolute, daily) | Partial | parser corpus (see "Release parsing") | Scene-numbering mapping to library episodes (`ParsingService`, Phase 3/6). |
| 8 | Quality profiles | Sonarr `Profiles/Qualities` | `kitsunarr-db::quality`, API `/qualityprofiles`, web | Done (CRUD) | db `quality::tests` | Use in the decision engine. |
| 9 | Custom formats | Sonarr `CustomFormats` | `kitsunarr-db::quality`, API `/customformats` | Model + CRUD | db `quality::tests` | Evaluation engine (Phase 3). |
| 10 | Indexers | Sonarr `Indexers/Newznab`, `Torznab`, `HttpIndexerBase`, `NewznabCapabilitiesProvider` | `kitsunarr-indexers` (capabilities, request generator, feed parser, client); API `POST /providers/indexers/test`; web indexer editor "Test" | Partial | indexers `sonarr_feeds` (Sonarr's recorded feeds and `NewznabFixture`/`TorznabFixture` cases), `request_generator` (`NewznabRequestGeneratorFixture` cases), `client` (mock indexer); HTTP `indexer_test_and_interactive_search`; browser e2e (mock Newznab) | Newznab and Torznab work: capabilities (cached for a week, upstream defaults when unreadable), ID searches then title searches, paging, daily/anime/season request forms, error and request-limit detection, indexer flags, GUID de-duplication and 2 s spacing between requests to one indexer. Not implemented: backing off from failing indexers, RSS bookkeeping, indexer types other than Newznab/Torznab, proxies, seed criteria. |
| 11 | Download clients | Sonarr `Download/Clients/*` | provider storage | Model | db `providers::tests` | qBittorrent, Transmission, Deluge, SABnzbd, NZBGet clients (Phase 4). |
| 12 | Interactive search | Sonarr `IndexerSearch`, `ReleaseSearchService` | API `GET /releases`; web `components::releases`, Search page, season and episode "Search" on detail pages | Partial | api `routes::search::tests` (episode search forms); HTTP `indexer_test_and_interactive_search`; browser e2e | Season and episode searches query every indexer with interactive search enabled, in parallel. Episodes are searched as Sonarr's `EpisodeSearch` does: daily by air date, anime by absolute number, specials by "series + episode title" and by number. Results are de-duplicated by GUID. Each release is parsed (episodes, quality, languages, group); releases of other series are flagged and listed last; per-indexer failures are shown. Download links stay on the server. Not implemented: grabbing (needs download clients), the decision engine's accept/reject reasons and ordering, scene-mapping titles. |
| 13 | RSS synchronisation | Sonarr `RssSyncService` | — | Not started | — | Phase 5. |
| 14 | Monitoring | Sonarr monitored flags | series/season/episode flags, API updates, bulk editor (UI) | Partial | db `series::tests`; HTTP library test | Automation that acts on the flags. |
| 15 | Importing (media files) | Sonarr `MediaFiles/EpisodeImport` | `kitsunarr-library` (planned) | Not started | — | Phase 4. |
| 16 | File renaming | Sonarr `Organizer` | naming settings | Model | — | Renamer (Phase 4). |
| 17 | Disk management | Sonarr `DiskProvider` | root folders with free space, health checks | Partial | HTTP `settings_validation_and_secret_redaction` | Path guards for file operations, recycle bin. |
| 18 | Notifications | Sonarr `Notifications/*` | provider storage | Model | — | Webhook/Discord/Jellyfin (Phase 5). |
| 19 | Task scheduling | Sonarr `TaskManager`, `CommandQueue` | `kitsunarr-scheduler`; jobs Housekeeping, CleanupSessions, Backup, HealthCheck, RefreshMetadata | Done | scheduler tests; library `refresh_scheduling_follows_status_and_age` | More jobs as features land. |
| 20 | Authentication | Sonarr Forms/Basic/API key | `kitsunarr-auth`, `kitsunarr-api::auth`, web setup and login pages | Done | auth unit tests; HTTP `first_run_setup_login_and_logout`, `login_throttling_and_api_keys`, `cross_site_writes_are_rejected`; browser e2e | Additional users and roles. |
| 21 | API compatibility | Sonarr `/api/v3` | native `/api/v1` with OpenAPI | Partial | HTTP `openapi_spa_fallback_and_events` | Sonarr-v3 compatibility layer for third-party tools. |
| 22 | Backup and restore | Sonarr `Backup` | `kitsunarr-db::maintenance`, CLI `backup` and `restore`, scheduled job | Done | db maintenance tests, server job tests | Restore from the web UI. |
| 23 | Jellyfin integration | Tanukarr Jellyfin refresh and watched state | — | Not started | — | Phase 6. Watched-state values are preserved by migration. |
| 24 | Localisation | Sonarr/Tanukarr `Localization/*.json` | web `i18n` module | Partial | — | English only. Strings are centralised; more languages are planned. |

## Release parsing (port of Sonarr's parser)

Each component is checked against Sonarr's own test cases (`tests/fixtures/parser/sonarr/`, 1,683 active rows extracted from 27 upstream fixtures). A component is listed as done only when every active upstream case for it passes. 23 rows upstream keeps commented out (its known failures) are skipped, as upstream skips them.

| Component | Upstream | Status | Upstream cases passing |
|---|---|---|---|
| Quality (source, resolution, remux, proper/repack/version, REAL), extension fallback | `QualityParser.cs`, `QualityFinder.cs`, `MediaFileExtensions.cs` | Done | 290/290 `QualityParserFixture`, 30/30 `ExtendedQualityParserRegex`, plus both `TestCaseSource` tables |
| Release groups (including anime subgroups, exception lists, repost cleanup, Chinese/Korean pre-substitutions) | `ReleaseGroupParser.cs`, `ParserCommon.cs`, `RegexReplace.cs`, `FileExtensions.cs` | Done | 151/151 `ReleaseGroupParserFixture` |
| Languages in release titles, subtitle languages/flags/titles, ISO 639 lookups | `LanguageParser.cs`, `IsoLanguages.cs` | Done | 221/221 `LanguageParserFixture`, 26/26 `IsoLanguagesFixture` |
| Titles, seasons and episodes: standard, multi-episode, daily, anime and absolute numbering, season/multi-season/partial packs, specials, split episodes, mini-series, hashed and reversed releases, file paths | `Parser.cs` (all 109 title patterns, verbatim), `ParserCommon.cs`, `ParsedEpisodeInfo.cs` | Done | 167/167 `AbsoluteEpisodeNumberParserFixture`, 185/185 `SingleEpisodeParserFixture`, 77/77 `MultiEpisodeParserFixture`, 76/76 `SeasonParserFixture`, 57/57 `DailyEpisodeParserFixture`, 56/56 `UnicodeReleaseParserFixture`, 47/47 `ParserFixture`, 36/36 `AnimeMetadataParserFixture`, 33/33 `PathParserFixture`, 30/30 `CrapParserFixture`, 31/31 `UrlFixture`, 21/21 `SeasonTitleParserFixture`, 20/20 `HashedReleaseFixture`, 11/11 `MiniSeriesEpisodeParserFixture`, and the smaller fixtures |
| Title normalisation (`CleanSeriesTitle`, diacritics, episode titles, IMDb ids), scene-title check, season formatting | `Parser.cs`, `SceneChecker.cs`, `SeasonNumbersFormatter.cs`, Diacritical.Net table | Done | 41/41 `NormalizeSeriesTitleFixture`, 6/6 `NormalizeEpisodeTitleFixture`, 13/13 `SceneCheckerFixture`, 8/8 `SeasonNumbersFormatterFixture` |
| Matching parse results to library series and episodes | `ParsingService.cs` | Not started | 21 rows in `ParsingServiceTests/` (need the library lookups; part of the decision engine work, #5) |

## Migration (Phase 2)

| Capability | Status | Tests |
|---|---|---|
| Sonarr API migration (`/api/v3`) | Done | Simulated replay of captured Sonarr 4.0.20 responses (`api_source_matches_backup_source`); `live_source_dry_run` (ignored, real service) |
| Tanukarr API migration | Done | Simulated replay of Tanukarr 0.1.5 responses (`tanukarr_api_source_is_detected`) |
| Backup migration (`.zip` or `sonarr.db`) | Done | Real Sonarr 4.0.20 and Tanukarr 0.1.5 backups |
| Version-aware reading (schema introspection, legacy table names) | Done | Normaliser unit tests (v3 comma-separated release profiles, both JSON encodings) |
| Dry-run preview (identical to apply) | Done | `sonarr_backup_dry_run_then_apply_then_rerun` |
| Path mapping (Docker, Windows → POSIX) | Done | `paths::tests` |
| Destination backup before apply | Done | fixture test |
| Validation (integrity, counts, monitored flags, paths, links) | Done | fixture tests, manual HTTP run |
| Idempotent re-run, skip/update conflict policy | Done | `update_policy_overwrites_and_skip_policy_preserves` |
| Unsupported values preserved, never dropped | Done | `migration_unmapped` table; normaliser tests |
| No filesystem modification | Done | Directory-tree comparisons in the fixture tests |
| Web wizard | Done | Browser e2e `first_run_setup_migration_and_library` (headless Chromium 156 through WebDriver, real Sonarr backup); HTTP `migration_over_http_then_library_operations` |
| CLI (`kitsunarr migrate`) | Done | Manual run on the Tanukarr 0.1.5 backup (dry run and apply, all checks passed) |

## Tanukarr-specific features

| Feature | Tanukarr status | Kitsunarr status |
|---|---|---|
| Unified TV, anime and movies | Operational | Separate series and movie libraries; Tanukarr movies migrate as movies. |
| TheTVDB, TMDB TV, TVmaze, AniList, MAL/Jikan, TMDB Movies, OMDb | Operational | Provider codes and identities are preserved. TVmaze and AniList adapters are done, with Tanukarr's numbering: TVmaze specials go to season 0 in episode-id order with no absolute numbers; AniList titles have a single season. Refreshing a migrated TVmaze or AniList title therefore lines up with its existing episodes. The other adapters are not started. |
| Per-title metadata ownership | Operational | Done (model and migration). |
| TMDB episode groups | Partial | `episode_ordering` preserved by migration; not applied yet. |
| Absolute anime numbering | Mostly upstream | Stored; logic not started. |
| Prequel/sequel relationships, manual grouping | Operational | Groups migrate to collections; relation traversal not started. |
| Movie and series collections | Operational | Done (model, migration and listing). |
| Multi-season and complete-series packs, interactive pack search | Operational | Not started (Phase 6). |
| Quality and codec settings | Operational | Codec factors and per-item sizes are preserved as unmapped values; not applied. |
| Jellyfin refresh and watched state | Operational | Watched episode ids are preserved as unmapped values; integration not started. |

## Web interface (Dioxus, WebAssembly)

| Page | Status |
|---|---|
| Setup and login, dashboard, TV/anime/movie libraries (poster grid and table, server-side paging, debounced search, sorting, filters, bulk editing), series and movie detail (virtualised episode lists, monitoring), collections, calendar, history | Done. Browser e2e covers setup, dashboard, libraries, detail, table view, theme and sign-out. |
| Import / migration wizard | Done (browser e2e) |
| Settings: general (API key, password), media management (root folders, naming, files), quality profiles editor, custom formats (JSON editor), indexers, download clients, notifications, import lists, tags | Done (CRUD on stored settings). Download clients covered by browser e2e. |
| System: status and health, scheduled tasks (run now, history), backups, logs | Done |
| Add media: search TVmaze or AniList, see which results are already in the library, add dialog (root folder, profile, monitoring, series type, season folders, tags, exact folder preview); "Refresh metadata" on detail pages | Done. Browser e2e uses a mock TVmaze replaying recorded responses. |
| Interactive search: Search page and per-season/per-episode search on detail pages; indexer "Test" in settings | Done for listing releases (browser e2e with a mock Newznab indexer). "Grab" is disabled until download clients exist. |
| Download queue, metadata source settings | Honest "not available yet" pages; the backends are not implemented |
| Keyboard shortcuts (`/` search, `g` + letter navigation, `Esc`), dark and light themes, responsive layout with a mobile drawer, live updates over SSE | Done |
