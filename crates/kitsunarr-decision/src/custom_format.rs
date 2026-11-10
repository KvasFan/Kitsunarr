//! Custom format evaluation: `CustomFormatCalculationService` and the eight specifications in
//! Sonarr's `CustomFormats/Specifications`.

use kitsunarr_domain::Language;
use kitsunarr_domain::api::settings::{CustomFormat, CustomFormatSpecification};
use kitsunarr_domain::quality::Quality;
use kitsunarr_parser::ReleaseType;
use kitsunarr_parser::netregex::NetRegex;
use serde_json::Value;

/// What a custom format is evaluated against (`CustomFormatInput`).
#[derive(Debug, Clone, Copy)]
pub struct CustomFormatInput<'a> {
    /// `EpisodeInfo.ReleaseTitle`.
    pub release_title: &'a str,
    /// `Filename` (files only).
    pub file_name: Option<&'a str>,
    pub release_group: Option<&'a str>,
    pub quality: &'static Quality,
    pub size: i64,
    pub indexer_flags: u32,
    pub languages: &'a [&'static Language],
    pub release_type: ReleaseType,
    /// The series' original language, which the `Original` language value stands for.
    pub original_language: Option<&'static Language>,
}

/// Sonarr's numeric `ReleaseType`.
pub fn release_type_id(t: ReleaseType) -> i64 {
    match t {
        ReleaseType::Unknown => 0,
        ReleaseType::SingleEpisode => 1,
        ReleaseType::MultiEpisode => 2,
        ReleaseType::SeasonPack => 3,
        ReleaseType::MultiSeasonPack => 4,
    }
}

#[derive(Debug)]
enum Spec {
    ReleaseTitle(Option<NetRegex>),
    ReleaseGroup(Option<NetRegex>),
    Language {
        value: i64,
        except: bool,
    },
    IndexerFlag(i64),
    Source(i64),
    Resolution(i64),
    Size {
        min: f64,
        max: f64,
    },
    ReleaseType(i64),
    /// An implementation Kitsunarr cannot evaluate: never satisfied (reported when compiled).
    Unsupported,
}

#[derive(Debug)]
struct CompiledSpec {
    implementation: String,
    spec: Spec,
    negate: bool,
    required: bool,
}

/// A custom format ready for evaluation.
#[derive(Debug)]
pub struct CompiledFormat {
    pub id: i64,
    pub name: String,
    specs: Vec<CompiledSpec>,
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Reads a field case-insensitively (API `fields` use camelCase; database bodies may not).
fn field<'a>(s: &'a CustomFormatSpecification, name: &str) -> Option<&'a Value> {
    s.fields
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

fn int_field(s: &CustomFormatSpecification, name: &str) -> i64 {
    match field(s, name) {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::String(v)) => v.trim().parse().unwrap_or(0),
        Some(Value::Bool(b)) => i64::from(*b),
        _ => 0,
    }
}

fn float_field(s: &CustomFormatSpecification, name: &str) -> f64 {
    match field(s, name) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(v)) => v.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn bool_field(s: &CustomFormatSpecification, name: &str) -> bool {
    match field(s, name) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().is_some_and(|v| v != 0),
        Some(Value::String(v)) => v.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// `RegexSpecificationBase`: `new Regex(value, IgnoreCase)` when the value is not blank.
fn regex(
    format: &CustomFormat,
    s: &CustomFormatSpecification,
    problems: &mut Vec<String>,
) -> Option<NetRegex> {
    let value = match field(s, "value") {
        Some(Value::String(v)) => v.as_str(),
        _ => "",
    };
    if value.trim().is_empty() {
        return None;
    }
    match NetRegex::new(value, true) {
        Ok(r) => Some(r),
        Err(e) => {
            problems.push(format!(
                "Custom format \"{}\": the pattern of \"{}\" is not valid ({e}); it never matches.",
                format.name, s.name
            ));
            None
        }
    }
}

/// Compiles the stored custom formats. Specifications that cannot be evaluated (unknown
/// implementations, invalid patterns) never match and are reported, never silently dropped.
pub fn compile_formats(formats: &[CustomFormat]) -> (Vec<CompiledFormat>, Vec<String>) {
    let mut problems = Vec::new();
    let compiled = formats
        .iter()
        .map(|f| CompiledFormat {
            id: f.id,
            name: f.name.clone(),
            specs: f
                .specifications
                .iter()
                .map(|s| {
                    let spec = match s.implementation.as_str() {
                        "ReleaseTitleSpecification" => {
                            Spec::ReleaseTitle(regex(f, s, &mut problems))
                        }
                        "ReleaseGroupSpecification" => {
                            Spec::ReleaseGroup(regex(f, s, &mut problems))
                        }
                        "LanguageSpecification" => Spec::Language {
                            value: int_field(s, "value"),
                            except: bool_field(s, "exceptLanguage"),
                        },
                        "IndexerFlagSpecification" => Spec::IndexerFlag(int_field(s, "value")),
                        "SourceSpecification" => Spec::Source(int_field(s, "value")),
                        "ResolutionSpecification" => Spec::Resolution(int_field(s, "value")),
                        "SizeSpecification" => Spec::Size {
                            min: float_field(s, "min"),
                            max: float_field(s, "max"),
                        },
                        "ReleaseTypeSpecification" => Spec::ReleaseType(int_field(s, "value")),
                        other => {
                            problems.push(format!(
                                "Custom format \"{}\": Kitsunarr cannot evaluate {other} specifications yet; \"{}\" never matches.",
                                f.name, s.name
                            ));
                            Spec::Unsupported
                        }
                    };
                    CompiledSpec {
                        implementation: s.implementation.clone(),
                        spec,
                        negate: s.negate,
                        required: s.required,
                    }
                })
                .collect(),
        })
        .collect();
    (compiled, problems)
}

fn matches(regex: &Option<NetRegex>, text: Option<&str>) -> bool {
    match (regex, text) {
        (Some(r), Some(t)) => r.is_match(t),
        _ => false,
    }
}

/// `Gigabytes()`: `Convert.ToInt64(gb * 1024³)` (rounds to nearest, ties to even).
fn gigabytes(gb: f64) -> i64 {
    (gb * GIB).round_ties_even() as i64
}

impl CompiledSpec {
    /// `IsSatisfiedByWithoutNegate`.
    fn satisfied_without_negate(&self, input: &CustomFormatInput<'_>) -> bool {
        match &self.spec {
            Spec::ReleaseTitle(r) => {
                matches(r, Some(input.release_title)) || matches(r, input.file_name)
            }
            Spec::ReleaseGroup(r) => matches(r, input.release_group),
            Spec::Language { value, except } => {
                // `Original` stands for the series' original language when it is known.
                let compared = match input.original_language {
                    Some(original) if *value == -2 && original.id != 0 => i64::from(original.id),
                    _ => *value,
                };
                if *except {
                    input.languages.iter().any(|l| i64::from(l.id) != compared)
                } else {
                    input.languages.iter().any(|l| i64::from(l.id) == compared)
                }
            }
            // `HasFlag`: every bit of the value is set (so 0 always matches).
            Spec::IndexerFlag(v) => {
                let v = *v as u32;
                input.indexer_flags & v == v
            }
            Spec::Source(v) => i64::from(input.quality.source.sonarr_id()) == *v,
            Spec::Resolution(v) => i64::from(input.quality.resolution) == *v,
            Spec::Size { min, max } => {
                input.size > gigabytes(*min) && input.size <= gigabytes(*max)
            }
            Spec::ReleaseType(v) => release_type_id(input.release_type) == *v,
            Spec::Unsupported => false,
        }
    }

    /// `IsSatisfiedBy`: the result, inverted when negated. (The language specification's own
    /// negation is equivalent whenever the language list exists, which it always does here.)
    fn satisfied(&self, input: &CustomFormatInput<'_>) -> bool {
        let m = self.satisfied_without_negate(input);
        if matches!(self.spec, Spec::Unsupported) {
            return false;
        }
        m != self.negate
    }
}

impl CompiledFormat {
    /// `ParseCustomFormat`: specifications are grouped by implementation; every group must match,
    /// and a group matches unless a required member fails or no member matches
    /// (`SpecificationMatchesGroup.DidMatch`).
    pub fn matches(&self, input: &CustomFormatInput<'_>) -> bool {
        let mut groups: Vec<(&str, Vec<(bool, bool)>)> = Vec::new();
        for spec in &self.specs {
            let result = (spec.required, spec.satisfied(input));
            match groups
                .iter_mut()
                .find(|(k, _)| *k == spec.implementation.as_str())
            {
                Some((_, v)) => v.push(result),
                None => groups.push((spec.implementation.as_str(), vec![result])),
            }
        }
        groups.iter().all(|(_, members)| {
            let required_failed = members.iter().any(|(req, ok)| *req && !ok);
            let none_matched = members.iter().all(|(_, ok)| !ok);
            !(required_failed || none_matched)
        })
    }
}

/// The formats an input matches, ordered by name (`OrderBy(x => x.Name)`; culture-aware
/// ordering is approximated as case-insensitive, then ordinal).
pub fn evaluate<'f>(
    formats: &'f [CompiledFormat],
    input: &CustomFormatInput<'_>,
) -> Vec<&'f CompiledFormat> {
    let mut found: Vec<&CompiledFormat> = formats.iter().filter(|f| f.matches(input)).collect();
    found.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    found
}
