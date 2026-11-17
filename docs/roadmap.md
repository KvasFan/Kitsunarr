# Development roadmap

Milestones 1–50 are recorded in the dated milestone reports. The current search follow-up
closes milestones 55–60; verification is recorded in the [checkpoint report](milestones-2026-11-16-60.md).

A maintenance follow-up fixes invalid negated custom-format regexes affecting release
scores and naming. See the [regression report](custom-format-regex-2026-11-16.md).

The development follow-up covers reliability, guided configuration and anime search usability:

| Milestone | Scope | Status |
|---|---|---|
| 61 | Invalid negated regexes cannot grant format scores or alter filenames | Done: matching, decision and filesystem regressions |
| 62 | Preserve migrated condition fields when switching editors and editing their values | Done: db and Chromium editor checks |
| 63 | Guided editing of all eight custom-format condition types, retaining JSON mode | Done: Chromium editor flow |
| 64 | Test unsaved custom formats against TV, anime and movie release names | Done: HTTP and Chromium preview |
| 65 | Filter interactive release results by subgroup, text and acceptance/mapping status | Done: Chromium filter flow |
| 66 | Verify the full workflow, narrow/zoomed layouts and packaged runtime | Done: 848 Rust tests, Chromium at 50–400% zoom, release container |

Implementation and verification: [milestones 61–66](milestones-2026-11-16-66.md).

Monitoring choices and library import, after comparing the remaining gaps with Sonarr:

| Milestone | Scope | Status |
|---|---|---|
| 67 | All of Sonarr's monitoring choices when adding, with `EpisodeMonitoredService` season rules | Done: 10 ported fixture cases, HTTP |
| 68 | Apply monitoring choices to existing series: series dialog, bulk editor, v3 season pass | Done: HTTP and Chromium |
| 69 | Unmapped root-folder folders and adding titles with their existing folders (native and v3) | Done: HTTP |
| 70 | Library import page with catalogue matching by name, year or id tag | Done: Chromium |
| 71 | Verify the workflow, zoomed layouts and the clean-clone CI jobs | Done: 864 Rust tests, Chromium |

Implementation and verification: [milestones 67–71](milestones-2026-11-16-71.md).

Moving title folders, the next Sonarr gap:

| Milestone | Scope | Status |
|---|---|---|
| 72 | Move a series or film folder, or record a folder moved by hand (`MoveSeriesService`) | Done: HTTP and library tests |
| 73 | Bulk moves to another root folder, keeping group layouts | Done: HTTP |
| 74 | v3 path edit with `moveFiles` and Sonarr's series editor | Done: HTTP |
| 75 | Move folder dialogs and the bulk "Move to root folder…" action | Done: Chromium |
| 76 | Verify moves across file systems, refusals and the clean-clone CI jobs | Done |

Implementation and verification: [milestones 72–76](milestones-2026-11-16-76.md).

Removal and deletion behaviour:

| Milestone | Scope | Status |
|---|---|---|
| 77 | "Unmonitor deleted episodes" for manual deletions and files missing from disk | Done: HTTP, migration |
| 78 | Import list exclusions when removing a series | Done: HTTP |
| 79 | Bulk removal (native, v3 series editor, library bar) | Done: HTTP and Chromium |
| 80 | Verify the workflow and the clean-clone CI jobs | Done: 870 Rust tests, Chromium |

Implementation and verification: [milestones 77–80](milestones-2026-11-16-80.md).

| Milestone | Scope | Status |
|---|---|---|
| 81 | Organize several series at once (native bulk rename, library action with confirmation) | Done: HTTP |
| 82 | v3 `RenameFiles`/`RenameSeries` commands and multi-series refresh/rescan | Done: HTTP |

Both reuse the single-series rename, its previews' naming engine and its `OnRename`
notification. A failing series is reported and does not stop the others. The library's
Organize button asks for confirmation first; it was checked by compilation and lint only.
The HTTP test covers the routes.

Sonarr API completeness and transient-state migration:

| Milestone | Scope | Status |
|---|---|---|
| 83 | v3 `config/naming`, `mediamanagement`, `indexer`, `downloadclient`, `importlist`, `ui`, `host` (partial writes through the native validation) | Done: HTTP, unit |
| 84 | v3 tags (`tag/{id}`, `tag/detail`), root folders, delay profiles, remote path mappings, import lists and exclusions | Done: HTTP, unit |
| 85 | "Send now" for pending releases (native `POST /queue/{id}/grab`, queue button, v3 `queue/grab`), v3 `queue/status`, `queue/details`, `system/task`, `system/backup`, `log`, `rename` | Done: HTTP, decision |
| 86 | Sonarr/Tanukarr blocklist migration (backups and API) | Done: migrate fixtures, unit |
| 87 | v3 `manualimport` and the `DownloadedEpisodesScan`, `RefreshMonitoredDownloads` and `ManualImport` commands (post-processing scripts) | Done: HTTP |
| 88 | v3 commands tracked to their end (`GET /command`, `GET /command/{id}`: started, then completed or failed with the outcome) | Done: HTTP, unit |
| 89 | Anime season searches also search each episode (Sonarr's `SearchAnimeSeason`); Tanukarr's "Prefer anime season packs" skips the episodes an approved pack covers | Done: HTTP |
| 90 | Real Recyclarr 7.5.2 and 8.8.1 syncs; fixes: null quality sizes are no limit, v3 status answers as Sonarr | Done: real containers, HTTP |
| 91 | Browser timings with 1,000 series (issue #12) | Done: Chromium benchmark |
| 92 | linux/arm64 image and aarch64 binary (issue #12): cross-compiled server, multi-architecture release | Done: Podman build, QEMU smoke test |

Details: [milestones 83–89](milestones-2026-11-17-89.md), the [Recyclarr verification](recyclarr-verification-2026-11-17.md) (90) and the [browser timings](performance.md#browser-at-1000-series) (91).

| Milestone | Scope | Status |
|---|---|---|
| 51 | Native Nyaa season searches without season aliases; standard and daily query coverage | Done: generator and HTTP tests |
| 52 | Distinguish unavailable sources from empty results; retry the same search explicitly | Done: HTTP and Chromium checks |
| 53 | HTTP and browser regressions for direct Nyaa, partial failures and repeated searches | Done: 817 workspace tests and Chromium passed |
| 54 | Verify live Nyaa searches and deploy the fixes to the existing local instance | Done: live search and local health verified |
| 55 | Let an interactive search select individual enabled indexers | Done: native series/movie APIs and browser source controls |
| 56 | Show useful per-source query diagnostics without exposing credentials | Done: bounded, redacted plans and per-source outcomes |
| 57 | Cancel superseded searches and bound total multi-query search duration | Done: source/request budgets, session supersession and stale-result protection |
| 58 | Investigate bounded older-result retrieval beyond Nyaa's first RSS page | Done: upstream RSS limit verified; visible diagnostic documents the result |
| 59 | Improve alternate-title/season-name diagnostics when releases do not map | Done: considered titles, season alias filtering and unmatched parsed titles |
| 60 | Verify the next search improvements across TV, anime and movie workflows | Done: 840 Rust tests, full Chromium workflow, release container and local verification |

## Diagnosis at milestone 54

The local instance's imported Torznab indexers point at an unavailable Prowlarr service.
Connection-refused errors must remain visible as source failures, rather than appearing as
an ordinary search with no matches. A native Nyaa connection was added to the local instance
for interactive searches; its RSS and automatic-search switches are off. Existing imported
indexer configurations are preserved.

Live requests to Nyaa return RSS successfully. Nyaa accepts both current `q/c/f` parameters
and legacy `term/cats/filter` aliases; the legacy parameter names are not the cause of this
failure. This is also documented by Nyaa's own
[`views/main.py`](https://github.com/nyaadevs/nyaa/blob/master/nyaa/views/main.py).

The native adapter also silently generated no request for an ordinary anime season without
season-specific aliases unless standard-format search was enabled. Milestone 51 adds a
bare-title fallback; the decision engine still maps/rejects each returned release. It also
keeps standard TV numbering independent of the anime-format option.

Live verification is limited to searches and indexer connectivity. It does not grab releases
or send work to download clients. RSS only exposes a limited recent result set; retrieving
older results was investigated in milestone 58; Nyaa ignores RSS pagination, so the limitation
is now explained in source diagnostics. See the [investigation](milestones-2026-11-16-60.md#nyaa-older-result-investigation).

## Live local check, 15 November 2026

The reported title, **Attack on Titan Season 3 Part 2**, returned **291 releases** from the
direct Nyaa source, including **27 matching releases**, in one 26.66-second search across
its aliases. Three Prowlarr-backed sources still reported connection-refused errors. Library
counts stayed at 80 series and 14 movies. Searches did not invoke a download/grab action.

All matching releases were rejected by the existing custom-format minimum: score 0 versus
minimum 500. Search retrieval and automatic acceptance are separate; that profile setting
was preserved. See the per-release rejection messages before choosing a release or changing
a profile.

The interactive search API now reports `successfulIndexers` alongside attempted sources,
so a valid empty feed, partial failure and total source failure produce distinct UI states.
Retry controls issue fresh requests; configured source back-off and rate limits still apply.

Verification and implementation notes: [milestones 51–54](milestones-2026-11-15-54.md).

## Prowlarr integration follow-up

A real local Prowlarr 2.7.0.5723 container exposed a missing compatibility response header.
The fix was verified with Prowlarr's application test, full indexer sync, forced resync,
invalid-key rejection and a live series search through its Nyaa indexer. The original remote
Prowlarr remains unreachable. See the [integration verification](prowlarr-verification-2026-11-15.md).

## Download-client integration follow-up

Real qBittorrent 5.3.0, Transmission 4.1.4, Deluge 2.2.0, SABnzbd 5.1.3 and NZBGet 26.3
containers passed connection, credential rejection, paused queue/removal and local generated
data-transfer checks. Native queue handling and v3 connection probes also passed. A Transmission
paused-state classification bug was fixed and deployed locally. See the
[verification report](download-client-verification-2026-11-15.md) for scope and repeatable tests.

## Quality-profile verification follow-up

Kitsunarr matches 5,376 scenarios executed from Tanukarr's original C# profile and upgrade
algorithms. This audit adds stricter profile validation, Tanukarr size/codec fields in the
v3 profile API, preference-order controls and the minimum upgrade increment in the browser.
Null size overrides are explicitly unlimited; a separate control restores global sizes.
The local Seasonal profile's minimum of 500 is reachable and was preserved. See the
[verification report](quality-profile-verification-2026-11-15.md) for tests and remaining scope.

## Media-renaming verification follow-up

The compiled Tanukarr naming engine agrees on 302 filename cases and 72 custom-format regex
cases after fixing the reduced movie renderer. Movie imports, manual imports and explicit
renames now use the same token engine as episode naming, including regex-matched custom
formats. Naming validation accepts literal ellipses while rejecting parent path components.
See the [verification report](media-rename-verification-2026-11-15.md) for filesystem and API
checks. Naming formats remain token templates; arbitrary regex search/replacement is not a
Tanukarr naming setting.

## Quality-group editor follow-up

Quality profiles now support browser drag/drop ordering of qualities and groups, group
creation/renaming, moving members between groups, extracting members and ungrouping.
Arrows and selectors remain available as alternatives to dragging. Shared domain operations
preserve the quality catalogue and settings, repair cutoff references and block invalid drafts.
See the [editor verification](quality-group-editor-2026-11-15.md) for behavior and tests.
