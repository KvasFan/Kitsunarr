//! Language catalogue.
//!
//! Identifiers match Sonarr (captured from `/api/v3/language` on Sonarr 4.0.20, plus the ids
//! later Sonarr versions added) so that file language lists, custom format specifications and
//! profiles migrate unchanged.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Language {
    pub id: i32,
    pub name: &'static str,
    /// BCP 47 tag used for display and metadata provider requests.
    pub code: &'static str,
}

const fn l(id: i32, name: &'static str, code: &'static str) -> Language {
    Language { id, name, code }
}

pub const LANGUAGES: &[Language] = &[
    l(-2, "Original", "und"),
    l(0, "Unknown", "und"),
    l(1, "English", "en"),
    l(2, "French", "fr"),
    l(3, "Spanish", "es"),
    l(4, "German", "de"),
    l(5, "Italian", "it"),
    l(6, "Danish", "da"),
    l(7, "Dutch", "nl"),
    l(8, "Japanese", "ja"),
    l(9, "Icelandic", "is"),
    l(10, "Chinese", "zh"),
    l(11, "Russian", "ru"),
    l(12, "Polish", "pl"),
    l(13, "Vietnamese", "vi"),
    l(14, "Swedish", "sv"),
    l(15, "Norwegian", "no"),
    l(16, "Finnish", "fi"),
    l(17, "Turkish", "tr"),
    l(18, "Portuguese", "pt"),
    l(19, "Flemish", "nl-BE"),
    l(20, "Greek", "el"),
    l(21, "Korean", "ko"),
    l(22, "Hungarian", "hu"),
    l(23, "Hebrew", "he"),
    l(24, "Lithuanian", "lt"),
    l(25, "Czech", "cs"),
    l(26, "Arabic", "ar"),
    l(27, "Hindi", "hi"),
    l(28, "Bulgarian", "bg"),
    l(29, "Malayalam", "ml"),
    l(30, "Ukrainian", "uk"),
    l(31, "Slovak", "sk"),
    l(32, "Thai", "th"),
    l(33, "Portuguese (Brazil)", "pt-BR"),
    l(34, "Spanish (Latino)", "es-419"),
    l(35, "Romanian", "ro"),
    l(36, "Latvian", "lv"),
    l(37, "Persian", "fa"),
    l(38, "Catalan", "ca"),
    l(39, "Croatian", "hr"),
    l(40, "Serbian", "sr"),
    l(41, "Bosnian", "bs"),
    l(42, "Estonian", "et"),
    l(43, "Tamil", "ta"),
    l(44, "Indonesian", "id"),
    l(45, "Macedonian", "mk"),
    l(46, "Slovenian", "sl"),
    // Added by later Sonarr versions (ids from Sonarr's `Language.cs`, revision c018578e4).
    l(47, "Azerbaijani", "az"),
    l(48, "Uzbek", "uz"),
    l(49, "Malay", "ms"),
    l(50, "Urdu", "ur"),
    l(51, "Romansh", "rm"),
    l(52, "Georgian", "ka"),
    l(53, "Irish", "ga"),
];

impl Language {
    pub fn by_id(id: i32) -> Option<&'static Language> {
        LANGUAGES.iter().find(|l| l.id == id)
    }

    pub fn by_name(name: &str) -> Option<&'static Language> {
        LANGUAGES.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    pub fn all() -> &'static [Language] {
        LANGUAGES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_matches_sonarr_size_and_ids() {
        // Sonarr 4.0.20's 48 plus the 7 added by later versions.
        assert_eq!(LANGUAGES.len(), 55);
        assert_eq!(Language::by_id(53).unwrap().name, "Irish");
        assert_eq!(Language::by_id(8).unwrap().name, "Japanese");
        assert_eq!(Language::by_name("portuguese (brazil)").unwrap().id, 33);
    }
}
