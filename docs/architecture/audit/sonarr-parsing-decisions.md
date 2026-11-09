> Audit notes produced during Kitsunarr's source analysis (read-only review of the GPL-3.0 Sonarr and Tanukarr sources, November 2026). Line references point at the audited revisions; see `../source-audit.md` for the summary.

# Sonarr release parsing / quality / decision engine / naming — porting audit for Kitsunarr

Source audited (read-only): `ref/sonarr` = `ref/sonarr`
- git HEAD `c018578e4b79ee048575c02d7a8aa2b929e4b837` (branch `develop`, 2026-11-02), **tagged `v5.0.0.2941`**.
  The brief says "Sonarr v4"; the checkout is the v5 line (successor of v4). Notable v5-isms that a v4 port
  would not have: `ParsedEpisodeInfo.SeasonNumbers` is a `List<int>` (multi-season packs, `ReleaseType.MultiSeasonPack`),
  `ParseSeasonTitle` (anime season titles), `IsSeasonTitle`. Any statement of the form "v4 did X" is **UNVERIFIED**.
- All paths below are relative to `ref/sonarr/src/` unless stated.

## 0. Porting conventions used in this report (read first)

* **Regex text is byte-exact.** Every pattern in a ```` ```regex ```` block was machine-extracted from the C# source and then
  machine-verified to exist verbatim in the source (after re-escaping). C# verbatim strings (`@"..."`) have exactly one
  escape: `""` means `"`. The blocks below show the **effective regex** (i.e. `""` already collapsed to `"`); nothing else
  is altered. Where a pattern came from a regular (non-verbatim) C# string, that is said explicitly.
* Options: `IgnoreCase` = .NET `RegexOptions.IgnoreCase` (culture-invariant-ish case folding; `Compiled` is irrelevant
  for semantics). Patterns **without** `IgnoreCase` are case-sensitive — this matters (e.g. some RejectHashed patterns).
* **.NET regex features used that Rust's `regex` crate does NOT support** (must use `fancy-regex`, `pcre2`, or a
  hand-rolled matcher; see risk list in §11):
  1. **Capture collections** — `Group.Captures` returns *every* capture of a quantified group (e.g. `(?:...(?<episode>\d+))+`
     yields all episodes), and `Group.Value` is the **last** capture. The parser relies on `Captures.First()` /
     `Captures.Last()` / `Captures.Count` everywhere (episodes, absolute episodes, seasons, titles). No Rust engine
     (regex, fancy-regex, pcre2) exposes capture stacks. **This is the #1 porting risk.**
  2. **Duplicate group names** in one pattern (e.g. two `(?<title>...)`, two `(?<absoluteepisode>...)`, two `(?<episode>...)`):
     in .NET they are the *same* group and captures accumulate in left-to-right match order.
  3. **Lookbehind of variable length**: `(?<!\d+)`, `(?<=\b\d+)`, `(?<!\d+\W+)`, `(?<![-_. ]|\b[0]\d+)`, `(?<!\d{1,2}-)`,
     `(?<![()\[!]|\d{1,2}-)`. Note `(?<!\d+)` ≡ `(?<!\d)` and `(?<=\b\d+)` ≡ `(?<=\d)` *only if* the digit run is
     word-bounded (it always is after `\d` preceded by `\b`… careful: `\b\d+` requires a word boundary *before the run*,
     so `(?<=\b\d+)%` means "% preceded by a run of digits that starts at a word boundary", e.g. `a1%` does NOT match).
  4. **Backreference** `\k<sep>` (daily patterns), **atomic group** `(?>...)` (Part One…Nine pattern), conditional-free.
  5. `\d`, `\w`, `\b`, `\s` are **Unicode-aware** in .NET (no `ECMAScript` option is used anywhere in these files).
  6. `Regex.Matches()` = all non-overlapping matches scanned left→right; the parser uses `MatchCollection[0]` for most
     things but iterates **all** matches for episode numbers (last match wins). See §1.6.
* `Group.Index`/`Length` for a group that did not participate are `0`/`0` (so `EndIndex()` = 0). `EndIndex()` is
  `capture.Index + capture.Length` (`NzbDrone.Common/Extensions/RegexExtensions.cs`). For a group, Index/Length are those
  of its **last** capture.

---

## 1. Parser (`NzbDrone.Core/Parser/Parser.cs`, `ParserCommon.cs`, `RegexReplace.cs`, `ReleaseGroupParser.cs`)

### 1.1 Public entry points (Parser.cs)

| Method | Purpose |
|---|---|
| `ParseTitle(string title)` | Main release-title parser → `ParsedEpisodeInfo` or `null`. |
| `ParsePath(string path)` | Parses a file path: filename first, then folder+file combos (§1.9). |
| `ParseSeasonTitle(string title)` | v5: "title-only" anime season pack fallback (§1.10). |
| `ParseSeasonFolder(string folderName)` | Season number from a folder name (§1.10). |
| `ParseSeriesName(string title)` | `ParseTitle(title)?.SeriesTitle ?? CleanSeriesTitle(title)`. |
| `CleanSeriesTitle(this string)` | Canonical lookup key for titles (§1.8). |
| `NormalizeEpisodeTitle(string)`, `NormalizeTitle(string)` | Episode-title / generic normalisation (§1.8). |
| `NormalizeImdbId(string)` | `^(\d{1,10}|(tt)\d{1,10})$` (case-sensitive) else null; strips `tt`, left-pads to 7 digits with `0`, prefixes `tt`; returns null if input length ≤ 2. |
| `SimplifyTitle(string)` | Validate → un-reverse → WebsitePrefix → WebsitePostfix → CleanTorrentSuffix (no SimpleTitleRegex!). Returns input unchanged if validation fails. |
| `RemoveFileExtension(string)` | → `FileExtensions.RemoveFileExtension` (§1.11). |
| `HasMultipleLanguages(string)` | `MultiRegex` (§1.7) `IsMatch`. |

### 1.2 Pre-validation — `ValidateBeforeParsing(title)` (Parser.cs L1277)

Returns false (→ `ParseTitle` returns null) when, in order:
1. `title.ToLower()` contains both `"password"` **and** `"yenc"`.
2. No char satisfies `char.IsLetterOrDigit` (Unicode).
3. `RemoveFileExtension(title)` matches **any** of `RejectHashedReleasesRegexes` (§1.7).
4. `RemoveFileExtension(title)` matches `SeasonFolderRegexes` = `^(Season[ ._-]*\d+|Specials)$` (**case-sensitive**).

### 1.3 `ParseTitle(title)` exact flow (Parser.cs L721–849)

Everything is inside one `try { … } catch (Exception)`; any unexpected exception (e.g. `FormatException` from
`ParseNumber`) aborts the whole parse and returns **null** (it does *not* move to the next regex).

1. `ValidateBeforeParsing(title)` → null on failure.
2. **Reversed titles**: if `ReversedTitleRegex` matches the *original* title (with extension):
   `t = RemoveFileExtension(title)`; `title = reverse(t chars) + title.Substring(t.Length)` (extension kept in place).
3. `releaseTitle = RemoveFileExtension(title)`; then replace fullwidth brackets `【`→`[` and `】`→`]`.
4. Apply **every** `ParserCommon.PreSubstitutionRegex` in order, cumulatively (`TryReplace` always replaces; return value only drives logging) — §1.5.
5. `simpleTitle = SimpleTitleRegex.Replace(releaseTitle, "")` (strips resolution/codec/bit-depth tokens).
6. `simpleTitle = WebsitePrefixRegex.Replace(simpleTitle)`, then `WebsitePostfixRegex`, then `CleanTorrentSuffixRegex` (all → "").
7. `CleanQualityBracketsRegex` (`\[[a-z0-9 ._-]+\]$`, IgnoreCase) with evaluator: remove the trailing `[...]` **only if**
   `QualityParser.ParseQualityName(m.Value).Quality != Quality.Unknown`.
8. **Six-digit air date fix**: `SixDigitAirDateRegex.Match(simpleTitle)` (first match only). If success and
   NOT (`airmonth == "00"` AND `airday == "00"`): `simpleTitle = simpleTitle.Replace(<airdate text>, "20{yy}.{mm}.{dd}")`
   (`string.Replace` = replaces **all** occurrences of that 6-char substring).
9. For each regex in `ReportTitleRegex` **in order** (§1.4): `matches = regex.Matches(simpleTitle)`; if `Count != 0`:
   * `result = ParseMatchCollection(matches, releaseTitle)` (**note: matches are on `simpleTitle`, but indices are later
     applied to `releaseTitle`** — see ReleaseTokens in §1.6. This offset bug must be reproduced for language parsing parity).
   * If `result == null` → **continue** with the next regex.
   * `InvalidDateException` → **break** (stop trying regexes; return null).
   * On non-null result:
     1. if `result.FullSeason && result.ReleaseTokens` contains `"Special"` (case-insensitive) → `FullSeason=false; Special=true`.
     2. `result.Languages = LanguageParser.ParseLanguages(result.ReleaseTokens)` (only the tail after the episode numbers!).
     3. `result.Quality = QualityParser.ParseQuality(title)` (the un-reversed **original title incl. extension**).
     4. `result.ReleaseGroup = ReleaseGroupParser.ParseReleaseGroup(releaseTitle)`; then if the match has a successful,
        non-whitespace `subgroup` group (`matches[0].Groups["subgroup"]`), it **overrides** ReleaseGroup.
     5. `result.ReleaseHash` = `matches[0].Groups["hash"]` value trimmed of `[ ] ( )`; returns "" if it equals `1280x720`.
     6. return result.
10. Nothing matched → null.

### 1.4 `ReportTitleRegex` — full ordered list (Parser.cs L21–468)

109 active patterns, tried **in this order**, first one whose `Matches()` is non-empty **and** whose
`ParseMatchCollection` returns non-null wins. Unless stated, options are `RegexOptions.IgnoreCase | RegexOptions.Compiled`.
The array starts with a **commented-out** (disabled) pattern — do not port it:
`// Anime - Absolute Episode Number + Title + Season+Episode` / `// Todo: This currently breaks series that start with numbers`.
Headings = the C# comment(s) immediately above each pattern (multi-line comments joined with " / ").
Pattern #1 really does start with `^^` (double anchor; harmless).

#### 1. Daily episode with year in series title and air time after date (Plex DVR format)

```regex
^^(?<title>.+?\((?<titleyear>\d{4})\))[-_. ]+(?<airyear>19[4-9]\d|20\d\d)(?<sep>[-_]?)(?<airmonth>0\d|1[0-2])\k<sep>(?<airday>[0-2]\d|3[01])[-_. ]\d{2}[-_. ]\d{2}[-_. ]\d{2}
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 2. Daily episodes without title (2018-10-12, 20181012) (Strict pattern to avoid false matches)

```regex
^(?<airyear>19[6-9]\d|20\d\d)(?<sep>[-_]?)(?<airmonth>0\d|1[0-2])\k<sep>(?<airday>[0-2]\d|3[01])(?!\d)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 3. Multi-Part episodes without a title (S01E05.S01E06)

```regex
^(?:\W*S(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:e{1,2}(?<episode>\d{1,3}(?!\d+)))+){2,}
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 4. Multi-Part episodes without a title (1x05.1x06)

```regex
^(?:\W*(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:x{1,2}(?<episode>\d{1,3}(?!\d+)))+){2,}
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 5. Episodes without a title, Multi (S01E04E05, 1x04x05, etc)

```regex
^(?:S?(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:(?:[-_]|[ex]){1,2}(?<episode>\d{2,3}(?!\d+))){2,})
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 6. Split episodes (S01E05a, S01E05b, etc)

```regex
^(?<title>.+?)(?:S?(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:(?:[-_ ]?[ex])(?<episode>\d{2,3}(?!\d+))(?<splitepisode>[a-d])(?:[ _.])))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 7. Episodes without a title, Single (S01E05, 1x05)

```regex
^(?:S?(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:(?:[-_ ]?[ex])(?<episode>\d{2,3}(?!\d+))))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 8. Anime - [SubGroup] Title Absolute (Season+Episode)

```regex
^(?:\[(?<subgroup>.+?)\](?:_|-|\s|\.)?)(?<title>.+?)[-_. ]+(?<absoluteepisode>(?<!\d+)\d{2,3}(\.\d{1,2})?(?!\d+))(?:[-_. ])+\((?:S(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:[ex]|\W[ex]){1,2}(?<episode>\d{2}(?!\d+))))(?:v\d+)?(?:\)(?!\d+)).*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 9. Anime - [SubGroup] Title Absolute - Season+Episode

```regex
^(?:\[(?<subgroup>.+?)\](?:_|-|\s|\.)?)(?<title>.+?)[-_. ]+(?<absoluteepisode>(?<!\d+)\d{2,3}(\.\d{1,2})?(?!\d+))(?:[-_. ](?<![()\[!]))+(?:S(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:[ex]|\W[ex]){1,2}(?<episode>\d{2}(?!\d+))))(?:v\d+)?(?:[_. ](?!\d+)).*?(?<hash>[(\[][0-9A-F]{8}[)\]])$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 10. Anime - [SubGroup] Title Season+Episode

```regex
^(?:\[(?<subgroup>.+?)\](?:_|-|\s|\.)?)(?<title>.+?)(?:[-_\W](?<![()\[!]))+(?:S?(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:[ex]|\W[ex]){1,2}(?<episode>\d{2}(?!\d+)))+)(?:v\d+)?(?:[_. ](?!\d+)).*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 11. Anime - [SubGroup] Title Episode Absolute Episode Number ([SubGroup] Series Title Episode 01)

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)(?<title>.+?)[-_. ]+?(?:Episode)(?:[-_. ]+(?<absoluteepisode>(?<!\d+)\d{2,3}(\.\d{1,2})?(?!\d+)))+.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 12. Anime - [SubGroup] Title Absolute Episode Number + Season+Episode

```regex
^(?:\[(?<subgroup>.+?)\](?:_|-|\s|\.)?)(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+(?<absoluteepisode>\d{2,3}(\.\d{1,2})?))+(?:_|-|\s|\.)+(?:S?(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:\-|[ex]|\W[ex]){1,2}(?<episode>\d{2}(?!\d+)))+).*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 13. Anime - [SubGroup] Title Multi-episode + optional Absolute Episode Number

```regex
^(?:\[(?<subgroup>.+?)\](?:_|-|\s|\.)?)(?<title>.+?)(?:[-_\W](?<![()\[!]))+(?:S?(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:\-|[ex]|\W[ex]){1,2}(?<episode>\d{2}(?!\d+))){2,})(?:(?:_|-|\s|\.)+(?<absoluteepisode>(?<!\d+)\d{2,3}(\.\d{1,2})?(?!\d+|\-[a-z])))*.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 14. Anime - [SubGroup] Title Single episode + Absolute Episode Number

```regex
^(?:\[(?<subgroup>.+?)\](?:_|-|\s|\.)?)(?<title>.+?)(?:[-_\W](?<![()\[!]))+(?:S?(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:\-|[ex]|\W[ex]){1,2}(?<episode>\d{2}(?!\d+))))(?:(?:_|-|\s|\.)+(?<absoluteepisode>(?<!\d+)\d{2,3}(\.\d{1,2})?(?!\d+|\-[a-z])))+.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 15. Anime - [SubGroup] Title with trailing number Absolute Episode Number - Batch separated with tilde

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>.+?[^-]+?)(?:(?<![-_. ]|\b[0]\d+) - )[-_. ]?(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+))\s?~\s?(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+))(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 16. Anime - [SubGroup] Title with season number in brackets Absolute Episode Number

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>[^-]+?)[_. ]+?\(Season[_. ](?<season>\d+)\)[-_. ]+?(?:[-_. ]?(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+)))+(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 17. Anime - [SubGroup] Title with trailing 3-digit number and sub title - Absolute Episode Number

```regex
^\[(?<subgroup>[^\]]+?)\][-_. ]?(?<title>[^]]+?)(?:[-_. ]{3}?(?<absoluteepisode>\d{2}(\.\d{1,2})?(?!-?\d+|-[a-z]+)))+(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 18. Anime - [SubGroup] Multi-season pack

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>.+?)[-_. ]+S(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))(?:(?:-S?|[-_. ]{1,3}S)(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))){1,}
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 19. Anime - [SubGroup] Partial season pack

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>.+?)(?:\W+S(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))\W+(?:(?:(?:Part|Vol)\W?|(?<!\d+\W+)e|p)(?<seasonpart>\d{1,2}(?!\d+)))+)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 20. Anime - [SubGroup] Title with trailing number Absolute Episode Number

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>[^-]+?)(?:(?<![-_. ]|\b[0]\d+) - )(?:[-_. ]?(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+)))+(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 21. Anime - [SubGroup] Title with trailing number S## (Full season)

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>.+?)[-_. ]+(?:S(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?![ex]?\d+))).+?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 22. Anime - [SubGroup] Title with trailing number Absolute Episode Number

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>[^-]+?)(?:(?<![-_. ]|\b[0]\d+)[_ ]+)(?:[-_. ]?(?<absoluteepisode>\d{3}(\.\d{1,2})?(?!\d+|-[a-z]+)))+(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 23. Anime - [SubGroup] Title - Absolute Episode Number

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>.+?)(?:(?<!\b[0]\d+))(?:[. ]-[. ](?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+|[-])))+(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 24. Anime - [SubGroup] Title Absolute Episode Number - Absolute Episode Number (batches without full separator between title and absolute episode numbers)

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>.+?)(?:(?<!\b[0]\d+))(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+|[-]))[. ]-[. ](?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+|[-]))(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 25. Anime - [SubGroup] Title (Season only)

```regex
^(?:\[(?<subgroup>.+?)\][-_. ])?(?<title>.+?)[-_. ]+?[\[(](?:S|Season|Saison|Series|Stagione|Sezon)[-_. ]?(?<season>\d{1,2}(?![-_. ]?\d+))(?:[-_. )\]]|$)+(?<extras>EXTRAS|SUBPACK)?(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 26. Anime - [SubGroup] Title Absolute Episode Number

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>.+?)[-_. ]+\(?(?:[-_. ]?#?(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+|-[a-z]+)))+\)?(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 27. Multi-episode Repeated (S01E05 - S01E06)

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+S(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:(?:e|[-_. ]e){1,2}(?<episode>\d{1,3}(?!\d+)))+){2,}
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 28. Multi-episode Repeated (1x05 - 1x06)

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:x{1,2}(?<episode>\d{1,3}(?!\d+)))+){2,}
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 29. Single episodes with a title (S01E05, 1x05, etc) followed by ".5 [SP]"

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+S?(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))(?:[ex]|\W[ex]|_){1,2}(?<episode>\d{2,3}(?!\d+|(?:[ex]|\W[ex]|_|-){1,2}\d+))(?<special>\.5[ .]\[SP\]))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 30. Single episodes with a title (S01E05, 1x05, etc) and trailing info in slashes

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+S?(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))(?:[ex]|\W[ex]|_){1,2}(?<episode>\d{2,3}(?!\d+|(?:[ex]|\W[ex]|_|-){1,2}\d+))).+?(?:\[.+?\])(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 31. Anime - Title Season EpisodeNumber + Absolute Episode Number [SubGroup]

```regex
^(?<title>.+?)(?:[-_\W](?<![()\[!]))+(?:S?(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:[ex]|\W[ex]|-){1,2}(?<episode>(?<!\d+)\d{2}(?!\d+)))+)[-_. (]+?(?:[-_. ]?(?<absoluteepisode>(?<!\d+)\d{3}(\.\d{1,2})?(?!\d+|[pi])))+.+?\[(?<subgroup>.+?)\](?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 32. Multi-Episode with a title (S01E05E06, S01E05-06, S01E05 E06, etc) and trailing info in slashes

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+S?(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))(?:[ex]|\W[ex]|_){1,2}(?<episode>\d{2,3}(?!\d+))(?:(?:\-|[ex]|\W[ex]|_){1,2}(?<episode>\d{2,3}(?!\d+)))+).+?(?:\[.+?\])(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 33. Anime - Title Absolute Episode Number [SubGroup] [Hash]? (Series Title Episode 99-100 [RlsGroup] [ABCD1234])

```regex
^(?<title>.+?)[-_. ]Episode(?:[-_. ]+(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+)))+(?:.+?)\[(?<subgroup>.+?)\].*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 34. Anime - Title Absolute Episode Number [SubGroup] [Hash]

```regex
^(?<title>.+?)(?:(?:_|-|\s|\.)+(?<absoluteepisode>\d{3}(\.\d{1,2})(?!\d+)))+(?:.+?)\[(?<subgroup>.+?)\].*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 35. Anime - Title Absolute Episode Number (Year) [SubGroup]

```regex
^(?<title>.+?)[-_. ]+(?<absoluteepisode>(?<!\d+)\d{2}(?!\d+))[-_. ](\(\d{4}\))[-_. ]\[(?<subgroup>.+?)\]
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 36. Anime - Title with trailing number, Absolute Episode Number and hash

```regex
^(?<title>[^-]+?)(?:(?<![-_. ]|\b[0]\d+) - )(?:[-_. ]?(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+)))+(?:[-_. ]+(?<special>special|ova|ovd))?.*?(?<hash>[(\[][0-9A-F]{8}[)\]])(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 37. Anime - Title Absolute Episode Number [Hash]

```regex
^(?<title>.+?)(?:(?:_|-|\s|\.)+(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+)))+(?:[-_. ]+(?<special>special|ova|ovd))?[-_. ]+.*?(?<hash>[(\[][0-9A-F]{8}[)\]])$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 38. Episodes with airdate AND season/episode number, capture season/episode only

```regex
^(?<title>.+?)?\W*(?<airdate>\d{4}\W+[0-1][0-9]\W+[0-3][0-9])(?!\W+[0-3][0-9])[-_. ](?:s?(?<season>(?<!\d+)(?:\d{1,2})(?!\d+)))(?:[ex](?<episode>(?<!\d+)(?:\d{1,3})(?!\d+)))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 39. Episodes with airdate AND season/episode number

```regex
^(?<title>.+?)?\W*(?<airyear>\d{4})\W+(?<airmonth>[0-1][0-9])\W+(?<airday>[0-3][0-9])(?!\W+[0-3][0-9]).+?(?:s?(?<season>(?<!\d+)(?:\d{1,2})(?!\d+)))(?:[ex](?<episode>(?<!\d+)(?:\d{1,3})(?!\d+)))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 40. Episodes with absolute episode number AND airdate (TJET Wrestling)

```regex
^(?<title>.+?)?[-_. ](?:e\d{2,3}(?!\d+))[-_. ](?<airyear>\d{4})\W+(?<airmonth>[0-1][0-9])\W+(?<airday>[0-3][0-9])(?!\W+[0-3][0-9]).+?(?:\[[a-z]+\])
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 41. Single or multi episode releases with multiple titles, then season and episode numbers after the last title. (Title1 / Title2 / ... / S1E1-2 of 6)

```regex
^((?<title>.*?)[ ._]\/[ ._])+\(?S(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:\W|_)?E?[ ._]?(?<episode>(?<!\d+)\d{1,2}(?!\d+))(?:-(?<episode>(?<!\d+)\d{1,2}(?!\d+)))?(?:[ ._]of[ ._](?<episodecount>\d{1,2}))?\)?[ ._][\(\[]
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 42. Multi-episode with title (S01E99-100, S01E05-06)

```regex
^(?<title>.+?)(?:[-_\W](?<![()\[!]))+S(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))E(?<episode>\d{2,3}(?!\d+))(?:-(?<episode>\d{2,3}(?!\d+)))+(?:[-_. ]|$)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 43. Multi-episode with title (S01E05-06, S01E05-6)

```regex
^(?<title>.+?)(?:[-_\W](?<![()\[!]))+S(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))E(?<episode>\d{1,2}(?!\d+))(?:-(?<episode>\d{1,2}(?!\d+)))+(?:[-_. ]|$)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 44. Episodes with a Chinese title and English title, Single episodes (S01E05, 1x05, etc) & Multi-episode (S01E05E06, S01E05-06, S01E05 E06, etc)

```regex
^(?<title>[\u4E00-\u9FCC]+)[_. ](?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+S?(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))(?:[ex]|\W[ex]){1,2}(?<episode>\d{2,3}(?!\d+))(?:(?:\-|[ex]|\W[ex]|_){1,2}(?<episode>\d{2,3}(?!\d+)))*)(?:[-_. ]|$)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 45. Episodes with a title, Single episodes (S01E05, 1x05, etc) & Multi-episode (S01E05E06, S01E05-06, S01E05 E06, etc)

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+S?(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))(?:[ex]|\W[ex]){1,2}(?<episode>\d{2,3}(?!\d+))(?:(?:\-|[ex]|\W[ex]|_){1,2}(?<episode>\d{2,3}(?!\d+)))*)(?:[-_. ]|$)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 46. Episodes with a title, 4 digit season number, Single episodes (S2016E05, etc) & Multi-episode (S2016E05E06, S2016E05-06, S2016E05 E06, etc)

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+S(?<season>(?<!\d+)(?:\d{4})(?!\d+))(?:e|\We|_){1,2}(?<episode>\d{2,4}(?!\d+))(?:(?:\-|e|\We|_){1,2}(?<episode>\d{2,3}(?!\d+)))*)\W?(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 47. Episodes with a title, 4 digit season number, Single episodes (2016x05, etc) & Multi-episode (2016x05x06, 2016x05-06, 2016x05 x06, etc)

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+(?<season>(?<!\d+)(?:\d{4})(?!\d+))(?:x|\Wx){1,2}(?<episode>\d{2,4}(?!\d+))(?:(?:\-|x|\Wx|_){1,2}(?<episode>\d{2,3}(?!\d+)))*)\W?(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 48. Episodes with a title, Single episodes (s01.05)

```regex
^(?<title>.+?)(?:[-_\W](?<![()\[!]))+S(?<season>(?<!\d+)(?:\d{2})(?!\d+))(?:\.)(?<episode>\d{2,3}(?!\d+))(?:[-_. ]|$)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 49. Multi-season pack

```regex
^(?<title>.+?)(Complete Series)?[-_. ]+(?:S|(?:Season|Saison|Series|Stagione)[_. ])(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))(?:(?:[-_. ]{1}|[-_. ]{3})(?:S|(?:Season|Saison|Series|Stagione)[_. ])?(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))){1,}
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 50. Partial season pack

```regex
^(?<title>.+?)(?:\W+S(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))\W+(?:(?:(?:Part|Vol)\W?|(?<!\d+\W+)e|p)(?<seasonpart>\d{1,2}(?!\d+)))+)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 51. Anime - 4 digit absolute episode number in batch (1017-1088) or 1017-1088

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)(?<title>.+?)[-_. ]+?\(?(?<absoluteepisode>\d{4}(\.\d{1,2})?(?!\d+))[-](?<absoluteepisode>\d{4}(\.\d{1,2})?(?!\d+))\)?
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 52. Anime - 4 digit absolute episode number

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)(?<title>.+?)[-_. ]+?(?<absoluteepisode>\d{4}(\.\d{1,2})?(?!\d+))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 53. Mini-Series, treated as season 1, multi episodes are labelled as E1-E2

```regex
^(?<title>.+?)(?:[-._ ][e])(?<episode>\d{2,3}(?!\d+))(?:(?:\-?[e])(?<episode>\d{2,3}(?!\d+)))+
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 54. 5 digit episode number with a title

```regex
^(?:(?<title>.+?)(?:_|-|\s|\.)+)(?:S?(?<season>(?<!\d+)\d{1,2}(?!\d+)))(?:(?:\-|[ex]|\W[ex]|_){1,2}(?<episode>(?<!\d+)\d{5}(?!\d+)))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 55. 5 digit multi-episode with a title

```regex
^(?:(?<title>.+?)(?:_|-|\s|\.)+)(?:S?(?<season>(?<!\d+)\d{1,2}(?!\d+)))(?:(?:[-_. ]{1,3}ep){1,2}(?<episode>(?<!\d+)\d{5}(?!\d+)))+
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 56. Episodes with airdate and part (2018.04.28.Part.2)

```regex
^(?<title>.+?)?\W*(?<airyear>\d{4})[-_. ]+(?<airmonth>[0-1][0-9])[-_. ]+(?<airday>[0-3][0-9])(?![-_. ]+[0-3][0-9])[-_. ]+Part[-_. ]?(?<part>[1-9])
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 57. Episodes with airdate (2018.04.28)

```regex
^(?<title>.+?)?\W*(?<airyear>\d{4})[-_. ]+(?<airmonth>[0-1][0-9])[-_. ]+(?<airday>[0-3][0-9])(?![-_. ]+[0-3][0-9])
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 58. Anime - Title 4-digit Absolute Episode Number [SubGroup]

```regex
^(?<title>.+?)[-_. ]+(?<absoluteepisode>(?<!\d+)\d{4}(?!\d+))[-_. ]\[(?<subgroup>.+?)\]
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 59. Mini-Series with year in title, treated as season 1, episodes are labelled as Part01, Part 01, Part.1

```regex
^(?<title>.+?\d{4})(?:\W+(?:(?:Part\W?|e)(?<episode>\d{1,2}(?!\d+)))+)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 60. 3-digit season number / Single episode: Title.S001E01 / Multi episode: Title.S001E01-E02

```regex
^(?<title>.+?)[-_. ]+S(?<season>(?<!\d+)\d{3}(?!\d+))E(?<episode>(?<!\d+)\d{2}(?!\d+))(?:-[ex](?<episode>(?<!\d+)\d{2}(?!\d+)))?([_. ]|$)(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 61. Mini-Series, treated as season 1, episodes are labelled as XofY

```regex
^(?<title>.+?)(?:\W+(?:(?<episode>(?<!\d+)\d{1,2}(?!\d+))of\d+)+)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 62. Supports Season 01 Episode 03

```regex
(?:.*(?:\"|^))(?<title>.*?)(?:[-_\W](?<![()\[]))+(?:\W?Season\W?)(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:\W|_)+(?:Episode\W)(?:[-_. ]?(?<episode>(?<!\d+)\d{1,2}(?!\d+)))+
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 63. Multi-episode with episodes in square brackets (Series Title [S01E11E12] or Series Title [S01E11-12])

```regex
(?:.*(?:^))(?<title>.*?)[-._ ]+\[S(?<season>(?<!\d+)\d{2}(?!\d+))(?:[E-]{1,2}(?<episode>(?<!\d+)\d{2}(?!\d+)))+\]
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 64. Multi-episode with episodes in brackets (Series Title (S01E11E12) or Series Title (S01E11-12) or Series Title (S01E1-3 of 12))

```regex
(?:.*(?:^))(?<title>.*?)[-._ ]+\(S(?<season>(?<!\d+)\d{2}(?!\d+))(?:[E-]{1,2}(?<episode>(?<!\d+)\d{1,2}(?!\d+)))+([._ ]+of[._ ]+[\dx]+)?\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 65. Multi-episode release with no space between series title and season (S01E11E12)

```regex
(?:.*(?:^))(?<title>.*?)S(?<season>(?<!\d+)\d{2}(?!\d+))(?:E(?<episode>(?<!\d+)\d{2}(?!\d+)))+
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 66. Multi-episode with single episode numbers (S6.E1-E2, S6.E1E2, S6E1E2, etc)

```regex
^(?<title>.+?)[-_. ]S(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:[-_. ]?[ex]?(?<episode>(?<!\d+)\d{1,2}(?!\d+)))+
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 67. Single or multi episode releases with multiple titles, each followed by season and episode numbers in brackets

```regex
^(?<title>.*?)[ ._]\(S(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:\W|_)?E?[ ._]?(?<episode>(?<!\d+)\d{1,2}(?!\d+))(?:-(?<episode>(?<!\d+)\d{1,2}(?!\d+)))?\)(?:[ ._]\/[ ._])(?<title>.*?)[ ._]\(
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 68. Multi-episode with episodes in brackets (Series Title (S01E11E12) or Series Title (S1E11-12) or Series Title (S1E1-3 of 12))

```regex
(?:.*(?:^))(?<title>.*?)[-._ ]+\(S(?<season>(?<!\d+)\d{1}(?!\d+))(?:[E-]{1,2}(?<episode>(?<!\d+)\d{1,2}(?!\d+)))+([._ ]+of[._ ]+[\dx]+)?\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 69. Single episode season or episode S1E1 or S1-E1 or S1.Ep1 or S01.Ep.01

```regex
(?:.*(?:\"|^))(?<title>.*?)(?:\W?|_)S(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:\W|_)?Ep?[ ._]?(?<episode>(?<!\d+)\d{1,2}(?!\d+))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 70. 3 digit season S010E05

```regex
(?:.*(?:\"|^))(?<title>.*?)(?:\W?|_)S(?<season>(?<!\d+)\d{3}(?!\d+))(?:\W|_)?E(?<episode>(?<!\d+)\d{1,2}(?!\d+))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 71. Separated season and episode numbers S01 - E01

```regex
^(?<title>.+?)(?:_|-|\s|\.)+S(?<season>\d{2}(?!\d+))(\W-\W)E(?<episode>(?<!\d+)\d{2}(?!\d+))(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 72. Season and episode numbers in square brackets (single and multi-episode) / Series Title - [02x01] - Episode 1 / Series Title - [02x01x02] - Episode 1

```regex
^(?<title>.+?)?(?:[-_\W](?<![()\[!]))+\[(?:s)?(?<season>(?<!\d+)\d{1,2})(?:(?:[ex])(?<episode>\d{2}))(?:(?:[-ex]){1,2}(?<episode>\d{2}))*\].+?(?:\.|$)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 73. Anime - Title with season number - Absolute Episode Number (Title S01 - EP14)

```regex
^(?<title>.+?S\d{1,2})[-_. ]{3,}(?:EP)?(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+|[-]))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 74. Anime - French titles with single episode numbers, with or without leading sub group ([RlsGroup] Title - Episode 1)

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)[-_. ]+?(?:Episode[-_. ]+?)(?<absoluteepisode>\d{1}(\.\d{1,2})?(?!\d+))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 75. Anime - Absolute episode number in square brackets

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)(?<title>.+?)[-_. ]+?\[(?<absoluteepisode>\d{2,3}(\.\d{1,2})?(?!\d+))\]
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 76. Japanese variety shows with leading date

```regex
^(?<airyear>\d{2})(?<airmonth>[0-1][0-9])(?<airday>[0-3][0-9])(?![-_. ]+[0-3][0-9])[-_. ](?<title>.+?)[-_. ](?:Season[-_. ]?(?<season>\d{1,2})[-_. ])?(?:ep|#)(?<episode>\d{2,3})
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 77. Season only releases followed by year

```regex
^(?<title>.+?)[-_. ]+?(?:S|Season|Saison|Series|Stagione)[-_. ]?(?<season>\d{1,2}(?=[-_. ]\d{4}[-_. ]+))(?<extras>EXTRAS|SUBPACK)?(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 78. Season only releases

```regex
^(?<title>.+?)[-_. ]+?(?:S|Season|Saison|Series|Stagione)[-_. ]?(?<season>\d{1,2}(?![-_. ]?\d+))(?:[-_. ]|$)+(?<extras>EXTRAS|SUBPACK)?(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 79. 4 digit season only releases

```regex
^(?<title>.+?)[-_. ]+?(?:S|Season|Saison|Series|Stagione)[-_. ]?(?<season>\d{4}(?![-_. ]?\d+))(\W+|_|$)(?<extras>EXTRAS|SUBPACK)?(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 80. Spanish tracker releases

```regex
^(?<title>.+?)(?:(?:[-_. ]+?Temporada.+?|\[.+?\])\[Cap)(?:[-_. ]+(?<season>(?<!\d+)\d{1,2})(?<episode>(?<!e|x)(?:[1-9][0-9]|[0][1-9])))+(?:\])
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 81. Supports 103/113 naming

```regex
^(?<title>.+?)?(?:(?:[_.-](?<![()\[!]))+(?<season>(?<!\d+)[1-9])(?<episode>[1-9][0-9]|[0][1-9])(?![a-z]|\d+))+(?:[_.]|$)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 82. 4 digit episode number / Episodes without a title, Single (S01E05, 1x05) AND Multi (S01E04E05, 1x04x05, etc)

```regex
^(?:S?(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:\-|[ex]|\W[ex]|_){1,2}(?<episode>\d{4}(?!\d+|i|p)))+)(\W+|_|$)(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 83. 4 digit episode number / Episodes with a title, Single episodes (S01E05, 1x05, etc) & Multi-episode (S01E05E06, S01E05-06, S01E05 E06, etc)

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]|\d{1,2}-))+S?(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:\-|[ex]|\W[ex]|_){1,2}(?<episode>\d{4}(?!\d+|i|p)))+)\W?(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 84. Turkish tracker releases (01 BLM, 3. Blm, 04.Bolum, etc)

```regex
^(?<title>.+?)[_. ](?<absoluteepisode>\d{1,4})(?:[_. ]+)(?:BLM|B[oö]l[uü]m)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 85. Episodes with airdate (04.28.2018)

```regex
^(?<title>.+?)?\W*(?<ambiguousairmonth>[0-1][0-9])[-_. ]+(?<ambiguousairday>[0-3][0-9])[-_. ]+(?<airyear>\d{4})(?!\d+)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 86. Episodes with airdate (28.04.2018)

```regex
^(?<title>.+?)?\W*(?<ambiguousairday>[0-3][0-9])[-_. ]+(?<ambiguousairmonth>[0-1][0-9])[-_. ]+(?<airyear>\d{4})(?!\d+)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 87. Episodes with airdate (20180428)

```regex
^(?<title>.+?)?\W*(?<!\d+)(?<airyear>\d{4})(?<airmonth>[0-1][0-9])(?<airday>[0-3][0-9])(?!\d+)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 88. Anime - Title Absolute Episode Number (E195 or E1206)

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)(?:(?:_|-|\s|\.)+(?:e|ep)(?<absoluteepisode>(\d{3}|\d{4})(\.\d{1,2})?))+[-_. ].*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 89. Daily episodes that use short month format instead of number

```regex
^(?<title>.+?)[-_. ]+(?<airday>[1-2]\d|3[01]|[1-9])(?:th|st|rd)[-_. ](?<shortairmonth>jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[-_. ](?<airyear>19[4-9]\d|20\d\d)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 90. Supports 1103/1113 naming

```regex
^(?<title>.+?)?(?:(?:[-_. ](?<![()\[!]))*(?<!\d{1,2}-)(?<season>(?<!\d+|\(|\[|e|x)\d{2})(?<episode>(?<!e|x)(?:[1-9][0-9]|[0][1-9])(?!p|i|\d+|\)|\]|\W\d+|\W(?:e|ep|x)\d+)))+([-_. ]+|$)(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 91. Dutch/Flemish release titles

```regex
^(?<title>.+?)[-_. ](?:Se\.(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:(?:[-_ ]?afl\.)(?<episode>\d{1,3}(?!\d+))(?:(?:[-]|[-_ ]en[-_ ])(?<episode>\d{1,3}(?!\d+)))*))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 92. Episodes with single digit episode number (S01E1, S01E5E6, etc)

```regex
^(?<title>.*?)(?:(?:[-_\W](?<![()\[!]))+S?(?<season>(?<!\d+)\d{1,2}(?!\d+))(?:(?:\-|[ex]){1,2}(?<episode>\d{1}))+)+(\W+|_|$)(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 93. iTunes Season 1\05 Title (Quality).ext

```regex
^(?:Season(?:_|-|\s|\.)(?<season>(?<!\d+)\d{1,2}(?!\d+)))(?:_|-|\s|\.)(?<episode>(?<!\d+)\d{1,2}(?!\d+))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 94. iTunes 1-05 Title (Quality).ext

```regex
^(?:(?<season>(?<!\d+)(?:\d{1,2})(?!\d+))(?:-(?<episode>\d{2,3}(?!\d+))))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 95. Mini-Series, treated as season 1, episodes are labelled as Part01, Part 01, Part.1

```regex
^(?<title>.+?)(?:\W+(?:(?:(?<!\()Part\W?|(?<!\d+\W+)e)(?<episode>\d{1,2}(?!\d+|\))))+)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 96. Mini-Series, treated as season 1, episodes are labelled as Part One/Two/Three/...Nine, Part.One, Part_One

```regex
^(?<title>.+?)(?:\W+(?:Part[-._ ](?<episode>One|Two|Three|Four|Five|Six|Seven|Eight|Nine)(?>[-._ ])))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 97. Anime Range - Title Absolute Episode Number (ep01-12)

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)(?:_|\s|\.)+(?:e|ep)(?<absoluteepisode>\d{2,3}(\.\d{1,2})?)-(?<absoluteepisode>(?<!\d+)\d{1,2}(\.\d{1,2})?(?!\d+|-)).*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 98. Anime - Title Absolute Episode Number (e66)

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)(?:(?:_|-|\s|\.)+(?:e|ep)(?<absoluteepisode>\d{2,4}(\.\d{1,2})?))+[-_. ].*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 99. Anime - Title Episode Absolute Episode Number (Series Title Episode 01)

```regex
^(?<title>.+?)[-_. ](?:Episode)(?:[-_. ]+(?<absoluteepisode>(?<!\d+)\d{2,3}(\.\d{1,2})?(?!\d+)))+.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 100. Anime Range - Title Absolute Episode Number (1 or 2 digit absolute episode numbers in a range, 1-10)

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)[_. ]+(?<absoluteepisode>(?<!\d+)\d{1,2}(\.\d{1,2})?(?!\d+))-(?<absoluteepisode>(?<!\d+)\d{1,2}(\.\d{1,2})?(?!\d+|-)).*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 101. Anime - Title Episode/Episodio Absolute Episode Number

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)[-_. ]+(?:Episode|Episodio)(?:[-_. ]+(?<absoluteepisode>(?<!\d+)\d{2,4}(\.\d{1,2})?(?!\d+|[ip])))+.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 102. Anime - Title [Absolute Episode Number] from AniLibriaTV

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)(?:[-_. ]\[)(?:(?:-?)(?<absoluteepisode>(?<!\d+)\d{2,3}(\.\d{1,2})?(?!\d+|[ip])))+(?:\][-_. ]).*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 103. Simple season releases in the format 'Series (year) - Temporada season'

```regex
^(?<title>.+?)[_. ]-[_. ](?:Temporada)[-_. ]?(?<season>\d{1,2}(?![_. ]?\d+))(?:[_. ]|$)+(?<extras>EXTRAS|SUBPACK)?(?!\\)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 104. Anime - Title Absolute Episode Number

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)(?:[-_. ]+(?<absoluteepisode>(?<!\d+)\d{2,4}(\.\d{1,2})?(?!\d+|[ip])))+.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 105. Anime - Title {Absolute Episode Number}

```regex
^(?:\[(?<subgroup>.+?)\][-_. ]?)?(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+(?<absoluteepisode>(?<!\d+)\d{2,3}(\.\d{1,2})?(?!\d+|[ip])))+.*?(?<hash>[(\[][0-9A-F]{8}[)\]])?$
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 106. Extant, terrible multi-episode naming (extant.10708.hdtv-lol.mp4)

```regex
^(?<title>.+?)[-_. ](?<season>[0]?\d?)(?:(?<episode>\d{2}){2}(?!\d+))[-_. ]
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 107. Episodes without a title, Single episode numbers (S1E1, 1x1)

```regex
^(?:S?(?<season>(?<!\d+)(?:\d{1,2}|\d{4})(?!\d+))(?:(?:[-_ ]?[ex])(?<episode>\d{1}(?!\d+))))
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 108. Anime OVA special

```regex
^\[(?<subgroup>.+?)\][-_. ]?(?<title>.+?)(?:[-_. ]+(?<special>special|ova|ovd|ncop|nced)).*?(?<hash>[(\[][0-9A-F]{8}[)\]])?(?:$|\.mkv)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 109. Episodes with a title, 3 digit season number, Single episodes (S001E05, 1x05, etc) & Multi-episode (S001E05E06, S001E05-06, S001E05 E06, etc)

```regex
^(?<title>.+?)(?:(?:[-_\W](?<![()\[!]))+S?(?<season>(?<!\d+)(?:\d{3})(?!\d+))(?:[ex]|\W[ex]){1,2}(?<episode>\d{2,3}(?!\d+))(?:(?:\-|[ex]|\W[ex]|_){1,2}(?<episode>\d{2,3}(?!\d+)))*)(?:[-_. ]|$)
```
Options/args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

### 1.5 `ParserCommon.cs` — pre-substitutions and website/torrent cleaners

`RegexReplace` (RegexReplace.cs): wraps a Regex + .NET replacement string (`$1`, `${name}`; with duplicate names the
**last** capture is substituted). `Replace(input)` = `Regex.Replace` (all matches). `TryReplace(ref input)` returns
`IsMatch(input)` but **always** performs the replace.

**Usage differences (must replicate):**
* `Parser.ParseTitle`: applies **all 14** `PreSubstitutionRegex` sequentially (cumulative).
* `ReleaseGroupParser.ParseReleaseGroup`: applies them in order but **stops after the first one that matched** (`break`).
* `Parser.ParsePath`: applies `PathPreSubstitutionRegex` to the *file name* (with extension) before `ParseTitle`.

`一-鿌` inside these verbatim strings is a **regex** escape (CJK Unified Ideographs), not a C# escape.
Replacement strings are given after "Options/args" (first quoted string = replacement).

#### 1.5.1 `PreSubstitutionRegex` (ParserCommon.cs L9–52), in order
#### 1. Korean series without season number, replace with S01Exxx and remove airdate

```regex
\.E(\d{2,4})\.\d{6}\.(.*-(F1RST|NEXT))$
```
Options/args: `".S01E$1.$2", RegexOptions.Compiled`

#### 2. Some Chinese anime releases contain both English and Chinese titles, remove the Chinese title and replace with normal anime pattern

```regex
^\[(?:(?<subgroup>[^\]]+?)(?:[\u4E00-\u9FCC]+)?)\]\[(?<title>[^\]]+?)(?:\s(?<chinesetitle>[\u4E00-\u9FCC][^\]]*?))\]\[(?:(?:[\u4E00-\u9FCC]+?)?(?<episode>\d{1,4})(?:[\u4E00-\u9FCC]+?)?)\]
```
Options/args: `"[${subgroup}] ${title} - ${episode} - ", RegexOptions.Compiled`

#### 3. Chinese LoliHouse/ZERO/Lilith-Raws/Skymoon-Raws/orion origin releases don't use the expected brackets, normalize using brackets

```regex
^\[(?<subgroup>[^\]]*?(?:LoliHouse|ZERO|Lilith-Raws|Skymoon-Raws|orion origin)[^\]]*?)\](?<title>[^\[\]]+?)(?: - (?<episode>[0-9-]+)\s*|\[第?(?<episode>[0-9]+(?:-[0-9]+)?)话?(?:END|完)?\])\[
```
Options/args: `"[${subgroup}][${title}][${episode}][", RegexOptions.Compiled`

#### 4. Most Chinese anime releases contain additional brackets/separators for chinese and non-chinese titles, remove junk first and if it has S0x as season number, convert it to Sx

```regex
^\[(?<subgroup>[^\]]+)\](?:\s?★[^\[ -]+\s?)?\[?(?:(?<chinesetitle>(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*?)(?:\]\[|\s*[_/·]\s*)){0,2}(?<title>[^\[\]]+?)(?:\s(?:S?(?<!\d+)((0)(?<season>\d)|(?<season>[1-9]\d))(?!\d+)))\]?(?:\[\d{4}\])?\[第?(?<episode>[0-9]+(?:-[0-9]+)?)(?:话|集)?(?: ?END|完| ?Fin)?\]
```
Options/args: `"[${subgroup}] ${title} S${season} - ${episode} ", RegexOptions.Compiled`

#### 5. Some Chinese releases don't include a separation between Chinese and English titles within the same bracketed group

```regex
^\[(?<subgroup>[^\]]+)\]\[(?<chinesetitle>(?<![^a-zA-Z0-9])[^a-zA-Z0-9]+)(?<title>[^\]]+?)\](?:\[\d{4}\])?\[第?(?<episode>[0-9]+(?:-[0-9]+)?)(?:话|集)?(?: ?END|完| ?Fin)?\]
```
Options/args: `"[${subgroup}] ${title} - ${episode} ", RegexOptions.Compiled`

#### 6. Chinese season packs, that may not actually have the full season so treated as multi-episode

```regex
^\[(?<subgroup>[^\]]+)\](?:\s?★[^\[ -]+\s?)?\[?(?:(?<chinesetitle>(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*?)(?:\]\[|\s*[_/·]\s*)){0,2}(?<title>[^\]]+?)\]?(?:\[\d{4}\])?\[(?:第|全)?(?<episode>[0-9]{1,4}-[0-9]{1,4})(?:话|集)?\](?:\[(?<language>.+?)\+.+?\])[_. ](?<title>.+?)S(?<season>[0-9]{1,2})(?<rest>.+?)$
```
Options/args: `"${title}S${season}E${episode}.${language}${rest}", RegexOptions.Compiled`

#### 7. Chinese season packs, that actually have the full season so treated as multi-episode

```regex
^\[(?<subgroup>[^\]]+)\](?:\s?★[^\[ -]+\s?)?\[?(?:(?<chinesetitle>(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*?)(?:\]\[|\s*[_/·]\s*)){0,2}(?<title>[^\]]+?)\]?(?:\[\d{4}\])?\[(?:全)?(?<episode>[0-9]{1,4})(?:话|集)?\](?:\[(?<language>.+?)\+.+?\])[_. ](?<title>.+?)S(?<season>[0-9]{1,2})(?<rest>.+?)$
```
Options/args: `"${title}S${season}.${language}${rest}", RegexOptions.Compiled`

#### 8. Most Chinese anime releases contain additional brackets/separators for chinese and non-chinese titles, remove junk and replace with normal anime pattern

```regex
^\[(?<subgroup>[^\]]+)\](?:\s?★[^\[ -]+\s?)?\[?(?:(?<chinesetitle>(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*?)(?:\]\[|\s*[_/·]\s*)){0,2}(?<title>[^\]]+?)\]?(?:\[\d{4}\])?\[第?(?<episode>[0-9]{1,4}(?:-[0-9]{1,4})?)(?:话|集)?(?: ?END|完| ?Fin)?\]
```
Options/args: `"[${subgroup}] ${title} - ${episode} ", RegexOptions.Compiled`

#### 9. Some Chinese anime releases contain both Chinese and English titles, remove the Chinese title first and if it has S0x as season number, convert it to Sx

```regex
^\[(?<subgroup>[^\]]+)\](?:\s)(?:(?<chinesetitle>(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*?)(?:\s/\s))(?<title>[^\[\]]+?)(?:\s(?:S?(?<!\d+)((0)(?<season>\d)|(?<season>[1-9]\d))(?!\d+)))(?:[- ]+)(?<episode>[0-9]+(?:-[0-9]+)?)话?(?:END|完)?
```
Options/args: `"[${subgroup}] ${title} S${season} - ${episode} ", RegexOptions.Compiled`

#### 10. Some Chinese anime releases contain both English and Chinese titles, remove the Chinese title and replace with normal anime pattern

```regex
^\[(?<subgroup>[^\]]+)\](?:\s)(?:(?<title>[^\]]+?)(?:\s/\s))(?<chinesetitle>(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*?)(?:[- ]+)(?<episode>[0-9]+(?:-[0-9]+)?(?![a-z]))话?(?:END|完)?
```
Options/args: `"[${subgroup}] ${title} - ${episode} ", RegexOptions.Compiled`

#### 11. Some Chinese anime releases contain both Chinese and English titles, remove the Chinese title and replace with normal anime pattern

```regex
^\[(?<subgroup>[^\]]+)\](?:\s)(?:(?<chinesetitle>(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*?)(?:\s/\s))(?<title>[^\]]+?)(?:[- ]+)(?<episode>[0-9]+(?:-[0-9]+)?(?![a-z]))话?(?:END|完)?
```
Options/args: `"[${subgroup}] ${title} - ${episode} ", RegexOptions.Compiled`

#### 12. GM-Team releases with lots of square brackets

```regex
^\[(?<subgroup>[^\]]+)\](?:(?<chinesubgroup>\[(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*\])+)\[(?<title>[^\]]+?)\](?<junk>\[[^\]]+\])*\[(?<episode>[0-9]+(?:-[0-9]+)?)( END| Fin)?\]
```
Options/args: `"[${subgroup}] ${title} - ${episode} ", RegexOptions.Compiled`

#### 13. Some Chinese anime releases contain both Chinese and English titles separated by | instead of /, remove the Chinese title and replace with normal anime pattern

```regex
^\[(?<subgroup>[^\]]+)\](?:\s)(?:(?<chinesetitle>(?=[^\]]*?[\u4E00-\u9FCC])[^\]]*?)(?:\s\|\s))(?<title>[^\]]+?)(?:[- ]+)(?<episode>[0-9]+(?:-[0-9]+)?(?![a-z]))话?(?:END|完)?
```
Options/args: `"[${subgroup}] ${title} - ${episode} ", RegexOptions.Compiled`

#### 14. Spanish releases with information in brackets

```regex
^(?<title>.+?(?=[ ._-]\()).+?\((?<year>\d{4})\/(?<info>S[^\/]+)
```
Options/args: `"${title} (${year}) - ${info} ", RegexOptions.Compiled`

#### 1.5.2 `PathPreSubstitutionRegex` (ParserCommon.cs L54–58)
#### 1. Season and episode number separated by a space (S02 01)

```regex
(?<separator>^|[-_. ])S(?<season>(?<!\d+)\d{1,2}(?!\d+)) (?<episode>(?<!\d+)\d{1,2}(?!\d+))(?!\d+)
```
Options/args: `"${separator}S${season}E${episode}", RegexOptions.IgnoreCase | RegexOptions.Compiled`

#### 1.5.3 Website prefix/postfix and torrent suffix cleaners (ParserCommon.cs L60–70) — all replace with `""`
- L60 `WebsitePrefixRegex`

```regex
^(?:(?:\[|\()\s*)?(?:www\.)?[-a-z0-9-]{1,256}\.(?<!Naruto-Kun\.)(?:[a-z]{2,6}\.[a-z]{2,6}|xn--[a-z0-9-]{4,}|[a-z]{2,})\b(?:\s*(?:\]|\))|[ -]{2,})[ -]*
```
  args: `string.Empty, RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L64 `WebsitePostfixRegex`

```regex
(?:\[\s*)?(?:www\.)?[-a-z0-9-]{1,256}\.(?:xn--[a-z0-9-]{4,}|[a-z]{2,6})\b(?:\s*\])$
```
  args: `string.Empty, RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L68 `CleanTorrentSuffixRegex`

```regex
\[(?:ettv|rartv|rarbg|cttv|publichd)\]$
```
  args: `string.Empty, RegexOptions.IgnoreCase | RegexOptions.Compiled`

Note `WebsitePrefixRegex` has a negative lookbehind `(?<!Naruto-Kun\.)` so the anime group "Naruto-Kun.Hu" is not
stripped as a website, and requires the domain to be followed by `]`/`)` or **two or more** `[ -]` characters.

### 1.6 `ParseMatchCollection(matches, releaseTitle)` — how numbers are derived (Parser.cs L1018–1275)

Let `m0 = matches[0]`.

* `seriesName = m0.Groups["title"].Value` (**last** capture if `title` repeats) with `.`→space and `_`→space; then
  `RequestInfoRegex` (`^(?:\[.+?\])+`, case-sensitive) removed; then `Trim(' ')` (spaces only).
* `airYear = int.TryParse(m0.Groups["airyear"].Value)` (0 when group absent/failed).
* `lastIdx = m0.Groups["title"].EndIndex()` (0 when no title group).

**Standard path (`airYear < 1900`)** — note the Japanese-variety pattern (#76) captures a **2-digit** `airyear`, so it
always takes this path and its date is ignored.
1. `result = { ReleaseTitle = releaseTitle, SeasonNumbers = [0], EpisodeNumbers = [], AbsoluteEpisodeNumbers = [] }`.
2. **For each match `mg` in `matches`** (all of them, in order; later matches overwrite earlier):
   * `eps = mg.Groups["episode"].Captures`, `abs = mg.Groups["absoluteepisode"].Captures`.
   * If `eps` non-empty: `first = ParseNumber(eps.First)`, `last = ParseNumber(eps.Last)`; if `first > last` →
     **return null** (next regex is tried). `EpisodeNumbers = [first..=last]` (a **range**, intermediate captures ignored:
     `S01E01E03` → 1,2,3). `lastIdx = max(lastIdx, eps.Last.EndIndex)`. `mg.special` success → `Special = true`;
     `mg.splitepisode` success → `IsSplitEpisode = true`.
   * If `abs` non-empty: `first/last = ParseDecimal(...)` (InvariantCulture, `NumberStyles.Float`); `first > last` → return null.
     If either has a fractional part: if `abs.Count != 1` → **return null**; else `SpecialAbsoluteEpisodeNumbers = [first]`,
     `Special = true`, `lastIdx = max(lastIdx, abs.First.EndIndex)`. Otherwise `AbsoluteEpisodeNumbers = [(int)first..=(int)last]`,
     `mg.special` success → `Special = true`, `lastIdx = max(lastIdx, abs.Last.EndIndex)`.
   * If **both** empty (uses `m0`, not `mg`): `m0.extras` non-blank → `IsSeasonExtra = true` (and continue below);
     then: `m0.seasonpart` non-blank → `SeasonPart = int`, `IsPartialSeason = true`; else if `m0.special` success →
     `Special = true`; else → `FullSeason = true`.
   * If `eps.Count == 2` and `m0.episodecount` success and `eps.Last.Value == m0.episodecount.Value` (string compare) →
     `EpisodeNumbers = []`, `FullSeason = true` (e.g. `S01E01-06 of 06`… only when the *text* of the last episode equals the count text).
3. Seasons: for each capture of `m0.Groups["season"]` that `int.TryParse`s: add, `lastIdx = max(lastIdx, capture.EndIndex)`.
   `distinct = seasons.Distinct().OrderBy()`.
   * `Count == 1` or `Count > 2` → `SeasonNumbers = distinct` (e.g. `S01 S03 S05` → [1,3,5]).
   * `Count == 2` → `SeasonNumbers = [min..=max]` (range fill: `S01-S05` → 1..5; `S01 S03` → 1,2,3).
   * `Count == 0` and `AbsoluteEpisodeNumbers` empty and `EpisodeNumbers` non-empty → `SeasonNumbers = [1]`, `IsMiniSeries = true`.
   * otherwise stays `[0]`.

**Daily path (`airYear >= 1900`)**:
1. If `m0.ambiguousairmonth` and `m0.ambiguousairday` both succeeded: if **both ≤ 12** → throw `InvalidDateException`
   (→ ParseTitle `break` → null). Else use them.
   Else if `m0.shortairmonth` succeeded: map `jan..dec` (lowercased) → 1..12 (unknown → InvalidDateException); day = `airday`.
   Else month = `airmonth`, day = `airday` (`Convert.ToInt32`).
2. If `month > 12` → swap day and month.
3. `new DateTime(year, month, day)`; failure → InvalidDateException.
4. `airDate > DateTime.Now.AddDays(14).Date` → InvalidDateException (**local clock**; tests depend on "now").
5. `airDate < 1970-01-01` and `m0.titleyear` blank → InvalidDateException (pattern #1 / Plex DVR allows older dates).
6. `lastIdx = max(lastIdx, EndIndex of m0 airyear, airmonth, airday)` (failed groups contribute 0).
7. `result = { ReleaseTitle, SeasonNumbers = [0], AirDate = airDate.ToString("yyyy-MM-dd") }`; `m0.part` success → `DailyPart = int`.
   (EpisodeNumbers/AbsoluteEpisodeNumbers are empty arrays from the constructor.)

**Common tail:**
* `ReleaseTokens = lastIdx < releaseTitle.Length ? releaseTitle.Substring(lastIdx) : releaseTitle` —
  **`lastIdx` was computed on `simpleTitle`** (after SimpleTitleRegex/website stripping/six-digit-date rewrite) but is
  applied to `releaseTitle`. Reproduce this exactly or language/"Special" detection will diverge.
* `SeriesTitle = seriesName`; `SeriesTitleInfo = GetSeriesTitleInfo(seriesName, matches)`:
  * `Title = seriesName`.
  * `YearInTitleRegex` (`^(?<title>.+?)[-_. ]+?[\(\[]?(?<year>\d{4})[\]\)]?`, not end-anchored, no range check) → if match:
    `TitleWithoutYear = title group`, `Year = int(year)`; else `TitleWithoutYear = Title`, `Year = 0`.
    (Quirk: "The 4400" → TitleWithoutYear "The", Year 4400 — handled later by ParsingService fallbacks.)
  * `TitleComponentsRegex` on `TitleWithoutYear` → `AllTitles` = all `title` captures (`A (B)`, `A | B`, `A AKA B`, `A / B`);
    else if `m0.Groups["title"].Captures.Count > 1` → `AllTitles` = each capture with `.`/`_`→space; else `AllTitles = null`.

**Number parsing** (`ParseNumber`/`ParseDecimal`): input `Normalize(FormKC)`, then every `char.IsNumber` char is replaced by
`char.GetNumericValue(c)` (so fullwidth/Arabic-Indic digits become ASCII; NB `GetNumericValue` returns a double, e.g.
`'５'`→"5"), then `int.TryParse`; `ParseNumber` falls back to index in
`["zero","one","two","three","four","five","six","seven","eight","nine"]` (case-insensitive; used by the "Part One…Nine"
pattern #96); otherwise `FormatException` (→ whole parse returns null).

#### 1.6.1 Ambiguity, ties and validation — summary

* **Pattern precedence is the only tie-breaker**: the first ReportTitleRegex (array order) with ≥1 match whose
  ParseMatchCollection is non-null wins; there is no scoring between candidate patterns.
* A pattern is skipped (next pattern tried) when episode or absolute ranges are descending (`first > last`) or when a
  fractional absolute number appears with more than one absolute capture.
* An **invalid/ambiguous date aborts the whole parse** (InvalidDateException → `break` → null): both ambiguous day and month
  ≤ 12 (`04.05.2018`-style patterns #85/#86), impossible dates, dates > today+14 days, dates < 1970-01-01 unless the Plex
  DVR pattern (#1, `titleyear`) matched, unknown short month. Year ranges embedded in patterns: #1 `19[4-9]\d|20\d\d`,
  #2 `19[6-9]\d|20\d\d`, #89 `19[4-9]\d|20\d\d`; other daily patterns accept any `\d{4}` (then the 1970/+14-day checks apply);
  the six-digit `yyMMdd` rewrite always assumes `20yy`.
* Month > 12 → month and day are **swapped** (scene typo tolerance); if the result is still invalid the date check fails.
* Several matches of the same pattern: title/season/extras/seasonpart/episodecount (and `special` for season-only results)
  come from the **first** match; episode/absolute numbers come from the **last** match that has them, while `Special` /
  `IsSplitEpisode` are set (never cleared) by any match that has episodes/absolutes plus the group.
* Two distinct seasons → inclusive range; one or ≥3 → as listed (no gap filling).
* No season but episodes → season 1 + `IsMiniSeries`; absolute-only → season `[0]` (resolved later via scene mapping/absolute).
* `ParseNumber` failures (e.g. unexpected Unicode) abort the parse (caught by the outer catch → null).

### 1.7 All other regexes in Parser.cs (L470–572), verbatim

Notes before the list:
* `RejectHashedReleasesRegexes[0,10,11,12,13]` are **not end-anchored**: any title (extension removed) whose first 24
  characters are ASCII alphanumerics is rejected (`[13]` = `^[0-9a-zA-Z]{24}` subsumes the 26/30/32/39 variants).
  These are applied to `RemoveFileExtension(title)`; options as listed (most are case-sensitive).
* `SimpleTitleRegex` (replace with "") is the token stripper run before ReportTitleRegex: removes `480p…2160p/i`,
  standalone `2k/4k/8k`, `x264/h.265/x 265…`, `DD5.1`, `<>?*`, explicit WxH resolutions, `8bit/10bit/10-b`.
* `MultiRegex` is only used by `HasMultipleLanguages` (Parser), used by the decision/aggregation code (not by ParseTitle).
* `SpecialEpisodeWordRegex` is declared but its only use is **commented out** in `NormalizeEpisodeTitle`.

- L472 `SpecialEpisodeTitleRegex[0]`

```regex
\.S\d+E00\.(?<episodetitle>.+?)(?:\.(?:720p|1080p|2160p|HDTV|WEB|WEBRip|WEB-DL)\.|$)
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L475 `SpecialEpisodeTitleRegex[1]`

```regex
\.S\d+\.Special\.(?<episodetitle>.+?)(?:\.(?:720p|1080p|2160p|HDTV|WEB|WEBRip|WEB-DL)\.|$)
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L482 `RejectHashedReleasesRegexes[0]` — Generic match for md5 and mixed-case hashes (case-SENSITIVE)

```regex
^[0-9a-zA-Z]{32}
```
  args: `RegexOptions.Compiled`

- L485 `RejectHashedReleasesRegexes[1]` — Generic match for shorter lower-case hashes

```regex
^[a-z0-9]{24}$
```
  args: `RegexOptions.Compiled`

- L489 `RejectHashedReleasesRegexes[2]` — Format seen on some NZBGeek releases (strict: close to valid 101 numbering)

```regex
^[A-Z]{11}\d{3}$
```
  args: `RegexOptions.Compiled`

- L490 `RejectHashedReleasesRegexes[3]` — (NZBGeek, lower-case variant)

```regex
^[a-z]{12}\d{3}$
```
  args: `RegexOptions.Compiled`

- L493 `RejectHashedReleasesRegexes[4]` — Backup filename (Unknown origins)

```regex
^Backup_\d{5,}S\d{2}-\d{2}$
```
  args: `RegexOptions.Compiled`

- L496 `RejectHashedReleasesRegexes[5]` — 123 - Started appearing December 2014

```regex
^123$
```
  args: `RegexOptions.Compiled`

- L499 `RejectHashedReleasesRegexes[6]` — abc - Started appearing January 2015

```regex
^abc$
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L502 `RejectHashedReleasesRegexes[7]` — abc - Started appearing 2020

```regex
^abc[-_. ]xyz
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L505 `RejectHashedReleasesRegexes[8]` — b00bs - Started appearing January 2015

```regex
^b00bs$
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L508 `RejectHashedReleasesRegexes[9]` — 170424_26 - Started appearing August 2018 (NO options at all)

```regex
^\d{6}_\d{2}$
```

- L511 `RejectHashedReleasesRegexes[10]` — additional Generic match for mixed-case hashes. - Started appearing Dec 2020

```regex
^[0-9a-zA-Z]{30}
```
  args: `RegexOptions.Compiled`

- L514 `RejectHashedReleasesRegexes[11]` — additional Generic match for mixed-case hashes. - Started appearing Jan 2021

```regex
^[0-9a-zA-Z]{26}
```
  args: `RegexOptions.Compiled`

- L517 `RejectHashedReleasesRegexes[12]` — additional Generic match for mixed-case hashes. - Started appearing Jan 2021

```regex
^[0-9a-zA-Z]{39}
```
  args: `RegexOptions.Compiled`

- L520 `RejectHashedReleasesRegexes[13]` — additional Generic match for mixed-case hashes. - Started appearing Jan 2021

```regex
^[0-9a-zA-Z]{24}
```
  args: `RegexOptions.Compiled`

- L525 `SeasonFolderRegexes[0]` (used by ValidateBeforeParsing; case-SENSITIVE)

```regex
^(Season[ ._-]*\d+|Specials)$
```
  args: `RegexOptions.Compiled`

- L529 `ReversedTitleRegex`

```regex
(?:^|[-._ ])(p027|p0801|\d{2,3}E-?\d{2}S)[-._ ]
```
  args: `RegexOptions.Compiled`

- L531 `AnimeSeasonReleaseMetadataRegex`

```regex
\[[^\[\]]+\]|\([^()]+\)|\{[^{}]+\}
```
  args: `RegexOptions.Compiled`

- L533 `AnimeSpecialMarkerRegex`

```regex
\b(special|ova|ovd|ncop|nced)\b
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L536 `NormalizeRegex`

```regex
((?:\b|_)(?<!^)([aà](?!$)|an|the|and|or|of)(?!$)(?:\b|_))|\W|_
```
  args: `string.Empty, RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L540 `PercentRegex`

```regex
(?<=\b\d+)%
```
  args: `RegexOptions.Compiled`

- L542 `SimpleTitleRegex`

```regex
(?:(480|540|576|720|1080|1440|2160)[ip]|(?<![a-z0-9])[248]k(?![a-z0-9])|[xh][\W_]?26[456]|DD\W?5\W1|[<>?*]|848x480|1280x720|1920x1080|3840x2160|4096x2160|(?<![a-f0-9])(8|10)[ -]?(b(?![a-z0-9])|bit))\s*?
```
  args: `string.Empty, RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L548 `SixDigitAirDateRegex`

```regex
(?<=[_.-])(?<airdate>(?<!\d)(?<airyear>[1-9]\d{1})(?<airmonth>[0-1][0-9])(?<airday>[0-3][0-9]))(?=[_.-])
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L551 `CleanQualityBracketsRegex`

```regex
\[[a-z0-9 ._-]+\]$
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L554 `YearInTitleRegex`

```regex
^(?<title>.+?)[-_. ]+?[\(\[]?(?<year>\d{4})[\]\)]?
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L557 `TitleComponentsRegex`

```regex
^(?:(?<title>.+?) \((?<title>.+?)\)|(?<title>.+?) \| (?<title>.+?)|(?<title>.+?) AKA (?<title>.+?)|(?<title>.+?) / (?<title>.+?))$
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L560 `PartRegex`

```regex
\(\d+\)$
```
  args: `RegexOptions.Compiled`

- L561 `PunctuationRegex`

```regex
[^\w\s]
```
  args: `RegexOptions.Compiled`

- L562 `ArticleWordRegex`

```regex
^(a|an|the)\s
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L563 `SpecialEpisodeWordRegex`

```regex
\b(part|special|edition|christmas)\b\s?
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L564 `DuplicateSpacesRegex`

```regex
\s{2,}
```
  args: `RegexOptions.Compiled`

- L565 `SeasonFolderRegex`

```regex
^(?:S|Season|Saison|Series|Stagione|Sezon)[-_. ]*(?<season>(?<!\d+)\d{1,4}(?!\d+))(?:[_. ]+(?!\d+)|$)
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L566 `SimpleEpisodeNumberRegex`

```regex
^[ex]?(?<episode>(?<!\d+)\d{1,3}(?!\d+))(?:[ex-](?<episode>(?<!\d+)\d{1,3}(?!\d+)))?(?:[_. ](?!\d+)(?<remaining>.+)|$)
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L568 `RequestInfoRegex`

```regex
^(?:\[.+?\])+
```
  args: `RegexOptions.Compiled`

- L572 `MultiRegex`

```regex
[-_. \[](?<multi>multi|multilang|multilanguage)[-_. \]]
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

`ShortMonths` (L574): `jan:1 feb:2 mar:3 apr:4 may:5 jun:6 jul:7 aug:8 sep:9 oct:10 nov:11 dec:12`.

### 1.8 Title normalisation functions

* **`CleanSeriesTitle(title)`** (L910): null/whitespace → returned as is; `long.TryParse(title)` succeeds (all digits,
  optional sign/whitespace per .NET rules) → returned as is; `PercentRegex` → replace `%` with `"percent"`;
  `NormalizeRegex.Replace(…, "")`; `ToLowerInvariant()`; `RemoveDiacritics()` (NuGet **Diacritical.Net 1.0.5** lookup
  table — a Rust port needs an equivalent table; NFD+strip-marks is close but not identical, **UNVERIFIED** equivalence).
  Effect of NormalizeRegex: deletes every non-word char and `_`, and deletes the words `a`, `à`, `an`, `the`, `and`, `or`,
  `of` when they are whole words, **not at position 0** (`(?<!^)`) and **not at end** (`(?!$)`); `a`/`à` additionally
  must not be the last char. E.g. "The Office" → `theoffice`; "Law & Order" → `laworder`.
* **`NormalizeEpisodeTitle(title)`** (L929): first successful `SpecialEpisodeTitleRegex` → take `episodetitle`;
  `PartRegex` (`\(\d+\)$`) → ""; `PunctuationRegex` (`[^\w\s]`) → " "; `DuplicateSpacesRegex` → " "; `Trim().ToLower()`.
* **`NormalizeTitle(title)`** (L950): `PunctuationRegex` → ""; `ArticleWordRegex` (`^(a|an|the)\s`) → "";
  `DuplicateSpacesRegex` → " "; `Trim().ToLower()`.

### 1.9 `ParsePath(path)` (L590–672)

1. `fileName = FileInfo(path).Name`; apply every `PathPreSubstitutionRegex` (`S02 01` → `S02E01`).
2. `result = ParseTitle(fileName)`.
3. If `SimpleEpisodeNumberRegex` matches `fileName` AND the parent dir name is non-null AND (`result == null` OR
   `result.IsMiniSeries` OR `result.AbsoluteEpisodeNumbers.Any()`): if `SeasonFolderRegex` matches the **parent directory
   name** with a `season` group → build `pathTitle = $"S{season}E{first:00}"` (+ `-E{last:00}` if first≠last)
   (+ `" " + remaining` if the `remaining` group matched) and **return `ParseTitle(pathTitle)`** (can be null).
4. If `result == null` and `Path.GetFileNameWithoutExtension(fileName)` is an int `n`: `result = ParseTitle(dirName)`; keep it
   only if its `AbsoluteEpisodeNumbers` contains `n` (→ set to `[n]`) or else its `EpisodeNumbers` contains `n` (→ `[n]`); else null.
5. If still null → `ParseTitle(dirName + " " + fileName)`.
6. If still null → `ParseTitle(dirName + fileExtension)`.

### 1.10 Other parser entry points

* **`ParseSeasonTitle(title)`** (L851, v5): `releaseTitle = RemoveFileExtension(title)`; if `AnimeSpecialMarkerRegex`
  matches → null. `seriesTitle = AnimeSeasonReleaseMetadataRegex.Replace(releaseTitle, " ")` (drops all `[..]`, `(..)`,
  `{..}` groups), collapse `\s{2,}` → " ", Trim. If empty or still contains any of `[]{}()` → null. Else returns
  `{ ReleaseTitle, SeriesTitle = seriesTitle, SeriesTitleInfo{Title=TitleWithoutYear=seriesTitle}, EpisodeNumbers=[],
  AbsoluteEpisodeNumbers=[], FullSeason=true, IsSeasonTitle=true, Languages = ParseLanguages(releaseTitle),
  Quality = ParseQuality(title), ReleaseGroup = ParseReleaseGroup(releaseTitle) }` with `SeasonNumbers = []` (empty!).
  Used by ParsingService only when ParseTitle failed and mapping by scene title is attempted (§5).
* **`ParseSeasonFolder(folderName)`** (L702): `SeasonFolderRegex` match with `season` → `ParseNumber(season)`; else
  `ParseTitle(folderName)` and if `FullSeason && SeasonNumbers.Count == 1` → that season; else null.
* **`SimplifyTitle`** (L674): see §1.1.

### 1.11 `FileExtensions.RemoveFileExtension` (`NzbDrone.Core/MediaFiles/FileExtensions.cs`)

`\.[a-z0-9]{2,4}$` (IgnoreCase) is removed **only if** the lower-cased extension is in `MediaFileExtensions.Extensions`
(§3.9) or in `{".par2", ".nzb"}`. So `.mkv`, `.strm`, `.m3u`, `.ifo` are stripped; `.srt`/`.rar`/`.nfo` are **not**;
`.dvr-ms` is in the list but can never be stripped (the `-` is outside the `[a-z0-9]{2,4}` class).

### 1.12 `ReleaseGroupParser.ParseReleaseGroup(title)` (`Parser/ReleaseGroupParser.cs`)

Flow:
1. `title = title.Trim()`; `RemoveFileExtension`.
2. PreSubstitution: apply in order, **break after first match** (see §1.5).
3. `WebsitePrefixRegex` → ""; `CleanTorrentSuffixRegex` → "" (WebsitePostfix is **not** applied here).
4. `AnimeReleaseGroupRegex` match → return `subgroup` (leading `[Group]`, no leading/trailing whitespace inside).
5. `CleanReleaseGroupRegex` → "" (strips everything up to and including ` SxxEyy ` **and** trailing junk suffixes like
   `-RP`, `-1`, `-NZBGeek`, `-Obfuscated`, `-postbot`, `-Rakuv…`, etc.; alternation means either part may apply).
6. `ExceptionReleaseGroupRegex.Matches` non-empty → **last** match's `releasegroup`.
7. `ExceptionReleaseGroupRegexExact.Matches` non-empty → **last** match's `releasegroup`.
8. `ReleaseGroupRegex.Matches` non-empty → last match's `releasegroup`; return null if it parses as int or matches
   `InvalidReleaseGroupRegex` (`^([se]\d+|[0-9a-f]{8})$`); else return it.
9. else null.

Regexes (verbatim; note `\k<part2>` backreference inside a lookbehind and Latin-1 letter ranges `À-ÖØ-öø-ÿ`):

- L9 `ReleaseGroupRegex`

```regex
-(?<releasegroup>[A-Za-zÀ-ÖØ-öø-ÿ0-9]+(?<part2>-[A-Za-zÀ-ÖØ-öø-ÿ0-9]+)?(?!.+?(?:HDTV|SDTV|480p|576p|720p|1080p|2160p)))(?<!(?:HDTV|SDTV|WEB-DL|Blu-Ray|480p|576p|720p|1080p|2160p|DTS-HD|DTS-X|DTS-MA|DTS-ES|-ES|-EN|-CAT|-GER|-FRA|-FRE|-ITA|\d{1,2}-bit|[ ._]\d{4}-\d{2}|-\d{2})(?:\k<part2>)?)(?:\b|[-._ ]|$)|[-._ ]\[(?<releasegroup>[A-Za-zÀ-ÖØ-öø-ÿ0-9]+)\]$
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L12 `InvalidReleaseGroupRegex`

```regex
^([se]\d+|[0-9a-f]{8})$
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L14 `AnimeReleaseGroupRegex`

```regex
^(?:\[(?<subgroup>(?!\s).+?(?<!\s))\](?:_|-|\s|\.)?)
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L19 `ExceptionReleaseGroupRegexExact`

```regex
(?:(?<releasegroup>Fight-BB|VARYG|E\.N\.D|KRaLiMaRKo|BluDragon|DarQ|KCRT|BEN[_. ]THE[_. ]MEN|TAoE|QxR|Vialle)\b)
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L22 `ExceptionReleaseGroupRegex`

```regex
(?<=[._ \[])(?<releasegroup>(Joy|ImE|UTR|t3nzin|Anime Time|Project Angel|Hakata Ramen|HONE|Vyndros|SEV|Garshasp|Kappa|Natty|RCVR|SAMPA|YOGI|r00t|EDGE2020|Celdra)(?=\]|\)))
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L24 `CleanReleaseGroupRegex`

```regex
^(.*?[-._ ](S\d+E\d+)[-._ ])|(-(RP|1|NZBGeek|\[N-Z-B\]|Obfuscated|Scrambled|sample|Pre|postbot|xpost|Rakuv[a-z0-9]*|WhiteRev|BUYMORE|AsRequested|AlternativeToRequested|GEROV|Z0iDS3N|Chamele0n|4P|4Planet|AlteZachen|RePACKPOST))+$
```
  args: `string.Empty, RegexOptions.IgnoreCase | RegexOptions.Compiled`


---

## 2. `ParsedEpisodeInfo` (`NzbDrone.Core/Parser/Model/ParsedEpisodeInfo.cs`) and related models

| Field | Type / default | Semantics |
|---|---|---|
| `ReleaseTitle` | string | Title after un-reversal + extension removal + `【】`→`[]` + **PreSubstitution** (i.e. possibly rewritten). |
| `SeriesTitle` | string | `title` group (last capture), `.`/`_`→space, leading `[..]` blocks removed, trimmed. May be `""` (title-less patterns). |
| `SeriesTitleInfo` | `{Title, TitleWithoutYear, Year:int (0=none), AllTitles:string[]? }` | §1.6. |
| `Quality` | `QualityModel` | `QualityParser.ParseQuality(original title incl. extension)`. |
| `SeasonNumbers` | `List<int>`, default `[]` | `[0]` for daily/absolute-only/no season; `[1]` for mini-series; range-filled for 2 distinct seasons. (v5: list; v4 had `int SeasonNumber` — **UNVERIFIED** for v4.) |
| `EpisodeNumbers` | `int[]`, default `[]` | Contiguous range first..last of `episode` captures (last match in MatchCollection wins). |
| `AbsoluteEpisodeNumbers` | `int[]`, default `[]` | Contiguous range of integer `absoluteepisode` captures. |
| `SpecialAbsoluteEpisodeNumbers` | `decimal[]`, default `[]` | Single fractional absolute (e.g. `12.5`) → `[12.5]`, `Special=true`. |
| `AirDate` | string `yyyy-MM-dd` or null | Daily releases. |
| `Languages` | `List<Language>`, default `[]` | `LanguageParser.ParseLanguages(ReleaseTokens)` (§4). |
| `FullSeason` | bool | No episode/absolute captures and not partial/special; or `SxEa-b of b` form. |
| `IsPartialSeason` | bool | `seasonpart` captured (`S01 Part 2`, `Vol.1`, `p2`, `e2` after season-only). |
| `IsMultiSeason` | computed `SeasonNumbers.Count > 1` | |
| `IsSeasonExtra` | bool | `extras` group (`EXTRAS`/`SUBPACK`) on a season-only release. |
| `IsSeasonTitle` | bool | v5 `ParseSeasonTitle` result (anime season named by its own title). |
| `IsSplitEpisode` | bool | `S01E05a`… pattern #6 (`splitepisode` group). |
| `IsMiniSeries` | bool | Episodes found but no season → season forced to 1. |
| `Special` | bool | `special` group (OVA/OVD/special/`.5 [SP]`), fractional absolute, or FullSeason + "Special" in ReleaseTokens. |
| `ReleaseGroup` | string? | `ReleaseGroupParser` result, overridden by anime `subgroup`. |
| `ReleaseHash` | string | 8-hex CRC from anime patterns (`hash` group) with brackets trimmed; `""` if none or `1280x720`. |
| `SeasonPart` | int | Value of `seasonpart`. |
| `ReleaseTokens` | string | Tail of ReleaseTitle after the last consumed number (see offset caveat §1.6). |
| `DailyPart` | int? | `Part N` after an air date (pattern #56). |

Computed properties:
* `IsDaily` = `!IsNullOrWhiteSpace(AirDate)`.
* `IsAbsoluteNumbering` = `AbsoluteEpisodeNumbers.Any()`.
* `IsPossibleSpecialEpisode` =
  `(AirDate blank && SeriesTitle blank && (EpisodeNumbers.Length == 0 || SeasonNumbers.FirstOrDefault(-1) == 0))
   || (SeriesTitle not blank && Special)
   || (EpisodeNumbers.Length == 1 && EpisodeNumbers[0] == 0)`.
* `IsPossibleSceneSeasonSpecial` = `SeasonNumbers.FirstOrDefault() != 0 && EpisodeNumbers.Length == 1 && EpisodeNumbers[0] == 0`.
* `ReleaseType` (enum `Unknown=0, SingleEpisode=1, MultiEpisode=2, SeasonPack=3, MultiSeasonPack=4`):
  `EpisodeNumbers.Length > 1 || AbsoluteEpisodeNumbers.Length > 1` → MultiEpisode; `== 1` (either) → SingleEpisode;
  `FullSeason` → (IsMultiSeason ? MultiSeasonPack : SeasonPack); else Unknown.
* `ToString()` = `"{SeriesTitle} - {episodeString} {Quality}"`; episodeString: daily w/o episodes → AirDate; FullSeason →
  `SeasonNumbersFormatter.Format(SeasonNumbers, "00")` (`Season 01` / `Seasons 01-03` (contiguous) / `Seasons 01, 03`);
  episodes → `S{season:00}E{eps "00" joined by "-"}`; absolute → `{abs "000" joined "-"}`; Special → `[Unknown Season NN Special]` or `[Unknown Special]`; else `[Unknown Episode]`.

Related models (decision engine inputs):
* `RemoteEpisode`: `Release: ReleaseInfo`, `ParsedEpisodeInfo`, `SceneMapping`, `MappedSeasonNumber: int?`, `Series`,
  `Episodes: List<Episode>`, `EpisodeRequested`, `DownloadAllowed`, `SeedConfiguration`, `CustomFormats`,
  `CustomFormatScore`, `SeriesMatchType`, `Languages` (final, after aggregation), `ReleaseSource`
  (`Unknown=0, Rss=1, Search=2, UserInvokedSearch=3, InteractiveSearch=4, ReleasePush=5`).
  `IsRecentEpisode()` = any episode `AirDateUtc >= UtcNow.Date - 14 days`.
* `ReleaseInfo`: `Guid, Title, Size(long bytes), DownloadUrl, InfoUrl, CommentUrl, IndexerId, Indexer, IndexerPriority,
  SeasonSearchMaximumSingleEpisodeAge, DownloadProtocol, TvdbId, TvRageId, ImdbId, PublishDate, Origin, Source, Container,
  Codec, Resolution, Languages, IndexerFlags, PendingReleaseReason?`; `Age` = whole days since PublishDate (UTC),
  `AgeHours`, `AgeMinutes` (double). `TorrentInfo` adds `MagnetUrl, InfoHash, Seeders?, Peers?`.
* `IndexerFlags` ([Flags]): `Freeleech=1, Halfleech=2, DoubleUpload=4, Internal=8, Scene=16, Freeleech75=32,
  Freeleech25=64, Nuked=128, Subtitles=256`.

---

## 3. Quality parsing (`NzbDrone.Core/Parser/QualityParser.cs`) and the QualityModel

### 3.1 Regexes (verbatim, QualityParser.cs L17–66)

**`SourceRegex` uses `RegexOptions.IgnorePatternWhitespace`** and spans lines L17–29. In .NET's x-mode whitespace
*inside character classes is literal*; in Rust `regex`'s `(?x)` whitespace inside classes is **ignored** — so do NOT port
the multi-line form with `(?x)`. Use this machine-collapsed single-line equivalent (only newline+indentation removed;
verified there is no other whitespace outside character classes), compiled case-insensitively, **without** x-mode:

```regex
\b(?:(?<bluray>BluRay|Blu-Ray|HD-?DVD|BDMux|BD(?!$))|(?<webdl>WEB[-_. ]DL(?:mux)?|WEBDL|AmazonHD|AmazonSD|iTunesHD|MaxdomeHD|NetflixU?HD|WebHD|HBOMaxHD|DisneyHD|[. ]WEB[. ](?:[xh][ .]?26[456]|AVC|HEVC|EAC3|DDP?[ .]?5[. ]1)|[. ](?-i:WEB)$|(?:720|1080|2160)p[-. ]WEB[-. ]|[-. ]WEB[-. ](?:720|1080|2160)p|\b\s\/\sWEB\s\/\s\b|(?:AMZN|ATVP|DP|NF)[. -]WEB[. -](?!Rip))|(?<webrip>WebRip|Web-Rip|WEBMux)|(?<hdtv>HDTV)|(?<bdrip>BDRip|BDLight)|(?<brrip>BRRip)|(?<dvd>DVD|DVDRip|NTSC|PAL|xvidvd)|(?<dsr>WS[-_. ]DSR|DSR)|(?<pdtv>PDTV)|(?<sdtv>SDTV)|(?<tvrip>TVRip))(?:\b|$|[ .])
```

Note the inline `(?-i:WEB)` (case-sensitive `WEB` at end of string after `.`/space).
The original multi-line literal and all other QualityParser regexes, verbatim:

- L17 `SourceRegex`

```regex
\b(?:
                                                                (?<bluray>BluRay|Blu-Ray|HD-?DVD|BDMux|BD(?!$))|
                                                                (?<webdl>WEB[-_. ]DL(?:mux)?|WEBDL|AmazonHD|AmazonSD|iTunesHD|MaxdomeHD|NetflixU?HD|WebHD|HBOMaxHD|DisneyHD|[. ]WEB[. ](?:[xh][ .]?26[456]|AVC|HEVC|EAC3|DDP?[ .]?5[. ]1)|[. ](?-i:WEB)$|(?:720|1080|2160)p[-. ]WEB[-. ]|[-. ]WEB[-. ](?:720|1080|2160)p|\b\s\/\sWEB\s\/\s\b|(?:AMZN|ATVP|DP|NF)[. -]WEB[. -](?!Rip))|
                                                                (?<webrip>WebRip|Web-Rip|WEBMux)|
                                                                (?<hdtv>HDTV)|
                                                                (?<bdrip>BDRip|BDLight)|
                                                                (?<brrip>BRRip)|
                                                                (?<dvd>DVD|DVDRip|NTSC|PAL|xvidvd)|
                                                                (?<dsr>WS[-_. ]DSR|DSR)|
                                                                (?<pdtv>PDTV)|
                                                                (?<sdtv>SDTV)|
                                                                (?<tvrip>TVRip)
                                                                )(?:\b|$|[ .])
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase | RegexOptions.IgnorePatternWhitespace`

- L32 `RawHDRegex`

```regex
\b(?<rawhd>RawHD|Raw[-_. ]HD)\b
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L35 `MPEG2Regex`

```regex
\b(?<mpeg2>MPEG[-_. ]?2)\b
```

- L37 `ProperRegex`

```regex
\b(?<proper>proper)\b
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L40 `RepackRegex`

```regex
\b(?<repack>repack\d?|rerip\d?)\b
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L43 `VersionRegex`

```regex
\d[-._ ]?v(?<version>\d)[-._ ]|\[v(?<version>\d)\]|repack(?<version>\d)|rerip(?<version>\d)|(?:480|576|720|1080|2160)p[._ ]v(?<version>\d)
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L46 `RealRegex`

```regex
\b(?<real>REAL)\b
```
  args: `RegexOptions.Compiled`

- L49 `ResolutionRegex`

```regex
\b(?:(?<R360p>360p)|(?<R480p>480p|480i|640x480|848x480)|(?<R540p>540p)|(?<R576p>576p)|(?<R720p>720p|1280x720|960p)|(?<R1080p>1080p|1920x1080|1440p|FHD|1080i|4kto1080p)|(?<R2160p>2160p|3840x2160|4k[-_. ](?:UHD|HEVC|BD|H26[56])|(?:UHD|HEVC|BD|H26[56])[-_. ]4k))\b
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L53 `AlternativeResolutionRegex`

```regex
\b(?<R2160p>UHD)\b|(?<R2160p>\[4K\])
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L56 `CodecRegex`

```regex
\b(?:(?<x264>x264)|(?<h264>h264)|(?<xvidhd>XvidHD)|(?<xvid>Xvid)|(?<divx>divx))\b
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L59 `OtherSourceRegex`

```regex
(?<hdtv>HD[-_. ]TV)|(?<sdtv>SD[-_. ]TV)
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L61 `AnimeBlurayRegex`

```regex
bd(?:720|1080|2160)|(?<=[-_. (\[])bd(?=[-_. )\]])
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L62 `AnimeWebDlRegex`

```regex
\[WEB\]|[\[\(]WEB[ .]
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L64 `HighDefPdtvRegex`

```regex
hr[-_. ]ws
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L66 `RemuxRegex`

```regex
(?:[_. ]|\d{4}p-|\bHybrid-)(?<remux>(?:(BD|UHD)[-_. ]?)?Remux)\b|(?<remux>(?:(BD|UHD)[-_. ]?)?Remux[_. ]\d{4}p)
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

Case sensitivity traps: `MPEG2Regex` has **no options** (case-sensitive, `MPEG2`/`MPEG-2` uppercase only);
`RealRegex` has only `Compiled` (case-sensitive: only uppercase `REAL`) and is run on the **raw** `name` (underscores not
normalised, so `_REAL_` does not match `\bREAL\b`).

`enum Resolution { R360p=360, R480p=480, R540p=540, R576p=576, R720p=720, R1080p=1080, R2160p=2160, Unknown=0 }`.

### 3.2 `ParseQuality(name)` (L68)

1. null/whitespace → `QualityModel{Quality=Unknown}`. `name = name.Trim()`.
2. `result = ParseQualityName(name)`.
3. If `result.Quality == Unknown` and `!name.ContainsInvalidPathChars()` (`Path.GetInvalidPathChars()`: on Linux only `\0`;
   on Windows also `"<>|` and control chars → platform-dependent): `Quality = MediaFileExtensions.GetQualityForExtension(Path.GetExtension(name))`
   (unknown ext → Unknown), and **both** `SourceDetectionSource` and `ResolutionDetectionSource = Extension` (even if the
   extension mapped to Unknown). `ArgumentException` swallowed.

### 3.3 `ParseQualityName(name)` — exact decision tree (L100–598)

`normalizedName = name.Replace('_', ' ').Trim()`. `result = ParseQualityModifiers(name, normalizedName)` (§3.4).

1. `RawHDRegex` matches → Source&Resolution detection = Name, **Quality = RAWHD**, return.
2. `sourceMatch = SourceRegex.Matches(normalizedName).LastOrDefault()` (**LAST** source token wins);
   `resolution = ParseResolution(normalizedName)`; `codec = CodecRegex.Match` (first); `remux = RemuxRegex.IsMatch`.
3. If resolution ≠ Unknown → ResolutionDetectionSource = Name.
4. If a source matched (SourceDetectionSource = Name), check groups in this order:
   * **bluray**: codec `xvid` or `divx` → **Bluray-480p**; R2160p → remux ? **Bluray-2160p Remux** : **Bluray-2160p**;
     R1080p → remux ? **Bluray-1080p Remux** : **Bluray-1080p**; R576p → **Bluray-576p**; R360p/R480p/R540p → **Bluray-480p**;
     remux && resolution ≠ R720p → **Bluray-1080p Remux**; else **Bluray-720p**.
   * **webdl**: 2160 → WEBDL-2160p; 1080 → WEBDL-1080p; 720 → WEBDL-720p; `name.Contains("[WEBDL]")` (case-sensitive) →
     WEBDL-720p; else **WEBDL-480p**.
   * **webrip**: 2160/1080/720 → WEBRip-2160p/1080p/720p; else **WEBRip-480p**.
   * **hdtv**: `MPEG2Regex` → **RAWHD**; 2160/1080/720 → HDTV-2160p/1080p/720p; `name.Contains("[HDTV]")` → HDTV-720p; else **SDTV**.
   * **bdrip / brrip**: 720 → Bluray-720p; 1080 → Bluray-1080p; 2160 → Bluray-2160p; else **Bluray-480p**.
   * **dvd** → **DVD**.
   * **pdtv / sdtv / dsr / tvrip**: R1080p or normalizedName contains `1080p` (ci) → HDTV-1080p; R720p or contains `720p`
     → HDTV-720p; `HighDefPdtvRegex` (`hr[-_. ]ws`) → ResolutionDetectionSource=Name, HDTV-720p; else **SDTV**.
   (Every listed source group returns, so falling through only happens if none of these groups succeeded — impossible
   in practice; treat as fall-through to step 5.)
5. **No source** (`sourceMatch == null`) && remux && resolution ≠ Unknown: SourceDetectionSource = Unknown;
   R480p → Bluray-480p; R720p → Bluray-720p; R2160p → Bluray-2160p Remux; R1080p → Bluray-1080p Remux
   (other resolutions fall through).
6. **Anime BluRay** (`AnimeBlurayRegex`): SourceDetection=Name; 360/480/540/576 or contains `480p` → **DVD** (sic);
   1080 or contains `1080p` → remux ? Bluray-1080p Remux : Bluray-1080p; 2160 or contains `2160p` → remux ? Bluray-2160p Remux : Bluray-2160p;
   remux && ≠720 → Bluray-1080p Remux; else **Bluray-720p**. (Resolution detection = Name in the explicit branches.)
7. **Anime WEB-DL** (`AnimeWebDlRegex`): 360/480/540/576 or `480p` → WEBDL-480p; 1080/`1080p` → WEBDL-1080p;
   2160/`2160p` → WEBDL-2160p; else **WEBDL-720p**.
8. **Resolution only** (resolution ≠ Unknown): `source = Unknown`; if remux → SourceDetection=Name, `source = BlurayRaw`;
   else `q = GetQualityForExtension(name.GetPathExtension())` (substring from last `.`; "" if none/trailing dot) and if
   `q ≠ Unknown` → SourceDetection=Extension, `source = q.Source`. Then:
   2160 → source Unknown ? **HDTV-2160p** : `FindBySourceAndResolution(source, 2160)`; 1080 → HDTV-1080p / FBSR(1080);
   720 → HDTV-720p / FBSR(720); 360/480/540/576 → **SDTV** / FBSR(source, 480). (ResolutionDetection=Name.)
9. codec `x264` → **SDTV**.
10. `normalizedName.Contains("848x480")` (case-sensitive): ResolutionDetection=Name; contains `"dvd"` (case-**sensitive**,
    lowercase) → DVD (+SourceDetection=Name); else contains `bluray` (ci) → Bluray-480p (+Name); else SDTV.
11. contains `1280x720` (ci): contains `bluray` → Bluray-720p else HDTV-720p.
12. contains `1920x1080` (ci): contains `bluray` → Bluray-1080p else HDTV-1080p.
13. contains `bluray720p` → Bluray-720p; `bluray1080p` → Bluray-1080p; `bluray2160p` → Bluray-2160p (all ci, both detection=Name).
14. `OtherSourceRegex` first match: `sdtv` group → SDTV; `hdtv` group → **HDTV-720p**; (SourceDetection=Name).
15. else Unknown.

`ParseResolution(name)`: `ResolutionRegex.Match` (first match) and `AlternativeResolutionRegex.Match`; if neither → Unknown;
check groups of the main match in order R360p, R480p, R540p, R576p, R720p, R1080p, R2160p; then alternative R2160p
(`UHD` word or `[4K]`). Notable mappings: `960p`→720; `1440p`, `FHD`, `1080i`, `4kto1080p` → 1080; `480i`, `640x480` → 480.

`QualityFinder.FindBySourceAndResolution(source, res)` (Qualities/QualityFinder.cs): exact `Quality.All` match on
(Source, Resolution) → it. Else if `res < 720`: Television → SDTV, Web → WEBDL-480p, WebRip → WEBRip-480p. Else take
qualities with that resolution ordered by `Source` enum value and return the first whose `Source >= source`
(Unknown if none; logs a warning).

### 3.4 Modifiers — `ParseQualityModifiers(name, normalizedName)` (L675)

`result = QualityModel{Quality=Unknown}` → `Revision{Version=1, Real=0, IsRepack=false}`.
1. `VersionRegex.Match(normalizedName)` success → `Version = int(version)`; RevisionDetection=Name.
   (Forms: `<digit>[-._ ]?v<d>[-._ ]` e.g. `01v2 `, `[v2]`, `repack2`, `rerip2`, `720p v2`/`1080p.v2`.)
2. `ProperRegex` (`\bproper\b`, ci) → `Version = versionMatched ? version + 1 : 2`.
3. `RepackRegex` (`\b(repack\d?|rerip\d?)\b`, ci) → `Version = versionMatched ? version + 1 : 2`, `IsRepack = true`.
   (So `REPACK2` → VersionRegex gives 2, then repack → 3. PROPER+REPACK → 2, IsRepack.)
4. `RealRegex.Matches(name)` (raw name, case-sensitive) → `Real = count of matches` (e.g. `REAL.REAL` → 2).

Anime "v2" is just Version=2 via VersionRegex; there is no separate anime flag in the model. The decision engine treats
anime version upgrades specially (§9).

### 3.5 QualityModel / Revision / enums (`NzbDrone.Core/Qualities/`)

* `QualityModel { Quality Quality; Revision Revision; [JsonIgnore] SourceDetectionSource, ResolutionDetectionSource,
  RevisionDetectionSource : QualityDetectionSource }`; default = `(Unknown, new Revision())`.
  `Equals` = same Quality **Id** and Revision equal. `ToString()` = `"{Quality.Name} v{Version}[ Real:{Real}]"`.
  `CompareTo` (profile-independent; rarely used) compares **default weight** (§7.2), then Real, then Version.
* `Revision { int Version = 1; int Real = 0; bool IsRepack = false }`. `Equals` compares **Version and Real only**
  (IsRepack ignored). `CompareTo`: Real first, then Version. Operators `> < >= <=` treat null specially (null < anything).
* `QualityDetectionSource { Unknown=0, Name=1, Extension=2, MediaInfo=3 }`.
* `QualitySource { Unknown=0, Television=1, TelevisionRaw=2, Web=3, WebRip=4, DVD=5, Bluray=6, BlurayRaw=7 }` (order matters
  for QualityFinder).
* `ProperDownloadTypes { PreferAndUpgrade=0, DoNotUpgrade=1, DoNotPrefer=2 }` (config `DownloadPropersAndRepacks`, default
  PreferAndUpgrade — see §9).
* `QualityModelComparer(profile)` (Qualities/QualityModelComparer.cs): compare `profile.GetIndex(quality, respectGroupOrder)`
  of both (§7.3), and only if equal compare `Revision.CompareTo` (Real, then Version).

### 3.6 Extension fallback — `MediaFileExtensions` (`NzbDrone.Core/MediaFiles/MediaFileExtensions.cs`)

Case-insensitive map extension → Quality:
* Unknown: `.webm`
* SDTV: `.m4v .3gp .nsv .ty .strm .rm .rmvb .m3u .ifo .mov .qt .divx .xvid .bivx .nrg .pva .wmv .asf .asx .ogm .ogv .m2v
  .avi .bin .dat .dvr-ms .mpg .mpeg .mp4 .avc .vp3 .svq3 .nuv .viv .dv .fli .flv .wpl`
* DVD: `.img .iso .vob`
* HDTV-720p: `.mkv .ts .wtv`
* Bluray-720p: `.m2ts`

`Extensions` = all keys; `DiskExtensions = {.img,.iso,.vob}`; `StreamingExtensions = {.m3u,.strm}`.
(Media-file import also re-derives quality from MediaInfo resolution — `QualityDetectionSource.MediaInfo` — that is in
`MediaFiles/EpisodeImport/Aggregation/Aggregators/AggregateQuality.cs`, out of scope here, **not audited**.)

---

## 4. Language parsing (`NzbDrone.Core/Parser/LanguageParser.cs`, `Languages/Language.cs`, `Parser/IsoLanguages.cs`)

### 4.1 Regexes (verbatim)

`LanguageRegex` is a C# **interpolated** verbatim string; .NET inserts `LanguagesOnlyRegex.ToString()` (= its pattern),
so the compiled pattern contains every named group **twice** (once in the main alternation, once inside the negative
lookahead). Only captures of the first copy matter (the lookahead is negative, so it never contributes captures).
The negative lookahead rejects a language token followed (via `[-_. ]` + further language tokens) by `sub`/`subs` —
i.e. "ITA.ENG.SUBS" counts as subtitles, not audio.

`CaseSensitiveLanguageRegex` has **no IgnoreCase**; the `(?i)` inside `(?:(?i)(?<!SUB[\W|_|^]))` applies only to the end of
that enclosing group (same scoping in Rust). `[\W|_|^]` is a character class (literal `|`, `_`, `^` plus `\W`), not alternation.

- L20 `CleanSeriesTitleRegex[0]` (RegexReplace, replacement "$1")

```regex
.*?[_. ](S\d{2}(?:E\d{2,4})*[_. ].*)
```
  args: `"$1", RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L23 `LanguagesOnlyRegex`

```regex
(?<english>\b(?:ing|eng)\b)|(?<italian>\b(?:ita|italian)\b)|(?<german>(?:swiss)?german\b|videomann|ger[. ]dub|\bger\b)|(?<flemish>flemish)|(?<greek>greek)|(?<french>(?:_|\b)(?:FR|VF|VF2|VFF|VFI|VFQ|TRUEFRENCH|FRENCH|FRE|FRA)(?:_|\b))|(?<russian>\b(?:rus|ru)\b)|(?<hungarian>\b(?:HUNDUB|HUN)\b)|(?<hebrew>\bHebDub\b)|(?<polish>\b(?:PL\W?DUB|DUB\W?PL|LEK\W?PL|PL\W?LEK)\b)|(?<chinese>\[(?:CH[ST]|BIG5|GB)\]|简|繁|字幕|国语音轨[.+])|(?<bulgarian>\bbgaudio\b)|(?<spanish>\b(?:español|castellano|esp|spa(?!\(Latino\)))\b)|(?<ukrainian>\b(?:\dx?)?(?:ukr))|(?<thai>\b(?:THAI)\b)|(?<romanian>\b(?:RoDubbed|ROMANIAN)\b)|(?<catalan>[-,. ]cat[. ](?:DD|subs)|\b(?:catalan|catalán)\b)|(?<latvian>\b(?:lat|lav|lv)\b)|(?<turkish>\b(?:tur)\b)|(?<urdu>\burdu\b)|(?<romansh>\b(?:romansh|rumantsch|romansch)\b)|(?<georgian>\b(?:geo|ka|kat|georgian)\b)|(?<irish>\b(?:irish|gaeilge)\b)|(?<japanese>\(JA\)|JAP|JPN)|(?<portuguese>[_. ]por[_. ])|(?<original>\b(?:orig|original)\b)
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L25 `LanguageRegex`

C# interpolated verbatim string `@$"..."`; template:

```regex
(?:{LanguagesOnlyRegex})(?!(?:[-_. ](?:{LanguagesOnlyRegex}))*[-_. ]subs?)
```

Expanded (what .NET compiles; `{LanguagesOnlyRegex}` → `LanguagesOnlyRegex.ToString()` = its pattern):

```regex
(?:(?<english>\b(?:ing|eng)\b)|(?<italian>\b(?:ita|italian)\b)|(?<german>(?:swiss)?german\b|videomann|ger[. ]dub|\bger\b)|(?<flemish>flemish)|(?<greek>greek)|(?<french>(?:_|\b)(?:FR|VF|VF2|VFF|VFI|VFQ|TRUEFRENCH|FRENCH|FRE|FRA)(?:_|\b))|(?<russian>\b(?:rus|ru)\b)|(?<hungarian>\b(?:HUNDUB|HUN)\b)|(?<hebrew>\bHebDub\b)|(?<polish>\b(?:PL\W?DUB|DUB\W?PL|LEK\W?PL|PL\W?LEK)\b)|(?<chinese>\[(?:CH[ST]|BIG5|GB)\]|简|繁|字幕|国语音轨[.+])|(?<bulgarian>\bbgaudio\b)|(?<spanish>\b(?:español|castellano|esp|spa(?!\(Latino\)))\b)|(?<ukrainian>\b(?:\dx?)?(?:ukr))|(?<thai>\b(?:THAI)\b)|(?<romanian>\b(?:RoDubbed|ROMANIAN)\b)|(?<catalan>[-,. ]cat[. ](?:DD|subs)|\b(?:catalan|catalán)\b)|(?<latvian>\b(?:lat|lav|lv)\b)|(?<turkish>\b(?:tur)\b)|(?<urdu>\burdu\b)|(?<romansh>\b(?:romansh|rumantsch|romansch)\b)|(?<georgian>\b(?:geo|ka|kat|georgian)\b)|(?<irish>\b(?:irish|gaeilge)\b)|(?<japanese>\(JA\)|JAP|JPN)|(?<portuguese>[_. ]por[_. ])|(?<original>\b(?:orig|original)\b))(?!(?:[-_. ](?:(?<english>\b(?:ing|eng)\b)|(?<italian>\b(?:ita|italian)\b)|(?<german>(?:swiss)?german\b|videomann|ger[. ]dub|\bger\b)|(?<flemish>flemish)|(?<greek>greek)|(?<french>(?:_|\b)(?:FR|VF|VF2|VFF|VFI|VFQ|TRUEFRENCH|FRENCH|FRE|FRA)(?:_|\b))|(?<russian>\b(?:rus|ru)\b)|(?<hungarian>\b(?:HUNDUB|HUN)\b)|(?<hebrew>\bHebDub\b)|(?<polish>\b(?:PL\W?DUB|DUB\W?PL|LEK\W?PL|PL\W?LEK)\b)|(?<chinese>\[(?:CH[ST]|BIG5|GB)\]|简|繁|字幕|国语音轨[.+])|(?<bulgarian>\bbgaudio\b)|(?<spanish>\b(?:español|castellano|esp|spa(?!\(Latino\)))\b)|(?<ukrainian>\b(?:\dx?)?(?:ukr))|(?<thai>\b(?:THAI)\b)|(?<romanian>\b(?:RoDubbed|ROMANIAN)\b)|(?<catalan>[-,. ]cat[. ](?:DD|subs)|\b(?:catalan|catalán)\b)|(?<latvian>\b(?:lat|lav|lv)\b)|(?<turkish>\b(?:tur)\b)|(?<urdu>\burdu\b)|(?<romansh>\b(?:romansh|rumantsch|romansch)\b)|(?<georgian>\b(?:geo|ka|kat|georgian)\b)|(?<irish>\b(?:irish|gaeilge)\b)|(?<japanese>\(JA\)|JAP|JPN)|(?<portuguese>[_. ]por[_. ])|(?<original>\b(?:orig|original)\b)))*[-_. ]subs?)
```
  args: `RegexOptions.IgnoreCase | RegexOptions.Compiled`

- L28 `CaseSensitiveLanguageRegex`

```regex
(?:(?i)(?<!SUB[\W|_|^]))(?:(?<lithuanian>\bLT\b)|(?<czech>\bCZ\b)|(?<polish>\bPL\b)|(?<bulgarian>\bBG\b)|(?<slovak>\bSK\b)|(?<german>\bDE\b))(?:(?i)(?![\W|_|^]SUB))
```
  args: `RegexOptions.Compiled`

- L31 `GermanDualLanguageRegex`

```regex
(?<!WEB[-_. ]?)\bDL\b
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L32 `GermanMultiLanguageRegex`

```regex
\bML\b
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L34 `SubtitleLanguageRegex` (regular C# string literal; backslash escapes are C# escapes)

```regex
.+?([-_. ](?<tags>forced|foreign|default|cc|psdh|sdh))*[-_. ](?<iso_code>[a-z]{2,3})([-_. ](?<tags>forced|foreign|default|cc|psdh|sdh))*$
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L36 `SubtitleLanguageTitleRegex`

```regex
.+?(\.((?<tags1>forced|foreign|default|cc|psdh|sdh)|(?<iso_code>[a-z]{2,3})))*[-_. ](?<title>[^.]*)(\.((?<tags2>forced|foreign|default|cc|psdh|sdh)|(?<iso_code>[a-z]{2,3})))*$
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L38 `SubtitleTitleRegex`

```regex
^((?<title>.+) - )?(?<copy>(?<!\d+)\d{1,3}(?!\d+))$
```
  args: `RegexOptions.Compiled`

### 4.2 `ParseLanguages(title)` algorithm (L40–227)

Input in `ParseTitle` is `ParsedEpisodeInfo.ReleaseTokens` (the tail after the episode numbers); `ParseSeasonTitle`
passes the whole release title.

1. `CleanSeriesTitleRegex`: `.*?[_. ](S\d{2}(?:E\d{2,4})*[_. ].*)` → `$1` (IgnoreCase) — drops anything before a
   `.S01E01.` marker (TryReplace + break; only one regex).
2. `lowerTitle = title.ToLower()`. Substring checks (plain `Contains`, no word boundaries), appended **in this order**:
   `spanish`→Spanish, `danish`→Danish, `dutch`→Dutch, `japanese`→Japanese, `icelandic`→Icelandic,
   `mandarin|cantonese|chinese`→Chinese, `korean`→Korean, `russian`→Russian, `polish`→Polish, `vietnamese`→Vietnamese,
   `swedish`→Swedish, `norwegian`→Norwegian, `finnish`→Finnish, `turkish`→Turkish, `portuguese`→Portuguese,
   `hungarian`→Hungarian, `hebrew`→Hebrew, `arabic`→Arabic, `hindi`→Hindi, `malayalam`→Malayalam,
   `ukrainian`→Ukrainian, `bulgarian`→Bulgarian, `slovak`→Slovak, `brazilian|dublado`→Portuguese (Brazil),
   `latino`→Spanish (Latino), `latvian`→Latvian, `azerbaijani|azerbaijan`→Azerbaijani, `uzbek|uzbekistan`→Uzbek.
3. `RegexLanguage(title)` results appended:
   * `CaseSensitiveLanguageRegex.Match` (**first match only**): lithuanian→Lithuanian, czech→Czech, polish→Polish,
     bulgarian→Bulgarian, slovak→Slovak, german→German (only one group can succeed per match).
   * `LanguageRegex.Matches` (all matches), per match in order of checks: english, italian, german, flemish, greek,
     french, russian, (dutch — no such group, never), hungarian, hebrew, polish, chinese, bulgarian, ukrainian, spanish,
     thai, romanian, catalan, latvian, turkish, urdu, romansh, georgian, irish, japanese, portuguese, original(→`Original`).
4. `lowerTitle.Contains("english")` → English (appended **after** regex languages).
5. Empty → `[Unknown]`.
6. If exactly `[German]`: `GermanDualLanguageRegex` (`(?<!WEB[-_. ]?)\bDL\b`) → add `Original`; else
   `GermanMultiLanguageRegex` (`\bML\b`) → add `Original`, `English`.
7. Return `DistinctBy(Id)` preserving first-occurrence order.

Language **order matters** downstream (e.g. `{MediaInfo AudioLanguages}`, LanguagesComparer, custom-format
"Language" spec with `ExceptLanguage`). Substring checks can create false positives (e.g. "latino" anywhere).

### 4.3 Language list (`Languages/Language.cs`) — `Id: Name`

`0 Unknown, 1 English, 2 French, 3 Spanish, 4 German, 5 Italian, 6 Danish, 7 Dutch, 8 Japanese, 9 Icelandic, 10 Chinese,
11 Russian, 12 Polish, 13 Vietnamese, 14 Swedish, 15 Norwegian, 16 Finnish, 17 Turkish, 18 Portuguese, 19 Flemish,
20 Greek, 21 Korean, 22 Hungarian, 23 Hebrew, 24 Lithuanian, 25 Czech, 26 Arabic, 27 Hindi, 28 Bulgarian, 29 Malayalam,
30 Ukrainian, 31 Slovak, 32 Thai, 33 "Portuguese (Brazil)", 34 "Spanish (Latino)", 35 Romanian, 36 Latvian, 37 Persian,
38 Catalan, 39 Croatian, 40 Serbian, 41 Bosnian, 42 Estonian, 43 Tamil, 44 Indonesian, 45 Macedonian, 46 Slovenian,
47 Azerbaijani, 48 Uzbek, 49 Malay, 50 Urdu, 51 Romansh, 52 Georgian, 53 Irish, -2 Original`.
`Language.All` is in exactly that order (Original last). Equality by Id. There is **no** `Any` (-1) language in Sonarr and
no language profile (only Delay/Qualities/Releases exist under `Profiles/`; the only `LanguageProfile` references are the
Sonarr-v3 import-list proxy) — language preference is expressed solely via the custom-format Language specification (§8).
`(Language)"name"` looks up by Name (invariant ignore-case) and throws if unknown; `FindById(0)` → Unknown, unknown id throws.

`LanguagesComparer` (Languages/LanguagesComparer.cs), used for sorting lists of languages: both empty → 0; empty sorts
**after** non-empty; when both have >1 items more items sorts after; multi (>1) sorts after single; two singles compare
by `Name` (culture string compare); else 0.

### 4.4 Multi-language and release-level aggregation (`Download/Aggregation/Aggregators/AggregateLanguages.cs`)

Runs after mapping (RemoteEpisode). Order:
1. Series null → `Release.Languages` if non-empty, else parsed languages. Done.
2. If the indexer/user supplied `Release.Languages` → use them. Else: for each mapped episode, parse its **title**'s
   languages; if not Unknown and the normalised episode title occurs in normalised ReleaseTokens → remove that span from
   the (raw) tokens (index from normalised string applied to the raw string — quirk) and mark those languages for removal;
   then keep in the removal list only languages **not** re-detected in the trimmed tokens; remove them from the list.
3. **MULTI**: if the release title matches `MultiRegex` (`[-_. \[](multi|multilang|multilanguage)[-_. \]]`, ci) and the
   indexer (by IndexerId, else by name) has `Settings.MultiLanguages` non-empty: if current list is empty/[Unknown] →
   replace by the indexer's MultiLanguages; else append those not already present.
4. Still empty or `[Unknown]` → `[series.OriginalLanguage]`.
5. If list contains `Original`: remove it; add `series.OriginalLanguage` if absent, **else add `Unknown`** (sic).

### 4.5 Subtitles (`ParseSubtitleLanguage`, `ParseSubtitleLanguageInformation`, `ParseLanguageTags`)

There is **no separate `SubtitleTitleParser` class** in this tree; subtitle parsing lives in `LanguageParser` and is called
from `MediaFiles/EpisodeImport/Aggregation/Aggregators/AggregateSubtitleInfo.cs` (and migration 198).
Used for extra files (subtitle import/rename), not release parsing:
* `ParseSubtitleLanguage(fileName)`: on `GetFileNameWithoutExtension`, `SubtitleLanguageRegex` → `iso_code` →
  `IsoLanguages.Find(lower)` → Language or Unknown; if no match, the first `Language.All` whose `Name` the file name
  ends with (ordinal ignore-case); else Unknown.
* `ParseSubtitleLanguageInformation`: `SubtitleLanguageTitleRegex`; if no `title` group or `iso_code` capture count ≠ 1
  → `ParseBasicSubtitle` (TitleFirst=false, tags from `SubtitleLanguageRegex` `tags` captures lower-cased, language as above).
  Else language from `iso_code`; LanguageTags = union of `tags1`+`tags2` captures (lowercased, non-empty);
  `TitleFirst = tags1 captures empty`; `RawTitle = title`; then `SubtitleTitleRegex` (`^((?<title>.+) - )?(?<copy>…\d{1,3}…)$`)
  splits "Title - 2" into Title + Copy (int), else Title=RawTitle, Copy=0.
* `IsoLanguages.Find(code)`: split on `-`; 2-letter → match `TwoLetterCode`; if a region part exists prefer entries with
  that `CountryCode`, else those with empty CountryCode; first. Else `AlternateIsoCodeMappings`
  (`cze, dut, mac, rum, yue, zhtw`) on the **whole** code. Else 3-letter → map ISO-639-2/B→T via
  `FileNameBuilder.Iso639BTMap` then match `ThreeLetterCode`. Table (2,country,3 → Language) in IsoLanguages.cs L13–66
  (note `nl`/`nld` appears for both Dutch and Flemish — Dutch wins as first; `no` and `nb` → Norwegian; `pt-br` →
  Portuguese (Brazil); `es-mx` → Spanish (Latino); `zh`'s country is `cn`; `fr`,`de`,`pt`,`th` have country = same code).

---

## 5. Scene checking and mapping parsed info to series/episodes

### 5.1 `SceneChecker.IsSceneTitle(title)` (`Parser/SceneChecker.cs`)

"Prefer false negatives". Returns true only if: title contains `.`; contains **no** space; `ParseTitle(title)` non-null;
`ReleaseGroup != null`; `Quality.Quality != Unknown`; `SeriesTitle` not blank. (Used by import/rename to decide whether to
keep the scene name as "original/scene name".)

### 5.2 Release pipeline entry — `DownloadDecisionMaker.GetDecisions` (`DecisionEngine/DownloadDecisionMaker.cs` L58–197)

Per `ReleaseInfo report`:
1. `parsed = Parser.ParseTitle(report.Title)`.
2. If `parsed == null || parsed.IsPossibleSpecialEpisode` → `special = ParsingService.ParseSpecialEpisodeTitle(parsed,
   report.Title, report.TvdbId, report.TvRageId, report.ImdbId, searchCriteria)`; if non-null it **replaces** `parsed`.
3. If still null → `Parser.ParseSeasonTitle(report.Title)` (v5 anime season-title fallback).
4. If `parsed != null && SeriesTitle` not blank:
   * `remote = ParsingService.Map(parsed, report.TvdbId, report.TvRageId, report.ImdbId, searchCriteria)`;
     `remote.Release = report`; `ReleaseSource` = search ? (Interactive → InteractiveSearch, UserInvoked → UserInvokedSearch,
     else Search) : (pushed ? ReleasePush : Rss).
   * `remote.Series == null` → if `SceneMappingService.FindTvdbId(SeriesTitle, ReleaseTitle, SeasonNumbers.FirstOrDefault(-1))`
     has a value → rejection **MatchesAnotherSeries** ("{title} matches an alias for series with TVDB ID: {id}"), else **UnknownSeries** ("Unknown Series").
   * else `remote.Episodes` empty → **UnknownEpisode** ("Unable to identify correct episode(s) using release name and scene mappings").
   * else: `AggregationService.Augment(remote)` (only `AggregateLanguages`, §4.4; exceptions logged & ignored);
     `remote.CustomFormats = CustomFormatCalculationService.ParseCustomFormat(remote, report.Size)`;
     `remote.CustomFormatScore = series.QualityProfile.CalculateCustomFormatScore(CustomFormats)`;
     `DownloadAllowed = Episodes.Any()`; run specifications (§9.1).
5. If `searchCriteria != null`: if `parsed == null` create `{Languages = ParseLanguages(title), Quality = ParseQuality(title)}`;
   if `parsed.SeriesTitle` blank → decision = rejection **UnableToParse** ("Unable to parse release").
   (In RSS, an unparseable / title-less release yields **no decision at all**.)
6. Any exception → rejection **Error** ("Unexpected error processing release").

### 5.3 `ParsingService.Map(parsed, tvdbId, tvRageId, imdbId, series = null, searchCriteria)` (`Parser/ParsingService.cs` L176–249)

1. `seasonNumber = parsed.SeasonNumbers.Any() ? First : null`.
2. `sceneMapping = SceneMappingService.FindSceneMapping(parsed.SeriesTitle, parsed.ReleaseTitle, seasonNumber ?? -1)`
   (may **throw** `InvalidSceneMappingException` when >1 distinct TVDB ids match → caught as Error by the decision maker).
3. `remote = { ParsedEpisodeInfo, SceneMapping, MappedSeasonNumber = seasonNumber }`; `sceneSource = true`.
4. If sceneMapping: `MappedSeasonNumber = MapSeasonNumber(sceneMapping, seasonNumber, parsed.IsSeasonTitle)`;
   `SceneOrigin == "tvdb"` → `sceneSource = false`.
5. If no series given: `FindSeries(...)` (§5.4) → `Series` + `SeriesMatchType`.
6. If series: if `series.SeasonType != Official` (non-official episode order) → MappedSeasonNumber reset to the parsed
   season (if any), `SceneMapping = null`, `sceneSource = false`. Then if `MappedSeasonNumber.HasValue` **and**
   `ValidateParsedEpisodeInfo.ValidateForSeriesType(parsed, series)` (rejects daily-style parse for a `Standard` series —
   logs a warning) → `Episodes = GetEpisodes(parsed, series, MappedSeasonNumber, sceneSource, searchCriteria)`.
   (Note: `ParseSeasonTitle` results have empty SeasonNumbers → MappedSeasonNumber is null unless a scene mapping supplies
   one via `IsSeasonTitle` handling.)
7. `remote.Languages = parsed.Languages`; `Episodes ??= []`; with search criteria, `EpisodeRequested` = any mapped episode id
   is in `searchCriteria.Episodes`.

`MapSeasonNumber(sceneMapping, seasonNumber, isSeasonTitle)` (L418):
* start `mapped = seasonNumber`;
* if `sceneMapping.SeasonNumber >= 0` (non-null) and `SceneSeasonNumber <= seasonNumber` →
  `mapped += SeasonNumber - SceneSeasonNumber` (C# lifted nullable arithmetic: null if any operand null);
* if `isSeasonTitle`: `mapped = SceneSeasonNumber.NonNegative() ?? SeasonNumber.NonNegative()` when that has a value;
* if `SceneOrigin != "tvdb"` and `Type == "XemService"` and `SceneSeasonNumber >= 0` and `seasonNumber == 1` and
  `SceneSeasonNumber != seasonNumber` → `mapped = SceneSeasonNumber`.

### 5.4 `FindSeries` (L492–648) — series resolution order and `SeriesMatchType {Unknown=0, Title=1, Alias=2, Id=3}`

1. **Scene mapping found**: if searchCriteria.Series.TvdbId == mapping.TvdbId → (searchCriteria.Series, Alias);
   else `FindByTvdbId(mapping.TvdbId)` → (series, Alias) or **null** (no further fallback!).
2. **Search criteria present** (in order): `Series.CleanOriginalTitle` non-blank and == `SeriesTitle.CleanSeriesTitle()` → Title;
   `Series.CleanTitle == SeriesTitle.CleanSeriesTitle()` → Title; `tvdbId > 0 && == Series.TvdbId` → Id;
   `tvRageId > 0 && == Series.TvRageId && tvdbId <= 0` → Id; `imdbId` non-blank and ordinal-equal `Series.ImdbId` and
   `tvdbId <= 0` → Id. (Search-criteria matches return immediately.)
3. `SeriesService.FindByTitle(SeriesTitle)` → Title.
4. still null and `SeriesTitleInfo.AllTitles != null` → `GetSeriesByAllTitles` → Title.
5. still null and `SeriesTitleInfo.Year > 0` → `FindByTitle(TitleWithoutYear, Year)` → Title; else
   `GetSeriesAliasTitleAndYear` (scene mapping for TitleWithoutYear whose series.Year == Year) → Alias.
6. still null and `tvdbId > 0` → `FindByTvdbId` → Id.
7. still null and `tvRageId > 0 && tvdbId <= 0` → `FindByTvRageId` → Id.
8. still null and imdbId non-blank and `tvdbId <= 0` → `FindByImdbId` → Id.
9. null → no match.

`GetSeriesByAllTitles`: for each title in `AllTitles` (year>0: `FindByTitle(title, year)` then `FindByTitle($"{title} {year}")`;
else `FindByTitle(title)`); if no series, scene-mapping tvdbId for that title; titles without a match are skipped; if two
titles resolve to **different** TVDB ids → null; result = first found series, or `FindByTvdbId(foundTvdbId)`.

`SeriesService.FindByTitle(title[, year])` → repository match on `CleanSeriesTitle(title).ToLowerInvariant()` against
`CleanTitle` OR `CleanOriginalTitle` (and `Year == year`); if several, prefer those whose `CleanTitle` matches; >1 left →
**throws** `MultipleSeriesFoundException`. `FindByTitleInexact(releaseTitle)`: series whose CleanTitle/CleanOriginalTitle is
a **substring** of the cleaned release title; if several, the one whose match position is leftmost, ties → longest.

`GetSeries(title)` (used for specials): `ParseTitle` null → `FindByTitle(title)`; else scene-mapping tvdbId → series; else
FindByTitle(SeriesTitle) → AllTitles → FindByTitle(TitleWithoutYear, Year).

### 5.5 Scene mappings (`DataAugmentation/Scene/SceneMappingService.cs`, `SceneMapping.cs`)

`SceneMapping { MappingId, Title, ParseTerm (= Title.CleanSeriesTitle()), SearchTerm, TvdbId, SeasonNumber? (json "season"),
SceneSeasonNumber?, SceneOrigin ("tvdb"/"scene"/…), SearchMode? {Default=0, SearchID=1, SearchTitle=2} [Flags], Comment,
FilterRegex, Type (provider class name, e.g. "XemService") }`.
* `FindMappings(seriesTitle, releaseTitle)`: candidates = mappings whose `ParseTerm == seriesTitle.CleanSeriesTitle()`;
  `FilterSceneMappings(candidates, releaseTitle)`: candidates with a `FilterRegex` are kept only if
  `Regex.IsMatch(Parser.SimplifyTitle(releaseTitle), FilterRegex)` (**default options = case-sensitive**); if any filtered
  candidate matches → return only those; else return the candidates without FilterRegex (also when releaseTitle blank).
  If >1 remain: prefer those whose `Title == seriesTitle` exactly (ordered by SeasonNumber desc); else choose the
  `Title` with minimum `seriesTitle.LevenshteinDistance(v.Title, costInsert=10, costDelete=1, costSubstitute=10)`
  (then SeasonNumber desc) and return all candidates with that Title.
* `FilterSceneMappings(candidates, sceneSeasonNumber)`: "season-specific" = both `SceneSeasonNumber` and `SeasonNumber`
  not null/-1. If `sceneSeasonNumber == -1` → only the non-season-specific ones. Else if any season-specific exist →
  those with `SceneSeasonNumber <= sceneSeasonNumber`, grouped by Title, each group's max SceneSeasonNumber (then max
  SeasonNumber) → return those (possibly empty!); else the non-season-specific ones.
* `FindSceneMapping(seriesTitle, releaseTitle, sceneSeasonNumber)`: blank title → null; FindMappings → FilterSceneMappings
  (season) → distinct by TvdbId: 0 → null, 1 → it, >1 → **throw InvalidSceneMappingException**.
* `FindTvdbId(...)` = `FindSceneMapping(...)?.TvdbId`. `GetSceneSeasonNumber(seriesTitle, releaseTitle)` =
  `FindSceneMapping(seriesTitle, releaseTitle, -1)?.SceneSeasonNumber`.
* `FindSceneMappings(seriesTitle, releaseTitle, tvdbId, seasons)` (multi-season packs): per season,
  first of `FilterSceneMappings(mappings for tvdbId, season)`.
* `GetSceneNames(tvdbId, seasonNumbers, sceneSeasonNumbers)` (search terms): mappings whose SeasonNumber ∈ seasons, or
  SceneSeasonNumber ∈ sceneSeasons, or (both -1/null and origin != "tvdb") → distinct `SearchTerm` (case-insensitive).

`LevenshteinDistance(text, other, costInsert=1, costDelete=1, costSubstitute=1)` (`NzbDrone.Common/Extensions/LevenstheinExtensions.cs`):
standard single-row DP; equal strings → 0; empty → length × cost.

### 5.6 `GetEpisodes(parsed, series, mappedSeason, sceneSource, searchCriteria)` (L268–314) — episode resolution

Order of checks:
1. **FullSeason**: multi-season → `GetMultiSeasonEpisodes` (per season: scene mapping via `FindSceneMappings` when
   sceneSource; `mapped = MapSeasonNumber(mapping, season, false)`; season scene source = `SceneOrigin != "tvdb"`; if
   `series.UseSceneNumbering && seasonSceneSource` take episodes with `SceneSeasonNumber == mapped`, fallback episodes
   with `SeasonNumber == mapped`; distinct by Id). Single season → `GetSeasonEpisodes`: `UseSceneNumbering && sceneSource`
   → episodes by `SceneSeasonNumber` (if any), else by `SeasonNumber == mappedSeason`.
2. **Daily** (`IsDaily`): searchCriteria episode with `AirDate == parsed.AirDate` (SingleOrDefault), else
   `EpisodeService.FindEpisode(seriesId, airDate, DailyPart)`: 0 eps → null; 1 → it; several → drop specials (season 0):
   if exactly 1 regular and no part → it; else if part ≤ regular count → regular episodes sorted (season, episode)[part-1];
   else **throws InvalidOperationException**.
3. **Absolute** (`IsAbsoluteNumbering`) → `GetAnimeEpisodes` (below).
4. **IsPossibleSceneSeasonSpecial** (SxxE00, season ≠ 0) → `ParseSpecialEpisodeTitle(parsed, ReleaseTitle, series)`; if it
   yields info → `GetStandardEpisodes(series, info, info.SeasonNumbers.First(), sceneSource=false)`.
5. `parsed.Special && mappedSeason != 0` → **no episodes**.
6. `GetStandardEpisodes`: for each episode number: if `UseSceneNumbering && sceneSource`: from searchCriteria episodes with
   `SceneSeasonNumber == parsed.SeasonNumbers.FirstOrDefault(-1)` and `SceneEpisodeNumber == n`, else DB
   `FindEpisodesBySceneNumbering(seriesId, mappedSeason, n)`; if found → add all, continue. Otherwise searchCriteria
   episode with `(SeasonNumber, EpisodeNumber) == (mappedSeason, n)` (SingleOrDefault), else DB `FindEpisode(seriesId, mappedSeason, n)`.

`GetAnimeEpisodes` (L668) — absolute numbering via `SceneAbsoluteEpisodeNumber`:
`sceneSeasonNumber = SceneMappingService.GetSceneSeasonNumber(SeriesTitle, ReleaseTitle)`. For each absolute number `a`:
* `parsed.Special` → `FindEpisode(seriesId, season 0, episode a)`.
* else if sceneSource:
  * sceneSeasonNumber has value → `FindEpisodesBySceneNumbering(seriesId, sceneSeason, a)` (scene season+episode), else
    `FindEpisode(seriesId, sceneSeason, a)`;
  * else if `SeasonNumbers.FirstOrDefault() > 1 && EpisodeNumbers.Empty()` → same two lookups with `SeasonNumbers.First()`;
  * else `FindEpisodesBySceneNumbering(seriesId, a)` (by **SceneAbsoluteEpisodeNumber**); if >1 results → cleared.
* if nothing → `FindEpisode(seriesId, a)` (by `AbsoluteEpisodeNumber`, SingleOrDefault).
(Note `mappedSeasonNumber` is not used in the anime path except through scene mapping.)

### 5.7 Special episodes — `ParseSpecialEpisodeTitle` (L316–365, L451–490)

With search criteria: if report tvdbId (≠0) / tvRageId (≠0) / imdbId equals the searched series' ids → use that series.
Otherwise `GetSeries(releaseTitle)` → `FindByTitleInexact(releaseTitle)` → `FindByTvdbId` → `FindByTvRageId` →
`FindByImdbId`; none → null. With the series:
* if `parsed.IsPossibleSceneSeasonSpecial && series.UseSceneNumbering` and any episode has scene numbering
  `(season, 0)` → return the original parsed info (XEM-mapped SxxE00).
* `EpisodeService.FindEpisodeByTitle(seriesId, season 0, releaseTitle)`: normalised release title
  (`NormalizeEpisodeTitle`) and its `CleanSeriesTitle` form are searched (current-culture ignore-case `IndexOf`) for each
  season-0 episode's normalised title (and cleaned form); matches with non-empty episode title ordered by position asc,
  then length desc → first.
* found → new `ParsedEpisodeInfo { ReleaseTitle, SeriesTitle = series.Title, SeriesTitleInfo{Title}, SeasonNumbers=[ep.Season],
  EpisodeNumbers=[ep.Episode], FullSeason=false, Quality=ParseQuality(releaseTitle), ReleaseGroup=ParseReleaseGroup(releaseTitle),
  Languages=ParseLanguages(releaseTitle) (whole title), Special=true }`.

TVDB/TVRage/IMDb ids come from the indexer result (`ReleaseInfo.TvdbId/TvRageId/ImdbId`) and are only used as the
fallbacks described above (and with search criteria). `Parser.NormalizeImdbId` exists for indexer parsing.

---

## 6. Test corpus inventory (`src/NzbDrone.Core.Test/`)

### 6.1 Totals

| Area | Files | active `[TestCase]` rows | `[Test]` methods | Notes |
|---|---|---|---|---|
| `ParserTests/` (incl. `ParsingServiceTests/`) | 33 | **1,654** | 93 | + `TestCaseSource` rows: HashedReleaseFixture 20, QualityParserFixture 16 + 14 (each "other source" row runs ×4 separators). 23 `[TestCase]` lines are commented out (`// [TestCase…`) — do not port as expectations. |
| `Qualities/` | 5 | 8 | 25 | QualityFinder, comparer, revision ordering, definitions. |
| `Profiles/` | 7 | 10 | 26 | QualityIndex.CompareTo, rank service, delay/release profiles. |
| `CustomFormats/` | 5 | 0 | 21 | Language (single/multi/original) and ReleaseType specs. |
| `DecisionEngineTests/` | 37 | 63 | 313 | Mostly fixture-built objects (AutoMoq/FizzWare `Builder<T>`), not string tables. |
| `OrganizerTests/` | 33 | 244 | 138 | FileNameBuilder token tests (need NamingConfig + Series/Episode/EpisodeFile builders). |
| `Download/Aggregation/AggregateLanguagesFixture.cs` | 1 | 32 | 8 | Multi-language indexer setting, Original handling. |
| `MediaFiles/EpisodeImport/Aggregation/**` | 9 | 31 | 48 | Import-side quality/language/release-group aggregation (MediaInfo) — not audited in depth. |

**Parser test-case total ≈ 1,654 `[TestCase]` + 50 `TestCaseSource` rows + 93 plain `[Test]` ≈ 1,800 executions.**

### 6.2 How to interpret the rows (helpers you must replicate)

* Assertions are FluentAssertions: `.Be()` exact, `.Equal(x)` on `SeasonNumbers` = exact sequence (e.g. `Equal(2)` → `[2]`),
  `.BeEquivalentTo(array)` = **order-insensitive** set equality, `.Contain(x)` = membership only (most language tests only
  check membership, not exclusivity).
* `SeriesTitle` is compared **exactly** (`"Series Title 2010"` keeps the year, `"Series (1994)"` keeps parentheses), except
  `ParserFixture.should_parse_series_name` and `UrlFixture.should_not_parse_url_in_name`, which compare
  `ParseSeriesName(postTitle).CleanSeriesTitle()` to `title.CleanSeriesTitle()`.
* `.AsOsAgnostic()` (`NzbDrone.Test.Common/StringExtensions.cs`): on non-Windows, `C:\x\y` → `/C\x\y` with `:` removed then
  every `\` → `/` (so `C:\Test\a.mkv` → `/C/Test/a.mkv`). Path tests (`PathParserFixture`, `HashedReleaseFixture`,
  `ReleaseGroupParserFixture.should_not_include_extension_in_release_group`, absolute batch test) depend on it.
* `XmlCleaner.ReplaceUnicode(s)` (`NzbDrone.Core/Indexers/XmlCleaner.cs`): removes chars matching
  `[^\x09\x0A\x0D\u0020-\uD7FF\uE000-\uFFFD]` — applied to inputs in `UnicodeReleaseParserFixture` before parsing.
* `"...{year}...".Expand(new { year, month, day })` (`NzbDrone.Common/Expansive`): replaces `{name}` tokens
  (`\{[a-zA-Z]\w*\}`) with the object's property values (Daily "ancient"/"future limit" tests).
* Daily tests use `DateTime.Now`/`Today` (future limit = today + 14 days, local time). Some fixed-date rows are in 2026
  (e.g. `2026-07-03`) — they pass only when "now" ≥ date − 14d. **Inject a clock** in the Rust port.
* `CoreTest` base + `ExceptionVerification`: a test fails if unexpected Warn/Error logs are emitted (some tests call
  `ExceptionVerification.IgnoreWarns()`); not needed for a Rust port but explains some expectations (e.g. "crap" titles
  must not throw).
* `TestCase(..., Description = "...")` named arguments appear (SceneCheckerFixture) — ignore `Description`.
* `CrapParserFixture.should_not_parse_md5` hashes `"CRAPPY TEST SEED"` 100× with MD5 (uppercase hex, no dashes) — all must
  fail to parse; `should_not_parse_random(length)` uses .NET `Random` (non-deterministic) — port as property tests.
* Quality tests: `ParseAndVerifyQuality(title, quality, proper)` asserts `Quality` and `Revision.Version == (proper ? 2 : 1)`.
  `parsing_our_own_quality_enum_name`: `"My series S01E01 [{quality.Name}]"` must round-trip for 16 qualities.
  `should_parse_quality_from_other_source`: `"My series S01E01 " + qualityString.Replace(' ', c)` for c in `- . space _`.

### 6.3 Per-file inventory — `ParserTests/` (method → row count, 3 verbatim sample rows from the largest method)

#### `ParserTests/AbsoluteEpisodeNumberParserFixture.cs` — 167 active `[TestCase]`

* `should_parse_absolute_numbers(string postTitle, string title, int absoluteEpisodeNumber, int seasonNumber, int episodeNumber)` — 123 cases
* `should_parse_absolute_specials(string postTitle, string title, int absoluteEpisodeNumber)` — 3 cases
* `should_parse_absolute_specials_without_absolute_number(string postTitle, string title, int absoluteEpisodeNumber)` — 3 cases
* `should_parse_multi_episode_absolute_numbers(string postTitle, string title, int firstAbsoluteEpisodeNumber, int lastAbsoluteEpisodeNumber)` — 23 cases
* `should_parse_anime_season_packs(string postTitle, string title, int seasonNumber)` — 7 cases
* `should_handle_anime_recap_numbering(string postTitle, string title, double specialEpisodeNumber)` — 3 cases
* `should_handle_ambiguously_named_anime_files_in_batch_release(string releaseName, string filename, string title, int absoluteEpisodeNumber)` — 1 cases
* `should_parse_absolute_followed_by_standard_as_standard(string releaseName, string title, decimal specialEpisodeNumber, int seasonNumber, int episodeNumber)` — 4 cases

```csharp
[TestCase("[SubDESU]_Show_One_07_(1280x720_x264-AAC)_[6B7FD717]", "Show One", 7, 0, 0)]  // -> should_parse_absolute_numbers
[TestCase("[Hatsuyuki] Series Title (2014) - 017 (115) [1280x720][B2CFBC0F]", "Series Title (2014)", 17, 0, 0)]  // -> should_parse_absolute_numbers
[TestCase("[Kaleido-subs] Animation - 12 (S01E12) - (WEB 1080p HEVC x265 10-bit E-AC3 2.0) [1ADD8F6D]", "Animation", 12, 1, 12)]  // -> should_parse_absolute_numbers
```

#### `ParserTests/AnimeMetadataParserFixture.cs` — 36 active `[TestCase]`

* `should_parse_releasegroup_and_hash(string postTitle, string subGroup, string hash)` — 30 cases
* `should_parse_release_hashes_with_10b_or_8b(string postTitle, string hash)` — 6 cases

```csharp
[TestCase("[SubDESU]_Show_Title_DxD_07_(1280x720_x264-AAC)_[6B7FD717]", "SubDESU", "6B7FD717")]  // -> should_parse_releasegroup_and_hash
[TestCase("[SubDESU]_Show_Title_DxD_07_(1280x720_x264-AAC)_(6B7FD717)", "SubDESU", "6B7FD717")]  // -> should_parse_releasegroup_and_hash
[TestCase("[S-T-D] Series Title! - 06 (1280x720 10bit AAC) (59B3F2EA).mkv", "S-T-D", "59B3F2EA")]  // -> should_parse_releasegroup_and_hash
```

#### `ParserTests/AnimeVersionFixture.cs` — 6 active `[TestCase]`

* `should_be_able_to_parse_repack(string title, int version)` — 6 cases

```csharp
[TestCase("Anime Title - S01E01 - (BD 1080p HEVC FLAC) [Dual Audio] [Group]", 1)]  // -> should_be_able_to_parse_repack
[TestCase("[SubsPlease] Anime Title - 01 (1080p) [B1F227CF]", 1)]  // -> should_be_able_to_parse_repack
[TestCase("[SubsPlease] Anime Title - 01 v2 (1080p) [B1F227CF]", 2)]  // -> should_be_able_to_parse_repack
```

#### `ParserTests/CrapParserFixture.cs` — 30 active `[TestCase]`

* `should_not_parse_crap(string title)` — 23 cases
* `should_parse_unknown_formats_without_error(string title)` — 2 cases
* `should_not_parse_random(int length)` — 2 cases
* `should_not_parse_file_name_without_proper_spacing(string fileName)` — 1 cases
* `should_not_parse_invalid_release_name(string fileName)` — 1 cases
* `should_not_parse_special_with_part_number(string fileName)` — 1 cases

```csharp
[TestCase("76El6LcgLzqb426WoVFg1vVVVGx4uCYopQkfjmLe")]  // -> should_not_parse_crap
[TestCase("ce39afb7da6cf7c04eba3090f0a309f609883862")]  // -> should_not_parse_crap
[TestCase("_unpack e096aeb3c2c0483a96f5b32fc6d10ff5.mkv")]  // -> should_not_parse_crap
```

#### `ParserTests/DailyEpisodeParserFixture.cs` — 57 active `[TestCase]`

* `should_parse_daily_episode(string postTitle, string title, int year, int month, int day)` — 30 cases
* `should_parse_daily_episode_with_multiple_parts(string postTitle, string title, int year, int month, int day, int part)` — 4 cases
* `should_not_accept_ancient_daily_series(string title)` — 7 cases
* `should_accept_dates_within_future_limit(string title)` — 7 cases
* `should_parse_episodes_released_before_their_air_date(int days)` — 2 cases
* `should_not_parse_dates_beyond_future_limit(int days)` — 2 cases
* `should_not_parse_ambiguous_daily_episode(string postTitle)` — 2 cases
* `should_parse_daily_episode_using_short_month_format(string postTitle, string title, int year, int month, int day)` — 3 cases

```csharp
[TestCase("Series Title 2011 04 18 Emma Roberts HDTV XviD BFF", "Series Title", 2011, 04, 18)]  // -> should_parse_daily_episode
[TestCase("Tree_Series_2018_06_22_Seth_Meyers_720p_HEVC_x265-MeGusta", "Tree Series", 2018, 6, 22)]  // -> should_parse_daily_episode
[TestCase("Series Title - 2026-07-03 - Look Back At 2025 Part.1", "Series Title", 2026, 7, 3)]  // -> should_parse_daily_episode
```

#### `ParserTests/ExtendedQualityParserRegex.cs` — 30 active `[TestCase]`

* `should_parse_reality_from_title(string title, int reality)` — 14 cases
* `should_parse_version_from_title(string title, int version)` — 16 cases

```csharp
[TestCase("Series.Title.S04E05.HDTV.XviD-LOL", 1)]  // -> should_parse_version_from_title
[TestCase("Series.Title.S07E11.PROPER.REAL.RERIP.1080p.BluRay.x264-TENEIGHTY", 2)]  // -> should_parse_version_from_title
[TestCase("[Vivid-Asenshi] Series Title - 02v2 [1F67AB55]", 2)]  // -> should_parse_version_from_title
```

#### `ParserTests/HashedReleaseFixture.cs` — 0 active `[TestCase]`


#### `ParserTests/IsoLanguagesFixture.cs` — 26 active `[TestCase]`

* `should_return_iso_language_for_English(string isoCode)` — 4 cases
* `unknown_or_invalid_code_should_return_null(string isoCode)` — 4 cases
* `should_return_portuguese(string isoCode)` — 3 cases
* `should_not_return_portuguese(string isoCode)` — 1 cases
* `should_lookup_cze_via_alternate_iso_code_mapping(string isoCode)` — 1 cases
* `should_return_urdu(string isoCode)` — 3 cases
* `should_return_romansh(string isoCode)` — 3 cases
* `should_return_georgian(string isoCode)` — 4 cases
* `should_return_irish(string isoCode)` — 3 cases

```csharp
[TestCase("en")]  // -> should_return_iso_language_for_English
[TestCase("en-US")]  // -> should_return_iso_language_for_English
[TestCase("en-GB")]  // -> should_return_iso_language_for_English
```

#### `ParserTests/IsPossibleSpecialEpisodeFixture.cs` — 5 active `[TestCase]`

* `IsPossibleSpecialEpisode_should_be_true(string title)` — 3 cases
* `IsPossibleSpecialEpisode_should_be_true_if_e00_special(string title)` — 1 cases
* `IsPossibleSpecialEpisode_should_be_false_for_Special_in_title(string title)` — 1 cases

```csharp
[TestCase("Title.the.Series.S02.Special-Inside.Chesters.Mill.HDTV.x264-BAJSKORV")]  // -> IsPossibleSpecialEpisode_should_be_true
[TestCase("Title.the.Series.S02.Special-Inside.Chesters.Mill.720p.HDTV.x264-BAJSKORV")]  // -> IsPossibleSpecialEpisode_should_be_true
[TestCase("Title.the.Series.S05.Special.HDTV.x264-2HD")]  // -> IsPossibleSpecialEpisode_should_be_true
```

#### `ParserTests/LanguageParserFixture.cs` — 221 active `[TestCase]`

* `should_parse_subtitle_language_unknown(string fileName)` — 1 cases
* `should_parse_subtitle_language_english(string fileName)` — 11 cases
* `should_parse_language_unknown(string postTitle)` — 15 cases
* `should_parse_language_english(string postTitle)` — 4 cases
* `should_parse_language_french(string postTitle)` — 18 cases
* `should_parse_language_french_english(string postTitle)` — 5 cases
* `should_parse_language_spanish(string postTitle)` — 3 cases
* `should_parse_language_german(string postTitle)` — 11 cases
* `should_parse_language_italian(string postTitle)` — 4 cases
* `should_parse_language_danish(string postTitle)` — 1 cases
* `should_parse_language_dutch(string postTitle)` — 1 cases
* `should_parse_language_japanese(string postTitle)` — 7 cases
* `should_parse_language_icelandic(string postTitle)` — 2 cases
* `should_parse_language_chinese(string postTitle)` — 14 cases
* `should_parse_language_korean(string postTitle)` — 1 cases
* `should_parse_language_latvian(string postTitle)` — 4 cases
* `should_parse_language_russian(string postTitle)` — 3 cases
* `should_parse_language_polish(string postTitle)` — 10 cases
* `should_parse_language_vietnamese(string postTitle)` — 1 cases
* `should_parse_language_swedish(string postTitle)` — 1 cases
* `should_parse_language_norwegian(string postTitle)` — 1 cases
* `should_parse_language_finnish(string postTitle)` — 1 cases
* `should_parse_language_turkish(string postTitle)` — 1 cases
* `should_parse_language_portuguese(string postTitle)` — 1 cases
* `should_parse_language_flemish(string postTitle)` — 1 cases
* `should_parse_language_greek(string postTitle)` — 1 cases
* `should_parse_language_hungarian(string postTitle)` — 3 cases
* `should_parse_language_hebrew(string postTitle)` — 1 cases
* `should_parse_language_lithuanian(string postTitle)` — 1 cases
* `should_parse_language_czech(string postTitle)` — 1 cases
* `should_parse_language_arabic(string postTitle)` — 1 cases
* `should_parse_language_hindi(string postTitle)` — 2 cases
* `should_parse_language_bulgarian(string postTitle)` — 3 cases
* `should_parse_language_malayalam(string postTitle)` — 3 cases
* `should_parse_language_ukrainian(string postTitle)` — 3 cases
* `should_parse_language_slovak(string postTitle)` — 2 cases
* `should_parse_language_thai(string postTitle)` — 3 cases
* `should_parse_language_portuguese_brazil(string postTitle)` — 2 cases
* `should_parse_language_spanish_latino(string postTitle)` — 6 cases
* `should_parse_language_romanian(string postTitle)` — 2 cases
* `should_parse_language_catalan(string postTitle)` — 6 cases
* `should_parse_hindi_and_english(string postTitle)` — 2 cases
* `should_parse_spanish_and_catalan(string postTitle)` — 3 cases
* `should_parse_english_spanish_and_catalan(string postTitle)` — 2 cases
* `should_add_original_language_to_german_release_with_dl_tag(string postTitle)` — 3 cases
* `should_not_add_original_language_to_german_release_when_title_contains_web_dl(string postTitle)` — 4 cases
* `should_add_original_language_and_english_to_german_release_with_ml_tag(string postTitle)` — 1 cases
* `should_parse_original_title_from_release_name(string postTitle)` — 3 cases
* `should_parse_english_and_ukranian(string postTitle)` — 3 cases
* `should_parse_turkish_and_ukranian(string postTitle)` — 1 cases
* `should_parse_azerbaijani(string postTitle)` — 2 cases
* `should_parse_uzbek(string postTitle)` — 2 cases
* `should_parse_language_urdu(string postTitle)` — 1 cases
* `should_parse_language_romansh(string postTitle)` — 3 cases
* `should_parse_language_georgian(string postTitle)` — 3 cases
* `should_parse_language_russian_and_georgian(string postTitle)` — 1 cases
* `should_parse_language_irish(string postTitle)` — 3 cases
* `should_parse_language_english_french_german_italian_portuguese_spanish(string postTitle)` — 1 cases
* `should_parse_title_and_tags(string postTitle, string[] expectedTags, string expectedTitle, string expectedLanguage)` — 17 cases
* `should_not_parse_false_title(string postTitle)` — 4 cases

```csharp
[TestCase("Title.the.Series.2009.S01E14.French.HDTV.XviD-LOL")]  // -> should_parse_language_french
[TestCase("Series Title S01 Bluray 4k HDR HEVC AC3 VFF")]  // -> should_parse_language_french
[TestCase("Series Title S01 (BDrip 1080p ENG-ITA-FRE) Multisub x264")]  // -> should_parse_language_french
```

#### `ParserTests/MiniSeriesEpisodeParserFixture.cs` — 11 active `[TestCase]`

* `should_parse_mini_series_episode(string postTitle, string title, int episodeNumber)` — 4 cases
* `should_parse_korean_series_episode(string postTitle, string title, int episodeNumber, string airdate)` — 7 cases

```csharp
[TestCase("It's a Series Title.E56.190121.720p-NEXT.mp4", "It's a Series Title", 56, "2019-01-21")]  // -> should_parse_korean_series_episode
[TestCase("The Series Title Challenge.E932.190120.720p-NEXT.mp4", "The Series Title Challenge", 932, "2019-01-20")]  // -> should_parse_korean_series_episode
[TestCase("나는 SOLO 그 후, 사랑은 계속된다(나솔사계).E135.251016.1080p-F1RST", "나는 SOLO 그 후, 사랑은 계속된다(나솔사계)", 135, "2025-10-16")]  // -> should_parse_korean_series_episode
```

#### `ParserTests/MultiEpisodeParserFixture.cs` — 77 active `[TestCase]`

* `should_parse_multiple_episodes(string postTitle, string title, int season, int[] episodes)` — 77 cases

```csharp
[TestCase("Series.S03E01-06.DUAL.BDRip.XviD.AC3.-HELLYWOOD", "Series", 3, new[] { 1, 2, 3, 4, 5, 6 })]  // -> should_parse_multiple_episodes
[TestCase("Series Title.S6.E1E3.Episode Name.1080p.WEB-DL", "Series Title", 6, new[] { 1, 2, 3 })]  // -> should_parse_multiple_episodes
[TestCase("[AL] Series.Title.S04E01-16.German.DL.DTS.1080p.BluRay.x264-Pudding.GerSub-sama", "Series Title", 4, new[] { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16 })]  // -> should_parse_multiple_episodes
```

#### `ParserTests/NormalizeEpisodeTitleFixture.cs` — 6 active `[TestCase]`

* `should_normalize_episode_title(string input, string expected)` — 6 cases

```csharp
[TestCase("Episode Title", "episode title")]  // -> should_normalize_episode_title
[TestCase("French Title (1)", "french title")]  // -> should_normalize_episode_title
[TestCase("Series.Title.S01E00.Episode.Title.720p.HDTV.x264-Sonarr", "episode title")]  // -> should_normalize_episode_title
```

#### `ParserTests/NormalizeSeriesTitleFixture.cs` — 41 active `[TestCase]`

* `should_normalize_series_title(string parsedSeriesName, string seriesName)` — 4 cases
* `should_remove_special_characters_and_casing(string dirty, string clean)` — 9 cases
* `should_remove_common_words_from_middle_of_title(string word)` — 5 cases
* `should_not_remove_common_words_from_end_of_title(string word)` — 5 cases
* `should_not_remove_common_words_in_the_middle_of_word(string word)` — 6 cases
* `should_not_remove_from_the_beginning_of_the_title(string parsedSeriesName, string seriesName)` — 3 cases
* `should_not_clean_word_from_beginning_of_string(string word)` — 6 cases
* `should_replace_percent_sign_with_percent_following_numbers(string input, string expected)` — 3 cases

```csharp
[TestCase("CaPitAl", "capital")]  // -> should_remove_special_characters_and_casing
[TestCase("90210", "90210")]  // -> should_remove_special_characters_and_casing
[TestCase("Ríkið", "rikid")]  // -> should_remove_special_characters_and_casing
```

#### `ParserTests/ParsedEpisodeInfoFixture.cs` — 8 active `[TestCase]`

* `should_format_full_season(int[] seasonNumbers, string expected)` — 4 cases
* `should_get_release_type(string title, ReleaseType expected)` — 4 cases

```csharp
[TestCase(new[] { 1, 2, 3 }, "Seasons 01-03")]  // -> should_format_full_season
[TestCase(new[] { 2 }, "Season 02")]  // -> should_format_full_season
[TestCase(new int[0], "[Unknown Season]")]  // -> should_format_full_season
```

#### `ParserTests/ParserFixture.cs` — 47 active `[TestCase]`

* `should_parse_series_name(string postTitle, string title)` — 17 cases
* `should_parse_series_title_info(string postTitle, string titleWithoutYear, int year = 0)` — 18 cases
* `should_clean_up_invalid_path_characters(string postTitle)` — 1 cases
* `should_remove_request_info_from_title(string postTitle, string title)` — 1 cases
* `should_parse_quality_from_extension(string title)` — 2 cases
* `should_parse_releasetitle(string path, string releaseTitle)` — 1 cases
* `should_parse_multiple_series_titles(string postTitle, params string[] titles)` — 6 cases
* `should_parse_language_after_parsing_title(string postTitle, string expectedLanguage)` — 1 cases

```csharp
[TestCase("Series S03E14 720p HDTV X264-DIMENSION", "Series")]  // -> should_parse_series_title_info
[TestCase("1234 S03E14 720p HDTV X264-DIMENSION", "1234")]  // -> should_parse_series_title_info
[TestCase("1234_2022_S03E14_720p_HDTV_X264-DIMENSION", "1234", 2022)]  // -> should_parse_series_title_info
```

#### `ParserTests/ParsingServiceTests/GetEpisodesFixture.cs` — 21 active `[TestCase]`

* `should_match_early_daily_release_to_known_air_date(int days)` — 3 cases
* `should_use_scene_numbering_when_scene_season_number_has_value(int seasonNumber)` — 3 cases
* `should_find_episode_by_season_and_scene_absolute_episode_number(int seasonNumber)` — 3 cases
* `should_find_episode_by_parsed_season_and_absolute_episode_number_when_season_number_is_2_or_higher(int seasonNumber)` — 2 cases
* `should_find_episode_by_parsed_season_and_absolute_episode_number_when_season_number_is_2_or_higher_and_scene_season_number_lookup_failed(int seasonNumber)` — 2 cases
* `should_not_find_episode_by_parsed_season_and_absolute_episode_number_when_season_number_is_2_or_higher_and_a_episode_number_was_parsed(int seasonNumber)` — 2 cases
* `should_return_episodes_when_scene_absolute_episode_number_returns_multiple_results(int seasonNumber)` — 3 cases
* `should_find_episode_by_season_and_absolute_episode_number_when_scene_absolute_episode_number_returns_no_results(int seasonNumber)` — 3 cases

```csharp
[TestCase(2)]  // -> should_match_early_daily_release_to_known_air_date
[TestCase(7)]  // -> should_match_early_daily_release_to_known_air_date
[TestCase(14)]  // -> should_match_early_daily_release_to_known_air_date
```

#### `ParserTests/ParsingServiceTests/GetSeriesFixture.cs` — 0 active `[TestCase]`


#### `ParserTests/ParsingServiceTests/MapFixture.cs` — 0 active `[TestCase]`


#### `ParserTests/PathParserFixture.cs` — 33 active `[TestCase]`

* `should_parse_from_path(string path, int season, int episode)` — 31 cases
* `should_parse_multi_episode_from_path(string path, string title, int season, int[] episodes)` — 2 cases

```csharp
[TestCase(@"z:\tv shows\series title (2003)\Season 3\S03E05 - Title.mkv", 3, 5)]  // -> should_parse_from_path
[TestCase(@"C:\Test\Unsorted\Series.Title.S02E19.720p.BluRay.x264-SiNNERS-RP\ba27283b17c00d01193eacc02a8ba98eeb523a76.mkv", 2, 19)]  // -> should_parse_from_path
[TestCase(@"C:\Test\Series Title S01 02\Series Title S02 01 1080p WEB-DL.mkv", 2, 1)]  // -> should_parse_from_path
```

#### `ParserTests/QualityParserFixture.cs` — 290 active `[TestCase]`

* `should_parse_sdtv_quality(string title, bool proper)` — 23 cases
* `should_parse_dvd_quality(string title, bool proper)` — 7 cases
* `should_parse_webdl480p_quality(string title, bool proper)` — 8 cases
* `should_parse_bluray480p_quality(string title, bool proper)` — 11 cases
* `should_parse_webrip480p_quality(string title, bool proper)` — 3 cases
* `should_parse_bluray576p_quality(string title, bool proper)` — 1 cases
* `should_parse_hdtv720p_quality(string title, bool proper)` — 22 cases
* `should_parse_hdtv1080p_quality(string title, bool proper)` — 16 cases
* `should_parse_hdtv2160p_quality(string title, bool proper)` — 4 cases
* `should_parse_webdl720p_quality(string title, bool proper)` — 28 cases
* `should_parse_webrip720p_quality(string title, bool proper)` — 4 cases
* `should_parse_webdl1080p_quality(string title, bool proper)` — 35 cases
* `should_parse_webrip1080p_quality(string title, bool proper)` — 9 cases
* `should_parse_webdl2160p_quality(string title, bool proper)` — 14 cases
* `should_parse_webrip2160p_quality(string title, bool proper)` — 6 cases
* `should_parse_bluray720p_quality(string title, bool proper)` — 17 cases
* `should_parse_bluray1080p_quality(string title, bool proper)` — 15 cases
* `should_parse_bluray1080p_remux_quality(string title, bool proper)` — 10 cases
* `should_parse_bluray2160p_quality(string title, bool proper)` — 7 cases
* `should_parse_bluray2160p_remux_quality(string title, bool proper)` — 7 cases
* `should_parse_raw_quality(string title, bool proper)` — 9 cases
* `quality_parse(string title, bool proper)` — 4 cases
* `should_parse_full_quality_from_name(string title)` — 9 cases
* `should_parse_resolution_from_name(string title)` — 4 cases
* `should_parse_source_from_name(string title)` — 4 cases
* `should_parse_quality_from_extension(string title)` — 4 cases
* `should_parse_resolution_from_name_and_source_from_extension(string title)` — 2 cases
* `should_be_able_to_parse_repack(string title, bool isRepack, int version)` — 6 cases
* `should_be_able_to_parse_anime_version(string title, int version)` — 1 cases

```csharp
[TestCase("Series S09E03 1080p WEB DL DD5 1 H264 NFHD", false)]  // -> should_parse_webdl1080p_quality
[TestCase("Series.S01E08.Pro.Gamer.1440p.BKPL.WEB-DL.H.264-LiGHT", false)]  // -> should_parse_webdl1080p_quality
[TestCase("Series.Title.S04E01.1080p.WEB.H266-GROUP", false)]  // -> should_parse_webdl1080p_quality
```

#### `ParserTests/ReleaseComparerFixture.cs` — 0 active `[TestCase]`


#### `ParserTests/ReleaseGroupParserFixture.cs` — 151 active `[TestCase]`

* `should_parse_release_group(string title, string expected)` — 45 cases
* `should_parse_exception_release_group(string title, string expected)` — 68 cases
* `should_not_include_language_in_release_group(string title, string expected)` — 3 cases
* `should_not_include_repost_in_release_group(string title, string expected)` — 26 cases
* `should_parse_anime_release_groups(string title, string expected)` — 5 cases
* `should_not_parse_anime_hash_as_release_group(string title)` — 1 cases
* `should_not_parse_episode_title_as_release_group(string title)` — 3 cases

```csharp
[TestCase("Show.Name.S01E01.1080p.WEB-DL.H264.Fight-BB.mkv", "Fight-BB")]  // -> should_parse_exception_release_group
[TestCase("Example (2013) S01E01 (1080p iP WEBRip x265 SDR AAC 2.0 English - DarQ)", "DarQ")]  // -> should_parse_exception_release_group
[TestCase("The Series (2025) - S03E01 - We're Already Here (1080p AMZN WEB-DL x265 Celdra)", "Celdra")]  // -> should_parse_exception_release_group
```

#### `ParserTests/SceneCheckerFixture.cs` — 13 active `[TestCase]`

* `should_return_true_for_scene_names(string title)` — 5 cases
* `should_return_false_for_non_scene_names(string title)` — 8 cases

```csharp
[TestCase("S08E05 - Virtual In-Stanity [WEBDL-720p]")]  // -> should_return_false_for_non_scene_names
[TestCase("Series.Title.S04E17.720p.HDTV.X264", Description = "no group")]  // -> should_return_false_for_non_scene_names
[TestCase("Series.Title.720p.HDTV.X264-DIMENSION", Description = "no episode")]  // -> should_return_false_for_non_scene_names
```

#### `ParserTests/SeasonNumbersFormatterFixture.cs` — 8 active `[TestCase]`

* `should_format_season_numbers(int[] seasonNumbers, string expected)` — 5 cases
* `should_format_season_numbers_with_number_format(int[] seasonNumbers, string expected)` — 3 cases

```csharp
[TestCase(new[] { 2 }, "Season 2")]  // -> should_format_season_numbers
[TestCase(new[] { 1, 2, 3 }, "Seasons 1-3")]  // -> should_format_season_numbers
[TestCase(new[] { 1, 3, 5 }, "Seasons 1, 3, 5")]  // -> should_format_season_numbers
```

#### `ParserTests/SeasonParserFixture.cs` — 76 active `[TestCase]`

* `should_parse_full_season_release(string postTitle, string title, int season)` — 44 cases
* `should_parse_season_extras(string postTitle, string title, int season)` — 5 cases
* `should_parse_season_subpack(string postTitle, string title, int season)` — 3 cases
* `should_parse_partial_season_release(string postTitle, string title, int season, int seasonPart)` — 6 cases
* `should_parse_multi_season_release(string postTitle, string title, int[] expectedSeasons)` — 14 cases
* `should_not_be_multi_season_for_a_single_season_release(string postTitle, int season)` — 4 cases

```csharp
[TestCase("30.Series.Season.04.HDTV.XviD-DIMENSION", "30 Series", 4)]  // -> should_parse_full_season_release
[TestCase("Series Title - Series 1 (1970) DivX", "Series Title", 1)]  // -> should_parse_full_season_release
[TestCase("1234 (2026) (S03) [MULTi] [1080p] [NF] [WEB-DL] [H.264] [DD5.1-RX] [Serial polski]", "1234 (2026)", 3)]  // -> should_parse_full_season_release
```

#### `ParserTests/SeasonTitleParserFixture.cs` — 21 active `[TestCase]`

* `should_return_the_title_when_there_is_no_season_or_episode(string releaseTitle, string expectedTitle)` — 5 cases
* `should_populate_the_fields_the_decision_engine_reads(string releaseTitle)` — 2 cases
* `should_not_return_a_title_when_stripping_leaves_metadata(string releaseTitle)` — 3 cases
* `should_not_return_a_title_for_an_extra(string releaseTitle)` — 5 cases
* `should_not_flag_a_release_the_regexes_can_parse(string releaseTitle)` — 3 cases
* `should_leave_parse_title_rejecting_what_it_rejects_today(string releaseTitle)` — 3 cases

```csharp
[TestCase("[Group] Series Title Curse (BS11 1280x720 x264 AAC)", "Series Title Curse")]  // -> should_return_the_title_when_there_is_no_season_or_episode
[TestCase("[Group] Series.Title - Curse (BD, 720p)", "Series.Title - Curse")]  // -> should_return_the_title_when_there_is_no_season_or_episode
[TestCase("[Group] Series Title: Curse [Dual Audio 10bit 1080p][HEVC-x265]", "Series Title: Curse")]  // -> should_return_the_title_when_there_is_no_season_or_episode
```

#### `ParserTests/SeriesTitleInfoFixture.cs` — 0 active `[TestCase]`


#### `ParserTests/SingleEpisodeParserFixture.cs` — 185 active `[TestCase]`

* `should_parse_single_episode(string postTitle, string title, int seasonNumber, int episodeNumber)` — 176 cases
* `should_parse_japanese_variety_show_format(string postTitle, string title, int seasonNumber, int episodeNumber)` — 7 cases
* `should_parse_decimal_number_as_special(string postTitle, string title, int seasonNumber, int episodeNumber)` — 1 cases
* `should_parse_split_episode(string postTitle, string title, int seasonNumber, int episodeNumber)` — 1 cases

```csharp
[TestCase("Series.With.Title.S02E15", "Series With Title", 2, 15)]  // -> should_parse_single_episode
[TestCase("20-1.2014.S02E01.720p.HDTV.x264-CROOKS", "20-1 2014", 2, 1)]  // -> should_parse_single_episode
[TestCase("Series - S03E10 - Episode #3.10 - rus 720p WEBDL (LostFilm)", "Series", 3, 10)]  // -> should_parse_single_episode
```

#### `ParserTests/SubGroupParserFixture.cs` — 1 active `[TestCase]`

* `should_parse_sub_group_from_title_as_release_group(string title, string expected)` — 1 cases

```csharp
[TestCase("[GHOST][1080p] Series - 25 [BD HEVC 10bit Dual Audio AC3][AE0ADDBA]", "GHOST")]  // -> should_parse_sub_group_from_title_as_release_group
```

#### `ParserTests/UnicodeReleaseParserFixture.cs` — 56 active `[TestCase]`

* `should_parse_chinese_anime_releases(string postTitle, string title, string subgroup, int absoluteEpisodeNumber)` — 16 cases
* `should_parse_chinese_anime_season_episode_releases(string postTitle, string title, string subgroup, int absoluteEpisodeNumber)` — 18 cases
* `should_parse_unbracketed_chinese_anime_releases(string postTitle, string title, string subgroup, int absoluteEpisodeNumber)` — 9 cases
* `should_parse_chinese_multiepisode_releases(string postTitle, string title, string subgroup, int[] absoluteEpisodeNumbers)` — 2 cases
* `should_parse_gm_team_releases_and_files(string postTitle, string title, int absoluteEpisodeNumber)` — 7 cases
* `should_parse_unicode_digits(string postTitle, string title, int absoluteEpisodeNumber)` — 1 cases
* `should_parse_false_positive_chinese_anime_releases(string postTitle, string title, string subgroup, int absoluteEpisodeNumber)` — 1 cases
* `should_parse_full_season_releases(string postTitle, string title, string releaseGroup, int season)` — 1 cases
* `should_parse_multi_episode_release(string postTitle, string title, string releaseGroup, int season, int[] episodes)` — 1 cases

```csharp
[TestCase("[Lilith-Raws] 在地下城尋求邂逅是否搞錯了什麼 / Anime-Series Title S04 - 12 [Baha][WEB-DL][1080p][AVC AAC][CHT][MP4]", "Anime-Series Title S4", "Lilith-Raws", 12)]  // -> should_parse_chinese_anime_season_episode_releases
[TestCase("[天月搬运组] 不要欺负我，长瀞同学 2nd Attack / Anime-Series Title S02 - 01 [1080P][简繁日外挂]", "Anime-Series Title S2", "天月搬运组", 1)]  // -> should_parse_chinese_anime_season_episode_releases
[TestCase("[Q] 全职高手 第3季 | Series S3 - 09 (1080p HBR HEVC Multi-Sub)", "Series S3", "Q", 9)]  // -> should_parse_chinese_anime_season_episode_releases
```

#### `ParserTests/UrlFixture.cs` — 31 active `[TestCase]`

* `should_not_parse_url_in_name(string postTitle, string title)` — 16 cases
* `should_not_parse_url_in_group(string title, string expected)` — 15 cases

```csharp
[TestCase("[www.test.com] - Series.S03E14.720p.HDTV.X264-DIMENSION", "Series")]  // -> should_not_parse_url_in_name
[TestCase("[www.test123.de] - Mad Series - Season 1 [Bluray720p]", "Mad Series")]  // -> should_not_parse_url_in_name
[TestCase("(seriesawake.com) Series Super - 57 [720p] [English Subbed]", "Series Super")]  // -> should_not_parse_url_in_name
```

#### `ParserTests/ValidateParsedEpisodeInfoFixture.cs` — 0 active `[TestCase]`


`HashedReleaseFixture` uses `TestCaseSource(HashedReleaseParserCases)`: 20 rows of `{ path.AsOsAgnostic(), seriesTitle,
Quality, releaseGroup }` checked through `Parser.ParsePath` (e.g. `@"C:\Test\Some.Hashed.Release.S01E01.720p.WEB-DL.AAC2.0.H.264-Mercury\0e895c37245186812cb08aab1529cf8ee389dd05.mkv"`,
`"Some Hashed Release"`, `Quality.WEBDL720p`, `"Mercury"`; also a reversed-title row
`yrucreM-462.H.0.2CAA.LD-BEW.p027.10E10S.esaeleR.dehsaH.emoS.mkv`).
`ParsingServiceTests/*` (GetEpisodes/GetSeries/Map) are mock-based (Moq verifies which repository calls happen) — port
as behavioural tests of §5, not as data tables.

### 6.4 Per-file inventory — quality, profiles, decision engine (data-driven parts), aggregation, organizer

#### `Qualities/QualityFinderFixture.cs` — 8 active `[TestCase]`

* `should_return_SDTV(QualitySource source, int resolution)` — 2 cases
* `should_return_HDTV_720p(QualitySource source, int resolution)` — 2 cases
* `should_return_HDTV_1080p(QualitySource source, int resolution)` — 2 cases
* `should_return_Bluray720p(QualitySource source, int resolution)` — 2 cases

```csharp
[TestCase(QualitySource.Television, 480)]  // -> should_return_SDTV
[TestCase(QualitySource.Unknown, 480)]  // -> should_return_SDTV
```

#### `Profiles/Qualities/QualityIndexCompareToFixture.cs` — 10 active `[TestCase]`

* `should_match_expected_when_respect_group_order_is_true(int leftIndex, int leftGroupIndex, int rightIndex, int rightGroupIndex, int expected)` — 5 cases
* `should_match_expected_when_respect_group_order_is_false(int leftIndex, int leftGroupIndex, int rightIndex, int rightGroupIndex, int expected)` — 5 cases

```csharp
[TestCase(1, 0, 1, 0, 0)]  // -> should_match_expected_when_respect_group_order_is_true
[TestCase(2, 0, 1, 0, 1)]  // -> should_match_expected_when_respect_group_order_is_true
[TestCase(1, 0, 2, 0, -1)]  // -> should_match_expected_when_respect_group_order_is_true
```

#### `DecisionEngineTests/AcceptableSizeSpecificationFixture.cs` — 26 active `[TestCase]`

* `single_episode(int runtime, int sizeInMegaBytes, bool expectedResult)` — 6 cases
* `should_return_expected_result_for_first_episode_of_season(int runtime, int sizeInMegaBytes, bool expectedResult)` — 4 cases
* `should_return_expected_result_for_last_episode_of_season(int runtime, int sizeInMegaBytes, bool expectedResult)` — 4 cases
* `multi_episode(int runtime, int sizeInMegaBytes, bool expectedResult)` — 6 cases
* `multiset_episode(int runtime, int sizeInMegaBytes, bool expectedResult)` — 6 cases

```csharp
[TestCase(30, 50, false)]  // -> single_episode
[TestCase(60, 100, false)]  // -> single_episode
[TestCase(60, 1000, false)]  // -> single_episode
```

#### `DecisionEngineTests/RawDiskSpecificationFixture.cs` — 11 active `[TestCase]`

* `should_return_false_if_matches_disc_format(string title)` — 9 cases
* `should_return_true_if_dvdrip(string title)` — 2 cases

```csharp
[TestCase("Series Title S02 Disc 1 1080i Blu-ray DTS-HD MA 2.0 AVC-TrollHD")]  // -> should_return_false_if_matches_disc_format
[TestCase("Series Title S05 Disc 1 BluRay 1080p AVC Atmos TrueHD 7 1-MTeam")]  // -> should_return_false_if_matches_disc_format
[TestCase("Series.of.Desire.2000.S1_D01.NTSC.DVD5")]  // -> should_return_false_if_matches_disc_format
```

#### `DecisionEngineTests/ReleaseRestrictionsSpecificationFixture.cs` — 12 active `[TestCase]`

* `should_ignore_case_when_matching_required(string required)` — 4 cases
* `should_ignore_case_when_matching_ignored(string ignored)` — 4 cases
* `should_match_perl_regex(string pattern, bool expected)` — 4 cases

```csharp
[TestCase("EdiTED")]  // -> should_ignore_case_when_matching_required
[TestCase("X264")]  // -> should_ignore_case_when_matching_required
[TestCase("X264,NOTTHERE")]  // -> should_ignore_case_when_matching_required
```

#### `DecisionEngineTests/SingleEpisodeAgeDownloadDecisionFixture.cs` — 10 active `[TestCase]`

* `single_episode_release(int episode, int seasonSearchMaximumSingleEpisodeAge, bool expectedResult)` — 6 cases
* `multi_episode_release(int seasonSearchMaximumSingleEpisodeAge, bool expectedResult)` — 4 cases

```csharp
[TestCase(1, 200, false)]  // -> single_episode_release
[TestCase(1, 365, true)]  // -> single_episode_release
[TestCase(1, 0, true)]  // -> single_episode_release
```

#### `DecisionEngineTests/Search/TorrentSeedingSpecificationFixture.cs` — 4 active `[TestCase]`

* `should_return_true_if_seeds_above_or_equal_to_limit(int seeders)` — 2 cases
* `should_return_false_if_seeds_belove_limit(int seeders)` — 2 cases

```csharp
[TestCase(5)]  // -> should_return_true_if_seeds_above_or_equal_to_limit
[TestCase(6)]  // -> should_return_true_if_seeds_above_or_equal_to_limit
```

#### `Download/Aggregation/Aggregators/AggregateLanguagesFixture.cs` — 32 active `[TestCase]`

* `should_return_multi_languages_when_indexer_id_has_multi_languages_configuration(string releaseTitle)` — 4 cases
* `should_return_multi_languages_from_indexer_with_id_when_indexer_id_and_name_are_set(string releaseTitle)` — 4 cases
* `should_return_multi_languages_when_indexer_name_has_multi_languages_configuration(string releaseTitle)` — 4 cases
* `should_return_multi_languages_when_release_as_unknown_as_default_language_and_indexer_has_multi_languages_configuration(string releaseTitle)` — 4 cases
* `should_return_multi_languages_when_release_as_specified_language_and_indexer_has_multi_languages_configuration(string releaseTitle)` — 4 cases
* `should_return_multi_languages_when_release_as_other_language_and_indexer_has_multi_languages_configuration(string releaseTitle)` — 4 cases
* `should_return_original_when_indexer_has_no_multi_languages_configuration(string releaseTitle)` — 4 cases
* `should_return_original_when_no_indexer_value(string releaseTitle)` — 4 cases

```csharp
[TestCase("Series.Title.S01E01.MULTi.1080p.WEB.H265-RlsGroup")]  // -> should_return_multi_languages_when_indexer_id_has_multi_languages_configuration
[TestCase("Series Title - Stagione 1 (2025) [COMPLETA] 720p H264 MULTILANG AAC 2.0 MULTISUB-RlsGroup")]  // -> should_return_multi_languages_when_indexer_id_has_multi_languages_configuration
[TestCase("Series Title (2007) S01E01 [Multilang AC3 Sub Spa Eng Rus]")]  // -> should_return_multi_languages_when_indexer_id_has_multi_languages_configuration
```

#### `MediaFiles/EpisodeImport/Aggregation/Aggregators/Augmenters/Quality/AugmentQualityFromMediaInfoFixture.cs` — 17 active `[TestCase]`

* `should_return_closest_resolution(int mediaInfoWidth, int mediaInfoHeight, int expectedResolution)` — 17 cases

```csharp
[TestCase(4096, 1, 2160)] // True 4K  // -> should_return_closest_resolution
[TestCase(1490, 1, 720)]  // -> should_return_closest_resolution
[TestCase(100, 1, 480)]  // -> should_return_closest_resolution
```

#### `OrganizerTests/BuildFilePathFixture.cs` — 6 active `[TestCase]`

* `CalculateFilePath_SeasonFolder_SingleNumber(string filename, int seasonNumber, bool useSeasonFolder, string seasonFolderFormat, string expectedPath)` — 6 cases

```csharp
[TestCase("30 Rock - S01E05 - Episode Title", 1, true, "Season {season:00}", @"C:\Test\30 Rock\Season 01\30 Rock - S01E05 - Episode Title.mkv")]  // -> CalculateFilePath_SeasonFolder_SingleNumber
[TestCase("30 Rock - S01E05 - Episode Title", 1, false, "Season {season}", @"C:\Test\30 Rock\30 Rock - S01E05 - Episode Title.mkv")]  // -> CalculateFilePath_SeasonFolder_SingleNumber
[TestCase("30 Rock - S00E05 - Episode Title", 0, true, "Season {season}", @"C:\Test\30 Rock\MySpecials\30 Rock - S00E05 - Episode Title.mkv")]  // -> CalculateFilePath_SeasonFolder_SingleNumber
```

#### `OrganizerTests/CleanFilenameFixture.cs` — 4 active `[TestCase]`

* `should_replaace_invalid_characters(string name, string expectedName)` — 1 cases
* `should_remove_periods_from_start(string name, string expectedName)` — 1 cases
* `should_remove_spaces_from_start_and_end(string name, string expectedName)` — 2 cases

```csharp
[TestCase(" Series Title - S01E01 - Episode Title", "Series Title - S01E01 - Episode Title")]  // -> should_remove_spaces_from_start_and_end
[TestCase("Series Title - S01E01 - Episode Title ", "Series Title - S01E01 - Episode Title")]  // -> should_remove_spaces_from_start_and_end
```

#### `OrganizerTests/FileNameBuilderTests/AbsoluteEpisodeFormatFixture.cs` — 0 active `[TestCase]`


#### `OrganizerTests/FileNameBuilderTests/CleanTitleFixture.cs` — 29 active `[TestCase]`

* `should_get_expected_title_back(string title, string expected)` — 29 cases

```csharp
[TestCase("Florence + the Machine", "Florence + the Machine")]  // -> should_get_expected_title_back
[TestCase("Is this okay?", "Is this okay")]  // -> should_get_expected_title_back
[TestCase("Joker: Folie à deux", "Joker Folie a deux")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/CleanTitleTheFixture.cs` — 14 active `[TestCase]`

* `should_get_expected_title_back(string title, string expected)` — 10 cases
* `should_not_change_title(string title)` — 4 cases

```csharp
[TestCase("The Mist", "Mist, The")]  // -> should_get_expected_title_back
[TestCase("The Fixer (ZH) (2015)", "Fixer, The ZH 2015")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", "Climax I Almost Got Away With It, The 2016")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/CleanTitleTheWithoutYearFixture.cs` — 4 active `[TestCase]`

* `should_get_expected_title_back(string title, int year, string expected)` — 4 cases

```csharp
[TestCase("The Mist", 2018, "Mist, The")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", 2016, "Climax I Almost Got Away With It, The")]  // -> should_get_expected_title_back
[TestCase("A", 2017, "A")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/CleanTitleTheYearFixture.cs` — 6 active `[TestCase]`

* `should_get_expected_title_back(string title, int year, string expected)` — 6 cases

```csharp
[TestCase("The Mist", 2018, "Mist, The 2018")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", 0, "Climax I Almost Got Away With It, The 2016")]  // -> should_get_expected_title_back
[TestCase("A", 2017, "A 2017")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/CleanTitleWithoutYearFixture.cs` — 5 active `[TestCase]`

* `should_get_expected_title_back(string title, int year, string expected)` — 5 cases

```csharp
[TestCase("The Mist", 2018, "The Mist")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", 2016, "The Climax I Almost Got Away With It")]  // -> should_get_expected_title_back
[TestCase("The Series Title's", 2016, "The Series Title's")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/CleanTitleYearFixture.cs` — 4 active `[TestCase]`

* `should_get_expected_title_back(string title, int year, string expected)` — 4 cases

```csharp
[TestCase("The Mist", 2018, "The Mist 2018")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", 2016, "The Climax I Almost Got Away With It 2016")]  // -> should_get_expected_title_back
[TestCase("The Series Title's", 2016, "The Series Titles 2016")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/ColonReplacementFixture.cs` — 12 active `[TestCase]`

* `should_replace_colon_followed_by_space_with_expected_result(string seriesName, ColonReplacementFormat replacementFormat, string expected)` — 5 cases
* `should_replace_colon_with_expected_result(string seriesName, ColonReplacementFormat replacementFormat, string expected)` — 5 cases
* `should_replace_colon_with_custom_format(string seriesName, ColonReplacementFormat replacementFormat, string customFormat, string expected)` — 2 cases

```csharp
[TestCase("CSI: Vegas", ColonReplacementFormat.Smart, "CSI - Vegas")]  // -> should_replace_colon_followed_by_space_with_expected_result
[TestCase("CSI: Vegas", ColonReplacementFormat.Delete, "CSI Vegas")]  // -> should_replace_colon_followed_by_space_with_expected_result
[TestCase("CSI: Vegas", ColonReplacementFormat.SpaceDashSpace, "CSI - Vegas")]  // -> should_replace_colon_followed_by_space_with_expected_result
```

#### `OrganizerTests/FileNameBuilderTests/CustomFormatsFixture.cs` — 20 active `[TestCase]`

* `should_replace_custom_formats(string format, string expected)` — 1 cases
* `should_replace_custom_formats_with_no_custom_formats(string format, string expected)` — 1 cases
* `should_replace_custom_formats_with_filtered_names(string format, string expected)` — 6 cases
* `should_not_replace_custom_formats_due_to_invalid_token(string format, string expected)` — 2 cases
* `should_replace_custom_format(string format, string expected)` — 7 cases
* `should_replace_custom_format_with_no_custom_formats(string format, string expected)` — 3 cases

```csharp
[TestCase("{Custom Format}", "")]  // -> should_replace_custom_format
[TestCase("{Custom Format:NAME WITH SPACES}", "NAME WITH SPACES")]  // -> should_replace_custom_format
[TestCase("{Custom Format:AMZN} - {Custom Format:INTERNAL}", "AMZN - INTERNAL")]  // -> should_replace_custom_format
```

#### `OrganizerTests/FileNameBuilderTests/EpisodeTitleCollapseFixture.cs` — 8 active `[TestCase]`

* `should_collapse_episode_titles_when_episode_titles_are_the_same(string title1, string title2, string expected)` — 8 cases

```csharp
[TestCase("Hey, Baby, What's Wrong (1)", "Hey, Baby, What's Wrong (2)", "Hey, Baby, What's Wrong")]  // -> should_collapse_episode_titles_when_episode_titles_are_the_same
[TestCase("Meet the Guys and Girls of Cycle 20 Part 01", "Meet the Guys and Girls of Cycle 20 Part 02", "Meet the Guys and Girls of Cycle 20")]  // -> should_collapse_episode_titles_when_episode_titles_are_the_same
[TestCase("Meet the Guys and Girls of Cycle 20 pt. 1", "Meet the Guys and Girls of Cycle 20 pt. 2", "Meet the Guys and Girls of Cycle 20")]  // -> should_collapse_episode_titles_when_episode_titles_are_the_same
```

#### `OrganizerTests/FileNameBuilderTests/FileNameBuilderFixture.cs` — 56 active `[TestCase]`

* `should_escape_token_in_format(string format, string expected)` — 4 cases
* `should_format_languagecodes_properly(string language, string code)` — 16 cases
* `should_trim_extra_separators_from_end_when_quality_proper_is_not_included(char separator)` — 4 cases
* `should_trim_extra_separators_from_middle_when_quality_proper_is_not_included(char separator)` — 4 cases
* `should_not_use_Sonarr_as_release_group_if_pattern_has_separator(string pattern, string expectedFileName)` — 3 cases
* `should_use_existing_casing_for_release_group(string releaseGroup)` — 3 cases
* `should_replace_all_tokens_for_different_cultures(string culture)` — 4 cases
* `should_format_audio_languages(string audioLanguages, string expected)` — 2 cases
* `should_format_audio_languages_all(string audioLanguages, string expected)` — 2 cases
* `should_format_subtitle_languages_all(string subtitleLanguages, string format, string expected)` — 9 cases
* `should_include_hdr_for_mediainfo_videodynamicrange_with_valid_properties(HdrFormat hdrFormat, string expectedName)` — 3 cases
* `should_use_updated_media_info_if_token_configured_and_revision_is_old(string standardEpisodeFormat)` — 2 cases

```csharp
[TestCase("nob", "NB")]  // -> should_format_languagecodes_properly
[TestCase("gsw", "DE")]  // -> should_format_languagecodes_properly
[TestCase("gle", "GA")]  // -> should_format_languagecodes_properly
```

#### `OrganizerTests/FileNameBuilderTests/IdFixture.cs` — 0 active `[TestCase]`


#### `OrganizerTests/FileNameBuilderTests/MultiEpisodeFixture.cs` — 0 active `[TestCase]`


#### `OrganizerTests/FileNameBuilderTests/MultiEpisodeTitleFixture.cs` — 3 active `[TestCase]`

* `should_replace_Series_space_Title(string firstTitle, string secondTitle)` — 3 cases

```csharp
[TestCase("Episode Title (1)", "Episode Title (2)")]  // -> should_replace_Series_space_Title
[TestCase("Episode Title Part 1", "Episode Title Part 2")]  // -> should_replace_Series_space_Title
[TestCase("Episode Title", "Episode Title: Part 2")]  // -> should_replace_Series_space_Title
```

#### `OrganizerTests/FileNameBuilderTests/OriginalTitleFixture.cs` — 0 active `[TestCase]`


#### `OrganizerTests/FileNameBuilderTests/ReplaceCharacterFixure.cs` — 11 active `[TestCase]`

* `should_replace_illegal_characters(string title, string expected)` — 11 cases

```csharp
[TestCase("CSI: Crime Scene Investigation", "CSI - Crime Scene Investigation")]  // -> should_replace_illegal_characters
[TestCase("Less Than<", "Less Than")]  // -> should_replace_illegal_characters
[TestCase("Quotes\"", "Quotes")]  // -> should_replace_illegal_characters
```

#### `OrganizerTests/FileNameBuilderTests/RequiresAbsoluteEpisodeNumberFixture.cs` — 0 active `[TestCase]`


#### `OrganizerTests/FileNameBuilderTests/RequiresEpisodeTitleFixture.cs` — 0 active `[TestCase]`


#### `OrganizerTests/FileNameBuilderTests/ReservedDeviceNameFixture.cs` — 6 active `[TestCase]`

* `should_replace_reserved_device_name_in_series_folder(string title, string expected)` — 2 cases
* `should_replace_reserved_device_name_in_season_folder(string title, string expected)` — 2 cases
* `should_replace_reserved_device_name_in_file_name(string title, string expected)` — 2 cases

```csharp
[TestCase("Con Game", "Con_Game")]  // -> should_replace_reserved_device_name_in_series_folder
[TestCase("Com1 Sat", "Com1_Sat")]  // -> should_replace_reserved_device_name_in_series_folder
```

#### `OrganizerTests/FileNameBuilderTests/SeriesTitleFirstCharacterFixture.cs` — 9 active `[TestCase]`

* `should_get_expected_folder_name_back(string title, string expected)` — 9 cases

```csharp
[TestCase("The Mist", "M\\The Mist")]  // -> should_get_expected_folder_name_back
[TestCase("좀비버스", "좀\\좀비버스")]  // -> should_get_expected_folder_name_back
[TestCase("Æon Flux", "A\\Æon Flux")]  // -> should_get_expected_folder_name_back
```

#### `OrganizerTests/FileNameBuilderTests/SeriesYearFixture.cs` — 3 active `[TestCase]`

* `should_get_expected_folder_name_back(string title, int year, string expected)` — 3 cases

```csharp
[TestCase("The Mist", 2017, "2017\\The Mist")]  // -> should_get_expected_folder_name_back
[TestCase("A", 2021, "2021\\A")]  // -> should_get_expected_folder_name_back
[TestCase("30 Rock", 2006, "2006\\30 Rock")]  // -> should_get_expected_folder_name_back
```

#### `OrganizerTests/FileNameBuilderTests/TitleTheFixture.cs` — 14 active `[TestCase]`

* `should_get_expected_title_back(string title, string expected)` — 10 cases
* `should_not_change_title(string title)` — 4 cases

```csharp
[TestCase("The Mist", "Mist, The")]  // -> should_get_expected_title_back
[TestCase("The Fixer (ZH) (2015)", "Fixer, The (ZH) (2015)")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", "Climax - I (Almost) Got Away With It, The (2016)")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/TitleTheWithoutYearFixture.cs` — 4 active `[TestCase]`

* `should_get_expected_title_back(string title, int year, string expected)` — 4 cases

```csharp
[TestCase("The Mist", 2018, "Mist, The")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", 2016, "Climax - I (Almost) Got Away With It, The")]  // -> should_get_expected_title_back
[TestCase("A", 2017, "A")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/TitleTheYearFixture.cs` — 4 active `[TestCase]`

* `should_get_expected_title_back(string title, int year, string expected)` — 4 cases

```csharp
[TestCase("The Mist", 2018, "Mist, The (2018)")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", 2016, "Climax - I (Almost) Got Away With It, The (2016)")]  // -> should_get_expected_title_back
[TestCase("A", 2017, "A (2017)")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/TitleWithoutYearFixture.cs` — 3 active `[TestCase]`

* `should_get_expected_title_back(string title, int year, string expected)` — 3 cases

```csharp
[TestCase("The Mist", 2018, "The Mist")]  // -> should_get_expected_title_back
[TestCase("The Rat Pack (A&E)", 1999, "The Rat Pack (A&E)")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", 2016, "The Climax - I (Almost) Got Away With It")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/TitleYearFixture.cs` — 3 active `[TestCase]`

* `should_get_expected_title_back(string title, int year, string expected)` — 3 cases

```csharp
[TestCase("The Mist", 2018, "The Mist (2018)")]  // -> should_get_expected_title_back
[TestCase("The Rat Pack (A&E)", 1999, "The Rat Pack (A&E) (1999)")]  // -> should_get_expected_title_back
[TestCase("The Climax: I (Almost) Got Away With It (2016)", 2016, "The Climax - I (Almost) Got Away With It (2016)")]  // -> should_get_expected_title_back
```

#### `OrganizerTests/FileNameBuilderTests/TruncatedEpisodeTitlesFixture.cs` — 0 active `[TestCase]`


#### `OrganizerTests/FileNameBuilderTests/TruncatedReleaseGroupFixture.cs` — 0 active `[TestCase]`


#### `OrganizerTests/FileNameBuilderTests/TruncatedSeriesTitleFixture.cs` — 3 active `[TestCase]`

* `should_truncate_series_title(string format, string expected)` — 3 cases

```csharp
[TestCase("{Series Title:16}", "The Fantastic...")]  // -> should_truncate_series_title
[TestCase("{Series TitleThe:17}", "Fantastic Life...")]  // -> should_truncate_series_title
[TestCase("{Series CleanTitle:-13}", "...Mr. Sisko")]  // -> should_truncate_series_title
```

#### `OrganizerTests/GetSeasonFolderFixture.cs` — 7 active `[TestCase]`

* `should_use_seriesFolderFormat_to_build_folder_name(string seriesTitle, int seasonNumber, string format, string expected)` — 5 cases
* `should_not_throw_when_season_has_no_title(string format)` — 2 cases

```csharp
[TestCase("Venture Bros.", 1, "{Series.Title}.{season:00}", "Venture.Bros.01")]  // -> should_use_seriesFolderFormat_to_build_folder_name
[TestCase("Series Title?", 1, "{Series Title} Season {season:00}", "Series Title! Season 01")]  // -> should_use_seriesFolderFormat_to_build_folder_name
[TestCase("Series Title?", 1, "Season {season:00} - {Season CleanTitle}", "Season 01 - Part 1")]  // -> should_use_seriesFolderFormat_to_build_folder_name
```

#### `OrganizerTests/GetSeriesFolderFixture.cs` — 6 active `[TestCase]`

* `should_use_seriesFolderFormat_to_build_folder_name(string seriesTitle, string format, string expected)` — 6 cases

```csharp
[TestCase("30 Rock", "{Series Title}", "30 Rock")]  // -> should_use_seriesFolderFormat_to_build_folder_name
[TestCase("Venture Bros.", "{Series.Title}", "Venture.Bros")]  // -> should_use_seriesFolderFormat_to_build_folder_name
[TestCase("30 Rock", ".{Series.Title}.", "30.Rock")]  // -> should_use_seriesFolderFormat_to_build_folder_name
```

---

## 7. Quality model, definitions and profiles

### 7.1 `Quality` (`NzbDrone.Core/Qualities/Quality.cs`) — every quality

Equality/hash by `Id` only. `Quality.All` order (L128–152) is: Unknown, SDTV, DVD, WEBRip-480p, WEBDL-480p, Bluray-480p,
Bluray-576p, HDTV-720p, WEBRip-720p, WEBDL-720p, Bluray-720p, Bluray-1080p, HDTV-1080p, WEBRip-1080p, WEBDL-1080p,
Raw-HD, HDTV-2160p, WEBRip-2160p, WEBDL-2160p, Bluray-2160p, Bluray-1080p Remux, Bluray-2160p Remux.
`FindById(0)` → Unknown; unknown id → `ArgumentException`. Id 11 (`HDTV-480p`) is commented out / unused.

| Id | Name (exact) | Source | Resolution | Default Weight | Group (GroupName) | MinSize | MaxSize | PreferredSize |
|---|---|---|---|---|---|---|---|---|
| 0 | `Unknown` | Unknown | 0 | 1 | – | 1 | 199.9 | 95 |
| 1 | `SDTV` | Television | 480 | 2 | – | 2 | 100 | 95 |
| 12 | `WEBRip-480p` | WebRip | 480 | 3 | `WEB 480p` | 2 | 100 | 95 |
| 8 | `WEBDL-480p` | Web | 480 | 3 | `WEB 480p` | 2 | 100 | 95 |
| 2 | `DVD` | DVD | 480 | 4 | – | 2 | 100 | 95 |
| 13 | `Bluray-480p` | Bluray | 480 | 5 | – | 2 | 100 | 95 |
| 22 | `Bluray-576p` | Bluray | 576 | 6 | – | 2 | 100 | 95 |
| 4 | `HDTV-720p` | Television | 720 | 7 | – | 3 | 125 | 95 |
| 9 | `HDTV-1080p` | Television | 1080 | 8 | – | 4 | 125 | 95 |
| 10 | `Raw-HD` | TelevisionRaw | 1080 | 9 | – | 4 | null (∞) | 95 |
| 14 | `WEBRip-720p` | WebRip | 720 | 10 | `WEB 720p` | 3 | 130 | 95 |
| 5 | `WEBDL-720p` | Web | 720 | 10 | `WEB 720p` | 3 | 130 | 95 |
| 6 | `Bluray-720p` | Bluray | 720 | 11 | – | 4 | 130 | 95 |
| 15 | `WEBRip-1080p` | WebRip | 1080 | 12 | `WEB 1080p` | 4 | 130 | 95 |
| 3 | `WEBDL-1080p` | Web | 1080 | 12 | `WEB 1080p` | 4 | 130 | 95 |
| 7 | `Bluray-1080p` | Bluray | 1080 | 13 | – | 4 | 155 | 95 |
| 20 | `Bluray-1080p Remux` | BlurayRaw | 1080 | 14 | – | 35 | null | 95 |
| 16 | `HDTV-2160p` | Television | 2160 | 15 | – | 35 | 199.9 | 95 |
| 17 | `WEBRip-2160p` | WebRip | 2160 | 16 | `WEB 2160p` | 35 | null | 95 |
| 18 | `WEBDL-2160p` | Web | 2160 | 16 | `WEB 2160p` | 35 | null | 95 |
| 19 | `Bluray-2160p` | Bluray | 2160 | 17 | – | 35 | null | 95 |
| 21 | `Bluray-2160p Remux` | BlurayRaw | 2160 | 18 | – | 35 | null | 95 |

Table rows are in `DefaultQualityDefinitions` declaration order (L158–179; HashSet enumeration = insertion order).
Sizes are **MB per minute of runtime** (doubles; null max = unlimited) — see AcceptableSizeSpecification §9.
`QualityDefinitionLimits { Min = 0, Max = 1000 }` (API validation bounds).

### 7.2 `QualityDefinition` & `QualityDefinitionService` (`Qualities/QualityDefinition*.cs`)

* `QualityDefinition : ModelBase { Quality, Title (default = Quality.Name), GroupName, Weight, MinSize?, MaxSize?, PreferredSize? }`.
* On `ApplicationStartedEvent` → `InsertMissingDefinitions()`: inserts any default definition whose Quality is missing,
  keeps existing rows, **deletes** rows for qualities no longer defined.
* `Weight` is never trusted from the DB: `WithWeight()` overwrites it from the defaults on every read. `All()` is ordered by Weight.
* `ResetQualityDefinitionsCommand` only resets `Title` (if `ResetTitles`) — **it does not reset sizes** server-side.
* v5: size limits are also **copied into each QualityProfile item** (`QualityProfileQualityItem.MinSize/MaxSize/PreferredSize`);
  `QualityProfileService.UpdateAllSizeLimits(QualityProfileSizeLimit[])` pushes definition sizes into every profile.
  The decision engine reads sizes from the **profile item** (§9.3). (v4 read them from QualityDefinition — **UNVERIFIED**.)

### 7.3 `QualityProfile` (`Profiles/Qualities/QualityProfile.cs`)

```
QualityProfile : ModelBase {
  string Name; bool UpgradeAllowed; int Cutoff;            // Cutoff = quality Id OR group Id (>= 1000)
  int MinFormatScore; int CutoffFormatScore; int MinUpgradeFormatScore;
  List<ProfileFormatItem> FormatItems;                     // { CustomFormat Format; int Score }
  List<QualityProfileQualityItem> Items;                   // index 0 = worst ... last = best
}
QualityProfileQualityItem { int Id (group id, 0 for single); string Name (group name); Quality Quality (null for groups);
  List<QualityProfileQualityItem> Items (group members); bool Allowed; double? MinSize, MaxSize, PreferredSize }
```
* `GetIndex(qualityOrId, respectGroupOrder=false)`: scan items in order; a single item matches by `Quality.Id`; a group
  matches by its own `Id` (>0) **or** by a member quality id; members return `QualityIndex(i, g)` when respectGroupOrder
  else `QualityIndex(i)`; not found → `QualityIndex(0,0)` (same as the first item!).
* `QualityIndex.CompareTo(other, respectGroupOrder)`: compare `Index`; if equal and respectGroupOrder → compare `GroupIndex`.
  (Plain integer compare: a member later in the group's `Items` list ranks higher when group order is respected.)
  `CompareTo(null)` → 1.
  Default `CompareTo(object)` uses respectGroupOrder = **true**.
* `FirststAllowedQuality()` (sic) / `LastAllowedQuality()`: first/last allowed item; for groups first/last member.
* `CalculateCustomFormatScore(formats)` = sum of `Score` of `FormatItems` whose `Format` is in `formats` (CustomFormat
  equality = by Id, §8).
* `QualityProfileRankService` (v5) precomputes `score = index / (items-1)` per quality for DB sorting in API/history/queue
  lists only — not used by the decision engine.

### 7.4 Default profiles created on first start (`Profiles/Qualities/QualityProfileService.cs` `Handle(ApplicationStartedEvent)`)

Only if **no** profiles exist. `GetDefaultProfile(name, cutoff, allowed…)` builds `Items` from
`DefaultQualityDefinitions.GroupBy(Weight)` (first-occurrence order = table order above): single qualities become plain
items; weights shared by 2 qualities become groups with ids **1000 = "WEB 480p" (WEBRip-480p, WEBDL-480p), 1001 = "WEB 720p",
1002 = "WEB 1080p", 1003 = "WEB 2160p"**; a group is allowed if any member is in `allowed` (members inherit the group's
Allowed). Each item gets the definition's Min/Max/Preferred size. If the cutoff quality is in a group, `Cutoff = groupId`.
`FormatItems` = all existing custom formats with score 0; `MinFormatScore = 0`, `CutoffFormatScore = 0`,
`MinUpgradeFormatScore = 1`; **`UpgradeAllowed` not set → false**.

| Name | Cutoff | Allowed |
|---|---|---|
| `Any` | SDTV (1) | SDTV, WEBRip-480p, WEBDL-480p, DVD, Bluray-480p, Bluray-576p, HDTV-720p, HDTV-1080p, WEBRip-720p, WEBDL-720p, WEBRip-1080p, WEBDL-1080p, Bluray-720p, Bluray-1080p |
| `SD` | SDTV (1) | SDTV, WEBRip-480p, WEBDL-480p, DVD, Bluray-480p, Bluray-576p |
| `HD-720p` | HDTV-720p (4) | HDTV-720p, WEBRip-720p, WEBDL-720p, Bluray-720p |
| `HD-1080p` | HDTV-1080p (9) | HDTV-1080p, WEBRip-1080p, WEBDL-1080p, Bluray-1080p |
| `Ultra-HD` | HDTV-2160p (16) | HDTV-2160p, WEBRip-2160p, WEBDL-2160p, Bluray-2160p |
| `HD - 720p/1080p` | HDTV-720p (4) | HDTV-720p, HDTV-1080p, WEBRip-720p, WEBDL-720p, WEBRip-1080p, WEBDL-1080p, Bluray-720p, Bluray-1080p |

(The first quality argument in each `AddDefaultProfile` call is the cutoff; it is repeated as the first allowed quality.)
Custom format events: on CF add, every profile gets `{Format, Score 0}` **inserted at index 0**; on CF delete the item is
removed and, if no FormatItems remain, `MinFormatScore=0, CutoffFormatScore=0, MinUpgradeFormatScore=1`.
Delete is refused (`QualityProfileInUseException`) if any series or import list uses the profile.

### 7.5 Delay profiles (`Profiles/Delay/DelayProfile.cs`, `DelayProfileService.cs`)

`DelayProfile { EnableUsenet, EnableTorrent, PreferredProtocol (DownloadProtocol: Unknown=0, Usenet=1, Torrent=2),
UsenetDelay, TorrentDelay (minutes), Order, BypassIfHighestQuality, BypassIfAboveCustomFormatScore,
MinimumCustomFormatScore, Tags }`; `GetProtocolDelay(p)` = Torrent ? TorrentDelay : UsenetDelay.
* `BestForTags(seriesTags)` = profiles where `Tags ∩ seriesTags ≠ ∅ || Tags empty`, ordered by `Order` asc, **first**
  (cached 30 s). New profiles get `Order = count`; the default profile has `Order = int.MaxValue` so it is always last.
* Default (migration 070 + 156): `EnableUsenet=true, EnableTorrent=true, PreferredProtocol=Usenet, delays 0,
  Order=int.MaxValue, Tags=[]`, `BypassIfHighestQuality=true` (156 sets all existing rows true),
  `BypassIfAboveCustomFormatScore=false`, `MinimumCustomFormatScore=null→0`.

### 7.6 Release profiles (`Profiles/Releases/*.cs`)

`ReleaseProfile { Name, Enabled=true, Required: List<string>, Ignored: List<string>, AirDateRestriction (v5),
AirDateGracePeriod (v5, **hours** — used with `AddHours`), AllowSeasonPackWithoutAllEpisodesAired (v5), IndexerIds: List<int> (empty = all),
Tags, ExcludedTags (v5) }`.
* `EnabledForTags(seriesTags, indexerId)` = `(Tags ∩ seriesTags ≠ ∅ || Tags empty) && !(ExcludedTags ∩ seriesTags ≠ ∅)`
  `&& Enabled && (IndexerIds contains indexerId || IndexerIds empty)`.
* Terms: `TermMatcherService` caches a matcher per term (24 h). `PerlRegexFactory.TryCreateRegex(term)`: if the term matches
  `/(?<pattern>.*)/(?<modifiers>[a-z]*)` (**not anchored**, greedy) → .NET Regex with modifiers `m`→Multiline,
  `s`→Singleline, `i`→IgnoreCase, `x`→IgnorePatternWhitespace, `n`→ExplicitCapture, any other letter → `ArgumentException`.
  Otherwise `CaseInsensitiveTermMatcher`: `value.ToLowerInvariant().Contains(term.ToLowerInvariant())`.
  `MatchingTerm(value)` returns the regex match value / the original term, or null.
* Preferred-word scoring (v3) is gone; `ReleaseProfilePreferredComparer` is vestigial.

---

## 8. Custom formats (`NzbDrone.Core/CustomFormats/`)

### 8.1 Model

`CustomFormat : ModelBase { string Name; bool IncludeCustomFormatWhenRenaming; List<ICustomFormatSpecification> Specifications }`;
equality and hash by **Id** only. Each specification: `{ Name, Negate, Required, Order (type constant), ImplementationName }`
plus type-specific fields. `CustomFormatSpecificationBase.IsSatisfiedBy` = `IsSatisfiedByWithoutNegate(input)` XOR `Negate`
(LanguageSpecification overrides, see below).

`CustomFormatInput { ParsedEpisodeInfo EpisodeInfo; Series Series; long Size; IndexerFlags IndexerFlags;
List<Language> Languages; string Filename; ReleaseType ReleaseType }`. For a release (`ParseCustomFormat(RemoteEpisode, size)`):
`EpisodeInfo = remote.ParsedEpisodeInfo`, `Series`, `Size = release size`, `Languages = remote.Languages` (aggregated, §4.4),
`IndexerFlags = Release.IndexerFlags ?? 0`, `ReleaseType = ParsedEpisodeInfo.ReleaseType`, `Filename = null`.
For episode files: `ReleaseTitle` = SceneName, else file name of OriginalFilePath, else file name of RelativePath;
`Filename = file name of RelativePath`; Quality/Languages/ReleaseGroup/IndexerFlags/ReleaseType/Size from the EpisodeFile.
For history/blocklist: ReleaseTitle = `ParseTitle(sourceTitle)?.ReleaseTitle ?? sourceTitle`, ReleaseGroup from parse,
Quality/Languages stored; history reads size/indexerFlags/releaseType from `history.Data`.

### 8.2 Matching algorithm (`CustomFormatCalculationService.ParseCustomFormat(input, allFormats)`)

For each custom format: group its specifications **by implementation type**; for each group compute every spec's
`IsSatisfiedBy(input)`; a group matches (`SpecificationMatchesGroup.DidMatch`) iff **no `Required` spec in the group is false**
AND **at least one spec in the group is true**. The format matches iff **every type-group matches** (AND across types,
OR within a type, Required forces that spec). A format with zero specifications matches (vacuous `All`). Result list is
ordered by `Name` (culture-sensitive string order).

### 8.3 Specification types (`CustomFormats/Specifications/*.cs`)

| Implementation (ImplementationName) | Order | Fields | `IsSatisfiedByWithoutNegate` |
|---|---|---|---|
| `ReleaseTitleSpecification` ("Release Title") | 1 | `Value` (regex) | `regex.IsMatch(EpisodeInfo.ReleaseTitle)` **or** `regex.IsMatch(Filename)`; null strings → false. |
| `LanguageSpecification` ("Language") | 3 | `Value` (language id), `ExceptLanguage` (bool) | compared = (EpisodeInfo≠null && Series≠null && Value == Original(-2) && Series.OriginalLanguage ≠ Unknown) ? Series.OriginalLanguage : Language(Value). ExceptLanguage → `Languages.Any(l => l != compared)`; else `Languages.Contains(compared)`; null list → false. **Negate is special**: returns `!(…)` of the same expression but a null list still gives false. |
| `IndexerFlagSpecification` ("Indexer Flag") | 4 | `Value` (IndexerFlags int) | `input.IndexerFlags.HasFlag((IndexerFlags)Value)` (Value 0 is rejected by validation). |
| `SourceSpecification` ("Source") | 5 | `Value` (QualitySource int) | `EpisodeInfo.Quality.Quality.Source == Value` (missing → Unknown). |
| `ResolutionSpecification` ("Resolution") | 6 | `Value` (Resolution int: 360/480/540/576/720/1080/2160/0) | `EpisodeInfo.Quality.Quality.Resolution == Value` (missing → 0). Note Quality resolutions are only 0/480/576/720/1080/2160, so 360/540 never match a parsed quality. |
| `SizeSpecification` ("Size") | 8 | `Min`, `Max` (GB, double) | `size > Min*1024³ && size <= Max*1024³` (bytes; `Convert.ToInt64` rounding). Validation: Min ≥ 0, Max > Min. |
| `ReleaseGroupSpecification` ("Release Group") | 9 | `Value` (regex) | `regex.IsMatch(EpisodeInfo.ReleaseGroup)` (null → false). |
| `ReleaseTypeSpecification` ("Release Type") | 10 | `Value` (ReleaseType int) | `input.ReleaseType == Value` (Unknown=0, SingleEpisode=1, MultiEpisode=2, SeasonPack=3, MultiSeasonPack=4). |

* Regex specs (`RegexSpecificationBase`): `new Regex(Value, Compiled | IgnoreCase)` — **user-supplied .NET regex**
  (lookbehind etc. allowed) compiled with IgnoreCase; empty Value fails validation; `_regex == null` → no match.
  A Rust port must accept .NET-flavoured patterns from users/TRaSH guides (`fancy-regex` covers most).
* There is **no** QualityModifier, Edition or Year specification in Sonarr (Order 2 and 7 are unused here; those exist in
  Radarr). The brief's "QualityModifier"/"Year" items do not apply.

### 8.4 Scoring and use

* `RemoteEpisode.CustomFormatScore = profile.CalculateCustomFormatScore(CustomFormats)` = Σ score of the profile's
  `FormatItems` whose format matched (formats not present in the profile score 0).
* Profile thresholds: `MinFormatScore` (reject below, §9 CustomFormatAllowedByProfile), `CutoffFormatScore` (stop
  upgrading once reached), `MinUpgradeFormatScore` (minimum increment for a CF-only upgrade; default 1).
* `{Custom Formats}` naming token uses formats with `IncludeCustomFormatWhenRenaming = true` (§10).

---

## 9. Decision engine (`NzbDrone.Core/DecisionEngine/`)

### 9.1 Evaluation model

* Spec interface (`Specifications/IDownloadDecisionEngineSpecification.cs`): `RejectionType Type`, `SpecificationPriority Priority`,
  `DownloadSpecDecision IsSatisfiedBy(RemoteEpisode, ReleaseDecisionInformation{PushedRelease, SearchCriteria})`.
* `SpecificationPriority { Default = 0, Parsing = 0, Database = 0, Disk = 1 }` → effectively **two tiers**: everything
  except the two `Disk` specs runs first; **all** specs of a tier run (no short-circuit inside a tier) and if any rejects,
  later tiers are skipped (`DownloadDecisionMaker.GetDecisionForReport`, L199–216).
* A spec throwing → rejection `DecisionError` ("{SpecTypeName}: {message}"), Permanent.
* `DownloadDecision.Approved` = no rejections; `TemporarilyRejected` = ≥1 rejection and **all** Temporary;
  `Rejected` = any Permanent. Rejection = `{ Reason: DownloadRejectionReason, Message, Type }`.
* Order of rejections inside a tier = DI enumeration order of the spec classes (**UNVERIFIED**; only affects message order).
* `DownloadRejectionReason` enum (exact order, 0-based): Unknown, UnknownSeries, UnknownEpisode, MatchesAnotherSeries,
  UnableToParse, Error, DecisionError, MinimumAgeDelay, MinimumAgeDelayPushed, SeriesNotMonitored, EpisodeNotMonitored,
  HistoryRecentCutoffMet, HistoryCdhDisabledCutoffMet, HistoryHigherPreference, HistoryHigherRevision, HistoryCutoffMet,
  HistoryCustomFormatCutoffMet, HistoryCustomFormatScore, HistoryCustomFormatScoreIncrement, HistoryUpgradesNotAllowed,
  NoMatchingTag, PropersDisabled, ProperForOldFile, WrongEpisode, WrongSeason, WrongSeries, FullSeason, UnknownRuntime,
  BelowMinimumSize, AboveMaximumSize, AlreadyImportedSameHash, AlreadyImportedSameName, UnknownReleaseGroup,
  ReleaseGroupDoesNotMatch, IndexerDisabled, Blocklisted, CustomFormatMinimumScore, MinimumFreeSpace, FullSeasonNotAired,
  MaximumSizeExceeded, MinimumAge, MaximumAge, MultiSeason, Sample, ProtocolDisabled, QualityNotWanted,
  QualityUpgradesDisabled, QueueHigherPreference, QueueHigherRevision, QueueCutoffMet, QueueCustomFormatCutoffMet,
  QueueCustomFormatScore, QueueCustomFormatScoreIncrement, QueueUpgradesNotAllowed, QueuePropersDisabled, Raw,
  MustContainMissing, MustNotContainPresent, RepackDisabled, RepackUnknownReleaseGroup, RepackReleaseGroupDoesNotMatch,
  ExistingFileHasMoreEpisodes, AmbiguousNumbering, NotSeasonPack, SplitEpisode, MinimumSeeders, DiskHigherPreference,
  DiskHigherRevision, DiskCutoffMet, DiskCustomFormatCutoffMet, DiskCustomFormatScore, DiskCustomFormatScoreIncrement,
  DiskUpgradesNotAllowed, DiskNotUpgrade, BeforeAirDate.

Relevant config defaults (`Configuration/ConfigService.cs`): `DownloadPropersAndRepacks = PreferAndUpgrade`,
`SeasonPackUpgrade = All` (`SeasonPackUpgradeType { All=0, Threshold=1, Any=2 }`), `SeasonPackUpgradeThreshold = 100.0`,
`MinimumAge = 0` (minutes), `Retention = 0` (days, 0 = unlimited), `MaximumSize = 0` (MB, 0 = unlimited),
`EnableCompletedDownloadHandling = true`, `AutoUnmonitorPreviouslyDownloadedEpisodes = false`,
`SkipFreeSpaceCheckWhenGrabbing = false`, `MinimumFreeSpaceWhenImporting = 100` (MB), `RssSyncInterval = 15`.

### 9.2 Every specification (`DecisionEngine/Specifications/**`)

P = priority tier (0 or 1), T = rejection type (Perm/Temp).

| Spec class | P | T | Rule (reject when …) → reason |
|---|---|---|---|
| `AcceptableSizeSpecification` | 0 | Perm | see §9.3 → UnknownRuntime / BelowMinimumSize / AboveMaximumSize |
| `AirDateSpecification` (v5) | 0 | Perm | Enabled release profiles for series tags+indexer; pick the one ordered by `AirDateRestriction` desc then `AirDateGracePeriod` desc; if it restricts: any episode without `AirDateUtc` → BeforeAirDate ("No air date available"); `Release.PublishDate < AirDateUtc + grace hours` → BeforeAirDate |
| `AlreadyImportedSpecification` | 0 | Perm | Only if CDH enabled. For each episode **with a file**: last `Grabbed` history + a `DownloadFolderImported` with the same DownloadId whose quality **differs** from the grabbed quality → torrent with same InfoHash (upper-cased) as that DownloadId → AlreadyImportedSameHash; release title equals grabbed SourceTitle (invariant ignore-case) → AlreadyImportedSameName |
| `AnimeVersionUpgradeSpecification` | 0 | Perm | Anime series only and propers not `DoNotPrefer`: for each existing file where `IsRevisionUpgrade(file.Quality, new)`: file group blank → UnknownReleaseGroup ("Existing release group is unknown"); new group blank → UnknownReleaseGroup; groups differ (**case-sensitive** `!=`) → ReleaseGroupDoesNotMatch |
| `BlockedIndexerSpecification` | 0 | **Temp** | indexer currently disabled due to failures (blocked-indexer cache refreshed every 15 s) → IndexerDisabled |
| `BlocklistSpecification` | 0 | Perm | `BlocklistService.Blocklisted(seriesId, release)` → Blocklisted. Torrent: if InfoHash set → any blocklist row for the series with that hash where `SameTorrent`; else rows with the same **title** (protocol torrent) where `SameTorrent` (InfoHash equal ignore-case if release has one, else same indexer — blank blocklist indexer matches any). Usenet: same-title rows (protocol usenet) where indexer matches (blank = any) **and** `PublishedDate == release.PublishDate` (`Parser/ReleaseComparer.cs`). |
| `CustomFormatAllowedByProfileSpecification` | 0 | Perm | `CustomFormatScore < profile.MinFormatScore` → CustomFormatMinimumScore |
| `FreeSpaceSpecification` | **1** | Perm | unless `SkipFreeSpaceCheckWhenGrabbing`; free space of series path (cached 5 s; missing dir → accept): `free - size <= 0` → MinimumFreeSpace; `< MinimumFreeSpaceWhenImporting MB` → MinimumFreeSpace |
| `FullSeasonSpecification` | 0 | Perm | `FullSeason` and not allowed by release profile (`AllowSeasonPackWithoutAllEpisodesAired`; with several profiles the one ordered `false` first wins, i.e. most restrictive) and any episode has no air date or airs after `UtcNow + 24h` → FullSeasonNotAired |
| `MaximumSizeSpecification` | 0 | Perm | `MaximumSize` MB ≠ 0, size ≠ 0, size > max → MaximumSizeExceeded |
| `MinimumAgeSpecification` | 0 | **Temp** | Usenet only, `MinimumAge` ≠ 0, `AgeMinutes < MinimumAge` → MinimumAge |
| `MultiSeasonSpecification` | 0 | Perm | `ParsedEpisodeInfo.IsMultiSeason` → MultiSeason (always; only interactive grab overrides) |
| `NotSampleSpecification` | 0 | Perm | title (lower) contains `sample` and size < 70 MiB → Sample |
| `ProtocolSpecification` | 0 | Perm | delay profile (BestForTags) disables Usenet/Torrent for that protocol → ProtocolDisabled |
| `QualityAllowedByProfileSpecification` | 0 | Perm | `profile.Items[GetIndex(quality).Index].Allowed == false` (group's Allowed for grouped qualities; unknown quality → item 0 = Unknown) → QualityNotWanted |
| `QueueSpecification` | 0 | Perm | for each queue item of the same series sharing an episode (skip `FailedPending`): CFs of queued item recomputed; `!CutoffNotMet(profile, queued.Quality, queuedCFs, new.Quality)` → QueueCutoffMet; then `IsUpgradable(...)` reason mapping → QueueHigherPreference / QueueHigherRevision / QueueCutoffMet / QueueCustomFormatCutoffMet / QueueCustomFormatScore / QueueCustomFormatScoreIncrement / QueueUpgradesNotAllowed; then revision upgrade with `DoNotUpgrade` → QueuePropersDisabled |
| `RawDiskSpecification` | 0 | Perm | release title matches a DiscRegex (below) → Raw ("Raw Bluray/DVD release"); `Release.Container` (lower) in `{vob, iso}` → Raw DVD; `{m2ts}` → Raw Bluray |
| `ReleaseRestrictionsSpecification` | 0 | Perm | for each enabled profile with Required terms: none matches title → MustContainMissing; for each with Ignored terms: any matches → MustNotContainPresent (term matching §7.6; against **Release.Title**) |
| `RepackSpecification` | 0 (Database) | Perm | release `IsRepack` and propers not DoNotPrefer: for each file with `IsRevisionUpgrade(file, new)`: `DoNotUpgrade` → RepackDisabled; file group blank / release group blank → RepackUnknownReleaseGroup; groups differ (invariant ignore-case) → RepackReleaseGroupDoesNotMatch |
| `RetentionSpecification` | 0 | Perm | Usenet only; `Retention > 0 && Age(days) > Retention` → MaximumAge |
| `SameEpisodesGrabSpecification` | 0 | Perm | any existing episode file (of the release's episodes) also contains an episode **not** in the release → ExistingFileHasMoreEpisodes |
| `SceneMappingSpecification` | 0 | **Temp** | scene mapping `SceneOrigin` split on `:`; first part `mixed` → AmbiguousNumbering (message literally `"{0} has ambiguous numbering"` when a Comment exists, else "Ambiguous numbering"); `unknown` → accept (log only) |
| `SeasonPackOnlySpecification` | 0 | Perm | search with >1 episode, `Release.SeasonSearchMaximumSingleEpisodeAge > 0`, Standard series, not FullSeason, ≥1 episode: the newest aired search episode of that season aired before `UtcNow - N days` → NotSeasonPack |
| `SplitEpisodeSpecification` | 0 | Perm | `IsSplitEpisode` → SplitEpisode |
| `TorrentSeedingSpecification` | 0 | Perm | torrent with IndexerId ≠ 0, indexer settings `MinimumSeeders`, `Seeders` known and `< MinimumSeeders` → MinimumSeeders |
| `UpgradeAllowedSpecification` | 0 | Perm | for each existing file: `!IsUpgradeAllowed(profile, file.Quality, fileCFs, new.Quality, newCFs)` → QualityUpgradesDisabled |
| `UpgradeDiskSpecification` | 0 | Perm | §9.5 (DiskCutoffMet, DiskHigherPreference, DiskHigherRevision, DiskCustomFormatCutoffMet, DiskCustomFormatScore, DiskCustomFormatScoreIncrement, DiskUpgradesNotAllowed, DiskNotUpgrade) |
| `RssSync/DelaySpecification` | 0 | **Temp** | §9.6 → MinimumAgeDelay |
| `RssSync/DeletedEpisodeFileSpecification` | **1** | **Temp** | only if `AutoUnmonitorPreviouslyDownloadedEpisodes` and not a search: any episode file in DB missing on disk → EpisodeNotMonitored |
| `RssSync/HistorySpecification` | 0 | Perm | skipped in searches. For each episode whose **most recent** history event is `Grabbed`: if older than 12 h and CDH enabled → ignore; else (recent, or CDH disabled) with CFs of that history: `!CutoffNotMet` → HistoryRecentCutoffMet (recent) / HistoryCdhDisabledCutoffMet; then IsUpgradable reason → HistoryHigherPreference / HistoryHigherRevision / HistoryCutoffMet / HistoryCustomFormatCutoffMet / HistoryCustomFormatScore / HistoryCustomFormatScoreIncrement / HistoryUpgradesNotAllowed |
| `RssSync/IndexerTagSpecification` | 0 | Perm | indexer has tags and none intersect series tags → NoMatchingTag |
| `RssSync/MonitoredEpisodeSpecification` | 0 | Perm | unless search with `MonitoredEpisodesOnly == false`: series unmonitored → SeriesNotMonitored; any episode unmonitored → EpisodeNotMonitored ("Episode is not monitored" for single, "One or more episodes is not monitored" otherwise) |
| `RssSync/PendingSpecification` | 0 | **Temp** | not RSS and not user-invoked search (i.e. pushed releases / automatic search): a pending release of the same series shares an episode → MinimumAgeDelayPushed |
| `RssSync/ProperSpecification` | 0 | Perm | not in searches; propers not DoNotPrefer; for each file with `IsRevisionUpgrade`: `DoNotUpgrade` → PropersDisabled; `file.DateAdded < Today - 7 days` → ProperForOldFile |
| `Search/EpisodeRequestedSpecification` | 0 | Perm | search: no overlap between requested and mapped episodes → WrongEpisode ("Episode wasn't requested: SxE[-E]") |
| `Search/SeasonMatchSpecification` | 0 | Perm | `SeasonSearchCriteria`: SeasonNumbers non-empty ? contains search season : `MappedSeasonNumber == season`; else WrongSeason |
| `Search/SeriesSpecification` | 0 | Perm | search series id ≠ release series id → WrongSeries |
| `Search/SingleEpisodeSearchMatchSpecification` | 0 | Perm | `SingleEpisodeSearchCriteria`: season mismatch (as above) → WrongSeason; no episode numbers → FullSeason ("Full season pack"); episode not in EpisodeNumbers → WrongEpisode. `AnimeEpisodeSearchCriteria`: FullSeason && !IsSeasonSearch → FullSeason |

`RawDiskSpecification.DiscRegex` (verbatim, `RegexOptions.Compiled | RegexOptions.IgnoreCase`):

- L13 `(inline)`

```regex
(?:dis[ck])(?:[-_. ]\d+[-_. ])(?:(?:(?:480|720|1080|2160)[ip]|)[-_. ])?(?:Blu\-?ray)
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L14 `(inline)`

```regex
(?:(?:480|720|1080|2160)[ip]|)[-_. ](?:full)[-_. ](?:Blu\-?ray)
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L15 `(inline)`

```regex
(?:\d?x?M?DVD-?[R59])(?:[ ._]|$)
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

### 9.3 Size limits from runtime — `AcceptableSizeSpecification`

1. `ParsedEpisodeInfo.Special` → accept. `Release.Size == 0` → accept.
2. `seriesRuntime = Series.Runtime` (minutes). If 0: take the first season > 0 and its first episode (`GetEpisodesBySeason(...).First()`,
   "pilot"); if the release's first episode is in that season and **all** release episodes have no AirDateUtc or air before
   `(pilot.AirDateUtc ?? Release.PublishDate) + 24h` → `seriesRuntime = 45`.
3. `runtime = Σ over release episodes (episode.Runtime > 0 ? episode.Runtime : seriesRuntime)`; `runtime == 0` → reject
   UnknownRuntime ("Runtime of all episodes is 0, unable to validate size until it is available").
4. Limits come from the **profile item** for the release quality: `GetIndex(quality, respectGroupOrder: true)` → item or
   group member. `MinSize` set → `min = Convert.ToInt64(MinSize * 1024 * 1024) * runtime`; `size < min` → BelowMinimumSize.
   `MaxSize` set and ≠ 0 → `max = Convert.ToInt64(MaxSize * 1024 * 1024) * runtime`; `size > max` → AboveMaximumSize.
   (`Convert.ToInt64(double)` = round-half-to-even.) Messages: `"{size} is smaller than minimum allowed {min} (for {runtime}min)"`
   or `(for {n} episodes totalling {runtime}min)`; sizes printed with `SizeSuffix()`.
* Season packs are covered implicitly: `Episodes` = all mapped episodes of the season, so runtime sums them.

### 9.4 Upgrade / cutoff logic — `UpgradableSpecification` (`Specifications/UpgradableSpecification.cs`)

Comparisons use `QualityModelComparer(profile)` (profile item index, **group members are equal** — respectGroupOrder=false).

`IsUpgradable(profile, current, currentCFs, new, newCFs) → UpgradeableRejectReason { None=0, BetterQuality=1,
BetterRevision=2, QualityCutoff=3, CustomFormatScore=4, CustomFormatCutoff=5, MinCustomFormatScore=6, UpgradesNotAllowed=7 }`:
1. `qc = Compare(new.Quality, current.Quality)`.
2. `qc > 0 && QualityCutoffNotMet(profile, current, new)` → **None** (quality upgrade).
3. `qc < 0` → **BetterQuality**.
4. `rc = new.Revision.CompareTo(current.Revision)` (Real, then Version). If propers ≠ DoNotPrefer and `rc > 0` → **None**.
5. `!profile.UpgradeAllowed` → **UpgradesNotAllowed**.
6. propers ≠ DoNotPrefer and `rc < 0` → **BetterRevision**.
7. `qc > 0` (better quality but cutoff already met) → **QualityCutoff**.
8. `newScore <= currentScore` → **CustomFormatScore**.
9. `currentScore >= profile.CutoffFormatScore` → **CustomFormatCutoff**.
10. `newScore < currentScore + profile.MinUpgradeFormatScore` → **MinCustomFormatScore**.
11. → **None**.

`QualityCutoffNotMet(profile, current, new = null)`: `cutoff = UpgradeAllowed ? profile.Cutoff : FirststAllowedQuality().Id`;
true if `Compare(current.Quality.Id, cutoff) < 0` (ids resolved with `GetIndex`, so a group id cutoff works); else true if
`new != null && IsRevisionUpgrade(current, new)`; else false.
`CustomFormatCutoffNotMet(profile, currentCFs)` = `score(currentCFs) < (UpgradeAllowed ? CutoffFormatScore : MinFormatScore)`.
`CutoffNotMet(profile, current, currentCFs, new = null)` = QualityCutoffNotMet || CustomFormatCutoffNotMet.
`IsRevisionUpgrade(current, new)` = **same Quality (by Id)** and `new.Revision.CompareTo(current.Revision) > 0`
(so a PROPER WEBRip never "upgrades" a WEBDL of the same group/tier).
`IsUpgradeAllowed(profile, current, currentCFs, new, newCFs)`: revision upgrade → true; (quality upgrade via
`Compare(QualityModel…)` incl. revision, or CF score increase) → `profile.UpgradeAllowed`; otherwise → true.

### 9.5 Existing files — `UpgradeDiskSpecification`

* **Not FullSeason**: for each mapped episode with a file: `!CutoffNotMet(profile, file.Quality, CFs(file), new.Quality)` →
  DiskCutoffMet ("Existing file meets cutoff: {cutoff item}", cutoff = UpgradeAllowed ? Cutoff : first allowed); else
  `IsUpgradable(...)` reason mapping: BetterQuality → DiskHigherPreference, BetterRevision → DiskHigherRevision,
  QualityCutoff → DiskCutoffMet, CustomFormatCutoff → DiskCustomFormatCutoffMet, CustomFormatScore → DiskCustomFormatScore,
  MinCustomFormatScore → DiskCustomFormatScoreIncrement, UpgradesNotAllowed → DiskUpgradesNotAllowed; None → next file.
* **FullSeason (season packs)**: `upgraded = #episodes without file`; if no episode has a file → accept. For each existing
  file: if `CutoffNotMet(...)` and `IsUpgradable(...) == None` → `upgraded++`. `pct = upgraded / total * 100`.
  `SeasonPackUpgrade == Any` → accept if `upgraded > 0`; else threshold = (All ? 100.0 : SeasonPackUpgradeThreshold) and
  accept if `pct >= threshold`; otherwise reject DiskNotUpgrade ("Season pack does not meet the upgrade criteria. Upgradable:
  {u}/{t} ({pct:0.##}%), Mode: {mode}, Threshold: {threshold}%").

### 9.6 Delay profiles — `RssSync/DelaySpecification` (Temporary)

1. User-invoked search → accept.
2. `delayProfile = BestForTags(series.Tags)`; `delay = GetProtocolDelay(protocol)` (minutes); `delay == 0` → accept.
3. If release protocol is the preferred one and propers = PreferAndUpgrade: any existing file with same quality index
   (`Compare(new.Quality, file.Quality) == 0`) and better revision → accept (propers skip delay).
4. `BypassIfHighestQuality` and `Compare(new.Quality, profile.LastAllowedQuality()) >= 0` and preferred protocol → accept.
5. `BypassIfAboveCustomFormatScore` and `CustomFormatScore >= MinimumCustomFormatScore` and preferred protocol → accept.
6. `oldest = PendingReleaseService.OldestPendingRelease(seriesId, episodeIds)`; if `oldest.Release.AgeMinutes > delay` → accept.
7. `Release.AgeMinutes < delay` → reject MinimumAgeDelay ("Waiting for better quality release"); else accept.

### 9.7 Propers / repacks (`ProperDownloadTypes`)

* `PreferAndUpgrade` (default): revision (Real, then Version) participates in ranking (§9.8), in `IsUpgradable` (rc>0 →
  upgrade even if cutoff met), ProperSpecification allows revision upgrades only for files added in the last 7 days
  (`file.DateAdded >= Today - 7 days`) during RSS; RepackSpecification requires same release group (case-insensitive);
  AnimeVersionUpgradeSpecification requires same release group (case-sensitive) for anime.
* `DoNotUpgrade`: revision upgrades are rejected (PropersDisabled / RepackDisabled / QueuePropersDisabled) but revisions still
  rank higher in the comparer.
* `DoNotPrefer`: revisions ignored in the comparer and in IsUpgradable; Proper/Repack/AnimeVersion specs accept.
  (`QualityModelComparer` itself always compares Revision as tie-break — used by IsUpgradeAllowed and import.)

### 9.8 Prioritisation — `DownloadDecisionComparer` (exact chain, `DecisionEngine/DownloadDecisionComparer.cs`)

`DownloadDecisionPriorizationService.PrioritizeDecisions`: decisions with a Series are grouped by `Series.Id` (groups in first-
appearance order), each group sorted **descending** with the comparer (stable `OrderByDescending`), concatenated, then
decisions without series appended (`Union`). Comparer = first non-zero of (positive = x better):
1. **CompareQuality**: `profile.GetIndex(quality)` (index only); if propers ≠ DoNotPrefer, tie → `Revision.CompareTo` (Real, then Version).
2. **CompareCustomFormatScore**: higher `CustomFormatScore` wins.
3. **CompareProtocol**: `Release.DownloadProtocol == delayProfile.PreferredProtocol` (true > false).
4. **CompareEpisodeCount**: `FullSeason` true > false; else if **both** series are Anime → **more** episodes wins; else **fewer**
   episodes wins.
5. **CompareEpisodeNumber**: lower minimum episode number wins (`MinOrDefault` → 0 when no episodes).
6. **CompareIndexerPriority**: lower `Release.IndexerPriority` value wins.
7. **ComparePeersIfTorrent** (only if both torrent): `round(log10(seeders))` (0 if null/≤0) higher wins, then same for peers
   (`Math.Round` = banker's rounding).
8. **CompareAgeIfUsenet** (only if both usenet): score = `AgeHours < 1 → 1000; AgeHours <= 24 → 100; Age(days) <= 7 → 10;
   else -round(log10(Age days))`; higher wins.
9. **CompareSize**: profile item (group-aware) `PreferredSize`; `runtime = episodes.Count == 0 ? Series.Runtime :
   Σ(ep.Runtime > 0 ? ep.Runtime : Series.Runtime)`. If PreferredSize set and runtime > 0: value =
   `-|Round(size - runtime * PreferredSize MB, 200 MiB)|` (closest to preferred wins); else value = `Round(size, 200 MiB)`
   (bigger wins). `Round(n, level)` truncates toward zero to a multiple of `level` (`NzbDrone.Core/Fluent.cs`).

### 9.9 Grabbing order — `Download/ProcessDownloadDecisions.cs`

Qualified = `Approved || TemporarilyRejected`, with ≥1 episode. After prioritisation, iterate: skip if any of its episodes was
already grabbed in this batch; temporarily rejected → pending (reason Delay, or Fallback if episodes already grabbed/pending);
if the protocol's client already failed in this batch → pending (DownloadClientUnavailable); else attempt grab
(Grabbed / Pending / Rejected / Failed → marks protocol failed and pends / Skipped).

### 9.10 Season pack vs episodes — summary of all rules

* Parsing: `FullSeason` from season-only patterns (§1.6); multi-season ranges; `IsSeasonExtra` (EXTRAS/SUBPACK) is still
  FullSeason; `IsPartialSeason` is **not** FullSeason (maps through standard path with no episodes → UnknownEpisode).
* Mapping: FullSeason → all episodes of the (scene-)season; multi-season → union of seasons (§5.6).
* Specs: MultiSeason always rejected; FullSeasonNotAired unless release profile allows; SeasonPackOnly (search setting);
  SingleEpisodeSearchMatch rejects packs for single-episode searches; UpgradeDisk threshold logic; AcceptableSize sums runtime.
* Ranking: season packs beat episode releases at equal quality/CF score/protocol (CompareEpisodeCount); among non-anime
  episode releases fewer episodes wins, among anime more episodes wins.

---

## 10. File and folder naming (`NzbDrone.Core/Organizer/`)

### 10.1 Defaults (`NamingConfig.Default`, inserted on first `GetConfig()`)

| Setting | Default |
|---|---|
| `RenameEpisodes` | **false** (files keep original name: `GetOriginalTitle(file, true) + extension`) |
| `ReplaceIllegalCharacters` | true |
| `ColonReplacementFormat` | `Smart` (4) |
| `CustomColonReplacementFormat` | `""` |
| `MultiEpisodeStyle` | `PrefixedRange` (5) |
| `StandardEpisodeFormat` | `{Series Title} - S{season:00}E{episode:00} - {Episode Title} {Quality Full}` |
| `DailyEpisodeFormat` | `{Series Title} - {Air-Date} - {Episode Title} {Quality Full}` |
| `AnimeEpisodeFormat` | `{Series Title} - S{season:00}E{episode:00} - {Episode Title} {Quality Full}` |
| `SeriesFolderFormat` | `{Series Title}` |
| `SeasonFolderFormat` | `Season {season}` |
| `SpecialsFolderFormat` | `Specials` |

Enums: `MultiEpisodeStyle { Extend=0, Duplicate=1, Repeat=2, Scene=3, Range=4, PrefixedRange=5 }`;
`ColonReplacementFormat { Delete=0, Dash=1, SpaceDash=2, SpaceDashSpace=3, Smart=4, Custom=5 }`.

### 10.2 Regexes (verbatim, `Organizer/FileNameBuilder.cs`, `FileNameValidation.cs`, `FileNameBuilderTokenEqualityComparer.cs`)

Note `ScenifyRemoveChars` contains **unescaped `$` and `^`** in the first alternation — they are anchors (effectively dead
alternatives), not literal `$`/`^`; reproduce as-is for parity.

- L49 `TitleRegex`

```regex
(?<escaped>\{\{|\}\})|\{(?<prefix>[- ._\[(]*)(?<token>(?:[a-z0-9]+)(?:(?<separator>[- ._]+)(?:[a-z0-9]+))?)(?::(?<customFormat>[ ,a-z0-9+-]+(?<![- ])))?(?<suffix>[- ._)\]]*)\}
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase | RegexOptions.CultureInvariant`

- L52 `EpisodeRegex`

```regex
(?<episode>\{episode(?:\:0+)?})
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L55 `SeasonRegex`

```regex
(?<season>\{season(?:\:0+)?})
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L58 `AbsoluteEpisodeRegex`

```regex
(?<absolute>\{absolute(?:\:0+)?})
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L61 `SeasonEpisodePatternRegex`

```regex
(?<separator>(?<=})[- ._]+?)?(?<seasonEpisode>s?{season(?:\:0+)?}(?<episodeSeparator>[- ._]?[ex])(?<episode>{episode(?:\:0+)?}))(?<separator>[- ._]+?(?={))?
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L64 `AbsoluteEpisodePatternRegex`

```regex
(?<separator>(?<=})[- ._]+?)?(?<absolute>{absolute(?:\:0+)?})(?<separator>[- ._]+?(?={))?
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L67 `AirDateRegex`

```regex
\{Air(\s|\W|_)Date\}
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L69 `SeriesTitleRegex`

```regex
(?<token>\{(?:Series)(?<separator>[- ._])(Clean)?Title(The)?(Without)?(Year)?(?::(?<customFormat>[0-9-]+))?\})
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L72 `FileNameCleanupRegex`

```regex
([- ._])(\1)+
```
  args: `RegexOptions.Compiled`

- L73 `TrimSeparatorsRegex`

```regex
[- ._]+$
```
  args: `RegexOptions.Compiled`

- L75 `ScenifyRemoveChars`

```regex
(?<=\s)(,|<|>|\/|\\|;|:|'|"|\||`|’|~|!|\?|@|$|%|^|\*|-|_|=){1}(?=\s)|('|`|’|:|\?|,)(?=(?:(?:s|m|t|ve|ll|d|re)\s)|\s|$)|(\(|\)|\[|\]|\{|\})
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L76 `ScenifyReplaceChars`

```regex
[\/]
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L79 `MultiPartCleanupRegex`

```regex
(?:\:?\s?(?:\(\d+\)|(Part|Pt\.?)\s?\d+))$
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L83 `TitlePrefixRegex`

```regex
^(The|An|A) (.*?)((?: *\([^)]+\))*)$
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L85 `YearRegex`

```regex
\(\d{4}\)$
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L87 `ReservedDeviceNamesRegex`

```regex
^(?:aux|com[1-9]|con|lpt[1-9]|nul|prn)\.
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L14 `SeasonFolderRegex`

```regex
(\{season(\:\d+)?\})
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L17 `OriginalTokenRegex`

```regex
(\{original[- ._](?:title|filename)\})
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

- L10 `SimpleTokenRegex`

```regex
\s|_|\W
```
  args: `RegexOptions.Compiled | RegexOptions.IgnoreCase`

### 10.3 Token syntax and replacement rules (`TitleRegex`, `ReplaceToken`, L850–927)

* A token is `{` + optional **prefix** chars `[- ._\[(]*` + **token** (`word` or `word<sep>word`, sep `[- ._]+`) + optional
  `:customFormat` (`[ ,a-z0-9+-]+`, not ending in space/dash) + optional **suffix** chars `[- ._)\]]*` + `}`. `{{`/`}}` are escapes.
* Token lookup uses `FileNameBuilderTokenEqualityComparer`: strip `\s|_|\W` and lowercase — so `{Series Title}`,
  `{Series.Title}`, `{series_title}`, `{SERIES-TITLE}` and `{Air-Date}`≡`{Air Date}` all hit the same handler. Unknown
  tokens → `""`.
* Handler returns `null` → original token text is kept (used for the episode-title placeholder pass).
* Replacement is `Trim()`med; if all letters in the token text are lower-case → replacement `ToLower()`; if all upper-case →
  `ToUpper()`; mixed → unchanged. If the token contains a separator (`.`, `_`, `-`, ` ` between words) → every space in the
  replacement becomes that separator (e.g. `{Series.Title}` → `The.Series.Title`).
* Then `CleanFileName(replacement, namingConfig)` (§10.6). Prefix/suffix are only added when the replacement is non-blank.
  In the first (escaped) pass `{`/`}` inside replacement text are doubled.
* `customFormat` meaning per token: numeric format for `{season:00}`, `{episode:00}`, `{absolute:000}`; max length for title
  tokens (`{Series Title:30}`; **negative** = keep the end: `{Series Title:-30}` → `{ellipsis}` + last chars); filters for
  `{Custom Formats:...}` and `{MediaInfo AudioLanguages:...}`; the exact name for `{Custom Format:Name}`.

### 10.4 All tokens

| Token(s) | Value |
|---|---|
| `{Series Title}` | series title (or folder title arg) truncated per `:N` |
| `{Series CleanTitle}` | `CleanTitle(title)` |
| `{Series TitleYear}` / `{Series CleanTitleYear}` | `TitleYear(title, year)` = `"{title} ({year})"` unless year 0 or title already ends `(dddd)`; Clean variant = CleanTitle of that |
| `{Series TitleWithoutYear}` / `{Series CleanTitleWithoutYear}` | remove trailing `(dddd)` |
| `{Series TitleThe}` / `{Series CleanTitleThe}` | `TitlePrefixRegex.Replace(title, "$2, $1$3")` ("The Series (US)" → "Series, The (US)"); Clean: `CleanTitle(rest).Trim() + ", " + article + CleanTitle(suffix)` |
| `{Series TitleTheYear}` / `{Series CleanTitleTheYear}` | TitleYear(TitleThe(title)); Clean: `CleanTitleThe(title-without-(year)) + " " + year` (year from title's `(dddd)` if present) |
| `{Series TitleTheWithoutYear}` / `{Series CleanTitleTheWithoutYear}` | as named |
| `{Series TitleFirstCharacter}` | first char of `TitleThe(title)` if letter/digit, else second char, upper-cased, diacritics removed; else `_` |
| `{Series Year}` | `series.Year` |
| `{ImdbId}` `{TvdbId}` `{TvMazeId}` `{TmdbId}` | ids (`TvMazeId`/`TmdbId` empty when 0; `ImdbId` empty when null) |
| `{season}` / `{season:00}` | season number (first episode's season; `ReplaceNumberToken`: no format → `"0"` format) |
| `{episode}` / `{episode:00}` (outside an SxE pattern) | first episode, or `"{first}-{last}"` when multi-episode |
| `S{season:00}E{episode:00}`, `{season}x{episode:00}`, … | `SeasonEpisodePatternRegex` sub-pattern → expanded per MultiEpisodeStyle (§10.5) |
| `{absolute}` / `{absolute:000}` | anime only and **all** episodes have AbsoluteEpisodeNumber, else the absolute sub-pattern (incl. its separators) is **removed** |
| `{Air Date}` / `{Air-Date}` / `{Air.Date}` / `{Air_Date}` | first episode's `AirDate` (`yyyy-MM-dd`) with `-`→space then separator re-applied; `"Unknown"` if none |
| `{Episode Title}` | titles joined by `" + "` (multi: trailing ` .?` trimmed, `MultiPartCleanupRegex` removes `(1)`/`Part 1` suffixes, distinct; if all become blank use raw titles), byte-length-limited (§10.7) |
| `{Episode CleanTitle}` | as above with `CleanTitle` on each and separator `" and "` |
| `{Quality Full}` | `"{QualityDefinition.Title} {Proper} {Real}"` (then trimmed; double spaces collapsed later) |
| `{Quality Title}` | `QualityDefinition.Title` (user-editable title, default = quality name) |
| `{Quality Proper}` | Version > 1: anime → `v{Version}`, else `Proper`; else `""` |
| `{Quality Real}` | Real > 0 → `REAL` |
| `{Release Group}` | `EpisodeFile.ReleaseGroup` (truncatable); blank → `"Sonarr"` **only if** the token has no prefix/suffix, else `""` |
| `{Release Hash}` | `EpisodeFile.ReleaseHash` or `""` |
| `{Original Title}` | `CleanFileName(SceneName)` or, if blank, cleaned original file name |
| `{Original Filename}` | file name w/o extension of RelativePath (or Path); `""` if the pattern has an episode identifier and the file already exists (`useCurrentFilenameAsFallback = !patternHasEpisodeIdentifier || file.Id == 0`) |
| `{Custom Formats}` / `{Custom Formats:A,B}` / `{Custom Formats:-A,B}` | names of matched formats with `IncludeCustomFormatWhenRenaming`, space-joined; filter include list or `-`exclude list (comma-separated exact names) |
| `{Custom Format:Name}` | that format's name if matched and renaming-enabled, else `""`; no name → `""` |
| `{MediaInfo Video}` = `{MediaInfo VideoCodec}` | `MediaInfoFormatter.FormatVideoCodec(mediaInfo, sceneName)` |
| `{MediaInfo VideoBitDepth}` | bit depth or `8` |
| `{MediaInfo VideoDynamicRange}` / `{MediaInfo VideoDynamicRangeType}` | `FormatVideoDynamicRange` / `FormatVideoDynamicRangeType` (trigger a MediaInfo rescan if schema revision < 5 / < 11) |
| `{MediaInfo Audio}` = `{MediaInfo AudioCodec}` | `FormatAudioCodec(primaryAudioStream, sceneName)` |
| `{MediaInfo AudioChannels}` | channels formatted `F1` invariant (e.g. `5.1`), `""` if 0 |
| `{MediaInfo AudioLanguages}` | `[EN+DE]` style; **skips** when the only language is EN; filter `:EN+DE` (intersect, `+` separated), `:-EN` (except, `-` separated), trailing `+` adds `--` when filtered |
| `{MediaInfo AudioLanguagesAll}` | as above but keeps EN-only |
| `{MediaInfo SubtitleLanguages}` / `{MediaInfo SubtitleLanguagesAll}` | subtitle languages (EN-only kept in both) |
| `{MediaInfo Simple}` | `"{video} {audio}"` |
| `{MediaInfo Full}` | `"{video} {audio}{AudioLanguages} {SubtitleLanguages}"` |
| `{Season Title}` / `{Season CleanTitle}` | season folder formats only: `Series.Seasons[n].Title` |

Language tokens: values are MediaInfo stream language codes; `und`/blank dropped; each code lower-cased, mapped B→T via
`Iso639BTMap`, then `new CultureInfo(code).TwoLetterISOLanguageName.ToUpper()` (failures keep the raw code) → distinct.
MediaInfo codec/HDR formatting lives in `MediaFiles/MediaInfo/MediaInfoFormatter.cs` (**not audited here**).

### 10.5 `MultiEpisodeStyle` (`AddSeasonEpisodeNumberingTokens`, `AddAbsoluteNumberingTokens`)

`SeasonEpisodePatternRegex` finds `[s]{season[:0+]}<sep>[ex]{episode[:0+]}` (+ surrounding separators); for each distinct
pattern P (with `EpisodeSeparator` = `[- ._]?[ex]`, `EpisodePattern` = `{episode:..}`, `Separator` = last captured
surrounding separator), the first episode uses P and each following episode appends:

| Style | Appended per extra episode | Example (`S{season:00}E{episode:00}`, eps 6–8 / 6–7) | Anime absolute (`{absolute:000}`, 100–102) |
|---|---|---|---|
| Extend (0) | `-` + `{episode}` | `S15E06-07-08` (test: 6–7 → `S15E06-07`) | `100-101-102` |
| Duplicate (1) | Separator + full P | `S15E06 - S15E07` | Separator + P → `100 - 101 - 102` |
| Repeat (2) | EpisodeSeparator + `{episode}` | `S15E06E07` | sep (trimmed Separator or space) → `100-101-102` |
| Scene (3) | `-` + EpisodeSeparator + `{episode}` | `S15E06-E07` | `100-101-102` |
| Range (4) | first + `-{episode}` of **last** only | `S15E06-08` | `100-102` |
| PrefixedRange (5) | first + `-` + EpisodeSeparator + last | `S15E06-E08`; `{season:0}x{episode:00}` → `15x06-x08` | `100-102` |

Single episode → just P (`S15E06`). Examples are from `NzbDrone.Core.Test/OrganizerTests/FileNameBuilderTests/MultiEpisodeFixture.cs`.
Season tokens inside the expanded pattern use the **first** episode's season. Absolute pattern separator defaults to `-`
when none captured.

### 10.6 Illegal characters and colon handling — `CleanFileName(name, namingConfig)` (L1173)

1. If `ReplaceIllegalCharacters`: colon handling — `Smart`: `": "` → `" - "`, then remaining `":"` → `"-"`; `Delete` → `""`;
   `Dash` → `"-"`; `SpaceDash` → `" -"`; `SpaceDashSpace` → `" - "`; `Custom` → `CustomColonReplacementFormat`
   (validated: may not contain `\ / < > ? * | "` or `:`). If **not** ReplaceIllegalCharacters: `":"` → `""`.
2. Replace in order `\`→`+`, `/`→`+`, `<`→``, `>`→``, `?`→`!`, `*`→`-`, `|`→``, `"`→`` (`BadCharacters`/`GoodCharacters`);
   when ReplaceIllegalCharacters is false all of them → `""`.
3. `TrimStart(' ', '.')`, `TrimEnd(' ')`.
`CleanFolderName(name)` = collapse runs of the same separator char (`([- ._])(\1)+` → that char) then `Trim(' ', '.')`.
`ReplaceReservedDeviceNames`: `^(aux|com[1-9]|con|lpt[1-9]|nul|prn)\.` (ci) → the `.` becomes `_`.
`CleanTitle(title)` (for *CleanTitle tokens): `&`→`and`; `/`→space; remove `ScenifyRemoveChars` matches (isolated
punctuation between spaces, apostrophes/colons/?/commas before s/m/t/ve/ll/d/re/space/end, and all brackets); `RemoveDiacritics`.
(e.g. "Joker: Folie à deux" → "Joker Folie a deux"; "Is this okay?" → "Is this okay"; "Florence + the Machine" unchanged.)

### 10.7 Build algorithm and truncation (`BuildFileName`, L137–225)

1. `RenameEpisodes == false` → original title + extension. Empty format for the series type → `NamingFormatException`.
2. Episodes ordered by (season, episode). Pattern: Standard; Daily series and first episode season > 0 → Daily format;
   Anime and (all episodes have absolute numbers **or** the anime format has no `{absolute}`) → Anime format.
3. Split pattern on `\` and `/` (empty parts dropped) → each part is one path component (sub-folders allowed).
4. Per component: SxE/absolute expansion → `...` → `{{ellipsis}}` → MediaInfo refresh if needed → token pass 1 (episode
   titles kept as placeholders, escape mode) → `Trim` → `maxSegment = min(MAX_NAME=255 (env MAX_NAME), maxPath)` minus the
   extension's UTF-8 byte count for the last component → `maxTitle = maxSegment − bytes(component without episode-title
   tokens)` → token pass 2 with episode titles limited to `maxTitle` bytes → `Trim` → collapse repeated separators
   (`FileNameCleanupRegex` → first char) → strip trailing `[- ._]+` → `{ellipsis}`→`...` → reserved device names.
5. Join components with the OS directory separator + extension.
* `maxPath`: `BuildFilePath` passes `MAX_PATH − bytes(seasonPath) − 1` (`MAX_PATH` = env `MAX_PATH` or 4096 on Linux /
  long-path Windows, 259 otherwise).
* Episode-title fitting (`GetEpisodeTitle`, byte-based, UTF-8): `:N` formatter further caps the length; joined titles fit →
  use; else (≥2 titles) `first + "{ellipsis}" + last` if `bytes(first)+bytes(last)+3 <= max`; else `first + "{ellipsis}"`
  if multi and `bytes(first)+3 <= max`; else single title if it fits; else `Truncate(first, max−3).TrimEnd(' ', '.') + "{ellipsis}"`.
  (Quirk: the length budget counts the still-escaped `{ellipsis}` as 10 bytes, not 3.)
* Generic `Truncate(input, ":N")` for title tokens: `N == 0` or `len <= |N|` (**char** length) → unchanged; `N > 0` →
  `input.Truncate(N−3)` (UTF-8 **byte** budget, surrogate-safe) `.TrimEnd(' ', '.') + "{ellipsis}"`; `N < 0` → reverse,
  truncate `|N|−3`, TrimEnd, reverse back, prefixed with `{ellipsis}`.

### 10.8 Folders

* `GetSeriesFolder(series[, title])`: Series + Id tokens only on `SeriesFolderFormat`; `CleanFolderName`; reserved names;
  `{ellipsis}` → `...`. (Must contain a series title token — validator uses `SeriesTitleRegex`.)
* `GetSeasonFolder(series, n)`: Series, Id, `{season}` (n) and `{Season Title}`/`{Season CleanTitle}` tokens on
  `SeasonFolderFormat`, or `SpecialsFolderFormat` when `n == 0`; `CleanFolderName` then `Trim(' ', '.', '-', '_')`; reserved
  names; ellipsis. `BuildSeasonPath` adds the season folder only when `series.SeasonFolder` is true, after `CleanFileName`.
* Validation (`FileNameValidation.cs`): Standard format must match `SeasonEpisodePatternRegex`, or both `SeasonRegex` and
  `EpisodeRegex`, or `{Original Title|Filename}`; Daily additionally accepts `AirDateRegex`; Anime additionally accepts
  `AbsoluteEpisodePatternRegex`; Season folder must contain `{season(:\d+)?}`; Specials folder must not match a filtered
  sub-folder pattern; custom colon replacement may not contain illegal chars.
* `RequiresEpisodeTitle(series, episodes)`: renaming on and the relevant format contains `{Episode Title}`/`{Episode CleanTitle}`
  (used by import to wait for titles). `RequiresAbsoluteEpisodeNumber()`: anime format contains `{absolute…}`.

---

## 11. Porting risks and .NET-semantics checklist (ranked)

1. **Capture collections (highest risk).** `ParseMatchCollection` needs, per match: first & last `episode` capture, first &
   last `absoluteepisode` capture and their **count** (fractional specials require exactly 1), **all** `season` captures
   (multi-season packs #18/#49: `S01 S03 S05` → [1,3,5], 2 distinct → range), `title` capture count/all captures
   (#44, #67, TitleComponentsRegex), and the **last** `title`/`subgroup`/`hash` value. Patterns using duplicate group names:
   #1–2 (backref), #15, #18, #24, #32, #41–47, #49, #51, #53, #60, #67, #72, #91, #97, #100, #109 (see §1.4 numbering);
   in addition, every pattern with a named group inside `(...)+`/`{2,}` (most of them) relies on per-iteration captures.
   No Rust engine (`regex`, `fancy-regex`, `pcre2`) exposes capture stacks. Options: (a) mechanically rewrite each pattern so
   the first iteration has its own group (`(?:X(?<ep_first>..))(?:X(?<ep>..))*` → first/last) and re-scan the matched span
   with a sub-pattern when *all* captures are needed (seasons, titles); (b) embed a .NET-compatible backtracking engine.
   Either way, drive it with the 1,654-row corpus.
2. **Variable-length lookbehind.** Safe rewrites: `(?<!\d+)` ≡ `(?<!\d)`; `(?<!\d{1,2}-)` ≡ `(?<!\d-)`;
   `(?<!\d+|\(|\[|e|x)` ≡ `(?<![\d(\[ex])` (IgnoreCase; #90); `(?<![()\[!]|\d{1,2}-)` ≡ `(?<![()\[!])(?<!\d-)` (#83);
   `(?<!WEB[-_. ]?)` ≡ `(?<!WEB)(?<!WEB[-_. ])`; `(?<=\b\d+)%` ≡ forward `\b(\d+)%` → `${1}percent`.
   Genuinely unbounded (need code or an engine with unbounded lookbehind): `(?<!\d+\W+)` (#19/#50 partial-season `e`
   marker and #95 mini-series `e` marker), `(?<![-_. ]|\b[0]\d+)` (#15, #20, #22, #36) and `(?<!\b[0]\d+)` (#23, #24), and ReleaseGroupRegex's
   `(?<!(?:HDTV|…|\d{1,2}-bit|[ ._]\d{4}-\d{2}|-\d{2})(?:\k<part2>)?)` (**backreference inside a lookbehind**).
3. **Backreference** `\k<sep>` (daily #1/#2) and **atomic group** `(?>[-._ ])` (#96): need `fancy-regex`/`pcre2` (the atomic
   group around a single char class is equivalent to `[-._ ]`).
4. **`IgnorePatternWhitespace` SourceRegex** — use the collapsed form in §3.1; Rust `(?x)` drops spaces inside `[...]`.
5. **Index offset bug**: match indices on `simpleTitle` applied to `releaseTitle` for `ReleaseTokens` (§1.6) — affects
   `Languages` and the "Special" FullSeason override; the language tests encode this behaviour.
6. **Clock dependence**: daily future limit (`DateTime.Now.AddDays(14).Date`, local time), Proper 7-day window (`DateTime.Today`),
   history 12 h, FullSeason 24 h, release ages. Inject a clock.
7. **Culture/Unicode details**: `ToLower()`/`ToUpper()` without invariant culture in several places (LanguageParser,
   ValidateBeforeParsing, ScenifyRemoveChars consumers) — use invariant/Unicode-simple lowercasing; .NET `\w` =
   `[\p{L}\p{Mn}\p{Nd}\p{Pc}]` vs Rust's broader `\w` (adds `\p{Mc}`, `\p{Me}`, `\p{Nl}`, Join_Control) — affects `\b`,
   `\W`, NormalizeRegex on exotic scripts; `$` in .NET also matches before a final `\n`; `RemoveDiacritics` = Diacritical.Net
   table; `char.GetNumericValue` + `Normalize(FormKC)` in ParseNumber.
8. **Rounding**: `Convert.ToInt64(double)` and `Math.Round(double)` are round-half-to-even; `Fluent.Round(long, level)`
   truncates toward zero; size limits use MiB (1024²).
9. **Exceptions as control flow**: `InvalidDateException` stops the regex loop; any other exception inside ParseTitle →
   null; `MultipleSeriesFoundException`, `InvalidSceneMappingException`, daily `InvalidOperationException` bubble up to the
   decision maker as `Error` rejections.
10. **Ordering semantics**: LINQ `OrderBy`/`OrderByDescending` are stable; `GroupBy` keeps first-occurrence order;
    `HashSet` enumeration = insertion order (DefaultQualityDefinitions, IsoLanguages); `Distinct` keeps first occurrence.
11. **User-supplied regexes** (custom formats `Value`, release-profile `/regex/i` terms, scene-mapping `FilterRegex`) are .NET
    flavoured; compile them with the most .NET-compatible engine available and surface compile errors as validation errors.

## 12. Items marked UNVERIFIED / not audited

* Behaviour of Sonarr **v4** specifically (this tree is v5.0.0.2941): `SeasonNumbers` list vs v4 `SeasonNumber`,
  profile-item size limits vs v4 QualityDefinition sizes, `AirDateSpecification`/`ExcludedTags`/season-title fallback.
* DI enumeration order of decision specifications (only affects rejection message order).
* Equivalence of a Rust diacritics remover with Diacritical.Net 1.0.5.
* `MediaInfoFormatter` token formatting (video/audio codec names, HDR types) and import-time quality augmentation from
  MediaInfo (`AggregateQuality`, `AugmentQualityFromMediaInfo`) — out of scope, not audited.
* Indexer-side parsing (Newznab/Torznab attributes → `ReleaseInfo.Languages`, `IndexerFlags`, `TvdbId`) — not audited.
* Pending-release service, download-client selection and `ProcessDecisionInternal` details beyond §9.9 — not audited.
