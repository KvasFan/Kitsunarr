# Source audit: Sonarr and Tanukarr

This is a summary of the source-level analysis Kitsunarr is based on. The detailed notes (data
model, protocols, parser regexes, decision engine, Tanukarr features) are in
[`audit/`](audit/). They quote GPL-3.0-or-later source material, which Kitsunarr is licensed under too.

## Audited revisions

| Source | Revision | Notes |
|---|---|---|
| Sonarr source | `develop` @ `c018578e` (2026-11-02), tagged `v5.0.0.2941` | `package.json` still says 4.0.0, but this is the v5 line (migrations up to 240, `/api/v5`). |
| Sonarr release | `lscr.io/linuxserver/sonarr` 4.0.20.3014-ls4 | Run locally to capture real schemas, API responses and backups. Schema at migration **217**. |
| Tanukarr source | `main` @ `24382f1f` (2026-11-03), version 0.1.5 | Fork of Sonarr (fork point upstream `59c22e8`, merged `57816c1`). |
| Tanukarr release | `ghcr.io/kvasfan/tanukarr:0.1.5` | Run locally; schema at migration **254**; reports `appName: "Sonarr"`, `productName: "Tanukarr"`. |

The detailed reports list every file they cite.

- [`audit/sonarr-data-model.md`](audit/sonarr-data-model.md): schema, JSON encodings, enums, backup layout.
- [`audit/sonarr-parsing-decisions.md`](audit/sonarr-parsing-decisions.md): parser regexes (verbatim), quality and language parsing, decision engine, naming tokens, and an inventory of the test corpus.
- [`audit/sonarr-integrations.md`](audit/sonarr-integrations.md): Newznab/Torznab, the five download clients, completed-download handling, import pipeline, scheduler, notifications, API v3 and security.
- [`audit/tanukarr-metadata.md`](audit/tanukarr-metadata.md): the seven metadata catalogues, identity model, ordering, grouping, movies and collections.
- [`audit/tanukarr-acquisition.md`](audit/tanukarr-acquisition.md): packs, complete-series search, fake-release filter, quality/codec factors, Jellyfin, import lists, library tools and authentication.

## Architecture of the references

Both applications are .NET modular monoliths with the same layout:

- `NzbDrone.Core` holds the business logic: the TV model, parser, decision engine, indexers, download clients, import pipeline, organizer, notifications, metadata, housekeeping and the datastore.
- `Sonarr.Api.V3` and `Sonarr.Api.V5` are ASP.NET controllers.
- `Sonarr.Http` handles authentication and middleware.
- `NzbDrone.Host` is the entry point.
- The frontend is a React/TypeScript SPA that gets real-time updates over SignalR.

Persistence is SQLite (or PostgreSQL) through Dapper, with FluentMigrator migrations recorded in `VersionInfo`.

Key facts:

- **Everything is a `Series` row in Tanukarr.** That includes movies (`MediaType=Movie`, with one S01E01 episode), specials and collection members. Groups and collections are columns on `Series` (`AnimeGroup*`, `CollectionTitle`), not tables.
- **Identity.** Sonarr identifies a series by `TvdbId`. Tanukarr uses `(MetadataSource, ForeignId)` with no unique constraint.
  - `TvdbId` may hold a negative placeholder.
  - `TmdbId` holds a TV id or a movie id depending on the owning provider.
  - `TitleSlug` is not unique.
- **Migration numbering collides.** Tanukarr migrations 233–254 reuse numbers that Sonarr 233–240 use for different changes. A database past 232 is not compatible in either direction, so the schema version alone cannot identify a database. Column introspection and `VersionInfo.Description` can.
- **Stored encodings** (verified on real databases):
  - Columns are PascalCase.
  - JSON is camelCase and pretty-printed.
  - Enum columns are integers; enums inside JSON are camelCase strings.
  - Dates look like `2026-11-09 17:28:34.966664Z`.
  - Booleans are `0/1`, but some legacy rows contain the text `'false'`.
  - Custom-format specifications are stored as `[{type, body}]`.
- **The API hides secrets.** `/api/v3` returns provider password and API-key fields as `********`, so only a backup (or the database) carries them. Tanukarr does not serve backup downloads to API-key requests.
- **Sonarr's cloud services are blocked in Tanukarr.** It blocks `services.sonarr.tv` (scene mappings, the daily-series list). Only TheTVDB still goes through SkyHook; the other six catalogues are called directly, and TMDB and OMDb need user-supplied keys.

## Tanukarr feature status (verified in code)

This summarises the status tables in the two Tanukarr audit reports. "Operational" means the backend code path exists and is wired; it does not mean it was exercised against live services.

| Feature | Status | Notes |
|---|---|---|
| Seven metadata catalogues (TVDB, TMDB TV, TVmaze, AniList, MAL/Jikan, TMDB Movies, OMDb) | Operational | TMDB/OMDb need user keys. MAL/Jikan supplies no alternate titles. Jikan placeholder episodes are recreated on refresh (risk of unlinked files). |
| Per-title metadata ownership | Operational | `MetadataSource` + `ForeignId`; not re-resolved on refresh. |
| Multiple metadata identities | Partial | Several MAL/AniList ids per series are stored, but there is no general multi-identity model. |
| TMDB episode groups | Partial | Applied only for TMDB-TV owners. The UI offers them for any owner. IMDb orderings do nothing. |
| Absolute numbering for season-ordered series | Mostly upstream | The fork adds absolute numbers for TMDB/AniList/MAL owners only. |
| Prequel/sequel grouping | Operational | Strict, bounded AniList/MAL chains; assigned once. |
| Manual grouping | Operational | For every owner; unique indexes plus a season-range trigger. |
| Movies and collections | Operational | Movies are `Series` rows; collections are a title column. |
| Multi-season packs | Operational (diverges from upstream) | On by default. Imports only the grabbed episodes, by copy. "S01 S04" parses as [1,4]. |
| Complete-series search | Operational, torrent-only | Pack scope ranks before quality. RSS never upgrades complete seasons. |
| Interactive season-pack search | Operational | |
| Fake-release filter | Operational, loose | Torrent-only and fails open. Rejections are not blocklisted. "Password-protected" is a filename heuristic. |
| Season-pack trimming | Partial | Packs with skipped episodes stay import-blocked. |
| Codec factors and custom quality presets | Operational | Preset ids ≥ 10000 are retired rather than deleted. |
| Jellyfin refresh and watched sync | Operational | Watched snapshots above 100,000 items never complete. |
| Import lists (AniList custom lists, MAL, Simkl, TMDB/Trakt/MDBList include/exclude) | Operational, live writes unverified | |
| Library tools (pause, previews, retained versions, companion keys) | Mostly operational | "Network-disabled ffprobe" has no code behind it. |

## Consequences for Kitsunarr

1. **One model for identity.** Every title has an owning catalogue and a native id, `(metadata_provider, metadata_id)`, which is unique. External identifiers live in a many-per-kind table, so one TheTVDB series can carry several AniList entries.
2. **Movies are first-class.** Tanukarr's movie rows migrate into the `movies` table, and their single episode file becomes the movie file.
3. **Collections are first-class.** Tanukarr's group and collection columns become `collections` and `collection_members`.
4. **The migration importer is version-adaptive.** It reads every column dynamically and decodes each value leniently, rather than branching on a version number.
5. **Backups are the recommended migration path** because they carry secrets. The API path warns about every masked field.
