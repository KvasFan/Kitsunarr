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

## 14. Indexers: Sonarr's Newznab/Torznab behaviour, checked against its recorded feeds

`kitsunarr-indexers` ports the parts of Sonarr that talk to Newznab and Torznab indexers: `NewznabCapabilitiesProvider`, `NewznabRequestGenerator`, `NewznabRssParser`/`TorznabRssParser` and the paging loop of `HttpIndexerBase`. Upstream behaviour kept on purpose:

- **Requests:** ID parameters are preferred in Sonarr's order (TheTVDB, IMDb, TVRage, TVmaze, TMDB), or sent together when the indexer supports it. Title queries follow only when the ID searches find nothing. Season 0 is sent as `00`, daily episodes as `season=yyyy&ep=MM/dd`, and anime absolute numbers padded to two digits.
- **Paging:** a query pages until a page comes back short, up to 30 pages or 1,000 releases. The page size is Sonarr's clamp of the indexer's default and maximum.
- **Errors:** `<error>` codes 100–199 mean a bad API key; "Request limit reached" and HTTP 429 are request limits. A Torznab feed answering a Newznab request (or the reverse) is rejected. A missing `pubDate` fails the whole feed; any other bad item is skipped.
- **Politeness:** at least 2 s between two requests to the same indexer.

Verification uses upstream's own material. `tests/fixtures/indexers/sonarr/` holds the feeds recorded in Sonarr's test suite (`src/NzbDrone.Core.Test/Files/Indexers/`), plus the `[TestCase]` rows of its Newznab, Torznab, request-generator and capabilities fixtures, extracted by `tests/fixtures/generate/sonarr_test_cases.py`.

Two choices differ from Sonarr:

- **Download links never reach the browser.** Newznab download URLs embed the indexer's API key, so the release list sent to the UI omits them. A future "grab" will refer to a release by indexer and GUID, and the server will resolve the link.
- **Request errors report their cause without the URL.** For example: "Could not connect to the indexer: failed to lookup address information". The cause is useful, but the URL would expose the API key.

## 15. Release decisions: Sonarr's decision engine, with Kitsunarr's v4 data model

`kitsunarr-decision` decides whether each release found by a search is wanted, and ranks the results. It ports Sonarr's `DownloadDecisionMaker`, the decision specifications, `CustomFormatCalculationService` and its eight specifications, `UpgradableSpecification`, `DownloadDecisionComparer`, `AggregateLanguages`, and the search side of `ParsingService`. It follows the same Sonarr revision as the parser (`c018578e4`, the v5 line), with one deliberate exception: size limits come from the quality definitions, as in Sonarr v4. Kitsunarr's data model, like the Sonarr 4 installations it migrates, keeps sizes there; v5 moved them into quality profiles.

What a search does with each release, as upstream:

1. **Parse:** parse the title, then try a special-episode title match and a season-title parse.
2. **Map:** map the release to a series and episodes. Titles, alternate titles and the identifiers the indexer reported are checked against the searched series first, then the rest of the library.
3. **Score:** aggregate languages (indexer languages, episode-title words, multi-language indexers, the series' original language), evaluate custom formats and sum their scores.
4. **Check:** run every applicable specification and keep all rejection messages.
5. **Rank:** de-duplicate by GUID (fewest rejections, then indexer priority), and rank with the comparer chain: quality and revision, custom format score, preferred protocol, season packs and episode count, episode number, indexer priority, seeders and peers, age, closeness to the preferred size. Ranking ignores rejections, as in Sonarr.

Regular expressions in custom formats and release profiles run on the .NET-semantics engine of the parser crate, so patterns written for Sonarr behave the same.

Known differences, all reported rather than hidden:

- **The download queue** is checked against the downloads Kitsunarr grabbed that a download client still holds and that are not imported. The list is kept in memory: it is filled by grabs and by the minute-by-minute download check, so after a restart it is complete only after the first check.
- **Checks that need missing data are not evaluated yet:** the blocklist, pending releases, free space, scene mappings (TheXEM) and v5-only release-profile options. Delay-profile delays do not apply to interactive searches upstream either.
- **Settings:** minimum age, retention and maximum size (Settings › Indexers › Options) and "propers and repacks" (Settings › Media management) are Kitsunarr settings, migrated from the source's database `Config` keys or its `/config/indexer` and `/config/mediamanagement` API. Season-pack upgrades use Sonarr's default (every episode must be missing or upgradable), as that setting does not exist in Sonarr 4.
- **Releases of other library series are rejected as "Wrong series" without evaluating that series' episodes and profile.** They are listed after the searched series' releases, in arrival order; upstream orders series groups by first appearance.
- **A custom format specification that cannot be evaluated never matches, and a warning is shown with the search results.** This covers an unknown implementation, such as Tanukarr's `SeasonPackSpecification`, and an invalid pattern. Sonarr would refuse to load such a format.

Verification: tests ported from Sonarr's `DecisionEngineTests` cover ranking (all 30 `PrioritizeDownloadDecisionFixture` tests) and the sizes, upgrades, raw discs, release restrictions, search matching, repacks and other specifications. Sonarr's custom format language and release-type fixtures are ported too. End-to-end tests run real release names through parsing, mapping, decisions and ranking. Mutation checks confirm the ranking and size tests fail when the logic is broken.

## 16. Grabbing: releases resolved on the server; qBittorrent, Deluge, Transmission, SABnzbd and NZBGet

A grab names a release by indexer and GUID, never by download link. Search results are kept on the server for 30 minutes, as in Sonarr; the grab resolves the link there, so indexer API keys stay out of the browser, the API and the history.

The rest follows Sonarr:

- **Client choice** (`DownloadClientProvider`), in order:
  1. enabled clients for the release's protocol;
  2. among them, those sharing a tag with the series, otherwise those without tags;
  3. the client the indexer is assigned to, if any;
  4. the best priority, rotating between clients of equal priority.
- **Torrent source** (`TorrentClientBase`):
  - the magnet link is used first;
  - if the client cannot take it, the `.torrent` file is used instead. The file is fetched through the indexer client, so it shares the indexer's rate limit, and redirects are followed by hand so a redirect to a magnet link is recognised.
  - downloads are tracked by the BitTorrent v1 info hash, from the magnet link or the SHA-1 of the file's `info` dictionary.
- **qBittorrent** (`QBittorrent.cs`, API v2 proxy):
  - login caches the session cookie and retries once on 403;
  - the Test creates missing categories and rejects share-limit settings that would remove torrents before import, as upstream;
  - adding a torrent sets the category and the initial state (`stopped` on Web API 2.11 and later, `paused` before), then top priority for recent episodes, force start and series tags.
- **Deluge** (`Deluge.cs`, `DelugeProxy.cs`), through the Web UI's JSON-RPC API (`POST /json`):
  - logs in with the Web UI password and caches the `_session_id` cookie; when a call answers "not authenticated" or "not connected" (codes 1 and 2) or times out, it logs in again once;
  - after login, a Web UI not connected to a daemon is connected to the local one (`127.0.0.1` in its Connection Manager), as upstream does;
  - the Test needs the Label plugin when labels are set, and creates missing labels, as upstream;
  - adding a torrent passes the download and completed directories and "add paused", sets the label, then moves recent (or older) episodes to the top of the queue when their priority is "first";
  - the torrent list comes from `web.update_ui` filtered by label, with Sonarr's state mapping (error, finished unless checking, queued, paused, else downloading) and the save path plus name as output path.
- **Transmission** (`TransmissionBase.cs`, `TransmissionProxy.cs`), through its RPC API (`POST {base}/rpc`, URL base `/transmission/` by default):
  - basic authentication; the `X-Transmission-Session-Id` handshake on HTTP 409, with the id cached per address;
  - labels from Transmission 4: adds carry the TV label, the list keeps labelled torrents (and, for torrents from before the upgrade, those in the category folder), and imports swap the TV label for the post-import one;
  - before Transmission 4 the category is a subfolder of the download directory, and nothing changes after import;
  - the Test needs Transmission 2.40 or newer, as upstream.
- **SABnzbd** (`Sabnzbd.cs`, `SabnzbdProxy.cs`), through `GET {base}/api?mode=..&output=json` with the API key (or `ma_username`/`ma_password`):
  - NZBs are uploaded with `mode=addfile` (multipart field `name`), with the category and the recent or older priority; the answer's first `nzo_id` is the download id;
  - the queue and the history (60 entries, as upstream's default `DownloadClientHistoryLimit`) are read with Sonarr's state mapping; a history job's output path is the outermost folder named after the job;
  - the Test follows upstream: version 0.7+, "check before download" before 1.1, the category must exist and not end with `*` (job folders), no TV/movie/date sorting for it, and a warning when the history is kept for less than 14 days;
  - SABnzbd returns numbers as strings and booleans as 0/1; answers are read leniently, as Newtonsoft does upstream.
- **NZBGet** (`Nzbget.cs`, `NzbgetProxy.cs`), through `POST {base}/jsonrpc` with basic authentication:
  - only NZBGet 16 (2016) and newer: `append` carries a `drone` parameter with a random id, which tracks the download. Upstream's legacy forms for 12–15 (append, then find the job and set the parameter) are not ported, and the Test reports older versions;
  - sizes are joined from their 32-bit halves; time left accumulates over the queue at the current download rate; history states follow the PAR, unpack, move, script, delete and mark statuses as upstream.
- **NZBs** are fetched through the indexer client (its rate limit and redirects) and validated as `NzbValidationService` does (an `nzb` root with at least one `file`; an indexer `<error>` document is reported) before they reach the client.
- **Usenet imports move the files** (`CanMoveFiles`); across file systems the file is copied, then the source removed. Torrent imports still copy or hard-link.
- **History:** one "Grabbed" entry per episode, with Sonarr's data keys except `downloadUrl`, which would store the indexer key.

Two choices differ from Sonarr:

- **Grabs are rejected rather than mapped by hand.** A release that could not be matched to episodes cannot be grabbed until manual episode selection exists; Sonarr would ask the user to pick them.
- **"Implemented" means implemented.** qBittorrent, Deluge, Transmission, SABnzbd and NZBGet are marked as implemented download clients, and no notification type is. Other providers are stored with all their settings, and the health check and migration report say they are not implemented yet.

## 17. Importing completed downloads, safer than upstream where files are at stake

Every minute, a check (Sonarr's `RefreshMonitoredDownloads`) reads the download clients. It imports completed downloads that Kitsunarr grabbed and has not imported yet; a "downloadFolderImported" history entry newer than the last grab marks a download as imported. The import follows `DownloadedEpisodesImportService`, `ImportDecisionMaker`, the aggregators and `ImportApprovedEpisodes`; the feature matrix lists what is covered.

Where files are at stake, Kitsunarr is stricter than Sonarr:

- **New file first.** The new file is placed before a file it upgrades is removed. Upstream removes first, so a failed transfer would lose the old file.
- **Removals stay inside the series folder.** A replaced file is removed only if its path lies inside the series folder, checked lexically with `..` resolved. As upstream, it goes to the recycle bin when one is set, and is deleted otherwise.
- **No overwrites.** An existing destination is never overwritten: "Destination already exists".
- **Torrents stay in the client.** Their files are copied, hard-linked when "Use hardlinks" is on, so they keep seeding. Usenet downloads are moved, as upstream (section 16).
- **Removal from the client follows upstream.** With the client's "Remove completed downloads" option on, a download is removed (with its data) after import only when the client is done with it (`CanBeRemoved`): Usenet jobs always; qBittorrent torrents stopped at their seed limit (ratio or seeding time, the torrent's or the global one); Transmission torrents past their ratio or idle limit; Deluge torrents it manages and stopped at their ratio. A torrent that finishes seeding later is removed by a later check (`DownloadCanBeRemovedEvent`). The inactive-seeding limit of qBittorrent is not read, so such torrents stay.

Where Kitsunarr lacks a component, it says so instead of guessing:

- **Naming:** files and season folders are named by the port of `FileNameBuilder` (section 18). With "Rename files on import" off (Sonarr's default), files keep the release or file name, cleaned as `CleanFileName` does.
- **Media info:** there is none yet (Sonarr uses ffprobe). Samples are recognised by name only, and quality comes from names, not from the video's resolution.

## 18. File and folder naming: Sonarr's `FileNameBuilder`, ported token by token

`kitsunarr-library::file_names` ports `FileNameBuilder`: the token regular expression, token matching that ignores case and separators, casing rules (an all-lower-case token lowers its value), separators, prefixes and suffixes, `{{`/`}}` escapes, the six multi-episode styles for season/episode and absolute numbers, episode titles shortened to fit 255 bytes, `CleanFileName` with the colon settings, and reserved device names. It runs on the parser's .NET-semantics regex engine.

It is tested with Sonarr's naming fixtures (about 80 cases). Mutation checks confirm the tests fail when separator collapsing or the casing rules are broken.

Series folders are named with it when a title is added. The add dialog previews the folder through `GET /library/folder-preview`, so the preview always matches the folder actually created; the browser does not carry its own copy of the engine.

Differences:

- **Fallback release group:** "Kitsunarr" instead of "Sonarr".
- **No media info:** media-info tokens render empty, as upstream does for files without media info.
- **Movies:** movie folders still use the simple helper, which covers Tanukarr's default movie formats.
- **Default multi-episode style:** corrected to Sonarr's (PrefixedRange). Kitsunarr previously defaulted to Extend.

## 19. Automatic search: Sonarr's processing of decisions, run on request

`POST /releases/search` runs the same search as interactive search for a season or an episode, on the indexers enabled for automatic search, then grabs as `ProcessDownloadDecisions.ProcessDecisions` does: approved releases best first; a release is skipped when one of its episodes was already grabbed by this search; when no download client is usable for a protocol, the remaining releases of that protocol are skipped. Automatic season searches only want monitored episodes (`MonitoredEpisodeSpecification`); episode searches take the requested episode whatever its flag, as Sonarr's `EpisodeSearch` command does.

Differences:

- **Runs while the request waits:** Sonarr queues a command and reports through its command list. Kitsunarr answers with the outcome (releases found and approved, grabs, failures), shown as a notification. Scheduled searches (missing, cutoff unmet, RSS) are not implemented yet.
- **Not covered yet:** pending releases (delay profiles hold nothing back: delays are not evaluated).

## 20. RSS sync: library-wide mapping, then per-series decisions

The `RssSync` job reads the newest releases of every indexer with RSS enabled (`GetRecentRequests`), maps each release to a library series and decides on each series' releases with the RSS rules, then grabs as automatic search does (section 19). Its interval is the "RSS sync interval" setting (0 turns it off), applied to the scheduled task when the setting is saved, after a migration and at startup.

Mapping follows `GetRssDecision`: a title that does not parse, or parses as a possible special, is first checked against the specials of the series its ids or title point to. Otherwise the series is found as `FindSeries` does without search criteria: scene alias (alternate titles), title, all titles, title without year, then TheTVDB, TVRage and IMDb ids.

The RSS-only rules are ported with Sonarr's fixtures:

- **History:** an episode whose most recent history event is a grab from the last 12 hours, at the same or better preference, is not grabbed again. Completed download handling is always on in Kitsunarr, so older grabs do not count.
- **Delay profiles:** a release younger than the delay of its protocol is rejected ("Waiting for better quality release"), with the bypasses for the best quality in the profile, a minimum custom format score, and propers of the preferred protocol. User-started searches are not delayed, as upstream.
- **Propers:** a proper or repack for a file on disk is rejected when the "propers and repacks" setting is "do not upgrade", or when the file was added more than seven days ago.
- **Monitored:** only monitored episodes of monitored series.

Differences:

- **No pending releases:** upstream keeps delayed releases and grabs them when the delay ends. Kitsunarr grabs a delayed release only if a later sync still finds it in the feed.
- **Decisions are made series by series:** releases of different series are ranked separately, and a series that cannot be loaded is skipped and logged.

## 21. Wanted lists and bulk searches

Wanted › Missing ports `EpisodesWithoutFiles`: episodes without a file whose air date plus the series runtime has passed, specials included, either monitored (episode and series) or not. Cutoff unmet ports `EpisodesWhereCutoffUnmet`: episodes whose file quality ranks before the profile's cutoff (the first allowed quality when upgrades are off). As in Sonarr 4, custom format scores do not put an episode on the cutoff list.

Searching ports `EpisodeSearchService.SearchForBulkEpisodes`:

- **Skipped:** episodes already in the queue.
- **Grouping:** the rest are grouped by series and season, and the groups searched never-searched first, then oldest search first.
- **Search per group:** a season search limited to the group's episodes when it has several, an episode search otherwise. Each group's approved releases are grabbed as automatic search grabs them (section 19).
- **Last search time:** every search records it on the searched episodes when at least one indexer was queried (`UpdateLastSearchTime`).
- **Indexers:** only indexers without tags, or sharing a tag with the series, are searched, as `ReleaseSearchService` does.

Differences:

- **Runs as a background task of the server, not a queued command.** One wanted search runs at a time: another request is refused until it finishes, and a search started by adding a series waits for its turn. The outcome arrives as a notification and in the log.
- **No scheduled missing or cutoff searches**, as in Sonarr, where they only run on request.


## 22. Failed downloads and the blocklist

Failed download handling follows Sonarr's `FailedDownloadService` and the handlers of its `DownloadFailedEvent`:

- **Detection:** a download Kitsunarr grabbed that its client reports as failed (SABnzbd's failed jobs, NZBGet's failed PAR/unpack/script/delete statuses) is handled once: a "downloadFailed" history entry per episode, a blocklist entry describing the release (indexer, publish date, size, protocol, info hash for torrents), and, when the client's "Remove failed downloads" option is on, its removal from the client. A "downloadFailed" entry newer than the last grab marks it as handled, so later checks leave it alone.
- **Searching again** (`RedownloadFailedDownloadService`): unless Settings › Download clients turns it off (separately for releases grabbed from interactive search), one episode is searched as an episode search, a whole season as a season search, and other multi-episode releases episode by episode. Searches run in the background after the check and report with a notification. Grabs now record their source (`Rss`, `Search`, `InteractiveSearch`) as upstream's `releaseSource`.
- **By hand:** the queue's Remove (Sonarr's `QueueController.Remove`) can remove the download from its client, blocklist the release (which marks it as failed, with or without a new search), or both; "Mark as failed" on a grab in History does the same as upstream's `MarkAsFailed`.
- **The blocklist** (`BlocklistSpecification`, `BlocklistService.Blocklisted`, `ReleaseComparer`) rejects, for the same series, a torrent with the same info hash, a torrent without one whose title is contained in a blocklisted torrent's title from the same indexer (or an entry without indexer), and a Usenet release with such a title, indexer and the same publish date. Activity › Blocklist lists the entries; they can be removed one by one or all at once.

Differences:

- **Stalled torrents** are not failed automatically (upstream's newer stalled-torrent timeout) and encrypted SABnzbd jobs are not detected as such.
- **Migration** does not import Sonarr's or Tanukarr's blocklist yet (the fixtures hold none to verify against).
- A failed download's queue entry stays until the client removes it; it no longer counts as "in the queue" for decisions, so the new search is not blocked by it.

## 23. Metadata catalogues: Tanukarr's seven, with its numbering

Every title belongs to one catalogue (`metadata_provider`, `metadata_id`), as in Tanukarr; refreshes read that catalogue only. The adapters port Tanukarr's proxies and keep their numbering, so migrated titles line up with refreshed ones:

- **TheTVDB through SkyHook** (`SkyHookProxy`): `GET {base}/shows/eng/{tvdbId}/{seasonType}` and `GET {base}/search/?language=eng&term=…`, no key. Sonarr's mapping: statuses (ended, upcoming, else continuing), the original-language title from the translations, air time, ratings, cross-ids (TMDB, TVmaze, TVRage, IMDb, MyAnimeList, AniList), artwork. The title's `episode_ordering` is the season type (`official`, `dvd`, `absolute`, ...). Anime are recognised by TheTVDB's "Anime" genre. Titles migrated from Sonarr belong to TheTVDB, so they refresh without re-matching. SkyHook is Sonarr's service; Kitsunarr calls it as Tanukarr does, identified by its own user agent, and the endpoint can be changed (`KITSUNARR_TVDB_URL`).
- **TMDB** (`TmdbProxy`, `TmdbMovieProxy`), with the user's v3 key: `/tv/{id}` with `external_ids`, one request per season, and `/tv/episode_group/{id}` when the title's ordering is an episode group. `TmdbEpisodeGroupProjector` is ported: positions number the episodes, group order the seasons, duplicates are ignored, unlisted episodes keep their numbers; seasons follow the episodes; absolute numbers are given in order. A failed group fetch fails the refresh, keeping the numbering on disk. Films: `/movie/{id}`, `/find/{imdbId}` and `/search/movie`, one feature episode.
- **MyAnimeList through Jikan** (`JikanProxy`): `/anime/{id}/full` and up to 30 pages of `/anime/{id}/episodes`, 3 requests a second and 60 a minute. One season; episode number = Jikan's episode id; numbers up to the declared count are filled when the list is short. Unlike Tanukarr, a filled-in episode gets no provider id and episodes are matched by number, so the data-loss path the audit found (placeholders recreated when titles arrive) cannot happen. Adult (`Rx`) entries are left out of searches.
- **OMDb** (`OmdbProxy`), with the user's key: films by IMDb id or title; only `Type == movie`; posters only from IMDb's image hosts.
- **TVmaze and AniList** as before (section 12 of the migration design).

Keys are stored in the settings table (`metadataSources`), migrated from Tanukarr's `tmdbapikey`/`omdbapikey`, redacted in API responses (the placeholder keeps a key), and applied to the clients at start-up and on save. A catalogue that needs a key is listed as unavailable: searches and refreshes say so, and the scheduled refresh skips its titles.

Not ported: Tanukarr's presentation enrichment (filling a missing overview or artwork from a second catalogue), choosing an ordering when adding a title, and TheTVDB languages other than English. TMDB, OMDb and Jikan are tested on their documented response shapes, not against the live services (no keys; Jikan was unreachable from the test network); SkyHook is tested on recorded responses and live.

## 24. Notifications: Sonarr's services and payloads

`kitsunarr-notifications` ports `NotificationService`'s filters and each service's message format. An event goes to every stored connection whose switch for it is on (`onGrab`, `onDownload`, `onSeriesAdd`, ...). An upgrade also needs `onUpgrade`, and health warnings need `includeHealthWarnings`. Connection tags filter series events (`ShouldHandleSeries`); health events ignore tags, as upstream. Deliveries run in the background after the action that caused them. A failing connection is logged and never stops the others.

- **Texts** are Sonarr's: titles ("Episode Grabbed", "Episode Downloaded", ...), and messages built by `GetMessage` (`Title - 1x01x02 - First + Second [Quality Proper]`, anime `v2`, daily by air date).
- **Webhook payloads** keep Sonarr's shape (`eventType` `Grab`/`Download`/`SeriesAdd`/`SeriesDelete`/`Health`/`HealthRestored`/`Test`, camelCase properties, the series with its ids and tag labels), so existing receivers and Notifiarr work unchanged. The webhook can use POST or PUT, basic authentication and custom headers.
- **Discord** builds upstream's embed: colours, title, description, the grab and import field choices, the poster as thumbnail and the fanart as image.
- **Media servers** refresh the series folder after imports and deletions, mapped with `mapFrom`/`mapTo` (or Tanukarr's Jellyfin path prefixes):
  - Emby/Jellyfin: `POST /Library/Media/Updated`, plus admin notifications when asked;
  - Plex: the TV sections that hold the folder;
  - Kodi: `VideoLibrary.Scan` of the folder, plus `GUI.ShowNotification` and an optional clean.
- **Health notifications** compare each evaluation with the previous one, and send issues that appeared and issues that cleared. The first evaluation after start-up only records the baseline, so restarts do not repeat known issues; upstream notifies on start-up after a grace period.

Not ported yet: the rename, episode-file-deleted, manual-interaction and application-update events; the Email, Custom Script, Twitter, Trakt, Synology, Mailgun, SendGrid and Pushcut services; Tanukarr's Jellyfin watched-state sync. Migrated connections of those types stay stored and are reported as not implemented.

## 25. Library file operations: disk scans, renames, deletions

`kitsunarr-library::files` ports `DiskScanService`, `RenameEpisodeFileService` and `MediaFileDeletionService`:

- **Disk scan** (series page "Scan disk", the `RescanFolders` task every 12 hours):
  - file records whose file is gone are removed, with "episodeFileDeleted" history (reason `MissingFromDisk`);
  - video files in the series folder that the library does not know are matched by their names and recorded where they are (`newDownload: false`: nothing is moved and no history is written), unless an episode already has a file;
  - sizes are updated.

  As upstream, a series whose root folder is missing or empty is skipped, so a disconnected drive does not empty the library. Records only are removed, never files, and a later scan adds the files back once they are reachable again. The exclusion rules for extras and system folders are the import's (`FilterPaths`).
- **Renaming** previews every file whose path differs from what the naming settings give, with the same naming engine imports use (custom formats evaluated on the file), then renames the chosen files. Destinations stay inside the series folder and are never overwritten. Each rename writes "episodeFileRenamed" history (source and new path), and an emptied season folder is removed.
- **Deleting** a file sends it to the recycle bin (or deletes it when none is set), with "episodeFileDeleted" history (reason `Manual`). Removing a series can also delete its folder, in the same way. The folder is deleted only when it lies inside its root folder.

## 26. Multi-season packs: Tanukarr's rules

Kitsunarr follows Tanukarr here, not upstream Sonarr. Upstream rejects multi-season packs from automatic grabs and imports every season of a pack grabbed by hand.

- **Accepted** (`MultiSeasonSpecification`) when Settings › Indexers › Options "Multi-season releases" is on (default; migrated from Tanukarr's `enablemultiseasonreleases`) and the pack maps to episodes.
- **In a season search**, a pack that contains the season stands for that season's episodes only; otherwise it maps to every season it names.
- **Ranking** puts the release scope first (`CompareReleaseScope`): a multi-season pack beats a season pack, which beats episodes, before quality. As in Tanukarr, a lower-quality pack can therefore outrank a better single episode.
- **RSS** does not upgrade complete seasons (`CompleteSeasonUpgradeSpecification`): a feed release whose seasons have no monitored, aired episode without a file is rejected. Searches keep the upgrade rules.
- **Import** takes only the grabbed episodes; files of other seasons are refused one by one. The pack is copied, never moved (Usenet included), and it is never removed from the client, so its other seasons stay intact.

The parser keeps upstream's season-range rule (`S01 S04` → seasons 1–4), which passes Sonarr's parser fixtures; Tanukarr's narrower rule (`S01 S04` → 1 and 4) is not ported.

### Complete-series search

The series page's "Complete series" button runs Tanukarr's complete-series search interactively (`GET /releases?seriesId=…&completeSeries=true`; automatic grabs through `POST /releases/search` take the same flag):

- **Indexers:** torrent indexers only, as Tanukarr's `Dispatch`.
- **Requests:** an ID search for the series, then text queries for its first three clean titles, `Title S01-Sxx` and `Title complete`, and (interactive) one `Title Sxx` per season.
- **Reading releases:** only `CompleteSeriesReleaseParser` reads the results. The release must start with one of the series' titles (with its year, when it gives one), then name a complete run:
  - a "complete" label;
  - seasons or season ranges, which must be seasons of the series;
  - for anime, an absolute range covering every episode;
  - for a single-season show, only quality tokens.

  Other releases are left out, and releases whose ids name another series are rejected. Automatic searches need the whole series; interactive ones also list packs of some of its seasons.

Not ported: collection titles (anime groups do not exist yet), and the automatic complete-series attempt that Tanukarr makes before series and missing searches.

## 27. Fake release protection and pack trimming (Tanukarr)

- **Fake releases** (`FakeReleaseInspector`, default on): a release title ending in an executable extension is refused. A torrent with a `.torrent` link is fetched first, even for clients that prefer magnets, and its file list (BEP-47 padding ignored) is refused when it holds:
  - a disguised executable (`.mkv.exe`);
  - any executable or script;
  - archives with no video, next to a "password"/"how to open" file;
  - neither video of at least 10 MiB nor archives.

  Unreadable file lists and magnet-only releases pass, with a warning. A rejected grab moves an automatic search on to the next release; nothing is stored, as upstream.
- **Pack trimming** (Media management, default off): a file whose episodes are all unmonitored is not imported from a download. Tanukarr still expects those episodes, so a trimmed pack stays "import blocked" forever. Kitsunarr expects only the monitored grabbed episodes, so the download completes.

## 28. Jellyfin watched sync (Tanukarr)

Ported from Tanukarr's `JellyfinWatchedService`, `JellyfinWatchProxy` and `JellyfinWatchPath`.
A Jellyfin connection (Settings › Connections) with `enableWatchedSync` reads, for one user
(`watchedUserId`) and one library (`libraryId`), the played state of every episode and film.
The scheduled task `SyncJellyfinWatched` runs every minute and syncs when the connection's
`watchedSyncIntervalMinutes` (default 5) has passed or its target changed.

- **All or nothing.** The user must exist, the library must exist and be visible to the user,
  every page must agree on the total (at most 100 000 items, 200 per page), ids must be unique
  and every item must have a valid path and a played flag. Any failure keeps the previous state.
  Two Jellyfin items with the same translated path and different played states reject the
  snapshot. Nothing is written until the whole snapshot is checked, then all updates run in one
  transaction.
- **Paths.** Jellyfin paths are normalised (`.`/`..` segments rejected; Windows paths compared
  without case) and translated from `jellyfinPathPrefix` to `kitsunarrPathPrefix` (Tanukarr's
  `tanukarrPathPrefix` is read too) on directory boundaries. They are matched against
  `series.path/relative_path` of files in the connection's tag scope; paths shared by two files
  are ignored. Items Jellyfin does not list never mark anything unwatched.
- **Ownership.** The source is a SHA-256 fingerprint of the connection id, host, port, TLS,
  URL base, library, user and prefixes. With `unmonitorWatchedEpisodes`, an episode that becomes
  watched is unmonitored and the fingerprint recorded as its owner (on the first sync, or when
  the target or the option changed, already-watched episodes are unmonitored too). With
  `remonitorUnwatchedEpisodes`, an episode marked unwatched again is monitored only if this
  sync unmonitored it and its series and season are monitored. Any manual monitoring edit clears
  the owner. The update is one SQL statement against the current row, so a manual change made
  while the snapshot was read is not overwritten.
- **Not ported:** Tanukarr's user and library pickers (the ids are typed), list progress sync
  and the Jellyfin refresh queue.
