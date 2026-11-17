//! Settings › Quality: the global quality definitions (Tanukarr's `QualityDefinitions`): each
//! quality's title and its minimum, preferred and maximum size, entered in MiB per minute of
//! runtime and shown per hour. Profiles may keep their own limits (Settings › Quality
//! profiles); a size that differs in some profile reads "Mixed", and a value entered here is
//! written into every profile, as Tanukarr's `QualityDefinitionUpdateService` does.

use dioxus::prelude::*;
use kitsunarr_domain::api::settings::{
    OrderedF64, QualityDefinition, QualityDefinitionUpdate, QualityProfile, QualitySizeLimit,
};

use crate::components::common::{Confirm, ErrorBox, Loading};
use crate::route::Route;
use crate::state::{use_app, use_client};
use crate::util::format_bytes;

/// The three size limits of a definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Size {
    Min,
    Preferred,
    Max,
}

const SIZES: [Size; 3] = [Size::Min, Size::Preferred, Size::Max];

impl Size {
    fn label(self) -> &'static str {
        match self {
            Size::Min => "Minimum",
            Size::Preferred => "Preferred",
            Size::Max => "Maximum",
        }
    }

    fn of_definition(self, d: &QualityDefinition) -> Option<f64> {
        match self {
            Size::Min => d.min_size,
            Size::Preferred => d.preferred_size,
            Size::Max => d.max_size,
        }
        .map(|v| v.0)
    }

    fn of_limit(self, l: &QualitySizeLimit) -> Option<f64> {
        match self {
            Size::Min => l.min_size,
            Size::Preferred => l.preferred_size,
            Size::Max => l.max_size,
        }
        .map(|v| v.0)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One editable row.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    definition: QualityDefinition,
    title: String,
    sizes: [Option<f64>; 3],
    /// Sizes changed on this page (written to the definition and every profile).
    edited: [bool; 3],
    /// Sizes some profile sets differently (Tanukarr's `minSizeIsMixed`, ...).
    mixed: [bool; 3],
}

impl Row {
    fn new(d: &QualityDefinition, profiles: &[QualityProfile]) -> Self {
        let mixed = SIZES.map(|s| {
            profiles.iter().any(|p| {
                p.size_limits
                    .iter()
                    .find(|l| l.quality_id == d.quality_id)
                    .is_some_and(|l| s.of_limit(l) != s.of_definition(d))
            })
        });
        Self {
            definition: d.clone(),
            title: d.title.clone(),
            sizes: SIZES.map(|s| s.of_definition(d)),
            edited: [false; 3],
            mixed,
        }
    }

    fn update(&self) -> Option<QualityDefinitionUpdate> {
        let title_changed = self.title.trim() != self.definition.title;
        if !title_changed && !self.edited.contains(&true) {
            return None;
        }
        let size = |s: Size| self.sizes[s.index()].map(OrderedF64);
        Some(QualityDefinitionUpdate {
            quality_id: self.definition.quality_id,
            title: title_changed.then(|| self.title.trim().to_string()),
            min_size: size(Size::Min),
            preferred_size: size(Size::Preferred),
            max_size: size(Size::Max),
            update_min_size: self.edited[0],
            update_preferred_size: self.edited[1],
            update_max_size: self.edited[2],
        })
    }
}

/// Tanukarr's size caption: the hourly size, "Unlimited", or 0 for no minimum.
fn per_hour(size: Size, value: Option<f64>) -> String {
    match value {
        Some(v) if !(size == Size::Max && v == 0.0) => format!("{:.2} GiB/h", v * 60.0 / 1024.0),
        None if size == Size::Min => "0 GiB/h".into(),
        _ => "Unlimited".into(),
    }
}

/// Tanukarr's `QualityDefinitionLimits`: an episode's size at 30, 45 and 60 minutes.
fn episode_sizes(value: Option<f64>) -> String {
    match value.filter(|v| *v > 0.0) {
        Some(v) => [30.0, 45.0, 60.0]
            .iter()
            .map(|m| {
                format!(
                    "{m} min: {}",
                    format_bytes((v * m * 1024.0 * 1024.0) as i64)
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        None => "No limit".into(),
    }
}

#[component]
pub fn SettingsQuality() -> Element {
    let app = use_app();
    let client = use_client();
    let c0 = client.clone();
    let mut loaded = use_resource(move || {
        let _ = (app.settings_rev)();
        let c = c0.clone();
        async move {
            let defs = c.quality_definitions().await?;
            let profiles = c.quality_profiles().await?;
            Ok::<_, kitsunarr_client::ClientError>((defs, profiles))
        }
    });
    let read = loaded.read();
    match &*read {
        None => rsx! { div { class: "page-head", h1 { "Quality" } } Loading {} },
        Some(Err(e)) => {
            rsx! { div { class: "page-head", h1 { "Quality" } } ErrorBox { message: e.to_string(), on_retry: move |_| loaded.restart() } }
        }
        Some(Ok((defs, profiles))) => {
            // Remounted when the stored values change, so the form starts from them.
            let key = defs
                .iter()
                .map(|d| {
                    format!(
                        "{}:{}:{:?}:{:?}:{:?}",
                        d.quality_id, d.title, d.min_size, d.preferred_size, d.max_size
                    )
                })
                .chain(profiles.iter().map(|p| format!("{:?}", p.size_limits)))
                .collect::<Vec<_>>()
                .join("|");
            let rows: Vec<Row> = defs.iter().map(|d| Row::new(d, profiles)).collect();
            rsx! { QualityDefinitionsForm { key: "{key}", rows, on_saved: move |_| loaded.restart() } }
        }
    }
}

#[component]
fn QualityDefinitionsForm(rows: Vec<Row>, on_saved: EventHandler<()>) -> Element {
    let app = use_app();
    let client = use_client();
    let mut form = use_signal(|| rows.clone());
    let mut show_retired = use_signal(|| false);
    let mut confirm_reset = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let pending = form.read().iter().any(|r| r.update().is_some());
    let has_retired = form.read().iter().any(|r| r.definition.is_retired);
    let c1 = client.clone();
    let save = move |_| {
        let updates: Vec<QualityDefinitionUpdate> =
            form.read().iter().filter_map(Row::update).collect();
        if updates.is_empty() {
            return;
        }
        let c = c1.clone();
        busy.set(true);
        spawn(async move {
            match c.update_quality_definition_sizes(&updates).await {
                Ok(_) => {
                    app.success("Quality definitions saved.");
                    on_saved.call(());
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let reset = move |_| {
        confirm_reset.set(false);
        let c = client.clone();
        busy.set(true);
        spawn(async move {
            match c.reset_quality_definitions().await {
                Ok(_) => {
                    app.success("Quality definitions reset to the defaults.");
                    on_saved.call(());
                }
                Err(e) => app.error(&e),
            }
            busy.set(false);
        });
    };
    let mut set_size = move |i: usize, size: Size, value: Option<f64>| {
        let mut f = form.write();
        f[i].sizes[size.index()] = value;
        f[i].edited[size.index()] = true;
    };
    rsx! {
        div { class: "page-head",
            h1 { class: "grow", "Quality" }
            button { class: "btn", disabled: busy(), onclick: move |_| confirm_reset.set(true), "Reset definitions" }
            button { class: "btn primary", disabled: busy() || !pending, onclick: save, if busy() { "Saving…" } else { "Save" } }
        }
        p { class: "help small faint",
            "Sizes are in MiB per minute of runtime; the hourly size is shown under each. A release outside a quality's limits is rejected; the preferred size ranks releases. Profiles can keep their own limits ("
            Link { to: Route::SettingsProfiles {}, "Settings › Quality profiles" }
            "): \"Mixed\" means profiles use different limits, and a value entered here is written into every profile."
        }
        div { class: "table-wrap",
            table { class: "table quality-definitions",
                thead { tr {
                    th { "Quality" } th { "Title" }
                    for s in SIZES { th { "{s.label()}" } }
                } }
                tbody {
                    for (i, row) in form().iter().enumerate().filter(|(_, r)| show_retired() || !r.definition.is_retired) {
                        tr { key: "{row.definition.quality_id}", class: if row.definition.is_retired { "rejected" } else { "" },
                            td {
                                {kitsunarr_domain::Quality::by_id(row.definition.quality_id).map_or_else(|| row.definition.title.clone(), |q| q.name.to_string())}
                                if row.definition.is_retired { " " span { class: "badge", "retired" } }
                            }
                            td {
                                input { class: "input", style: "min-width: 9rem", "aria-label": "Title of {row.definition.title}", value: "{row.title}",
                                    oninput: move |e| form.write()[i].title = e.value() }
                            }
                            for size in SIZES {
                                {
                                    let k = size.index();
                                    let mixed = row.mixed[k] && !row.edited[k];
                                    let value = row.sizes[k];
                                    let shown = if mixed { String::new() } else { value.map(|v| v.to_string()).unwrap_or_default() };
                                    let name = row.definition.title.clone();
                                    rsx! {
                                        td { key: "{k}",
                                            div { class: "stack tight",
                                                input { class: "input", r#type: "number", min: "0", max: "1000", step: "0.1", style: "width: 7rem",
                                                    "aria-label": "{size.label()} size of {name} (MiB per minute)",
                                                    placeholder: if mixed { "Mixed" } else if size == Size::Min { "None" } else { "Unlimited" },
                                                    title: "{episode_sizes(value)}",
                                                    value: "{shown}",
                                                    oninput: move |e: FormEvent| set_size(i, size, e.value().trim().parse().ok().filter(|v: &f64| v.is_finite())) }
                                                span { class: "xs muted",
                                                    if mixed { "Profiles use different limits" } else { {per_hour(size, value)} }
                                                }
                                                if mixed {
                                                    button { class: "btn sm ghost", onclick: move |_| set_size(i, size, None),
                                                        if size == Size::Min { "No minimum" } else { "Unlimited" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if has_retired {
            div { class: "row",
                button { class: "btn sm ghost", onclick: move |_| show_retired.toggle(),
                    if show_retired() { "Hide retired qualities" } else { "Show retired qualities" }
                }
            }
        }
        if confirm_reset() {
            Confirm {
                title: "Reset quality definitions".to_string(),
                message: "Give every standard quality its default title and size limits? Custom presets and the profiles' own limits are kept.".to_string(),
                confirm_label: "Reset".to_string(),
                danger: true,
                on_confirm: reset,
                on_cancel: move |_| confirm_reset.set(false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition() -> QualityDefinition {
        QualityDefinition {
            quality_id: 7,
            title: "Bluray-1080p".into(),
            min_size: Some(OrderedF64(4.0)),
            preferred_size: Some(OrderedF64(95.0)),
            max_size: None,
            ..Default::default()
        }
    }

    #[test]
    fn mixed_sizes_and_updates() {
        let mut profile = QualityProfile {
            id: 1,
            name: "HD".into(),
            upgrade_allowed: true,
            cutoff: 7,
            items: Vec::new(),
            min_format_score: 0,
            cutoff_format_score: 0,
            min_upgrade_format_score: 1,
            format_items: Vec::new(),
            size_limits: vec![QualitySizeLimit {
                quality_id: 7,
                min_size: Some(OrderedF64(4.0)),
                preferred_size: Some(OrderedF64(50.0)),
                max_size: None,
            }],
            codec_multipliers: Default::default(),
        };
        let row = Row::new(&definition(), std::slice::from_ref(&profile));
        assert_eq!(row.mixed, [false, true, false]);
        assert!(row.update().is_none());
        profile.size_limits.clear();
        assert_eq!(Row::new(&definition(), &[profile]).mixed, [false; 3]);
        let mut edited = row.clone();
        edited.sizes[2] = Some(100.0);
        edited.edited[2] = true;
        edited.title = " Blu-ray 1080p ".into();
        let u = edited.update().unwrap();
        assert_eq!(u.title.as_deref(), Some("Blu-ray 1080p"));
        assert!(u.update_max_size && !u.update_min_size && !u.update_preferred_size);
        assert_eq!(u.max_size, Some(OrderedF64(100.0)));
    }

    #[test]
    fn hourly_captions() {
        assert_eq!(per_hour(Size::Max, Some(17.0666)), "1.00 GiB/h");
        assert_eq!(per_hour(Size::Max, Some(0.0)), "Unlimited");
        assert_eq!(per_hour(Size::Max, None), "Unlimited");
        assert_eq!(per_hour(Size::Min, None), "0 GiB/h");
        assert_eq!(episode_sizes(None), "No limit");
        assert!(episode_sizes(Some(10.0)).starts_with("30 min: 300"));
    }
}
