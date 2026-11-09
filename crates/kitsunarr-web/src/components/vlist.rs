//! Windowed list: renders only the rows in view (plus overscan), so lists with thousands of
//! rows (long anime seasons, history) stay responsive.

use dioxus::prelude::*;

const OVERSCAN: usize = 8;

#[component]
pub fn VirtualList(
    count: usize,
    row_height: f64,
    /// Viewport height in pixels.
    height: f64,
    render: Callback<usize, Element>,
    #[props(default)] label: Option<String>,
) -> Element {
    let mut scroll_top = use_signal(|| 0.0_f64);
    let first = ((scroll_top() / row_height) as usize).saturating_sub(OVERSCAN);
    let visible = (height / row_height).ceil() as usize + OVERSCAN * 2;
    let last = (first + visible).min(count);
    let total = count as f64 * row_height;
    rsx! {
        div {
            class: "vlist",
            style: "height: {height}px",
            role: "list",
            "aria-label": label.unwrap_or_default(),
            onscroll: move |e: ScrollEvent| scroll_top.set(e.scroll_top()),
            div { style: "height: {total}px; position: relative",
                for i in first..last {
                    div {
                        key: "{i}",
                        class: "vlist-row",
                        role: "listitem",
                        style: "top: {i as f64 * row_height}px; height: {row_height}px",
                        {render.call(i)}
                    }
                }
            }
        }
    }
}
