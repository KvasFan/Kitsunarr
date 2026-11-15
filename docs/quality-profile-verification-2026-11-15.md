# Quality profile verification — 15 November 2026

Compared Kitsunarr with the local Tanukarr checkout at
`24382f1f7f23ca13d991e5416d257f631f606529`. The reference checkout was clean and
was mounted read-only. No source application or user profile was modified.

## Comparison and fixes

The original C# `QualityProfile`, `QualityModelComparer`, `Revision` and
`UpgradableSpecification` were compiled and executed in the cached .NET 10 SDK
container, with networking disabled. Infrastructure stubs provide logging, configuration
and identity objects; the algorithms under comparison are the original source files.
The resulting fixture records the reference revision and source-file SHA-256 hashes.

All **5,376 scenarios match** Kitsunarr: enabled/disabled upgrades, quality and group
cutoffs, lower/equal/higher qualities, revisions including REAL, all three proper policies,
negative and positive custom-format scores, score cutoff and minimum upgrade increment.
Grouped qualities rank equally. A same-quality revision upgrade is treated separately from
an ordinary upgrade, as in Tanukarr. This comparison exercises the algorithms; it does not
run a complete Tanukarr application or download media.

The audit found and fixed these surrounding gaps:

- Profile writes accepted impossible minimum scores and score increments below one.
  Validation now rejects these, duplicate score entries, nonexistent scored formats,
  missing standard qualities and invalid/duplicate quality groups. Requiring the quality
  catalogue also prevents an omitted quality inheriting the first item's allowed flag
  through Tanukarr's index-zero lookup fallback. Native profiles retain sparse scores:
  omitted custom-format scores contribute zero.
- The Sonarr-compatible profile API ignored Tanukarr's per-quality size fields and codec
  factors. Both now round-trip, including sizes on qualities inside groups. Omitted fields
  preserve existing values; explicit null size limits mean unlimited. Rejected HTTP writes
  preserve the previously stored profile.
- The browser editor lacked controls for the minimum score increment and preference order.
  It now exposes both, with controls moving a quality or an existing group up/down.
- The size editor described empty limits as global values even though an existing override
  makes null unlimited. Labels now distinguish those states, clearing the last limit keeps
  an explicit unlimited override, and **Use global** removes that quality's override.

Size checks cover profile overrides, null/zero unlimited maxima, global fallback and codec
normalization. Release-decision tests parse real title strings and verify that changing the
profile order changes ranking, equal-quality groups use format scores to break ties, and
the minimum score rejects otherwise matching releases. These are generated release inputs,
not a live indexer/download test.

## Verification

- Offline Tanukarr source oracle: **5,376 comparisons passed**.
- Workspace tests: **824 passed, 0 failed, 12 ignored**. The ignored scenarios include
  separately invoked browser and live-service tests.
- Native and v3 HTTP round-trip/validation regression passed on a disposable real server
  and SQLite database.
- Workspace clippy, WebAssembly clippy, formatting and whitespace checks passed.
- Release web build and server build passed.
- Full Chromium scenario: **1 passed in 176.96 seconds**. It changes/saves/reopens the
  preference order and minimum increment, clears all three size limits through keyboard
  input, confirms an unlimited override survives reopening, then restores global sizes and
  confirms that state survives too. The existing broader UI/zoom checks also passed.

The increment validation regression was first run against the previous implementation and
failed, then passed with the fix. An initial browser probe used WebDriver's `clear()` without
an input event; the final test uses actual keyboard deletion to exercise the user's action.

## Local instance

The existing instance was stopped when checked. After a SQLite backup it was restarted at
**http://127.0.0.1:8990** with the updated API and embedded web application. The API responds;
all **7 profiles** are identical to the pre-start database snapshot. Library counts remain
**80 series and 14 movies**.

The instance runs independently of the terminal under the transient user service
`kitsunarr-local-8990.service`. Inspect it with
`systemctl --user status kitsunarr-local-8990`; restart or stop it with the corresponding
`systemctl --user restart` or `stop` command. Logs remain in `.dev/instance/server.log`.
This starts the existing local instance; it does not install a boot-time service.

The imported **Seasonal** profile's minimum of **500** passes Tanukarr's numerical reachability
guard (positive assigned scores total 35,100). Its preferences reward particular release groups and
Italian-subtitle formats. A release matching none of them scores zero and is correctly
rejected. The earlier Nyaa search's score-zero rejections do not establish a profile bug;
the profile and its thresholds were preserved.

## Remaining scope

The browser handles existing quality groups and their order; creating, splitting or changing
group membership still uses the native/v3 profile API. This checkpoint does not claim full
Tanukarr editor parity, exhaustive custom-format matcher parity, or live import/upgrade and
download verification. Earlier download-client checks remain scoped to their
[own report](download-client-verification-2026-11-15.md).

To regenerate the reference oracle and run its portable Rust comparison, see the
[fixture instructions](../tests/fixtures/README.md).
