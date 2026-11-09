Copyright (C)
2026 Kitsunarr contributors

Kitsunarr is a Rust implementation informed by a source-level study of Sonarr and Tanukarr. Portions of Kitsunarr port behaviour, data formats, parsing rules and test data from those projects, which are distributed under the GNU General Public License, version 3 or (at your option) any later version. Their notices are retained below.

Sonarr:
Copyright (C)
2014-2017 Mark McDowall, Keivan Beigi, Taloth Saldono and contributors
2010-2014 Mark McDowall, Keivan Beigi and contributors
See https://github.com/Sonarr/Sonarr and its Git history for all contributors.

Tanukarr (https://github.com/KvasFan/Tanukarr):
Retains the Sonarr notices above. Additional work is credited in its CONTRIBUTORS.md, including KakarottoCake, gaboy498-byte, ikatun (Sonarr-Pro forks) and the Tanukarr maintainers.

Test fixtures in `tests/fixtures/` were produced by running unmodified Sonarr 4.0.20.3014 (linuxserver.io image) and Tanukarr 0.1.5 (ghcr.io/kvasfan/tanukarr) with synthetic configuration. Titles and identifiers come from TheTVDB, AniList and TVmaze lookups performed by those applications; descriptive text was removed.

`crates/kitsunarr-parser` ports Sonarr's release parser (`src/NzbDrone.Core/Parser/`, revision `c018578e4`). Its regular expressions and decision rules are Sonarr's, adapted where .NET-only regex features required it. `tests/fixtures/parser/sonarr/` contains the test cases of Sonarr's parser test suite (`src/NzbDrone.Core.Test/ParserTests/`), extracted verbatim by `tests/fixtures/generate/sonarr_test_cases.py`.

`crates/kitsunarr-parser/src/diacritics.rs` is generated from Diacritical.Net 1.0.5 (https://github.com/anth12/diacritical-dotnet), used under the MIT License:

> Copyright (c) 2019 Anthony Halliday
>
> Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

The recorded TVmaze API responses in `tests/fixtures/metadata/` are TVmaze data, licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) by TVmaze.com; summaries were removed. At runtime, Kitsunarr credits TVmaze ("TV data provided by TVmaze.com under CC BY-SA 4.0") and AniList on the Add media page, where their data is searched.

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You can find a copy of the GNU General Public License in the LICENSE file and at <https://www.gnu.org/licenses/>.
