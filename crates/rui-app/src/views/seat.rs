//! Seat label and seat page.

use super::{card, page_head, tag};
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
            .child(page_head("ACCOUNTS", "Seats", cx))
            .child(seat_card(seat.as_ref(), &th, cx))
            .child(note)
    }
}

fn subscription_name(seat: &SeatInfo) -> String {
    match seat.subscription_type.as_deref() {
        Some(t) if !t.is_empty() => {
            let mut chars = t.chars();
            let first = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            format!("Claude {first}{}", chars.as_str())
        }
        _ => seat.auth_method.clone().unwrap_or_else(|| "Unknown seat".into()),
    }
}

/// One-line description of the seat, e.g. `Claude Max · me@example.com · Model: claude-opus-5-5`.
pub fn seat_label(seat: &SeatInfo) -> String {
    if !seat.logged_in {
        return match &seat.error {
            Some(e) => format!("Not signed in · {e}"),
            None => "Not signed in".into(),
        };
    }
    let mut parts = vec![subscription_name(seat)];
    parts.extend(seat.email.clone());
    parts.push(format!("Model: {}", seat.last_model.as_deref().unwrap_or("—")));
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat() -> SeatInfo {
        SeatInfo {
            logged_in: true,
            auth_method: Some("claude.ai".into()),
            email: Some("me@example.com".into()),
            subscription_type: Some("max".into()),
            last_model: Some("claude-opus-5-5".into()),
            ..Default::default()
        }
    }

    #[test]
    fn full_label() {
        assert_eq!(seat_label(&seat()), "Claude Max · me@example.com · Model: claude-opus-5-5");
    }

    #[test]
    fn pro_and_missing_model() {
        let s = SeatInfo { subscription_type: Some("pro".into()), last_model: None, ..seat() };
        assert_eq!(seat_label(&s), "Claude Pro · me@example.com · Model: —");
    }

    #[test]
    fn falls_back_to_auth_method_then_unknown() {
        let s = SeatInfo { subscription_type: None, email: None, ..seat() };
        assert_eq!(seat_label(&s), "claude.ai · Model: claude-opus-5-5");
        let s = SeatInfo { subscription_type: None, auth_method: None, ..seat() };
        assert!(seat_label(&s).starts_with("Unknown seat"));
    }

    #[test]
    fn not_signed_in() {
        let s = SeatInfo::default();
        assert_eq!(seat_label(&s), "Not signed in");
        let s = SeatInfo { error: Some("boom".into()), ..SeatInfo::default() };
        assert_eq!(seat_label(&s), "Not signed in · boom");
    }
}
