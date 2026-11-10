# Kitsunarr

*A modern Rust-powered media manager for TV, anime and movies.*

Kitsunarr is an independent, self-hosted media automation platform built with Rust, Dioxus and Axum. It manages TV shows, anime, movies, metadata, downloads, libraries and automated workflows, and it can migrate existing Sonarr and Tanukarr installations.

> **Project status: early development.** The application foundation (server, database, authentication, library, settings, background tasks) and migration from Sonarr and Tanukarr work and are tested. Titles can be added and refreshed from TVmaze (TV series) and AniList (anime and anime films). Newznab and Torznab indexers can be tested and searched interactively; each release is parsed, checked against the quality profile, custom formats and release profiles, and ranked as Sonarr does, a torrent can be grabbed into qBittorrent, followed in the queue and, once complete, imported into the series folder. RSS sync grabs wanted releases from the indexers' feeds, and an automatic search can be started for a season or an episode. Other download clients, renaming existing files, other metadata catalogues and scheduled missing and upgrade searches are **not implemented yet**, and only qBittorrent is supported. **If Kitsunarr shares indexers and a download client with an installation that is still running, set Settings › Indexers › "RSS sync interval" to 0**, or both will grab the same releases. Keep your existing installation running. Migration is read-only and never touches your media. The [feature matrix](docs/architecture/feature-matrix.md) tracks exactly what works.

## Highlights

- **A single Rust executable.**
  - An Axum REST API (OpenAPI document at `/api/v1/openapi.json`).
  - Server-Sent Events for live updates.
  - The embedded Dioxus web interface, compiled to WebAssembly.
  - No Node.js, Python or .NET at runtime.
- **One library for TV, anime and movies.**
  - Per-title metadata ownership, with any number of external identities per title.
  - Add titles from TVmaze and AniList, with daily metadata refresh that keeps your monitoring choices.
  - Collections of series and movies.
- **Migration from Sonarr (v3/v4) and Tanukarr**, through the source API or a backup file.
  - A dry-run preview that is identical to the real run.
  - Path mapping for Docker layouts.
  - A pre-migration database backup.
  - Validation checks and idempotent re-runs.
  - Your media files are never moved, renamed or deleted.
- **Secure defaults.**
  - First-run administrator setup and Argon2id password hashing.
  - Hashed session tokens and Sonarr-compatible API keys.
  - CSRF protection, login throttling and SSRF guards.
  - Secrets redacted in API responses.
- **SQLite with versioned migrations.** WAL mode, enforced foreign keys, consistent online backups and integrity checks.

## Quick start (Docker)

```yaml
services:
  kitsunarr:
    image: ghcr.io/kvasfan/kitsunarr:latest
    container_name: kitsunarr
    ports:
      - "8990:8990"
    environment:
      - TZ=Etc/UTC
    volumes:
      - ./config:/config     # database, backups, config.toml
      - /srv/media:/media    # the same media your Sonarr/Tanukarr uses
    restart: unless-stopped
```

Open `http://<host>:8990`, create the administrator account, then choose **Import / Migration** to bring over an existing installation. See [docs/install.md](docs/install.md) for the Docker image (published by the release workflow) and other options.

## Migrating from Sonarr or Tanukarr

1. In Sonarr or Tanukarr, open *System → Backup*, create a backup and download it. A backup includes provider passwords and API keys, while the API method cannot read them.
2. In Kitsunarr, open **Import / Migration** and upload the `.zip`. You can also enter the source URL and API key instead.
3. Add path mappings if Kitsunarr sees your media under different paths, for example `/tv` → `/media/tv`.
4. Review the preview, then apply. Kitsunarr backs up its own database first and validates the result before committing.

The CLI offers the same thing, as a dry run unless you pass `--apply`:

```sh
kitsunarr migrate --backup sonarr_backup.zip --map /tv=/media/tv
kitsunarr migrate --url http://sonarr:8989 --api-key … --map /tv=/media/tv --apply
```

Details are in the [migration design](docs/architecture/migration-design.md) and the [compatibility matrix](docs/architecture/compatibility-matrix.md).

## Configuration

Settings edited in the web interface live in the database. Process settings come from `config.toml` in the configuration directory (`/config` by default; an example is written on first start), from `KITSUNARR_*` environment variables, or from command-line flags.

| Variable | Default | Purpose |
|---|---|---|
| `KITSUNARR_CONFIG_DIR` | `/config` | Location of `config.toml` |
| `KITSUNARR_DATA_DIR` | `/var/lib/kitsunarr` (`/config` in the image) | Database, backups, temporary files |
| `KITSUNARR_PORT` | `8990` | HTTP port |
| `KITSUNARR_BIND_ADDRESS` | `0.0.0.0` | Listen address |
| `KITSUNARR_URL_BASE` | *(empty)* | Path prefix behind a reverse proxy, e.g. `/kitsunarr` |
| `KITSUNARR_TRUSTED_PROXIES` | *(none)* | Comma-separated proxy IPs or CIDRs whose `X-Forwarded-*` headers are trusted |
| `KITSUNARR_SECURE_COOKIES` | `auto` | `auto`, `always` or `never` |
| `KITSUNARR_LOG_LEVEL` / `KITSUNARR_LOG_FORMAT` | `info` / `text` | Logging (`json` for structured logs) |

Subcommands:

- `kitsunarr backup` and `kitsunarr restore <file>` (the server must be stopped to restore);
- `kitsunarr healthcheck`, used by the container health check;
- `kitsunarr config`, which prints the effective configuration.

## Building from source

```sh
rustup show                                     # installs the pinned toolchain and the wasm32 target
cargo install dioxus-cli@0.7.10 --locked        # once
dx build -p kitsunarr-web --release --profile web-release   # web UI → target/dx/…/public
cargo build --release -p kitsunarr-server       # embeds the web UI → target/release/kitsunarr
cargo test --workspace
```

Without the `dx` step the server still builds and serves the API, plus a notice page instead of the UI.

## Architecture

The project is a modular monolith in a Cargo workspace:

| Crate | Responsibility |
|---|---|
| `kitsunarr-domain` | Shared domain model and the typed API contract (native and WebAssembly) |
| `kitsunarr-config` | Configuration loading |
| `kitsunarr-db` | SQLite schema, migrations, repositories, backups |
| `kitsunarr-auth` | Argon2id, session tokens, API keys |
| `kitsunarr-events` | Bounded event bus feeding SSE |
| `kitsunarr-api` | Axum routes, authentication, CSRF, OpenAPI, asset serving |
| `kitsunarr-migrate` | Sonarr/Tanukarr readers, normaliser, importer |
| `kitsunarr-scheduler` | Background tasks with history and cancellation |
| `kitsunarr-client` | Typed API client used by the web UI |
| `kitsunarr-web` | Dioxus web interface |
| `apps/kitsunarr-server` | The `kitsunarr` executable |

Further reading:

- [source audit](docs/architecture/source-audit.md)
- [technical decisions](docs/architecture/technical-decisions.md)
- [feature matrix](docs/architecture/feature-matrix.md)

## License and attribution

Kitsunarr is free software under the **GNU General Public License v3.0 or later** (see [LICENSE](LICENSE)). It is a clean-room Rust implementation informed by a source-level study of [Sonarr](https://github.com/Sonarr/Sonarr) and [Tanukarr](https://github.com/KvasFan/Tanukarr). Where it ports their behaviour, data formats, parser rules or test cases, their GPL notices apply and are kept in [COPYRIGHT.md](COPYRIGHT.md).

Kitsunarr is not affiliated with, endorsed by or supported by the Sonarr project. Please report Kitsunarr problems in this repository, not to Sonarr.
