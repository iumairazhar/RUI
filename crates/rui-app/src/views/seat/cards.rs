//! One card per seat: label, kind, active badge, usage bar with the 90% switch line, window and warnings.

use super::SeatView;
use crate::api::SeatStatus;
use crate::theme::Theme;
use crate::views::activity::local_offset;
use crate::views::library::danger_button;
use crate::views::{card, tag};
use gpui::{AnyElement, Context, Div, FontWeight, div, prelude::*, px, relative};

/// Share of the budget where Rui moves to the next seat.
pub const SWITCH_AT: f32 = 0.9;

/// How full the usage bar is, 0 to 1; `None` when the seat has no budget.
pub fn usage_fraction(s: &SeatStatus) -> Option<f32> {
    let pct = s.used_pct?;
    Some(((pct / 100.0) as f32).clamp(0.0, 1.0))
}

/// "$3.50 of $20.00 · 18%", or "$3.50 used · no limit".
pub fn usage_text(s: &SeatStatus) -> String {
    match (s.budget_usd.filter(|b| *b > 0.0), s.used_pct) {
        (Some(b), Some(p)) => format!("${:.2} of ${b:.2} \u{b7} {p:.0}%", s.used_usd),
        _ => format!("${:.2} used \u{b7} no limit", s.used_usd),
    }
}

/// "Subscription" or "API key".
pub fn kind_word(kind: &str) -> &'static str {
    if kind == "api_key" { "API key" } else { "Subscription" }
}

/// "Paused until 14:05", local time.
pub fn paused_text(until: i64, offset: i64) -> String {
    let secs = (until + offset).rem_euclid(86_400);
    format!("Paused until {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// An API key seat whose variable is not set can't run.
pub fn env_missing(s: &SeatStatus) -> bool {
    s.kind == "api_key" && !s.env_var_present
}

/// Track, fill, and a thin line at the switch point.
fn usage_bar(frac: f32, th: &Theme) -> Div {
    let color = if frac >= SWITCH_AT { th.block.fg } else { th.run.fg };
    let fill = div().absolute().left_0().top_0().h_full().w(relative(frac)).rounded(px(3.)).bg(color);
    let mark = div().absolute().top(px(-2.)).h(px(10.)).w(px(1.5)).left(relative(SWITCH_AT)).bg(th.fg.opacity(0.55));
    div().relative().w_full().h(px(6.)).rounded(px(3.)).bg(th.well).child(fill).child(mark)
}

fn small(text: String, th: &Theme) -> Div {
    div().text_size(px(11.5)).text_color(th.muted).child(text)
}

/// Label, kind chip and active badge on one line.
fn head(s: &SeatStatus, th: &Theme) -> Div {
    let label = div().flex_1().min_w_0().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(s.label.clone());
    let kind = tag(&th.think, kind_word(&s.kind));
    let active = s.active.then(|| tag(&th.done, "Active"));
    div().flex().items_center().gap(px(8.)).child(label).child(kind).children(active)
}

/// Window, pause and env var lines under the bar.
fn notes(s: &SeatStatus, th: &Theme) -> Vec<Div> {
    let mut out = vec![small(format!("{} \u{b7} {}-hour window", usage_text(s), s.window_hours), th)];
    out.extend(s.exhausted_until.map(|u| small(paused_text(u, local_offset()), th).text_color(th.wait.fg)));
    if env_missing(s) {
        let name = s.env_var.clone().unwrap_or_default();
        out.push(small(format!("Env var not set: {name}"), th).text_color(th.block.fg));
    }
    out
}

impl SeatView {
    /// One seat's card with its Delete button (a second click confirms).
    pub(super) fn seat_card(&self, i: usize, s: &SeatStatus, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let bar = usage_fraction(s).map(|f| usage_bar(f, th));
        let armed = self.confirm_delete.as_deref() == Some(s.id.as_str());
        let id = s.id.clone();
        let del = danger_button(format!("seat-delete-{i}"), if armed { "Click again to delete" } else { "Delete" }, true, th);
        let del = del.on_click(cx.listener(move |this, _, _, cx| this.delete_click(id.clone(), cx)));
        let foot = div().flex().justify_end().child(del);
        let c = card(cx).px(px(16.)).py(px(12.)).flex().flex_col().gap(px(8.)).max_w(px(560.));
        c.child(head(s, th)).children(bar).children(notes(s, th)).child(foot).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(kind: &str, budget: Option<f64>, used: f64, pct: Option<f64>, env: bool) -> SeatStatus {
        SeatStatus {
            id: "s".into(), kind: kind.into(), label: "S".into(), config_dir: None, env_var: Some("K".into()), budget_usd: budget,
            window_hours: 5, order: 0, used_usd: used, used_pct: pct, exhausted_until: None, active: false, env_var_present: env,
        }
    }

    #[test]
    fn usage_bar_fraction_is_clamped_and_none_without_budget() {
        assert_eq!(usage_fraction(&seat("subscription", Some(20.0), 5.0, Some(25.0), true)), Some(0.25));
        assert_eq!(usage_fraction(&seat("subscription", Some(20.0), 30.0, Some(150.0), true)), Some(1.0));
        assert_eq!(usage_fraction(&seat("subscription", None, 3.0, None, true)), None);
    }

    #[test]
    fn words_for_usage_kind_pause_and_env() {
        assert_eq!(usage_text(&seat("api_key", Some(20.0), 3.5, Some(17.5), true)), "$3.50 of $20.00 \u{b7} 18%");
        assert_eq!(usage_text(&seat("api_key", None, 1.0, None, true)), "$1.00 used \u{b7} no limit");
        assert_eq!((kind_word("api_key"), kind_word("subscription")), ("API key", "Subscription"));
        assert_eq!(paused_text(36_300, 0), "Paused until 10:05");
        assert!(env_missing(&seat("api_key", None, 0.0, None, false)));
        assert!(!env_missing(&seat("subscription", None, 0.0, None, false)));
    }
}
