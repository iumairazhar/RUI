//! UI views and shared building blocks.

pub mod activity;
pub mod agents;
pub mod apps;
pub mod asks;
pub mod attach;
pub mod character;
pub mod chat;
pub mod client_knowledge;
mod client_deploy;
mod client_edit;
mod client_triggers;
mod chat_tools;
pub mod clients;
pub mod decision;
pub mod inbox;
pub mod inbox_kinds;
pub mod library;
pub mod live;
pub mod markdown;
pub mod motion;
pub mod playbook;
pub mod preferences;
pub mod results;
pub mod roster;
pub mod seat;
pub mod section;
pub mod skills;
pub mod spend;
pub mod status;
pub mod toast;
pub mod trace;

use crate::theme::{Status, theme};
use gpui::{
    App, Div, FontWeight, Hsla, SharedString, Stateful, div, prelude::*, px,
};

/// Page header: small eyebrow line above a big title. Pass the eyebrow already uppercase.
pub fn page_head(eyebrow: &str, title: &str, cx: &App) -> Div {
    let th = theme(cx);
    let eyebrow = div()
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(th.muted)
        .child(eyebrow.to_string());
    let title = div()
        .text_size(px(34.))
        .font_weight(FontWeight::BOLD)
        .text_color(th.fg)
        .child(title.to_string());
    div().flex().flex_col().gap(px(7.)).pt(px(12.)).pb(px(20.)).child(eyebrow).child(title)
}

/// Small colored status tag.
pub fn tag(status: &Status, text: &str) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(20.))
        .px(px(8.))
        .rounded(px(7.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .bg(status.dim)
        .text_color(status.fg)
        .border_1()
        .border_color(status.rim)
        .child(text.to_string())
}

/// Square role chip with the two-letter code on the client's color (mock `.ag-code`).
pub fn role_chip(code: &str, size: f32, color: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size * 0.29))
        .bg(color)
        .text_color(gpui::white())
        .font_family("Menlo")
        .text_size(px(size * 0.375))
        .font_weight(FontWeight::BOLD)
        .child(code.to_string())
}

/// Raised surface card.
pub fn card(cx: &App) -> Div {
    let th = theme(cx);
    div().bg(th.surface).rounded(px(9.)).py(px(9.)).px(px(10.)).shadow(th.card_shadow())
}

/// Filled green action button.
pub fn primary_button(id: impl Into<SharedString>, label: &str, cx: &App) -> Stateful<Div> {
    let th = theme(cx);
    div()
        .id(id.into())
        .flex()
        .items_center()
        .h(px(34.))
        .px(px(14.))
        .rounded(px(9.))
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .bg(th.accent_fill)
        .text_color(th.on_accent)
        .cursor_pointer()
        .hover(|s| s.bg(th.accent_fill_hover))
        .active(|s| s.bg(th.accent_fill_active))
        .child(label.to_string())
}

/// Neutral small button.
pub fn chip(id: impl Into<SharedString>, label: &str, cx: &App) -> Stateful<Div> {
    let th = theme(cx);
    div()
        .id(id.into())
        .flex()
        .items_center()
        .h(px(30.))
        .px(px(12.))
        .rounded(px(7.))
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .bg(th.ctl)
        .text_color(th.fg)
        .shadow(th.card_shadow().into_iter().take(2).collect())
        .cursor_pointer()
        .hover(|s| s.bg(th.surface_2))
        .child(label.to_string())
}
