# Real download-client containers, 15 November 2026

Five real clients were started with rootless Podman and tested through Kitsunarr's production
adapters and HTTP API. Their configuration and generated data are isolated under
`.dev/download-clients`. Provider records were added only to the disposable Kitsunarr instance
at `http://127.0.0.1:8991`; the main library/configuration was preserved.

| Client | Container version | Local UI | Connection, bad credentials, paused job and removal | Generated-data transfer and SHA-256 | Native queue API and v3 connection test |
|---|---|---|---|---|---|
| qBittorrent | 5.3.0, Web API 2.16.2 | http://127.0.0.1:8088 | Passed | Passed | Passed |
| Transmission | 4.1.4 | http://127.0.0.1:9091/transmission/web/ | Passed | Passed | Passed |
| Deluge | 2.2.0 | http://127.0.0.1:8112 | Passed | Passed | Passed |
| SABnzbd | 5.1.3 | http://127.0.0.1:8085 | Passed | Passed | Passed |
| NZBGet | 26.3 | http://127.0.0.1:6789 | Passed | Passed | Passed |

The images are the LinuxServer `lscr.io/linuxserver/{client}:latest` images pulled for this
run. The observed builds are `5.3.0_v2.1.2-ls487`, `4.1.4-r0-ls369`, `2.2.0-ls387`,
`5.1.3-ls281` and `v26.3-ls272`, respectively. Each client is limited to two CPUs and
512–768 MiB of memory. RPC, UI and peer listeners bind to loopback; host networking lets the
clients reach the local fixture source. SABnzbd's service launcher has a test-only bind override
because the stock launcher forces a wildcard bind.

## What was exercised

- Valid and invalid credentials for all five adapters, and destination-folder discovery.
- Submission through `Clients::add` / `Clients::add_nzb`, recognition as paused, queue reads
  and removal through `Clients::remove`.
- Fresh transfers of a generated **2,097,152-byte** file. The torrent clients use an owned local
  tracker/web seed; SABnzbd and NZBGet use an owned NNTP fixture serving the generated yEnc
  article. No public swarm, Usenet account or external media was used.
- Completion detection and SHA-256 of the exact bytes at each client's reported output path:
  `05a0fbcb081fe854c4ab0e561f60fd13fdaa42502787ef4920033558ab8831db`.
- Post-import category/label changes for the three torrent clients, then job removal.
- All five jobs appearing with their correct client and **paused** state in native
  `GET /api/v1/queue`, then removal through the native queue deletion endpoint.
- All five saved providers passing `POST /api/v3/downloadclient/test`, including use of their
  redacted stored credentials in the request.

Final isolated queue: **5 readable clients, 0 jobs, 0 client failures**. The main instance
remained at **80 series, 14 movies, 8 indexers and its original 1 download client**. No real
library files or production client configurations were changed.

## Findings and fix

The real Transmission check exposed a status-mapping bug: an incomplete stopped torrent was
reported as downloading. The adapter now reports **paused**, while stopped complete torrents
stay **completed**. A regression test covers both cases and an actively downloading torrent.
The updated backend is running on the main local instance at `http://127.0.0.1:8990`.

Two authentication checks also caught test-container defaults: LinuxServer's Transmission
launcher disables authentication unless `USER`/`PASS` are supplied, and NZBGet's default
`AuthorizedIP=127.0.0.1` bypasses authentication locally. The isolated configurations now
require credentials, and all five reject incorrect credentials. Deluge's Label plugin is enabled
for category tests. NZBGet duplicate checking is disabled only in this repeatable fixture,
because repeatedly submitting the same generated NZB otherwise produces a duplicate-copy
failure. Existing production settings are untouched.

SABnzbd can rename the output file. Integrity checks follow the job's reported destination
and verify its contents instead of assuming an unchanged filename. Each fresh transfer removes
only prior generated proof byte files within the dedicated sandbox. Torrent metadata is private,
with DHT, PEX and local discovery disabled. HTTP web seeds allow the transfer test to stay on
loopback without depending on public peer discovery.

## Repeatable checks

The source server and live clients remain available locally. Private credentials/settings are
in `.dev/download-clients/clients.json`, which is ignored by Git. The file defines `sandbox`,
`clients` (implementation, settings, downloadsPath) and `api` (baseUrl, apiKey). The API target
is restricted to the disposable `127.0.0.1:8991` instance. No credentials are checked in.

Start the source fixture if it is not already running:

```sh
python3 tests/fixtures/generate/download_clients.py
```

Run all three opt-in scenarios:

```sh
KITSUNARR_LIVE_CLIENT_CONFIG="$PWD/.dev/download-clients/clients.json" \
  cargo test -p kitsunarr-downloaders --test live_containers -- --ignored --nocapture
```

The live tests serialize access to their owned jobs. The checks are ignored in an ordinary
workspace test run because they require the local containers and private settings file.
The fixtures include only generated bytes, a private `.torrent`, one NZB/yEnc article and a
loopback source server. Test reports are under `.dev/download-clients/*-results.json` and
`.dev/verification/download-clients-*`, including a sanitized combined JSON report.

Container names are `kitsunarr-qbittorrent-test`, `kitsunarr-transmission-test`,
`kitsunarr-deluge-test`, `kitsunarr-sabnzbd-test` and `kitsunarr-nzbget-test`. They can be
stopped with `podman stop <name>` and started again with `podman start <name>`.

## Verification and scope

- Live-container scenarios: **3 passed, 0 failed**, 9.59 seconds in the final combined run.
- `cargo test --workspace`: **819 passed, 0 failed, 12 ignored**. The three live scenarios were
  run explicitly as above; the other ignored checks were not run in this cycle.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`, `git diff --check`, source-fixture syntax and native server
  build: passed. Main and isolated HTTP health checks: passed.
- These checks verify client APIs, queue handling and local data transport. Media import,
  remote path mappings, external peers/providers and paid Usenet accounts were not exercised.
  Chromium, WebAssembly and a new Kitsunarr image build were not rerun for this backend change.
