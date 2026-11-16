//! Guided custom-format conditions and a preview backed by the shared decision engine.

use dioxus::prelude::*;
use kitsunarr_domain::api::settings::{
    CustomFormat, CustomFormatTestRequest, CustomFormatTestResult,
};
use kitsunarr_domain::custom_formats as catalogue;
use serde_json::Value;

use super::common::{Badge, Field, Modal};
use crate::state::use_client;

fn current_format(draft: CustomFormat, advanced: bool, json: &str) -> Result<CustomFormat, String> {
    if advanced {
        Ok(CustomFormat {
            specifications: serde_json::from_str(json)
                .map_err(|e| format!("Specifications must be valid JSON: {e}"))?,
            ..draft
        })
    } else {
        Ok(draft)
    }
}

#[component]
pub fn FormatEditor(
    format: CustomFormat,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let client = use_client();
    let initial_json = serde_json::to_string_pretty(&format.specifications).unwrap_or_default();
    let mut draft = use_signal(|| format);
    let mut advanced = use_signal(|| false);
    let mut json = use_signal(|| initial_json);
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);
    let mut testing = use_signal(|| false);
    let mut kind = use_signal(|| "ReleaseTitleSpecification".to_string());
    let mut title = use_signal(String::new);
    let mut filename = use_signal(String::new);
    let mut size = use_signal(|| "1".to_string());
    let mut quality = use_signal(String::new);
    let mut release_type = use_signal(String::new);
    let mut language = use_signal(String::new);
    let mut original = use_signal(String::new);
    let mut flags = use_signal(|| "0".to_string());
    let mut preview = use_signal(|| {
        None::<(
            CustomFormatTestRequest,
            Result<CustomFormatTestResult, String>,
        )>
    });
    let current = current_format(draft(), advanced(), &json());
    let request = (|| -> Result<CustomFormatTestRequest, String> {
        let gib = size()
            .parse::<f64>()
            .map_err(|_| "Enter a valid test size in GiB.")?;
        if !gib.is_finite() || gib < 0.0 || gib * 1073741824.0 >= i64::MAX as f64 {
            return Err("Enter a non-negative test size within the supported range.".into());
        }
        Ok(CustomFormatTestRequest {
            format: current.clone()?,
            release_title: title(),
            file_name: if filename().is_empty() {
                None
            } else {
                Some(filename())
            },
            size: (gib * 1073741824.0).round() as i64,
            indexer_flags: flags()
                .parse()
                .map_err(|_| "Enter valid test indexer flags.")?,
            quality_id: quality().parse().ok(),
            release_type: release_type().parse().ok(),
            languages: language().parse::<i32>().ok().map(|id| vec![id]),
            original_language: original().parse().ok(),
        })
    })();
    let c1 = client.clone();
    let save = move |_| {
        let cf = match current_format(draft(), advanced(), &json()) {
            Ok(cf) => cf,
            Err(e) => {
                error.set(Some(e));
                return;
            }
        };
        saving.set(true);
        error.set(None);
        let c = c1.clone();
        spawn(async move {
            let result = if cf.id == 0 {
                c.post::<CustomFormat, _>("/customformats", &cf).await
            } else {
                c.put::<CustomFormat, _>(&format!("/customformats/{}", cf.id), &cf)
                    .await
            };
            saving.set(false);
            match result {
                Ok(_) => on_saved.call(()),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let request_for_test = request.clone();
    let test = move |_| {
        let req = match request_for_test.clone() {
            Ok(req) => req,
            Err(e) => {
                error.set(Some(e));
                return;
            }
        };
        error.set(None);
        testing.set(true);
        preview.set(None);
        let c = client.clone();
        spawn(async move {
            let result = c.test_custom_format(&req).await.map_err(|e| e.to_string());
            preview.set(Some((req, result)));
            testing.set(false);
        });
    };
    // A result belongs to its exact draft and sample, so editing cannot display a stale match.
    let shown_preview = preview().filter(|(tested, _)| request.as_ref().is_ok_and(|r| r == tested));
    rsx! {
        Modal { title: if draft.read().id == 0 { "New custom format".into() } else { format!("Edit “{}”", draft.read().name) }, wide: true,
            on_close: move |_| on_close.call(()),
            footer: rsx! {
                button { class: "btn", disabled: saving(), onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn primary", disabled: saving() || draft.read().name.trim().is_empty() || current.as_ref().map_or(true, |f| f.specifications.is_empty()), onclick: save, if saving() { "Saving…" } else { "Save" } }
            },
            if let Some(msg) = error() { div { class: "alert danger", role: "alert", "{msg}" } }
            Field { label: "Name", input { class: "input", "aria-label": "Custom format name", value: "{draft.read().name}", oninput: move |e| draft.write().name = e.value() } }
            label { class: "check small",
                input { r#type: "checkbox", checked: draft.read().include_when_renaming, onchange: move |e| draft.write().include_when_renaming = e.checked() }
                "Include in file names ({{Custom Formats}} token)"
            }
            div { class: "row", style: "margin: var(--sp-3) 0",
                h3 { class: "grow", "Conditions" }
                button { class: "btn sm", onclick: move |_| {
                    error.set(None);
                    if advanced() {
                        match current_format(draft(), true, &json()) {
                            Ok(cf) => { draft.set(cf); advanced.set(false); }
                            Err(e) => error.set(Some(e)),
                        }
                    } else {
                        json.set(serde_json::to_string_pretty(&draft.read().specifications).unwrap_or_default());
                        advanced.set(true);
                    }
                }, if advanced() { "Guided editor" } else { "Edit JSON" } }
            }
            p { class: "small muted", "Every condition type must match. Within one type, any condition may match and every required condition must match. Negate reverses a valid condition." }
            if advanced() {
                Field { label: "Specifications (JSON)", textarea { class: "input mono", "aria-label": "Specifications JSON", rows: "14", spellcheck: false, value: "{json}", oninput: move |e| json.set(e.value()) } }
            } else {
                for (i, spec) in draft.read().specifications.iter().enumerate() {
                    FormatCondition { key: "{i}-{spec.implementation}", draft, index: i }
                }
                div { class: "row wrap", style: "margin: var(--sp-3) 0",
                    select { class: "select", "aria-label": "New condition type", value: "{kind}", onchange: move |e| kind.set(e.value()),
                        for (value, label) in catalogue::SPECIFICATIONS { option { value: "{value}", "{label}" } }
                    }
                    button { class: "btn sm", onclick: move |_| {
                        if let Some(spec) = catalogue::new_specification(&kind()) { draft.write().specifications.push(spec); }
                    }, "Add condition" }
                }
            }
            hr {}
            h3 { "Test a release" }
            p { class: "small muted", "Try a TV, anime or movie release name before saving. This checks the format's conditions; quality-profile scores and download approval are separate." }
            Field { label: "Release title", input { class: "input mono", "aria-label": "Test release title", placeholder: "[Group] Anime Title - 01 [1080p HEVC]", value: "{title}", oninput: move |e| title.set(e.value()) } }
            details { class: "card format-test-details", style: "margin-bottom: var(--sp-3)",
                summary { "Test details" }
                div { class: "form-grid",
                    Field { label: "Size (GiB)", input { class: "input", r#type: "number", min: "0", step: "any", "aria-label": "Test size in GiB", value: "{size}", oninput: move |e| size.set(e.value()) } }
                    Field { label: "File name (optional)", input { class: "input", "aria-label": "Test file name", value: "{filename}", oninput: move |e| filename.set(e.value()) } }
                    Field { label: "Quality", select { class: "select", "aria-label": "Test quality", value: "{quality}", onchange: move |e| quality.set(e.value()),
                        option { value: "", "Detect from title" }
                        for q in kitsunarr_domain::quality::QUALITIES { option { value: "{q.id}", "{q.name}" } }
                    } }
                    Field { label: "Release type", help: "Override for a mapped anime batch or a movie feature (single episode).",
                        select { class: "select", "aria-label": "Test release type", value: "{release_type}", onchange: move |e| release_type.set(e.value()),
                            option { value: "", "Detect from title" }
                            for (id, label) in catalogue::RELEASE_TYPES { option { value: "{id}", "{label}" } }
                        }
                    }
                    Field { label: "Language", select { class: "select", "aria-label": "Test language", value: "{language}", onchange: move |e| language.set(e.value()),
                        option { value: "", "Detect from title" }
                        for l in kitsunarr_domain::Language::all().iter().filter(|l| l.id >= 0) { option { value: "{l.id}", "{l.name}" } }
                    } }
                    Field { label: "Original language", select { class: "select", "aria-label": "Test original language", value: "{original}", onchange: move |e| original.set(e.value()),
                        option { value: "", "Unknown" }
                        for l in kitsunarr_domain::Language::all().iter().filter(|l| l.id > 0) { option { value: "{l.id}", "{l.name}" } }
                    } }
                    Field { label: "Indexer flags (bitmask)", input { class: "input", r#type: "number", min: "0", "aria-label": "Test indexer flags", value: "{flags}", oninput: move |e| flags.set(e.value()) } }
                }
            }
            button { class: "btn", disabled: testing() || title().trim().is_empty(), onclick: test, if testing() { "Testing…" } else { "Test format" } }
            if let Some((_, result)) = shown_preview {
                match result {
                    Err(e) => rsx! { div { class: "alert danger format-preview", role: "alert",
                        onmounted: move |event| async move { let _ = event.scroll_to(dioxus::html::ScrollBehavior::Instant).await; },
                        "{e}" } },
                    Ok(result) => rsx! {
                        div { class: if result.matches { "alert success format-preview" } else { "alert warning format-preview" }, role: "status",
                            onmounted: move |event| async move { let _ = event.scroll_to(dioxus::html::ScrollBehavior::Instant).await; },
                            strong { if result.matches { "Format matches" } else { "Format does not match" } }
                        }
                        p { class: "small muted", "Quality: {result.quality} · Group: {result.release_group.as_deref().unwrap_or(\"unknown\")} · Languages: {result.languages.join(\", \")} · {result.release_type}" }
                        for (i, warning) in result.warnings.iter().enumerate() { p { key: "warning-{i}", class: "alert warning", "{warning}" } }
                        ul { class: "small format-condition-results",
                            for (i, condition) in result.specifications.iter().enumerate() {
                                li { key: "condition-{i}",
                                    if condition.matches { "✓ " } else { "✕ " }
                                    "{condition.name}"
                                    if condition.negate { " · negated" }
                                    if condition.required { " · required" }
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn FormatCondition(mut draft: Signal<CustomFormat>, index: usize) -> Element {
    let spec = draft.read().specifications[index].clone();
    let known = catalogue::SPECIFICATIONS
        .iter()
        .find(|(kind, _)| *kind == spec.implementation);
    let label = known.map_or(spec.implementation.as_str(), |(_, name)| *name);
    let value = spec.field("value").cloned().unwrap_or(Value::Null);
    let int_value = value
        .as_i64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap_or(0);
    let mut set_field = move |field: &str, value: Value| {
        draft.write().specifications[index].set_field(field, value);
    };
    let choices = match spec.implementation.as_str() {
        "SourceSpecification" => catalogue::SOURCES,
        "ResolutionSpecification" => catalogue::RESOLUTIONS,
        "ReleaseTypeSpecification" => catalogue::RELEASE_TYPES,
        _ => &[],
    };
    rsx! {
        div { class: "card format-condition",
            div { class: "row wrap",
                Badge { text: label }
                input { class: "input grow", "aria-label": "Condition name {index + 1}", value: "{spec.name}", oninput: move |e| draft.write().specifications[index].name = e.value() }
                button { class: "btn sm danger", "aria-label": "Remove condition {index + 1}", onclick: move |_| { draft.write().specifications.remove(index); }, "Remove" }
            }
            div { class: "form-grid", style: "margin-top: var(--sp-3)",
                match spec.implementation.as_str() {
                    "ReleaseTitleSpecification" | "ReleaseGroupSpecification" => rsx! {
                        Field { label: "Regular expression", input { class: "input mono", "aria-label": "Condition regular expression {index + 1}", spellcheck: false, value: "{value.as_str().unwrap_or_default()}", oninput: move |e| set_field("value", Value::from(e.value())) } }
                    },
                    "LanguageSpecification" => rsx! {
                        Field { label: "Language", select { class: "select", "aria-label": "Condition language", value: "{int_value}", onchange: move |e| set_field("value", Value::from(e.value().parse::<i64>().unwrap_or(0))),
                            if !kitsunarr_domain::Language::all().iter().any(|l| i64::from(l.id) == int_value) { option { value: "{int_value}", "Unknown value ({int_value})" } }
                            for l in kitsunarr_domain::Language::all() { option { value: "{l.id}", selected: i64::from(l.id) == int_value, "{l.name}" } }
                        } }
                        label { class: "check small", input { r#type: "checkbox", checked: spec.field("exceptLanguage").is_some_and(|v| *v == Value::Bool(true) || v.as_i64().is_some_and(|n| n != 0) || v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("true"))), onchange: move |e| set_field("exceptLanguage", Value::from(e.checked())) } "Match any other language" }
                    },
                    "SizeSpecification" => rsx! {
                        for field in ["min", "max"] {
                            Field { key: "{field}", label: if field == "min" { "Minimum GiB (exclusive)" } else { "Maximum GiB (inclusive)" },
                                input { class: "input", r#type: "number", min: "0", step: "any", "aria-label": "Condition size {field}", value: "{spec.field(field).map_or(String::new(), |v| v.as_str().map_or_else(|| v.to_string(), str::to_string))}",
                                    oninput: move |e| { if let Ok(n) = e.value().parse::<f64>() { set_field(field, Value::from(n)); } }
                                }
                            }
                        }
                    },
                    "IndexerFlagSpecification" => rsx! {
                        Field { label: "Indexer flags (bitmask)", input { class: "input", r#type: "number", min: "0", "aria-label": "Condition indexer flags", value: "{int_value}", oninput: move |e| { if let Ok(n) = e.value().parse::<u32>() { set_field("value", Value::from(n)); } } } }
                        div { class: "small muted", "All selected flags must be present." }
                        for (bit, name) in catalogue::INDEXER_FLAGS {
                            label { key: "{bit}", class: "check small", input { r#type: "checkbox", checked: int_value & bit == *bit, onchange: move |e| set_field("value", Value::from(if e.checked() { int_value | bit } else { int_value & !bit })) } "{name}" }
                        }
                    },
                    _ if !choices.is_empty() => rsx! {
                        Field { label: "Value", select { class: "select", "aria-label": "Condition value", value: "{int_value}", onchange: move |e| set_field("value", Value::from(e.value().parse::<i64>().unwrap_or(0))),
                            if !choices.iter().any(|(id, _)| *id == int_value) { option { value: "{int_value}", "Unknown value ({int_value})" } }
                            for (id, name) in choices { option { value: "{id}", selected: *id == int_value, "{name}" } }
                        } }
                    },
                    _ => rsx! { p { class: "small muted", "This condition is preserved. Use Edit JSON to change its fields; Test format reports whether it can be evaluated." } },
                }
            }
            div { class: "row wrap",
                label { class: "check small", input { r#type: "checkbox", "aria-label": "Negate condition {index + 1}", checked: spec.negate, onchange: move |e| draft.write().specifications[index].negate = e.checked() } "Negate" }
                label { class: "check small", input { r#type: "checkbox", "aria-label": "Required condition {index + 1}", checked: spec.required, onchange: move |e| draft.write().specifications[index].required = e.checked() } "Required" }
            }
        }
    }
}
