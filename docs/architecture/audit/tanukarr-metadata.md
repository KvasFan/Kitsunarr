> Audit notes produced during Kitsunarr's source analysis (read-only review of the GPL-3.0 Sonarr and Tanukarr sources, November 2026). Line references point at the audited revisions; see `../source-audit.md` for the summary.

# Tanukarr audit — metadata, identity, ordering, grouping, collections, movies

Scope: what Tanukarr (product version 0.1.5, `VERSION` file; fork of Sonarr `v5-develop`) actually
implements in the metadata/identity/ordering/grouping/collection/movie subsystem, verified against
backend code paths. Acquisition/quality/import/Jellyfin/import-list internals are covered elsewhere and
only touched where they define identity.

Method: static reading of `ref/tanukarr/src` (C#), `ref/tanukarr/frontend/src` (React),
`ref/tanukarr/docs`, `FORK.md`, `README.md`, compared with `ref/sonarr/src` (upstream HEAD
`c018578`, 2026-11-02) and `srcdiff.txt`. Nothing was built or executed. Statements about runtime
behaviour are derived from code; anything not derivable is marked **UNVERIFIED**.

Path abbreviations used below:

- `Core/` = `ref/tanukarr/src/NzbDrone.Core/`
- `Test/` = `ref/tanukarr/src/NzbDrone.Core.Test/`
- `V3/` = `ref/tanukarr/src/Sonarr.Api.V3/`, `V5/` = `ref/tanukarr/src/Sonarr.Api.V5/`
- `FE/` = `ref/tanukarr/frontend/src/`
- `Mig/` = `Core/Datastore/Migration/`
- `Common/` = `ref/tanukarr/src/NzbDrone.Common/`
- In §9, implementation paths without a prefix are relative to `Core/`.

## 0. Key findings (for the Kitsunarr importer)

1. **Version numbers collide**: Tanukarr 233–240 ≠ Sonarr 233–240. Identify the DB by `VersionInfo.Description`
   (class names) or by Tanukarr-only columns (`Series.MetadataSource`, `Series.ForeignId`, `Episodes.ForeignId`, …),
   never by the max version (§1.3). Sonarr's 237 PendingReleases rewrite never ran on Tanukarr DBs.
2. **Everything is still a `Series` row**: TV, anime, movies (`MediaType=Movie`, one S01E01 episode), specials and
   collection members. Groups/collections are columns on `Series` (`AnimeGroup*`, `CollectionTitle`), not entities.
3. **Identity** = `(MetadataSource int, ForeignId string)`; `TvdbId` may be a negative placeholder; `TmdbId` is a TV
   id or a *movie* id depending on owner; no uniqueness on `(MetadataSource, ForeignId)`.
4. **Not in the API**: provider alternate titles (`Series.AlternateTitles`) and native episode ids
   (`Episodes.ForeignId`). An API-only import loses them; a DB import or a provider re-fetch is needed. API omits
   null fields.
5. **Only TheTVDB uses SkyHook**; the other six call TMDB/AniList/Jikan/TVmaze/OMDb directly; TMDB/OMDb need user keys.
   Scene-mapping and daily-series cloud feeds are disabled.
6. **Ordering**: TMDB episode groups work only for TMDB-TV owners (inert `OrderingId` stored for others); "Absolute" is
   not special-cased; IMDb ordering is UI-only and breaks TMDB owners if chosen.
7. **Grouping**: strict, bounded AniList/MAL prequel/sequel chains and TMDB movie collections are real and assigned
   once; manual grouping works for all owners; group paths are generated only on add.
8. **Doc overstatements / defects**: alternate titles never refresh (and only AniList supplies them); Jikan placeholder
   episodes are recreated (file links at risk) when titles arrive; Kodi NFO/webhooks leak negative TVDB ids; stale docs
   still say "four providers" and "no TMDB movie endpoint" (§10).

---

## 1. Schema deltas

### 1.1 Tanukarr migrations 233–254 (exact DDL)

All are FluentMigrator `NzbDroneMigrationBase` subclasses (`MainDbUpgrade`). Types below are
FluentMigrator types (`AsString()` = `TEXT` on SQLite / `text` on PostgreSQL; `AsString(n)` = `TEXT`
on SQLite via Sonarr's type map, `varchar(n)` on PostgreSQL; `AsInt32` = `INTEGER`; `AsInt64` =
`INTEGER`/`bigint`; `AsBoolean` = `INTEGER`/`boolean`; `AsDateTime` = `DATETIME`/`timestamp`).

| # | Class (= `VersionInfo.Description`) | Exact changes | Domain |
|---|---|---|---|
| 233 | `add_metadata_source_to_series` (`Mig/233_add_metadata_source_to_series.cs`) | `Series.MetadataSource` INT32 NOT NULL DEFAULT 0; `Series.ForeignId` STRING NULL. Data: `UPDATE Series SET ForeignId = CAST(TvdbId AS VARCHAR) WHERE TvdbId > 0`. No index on either column. | identity |
| 234 | `add_episode_ordering` | `Series.OrderingId` STRING NULL; `Episodes.ForeignId` STRING NULL. Data: `UPDATE Episodes SET ForeignId = CAST(TvdbId AS VARCHAR) WHERE TvdbId > 0`. No index. | ordering / episode identity |
| 235 | `add_alternate_titles_to_series` | `Series.AlternateTitles` STRING NULL (JSON array of strings via Sonarr's embedded-document converter). Comment claims "Populated on the next refresh" — **false**, see §7. | titles |
| 236 | `quality_profile_quality_ranks` | Identical (except number) to **Sonarr 233**: table `QualityProfileQualityRanks` (Id PK, ProfileId INT32 NN, QualityId INT32 NN, Score DOUBLE NN) + unique index (ProfileId, QualityId); expression indexes `IX_History_QualityId`, `IX_EpisodeFiles_QualityId`, `IX_Blocklist_QualityId` on `json_extract(Quality,'$.quality')` (SQLite) / `(Quality::jsonb->>'quality')::int` (PG). | upstream (renumbered) |
| 237 | `preserve_source_release_context` | `EpisodeFiles.SourceReleaseSize` INT64 NULL; `EpisodeFiles.SourceReleaseTitle` STRING(max) NULL; backfill from `History` (grab EventType 1 joined to import EventType 3 by DownloadId/SeriesId/EpisodeId, newest wins). | acquisition (other agent) |
| 238 | `add_pending_path_to_series` | Identical to **Sonarr 234**: `Series.PendingPath` STRING NULL. | upstream (renumbered) |
| 239 | `retained_episode_versions` | New table `RetainedVersions` (Id INT32 PK identity, SeriesId INT32 NN, RelativePath STRING(max) NN, MetadataJson STRING(max) NN, EpisodeIdsJson STRING(max) NN, Created DATETIME NN) + index on SeriesId. | library tools |
| 240 | `preserve_subtitle_language_codes` | `SubtitleFiles.LanguageCode` STRING(64) NULL. | extras |
| 241 | `import_list_tag_existing_type` | Identical to **Sonarr 236**: `ImportLists.TagExisting` → INT32 NOT NULL DEFAULT 0 (SQLite AlterColumn; PG `ALTER ... TYPE integer USING TagExisting::integer`). | upstream (renumbered) |
| 242 | `preserve_import_list_exclusion_identity` | `ImportListExclusions.MetadataSource` INT32 NN DEFAULT 0; `ImportListExclusions.ForeignId` STRING NULL. | identity (lists) |
| 243 | `persistent_jellyfin_refresh_queue` | Table `JellyfinRefreshStates` (Id PK, NotificationId INT32 NN, TargetFingerprint STRING(64) NN, StateToken STRING(32) NN, LastRefreshMode INT32 NN DEF 1, ImportedGeneration/ImmediateGeneration/TickGeneration INT64 NN DEF 0, LastTickUtc DATETIME NN) + unique index NotificationId. | Jellyfin (other agent) |
| 244 | `anime_series_grouping` | `Series.AnimeGroupingMode` INT32 NN DEF 0; `AnimeGroupId` STRING(96) NULL; `AnimeGroupTitle` STRING(180) NULL; `AnimeGroupSeasonNumber` INT32 NULL; `AnimeGroupFolder` STRING(180) NULL. Unique index `IX_Series_AnimeGroupSlot` (AnimeGroupId, AnimeGroupSeasonNumber) — dropped again in 245. | grouping |
| 245 | `anime_group_movies` | `Series.AnimeGroupMemberKind` INT32 NN DEF 0; `AnimeGroupKind` INT32 NN DEF 0; `AnimeGroupMovieFolderStyle` INT32 NN DEF 0; `AnimeGroupMovieNumber` INT32 NULL; `AnimeGroupMemberFolder` STRING(180) NULL. Drops `IX_Series_AnimeGroupSlot`; creates UNIQUE `IX_Series_AnimeGroupSeasonSlot` (AnimeGroupId, AnimeGroupMemberKind, AnimeGroupSeasonNumber) and UNIQUE `IX_Series_AnimeGroupMovieSlot` (AnimeGroupId, AnimeGroupMemberKind, AnimeGroupMovieNumber). NULLs make the indexes inert for ungrouped rows. | grouping |
| 246 | `anime_group_naming` | `NamingConfig.AnimeGroupFolderFormat` STRING NN DEF `{Group Title}`; `AnimeGroupSeasonFolderFormat` DEF `Season {Group Season:00}`; `AnimeGroupMovieFolderFormat` DEF `{Series Title}`; `AnimeGroupMovieSagaFolderFormat` DEF `Film {Group Movie:00}`. | grouping naming |
| 247 | `anime_group_arc_naming` | `NamingConfig.AnimeGroupArcFolderFormat` STRING NN DEF `{Group Arc Title}`. | grouping naming |
| 248 | `jellyfin_watched_state` | `Episodes.Watched` BOOL NN DEF false, `WatchedAtUtc` DATETIME NULL, `WatchedSource` STRING(64) NULL, `JellyfinUnmonitorOwner` STRING(64) NULL; table `JellyfinWatchStates` (Id PK, NotificationId INT32 NN, TargetFingerprint STRING(64) NN, LastSyncUtc DATETIME NN, UnmonitorWatchedEpisodes BOOL NN DEF false) + unique index NotificationId. | Jellyfin (other agent) |
| 249 | `import_list_progress_queue` | Table `ImportListProgressStates` (Id PK, ImportListId, SeriesId INT32 NN, TargetFingerprint STRING(64) NN, PendingProgress/AcknowledgedProgress INT32 NN DEF 0, AcknowledgedIdentityFingerprint STRING(64) NN DEF '', FailureCount INT32 NN DEF 0, NextAttemptUtc DATETIME NN, LastAttemptUtc DATETIME NULL) + unique (ImportListId, SeriesId). | lists (other agent) |
| 250 | `import_list_metadata_source` | `ImportLists.PreferredMetadataSource` INT32 NN DEF 0. **Note:** stores `ImportListMetadataSource` (Automatic=0, Tvdb=1, Tmdb=2, AniList=3, MyAnimeList=4), a *different* numbering from `MetadataSourceType` (`Core/ImportLists/ImportListMetadataSource.cs`). | identity (lists) |
| 251 | `custom_quality_presets` | `QualityDefinitions.Source` INT32 NULL, `Resolution` INT32 NULL, `Codec` STRING NN DEF '', `TitlePattern` STRING NN DEF '', `IsRetired` BOOL NN DEF false; `QualityProfiles.CodecMultipliers` STRING NN DEF `{}`. | quality (other agent) |
| 252 | `series_collection_title` | `Series.CollectionTitle` STRING(512) NULL. | collections |
| 253 | `movie_metadata_and_special_members` | `Series.MediaType` INT32 NN DEF 0; `Series.AnimeGroupSpecialNumber` INT32 NULL. Data: `UPDATE Series SET MediaType=1 WHERE AnimeGroupMemberKind=1 AND MetadataSource IN (2,3) AND (exactly one Episodes row) AND that row is S01E01`. UNIQUE `IX_Series_AnimeGroupId_Special` (AnimeGroupId, AnimeGroupMemberKind, AnimeGroupSpecialNumber). `NamingConfig.MovieFormat` STRING NN DEF `{Series Title} ({Series Year}) {Quality Full}`. **Triggers:** SQLite `TR_Series_CollectionSeasonRange_INSERT` / `_UPDATE` (BEFORE INSERT/UPDATE ON Series; `RAISE(ABORT,'Collection season range overlaps: Series.AnimeGroupSeasonNumber')` when another non-Disabled Season member of the same AnimeGroupId has an overlapping range `[AnimeGroupSeasonNumber, +count(distinct positive seasonNumber in Seasons JSON))`); PostgreSQL function `tanukarr_validate_collection_season_range()` + trigger `TR_Series_CollectionSeasonRange` (takes `pg_advisory_xact_lock(hashtextextended(AnimeGroupId,0))`, raises SQLSTATE 23505 with constraint name `IX_Series_AnimeGroupSeasonRange`). | movies / grouping |
| 254 | `series_title_slug_not_unique` | Identical to **Sonarr 240**: drop unique index on `Series.TitleSlug`, recreate non-unique. | upstream (renumbered) |

Fixtures: `Test/Datastore/Migration/{233,234,236,237,242,243,244,245,246,247,248,250,251,252,253,254}_*Fixture.cs`
(no fixtures for 235, 238, 239, 240, 241, 249).

### 1.2 Upstream Sonarr 233–240 (reference `ref/sonarr`)

| # | Sonarr class (= `VersionInfo.Description`) | Change | Tanukarr equivalent |
|---|---|---|---|
| 233 | `quality_profile_quality_ranks` | `QualityProfileQualityRanks` table + 3 expression indexes | Tanukarr **236** (identical) |
| 234 | `add_pending_path_to_series` | `Series.PendingPath` | Tanukarr **238** (identical) |
| 235 | — (no migration) | — | — |
| 236 | `import_list_tag_existing_type` | `ImportLists.TagExisting` → int | Tanukarr **241** (identical) |
| 237 | `pending_releases_season_numbers` | Data-only: rewrites `PendingReleases.ParsedEpisodeInfo` JSON, moving `seasonNumber` → `seasonNumbers` array | **None** — Tanukarr never runs it. Tanukarr's `ParsedEpisodeInfo` serialises both `seasonNumber` (legacy, `[JsonPropertyOrder(-1)]`) and `seasonNumbers` (`Core/Parser/Model/ParsedEpisodeInfo.cs:23-31`) and also carries movie fields (`MovieYear`, `MovieImdbId`, `IsMovieCollectionPack`, ...). |
| 238, 239 | — (no migrations) | — | — |
| 240 | `series_title_slug_not_unique` | `Series.TitleSlug` index non-unique | Tanukarr **254** (identical) |

FORK.md confirms the renumbering ("upstream quality-rank migration remains numbered 236 here; the
upstream import-list tag migration is numbered 241").

### 1.3 Telling a Tanukarr DB from a Sonarr DB at the same version

`Database.cs` reads `SELECT Version FROM VersionInfo ORDER BY Version DESC LIMIT 1`, so the max
version alone is ambiguous (233–240). FluentMigrator 8.0.1 (`Core/Sonarr.Core.csproj`) uses the default
`VersionInfo(Version, AppliedOn, Description)` table and writes the migration class name into
`Description` when the `[Migration]` attribute has no description (none of these do) — standard
FluentMigrator behaviour, **UNVERIFIED on a real DB file**. Reliable discriminators, best first:

1. `VersionInfo` rows — Tanukarr: `(233,'add_metadata_source_to_series')`, `(234,'add_episode_ordering')`,
   `(235,'add_alternate_titles_to_series')`, rows 235/238/239 exist; Sonarr: `(233,'quality_profile_quality_ranks')`,
   `(234,'add_pending_path_to_series')`, no rows 235/238/239, `(237,'pending_releases_season_numbers')`,
   `(240,'series_title_slug_not_unique')`.
2. Columns: `Series.MetadataSource` / `Series.ForeignId` (≥233), `Episodes.ForeignId` + `Series.OrderingId`
   (≥234), `Series.AlternateTitles` (≥235), `RetainedVersions` table (≥239), `SubtitleFiles.LanguageCode`
   (≥240), `Series.AnimeGroup*` (≥244), `Series.MediaType` (≥253). None exist in Sonarr.
3. Version > 240 ⇒ Tanukarr (Sonarr's newest is 240 in the reference tree).
4. Config table keys (lower-cased by `ConfigService.SetValue`): `tmdbapikey`, `omdbapikey`,
   `metadatasourceorder`, `disabledmetadatasources`, `prooptions`, `prosearchhistory`, `proaccesskeys`,
   `enablefakereleaseprotection`, `enablemultiseasonreleases`, `skipunmonitoredepisodesfrompacks`,
   `stalledtorrenttimeout` (only present once written).

Cross-opening hazards (derived from FluentMigrator `MigrateUp()` applying every version missing from
`VersionInfo`; **UNVERIFIED by execution**): a Sonarr DB at 240 opened by Tanukarr would *skip*
Tanukarr 233/234/236/237/240 (rows exist with different meaning → `MetadataSource`, `ForeignId`,
`OrderingId`, `Episodes.ForeignId`, `SourceRelease*`, `SubtitleFiles.LanguageCode` never created) and
*run* 235/238/239 — 238 would fail with a duplicate `PendingPath` column. A Tanukarr DB opened by Sonarr
would never get Sonarr 237's PendingReleases JSON rewrite. An importer must therefore key on
`VersionInfo.Description`/columns, never on the version number.

### 1.4 Inherited migrations below 233

Tanukarr did **not** alter any inherited migration. The three files reported as different in
`srcdiff.txt` are newer upstream edits that Tanukarr has not merged (Tanukarr keeps the older upstream
text; its shallow git history shows only upstream authors on these files):

- `208_metadata_translations.cs`: upstream now also runs `UPDATE Series SET LastInfoSync='2026-01-01'`
  (forces a refresh); Tanukarr lacks that line. No schema effect.
- `225_mediainfo_multiple_streams.cs`: upstream rewrote it as batched `JsonObject` processing; Tanukarr keeps
  the older typed `MediaInfo224→225` conversion (writes indented JSON). Same resulting shape.
- `Framework/MigrationController.cs`: upstream masks PostgreSQL passwords in the log line; Tanukarr logs the raw
  connection string.

Non-schema DB facts relevant to identity: `Series.TvdbId` keeps its UNIQUE index (`Mig/019_restore_unique_constraints.cs`);
there is **no** unique index on `(MetadataSource, ForeignId)` and no API validator for it (see §3.5).

---

## 2. The seven metadata providers

### 2.1 Common plumbing

| Piece | File | Behaviour |
|---|---|---|
| Provider enum | `Core/MetadataSource/MetadataSourceType.cs` | `Tvdb=0, Tmdb=1, AniList=2, MyAnimeList=3, TvMaze=4, TmdbMovie=5, Omdb=6`. Persisted as int in `Series.MetadataSource` / `ImportListExclusions.MetadataSource`; serialised in the API as camelCase strings `tvdb, tmdb, aniList, myAnimeList, tvMaze, tmdbMovie, omdb` (STJson `JsonStringEnumConverter(CamelCase)`). |
| Provider contract | `Core/MetadataSource/IMetadataProvider.cs` | `Source`, `GetSeriesInfo(Series)` (keyed on `Series.ForeignId`), `SearchForNewSeries(string title)` — **no language parameter**. Deliberately does not extend upstream `IProvideSeriesInfo`/`ISearchForNewSeries` (only `SkyHookProxy` implements those; pinned by `Test/MetadataSource/MetadataProviderRegistrationFixture.cs`). |
| Factory | `Core/MetadataSource/MetadataProviderFactory.cs` | `GetProvider(series.MetadataSource)`; an unregistered source silently **falls back to Tvdb** with a warning. |
| Search dispatcher | `Core/MetadataSource/MetadataSearchService.cs` | Explicit source must be enabled (else `ArgumentException`). `allSources` iterates `MetadataSourcePolicy.Enabled` order; TVDB is called through upstream `ISearchForNewSeries.SearchForNewSeries(term, language)` (language-aware), every other provider through `SearchForNewSeries(term)`. In all-sources mode `NzbDroneException`/`HttpException`/`WebException` per source are logged and skipped; the last failure is rethrown only if no source succeeded. Results are **concatenated per source, not re-ranked globally**. |
| Refresh | `Core/Tv/RefreshSeriesService.cs` | `_metadataProviderFactory.GetProvider(series).GetSeriesInfo(series)`; copies TvRageId/TvMazeId/TmdbId/**MediaType**/ImdbId/MalIds/AniListIds etc.; overwrites `TvdbId` only if provider value > 0; runs `SeriesPresentationService.Enrich`; runs automatic grouping only when `AnimeGroupId` is empty. Does **not** copy `AlternateTitles`, `MetadataSource`, `ForeignId`, `OrderingId`. |
| Add | `Core/Tv/AddSeriesService.cs` | Fetches full metadata from the owning provider, enriches presentation, `ApplyChanges(request)`, sets `OrderingId` from the request, applies grouping, allocates a negative placeholder `TvdbId` if 0 (§3.3). Import-list batch adds stop calling a provider after its first `TooManyRequestsException`. |
| Optional presentation enrichment | `Core/MetadataSource/SeriesPresentationService.cs`, `SeriesPresentationCache.cs` | If overview/poster/fanart missing, ask up to **2** *other enabled* providers implementing `ISeriesPresentationProvider` (SkyHook, TmdbProxy, AniListProxy, JikanProxy) using only verified cross-IDs (keys `tvdb:{id}:{lang}:{seasonType}`, `tmdb:{tvId}` (never for movies), `anilist:{id}`, `anilist:mal:{malId}`, `jikan:{malId}`). Fills only missing Overview/Poster/Fanart; images restricted to trusted HTTPS hosts (`s4/s3/s.anilist.co`, `cdn.myanimelist.net`, `image.tmdb.org`, `artworks/banners.thetvdb.com`). In-memory cache: 512 entries, 32 KiB/entry, 8 MiB total, 15 min success / 2 min miss, 2 min per-provider cooldown after an exception. Also preserves the previous overview/images when a refresh response from the same identity is empty. |

Only **TheTVDB is proxied via Sonarr's SkyHook** (`https://skyhook.sonarr.tv/v2/tvdb/{route}/`,
`Common/Cloud/SonarrCloudRequestBuilder.cs`). All other providers call their public APIs directly.
`services.sonarr.tv` (scene mappings, daily-series list) and `sentry.sonarr.tv` are **blocked** by
`Common/Cloud/SonarrCloudRequestPolicy.cs` (synthetic HTTP 403); `SceneMappingService` keeps existing
mappings and `DailySeriesDataProxy` returns nothing, so no TVDB series is auto-typed Daily. XEM
(`https://thexem.info/map/`) still works, but `XemService.PerformUpdate` now skips `TvdbId <= 0`.

### 2.2 Per-provider detail

#### TheTVDB (`MetadataSourceType.Tvdb`)

| Aspect | Implementation |
|---|---|
| Classes | `Core/MetadataSource/SkyHook/SkyHookProxy.cs` (upstream; now also implements `IMetadataProvider`, `ISeriesPresentationProvider`), `SkyHook/Resource/*`. |
| Endpoints | Search `GET https://skyhook.sonarr.tv/v2/tvdb/search/?language={iso639-3}&term={lowercased term}`; details `GET .../shows/{iso639-3}/{tvdbId}/{seasonType}`; presentation the same `shows` route with 5 s timeout. `tvdb:`/`tvdbid:` prefixes are resolved client-side (library hit returned as-is); `imdb:`, `tmdb:`, `anilist:`, `mal:` prefixed terms are passed to SkyHook's search (`SearchForNewSeriesBy*Id`). |
| Auth | None (shared Sonarr infrastructure). No user key setting. |
| Rate limit / cache | None client-side (upstream behaviour). |
| Mapping | Upstream mapping plus `MetadataSource=Tvdb`, `ForeignId=TvdbId.ToString()`. Translations, actors, season types, OriginalTitle, MalIds/AniListIds/TmdbId/TvMazeId/ImdbId all from SkyHook. Episodes carry `TvdbId`; `Episode.ForeignId` is **not** set (only legacy rows backfilled by migration 234 have it). |
| Numbering | TVDB numbering for the series' upstream `SeasonType` (official/dvd/absolute/alternate…, the upstream v5 "Episode Order" feature, still functional for TVDB owners); TVDB absolute numbers. |
| Artwork / ratings | SkyHook images and rating (upstream). |
| Failure | Search: `SkyHookException`. Details: 404 → `SeriesNotFoundException`, else `HttpException`. |
| Tests | `Test/MetadataSource/SkyHook/SkyHookProxyFixture.cs`, `SkyHookProxySearchFixture.cs`. |

#### TMDB TV (`MetadataSourceType.Tmdb`)

| Aspect | Implementation |
|---|---|
| Classes | `Core/MetadataSource/Tmdb/TmdbProxy.cs` (`IMetadataProvider`, `IProvideEpisodeOrderings`, `ISeriesPresentationProvider`), `TmdbEpisodeGroupProjector.cs`, `TmdbEpisodeGroupType.cs`, `TmdbApiKeyMissingException.cs`, `Resource/TmdbSeriesResource.cs`, `TmdbEpisodeResource.cs`, `TmdbEpisodeGroupResource.cs`. Newtonsoft `[JsonProperty("snake_case")]` resources. |
| Endpoints (base `https://api.themoviedb.org/3`) | `GET /tv/{id}?append_to_response=external_ids,content_ratings`; `GET /tv/{id}/season/{n}` once per listed season (incl. season 0); `GET /tv/{id}/episode_groups` (ordering list); `GET /tv/episode_group/{groupId}` (applied ordering); `GET /search/tv?query=`; presentation `GET /tv/{id}` (5 s). No `language` parameter is sent (TMDB default locale). |
| Auth | User-supplied **TMDB v3 API key** from config key `TmdbApiKey`, sent as `api_key` query param. No built-in key. |
| Rate limit / cache | None (no `RateLimit`, no cache). |
| Search | `/search/tv`, results sorted by `SearchSeriesComparer`. No key → empty list; HTTP error → empty list. No ID-prefix syntax. |
| Mapping | `MetadataSource=Tmdb`, `ForeignId=TmdbId=id`, `TitleSlug="tmdb-{id}"`, `TvdbId` from `external_ids.tvdb_id` when > 0, `ImdbId` from external ids, Network = first network, Status (`Ended`/`Canceled`→Ended, `Returning Series`→Continuing, `In Production`/`Planned`/`Pilot`→Upcoming), Genres, Runtime (`episode_run_time[0]`), OriginalLanguage/OriginalCountry, FirstAired/Year/LastAired, Ratings (`vote_average`/`vote_count`), Certification = US content rating, `SeasonType=official`, single `SeasonTypes` entry. **No AlternateTitles, no translations, no actors.** Episodes: `ForeignId`=TMDB episode id, title, overview, runtime, air date, ratings, still image. |
| Numbering | TMDB default season/episode numbers, or the projected numbering of the chosen episode group (§5). Absolute numbers are **synthesised** for every season>0 episode in (season, episode) order after projection. Seasons are rebuilt from the final episode list (monitored = previous value or `season > 0`; the TMDB season names set in `MapSeries` are discarded by this rebuild). |
| Artwork | Poster + Fanart from `poster_path`/`backdrop_path` at `https://image.tmdb.org/t/p/original…`; episode stills as `Screenshot`. No season posters. |
| Failure | No key → `TmdbApiKeyMissingException`; 404 → `SeriesNotFoundException`; 401 → `TmdbApiKeyMissingException("rejected")`; other → `HttpException`. Group fetch failure during refresh is logged and **rethrown** (refresh aborts, existing numbering kept). Group *list* failure → default-only list. |
| Tests | `Test/MetadataSource/Tmdb/TmdbEpisodeGroupProjectorFixture.cs`, `TmdbResourceDeserializationFixture.cs`; presentation in `SeriesPresentationProviderFixture.cs`. No fixture exercises `TmdbProxy.GetSeriesInfo` end-to-end with an ordering. |

#### TVmaze (`MetadataSourceType.TvMaze`)

| Aspect | Implementation |
|---|---|
| Classes | `Core/MetadataSource/TvMaze/TvMazeProxy.cs`, `TvMazeRateLimiter.cs`, `TvMazeException.cs`, `Resource/TvMazeResource.cs`. |
| Endpoints (base `https://api.tvmaze.com`) | `GET /search/shows?q={escaped}`; `GET /shows/{id}`; `GET /shows/{id}/episodes?specials=1`. `tvmaze:{id}` search term → direct `/shows/{id}`. |
| Auth | None. |
| Rate limit | Process-wide limiter: one request per 600 ms; HTTP 429 → back off `Retry-After` (default 5 s, capped 5 min), one retry; waits max 10 s for a slot else `RequestLimitReachedException`. 15 s timeout. |
| Cache | None. Not a presentation provider. |
| Mapping | `ForeignId=TvMazeId=id`, `TitleSlug="tvmaze-{id}"`, `TvdbId=externals.thetvdb` (≥0), `TvRageId`, `ImdbId` (normalised), summary HTML stripped, Status (`Ended`→Ended, `In Development`→Upcoming, else Continuing), Genres, Network or WebChannel, Runtime (`averageRuntime`/`runtime`), OriginalLanguage by language name, `Ratings.Value=rating.average`, Votes 0. Search returns ≤20 distinct shows. |
| Numbering | TVmaze season/number. Specials without a number go to **season 0, numbered 1..n ordered by TVmaze episode id**. **No absolute numbers.** `Episode.ForeignId` = TVmaze episode id. Seasons are derived from the episodes, all `Monitored=true` (season 0 included). Episode list validated (≤20 000 rows, no duplicate ids, no duplicate (season, number), no negative season, empty list rejected if series already has seasons) else `TvMazeException` and existing episodes are kept. |
| Artwork | Poster only, only if HTTPS on `static.tvmaze.com`. |
| Failure | 404 → null → `SeriesNotFoundException`; other errors `HttpException`; search null → `TvMazeException`. |
| Tests | `Test/MetadataSource/TvMaze/TvMazeProxyFixture.cs`, `TvMazeRateLimiterFixture.cs`. |

#### AniList (`MetadataSourceType.AniList`)

| Aspect | Implementation |
|---|---|
| Classes | `Core/MetadataSource/AniList/AniListProxy.cs` (`IMetadataProvider`, `IAnimeRelationProvider`, `ISeriesPresentationProvider`), `AniListQueries.cs`, `AniListEpisodeMapper.cs`, `AniListMetadataCache.cs`, `AniListPublicRelationSnapshot.cs`, `Resource/AniListMediaResource.cs`; shared transport `Core/AniList/AniListRequestService.cs`, `AniListRequestDelay.cs` (also used by AniList import lists). |
| Endpoint | `POST https://graphql.anilist.co` (JSON `{query, variables}`). Queries (`AniListQueries.cs`): `MediaById($id,$page)` = `Media(id,type:ANIME){id idMal type title{romaji english native userPreferred} synonyms description(asHtml:false) format status episodes duration startDate endDate countryOfOrigin genres averageScore popularity coverImage{extraLarge large} bannerImage studios{nodes{name isAnimationStudio}} relations{edges{relationType(version:2) node{id type format}}} airingSchedule(page:$page,perPage:50){pageInfo{currentPage hasNextPage} nodes{episode airingAt}}}`; `AiringScheduleById($id,$page)` (pages 2..20); `RelationsById($id)`; `PresentationById($id,$idMal)` (`Media(id:,idMal:)`); `SearchByTitle($search)` = `Page(page:1,perPage:20){media(search:,type:ANIME){…}}`. |
| Auth | None for metadata (public GraphQL). |
| Rate limit | `AniListRequestService` (shared lock + state across instances): min spacing 2.1 s (`IRateLimitService` key `graphql.anilist.co`), widened from `X-RateLimit-Limit` (61 s / limit) and observed `X-RateLimit-Remaining`/`X-RateLimit-Reset`; HTTP 429 → cooldown from `Retry-After`/reset (default 60 s), ≤3 attempts within 2 min; optional (presentation) calls: 1 attempt, 5 s budget, 2 s lock wait. Throws a sanitized `TooManyRequestsException`. |
| Cache | `AniListMetadataCache`: in-memory, 512 entries, ≤512 KiB/entry, `media:{id}` 5 min (only cached when the airing schedule was read completely), `relations:{id}` 15 min; single-flight per key; returns deep copies. Relation nodes can also come from an import-list `AniListPublicRelationSnapshot` scope. |
| Search | Title search only (no ID-prefix syntax), sorted by `SearchSeriesComparer` (uses alternate titles); `HttpException` (incl. 429) → empty list. |
| Mapping | `ForeignId=id`, `AniListIds={id}`, `MalIds={idMal}`, `TitleSlug="anilist-{id}"`, Title = first of [english, romaji, userPreferred, native, synonyms…] (deduped), **`AlternateTitles` = the rest**, `SeriesType=Anime`, `MediaType` = Movie for `MOVIE`, Special for `SPECIAL`/`OVA`, else Series (and `AnimeGroupMemberKind` likewise), Status (`FINISHED`/`CANCELLED`→Ended, `RELEASING`/`HIATUS`→Continuing, `NOT_YET_RELEASED`→Upcoming), Genres, Runtime=`duration`, Network = animation studio, OriginalCountry, OriginalLanguage from country (CN/TW→Chinese, KR→Korean, else Japanese), Ratings = `averageScore/10` with **Votes = popularity**, overview HTML-stripped. `TvdbId` never set (placeholder later). |
| Numbering | Single season 1, episodes 1..N where N = max(`episodes`, highest airing-schedule episode) (`MOVIE` ≥1), `AbsoluteEpisodeNumber = EpisodeNumber`, `Episode.ForeignId="{anilistId}:{n}"`, **no episode titles/overviews**, air dates from schedule (≤20×50 entries). |
| Artwork | Poster = `coverImage.extraLarge`/`large`; Fanart = `bannerImage`. |
| Failure | GraphQL `errors`, wrong id, page mismatch, missing `hasNextPage` → `HttpException(503 "AniList returned incomplete metadata")`; 404 → `SeriesNotFoundException`. |
| Tests | `Test/MetadataSource/AniList/AniListProxyFixture.cs`, `AniListEpisodeMapperFixture.cs`, `AniListMetadataCacheFixture.cs`, `Test/AniList/AniListRequestServiceFixture.cs`, `Test/Files/Metadata/AniList/official-group-relations.json`. |

#### MyAnimeList via Jikan (`MetadataSourceType.MyAnimeList`)

| Aspect | Implementation |
|---|---|
| Classes | `Core/MetadataSource/Jikan/JikanProxy.cs` (`IMetadataProvider`, `IAnimeRelationProvider`, `ISeriesPresentationProvider`), `JikanRateLimiter.cs`, `Resource/JikanResource.cs`. |
| Endpoints (base `https://api.jikan.moe/v4`) | `GET /anime?q={escaped}` (search; deliberately no other params); `GET /anime/{malId}/full` (details, relations, presentation); `GET /anime/{malId}/episodes?page={n}` (≤30 pages). |
| Auth | None (public Jikan; not the official MAL API — that is only used by the separate MAL import list). |
| Rate limit | Sliding window 3 req/s and 60 req/min (process-wide); 429 → back off `Retry-After` (default 5 s); ≤3 attempts within a 30 s deadline; a cooldown > 30 s throws `RequestLimitReachedException`; presentation waits ≤5 s. |
| Cache | None (only the presentation cache). |
| Search | Filters `mal_id>0`, non-empty title, drops `Rx` (hentai) ratings, dedupes, takes 20, sorts with `SearchSeriesComparer`; `HttpException`/rate-limit → empty. No ID-prefix syntax. |
| Mapping | `ForeignId=malId`, `MalIds={malId}`, `TitleSlug="mal-{id}"`, Title = `title_english` else `title`, Overview = synopsis, `SeriesType=Anime`, MediaType Movie/Special(`Special`,`OVA`)/Series, Status (`Finished Airing`→Ended, `Currently Airing`→Continuing, `Not yet aired`→Upcoming), Genres, Runtime parsed from prose ("24 min per ep", "1 hr 30 min"), Network = first studio, OriginalLanguage Japanese, OriginalCountry JP, Ratings = `score`/`scored_by`, Certification = MAL rating string upper-cased. **No AlternateTitles** (even though `titles`/`title_japanese` are deserialised). |
| Numbering | Single season 1; episode number = Jikan episode `mal_id` (position fallback); absolute = same; titles and aired dates from Jikan; missing numbers up to the declared `episodes` count are filled with untitled placeholders (`Movie` ≥1). **`Episode.ForeignId` format is inconsistent**: listed episodes use `"{episode mal_id}"` (e.g. `"5"`), placeholders use `"{animeMalId}:{n}"` (e.g. `"1735:5"`). Because `RefreshEpisodeService` refuses to match two rows whose different non-null ForeignIds share a number (`Test/TvTests/RefreshEpisodeProviderIdentityFixture.should_not_attach_a_different_known_episode_file_to_a_reused_number`), a placeholder row is deleted and recreated when the real episode later appears, dropping its `EpisodeFileId` — a likely data-loss path for the "titles arrive on a later refresh" behaviour FORK.md advertises (**derived from code; not covered by any test; UNVERIFIED at runtime**). |
| Artwork | Poster only (jpg/webp large → regular). |
| Failure | 404 → `SeriesNotFoundException`; episode-list HTTP/rate-limit failures → keep what was read + placeholders. |
| Tests | `Test/MetadataSource/Jikan/JikanContractFixture.cs`, `JikanMovieFixture.cs`, `JikanRateLimiterFixture.cs`. |

#### TMDB Movies (`MetadataSourceType.TmdbMovie`)

| Aspect | Implementation |
|---|---|
| Classes | `Core/MetadataSource/Tmdb/TmdbMovieProxy.cs` (`IMetadataProvider`, `IMovieCollectionProvider`), `Resource/TmdbMovieResource.cs`, `Core/MetadataSource/MovieMetadata.cs`, `MovieMetadataException.cs`. |
| Endpoints | `GET /3/movie/{id}`; `GET /3/find/{tt…}?external_source=imdb_id` (search term `tt…` or `imdb:tt…`); `GET /3/search/movie?query=`; `GET /3/collection/{collectionId}` (grouping). `tmdb:{id}`/`tmdbmovie:{id}` terms resolve directly. |
| Auth | Same user-supplied `TmdbApiKey` (`api_key` param). |
| Rate limit | `RateLimit=250 ms` keyed `api.themoviedb.org` via upstream `IRateLimitService`; 15 s timeout; redirects refused; responses > 4 MiB rejected. |
| Mapping | `MovieMetadata.Create`: `MediaType=Movie`, `AnimeGroupMemberKind=Movie`, `ForeignId=TmdbId=movieId` (**TMDB movie id stored in the same `TmdbId` column TMDB TV uses for TV ids**), `TitleSlug="tmdbmovie-{id}"`, `SeriesType=Standard`, `SeasonFolder=false`, one Season 1, `ImdbId` (validated `tt\d{7,12}`), `OriginalTitle`, `AlternateTitles=[original_title]` when different, genres, ratings, Status Ended if released/past else Upcoming, Year/FirstAired=LastAired=release date. |
| Numbering | `MovieMetadata.WithFeature`: exactly one episode S01E01, absolute 1, `ForeignId` = movie id, title = movie title, air date = release date. |
| Artwork | Poster + Fanart (`/t/p/original`, path sanitised). |
| Failure | Every error → `MovieMetadataException` with sanitised message ("existing metadata is retained"); identity mismatch rejected. Search with no key → empty; HTTP failure during an explicit-source search propagates. |
| Tests | `Test/MetadataSource/TmdbMovieProxyFixture.cs`, `Test/MetadataSource/Grouping/CollectionMovieStyleInheritanceFixture.cs`. |

#### OMDb (`MetadataSourceType.Omdb`)

| Aspect | Implementation |
|---|---|
| Classes | `Core/MetadataSource/Omdb/OmdbProxy.cs`, `OmdbResource.cs`. |
| Endpoint | `GET https://www.omdbapi.com/?apikey={key}&i={tt…}&type=movie&plot=full` (lookup) or `…&s={title}&type=movie&plot=full` (search). Search terms `tt…`, `imdb:tt…`, `omdb:tt…` → direct `i=` lookup. |
| Auth | User-supplied `OmdbApiKey` config. |
| Rate limit | `RateLimit=1000 ms` keyed `www.omdbapi.com`; 15 s timeout; no redirects; ≤4 MiB. |
| Mapping | `MovieMetadata.Create(Omdb, imdbId, title)`: `ForeignId=ImdbId=tt…`, `TitleSlug="omdb-tt…"`, Overview=Plot, Certification=Rated, Genres split, Year, Runtime from "NNN min", FirstAired from "dd MMM yyyy", Ratings = imdbRating (clamped 0–10)/imdbVotes. Only `Type=="movie"` accepted (series/episode responses rejected). |
| Numbering | One S01E01 feature (as TMDB Movies). |
| Artwork | Poster only, HTTPS on `m.media-amazon.com`/`ia.media-imdb.com`. Not a presentation provider. |
| Failure | Missing key or HTTP failure → `MovieMetadataException`. |
| Tests | `Test/MetadataSource/OmdbProxyFixture.cs`. |

### 2.3 Cross-provider observations

- Per-title source *language* is only honoured by TheTVDB (SkyHook `language` segment); the other six ignore
  `Series.Language` and the lookup `language` parameter.
- `SearchSeriesComparer` ranking is applied only inside TMDB TV, TMDB Movies, AniList and Jikan searches;
  TVDB, TVmaze and OMDb keep provider order (`grep new SearchSeriesComparer`).
- Provider-specific "alternative titles" exist only for AniList (english/romaji/userPreferred/native/synonyms) and
  TMDB Movies (original title).
- `ProviderSearchContractFixture.cs` and `MetadataSourcePolicyFixture.cs` cover search/enablement contracts.

---

## 3. Identity model

### 3.1 Ownership

A library row is still a `Series` (`Core/Tv/Series.cs`). Ownership = `(MetadataSource, ForeignId)`:

- `MetadataSource` (`MetadataSourceType`, int column, default 0 = Tvdb) — exactly one owner; never merged.
- `ForeignId` (string) — the owner's native id. Backfilled from `TvdbId` for pre-233 rows.
- `Series.ApplyChanges` deliberately never copies `MetadataSource`/`ForeignId`/`OrderingId`
  (`Series.cs` comment), so v3/v5 `PUT /series` cannot re-point ownership. During
  `AddSeriesService.AddSkyhookData` the request's `metadataSource`/`foreignId` select the provider, the stored values
  come from the provider response, and `OrderingId` is copied from the request. There is no "change owner" operation
  anywhere.
- Upstream cross-reference columns are kept and filled per provider: `TvdbId` (unique), `TvRageId`,
  `TvMazeId`, `TmdbId`, `ImdbId`, `MalIds` (JSON set), `AniListIds` (JSON set).
- `Episode.ForeignId` = provider episode id (stable across renumbering); `Episode.TvdbId` kept.

| Owner | `ForeignId` | `TvdbId` | `TmdbId` | `TvMazeId` | `ImdbId` | `MalIds`/`AniListIds` | `TitleSlug` | `Episode.ForeignId` |
|---|---|---|---|---|---|---|---|---|
| Tvdb (0) | TVDB id | TVDB id (>0) | from SkyHook | from SkyHook | from SkyHook | from SkyHook | SkyHook slug | not set by SkyHook (legacy rows: TVDB episode id via mig. 234) |
| Tmdb (1) | TMDB **TV** id | `external_ids.tvdb_id` or negative placeholder | TMDB TV id | 0 | external ids | — | `tmdb-{id}` | TMDB episode id |
| AniList (2) | AniList id | negative placeholder (or a positive TVDB id carried in from an import-list item) | 0 | 0 | — | `{idMal}` / `{id}` | `anilist-{id}` | `{anilistId}:{n}` |
| MyAnimeList (3) | MAL id | negative placeholder | 0 | 0 | — | `{malId}` / — | `mal-{id}` | `{episodeMalId}` or `{malId}:{n}` (placeholder) |
| TvMaze (4) | TVmaze id | `externals.thetvdb` or placeholder | 0 | TVmaze id | `externals.imdb` | — | `tvmaze-{id}` | TVmaze episode id |
| TmdbMovie (5) | TMDB **movie** id | placeholder | TMDB **movie** id | 0 | `imdb_id` | — | `tmdbmovie-{id}` | = movie id |
| Omdb (6) | IMDb `tt…` | placeholder | 0 | 0 | `tt…` | — | `omdb-tt…` | = `tt…` |

Migration hazards in this table: `TmdbId` is overloaded (TV id for owner 1, movie id for owner 5; the
code disambiguates with `MediaType`, e.g. `TmdbProxy.GetPresentation`, `SeriesPresentationService.PresentationKey`,
`FileNameBuilder.GetPlexId`); `TitleSlug` is no longer unique (mig. 254) and v0.1.5 routes series pages
by id; `Episode.ForeignId` formats are provider-specific strings.

### 3.2 MediaType and movies

`Core/Tv/MediaType.cs`: `Series=0, Movie=1, Special=2` (column `Series.MediaType`, mig. 253, read-only in
the API — `[SwaggerSchema(ReadOnly = true)]`, not mapped in `ToModel`). Set by the provider on add and
**overwritten on every refresh** (`RefreshSeriesService` copies `seriesInfo.MediaType`):

- TMDB Movies / OMDb → always `Movie` (`MovieMetadata.Create`).
- AniList `format` / Jikan `type` → `MOVIE`/`Movie` ⇒ Movie; `SPECIAL`,`OVA` / `Special`,`OVA` ⇒ Special; else Series.
- TVDB, TMDB TV, TVmaze → default `Series` (never set).
- Migration 253 back-filled `Movie` only for AniList/MAL rows already in a group as Movie members with exactly one S01E01 episode.

A **movie is a `Series` row with one season (1) and exactly one `Episode` S01E01** (`MovieMetadata.WithFeature`;
anime movies from AniList/Jikan get ≥1 episode the same way). `AddSeriesService.SetPropertiesAndValidate`
forces `SeasonFolder=false` for movies. Movie-specific code paths keyed on `MediaType.Movie`:
search (`IndexerSearch/ReleaseSearchService.MovieSearch` + `Definitions/MovieSearchCriteria.cs`, queries
"Title Year" for up to 3 titles, plus "{Collection} {firstYear}-{lastYear}" for collection packs),
decision/parsing (`Parser/MovieReleaseParser.cs`, `MovieCollectionPackParser.cs`, `ParsingService` 257/349/459,
`DecisionEngine/DownloadDecisionMaker.cs`), grab/import (`Download/MovieGrabContextService.cs`,
`MediaFiles/EpisodeImport/MovieFileIdentityService.cs`, `ImportDecisionMaker`, `AggregateEpisodes`,
`DownloadedEpisodesImportService`, `CompletedDownloadService.VerifyMovieCoverage`), naming (`FileNameBuilder`
uses `NamingConfig.MovieFormat` only when `MediaType==Movie && MetadataSource in {TmdbMovie, Omdb}`; anime films
keep the anime episode format). `MediaType.Special` has **no backend behaviour** beyond being stored/exposed
(only set by the two anime providers; grep shows no consumer).

### 3.3 Synthetic (negative) TVDB ids

`Core/Tv/SyntheticSeriesIdService.cs` (`IAllocateSyntheticSeriesIds.AllocateTvdbId`), called once from
`AddSeriesService.AddSkyhookData` when `series.TvdbId == 0` after the provider fetch. Allocation:
`min(lastAllocated, min(all Series.TvdbId), 0) - 1` under an in-process lock → `-1, -2, …`. Reason: the
`Series.TvdbId` UNIQUE index and `FindByTvdbId` single-match contract. The placeholder is never replaced
automatically unless a later refresh returns a positive TVDB id (`RefreshSeriesService`: `if (seriesInfo.TvdbId > 0 && …)`).
41 `TvdbId > 0`-style guards exist (vs 12 upstream), e.g. XEM, indexer request generators (Newznab/HDBits/BTN),
`ExternalIdsSpecification`, import-list identity, notifications link generator, `{TvdbId}` naming token.
**Not guarded** (placeholder leaks): Kodi/XBMC NFO (`Extras/Metadata/Consumers/Xbmc/XbmcMetadata.cs:165-170,272`
writes `<id>`, `<uniqueid type="tvdb" default="true">` and a thetvdb.com URL with the negative id; it also writes a
TMDB *movie* id as `uniqueid type="tmdb"` in `tvshow.nfo`), `KodiEpisodeGuide`, webhook payloads
(`Notifications/Webhook/WebhookSeries.cs`). Tests: `Test/TvTests/SyntheticSeriesIdServiceFixture.cs`.

### 3.4 Special members, provider-specific catalogue entries

- "Special" group members: `AnimeGroupMemberKind.Special` + `Series.AnimeGroupSpecialNumber` (mig. 253 unique
  index). AniList/Jikan pre-set the member kind to Special for SPECIAL/OVA formats, but automatic grouping never
  resolves those formats (Unsupported), so Special membership is **manual only**. Folder is hard-coded
  `Special {NN} - {Title}` (`FileNameBuilder.GetAnimeGroupMemberFolder`), not configurable.
- "Provider-specific catalogue entries" (README) = adding a title from any enabled provider with its own
  `(MetadataSource, ForeignId)`; no extra data model.

### 3.5 Multiple identities / duplicates

There is **no multi-identity model**: one owner per row, plus the cross-reference columns above. Consequences
visible in code:
- No unique index or API validator on `(MetadataSource, ForeignId)`; duplicate prevention for non-TVDB owners
  relies on path validators and the frontend `isSameSeriesIdentity` key `"{source}:{foreignId}"`
  (`FE/Series/isSameSeriesIdentity.ts`).
- `Series.TvdbId` stays UNIQUE, so the same show cannot be held twice when two providers both know its TVDB id.
  v3/v5 POST runs `SeriesExistsValidator` on `tvdbId` only when it is > 0 *in the request*; TMDB TV search
  results carry no `external_ids`, so a TMDB add of a show already owned by TVDB is caught either by the path
  validator (same generated folder) or, with a different path, only by the DB unique index at insert time
  (`SeriesService.InsertSeries` only translates group-slot violations, so this likely surfaces as an unhandled
  constraint error — **UNVERIFIED**).
- Lookup results for non-TVDB sources are never matched to existing library rows server-side (SkyHook's
  `MapSearchResult` still returns the stored series for TVDB).

---

## 4. Metadata source configuration

Storage (global `Config` table; keys lower-cased by `ConfigService`): `metadatasourceorder` (CSV of
`MetadataSourceType` ints), `disabledmetadatasources` (CSV of ints), `tmdbapikey`, `omdbapikey`
(`Core/Configuration/ConfigService.cs`). Defaults: empty strings ⇒ order = enum order
`tvdb, tmdb, aniList, myAnimeList, tvMaze, tmdbMovie, omdb`, nothing disabled, no keys.

Policy (`Core/MetadataSource/MetadataSourcePolicy.cs`):
- `Order` = saved order + any enum values missing from it (newly added sources are appended).
- `Enabled` = `Order` minus `Disabled`. Used by lookup, all-sources search and presentation enrichment
  (`SeriesPresentationService`, `GetCollectionPresentation`). **Disabling a source does not stop refreshes of
  series it already owns** (`MetadataProviderFactory` ignores enablement).
- `Save(order, disabled)`: null = keep current; order must be non-empty, distinct, defined; disabled distinct and
  defined; at least one source enabled; otherwise FluentValidation errors (`MetadataSourceOrder`,
  `DisabledMetadataSources`). Saved order always gets missing sources appended.

API:

| Route | Method | Fields | Notes |
|---|---|---|---|
| `/api/v3/config/metadatasource` (+ `/{id}`) | GET, PUT | `id`, `metadataSourceOrder[]`, `disabledMetadataSources[]`, `tmdbApiKeyConfigured`, `omdbApiKeyConfigured`, `tmdbApiKey`, `omdbApiKey` | `V3/Config/MetadataSourceConfigController.cs`, `MetadataSourceConfigResource.cs`. Keys are returned as `"********"` when set, `""` otherwise; PUT ignores `null` and `"********"`, trims, and `""` clears. |
| `/api/v5/settings/metadata` and alias `/api/v5/settings/metadatasource` (+ `/{id}`) | GET, PUT | upstream `preferredMetadataLanguage` (now `int?`) + the same six fields | `V5/Settings/MetadataSourceSettingsController.cs` (extra `[Route]` attribute), `MetadataSourceSettingsResource.cs` (`Mask = "********"`). |

Credentials: only TMDB v3 key (shared by TMDB TV, TMDB Movies and the ordering list) and OMDb key are
configurable. TheTVDB has no user key (SkyHook), AniList/Jikan/TVmaze need none. There is no built-in key.
The TMDB *import lists* use a separate Read Access Token stored in each list's settings.

"Automatic": there is **no Automatic mode in the metadata-source settings**. "Automatic" exists in two
other places:
1. `ImportLists.PreferredMetadataSource` (`ImportListMetadataSource.Automatic=0`, mig. 250; v3/v5
   `ImportListResource.preferredMetadataSource`). Resolution (`Core/ImportLists/ImportListIdentity.GetSource`): the
   preferred source is used only if the list item carries a positive id for it; otherwise fixed order
   **TVDB → AniList → TMDB → MAL** (not the user's configured order); items with only an IMDb id go through SkyHook
   `imdb:` search. TVmaze/TMDB Movies/OMDb cannot be chosen for list items.
2. `Series.AnimeGroupingMode.Automatic` (grouping, §6).

Lookup defaults: `GET /api/v{3,5}/series/lookup?term=&language=&metadataSource=&allSources=`;
`metadataSource` defaults to `tvdb`; an explicit disabled source → 400 ("disabled or unknown");
`allSources=true` searches all enabled sources in saved order.

UI: `FE/Settings/MetadataSource/{MetadataSourceSettings,Sources,metadataSources}.tsx|ts` (enable toggles, up/down
ordering, key fields); `FE/AddSeries/AddNewSeries/AddNewSeries.tsx` (source dropdown incl. "All enabled sources").

---

## 5. Episode ordering

Model: `Core/MetadataSource/Ordering/EpisodeOrdering.cs` (`Id` null = default, `Name`, `Description`,
`EpisodeCount`, `SeasonCount`, `IsAbsolute`, `IsDefault`). Persisted per series in `Series.OrderingId`
(string, mig. 234). Set **only on add** (`AddSeriesService`: `series.OrderingId = newSeries.OrderingId`);
`ApplyChanges` never copies it, so PUT cannot change it (re-adding is the only way, as the docs say).

Endpoint (v5 only): `GET /api/v5/series/ordering?tmdbId={int}&imdbId={tt…}` →
`EpisodeOrderingResource[] {id, name, description, episodeCount, seasonCount, isAbsolute, isDefault}`
(`V5/Series/EpisodeOrderingController.cs`, `EpisodeOrderingResource.cs`). When `tmdbId>0` it returns the
synthetic default `{id:null, name:"Default (Aired order)"}` plus TMDB groups (`GET /3/tv/{tmdbId}/episode_groups`,
only groups with `episode_count>0`; failures swallowed → default only). It appends an `{id:"imdb", name:"IMDb Order"}`
entry when the local IMDb index has episodes for `imdbId`. No v3 equivalent.

TMDB group application (`TmdbProxy.GetSeriesInfo` → `ApplyOrdering` → `TmdbEpisodeGroupProjector.Project`), run on
add **and every refresh**:
1. Fetch all default-order episodes (per season) keyed by TMDB episode id (`Episode.ForeignId`).
2. `GET /3/tv/episode_group/{OrderingId}`; for each group in ascending `order`: season number = the group's
   `order` value; episode numbers restart at 1 within each group in ascending entry `order`; duplicate entries are
   skipped.
3. Episodes not mentioned keep their default numbering (e.g. specials stay in season 0) and are appended.
4. Absolute numbers are then synthesised across all season>0 episodes; seasons rebuilt from the result.
5. Any failure fetching the group is logged and rethrown (add/refresh fails; numbers unchanged).

"Absolute" (`TmdbEpisodeGroupType.Absolute = 2`) is only a flag for the UI (`IsAbsolute`); the projector has no
special case. A continuous "season 1" therefore results only when TMDB publishes the absolute group as a single
sub-group whose `order` is 1 (the fixture `should_renumber_seasons_into_one_absolute_run` assumes exactly that). If
TMDB splits it into several sub-groups numbering restarts per sub-group; a sub-group with `order` 0 becomes season 0
(**UNVERIFIED against live TMDB data**). Types 1,3–7 (original air date, DVD, digital, story arc, production, TV)
are projected the same way.

Non-TMDB owners: the add dialog (`FE/AddSeries/AddNewSeries/AddNewSeriesModalContent.tsx` +
`useEpisodeOrderings.ts`) requests orderings with the lookup result's `tmdbId`/`imdbId` for **any** owner (TVDB
results usually carry a TMDB id) and posts the chosen `orderingId`. The backend stores it, but only `TmdbProxy` reads
`OrderingId`; SkyHook, AniList, Jikan, TVmaze, TMDB Movies and OMDb ignore it. Result: a stored but inert value.
TVDB owners keep upstream Sonarr's separate `SeasonType` ("Episode Order": official/dvd/absolute/… via the SkyHook
route segment), which *is* functional; for every other owner `SeasonType` is fixed to `official`.

IMDb options (README/FORK/docs say "not applied" — confirmed):
- `Core/MetadataSource/Imdb/ImdbDatasetService.cs` downloads `https://datasets.imdbws.com/title.episode.tsv.gz`
  into `{AppData}/imdb.db` (`ImdbIndex.cs`, `ImdbTsvReader.cs`, `ImdbEpisodePacker.cs`) via
  `RefreshImdbDatasetCommand` (scheduled daily in `Core/Jobs/TaskManager.cs`, but only rebuilds an existing index
  older than 30 days; manual runs always build).
- `ImdbOrderingProvider.GetOrdering` feeds the selector. `ImdbOrderingProvider.ApplyOrdering` has **no caller**
  in production code, and could not work anyway: it matches `Episode.ForeignId` as an IMDb `tconst`, which no
  provider produces.
- Choosing `imdb` for a TMDB-owned series makes `TmdbProxy.ApplyOrdering` call `/3/tv/episode_group/imdb` → 404 →
  `SeriesNotFoundException` → add/refresh fails. For other owners it is silently ignored.
- Tests (`Test/MetadataSource/Imdb/ImdbEpisodePackerFixture.cs`, `ImdbOrderingProviderFixture.cs`) cover only the
  index/packer and `GetOrdering`/`ApplyOrdering` in isolation.

---

## 6. Series / anime / movie grouping and collections

### 6.1 Data model (all on the `Series` row; no separate group table)

| Field (column) | Enum / type | Meaning |
|---|---|---|
| `AnimeGroupingMode` | `Automatic=0, Manual=1, Disabled=2` (`Core/Tv/AnimeGroupingMode.cs`) | Default Automatic (DB default and API `ToModel` default). |
| `AnimeGroupId` | string ≤96, regex `\A[A-Za-z0-9][A-Za-z0-9._:-]*\z` | Group key. Formats produced: `anilist:{rootAniListId}`, `mal:{rootMalId}`, `tmdb-movie:{tmdbCollectionId}` (automatic), `manual:{uuid}` (generated by the frontend `createManualGroupId`, `FE/Series/Grouping/animeGrouping.ts`; backend accepts any valid id). |
| `AnimeGroupTitle` | string ≤180, no control chars | Shared display title of the group (copied onto every member). |
| `AnimeGroupKind` | `Series=0, MovieSaga=1` (`AnimeGroupKind.cs`) | MovieSaga only allowed when the member is a Movie. |
| `AnimeGroupMemberKind` | `Season=0, Movie=1, Special=2` (`AnimeGroupMemberKind.cs`) | Which ordinal applies. |
| `AnimeGroupSeasonNumber` / `AnimeGroupMovieNumber` / `AnimeGroupSpecialNumber` | int 1–999, exactly one set | Ordinal within its kind. A Season member with N positive native seasons occupies slots `[n, n+N-1]`. |
| `AnimeGroupMovieFolderStyle` | `Automatic=0, Numbered=1, Title=2` (`AnimeGroupMovieFolderStyle.cs`) | Movie members: Numbered → `Film NN` format, Title → `{Series Title}` format. **Reused for Season members**: Title → story-arc folder, Numbered → `Season NN`, Automatic → arc only when the strict arc rule matches. |
| `AnimeGroupFolder` / `AnimeGroupMemberFolder` | string ≤180 | Server-derived group leaf / member leaf (read-only in v5 API). |
| `CollectionTitle` | string ≤512 (mig. 252) | Display/folder override for an *ungrouped* single series (native multi-season TV). |

DB enforcement: unique `(AnimeGroupId, AnimeGroupMemberKind, AnimeGroupSeasonNumber|MovieNumber|SpecialNumber)`
indexes plus the season-range trigger/function (mig. 245, 253). App enforcement:
`Core/Tv/AnimeGroupingValidator.cs` (`Validate`, `ValidateSlot`: enum validity, id/title validity, exactly one
ordinal, 1–999, MovieSaga ⇒ Movie member, slot-range overlap with other members of the same kind, member path not
equal/parent/child of any other series path) and `SeriesService` (`SeriesMutationLock`, slot revalidation right
before insert/update, translation of SQLite/PG unique-violation messages into validation errors, per-row fallback
inserts for import-list batches).

### 6.2 Automatic resolution (`Core/MetadataSource/Grouping/`)

`AnimeGroupingService.Resolve(series)` (`AnimeGroupingService.cs`):
1. Disabled → `Disabled` (Apply clears all group fields).
2. Manual → must already hold a valid group (`HasValidGroup`), else `Ambiguous`; result = stored values + folders.
3. Saved row (`Id>0`) with a valid group and `AnimeGroupFolder` → `Existing` (membership never re-resolved;
   `RefreshSeriesService` only calls grouping when `AnimeGroupId` is empty).
4. `MediaType==Movie` with an `IMovieCollectionProvider` for the owner (only `TmdbMovieProxy`): TMDB collection:
   `GET /3/movie/{id}` → `belongs_to_collection.id` → `GET /3/collection/{id}`; parts must be complete (positive ids,
   titles, valid release dates, ≤999, unique); movie number = index after sorting parts by release date then id;
   group `tmdb-movie:{collectionId}`, kind MovieSaga, title = collection name. No collection → `Unrelated`;
   incomplete collection or HTTP failure → `MovieMetadataException` → `Unavailable`. With the default Automatic movie
   folder style a newly added collection film therefore lands in `{root}/{Collection Name}/Film NN` (NN = release-order
   index across the *whole* TMDB collection, not just films in the library) unless the user disables grouping or
   supplies a path.
5. Owners other than AniList/MyAnimeList → `Unsupported` (TVDB, TMDB TV, TVmaze, OMDb: manual only).
6. AniList/MAL → `AnimeGroupingResolver.Resolve(source, foreignId, provider.GetAnimeRelations, allSeries)`.

Relation sources (`IAnimeRelationProvider`): `AniListProxy.GetAnimeRelations` (`RelationsById` query, edges with
`relationType(version:2)` PREQUEL/SEQUEL/PARENT/SIDE_STORY of type ANIME, plus each neighbour's `format` in the same
response; cached 15 min) and `JikanProxy.GetAnimeRelations` (`/anime/{id}/full` → `relations[]` "Prequel", "Sequel",
"Parent story", "Side story" with `type=="anime"`; format = `type` upper-cased with `_`; neighbour formats are not
included, so each neighbour costs another `/full` request).

`AnimeGroupingResolver` algorithm (`AnimeGroupingResolver.cs`):
- Eligible formats: `TV`, `TV_SHORT`, `ONA`, `MOVIE` (OVA, SPECIAL, MUSIC, TV_SPECIAL excluded). A node must have a
  valid title and positive id ("Compatible"), else the start node is `Unsupported` / a chain node `Ambiguous`.
- Budgets: ≤16 distinct node fetches (`LimitReached`), ≤8 candidates per relation list (`Ambiguous`), ordinal ≤999.
- Walk **backwards** from the member through PREQUEL links. If >1 eligible prequel, drop any prequel that is a
  reciprocal SIDE_STORY/PARENT of another prequel; still >1 → `Ambiguous`. Each backward hop must be reciprocal:
  the child's only predecessor is the parent, the parent lists the child as SEQUEL, the parent has exactly one sequel of
  the child's kind (movie vs non-movie), a movie child excludes TV sequels of the parent, and the child has ≤1 TV
  sequel (and, for a movie child, ≤1 movie sequel and not both). At every node the unique forward successor is also
  checked (TV sequel preferred; parallel movie branches → `Ambiguous`). Cycles → `Ambiguous`; missing node →
  `Unavailable`. No prequel and no sequel → `Unrelated` (standalone).
- Canonical root = earliest non-movie ancestor; if the backward chain is all movies, follow unique successors forward
  to the first non-movie; none → **Movie Saga** rooted at the earliest movie.
- Member kind = Movie if format MOVIE else Season; ordinal = number of same-kind nodes from the root to the member
  (TV→Movie→TV gives Season 1, Movie 1, Season 2). If a library row already belongs to the chain (anchor, same kind),
  its group id/title/folder are reused and the ordinal is offset from the anchor's.
- Group id `anilist:{root}`/`mal:{root}`; title = root node title (AniList: English-first; Jikan: English else default);
  folder = sanitised title (`CreateGroupFolder`: cleaned, control chars stripped, reserved device names prefixed `_`,
  ≤180 bytes, fallback "Anime").
- Groups never cross providers; titles are never compared.

`WithFolders`: if another valid member already has a group folder, its title/folder (and, for a new automatic member,
its movie-folder style) win and the group kind is forced to Series when any member is a season; the member leaf is
kept if stored, else inferred from an existing path whose parent equals the group folder, else generated (§6.4).

Preview endpoint: `GET /api/v{3,5}/series/grouping?metadataSource={enum}&foreignId={positive int}` →
`AnimeGroupingResult {animeGroupingMode, status (resolved|existing|manual|disabled|unrelated|unsupported|ambiguous|
unavailable|limitReached), animeGroupId, animeGroupTitle, animeGroupSeasonNumber, animeGroupFolder, animeGroupMemberKind,
animeGroupKind, animeGroupMovieFolderStyle, animeGroupMovieNumber, animeGroupSpecialNumber, animeGroupMemberFolder,
isResolved}` (`V3/Series/AnimeGroupingController.cs`, `V5/Series/AnimeGroupingController.cs`). Side-effect free except
provider calls/caches; `MediaType` is inferred as Movie only for `tmdbMovie`.

### 6.3 Manual grouping and editing

- Add/PUT series with `animeGroupingMode:"manual"` plus group id/title/kind/member kind/ordinal (v3/v5 `SeriesResource`).
  `Series.ApplyChanges` copies manual fields, nulls the ordinals of other kinds, and clears derived folders when the
  group or kind changes. Switching to Disabled (or between modes) clears membership. PUT with `animeGroupingMode`
  omitted keeps the stored grouping (v3/v5 `SeriesResourceMapper.ToModel(resource, series)`); switching back to
  Automatic re-resolves (`SeriesController.UpdateSeries` → `AnimeGroupingService.Apply` → `AnimeGroupingValidator.Validate`).
- Bulk/collection edit: `PUT /api/v{3,5}/series/editor` gains `collectionTitle`, `animeGroupTitle`, `animeGroupKind`,
  `animeGroupMovieFolderStyle` → `Core/Tv/SeriesCollectionEditor.cs`: group changes require selecting **every** member of
  one group, set all of them to Manual mode; MovieSaga requires all-movie members; `collectionTitle` only for a single
  ungrouped series (empty string clears it). Paths/files are never moved by these edits.
- Available for all owners (backend has no owner check for Manual; `FE/Series/Grouping/animeGrouping.ts`
  `canConfigureAnimeGrouping` returns true for any series).

### 6.4 Naming and path generation

- Only when a grouped series is **added without an explicit path**: `AnimeGroupPathBuilder.BuildPath`
  (`Core/Organizer/AnimeGroupPathBuilder.cs`) → `{RootFolder}/{AnimeGroupFolder}/{AnimeGroupMemberFolder}`, with
  `SeasonFolder=false`; rejects unsafe leaves, escaping the root, overlap with any registered series path, and a group
  folder already used by a *different* group. `SeriesPathBuilder.BuildPath(series, useExistingRelativeFolder:false)` reuses it for a member whose
  current path already matches the generated `{group}/{member}` layout (e.g. a root-folder change). Existing members
  are never moved by grouping itself.
- Leaves (`FileNameBuilder.GetAnimeGroupFolder/GetAnimeGroupMemberFolder`, validated by
  `Organizer/AnimeGroupNamingFormat.cs`; single component, ≤256 chars, required token per kind):

| NamingConfig column (mig.) | Default | Used for |
|---|---|---|
| `AnimeGroupFolderFormat` (246) | `{Group Title}` | group directory |
| `AnimeGroupSeasonFolderFormat` (246) | `Season {Group Season:00}` | Season member, numbered style |
| `AnimeGroupArcFolderFormat` (247) | `{Group Arc Title}` | Season member, story-arc style |
| `AnimeGroupMovieFolderFormat` (246) | `{Series Title}` | Movie member, title style (default in Series groups) |
| `AnimeGroupMovieSagaFolderFormat` (246) | `Film {Group Movie:00}` | Movie member, numbered style (default in Movie Sagas) |
| (hard-coded) | `Special {NN:00} - {Series Title}` | Special member |

- Strict story-arc rule (`FileNameBuilder.GetMemberArcTitle(series, requireArcSuffix:true)`): member title must start
  with the group title (case-insensitive), followed by `:`/`-`/en-dash/em-dash, and the subtitle must end with the word
  `arc`, `hen` or `chapter`; Automatic style uses the arc folder only then. `{Group Arc Title}` otherwise yields the
  subtitle, or the full title when the prefix is absent.
- Tokens `{Group Title}`, `{Group Arc Title}`, `{Group Season}`, `{Group Movie}` are also accepted in the standard/daily/
  anime episode formats (empty for ungrouped series or other member kinds; validated by
  `FileNameValidation.ValidAnimeGroupTokensValidator`). Native `{season}`/`{episode}` remain mandatory, so grouped files
  keep native numbering (a Season-2 member can still be `S01E01`).
- `BuildFilePath` skips the season sub-folder for grouped member paths. `CollectionTitle` (ungrouped) replaces the title
  in `GetSeriesFolder` for newly generated series folders only.
- New naming fields in `GET/PUT /api/v3/config/naming` and `/api/v5/settings/naming`: `movieFormat`,
  `animeGroupFolderFormat`, `animeGroupSeasonFolderFormat`, `animeGroupArcFolderFormat`, `animeGroupMovieFolderFormat`,
  `animeGroupMovieSagaFolderFormat` (null keeps the stored value); `.../naming/examples` adds `movieExample`,
  `animeGroupFolderExample`, `animeGroupSeasonFolderExample`, `animeGroupArcFolderExample`, `animeGroupMovieFolderExample`,
  `animeGroupMovieSagaFolderExample`, `animeGroupMovieEpisodeExample`.

### 6.5 Display, presentation and search of grouped members

- Library grouping is **client-side**: `FE/Series/Grouping/animeGrouping.ts` `groupAnimeSeries` groups rows with a valid
  group by `animeGroupId` after filtering/sorting (members ordered Season → Movie → Special, then ordinal); "Show groups /
  Show individual series" toggle in the index. Collection page route `/collections/group:{animeGroupId}` or
  `/collections/series:{seriesId}` (native multi-season TV) (`FE/App/AppRoutes.tsx`, `FE/Series/Grouping/CollectionPage.tsx`).
- `GET /api/v{3,5}/collection/presentation?groupId=` (`*/Series/CollectionPresentationController.cs`): 400 if the id is
  invalid, 404 unless some library member has that group; returns `{overview, images[]}` (`SeriesPresentation`, image URLs
  rewritten to the local cover proxy). Only `anilist:{id}` / `mal:{id}` ids are looked up (root entry's overview, poster,
  banner/fanart, even if the root itself is not in the library); `tmdb-movie:` and `manual:` groups get an empty object,
  and the page falls back to member artwork.
- Search: members are searched individually by their native identity. Complete-series search of a Season member adds
  "`{Group Title} S{slot:00}`" queries (`IndexerSearch/Definitions/CompleteSeriesSearchCriteria.UseCollectionNumbering`,
  consumed by Newznab/Torznab/Nyaa generators) and `Parser/CompleteSeriesReleaseParser.cs` maps collection season numbers
  back to native seasons. Movie members of a group add a "{Group Title} {firstYear}-{lastYear}" collection-pack query.
  Season-pack interactive search: `GET /api/v3/release?seriesId=&seasonNumber=&seasonPacksOnly=true` on the native member.
  "Search Collection" in the UI loops over members client-side (`FE/Series/Grouping/CollectionSearchModal.tsx`).

### 6.6 Tests

`Test/MetadataSource/Grouping/AnimeGroupingResolverFixture.cs` (24 tests), `AnimeGroupingServiceFixture.cs` (15),
`AnimeRelationProviderFixture.cs` (8), `CollectionMovieStyleInheritanceFixture.cs` (3); `Test/TvTests/AnimeGroupingValidatorFixture.cs`
(25), `AddSeriesGroupingFixture.cs` (7), `MixedCollectionSlotFixture.cs` (3), `SeriesCollectionEditorFixture.cs` (13),
`SeriesServiceTests/AddGroupedSeriesPersistenceFixture.cs` (24); `Test/OrganizerTests/AnimeGroupArcNamingFixture.cs` (8),
`AnimeGroupNamingFixture.cs` (13), `AnimeGroupNamingFormatFixture.cs` (4), `AnimeGroupPathBuilderFixture.cs` (16),
`FileNameBuilderTests/CollectionTitleNamingFixture.cs` (7); `Test/ParserTests/CompleteSeriesReleaseParserFixture.cs`;
migration fixtures 244–247, 252, 253. Relation fixture data: `Test/Files/Metadata/AniList/official-group-relations.json`.

---

## 7. Alternate titles and season-label matching

### 7.1 Where alternate titles come from

`Series.AlternateTitles` (JSON `List<string>`, mig. 235) is populated **only** by `AniListProxy.MapSeries`
(english → romaji → userPreferred → native → synonyms, case-insensitively deduped; the first becomes `Title`, the rest
`AlternateTitles`) and `TmdbMovieProxy.Map` (`original_title` when it differs). TVDB, TMDB TV, TVmaze, Jikan and OMDb
never set it (Jikan deserialises `titles`/`title_japanese` but drops them).

It is written **only at add time** (the provider object returned by `GetSeriesInfo` is what gets inserted).
`RefreshSeriesService` never copies `AlternateTitles`, so the migration comment "Populated on the next refresh of each
series" is false: rows that existed before migration 235 keep `NULL`, and titles never update. It is **not exposed** by any
API: v3/v5 `SeriesResource.alternateTitles` is still upstream's scene-mapping list (`PopulateAlternateTitles` from
`ISceneMappingService`), whose cloud source (`services.sonarr.tv`) Tanukarr blocks, so those lists are frozen at whatever
was cached in the `SceneMappings` table.

### 7.2 Lookup ranking (interactive search only)

`Core/MetadataSource/SearchSeriesComparer.cs` (applied inside TMDB TV, TMDB Movies, AniList and Jikan searches only):
upstream stages kept — (1) exact title ignoring `-._:`; (2) exact ignoring leading a/an/the; (3) Levenshtein ≤1;
(4) Levenshtein ≤1 against the query without a trailing year, with year preference; (5) `query:` prefix with year
preference — then the fork replaces upstream's final Levenshtein ordering with (6) highest
`SeriesTitleMatcher.Score(query, t)` over `Title` ∪ `AlternateTitles`, plus `yearFactor/100` (20 for exact year, 10 for ±1,
when the query ends in a year), and (7) more `Ratings.Votes` first (for AniList this is `popularity`).

`Core/MetadataSource/SeriesTitleMatcher.cs`:
- `Normalize`: lower-case → strip diacritics (NFD, drop non-spacing marks) → non letter/digit → space → collapse
  whitespace → a trailing roman numeral `i…x` (as its own last word) → `season N` → regex
  `\b(?:season\s*(?<n>\d+)|(?<n>\d+)(?:st|nd|rd|th)\s*season|s(?<n>\d{1,2})|part\s*(?<n>\d+)|cour\s*(?<n>\d+))\b` →
  `season N` (leading zeros dropped) → drop a leading article (a / an / the).
- `Score`: 0 if either empty; 1.0 if normalised strings equal; 0.95 if title starts with `query + " "`; else
  `max(JaroWinkler, TokenSet)` where Jaro-Winkler uses match window `max(len)/2-1`, Winkler boost only when Jaro ≥ 0.7,
  prefix ≤4, scale 0.1; TokenSet = `(matched query words / query words) × (0.7 + 0.3 × matched / title words)`.
- Consequence: "2nd Season", "Season 2", "S2", "Part 2", "Cour 2" and "II" all normalise to "season 2" (so "Part 2" and
  "Season 2" are indistinguishable for ranking).

### 7.3 Release matching (unattended)

Not fuzzy. Changes vs upstream:
- `SeriesService.FindByTitle` → `SeriesRepository.FindByTitle(cleanTitle) ?? FindByAlternateTitle(cleanTitle)`; the latter
  scans all series in memory comparing `CleanSeriesTitle()` of each alternate title; >1 hit throws
  `MultipleSeriesFoundException`.
- `ParsingService.FindByTitleWithoutSeasonSuffix`: a release title ending in a 1–2 digit number equal to the release's own
  season ("Tensei Shitara Slime Datta Ken 4 - S04E16") is retried without that number (scene mapping first, then exact
  title); skipped when the parser found a year.
- `ParsingService.MatchesNativeSeriesTitle` (native complete-series searches and absolute-number handling for
  placeholder-TVDB series) and `TrackedDownloadService` grab verification compare against `{Title} ∪ AlternateTitles`.
- Search query titles: `ReleaseSearchService` appends `AlternateTitles` to complete-series queries, anime season-pack
  queries (max 3 titles) and movie queries ("Title Year", max 3).

Tests: `Test/MetadataSource/SeriesTitleMatcherFixture.cs`, `SynonymRankingFixture.cs`, `SearchSeriesComparerFixture.cs`,
`Test/TvTests/SeriesServiceTests/FindByAlternateTitleFixture.cs`,
`Test/ParserTests/ParsingServiceTests/TrailingSeasonSuffixFixture.cs`, `NativeProviderSceneIsolationFixture.cs`.
(Upstream's `FindByTitleInexactFixture.cs` and its `CleanOriginalTitle` handling in `SeriesService.FindByTitleInexact`
are *absent* in Tanukarr — an upstream change not merged, not a fork feature.)

---

## 8. API surface for these features (Sonarr.Api.V3 / V5)

Serialization: System.Text.Json, camelCase, enums as camelCase strings (integers also accepted on input),
`DefaultIgnoreCondition = WhenWritingNull` (`Common/Serializer/System.Text.Json/STJson.cs`) — **null fields are omitted**
from responses (only `collectionTitle` is forced to always appear). Route list confirmed against the shipped
`V3/openapi.json` / `V5/openapi.json`.

### 8.1 New or changed endpoints

| Route | Method | Request | Response | Code |
|---|---|---|---|---|
| `/api/v3/series/lookup`, `/api/v5/series/lookup` | GET | `term`, `language` (TVDB only), **`metadataSource`** (`tvdb` default … `omdb`), **`allSources`** (bool) | `SeriesResource[]` incl. new fields; non-TVDB results have `id=0`, `metadataSource`, `foreignId`, placeholder-free `tvdbId` (0 or cross-ref) | `V3/Series/SeriesLookupController.cs`, `V5/Series/SeriesLookupController.cs` |
| `/api/v3/series`, `/api/v5/series` | POST | adds `metadataSource`, `foreignId`, `orderingId`, grouping fields (`animeGroupingMode`, `animeGroupId`, `animeGroupTitle`, `animeGroupSeasonNumber`, `animeGroupMemberKind`, `animeGroupKind`, `animeGroupMovieFolderStyle`, `animeGroupMovieNumber`, `animeGroupSpecialNumber`) | created `SeriesResource` | Validation: TVDB owner ⇒ `tvdbId>0` + not existing; other owners ⇒ `foreignId` required, `tvdbId` checked only if >0. Companion keys restricted by `LibraryTools/CompanionAddSeriesPolicy`. |
| `/api/v3/series/{id}`, `/api/v5/series/{id}` | PUT | grouping fields (omitted `animeGroupingMode` keeps stored grouping); `metadataSource`/`foreignId`/`orderingId`/`mediaType` ignored | `SeriesResource` | grouping re-resolved/validated on every PUT |
| `/api/v3/series/editor`, `/api/v5/series/editor` | PUT | adds `collectionTitle`, `animeGroupTitle`, `animeGroupKind`, `animeGroupMovieFolderStyle` | — | `Core/Tv/SeriesCollectionEditor.cs` |
| `/api/v3/series/grouping`, `/api/v5/series/grouping` | GET | `metadataSource`, `foreignId` (positive int) | `AnimeGroupingResult` (13 fields, §6.2) | `*/Series/AnimeGroupingController.cs` |
| `/api/v3/collection/presentation`, `/api/v5/collection/presentation` | GET | `groupId` | `{overview, images[]}`; 400 invalid id, 404 unknown group | `*/Series/CollectionPresentationController.cs` |
| `/api/v5/series/ordering` (v5 only) | GET | `tmdbId` (TMDB **TV** id), `imdbId` | `EpisodeOrderingResource[] {id, name, description, episodeCount, seasonCount, isAbsolute, isDefault}` | `V5/Series/EpisodeOrderingController.cs` |
| `/api/v3/config/metadatasource` (+`/{id}`) | GET, PUT | — | `{id, metadataSourceOrder[], disabledMetadataSources[], tmdbApiKeyConfigured, omdbApiKeyConfigured, tmdbApiKey(masked), omdbApiKey(masked)}` | `V3/Config/MetadataSourceConfig*.cs` |
| `/api/v5/settings/metadata` and `/api/v5/settings/metadatasource` (+`/{id}`) | GET, PUT | — | upstream `preferredMetadataLanguage` (now nullable) + same six fields | `V5/Settings/MetadataSourceSettings*.cs` |
| `/api/v3/config/naming`, `/api/v5/settings/naming` | GET, PUT | + `movieFormat`, `animeGroupFolderFormat`, `animeGroupSeasonFolderFormat`, `animeGroupArcFolderFormat`, `animeGroupMovieFolderFormat`, `animeGroupMovieSagaFolderFormat` (null = keep) | same | `V3/Config/NamingConfig*.cs`, `V5/Settings/NamingSettings*.cs` |
| `/api/v3/config/naming/examples`, v5 equivalent | GET | — | + `movieExample`, `animeGroupFolderExample`, `animeGroupSeasonFolderExample`, `animeGroupArcFolderExample`, `animeGroupMovieFolderExample`, `animeGroupMovieSagaFolderExample`, `animeGroupMovieEpisodeExample` | `*/NamingExampleResource.cs` |
| `/api/v3/importlistexclusion`, v5 equivalent | all | + `metadataSource`, `foreignId` | same | `*/ImportLists/ImportListExclusionResource.cs` |
| `/api/v3/importlist`, v5 equivalent | all | + `preferredMetadataSource` (`automatic`, `tvdb`, `tmdb`, `aniList`, `myAnimeList`) | same | `*/ImportLists/ImportListResource.cs` |
| `/api/v3/command` | POST | `{"name":"RefreshImdbDataset"}` | command | `Core/MetadataSource/Imdb/Commands/RefreshImdbDatasetCommand.cs` |
| `/api/v3/release` | GET | + `completeSeries`, `seasonPacksOnly` (collection/pack searches; detail in acquisition audit) | `ReleaseResource[]` | `V3/Indexers/ReleaseController.cs` |

### 8.2 `SeriesResource` fields Tanukarr returns beyond upstream Sonarr

`GET /api/v3/series` (and `/{id}`, lookup) — Tanukarr-only fields (`V3/Series/SeriesResource.cs`):

| Field | Type | Notes |
|---|---|---|
| `metadataSource` | enum string (nullable on input) | owner; always present on output |
| `foreignId` | string | omitted when null (pre-233 rows with `TvdbId<=0`) |
| `orderingId` | string | omitted when null (default ordering) |
| `malIds` | int[] | **new in v3** (upstream v3 resource lacks it; upstream v5 has it) |
| `aniListIds` | int[] | **new in v3** |
| `collectionTitle` | string or `null` | always emitted |
| `mediaType` | `series`/`movie`/`special` | read-only |
| `animeGroupingMode` | `automatic`/`manual`/`disabled` | |
| `animeGroupId`, `animeGroupTitle`, `animeGroupFolder`, `animeGroupMemberFolder` | string | omitted when null |
| `animeGroupSeasonNumber`, `animeGroupMovieNumber`, `animeGroupSpecialNumber` | int | omitted when null |
| `animeGroupMemberKind` | `season`/`movie`/`special` | |
| `animeGroupKind` | `series`/`movieSaga` | |
| `animeGroupMovieFolderStyle` | `automatic`/`numbered`/`title` | |

`GET /api/v5/series`: same list except `malIds`/`aniListIds` (already upstream in v5); `metadataSource` is non-nullable;
`animeGroupFolder`/`animeGroupMemberFolder`/`mediaType`/`collectionTitle` are marked read-only.

Upstream-semantics differences an API importer must know:
- `tvdbId` may be **negative** (placeholder) for non-TVDB owners — never treat it as a TheTVDB id.
- `tmdbId` means a TMDB **movie** id when `metadataSource=tmdbMovie`.
- `titleSlug` is not unique; `alternateTitles` is the (frozen) scene-mapping list, not provider titles.
- `seasonType` is meaningful only for TVDB owners; `orderingId` only for TMDB TV owners.
- **Not available through any API**: `Series.AlternateTitles` (provider titles), `Episode.ForeignId` (native episode
  ids), IMDb index state, presentation cache, per-series `ProOptions` (except via `/api/v5/librarytools/series/{id}`,
  which carries per-series `seasonTitles`/`automaticRenaming`/`keepVersions` that affect naming). An API-based migration
  must re-derive episode identity by re-fetching from the owner provider (or read `Episodes.ForeignId` from the DB).

Episode resources (`V3/Episodes/EpisodeResource.cs`, `V5/...`) only add `watched`, `watchedAtUtc` (Jellyfin feature);
movies appear as a single episode S01E01 via the normal episode/episodefile endpoints.

---

## 9. Feature status

Status legend: **Operational** = complete backend path exists and is wired end-to-end in code (runtime not executed
here); **Partial** = works only for a subset of cases advertised, or has a code-level defect; **UI-only** = exposed in
UI/API but no backend effect; **Not implemented**. Test paths relative to `Test/` unless noted; `ApiTest/` =
`ref/tanukarr/src/NzbDrone.Api.Test/`; `FE-tests/` = `ref/tanukarr/frontend/tests/`.

| Feature | Implementation files | Behaviour summary | Status (evidence) | Existing tests |
|---|---|---|---|---|
| TheTVDB | `MetadataSource/SkyHook/SkyHookProxy.cs`, `Common/Cloud/SonarrCloudRequestBuilder.cs` | Upstream SkyHook `v2/tvdb` search/details, language + SeasonType aware; owner stamp `Tvdb`/`ForeignId`. Scene-mapping and daily-series cloud feeds are blocked (`SonarrCloudRequestPolicy`), XEM still used for positive ids. | **Operational** (upstream path unchanged; depends on skyhook.sonarr.tv) — scene mappings frozen | `MetadataSource/SkyHook/SkyHookProxyFixture.cs`, `SkyHookProxySearchFixture.cs` |
| TMDB TV | `MetadataSource/Tmdb/TmdbProxy.cs`, `Tmdb/Resource/*` | Direct TMDB v3 with user key; TV details + per-season episodes; synthesized absolute numbers; groups; poster/fanart/stills; TVDB/IMDb cross-ids. No language, no alternate titles, no rate limit/cache. | **Operational** (requires `tmdbapikey`) | `MetadataSource/Tmdb/TmdbEpisodeGroupProjectorFixture.cs`, `TmdbResourceDeserializationFixture.cs`, `SeriesPresentationProviderFixture.cs`, `TvTests/RefreshEpisodeProviderIdentityFixture.cs` |
| TVmaze | `MetadataSource/TvMaze/TvMazeProxy.cs`, `TvMazeRateLimiter.cs` | Public API, 600 ms pacing, strict episode-list validation, specials → season 0, no absolute numbers, poster only. | **Operational** | `MetadataSource/TvMaze/TvMazeProxyFixture.cs`, `TvMazeRateLimiterFixture.cs` |
| AniList | `MetadataSource/AniList/*`, `AniList/AniListRequestService.cs` | Public GraphQL; single-season 1..N numbering; no episode titles; alternate titles; 5/15-min cache; adaptive shared rate limiting; relations for grouping. | **Operational** | `MetadataSource/AniList/AniListProxyFixture.cs`, `AniListEpisodeMapperFixture.cs`, `AniListMetadataCacheFixture.cs`, `AniList/AniListRequestServiceFixture.cs` |
| MAL / Jikan | `MetadataSource/Jikan/JikanProxy.cs`, `JikanRateLimiter.cs` | Jikan v4 `/full` + paged episodes; titles/air dates; placeholders to declared count; 3/s-60/min limiter. | **Partial** — works, but placeholder vs real episode `ForeignId` formats differ (`{malId}:{n}` vs `{n}`), so the advertised "titles arrive on a later refresh" path deletes/recreates rows and can drop `EpisodeFileId` (derived from `RefreshEpisodeService` matching rules; untested; UNVERIFIED at runtime); no alternate titles | `MetadataSource/Jikan/JikanContractFixture.cs`, `JikanMovieFixture.cs`, `JikanRateLimiterFixture.cs` |
| TMDB Movies | `MetadataSource/Tmdb/TmdbMovieProxy.cs`, `MetadataSource/MovieMetadata.cs` | `/movie`, `/find` (IMDb), `/search/movie`, `/collection`; single feature S01E01; automatic TMDB collection grouping (`tmdb-movie:{id}`, Movie Saga, `Film NN` folders by default). | **Operational** | `MetadataSource/TmdbMovieProxyFixture.cs`, `MetadataSource/Grouping/CollectionMovieStyleInheritanceFixture.cs` |
| OMDb | `MetadataSource/Omdb/OmdbProxy.cs` | `i=`/`s=` with `type=movie`, user key, 1 req/s; movies only; manual grouping only. | **Operational** (requires `omdbapikey`) | `MetadataSource/OmdbProxyFixture.cs` |
| Per-title ownership | `Tv/Series.cs` (`MetadataSource`, `ForeignId`), `MetadataSource/MetadataProviderFactory.cs`, `Tv/AddSeriesService.cs`, `Tv/RefreshSeriesService.cs`, mig. 233 | Owner chosen at add (lookup source), immutable via API, used for every refresh; disabled sources still refresh owned series; unknown owner falls back to TVDB. | **Operational** | `Datastore/Migration/233_add_metadata_source_to_seriesFixture.cs`, `MetadataSource/MetadataProviderRegistrationFixture.cs`, `TvTests/SeriesApplyChangesFixture.cs`, `TvTests/AddSeriesFixture.cs`, `TvTests/RefreshSeriesServiceFixture.cs`, `ApiTest/Series/SeriesProviderIdentityFixture.cs`, `FE-tests/metadataIdentity.test.cjs` |
| Multiple metadata identities | — | One owner per row + upstream cross-id columns (TvdbId/TmdbId/TvMazeId/ImdbId/MalIds/AniListIds) used for matching/enrichment. No identity table, no `(source, foreignId)` uniqueness, no owner switching. | **Not implemented** (by design; docs say "selected, not merged") | `ImportListTests/ImportListIdentityFixture.cs` (cross-id matching only) |
| TMDB episode groups | `MetadataSource/Tmdb/TmdbProxy.cs` (`GetOrderings`, `ApplyOrdering`), `TmdbEpisodeGroupProjector.cs`, `V5/Series/EpisodeOrderingController.cs`, mig. 234 | Group list via v5 `/series/ordering`; chosen id stored in `OrderingId` at add; re-projected on each refresh for TMDB owners; group `order` → season, per-group renumbering, unmentioned episodes keep default numbers. | **Partial** — applied only for TMDB-TV owners although the UI offers groups for any owner with a `tmdbId` (inert for others); "Absolute → continuous season 1" holds only for single-sub-group absolute groups with `order`=1 | `MetadataSource/Tmdb/TmdbEpisodeGroupProjectorFixture.cs`, `TmdbResourceDeserializationFixture.cs`, `Datastore/Migration/234_add_episode_orderingFixture.cs`, `FE-tests/addSeriesModal.test.cjs` |
| IMDb episode ordering | `MetadataSource/Imdb/*`, `Jobs/TaskManager.cs` | Dataset download + local index + selector entry; `ApplyOrdering` has no caller; selecting it on a TMDB owner breaks add/refresh. | **UI-only** (confirmed; docs agree) | `MetadataSource/Imdb/ImdbEpisodePackerFixture.cs`, `ImdbOrderingProviderFixture.cs` |
| Absolute anime numbering | Upstream `Parser/ParsingService.GetAnimeEpisodes`; provider absolute numbers (`TmdbProxy.SetAbsoluteNumbering`, `AniListEpisodeMapper`, `JikanProxy`, SkyHook); fork guard `nativeNumbering` for placeholder-TVDB series | Absolute-numbered releases resolve via `Episode.AbsoluteEpisodeNumber` for any series type (inherited behaviour); TMDB synthesises absolute numbers, AniList/Jikan use the episode number, **TVmaze supplies none**. | **Operational** (mostly inherited; README's "Slime - 88 → S04E16" example is not under test) | `ParserTests/AbsoluteEpisodeNumberParserFixture.cs`, `ParserTests/ParsingServiceTests/NativeProviderSceneIsolationFixture.cs`, `ParserTests/EpisodeNumberingResolverFixture.cs`, `ParserTests/ParsingServiceTests/GetEpisodesFixture.cs` |
| Prequel/sequel relationships | `MetadataSource/Grouping/AnimeGroupingResolver.cs`, `AnimeGroupingService.cs`, `AniListProxy.GetAnimeRelations`, `JikanProxy.GetAnimeRelations` | Strict reciprocal PREQUEL/SEQUEL chain over TV/TV_SHORT/ONA/MOVIE, ≤16 lookups, ≤8 candidates, side-story disambiguation, Movie Saga detection; assigned once (never re-resolved after save). | **Operational** (AniList/MAL owners only) | `MetadataSource/Grouping/AnimeGroupingResolverFixture.cs`, `AnimeRelationProviderFixture.cs`, `AnimeGroupingServiceFixture.cs`, `Files/Metadata/AniList/official-group-relations.json`, `ApiTest/Series/AnimeGroupingPreviewFixture.cs` |
| Manual anime grouping | `Tv/Series.ApplyChanges`, `Tv/AnimeGroupingValidator.cs`, `Tv/SeriesCollectionEditor.cs`, `Tv/SeriesService.cs`, `Organizer/AnimeGroupPathBuilder.cs`, migs. 244/245/253 | Season/Movie/Special members with 1–999 ordinals, DB unique indexes + season-range trigger, generated `{root}/{group}/{member}` paths on add only, collection-wide edits force Manual mode. | **Operational** (all owners) | `TvTests/AnimeGroupingValidatorFixture.cs`, `TvTests/SeriesCollectionEditorFixture.cs`, `TvTests/SeriesServiceTests/AddGroupedSeriesPersistenceFixture.cs`, `TvTests/MixedCollectionSlotFixture.cs`, `TvTests/AddSeriesGroupingFixture.cs`, `OrganizerTests/AnimeGroupPathBuilderFixture.cs`, `ApiTest/Series/AnimeGroupingUpdateFixture.cs`, `AnimeGroupingResourceFixture.cs`, `FE-tests/animeGrouping.test.cjs` |
| Movie & series collections | Group fields above; `Series.CollectionTitle` (mig. 252); `*/Series/CollectionPresentationController.cs`; `SeriesPresentationService.GetCollectionPresentation`; `IndexerSearch/Definitions/CompleteSeriesSearchCriteria.UseCollectionNumbering`; `Parser/CompleteSeriesReleaseParser.cs`; frontend `FE/Series/Grouping/*` | A "collection" is the set of rows sharing `AnimeGroupId` (or one native multi-season series); composed client-side; root artwork/overview only for `anilist:`/`mal:` groups; collection-numbered ("Title S02") search and parse; group/arc/season/movie folder + filename tokens. | **Operational** as a presentation/naming layer; **no collection entity** (no table, no collection-level monitoring/IDs); TMDB-movie and manual groups get no collection artwork | `OrganizerTests/AnimeGroupNamingFixture.cs`, `AnimeGroupArcNamingFixture.cs`, `AnimeGroupNamingFormatFixture.cs`, `FileNameBuilderTests/CollectionTitleNamingFixture.cs`, `ParserTests/CompleteSeriesReleaseParserFixture.cs`, `Datastore/Migration/252_series_collection_titleFixture.cs`, `ApiTest/Series/CollectionPresentationControllerFixture.cs`, `CollectionEditingResourceFixture.cs`, `ApiTest/Config/AnimeGroupNamingResourceFixture.cs`, `FE-tests/collections.test.cjs`, `groupNamingAndSearch.test.cjs` |
| Movies as a media type | `Tv/MediaType.cs`, `MetadataSource/MovieMetadata.cs`, `IndexerSearch/ReleaseSearchService.MovieSearch`, `Parser/MovieReleaseParser.cs`, `MovieCollectionPackParser.cs`, `Download/MovieGrabContextService.cs`, `MediaFiles/EpisodeImport/MovieFileIdentityService.cs`, `Organizer/FileNameBuilder` (MovieFormat), mig. 253 | A movie is a `Series` row (`MediaType=Movie`) with one Season 1 and one Episode S01E01; title+year searches, movie-specific parse/import/grab verification, `MovieFormat` naming for TMDB-Movies/OMDb owners. | **Operational** in code (end-to-end runtime UNVERIFIED; acquisition details in the other audit) | `MetadataSource/TmdbMovieProxyFixture.cs`, `OmdbProxyFixture.cs`, `MetadataSource/Jikan/JikanMovieFixture.cs`, `ParserTests/MovieReleaseParserFixture.cs`, `MovieCollectionPackParserFixture.cs`, `ParserTests/ParsingServiceTests/MovieRssMappingFixture.cs`, `MediaFiles/EpisodeImport/MovieFileIdentityServiceFixture.cs`, `Download/MovieGrabContextServiceFixture.cs`, `Datastore/Migration/253_movie_metadata_and_special_membersFixture.cs` |
| Provider alternate titles in matching | `MetadataSource/AniList/AniListEpisodeMapper.GetAllTitles`, `Tv/SeriesRepository.FindByAlternateTitle`, `Tv/SeriesService.FindByTitle`, `Parser/ParsingService.MatchesNativeSeriesTitle` | Exact clean-title fallback against stored alternate titles. | **Partial** — only AniList (and TMDB-Movies original title) supply titles, only at add time, never refreshed, absent for pre-235 rows | `TvTests/SeriesServiceTests/FindByAlternateTitleFixture.cs`, `MetadataSource/SynonymRankingFixture.cs` |
| Season-label lookup ranking | `MetadataSource/SeriesTitleMatcher.cs`, `SearchSeriesComparer.cs` | Normalised "season N" forms, Jaro-Winkler/token-set, alternate titles, votes tie-break. | **Operational** for TMDB TV/TMDB Movies/AniList/Jikan searches only (not TVDB/TVmaze/OMDb, not across sources) | `MetadataSource/SeriesTitleMatcherFixture.cs`, `SearchSeriesComparerFixture.cs`, `SynonymRankingFixture.cs` |
| Optional presentation enrichment | `MetadataSource/SeriesPresentationService.cs`, `SeriesPresentationCache.cs` | Fills missing overview/poster/fanart from ≤2 other enabled providers via verified cross-ids; keeps previous art on empty refresh. | **Operational** | `MetadataSource/SeriesPresentationServiceFixture.cs`, `SeriesPresentationCacheFixture.cs`, `SeriesPresentationProviderFixture.cs` |
| Metadata source enable/order settings | `MetadataSource/MetadataSourcePolicy.cs`, `V3/Config/MetadataSourceConfig*.cs`, `V5/Settings/MetadataSourceSettings*.cs` | Ordered/enabled sources for discovery; masked keys. | **Operational** | `MetadataSource/MetadataSourcePolicyFixture.cs`, `ApiTest/MetadataSourceSettingsFixture.cs`, `FE-tests/metadataSources.test.cjs` |

---

## 10. Documentation claims vs code

| Claim (source) | Code reality |
|---|---|
| Mig. 235 comment: alternate titles "Populated on the next refresh of each series" | `RefreshSeriesService` never copies `AlternateTitles`; only set at add time. |
| README "What's added": release matching "can use alternative titles supplied by the owning provider, including English and romaji names for anime" | Only AniList (and TMDB Movies' original title) provide them; MAL/Jikan, TMDB TV, TVmaze, TVDB do not. |
| FORK.md: Jikan episode titles "appear on a later refresh" | They do, but via delete+insert of placeholder rows (ForeignId format mismatch) — files can detach (UNVERIFIED runtime). |
| README/FORK: TMDB "Absolute group produces continuous numbering in season 1" | No special case; season = group `order`, numbering restarts per sub-group. True only for single-sub-group absolute groups. |
| `docs/anime-season-groups.md`: "grouping does not add a TMDB movie endpoint or a general movie manager" | Stale: `TmdbMovieProxy` + `IMovieCollectionProvider` implement TMDB movie collections and movies are a media type. |
| README "First-run setup" and FORK.md "Metadata sources": selector offers four providers (TheTVDB, TMDB, AniList, MyAnimeList) | UI and backend offer seven (`FE/Settings/MetadataSource/metadataSources.ts`, `MetadataSourceType`). |
| FORK.md: placeholder TVDB id — "Every TVDB-specific behaviour tests for a positive id" | Kodi NFO (`XbmcMetadata.cs`), `KodiEpisodeGuide` and webhook payloads emit the negative id. |
| README: "Metadata provider for new series defaults to Automatic" | Applies to import lists only; Automatic order is hard-coded TVDB→AniList→TMDB→MAL, ignoring the configured source order; TVmaze/TMDB Movies/OMDb unavailable to lists. |
| README/FORK/docs: IMDb ordering not applied | Confirmed (`ApplyOrdering` uncalled; TMDB owner + `imdb` fails). |
| README/docs: non-TMDB owners should use Default ordering | Confirmed: value stored but ignored. TVDB owners still have upstream `SeasonType` orderings. |
| docs: "Disabling a catalogue removes it from discovery and optional presentation enrichment… existing series continue to refresh" | Confirmed. |
| docs: Jikan "at most three attempts within thirty seconds"; AniList cache 5 min / 15 min / 512 entries / 512 KiB | Confirmed. |
