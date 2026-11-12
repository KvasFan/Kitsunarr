//! Film release names: Tanukarr's `MovieReleaseParser`.
//!
//! A film release is `Title Year …technical tokens` (or carries one IMDb id); anything that
//! looks episodic (`S01E01`, `1x01`, `Season 2`, `CD1`) is not a film. A release matches a
//! library film when its IMDb id is the film's, or when its year and cleaned title (or an
//! alternate or original title) agree.

use std::sync::LazyLock;

use fancy_regex::Regex;
use kitsunarr_domain::language::Language;

use crate::quality::{ParsedQuality, parse_quality};
use crate::{clean_series_title, parse_languages, parse_path, parse_release_group};

static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?<![0-9])(?<year>(?:18|19|20)[0-9]{2})(?![0-9])").unwrap());
static IMDB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?<![A-Za-z0-9])(?<id>tt[0-9]{7,12})(?![0-9])").unwrap());
static EPISODIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:\bS[0-9]{1,3}[ ._-]*E[0-9]{1,4}\b|\b[0-9]{1,3}x[0-9]{2,4}\b|\bseasons?[ ._-]*[0-9]+|\b(?:cd|disc)[ ._-]*[0-9]+\b)",
    )
    .unwrap()
});
static TECHNICAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\A[ ._\[\](){}-]*(?:(?:[0-9]{3,4}p|bluray|blu-ray|bdrip|brrip|dvd|web(?:[ ._-]?dl|rip)?|hdtv|remux|uhd|x26[45]|h[ .]?26[45]|hevc|av1|aac|dts|ddp|proper|repack|multi|dual|imdb|tt[0-9]+)(?:\b|[ ._-]))",
    )
    .unwrap()
});

/// A parsed film release.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedMovie {
    pub release_title: String,
    pub title: String,
    /// 0 when the name has none (an IMDb-only name).
    pub year: i32,
    pub imdb_id: Option<String>,
    pub quality: ParsedQuality,
    pub languages: Vec<&'static Language>,
    pub release_group: Option<String>,
}

/// What a release is compared with.
#[derive(Debug, Clone, Default)]
pub struct MovieCandidate<'a> {
    pub title: &'a str,
    pub original_title: Option<&'a str>,
    pub alternate_titles: &'a [String],
    pub year: i32,
    pub imdb_id: Option<&'a str>,
    /// Owned by AniList or MyAnimeList: such films are also released as "episode 1".
    pub anime_catalogue: bool,
    /// The film's collection, whose packs also stand for it (Tanukarr's collection packs).
    pub collection: Option<&'a CollectionPack>,
}

/// A film collection for Tanukarr's `MovieCollectionPackParser`: one release holding every
/// film of the collection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CollectionPack {
    pub title: String,
    pub movies: Vec<PackMovie>,
}

/// A film of a [`CollectionPack`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackMovie {
    pub id: i64,
    pub year: i32,
    /// Minutes.
    pub runtime: i32,
}

impl CollectionPack {
    /// The films' runtimes together (`MovieCollectionRuntime`), for the size limits.
    pub fn runtime(&self) -> i32 {
        self.movies.iter().map(|m| m.runtime).sum()
    }

    /// Whether packs can be recognised: a title and 2 to 999 films, each with a year and a
    /// runtime of at most a week.
    pub fn is_valid(&self) -> bool {
        !self.title.trim().is_empty()
            && (2..=999).contains(&self.movies.len())
            && self
                .movies
                .iter()
                .all(|m| m.year > 0 && (1..=10080).contains(&m.runtime))
    }
}

static PACK_EPISODIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:S[0-9]+(?:[ ._-]*(?:E[0-9]+|-[ ._-]*S[0-9]+))?|seasons?[ ._-]*[0-9]+|specials?[ ._-]*[0-9]+)\b",
    )
    .unwrap()
});

/// `MovieCollectionPackParser.Match`: `{collection title} [complete] {first year}-{last year}`
/// with nothing episodic in it. The result names the collection and its first year.
pub fn match_collection_pack(release: &str, pack: &CollectionPack) -> Option<ParsedMovie> {
    if !pack.is_valid()
        || release.trim().is_empty()
        || release.len() > 2048
        || is_match(&PACK_EPISODIC, release)
    {
        return None;
    }
    let name = pack
        .title
        .split_whitespace()
        .map(|w| fancy_regex::escape(w).into_owned())
        .collect::<Vec<_>>()
        .join(r"[ ._-]+");
    let first = pack.movies.iter().map(|m| m.year).min()?;
    let last = pack.movies.iter().map(|m| m.year).max()?;
    let pattern = format!(
        r"(?i)\A{name}[ ._()\[\]-]+(?:complete[ ._-]+)?{first}[ ._-]+{last}(?=[ ._()\[\]-]|$)"
    );
    let regex = Regex::new(&pattern).ok()?;
    if !is_match(&regex, release.trim()) {
        return None;
    }
    Some(ParsedMovie {
        release_title: release.to_string(),
        title: pack.title.clone(),
        year: first,
        imdb_id: None,
        quality: parse_quality(release),
        languages: parse_languages(release),
        release_group: parse_release_group(release),
    })
}

fn is_match(r: &Regex, s: &str) -> bool {
    r.is_match(s).unwrap_or(false)
}

fn imdb_ids(name: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for m in IMDB.captures_iter(name).flatten() {
        if let Some(id) = m.name("id") {
            let id = id.as_str().to_ascii_lowercase();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

fn file_name(release: &str) -> String {
    let base = release
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(release)
        .to_string();
    match base.rsplit_once('.') {
        Some((stem, ext)) if crate::extensions::is_video_extension(&format!(".{ext}")) => {
            stem.to_string()
        }
        _ => base,
    }
}

fn build(release: &str, title: &str, year: i32, imdb: Option<String>) -> ParsedMovie {
    ParsedMovie {
        release_title: release.to_string(),
        title: title.to_string(),
        year,
        imdb_id: imdb,
        quality: parse_quality(release),
        languages: parse_languages(release),
        release_group: parse_release_group(release),
    }
}

/// `MovieReleaseParser.Parse`.
pub fn parse_movie(release: &str) -> Option<ParsedMovie> {
    if release.trim().is_empty() || release.len() > 2048 {
        return None;
    }
    let name = file_name(release);
    if is_match(&EPISODIC, &name) {
        return None;
    }
    let ids = imdb_ids(&name);
    if ids.len() > 1 {
        return None;
    }
    let max_year = chrono::Datelike::year(&chrono::Utc::now()) + 5;
    let trim_title: &[char] = &[' ', '.', '_', '-', '(', '[', '{'];
    let trim_suffix: &[char] = &[' ', '.', '_', '-', ')', ']', '}'];
    for m in YEAR.captures_iter(&name).flatten() {
        let Some(y) = m.name("year") else { continue };
        let year: i32 = y.as_str().parse().unwrap_or(0);
        let title = name[..y.start()].trim_matches(trim_title);
        let suffix = name[y.end()..].trim_matches(trim_suffix);
        if !(1878..=max_year).contains(&year)
            || title.is_empty()
            || (!suffix.is_empty() && !is_match(&TECHNICAL, suffix))
        {
            continue;
        }
        let title = title.replace(['.', '_'], " ");
        return Some(build(release, title.trim(), year, ids.into_iter().next()));
    }
    // An explicit IMDb file name is a strong identity, even without a title and year.
    match ids.as_slice() {
        [one] => Some(build(release, &name, 0, Some(one.clone()))),
        _ => None,
    }
}

fn matches_title(movie: &MovieCandidate<'_>, title: &str) -> bool {
    if title.trim().is_empty() {
        return false;
    }
    let wanted = clean_series_title(title);
    std::iter::once(movie.title)
        .chain(movie.original_title)
        .chain(movie.alternate_titles.iter().map(String::as_str))
        .filter(|t| !t.trim().is_empty())
        .any(|t| clean_series_title(t) == wanted)
}

/// `MovieReleaseParser.Match`: the release as a release of this film, or `None`.
pub fn match_movie(release: &str, movie: &MovieCandidate<'_>) -> Option<ParsedMovie> {
    if release.trim().is_empty() || release.len() > 2048 {
        return None;
    }
    let parsed = parse_movie(release);
    if parsed.is_none() && movie.anime_catalogue {
        // Anime catalogues represent a film as native episode 1; that release convention is
        // accepted for them only.
        let ids = imdb_ids(&file_name(release));
        let own = movie.imdb_id.map(str::to_ascii_lowercase);
        if ids.len() > 1 || (ids.len() == 1 && Some(&ids[0]) != own.as_ref()) {
            return None;
        }
        if let Some(native) = parse_path(release) {
            let episode_one = (native.season_numbers == [1] && native.episode_numbers == [1])
                || (native.is_absolute_numbering() && native.absolute_episode_numbers == [1]);
            let year = native.series_title_info.year;
            let title = if native.series_title_info.title_without_year.is_empty() {
                native.series_title.clone()
            } else {
                native.series_title_info.title_without_year.clone()
            };
            if !native.full_season
                && !native.is_partial_season
                && episode_one
                && (year <= 0 || movie.year <= 0 || year == movie.year)
                && matches_title(movie, &title)
            {
                return Some(build(
                    release,
                    movie.title,
                    movie.year,
                    ids.into_iter().next(),
                ));
            }
        }
    }
    let mut parsed = parsed?;
    if parsed.year > 0 && movie.year > 0 && parsed.year != movie.year {
        return None;
    }
    match &parsed.imdb_id {
        Some(id) => {
            if movie
                .imdb_id
                .is_none_or(|own| !own.eq_ignore_ascii_case(id))
            {
                return None;
            }
        }
        None => {
            if parsed.year == 0 || movie.year == 0 || !matches_title(movie, &parsed.title) {
                return None;
            }
        }
    }
    parsed.title = movie.title.to_string();
    parsed.year = movie.year;
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cases after Tanukarr's `MovieReleaseParserFixture`.
    #[test]
    fn film_names_parse() {
        let p = parse_movie("The.Matrix.1999.1080p.BluRay.x264-GRP").unwrap();
        assert_eq!((p.title.as_str(), p.year), ("The Matrix", 1999));
        assert_eq!(p.quality.quality.name, "Bluray-1080p");
        assert_eq!(p.release_group.as_deref(), Some("GRP"));
        // A year inside the title is skipped when what follows is not technical.
        let p = parse_movie("Blade.Runner.2049.2017.2160p.WEB-DL.HEVC-X").unwrap();
        assert_eq!((p.title.as_str(), p.year), ("Blade Runner 2049", 2017));
        assert!(parse_movie("Show.S01E01.1080p.WEB-DL-GRP").is_none());
        assert!(parse_movie("Movie.2001.CD1.avi").is_none());
        assert!(parse_movie("Some.Movie.1080p.BluRay").is_none());
        let p = parse_movie("tt0133093.mkv").unwrap();
        assert_eq!((p.year, p.imdb_id.as_deref()), (0, Some("tt0133093")));
        assert!(parse_movie("tt0133093.tt0234215.mkv").is_none());
    }

    #[test]
    fn films_match_by_title_and_year_or_imdb() {
        let alts = vec!["Matrix".to_string()];
        let movie = MovieCandidate {
            title: "The Matrix",
            alternate_titles: &alts,
            year: 1999,
            imdb_id: Some("tt0133093"),
            ..Default::default()
        };
        assert!(match_movie("The.Matrix.1999.1080p.BluRay.x264-GRP", &movie).is_some());
        assert!(match_movie("Matrix.1999.720p.WEB-DL-GRP", &movie).is_some());
        assert!(match_movie("The.Matrix.2003.1080p.BluRay-GRP", &movie).is_none());
        assert!(match_movie("The.Matrix.Reloaded.2003.1080p.BluRay-GRP", &movie).is_none());
        assert!(match_movie("Anything.tt0133093.1080p", &movie).is_some());
        assert!(match_movie("The.Matrix.1999.tt0234215.1080p", &movie).is_none());
        // Anime films released as episode 1.
        let anime = MovieCandidate {
            title: "Your Name",
            year: 2016,
            anime_catalogue: true,
            ..Default::default()
        };
        assert!(match_movie("[Group] Your Name - 01 [1080p].mkv", &anime).is_some());
        assert!(match_movie("[Group] Your Name - 02 [1080p].mkv", &anime).is_none());
    }

    /// Tanukarr's `MovieCollectionPackParserFixture` cases.
    #[test]
    fn collection_packs() {
        let pack = CollectionPack {
            title: "Evangelion Rebuild".into(),
            movies: vec![
                PackMovie {
                    id: 1,
                    year: 2007,
                    runtime: 98,
                },
                PackMovie {
                    id: 2,
                    year: 2009,
                    runtime: 108,
                },
                PackMovie {
                    id: 3,
                    year: 2012,
                    runtime: 96,
                },
                PackMovie {
                    id: 4,
                    year: 2021,
                    runtime: 155,
                },
            ],
        };
        assert_eq!(pack.runtime(), 457);
        for title in [
            "Evangelion.Rebuild.2007-2021.1080p.BluRay.x264-GRP",
            "Evangelion Rebuild (2007-2021) [1080p]",
            "Evangelion_Rebuild_Complete_2007_2021_BDRip",
        ] {
            let m = match_collection_pack(title, &pack).unwrap_or_else(|| panic!("{title}"));
            assert_eq!(m.year, 2007);
        }
        let m = match_collection_pack("Evangelion.Rebuild.2007-2021.1080p.BluRay.x264-GRP", &pack)
            .unwrap();
        assert_eq!(m.quality.quality.resolution, 1080);
        assert_eq!(m.release_group.as_deref(), Some("GRP"));
        for title in [
            "Evangelion.Rebuild.2007-2020.1080p",
            "Evangelion.Rebuild.S01.2007-2021.1080p",
            "Evangelion.Rebuild.Season.1.2007-2021",
            "Neon.Evangelion.Rebuild.2007-2021",
            "Evangelion.Rebuild.2007-20211",
            "Evangelion.Rebuild.1.11.2007.1080p",
        ] {
            assert!(match_collection_pack(title, &pack).is_none(), "{title}");
        }
        // A collection of one film, or a film without a year or runtime, has no packs.
        let mut one = pack.clone();
        one.movies.truncate(1);
        assert!(match_collection_pack("Evangelion.Rebuild.2007-2007", &one).is_none());
        let mut no_runtime = pack.clone();
        no_runtime.movies[0].runtime = 0;
        assert!(match_collection_pack("Evangelion.Rebuild.2007-2021", &no_runtime).is_none());
    }
}
