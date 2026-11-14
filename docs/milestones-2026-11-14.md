# Development cycle: ten milestones, 14 November 2026

Starting point: commit `d8476e9`, recorded locally as `baseline-2026-11-14-d8476e9`.

| # | Milestone | Implementation and evidence |
|---|---|---|
| 1 | Complete Notifications description | Twitter, Pushcut, Synology Indexer and Trakt included in the English sentence and all 43 translated dictionaries. |
| 2 | Series catalogue preview | Read-only target lookup, identity verification, episode associations, explicit matches, duplicate detection and revision for relevant library and catalogue state. |
| 3 | Series catalogue change | One transaction preserves episode IDs, monitoring, file and history links, user settings, collections and old external identities; clears obsolete TheXEM season aliases. Retained episodes cannot be overwritten by later refreshes. Library tests on catalogue mocks. |
| 4 | Film catalogue preview | Verifies target media type and identities, requires identity confirmation when needed, checks conflicts and returns a review revision. |
| 5 | Film catalogue change | Atomically changes owner and metadata while preserving the file, path, settings and external identities. Library and HTTP tests on catalogue mocks. |
| 6 | Series catalogue dialog | Search or catalogue ID, preview, manual episode associations with target titles, paginated matches and apply. Browser preview covered by e2e. |
| 7 | Film catalogue dialog | Same search, identity confirmation and preview flow on film pages. Typed client and authenticated HTTP routes for both media types. |
| 8 | Atomic episode ordering and refresh | Reuses the fetched catalogue snapshot inside the renumbering transaction. Failures roll back numbering and ordering together; linked-file conflicts are refused. Refreshes reject obsolete catalogue ownership. |
| 9 | Deluge incomplete-record recovery | One reconnection and retry for incomplete records; parse errors are reported. Existing and new JSON-RPC mock tests. |
| 10 | Responsive layout, regression checks and local instance | Real Chromium zoom from 50% through 400% on 15 pages and the catalogue dialog, plus 20 px text. Fixed narrow series layout, form controls, dialog body scrolling and footer visibility, and the desktop navigation toggle. Format, host and wasm Clippy, workspace tests, UI build, browser e2e and rebuilt local server; results recorded below after completion. README and feature matrix updated. |

## Catalogue changes

Open a series or film and choose **Change catalogue**. Choose an available catalogue and search for the target title, or enter its native ID. **Preview** shows its title and identity match. Without a shared external ID, explicitly confirm that it is the same title and preview again.

For series, expand **Episode matches**. An ambiguous episode can be associated with a target episode using **Match**. Preview again after editing. A number occupied by an unmatched episode blocks the change; unmatched episodes with free numbers remain in the library. If future catalogue data conflicts with one of those retained numbers, refresh stops and the dialog can be used again to resolve the association.

**Change catalogue** becomes available after a successful preview. It refreshes metadata with the new catalogue's default episode order. Paths, files and user settings stay in place. This changes ownership within the existing series or film; it does not split or combine library titles.

## Verification

Completed locally:

- `cargo fmt --all -- --check` and `git diff --check` passed.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.
- `cargo clippy -p kitsunarr-web --target wasm32-unknown-unknown -- -D warnings` passed.
- `cargo test --workspace`: **769 passed, 0 failed, 9 ignored**. The ignored browser scenario was then run explicitly and passed; the other ignored checks were not run.
- `dx build -p kitsunarr-web --release --profile web-release` and `cargo build -p kitsunarr-server` passed.
- Browser e2e (headless Chromium): **1 passed**, including setup, migration, library operations, catalogue preview, searches, Italian localisation, **150 page views at real browser zoom**, **10 catalogue dialogs**, and four pages with 20 px default text. The final scenario completed in 160 seconds.
- The Notifications description was checked for all four service names in English and all 43 translated dictionaries.

Catalogue, Deluge and HTTP tests use local mocks; no claim of live-account verification or remote CI is made.

## Browser zoom results

Chromium window: 1366 × 900. Zoom is changed through Chromium's own settings API, with device pixel ratio checked after navigation. These are real zoom settings, rather than resized-window approximations. Page layout and modal footer bounds are checked separately; dialog bodies must have room for at least two text lines and scroll internally.

The first run found sideways overflow on the series page at 400% (422 CSS pixels of content in 337 available) and a dialog extending below the viewport. The final layout keeps a small poster beside the title, constrains form controls, wraps season actions and keeps the dialog header and footer visible while its body scrolls. The navigation toggle appears at the mobile breakpoint and stays hidden on desktop.

| Zoom | CSS viewport | Pages without sideways overflow | Dialog body and footer |
|---|---|---|---|

| 50% | 2732 × 1514 | 15 / 15 | Passed |
| 67% | 2038 × 1129 | 15 / 15 | Passed |
| 80% | 1707 × 946 | 15 / 15 | Passed |
| 100% | 1366 × 757 | 15 / 15 | Passed |
| 125% | 1093 × 606 | 15 / 15 | Passed |
| 150% | 911 × 505 | 15 / 15 | Passed |
| 200% | 683 × 378 | 15 / 15 | Passed |
| 250% | 546 × 303 | 15 / 15 | Passed |
| 300% | 455 × 252 | 15 / 15 | Passed |
| 400% | 341 × 189 | 15 / 15 | Passed |

Raw logs, viewport measurements and screenshots are retained locally under `.dev/verification/` (ignored by Git). The final screenshots are in `zoom-final/`; `zoom-results.json` records every measured case. Reproduce with:

```sh
KITSUNARR_E2E_ZOOM_TOUR=1 \
KITSUNARR_E2E_SCREENSHOTS="$PWD/.dev/verification/zoom-final" \
cargo test -p kitsunarr-e2e -- --ignored --nocapture
```
