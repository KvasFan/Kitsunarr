//! Search ranking for adding titles: Tanukarr's `SeriesTitleMatcher` and
//! `SearchSeriesComparer`, applied to TMDB (series and films), AniList and MyAnimeList search
//! results, whose own order buries an exact title under better-known entries of the franchise.
//!
//! Used only to rank lookup results, where a human picks from the list. It never decides which
//! series a release belongs to.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::SearchResult;

static PUNCTUATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{Nd}\s]").expect("valid regex"));
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid regex"));
static ARTICLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(a|an|the)\s+").expect("valid regex"));
/// The many ways a season is written, particularly for anime, reduced to "season N".
static SEASON_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:season\s*(?<a>\d+)|(?<b>\d+)(?:st|nd|rd|th)\s*season|s(?<c>\d{1,2})|part\s*(?<d>\d+)|cour\s*(?<e>\d+))\b",
    )
    .expect("valid regex")
});
static ROMAN_SEASON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s(?<r>i{1,3}|iv|v|vi{1,3}|ix|x)$").expect("valid regex"));

fn roman(value: &str) -> Option<u32> {
    Some(match value.to_ascii_lowercase().as_str() {
        "i" => 1,
        "ii" => 2,
        "iii" => 3,
        "iv" => 4,
        "v" => 5,
        "vi" => 6,
        "vii" => 7,
        "viii" => 8,
        "ix" => 9,
        "x" => 10,
        _ => return None,
    })
}

fn remove_diacritics(value: &str) -> String {
    value
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .nfc()
        .collect()
}

/// `Normalize`: lower case, accents stripped, punctuation removed, leading article dropped and
/// any season marker written as "season N".
pub fn normalize(title: &str) -> String {
    if title.trim().is_empty() {
        return String::new();
    }
    let value = remove_diacritics(&title.to_lowercase());
    let value = PUNCTUATION.replace_all(&value, " ");
    let mut value = WHITESPACE.replace_all(&value, " ").trim().to_string();
    // Before the numeric forms: "Ghost in the Shell II" ends in an isolated numeral.
    if let Some(m) = ROMAN_SEASON.captures(&value)
        && let Some(n) = roman(&m["r"])
    {
        let start = m.get(0).expect("whole match").start();
        value = format!("{} season {n}", &value[..start]);
    }
    let value = SEASON_SUFFIX.replace_all(&value, |c: &regex::Captures<'_>| {
        let n = ["a", "b", "c", "d", "e"]
            .iter()
            .find_map(|g| c.name(g))
            .map_or("", |m| m.as_str());
        format!("season {}", n.trim_start_matches('0'))
    });
    let value = ARTICLE.replace(&value, "");
    WHITESPACE.replace_all(&value, " ").trim().to_string()
}

/// `Score`: similarity of a query to a title, from 0 to 1.
pub fn score(query: &str, title: &str) -> f64 {
    let (a, b) = (normalize(query), normalize(title));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    // A query that is a whole prefix of the title (the first few words typed).
    if b.starts_with(&format!("{a} ")) {
        return 0.95;
    }
    jaro_winkler(&a, &b).max(token_set_ratio(&a, &b))
}

/// Proportion of query words found in the title, scaled by how much of the title is unused.
fn token_set_ratio(a: &str, b: &str) -> f64 {
    let query: Vec<&str> = a.split(' ').filter(|w| !w.is_empty()).collect();
    let title: HashSet<&str> = b.split(' ').filter(|w| !w.is_empty()).collect();
    if query.is_empty() {
        return 0.0;
    }
    let matched = query.iter().filter(|w| title.contains(*w)).count() as f64;
    let coverage = matched / query.len() as f64;
    let precision = matched / title.len().max(1) as f64;
    coverage * (0.7 + 0.3 * precision)
}

fn jaro_winkler(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let jaro = jaro(&a, &b);
    if jaro < 0.7 {
        return jaro;
    }
    let mut prefix = 0;
    while prefix < 4.min(a.len()).min(b.len()) && a[prefix] == b[prefix] {
        prefix += 1;
    }
    jaro + prefix as f64 * 0.1 * (1.0 - jaro)
}

fn jaro(a: &[char], b: &[char]) -> f64 {
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let (mut a_matched, mut b_matched) = (vec![false; a.len()], vec![false; b.len()]);
    let mut matches = 0usize;
    for i in 0..a.len() {
        let start = i.saturating_sub(window);
        let end = (i + window + 1).min(b.len());
        for j in start..end {
            if b_matched[j] || a[i] != b[j] {
                continue;
            }
            a_matched[i] = true;
            b_matched[j] = true;
            matches += 1;
            break;
        }
    }
    if matches == 0 {
        return 0.0;
    }
    let mut transpositions = 0.0;
    let mut k = 0;
    for i in 0..a.len() {
        if !a_matched[i] {
            continue;
        }
        while !b_matched[k] {
            k += 1;
        }
        if a[i] != b[k] {
            transpositions += 1.0;
        }
        k += 1;
    }
    transpositions /= 2.0;
    let m = matches as f64;
    (m / a.len() as f64 + m / b.len() as f64 + (m - transpositions) / m) / 3.0
}

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        previous = current;
    }
    previous[b.len()]
}

static CLEAN_PUNCTUATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[-._:]").expect("valid regex"));
static COUNTRY_YEAR_POSTFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\s\([A-Z]{2}\)|\s\(\d{4}\)|\s\([A-Z]{2}\)\s\(\d{4}\))$").expect("valid regex")
});
static CLEAN_ARTICLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(a|an|the)\s").expect("valid regex"));
static QUERY_YEAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?<query>.+)\s+(?:\((?<y1>\d{4})\)|(?<y2>\d{4}))$").expect("valid regex")
});

fn clean_punctuation(title: &str) -> String {
    CLEAN_PUNCTUATION.replace_all(title, "").to_lowercase()
}

fn clean_title(title: &str) -> String {
    let t = CLEAN_PUNCTUATION.replace_all(title, "");
    // `(?<=.+)`: never the whole title.
    let stripped = match COUNTRY_YEAR_POSTFIX.find(&t) {
        Some(m) if m.start() > 0 => format!("{}{}", &t[..m.start()], &t[m.end()..]),
        _ => t.to_string(),
    };
    stripped.to_lowercase()
}

fn clean_articles(title: &str) -> String {
    CLEAN_ARTICLE.replace(title, "").trim().to_lowercase()
}

/// `SearchSeriesComparer`.
pub struct SearchComparer {
    query: String,
    query_without_year: String,
    year: Option<i32>,
}

impl SearchComparer {
    pub fn new(query: &str) -> Self {
        let (query_without_year, year) = match QUERY_YEAR.captures(query) {
            Some(c) => (
                c["query"].to_lowercase(),
                c.name("y1")
                    .or_else(|| c.name("y2"))
                    .and_then(|y| y.as_str().parse().ok()),
            ),
            None => (query.to_lowercase(), None),
        };
        Self {
            query: query.to_string(),
            query_without_year,
            year,
        }
    }

    /// `Compare`: `Less` ranks `x` first.
    pub fn compare(&self, x: &SearchResult, y: &SearchResult) -> Ordering {
        // `true` first.
        let first = |f: &dyn Fn(&SearchResult) -> bool| f(y).cmp(&f(x));
        let exact = clean_punctuation(&self.query);
        let o = first(&|s| clean_punctuation(&s.title) == exact);
        if o.is_ne() {
            return o;
        }
        let articles = clean_articles(&self.query);
        let o = first(&|s| clean_articles(&s.title) == articles);
        if o.is_ne() {
            return o;
        }
        let o = first(&|s| levenshtein(&clean_punctuation(&s.title), &exact) <= 1);
        if o.is_ne() {
            return o;
        }
        // "Battlestar Galactica 1978".
        let o = self.compare_with_year(x, y, &|s| {
            levenshtein(&clean_title(&s.title), &self.query_without_year) <= 1
        });
        if o.is_ne() {
            return o;
        }
        // "CSI: ...".
        let prefix = format!("{}:", self.query_without_year);
        let o = self.compare_with_year(x, y, &|s| s.title.to_lowercase().starts_with(&prefix));
        if o.is_ne() {
            return o;
        }
        // The similarity score, against every name the catalogue knows.
        let key =
            |s: &SearchResult| self.best_title_score(s) + f64::from(self.year_factor(s)) / 100.0;
        let o = key(y).total_cmp(&key(x));
        if o.is_ne() {
            return o;
        }
        // A franchise's entries answer an abbreviation equally well: more votes first.
        y.votes.cmp(&x.votes)
    }

    fn compare_with_year(
        &self,
        x: &SearchResult,
        y: &SearchResult,
        can_match: &dyn Fn(&SearchResult) -> bool,
    ) -> Ordering {
        let (mx, my) = (can_match(x), can_match(y));
        if mx && my {
            if let Some(year) = self.year {
                let o = (y.year == Some(year)).cmp(&(x.year == Some(year)));
                if o.is_ne() {
                    return o;
                }
            }
            // Sonarr negates an ascending year comparison: the newer first.
            return y.year.unwrap_or(0).cmp(&x.year.unwrap_or(0));
        }
        my.cmp(&mx)
    }

    fn best_title_score(&self, s: &SearchResult) -> f64 {
        s.alternate_titles
            .iter()
            .map(|t| score(&self.query, t))
            .fold(score(&self.query, &s.title), f64::max)
    }

    fn year_factor(&self, s: &SearchResult) -> i32 {
        match (self.year, s.year) {
            (Some(wanted), Some(year)) if (year - wanted).abs() <= 1 => {
                20 - 10 * (year - wanted).abs()
            }
            _ => 0,
        }
    }

    /// Sorts search results best first.
    pub fn sort(&self, results: &mut [SearchResult]) {
        results.sort_by(|x, y| self.compare(x, y));
    }
}

/// Ranks catalogue search results for `query` (Tanukarr's `results.Sort(new
/// SearchSeriesComparer(title))`).
pub fn rank(query: &str, results: &mut [SearchResult]) {
    SearchComparer::new(query).sort(results);
}

#[cfg(test)]
mod tests {
    use super::*;
    use kitsunarr_domain::{MetadataProvider, SeriesStatus};

    fn result(title: &str, year: i32, votes: i64, alternates: &[&str]) -> SearchResult {
        SearchResult {
            provider: MetadataProvider::AniList,
            id: title.into(),
            kind: crate::EntryKind::Anime,
            title: title.into(),
            year: Some(year),
            overview: None,
            network: None,
            status: SeriesStatus::Ended,
            poster_url: None,
            external_ids: Vec::new(),
            alternate_titles: alternates.iter().map(|s| s.to_string()).collect(),
            votes,
        }
    }

    fn titles(query: &str, mut results: Vec<SearchResult>) -> Vec<String> {
        rank(query, &mut results);
        results.into_iter().map(|r| r.title).collect()
    }

    /// `SeriesTitleMatcherFixture`: every way a season is written reads the same.
    #[test]
    fn season_markers_normalize_alike() {
        for t in [
            "Attack on Titan Season 2",
            "Attack on Titan 2nd Season",
            "Attack on Titan S2",
            "Attack on Titan S02",
            "Attack on Titan Part 2",
            "Attack on Titan II",
            "attack on titan: season 2",
        ] {
            assert_eq!(normalize(t), "attack on titan season 2", "{t}");
        }
        assert_eq!(normalize("The Café Terrace"), "cafe terrace");
        assert_eq!(normalize("Mob Psycho 100 III"), "mob psycho 100 season 3");
        assert_eq!(
            score("attack on titan 2nd season", "Attack on Titan Season 2"),
            1.0
        );
        assert_eq!(score("Frieren", "Frieren Beyond Journey's End"), 0.95);
        assert!(score("Shingeki no Kyojn", "Shingeki no Kyojin") > 0.9);
        assert!(score("Frieren", "Spy x Family") < 0.6);
        assert_eq!(score("", "x"), 0.0);
    }

    #[test]
    fn jaro_winkler_matches_reference_values() {
        // The classic examples.
        assert!((jaro_winkler("martha", "marhta") - 0.9611).abs() < 1e-3);
        assert!((jaro_winkler("dwayne", "duane") - 0.84).abs() < 1e-3);
        assert!((jaro_winkler("dixon", "dicksonx") - 0.8133).abs() < 1e-3);
    }

    #[test]
    fn exact_titles_then_similarity_then_votes() {
        // AniList's popularity order buries the exact match.
        let ranked = titles(
            "Attack on Titan Season 2",
            vec![
                result("Attack on Titan", 2013, 900_000, &[]),
                result("Attack on Titan: Junior High", 2015, 50_000, &[]),
                result("Attack on Titan Season 2", 2017, 600_000, &[]),
            ],
        );
        assert_eq!(ranked[0], "Attack on Titan Season 2");
        // A synonym counts: "AoT" is only among the alternate titles; votes break the tie.
        let ranked = titles(
            "AoT",
            vec![
                result("Shingeki no Kyojin: Lost Girls", 2022, 10_000, &["AoT"]),
                result("Spy x Family", 2022, 900_000, &[]),
                result(
                    "Shingeki no Kyojin",
                    2013,
                    700_000,
                    &["AoT", "Attack on Titan"],
                ),
            ],
        );
        assert_eq!(
            ranked,
            [
                "Shingeki no Kyojin",
                "Shingeki no Kyojin: Lost Girls",
                "Spy x Family"
            ]
        );
    }

    #[test]
    fn years_and_articles() {
        let ranked = titles(
            "Battlestar Galactica 1978",
            vec![
                result("Battlestar Galactica (2003)", 2003, 10, &[]),
                result("Battlestar Galactica", 1978, 5, &[]),
            ],
        );
        assert_eq!(ranked[0], "Battlestar Galactica");
        let ranked = titles(
            "Office",
            vec![
                result("Office Space Stories", 2001, 0, &[]),
                result("The Office", 2005, 0, &[]),
            ],
        );
        assert_eq!(ranked[0], "The Office");
        // Two exact-ish matches without a year in the query: the newer first.
        let mut dunes = vec![result("Dune", 1984, 0, &[]), result("Dune", 2021, 0, &[])];
        rank("Dune", &mut dunes);
        assert_eq!(dunes[0].year, Some(2021));
        let ranked = titles(
            "CSI",
            vec![
                result("CSI: Miami", 2002, 0, &[]),
                result("CSI: Vegas", 2021, 0, &[]),
            ],
        );
        assert_eq!(ranked[0], "CSI: Vegas");
    }
}
