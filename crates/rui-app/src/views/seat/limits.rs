//! "Claude limit left" card: a bar per rate-limit window, when it resets and how fresh it is.

use crate::theme::{Status, Theme};
use crate::views::activity::local_offset;
use crate::views::card;
use gpui::{AnyElement, App, Div, FontWeight, div, prelude::*, px, relative};
use rui_core::{LimitWindow, SeatInfo};

const EMPTY: &str = "No limit data yet. It appears after the next agent run.";
const DAY: i64 = 86_400;

/// How close a window is to its limit; picks the bar color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Level {
    Fine,
    Close,
    Hit,
}

/// Under 70% used is fine, 70 to 90 is close, 90 and up is hit.
pub fn level(used_pct: f64) -> Level {
    match used_pct {
        p if p >= 90.0 => Level::Hit,
        p if p >= 70.0 => Level::Close,
        _ => Level::Fine,
    }
}

/// The theme status set for a level.
pub fn level_status(l: Level, th: &Theme) -> &Status {
    match l {
        Level::Fine => &th.done,
        Level::Close => &th.wait,
        Level::Hit => &th.block,
    }
}

/// "5-hour", "Weekly", or the raw name.
pub fn window_label(name: &str) -> String {
    match name {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        other => other.into(),
    }
}

/// "3h 12m" or "12m".
fn span(secs: i64) -> String {
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

/// "resets in 3h 12m" within a day, else "resets at Mon 14:00" in local time.
pub fn reset_text(at: i64, now: i64, offset: i64) -> String {
    let left = at - now;
    if left <= 0 {
        return "resets now".into();
    }
    if left <= DAY {
        return format!("resets in {}", span(left));
    }
    let local = at + offset;
    let days = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let day = days[local.div_euclid(DAY).rem_euclid(7) as usize];
    let secs = local.rem_euclid(DAY);
    format!("resets at {day} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// "just now", "5m ago", "3h ago" or "2d ago".
pub fn ago(at: i64, now: i64) -> String {
    match (now - at).max(0) {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < DAY => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / DAY),
    }
}

/// "14:00" in local time.
pub fn clock(at: i64, offset: i64) -> String {
    let secs = (at + offset).rem_euclid(DAY);
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// "62% of 5-hour limit used, resets 14:00" for the title bar; the 5-hour window first.
pub fn summary(seat: &SeatInfo, offset: i64) -> Option<String> {
    let w = seat.limits.iter().find(|w| w.name == "five_hour").or(seat.limits.first())?;
    let label = window_label(&w.name);
    let reset = w.resets_at.map(|at| format!(", resets {}", clock(at, offset))).unwrap_or_default();
    match (w.used_pct, w.status.as_deref()) {
        (Some(p), _) => Some(format!("{p:.0}% of {label} limit used{reset}")),
        (None, Some(st)) => Some(format!("{label} limit {st}{reset}")),
        (None, None) => None,
    }
}

fn small(text: String, th: &Theme) -> Div {
    div().text_size(px(11.5)).text_color(th.muted).child(text)
}

fn bar(used_pct: f64, th: &Theme) -> Div {
    let frac = ((used_pct / 100.0) as f32).clamp(0.0, 1.0);
    let color = level_status(level(used_pct), th).fg;
    let fill = div().absolute().left_0().top_0().h_full().w(relative(frac)).rounded(px(3.)).bg(color);
    div().relative().w_full().h(px(6.)).rounded(px(3.)).bg(th.well).child(fill)
}

/// "42% used" with the status as a fallback when no utilization came.
fn used_text(w: &LimitWindow) -> String {
    match (w.used_pct, w.status.as_deref()) {
        (Some(p), _) => format!("{p:.0}% used"),
        (None, Some(st)) => st.to_string(),
        (None, None) => "no data".to_string(),
    }
}

/// Name and "42% used" on one line, the bar, then when it resets.
fn window_row(w: &LimitWindow, now: i64, th: &Theme) -> Div {
    let name = div().flex_1().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(window_label(&w.name));
    let used = div().text_size(px(12.)).text_color(th.fg_2).child(used_text(w));
    let top = div().flex().items_center().gap(px(8.)).child(name).child(used);
    let reset = w.resets_at.map(|at| small(format!("{} ({})", reset_text(at, now, local_offset()), clock(at, local_offset())), th));
    let bar = w.used_pct.map(|p| bar(p, th));
    div().flex().flex_col().gap(px(5.)).child(top).children(bar).children(reset)
}

/// The one card, or the empty note.
pub fn cards(seat: Option<&SeatInfo>, now: i64, th: &Theme, cx: &App) -> Vec<AnyElement> {
    let Some(seat) = seat.filter(|s| !s.limits.is_empty()) else {
        return vec![small(EMPTY.into(), th).into_any_element()];
    };
    let title = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child("Claude limit left");
    let seen = seat.limits_seen_at.map(|at| small(format!("Last seen {}", ago(at, now)), th));
    let head = div().flex().items_baseline().justify_between().gap(px(8.)).child(title).children(seen);
    let rows = seat.limits.iter().map(|w| window_row(w, now, th));
    let c = card(cx).px(px(16.)).py(px(12.)).flex().flex_col().gap(px(10.)).max_w(px(560.));
    vec![c.child(head).children(rows).into_any_element()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_names_read_plainly() {
        assert_eq!(window_label("five_hour"), "5-hour");
        assert_eq!(window_label("seven_day"), "Weekly");
        assert_eq!(window_label("seven_day_opus"), "seven_day_opus");
    }

    #[test]
    fn reset_text_is_relative_within_a_day() {
        assert_eq!(reset_text(1000 + 3 * 3600 + 12 * 60, 1000, 0), "resets in 3h 12m");
        assert_eq!(reset_text(1000 + 5 * 60, 1000, 0), "resets in 5m");
        assert_eq!(reset_text(900, 1000, 0), "resets now");
        // 1970-01-05 is a Monday; 14:00 UTC, two days after "now".
        let mon = 4 * DAY + 14 * 3600;
        assert_eq!(reset_text(mon, mon - 2 * DAY, 0), "resets at Mon 14:00");
        assert_eq!(reset_text(mon, mon - 2 * DAY, 3600), "resets at Mon 15:00");
    }

    #[test]
    fn bar_level_thresholds() {
        assert_eq!((level(0.0), level(69.9)), (Level::Fine, Level::Fine));
        assert_eq!((level(70.0), level(89.9)), (Level::Close, Level::Close));
        assert_eq!((level(90.0), level(120.0)), (Level::Hit, Level::Hit));
    }

    #[test]
    fn ago_text() {
        assert_eq!((ago(100, 130), ago(0, 300), ago(0, 7200), ago(0, 2 * DAY)), ("just now".into(), "5m ago".into(), "2h ago".into(), "2d ago".into()));
    }

    fn seat_with(w: LimitWindow) -> SeatInfo {
        SeatInfo { limits: vec![w], ..Default::default() }
    }

    #[test]
    fn title_bar_summary_text() {
        let w = LimitWindow { name: "five_hour".into(), used_pct: Some(62.4), resets_at: Some(14 * 3600), status: None };
        assert_eq!(summary(&seat_with(w), 0).as_deref(), Some("62% of 5-hour limit used, resets 14:00"));
        let w = LimitWindow { name: "five_hour".into(), used_pct: None, resets_at: Some(14 * 3600), status: Some("allowed".into()) };
        assert_eq!(summary(&seat_with(w), 3600).as_deref(), Some("5-hour limit allowed, resets 15:00"));
        assert_eq!(summary(&SeatInfo::default(), 0), None);
    }

    #[test]
    fn row_text_falls_back_to_status() {
        let w = LimitWindow { name: "x".into(), used_pct: Some(42.0), ..Default::default() };
        assert_eq!(used_text(&w), "42% used");
        let w = LimitWindow { status: Some("rejected".into()), ..Default::default() };
        assert_eq!(used_text(&w), "rejected");
    }
}
