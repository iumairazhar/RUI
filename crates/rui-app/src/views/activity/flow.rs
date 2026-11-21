//! The Flow tab: the thread map. Agent work runs along a spine, what came in hangs above it, what
//! went out hangs below it, a "Now" line sits between started and planned steps.

use super::{ThreadsView, mono, parse_secs};
use crate::api::{Node, Thread};
use crate::theme::{Status, Theme};
use gpui::{
    AnyElement, BoxShadow, Bounds, Context, FontWeight, Hsla, PathBuilder, Pixels, Rgba, ScrollWheelEvent, Window, canvas, div, linear_color_stop,
    linear_gradient, point, prelude::*, px, svg,
};

/// Lanes top to bottom: daemon lane key and its label.
pub const LANES: [(&str, &str); 3] = [("in", "Came in"), ("work", "Agent work"), ("out", "Went out")];
/// Width of the lane-label column left of the scrolling track.
const GUTTER_W: f32 = 128.;
/// The spine's y on the track, and the track's height.
const SPINE_Y: f32 = 196.;
const WORLD_H: f32 = 390.;
/// How far above or below the spine the first and second attached node sit.
const NEAR: f32 = 108.;
const FAR: f32 = 162.;
/// Where the first spine node sits, from the track's left edge.
const FIRST_X: f32 = 70.;
const DOT: f32 = 34.;
const DOT_SIDE: f32 = 28.;
const LABEL_W: f32 = 200.;
/// How far right of a dot its card ends (half dot, gap, card).
const CARD_RIGHT: f32 = DOT / 2. + 10. + LABEL_W;
/// The Now line sits this far right of the last started step, clear of its card.
const NOW_AFTER: f32 = CARD_RIGHT + 24.;
const _: () = assert!(NOW_AFTER > CARD_RIGHT);

/// A point on the track, in pixels from its top-left corner.
type Point = (f32, f32);

/// Where a node hangs: above the spine, on it, or below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    In,
    Spine,
    Out,
}

/// A node's place on the track (the middle of its dot).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub node: usize,
    pub zone: Zone,
    pub planned: bool,
    pub x: f32,
    pub y: f32,
    /// The spine node an attached node hangs from.
    pub anchor: Option<usize>,
}

/// Every node placed (spine first, in order), the now line and the track's width.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub placed: Vec<Placed>,
    pub spine_len: usize,
    pub now_x: Option<f32>,
    pub width: f32,
}

/// A planned node has not started; it sits after the now line.
pub fn is_planned(n: &Node) -> bool {
    n.status == "planned" || n.started_at.is_empty()
}

/// Lane row for a daemon lane key; unknown lanes read as agent work.
pub fn lane_row(lane: &str) -> usize {
    LANES.iter().position(|(k, _)| *k == lane).unwrap_or(1)
}

/// A planned node always rides the spine; others by lane.
fn zone_of(n: &Node) -> Zone {
    match (is_planned(n), lane_row(&n.lane)) {
        (false, 0) => Zone::In,
        (false, 2) => Zone::Out,
        _ => Zone::Spine,
    }
}

/// A step the thread is not past: running, waiting, stalled, failed or taken over.
fn is_active(status: &str) -> bool {
    matches!(status, "running" | "needs_you" | "waiting" | "stalled" | "failed" | "owner")
}

/// Minutes from the end of one step to the start of the next; zero when a time is missing.
fn gap_minutes(prev: &Node, next: &Node) -> f32 {
    let end = prev.ended_at.as_deref().filter(|s| !s.is_empty()).unwrap_or(&prev.started_at);
    match (parse_secs(end), parse_secs(&next.started_at)) {
        (Some(a), Some(b)) => ((b - a) as f32 / 60.).max(0.),
        _ => 0.,
    }
}

/// Started spine nodes by `started_at`, then planned ones in daemon order.
fn spine_order(nodes: &[Node]) -> Vec<usize> {
    let mut started: Vec<usize> = (0..nodes.len()).filter(|&i| zone_of(&nodes[i]) == Zone::Spine && !is_planned(&nodes[i])).collect();
    started.sort_by(|&a, &b| nodes[a].started_at.cmp(&nodes[b].started_at));
    started.into_iter().chain((0..nodes.len()).filter(|&i| is_planned(&nodes[i]))).collect()
}

/// The spine: each next step `262 + min(70, 16 log2(1 + minutes))` on, Now between started and planned.
fn place_spine(nodes: &[Node], order: &[usize]) -> (Vec<Placed>, Option<f32>) {
    let mut out: Vec<Placed> = Vec::new();
    let (mut x, mut now_x) = (FIRST_X, None);
    for &i in order {
        let planned = is_planned(&nodes[i]);
        if let Some(prev) = out.last() {
            x += match (planned, prev.planned) {
                (true, false) => {
                    now_x = Some(x + NOW_AFTER);
                    NOW_AFTER + 50.
                }
                (true, true) => 245.,
                _ => 262. + (16. * (1. + gap_minutes(&nodes[prev.node], &nodes[i])).log2()).min(70.),
            };
        }
        out.push(Placed { node: i, zone: Zone::Spine, planned, x, y: SPINE_Y, anchor: None });
    }
    let last_started = out.iter().rev().find(|p| !p.planned);
    if now_x.is_none()
        && let Some(p) = last_started
        && is_active(&nodes[p.node].status)
    {
        now_x = Some(p.x + NOW_AFTER);
    }
    (out, now_x)
}

/// The spine step an attached node hangs from: an `after` link to or from it, else the nearest in time.
fn anchor_of(nodes: &[Node], i: usize, spine: &[Placed]) -> Option<usize> {
    let n = &nodes[i];
    let steps = || spine.iter().filter(|p| !p.planned).map(|p| p.node);
    if zone_of(n) == Zone::Out {
        let linked = n.after.iter().find_map(|a| steps().find(|&s| nodes[s].id == *a));
        return linked.or_else(|| steps().filter(|&s| nodes[s].started_at <= n.started_at).next_back()).or_else(|| steps().next());
    }
    let linked = steps().find(|&s| nodes[s].after.contains(&n.id));
    linked.or_else(|| steps().find(|&s| nodes[s].started_at >= n.started_at)).or_else(|| steps().next_back())
}

/// Horizontal room one attached card takes, gap included, so cards in a row never overlap.
const CARD_W: f32 = DOT / 2. + CARD_RIGHT + 14.;

/// The x where a card goes in its row: as close to `want` as the cards already there allow.
fn free_x(rows: &mut Vec<(Zone, bool, f32)>, zone: Zone, far: bool, want: f32) -> f32 {
    let row = rows.iter_mut().find(|(z, f, _)| *z == zone && *f == far);
    let x = row.as_ref().map_or(want, |(_, _, next)| want.max(*next));
    match row {
        Some(r) => r.2 = x + CARD_W,
        None => rows.push((zone, far, x + CARD_W)),
    }
    x
}

/// Attached nodes: above (came in) or below (went out) their spine step. Cards for one step take turns
/// between the near and far row, and each row pushes a card right until it clears the one before.
fn place_attached(nodes: &[Node], spine: &[Placed]) -> Vec<Placed> {
    let mut rank: Vec<(Option<usize>, Zone, u32)> = Vec::new();
    let mut rows: Vec<(Zone, bool, f32)> = Vec::new();
    let mut out = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let zone = zone_of(n);
        if zone == Zone::Spine {
            continue;
        }
        let anchor = anchor_of(nodes, i, spine);
        let r = match rank.iter_mut().find(|(a, z, _)| *a == anchor && *z == zone) {
            Some(e) => {
                e.2 += 1;
                e.2
            }
            None => {
                rank.push((anchor, zone, 1));
                1
            }
        };
        let ax = anchor.and_then(|a| spine.iter().find(|p| p.node == a)).map_or(FIRST_X, |p| p.x);
        let far = r % 2 == 0;
        let off = if far { FAR } else { NEAR };
        let (want, y) = match zone {
            Zone::In => (ax - 40., SPINE_Y - off),
            _ => (ax + 40., SPINE_Y + off),
        };
        let x = free_x(&mut rows, zone, far, want);
        out.push(Placed { node: i, zone, planned: false, x, y, anchor });
    }
    out
}

/// Place every node: the spine, the nodes hanging off it, and the now line.
pub fn layout(nodes: &[Node]) -> Layout {
    let (mut placed, now_x) = place_spine(nodes, &spine_order(nodes));
    let spine_len = placed.len();
    let attached = place_attached(nodes, &placed);
    placed.extend(attached);
    let right = placed.iter().map(|p| p.x).fold(0., f32::max);
    Layout { placed, spine_len, now_x, width: right + 300. }
}

/// How a line on the track looks (the state grammar: finished, working, waiting, failed, taken over, planned).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Done,
    Run,
    Stalled,
    Wait,
    Fail,
    Human,
    Plan,
    Link(Hue),
}

/// One line on the track: along the spine, or a curve between the spine and an attached node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    Seg { kind: EdgeKind, x0: f32, x1: f32 },
    Link { from: Point, to: Point, hue: Hue },
}

/// How the line from an active step to Now looks.
fn active_edge(status: &str) -> EdgeKind {
    match status {
        "needs_you" | "waiting" => EdgeKind::Wait,
        "failed" => EdgeKind::Fail,
        "owner" => EdgeKind::Human,
        "stalled" => EdgeKind::Stalled,
        _ => EdgeKind::Run,
    }
}

/// Spine lines (finished, to Now, planned) plus a curve to every attached node.
pub fn edges(nodes: &[Node], l: &Layout) -> Vec<Edge> {
    let spine = &l.placed[..l.spine_len];
    let active = |p: &Placed| is_active(&nodes[p.node].status).then(|| active_edge(&nodes[p.node].status));
    let mut out = Vec::new();
    let last_started = spine.iter().rev().find(|p| !p.planned);
    if let (Some(first), Some(last)) = (spine.first(), last_started)
        && last.x > first.x
    {
        out.push(Edge::Seg { kind: EdgeKind::Done, x0: first.x, x1: last.x });
    }
    for w in spine.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        match (b.planned, active(a), l.now_x) {
            (false, ..) => {}
            (true, Some(kind), Some(nx)) => {
                out.push(Edge::Seg { kind, x0: a.x + 20., x1: nx });
                out.push(Edge::Seg { kind: EdgeKind::Plan, x0: nx, x1: b.x });
            }
            (true, ..) => out.push(Edge::Seg { kind: EdgeKind::Plan, x0: a.x, x1: b.x }),
        }
    }
    if let (Some(p), Some(nx)) = (spine.last(), l.now_x)
        && !p.planned
        && let Some(kind) = active(p)
    {
        out.push(Edge::Seg { kind, x0: p.x + 20., x1: nx });
    }
    out.extend(l.placed[l.spine_len..].iter().filter_map(|p| link(nodes, spine, p)));
    out
}

/// The curve from a spine step to an attached node, in the node's own colour.
fn link(nodes: &[Node], spine: &[Placed], p: &Placed) -> Option<Edge> {
    let ax = spine.iter().find(|s| Some(s.node) == p.anchor)?.x;
    let (from, to) = match p.zone {
        Zone::In => ((p.x, p.y + 14.), (ax, SPINE_Y - 18.)),
        _ => ((ax, SPINE_Y + 18.), (p.x, p.y - 14.)),
    };
    Some(Edge::Link { from, to, hue: hue_of(&nodes[p.node], p.zone) })
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

/// The colour a node is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hue {
    Wait,
    Run,
    Done,
    Think,
    Block,
    Plain,
}

/// A node kind's own colour, when it has one.
fn kind_hue(kind: &str) -> Option<Hue> {
    Some(match kind {
        "alert" | "schedule" | "armed" | "decision" => Hue::Wait,
        "message" => Hue::Plain,
        "taskIn" | "taskOut" | "meeting" | "reply" => Hue::Think,
        "step" => Hue::Run,
        "pr" | "deploy" | "email" | "doc" | "demo" | "result" => Hue::Done,
        _ => return None,
    })
}

/// A node's kind colour, else its lane's: came in amber, agent work blue, went out green.
fn hue_of(n: &Node, zone: Zone) -> Hue {
    kind_hue(&n.kind).unwrap_or(match zone {
        Zone::In => Hue::Wait,
        Zone::Spine => Hue::Run,
        Zone::Out => Hue::Done,
    })
}

/// The colour of a node's dot: its status where it has one, else its kind.
fn dot_hue(n: &Node, zone: Zone) -> Hue {
    match n.status.as_str() {
        "failed" => Hue::Block,
        "needs_you" | "waiting" | "stalled" => Hue::Wait,
        "running" => Hue::Run,
        "owner" => Hue::Plain,
        _ => hue_of(n, zone),
    }
}

/// Ink, wash and rim of a hue on the dark surface.
struct Tone {
    fg: Hsla,
    wash: Hsla,
    rim: Hsla,
}

fn tone(h: Hue, th: &Theme) -> Tone {
    let of = |s: &Status| Tone { fg: s.fg, wash: blend(s.dim, th.surface), rim: s.rim };
    match h {
        Hue::Wait => of(&th.wait),
        Hue::Run => of(&th.run),
        Hue::Done => of(&th.done),
        Hue::Think => of(&th.think),
        Hue::Block => of(&th.block),
        Hue::Plain => Tone { fg: th.fg_2, wash: blend(th.fg_2.opacity(0.12), th.surface), rim: th.hairline },
    }
}

/// `top` laid over `base`, as one opaque colour (so lines under a node do not show through it).
fn blend(top: Hsla, base: Hsla) -> Hsla {
    let (t, b): (Rgba, Rgba) = (top.into(), base.into());
    let mix = |t: f32, b: f32| t * top.a + b * (1. - top.a);
    Rgba { r: mix(t.r, b.r), g: mix(t.g, b.g), b: mix(t.b, b.b), a: 1. }.into()
}

/// A line ready to paint: its ends, whether it curves, and how it is stroked.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stroke {
    from: Point,
    to: Point,
    curve: bool,
    width: f32,
    dash: Option<[f32; 2]>,
    color: Hsla,
    /// A finished line fades from `color` to this along its length.
    to_color: Option<Hsla>,
}

fn stroke_of(e: &Edge, th: &Theme) -> Stroke {
    let (from, to, curve, kind) = match *e {
        Edge::Seg { kind, x0, x1 } => ((x0, SPINE_Y), (x1, SPINE_Y), false, kind),
        Edge::Link { from, to, hue } => (from, to, true, EdgeKind::Link(hue)),
    };
    let mut s = Stroke { from, to, curve, width: 3., dash: None, color: th.fg, to_color: None };
    let dashed = |color: Hsla, on, off| (color, Some([on, off]));
    (s.color, s.dash) = match kind {
        EdgeKind::Done => {
            s.to_color = Some(th.done.fg.opacity(0.85));
            (th.run.fg.opacity(0.85), None)
        }
        EdgeKind::Run => dashed(th.run.fg, 7., 7.),
        EdgeKind::Stalled => dashed(th.wait.fg, 3., 6.),
        EdgeKind::Wait => dashed(th.wait.fg, 7., 7.),
        EdgeKind::Fail => dashed(th.block.fg, 3., 6.),
        EdgeKind::Human => dashed(th.fg_2, 7., 7.),
        EdgeKind::Plan => {
            s.width = 2.;
            dashed(th.muted.opacity(0.4), 1., 7.)
        }
        EdgeKind::Link(h) => {
            s.width = 1.6;
            (tone(h, th).fg.opacity(0.7), None)
        }
    };
    s
}

/// Lines behind the nodes: spine segments straight, links as an S down or up.
fn paint_strokes(b: Bounds<Pixels>, strokes: &[Stroke], window: &mut Window) {
    let at = |(x, y): Point| point(b.left() + px(x), b.top() + px(y));
    for s in strokes {
        let mut pb = PathBuilder::stroke(px(s.width));
        if let Some([on, off]) = s.dash {
            pb = pb.dash_array(&[px(on), px(off)]);
        }
        pb.move_to(at(s.from));
        let m = (s.from.1 + s.to.1) / 2.;
        match s.curve {
            true => pb.cubic_bezier_to(at(s.to), at((s.from.0, m)), at((s.to.0, m))),
            false => pb.line_to(at(s.to)),
        }
        let Ok(path) = pb.build() else { continue };
        match s.to_color {
            Some(end) => window.paint_path(path, linear_gradient(90., linear_color_stop(s.color, 0.), linear_color_stop(end, 1.))),
            None => window.paint_path(path, s.color),
        }
    }
}

/// The left edge to scroll the track to: `focus` (the Now line) at 60% of the viewport, moved to
/// the picked node when that would leave it out of view, and kept inside the world.
fn scroll_target(focus: f32, picked: Option<f32>, viewport: f32, world: f32) -> f32 {
    let mut left = focus - viewport * 0.6;
    if let Some(x) = picked.filter(|x| !card_in_view(*x, left, viewport)) {
        left = x - viewport * 0.5;
    }
    left.clamp(0., (world - viewport).max(0.))
}

/// Whether the dot at `x` and its whole card fit between `left` and the viewport's right edge.
fn card_in_view(x: f32, left: f32, viewport: f32) -> bool {
    x - DOT >= left && x + CARD_RIGHT + 16. <= left + viewport
}

/// A mostly-vertical wheel or swipe over the map pans it sideways; returns the new left edge.
fn pan_left(left: f32, dx: f32, dy: f32, max: f32) -> Option<f32> {
    if max <= 0. || dy.abs() <= dx.abs() {
        return None;
    }
    Some((left - dy).clamp(0., max))
}

/// The highlighted node: the one the side panel shows for this thread, else its default node.
fn selected_id(view: &ThreadsView, t: &Thread) -> Option<String> {
    let open = view.selected_node(t.id).filter(|id| t.nodes.iter().any(|n| n.id == *id));
    open.map(str::to_string).or_else(|| super::inspector::default_node(&t.nodes).map(|n| n.id.clone()))
}

impl ThreadsView {
    /// The Flow tab body: lane labels on the left, the scrolling track on the right.
    pub(super) fn flow(&mut self, thread: Option<&Thread>, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(t) = thread else {
            return div().text_size(px(12.5)).text_color(th.muted).child("Loading the flow\u{2026}").into_any_element();
        };
        self.sync_flow_scroll(t, cx);
        let track = div().id("flow-track").flex_1().min_w_0().h_full().overflow_x_scroll().track_scroll(&self.flow_scroll);
        let track = track.child(self.track(t, th, cx));
        let pan = div().id("flow-pan").flex_1().min_w_0().h_full().flex().child(track);
        let pan = pan.on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, window, cx| this.pan_flow(ev, window, cx)));
        div().id("flow").flex().w_full().h(px(WORLD_H)).child(gutter(th)).child(pan).into_any_element()
    }

    /// Vertical wheel or swipe over the map moves it sideways, and the page under it stays put.
    /// A plain mouse wheel (no sideways part) is already moved by the track itself.
    fn pan_flow(&mut self, ev: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let d = ev.delta.pixel_delta(window.line_height());
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        let max = f32::from(self.flow_scroll.max_offset().width);
        let left = -f32::from(self.flow_scroll.offset().x);
        if max <= 0. || dy.abs() <= dx.abs() {
            return;
        }
        cx.stop_propagation();
        if dx == 0. {
            return;
        }
        if let Some(x) = pan_left(left, dx, dy, max) {
            self.flow_scroll.set_offset(point(px(-x), px(0.)));
            cx.notify();
        }
    }

    /// Scroll the map so the Now line (and the picked node) is in view: when a thread opens, and
    /// when the picked node changes to one that is off screen.
    fn sync_flow_scroll(&mut self, t: &Thread, cx: &mut Context<Self>) {
        let picked = selected_id(self, t);
        let viewport = f32::from(self.flow_scroll.bounds().size.width);
        if viewport <= 0. {
            cx.notify();
            return;
        }
        let key = (t.id, picked.clone(), viewport.round() as i32);
        if self.flow_key.as_ref() == Some(&key) {
            return;
        }
        let l = layout(&t.nodes);
        let picked_x = picked.and_then(|id| l.placed.iter().find(|p| t.nodes[p.node].id == id).map(|p| p.x));
        let left = -f32::from(self.flow_scroll.offset().x);
        let in_view = picked_x.is_some_and(|x| card_in_view(x, left, viewport));
        let same_thread = self.flow_key.as_ref().is_some_and(|k| k.0 == t.id && k.2 == key.2);
        self.flow_key = Some(key);
        if same_thread && in_view {
            return;
        }
        let focus = l.now_x.or(picked_x).unwrap_or(0.);
        let x = scroll_target(focus, picked_x, viewport, l.width);
        self.flow_scroll.set_offset(point(px(-x), px(0.)));
    }

    fn track(&self, t: &Thread, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let l = layout(&t.nodes);
        let strokes: Vec<Stroke> = edges(&t.nodes, &l).iter().map(|e| stroke_of(e, th)).collect();
        let lines = canvas(|_, _, _| (), move |b, _, window, _| paint_strokes(b, &strokes, window));
        let area = div().relative().w(px(l.width)).h(px(WORLD_H)).child(lines.absolute().top_0().left_0().size_full());
        let area = area.children(l.now_x.map(|x| now_line(x, th)));
        let area = area.children(l.now_x.filter(|_| l.placed.iter().any(|p| p.planned)).map(|x| planned_label(x, th)));
        let picked = selected_id(self, t);
        let nodes = l.placed.iter().map(|p| self.placed_node(t, p, picked.as_deref(), th, cx));
        area.children(nodes.collect::<Vec<_>>()).into_any_element()
    }

    fn placed_node(&self, t: &Thread, p: &Placed, picked: Option<&str>, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let n = &t.nodes[p.node];
        let (tid, nid) = (t.id, n.id.clone());
        let selected = picked == Some(n.id.as_str());
        let group = format!("flow-nd-{}", p.node);
        let chip = div().id(("flow-node", p.node)).group(group.clone()).flex().items_center().gap(px(10.)).h(px(80.)).cursor_pointer();
        let chip = chip.child(dot_view(n, p, selected, th)).child(label_view(n, p, selected, group, th));
        let chip = chip.on_click(cx.listener(move |this, _, _, cx| {
            this.inspect(tid, nid.clone(), cx);
        }));
        let dx = if p.zone == Zone::Spine { DOT / 2. } else { DOT_SIDE / 2. };
        chip.absolute().left(px(p.x - dx)).top(px(p.y - 40.)).into_any_element()
    }
}

/// The three lane names at the canvas's left edge, each with a coloured bar and a sub-line.
fn gutter(th: &Theme) -> AnyElement {
    let label = |row: usize| lane_label(row, th).absolute().left(px(18.)).top(px(SPINE_Y + (row as f32 - 1.) * NEAR - 24.));
    let col = div().relative().flex_none().w(px(GUTTER_W)).h_full();
    col.child(label(0)).child(label(1)).child(label(2)).into_any_element()
}

/// Sub-line under each lane title.
fn lane_hint(row: usize) -> &'static str {
    ["alerts, messages, tasks", "steps and decisions", "PRs, emails, tasks"][row]
}

fn lane_label(row: usize, th: &Theme) -> gpui::Div {
    let bar_color = [th.wait.fg, th.run.fg, th.done.fg][row];
    let bar = div().w(px(18.)).h(px(3.)).rounded(px(2.)).bg(bar_color).mb(px(5.));
    let title = div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(LANES[row].1);
    let hint = div().w(px(104.)).text_size(px(10.5)).text_color(th.muted.opacity(0.7)).child(lane_hint(row));
    div().flex().flex_col().gap(px(1.)).child(bar).child(title).child(hint)
}

/// Small caption above the spine, just past the now line.
fn planned_label(now_x: f32, th: &Theme) -> AnyElement {
    let text = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("PLANNED NEXT");
    div().absolute().left(px(now_x + 24.)).top(px(SPINE_Y - 46. - 6.5)).child(text).into_any_element()
}

/// Local time as HH:MM, from the clock and the offset `date` gave once at start.
fn local_hhmm() -> String {
    hhmm(crate::store::now_secs() + super::timeline::local_offset())
}

/// "HH:MM" of a unix time already shifted to local.
fn hhmm(local_secs: i64) -> String {
    let m = local_secs.rem_euclid(86_400) / 60;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// A vertical piece of the now line, fading `from` to `to` (alpha) top to bottom.
fn now_piece(color: Hsla, h: f32, from: f32, to: f32) -> gpui::Div {
    let fade = linear_gradient(180., linear_color_stop(color.opacity(from), 0.), linear_color_stop(color.opacity(to), 1.));
    div().w(px(2.)).h(px(h)).bg(fade)
}

/// The "now" line between started work and planned work, with a "Now HH:MM" pill on top.
fn now_line(x: f32, th: &Theme) -> AnyElement {
    let body = WORLD_H - 44.;
    let line = div().absolute().left(px(x - 1.)).top(px(22.)).flex().flex_col();
    let line = line.child(now_piece(th.run.fg, body * 0.2, 0., 0.55)).child(now_piece(th.run.fg, body * 0.6, 0.55, 0.55));
    let line = line.child(now_piece(th.run.fg, body * 0.2, 0.55, 0.));
    let text = mono(10.5, gpui::white(), format!("Now {}", local_hhmm())).font_weight(FontWeight::SEMIBOLD);
    let pill = div().px(px(8.)).py(px(4.)).rounded_full().bg(th.run.fg).child(text);
    let pill = div().absolute().left(px(x - 48.)).top(px(22.)).w(px(96.)).flex().justify_center().child(pill);
    div().child(line).child(pill).into_any_element()
}

/// Icon file (in `icons/`) for a node: by lane, with kind, failed, needs_you and planned overriding.
pub fn node_icon(n: &Node) -> &'static str {
    if is_planned(n) {
        return "claude";
    }
    match (n.status.as_str(), n.kind.as_str(), lane_row(&n.lane)) {
        ("needs_you", ..) => "hand",
        ("failed", ..) => "close",
        (_, "pr", _) => "diff",
        (_, "doc", _) => "file",
        (_, "reply", _) => "send",
        (.., 0) => "arrow",
        (.., 2) => "check",
        _ => "claude",
    }
}

/// What a kind of node is called; a lane word when the kind is not one of the known ones.
fn kind_label(kind: &str, zone: Zone) -> &'static str {
    match kind {
        "alert" => "Alert",
        "schedule" => "Schedule",
        "message" => "Message",
        "taskIn" => "Task handed to agent",
        "armed" => "Trigger set",
        "step" => "Agent work",
        "decision" => "Decision",
        "pr" => "Pull request",
        "deploy" => "Deploy",
        "email" => "Email",
        "doc" => "Document",
        "demo" => "Demo",
        "result" => "Test result",
        "taskOut" => "Task for the team",
        "meeting" => "Planning meeting",
        "reply" => "Lead replied",
        _ => match zone {
            Zone::In => "Came in",
            Zone::Out => "Went out",
            Zone::Spine => "Step",
        },
    }
}

/// The non-empty parts joined with " \u{b7} ".
fn dotted(parts: &[&str]) -> String {
    parts.iter().copied().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" \u{b7} ")
}

/// " \u{b7} low risk" when the decision says how risky it is.
fn risk_part(n: &Node) -> String {
    let risk = n.decision.as_ref().map(|d| d.risk.to_lowercase()).unwrap_or_default();
    if risk.is_empty() || risk == "unknown" { String::new() } else { format!(" \u{b7} {risk} risk") }
}

/// "Designer" plus the picked model and effort when set: "Designer \u{b7} Opus high".
pub fn agent_and_model(n: &Node) -> String {
    let agent = n.agent.as_deref().unwrap_or("");
    let Some(model) = n.model.as_deref().filter(|m| !m.is_empty()) else { return agent.to_string() };
    let mut m = model[..1].to_uppercase() + &model[1..];
    if let Some(e) = n.effort.as_deref().filter(|e| !e.is_empty()) {
        m = format!("{m} {e}");
    }
    dotted(&[agent, &m])
}

/// Card subtitle: "Waiting on you \u{b7} low risk", "Planned \u{b7} Ops", "Reviewer \u{b7} 10:20", "Task for the team \u{b7} unclaimed".
pub fn subtitle(n: &Node) -> String {
    let time = n.started_at.get(11..16).unwrap_or("");
    let who = agent_and_model(n);
    let agent = who.as_str();
    let zone = zone_of(n);
    if is_planned(n) {
        return dotted(&["Planned", agent]);
    }
    match n.status.as_str() {
        "needs_you" => format!("Waiting on you{}", risk_part(n)),
        "waiting" => format!("Waiting on others{}", risk_part(n)),
        "failed" => dotted(&[agent, "failed"]),
        "owner" => "You took over".into(),
        "stalled" => dotted(&[agent, "stalled"]),
        "running" => dotted(&[agent, "working now"]),
        _ => match zone {
            Zone::In => dotted(&[kind_label(&n.kind, zone), agent, time]),
            Zone::Out if n.kind == "taskOut" => dotted(&["Task for the team", if agent.is_empty() { "unclaimed" } else { agent }]),
            Zone::Out => kind_label(&n.kind, zone).to_string(),
            Zone::Spine => dotted(&[agent, time]),
        },
    }
}

/// The round dot: tinted by kind or status; orange, filled and haloed when it needs you; ringed when selected.
fn dot_view(n: &Node, p: &Placed, selected: bool, th: &Theme) -> AnyElement {
    let (d, glyph) = if p.zone == Zone::Spine { (DOT, 16.) } else { (DOT_SIDE, 14.) };
    let t = tone(dot_hue(n, p.zone), th);
    let (needs, white) = (!p.planned && n.status == "needs_you", gpui::white());
    let (bg, ink, rim) = match n.status.as_str() {
        _ if p.planned => (blend(th.fg.opacity(0.05), th.surface), th.muted, th.muted.opacity(0.4)),
        "needs_you" => (th.wait.fg, white, th.wait.fg),
        "failed" => (th.block.fg, white, th.block.fg),
        "owner" => (th.fg_2, th.surface, th.fg_2),
        "running" => (th.surface, th.run.fg, th.run.fg.opacity(0.6)),
        _ => (t.wash, t.fg, t.rim),
    };
    let ring = |spread: f32, color: Hsla| BoxShadow { color, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(spread) };
    let mut shadows = Vec::new();
    if needs {
        shadows.push(ring(9., th.wait.fg.opacity(0.28)));
    }
    if !p.planned && n.status == "failed" {
        shadows.push(ring(4., th.block.fg.opacity(0.18)));
    }
    if selected {
        shadows.push(ring(5., th.run.fg.opacity(0.7)));
    }
    if selected || needs {
        shadows.push(ring(3., th.surface));
    }
    let glyph = svg().path(format!("icons/{}.svg", node_icon(n))).size(px(glyph)).flex_none().text_color(ink);
    let dot = div().flex().flex_none().items_center().justify_center().size(px(d)).rounded_full().bg(bg);
    dot.border_1().border_color(rim).shadow(shadows).child(glyph).into_any_element()
}

/// The card beside the dot: a title of up to two lines and a subtitle. Dashed when planned, accent-bordered when it needs you.
fn label_view(n: &Node, p: &Placed, selected: bool, group: String, th: &Theme) -> AnyElement {
    let ink = if p.planned { th.muted } else { th.fg };
    let sub_ink = match n.status.as_str() {
        "failed" if !p.planned => th.block.fg,
        "stalled" if !p.planned => th.wait.fg,
        _ => th.muted,
    };
    let title = div().w_full().text_size(px(12.5)).line_height(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(ink).line_clamp(2);
    let sub = div().w_full().h(px(14.)).overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(11.)).line_height(px(14.)).text_color(sub_ink);
    let card = div().flex().flex_col().flex_none().gap(px(3.)).w(px(LABEL_W)).px(px(11.)).pt(px(6.)).pb(px(7.)).rounded(px(12.));
    let card = card.child(title.child(n.title.clone())).child(sub.child(subtitle(n)));
    let hover = th.surface_2;
    let card = match (p.planned, n.status == "needs_you") {
        (true, _) => card.bg(blend(th.fg.opacity(0.05), th.surface)).border_1().border_dashed().border_color(th.muted.opacity(0.4)),
        (_, true) => super::border_w(card, 1.5).border_color(th.accent_text).bg(th.surface).shadow_sm(),
        _ => card.bg(th.surface).border_1().border_color(th.hairline).shadow_sm(),
    };
    let card = card.group_hover(group, move |s| s.bg(hover));
    if !selected {
        return card.into_any_element();
    }
    let ring = BoxShadow { color: th.run.fg.opacity(0.75), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(2.) };
    let glow = BoxShadow { color: th.run.fg.opacity(0.18), offset: point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.) };
    card.bg(th.surface_2).shadow(vec![glow, ring]).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Decision;

    #[test]
    fn scroll_target_keeps_now_and_the_picked_node_in_view() {
        // Now near the right edge of a wide world: scroll so it sits at 60% of the viewport.
        assert!(close(scroll_target(1000., None, 800., 3000.), 520.));
        // Never before the start, never past the end.
        assert!(close(scroll_target(100., None, 800., 3000.), 0.));
        assert!(close(scroll_target(2990., None, 800., 3000.), 2200.));
        // A picked node left of the window pulls the view back to it.
        assert!(close(scroll_target(1000., Some(200.), 800., 3000.), 0.));
        assert!(close(scroll_target(1000., Some(700.), 800., 3000.), 520.));
        assert!(close(scroll_target(1000., Some(1400.), 800., 3000.), 1000.));
    }

    #[test]
    fn vertical_wheel_pans_sideways_within_the_map() {
        assert_eq!(pan_left(100., 0.5, -40., 500.), Some(140.));
        assert_eq!(pan_left(10., 0., 40., 500.), Some(0.));
        assert_eq!(pan_left(490., 0., -40., 500.), Some(500.));
        assert_eq!(pan_left(100., 30., 5., 500.), None);
        assert_eq!(pan_left(100., 0., -40., 0.), None);
    }

    #[test]
    fn hhmm_wraps_the_day() {
        assert_eq!(hhmm(0), "00:00");
        assert_eq!(hhmm(23 * 3600 + 5 * 60 + 59), "23:05");
        assert_eq!(hhmm(86_400 + 61), "00:01");
        assert_eq!(hhmm(-60), "23:59");
    }

    fn node(id: &str, lane: &str, status: &str, at: &str) -> Node {
        Node { id: id.into(), lane: lane.into(), status: status.into(), started_at: at.into(), ..Default::default() }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    fn at<'a>(l: &'a Layout, nodes: &[Node], id: &str) -> &'a Placed {
        l.placed.iter().find(|p| nodes[p.node].id == id).unwrap()
    }

    #[test]
    fn lanes_map_to_rows_and_zones() {
        assert_eq!((lane_row("in"), lane_row("work"), lane_row("out"), lane_row("?")), (0, 1, 2, 1));
        assert_eq!(zone_of(&node("a", "in", "done", "2026-11-18 10:00:00")), Zone::In);
        assert_eq!(zone_of(&node("a", "out", "done", "2026-11-18 10:00:00")), Zone::Out);
        assert_eq!(zone_of(&node("a", "out", "planned", "")), Zone::Spine);
    }

    #[test]
    fn spine_is_by_time_then_planned_with_now_between() {
        let nodes = vec![
            node("p1", "work", "planned", ""),
            node("b", "work", "running", "2026-11-18 10:05:00"),
            node("a", "work", "done", "2026-11-18 10:00:00"),
            node("p2", "out", "planned", ""),
        ];
        let l = layout(&nodes);
        let ids: Vec<&str> = l.placed.iter().map(|p| nodes[p.node].id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "p1", "p2"]);
        assert_eq!(l.spine_len, 4);
        let xs: Vec<f32> = l.placed.iter().map(|p| p.x).collect();
        let first_gap = 262. + 16. * 6f32.log2();
        assert!(close(xs[0], 70.) && close(xs[1], 70. + first_gap));
        assert!(close(xs[2], xs[1] + NOW_AFTER + 50.) && close(xs[3], xs[2] + 245.));
        assert!(close(l.now_x.unwrap(), xs[1] + NOW_AFTER));
        assert!(l.placed.iter().all(|p| close(p.y, SPINE_Y)));
        assert_eq!(l.placed.iter().map(|p| p.planned).collect::<Vec<_>>(), [false, false, true, true]);
    }

    #[test]
    fn long_gaps_are_capped() {
        let nodes = vec![node("a", "work", "done", "2026-11-18 01:00:00"), node("b", "work", "done", "2026-11-18 20:00:00")];
        let l = layout(&nodes);
        assert!(close(l.placed[1].x - l.placed[0].x, 262. + 70.));
    }

    #[test]
    fn now_follows_an_active_last_step_only() {
        let a = node("a", "work", "running", "2026-11-18 10:00:00");
        assert!(close(layout(std::slice::from_ref(&a)).now_x.unwrap(), 70. + NOW_AFTER));
        let done = node("a", "work", "done", "2026-11-18 10:00:00");
        assert_eq!(layout(&[done]).now_x, None);
        assert_eq!(layout(&[]).now_x, None);
    }

    #[test]
    fn attached_nodes_hang_off_their_step() {
        let mut out1 = node("o1", "out", "done", "2026-11-18 10:09:00");
        out1.after = vec!["a".into()];
        let out2 = node("o2", "out", "done", "2026-11-18 10:08:00");
        let mut a = node("a", "work", "done", "2026-11-18 10:00:00");
        a.after = vec!["i".into()];
        let nodes = vec![node("i", "in", "done", "2026-11-18 09:59:00"), a, out1, out2];
        let l = layout(&nodes);
        let ax = at(&l, &nodes, "a").x;
        let i = at(&l, &nodes, "i");
        assert_eq!((i.zone, i.anchor.map(|n| nodes[n].id.as_str())), (Zone::In, Some("a")));
        assert!(close(i.x, ax - 40.) && close(i.y, SPINE_Y - NEAR));
        let (o1, o2) = (at(&l, &nodes, "o1"), at(&l, &nodes, "o2"));
        assert!(close(o1.x, ax + 40.) && close(o1.y, SPINE_Y + NEAR));
        assert!(close(o2.x, ax + 40.) && close(o2.y, SPINE_Y + FAR));
    }

    #[test]
    fn card_shows_the_picked_model_and_effort() {
        let mut n = node("s", "work", "running", "2026-11-18 10:00:00");
        n.agent = Some("Designer".into());
        assert_eq!(agent_and_model(&n), "Designer");
        n.model = Some("opus".into());
        n.effort = Some("high".into());
        assert_eq!(agent_and_model(&n), "Designer \u{b7} Opus high");
    }

    #[test]
    fn cards_in_one_row_never_overlap() {
        let mut a = node("a", "work", "done", "2026-11-18 10:00:00");
        a.after = vec!["i1".into(), "i2".into(), "i3".into(), "i4".into()];
        let ins: Vec<Node> = (1..=4).map(|k| node(&format!("i{k}"), "in", "done", "2026-11-18 09:59:00")).collect();
        let nodes: Vec<Node> = std::iter::once(a).chain(ins).collect();
        let l = layout(&nodes);
        let cards: Vec<&Placed> = l.placed.iter().filter(|p| p.zone == Zone::In).collect();
        for (i, p) in cards.iter().enumerate() {
            for q in &cards[i + 1..] {
                assert!(!close(p.y, q.y) || (p.x - q.x).abs() >= CARD_W - 0.5, "cards overlap");
            }
        }
    }

    #[test]
    fn edges_run_along_the_spine_and_curve_to_attached_nodes() {
        let mut o = node("o", "out", "done", "2026-11-18 10:09:00");
        o.after = vec!["b".into()];
        let nodes = vec![
            node("a", "work", "done", "2026-11-18 10:00:00"),
            node("b", "work", "needs_you", "2026-11-18 10:05:00"),
            o,
            node("p", "work", "planned", ""),
        ];
        let l = layout(&nodes);
        let e = edges(&nodes, &l);
        let (a, b, p) = (l.placed[0].x, l.placed[1].x, l.placed[2].x);
        let nx = l.now_x.unwrap();
        assert_eq!(e[0], Edge::Seg { kind: EdgeKind::Done, x0: a, x1: b });
        assert_eq!(e[1], Edge::Seg { kind: EdgeKind::Wait, x0: b + 20., x1: nx });
        assert_eq!(e[2], Edge::Seg { kind: EdgeKind::Plan, x0: nx, x1: p });
        let Edge::Link { from, to, hue } = e[3] else { panic!("{e:?}") };
        assert_eq!((from, hue), ((b, SPINE_Y + 18.), Hue::Done));
        assert!(close(to.0, b + 40.) && close(to.1, SPINE_Y + NEAR - 14.));
        assert_eq!(e.len(), 4);
    }

    #[test]
    fn planned_only_after_finished_work_is_a_dotted_line() {
        let nodes = vec![node("a", "work", "done", "2026-11-18 10:00:00"), node("p", "work", "planned", "")];
        let l = layout(&nodes);
        let e = edges(&nodes, &l);
        assert_eq!(e, vec![Edge::Seg { kind: EdgeKind::Plan, x0: l.placed[0].x, x1: l.placed[1].x }]);
    }

    #[test]
    fn icons_by_lane_and_status() {
        let at = "2026-11-18 10:00:00";
        let icon = |l: &str, st: &str, t: &str| node_icon(&node("x", l, st, t));
        assert_eq!(icon("in", "done", at), "arrow");
        assert_eq!(icon("work", "running", at), "claude");
        assert_eq!(icon("out", "done", at), "check");
        assert_eq!(icon("work", "needs_you", at), "hand");
        assert_eq!(icon("work", "failed", at), "close");
        assert_eq!(icon("out", "planned", ""), "claude");
        for name in ["arrow", "claude", "check", "hand", "close", "diff", "file", "send"] {
            let path = format!("icons/{name}.svg");
            assert!(gpui::AssetSource::load(&crate::assets::Assets, &path).unwrap().is_some(), "{path}");
        }
        let mut pr = node("x", "out", "done", at);
        pr.kind = "pr".into();
        assert_eq!(node_icon(&pr), "diff");
        let mut reply = node("r", "out", "done", at);
        reply.kind = "reply".into();
        assert_eq!(node_icon(&reply), "send");
        assert_eq!((subtitle(&reply), dot_hue(&reply, zone_of(&reply))), ("Lead replied".to_string(), Hue::Think));
    }

    #[test]
    fn subtitles() {
        let at = "2026-11-18 10:20:00";
        let mut n = node("x", "work", "done", at);
        assert_eq!(subtitle(&n), "10:20");
        n.agent = Some("Reviewer".into());
        assert_eq!(subtitle(&n), "Reviewer \u{b7} 10:20");
        n.status = "running".into();
        assert_eq!(subtitle(&n), "Reviewer \u{b7} working now");
        n.status = "needs_you".into();
        assert_eq!(subtitle(&n), "Waiting on you");
        n.decision = Some(Decision { risk: "Low".into(), ..Default::default() });
        assert_eq!(subtitle(&n), "Waiting on you \u{b7} low risk");
        n.decision = Some(Decision { risk: "unknown".into(), ..Default::default() });
        assert_eq!(subtitle(&n), "Waiting on you");
        let mut p = node("p", "work", "planned", "");
        assert_eq!(subtitle(&p), "Planned");
        p.agent = Some("Ops".into());
        assert_eq!(subtitle(&p), "Planned \u{b7} Ops");
    }

    #[test]
    fn attached_subtitles_name_the_kind() {
        let at = "2026-11-18 10:00:00";
        let mut task = node("t", "out", "done", at);
        task.kind = "taskOut".into();
        assert_eq!(subtitle(&task), "Task for the team \u{b7} unclaimed");
        task.agent = Some("Priya".into());
        assert_eq!(subtitle(&task), "Task for the team \u{b7} Priya");
        let mut alert = node("a", "in", "done", at);
        alert.kind = "alert".into();
        assert_eq!(subtitle(&alert), "Alert \u{b7} 10:00");
        assert_eq!(subtitle(&node("o", "out", "done", at)), "Went out");
    }

    #[test]
    fn hues_follow_status_then_kind_then_lane() {
        let at = "2026-11-18 10:00:00";
        let hue = |l: &str, k: &str, st: &str| {
            let mut n = node("x", l, st, at);
            n.kind = k.into();
            dot_hue(&n, zone_of(&n))
        };
        assert_eq!(hue("work", "step", "failed"), Hue::Block);
        assert_eq!(hue("work", "decision", "needs_you"), Hue::Wait);
        assert_eq!(hue("work", "subtask", "running"), Hue::Run);
        assert_eq!(hue("work", "step", "done"), Hue::Run);
        assert_eq!(hue("out", "pr", "done"), Hue::Done);
        assert_eq!(hue("in", "", "done"), Hue::Wait);
        assert_eq!(hue("out", "", "done"), Hue::Done);
        assert_eq!(hue("in", "message", "done"), Hue::Plain);
    }

    #[test]
    fn blend_lays_a_translucent_colour_over_a_base() {
        let white: Hsla = gpui::white();
        let black: Hsla = gpui::black();
        let mixed: Rgba = blend(white.opacity(0.5), black).into();
        assert!(close(mixed.r, 0.5) && close(mixed.a, 1.));
        let same: Rgba = blend(white.opacity(0.), black).into();
        assert!(close(same.r, 0.));
    }

    #[test]
    fn strokes_follow_the_edge_grammar() {
        let th = Theme::dark();
        let seg = |kind| stroke_of(&Edge::Seg { kind, x0: 10., x1: 50. }, &th);
        assert_eq!((seg(EdgeKind::Run).dash, seg(EdgeKind::Run).width), (Some([7., 7.]), 3.));
        assert_eq!(seg(EdgeKind::Stalled).dash, Some([3., 6.]));
        assert_eq!((seg(EdgeKind::Plan).dash, seg(EdgeKind::Plan).width), (Some([1., 7.]), 2.));
        assert!(seg(EdgeKind::Done).dash.is_none() && seg(EdgeKind::Done).to_color.is_some());
        let s = seg(EdgeKind::Wait);
        assert_eq!((s.from, s.to, s.curve), ((10., SPINE_Y), (50., SPINE_Y), false));
        let l = stroke_of(&Edge::Link { from: (0., 0.), to: (5., 9.), hue: Hue::Done }, &th);
        assert!(l.curve && l.dash.is_none() && close(l.width, 1.6));
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
