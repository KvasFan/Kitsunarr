//! Media domain enums shared across subsystems.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Error returned when parsing one of the string-coded enums fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} value `{value}`")]
pub struct UnknownValue {
    pub kind: &'static str,
    pub value: String,
}

/// Defines an enum stored as a stable lowercase/camelCase string in the database and API.
macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $code:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $code)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $code),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownValue;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                $(if s.eq_ignore_ascii_case($code) { return Ok($name::$variant); })+
                Err(UnknownValue { kind: stringify!($name), value: s.to_string() })
            }
        }
    };
}

string_enum! {
    /// Library section a title belongs to.
    MediaKind {
        Series => "series",
        Anime => "anime",
        Movie => "movie",
    }
}

string_enum! {
    /// Episode numbering/search behaviour of a series (Sonarr-compatible).
    SeriesType {
        Standard => "standard",
        Daily => "daily",
        Anime => "anime",
    }
}

string_enum! {
    SeriesStatus {
        Continuing => "continuing",
        Ended => "ended",
        Upcoming => "upcoming",
        Deleted => "deleted",
        Unknown => "unknown",
    }
}

string_enum! {
    MovieStatus {
        Announced => "announced",
        InCinemas => "inCinemas",
        Released => "released",
        Deleted => "deleted",
        Unknown => "unknown",
    }
}

string_enum! {
    /// Whether newly discovered seasons/episodes are monitored automatically.
    MonitorNewItems {
        All => "all",
        None => "none",
    }
}

string_enum! {
    /// A metadata catalogue that can own a title. Codes match Tanukarr's `metadataSource`
    /// values so API and backup migrations keep provider ownership without translation.
    MetadataProvider {
        Tvdb => "tvdb",
        TmdbTv => "tmdb",
        TvMaze => "tvMaze",
        AniList => "aniList",
        MyAnimeList => "myAnimeList",
        TmdbMovie => "tmdbMovie",
        Omdb => "omdb",
    }
}

impl MetadataProvider {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Tvdb => "TheTVDB",
            Self::TmdbTv => "TMDB (TV)",
            Self::TvMaze => "TVmaze",
            Self::AniList => "AniList",
            Self::MyAnimeList => "MyAnimeList (Jikan)",
            Self::TmdbMovie => "TMDB (Movies)",
            Self::Omdb => "OMDb",
        }
    }

    /// Whether the catalogue describes films rather than episodic series.
    pub fn is_movie_catalogue(self) -> bool {
        matches!(self, Self::TmdbMovie | Self::Omdb)
    }
}

string_enum! {
    /// Kinds of external identifier a title may carry. A title can have several of each
    /// (for example one AniList entry per cour of a TheTVDB series).
    ExternalIdKind {
        Tvdb => "tvdb",
        Tmdb => "tmdb",
        TvMaze => "tvMaze",
        Imdb => "imdb",
        AniList => "aniList",
        MyAnimeList => "myAnimeList",
        TvRage => "tvRage",
    }
}

string_enum! {
    DownloadProtocol {
        Unknown => "unknown",
        Usenet => "usenet",
        Torrent => "torrent",
    }
}

string_enum! {
    /// How a movie's availability gates automatic searches.
    MinimumAvailability {
        Announced => "announced",
        InCinemas => "inCinemas",
        Released => "released",
    }
}

/// A piece of artwork reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MediaImage {
    /// `poster`, `banner`, `fanart`, `screenshot`, `headshot`, `clearlogo`.
    pub cover_type: String,
    pub remote_url: String,
}

/// An external identifier attached to a title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ExternalId {
    pub kind: ExternalIdKind,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_enums_round_trip() {
        for p in MetadataProvider::ALL {
            assert_eq!(p.as_str().parse::<MetadataProvider>().unwrap(), *p);
            let json = serde_json::to_string(p).unwrap();
            assert_eq!(json, format!("\"{}\"", p.as_str()));
        }
        assert_eq!("ANIME".parse::<SeriesType>().unwrap(), SeriesType::Anime);
        assert!("bogus".parse::<MediaKind>().is_err());
    }
}
