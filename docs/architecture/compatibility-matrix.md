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
| `X-Api-Key` header, `?apikey=` query and `Authorization: Bearer` | Available (Sonarr-compatible header and query names); companion app keys (Tanukarr's `spr_` tokens) work on the `/api/v3` and `/api/v5` library reads and series adds only |
| Sonarr `/api/v3` (Tanukarr 0.1.5's OpenAPI document: 250 operations) | Complete: every operation is served, with Sonarr's resource shapes and status codes, mapped onto the native model (`crates/kitsunarr-api/src/v3*.rs`). `system/status` reports Sonarr 4.0.0.0 with `appName` Sonarr, plus `productName` Kitsunarr and `kitsunarrVersion`; `GET /api` answers `{"current":"v3","deprecated":[]}`. Tanukarr's additions are included (`config/metadatasource`, `series/lookup` with `metadataSource` and `allSources`, `series/grouping`, `series/{id}/folder`, `collection/presentation`, quality presets and codec factors, the identity and anime-grouping fields of `SeriesResource`). Deliberate approximations, listed in technical-decisions §35: commands cannot be cancelled (they never queue), `update` lists only the running version, `system/routes` is a plain list. Exercised with the real Recyclarr 7.5.2 and 8.8.1 and simulated Overseerr, Prowlarr, Bazarr, LunaSea, nzb360 and Homepage requests. |
| Sonarr `/api/v5` (Tanukarr 0.1.5's OpenAPI document: 274 operations) | Complete: every operation is served, built on the v3 handlers with the v5 resource shapes (`connection`, `settings/*`, per-download queue items with `importProgress`, nested releases, media-info streams, `statistics`, `history/season|episode`, Tanukarr's `librarytools/*`, ordering and compression). Every v5 delete answers 204. Companion keys work on v5 with the same rules as v3. |
| Sonarr root routes | `/ping` (anonymous), `/api`, `/content/{path}`, `/feed/v3|v5/calendar/sonarr.ics`, and the form sign-in routes (`/login`, `/login/setup`, `/login/sso`, `/signin-oidc`, `/logout`, `/loggedout`, `/reset-password/request`, `/reset-password`) |

## Platforms

| Platform | Status |
|---|---|
| Linux x86_64 (glibc), native binary | Built and tested locally (Rust 1.99.1) |
| Linux aarch64 (glibc), native binary | Cross-compiled (`aarch64-linux-gnu-gcc`); run under QEMU user emulation in the arm64 image; released by `release.yml` |
| Docker image (`ghcr.io/kvasfan/kitsunarr`), linux/amd64 | Built and smoke-tested by the CI docker job (reproduced locally with Podman) |
| Docker image, linux/arm64 | Built locally with Podman (`--platform linux/arm64`: the server cross-compiled natively, the runtime stage under QEMU) and checked on 2026-11-17 under QEMU user emulation: start, ping and database, first-run setup and sign-in (a wrong password refused), API key, v3 status, default profiles, tag creation, backup, health, `healthcheck`, `--version`, ffprobe 7.1.5 and a clean stop. Not run on real arm64 hardware. `release.yml` publishes both architectures. |
| Browsers | Chromium and Firefox-based browsers with WebAssembly. Built-in features only; no JavaScript framework. |
