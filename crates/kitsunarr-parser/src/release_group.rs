//! Release group parsing (`Parser/ReleaseGroupParser.cs`).

use std::sync::LazyLock;

use crate::common::{CLEAN_TORRENT_SUFFIX, RegexReplace, WEBSITE_PREFIX, pre_substitute};
use crate::extensions::remove_file_extension;
use crate::re::{Re, first_of};

// `releasegroup` is used twice in the original. The backreference inside the variable-length
// lookbehind is supported by fancy-regex as in .NET.
static RELEASE_GROUP: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(
        r"-(?<releasegroup>[A-Za-zÀ-ÖØ-öø-ÿ0-9]+(?<part2>-[A-Za-zÀ-ÖØ-öø-ÿ0-9]+)?(?!.+?(?:HDTV|SDTV|480p|576p|720p|1080p|2160p)))(?<!(?:HDTV|SDTV|WEB-DL|Blu-Ray|480p|576p|720p|1080p|2160p|DTS-HD|DTS-X|DTS-MA|DTS-ES|-ES|-EN|-CAT|-GER|-FRA|-FRE|-ITA|\d{1,2}-bit|[ ._]\d{4}-\d{2}|-\d{2})(?:\k<part2>)?)(?:\b|[-._ ]|$)|[-._ ]\[(?<releasegroup__2>[A-Za-zÀ-ÖØ-öø-ÿ0-9]+)\]$",
    )
});
const RELEASE_GROUP_NAMES: &[&str] = &["releasegroup", "releasegroup__2"];

static INVALID_RELEASE_GROUP: LazyLock<Re> = LazyLock::new(|| Re::ci(r"^([se]\d+|[0-9a-f]{8})$"));

static ANIME_RELEASE_GROUP: LazyLock<Re> =
    LazyLock::new(|| Re::ci(r"^(?:\[(?<subgroup>(?!\s).+?(?<!\s))\](?:_|-|\s|\.)?)"));

// Release groups that don't follow -RlsGrp (manual list; matched by name only).
static EXCEPTION_RELEASE_GROUP_EXACT: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(
        r"(?:(?<releasegroup>Fight-BB|VARYG|E\.N\.D|KRaLiMaRKo|BluDragon|DarQ|KCRT|BEN[_. ]THE[_. ]MEN|TAoE|QxR|Vialle)\b)",
    )
});

// Groups whose releases end with RlsGroup) or RlsGroup]
static EXCEPTION_RELEASE_GROUP: LazyLock<Re> = LazyLock::new(|| {
    Re::ci(
        r"(?<=[._ \[])(?<releasegroup>(Joy|ImE|UTR|t3nzin|Anime Time|Project Angel|Hakata Ramen|HONE|Vyndros|SEV|Garshasp|Kappa|Natty|RCVR|SAMPA|YOGI|r00t|EDGE2020|Celdra)(?=\]|\)))",
    )
});

static CLEAN_RELEASE_GROUP: LazyLock<RegexReplace> = LazyLock::new(|| {
    RegexReplace::new(
        r"^(.*?[-._ ](S\d+E\d+)[-._ ])|(-(RP|1|NZBGeek|\[N-Z-B\]|Obfuscated|Scrambled|sample|Pre|postbot|xpost|Rakuv[a-z0-9]*|WhiteRev|BUYMORE|AsRequested|AlternativeToRequested|GEROV|Z0iDS3N|Chamele0n|4P|4Planet|AlteZachen|RePACKPOST))+$",
        "",
        true,
    )
});

/// `ReleaseGroupParser.ParseReleaseGroup`: the release group of a release or file name.
pub fn parse_release_group(title: &str) -> Option<String> {
    let mut title = remove_file_extension(title.trim());
    pre_substitute(&mut title);
    let title = CLEAN_TORRENT_SUFFIX.replace(&WEBSITE_PREFIX.replace(&title));

    if let Some(anime) = ANIME_RELEASE_GROUP.captures(&title) {
        return anime.name("subgroup").map(|m| m.as_str().to_string());
    }

    let title = CLEAN_RELEASE_GROUP.replace(&title);

    if let Some(last) = EXCEPTION_RELEASE_GROUP.captures_all(&title).last() {
        return last.name("releasegroup").map(|m| m.as_str().to_string());
    }
    if let Some(last) = EXCEPTION_RELEASE_GROUP_EXACT.captures_all(&title).last() {
        return last.name("releasegroup").map(|m| m.as_str().to_string());
    }
    let matches = RELEASE_GROUP.captures_all(&title);
    let last = matches.last()?;
    let group = first_of(last, RELEASE_GROUP_NAMES)
        .map(|m| m.as_str())
        .unwrap_or_default();
    // `int.TryParse` succeeds for plain (optionally signed) 32-bit integers.
    if group.parse::<i32>().is_ok() || INVALID_RELEASE_GROUP.is_match(group) {
        return None;
    }
    Some(group.to_string())
}
