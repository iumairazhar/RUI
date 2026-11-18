//! The Flow tab: a thread's nodes in three lanes, left to right by start time, with a now line.

use super::{ThreadsView, loop_dots, mono};
use crate::api::{Node, Thread};
use crate::theme::{Status, Theme};
use crate::views::roster::Loops;
use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};

/// Lanes top to bottom: daemon lane key and its label.
pub const LANES: [(&str, &str); 3] = [("in", "Came in"), ("work", "Agent work"), ("out", "Went out")];
const LABEL_W: f32 = 206.;
const LANE_H: f32 = 86.;
const COL_W: f32 = 196.;
const CHIP_W: f32 = 180.;
/// Room above the lanes for the "now" label.
const TOP: f32 = 18.;

/// A node's slot on the chart: lane row, column, and whether it is still planned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub node: usize,
    pub lane: usize,
    pub col: usize,
    pub planned: bool,
}

/// A planned node has not started; it sits after the now line.
pub fn is_planned(n: &Node) -> bool {
    n.status == "planned" || n.started_at.is_empty()
}

/// Lane row for a daemon lane key; unknown lanes read as agent work.
pub fn lane_row(lane: &str) -> usize {
    LANES.iter().position(|(k, _)| *k == lane).unwrap_or(1)
}

/// Started nodes by `started_at`, then planned ones in daemon order; plus the now column.
pub fn layout(nodes: &[Node]) -> (Vec<Slot>, usize) {
    let mut started: Vec<usize> = (0..nodes.len()).filter(|&i| !is_planned(&nodes[i])).collect();
    started.sort_by(|&a, &b| nodes[a].started_at.cmp(&nodes[b].started_at));
    let now = started.len();
    let planned = (0..nodes.len()).filter(|&i| is_planned(&nodes[i]));
    let order: Vec<usize> = started.into_iter().chain(planned).collect();
    let slot = |(col, &i): (usize, &usize)| Slot { node: i, lane: lane_row(&nodes[i].lane), col, planned: col >= now };
    (order.iter().enumerate().map(slot).collect(), now)
}

/// Colour family for a node status (see the mapping table in docs/thread-view-spec.md).
pub fn node_style<'a>(status: &str, th: &'a Theme) -> &'a Status {
    match status {
        "running" => &th.run,
        "needs_you" | "failed" => &th.block,
        "owner" => &th.think,
        "done" => &th.done,
        _ => &th.wait,
    }
}

impl ThreadsView {
    /// The Flow tab body: lane labels on the left, the scrolling track on the right.
    pub(super) fn flow(&self, thread: Option<&Thread>, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(t) = thread else {
            return div().text_size(px(12.5)).text_color(th.muted).child("Loading the flow\u{2026}").into_any_element();
        };
        let labels = LANES.iter().map(|(_, name)| lane_label(name, th));
        let labels = div().flex().flex_col().flex_none().w(px(LABEL_W)).pt(px(TOP)).children(labels);
        let track = div().id("flow-track").flex_1().min_w_0().overflow_x_scroll().child(self.track(t, th, cx));
        div().id("flow").flex().child(labels).child(track).into_any_element()
    }

    fn track(&self, t: &Thread, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (slots, now) = layout(&t.nodes);
        let cols = slots.len().max(now + 1);
        let area = div().relative().w(px(cols as f32 * COL_W + 24.)).h(px(TOP + LANE_H * 3.));
        let rules = (1..3).map(|i| lane_rule(i, th));
        let chips: Vec<AnyElement> = slots.iter().map(|s| self.placed_chip(t, s, th, cx)).collect();
        area.children(rules).child(now_line(now, th)).children(chips).into_any_element()
    }

    fn placed_chip(&self, t: &Thread, s: &Slot, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let n = &t.nodes[s.node];
        let (tid, nid) = (t.id, n.id.clone());
        let chip = node_chip(s.node, n, th).opacity(if s.planned { 0.45 } else { 1. });
        let chip = chip.on_click(cx.listener(move |this, _, _, cx| this.inspect(tid, nid.clone(), cx)));
        let at = div().absolute().left(px(s.col as f32 * COL_W + 8.)).top(px(TOP + s.lane as f32 * LANE_H + 8.));
        at.child(chip).into_any_element()
    }
}

fn lane_label(name: &str, th: &Theme) -> AnyElement {
    let text = div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(name.to_string());
    div().flex().items_center().h(px(LANE_H)).child(text).into_any_element()
}

/// Thin line between two lanes.
fn lane_rule(i: usize, th: &Theme) -> AnyElement {
    let y = TOP + i as f32 * LANE_H;
    div().absolute().left_0().right_0().top(px(y)).h(px(1.)).bg(th.hairline).into_any_element()
}

/// The 1.5 px "now" line between started work and planned work.
fn now_line(now: usize, th: &Theme) -> AnyElement {
    let x = now as f32 * COL_W + 4.;
    let label = mono(10., th.fg.opacity(0.55), "now");
    let line = div().w(px(1.5)).flex_1().bg(th.fg.opacity(0.35));
    let col = div().absolute().top_0().bottom_0().left(px(x - 10.)).w(px(20.)).flex().flex_col().items_center();
    col.child(label).child(line).into_any_element()
}

/// Node chip: title, agent, loop dots and output label, coloured by status.
fn node_chip(i: usize, n: &Node, th: &Theme) -> gpui::Stateful<gpui::Div> {
    let st = node_style(&n.status, th);
    let title = div().truncate().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(n.title.clone());
    let agent = n.agent.clone().map(|a| mono(10.5, th.muted, a));
    let dots = n.attempts.as_ref().filter(|a| a.max > 0).map(|a| loop_dots(Loops { used: a.used, max: a.max }, 5., th));
    let meta = div().flex().items_center().gap(px(6.)).children(agent).children(dots);
    let out = n.output.as_ref().map(|o| div().truncate().text_size(px(10.5)).text_color(st.fg).child(o.label.clone()));
    let chip = div().id(("flow-node", i)).flex().flex_col().gap(px(3.)).w(px(CHIP_W)).px(px(10.)).py(px(7.));
    let chip = chip.rounded(px(9.)).bg(st.dim).border_1().border_color(st.rim).cursor_pointer();
    let rim = st.base;
    chip.hover(move |s| s.border_color(rim)).child(title).child(meta).children(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, lane: &str, status: &str, at: &str) -> Node {
        Node { id: id.into(), lane: lane.into(), status: status.into(), started_at: at.into(), ..Default::default() }
    }

    #[test]
    fn lanes_map_to_rows() {
        assert_eq!((lane_row("in"), lane_row("work"), lane_row("out"), lane_row("?")), (0, 1, 2, 1));
    }

    #[test]
    fn started_sorted_then_planned_after_now() {
        let nodes = vec![
            node("p1", "work", "planned", "2026-11-18 09:00:00"),
            node("b", "work", "running", "2026-11-18 10:05:00"),
            node("a", "in", "done", "2026-11-18 10:00:00"),
            node("p2", "out", "done", ""),
            node("c", "out", "done", "2026-11-18 10:09:00"),
        ];
        let (slots, now) = layout(&nodes);
        let ids: Vec<&str> = slots.iter().map(|s| nodes[s.node].id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "p1", "p2"]);
        assert_eq!(now, 3);
        let planned: Vec<bool> = slots.iter().map(|s| s.planned).collect();
        assert_eq!(planned, [false, false, false, true, true]);
        assert_eq!(slots.iter().map(|s| s.lane).collect::<Vec<_>>(), [0, 1, 2, 1, 2]);
        assert_eq!(slots.iter().map(|s| s.col).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn empty_thread_has_now_at_start() {
        assert_eq!(layout(&[]), (vec![], 0));
    }

    #[test]
    fn statuses_map_to_styles() {
        let th = Theme::dark();
        let cases = [
            ("running", &th.run),
            ("waiting", &th.wait),
            ("stalled", &th.wait),
            ("planned", &th.wait),
            ("needs_you", &th.block),
            ("failed", &th.block),
            ("owner", &th.think),
            ("done", &th.done),
            ("mystery", &th.wait),
        ];
        for (s, want) in cases {
            assert_eq!(node_style(s, &th).base, want.base, "{s}");
        }
    }
}
