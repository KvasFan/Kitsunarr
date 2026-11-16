//! Custom format evaluation: `CustomFormatCalculationService` and the eight specifications in
//! Sonarr's `CustomFormats/Specifications`.

use kitsunarr_domain::Language;
use kitsunarr_domain::api::settings::{
    CustomFormat, CustomFormatSpecification, CustomFormatSpecificationTestResult,
    CustomFormatTestRequest, CustomFormatTestResult,
};
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
    /// A specification Kitsunarr cannot evaluate: never satisfied, even when negated
    /// (reported when compiled).
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
    /// Shown by the `{Custom Formats}` naming token.
    pub include_when_renaming: bool,
    specs: Vec<CompiledSpec>,
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Reads a field case-insensitively (API `fields` use camelCase; database bodies may not).
fn field<'a>(s: &'a CustomFormatSpecification, name: &str) -> Option<&'a Value> {
    s.field(name)
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
) -> Result<Option<NetRegex>, ()> {
    let value = match field(s, "value") {
        Some(Value::String(v)) => v.as_str(),
        _ => "",
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    match NetRegex::new(value, true) {
        Ok(r) => Ok(Some(r)),
        Err(e) => {
            problems.push(format!(
                "Custom format \"{}\": the pattern of \"{}\" is not valid ({e}); it never matches.",
                format.name, s.name
            ));
            Err(())
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
            include_when_renaming: f.include_when_renaming,
            specs: f
                .specifications
                .iter()
                .map(|s| {
                    let spec = match s.implementation.as_str() {
                        "ReleaseTitleSpecification" => regex(f, s, &mut problems)
                            .map(Spec::ReleaseTitle)
                            .unwrap_or(Spec::Unsupported),
                        "ReleaseGroupSpecification" => regex(f, s, &mut problems)
                            .map(Spec::ReleaseGroup)
                            .unwrap_or(Spec::Unsupported),
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

/// Preview uses the same compiled conditions and grouping as searches and file naming.
/// The caller may run this CPU-bound operation on a blocking worker.
pub fn test_format(request: &CustomFormatTestRequest) -> Result<CustomFormatTestResult, String> {
    if request.release_title.trim().is_empty() {
        return Err("A release title is required.".into());
    }
    if request.release_title.chars().count() > 2048
        || request
            .file_name
            .as_ref()
            .is_some_and(|s| s.chars().count() > 2048)
    {
        return Err("Test titles and file names must be at most 2048 characters.".into());
    }
    if request.format.specifications.is_empty() || request.format.specifications.len() > 32 {
        return Err("Test a format with between 1 and 32 conditions.".into());
    }
    if request
        .format
        .specifications
        .iter()
        .any(|s| serde_json::to_vec(&s.fields).map_or(true, |v| v.len() > 8192))
    {
        return Err("Condition fields must be at most 8192 bytes.".into());
    }
    if request.size < 0 {
        return Err("Size must not be negative.".into());
    }
    let original = request
        .original_language
        .map(|id| {
            Language::by_id(id)
                .filter(|l| l.id >= 0)
                .ok_or("Unknown original language.")
        })
        .transpose()?;
    let parsed = kitsunarr_parser::parse_title(&request.release_title);
    let quality = match request.quality_id {
        Some(id) => Quality::by_id(id).ok_or("Unknown quality.")?,
        None => kitsunarr_parser::parse_quality(&request.release_title).quality,
    };
    let group = kitsunarr_parser::parse_release_group(&request.release_title);
    let release_type = match request.release_type {
        None => parsed
            .as_ref()
            .map_or(ReleaseType::Unknown, |p| p.release_type()),
        Some(0) => ReleaseType::Unknown,
        Some(1) => ReleaseType::SingleEpisode,
        Some(2) => ReleaseType::MultiEpisode,
        Some(3) => ReleaseType::SeasonPack,
        Some(4) => ReleaseType::MultiSeasonPack,
        _ => return Err("Unknown release type.".into()),
    };
    let mut languages = match &request.languages {
        Some(ids) => ids
            .iter()
            .map(|id| {
                Language::by_id(*id)
                    .filter(|l| l.id >= 0)
                    .ok_or("Unknown test language.")
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => kitsunarr_parser::parse_languages(&request.release_title),
    };
    if request.languages.is_none()
        && languages.iter().all(|l| l.id == 0)
        && let Some(original) = original.filter(|l| l.id != 0)
    {
        languages = vec![original];
    }
    // The parser uses Original for dual-language releases, as AggregateLanguages does.
    if let Some(position) = languages.iter().position(|l| l.id == -2) {
        languages.remove(position);
        let unknown = Language::by_id(0).expect("Unknown language");
        let original = original.unwrap_or(unknown);
        languages.push(if languages.iter().any(|l| l.id == original.id) {
            unknown
        } else {
            original
        });
    }
    let input = CustomFormatInput {
        release_title: &request.release_title,
        file_name: request.file_name.as_deref(),
        release_group: group.as_deref(),
        quality,
        size: request.size,
        indexer_flags: request.indexer_flags,
        languages: &languages,
        release_type,
        original_language: original,
    };
    let (formats, warnings) = compile_formats(std::slice::from_ref(&request.format));
    let format = &formats[0];
    Ok(CustomFormatTestResult {
        matches: format.matches(&input),
        specifications: format
            .specs
            .iter()
            .zip(&request.format.specifications)
            .map(|(s, original)| CustomFormatSpecificationTestResult {
                name: original.name.clone(),
                implementation: s.implementation.clone(),
                negate: s.negate,
                required: s.required,
                matches: s.satisfied(&input),
            })
            .collect(),
        warnings,
        quality: quality.name.into(),
        release_group: group,
        languages: languages.iter().map(|l| l.name.into()).collect(),
        release_type: kitsunarr_domain::custom_formats::RELEASE_TYPES
            .iter()
            .find(|(id, _)| *id == release_type_id(release_type))
            .map_or("Unknown", |(_, name)| *name)
            .into(),
    })
}
