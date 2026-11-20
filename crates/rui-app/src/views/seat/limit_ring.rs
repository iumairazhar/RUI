//! "Limit left" pill in the title bar: a ring with the 5-hour window's share left.

use super::limits::{Level, clock, level, level_status, window_label};
use crate::theme::Theme;
use gpui::{AnyElement, Bounds, FontWeight, Hsla, PathBuilder, Pixels, Window, canvas, div, point, prelude::*, px, svg};
use rui_core::{LimitWindow, SeatInfo};

const RING: f32 = 24.;
const STROKE: f32 = 2.5;

/// What the ring shows, worked out from the newest limit window.
#[derive(Debug, Clone, PartialEq)]
pub struct RingInfo {
    /// 0 to 100 left; `None` once the window has reset and no fresh number came.
    pub left_pct: Option<f64>,
    pub level: Level,
    /// "5-hour limit left" or "5-hour limit reset".
    pub title: String,
    /// "resets 16:30" or "reset at 11:30, new numbers after the next run".
    pub detail: String,
}

fn main_window(seat: &SeatInfo) -> Option<&LimitWindow> {
    seat.limits.iter().find(|w| w.name == "five_hour").or(seat.limits.first())
}

/// The ring's numbers; a window whose reset time has passed counts as reset, not as its old usage.
pub fn info(seat: &SeatInfo, now: i64, offset: i64) -> Option<RingInfo> {
    let w = main_window(seat)?;
    let label = window_label(&w.name);
    if let Some(at) = w.resets_at.filter(|at| *at <= now) {
        let detail = format!("reset at {}, new numbers after the next run", clock(at, offset));
        return Some(RingInfo { left_pct: None, level: Level::Fine, title: format!("{label} limit reset"), detail });
    }
    let used = w.used_pct?;
    let detail = w.resets_at.map(|at| format!("resets {}", clock(at, offset))).unwrap_or_default();
    let left = (100.0 - used).clamp(0.0, 100.0);
    Some(RingInfo { left_pct: Some(left), level: level(used), title: format!("{label} limit left"), detail })
}

/// Show unless dismissed at this level or a worse one; a worse level brings it back.
pub fn visible(info: &RingInfo, dismissed: Option<Level>) -> bool {
    dismissed.is_none_or(|d| info.level > d)
}

fn arc(window: &mut Window, b: Bounds<Pixels>, frac: f32, color: Hsla) {
    let (cx, cy) = (f32::from(b.center().x), f32::from(b.center().y));
    let r = RING / 2. - STROKE;
    let steps = (64. * frac).ceil().max(1.) as usize;
    let mut pb = PathBuilder::stroke(px(STROKE));
    for i in 0..=steps {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * frac * i as f32 / steps as f32;
        let p = point(px(cx + r * a.cos()), px(cy + r * a.sin()));
        if i == 0 { pb.move_to(p) } else { pb.line_to(p) }
    }
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

/// The ring: a faint full track and a colored arc for the share left, the number inside.
pub fn ring(info: &RingInfo, th: &Theme) -> impl IntoElement + use<> {
    let frac = info.left_pct.map_or(1., |p| (p / 100.) as f32);
    let color = info.left_pct.map_or(th.muted, |_| level_status(info.level, th).fg);
    let track = th.fg.opacity(0.12);
    let paint = canvas(|_, _, _| (), move |b, _, window, _| {
        arc(window, b, 1., track);
        if frac > 0. {
            arc(window, b, frac, color);
        }
    });
    let label = info.left_pct.map_or("↺".to_string(), |p| format!("{p:.0}"));
    let text = div().absolute().size_full().flex().items_center().justify_center().text_size(px(8.5)).font_weight(FontWeight::BOLD).text_color(th.fg).child(label);
    div().relative().size(px(RING)).flex_none().child(paint.absolute().size_full()).child(text)
}

/// The notice card's body; the caller adds the open and dismiss handlers.
pub fn body(info: &RingInfo, th: &Theme) -> (gpui::Div, gpui::Stateful<gpui::Div>) {
    let title = div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(info.title.clone());
    let detail = div().text_size(px(11.)).text_color(th.muted).child(info.detail.clone());
    let text = div().flex().items_baseline().gap(px(6.)).whitespace_nowrap().child(title).child(detail);
    let close = svg().path("icons/close.svg").size(px(11.)).text_color(th.muted);
    let dismiss = div().id("limit-ring-dismiss").p(px(4.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(th.fg.opacity(0.08))).child(close);
    let row = div().flex().items_center().gap(px(8.)).child(ring(info, th)).child(text);
    (row, dismiss)
}

/// Wrap the row and the dismiss button into the pill at the title bar's right end.
pub fn card(row: AnyElement, dismiss: AnyElement, th: &Theme) -> gpui::Div {
    let shell = div()
        .flex()
        .items_center()
        .gap(px(4.))
        .pl(px(5.))
        .pr(px(4.))
        .py(px(3.))
        .rounded(px(16.))
        .bg(th.ctl.opacity(0.95))
        .border_1()
        .border_color(th.glass_rim)
        .child(row)
        .child(dismiss);
    div().absolute().right(px(16.)).top(px(8.)).child(shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(used: f64, resets_at: i64) -> SeatInfo {
        let w = LimitWindow { name: "five_hour".into(), used_pct: Some(used), resets_at: Some(resets_at), status: None };
        SeatInfo { limits: vec![w], ..Default::default() }
    }

    #[test]
    fn shows_share_left_before_reset() {
        let i = info(&seat(53.0, 2 * 3600), 3600, 0).unwrap();
        assert_eq!(i.left_pct, Some(47.0));
        assert_eq!(i.title, "5-hour limit left");
        assert_eq!(i.detail, "resets 02:00");
    }

    #[test]
    fn passed_reset_drops_the_old_number() {
        let i = info(&seat(53.0, 3600), 7200, 0).unwrap();
        assert_eq!(i.left_pct, None);
        assert_eq!(i.title, "5-hour limit reset");
        assert_eq!(i.detail, "reset at 01:00, new numbers after the next run");
    }

    #[test]
    fn dismissed_stays_hidden_until_it_gets_worse() {
        let fine = info(&seat(10.0, 9999), 0, 0).unwrap();
        let close = info(&seat(75.0, 9999), 0, 0).unwrap();
        assert!(visible(&fine, None));
        assert!(!visible(&fine, Some(Level::Fine)));
        assert!(visible(&close, Some(Level::Fine)));
        assert!(!visible(&close, Some(Level::Close)));
    }

    #[test]
    fn no_limits_no_ring() {
        assert_eq!(info(&SeatInfo::default(), 0, 0), None);
    }
}
