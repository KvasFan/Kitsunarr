# Recyclarr verification (2026-11-17)

Real Recyclarr containers synced TRaSH guide settings into a local Kitsunarr instance through
the Sonarr `/api/v3` compatibility layer. Before this, the Recyclarr flows were only simulated
by HTTP tests.

## Setup

- Kitsunarr from `main` plus the fixes below, debug build, listening on `127.0.0.1:18991` with
  a fresh data directory; catalogue URLs pointed at a closed port.
- `ghcr.io/recyclarr/recyclarr:7` (7.5.2) and `ghcr.io/recyclarr/recyclarr:8` (8.8.1), run
  rootless with Podman (`--network host --userns keep-id`). Recyclarr fetched the TRaSH
  guides and config templates from GitHub.
- Recyclarr 7: a hand-written config with media naming (TRaSH defaults, renaming on), the
  `series` quality sizes, a "WEB-1080p" profile with a "WEB 1080p" group and four TRaSH
  custom formats (BR-DISK, LQ, x265 (HD), Repack/Proper) scored.
- Recyclarr 8: the official `sonarr/templates/anime-remux-1080p.yml` template: the `anime`
  quality sizes, the "[Anime] Remux-1080p" profile and its 57 custom formats.

## Results

| Run | Outcome |
|---|---|
| Recyclarr 7, first sync | 4 custom formats created, 1 profile created, 14 sizes synced, media naming updated |
| Recyclarr 7, second sync | Nothing to change (after fix 1) |
| Recyclarr 8, first sync | Refused before fix 2; after it: 57 custom formats created, 20 sizes synced, 1 profile created |
| Recyclarr 8, second sync | Nothing to change |

Checked in Kitsunarr afterwards: the TRaSH episode, series-folder and season-folder formats
with renaming on; the profiles' cutoffs, upgrade settings, minimum and cutoff format scores,
qualities and groups; 50 scored formats in the anime profile (for example the Anime BD tiers
1400, 1300, 1200…); unlimited maximum and preferred sizes where the guide has none.

## Problems found and fixed

1. **Quality sizes were synced on every run.** Recyclarr leaves out null sizes (TRaSH's
   unlimited maximums); Kitsunarr kept the stored maximum for a size that was left out, so the
   next sync always found a difference. As in Sonarr, a `QualityDefinitionResource` now
   replaces all three sizes, and a missing or null size is no limit. Regression check: HTTP
   `sonarr_v3_settings_serve_recyclarr_sync`.
2. **Recyclarr 8 refused Kitsunarr.** It checks that `system/status` names Sonarr ("the
   service is Kitsunarr"). The v3 status now answers `appName: "Sonarr"` and names Kitsunarr in
   `productName`/`productVersion`, as Tanukarr does; the native `/api/v1/system/status` still
   says Kitsunarr. Kitsunarr's migration refuses a source whose `productName` is Kitsunarr.
   Checks: HTTP `sonarr_v3_api_serves_overseerr_and_prowlarr_flows`, migrate
   `remote::tests::detection`.

## Not covered

Recyclarr's release profiles (Sonarr v3 only) and Radarr; Overseerr, LunaSea, nzb360 and
Homepage were not run against Kitsunarr.
