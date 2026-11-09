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
| 1 | TV series management | Sonarr `Core/Tv`, `Api.V3/Series` | `kitsunarr-db::library::series`, `kitsunarr-api::routes::library`, web `pages::library`/`pages::detail` | Partial | db `series::tests`; HTTP `migration_over_http_then_library_operations`; browser e2e | Adding series (needs metadata providers), refresh, deletion of files from disk. |
| 2 | Anime management | Sonarr `SeriesTypes.Anime`, Tanukarr AniList/MAL owners | same as 1, `media_kind = anime` | Partial | as 1 | Anime numbering and search logic. |
| 3 | Movie management | Tanukarr `MediaType=Movie` | `kitsunarr-db::library::movies` | Partial | db `movies::tests`, migrate fixture tests | Add/search/download. |
| 4 | Collections | Tanukarr `CollectionTitle`, `AnimeGroup*` | `kitsunarr-db::library::collections` | Partial | db `collections::tests`, Tanukarr fixture test | Editing UI, TMDB collections. |
| 5 | Metadata providers | Sonarr SkyHook; Tanukarr 7 proxies | `kitsunarr-metadata` (planned) | Not started | — | Phase 3. |
| 6 | Multiple metadata identities | Tanukarr `MetadataSource`/`ForeignId`, `MalIds`/`AniListIds` | `series/movie_external_ids`, `(metadata_provider, metadata_id)` | Model | migrate fixture tests | Provider lookups by identity. |
| 7 | Episode numbering | Sonarr scene/absolute numbering | episode columns (absolute, scene) | Model | migrate fixture tests | Mapping logic (Phase 3/6). |
| 8 | Quality profiles | Sonarr `Profiles/Qualities` | `kitsunarr-db::quality`, API `/qualityprofiles`, web | Done (CRUD) | db `quality::tests` | Use in the decision engine. |
| 9 | Custom formats | Sonarr `CustomFormats` | `kitsunarr-db::quality`, API `/customformats` | Model + CRUD | db `quality::tests` | Evaluation engine (Phase 3). |
| 10 | Indexers | Sonarr `Indexers/Newznab`, `Torznab` | provider storage | Model | db `providers::tests` | Torznab/Newznab clients (Phase 3). |
| 11 | Download clients | Sonarr `Download/Clients/*` | provider storage | Model | db `providers::tests` | qBittorrent, Transmission, Deluge, SABnzbd, NZBGet clients (Phase 4). |
| 12 | Interactive search | Sonarr `IndexerSearch` | — | Not started | — | Phase 3. |
| 13 | RSS synchronisation | Sonarr `RssSyncService` | — | Not started | — | Phase 5. |
| 14 | Monitoring | Sonarr monitored flags | series/season/episode flags, API updates, bulk editor (UI) | Partial | db `series::tests`; HTTP library test | Automation that acts on the flags. |
| 15 | Importing (media files) | Sonarr `MediaFiles/EpisodeImport` | `kitsunarr-library` (planned) | Not started | — | Phase 4. |
| 16 | File renaming | Sonarr `Organizer` | naming settings | Model | — | Renamer (Phase 4). |
| 17 | Disk management | Sonarr `DiskProvider` | root folders with free space, health checks | Partial | HTTP `settings_validation_and_secret_redaction` | Path guards for file operations, recycle bin. |
| 18 | Notifications | Sonarr `Notifications/*` | provider storage | Model | — | Webhook/Discord/Jellyfin (Phase 5). |
| 19 | Task scheduling | Sonarr `TaskManager`, `CommandQueue` | `kitsunarr-scheduler` | Done | scheduler tests | More jobs as features land. |
| 20 | Authentication | Sonarr Forms/Basic/API key | `kitsunarr-auth`, `kitsunarr-api::auth`, web setup and login pages | Done | auth unit tests; HTTP `first_run_setup_login_and_logout`, `login_throttling_and_api_keys`, `cross_site_writes_are_rejected`; browser e2e | Additional users and roles. |
| 21 | API compatibility | Sonarr `/api/v3` | native `/api/v1` with OpenAPI | Partial | HTTP `openapi_spa_fallback_and_events` | Sonarr-v3 compatibility layer for third-party tools. |
| 22 | Backup and restore | Sonarr `Backup` | `kitsunarr-db::maintenance`, CLI `backup` and `restore`, scheduled job | Done | db maintenance tests, server job tests | Restore from the web UI. |
| 23 | Jellyfin integration | Tanukarr Jellyfin refresh and watched state | — | Not started | — | Phase 6. Watched-state values are preserved by migration. |
| 24 | Localisation | Sonarr/Tanukarr `Localization/*.json` | web `i18n` module | Partial | — | English only. Strings are centralised; more languages are planned. |

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
| TheTVDB, TMDB TV, TVmaze, AniList, MAL/Jikan, TMDB Movies, OMDb | Operational | Provider codes and identities are preserved; adapters not started. |
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
| Download queue, interactive search, add media, metadata sources | Honest "not available yet" pages; the backends are not implemented |
| Keyboard shortcuts (`/` search, `g` + letter navigation, `Esc`), dark and light themes, responsive layout with a mobile drawer, live updates over SSE | Done |
