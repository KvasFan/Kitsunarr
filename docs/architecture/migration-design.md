# Migration design: Sonarr / Tanukarr → Kitsunarr

## Goals

- Switch over without rebuilding the library or the settings by hand.
- Never modify the source installation, and never create, move, rename, delete or re-download media files.
- Preview exactly what will happen, then apply it safely and repeatably.

## Flow

```
source ──read-only──► RawSource ──normalise──► SourceSnapshot ──► importer (dry run | apply)
 (API or backup)       (records)                (typed model)        │
                                                                     ├─ dry run: transaction rolled back → preview
                                                                     └─ apply: VACUUM INTO backup → transaction
                                                                                → validation → commit or roll back
```

1. **Connect.**
   - *API*: `POST /api/v1/migration/sessions` with the source URL and API key.
   - *Backup*: `POST /api/v1/migration/sessions/backup` with the raw `.zip` or `sonarr.db`, streamed to disk with a size cap.
   - Both are also available from the CLI as `kitsunarr migrate --url …` or `--backup …`.
2. **Detect.** For an API source, `GET /api/v3/system/status` is read:
   - `productName = "Tanukarr"` means Tanukarr;
   - `appName = "Sonarr"` means Sonarr (v3 or later is required);
   - any other app (Radarr, for example) is refused with a clear message.

   For a backup, the importer checks for Tanukarr-only columns (`Series.MetadataSource`/`ForeignId`), the highest `VersionInfo` entry, and the `INFO` and `config.xml` files. The schema number alone is never trusted, because Tanukarr's 233–254 collide with Sonarr's 233–240.
3. **Read.**
   - The API path fetches every resource, plus episodes and episode files for each series (six at a time) and up to 100,000 history entries.
   - The backup path reads every relevant table with `SELECT *` from the read-only, immutable database copy. It accepts legacy names (`Profiles`, `Restrictions`).
4. **Normalise.** Both encodings are lowered into records with lower-case keys and decoded leniently:
   - enum fields are integers in the database and strings in the API;
   - JSON is stored as text in the database and inline in the API;
   - quality appears as `{quality: 7}` or `{quality: {id: 7}}`;
   - custom-format specifications are `[{type, body}]` or `[{implementation, fields}]`;
   - provider settings are a `Settings` object or an API `fields` array;
   - booleans can be text (legacy rows);
   - release profiles may be comma-separated strings (v3).

   Tests assert that a database record and the matching API record produce identical snapshots.
5. **Plan (dry run).** `POST …/plan` runs the full import in a transaction, then rolls it back. The response has:
   - counts per entity (create, update, skip, unsupported);
   - warnings;
   - a path preview with existence checks (read-only `stat`);
   - blocking problems.
6. **Apply.** `POST …/apply` runs in the background and reports progress over SSE. The steps are:
   1. take a database backup (`VACUUM INTO`);
   2. run the same import;
   3. validate;
   4. commit;
   5. record a migration run with a summary and report.

   Failures roll back and are recorded as failed runs.

## Matching and idempotency

- **Fingerprint.** `sha256(app + ":" + source API key)`. The key is the one entered for API sessions, or the one in `config.xml` for backups. API and backup migrations of the same instance therefore count as one source.
- **`migration_entity_map`.** Maps `(fingerprint, entity, source id)` to a Kitsunarr id. It is consulted first, so renaming a profile in Kitsunarr does not duplicate it on re-run.
- **Natural keys** are used as a fallback:
  - tags: normalised label;
  - root folders: mapped path;
  - profiles and custom formats: name (case-insensitive);
  - providers: kind and name;
  - series and movies: provider identity (a path conflict with a different title skips that title with an error);
  - exclusions: provider and external id;
  - remote path mappings: host and remote path.
- **Conflict policy.** *Skip* is the default and keeps existing Kitsunarr records; *Update* overwrites them. Values Kitsunarr seeded itself and nobody has changed (quality definitions, the six default profiles, naming and media-management settings) take the source values even under *Skip*, because they were never a user choice.
- **History** is imported once per source entry.

## Path mapping

`source prefix → Kitsunarr prefix` rules match at path-segment boundaries (`/tv` does not match `/tvshows`), with the longest prefix first. Windows sources (`D:\TV\…`, matched case-insensitively) become POSIX paths when the target starts with `/`. Relative file paths are copied verbatim. Mapping is textual only.

With *verify paths* on, which is the default, every mapped series folder is checked with a read-only `stat`.

- A missing folder produces a warning.
- If *no* series folder exists, Apply is blocked, because the mappings are almost certainly wrong. The preview explains how to fix it, or verification can be disabled.

## Secrets

The backup source carries every provider secret. The API source receives `********` for password and API-key fields. Those fields are imported empty, listed per provider as "re-enter", and existing stored secrets are never overwritten by masked values.

## Data that has no Kitsunarr equivalent yet

Nothing is dropped silently:

- **Tanukarr-specific values are kept** in `migration_unmapped` and reported as *info* warnings. These include:
  - codec multipliers and per-profile size limits;
  - release-profile air-date rules and excluded tags;
  - ordering ids;
  - group folder styles;
  - Jellyfin watched episode ids;
  - unknown settings keys.
- **Custom quality ids** (Tanukarr presets ≥ 10000) are removed from profiles with a warning. File qualities that use them become *Unknown*, with the original id preserved.
- **Internal keys** (encryption salts, client identifiers) are skipped, and the report says how many were skipped.
- **Users and passwords** are not migrated. Sonarr hashes passwords with PBKDF2, and Kitsunarr creates its own administrator.

## Validation checks

These checks run inside the transaction before commit. A failure means rollback.

| Check | Rule |
|---|---|
| Referential integrity | `pragma_foreign_key_check` reports 0 violations |
| Series records | Every written series exists |
| Episodes | Rows per written series equal the deduplicated source episodes |
| Episode files | Rows per written series equal the source files (unique relative paths) |
| Monitored episodes | The count equals the source |
| Episode ↔ file links | Linked episodes equal the source links whose files were imported |
| Series paths preserved | The stored path equals the mapped source path |
| Movie records | Every written movie exists |

## Fixtures and tests

`tests/fixtures/sonarr-4.0.20/` and `tests/fixtures/tanukarr-0.1.5/` hold real backups and API captures produced by the applications themselves, with fake secrets and third-party text scrubbed (see `tests/fixtures/README.md`). The `kitsunarr-migrate` integration tests cover:

- detection and inventories;
- a dry run that leaves no trace;
- apply with exact counts, preserved monitoring flags, mapped paths, provider identities, secrets and custom-format scores;
- idempotent re-runs;
- skip and update policies;
- missing-media blocking;
- Tanukarr movies and identities;
- API-versus-backup equivalence through a simulated API that replays the captures;
- a rejected API key;
- byte-identical source files and media trees after migration.

The `live_source_dry_run` test runs against a real instance. It is ignored by default and needs `KITSUNARR_TEST_SOURCE_URL` and `KITSUNARR_TEST_SOURCE_KEY`.
