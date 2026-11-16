//! UI views and shared building blocks.

pub mod activity;
pub mod agents;
pub mod apps;
pub mod asks;
pub mod board;
pub mod character;
pub mod chat;
pub mod library;
pub mod live;
pub mod markdown;
pub mod motion;
pub mod questions;
pub mod roster;
pub mod seat;
pub mod skills;
pub mod toast;
pub mod trace;

use crate::theme::{Status, Theme, theme};
use gpui::{
    AnyElement, App, Div, FontWeight, Hsla, SharedString, Stateful, div, prelude::*, px,
    svg,
};
use rui_core::TaskStatus;

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
    div().flex().flex_col().gap(px(7.)).pt(px(56.)).pb(px(20.)).child(eyebrow).child(title)
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

/// Status color family for a task status.
pub fn task_status<'a>(t: &TaskStatus, th: &'a Theme) -> &'a Status {
    match t {
        TaskStatus::New | TaskStatus::Clarifying => &th.think,
        TaskStatus::WaitingAnswer | TaskStatus::Open => &th.wait,
        TaskStatus::Split | TaskStatus::InProgress => &th.run,
        TaskStatus::Done => &th.done,
        TaskStatus::Stuck => &th.block,
    }
}

/// Amber card holding a question, an optional "why" input, an answer input and its button.
pub fn question_card(text: &str, why: Option<AnyElement>, input: AnyElement, answer_btn: AnyElement, cx: &App) -> Div {
    let th = theme(cx);
    let head = div()
        .flex()
        .items_start()
        .gap(px(8.))
        .child(svg().path("icons/hand.svg").size(px(15.)).text_color(th.wait.fg))
        .child(div().flex_1().child(markdown::render(text, th.wait.fg, 13., th)));
    let row = match why {
        Some(why) => div().flex().flex_col().gap(px(8.)).child(input).child(why).child(div().flex().child(answer_btn)),
        None => div().flex().items_center().gap(px(8.)).child(div().flex_1().child(input)).child(answer_btn),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(16.))
        .rounded(px(9.))
        .bg(th.wait.dim)
        .border_1()
        .border_color(th.wait.rim)
        .max_w(px(660.))
        .child(head)
        .child(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_task_status_maps() {
        let th = Theme::dark();
        for s in TaskStatus::ALL {
            let _ = task_status(s, &th);
        }
        assert_eq!(task_status(&TaskStatus::Stuck, &th).base, th.block.base);
    }
}
