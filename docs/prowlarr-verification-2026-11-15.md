# Real Prowlarr integration verification, 15 November 2026

The configured Prowlarr endpoint at `10.10.0.250:30050` refuses TCP connections. Kitsunarr's
saved-indexer connection test confirms the same failure. That service was not changed.

A separate **Prowlarr 2.7.0.5723** container was created with rootless Podman 6.1.4 using
`lscr.io/linuxserver/prowlarr:latest` (LinuxServer build `2.7.0.5723-ls168`, image digest
`sha256:6660787a99b2e58203539583663c973b48979fc00ff2cb78dc0ea17429e1f6ef`). This was a real
Prowlarr process talking to live Nyaa, rather than a stand-in or recorded response.

## Observed bug and fix

Prowlarr's application test initially returned **Failed to fetch Sonarr version**.
Its [`SonarrV3Proxy.TestConnection`](https://github.com/Prowlarr/Prowlarr/blob/develop/src/NzbDrone.Core/Applications/Sonarr/SonarrV3Proxy.cs)
reads `X-Application-Version` from the response to `POST /api/v3/indexer/test`. Reading
`version` from the status JSON is insufficient for this handshake.

The compatibility router now returns `X-Application-Version: 4.0.0.0`, matching its existing
compatibility version. The layer covers successful, validation-error and unauthorized v3
responses, including routes merged from the additional compatibility modules. Native v1
responses keep their own identity and do not receive the Sonarr compatibility header.
`appName` remains `Kitsunarr`; `kitsunarrVersion` still carries the native version.

## Results

| Check | Result |
|---|---|
| Original configured Prowlarr connection | Failed: connection refused |
| Local Prowlarr Nyaa indexer connection | Passed against live Nyaa |
| Prowlarr Sonarr application test against Kitsunarr | Passed after the header fix |
| Prowlarr application/indexer full sync | Completed; created `Nyaa.si (Prowlarr)` as Torznab |
| Kitsunarr test of that synced indexer | Passed: **150 releases**, no warnings |
| Deliberately invalid Prowlarr API key | Correctly rejected with HTTP 401 |
| Kitsunarr series search through Prowlarr | **7 releases / 5 matching**, one responding source, no source failures |
| Search title and elapsed time | **Cowboy Bebop**, season 1, 4.39 seconds |
| Force resync after changing the test profile | Completed; same indexer id, one record, updated switches |
| Download or grab action | None invoked |

After verification, the isolated Prowlarr application profile was set to interactive search
only, with RSS and automatic searches off. A forced resync propagated those switches without
creating another indexer. The main library stayed at **80 series, 14 movies and 8 indexers**;
its imported provider configurations were not rewritten.

## Local services and artifacts

- Main Kitsunarr: `http://127.0.0.1:8990`, updated with the compatibility fix.
- Test Prowlarr: `http://127.0.0.1:9696`, container `kitsunarr-prowlarr-test`.
- Isolated Kitsunarr used for application sync: `http://127.0.0.1:8991`.
- Prowlarr configuration: `.dev/prowlarr/config`.
- Isolated Kitsunarr configuration/database: `.dev/prowlarr/kitsunarr-test`.
- Sanitized results and logs: `.dev/verification/prowlarr-*`.

Prowlarr uses host networking to reach the host's loopback-only Kitsunarr test service;
its own `BindAddress` is set to `127.0.0.1`. It is constrained to two CPUs and 1 GiB of memory.
Configuration and API keys remain in ignored local directories. Both test services are left
available for inspection. To stop Prowlarr, run `podman stop kitsunarr-prowlarr-test`; restart
it with `podman start kitsunarr-prowlarr-test`.

## Regression checks

- New authenticated HTTP regression test: the indexer-test probe returns the version header
  on HTTP 200 and 400, an unauthenticated probe returns 401 with the same header, and native
  v1 status retains its own identity without that header.
- `cargo test --workspace`: **818 passed, 0 failed, 9 ignored**.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`, `git diff --check` and native server build: passed.
- Main and isolated servers return HTTP 200 and the expected compatibility header after
  restart. Chromium and the WebAssembly build were not rerun for this backend-header change.
  Remote CI and a new Kitsunarr container image build were not verified here.
