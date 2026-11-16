//! Custom-format editor catalogue. Names and values follow Sonarr's
//! `CustomFormats/Specifications` and `IndexerFlags` (GPL-3.0-or-later).

use crate::api::settings::CustomFormatSpecification;

impl CustomFormatSpecification {
    /// Migrated fields may retain PascalCase keys. Editing replaces that key rather than
    /// adding a second value that the evaluator might not read.
    pub fn field(&self, name: &str) -> Option<&serde_json::Value> {
        self.fields
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    }

    pub fn set_field(&mut self, name: &str, value: serde_json::Value) {
        self.fields.retain(|key, _| !key.eq_ignore_ascii_case(name));
        self.fields.insert(name.into(), value);
    }
}

pub const SPECIFICATIONS: &[(&str, &str)] = &[
    ("ReleaseTitleSpecification", "Release title"),
    ("ReleaseGroupSpecification", "Release group"),
    ("LanguageSpecification", "Language"),
    ("SourceSpecification", "Source"),
    ("ResolutionSpecification", "Resolution"),
    ("SizeSpecification", "Size"),
    ("IndexerFlagSpecification", "Indexer flags"),
    ("ReleaseTypeSpecification", "Release type"),
];

pub const SOURCES: &[(i64, &str)] = &[
    (0, "Unknown"),
    (1, "Television"),
    (2, "Raw television"),
    (3, "WEB-DL"),
    (4, "WEBRip"),
    (5, "DVD"),
    (6, "Blu-ray"),
    (7, "Blu-ray remux"),
];
pub const RESOLUTIONS: &[(i64, &str)] = &[
    (0, "Unknown"),
    (360, "360p"),
    (480, "480p"),
    (576, "576p"),
    (720, "720p"),
    (1080, "1080p"),
    (2160, "2160p"),
];
pub const RELEASE_TYPES: &[(i64, &str)] = &[
    (0, "Unknown"),
    (1, "Single episode"),
    (2, "Multiple episodes"),
    (3, "Season pack"),
    (4, "Multiple seasons"),
];
pub const INDEXER_FLAGS: &[(i64, &str)] = &[
    (1, "Freeleech"),
    (2, "Halfleech"),
    (4, "Double upload"),
    (8, "Internal"),
    (16, "Scene"),
    (32, "75% freeleech"),
    (64, "25% freeleech"),
    (128, "Nuked"),
    (256, "Subtitles"),
];

pub fn new_specification(implementation: &str) -> Option<CustomFormatSpecification> {
    let name = SPECIFICATIONS
        .iter()
        .find(|(kind, _)| *kind == implementation)?
        .1;
    let fields = match implementation {
        "ReleaseTitleSpecification" | "ReleaseGroupSpecification" => {
            serde_json::json!({"value": ""})
        }
        "LanguageSpecification" => serde_json::json!({"value": 1, "exceptLanguage": false}),
        "SourceSpecification" => serde_json::json!({"value": 3}),
        "ResolutionSpecification" => serde_json::json!({"value": 1080}),
        "SizeSpecification" => serde_json::json!({"min": 0, "max": 10}),
        "IndexerFlagSpecification" => serde_json::json!({"value": 1}),
        "ReleaseTypeSpecification" => serde_json::json!({"value": 3}),
        _ => return None,
    };
    Some(CustomFormatSpecification {
        name: name.into(),
        implementation: implementation.into(),
        negate: false,
        required: false,
        fields: fields.as_object().cloned().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_migrated_fields_replaces_case_variants_and_preserves_extensions() {
        let mut spec = new_specification("ReleaseTitleSpecification").unwrap();
        spec.fields =
            serde_json::json!({"Value": "before", "value": "duplicate", "futureField": 7})
                .as_object()
                .unwrap()
                .clone();
        spec.set_field("value", serde_json::json!("after"));
        assert_eq!(spec.field("VALUE"), Some(&serde_json::json!("after")));
        assert_eq!(spec.fields.len(), 2);
        assert_eq!(spec.fields["futureField"], 7);
    }
}
