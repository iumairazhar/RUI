//! "Claude limit left" cards: one per seat, a bar per rate-limit window, when it resets and how fresh it is.

use crate::api::{LimitWindow, SeatLimits};
use crate::theme::{Status, Theme};
use crate::views::activity::local_offset;
use crate::views::card;
use gpui::{AnyElement, App, Div, FontWeight, div, prelude::*, px, relative};

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

/// The header pill for the first seat's 5-hour window: ("5h 12%", level).
pub fn pill(limits: &[SeatLimits]) -> Option<(String, Level)> {
    let w = limits.first()?.windows.iter().find(|w| w.name == "five_hour")?;
    Some((format!("5h {:.0}%", w.used_pct), level(w.used_pct)))
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

/// Name and "54% left" on one line, the bar, then when it resets.
fn window_row(w: &LimitWindow, now: i64, th: &Theme) -> Div {
    let name = div().flex_1().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(window_label(&w.name));
    let left = div().text_size(px(12.)).text_color(th.fg_2).child(format!("{:.0}% left", w.left_pct));
    let top = div().flex().items_center().gap(px(8.)).child(name).child(left);
    let reset = w.resets_at.map(|at| small(reset_text(at, now, local_offset()), th));
    div().flex().flex_col().gap(px(5.)).child(top).child(bar(w.used_pct, th)).children(reset)
}

/// Last seen, stale and overage lines.
fn foot(l: &SeatLimits, now: i64, th: &Theme) -> Vec<Div> {
    let mut out: Vec<Div> = l.seen_at.map(|at| small(format!("Last seen {}", ago(at, now)), th)).into_iter().collect();
    if l.stale {
        out.push(small("May be out of date".into(), th));
    }
    if l.overage.using {
        out.push(small(format!("Using overage \u{b7} {}", l.overage.status), th).text_color(th.wait.fg));
    }
    out
}

fn seat_card(l: &SeatLimits, now: i64, th: &Theme, cx: &App) -> AnyElement {
    let title = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child("Claude limit left");
    let label = (!l.label.is_empty()).then(|| small(l.label.clone(), th));
    let head = div().flex().items_baseline().justify_between().gap(px(8.)).child(title).children(label);
    let rows = l.windows.iter().map(|w| window_row(w, now, th));
    let c = card(cx).px(px(16.)).py(px(12.)).flex().flex_col().gap(px(10.)).max_w(px(560.));
    c.child(head).children(rows).children(foot(l, now, th)).into_any_element()
}

/// One card per seat, or the empty note.
pub fn cards(limits: &[SeatLimits], now: i64, th: &Theme, cx: &App) -> Vec<AnyElement> {
    if limits.is_empty() {
        return vec![small(EMPTY.into(), th).into_any_element()];
    }
    limits.iter().map(|l| seat_card(l, now, th, cx)).collect()
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
    fn ago_and_pill() {
        assert_eq!((ago(100, 130), ago(0, 300), ago(0, 7200), ago(0, 2 * DAY)), ("just now".into(), "5m ago".into(), "2h ago".into(), "2d ago".into()));
        let w = LimitWindow { name: "five_hour".into(), used_pct: 12.4, ..Default::default() };
        let l = SeatLimits { windows: vec![w], ..Default::default() };
        assert_eq!(pill(&[l]), Some(("5h 12%".to_string(), Level::Fine)));
        assert_eq!(pill(&[]), None);
    }
}
