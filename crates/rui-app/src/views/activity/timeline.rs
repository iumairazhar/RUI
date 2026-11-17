//! "Every thread, last four hours": one smooth line per thread moving between the four bands.

use super::{Col, col_of, mono, parse_secs};
use crate::theme::Theme;
use gpui::{
    AnyElement, Bounds, FontWeight, Hsla, Pixels, canvas, div, point, prelude::*, px, quad, relative,
    size, BorderStyle, PathBuilder,
};
use rui_core::{Event, Task, TaskStatus};
use serde_json::Value;
use std::sync::OnceLock;

/// The chart spans four hours; "now" sits 90% of the way across.
const WINDOW_SECS: i64 = 4 * 3600;
const BEFORE_NOW: f32 = 0.9;
const HEIGHT: f32 = 340.;
/// Plot starts this far in from the left and stops before the band labels.
const PLOT_LEFT: f32 = 18.;
const PLOT_RIGHT: f32 = 220.;
const LABEL_W: f32 = 186.;

/// One thread's line: where it has been, oldest first, in its client's color.
pub struct Line {
    pub pts: Vec<(i64, Col)>,
    pub color: Hsla,
}

/// Everything the chart needs, already computed.
pub struct Chart {
    pub lines: Vec<Line>,
    /// Cards in each band, in `ROWS` order.
    pub counts: [usize; 4],
    pub now: i64,
    pub clients: Vec<(String, Hsla)>,
}

/// Bands top to bottom.
pub const ROWS: [(Col, &str); 4] =
    [(Col::Landed, "Landed"), (Col::Doing, "Doing"), (Col::Standing, "Standing by"), (Col::Needs, "Needs you")];

fn row_of(c: Col) -> usize {
    ROWS.iter().position(|r| r.0 == c).unwrap_or(0)
}

fn row_y(i: usize) -> f32 {
    42. + 82. * i as f32
}

/// Band a task event moves its thread into, if it moves it at all.
pub fn event_col(kind: &str, payload: &Value) -> Option<Col> {
    match kind {
        "init" => Some(Col::Doing),
        "stuck" => Some(Col::Needs),
        "stopped" => Some(Col::Standing),
        "rollup" | "status" => payload["status"].as_str().and_then(TaskStatus::parse).and_then(|s| col_of(&s)),
        _ => None,
    }
}

/// A task's path: created standing by, each status-changing event, then where it is now, extended to now.
pub fn trajectory(task: &Task, events: &[Event], current: Col, now: i64) -> Vec<(i64, Col)> {
    let start = parse_secs(&task.created_at).unwrap_or(now);
    let mut moves: Vec<(i64, Col)> = events
        .iter()
        .filter(|e| e.task_id == Some(task.id))
        .filter_map(|e| Some((parse_secs(&e.created_at)?, event_col(&e.kind, &e.payload)?)))
        .collect();
    moves.sort_by_key(|m| m.0);
    let mut pts = vec![(start, Col::Standing)];
    for (t, c) in moves {
        if pts.last().map(|p| p.1) != Some(c) {
            pts.push((t.max(start), c));
        }
    }
    let last = pts.last().copied().unwrap_or((start, Col::Standing));
    if last.1 != current {
        let at = parse_secs(&task.updated_at).unwrap_or(now).clamp(last.0, now.max(last.0));
        pts.push((at, current));
    }
    pts.push((now.max(pts.last().map_or(now, |p| p.0)), current));
    pts
}

/// Cut a path to start at `from`: the band held at that moment, then what follows.
pub fn clip(pts: Vec<(i64, Col)>, from: i64) -> Vec<(i64, Col)> {
    let Some(i) = pts.iter().rposition(|p| p.0 <= from) else { return pts };
    let mut out = vec![(from, pts[i].1)];
    out.extend(pts[i + 1..].iter().copied());
    out
}

/// Draw a thread only if it moved inside the window or is live now; old, settled threads stay off the chart.
pub fn worth_drawing(pts: &[(i64, Col)], from: i64, current: Col) -> bool {
    if matches!(current, Col::Doing | Col::Needs) {
        return true;
    }
    pts.iter().take(pts.len().saturating_sub(1)).any(|p| p.0 > from)
}

/// Whole local hours inside the window, as (unix seconds, "HH:00").
pub fn hour_ticks(from: i64, to: i64, offset: i64) -> Vec<(i64, String)> {
    let first = (from + offset + 3599).div_euclid(3600) * 3600 - offset;
    let hours = (first..=to).step_by(3600);
    hours.map(|t| (t, format!("{:02}:00", (t + offset).rem_euclid(86_400) / 3600))).collect()
}

/// "+0100" or "-0530" as seconds east of UTC.
pub fn parse_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    let sign = match s.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let n = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse().ok() };
    Some(sign * (n(1, 3)? * 3600 + n(3, 5)? * 60))
}

/// Local UTC offset in seconds, read once from `date`; zero if that fails.
pub fn local_offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        out.and_then(|o| parse_offset(&String::from_utf8_lossy(&o.stdout))).unwrap_or(0)
    })
}

/// Seconds at the left edge of the window.
pub fn window_start(now: i64) -> i64 {
    now - (WINDOW_SECS as f32 * BEFORE_NOW) as i64
}

fn circle(window: &mut gpui::Window, x: f32, y: f32, r: f32, fill: Hsla, ring: Option<(f32, Hsla)>) {
    let bounds = Bounds { origin: point(px(x - r), px(y - r)), size: size(px(2. * r), px(2. * r)) };
    let (w, c) = ring.unwrap_or((0., fill));
    window.paint_quad(quad(bounds, px(r), fill, px(w), c, BorderStyle::default()));
}

/// One line as a stroked path: flat runs, S-curves where the band changes.
fn stroke_line(window: &mut gpui::Window, pts: &[(f32, f32)], color: Hsla) {
    let Some(first) = pts.first() else { return };
    let mut pb = PathBuilder::stroke(px(2.));
    pb.move_to(point(px(first.0), px(first.1)));
    for pair in pts.windows(2) {
        let ((x1, y1), (x2, y2)) = (pair[0], pair[1]);
        let to = point(px(x2), px(y2));
        if (y1 - y2).abs() < f32::EPSILON {
            pb.line_to(to);
            continue;
        }
        let k = (x2 - x1) * 0.5;
        pb.cubic_bezier_to(to, point(px(x1 + k), px(y1)), point(px(x2 - k), px(y2)));
    }
    if let Ok(path) = pb.build() {
        window.paint_path(path, color.opacity(0.82));
    }
}

fn paint_chart(b: Bounds<Pixels>, chart: &Chart, th: &Theme, window: &mut gpui::Window) {
    let (left, top) = (f32::from(b.left()) + PLOT_LEFT, f32::from(b.top()));
    let width = f32::from(b.size.width) - PLOT_LEFT - PLOT_RIGHT;
    let from = window_start(chart.now);
    let x_of = |t: i64| left + width * (t - from) as f32 / WINDOW_SECS as f32;
    let ov = if th.dark { gpui::white() } else { gpui::black() };
    for i in 0..4 {
        let y = top + row_y(i);
        if i % 2 == 0 {
            let zone = Bounds { origin: point(px(left), px(y - 41.)), size: size(px(width), px(82.)) };
            window.paint_quad(quad(zone, px(10.), ov.opacity(0.028), px(0.), th.hairline, BorderStyle::default()));
        }
        let guide = Bounds { origin: point(px(left), px(y)), size: size(px(width), px(1.)) };
        window.paint_quad(gpui::fill(guide, th.hairline));
    }
    let now_x = x_of(chart.now);
    let mut dashed = PathBuilder::stroke(px(1.)).dash_array(&[px(3.), px(4.)]);
    dashed.move_to(point(px(now_x), px(top + 26.)));
    dashed.line_to(point(px(now_x), px(top + 306.)));
    if let Ok(path) = dashed.build() {
        window.paint_path(path, th.accent_text.opacity(0.7));
    }
    paint_lines(window, chart, th, (top, &x_of));
}

fn paint_lines(window: &mut gpui::Window, chart: &Chart, th: &Theme, (top, x_of): (f32, &dyn Fn(i64) -> f32)) {
    let n = chart.lines.len();
    for (i, line) in chart.lines.iter().enumerate() {
        let spread = if n > 1 { (i as f32 / (n - 1) as f32 - 0.5) * 42. } else { 0. };
        let pts: Vec<(f32, f32)> = line.pts.iter().map(|(t, c)| (x_of(*t), top + row_y(row_of(*c)) + spread)).collect();
        stroke_line(window, &pts, line.color);
        let inner = pts.len().saturating_sub(1);
        for p in pts.iter().take(inner).skip(1) {
            circle(window, p.0, p.1, 3.4, th.stage, Some((2., line.color)));
        }
        if let Some(end) = pts.last() {
            circle(window, end.0, end.1, 7.5, th.stage, None);
            circle(window, end.0, end.1, 5., line.color, None);
        }
    }
}

fn legend(clients: &[(String, Hsla)], th: &Theme) -> AnyElement {
    let keys = clients.iter().map(|(name, color)| {
        let dot = div().size(px(8.)).rounded_full().bg(*color);
        let text = div().text_size(px(11.)).text_color(th.muted).child(name.clone());
        div().flex().items_center().gap(px(6.)).child(dot).child(text)
    });
    div().flex().flex_wrap().justify_end().gap(px(12.)).children(keys).into_any_element()
}

fn band_label(i: usize, count: usize, th: &Theme) -> AnyElement {
    let name = div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(ROWS[i].1);
    let num = mono(13., th.fg, count.to_string()).font_weight(FontWeight::BOLD);
    div()
        .id(("tl-band", i))
        .absolute()
        .right(px(12.))
        .top(px(row_y(i) - 17.))
        .w(px(LABEL_W))
        .h(px(30.))
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(th.hairline)
        .child(name)
        .child(num)
        .into_any_element()
}

fn tick(frac: f32, text: String, th: &Theme) -> AnyElement {
    let label = mono(10., th.muted, text);
    let at = div().absolute().left(relative(frac)).top(px(HEIGHT - 24.)).ml(px(-22.)).w(px(44.));
    at.flex().justify_center().child(label).into_any_element()
}

fn now_label(th: &Theme) -> AnyElement {
    let text = div().text_size(px(9.5)).font_weight(FontWeight::BOLD).text_color(th.accent_text).child("NOW");
    div().absolute().left(relative(BEFORE_NOW)).top(px(4.)).ml(px(6.)).child(text).into_any_element()
}

/// The whole section: title and client legend, then the plot.
pub fn view(chart: Chart, th: &Theme) -> AnyElement {
    let title = div().flex_1().min_w_0().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg);
    let head = div().flex().items_end().gap(px(16.)).mb(px(8.));
    let head = head.child(title.child("Every thread, last four hours")).child(legend(&chart.clients, th));
    let from = window_start(chart.now);
    let ticks = hour_ticks(from, from + WINDOW_SECS, local_offset());
    let ticks = ticks.into_iter().map(|(t, text)| tick((t - from) as f32 / WINDOW_SECS as f32, text, th));
    let axis = div().absolute().top_0().bottom_0().left(px(PLOT_LEFT)).right(px(PLOT_RIGHT));
    let axis = axis.child(now_label(th)).children(ticks);
    let bands: Vec<AnyElement> = (0..4).map(|i| band_label(i, chart.counts[i], th)).collect();
    let theme = th.clone();
    let paint = canvas(|_, _, _| (), move |b, _, window, _| paint_chart(b, &chart, &theme, window));
    let plot = div().id("timeline-plot").relative().w_full().h(px(HEIGHT));
    let plot = plot.child(paint.absolute().top_0().left_0().size_full()).child(axis).children(bands);
    div().id("threads-timeline").flex_none().pt(px(16.)).mr(px(28.)).child(head).child(plot).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(status: &str) -> Task {
        serde_json::from_value(json!({"id":1,"client_id":1,"parent_id":null,"title":"t","body":"",
            "status":status,"done_command":null,"max_loops":5,"loops_used":0,"role":"builder",
            "branch":null,"source_kind":"user","source_ref":null,"depends_on":[],
            "created_at":"2026-11-15 10:00:00","updated_at":"2026-11-15 12:00:00"}))
        .unwrap()
    }

    fn ev(kind: &str, payload: Value, at: &str) -> Event {
        Event { id: 1, task_id: Some(1), chat_id: None, agent: "a".into(), kind: kind.into(), payload, created_at: at.into() }
    }

    #[test]
    fn events_map_to_bands() {
        assert_eq!(event_col("init", &json!({})), Some(Col::Doing));
        assert_eq!(event_col("rollup", &json!({"status":"done"})), Some(Col::Landed));
        assert_eq!(event_col("status", &json!({"status":"waiting_answer"})), Some(Col::Needs));
        assert_eq!(event_col("stuck", &json!({})), Some(Col::Needs));
        assert_eq!(event_col("assistant", &json!({})), None);
        assert_eq!(event_col("rollup", &json!({"status":"split"})), None);
    }

    #[test]
    fn path_follows_events_and_ends_now() {
        let base = parse_secs("2026-11-15 10:00:00").unwrap();
        let events = vec![ev("init", json!({}), "2026-11-15T10:30:00Z"), ev("rollup", json!({"status":"done"}), "2026-11-15T11:00:00Z")];
        let pts = trajectory(&task("done"), &events, Col::Landed, base + 7200);
        let bands: Vec<Col> = pts.iter().map(|p| p.1).collect();
        assert_eq!(bands, vec![Col::Standing, Col::Doing, Col::Landed, Col::Landed]);
        assert_eq!(pts[1].0, base + 1800);
        assert_eq!(pts.last().unwrap().0, base + 7200);
    }

    #[test]
    fn path_without_events_jumps_to_current_band() {
        let pts = trajectory(&task("in_progress"), &[], Col::Doing, parse_secs("2026-11-15 13:00:00").unwrap());
        assert_eq!(pts.iter().map(|p| p.1).collect::<Vec<_>>(), vec![Col::Standing, Col::Doing, Col::Doing]);
    }

    #[test]
    fn clip_holds_the_band_at_the_left_edge() {
        let pts = vec![(0, Col::Standing), (100, Col::Doing), (300, Col::Landed)];
        assert_eq!(clip(pts.clone(), 150), vec![(150, Col::Doing), (300, Col::Landed)]);
        assert_eq!(clip(pts.clone(), -5), pts);
    }

    #[test]
    fn only_recent_or_live_threads_are_drawn() {
        let old = vec![(100, Col::Landed), (500, Col::Landed)];
        assert!(!worth_drawing(&old, 100, Col::Landed));
        assert!(worth_drawing(&old, 100, Col::Doing));
        let moved = vec![(100, Col::Standing), (300, Col::Landed), (500, Col::Landed)];
        assert!(worth_drawing(&moved, 100, Col::Landed));
        let new = vec![(200, Col::Standing), (500, Col::Standing)];
        assert!(worth_drawing(&new, 100, Col::Standing));
    }

    #[test]
    fn ticks_are_whole_local_hours() {
        let ticks = hour_ticks(1800, 1800 + 4 * 3600, 3600);
        let labels: Vec<&str> = ticks.iter().map(|t| t.1.as_str()).collect();
        assert_eq!(labels, ["02:00", "03:00", "04:00", "05:00"]);
        assert_eq!(ticks[0].0, 3600);
    }

    #[test]
    fn offsets_parse() {
        assert_eq!(parse_offset("+0100\n"), Some(3600));
        assert_eq!(parse_offset("-0530"), Some(-19_800));
        assert_eq!(parse_offset("x"), None);
    }
}
