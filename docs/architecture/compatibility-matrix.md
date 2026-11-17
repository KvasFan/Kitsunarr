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
| Metadata language (`PreferredMetadataLanguage`, `Series.Language`) | — (series language from v5 resources only) | ✓ | The preferred language and TheTVDB series' languages. |
| Proxy (Settings › General) | ✓ (`/config/host`) | ✓ (`Config` table) | Type, host, port, credentials, bypass list and local bypass. |
| History | ✓ (up to 100,000 newest) | ✓ (all) | |
| Users and passwords | ✗ | ✗ | Kitsunarr creates its own administrator at first run. |
| Blocklist | ✓ (paged `/blocklist`) | ✓ (`Blocklist`) | Entries of imported series and Tanukarr films, with their episodes, quality, protocol and info hash; searches reject them again. |
| Pending releases, queue | ✗ | ✗ | Transient download state: the next RSS sync finds pending releases again; downloads are read from the clients. |

## API compatibility (consumers of Kitsunarr)

| API | Status |
|---|---|
| Native `/api/v1` (OpenAPI 3.1 at `/api/v1/openapi.json`) | Available |
| `X-Api-Key` header, `?apikey=` query and `Authorization: Bearer` | Available (Sonarr-compatible header and query names); companion app keys (Tanukarr's `spr_` tokens) work on the `/api/v3` library reads and series adds only |
| Sonarr `/api/v3` compatibility for Overseerr/Jellyseerr, Prowlarr, Bazarr, Recyclarr, LunaSea, nzb360, Homepage | Partial: `system/status` (reports Sonarr 4.0.0.0 with `appName` Sonarr, plus `productName` Kitsunarr and `kitsunarrVersion`), `health`, `qualityprofile`, `languageprofile` (v4's placeholder), `rootfolder`, `tag`, `series` (list, `?tvdbId=`, get, add from TheTVDB with season choices, update by path or body id, delete), `series/lookup` (`tvdb:ID` with seasons, or a TheTVDB search; library titles come back with their id), `episode` (by series, by ids, one, `episode/monitor`), `episodefile`, `command` (SeriesSearch, SeasonSearch, EpisodeSearch, RefreshSeries, RescanSeries, MissingEpisodeSearch, CutoffUnmetEpisodeSearch, RssSync, RenameFiles, RenameSeries, DownloadedEpisodesScan, RefreshMonitoredDownloads, ManualImport), `queue`, `calendar`, `indexer` (CRUD, schema, test — Prowlarr sync), `release/push` (Prowlarr, autobrr), `history` (paged and filtered, `history/series`, `history/since`), `blocklist` (paged, delete, bulk delete), `wanted/missing` and `wanted/cutoff` (paged, `includeSeries`), `release` (interactive search and grab), `queue/{id}` and `queue/bulk` (removal; ids stable per download), `diskspace`, `parse`, `episodefile/{id}` and `episodefile/bulk` (deletion), `customformat` (CRUD, schema), `qualityprofile` (CRUD, schema, scores for every custom format), `qualitydefinition` (one or bulk `update`), `releaseprofile` (CRUD) and `language`. Tested with Overseerr-, Prowlarr-, mobile-client- and Recyclarr-like flows (HTTP `sonarr_v3_api_serves_overseerr_and_prowlarr_flows`, `sonarr_v3_release_parse_diskspace_and_removals`, `sonarr_v3_settings_serve_recyclarr_sync`), with real Prowlarr 2.7.0.5723 additionally verified for application testing, indexer sync/resync and live Nyaa Torznab search (see [verification](../prowlarr-verification-2026-11-15.md)) and real Recyclarr 7.5.2 and 8.8.1 syncs of TRaSH naming, sizes, profiles and custom formats (see [verification](../recyclarr-verification-2026-11-17.md)); the other tool flows remain simulated. The v3 `system/status` answers `appName: "Sonarr"` (Recyclarr 8 requires it) with `productName: "Kitsunarr"`. The compatibility API includes `X-Application-Version: 4.0.0.0`. SignalR: `/signalr/messages` (negotiate, WebSocket JSON hub protocol) sends `series`, `episodefile`, `episode`, `queue`, `command` and `health` messages (HTTP `signalr_hub_streams_series_changes`). `downloadclient` and `notification` are writable (create, update, delete, schema, test; HTTP `sonarr_v3_download_clients_and_notifications_are_written`). Settings and administration (HTTP `sonarr_v3_config_rules_lists_and_system`): `config/naming`, `config/mediamanagement`, `config/indexer`, `config/downloadclient`, `config/importlist`, `config/ui` and `config/host` (Sonarr's names for Kitsunarr's settings; a `PUT` changes the keys it sends; Sonarr settings Kitsunarr does not have read as Sonarr's defaults; the host's address, port and URL base come from `config.toml`, and the API key and passwords are never returned), `importlist` (CRUD, schema, test; list options as top-level keys), `importlistexclusion` (CRUD, `paged`, `bulk`; TheTVDB ids, other catalogues as `provider`/`externalId`), `delayprofile`, `remotepathmapping`, `tag/{id}` (rename, delete), `tag/detail`, `rootfolder` (add, get, delete), `queue/status`, `queue/details`, `queue/grab/{id}` and `queue/grab/bulk` (pending releases sent now), `system/task`, `system/backup` (list), `log` (paged, from the in-memory log), `rename` (preview) and `manualimport` (a folder's files with their matches). Commands for post-processing scripts: `DownloadedEpisodesScan` (with `path`: the folder's matched files are imported, copied when `importMode` is `Copy`, else moved; without: completed-download handling), `RefreshMonitoredDownloads` and `ManualImport` (chosen `files` with series and episodes) (HTTP `sonarr_v3_manual_import_and_downloaded_episodes_scan`). Commands are tracked: `GET /command` lists the recent ones and `GET /command/{id}` reports started, completed or failed with the outcome. Not served (Sonarr's own web interface uses them): `filesystem`, `metadata` (consumers), `autotagging`, `customfilter`, `update`, `system/restart`/`shutdown`, `mediacover`, `localization`, `naming` examples. |

## Platforms

| Platform | Status |
|---|---|
| Linux x86_64 (glibc), native binary | Built and tested locally (Rust 1.99.1) |
| Linux aarch64 (glibc), native binary | Cross-compiled (`aarch64-linux-gnu-gcc`); run under QEMU user emulation in the arm64 image; released by `release.yml` |
| Docker image (`ghcr.io/kvasfan/kitsunarr`), linux/amd64 | Built and smoke-tested by the CI docker job (reproduced locally with Podman) |
| Docker image, linux/arm64 | Built locally with Podman (`--platform linux/arm64`: the server cross-compiled natively, the runtime stage under QEMU) and checked on 2026-11-17 under QEMU user emulation: start, ping and database, first-run setup and sign-in (a wrong password refused), API key, v3 status, default profiles, tag creation, backup, health, `healthcheck`, `--version`, ffprobe 7.1.5 and a clean stop. Not run on real arm64 hardware. `release.yml` publishes both architectures. |
| Browsers | Chromium and Firefox-based browsers with WebAssembly. Built-in features only; no JavaScript framework. |
