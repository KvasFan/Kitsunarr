# Regex and media-renaming verification — 15 November 2026

Compared the naming code with the local Tanukarr source checkout
`24382f1f7f23ca13d991e5416d257f631f606529`, and executed the unmodified compiled
`FileNameBuilder` and `ReleaseTitleSpecification` from the cached
`ghcr.io/kvasfan/tanukarr:0.1.5` image. The fixture records its image ID, assembly version
and SHA-256 hashes. The source checkout and application assemblies were read-only.

## What “regex renaming” means here

Tanukarr naming settings contain **token templates**, such as:

```text
{Series.Title} - S{season:00}E{episode:00} - {Episode Title} {[Custom Formats]}
```

Regexes interpret the token grammar and recognize custom formats. For example,
`(?<=\.)WEB-DL(?=\.)` can match a release/file name and add a custom format called `WEB`;
`{[Custom Format:WEB]}` then renders `[WEB]`. The custom format must have inclusion in
renaming enabled. A naming template is not an arbitrary regex search/replacement rule;
Tanukarr's naming configuration has no general search/replacement setting.

## Findings and changes

The initial differential test passed **292 episode/daily/anime cases**, but failed all
**10 movie cases**. Movie operations used a reduced string-replacement formatter, losing
Tanukarr tokens such as `{Series Year}`, separator/case variants, original-name tokens,
optional quality wrappers and custom formats.

Movie rename previews, manual imports and completed-download imports now use the same
regex/token renderer as episode naming. They receive the file's quality/revision, original
name, release group, stored media information and evaluated custom formats. Kitsunarr's
existing `{Movie Title}`, `{Movie TitleYear}`, `{Year}` and `{Release Year}` aliases remain
available, alongside Tanukarr's series-token spellings. Custom-format inclusion and filtering
apply consistently to previewed and imported movie names.

Naming settings also rejected every `..` substring, including the literal `...` supported
by Tanukarr's renderer. Validation now rejects parent path **components** and leading path
separators while accepting literal ellipses.

Example verified through the HTTP API:

```text
Template: {SERIES TITLE}...({Series Year}){[Custom Formats]}-{Release Group}
Before:   Proof.Film.2020.WEB-DL.1080p-GRP.mkv
After:    PROOF FILM...(2020)[WEB]-GRP.mkv
```

## Verification

- **302/302 filename comparisons passed** against Tanukarr's compiled renderer: all six
  multi-episode styles, daily dates, anime absolute numbering/proper revisions, separators,
  casing, cleanup, title variants/truncation, Unicode, escaping, original names and custom
  format inclusion/exclusion. The ten previously failing movie cases now match.
- **72/72 regex comparisons passed** against Tanukarr's compiled release-title specification:
  lookbehind/lookahead, negative lookbehind, named/repeated groups, backreferences, inline
  case options, negation and filename fallback.
- Real temporary-file tests passed for movie manual and completed-download imports,
  read-only preview, movie rename, matching subtitle rename, unchanged bytes, occupied-target
  refusal and idempotence. An HTTP regression also checks invalid regex rejection, persistence
  of the template with ellipses, traversal rejection, unchanged unrelated subtitles and an
  explicit rename while automatic renaming is disabled.
- Workspace tests: **828 passed, 0 failed, 12 ignored**. This includes the existing episode
  scan/rename, sidecar, import, filesystem-guard and rollback regression tests.
- Workspace clippy, formatting and whitespace checks passed; the server build passed.
- Full Chromium scenario: **1 passed in 176.07 seconds**. The test configures a regex-based
  `E2E WEB` format and a movie token template, checks the reviewed name
  `E2E FILM...(2020)[E2E WEB].mkv`, applies it through the browser and verifies the feature
  and its associated subtitle through the remaining file-management flow.

The comparison executes the original compiled methods inside an offline .NET SDK container;
it does not start the full Tanukarr server. Service proxies supply configuration and quality
metadata. Renderer cases supply synthetic custom-format names; regex cases separately call
the real specification. Integration tests use synthetic video/subtitle payloads in disposable
directories, not personal media or remote downloads. Playback/transcoding was not tested.

The reference generator and portable Rust oracle are documented in the
[fixture instructions](../tests/fixtures/README.md).

## Local instance

A SQLite backup and snapshots of naming settings and file paths were saved before updating
the existing local instance. The task does not invoke renaming on the user's library. The
recorded baseline is **80 series, 14 movies and 423 episode-file records**; there are no
movie-file records in that instance.

The updated server is running at **http://127.0.0.1:8990** under the existing user service
`kitsunarr-local-8990.service`. The native status endpoint responds. After restart, naming
settings, all **423 recorded file paths**, and the **80 series / 14 movies** counts match
the pre-update snapshot. No rename operation was invoked on that library. Renames performed
for this verification were confined to the disposable test directories.

## Limits

This verifies the naming templates and regex constructs above, not every possible .NET regex
feature or custom-format specification. Movie records currently do not store original-language
or indexer-flag metadata; naming-time custom-format evaluation uses the information available
on those records. Group-folder reorganization and arbitrary regex replacement rules are outside
this checkpoint.
