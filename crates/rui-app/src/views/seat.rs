//! Seat label and seat page.

use crate::store::{self, Store};
use gpui::{Context, Entity, SharedString, Window, div, prelude::*, px};
use rui_core::SeatInfo;

/// Minimal seat page: just the seat description for now.
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

impl Render for SeatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.store.read(cx).seat.as_ref().map(seat_label).unwrap_or_default();
        div().size_full().p(px(14.)).child(SharedString::from(text))
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
