//! UI columns.

pub mod board;
pub mod chat;
pub mod questions;

use gpui::{Div, div, prelude::*, px, rgb};

/// Standard column container with a title.
pub fn column(title: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .p(px(14.))
        .gap_3()
        .border_r_1()
        .border_color(rgb(0x33333d))
        .child(div().text_sm().text_color(rgb(0x8b8b99)).child(title.to_uppercase()))
}

/// A clickable button.
pub fn button(label: &str) -> gpui::Stateful<Div> {
    div()
        .id(gpui::SharedString::from(label.to_string()))
        .px(px(14.))
        .py(px(8.))
        .rounded_md()
        .bg(rgb(0x3b6fd4))
        .text_color(rgb(0xffffff))
        .text_sm()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0x4d80e6)))
        .child(label.to_string())
}
