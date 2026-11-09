# Technical decisions

Each entry gives the decision, the reason, and the alternatives that were rejected. Dependency versions were checked on crates.io on 2026-11-09.

## Stack (verified versions)

| Concern | Choice | Version |
|---|---|---|
| Language / toolchain | Rust, edition 2024 | 1.99.1 (pinned in `rust-toolchain.toml`) |
| HTTP server | Axum (+ tower-http) | 0.8.9 (tower-http 0.7.1) |
| Async runtime | Tokio | 1.54 |
| Database | SQLite via SQLx (bundled libsqlite3) | sqlx 0.9.0 |
| HTTP client | reqwest, rustls with the platform verifier | 0.13.5 |
| Web UI | Dioxus (web renderer, WebAssembly) + Dioxus Router | 0.7.10 (`dx` 0.7.10) |
| OpenAPI | utoipa + utoipa-axum | 6.0.0 / 0.3.0 |
| Passwords | argon2 (Argon2id) | 0.6.0 |
| Errors / logging | thiserror, anyhow, tracing | 2 / 1 / 0.1 |

## 1. A single router: Axum serves the API, SSE and the compiled Dioxus SPA

Dioxus *fullstack* (server functions plus SSR/hydration) would add a second, Dioxus-owned way of calling the backend. Kitsunarr needs a stable public REST API anyway, for third-party tools, the planned Sonarr-v3 compatibility layer and OpenAPI. So:

- The UI is a client-side Dioxus web app (`dx build --platform web`).
- It talks to the same REST API every other client uses, through the shared typed client `kitsunarr-client`, built on reqwest's WASM backend (the browser's `fetch`).
- Axum embeds the compiled assets (`rust-embed`, with a build-script fallback when they have not been built) and serves `index.html` for client-side routes.
- The result is one executable with no second router and no duplicated logic.
- A runtime URL base is supported: the server injects `<meta name="kitsunarr-url-base">` and rewrites asset URLs, and the router uses `WebHistory` with that prefix.

## 2. Shared contract crate, compiled twice

`kitsunarr-domain` holds the catalogues and enums plus every API request and response type. It contains no I/O and compiles for `wasm32-unknown-unknown`. The `openapi` feature adds `utoipa::ToSchema` derives on the server side only, so the WASM bundle does not pay for schema code.

## 3. OpenAPI generated from the router

Every handler carries `#[utoipa::path]` and is registered through `utoipa_axum::routes!`, so the published document (`/api/v1/openapi.json`) cannot drift from the real routes. No Swagger or Scalar UI is bundled, because those load JavaScript from CDNs. The JSON document works with any OpenAPI viewer.

## 4. SQLite access with runtime-checked queries

- **No compile-time query macros.** SQLx `query!` macros would need a live `DATABASE_URL` or committed offline metadata at build time. Kitsunarr uses `query`/`query_as` with `FromRow`, and every repository function is exercised by tests against a real SQLite database.
- **Explicit opt-in for dynamic SQL.** SQLx 0.9 only accepts `&'static str` or an explicit `AssertSqlSafe`. Dynamic fragments, such as sort columns and the static lists of table names, are mapped from whitelists, never from input.
- **Connection settings.** WAL, `synchronous=NORMAL`, `foreign_keys=ON`, a 15 s busy timeout and an 8-connection pool.
- **Schema rules.**
  - All tables are `STRICT`, with `CHECK` constraints on booleans, enum-like text and JSON validity.
  - Media↔tag links use join tables, so filtering stays indexed. Provider tags are small JSON arrays, cleaned up when a tag is deleted.
  - Timestamps are RFC 3339 UTC text with second precision, so they sort correctly as strings.
- **Backups** use `VACUUM INTO`, which gives a consistent copy while the server keeps running.

## 5. Identity model

Each series or movie has an owning catalogue and a native identifier, `(metadata_provider, metadata_id)`, which is unique. Any number of external identifiers can be attached per kind. This supports Tanukarr's per-title ownership and its "several AniList entries for one TheTVDB series" without assuming a universal id, and it keeps TheTVDB placeholder ids out of the data.

## 6. Migration: one code path for dry run and apply

The importer performs the complete migration inside a `BEGIN IMMEDIATE` transaction.

- A dry run rolls the transaction back.
- Apply takes a `VACUUM INTO` backup first, then validates (foreign keys, per-series counts, monitored flags, file links, paths) and commits only if every check passes.

Because both modes run the same code, the preview cannot diverge from the result, and constraint violations show up in the preview. Idempotency comes from `migration_entity_map`, keyed by a fingerprint of the source instance: a hash of its API key, which is also present in `config.xml` inside backups. See [migration-design.md](migration-design.md).

## 7. Security defaults

- **First-run setup.** An administrator is created through the web UI. The insert runs in a `BEGIN IMMEDIATE` transaction, so two concurrent setups cannot both succeed.
- **Passwords** are hashed with Argon2id (m=19 MiB, t=2, p=1, the OWASP baseline). Hashing runs in `spawn_blocking`. Logins for unknown users run a dummy verification so response timing doesn't reveal which usernames exist.
- **Sessions** use a random 256-bit token in an `HttpOnly`, `SameSite=Lax` cookie.
  - `Secure` is set when the request arrived over HTTPS through a trusted proxy, or always or never, depending on configuration.
  - The database stores only the SHA-256 of the token.
  - Expiry slides, and sessions are revoked when the password changes.
- **API keys.** One 128-bit hex key, compared in constant time and accepted as `X-Api-Key`, `?apikey=` or a Bearer token. It can only be read or regenerated from an interactive session.
- **CSRF.** `tower_http::csrf::CsrfLayer` (the Go 1.25 cross-origin scheme) checks `Sec-Fetch-Site` and `Origin` against `Host`. No token state is needed. Requests from non-browser clients are unaffected.
- **Login throttling.** Failed logins are rate-limited per client address (10 per 5 minutes by default). `X-Forwarded-For` and `X-Forwarded-Proto` are honoured only from configured trusted proxies.
- **SSRF** (migration API sources):
  - Only `http` and `https`; credentials in URLs are refused.
  - DNS is resolved once and the resolved address is pinned, which blocks rebinding.
  - Link-local and metadata addresses (including `fd00:ec2::254` and IPv4-mapped forms), multicast and unspecified addresses are always refused. Private, loopback and CGNAT ranges need explicit opt-in, which defaults to on because Sonarr usually runs on the LAN.
  - Redirects are not followed, and responses are capped at 256 MiB.
- **Archive extraction.** Backups are extracted under fixed names, so entry paths are never used and zip-slip is impossible. Decompression enforces byte limits, so archives that lie about their sizes are caught. The SQLite header is validated, and the database is opened read-only and immutable.
- **Secret redaction.** Provider fields whose names look like secrets are returned as `********`. Sending the placeholder back keeps the stored value. Logs mark `Authorization`, `Cookie` and `X-Api-Key` as sensitive.
- **Path safety.** Root folders must be existing absolute paths without `..`. Deleting a series or movie removes database records only, never files.
- **Response headers.** `X-Content-Type-Options: nosniff` and `Referrer-Policy: same-origin`.

## 8. Real-time updates over Server-Sent Events

A bounded `tokio::sync::broadcast` channel of 1024 events feeds `GET /api/v1/events`. A client that falls behind receives `resync` and refetches, instead of the server buffering without limit. SSE was chosen over WebSockets because the traffic is server→client only, and SSE works through proxies with plain HTTP semantics.

Over HTTP/1.1, browsers allow six connections per host. Every tab, and every page kept in the back/forward cache, shares that budget. An open event stream holds one connection permanently, so the web client holds it only while the page is in use:

- The stream closes on `pagehide`, and when the tab has been hidden for more than 10 seconds.
- It reopens when the page is shown again, followed by a refetch.
- After a connection error, the browser's automatic reconnect also triggers a refetch, since events may have been missed.

Without this, a handful of tabs or full page loads left API requests queued inside the browser. The browser end-to-end test hit exactly that: its seventh page load stalled until a cached page was evicted. Behind an HTTP/2 reverse proxy the limit does not apply, but the lifecycle rules are kept anyway.

## 9. Licensing

Sonarr and Tanukarr are GPL-3.0-or-later. Kitsunarr ports their behaviour, parser rules, data formats and test cases, so it is distributed under **GPL-3.0-or-later** and keeps the upstream copyright notices (see `COPYRIGHT.md`).

The Tanukarr image is labelled `GPL-3.0-only` while its `COPYRIGHT.md` says "or later". This audit records that inconsistency. The legal notice in the repository was treated as authoritative.

## 10. Measured WebAssembly bundle

These figures come from a clean `dx build -p kitsunarr-web --release --profile web-release` (Dioxus 0.7.10, Rust 1.99.1), measured on 2026-11-09 after the Add media page landed (before it, the WASM file was 2,287,717 bytes, or 605,742 with brotli). They are also recomputed in CI and shown in the job summary.

| File | Bytes | gzip -9 | brotli -11 |
|---|---:|---:|---:|
| `kitsunarr-web_bg-*.wasm` | 2,396,162 | 828,339 | 629,362 |
| `kitsunarr-web-*.js` (loader) | 64,189 | 14,964 | 12,803 |
| `main-*.css` | 20,732 | 4,960 | 4,366 |

Assets have content-hashed names and are served with `immutable` caching. Responses are compressed on the fly by tower-http (brotli or gzip).

## 11. Toolchain and build

- `rust-toolchain.toml` pins 1.99.1, with `rustfmt`, `clippy` and the `wasm32-unknown-unknown` target.
- The WebAssembly bundle is built with `dx build -p kitsunarr-web --release` using the `web-release` profile (`opt-level = "z"`, fat LTO, one codegen unit, `panic = "abort"`). The server build then embeds the output.
- A runtime container needs no Node.js, Python or .NET. Python appears only in `tests/fixtures/generate/`, a documented developer tool used to regenerate fixtures from real Sonarr/Tanukarr containers.

## 12. Metadata providers

- **Adapters.** `kitsunarr-metadata` turns each catalogue into one model, `SeriesInfo` and `SearchResult`, and keeps the catalogue's own identity. TVmaze (TV) and AniList (anime and anime films) are implemented; neither needs an API key. Both endpoints can be configured (`[metadata]`) so a caching proxy can be used.
- **Rate limits and caching.** Each adapter has a sliding-window limiter: TVmaze 18 requests per 10 s (its limit is 20), AniList 25 per minute (its degraded limit is 30). Both retry once after HTTP 429, honouring `Retry-After`. Detail responses are cached for 10 minutes; refreshes bypass the cache.
- **Numbering matches Tanukarr.** Titles migrated from Tanukarr and titles added in Kitsunarr must refresh onto the same rows. So the TVmaze adapter fetches episodes with `specials=1`, keeps TVmaze season/number, puts unnumbered specials in season 0 in episode-id order, and sets no absolute numbers. AniList titles get one season with absolute = episode number. An inconsistent TVmaze episode list (duplicate slot or id, negative season) is rejected, so a refresh cannot overwrite good data with bad.
- **Adding never touches the disk.** The folder path comes from the root folder and the naming settings. The web UI renders the same path with the shared `naming` module, so the preview is exact. The folder is created by the import pipeline when files arrive.
- **Monitoring follows Sonarr.**
  - Specials are never monitored automatically.
  - "Future episodes" includes episodes with unknown air dates, unless a later episode of the same season has already aired. AniList has no schedule entries for multi-episode premieres, and this rule handles that case.
  - On refresh, a newly discovered episode is monitored only when the series monitors new items and its season is monitored.
- **Refresh scope.** A refresh updates provider-owned fields, merges external ids, replaces provider-sourced alternate titles and adds missing seasons and episodes. It never changes monitoring, file links, paths, profiles or tags. A title owned by a catalogue Kitsunarr cannot query (for example a TheTVDB series migrated from Sonarr) is not refreshed through another catalogue: episode numbering differs between catalogues and could create phantom episodes. The UI says so instead of offering the action.
- **Library matching.** Search results and add requests are matched against the library by catalogue id and by every shared external id (TheTVDB, IMDb, MyAnimeList, ...). This prevents duplicates across catalogues.

## 13. Release parsing: Sonarr's parser, verbatim, on a .NET-semantics regex engine

Kitsunarr must read release names exactly as Sonarr does: the decision engine, download tracking and import all depend on it. Sonarr's `Parser.cs` holds 109 ordered title patterns plus about 20 helper patterns, and depends on .NET regex features no Rust engine offers:

- **Capture collections:** every iteration of a repeated group, read back as a list (episodes, absolute episodes, seasons, titles).
- **Group names reused within a pattern.**
- **Lookbehind of any length.**

Rewriting about 130 patterns to avoid these features would risk silent behaviour changes. Instead:

- **`netregex`** is a small backtracking engine with .NET semantics:
  - capture stacks per group, with duplicate names merged and .NET group numbering;
  - lookbehind matched right to left, backreferences (also inside lookbehinds), atomic groups and inline options;
  - .NET's Unicode `\d`/`\w`/`\s`;
  - a step budget and an input-length cap, so a pathological title cannot stall the server.
- **The patterns are generated, not hand-copied.** `tools/gen_title_patterns.py` decodes every `Regex`/`RegexReplace` field of `Parser.cs` into `src/title_patterns.rs`.
- **The control flow follows `Parser.cs` statement by statement**, including the quirks:
  - an exception anywhere ends parsing with no result;
  - positions found in the simplified title are used against the release title;
  - early air dates are allowed only up to 14 days ahead.
- **`RemoveDiacritics` is table-driven, like upstream:** Diacritical.Net 1.0.5 plus Sonarr's three additions, generated by `tools/gen_diacritics.py`. It is not Unicode decomposition, which would map differently (`ð` has no decomposition but becomes `d`).
- **The simpler parsers (quality, release group, language) use `fancy-regex`.** Their few duplicate group names and capture collections are handled explicitly, and the test corpus confirms each.

Verification:

- `tests/fixtures/generate/sonarr_test_cases.py` extracts the 1,683 active `[TestCase]`/`TestCaseSource` rows of Sonarr's parser test suite. All of them pass. A test method present upstream but not handled fails the run, so new upstream tests cannot be skipped silently.
- Mutation checks show the corpus is sensitive to the risky parts. Keeping only the last capture of each group, as an ordinary Rust regex would, fails 133 cases. Dropping the release-token languages fails 19.

Throughput is about 0.2 ms per title (`tests/perf.rs`). If parsing ever matters for large searches, the next optimisation is a required-literal prefilter per pattern.

