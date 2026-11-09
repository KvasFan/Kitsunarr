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

The recorded TVmaze API responses in `tests/fixtures/metadata/` are TVmaze data, licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) by TVmaze.com; summaries were removed. At runtime, Kitsunarr credits TVmaze ("TV data provided by TVmaze.com under CC BY-SA 4.0") and AniList on the Add media page, where their data is searched.

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You can find a copy of the GNU General Public License in the LICENSE file and at <https://www.gnu.org/licenses/>.
