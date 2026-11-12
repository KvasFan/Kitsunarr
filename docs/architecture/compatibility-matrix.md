# Compatibility matrix

## Migration sources

| Source | How it is detected | Tested against | Status |
|---|---|---|---|
| Sonarr v4 (schema 217, 4.0.x) | API: `appName = "Sonarr"`, no `productName`. Backup: no Tanukarr columns. | Real 4.0.20.3014 backup and API capture | Supported |
| Sonarr v5 / develop (schema up to 240) | Same as v4. Newer columns are read dynamically. | Schema known from source, not from a real backup | Expected to work; a note is shown for untested schemas |
| Sonarr v3 (schema ≥ 150) | Same as v4. Legacy table names (`Profiles`, `Restrictions`) are recognised. | Normaliser unit tests only | Best effort; upgrading the source to v4 is advised |
| Sonarr < v3 | API rejected (`version < 3`) | — | Not supported |
| Tanukarr 0.1.x (schema 254) | API: `productName = "Tanukarr"`. Backup: `Series.MetadataSource`/`ForeignId` columns. | Real 0.1.5 backup and API capture | Supported |
| PostgreSQL-backed Sonarr | The backup contains no database | — | Use API migration |

## What is imported

| Entity | API source | Backup source | Notes |
|---|---|---|---|
| Series (TV/anime), seasons, episodes, episode files | ✓ | ✓ | Monitoring flags, numbering, file metadata and relative paths are preserved; paths are mapped. |
| Tanukarr movies | ✓ | ✓ | Converted into Kitsunarr movies with one movie file. |
| Tanukarr groups and collection titles | ✓ | ✓ | Become collections. |
| Metadata identities | ✓ | ✓ | Provider-owned identity plus every external id. TVDB placeholders (≤ 0) are dropped. |
| Alternate titles | Sonarr scene titles; Tanukarr only when exposed | ✓ (Tanukarr column, Sonarr `SceneMappings`) | |
| Tags, root folders | ✓ | ✓ | Labels are normalised (lower case, dashes). |
| Quality definitions, quality profiles, custom formats | ✓ | ✓ | Kitsunarr's untouched default profiles take the source definition. Custom quality ids (Tanukarr presets ≥ 10000) are removed with a warning. |
| Indexers, download clients, notifications, import lists | ✓ (secrets masked) | ✓ (with secrets) | Every setting is kept verbatim; implementations Kitsunarr cannot run yet are stored and flagged. |
| Delay profiles, release profiles, remote path mappings, list exclusions | ✓ | ✓ | |
| Naming and media-management settings | ✓ | ✓ (`Config` table) | Unmapped keys are preserved in `migration_unmapped`. |
| Tanukarr retained episode versions (`RetainedVersions`) | — (not in the API) | ✓ | Rows, record snapshots and episodes; the archived files are left in `Plex Versions`. |
| Proxy (Settings › General) | ✓ (`/config/host`) | ✓ (`Config` table) | Type, host, port, credentials, bypass list and local bypass. |
| History | ✓ (up to 100,000 newest) | ✓ (all) | |
| Users and passwords | ✗ | ✗ | Kitsunarr creates its own administrator at first run. |
| Blocklist, pending releases, queue | ✗ | ✗ | Transient download state; not started. |

## API compatibility (consumers of Kitsunarr)

| API | Status |
|---|---|
| Native `/api/v1` (OpenAPI 3.1 at `/api/v1/openapi.json`) | Available |
| `X-Api-Key` header, `?apikey=` query and `Authorization: Bearer` | Available (Sonarr-compatible header and query names); companion app keys (Tanukarr's `spr_` tokens) work on the `/api/v3` library reads and series adds only |
| Sonarr `/api/v3` compatibility for Overseerr/Jellyseerr, Prowlarr, Bazarr, LunaSea | Partial: `system/status` (reports Sonarr 4.0.0.0, plus `kitsunarrVersion`), `health`, `qualityprofile`, `languageprofile` (v4's placeholder), `rootfolder`, `tag`, `series` (list, `?tvdbId=`, get, add from TheTVDB with season choices, update by path or body id, delete), `series/lookup` (`tvdb:ID` with seasons, or a TheTVDB search; library titles come back with their id), `episode` (by series, by ids, one, `episode/monitor`), `episodefile`, `command` (SeriesSearch, SeasonSearch, EpisodeSearch, RefreshSeries, RescanSeries, MissingEpisodeSearch, CutoffUnmetEpisodeSearch, RssSync), `queue`, `calendar`, `indexer` (CRUD, schema, test — Prowlarr sync), `release/push` (Prowlarr, autobrr), `history` (paged and filtered, `history/series`, `history/since`), `blocklist` (paged, delete, bulk delete), `downloadclient` and `notification` (read-only), `wanted/missing` and `wanted/cutoff` (paged, `includeSeries`). Tested with an Overseerr-like and a Prowlarr-like flow (HTTP `sonarr_v3_api_serves_overseerr_and_prowlarr_flows`), not against the real tools. Not implemented: writes to download clients and connections, SignalR. |

## Platforms

| Platform | Status |
|---|---|
| Linux x86_64 (glibc), native binary | Built and tested locally (Rust 1.99.1) |
| Docker image (`ghcr.io/kvasfan/kitsunarr`), linux/amd64 | Dockerfile provided |
| Browsers | Chromium and Firefox-based browsers with WebAssembly. Built-in features only; no JavaScript framework. |
