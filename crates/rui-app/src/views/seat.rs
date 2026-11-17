//! Seat page.

use super::section::{Sec, section_top};
use super::{card, tag};
use crate::store::{self, Store};
use crate::theme::{Theme, theme};
use gpui::{App, Context, Div, Entity, FontWeight, Window, div, prelude::*, px};
use rui_core::SeatInfo;

/// Seat page: CLI account details.
pub struct SeatView {
    store: Entity<Store>,
}

impl SeatView {
    /// Build the seat view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self { store }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn kv_row(label: &str, value: String, th: &Theme) -> Div {
    div()
        .flex()
        .gap(px(10.))
        .child(div().w(px(120.)).flex_none().text_size(px(11.5)).text_color(th.muted).child(label.to_string()))
        .child(div().text_size(px(13.)).text_color(th.fg).child(value))
}

fn seat_header(seat: &SeatInfo, th: &Theme) -> Div {
    let badge = if seat.logged_in { tag(&th.run, "Signed in") } else { tag(&th.block, "Signed out") };
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(th.fg)
                .child("Claude Code CLI"),
        )
        .child(badge)
}

fn seat_rows(seat: &SeatInfo, th: &Theme) -> Div {
    let dash = || "—".to_string();
    let plan = seat.subscription_type.as_deref().map(capitalize).unwrap_or_else(dash);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(kv_row("Plan", plan, th))
        .child(kv_row("Auth", seat.auth_method.clone().unwrap_or_else(dash), th))
        .child(kv_row("Organization", seat.org_name.clone().unwrap_or_else(dash), th))
        .child(kv_row("Last model", seat.last_model.clone().unwrap_or_else(dash), th))
}

fn seat_card(seat: Option<&SeatInfo>, th: &Theme, cx: &App) -> Div {
    let base = card(cx).p(px(20.)).max_w(px(560.)).flex().flex_col().gap(px(10.));
    let Some(seat) = seat else {
        return base.child(div().text_color(th.muted).child("Checking the CLI account…"));
    };
    let err = seat
        .error
        .as_ref()
        .map(|e| div().text_size(px(12.)).text_color(th.block.fg).child(e.clone()));
    base.child(seat_header(seat, th)).child(seat_rows(seat, th)).children(err)
}

impl Render for SeatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let seat = self.store.read(cx).seat.clone();
        let note = div()
            .id("seat-note")
            .mt(px(14.))
            .text_size(px(11.5))
            .text_color(th.muted)
            .child("Rui switches seats at 90% use. More seats come later.");
        div()
            .id("seat-page")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .pr(px(40.))
            .pb(px(28.))
            .child(section_top(Sec::Cost, None, None, cx))
            .child(seat_card(seat.as_ref(), &th, cx))
            .child(note)
    }
}
