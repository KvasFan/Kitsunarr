//! Small reusable building blocks.

use dioxus::prelude::*;

use crate::i18n::t;

#[component]
pub fn Loading(#[props(default)] label: Option<String>) -> Element {
    rsx! {
        div { class: "loading", role: "status",
            div { class: "spinner" }
            span { {label.unwrap_or_else(|| t().loading.to_string())} }
        }
    }
}

#[component]
pub fn ErrorBox(message: String, #[props(default)] on_retry: Option<EventHandler<()>>) -> Element {
    rsx! {
        div { class: "alert danger", role: "alert",
            div { class: "grow", "{message}" }
            if let Some(retry) = on_retry {
                button { class: "btn sm", onclick: move |_| retry.call(()), {t().retry} }
            }
        }
    }
}

#[component]
pub fn Empty(title: String, children: Element) -> Element {
    rsx! {
        div { class: "empty",
            h3 { "{title}" }
            {children}
        }
    }
}

#[component]
pub fn Badge(text: String, #[props(default)] tone: Option<&'static str>) -> Element {
    let class = match tone {
        Some(t) => format!("badge {t}"),
        None => "badge".to_string(),
    };
    rsx! { span { class: "{class}", "{text}" } }
}

#[component]
pub fn StatCard(label: String, value: String, #[props(default)] accent: bool) -> Element {
    rsx! {
        div { class: if accent { "stat accent" } else { "stat" },
            div { class: "value", "{value}" }
            div { class: "label", "{label}" }
        }
    }
}

#[component]
pub fn Progress(
    percent: f64,
    #[props(default)] tone: Option<&'static str>,
    #[props(default)] indeterminate: bool,
) -> Element {
    let mut class = String::from("progress");
    if let Some(t) = tone {
        class.push(' ');
        class.push_str(t);
    }
    if indeterminate {
        class.push_str(" indeterminate");
    }
    rsx! {
        div { class: "{class}", role: "progressbar", "aria-valuenow": "{percent:.0}", "aria-valuemin": "0", "aria-valuemax": "100",
            span { style: "width: {percent:.1}%" }
        }
    }
}

#[component]
pub fn Switch(
    checked: bool,
    onchange: EventHandler<bool>,
    #[props(default)] label: Option<String>,
    #[props(default)] disabled: bool,
) -> Element {
    rsx! {
        label { class: "check",
            span { class: "switch",
                input {
                    r#type: "checkbox",
                    checked,
                    disabled,
                    "aria-label": label.clone().unwrap_or_default(),
                    onchange: move |e: FormEvent| onchange.call(e.checked()),
                }
                span {}
            }
            if let Some(l) = label.as_ref() { span { "{l}" } }
        }
    }
}

#[component]
pub fn Pagination(page: u32, page_size: u32, total: u64, on_page: EventHandler<u32>) -> Element {
    let pages = total.div_ceil(u64::from(page_size.max(1))).max(1) as u32;
    let first = if total == 0 {
        0
    } else {
        u64::from(page - 1) * u64::from(page_size) + 1
    };
    let last = (u64::from(page) * u64::from(page_size)).min(total);
    rsx! {
        div { class: "pager",
            span { class: "muted small", "{first}–{last} of {total}" }
            div { class: "row tight",
                button { class: "btn sm", disabled: page <= 1, onclick: move |_| on_page.call(1), "«" }
                button { class: "btn sm", disabled: page <= 1, onclick: move |_| on_page.call(page - 1), "‹ Prev" }
                span { class: "small", "Page {page} / {pages}" }
                button { class: "btn sm", disabled: page >= pages, onclick: move |_| on_page.call(page + 1), "Next ›" }
                button { class: "btn sm", disabled: page >= pages, onclick: move |_| on_page.call(pages), "»" }
            }
        }
    }
}

#[component]
pub fn Modal(
    title: String,
    on_close: EventHandler<()>,
    #[props(default)] wide: bool,
    children: Element,
    footer: Option<Element>,
) -> Element {
    rsx! {
        div {
            class: "modal-backdrop",
            onclick: move |_| on_close.call(()),
            onkeydown: move |e: KeyboardEvent| if e.key() == Key::Escape { on_close.call(()) },
            div {
                class: if wide { "modal wide" } else { "modal" },
                role: "dialog",
                "aria-modal": "true",
                "aria-label": "{title}",
                onclick: move |e| e.stop_propagation(),
                div { class: "modal-head",
                    h2 { class: "grow", "{title}" }
                    button { class: "btn ghost icon", "aria-label": "Close", onclick: move |_| on_close.call(()), "✕" }
                }
                div { class: "modal-body", {children} }
                if let Some(f) = footer { div { class: "modal-foot", {f} } }
            }
        }
    }
}

/// Confirmation dialog for destructive or important actions.
#[component]
pub fn Confirm(
    title: String,
    message: String,
    confirm_label: String,
    #[props(default)] danger: bool,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        Modal {
            title: title.clone(),
            on_close: move |_| on_cancel.call(()),
            footer: rsx! {
                button { class: "btn", onclick: move |_| on_cancel.call(()), {t().cancel} }
                button { class: if danger { "btn danger solid" } else { "btn primary" }, onclick: move |_| on_confirm.call(()), "{confirm_label}" }
            },
            p { "{message}" }
        }
    }
}

/// Honest notice for features that are planned but not implemented yet.
#[component]
pub fn NotReady(feature: String, detail: String) -> Element {
    rsx! {
        div { class: "card",
            div { class: "card-head",
                h2 { class: "grow", "{feature}" }
                Badge { text: t().not_available_yet.to_string(), tone: "warning" }
            }
            p { class: "muted", "{detail}" }
            p { class: "small",
                "Implementation status is tracked in the "
                a { href: "https://github.com/KvasFan/Kitsunarr/blob/main/docs/architecture/feature-matrix.md", target: "_blank", rel: "noopener", "feature matrix" }
                "."
            }
        }
    }
}

#[component]
pub fn Field(label: String, #[props(default)] help: Option<String>, children: Element) -> Element {
    rsx! {
        div { class: "field",
            label { "{label}" }
            {children}
            if let Some(h) = help { span { class: "help", "{h}" } }
        }
    }
}
