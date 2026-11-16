//! Small glass toast at the bottom center: a title plus chips.

use super::motion::{self, ease, spring};
use crate::theme::Theme;
use gpui::{AnimationExt, AnyElement, FontWeight, IntoElement, SharedString, div, prelude::*, px};

/// Toast springs in over this long.
const IN_MS: u64 = 520;
/// Toast hides after this long.
pub const SHOW_MS: u64 = 4200;

/// One toast message; `seq` makes each new toast replay its entrance.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub seq: usize,
    pub title: String,
    pub chips: Vec<String>,
}

fn chip(text: &str, th: &Theme) -> impl IntoElement {
    let bg = th.fg.opacity(0.12);
    let el = div().px(px(8.)).py(px(3.)).rounded(px(6.)).bg(bg).text_color(th.fg_2).text_size(px(11.));
    el.child(text.to_string())
}

fn pip(th: &Theme) -> impl IntoElement {
    let dot = div().size(px(8.)).rounded_full().bg(th.run.fg);
    let ring = div().size(px(22.)).flex_none().rounded_full().flex().items_center().justify_center();
    ring.bg(th.run.dim).border_1().border_color(th.run.rim).child(dot)
}

fn card(t: &Toast, th: &Theme) -> impl IntoElement {
    let title = div().font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(t.title.clone());
    let chips = div().flex().flex_wrap().gap(px(6.)).children(t.chips.iter().map(|c| chip(c, th)));
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .py(px(12.))
        .max_w(px(720.))
        .rounded(px(15.))
        .bg(th.ctl.opacity(0.92))
        .border_1()
        .border_color(th.glass_rim)
        .shadow(th.card_shadow())
        .text_size(px(12.5))
        .child(pip(th))
        .child(title)
        .child(chips)
}

/// Bottom-centered toast that rises 16 px with the spring and fades in.
pub fn toast(t: &Toast, th: &Theme) -> AnyElement {
    let wrap = div().absolute().left_0().right_0().bottom(px(24.)).flex().justify_center().child(card(t, th));
    let id = (SharedString::from("toast"), t.seq);
    wrap.with_animation(id, motion::once(IN_MS), |el, d| {
        el.bottom(px(24. - 16. * (1. - spring(d)))).opacity(ease(d))
    })
    .into_any_element()
}
