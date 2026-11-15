//! UI views and shared building blocks.

pub mod activity;
pub mod board;
pub mod chat;
pub mod questions;
pub mod seat;

use crate::theme::{Status, Theme, theme};
use gpui::{
    AnyElement, App, Div, FontWeight, Hsla, Pixels, SharedString, Stateful, div, prelude::*, px,
    svg,
};
use rui_core::TaskStatus;

/// Page header: small eyebrow line above a big title. Pass the eyebrow already uppercase.
#[allow(dead_code)]
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
    div().flex().flex_col().gap(px(7.)).pt(px(40.)).pb(px(20.)).child(eyebrow).child(title)
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

/// Raised surface card.
#[allow(dead_code)]
pub fn card(cx: &App) -> Div {
    let th = theme(cx);
    div().bg(th.surface).rounded(px(9.)).py(px(9.)).px(px(10.)).shadow(th.card_shadow())
}

#[allow(dead_code)]
fn abs_box(x: f32, y: f32, w: f32, h: f32, r: f32, s: f32, color: Hsla) -> Div {
    div()
        .absolute()
        .left(px(x * s))
        .top(px(y * s))
        .w(px(w * s))
        .h(px(h * s))
        .rounded(px(r * s))
        .bg(color)
}

/// Little robot avatar drawn from divs, scaled from a 56 unit box.
#[allow(dead_code)]
pub fn avatar(color: Hsla, bulb: Hsla, size: Pixels, cx: &App) -> Div {
    let th = theme(cx);
    let s = f32::from(size) / 56.;
    let eye = |x: f32| abs_box(x - 2.8, 26.4 - 2.8, 5.6, 5.6, 2.8, s, th.fg);
    div()
        .relative()
        .size(size)
        .flex_none()
        .child(abs_box(28. - 1., 9.6, 2., 6.4, 0., s, color))
        .child(abs_box(10., 15., 36., 31., 12., s, color))
        .child(abs_box(15.5, 19.5, 25., 17., 8.5, s, th.stage))
        .child(eye(23.))
        .child(eye(33.))
        .child(abs_box(28. - 3.1, 7. - 3.1, 6.2, 6.2, 3.1, s, bulb))
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
#[allow(dead_code)]
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

/// Amber card holding a question, an answer input and its button.
#[allow(dead_code)]
pub fn question_card(text: &str, input: AnyElement, answer_btn: AnyElement, cx: &App) -> Div {
    let th = theme(cx);
    let head = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(svg().path("icons/hand.svg").size(px(15.)).text_color(th.wait.fg))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(th.wait.fg)
                .child(text.to_string()),
        );
    let row = div().flex().items_center().gap(px(8.)).child(div().flex_1().child(input)).child(answer_btn);
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
