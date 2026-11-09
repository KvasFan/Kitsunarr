//! Parse results (`Parser/Model/ParsedEpisodeInfo.cs`, `SeriesTitleInfo.cs`).

use kitsunarr_domain::Language;
use serde::Serialize;

use crate::quality::ParsedQuality;

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesTitleInfo {
    pub title: String,
    pub title_without_year: String,
    pub year: i32,
    /// Every title of a multi-title release (`Title A (Title B)`, `A / B`, ...).
    pub all_titles: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReleaseType {
    Unknown,
    SingleEpisode,
    MultiEpisode,
    SeasonPack,
    MultiSeasonPack,
}

/// Everything a release or file name says about the episodes it contains.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedEpisodeInfo {
    pub release_title: String,
    pub series_title: String,
    pub series_title_info: SeriesTitleInfo,
    pub quality: ParsedQuality,
    pub season_numbers: Vec<i32>,
    pub episode_numbers: Vec<i32>,
    pub absolute_episode_numbers: Vec<i32>,
    pub special_absolute_episode_numbers: Vec<f64>,
    /// `yyyy-MM-dd` for daily releases.
    pub air_date: Option<String>,
    pub languages: Vec<&'static Language>,
    pub full_season: bool,
    pub is_partial_season: bool,
    pub is_season_extra: bool,
    pub is_season_title: bool,
    pub is_split_episode: bool,
    pub is_mini_series: bool,
    pub special: bool,
    pub release_group: Option<String>,
    /// Empty when the release has no hash.
    pub release_hash: String,
    pub season_part: i32,
    pub release_tokens: String,
    pub daily_part: Option<i32>,
}

impl Default for ParsedEpisodeInfo {
    fn default() -> Self {
        Self {
            release_title: String::new(),
            series_title: String::new(),
            series_title_info: SeriesTitleInfo::default(),
            quality: ParsedQuality::unknown(),
            season_numbers: Vec::new(),
            episode_numbers: Vec::new(),
            absolute_episode_numbers: Vec::new(),
            special_absolute_episode_numbers: Vec::new(),
            air_date: None,
            languages: Vec::new(),
            full_season: false,
            is_partial_season: false,
            is_season_extra: false,
            is_season_title: false,
            is_split_episode: false,
            is_mini_series: false,
            special: false,
            release_group: None,
            release_hash: String::new(),
            season_part: 0,
            release_tokens: String::new(),
            daily_part: None,
        }
    }
}

impl ParsedEpisodeInfo {
    /// `SeasonNumbers.FirstOrDefault()`.
    pub fn season_number(&self) -> i32 {
        self.season_numbers.first().copied().unwrap_or(0)
    }

    pub fn is_multi_season(&self) -> bool {
        self.season_numbers.len() > 1
    }

    pub fn is_daily(&self) -> bool {
        self.air_date
            .as_deref()
            .is_some_and(|d| !d.trim().is_empty())
    }

    pub fn is_absolute_numbering(&self) -> bool {
        !self.absolute_episode_numbers.is_empty()
    }

    pub fn is_possible_special_episode(&self) -> bool {
        let no_title = self.series_title.trim().is_empty();
        ((!self.is_daily()
            && no_title
            && (self.episode_numbers.is_empty()
                || self.season_numbers.first().copied().unwrap_or(-1) == 0))
            || (!no_title && self.special))
            || (self.episode_numbers.len() == 1 && self.episode_numbers[0] == 0)
    }

    pub fn is_possible_scene_season_special(&self) -> bool {
        self.season_number() != 0 && self.episode_numbers.len() == 1 && self.episode_numbers[0] == 0
    }

    pub fn release_type(&self) -> ReleaseType {
        if self.episode_numbers.len() > 1 || self.absolute_episode_numbers.len() > 1 {
            ReleaseType::MultiEpisode
        } else if self.episode_numbers.len() == 1 || self.absolute_episode_numbers.len() == 1 {
            ReleaseType::SingleEpisode
        } else if self.full_season {
            if self.is_multi_season() {
                ReleaseType::MultiSeasonPack
            } else {
                ReleaseType::SeasonPack
            }
        } else {
            ReleaseType::Unknown
        }
    }
}

/// `SeasonNumbersFormatter.Format`: "Season 1", "Seasons 1-3" or "Seasons 1, 3".
pub fn format_season_numbers(seasons: &[i32], two_digits: bool) -> String {
    let mut seasons = seasons.to_vec();
    seasons.sort_unstable();
    seasons.dedup();
    let f = |n: i32| {
        if two_digits {
            format!("{n:02}")
        } else {
            n.to_string()
        }
    };
    match seasons.as_slice() {
        [] => "Seasons ".to_string(),
        [one] => format!("Season {}", f(*one)),
        [first, .., last] if (last - first + 1) as usize == seasons.len() => {
            format!("Seasons {}-{}", f(*first), f(*last))
        }
        all => format!(
            "Seasons {}",
            all.iter().map(|s| f(*s)).collect::<Vec<_>>().join(", ")
        ),
    }
}

impl std::fmt::Display for ParsedEpisodeInfo {
    /// `ParsedEpisodeInfo.ToString()`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let episodes = if self.is_daily() && self.episode_numbers.is_empty() {
            self.air_date.clone().unwrap_or_default()
        } else if self.full_season {
            if self.season_numbers.is_empty() {
                "[Unknown Season]".to_string()
            } else {
                format_season_numbers(&self.season_numbers, true)
            }
        } else if !self.episode_numbers.is_empty() {
            format!(
                "S{:02}E{}",
                self.season_number(),
                self.episode_numbers
                    .iter()
                    .map(|e| format!("{e:02}"))
                    .collect::<Vec<_>>()
                    .join("-")
            )
        } else if !self.absolute_episode_numbers.is_empty() {
            self.absolute_episode_numbers
                .iter()
                .map(|e| format!("{e:03}"))
                .collect::<Vec<_>>()
                .join("-")
        } else if self.special {
            if self.season_number() != 0 {
                format!("[Unknown Season {:02} Special]", self.season_number())
            } else {
                "[Unknown Special]".to_string()
            }
        } else {
            "[Unknown Episode]".to_string()
        };
        write!(
            f,
            "{} - {} {}",
            self.series_title, episodes, self.quality.quality.name
        )
    }
}
