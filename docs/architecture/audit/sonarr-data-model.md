> Audit notes produced during Kitsunarr's source analysis (read-only review of the GPL-3.0 Sonarr and Tanukarr sources, November 2026). Line references point at the audited revisions; see `../source-audit.md` for the summary.

# Sonarr data model audit (reference for the Kitsunarr backup importer)

Scope: Sonarr C# source at `ref/sonarr` (branch `develop`, commit `c018578e` 2026-11-02, "Fixed: Don't log Postgres
password during database migration"). This tree is **Sonarr v5 (5.0.0.x)** — see `.github/workflows/build_v5.yml`
(`SONARR_MAJOR_VERSION: 5`). Latest migration in tree: **240**.

Ground truth cross-check: a live **Sonarr 4.0.20.3014** (linuxserver/sonarr:latest) instance was running in
`a live Sonarr 4.0.20 container's /config`. I copied its `sonarr.db` (+ -wal/-shm) to `a copy of the live database ` (never touched the
original) and dumped the schema to `tests/fixtures/sonarr-4.0.20/backup.zip (sonarr.db)`. That DB is at migration **217**.
Wherever this report says "VERIFIED (live v4 DB)" the statement was checked against that copy.

Path abbreviations: `Core/` = `src/NzbDrone.Core/`, `Common/` = `src/NzbDrone.Common/`, `ApiV3/` = `src/Sonarr.Api.V3/`.

> IMPORTANT version landscape (affects every query):
> * **v4 stable (4.0.x)** backups top out at migration **217** (VERIFIED: VersionInfo rows = 1..206 except 80/133, then
>   209, 210, 211, 212, 214, 215, 217 — 211 rows; it does NOT have 207/208).
> * **v5 develop** backups contain 207 and 208 (v5-only) plus 218..240 (gaps: 213, 216, 222, 228, 229, 235, 238, 239 do not
>   exist in this tree).
> * Migration numbers are not a linear "shape" indicator across branches (207/208 exist in v5 but not in a v4 DB at 217).
>   **The importer should introspect columns with `PRAGMA table_info("<table>")` and branch on column presence, not on the
>   max VersionInfo number alone.**

---

## 1. Backup archive (`Core/Backup/BackupService.cs`, `Core/Backup/MakeDatabaseBackup.cs`)

### 1.1 What goes into the zip

`BackupService.Backup(BackupType)`:
1. Empties temp folder `<TempFolder>/sonarr_backup`.
2. `BackupConfigFile()` copies `<AppData>/config.xml` into the temp folder (file name kept: `config.xml`).
3. `BackupDatabase()` — **only if `_maindDb.DatabaseType == DatabaseType.SQLite`** — calls
   `MakeDatabaseBackup.BackupDatabase(mainDb, tempFolder)`.
4. `CreateVersionInfo()` writes a plain-text file named **`INFO`** containing two lines:
   `v{BuildInfo.Version}` and `{DateTime.Now:yyyy-MM-dd HH:mm:ss}` (local time, not UTC), e.g.
   ```
   v4.0.20.3014
   2026-11-09 18:21:41
   ```
5. Deletes `sonarr.db-journal` from the temp folder, then zips **every file** in the temp folder (flat, no directories;
   `Common/ArchiveService.cs` `CreateZip` adds each file with `Path.GetFileName`).

So a modern backup zip contains exactly: **`config.xml`, `sonarr.db`, `INFO`** (SQLite installs). On Postgres installs the
zip contains only `config.xml` and `INFO` (no database at all).

The **log database `logs.db` is never backed up** (tables `Logs`, `UpdateHistory` live there — see §3.1).

### 1.2 How the DB copy is produced
`MakeDatabaseBackup.BackupDatabase`: opens the live connection string, rewrites `DataSource` to
`<temp>/<same file name>` (=`sonarr.db`), sets `JournalMode = Truncate`, and uses the SQLite online backup API
(`sourceConnection.BackupDatabase(backupConnection, "main", "main", -1, null, 500)`), then executes
`pragma journal_mode=truncate` on the copy. Result: a self-contained single-file SQLite DB in rollback-journal mode (no
-wal/-shm needed). Header is `SQLite format 3\0`.

### 1.3 File naming / folders
* Zip name: `sonarr_backup_v{BuildInfo.Version}_{DateTime.Now:yyyy.MM.dd_HH.mm.ss}.zip`
  (e.g. `sonarr_backup_v4.0.20.3014_2026.11.09_18.21.41.zip`).
* Recognition regex (`BackupService.BackupFileRegex`, case-insensitive):
  `(nzbdrone|sonarr)_backup_(v[0-9.]+_)?[._0-9]+\.zip` — legacy (NzbDrone-era) backups are named `nzbdrone_backup_…zip`
  and older ones may lack the `v<version>_` part (exact version boundaries UNVERIFIED).
* Backup root = Config key `backupfolder` (default `"Backups"`; relative paths are relative to AppData). Per-type subfolder
  = `BackupType.ToString().ToLower()`: `scheduled/`, `manual/`, `update/`.
* `enum BackupType { Scheduled = 0, Manual = 1, Update = 2 }` (`Core/Backup/BackupCommand.cs`). `Update` backups are made
  by `Core/Update/InstallUpdateService.cs` before installing an update. Scheduled ones by `BackupCommand` (scheduled task
  interval derived from Config `backupinterval`, days, default 7). Retention: Config `backupretention` (days, default 28)
  — non-manual folders are pruned by last-write time.

### 1.4 Restore validation rules (`BackupService.Restore`)
* `.zip`: extracted to `<Temp>/sonarr_backup_restore`; **top level only** (`GetFiles(temp, false)`).
  * config files = file name equals `Config.xml` (case-insensitive).
  * database files = file name equals `nzbdrone.db` **or** `sonarr.db` (case-insensitive).
  * If neither found → `RestoreBackupFailedException(404, "Unable to restore database file from backup")`.
  * Each config file must parse as XML and contain **exactly one** `<Config>` descendant element.
  * Each DB file's first 16 bytes must equal `"SQLite format 3\0"`.
  * Config is moved over `<AppData>/config.xml`; DB is moved to `<AppData>/sonarr.restore`; on next start
    `Core/Datastore/DatabaseRestorationService.cs` renames current `sonarr.db{,-shm,-wal,-journal}` to `*.pre-restore*` and
    moves `sonarr.restore` → `sonarr.db`. Migrations then run on startup (so an old backup is upgraded in place).
* `.xml`: validated + restored as config only. Any other extension: treated as a raw SQLite DB file.
* The `INFO` file is ignored by restore (not validated). Importer may read it for the version but must tolerate absence
  (older backups did not have it — UNVERIFIED when INFO was introduced; not determinable from this tree).

Importer recommendation: accept `sonarr.db` or `nzbdrone.db` (case-insensitive) at zip root; read `config.xml`
(case-insensitive name); optionally `INFO`. Do not require -wal/-shm (backups never contain them). If a user hands over
a raw live `sonarr.db` instead of a backup, it may be in WAL mode with unflushed -wal content.

Constants: `Common/Extensions/PathExtensions.cs` — `config.xml`, `sonarr.db`, `sonarr.restore`, `logs.db`.
`Common/EnvironmentInfo/AppFolderFactory.cs` migrates a legacy `nzbdrone.db` to `sonarr.db` at startup.

---

## 2. config.xml (`Core/Configuration/ConfigFileProvider.cs`)

### 2.1 File format
* Root element `<Config>` (`CONFIG_ELEMENT_NAME = "Config"`), one child element per key, element name = key
  (PascalCase), text = value. Values are read with `.Value.Trim()` via `Descendants(key)` (case-sensitive XML names).
  Example (VERIFIED, live v4 instance's file):
  ```xml
  <Config>
    <BindAddress>*</BindAddress>
    <Port>8989</Port>
    <SslPort>9898</SslPort>
    <EnableSsl>False</EnableSsl>
    <LaunchBrowser>True</LaunchBrowser>
    <ApiKey>0123456789abcdef0123456789abcdef</ApiKey>
    <AuthenticationMethod>None</AuthenticationMethod>
    <AuthenticationRequired>Enabled</AuthenticationRequired>
    <AllowedHosts></AllowedHosts>
    <Branch>main</Branch>
    <LogLevel>debug</LogLevel>
    <SslCertPath></SslCertPath>
    <SslCertPassword></SslCertPassword>
    <UrlBase></UrlBase>
    <TrustedNetworks></TrustedNetworks>
    <InstanceName>Sonarr</InstanceName>
    <UpdateMechanism>Docker</UpdateMechanism>
  </Config>
  ```
* Booleans are written as .NET `bool.ToString()` → `True`/`False` (parse case-insensitively).
* Enums: written either as PascalCase name (`Forms`, `Enabled`, via `SetValue(string, object)` / API save) or lowercase
  name (`forms`, via `SetValue(string, Enum)` which does `ToString().ToLower()`). Parsed with
  `Enum.Parse(..., ignoreCase: true)` — numeric strings would also parse. Importer: case-insensitive name match, accept
  digits too.
* Every value can be overridden by environment / `IOptions` (`Common/Options/*.cs`, bound in `NzbDrone.Host/Bootstrap.cs`
  from sections `Sonarr:App`, `Sonarr:Auth`, `Sonarr:Server`, `Sonarr:Log`, `Sonarr:Update`, `Sonarr:Postgres`, i.e. env
  vars like `SONARR__SERVER__PORT`). Overrides are not written to the file, so a backup's config.xml may not reflect the
  effective runtime values.
* On startup (`HandleAsync(ApplicationStartedEvent)`): `MigrateConfigFile()` (disables SSL if legacy `SslCertHash` present
  or cert path empty; rewrites `Basic` auth → `Forms`), `EnsureDefaultConfigFile()`, then `DeleteOldValues()` removes every
  element whose name is not a public property of `ConfigFileProvider` (so obsolete keys such as `AuthenticationEnabled`,
  `SslCertHash` disappear after a start of a recent version).
* "persisted" below = the getter writes the default into the file when missing (`persist: true`), so it normally appears in
  the file; "not persisted" keys only appear if a user/API wrote them.

### 2.2 Keys

| Key | Type | Default | Persisted | Notes |
|---|---|---|---|---|
| `BindAddress` | string | `*` | yes | blank → `*` |
| `Port` | int | `8989` | yes | |
| `SslPort` | int | `9898` | yes | |
| `EnableSsl` | bool | `False` | yes | |
| `LaunchBrowser` | bool | `True` | yes | |
| `ApiKey` | string | new `Guid` as 32 lowercase hex, no dashes | yes | regenerated if blank. API save never changes it (`SaveConfigDictionary` skips `ApiKey`). |
| `AuthenticationMethod` | enum `AuthenticationType` | `Forms` (v5 source; the fresh v4.0.20 instance wrote `None`) | yes | `None=0, Basic=1 (obsolete), Forms=2, External=3, Oidc=4` (`Core/Authentication/AuthenticationType.cs`). **In v5 source** the getter coerces `Basic` **and `None`** to `Forms` and rewrites the file; v4 (live) still writes/accepts `None`. Legacy `<AuthenticationEnabled>True` → Forms. |
| `AuthenticationRequired` | enum `AuthenticationRequiredType` | `Enabled` | yes | `Enabled=0, DisabledForLocalAddresses=1, DisabledForLocalhost=2` (value 2 exists in v5 source; UNVERIFIED for v4) |
| `TrustCgnatIpAddresses` | bool | `False` | no | also exists as DB Config key `trustcgnatipaddresses` (host-config API writes both) |
| `AllowedHosts` | string | `""` | yes | v5 source; present in v4.0.20 file too |
| `OidcAuthority`, `OidcClientId`, `OidcClientSecret`, `OidcUserIdentifier` | string | `""` | no | v5 only (OIDC auth) |
| `OidcScopes` | string | `openid profile email` | no | v5 only |
| `AnalyticsEnabled` | bool | `True` | no | |
| `Branch` | string | `main` | yes | lowercased on read |
| `LogLevel` | string | `debug` | yes | lowercased (`trace`/`debug`/`info`…) |
| `ConsoleLogLevel` | string | `""` | no | |
| `ConsoleLogFormat` | enum `ConsoleLogFormat` | `Standard` | no | `Standard=0, Clef=1` (`Common/Instrumentation/NzbDroneLogger.cs`) |
| `Theme` | string | `auto` | no | |
| `PostgresHost` | string | `""` | no | presence switches Sonarr to Postgres (see §8) |
| `PostgresPort` | int | `5432` | no | |
| `PostgresUser`, `PostgresPassword` | string | `""` | no | |
| `PostgresMainDb` | string | `sonarr-main` | no | |
| `PostgresLogDb` | string | `sonarr-log` | no | |
| `PostgresMainDbConnectionString`, `PostgresLogDbConnectionString` | string | `""` | no | v5 source |
| `LogDbEnabled` | bool | `True` | no | |
| `LogSql` | bool | `False` | no | |
| `LogRotate` | int | `50` | no | |
| `LogSizeLimit` | int (MB) | `1` | no | clamped 0..10 |
| `FilterSentryEvents` | bool | `True` | no | |
| `SslCertPath` | string | `""` | yes | |
| `SslKeyPath` | string | `""` | yes | v5 source (absent in the v4.0.20 file) |
| `SslCertPassword` | string | `""` | yes | |
| `UrlBase` | string | `""` | yes | file may contain `sonarr` or `/sonarr` (API saves the getter form `/sonarr`); getter trims `/` and returns `"/" + base` (empty stays empty). Normalise on import. |
| `TrustedNetworks` | string | `""` | yes | comma list of CIDRs, normalised by API (present in the v4.0.20 file too) |
| `InstanceName` | string | `Sonarr` | yes | getter only honours it if it starts or ends with "Sonarr" |
| `UpdateAutomatically` | bool | `True` on Windows, else `False` | no | |
| `UpdateMechanism` | enum `UpdateMechanism` | `BuiltIn` | no | `BuiltIn=0, Script=1, External=10, Apt=11, Docker=12` (`Core/Update/UpdateMechanism.cs`) |
| `UpdateScriptPath` | string | `""` | no | |
| `SyslogServer` | string | `""` | no | |
| `SyslogPort` | int | `514` | no | |
| `SyslogLevel` | string | = LogLevel | no | |
| `ProfilerEnabled` | bool | `False` | no | v5 |
| `ProfilerPosition` | string | `bottom-right` | no | v5 |

Keys that were in older files and are deleted by `DeleteOldValues` on a modern start: anything not in the table, e.g.
`AuthenticationEnabled`, `SslCertHash`. Note that "proxy", "backup", "certificate validation", "application URL" settings
live in the **DB `Config` table**, not config.xml (see §4 Config).

---
## 3. Migration framework

### 3.1 Mechanics (`Core/Datastore/Migration/Framework/*.cs`)
* FluentMigrator 8 (`Core/Sonarr.Core.csproj`: `FluentMigrator.Runner.SQLite 8.0.1`), custom SQLite processor/type map
  (`NzbDroneSQLiteTypeMap.cs`: Int*/Boolean → `INTEGER`, String → `TEXT`, Double/Decimal → `NUMERIC`,
  DateTime/DateTimeOffset → `DATETIME`, Guid → `UNIQUEIDENTIFIER`, Binary → `BLOB`).
* Every migration class derives `NzbDroneMigrationBase` and overrides `MainDbUpgrade()` (sonarr.db) and/or
  `LogDbUpgrade()` (logs.db). The **same migration numbers are recorded in both DBs**.
  Log-DB-only tables: `Logs` (001/006/064), `UpdateHistory` (141/146) — **not in backups**.
* `Create.TableForModel(name)` = table with `"Id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT` first.
* `000_database_engine_version_check.cs` is a `[Maintenance(BeforeAll)]` (not versioned, no VersionInfo row).

### 3.2 `VersionInfo` table (FluentMigrator standard) — VERIFIED (live v4 DB)
```sql
CREATE TABLE "VersionInfo" ("Version" INTEGER NOT NULL, "AppliedOn" DATETIME, "Description" TEXT);
CREATE UNIQUE INDEX "UC_Version" ON "VersionInfo" ("Version" ASC);
```
* `Version` = the `[Migration(N)]` number. `AppliedOn` written by FluentMigrator as `'yyyy-MM-ddTHH:mm:ss'`
  (e.g. `2026-11-09T17:21:40`, no zone, UTC). `Description` = the **migration class name** (not the file name), e.g.
  1 → `InitialSetup`, 200 → `AddNewItemMonitorType`, 204 → `add_add_release_hash`, 214 → `add_blurary576p_quality_in_profiles`.
* Schema version of a backup = `SELECT MAX("Version") FROM "VersionInfo"`. Latest in this tree: **240**
  (`240_series_title_slug_not_unique.cs`). Latest in v4.0.20 stable: **217**.
* Missing numbers (never existed in this tree): 80, 133, 213, 216, 222, 228, 229, 235, 238, 239.
  214 is a no-op in this tree ("Replaced with 215"); the v4 DB records 214 under the same class name (whether v4's
  copy had a body is UNVERIFIED).

### 3.3 Migrations >= 150 (one line each; **bold** = rename/drop/shape change an importer must handle)

| # | Class (= VersionInfo.Description) | Change |
|---|---|---|
| 150 | add_scene_mapping_origin | SceneMappings + `SceneOrigin` TEXT NULL, `SearchMode` INT NULL, `Comment` TEXT NULL |
| 151 | remove_custom_filter_type | CustomFilters.Type `seriesIndex`/`seriesEditor`/`seasonPass` → `series` |
| 152 | update_btn_url_to_https | Indexers.Settings text replace for BroadcastheNet URL |
| 153 | add_on_episodefiledelete_for_upgrade | Notifications + `OnEpisodeFileDeleteForUpgrade` BOOL default 1 |
| 154 | add_name_release_profile | ReleaseProfiles + `Name` TEXT NULL |
| 155 | add_arabic_and_hindi_languages | data: appends languages into LanguageProfiles.Languages JSON |
| 156 | add_bypass_to_delay_profile | DelayProfiles + `BypassIfHighestQuality` BOOL default 0, then set **1 for all existing rows** |
| 157 | email_multiple_addresses | **Notifications(Email).Settings `to`: string → JSON array of strings** |
| 158 | cdh_per_downloadclient | DownloadClients + `RemoveCompletedDownloads`, `RemoveFailedDownloads` BOOL NOT NULL default 1; copies & **deletes** Config keys `removecompleteddownloads`/`removefaileddownloads`. NB: for RTorrent/Flood it writes the **TEXT `'false'`** into the INTEGER column (affinity keeps it as text) |
| 159 | add_indexer_tags | Indexers + `Tags` TEXT NULL |
| 160 | rename_blacklist_to_blocklist | **table `Blacklist` → `Blocklist`** (index keeps old name `IX_Blacklist_SeriesId`) |
| 161 | remove_plex_hometheatre | deletes Notifications `PlexHomeTheater`, `PlexClient`; PendingReleases.ParsedEpisodeInfo `quality.quality` object→int, `language` object→int |
| 162 | release_profile_to_array | **ReleaseProfiles `Required`/`Ignored`: comma-separated string → JSON string array** |
| 163 | mediainfo_to_ffmpeg | **EpisodeFiles.MediaInfo JSON rewritten to ffprobe schema (schemaRevision ≥ 8)** |
| 164 | download_client_per_indexer | Indexers + `DownloadClientId` INT default 0 |
| 165 | add_on_update_to_notifications | Notifications + `OnApplicationUpdate` BOOL default 0 |
| 166 | update_series_sort_title | data: recompute Series.SortTitle |
| 167 | add_tvdbid_to_episode | Episodes + `TvdbId` INT NULL |
| 168 | add_additional_info_to_pending_releases | PendingReleases + `AdditionalInfo` TEXT NULL |
| 169 | add_malayalam_and_ukrainian_languages | data: LanguageProfiles JSON |
| 170 | add_language_tags_to_subtitle_files | SubtitleFiles + `LanguageTags` TEXT NULL |
| 171 | add_custom_formats | **new table `CustomFormats`**; QualityProfiles + `FormatItems` TEXT default `'[]'`, `MinFormatScore`, `CutoffFormatScore` INT default 0; ReleaseProfiles.Preferred terms converted to one CF each (`ReleaseTitleSpecification`, name `<ProfileName>_<id>[_n]`, score into every profile's FormatItems); naming tokens `Preferred Words`→`Custom Formats`; **drops ReleaseProfiles `Preferred`, `IncludePreferredWhenRenaming`**; deletes release profiles with `Required='[]' AND Ignored='[]'` |
| 172 | add_SeasonSearchMaximumSingleEpisodeAge_to_indexers | Indexers + `SeasonSearchMaximumSingleEpisodeAge` INT NOT NULL default 0 |
| 173 | remove_omg | deletes Indexers `Omgwtfnzbs` |
| 174 | add_salt_to_users | Users + `Salt` TEXT NULL, `Iterations` INT NULL |
| 175 | language_profiles_to_custom_formats | EpisodeFiles/History/Blocklist + `Languages` TEXT NOT NULL default `'[]'` filled with `'[' \|\| Language \|\| ']'`; **drops `Language` from EpisodeFiles/History/Blocklist, drops `LanguageProfileId` from Series & ImportLists, drops table `LanguageProfiles`** |
| 176 | original_language | Series + `OriginalLanguage` INT default 1 (English) |
| 177 | add_on_manual_interaction_required_to_notifications | Notifications + `OnManualInteractionRequired` BOOL default 0 |
| 178 | list_sync_time | **ImportListStatus drop `LastSyncListInfo`**, + `LastInfoSync` DATETIME NULL |
| 179 | add_auto_tagging | **new table `AutoTagging`** (Name UNIQUE, Specifications default `'[]'`, RemoveTagsAutomatically, Tags default `'[]'`) |
| 180 | task_duration | ScheduledTasks + `LastStartTime` DATETIME NULL |
| 181 | quality_definition_preferred_size | QualityDefinitions + `PreferredSize` (NUMERIC) NULL = MaxSize-5 |
| 182 | add_custom_format_score_bypass_to_delay_profile | DelayProfiles + `BypassIfAboveCustomFormatScore` BOOL default 0, `MinimumCustomFormatScore` INT NULL |
| 183 | update_images_remote_url | **JSON key rename `"url"`→`"remoteUrl"` in Episodes.Images, Series.Images/Actors/Seasons** |
| 184 | remove_invalid_roksbox_metadata_images | MetadataFiles cleanup |
| 185 | add_episode_runtime | Episodes + `Runtime` INT default 0 |
| 186 | add_result_to_commands | Commands + `Result` INT default 1 |
| 187 | add_on_series_add_to_notifications | Notifications + `OnSeriesAdd` BOOL default 0 |
| 188 | postgres_update_timestamp_columns_to_with_timezone | deletes all Commands; ALTER date columns to DateTimeOffset (no-op type change on SQLite: still `DATETIME`; table rebuilt) |
| 189 | add_colon_replacement_to_naming_config | NamingConfig + `ColonReplacementFormat` INT default 4 (Smart) |
| 190 | health_restored_notification | Notifications + `OnHealthRestored` BOOL default 0 |
| 191 | add_download_client_tags | DownloadClients + `Tags` TEXT NULL |
| 192 | import_exclusion_type | ImportListExclusions.TvdbId TEXT → **INTEGER** |
| 193 | add_import_list_items | new table `ImportListItems`; ImportListStatus + `HasRemovedItemSinceLastClean` BOOL default 0 |
| 194 | add_notification_status | new table `NotificationStatus` |
| 195 | parse_language_tags_from_existing_subtitle_files | data (LastUpdated = CURRENT_TIMESTAMP) |
| 196 | add_finale_type | Episodes + `FinaleType` TEXT NULL |
| 197 | list_add_missing_search | ImportLists + `SearchForMissingEpisodes` BOOL NOT NULL default 1 |
| 198 | parse_title_from_existing_subtitle_files | SubtitleFiles + `Title` TEXT NULL, `Copy` INT default 0 |
| 199 | series_last_aired | Series + `LastAired` DATETIME NULL |
| 200 | AddNewItemMonitorType | Series + `MonitorNewItems` INT default 0; ImportLists + `MonitorNewItems` INT default 0 |
| 201 | email_encryption | **Notifications(Email).Settings: `requireEncryption` (bool) → `useEncryption` (int 0/1); `requireEncryption` set to null** |
| 202 | add_indexer_flags | Blocklist & EpisodeFiles + `IndexerFlags` INT default 0 |
| 203 | release_type | Blocklist & EpisodeFiles + `ReleaseType` INT default 0 (EpisodeFiles parsed from OriginalFilePath) |
| 204 | add_add_release_hash | EpisodeFiles + `ReleaseHash` TEXT NULL |
| 205 | rename_season_pack_spec | **CustomFormats.Specifications text replace `SeasonPackSpecification` → `ReleaseTypeSpecification`** |
| 206 | add_tmdbid | Series + `TmdbId` INT default 0 + index |
| 207 | add_size_to_quality_profiles | **v5 only.** Copies MinSize/MaxSize/PreferredSize into every QualityProfiles.Items entry (`minSize`/`maxSize`/`preferredSize`), then **drops QualityDefinitions `MinSize`, `MaxSize`, `PreferredSize`** |
| 208 | metadata_translations | **v5 only.** ImportLists + `Language` INT NULL default 1; Series + `Language` INT NULL default 1, `OriginalTitle` TEXT NULL, `CleanOriginalTitle` TEXT NULL (indexed), `SeasonType` TEXT NULL default `'official'`, `SeasonTypes` TEXT NULL default `'[]'`; new table `SeriesTranslations`; sets all Series.LastInfoSync = `2026-01-01 00:00:00` |
| 209 | add_on_import_complete_to_notifications | Notifications + `OnImportComplete` BOOL default 0 |
| 210 | add_monitored_seasons_filter | CustomFilters filter key `hasUnmonitoredSeason` → `seasonsMonitoredStatus` |
| 211 | add_custom_colon_replacement_to_naming_config | NamingConfig + `CustomColonReplacementFormat` TEXT NOT NULL default `''` |
| 212 | add_minium_upgrade_format_score_to_quality_profiles | QualityProfiles + `MinUpgradeFormatScore` INT default 1 |
| 214 | add_blurary576p_quality_in_profiles | (no-op in v5 tree; replaced by 215) |
| 215 | add_blurary576p_quality_in_profiles_with_grouped_blurary480p | QualityProfiles.Items: inserts Bluray-576p (22) after/grouped with Bluray-480p (13) |
| 217 | add_mal_and_anilist_ids | Series + `MalIds` TEXT default `'[]'`, `AniListIds` TEXT default `'[]'` (last v4.0.20 migration) |
| 218 | telegram_link_preview | Notifications(Telegram).Settings + `linkPreview` int (from old `metadataLinks` array; -1 = none) |
| 219 | nzb_su_url_to_nzb_life | Indexers.Settings text replace `//api.nzb.su` → `//api.nzb.life` |
| 220 | enable_season_pack_seeding_goal | Indexers.Settings.seedCriteria: if `seasonPackSeedTime` present → `seasonPackSeedGoal: 1`, `seasonPackSeedRatio` = `seedRatio` |
| 221 | add_exclusion_tags_to_release_profiles | ReleaseProfiles + `ExcludedTags` TEXT NOT NULL default `'[]'` |
| 223 | add_source_to_blocklist | Blocklist + `Source` TEXT NULL |
| 224 | release_profile_indexer_ids | ReleaseProfiles + `IndexerIds` TEXT default `'[]'` (old IndexerId>0 → `[id]`); **drops `IndexerId`** |
| 225 | mediainfo_multiple_streams | **EpisodeFiles.MediaInfo: `audioLanguages`/`audioFormat`/`audioCodecID`/`audioProfile`/`audioBitrate`/`audioChannels`/`audioChannelPositions`/`subtitles` removed → `audioStreams[]` + `subtitleStreams[]`** (written non-indented) |
| 226 | add_air_date_filtering_to_release_profiles | ReleaseProfiles + `AirDateRestriction` BOOL default 0, `AirDateGracePeriod` INT default 0 |
| 227 | original_country | Series + `OriginalCountry` TEXT NULL |
| 230 | add_mapping_id_to_scene_mappings | SceneMappings + `MappingId` TEXT NULL |
| 231 | add_tag_existing_to_importlists | ImportLists + `TagExisting` BOOL default 0 |
| 232 | add_season_pack_air_date_to_release_profiles | ReleaseProfiles + `AllowSeasonPackWithoutAllEpisodesAired` BOOL default 0 |
| 233 | quality_profile_quality_ranks | new table `QualityProfileQualityRanks` (+unique index ProfileId,QualityId); expression indexes `IX_History_QualityId`, `IX_EpisodeFiles_QualityId`, `IX_Blocklist_QualityId` on `json_extract("Quality",'$.quality')` (reader SQLite must have JSON1) |
| 234 | add_pending_path_to_series | Series + `PendingPath` TEXT NULL |
| 236 | import_list_tag_existing_type | **ImportLists.TagExisting BOOL → INT (enum) NOT NULL default 0** |
| 237 | pending_releases_season_numbers | **PendingReleases.ParsedEpisodeInfo: `seasonNumber` (int) → `seasonNumbers` (int array)** |
| 240 | series_title_slug_not_unique | Series index on TitleSlug: unique → non-unique |

Pre-150 renames that matter for very old (v2/v3) backups: 054 rename_profiles; 114 IndexerStatus.IndexerId → ProviderId;
119 Indexers.EnableSearch → EnableAutomaticSearch (+EnableInteractiveSearch); **127 table `Restrictions` → `ReleaseProfiles`**;
**128 table `Profiles` → `QualityProfiles`, Series.ProfileId → QualityProfileId**; 111 created `LanguageProfiles`
(+ Series.LanguageProfileId) — dropped again in 175. Recommendation: support only backups that are ≥ ~150 natively,
or (simplest/safest) require users to start the backup once on a current Sonarr v4 so it self-migrates.

---
## 4. Cumulative schema of the tables an importer needs

Legend: **NN** = NOT NULL. "v4" = VERIFIED against the live 4.0.20 DB (migration 217). "v5+" = only in this v5 tree
(migrations > 217 or v5-only 207/208). Full v4 DDL: `tests/fixtures/sonarr-4.0.20/backup.zip (sonarr.db)`.
Model classes give meaning; the DB type is the SQLite declared type.

### 4.1 `Series` (`Core/Tv/Series.cs`)
| Column | Type | NN | Meaning / notes |
|---|---|---|---|
| Id | INTEGER PK | ✓ | |
| TvdbId | INTEGER | ✓ | unique index `IX_Series_TvdbId` |
| TvRageId | INTEGER | ✓ | legacy, usually 0 |
| TvMazeId | INTEGER | ✓ | |
| ImdbId | TEXT | | `tt0903747` |
| TmdbId | INTEGER | ✓ (default 0) | migration 206 |
| MalIds | TEXT JSON `int[]` | ✓ (default `'[]'`) | MyAnimeList ids (217), model `HashSet<int>` |
| AniListIds | TEXT JSON `int[]` | ✓ (default `'[]'`) | (217) |
| Title | TEXT | ✓ | display title |
| CleanTitle | TEXT | ✓ | normalized title for matching (derived) |
| SortTitle | TEXT | | derived (lowercase, articles stripped) |
| TitleSlug | TEXT | | unique index in v4; non-unique since 240 |
| Status | INTEGER | ✓ | `SeriesStatusType` (§6) |
| Overview | TEXT | | |
| AirTime | TEXT | | local air time string e.g. `21:00` |
| Monitored | INTEGER bool | ✓ | |
| MonitorNewItems | INTEGER | ✓ (default 0) | `NewItemMonitorTypes` 0=All, 1=None (200) |
| QualityProfileId | INTEGER | | FK → QualityProfiles.Id |
| SeasonFolder | INTEGER bool | ✓ | use season folders |
| LastInfoSync | DATETIME | | last metadata refresh |
| LastDiskSync | DATETIME | | **legacy, not mapped by model** |
| Runtime | INTEGER | ✓ | minutes |
| Images | TEXT JSON `MediaCover[]` | ✓ | see below |
| SeriesType | INTEGER | ✓ | `SeriesTypes` 0=Standard,1=Daily,2=Anime |
| Network | TEXT | | |
| UseSceneNumbering | INTEGER bool | ✓ | |
| Path | TEXT | ✓ | absolute series folder path (non-unique index). Root folder is NOT stored (model `RootFolderPath` is ignored) — derive it by longest-prefix match against `RootFolders.Path` (Sonarr does the same) |
| Year | INTEGER | | |
| Seasons | TEXT JSON `Season[]` | | see below |
| Actors | TEXT JSON `Actor[]` | | |
| Ratings | TEXT JSON `Ratings` | | |
| Genres | TEXT JSON `string[]` | | |
| Certification | TEXT | | e.g. `TV-MA` (uppercased) |
| Tags | TEXT JSON `int[]` | | Tag ids (HashSet) |
| Added | DATETIME | | |
| FirstAired | DATETIME | | |
| NextAiring | DATETIME | | **legacy, not mapped** |
| LastAired | DATETIME | | (199) |
| AddOptions | TEXT JSON `AddSeriesOptions` | | non-NULL only until the first scan after adding (`SeriesScannedHandler` sets it NULL) |
| OriginalLanguage | INTEGER | ✓ | Language id (176, default 1 English) |
| Language | INTEGER | | **v5+ (208)**, default 1; preferred metadata language |
| OriginalTitle | TEXT | | **v5+ (208)** |
| CleanOriginalTitle | TEXT | | **v5+ (208)** indexed |
| SeasonType | TEXT | | **v5+ (208)** default `'official'` (episode ordering type name) |
| SeasonTypes | TEXT JSON `SeasonType[]` | | **v5+ (208)** default `'[]'` |
| OriginalCountry | TEXT | | **v5+ (227)** |
| PendingPath | TEXT | | **v5+ (234)** (path pending a move) |

Indexes (v4): `IX_Series_TvdbId` UNIQUE, `IX_Series_TitleSlug` UNIQUE (non-unique since v5 240), `IX_Series_Path`,
`IX_Series_CleanTitle`, `IX_Series_TvRageId`, `IX_Series_TvMazeId`, `IX_Series_TmdbId`.

JSON structures (camelCase, null props omitted):
* `Images` — `List<MediaCover>` (`Core/MediaCover/MediaCover.cs`): `{coverType: enum string, remoteUrl: string, url?: string}`.
  `coverType` ∈ `unknown, poster, banner, fanart, screenshot, headshot, clearlogo`. In the DB normally only
  `coverType` + `remoteUrl` (`url` is the local `/MediaCover/...` URL computed at API time; before migration 183 the
  remote URL was stored under `url`).
  ```json
  [
    { "coverType": "banner", "remoteUrl": "https://artworks.thetvdb.com/banners/graphical/81189-g21.jpg" },
    { "coverType": "poster", "remoteUrl": "https://artworks.thetvdb.com/banners/posters/81189-10.jpg" },
    { "coverType": "fanart", "remoteUrl": "https://artworks.thetvdb.com/banners/fanart/original/81189-21.jpg" }
  ]
  ```
* `Seasons` — `List<Season>` (`Core/Tv/Season.cs`): `{seasonNumber: int, monitored: bool, title?: string, images: MediaCover[]}`
  ```json
  [
    { "seasonNumber": 0, "monitored": false, "images": [] },
    { "seasonNumber": 1, "monitored": true,
      "images": [ { "coverType": "poster", "remoteUrl": "https://artworks.thetvdb.com/banners/seasons/81189-1-4.jpg" } ] }
  ]
  ```
* `Ratings` — `{votes: int, value: decimal}` → `{ "votes": 31714, "value": 9.4 }`
* `Genres` — `["Crime", "Drama", "Thriller"]`
* `Tags` / `MalIds` / `AniListIds` — `[1, 4]`
* `Actors` — `[{ "name": "Bryan Cranston", "character": "Walter White", "images": [ { "coverType": "headshot", "remoteUrl": "https://artworks.thetvdb.com/banners/actors/....jpg" } ] }]`
* `AddOptions` — `AddSeriesOptions : MonitoringOptions` (`Core/Tv/AddSeriesOptions.cs`, `MonitoringOptions.cs`):
  `{ "searchForMissingEpisodes": true, "searchForCutoffUnmetEpisodes": false, "ignoreEpisodesWithFiles": false, "ignoreEpisodesWithoutFiles": false, "monitor": "all" }`
  (`monitor` = `MonitorTypes` camelCase string, §6).
* `SeasonTypes` (v5) — `List<SeasonType>` (`Core/Tv/SeasonType.cs`): `{name: string, type: string, seasonNumbers: int[], episodeCount: int}`
  (illustrative: `[{"name":"Aired Order","type":"official","seasonNumbers":[1,2,3],"episodeCount":62}]` — values come from
  Sonarr's metadata API, exact strings UNVERIFIED).

### 4.2 `SeriesTranslations` — **v5+ only (208)** (`Core/Tv/SeriesTranslation.cs`)
`Id`, `SeriesId` INTEGER NN (indexed), `Title` TEXT, `CleanTitle` TEXT, `Overview` TEXT, `Language` INTEGER NN (Language id).
Metadata cache; importer can skip (re-fetchable).

### 4.3 `Episodes` (`Core/Tv/Episode.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Id | INTEGER PK | ✓ | |
| SeriesId | INTEGER | ✓ | FK Series |
| TvdbId | INTEGER | | episode TVDB id (167) |
| SeasonNumber | INTEGER | ✓ | 0 = specials |
| EpisodeNumber | INTEGER | ✓ | |
| AbsoluteEpisodeNumber | INTEGER | | anime absolute number |
| SceneAbsoluteEpisodeNumber, SceneSeasonNumber, SceneEpisodeNumber | INTEGER | | XEM/scene mapping |
| UnverifiedSceneNumbering | INTEGER bool | ✓ | |
| AiredAfterSeasonNumber, AiredBeforeSeasonNumber, AiredBeforeEpisodeNumber | INTEGER | | specials placement |
| Title | TEXT | | |
| Overview | TEXT | | |
| AirDate | TEXT | | local air date **`yyyy-MM-dd`** string (`Episode.AIR_DATE_FORMAT`) |
| AirDateUtc | DATETIME | | air timestamp UTC |
| Monitored | INTEGER bool | (nullable in DDL) | |
| EpisodeFileId | INTEGER | (nullable in DDL) | FK EpisodeFiles; **0 = no file** (model int) |
| Ratings | TEXT JSON `Ratings` | | `{ "votes": 0, "value": 0 }` |
| Images | TEXT JSON `MediaCover[]` | | usually one `screenshot` |
| LastSearchTime | DATETIME | | |
| Runtime | INTEGER | ✓ | minutes (185) |
| FinaleType | TEXT | | string from metadata API (e.g. `series`, `season`, `midseason`) (196) |

Indexes: `IX_Episodes_EpisodeFileId`, `IX_Episodes_SeriesId`, `IX_Episodes_SeriesId_SeasonNumber_EpisodeNumber`,
`IX_Episodes_SeriesId_AirDate`. Example `Images`: `[ { "coverType": "screenshot", "remoteUrl": "https://artworks.thetvdb.com/banners/episodes/81189/349232.jpg" } ]`.

### 4.4 `EpisodeFiles` (`Core/MediaFiles/EpisodeFile.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Id | INTEGER PK | ✓ | |
| SeriesId | INTEGER | ✓ | |
| SeasonNumber | INTEGER | ✓ | |
| RelativePath | TEXT | | path **relative to `Series.Path`** (`Path` property is ignored/not stored) |
| Size | INTEGER | ✓ | bytes |
| DateAdded | DATETIME | ✓ | |
| OriginalFilePath | TEXT | | path of the source file relative to the parent of the download's output path, e.g. `Show.S01E01.1080p-GRP/Show.S01E01.1080p-GRP.mkv`; only set for imports from a download client (`ImportApprovedEpisodes.cs`) (129) |
| SceneName | TEXT | | original release name |
| ReleaseGroup | TEXT | | |
| ReleaseHash | TEXT | | (204) |
| Quality | TEXT JSON `QualityModel` | ✓ | |
| Languages | TEXT JSON `int[]` | ✓ (default `'[]'`) | Language ids (175) |
| MediaInfo | TEXT JSON `MediaInfoModel` | | ffprobe summary (shape changed in 163 and 225) |
| IndexerFlags | INTEGER | ✓ (default 0) | `IndexerFlags` bit field (202) |
| ReleaseType | INTEGER | ✓ (default 0) | `ReleaseType` (203) |

Indexes: `IX_EpisodeFiles_SeriesId`; v5+ also `IX_EpisodeFiles_QualityId` on `json_extract("Quality",'$.quality')`.
Episode ↔ file link lives on **`Episodes.EpisodeFileId`** (one file can cover several episodes; there is no join table).

`Quality` (`Core/Qualities/QualityModel.cs`, `Revision.cs`; detection-source props are `[JsonIgnore]`):
```json
{ "quality": 7, "revision": { "version": 1, "real": 0, "isRepack": false } }
```
`quality` = Quality id (§6.1). `revision.version` > 1 = PROPER/REPACK v2…, `real` = REAL count.

`Languages`: `[1]` (English) / `[1, 2]`; written indented by Sonarr (`[\n  1\n]`) or compact by migration 175.

`MediaInfo` — **v4 shape** (schema of migration-163 `MediaInfo163` + `schemaRevision`; v4 model not in this tree):
```json
{
  "rawStreamData": "{...ffprobe json...}",          // may be absent in older rows
  "schemaRevision": 12,                              // v4 value UNVERIFIED (v5 = 14)
  "containerFormat": "matroska,webm",
  "videoFormat": "h264", "videoCodecID": "", "videoProfile": "High",
  "videoBitrate": 0, "videoBitDepth": 8, "videoMultiViewCount": 0,
  "videoColourPrimaries": "bt709", "videoTransferCharacteristics": "bt709",
  "videoHdrFormat": "none",
  "height": 1080, "width": 1920,
  "audioFormat": "eac3", "audioCodecID": "", "audioProfile": "", "audioBitrate": 640000,
  "runTime": "00:47:21.1200000",
  "audioStreamCount": 1, "audioChannels": 6, "audioChannelPositions": "5.1(side)",
  "videoFps": 23.976,
  "audioLanguages": ["eng"], "subtitles": ["eng", "spa"],
  "scanType": "Progressive"
}
```
**v5 shape after migration 225** (`Core/MediaFiles/MediaInfo/MediaInfoModel.cs`, `MediaInfoAudioStreamModel.cs`,
`MediaInfoSubtitleStreamModel.cs`): the eight audio/subtitle scalar keys are removed and replaced by arrays:
```json
{
  "rawStreamData": "{...}", "schemaRevision": 14, "containerFormat": "matroska,webm",
  "videoFormat": "hevc", "videoCodecID": "", "videoProfile": "Main 10", "videoBitrate": 0, "videoBitDepth": 10,
  "videoColourPrimaries": "bt2020", "videoTransferCharacteristics": "smpte2084", "videoHdrFormat": "hdr10",
  "height": 2160, "width": 3840, "runTime": "00:52:10.0000000", "videoFps": 23.976,
  "primaryAudioStream": { "language": "eng", "format": "truehd", "codecId": "", "profile": "", "bitrate": 0, "channels": 8, "channelPositions": "7.1" },
  "audioStreams": [ { "language": "eng", "format": "truehd", "codecId": "", "profile": "", "bitrate": 0, "channels": 8, "channelPositions": "7.1", "title": "TrueHD Atmos 7.1" } ],
  "subtitleStreams": [ { "language": "eng", "format": "subrip", "title": "English", "forced": false, "hearingImpaired": false } ],
  "scanType": "Progressive"
}
```
(`primaryAudioStream` is a read-only computed property that STJ also writes — ignore it.) `videoHdrFormat` values:
`none, unknownHdr, pq10, hdr10, hdr10Plus, hlg10, dolbyVision, dolbyVisionHdr10, dolbyVisionSdr, dolbyVisionHlg,
dolbyVisionHdr10Plus` (`HdrFormat.cs`; may be an int in very old rows). Language codes are ffprobe ISO-639-2 strings
(`und` when unknown). Treat MediaInfo as optional/re-scannable.

### 4.5 `QualityProfiles` (`Core/Profiles/Qualities/QualityProfile.cs`, `QualityProfileQualityItem.cs`, `Core/Profiles/ProfileFormatItem.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Id | INTEGER PK | ✓ | |
| Name | TEXT | ✓ | unique `IX_QualityProfiles_Name` |
| Cutoff | INTEGER | ✓ | id of the cutoff **quality id or group id (≥1000)** |
| Items | TEXT JSON | ✓ | ordered list, lowest → highest preference |
| UpgradeAllowed | INTEGER bool | (nullable) | (128) |
| FormatItems | TEXT JSON | ✓ (default `'[]'`) | custom-format scores (171) |
| MinFormatScore | INTEGER | ✓ (0) | |
| CutoffFormatScore | INTEGER | ✓ (0) | |
| MinUpgradeFormatScore | INTEGER | ✓ (1) | (212) |

`Items` element = `QualityProfileQualityItem`: `{id?: int (omitted when 0 via JsonIgnore WhenWritingDefault), name?: string,
quality?: int, items: [...], allowed: bool, minSize?/maxSize?/preferredSize?: number (v5+ only, migration 207)}`.
A plain quality has `quality` and empty `items`; a **group** has `id` (1000, 1001, …), `name`, no `quality`, and member
qualities in `items` (members carry their own `allowed`). VERIFIED (live v4 default profile "Any"):
```json
[
  { "quality": 0, "items": [], "allowed": false },
  { "quality": 1, "items": [], "allowed": true },
  { "id": 1000, "name": "WEB 480p",
    "items": [ { "quality": 12, "items": [], "allowed": true }, { "quality": 8, "items": [], "allowed": true } ],
    "allowed": true },
  { "quality": 2, "items": [], "allowed": true },
  { "quality": 10, "items": [], "allowed": false }
]
```
v5 item: `{ "quality": 7, "items": [], "allowed": true, "minSize": 4, "maxSize": 155, "preferredSize": 95 }`
(sizes are MB per minute of runtime, null = unlimited/unset; in v4 the sizes live in `QualityDefinitions`).

`FormatItems` = `[{ "format": <CustomFormats.Id>, "score": <int> }]`, e.g. `[ { "format": 1, "score": 100 }, { "format": 2, "score": -10000 } ]`.

Default profiles created on first run (VERIFIED v4): `Any`(cutoff 1), `SD`(1), `HD-720p`(4), `HD-1080p`(9),
`Ultra-HD`(16), `HD - 720p/1080p`(4), all `UpgradeAllowed=0`.

### 4.6 `QualityProfileQualityRanks` — **v5+ only (233)**
`Id`, `ProfileId` INTEGER NN, `QualityId` INTEGER NN, `Score` NUMERIC NN; unique (ProfileId, QualityId). **Derived** from
profile Items (`QualityProfileRankService.ComputeRanks`, score = index/(n-1)) for SQL sorting. Skip on import.

### 4.7 `QualityDefinitions` (`Core/Qualities/QualityDefinition.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Id | INTEGER PK | ✓ | |
| Quality | INTEGER | ✓ | Quality id (unique) |
| Title | TEXT | ✓ | user-editable display name (unique), default = quality name |
| MinSize | NUMERIC | | **v4 only** (dropped in v5 207) MB/min |
| MaxSize | NUMERIC | | **v4 only**; NULL **or 0** = unlimited (`AcceptableSizeSpecification.cs`: bytes = MB(1024²) × total episode runtime minutes) |
| PreferredSize | NUMERIC | | **v4 only** (181) |
`GroupName`, `Weight` are model-only (ignored). Defaults (VERIFIED v4) — see §6.1 table (min/max/preferred).

### 4.8 `CustomFormats` (`Core/CustomFormats/CustomFormat.cs`, `Specifications/*.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Id | INTEGER PK | ✓ | |
| Name | TEXT | ✓ | unique |
| Specifications | TEXT JSON | ✓ (default `'[]'`) | polymorphic list |
| IncludeCustomFormatWhenRenaming | INTEGER bool | ✓ (0) | |

`Specifications` = `[{ "type": "<SpecClassName>", "body": { … } }]`. Body = all public props of the spec (camelCase) incl.
read-only `order`, `implementationName`, `infoLink`; common: `name` (string), `negate` (bool), `required` (bool).

| `type` (class) | ImplementationName | Order | value fields in `body` |
|---|---|---|---|
| `ReleaseTitleSpecification` | Release Title | 1 | `value`: string (.NET regex, case-insensitive) |
| `LanguageSpecification` | Language | 3 | `value`: int Language id (−2 = Original); `exceptLanguage`: bool |
| `IndexerFlagSpecification` | Indexer Flag | 4 | `value`: int IndexerFlags bit (1,2,4,…,256) |
| `SourceSpecification` | Source | 5 | `value`: int `QualitySource` |
| `ResolutionSpecification` | Resolution | 6 | `value`: int `Resolution` (360,480,540,576,720,1080,2160) |
| `SizeSpecification` | Size | 8 | `min`, `max`: double **GB** |
| `ReleaseGroupSpecification` | Release Group | 9 | `value`: string regex |
| `ReleaseTypeSpecification` | Release Type | 10 | `value`: int `ReleaseType` (renamed from `SeasonPackSpecification` in 205) |

Example:
```json
[
  {
    "type": "ReleaseTitleSpecification",
    "body": {
      "order": 1,
      "implementationName": "Release Title",
      "infoLink": "https://wiki.servarr.com/sonarr/settings#custom-formats-2",
      "value": "\\b(x265|HEVC)\\b",
      "name": "x265",
      "negate": false,
      "required": true
    }
  },
  {
    "type": "SizeSpecification",
    "body": { "order": 8, "implementationName": "Size", "infoLink": "https://wiki.servarr.com/sonarr/settings#custom-formats-2",
              "min": 0, "max": 2.5, "name": "Small", "negate": false, "required": false }
  }
]
```
(Property order inside `body` is not guaranteed; parse by key. The C# reader requires `type` before `body` in the wrapper.)

### 4.9 `DelayProfiles` (`Core/Profiles/Delay/DelayProfile.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| EnableUsenet | INTEGER bool | ✓ | |
| EnableTorrent | INTEGER bool | ✓ | |
| PreferredProtocol | INTEGER | ✓ | `DownloadProtocol` 1=Usenet, 2=Torrent |
| UsenetDelay | INTEGER | ✓ | minutes |
| TorrentDelay | INTEGER | ✓ | minutes |
| Order | INTEGER | ✓ | evaluation order; the default (tag-less) profile has `2147483647` (VERIFIED) |
| Tags | TEXT JSON `int[]` | ✓ | empty = default profile |
| BypassIfHighestQuality | INTEGER bool | ✓ (0) | (156; existing rows set to 1) |
| BypassIfAboveCustomFormatScore | INTEGER bool | ✓ (0) | (182) |
| MinimumCustomFormatScore | INTEGER | (nullable) | (182) model `int` (NULL→0) |
Default row (VERIFIED v4): `1|1|1|1|0|0|2147483647|[]|1|0|NULL`.

### 4.10 `ReleaseProfiles` (`Core/Profiles/Releases/ReleaseProfile.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Id | INTEGER PK | ✓ | |
| Name | TEXT | | (154) |
| Enabled | INTEGER bool | ✓ | (136) |
| Required | TEXT JSON `string[]` | | "must contain" terms; a term wrapped in `/.../` (optionally `/i`) is a regex. Legacy comma-separated tolerated (§5.3) |
| Ignored | TEXT JSON `string[]` | | "must not contain" terms |
| Tags | TEXT JSON `int[]` | ✓ | series tags it applies to (empty = all) |
| IndexerId | INTEGER | ✓ | **v4 only**: 0 = any indexer. **Dropped in v5 (224)** |
| IndexerIds | TEXT JSON `int[]` | | **v5+ (224)** default `'[]'` = all |
| ExcludedTags | TEXT JSON `int[]` | ✓ | **v5+ (221)** default `'[]'` |
| AirDateRestriction | INTEGER bool | | **v5+ (226)** default 0 |
| AirDateGracePeriod | INTEGER | | **v5+ (226)** default 0 (days) |
| AllowSeasonPackWithoutAllEpisodesAired | INTEGER bool | | **v5+ (232)** default 0 |
Pre-171 rows also had `Preferred` (JSON `[{key, value}]`) and `IncludePreferredWhenRenaming` — converted to custom formats.

### 4.11 `Tags` (`Core/Tags/Tag.cs`)
`Id`, `Label` TEXT NN (unique `IX_Tags_Label`, SQLite binary collation). Tag ids are referenced from JSON `Tags` arrays in
Series, DelayProfiles, ReleaseProfiles, Indexers, DownloadClients, Notifications, ImportLists, AutoTagging (and
`AutoTagging.Specifications` TagSpecification, `ListSyncTag` config). Metadata.Tags is NOT a column (ignored).

### 4.12 `RootFolders` (`Core/RootFolders/RootFolder.cs`)
`Id`, `Path` TEXT NN (unique). `Accessible`, `IsEmpty`, `FreeSpace`, `TotalSpace`, `UnmappedFolders` are not stored.

### 4.13 `NamingConfig` (`Core/Organizer/NamingConfig.cs`) — single row (may be **absent**; then defaults apply and
`NamingConfigService.GetConfig()` inserts `NamingConfig.Default` on first use — VERIFIED: fresh v4 DB has 0 rows)
| Column | Type | NN | Default (code) | Meaning |
|---|---|---|---|---|
| MultiEpisodeStyle | INTEGER | ✓ | 5 PrefixedRange | `MultiEpisodeStyle` §6 |
| RenameEpisodes | INTEGER bool | (nullable) | 0 | |
| StandardEpisodeFormat | TEXT | | `{Series Title} - S{season:00}E{episode:00} - {Episode Title} {Quality Full}` | |
| DailyEpisodeFormat | TEXT | | `{Series Title} - {Air-Date} - {Episode Title} {Quality Full}` | |
| AnimeEpisodeFormat | TEXT | | `{Series Title} - S{season:00}E{episode:00} - {Episode Title} {Quality Full}` | |
| SeriesFolderFormat | TEXT | | `{Series Title}` | |
| SeasonFolderFormat | TEXT | | `Season {season}` | |
| SpecialsFolderFormat | TEXT | | `Specials` | (134) |
| ReplaceIllegalCharacters | INTEGER bool | ✓ | 1 | (093) |
| ColonReplacementFormat | INTEGER | ✓ | 4 Smart | `ColonReplacementFormat` §6 (189) |
| CustomColonReplacementFormat | TEXT | ✓ | `''` | used when ColonReplacementFormat=5 (211) |
No JSON columns. Tokens like `{Custom Formats}` replaced `{Preferred Words}` in 171.

### 4.14 `Config` (`Core/Configuration/Config.cs`, `ConfigService.cs`, `ConfigRepository.cs`)
`Id`, `Key` TEXT NN (unique, **always lowercase** — `Config.Key` setter and `ConfigService.SetValue` lowercase it),
`Value` TEXT NN. Rows exist **only for values that were explicitly saved** (defaults are not persisted, except the
`persist: true` keys below). VERIFIED: fresh v4 DB contains only `enablecompleteddownloadhandling=True`.
Value encodings: bool `True`/`False`; int decimal; double invariant culture; enum = **name**, either lowercase (internal
`SetValue(Enum)`) or PascalCase (API `SaveConfigDictionary` → `ToString()`); parse case-insensitively, also accept digits.

| Key (lowercase in DB) | Type | Default | Meaning |
|---|---|---|---|
| `downloadclientworkingfolders` | string | `_UNPACK_\|_FAILED_` | pipe-separated folder names ignored during import |
| `downloadclienthistorylimit` | int | 60 | number of history items fetched from SABnzbd/NZBGet |
| `enablecompleteddownloadhandling` | bool | True | CDH |
| `autoredownloadfailed` | bool | True | |
| `autoredownloadfailedfrominteractivesearch` | bool | True | |
| `autounmonitorpreviouslydownloadedepisodes` | bool | False | unmonitor deleted episodes |
| `recyclebin` | string | `""` | recycle bin path |
| `recyclebincleanupdays` | int | 7 | |
| `downloadpropersandrepacks` | enum ProperDownloadTypes | PreferAndUpgrade | §6 |
| `createemptyseriesfolders` | bool | False | |
| `deleteemptyfolders` | bool | False | |
| `filedate` | enum FileDateType | None | §6 |
| `skipfreespacecheckwhengrabbing` | bool | False | (v5 source; v4 availability UNVERIFIED) |
| `skipfreespacecheckwhenimporting` | bool | False | |
| `minimumfreespacewhenimporting` | int (MB) | 100 | |
| `copyusinghardlinks` | bool | True | |
| `enablemediainfo` | bool | True | |
| `usescriptimport` | bool | False | |
| `scriptimportpath` | string | `""` | |
| `importextrafiles` | bool | False | |
| `extrafileextensions` | string | `srt` | comma-separated |
| `rescanafterrefresh` | enum RescanAfterRefreshType | Always | §6 |
| `episodetitlerequired` | enum EpisodeTitleRequiredType | Always | §6 |
| `userrejectedextensions` | string | `""` | comma-separated extensions to reject |
| `seasonpackupgrade` | enum SeasonPackUpgradeType | All | §6 (v5 source; v4 UNVERIFIED) |
| `seasonpackupgradethreshold` | double | 100.0 | percent (v5 source) |
| `setpermissionslinux` | bool | False | |
| `chmodfolder` | string | `755` | |
| `chowngroup` | string | `""` | |
| `retention` | int (days) | 0 | usenet retention |
| `rsssyncinterval` | int (min) | 15 | 0 = disabled; 1..9 coerced to 10 |
| `maximumsize` | int (MB) | 0 | 0 = unlimited |
| `minimumage` | int (min) | 0 | usenet minimum age |
| `listsynclevel` | enum ListSyncLevelType | Disabled | §6 |
| `listsynctag` | int | 0 | tag id used by KeepAndTag |
| `firstdayofweek` | int | current culture (0=Sunday) | UI |
| `calendarweekcolumnheader` | string | `ddd M/D` | UI (moment.js format) |
| `shortdateformat` | string | `MMM D YYYY` | UI |
| `longdateformat` | string | `dddd, MMMM D YYYY` | UI |
| `timeformat` | string | `h(:mm)a` | UI |
| `timezone` | string | `""` | UI (v5 source) |
| `showrelativedates` | bool | True | UI |
| `enablecolorimpairedmode` | bool | False | UI |
| `headercolor` | string | `fullColor` | UI (v5 source) |
| `uilanguage` | int | 1 (English) | UI language id |
| `preferredmetadatalanguage` | int | 1 | (v5 source, translations) |
| `cleanupmetadataimages` | bool | True | internal one-shot flag |
| `plexclientidentifier` | string | new GUID (**persisted on first read**) | |
| `rijndaelpassphrase`, `hmacpassphrase`, `rijndaelsalt`, `hmacsalt` | string | new GUID (**persisted**) | forms-auth cookie crypto; do not import |
| `proxyenabled` | bool | False | |
| `proxytype` | enum ProxyType | Http | `Http=0, Socks4=1, Socks5=2` |
| `proxyhostname` | string | `""` | |
| `proxyport` | int | 8080 | |
| `proxyusername`, `proxypassword` | string | `""` | |
| `proxybypassfilter` | string | `""` | comma list |
| `proxybypasslocaladdresses` | bool | True | |
| `backupfolder` | string | `Backups` | relative to AppData unless rooted |
| `backupinterval` | int (days) | 7 | 1..7 |
| `backupretention` | int (days) | 28 | 1..90 |
| `certificatevalidation` | enum CertificateValidationType | Enabled | `Enabled=0, DisabledForLocalAddresses=1, Disabled=2` |
| `applicationurl` | string | `""` | external URL |
| `trustcgnatipaddresses` | bool | False | also in config.xml |
Legacy keys that may still exist in old DBs and are ignored by current code: `downloadedepisodesfolder` (drone factory,
migrated to a Blackhole client in 051), `removecompleteddownloads`/`removefaileddownloads` (deleted 158),
`autodownloadpropers` (→ `downloadpropersandrepacks` in 131), `filechmod`/`folderchmod`/`chownuser` (140/147),
`seasonfolderformat`/`useseasonfolder`, `releaserestrictions`.

### 4.15 `RemotePathMappings` (`Core/RemotePathMappings/RemotePathMapping.cs`)
`Id`, `Host` TEXT NN (download-client host as configured), `RemotePath` TEXT NN, `LocalPath` TEXT NN
(both paths normalized with a trailing separator on add/update: `new OsPath(p).AsDirectory().FullPath`,
`RemotePathMappingService.cs` lines 53-54).

### 4.16 Provider tables (common shape — `Core/ThingiProvider/ProviderDefinition.cs`)
All four provider tables + `Metadata` share: `Name` TEXT NN, `Implementation` TEXT NN (= provider **C# class name**, e.g.
`Newznab`, `QBittorrent`), `ConfigContract` TEXT (= settings **C# class name**, e.g. `NewznabSettings`; Sonarr resolves the
settings type from this column), `Settings` TEXT JSON (settings object, §7), `Tags` TEXT JSON `int[]` (not Metadata).
`Enable` is a real column only on DownloadClients and Metadata; for Indexers/ImportLists/Notifications it is computed.
`ImplementationName`, `Message` (ProviderMessage), `Protocol`, `Supports*`, `ListType`, `MinRefreshInterval` are **not stored**.

#### `Indexers` (`Core/Indexers/IndexerDefinition.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Name | TEXT | ✓ | unique |
| Implementation, ConfigContract, Settings | TEXT | Impl ✓ | |
| EnableRss | INTEGER bool | | |
| EnableAutomaticSearch | INTEGER bool | | (119, renamed from EnableSearch) |
| EnableInteractiveSearch | INTEGER bool | ✓ | |
| Priority | INTEGER | ✓ (25) | 1..50, lower = preferred |
| Tags | TEXT JSON | | (159) |
| DownloadClientId | INTEGER | ✓ (0) | 0 = any client (164) |
| SeasonSearchMaximumSingleEpisodeAge | INTEGER | ✓ (0) | days (172) |

#### `DownloadClients` (`Core/Download/DownloadClientDefinition.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Enable | INTEGER bool | ✓ | |
| Name, Implementation, Settings, ConfigContract | TEXT | ✓ | (Name not unique in v4 DDL) |
| Priority | INTEGER | ✓ (1) | 1..50 |
| RemoveCompletedDownloads | INTEGER bool | ✓ (1) | (158; may be TEXT `'false'`, see §5.2) |
| RemoveFailedDownloads | INTEGER bool | ✓ (1) | |
| Tags | TEXT JSON | | (191) |

#### `Notifications` (`Core/Notifications/NotificationDefinition.cs`)
`Name` NN, `Implementation` NN, `Settings` NN, `ConfigContract`, `Tags`, and trigger booleans (INTEGER):
`OnGrab` NN, `OnDownload` NN (= "On Import"), `OnUpgrade` (nullable), `OnImportComplete` NN(0, 209), `OnRename` NN,
`OnSeriesAdd` NN(0, 187), `OnSeriesDelete` NN(0, 149), `OnEpisodeFileDelete` NN(0, 149),
`OnEpisodeFileDeleteForUpgrade` NN(1, 153), `OnHealthIssue` NN(0, 135), `IncludeHealthWarnings` NN(0, 135),
`OnHealthRestored` NN(0, 190), `OnApplicationUpdate` NN(0, 165), `OnManualInteractionRequired` NN(0, 177).
(`Supports*` flags are computed, not stored.)

#### `ImportLists` (`Core/ImportLists/ImportListDefinition.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Name | TEXT | ✓ | unique |
| Implementation, Settings, ConfigContract | TEXT | Impl ✓ | |
| EnableAutomaticAdd | INTEGER bool | | |
| SearchForMissingEpisodes | INTEGER bool | ✓ (1) | (197) |
| ShouldMonitor | INTEGER | ✓ | **`MonitorTypes` enum int** (§6) |
| MonitorNewItems | INTEGER | ✓ (0) | `NewItemMonitorTypes` (200) |
| RootFolderPath | TEXT | ✓ | root folder path string (not an id) |
| QualityProfileId | INTEGER | ✓ | |
| SeriesType | INTEGER | ✓ | `SeriesTypes` (144) |
| SeasonFolder | INTEGER bool | ✓ | (144) |
| Tags | TEXT JSON | | |
| Language | INTEGER | | **v5+ (208)** default 1 |
| TagExisting | INTEGER | | **v5+**: bool in 231, `TagExistingType` int (0 None, 1 Add, 2 Sync) since 236 |
(Pre-175 had `LanguageProfileId`.)

#### `Metadata` (`Core/Extras/Metadata/MetadataDefinition.cs`) — "metadata consumers"
`Enable` INTEGER NN, `Name` NN, `Implementation` NN, `Settings` NN, `ConfigContract` NN. Rows are auto-created for every
consumer on startup (VERIFIED v4: 5 rows incl. `KometaMetadata` which no longer exists in the v5 tree — orphan
`ConfigContract` → Sonarr loads `NullConfig`). Settings: see §7.5.

#### Status tables (operational state; skip on import)
`IndexerStatus`, `DownloadClientStatus`, `ImportListStatus`, `NotificationStatus` (194): `ProviderId` INTEGER NN unique,
`InitialFailure`, `MostRecentFailure`, `DisabledTill` DATETIME, `EscalationLevel` INTEGER NN;
`IndexerStatus.LastRssSyncReleaseInfo` TEXT JSON (`ReleaseInfo`); `ImportListStatus.LastInfoSync` DATETIME (178),
`HasRemovedItemSinceLastClean` INTEGER NN (193).

### 4.17 `ImportListExclusions` (`Core/ImportLists/Exclusions/ImportListExclusion.cs`)
`Id`, `TvdbId` INTEGER NN (unique; TEXT before 192 — cast on read), `Title` TEXT NN.

### 4.18 `ImportListItems` (193) — cached list contents (`Core/Parser/Model/ImportListItemInfo.cs`); skip
`ImportListId` NN, `Title` NN, `TvdbId` NN, `Year`, `TmdbId`, `ImdbId`, `MalId`, `AniListId`, `ReleaseDate` DATETIME.

### 4.19 `History` (EpisodeHistory, `Core/History/EpisodeHistory.cs`, written by `Core/History/HistoryService.cs`)
| Column | Type | NN | Meaning |
|---|---|---|---|
| Id | INTEGER PK | ✓ | |
| EpisodeId | INTEGER | ✓ | one row **per episode** (multi-episode grabs produce several rows sharing DownloadId) |
| SeriesId | INTEGER | ✓ | |
| SourceTitle | TEXT | ✓ | release title / file name |
| Date | DATETIME | ✓ | |
| Quality | TEXT JSON `QualityModel` | ✓ | `{"quality":3,"revision":{"version":1,"real":0,"isRepack":false}}` |
| Data | TEXT JSON `Dictionary<string,string>` | ✓ | event-specific; keys **camelCased** on write |
| EventType | INTEGER | (nullable) | `EpisodeHistoryEventType` §6 |
| DownloadId | TEXT | | download-client item id (e.g. SABnzbd `nzo_id`, torrent info-hash) |
| Languages | TEXT JSON `int[]` | ✓ | (175) |
Indexes: `IX_History_Date`, `IX_History_EventType`, `IX_History_EpisodeId_Date`, `IX_History_DownloadId_Date`,
`IX_History_SeriesId`; v5+ `IX_History_QualityId` (json_extract expression).

`Data` keys by event (C# adds PascalCase keys; stored camelCase; all values strings, may be `null`):
* **Grabbed (1)**: `indexer, nzbInfoUrl, releaseGroup, age, ageHours, ageMinutes, publishedDate` (ISO `yyyy-MM-ddTHH:mm:ssZ`),
  `downloadClient` (client type name), `downloadClientName`, `size` (bytes), `downloadUrl, guid, tvdbId, tvRageId, imdbId`,
  `protocol` (`"1"` usenet / `"2"` torrent), `customFormatScore`, `seriesMatchType` (`Unknown|Title|Alias|Id`),
  `releaseSource` (`Unknown|Rss|Search|UserInvokedSearch|InteractiveSearch|ReleasePush`), `indexerFlags` (enum ToString,
  e.g. `"Freeleech, Internal"` or `"0"`), `releaseType` (`SingleEpisode`…), optional `releaseHash`, `torrentInfoHash`.
* **DownloadFolderImported (3)**: `fileId, droppedPath, importedPath, downloadClient, downloadClientName, releaseGroup,
  customFormatScore, size, indexerFlags, releaseType`. (SeriesFolderImported (2) is defined but no current code path writes it — grep finds no producer; old rows may have it.)
* **DownloadFailed (4)**: `downloadClient, downloadClientName, message, source, releaseGroup, size, indexer`.
* **EpisodeFileDeleted (5)**: `reason` (`MissingFromDisk|Manual|Upgrade|NoLinkedEpisodes|ManualOverride`), `releaseGroup,
  size, indexerFlags, releaseType`.
* **EpisodeFileRenamed (6)**: `sourcePath, sourceRelativePath, path, relativePath, releaseGroup, size, indexerFlags, releaseType`.
* **DownloadIgnored (7)**: `downloadClient, downloadClientName, message, releaseGroup, size, indexer, releaseType`.
Example (Grabbed):
```json
{
  "indexer": "NZBgeek (Prowlarr)",
  "nzbInfoUrl": "https://nzbgeek.info/geekseek.php?guid=abc",
  "releaseGroup": "NTb",
  "age": "0", "ageHours": "1.5", "ageMinutes": "92.3",
  "publishedDate": "2024-03-01T10:15:00Z",
  "downloadClient": "Sabnzbd", "downloadClientName": "SABnzbd",
  "size": "1572864000",
  "downloadUrl": "http://prowlarr:9696/1/download?...",
  "guid": "https://nzbgeek.info/geekseek.php?guid=abc",
  "tvdbId": "81189", "tvRageId": "0", "imdbId": null,
  "protocol": "1",
  "customFormatScore": "0",
  "seriesMatchType": "Id",
  "releaseSource": "Rss",
  "indexerFlags": "0",
  "releaseType": "SingleEpisode"
}
```
(Older rows written by v2/v3 contain additional/older keys; treat `Data` as an open string map.)
Gotcha: numeric values are produced with `.ToString()` under the **host's current culture** (no invariant culture is
set anywhere in the tree), so `ageHours`/`ageMinutes` may look like `1,5` on e.g. a German-locale host — parse leniently.

### 4.20 `Blocklist` (`Core/Blocklisting/Blocklist.cs`; table named `Blacklist` before migration 160)
| Column | Type | NN | Meaning |
|---|---|---|---|
| SeriesId | INTEGER | ✓ | index `IX_Blacklist_SeriesId` (old name kept) |
| EpisodeIds | TEXT JSON `int[]` | ✓ | |
| SourceTitle | TEXT | ✓ | |
| Quality | TEXT JSON `QualityModel` | ✓ | |
| Date | DATETIME | ✓ | when blocklisted |
| PublishedDate | DATETIME | | release publish date |
| Size | INTEGER | | bytes |
| Protocol | INTEGER | | `DownloadProtocol` |
| Indexer | TEXT | | indexer name |
| Message | TEXT | | failure reason |
| TorrentInfoHash | TEXT | | |
| Languages | TEXT JSON `int[]` | ✓ | |
| IndexerFlags | INTEGER | ✓ (0) | (202) |
| ReleaseType | INTEGER | ✓ (0) | (203) |
| Source | TEXT | | **v5+ (223)** |

### 4.21 `DownloadHistory` (`Core/Download/History/DownloadHistory.cs`, 139) — per-download events
`EventType` INTEGER NN (`DownloadHistoryEventType`: 1 DownloadGrabbed, 2 DownloadImported, 3 DownloadFailed,
4 DownloadIgnored, 5 FileImported), `SeriesId` NN, `DownloadId` TEXT NN, `SourceTitle` NN, `Date` NN, `Protocol` INTEGER,
`IndexerId` INTEGER, `DownloadClientId` INTEGER, `Release` TEXT JSON (`ReleaseInfo`, only on grab),
`Data` TEXT JSON dict (keys camelCase: `indexer, downloadClient, downloadClientName, customFormatScore` on grab;
`downloadClient, downloadClientName, sourcePath, destinationPath` on FileImported; client keys otherwise).
`ReleaseInfo` JSON (`Core/Parser/Model/ReleaseInfo.cs`, runtime type may be `TorrentInfo` adding `magnetUrl, infoHash,
seeders, peers`): `guid, title, size, downloadUrl, infoUrl, commentUrl, indexerId, indexer, indexerPriority,
seasonSearchMaximumSingleEpisodeAge, downloadProtocol ("usenet"/"torrent"), tvdbId, tvRageId, imdbId, publishDate,
origin, source, container, codec, resolution, languages ([int])` plus the computed `age` (int days), `ageHours`,
`ageMinutes` (double; public getter/private setter → written, ignored on read). `IndexerFlags` and `PendingReleaseReason`
are `[System.Text.Json.Serialization.JsonIgnore]` → **not stored** in `Release` JSON. `downloadProtocol` is an enum →
camelCase string (`"usenet"`/`"torrent"`).

### 4.22 `PendingReleases` (`Core/Download/Pending/PendingRelease.cs`) — delayed grabs; usually skip
`SeriesId` NN, `Title` NN, `Added` DATETIME NN, `ParsedEpisodeInfo` TEXT JSON NN, `Release` TEXT JSON NN (`ReleaseInfo`),
`Reason` INTEGER NN (`PendingReleaseReason` 0 Delay, 1 DownloadClientUnavailable, 2 Fallback), `AdditionalInfo` TEXT JSON
(`{"seriesMatchType": "title", "releaseSource": "rss"}` — enums as camelCase strings) (168).
`ParsedEpisodeInfo` (`Core/Parser/Model/ParsedEpisodeInfo.cs`): `releaseTitle, seriesTitle, seriesTitleInfo{title,
titleWithoutYear, year, allTitles[]}, quality{quality:int, revision{…}}, seasonNumber` (**v4: int**) / `seasonNumbers`
(**v5 after 237: int[]**), `episodeNumbers[], absoluteEpisodeNumbers[], specialAbsoluteEpisodeNumbers[] (decimal),
airDate, languages[int], fullSeason, isPartialSeason, isMultiSeason, isSeasonExtra, isSeasonTitle, isSplitEpisode,
isMiniSeries, special, releaseGroup, releaseHash, seasonPart, releaseTokens, dailyPart`, plus computed flags.

### 4.23 `AutoTagging` (`Core/AutoTagging/AutoTag.cs`, 179)
`Name` TEXT NN unique, `Specifications` TEXT JSON NN (default `'[]'`, same `{type, body}` wrapper as custom formats),
`RemoveTagsAutomatically` INTEGER bool NN, `Tags` TEXT JSON `int[]` NN (tags to apply).
Spec classes (`Core/AutoTagging/Specifications/*.cs`; common body fields `name, negate, required` + read-only
`order, implementationName`):

| `type` | value field(s) |
|---|---|
| `GenreSpecification` | `value`: string[] |
| `MonitoredSpecification` | (none) |
| `NetworkSpecification` | `value`: string[] |
| `OriginalCountrySpecification` | `value`: string[] (v5 tree; v4 UNVERIFIED) |
| `OriginalLanguageSpecification` | `value`: int Language id |
| `QualityProfileSpecification` | `value`: int profile id |
| `RootFolderSpecification` | `value`: string path |
| `SeriesTypeSpecification` | `value`: int SeriesTypes |
| `StatusSpecification` | **`status`**: int SeriesStatusType (note: not `value`) |
| `TagSpecification` | `value`: int tag id |
| `YearSpecification` | `min`, `max`: int |

### 4.24 `SceneMappings` (`Core/DataAugmentation/Scene/SceneMapping.cs`) — downloaded from Sonarr services; skip/refetch
`TvdbId` NN, `SeasonNumber` INTEGER (NULL or −1 = all seasons, per `SceneMappingService`), `SearchTerm` NN, `ParseTerm` NN, `Title`, `Type` TEXT (provider
class name), `SceneSeasonNumber`, `FilterRegex` (112), `SceneOrigin`, `SearchMode` INTEGER (`SearchMode` 0 Default,
1 SearchID, 2 SearchTitle), `Comment` (150), `MappingId` TEXT (**v5+ 230**).

### 4.25 `Users` (`Core/Authentication/User.cs`, `UserService.cs`)
`Identifier` TEXT NN unique (Guid string), `Username` TEXT NN unique (**stored lowercase**), `Password` TEXT NN,
`Salt` TEXT (174), `Iterations` INTEGER (174). Hash: if `Salt` empty → legacy `Password` = lowercase hex SHA-256 of UTF-8
password; else `Password` = Base64(PBKDF2-HMAC-SHA512(password, Base64Decode(Salt), Iterations, 32 bytes)), salt 16 bytes;
v5 target iterations 220000 (re-hashed on login when lower). These hashes are portable to a Rust verifier.

### 4.26 `ScheduledTasks` (`Core/Jobs/ScheduledTask.cs`, `Core/Jobs/TaskManager.cs`) — skip
`TypeName` TEXT NN unique (fully-qualified command type, e.g. `NzbDrone.Core.Backup.BackupCommand`), `Interval` INTEGER NN
(minutes), `LastExecution` DATETIME NN, `LastStartTime` DATETIME (180). (`Priority` ignored.) VERIFIED v4 defaults:
RefreshMonitoredDownloads 1, MessagingCleanup 5, ApplicationUpdateCheck 360, UpdateSceneMapping 180, CheckHealth 360,
RefreshSeries 720, Housekeeping 1440, CleanUpRecycleBin 1440, ImportListSync 5, Backup 10080, RssSync 15.

### 4.27 Other tables present in backups (usually not needed)
* `CustomFilters`: `Type` TEXT NN (e.g. `series`), `Label` NN, `Filters` TEXT NN (JSON array of `{key, value, type}`
  objects authored by the UI).
* `MetadataFiles`, `SubtitleFiles`, `ExtraFiles` (099): extra-file tracking (`SeriesId, SeasonNumber, EpisodeFileId,
  RelativePath, Extension, Added, LastUpdated`; Metadata adds `Consumer, Type, Hash`; Subtitles add `Language` INTEGER,
  `LanguageTags` TEXT JSON string[], `Title`, `Copy`).
* `Commands`: queued/finished commands (wiped by 188; transient).
* `VersionInfo`: §3.2.

---
## 5. Serialization rules (referenced throughout §4)

Sources: `Core/Datastore/TableMapping.cs` (`RegisterMappers`), `Core/Datastore/Converters/*.cs`,
`Core/ThingiProvider/ProviderRepository.cs`, `Common/Serializer/System.Text.Json/*.cs`,
`Core/Datastore/ConnectionStringFactory.cs`, `Common/Serializer/Newtonsoft.Json/Json.cs`.

### 5.1 Column mapping
* ORM = **Dapper** with a hand-rolled `TableMapper`. A model property becomes a column iff it is public get+set AND
  (`IsSimpleType()` — primitives, enums, string, DateTime, Version, decimal, `Nullable<>`/`List<>`/`IEnumerable<>` of those,
  KeyValuePair — OR a Dapper type handler is registered for its type) AND it is not `.Ignore()`d in `TableMapping.Map()`
  (`Core/Datastore/Extensions/MappingExtensions.cs`). **Column name = C# property name, PascalCase, quoted** (`"Series"."TvdbId"`).
* Every table has `"Id" INTEGER PRIMARY KEY AUTOINCREMENT` (from `ModelBase.Id`).
* `LazyLoaded<T>` properties (e.g. `Series.QualityProfile`, `EpisodeFile.Series`, `EpisodeFile.Episodes`,
  `Episode.EpisodeFile`) are **not columns**; relations are plain integer FK columns (`QualityProfileId`, `SeriesId`,
  `EpisodeFileId`, …). There are **no SQL foreign-key constraints**; orphans are possible (housekeeping cleans them).
* Columns that exist in the DB but are no longer in the model are simply ignored by Sonarr (e.g. `Series.LastDiskSync`,
  `Series.NextAiring` — still present, always NULL/stale in v4).

### 5.2 Scalar types (SQLite)
| C# type | Stored as | Notes |
|---|---|---|
| `int`, `long` | INTEGER | |
| `bool` | INTEGER 0/1 | System.Data.SQLite default. **Exception:** migration 158 wrote TEXT `'false'` into `DownloadClients.RemoveCompletedDownloads` for RTorrent/Flood rows; old migrations may contain similar text booleans. Importer: accept 0/1, `'0'/'1'`, `'true'/'false'` (any case). |
| `enum` (top-level column) | INTEGER (underlying value) | e.g. `Series.SeriesType`, `Series.Status`, `History.EventType`, `Blocklist.Protocol`. |
| `string` | TEXT | NULL allowed unless column NOT NULL |
| `double`/`decimal` | NUMERIC affinity | e.g. `QualityDefinitions.MinSize` (`199.9`) |
| `DateTime` / `DateTime?` | TEXT in a `DATETIME` column | `DapperUtcConverter.SetValue` converts to UTC; connection string has `DateTimeKind=Utc`; System.Data.SQLite ISO8601 format. **VERIFIED (live v4 DB)**: `yyyy-MM-dd HH:mm:ss.FFFFFFFZ`, e.g. `2026-11-09 17:23:11.5173711Z`, `0001-01-01 00:00:00Z` (trailing zero fraction digits trimmed, fraction omitted when 0, literal `Z`). Rows written by raw SQL in migrations can lack the `Z` and fraction (`CURRENT_TIMESTAMP` → `2024-03-01 12:00:00`; migration 208 writes `2026-01-01 00:00:00`). Treat zone-less values as UTC. `DateTime.MinValue` (`0001-01-01 00:00:00Z`) is used as "never" in NOT NULL columns. |
| `TimeSpan` | TEXT `TimeSpan.ToString()` ("c" format) | e.g. `Commands.Duration` = `00:00:00.0133349` (VERIFIED) |
| `Guid` | TEXT `Guid.ToString()` | lowercase, dashed: `3f2504e0-4f89-11d3-9a0c-0305e82c3301` (`GuidConverter`) |
| `Version` | TEXT `Version.ToString()` | `SystemVersionConverter` (UpdateHistory, logs DB) |
| `OsPath` | TEXT full path | |
| `Quality` (top-level) | INTEGER quality id | `DapperQualityIntConverter` (`QualityDefinitions.Quality`) |
| `Language` (top-level) | INTEGER language id | `DapperLanguageIntConverter` (`Series.OriginalLanguage`, `Series.Language`, `ImportLists.Language`, `SubtitleFiles.Language`, `SeriesTranslations.Language`); NULL → Unknown(0) |

### 5.3 JSON (embedded document) columns
* Any `IEmbeddedDocument` type, `List<IEmbeddedDocument>`, and the explicitly registered types
  (`List<int>`, `HashSet<int>`, `List<string>`, `Dictionary<string,string>`, `List<KeyValuePair<string,int>>`,
  `ParsedEpisodeInfo`, `ReleaseInfo`, `PendingReleaseAdditionalInfo`, `List<QualityProfileQualityItem>`,
  `List<ProfileFormatItem>`, `List<ICustomFormatSpecification>`, `List<IAutoTaggingSpecification>`, `QualityModel`,
  `List<Language>`, `Command`) are stored as **JSON text** via `EmbeddedDocumentConverter<T>`.
* System.Text.Json options used by `EmbeddedDocumentConverter` (identical in `ProviderRepository` and `STJson`):
  ```csharp
  AllowTrailingCommas = true,
  DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,   // null properties omitted
  PropertyNameCaseInsensitive = true,                            // reads accept any casing
  DictionaryKeyPolicy = JsonNamingPolicy.CamelCase,              // dictionary KEYS camel-cased on write!
  PropertyNamingPolicy = JsonNamingPolicy.CamelCase,             // property names camelCase
  WriteIndented = true                                           // pretty-printed, 2-space indent
  Converters: JsonStringEnumConverter(JsonNamingPolicy.CamelCase, allowIntegerValues: true),
              STJTimeSpanConverter, STJUtcConverter
  ```
  * **Property names: camelCase** (STJ algorithm: lowercase leading run of capitals except the last one before a lowercase
    letter: `TvdbId`→`tvdbId`, `AudioCodecID`→`audioCodecID`, `VideoCodecID`→`videoCodecID`, `UseSsl`→`useSsl`,
    `WebHookUrl`→`webHookUrl`, `IsValid`→`isValid`, `ID`→`id`).
  * **Enums inside JSON: camelCase strings** (`"monitor": "all"`, `"coverType": "poster"`, `"trigger": "scheduled"`,
    `"videoHdrFormat": "dolbyVisionHdr10"`). Integer values are also accepted on read (`allowIntegerValues: true`) and
    older Newtonsoft-era data uses the same camelCase-string convention. Enum-like settings declared as `int` in provider
    settings (most priorities/list types) are plain numbers.
  * **DateTime inside JSON**: `STJUtcConverter` writes `yyyy-MM-ddTHH:mm:ssZ` (no fraction, UTC), e.g.
    `"lastExecutionTime": "2026-11-09T17:21:41Z"` (VERIFIED). Reads use `DateTime.Parse(...).ToUniversalTime()`.
    Data written by migrations via Newtonsoft (`ToJson()`) uses ISO-8601 with optional 7-digit fraction and `Z`.
  * **TimeSpan inside JSON**: string `hh:mm:ss[.fffffff]` (e.g. MediaInfo `"runTime": "00:42:13.0400000"`).
  * **Read-only properties are serialized too** (STJ default), so JSON may contain computed keys that must be ignored on
    read: `isValid` in many settings (VERIFIED in Metadata rows), `primaryAudioStream` (MediaInfo v5), custom-format spec
    `order`/`implementationName`/`infoLink`, `PlexListSettings.scope`, etc. `[JsonIgnore]` properties are not written.
  * **Null properties are omitted**; but **null values inside `Dictionary<string,string>` are written** as `null`
    (`WhenWritingNull` does not apply to dictionary entries) — e.g. `History.Data` can contain `"imdbId": null`.
  * Embedded-document values that are themselves null: Dapper writes SQL NULL (handlers are not invoked for null) —
    UNVERIFIED for every column (library behaviour, not in repo); be defensive and also treat the literal text `null` as NULL.
  * Some migrations wrote compact JSON (e.g. 175 `Languages` = `[1]`, 225 MediaInfo non-indented). Never rely on formatting.
* Special converters:
  * `QualityIntConverter`: a `Quality` inside JSON is written as its **integer id** (e.g. `"quality": 7`), used for
    `QualityModel` (EpisodeFiles/History/Blocklist `Quality`), `QualityProfiles.Items`, `ParsedEpisodeInfo`.
  * `LanguageIntConverter`: `List<Language>` → JSON array of **integer ids** (e.g. `[1, 2]`); `null` token → Unknown(0).
  * `CustomFormatIntConverter`: `ProfileFormatItem.Format` → integer custom-format id (`{"format": 3, "score": 100}`).
  * `CustomFormatSpecificationListConverter` / `AutoTaggingSpecificationConverter`: polymorphic list written as
    `[{"type": "<C# class name>", "body": {…spec properties…}}]`; reader is **positional** (expects `type` then `body`).
  * `StringListConverter<List<string>>`: JSON array; on read, if the text does not start with `[` and end with `]` it is
    split on `,` (legacy comma-separated values from downgraded installs — ReleaseProfiles Required/Ignored).
  * `ProviderSettingConverter`: `Settings` written as JSON of the runtime settings object; **read is done in
    `ProviderRepository.Query`**: it looks up the CLR type named by the row's `ConfigContract` column and deserializes
    `Settings` into it with the options above; unknown contract or empty body → `NullConfig`.
  * `CommandConverter` (Commands.Body): JSON with a `name` property, resolved to `<name>Command` type.

### 5.4 Encoding/escaping notes for a Rust importer
* serde: use `#[serde(rename_all = "camelCase")]` but **beware STJ's acronym rule** (`audioCodecID`, `videoCodecID`); for
  robust reading deserialize with case-insensitive matching (STJ reads case-insensitively, and Newtonsoft-era rows may differ
  in casing) — e.g. go through `serde_json::Value` and look keys up case-insensitively, or add `alias`es.
* Provide `#[serde(default)]` everywhere; tolerate unknown fields.
* Enum fields in JSON: accept string (camelCase, compare case-insensitively) **or** integer.
* Integer-like strings: some dictionary values are numbers-as-strings (`"size": "1234567"`, `"protocol": "2"`).

---
## 6. Enums and ID tables (exact values)

Top-level DB columns store the **integer**; inside JSON, C#-enum-typed fields are **camelCase names** (int accepted on read);
provider-settings fields declared as `int` store the integer.

### 6.1 Quality (`Core/Qualities/Quality.cs`) — `Quality.Id` is what is stored everywhere
| Id | Name | Source (`QualitySource`) | Resolution | Default weight | Default min / max / preferred (MB/min) | Group |
|---|---|---|---|---|---|---|
| 0 | Unknown | Unknown | 0 | 1 | 1 / 199.9 / 95 | |
| 1 | SDTV | Television | 480 | 2 | 2 / 100 / 95 | |
| 12 | WEBRip-480p | WebRip | 480 | 3 | 2 / 100 / 95 | WEB 480p |
| 8 | WEBDL-480p | Web | 480 | 3 | 2 / 100 / 95 | WEB 480p |
| 2 | DVD | DVD | 480 | 4 | 2 / 100 / 95 | |
| 13 | Bluray-480p | Bluray | 480 | 5 | 2 / 100 / 95 | |
| 22 | Bluray-576p | Bluray | 576 | 6 | 2 / 100 / 95 | |
| 4 | HDTV-720p | Television | 720 | 7 | 3 / 125 / 95 | |
| 9 | HDTV-1080p | Television | 1080 | 8 | 4 / 125 / 95 | |
| 10 | Raw-HD | TelevisionRaw | 1080 | 9 | 4 / null / 95 | |
| 14 | WEBRip-720p | WebRip | 720 | 10 | 3 / 130 / 95 | WEB 720p |
| 5 | WEBDL-720p | Web | 720 | 10 | 3 / 130 / 95 | WEB 720p |
| 6 | Bluray-720p | Bluray | 720 | 11 | 4 / 130 / 95 | |
| 15 | WEBRip-1080p | WebRip | 1080 | 12 | 4 / 130 / 95 | WEB 1080p |
| 3 | WEBDL-1080p | Web | 1080 | 12 | 4 / 130 / 95 | WEB 1080p |
| 7 | Bluray-1080p | Bluray | 1080 | 13 | 4 / 155 / 95 | |
| 20 | Bluray-1080p Remux | BlurayRaw | 1080 | 14 | 35 / null / 95 | |
| 16 | HDTV-2160p | Television | 2160 | 15 | 35 / 199.9 / 95 | |
| 17 | WEBRip-2160p | WebRip | 2160 | 16 | 35 / null / 95 | WEB 2160p |
| 18 | WEBDL-2160p | Web | 2160 | 16 | 35 / null / 95 | WEB 2160p |
| 19 | Bluray-2160p | Bluray | 2160 | 17 | 35 / null / 95 | |
| 21 | Bluray-2160p Remux | BlurayRaw | 2160 | 18 | 35 / null / 95 | |
Id 11 (HDTV-480p) is commented out / unused. Rows are listed in weight order (= `Quality.DefaultQualityDefinitions`);
QualityDefinitions rows VERIFIED identical in the live v4 DB. Group ids in profiles start at 1000.

`QualitySource`: Unknown=0, Television=1, TelevisionRaw=2, Web=3, WebRip=4, DVD=5, Bluray=6, BlurayRaw=7.
`Resolution` (`Core/Parser/QualityParser.cs`): Unknown=0, R360p=360, R480p=480, R540p=540, R576p=576, R720p=720,
R1080p=1080, R2160p=2160. `Revision`: `{version:int (default 1), real:int, isRepack:bool}`.
**There is no `Modifier` enum in Sonarr** (that is Radarr's `QualityModifier`).

### 6.2 Language (`Core/Languages/Language.cs`) — stored as id
0 Unknown, 1 English, 2 French, 3 Spanish, 4 German, 5 Italian, 6 Danish, 7 Dutch, 8 Japanese, 9 Icelandic, 10 Chinese,
11 Russian, 12 Polish, 13 Vietnamese, 14 Swedish, 15 Norwegian, 16 Finnish, 17 Turkish, 18 Portuguese, 19 Flemish,
20 Greek, 21 Korean, 22 Hungarian, 23 Hebrew, 24 Lithuanian, 25 Czech, 26 Arabic, 27 Hindi, 28 Bulgarian, 29 Malayalam,
30 Ukrainian, 31 Slovak, 32 Thai, 33 Portuguese (Brazil), 34 Spanish (Latino), 35 Romanian, 36 Latvian, 37 Persian,
38 Catalan, 39 Croatian, 40 Serbian, 41 Bosnian, 42 Estonian, 43 Tamil, 44 Indonesian, 45 Macedonian, 46 Slovenian,
47 Azerbaijani, 48 Uzbek, 49 Malay, 50 Urdu, 51 Romansh, 52 Georgian, 53 Irish, **−2 Original** (CF LanguageSpecification
only). The list grows over versions — tolerate unknown ids (which ids exist in a given v4 build is UNVERIFIED).

### 6.3 Series / monitoring
* `SeriesTypes` (`Core/Tv/SeriesTypes.cs`): Standard=0, Daily=1, Anime=2.
* `SeriesStatusType` (`Core/Tv/SeriesStatusType.cs`): Deleted=−1, Continuing=0, Ended=1, Upcoming=2.
* `MonitorTypes` (`Core/Tv/MonitoringOptions.cs`): Unknown=0, All=1, Future=2, Missing=3, Existing=4, FirstSeason=5,
  LastSeason=6, LatestSeason=7 (obsolete), Pilot=8, Recent=9, MonitorSpecials=10, UnmonitorSpecials=11, None=12, Skip=13.
  Stored as int in `ImportLists.ShouldMonitor`; as camelCase string in `Series.AddOptions.monitor`: `unknown, all, future,
  missing, existing, firstSeason, lastSeason, latestSeason, pilot, recent, monitorSpecials, unmonitorSpecials, none, skip`.
* `NewItemMonitorTypes`: All=0, None=1 (`Series.MonitorNewItems`, `ImportLists.MonitorNewItems`).
* `MonitoringOptions` (embedded): `{ignoreEpisodesWithFiles: bool, ignoreEpisodesWithoutFiles: bool, monitor: MonitorTypes}`.
* `TagExistingType` (v5): None=0, Add=1, Sync=2. `ImportListType` (computed, not stored): Program, Plex, Trakt, Simkl, Tmdb, Other, Advanced.

### 6.4 History / downloads
* `EpisodeHistoryEventType` (`Core/History/EpisodeHistory.cs`): Unknown=0, Grabbed=1, SeriesFolderImported=2,
  DownloadFolderImported=3, DownloadFailed=4, EpisodeFileDeleted=5, EpisodeFileRenamed=6, DownloadIgnored=7.
* `DownloadHistoryEventType`: DownloadGrabbed=1, DownloadImported=2, DownloadFailed=3, DownloadIgnored=4, FileImported=5.
* `DownloadProtocol` (`Core/Indexers/DownloadProtocol.cs`): Unknown=0, Usenet=1, Torrent=2.
* `IndexerFlags` ([Flags], `Core/Parser/Model/IndexerFlags.cs`): Freeleech=1, Halfleech=2, DoubleUpload=4, Internal=8,
  Scene=16, Freeleech75=32, Freeleech25=64, Nuked=128, Subtitles=256.
* `ReleaseType`: Unknown=0, SingleEpisode=1, MultiEpisode=2, SeasonPack=3, MultiSeasonPack=4.
* `PendingReleaseReason`: Delay=0, DownloadClientUnavailable=1, Fallback=2.
* `SeriesMatchType`: Unknown=0, Title=1, Alias=2, Id=3. `ReleaseSourceType`: Unknown=0, Rss=1, Search=2,
  UserInvokedSearch=3, InteractiveSearch=4, ReleasePush=5.
* `DeleteMediaFileReason` (History `reason` string): MissingFromDisk, Manual, Upgrade, NoLinkedEpisodes, ManualOverride.

### 6.5 Config-table enums (stored as **names**, see §4.14)
* `ProperDownloadTypes` (`Core/Qualities/ProperDownloadTypes.cs`): PreferAndUpgrade=0, DoNotUpgrade=1, DoNotPrefer=2.
* `FileDateType`: None=0, LocalAirDate=1, UtcAirDate=2.
* `RescanAfterRefreshType`: Always=0, AfterManual=1, Never=2.
* `EpisodeTitleRequiredType`: Always=0, BulkSeasonReleases=1, Never=2.
* `SeasonPackUpgradeType` (v5): All=0, Threshold=1, Any=2.
* `ListSyncLevelType`: Disabled=0, LogOnly=1, KeepAndUnmonitor=2, KeepAndTag=3.
* `ProxyType`: Http=0, Socks4=1, Socks5=2. `CertificateValidationType`: Enabled=0, DisabledForLocalAddresses=1, Disabled=2.

### 6.6 Naming (`Core/Organizer/FileNameBuilder.cs`)
* `MultiEpisodeStyle`: Extend=0, Duplicate=1, Repeat=2, Scene=3, Range=4, PrefixedRange=5.
* `ColonReplacementFormat`: Delete=0, Dash=1, SpaceDash=2, SpaceDashSpace=3, Smart=4, Custom=5.

### 6.7 Misc stored enums
* `DelayProfiles.PreferredProtocol` = DownloadProtocol. `SceneMappings.SearchMode`: Default=0, SearchID=1, SearchTitle=2.
* `MetadataFiles.Type` (`MetadataType`): Unknown=0, SeriesMetadata=1, EpisodeMetadata=2, SeriesImage=3, SeasonImage=4, EpisodeImage=5.
* `MediaCoverTypes` (JSON camelCase): Unknown=0, Poster=1, Banner=2, Fanart=3, Screenshot=4, Headshot=5, Clearlogo=6.
* `HdrFormat` (JSON camelCase): None, UnknownHdr, Pq10, Hdr10, Hdr10Plus, Hlg10, DolbyVision, DolbyVisionHdr10,
  DolbyVisionSdr, DolbyVisionHlg, DolbyVisionHdr10Plus (0..10).
* `Commands`: `CommandStatus` Queued=0 … Orphaned=6, `CommandTrigger` Unspecified=0, Manual=1, Scheduled=2,
  `CommandResult` Unknown=0, Successful=1, Unsuccessful=2, Indeterminate=3, `CommandPriority` Low=−1, Normal=0, High=1.
* `ProviderMessage` / `ProviderMessageType` (Info, Warning, Error): **not stored** (no column, no type handler).
* `AuthenticationType` / `AuthenticationRequiredType` / `UpdateMechanism` / `BackupType`: config.xml / runtime only (§1, §2).

---
## 7. Provider implementations and their `Settings` JSON

Rules (all tables): `Implementation` = provider C# class name; `ConfigContract` = settings C# class name; `Settings` JSON
uses §5.3 conventions (camelCase keys, null keys omitted, indented, read-only computed keys like `isValid` may appear,
enum-like fields mostly plain ints). Default values below are from the settings class constructors / initializers
(C# defaults otherwise: `false`, `0`, null/omitted). Fields were extracted mechanically from this v5 tree
(script: `a local audit script providers.py`, raw dump `a local audit script settings_dump.txt`) and
spot-checked by hand. **Older backups may lack newer fields and may contain obsolete ones → `#[serde(default)]`, ignore
unknown.** Rows whose `Implementation` no longer exists (e.g. `KometaMetadata` — VERIFIED present in v4.0.20 DB; `Rarbg`,
`Boxcar` — UNVERIFIED, removed from code without a delete migration) must be skipped gracefully.
"privacy" = Sonarr marks the field as secret (ApiKey/Password/UserName) — expect real secrets in plaintext in the backup.

### 7.1 Indexers (`Core/Indexers/*`)
| Implementation | ConfigContract | Display name | Protocol |
|---|---|---|---|
| `Newznab` | `NewznabSettings` | Newznab | usenet |
| `Torznab` | `TorznabSettings` | Torznab | torrent |
| `BroadcastheNet` | `BroadcastheNetSettings` | BroadcasTheNet | torrent |
| `Fanzub` | `FanzubSettings` | Fanzub | usenet |
| `FileList` | `FileListSettings` | FileList | torrent |
| `HDBits` | `HDBitsSettings` | HDBits | torrent |
| `IPTorrents` | `IPTorrentsSettings` | IP Torrents | torrent |
| `Nyaa` | `NyaaSettings` | Nyaa | torrent |
| `TorrentRssIndexer` | `TorrentRssIndexerSettings` | Torrent RSS Feed | torrent |
| `Torrentleech` | `TorrentleechSettings` | TorrentLeech | torrent |
(Indexers added by Prowlarr sync are ordinary `Newznab`/`Torznab` rows; Name typically ends in ` (Prowlarr)`.)

**NewznabSettings** (`Core/Indexers/Newznab/NewznabSettings.cs`):
| JSON key | Type | Default | Meaning |
|---|---|---|---|
| `baseUrl` | string | — | indexer URL (e.g. `https://api.nzbgeek.info`) |
| `apiPath` | string | `/api` | |
| `apiKey` | string | — | privacy ApiKey |
| `categories` | int[] | `[5030, 5040]` | Newznab TV categories |
| `animeCategories` | int[] | `[]` | |
| `animeStandardFormatSearch` | bool | false | search anime with SxxEyy |
| `additionalParameters` | string | — | `&key=value…` |
| `multiLanguages` | int[] | `[]` | Language ids treated as MULTi |
| `failDownloads` | int[] | `[]` | `FailDownloads`: 0 Executables, 1 PotentiallyDangerous, 2 UserDefinedExtensions |

**TorznabSettings** = all Newznab keys + `minimumSeeders` (int, 1), `seedCriteria` (object, below),
`rejectBlocklistedTorrentHashesWhileGrabbing` (bool), `rejectTorrentFilesWithBlockedExtensionsWhileGrabbing` (bool).

**SeedCriteriaSettings** (`Core/Indexers/SeedCriteriaSettings.cs`, nested as `seedCriteria` in every torrent indexer):
`seedRatio` (double?), `seedTime` (int? minutes), `seasonPackSeedGoal` (int: 0 UseStandardSeedGoal, 1 UseSeasonPackSeedGoal),
`seasonPackSeedRatio` (double?), `seasonPackSeedTime` (int? minutes). Nulls omitted.

Example (Torznab via Prowlarr):
```json
{
  "minimumSeeders": 1,
  "seedCriteria": { "seedRatio": 1.0, "seedTime": 1440, "seasonPackSeedGoal": 0 },
  "rejectBlocklistedTorrentHashesWhileGrabbing": false,
  "rejectTorrentFilesWithBlockedExtensionsWhileGrabbing": false,
  "baseUrl": "http://prowlarr:9696/3/",
  "apiPath": "/api",
  "apiKey": "0123456789abcdef0123456789abcdef",
  "categories": [5000, 5030, 5040],
  "animeCategories": [5070],
  "animeStandardFormatSearch": false,
  "multiLanguages": [],
  "failDownloads": []
}
```
Other indexers (keys; defaults):
* `BroadcastheNetSettings`: `baseUrl` (`https://api.broadcasthe.net/`), `apiKey`, `minimumSeeders` (1), `seedCriteria`,
  `rejectBlocklistedTorrentHashesWhileGrabbing`, `rejectTorrentFilesWithBlockedExtensionsWhileGrabbing`, `multiLanguages`, `failDownloads`.
* `FanzubSettings`: `baseUrl` (`http://fanzub.com/rss/`), `animeStandardFormatSearch`, `multiLanguages`, `failDownloads`.
* `FileListSettings`: `username`, `passkey`, `baseUrl` (`https://filelist.io`), `categories` (`[23, 21, 27]` = TV_SD, TV_HD, TV_4K;
  enum FileListCategories Anime=24, Animation=15, TV_4K=27, TV_HD=21, TV_SD=23, Sport=13, RoDubbed=28), `animeCategories`,
  `minimumSeeders`, `seedCriteria`, the two reject flags, `multiLanguages`, `failDownloads`.
* `HDBitsSettings`: `baseUrl` (`https://hdbits.org`), `username`, `apiKey`, `categories` (`[2, 3]` Tv, Documentary),
  `codecs` (`[]`; H264=1, Mpeg2=2, Vc1=3, Xvid=4, Hevc=5), `mediums` (`[]`; Bluray=1, Encode=3, Capture=4, Remux=5, WebDl=6),
  `minimumSeeders`, `seedCriteria`, reject flags, `multiLanguages`, `failDownloads`.
* `IPTorrentsSettings`: `baseUrl` (RSS feed URL), `minimumSeeders`, `seedCriteria`, reject flags, `multiLanguages`, `failDownloads`.
* `NyaaSettings`: `baseUrl` (`""`), `animeStandardFormatSearch`, `additionalParameters` (`&cats=1_0&filter=1`), `minimumSeeders`,
  `seedCriteria`, reject flags, `multiLanguages`, `failDownloads`.
* `TorrentRssIndexerSettings`: `baseUrl` (`""`), `cookie`, `allowZeroSize` (false), `minimumSeeders`, `seedCriteria`, reject
  flags, `multiLanguages`, `failDownloads`.
* `TorrentleechSettings`: `baseUrl` (`http://rss.torrentleech.org`), `apiKey`, `minimumSeeders`, `seedCriteria`, reject flags,
  `multiLanguages`, `failDownloads`.

### 7.2 DownloadClients (`Core/Download/Clients/*`)
| Implementation | ConfigContract | Display name | Protocol |
|---|---|---|---|
| `QBittorrent` | `QBittorrentSettings` | qBittorrent | torrent |
| `Deluge` | `DelugeSettings` | Deluge | torrent |
| `Transmission` | `TransmissionSettings` | Transmission | torrent |
| `Vuze` | `TransmissionSettings` | Vuze | torrent |
| `RTorrent` | `RTorrentSettings` | rTorrent | torrent |
| `UTorrent` | `UTorrentSettings` | uTorrent | torrent |
| `Flood` | `FloodSettings` | Flood | torrent |
| `Aria2` | `Aria2Settings` | Aria2 | torrent |
| `Hadouken` | `HadoukenSettings` | Hadouken | torrent |
| `RQBit` | `RQbitSettings` (note lowercase b) | RQBit | torrent |
| `TriblerDownloadClient` | `TriblerDownloadSettings` | Tribler | torrent |
| `TorrentDownloadStation` | `DownloadStationSettings` | Download Station | torrent |
| `TorrentFreeboxDownload` | `FreeboxDownloadSettings` | Freebox Download | torrent |
| `TorrentBlackhole` | `TorrentBlackholeSettings` | Torrent Blackhole | torrent |
| `Sabnzbd` | `SabnzbdSettings` | SABnzbd | usenet |
| `Nzbget` | `NzbgetSettings` | NZBGet | usenet |
| `NzbVortex` | `NzbVortexSettings` | NZBVortex | usenet |
| `UsenetDownloadStation` | `DownloadStationSettings` | Download Station | usenet |
| `UsenetBlackhole` | `UsenetBlackholeSettings` | Usenet Blackhole | usenet |
| `Pneumatic` | `PneumaticSettings` | Pneumatic | usenet |

**QBittorrentSettings**: `host` (`localhost`), `port` (8080), `useSsl` (false), `urlBase`, `apiKey` (v5 tree; privacy),
`username`, `password`, `tvCategory` (`tv-sonarr`), `tvImportedCategory`, `recentTvPriority` / `olderTvPriority`
(int `QBittorrentPriority`: 0 Last, 1 First), `initialState` (int `QBittorrentState`: 0 Start, 1 ForceStart, 2 Stop),
`sequentialOrder` (bool), `firstAndLast` (bool), `contentLayout` (int: 0 Default, 1 Original, 2 Subfolder),
`addSeriesTags` (bool), `seedTimeType` (int `QBittorrentSeedTimeType`: 0 Total, 1 Inactive).
```json
{
  "host": "qbittorrent", "port": 8080, "useSsl": false, "urlBase": "",
  "username": "admin", "password": "adminadmin",
  "tvCategory": "tv-sonarr", "tvImportedCategory": "",
  "recentTvPriority": 0, "olderTvPriority": 0, "initialState": 0,
  "sequentialOrder": false, "firstAndLast": false, "contentLayout": 0
}
```
**DelugeSettings**: `host` (`localhost`), `port` (8112), `useSsl`, `urlBase`, `password` (`deluge`), `tvCategory` (`tv-sonarr`),
`tvImportedCategory`, `recentTvPriority`/`olderTvPriority` (0 Last, 1 First), `addPaused`, `downloadDirectory`, `completedDirectory`.
**TransmissionSettings** (Transmission & Vuze): `host` (`localhost`), `port` (9091), `useSsl`, `urlBase` (`/transmission/`),
`username`, `password`, `tvCategory` (`tv-sonarr`), `tvImportedCategory`, `tvDirectory`, `recentTvPriority`/`olderTvPriority`
(0 Last, 1 First), `addPaused`.
**RTorrentSettings**: `host` (`localhost`), `port` (8080), `useSsl`, `urlBase` (`RPC2`), `username`, `password`, `tvCategory`
(`tv-sonarr`), `tvImportedCategory`, `tvDirectory`, `recentTvPriority`/`olderTvPriority` (int `RTorrentPriority`: 0 VeryLow,
1 Low, 2 Normal (default), 3 High), `addStopped`.
**UTorrentSettings**: `host`, `port` (8080), `useSsl`, `urlBase`, `username`, `password`, `tvCategory` (`tv-sonarr`),
`tvImportedCategory`, `recentTvPriority`/`olderTvPriority` (0 Last, 1 First), **`intialState`** (sic, typo is the real key;
`UTorrentState` 0 Start, 1 ForceStart, 2 Pause, 3 Stop).
**FloodSettings**: `host` (`localhost`), `port` (3000), `useSsl`, `urlBase`, `username`, `password`, `destination`,
`tags` (string[], `["sonarr"]`), `postImportTags` (string[]), `additionalTags` (int[] `AdditionalTags`: 0 TitleSlug,
1 Quality, 2 Languages, 3 ReleaseGroup, 4 Year, 5 Indexer, 6 Network), `startOnAdd` (true).
**Aria2Settings**: `host` (`localhost`), `port` (6800), `rpcPath` (`/rpc`), `useSsl`, `secretToken` (`MySecretToken`), `directory`.
**HadoukenSettings**: `host`, `port` (7070), `useSsl`, `urlBase`, `username`, `password`, `category` (`sonarr-tv`).
**RQbitSettings**: `host`, `port` (3030), `useSsl`, `urlBase` (`/`).
**TriblerDownloadSettings**: `host`, `port` (20100), `useSsl`, `urlBase` (`""`), `apiKey`, `tvCategory`, `tvDirectory`,
`anonymityLevel` (1), `safeSeeding` (true).
**DownloadStationSettings**: `host`, `port`, `useSsl`, `username`, `password`, `tvCategory`, `tvDirectory`.
**FreeboxDownloadSettings**: `host` (`mafreebox.freebox.fr`), `port` (443), `useSsl` (true), `apiUrl` (`/api/v1/`), `appId`,
`appToken`, `destinationDirectory`, `category`, `recentPriority`/`olderPriority` (0 Last, 1 First), `addPaused`.
**TorrentBlackholeSettings**: `torrentFolder`, `watchFolder`, `saveMagnetFiles` (false), `magnetFileExtension` (`.magnet`), `readOnly` (true).
**SabnzbdSettings**: `host` (`localhost`), `port` (8080), `useSsl`, `urlBase`, `apiKey`, `username`, `password`,
`tvCategory` (`tv`), `recentTvPriority`/`olderTvPriority` (int `SabnzbdPriority`: −100 Default (default), −2 Paused,
−1 Low, 0 Normal, 1 High, 2 Force).
```json
{ "host": "sabnzbd", "port": 8080, "useSsl": false, "urlBase": "", "apiKey": "abcdef0123456789abcdef0123456789",
  "tvCategory": "tv", "recentTvPriority": -100, "olderTvPriority": -100 }
```
**NzbgetSettings**: `host` (`localhost`), `port` (6789), `useSsl`, `urlBase`, `username`, `password`, `tvCategory` (`tv`),
`recentTvPriority`/`olderTvPriority` (int `NzbgetPriority`: −100 VeryLow, −50 Low, 0 Normal (default), 50 High, 100 VeryHigh,
900 Force), `addPaused`.
**NzbVortexSettings**: `host` (`localhost`), `port` (4321), `urlBase`, `apiKey`, `tvCategory` (`TV Shows`),
`recentTvPriority`/`olderTvPriority` (−1 Low, 0 Normal, 1 High).
**UsenetBlackholeSettings**: `nzbFolder`, `watchFolder`. **PneumaticSettings**: `nzbFolder`, `strmFolder`.

### 7.3 Notifications (`Core/Notifications/*`) — "Connections"
| Implementation | ConfigContract | Display name |
|---|---|---|
| `Webhook` | `WebhookSettings` | Webhook |
| `Discord` | `DiscordSettings` | Discord |
| `Telegram` | `TelegramSettings` | Telegram |
| `Email` | `EmailSettings` | Email |
| `Pushover` | `PushoverSettings` | Pushover |
| `PlexServer` | `PlexServerSettings` | Plex Media Server |
| `MediaBrowser` | `MediaBrowserSettings` | Emby / Jellyfin |
| `Xbmc` | `XbmcSettings` | Kodi |
| `CustomScript` | `CustomScriptSettings` | Custom Script |
| `Gotify` | `GotifySettings` | Gotify |
| `Ntfy` | `NtfySettings` | ntfy.sh |
| `Apprise` | `AppriseSettings` | Apprise |
| `Notifiarr` | `NotifiarrSettings` | Notifiarr |
| `Slack` | `SlackSettings` | Slack |
| `Signal` | `SignalSettings` | Signal |
| `Join` | `JoinSettings` | Join |
| `MailGun` (capital G) | `MailgunSettings` | Mailgun |
| `SendGrid` | `SendGridSettings` | SendGrid |
| `Prowl` | `ProwlSettings` | Prowl |
| `PushBullet` | `PushBulletSettings` | Pushbullet |
| `Pushcut` | `PushcutSettings` | Pushcut |
| `Simplepush` | `SimplepushSettings` | Simplepush |
| `SynologyIndexer` | `SynologyIndexerSettings` | Synology Indexer |
| `Trakt` | `TraktSettings` | Trakt |
| `Twitter` | `TwitterSettings` | Twitter |
Removed by migrations (never in modern DBs): `NotifyMyAndroid`, `Pushalot` (125), `Growl` (145), `PlexHomeTheater`,
`PlexClient` (161).

* **WebhookSettings**: `url`, `method` (int `WebhookMethod`: 1 POST (default), 2 PUT), `username`, `password`,
  `headers` (array of `{key, value}` objects — `IEnumerable<KeyValuePair<string,string>>`; STJ (.NET 5+) applies the camelCase
  naming policy to KeyValuePair members → `{"key":"…","value":"…"}`; accept `Key`/`Value` too).
  ```json
  { "url": "https://example.org/hook", "method": 1, "username": "", "password": "", "headers": [ { "key": "X-Token", "value": "abc" } ] }
  ```
* **DiscordSettings**: `webHookUrl`, `username`, `avatar`, `author`, `grabFields` (int[], default all 0..12 of
  DiscordGrabFieldType: Overview, Rating, Genres, Quality, Group, Size, Links, Release, Poster, Fanart, Indexer,
  CustomFormats, CustomFormatScore), `importFields` (int[], default 0..12 of DiscordImportFieldType: Overview=0, Rating,
  Genres, Quality, Codecs, Group, Size, Languages, Subtitles, Links, Release, Poster, Fanart=12; CustomFormats=13,
  CustomFormatScore=14 exist but not default), `manualInteractionFields` (int[], default 0..9: Overview, Rating, Genres,
  Quality, Group, Size, Links, DownloadTitle, Poster, Fanart).
* **TelegramSettings**: `botToken`, `chatId`, `topicId` (int?), `sendSilently`, `includeAppNameInTitle`,
  `includeInstanceNameInTitle`, `metadataLinks` (int[] `MetadataLinkType`: 0 Imdb, 1 Tvdb, 2 Tvmaze, 3 Trakt),
  `linkPreview` (**v5+ 218**, int `MetadataLinkPreviewType`: −1 None (default), 0 Imdb, 2 Tvmaze, 3 Trakt).
* **EmailSettings**: `server`, `port` (587), `useEncryption` (int `EmailEncryptionType`: 0 Preferred, 1 Always, 2 Never;
  replaced bool `requireEncryption` in 201), `username`, `password`, `from`, `to` (string[]; was a comma string before 157),
  `cc` (string[]), `bcc` (string[]).
* **PushoverSettings**: `apiKey`, `userKey`, `devices` (string[]), `priority` (int: −2 Silent, −1 Quiet, 0 Normal, 1 High,
  2 Emergency), `retry` (int s), `expire` (int s), `ttl` (int), `sound`, `encryptionKey`, + computed `isValid`.
* **PlexServerSettings**: `host` (`""`), `port` (32400), `useSsl`, `urlBase`, `authToken`, `signIn` (`startOAuth`, UI only),
  `updateLibrary` (true), `mapFrom`, `mapTo`, + computed `isValid`. (`server` is `[JsonIgnore]`.)
* **MediaBrowserSettings** (Emby/Jellyfin): `host`, `port` (8096), `useSsl`, `urlBase`, `apiKey`, `notify` (bool),
  `updateLibrary` (true), `mapFrom`, `mapTo`, + computed `isValid` (`address` is `[JsonIgnore]`).
* **XbmcSettings** (Kodi): `host`, `port` (8080), `useSsl`, `urlBase` (`/jsonrpc`), `username`, `password`, `displayTime` (5),
  `notify`, `updateLibrary`, `cleanLibrary`, `alwaysUpdate` (`address` `[JsonIgnore]`).
* **CustomScriptSettings**: `path` (script path), `arguments` (string; hidden-if-not-set legacy field).
* **GotifySettings**: `server`, `appToken`, `priority` (int: 0 Min, 2 Low, 5 Normal (default), 8 High), `includeSeriesPoster`,
  `metadataLinks` (int[]), `preferredMetadataLink` (int, default 1 Tvdb).
* **NtfySettings**: `serverUrl`, `accessToken`, `userName` (note capital N), `password`, `priority` (int 1 Min … 3 Default
  … 5 Max), `topics` (string[]), `tags` (string[]), `clickUrl`, `headers` (`[{key, value}]`; v5 tree, v4 UNVERIFIED).
* **AppriseSettings**: `serverUrl`, `configurationKey`, `statelessUrls`, `notificationType` (int 0 Info, 1 Success,
  2 Warning, 3 Failure), `tags` (string[]), `includePoster`, `authUsername`, `authPassword`.
* **NotifiarrSettings**: `apiKey`.
* **SlackSettings**: `webHookUrl`, `username`, `icon`, `channel`.
* **SignalSettings**: `host`, `port`, `useSsl`, `senderNumber`, `receiverId`, `authUsername`, `authPassword`.
* **JoinSettings**: `apiKey`, `deviceIds` (legacy), `deviceNames`, `priority` (int −2..2, default 0).
* **MailgunSettings**: `apiKey`, `useEuEndpoint`, `from`, `senderDomain`, `recipients` (string[]).
* **SendGridSettings**: `baseUrl` (`https://api.sendgrid.com/v3/`), `apiKey`, `from`, `recipients` (string[]).
* **ProwlSettings**: `apiKey`, `priority` (int −2 VeryLow … 2 Emergency), + `isValid`.
* **PushBulletSettings**: `apiKey` (access token), `deviceIds` (string[]), `channelTags` (string[]), `senderId`.
* **PushcutSettings**: `notificationName`, `apiKey`, `timeSensitive`, `includePoster`, `metadataLinks` (int[]).
* **SimplepushSettings**: `key`, `event`, + `isValid`.
* **SynologyIndexerSettings**: `updateLibrary` (true).
* **TraktSettings** (notification): `accessToken`, `refreshToken`, `expires` (DateTime → `"yyyy-MM-ddTHH:mm:ssZ"`), `authUser`,
  `signIn`.
* **TwitterSettings**: `consumerKey`, `consumerSecret`, `accessToken`, `accessTokenSecret`, `mention`, `directMessage` (true),
  `authorizeNotification`.

### 7.4 ImportLists (`Core/ImportLists/*`)
| Implementation | ConfigContract | Display name | Key settings (JSON) |
|---|---|---|---|
| `SonarrImport` | `SonarrSettings` | Sonarr | `baseUrl`, `apiKey`, `syncSeasonMonitoring`, `profileIds` int[], `tagIds` int[], `rootFolderPaths` string[], `languageProfileIds` int[] (legacy) |
| `PlexImport` | `PlexListSettings` | Plex Watchlist | `accessToken`, `signIn`, computed `baseUrl`/`scope` |
| `PlexRssImport` | `PlexRssImportSettings` | Plex Watchlist RSS | `url` (+`baseUrl`) |
| `TraktListImport` | `TraktListSettings` | Trakt List | Trakt base keys + `username`, `listname`, `years` |
| `TraktUserImport` | `TraktUserSettings` | Trakt User | base + `traktListType` (0 UserWatchList, 1 UserWatchedList, 2 UserCollectionList), `traktWatchedListType` (0 All, 1 InProgress, 2 CompletelyWatched), `traktWatchSorting` (0 Rank, 1 Added, 2 Title, 3 Released), `username`, `years` |
| `TraktPopularImport` | `TraktPopularSettings` | Trakt Popular List | base + `traktListType` (0 Trending, 1 Popular (default), 2 Anticipated, 3..6 TopWatchedBy Week/Month/Year/AllTime, 7..10 RecommendedBy Week/Month/Year/AllTime), `years` |
| `TraktSmartListImport` | `TraktSmartListSettings` | Trakt Smart List | base + `smartListId` |
| `SimklUserImport` | `SimklUserSettings` | Simkl User Watchlist | `baseUrl` (`https://api.simkl.com`), `accessToken`, `refreshToken`, `expires`, `authUser`, `signIn`, `showType` (0 Shows, 1 Anime), `listType` (0 Watching, 1 PlanToWatch, 2 Hold, 3 Completed, 4 Dropped) |
| `MyAnimeListImport` | `MyAnimeListSettings` | MyAnimeList | `baseUrl`, `listStatus` (0 All, 1 Watching, 2 Completed, 3 OnHold, 4 Dropped, 5 PlanToWatch), `minimumScore`, `accessToken`, `refreshToken`, `expires`, `signIn` |
| `AniListImport` (C# `internal`) | `AniListSettings` | AniList List | `baseUrl` (`https://graphql.anilist.co`), `accessToken`, `refreshToken`, `expires`, `signIn`, `username`, `importCurrent` (true), `importPlanning` (true), `importCompleted`, `importDropped`, `importPaused`, `importRepeating`, `importFinished` (true), `importReleasing` (true), `importUnreleased`, `importCancelled`, `importHiatus` |
| `CustomImport` | `CustomSettings` | Custom List | `baseUrl` (URL of JSON `[{tvdbId, …}]`) |
| `TmdbAccountImport` | `TmdbAccountSettings` | TMDb Account | TMDb base (`baseUrl`, `accountId`, `authToken`, `signIn`) + `accountListType` (0 Favorite, 1 Rated, 2 Recommended, 3 Watchlist) — **v5 tree; not in v4 (UNVERIFIED)** |
| `TmdbDiscoverImport` | `TmdbDiscoverSettings` | TMDb Discover | base + `originalLanguage`, `sortByType`, `voteAverageMinimum`, `voteCountMinimum`, `airDateMinimumDays`, `airDateMaximumDays`, `withGenreTypes` int[], `withKeywords`, `withCompanies`, `withNetworks`, `includeNullFirstAirDates` (v5) |
| `TmdbListImport` | `TmdbListSettings` | TMDb List | base + `accountListId`, `listId` (v5) |
| `TmdbPersonImport` | `TmdbPersonSettings` | TMDb Person | base + `personId`, `includingCastCredits` (true), `includeDepartmentTypes` int[] (v5) |
Trakt base keys (`TraktSettingsBase`): `baseUrl` (`https://api.trakt.tv`), `accessToken`, `refreshToken`, `expires` (DateTime),
`authUser`, `rating`, `genres`, `traktAdditionalParameters`, `limit` (100), `signIn`.
OAuth tokens in import lists are tied to Sonarr's OAuth app (via Sonarr cloud); they will not work in another app — import
the list *config* but expect re-authentication.

### 7.5 Metadata (consumers) (`Core/Extras/Metadata/Consumers/*`)
| Implementation | ConfigContract | Display | Settings |
|---|---|---|---|
| `XbmcMetadata` | `XbmcMetadataSettings` | Kodi (XBMC) / Emby | `seriesMetadata` (true), `seriesMetadataEpisodeGuide` (false), `seriesMetadataUrl` (false), `episodeMetadata` (true), `episodeImageThumb` (false), `seriesImages` (true), `seasonImages` (true), `episodeImages` (true), `isValid` |
| `RoksboxMetadata` | `RoksboxMetadataSettings` | Roksbox | `episodeMetadata`, `seriesImages`, `seasonImages`, `episodeImages` (all true), `isValid` |
| `WdtvMetadata` | `WdtvMetadataSettings` | WDTV | same four flags + `isValid` |
| `PlexMetadata` | `PlexMetadataSettings` | Plex | `seriesPlexMatchFile` (true), `episodeMappings` (false) |
| `KometaMetadata` | `KometaMetadataSettings` | Kometa | `{"deprecated": true, "isValid": true}` — **v4 only** (VERIFIED in live DB; absent from v5 tree) |

---

## 8. Postgres

* Sonarr switches to Postgres when config.xml (or env `Sonarr__Postgres__*`) has `PostgresHost` or
  `PostgresMainDbConnectionString`/`PostgresLogDbConnectionString` (`Core/Datastore/ConnectionStringFactory.cs`).
  Main DB default name `sonarr-main`, log DB `sonarr-log`.
* **Backups never contain Postgres data**: `BackupService.BackupDatabase()` only runs for `DatabaseType.SQLite`; a Postgres
  user's backup zip contains only `config.xml` + `INFO`. Restore only accepts SQLite files (header check). The importer
  should detect "no DB in zip" and tell the user (Postgres users must export some other way, e.g. a `pg_dump`, or
  migrate to SQLite first).
* Schema is the same logical schema (same migrations, with `IfDatabase(...)` branches): date columns become
  `timestamp with time zone` (188), booleans are real `boolean`, JSON columns are `text` (v5 233 uses `::jsonb` only in
  expression indexes). Identifiers are quoted PascalCase in both engines (all SQL in the code quotes identifiers).
* If Kitsunarr ever reads a `pg_dump`, the same JSON/enum conventions (§5) apply, since serialization happens in C#.

---
## 9. Importer cookbook (derived from the above; not Sonarr code)

### 9.1 Opening the backup
1. Unzip (SharpZipLib 1.4.2 standard zip, deflate; Zip64 possible for very large DBs) into a fresh temp dir. Locate
   `sonarr.db` or `nzbdrone.db` and `config.xml` case-insensitively at the root; read `INFO` if present.
2. Verify the 16-byte header `SQLite format 3\0`; open **read-only** (`?mode=ro&immutable=1` is safe because backups are
   rollback-journal single files). Use an SQLite with JSON1 (v5 DBs contain `json_extract` expression indexes).
3. `SELECT MAX("Version") FROM "VersionInfo"` → schema version (≤ 217 ⇒ v4-or-older; ≥ 218 or rows 207/208 present ⇒ v5).
   Then `SELECT name FROM pragma_table_info('<Table>')` per table and build column lists dynamically.
4. No `sonarr.db` in the zip ⇒ the instance used Postgres (or the zip is config-only) → abort with a clear message.

### 9.2 Version-sensitive column checks (minimal set)
| Need | Present when |
|---|---|
| `Blocklist` vs `Blacklist` table | `Blocklist` ≥ 160 |
| `QualityProfiles` vs `Profiles`, `Series.QualityProfileId` vs `ProfileId` | ≥ 128 |
| `ReleaseProfiles` vs `Restrictions` | ≥ 127 |
| `CustomFormats`, `QualityProfiles.FormatItems` | ≥ 171 |
| `EpisodeFiles/History/Blocklist.Languages` (JSON) vs `.Language` (int) | ≥ 175 |
| `LanguageProfiles` table, `Series.LanguageProfileId` | < 175 |
| `AutoTagging` | ≥ 179 |
| `QualityDefinitions.MinSize/MaxSize/PreferredSize` | v4 (dropped by v5 207); v5 sizes in `QualityProfiles.Items[*].minSize/...` |
| `ReleaseProfiles.IndexerId` (int) vs `IndexerIds` (JSON) | IndexerId ≤ 223; IndexerIds ≥ 224 |
| `ImportLists.TagExisting` | v5 ≥ 231 (bool) / ≥ 236 (int enum) |
| `Series.MonitorNewItems` / `TmdbId` / `MalIds`,`AniListIds` | ≥ 200 / ≥ 206 / ≥ 217 |
| `EpisodeFiles.MediaInfo` stream arrays | ≥ 225 (else scalar audio keys + `audioLanguages`/`subtitles`) |

### 9.3 Core queries (v4 column names; add v5 columns when present)
```sql
SELECT "Id","Path" FROM "RootFolders";
SELECT "Id","Label" FROM "Tags";
SELECT "Id","Name","Cutoff","Items","UpgradeAllowed","FormatItems","MinFormatScore","CutoffFormatScore","MinUpgradeFormatScore"
  FROM "QualityProfiles";
SELECT "Quality","Title","MinSize","MaxSize","PreferredSize" FROM "QualityDefinitions";        -- v4 only for sizes
SELECT "Id","Name","Specifications","IncludeCustomFormatWhenRenaming" FROM "CustomFormats";
SELECT "Id","EnableUsenet","EnableTorrent","PreferredProtocol","UsenetDelay","TorrentDelay","Order","Tags",
       "BypassIfHighestQuality","BypassIfAboveCustomFormatScore","MinimumCustomFormatScore" FROM "DelayProfiles";
SELECT * FROM "ReleaseProfiles";                                                                -- column set varies
SELECT "Id","TvdbId","TvMazeId","TmdbId","ImdbId","Title","SortTitle","TitleSlug","Status","Overview","AirTime",
       "Monitored","MonitorNewItems","QualityProfileId","SeasonFolder","Runtime","Images","SeriesType","Network",
       "UseSceneNumbering","Path","Year","Seasons","Ratings","Genres","Certification","Tags","Added","FirstAired",
       "LastAired","OriginalLanguage","MalIds","AniListIds","LastInfoSync"
  FROM "Series";
SELECT "Id","SeriesId","TvdbId","SeasonNumber","EpisodeNumber","AbsoluteEpisodeNumber","SceneAbsoluteEpisodeNumber",
       "SceneSeasonNumber","SceneEpisodeNumber","UnverifiedSceneNumbering","Title","Overview","AirDate","AirDateUtc",
       "Monitored","EpisodeFileId","Runtime","FinaleType","Images","Ratings","LastSearchTime"
  FROM "Episodes";
SELECT "Id","SeriesId","SeasonNumber","RelativePath","Size","DateAdded","OriginalFilePath","SceneName","ReleaseGroup",
       "ReleaseHash","Quality","Languages","MediaInfo","IndexerFlags","ReleaseType"
  FROM "EpisodeFiles";
SELECT "Id","EpisodeId","SeriesId","SourceTitle","Date","Quality","Languages","Data","EventType","DownloadId"
  FROM "History" ORDER BY "Date";
SELECT * FROM "Blocklist";
SELECT "Id","Name","Implementation","ConfigContract","Settings","EnableRss","EnableAutomaticSearch",
       "EnableInteractiveSearch","Priority","Tags","DownloadClientId","SeasonSearchMaximumSingleEpisodeAge" FROM "Indexers";
SELECT "Id","Enable","Name","Implementation","ConfigContract","Settings","Priority","RemoveCompletedDownloads",
       "RemoveFailedDownloads","Tags" FROM "DownloadClients";
SELECT * FROM "Notifications";
SELECT * FROM "ImportLists";
SELECT "TvdbId","Title" FROM "ImportListExclusions";
SELECT "Host","RemotePath","LocalPath" FROM "RemotePathMappings";
SELECT * FROM "NamingConfig" LIMIT 1;                                                          -- may be empty
SELECT "Key","Value" FROM "Config";
SELECT "Id","Name","Specifications","RemoveTagsAutomatically","Tags" FROM "AutoTagging";
SELECT "Identifier","Username","Password","Salt","Iterations" FROM "Users";
```
Join rules: Episode → file via `Episodes.EpisodeFileId = EpisodeFiles.Id` (0 = none); file absolute path =
`Series.Path` + `/` + `EpisodeFiles.RelativePath` (paths may be Windows-style if the backup came from Windows — expect
`\` separators and drive letters; offer path remapping). Series → root folder: longest `RootFolders.Path` prefix of
`Series.Path`.

### 9.4 Rust/serde sketches
```rust
use serde::Deserialize;

#[derive(Deserialize, Default)] #[serde(default, rename_all = "camelCase")]
pub struct Revision { pub version: i32, pub real: i32, pub is_repack: bool }

#[derive(Deserialize)] #[serde(rename_all = "camelCase")]
pub struct QualityModel { pub quality: i32, #[serde(default)] pub revision: Revision }   // EpisodeFiles/History/Blocklist.Quality

#[derive(Deserialize)] #[serde(rename_all = "camelCase")]
pub struct ProfileItem {                                                                      // QualityProfiles.Items[]
    #[serde(default)] pub id: i32,                // group id (>=1000) or absent (0)
    #[serde(default)] pub name: Option<String>,   // group name
    #[serde(default)] pub quality: Option<i32>,   // quality id (absent for groups)
    #[serde(default)] pub items: Vec<ProfileItem>,
    #[serde(default)] pub allowed: bool,
    #[serde(default)] pub min_size: Option<f64>,  // v5 only
    #[serde(default)] pub max_size: Option<f64>,
    #[serde(default)] pub preferred_size: Option<f64>,
}

#[derive(Deserialize)] pub struct FormatItem { pub format: i32, pub score: i32 }               // QualityProfiles.FormatItems[]

#[derive(Deserialize)]
pub struct SpecWrapper { #[serde(alias = "Type")] pub r#type: String,                         // CustomFormats / AutoTagging
                         #[serde(alias = "Body")] pub body: serde_json::Value }

#[derive(Deserialize)] #[serde(rename_all = "camelCase")]
pub struct MediaCover { pub cover_type: serde_json::Value /* "poster" or int */,
                        #[serde(default)] pub remote_url: Option<String>, #[serde(default)] pub url: Option<String> }

#[derive(Deserialize)] #[serde(rename_all = "camelCase")]
pub struct Season { pub season_number: i32, #[serde(default)] pub monitored: bool,
                    #[serde(default)] pub title: Option<String>, #[serde(default)] pub images: Vec<MediaCover> }

#[derive(Deserialize, Default)] #[serde(default)]
pub struct Ratings { pub votes: i64, pub value: f64 }

// History.Data / DownloadHistory.Data:
pub type HistoryData = std::collections::HashMap<String, Option<String>>;
```
Helpers you will need: (a) bool from INTEGER **or** TEXT; (b) DateTime from `YYYY-MM-DD HH:MM:SS[.fffffff][Z]` and
`YYYY-MM-DDTHH:MM:SS[.fffffff][Z]` (assume UTC); (c) enum-from-JSON accepting camelCase/PascalCase string or int;
(d) case-insensitive key lookup for settings/MediaInfo (`audioCodecID` etc.); (e) `List<string>` columns that may be a
bare comma-separated string; (f) treat `NULL`, empty string and literal `null` alike for JSON columns.

### 9.5 Things that are NOT in the DB (do not look for them)
* Alternate titles: no `Series.AlternateTitles` column; Sonarr uses `SceneMappings` (by TvdbId, downloaded from Sonarr
  services) and, in v5, `SeriesTranslations`.
* Series root folder (derived), episode file absolute path (derived), `Episode.HasFile` (= `EpisodeFileId > 0`),
  quality weights/group names (code constants), provider "Supports*"/protocol flags (code), provider messages,
  logs and update history (logs.db, not backed up), MediaCover image files (`<AppData>/MediaCover/<seriesId>/…`, not backed up).

