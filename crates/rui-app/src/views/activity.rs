//! Threads: the board of agent threads, with a full task page for the open one.

mod task_page;
mod team_view;
mod teams;
mod timeline;

use super::live::live_action;
use super::trace::{DiffKind, DiffLine, Detail, Kind, TraceRow, parse_diff, touched_files};
use super::character::{char_state, character};
use super::motion::{self, delayed, ease, spring};
use super::roster::{Dot, Loops, member_name, pod_line};
use team_view::Phase;
use timeline::ROWS;
use teams::can_stop;
use super::section::{Sec, section_top};
use super::status::{self, Word, word_of};
use super::{chip, markdown, primary_button, tag};
use super::results::row_meta;
use crate::api::{Change, LiveSnapshot, LiveState, ResultFile};
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Status, Theme, theme};
use gpui::{
    AbsoluteLength, Animation, AnimationExt, AnyElement, Context, Div, Entity, FontWeight, Hsla, ScrollHandle,
    SharedString, Stateful, StyledText, Transformation, Window, div, prelude::*, px, radians,
    svg,
};
use rui_core::{Event, QuestionStatus, Role, Task, TaskStatus};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn one_line(s: &str, max: usize) -> String {
    truncate(&s.replace(['\n', '\r'], " "), max)
}

fn content_blocks(payload: &Value) -> Vec<&Value> {
    payload["message"]["content"].as_array().map(|a| a.iter().collect()).unwrap_or_default()
}

fn tool_use_part(block: &Value) -> String {
    let name = block["name"].as_str().unwrap_or("tool");
    let name = name.strip_prefix("mcp__rui__").unwrap_or(name);
    let input = if block["input"].is_null() { String::new() } else { block["input"].to_string() };
    format!("→ {name}({})", truncate(&input, 80))
}

fn summarize_assistant(payload: &Value) -> String {
    let blocks = content_blocks(payload);
    let text: Vec<&str> = blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(one_line(&text.join(" "), 200));
    }
    parts.extend(blocks.iter().filter(|b| b["type"] == "tool_use").map(|b| tool_use_part(b)));
    if parts.is_empty() { "assistant".into() } else { parts.join("  ") }
}

fn result_text(block: &Value) -> String {
    match &block["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" ")
        }
        _ => String::new(),
    }
}

fn summarize_toolresult(payload: &Value) -> (String, bool) {
    let blocks = content_blocks(payload);
    let Some(b) = blocks.iter().find(|b| b["is_error"] == true) else {
        return ("← ok".into(), false);
    };
    (format!("← error: {}", one_line(&result_text(b), 120)), true)
}

fn summarize_result(p: &Value) -> (String, bool) {
    if p["is_error"] == true {
        return ("✗ failed".into(), true);
    }
    match p["total_cost_usd"].as_f64() {
        Some(c) => (format!("✓ finished (${c:.2})"), false),
        None => ("✓ finished".into(), false),
    }
}

fn summarize_done_check(p: &Value) -> (String, bool) {
    if p["passed"] == true {
        return ("check passed".into(), false);
    }
    let tail = p["output_tail"].as_str().and_then(|s| s.lines().next()).unwrap_or("");
    if tail.is_empty() {
        return ("check failed".into(), true);
    }
    (format!("check failed {}", truncate(tail, 100)), true)
}

fn str_or<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p[key].as_str().filter(|s| !s.is_empty())
}

fn with_opt(prefix: &str, value: Option<&str>) -> String {
    match value {
        Some(v) => format!("{prefix}: {v}"),
        None => prefix.to_string(),
    }
}

/// Turn an event into a one-line summary plus whether it should be shown as an error.
pub fn summarize(e: &Event) -> (String, bool) {
    let p = &e.payload;
    match e.kind.as_str() {
        "assistant" => (summarize_assistant(p), false),
        "toolresult" => summarize_toolresult(p),
        "init" => match str_or(p, "model") {
            Some(m) => (format!("started · {m}"), false),
            None => ("started".into(), false),
        },
        "result" => summarize_result(p),
        "done_check" => summarize_done_check(p),
        "commit" => ("committed".into(), false),
        "rollup" => (format!("task → {}", str_or(p, "status").unwrap_or("?")), false),
        "recovered" => {
            let from = str_or(p, "from").unwrap_or("?");
            let to = str_or(p, "to").unwrap_or("?");
            (format!("recovered {from} → {to}"), false)
        }
        "agent_error" | "scheduler_error" => (with_opt("error", str_or(p, "error")), true),
        "lead_no_outcome" => ("lead ended without asking or splitting".into(), true),
        "stuck" => (with_opt("stuck", str_or(p, "reason")), true),
        "progress" => (with_opt("progress", str_or(p, "note")), false),
        "tool_call" => (with_opt("tool", str_or(p, "tool")), false),
        other => (other.to_string(), false),
    }
}


fn role_name(r: &Role) -> &'static str {
    match r {
        Role::Lead => "Lead",
        Role::Builder => "Builder",
        Role::Rui => "Rui",
    }
}

fn label(t: &Task) -> String {
    format!("{} {}", role_name(&t.role), t.id)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix seconds from "YYYY-MM-DD HH:MM:SS" or ISO "YYYY-MM-DDTHH:MM:SS...".
pub(super) fn parse_secs(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse().ok() };
    let days = days_from_civil(n(0, 4)?, n(5, 7)?, n(8, 10)?);
    Some(days * 86_400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19).unwrap_or(0))
}

fn now_secs() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since.map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Time since `updated_at` as "just now", "14m", "2h 10m" or "3d".
fn elapsed(updated_at: &str, now: i64) -> String {
    let Some(t) = parse_secs(updated_at) else { return String::new() };
    let secs = (now - t).max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 if secs % 3600 < 60 => format!("{}h", secs / 3600),
        3600..86_400 => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d", secs / 86_400),
    }
}

/// A task shown as a thread, with the extra data its row needs.
#[derive(Clone)]
struct Thread {
    task: Task,
    client: String,
    model: Option<String>,
    question: Option<(i64, String)>,
    /// "Backend Engineer 01", or "Builder 7" when the task has no agent.
    name: String,
    /// Pod line such as "Lead, Pod: BE FE", set on a lead parent.
    pod_line: Option<String>,
    /// The task's status word, from the daemon.
    word: Word,
    /// The daemon's one-line reason for the word.
    reason: String,
}

fn threads_of(s: &Store) -> Vec<Thread> {
    let open_q = |id: i64| {
        let q = s.questions.iter().find(|q| q.task_id == id && q.status == QuestionStatus::Open);
        q.map(|q| (q.id, q.text.clone()))
    };
    s.tasks.iter().map(|t| thread_of(s, t, open_q(t.id))).collect()
}

fn thread_of(s: &Store, t: &Task, question: Option<(i64, String)>) -> Thread {
    Thread {
        task: t.clone(),
        client: s.clients.iter().find(|c| c.id == t.client_id).map_or(String::new(), |c| c.name.clone()),
        model: s.runs.get(&t.id).and_then(|r| r.model.clone()),
        question,
        name: member_name(t, &s.tasks, &s.roles).unwrap_or_else(|| label(t)),
        pod_line: pod_line(t, &s.roles),
        word: word_of(&s.states, t.id),
        reason: s.states.get(&t.id).map_or(String::new(), |o| o.reason.clone()),
    }
}

/// Tab of the task page.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Trace,
    Changes,
    Files,
    Results,
}

pub(super) const MONO: &str = "Menlo";

pub(super) fn mono(size: f32, color: Hsla, text: impl Into<SharedString>) -> Div {
    div().font_family(MONO).text_size(px(size)).text_color(color).child(text.into())
}

pub(super) fn icon(name: &str, size: f32, color: Hsla) -> impl IntoElement {
    svg().path(format!("icons/{name}.svg")).size(px(size)).flex_none().text_color(color)
}

/// Set all four border widths (gpui only has whole-pixel helpers).
pub(super) fn border_w<E: Styled>(mut e: E, w: f32) -> E {
    let w: AbsoluteLength = px(w).into();
    let b = &mut e.style().border_widths;
    (b.top, b.right, b.bottom, b.left) = (Some(w), Some(w), Some(w), Some(w));
    e
}

/// Threads of the board, with a task page for the open one.
pub struct ThreadsView {
    store: Entity<Store>,
    open: Option<i64>,
    tab: Tab,
    file: Option<String>,
    /// A done task's page should switch to Results once its files load.
    auto_results: bool,
    input: Entity<TextInput>,
    scroll: ScrollHandle,
    last_len: usize,
    expanded: HashSet<usize>,
    /// The Stop button was clicked and the optional "Why?" is showing.
    stop_open: bool,
    /// One-line reason for a stop from the task page.
    why: Entity<TextInput>,
    /// Last column of each thread, to spot band changes.
    bands: HashMap<i64, Word>,
    /// Threads that just changed column: the column they left and when.
    moves: HashMap<i64, (Word, Instant)>,
    /// Threads already shown since the view last appeared; new ones rise unstaggered.
    seen: HashSet<i64>,
    /// Done band shows every card, not just the newest few.
    show_done: bool,
    /// Tasks being stopped and when, so their rows can play the leave animation.
    stopping: HashMap<i64, Instant>,
    /// The "Stop every running team?" confirm is open.
    confirm_all: bool,
}

impl ThreadsView {
    /// Build the threads view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let input = cx.new(|cx| TextInput::new("Answer it, or pass down new instructions…", cx));
        cx.subscribe(&input, |this, _, _: &Submit, cx| this.send(cx)).detach();
        let why = cx.new(|cx| TextInput::new("Why? (optional)", cx));
        cx.subscribe(&why, |this, _, _: &Submit, cx| this.stop_now(cx)).detach();
        let scroll = ScrollHandle::new();
        let (bands, moves, seen) = (HashMap::new(), HashMap::new(), HashSet::new());
        let expanded = HashSet::new();
        Self { store, open: None, tab: Tab::Trace, file: None, auto_results: false, input, scroll, last_len: 0, expanded, stop_open: false, why, bands, moves, seen, show_done: false, stopping: HashMap::new(), confirm_all: false }
    }

    /// Forget shown rows so the next render staggers them in again.
    pub fn reset_motion(&mut self) {
        self.seen.clear();
    }

    /// Open one task's page in place of the board.
    pub fn open_thread(&mut self, id: i64, cx: &mut Context<Self>) {
        self.open = Some(id);
        self.stop_open = false;
        self.why.update(cx, |i, cx| i.clear(cx));
        self.set_tab(Tab::Trace);
        self.file = None;
        self.auto_results = self.store.read(cx).tasks.iter().any(|t| t.id == id && t.status == TaskStatus::Done);
        self.expanded.clear();
        self.input.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| s.load_changes(Some(id), cx));
        cx.notify();
    }

    fn toggle_row(&mut self, i: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&i) {
            self.expanded.insert(i);
        }
        cx.notify();
    }

    fn trace_rows(&self, rows: &[TraceRow], th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if rows.is_empty() {
            return vec![div().text_size(px(12.5)).text_color(th.muted).child("No steps yet.").into_any_element()];
        }
        rows.iter().enumerate().map(|(i, r)| self.trace_row(i, r, th, cx)).collect()
    }

    fn trace_row(&self, i: usize, r: &TraceRow, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.expanded.contains(&i);
        let head = trace_head(i, r, open, th);
        let head = match r.detail {
            Some(_) => head.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.toggle_row(i, cx))).into_any_element(),
            None => head.into_any_element(),
        };
        let detail = r.detail.as_ref().filter(|_| open).map(|d| detail_block(i, d, th));
        let body = div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).child(head).children(detail);
        div().id(("trace-row", i)).flex().gap(px(10.)).child(tile(r, th)).child(body).into_any_element()
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.last_len = usize::MAX;
    }

    /// Back to the board.
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open.is_none() {
            return;
        }
        self.open = None;
        self.stop_open = false;
        self.store.update(cx, |s, cx| s.load_changes(None, cx));
        cx.notify();
    }

    /// Record column changes since the last render and drop finished ones.
    fn track_moves(&mut self, threads: &[Thread]) {
        let now = Instant::now();
        for t in threads {
            let col = t.word;
            match self.bands.insert(t.task.id, col) {
                Some(prev) if prev != col => _ = self.moves.insert(t.task.id, (prev, now)),
                _ => {}
            }
        }
        let keep = Duration::from_millis(LEAVE_MS + ARRIVE_MS + 100);
        self.moves.retain(|_, (_, at)| at.elapsed() < keep);
    }

    /// Stagger slot for a row: its index on a fresh view, else none.
    fn stagger(&self, i: usize) -> usize {
        if self.seen.is_empty() { i } else { 0 }
    }

    fn pick_file(&mut self, id: i64, path: String, cx: &mut Context<Self>) {
        self.file = Some(path.clone());
        self.store.update(cx, |s, cx| s.load_file(id, path, cx));
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.open else { return };
        let text = self.input.read(cx).content();
        if text.trim().is_empty() {
            return;
        }
        let open_q = self.store.read(cx).questions.iter().find(|q| {
            q.task_id == id && q.status == QuestionStatus::Open
        });
        let qid = open_q.map(|q| q.id);
        self.input.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| match qid {
            Some(q) => s.answer(q, text, None, cx),
            None => s.instruct_task(id, text, cx),
        });
    }

    fn fill(&mut self, text: &str, cx: &mut Context<Self>) {
        self.input.update(cx, |i, cx| i.set_content(text, cx));
    }

    fn task_data(&self, id: i64, cx: &Context<Self>) -> (Vec<Event>, Vec<Change>) {
        let s = self.store.read(cx);
        let events = match s.task_events.get(&id) {
            Some(list) => list.clone(),
            None => s.events.iter().filter(|e| e.task_id == Some(id)).cloned().collect(),
        };
        (events, s.changes.get(&id).cloned().unwrap_or_default())
    }

    /// One agent card: avatar, "Agent · task", "Client · reason · elapsed", and the seat mark when running.
    fn row(&self, t: &Thread, now: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = t.task.id;
        let color = th.client_color(t.task.client_id);
        let ring = if self.open == Some(id) { th.accent_text } else { gpui::transparent_black() };
        let live = self.store.read(cx).live.task(id).cloned();
        let state = if live.is_some() { char_state(&t.task.status, live.as_ref()) } else { status::char_state(t.word) };
        let reason = live.as_ref().map_or_else(|| t.reason.clone(), live_sub);
        let parts = [t.client.clone(), reason, elapsed(&t.task.updated_at, now)];
        let sub = parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" \u{b7} ");
        let title = format!("{} \u{b7} {}", t.name, t.task.title);
        let name = div().text_ellipsis().overflow_hidden().whitespace_nowrap().text_size(px(12.5));
        let name = name.font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(title);
        let text = div().flex().flex_col().flex_1().min_w_0().child(name).child(muted_line(sub, th).mt(px(2.)));
        let group = SharedString::from(format!("thread-card-{id}"));
        let hover = th.surface_2;
        let row = div()
            .id(("thread-row", id as usize))
            .group(group.clone())
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(9.))
            .mb(px(6.))
            .rounded(px(10.))
            .bg(th.surface)
            .shadow(th.card_shadow())
            .border_color(ring)
            .cursor_pointer()
            .hover(move |s| s.bg(hover).top(px(-1.)))
            .on_click(cx.listener(move |this, _, _, cx| this.open_thread(id, cx)))
            .child(character(state, px(34.), ("thread-rui", id as usize), color, th))
            .child(text)
            .children(seat_mark(&t.task.status))
            .children(can_stop(&t.task.status).then(|| {
                let pill = self.stop_pill(format!("thread-stop-{id}"), id, th, cx);
                pill.absolute().right(px(8.)).top(px(15.)).opacity(0.).group_hover(group, |s| s.opacity(1.))
            }));
        border_w(row, 1.5).into_any_element()
    }

    /// A row that rises in when new, or springs in after changing column.
    fn moving_row(&self, t: &Thread, i: usize, now: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = t.task.id;
        let row = self.row(t, now, th, cx);
        match self.phase(id) {
            Phase::Gone => return div().into_any_element(),
            Phase::Leaving => return leave(row, ("thread-stop-out", id as usize)),
            Phase::Idle => {}
        }
        let anim_id = ("thread-in", id as usize);
        if self.moves.contains_key(&id) {
            return arrive(row, anim_id).into_any_element();
        }
        let index = if self.seen.contains(&id) { 0 } else { self.stagger(i) };
        motion::rise(div().child(row), anim_id, index).into_any_element()
    }

    /// Ghost rows sliding out of the column a thread just left.
    fn leaving(&self, col: Word, all: &[Thread], now: i64, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let left_here = |t: &&Thread| match self.moves.get(&t.task.id) {
            Some((from, at)) => !self.stopping.contains_key(&t.task.id) && *from == col && at.elapsed() < Duration::from_millis(LEAVE_MS),
            None => false,
        };
        let ghosts: Vec<&Thread> = all.iter().filter(left_here).collect();
        ghosts.into_iter().map(|t| leave(self.row(t, now, th, cx), ("thread-out", t.task.id as usize))).collect()
    }

    fn tabs(&self, has_results: bool, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut tabs = vec![(Tab::Trace, "Trace"), (Tab::Changes, "Changes"), (Tab::Files, "Files")];
        if has_results {
            tabs.push((Tab::Results, "Results"));
        }
        let items: Vec<AnyElement> = tabs
            .into_iter()
            .enumerate()
            .map(|(i, (tab, name))| {
                let on = self.tab == tab;
                div()
                    .id(("task-tab", i))
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .h(px(28.))
                    .rounded(px(7.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .bg(if on { th.accent_fill } else { gpui::transparent_black() })
                    .text_color(if on { th.on_accent } else { th.muted })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.auto_results = false;
                        this.set_tab(tab);
                        cx.notify();
                    }))
                    .child(name)
                    .into_any_element()
            })
            .collect();
        div()
            .id("task-tabs")
            .flex()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(9.))
            .border_1()
            .border_color(th.hairline)
            .children(items)
            .into_any_element()
    }

    fn result_rows(&self, files: &[ResultFile], th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        files.iter().enumerate().map(|(i, f)| self.result_row(i, f, th, cx)).collect()
    }

    fn result_row(&self, i: usize, f: &ResultFile, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let agent = s.tasks.iter().find(|t| t.id == f.task_id).and_then(|t| member_name(t, &s.tasks, &s.roles));
        let meta = row_meta(&agent.or_else(|| f.agent.clone()).unwrap_or_default(), f.size);
        let (task_id, path, hover) = (f.task_id, f.path.clone(), th.surface_2);
        let names = div().flex().flex_col().flex_1().min_w_0().gap(px(2.));
        let names = names.child(mono(12., th.fg, f.path.clone()).truncate()).child(mono(11., th.muted, meta).truncate());
        div()
            .id(("result-row", i))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(9.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.store.update(cx, |s, cx| s.request_result(task_id, path.clone(), cx));
            }))
            .child(icon("file", 16., th.fg_2))
            .child(names)
            .into_any_element()
    }

    fn files_tab(&self, id: i64, rows: &[TraceRow], changes: &[Change], th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let names: Vec<String> = changes.iter().map(|c| c.file.clone()).collect();
        let files = touched_files(rows, &names);
        let items: Vec<AnyElement> = files
            .iter()
            .enumerate()
            .map(|(i, (path, changed))| self.file_item(id, i, path, *changed, th, cx))
            .collect();
        let none = files.is_empty().then(|| div().text_size(px(12.)).text_color(th.muted).child("No files yet."));
        let list = div().id("files-list").flex().flex_col().flex_none().w(px(186.)).gap(px(2.)).children(items).children(none);
        let view = self.file_viewer(id, th, cx);
        let row = div().id("files-tab").flex().gap(px(12.)).child(list).child(view);
        section("Files", vec![row.into_any_element()])
    }

    fn file_item(&self, id: i64, i: usize, path: &str, changed: bool, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let on = self.file.as_deref() == Some(path);
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let tag = if changed { tag(&th.done, "Changed here") } else { tag(&muted_status(th), "Read only") };
        let color = if on { th.on_accent } else { th.fg };
        let target = path.to_string();
        let hover = th.surface_2;
        let item = div()
            .id(("file-item", i))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(4.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(7.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.pick_file(id, target.clone(), cx)))
            .child(div().max_w_full().truncate().font_family(MONO).text_size(px(11.5)).text_color(color).child(name))
            .child(tag);
        match on {
            true => item.bg(th.sel).into_any_element(),
            false => item.hover(move |s| s.bg(hover)).into_any_element(),
        }
    }

    fn file_viewer(&self, id: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let note = |text: &str| div().text_size(px(12.)).text_color(th.muted).child(text.to_string()).into_any_element();
        let body = match &self.file {
            None => note("Pick a file to view it."),
            Some(path) => match &self.store.read(cx).file_view {
                Some((i, p, res)) if *i == id && p == path => file_lines(res, th),
                _ => note("Loading\u{2026}"),
            },
        };
        div()
            .id("file-viewer")
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .rounded(px(9.))
            .bg(th.well)
            .border_1()
            .border_color(th.hairline)
            .p(px(8.))
            .child(body)
            .into_any_element()
    }

    fn composer(&self, t: &Thread, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut chips: Vec<AnyElement> = Vec::new();
        if t.question.is_none() {
            for (i, text) in ["Narrow the scope", "Show me the diff before you continue"].into_iter().enumerate() {
                let c = small_chip(format!("suggest-{i}"), text, cx);
                chips.push(c.on_click(cx.listener(move |this, _, _, cx| this.fill(text, cx))).into_any_element());
            }
        }
        let send = primary_button("thread-send", "Send down", cx).on_click(cx.listener(|this, _, _, cx| this.send(cx)));
        let row = div().flex().items_center().gap(px(8.)).child(div().flex_1().child(self.input.clone())).child(send);
        div()
            .id("threads-composer")
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(10.))
            .pt(px(16.))
            .px(px(20.))
            .pb(px(20.))
            .border_t_1()
            .border_color(th.hairline)
            .children((!chips.is_empty()).then(|| div().flex().flex_wrap().gap(px(8.)).children(chips)))
            .child(row)
            .child(composer_hint(t, th))
            .into_any_element()
    }
}

fn small_chip(id: String, text: &str, cx: &mut Context<ThreadsView>) -> Stateful<Div> {
    chip(id, text, cx).h(px(26.)).px(px(10.)).text_size(px(11.5))
}

pub(super) fn muted_status(th: &Theme) -> Status {
    Status { base: th.muted, fg: th.muted, dim: th.hairline, rim: th.hairline }
}

fn composer_hint(t: &Thread, th: &Theme) -> Div {
    let text = format!("Goes to {}. If it can't run yet, RUI resumes what it waits on and tells you.", t.name);
    div().mt(px(-2.)).text_size(px(11.)).text_color(th.muted).child(text)
}

fn column_head(name: &str, count: usize, th: &Theme) -> AnyElement {
    let title = div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(name.to_string());
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .pt(px(10.))
        .px(px(12.))
        .pb(px(8.))
        .border_b_1()
        .border_color(th.hairline)
        .child(title)
        .child(mono(11., th.muted, count.to_string()))
        .into_any_element()
}

pub(super) fn section(title: &str, rows: Vec<AnyElement>) -> AnyElement {
    let head = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).child(title.to_uppercase());
    div().flex().flex_col().gap(px(12.)).child(head).children(rows).into_any_element()
}

type TileStyle = (Hsla, Hsla, Hsla);

fn tile_style(r: &TraceRow, th: &Theme) -> TileStyle {
    let family = |s: &Status| (s.dim, s.fg, s.rim);
    if r.error {
        return family(&th.block);
    }
    match r.kind {
        Kind::YouSaid => (th.accent_fill, th.on_accent, th.accent_fill),
        Kind::Stopped => family(&th.wait),
        Kind::Finished => family(&th.done),
        _ => (th.fg.opacity(0.09), th.muted, th.hairline),
    }
}

fn kind_icon(k: &Kind) -> &'static str {
    match k {
        Kind::Read => "file",
        Kind::Thought => "brain",
        Kind::Changed => "diff",
        Kind::Ran => "term",
        Kind::Stopped => "hand",
        Kind::YouSaid => "you",
        Kind::Finished => "check",
    }
}

fn tile(r: &TraceRow, th: &Theme) -> Div {
    let (bg, fg, rim) = tile_style(r, th);
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded(px(7.))
        .bg(bg)
        .border_1()
        .border_color(rim)
        .child(icon(kind_icon(&r.kind), 14., fg))
}

pub(super) fn chevron(open: bool, th: &Theme) -> impl IntoElement {
    let turn = Transformation::rotate(radians(if open { std::f32::consts::FRAC_PI_2 } else { 0. }));
    svg().path("icons/chevron.svg").size(px(12.)).flex_none().text_color(th.muted).with_transformation(turn)
}

fn trace_head(i: usize, r: &TraceRow, open: bool, th: &Theme) -> Stateful<Div> {
    let kind_color = if r.error { th.block.fg } else { th.fg_2 };
    let kind = div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(kind_color).child(r.kind.label());
    let arrow = r.detail.as_ref().map(|_| chevron(open, th));
    let meta = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(kind)
        .child(mono(10.5, th.muted, r.time.clone()))
        .child(div().flex_1())
        .children(arrow);
    let body = if r.kind == Kind::Thought { thought_text(r, th) } else { trace_line(r, th) };
    div().id(("trace-head", i)).flex().flex_col().gap(px(2.)).child(meta).child(body)
}

fn thought_text(r: &TraceRow, th: &Theme) -> AnyElement {
    markdown::render(&r.text, th.fg_2, 12.5, th)
}

/// One wrapped line: the code (tinted), then " — " and the result text.
fn trace_line(r: &TraceRow, th: &Theme) -> AnyElement {
    let color = if r.kind == Kind::YouSaid { th.fg } else { th.fg_2 };
    let line = div().text_size(px(12.5)).line_height(px(18.25)).text_color(color);
    let Some(code) = r.code.as_deref() else { return line.child(r.text.clone()).into_any_element() };
    let text = if r.text.is_empty() { code.to_string() } else { format!("{code} \u{2014} {}", r.text) };
    let marks = vec![(0..code.len(), markdown::code_style(th.fg))];
    line.child(StyledText::new(text).with_highlights(marks)).into_any_element()
}

fn detail_block(i: usize, d: &Detail, th: &Theme) -> AnyElement {
    match d {
        Detail::Patch(lines) => div()
            .id(("trace-detail", i))
            .overflow_hidden()
            .rounded(px(8.))
            .bg(th.surface_2)
            .child(diff_block(lines, th))
            .into_any_element(),
        Detail::Output { stdout, stderr } => {
            let mut rows = code_lines(stdout, th.fg_2);
            rows.extend(code_lines(stderr, th.block.fg));
            mono_block(i, rows, th)
        }
        Detail::Preview(text) => mono_block(i, text_lines(text, th.fg_2), th),
    }
}

pub(super) fn text_lines(text: &str, color: Hsla) -> Vec<AnyElement> {
    let line = |l: &str| {
        let shown = if l.is_empty() { " " } else { l };
        div().min_w_0().text_color(color).child(shown.to_string()).into_any_element()
    };
    text.lines().map(line).collect()
}

/// Command output: one row per line, cut with "…" instead of wrapping into a wall of text.
fn code_lines(text: &str, color: Hsla) -> Vec<AnyElement> {
    let line = |l: &str| {
        let shown = if l.is_empty() { " " } else { l };
        div().min_w_0().truncate().text_color(color).child(shown.to_string()).into_any_element()
    };
    text.lines().map(line).collect()
}

pub(super) fn mono_block(i: usize, rows: Vec<AnyElement>, th: &Theme) -> AnyElement {
    div()
        .id(("trace-detail", i))
        .flex()
        .flex_col()
        .overflow_hidden()
        .px(px(10.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(th.well)
        .font_family(MONO)
        .text_size(px(11.5))
        .line_height(px(17.))
        .children(rows)
        .into_any_element()
}

fn change_cards(changes: &[Change], th: &Theme) -> Vec<AnyElement> {
    if changes.is_empty() {
        let none = div().text_size(px(12.5)).text_color(th.muted).child("This thread has not changed any file yet.");
        return vec![none.into_any_element()];
    }
    changes.iter().enumerate().map(|(i, c)| change_card(i, c, th)).collect()
}

const MAX_DIFF_LINES: usize = 400;

fn change_card(i: usize, c: &Change, th: &Theme) -> AnyElement {
    let path = div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(11.5)).text_color(th.fg_2).child(c.file.clone());
    let head = div()
        .flex()
        .items_center()
        .gap(px(7.))
        .px(px(12.))
        .py(px(8.))
        .border_b_1()
        .border_color(th.hairline)
        .child(icon("diff", 14., th.muted))
        .child(path)
        .child(mono(11., th.done.fg, format!("+{}", c.added)))
        .child(mono(11., th.block.fg, format!("\u{2212}{}", c.removed)));
    let body = diff_block(&parse_diff(&c.diff), th);
    div()
        .id(("change-card", i))
        .mb(px(16.))
        .overflow_hidden()
        .rounded(px(9.))
        .bg(th.surface_2)
        .shadow(th.card_shadow())
        .child(head)
        .child(body)
        .into_any_element()
}

/// Diff lines as a column, capped, with a tail when cut.
fn diff_block(lines: &[DiffLine], th: &Theme) -> AnyElement {
    let rows: Vec<AnyElement> = lines.iter().take(MAX_DIFF_LINES).map(|l| diff_row(l, th).into_any_element()).collect();
    let more = (lines.len() > MAX_DIFF_LINES).then(|| div().px(px(12.)).py(px(6.)).text_size(px(11.)).text_color(th.muted).child("\u{2026} more lines"));
    div().flex().flex_col().children(rows).children(more).into_any_element()
}

fn diff_row(l: &DiffLine, th: &Theme) -> Div {
    if l.kind == DiffKind::Hunk {
        let hunk = mono(11., th.muted, l.text.clone());
        return hunk.px(px(12.)).py(px(3.)).bg(th.hairline).whitespace_nowrap().overflow_hidden();
    }
    let (bg, mark, num) = match l.kind {
        DiffKind::Add => (Some(th.done.dim), "+", l.new),
        DiffKind::Del => (Some(th.block.dim), "\u{2212}", l.old),
        _ => (None, " ", l.new),
    };
    let num = num.map(|n| n.to_string()).unwrap_or_default();
    let gutter = mono(10.5, th.muted, num).w(px(46.)).flex_none().pr(px(8.)).text_right();
    let marker = mono(11.5, th.muted, mark).w(px(14.)).flex_none();
    let text = code_text(&l.text, th);
    let row = div().flex().min_h(px(18.));
    let row = match bg {
        Some(b) => row.bg(b),
        None => row,
    };
    row.child(gutter).child(marker).child(text)
}

fn code_text(text: &str, th: &Theme) -> Div {
    let shown = if text.is_empty() { " " } else { text };
    mono(11.5, th.fg, shown.to_string()).flex_1().min_w_0().line_height(px(18.))
}

const MAX_FILE_LINES: usize = 1000;

fn file_lines(res: &Result<String, String>, th: &Theme) -> AnyElement {
    let text = match res {
        Ok(t) => t,
        Err(e) => return div().text_size(px(12.)).text_color(th.muted).child(e.clone()).into_any_element(),
    };
    let lines: Vec<AnyElement> = text.lines().take(MAX_FILE_LINES).enumerate().map(|(i, l)| {
        let gutter = mono(10.5, th.muted, (i + 1).to_string()).w(px(40.)).flex_none().pr(px(8.)).text_right();
        div().flex().min_h(px(18.)).child(gutter).child(code_text(l, th)).into_any_element()
    }).collect();
    let more = (text.lines().count() > MAX_FILE_LINES).then(|| div().text_size(px(11.)).text_color(th.muted).child("\u{2026} more lines"));
    div().flex().flex_col().children(lines).children(more).into_any_element()
}

/// Pulsing dot: 8px, run colour, opacity easing between 0.35 and 1.
fn pulse_dot(id: impl Into<gpui::ElementId>, th: &Theme) -> impl IntoElement {
    let wave = |t: f32| 0.675 + 0.325 * (t * std::f32::consts::TAU).sin();
    let anim = Animation::new(Duration::from_millis(1200)).repeat();
    let dot = div().flex_none().size(px(8.)).rounded_full().bg(th.run.base);
    dot.with_animation(id, anim, move |el, delta| el.opacity(wave(delta)))
}

fn muted_line(text: String, th: &Theme) -> Div {
    div().truncate().text_size(px(11.)).text_color(th.muted).child(text)
}

fn loops_of(t: &Task) -> Loops {
    Loops { used: t.loops_used, max: t.max_loops }
}

/// Loop dots: off is a faint tile, used are the accent, past the limit the block color.
fn loop_dots(loops: Loops, size: f32, th: &Theme) -> Div {
    let color = |d: Dot| match d {
        Dot::Off => th.fg.opacity(0.12),
        Dot::On => th.accent_text,
        Dot::Over => th.block.base,
    };
    let dots = loops.dots().into_iter().map(|d| div().flex_none().size(px(size)).rounded(px(size * 2. / 7.)).bg(color(d)));
    div().flex().items_center().gap(px(size * 3. / 7.)).children(dots)
}

/// Dots, "n/max" and a red "Over" tag once past the limit.
fn loops_line(loops: Loops, dot: f32, text: f32, th: &Theme) -> Div {
    let num_color = if loops.over() { th.block.fg } else { th.muted };
    let over = loops.over().then(|| tag(&th.block, "Over"));
    let line = div().flex().items_center().gap(px(7.)).child(loop_dots(loops, dot, th));
    line.child(mono(text, num_color, loops.text())).children(over)
}

/// "Effort" section of the drawer: what loops are for, and how many this task has used.
fn effort_section(task: &Task, th: &Theme) -> AnyElement {
    let loops = loops_of(task);
    let copy = match loops.over() {
        true => format!("Past the limit of {}. It escalates to its Lead rather than looping again.", task.max_loops),
        false => format!("A task escalates to its Lead rather than looping past {}.", task.max_loops),
    };
    let tone = if loops.over() { th.wait.fg } else { th.muted };
    let name = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child("Loops used");
    let text = div().text_size(px(11.5)).text_color(tone).child(copy);
    let left = div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(name).child(text);
    let row = div().id("drawer-effort").flex().items_center().gap(px(12.)).child(left).child(loops_line(loops, 7., 12., th));
    section("Effort", vec![row.into_any_element()])
}

/// Drawer slide and scrim fade, in milliseconds.
pub(super) const DRAWER_MS: u64 = 440;
pub(super) const SCRIM_MS: u64 = 320;
/// Band change: out of the old column, then a spring into the new one.
pub(super) const LEAVE_MS: u64 = 300;
const ARRIVE_MS: u64 = 620;

/// Spring in from the left after the old column's 300 ms exit.
fn arrive(row: AnyElement, id: (&'static str, usize)) -> impl IntoElement {
    let total = LEAVE_MS + ARRIVE_MS;
    div().child(row).with_animation(id, motion::once(total), move |el, d| {
        let p = delayed(d, total, LEAVE_MS, ARRIVE_MS);
        el.relative().left(px(-24. * (1. - spring(p)))).opacity(p)
    })
}

/// Slide 24 px right and fade out over 300 ms.
pub(super) fn leave(row: AnyElement, id: (&'static str, usize)) -> AnyElement {
    let anim = div().child(row).with_animation(id, motion::once(LEAVE_MS), |el, d| {
        let e = ease(d);
        el.relative().left(px(24. * e)).opacity(1. - e)
    });
    anim.into_any_element()
}

/// Sub-line text for a live task: "Running cargo test...".
fn live_sub(l: &LiveState) -> String {
    match live_action(l) {
        (label, Some(code)) => format!("{label} {code}\u{2026}"),
        (label, None) => format!("{label}\u{2026}"),
    }
}

/// Changes when a trace row is added or the live text grows.
fn scroll_sig(rows: usize, live: Option<&LiveState>) -> usize {
    rows * (1 << 20) + live.map_or(0, |l| l.text.len() + 1)
}

/// Tasks in Doing plus tasks with a live entry, counted once.
fn busy_count(doing: &[i64], live: &LiveSnapshot) -> usize {
    let extra = live.tasks.keys().filter_map(|k| k.parse::<i64>().ok()).filter(|id| !doing.contains(id));
    doing.len() + extra.count()
}

fn live_tile(th: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded(px(7.))
        .bg(th.run.dim)
        .border_1()
        .border_color(th.run.rim)
        .child(pulse_dot("trace-live-dot", th))
}

/// The row after the trace: what the agent is doing right now, with streaming text.
fn live_row(l: &LiveState, th: &Theme) -> AnyElement {
    let (label, code) = live_action(l);
    let kind = div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(th.run.fg).child(label);
    let meta = div().flex().items_center().gap(px(8.)).child(kind).child(mono(10.5, th.muted, "now"));
    let line = l.tool.as_ref().map(|_| live_code(code.as_deref().unwrap_or(""), th));
    let text = (!l.text.trim().is_empty()).then(|| markdown::render(&l.text, th.fg_2, 12.5, th));
    let body = div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(meta).children(line).children(text);
    div().id("trace-live").flex().gap(px(10.)).child(live_tile(th)).child(body).into_any_element()
}

fn live_code(code: &str, th: &Theme) -> AnyElement {
    let line = div().text_size(px(12.5)).line_height(px(18.25)).text_color(th.fg_2);
    if code.is_empty() {
        return line.child("\u{2026}").into_any_element();
    }
    let marks = vec![(0..code.len(), markdown::code_style(th.fg))];
    line.child(StyledText::new(format!("{code}\u{2026}")).with_highlights(marks)).into_any_element()
}

/// The Claude mark on a card whose agent is holding a seat.
fn seat_mark(status: &TaskStatus) -> Option<Div> {
    let running = matches!(status, TaskStatus::Clarifying | TaskStatus::InProgress);
    let spark = svg().path("icons/claude.svg").size(px(16.)).text_color(gpui::rgb(0xD97757));
    running.then(|| div().flex().flex_none().items_center().child(spark))
}

/// Counts for the lead line: agents holding a seat and threads that need the owner.
fn lead_counts(s: &Store, threads: &[Thread]) -> (usize, usize) {
    let doing: Vec<i64> = threads.iter().filter(|t| t.word == Word::Working).map(|t| t.task.id).collect();
    let needs = threads.iter().filter(|t| t.word == Word::NeedsYou).count();
    (busy_count(&doing, &s.live), needs)
}

/// "**4** working \u{b7} **1** need you"
fn lead_line(working: usize, needs: usize, th: &Theme) -> AnyElement {
    let bold = |n: usize| div().text_size(px(19.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(n.to_string());
    let verb = if needs == 1 { "needs you" } else { "need you" };
    let lead = div().id("threads-lead").flex().items_baseline().gap(px(5.)).text_size(px(15.)).text_color(th.fg_2);
    let need = div().flex().items_baseline().gap(px(5.)).child(bold(needs)).child(verb);
    lead.child(bold(working)).child("working \u{b7}").child(need).into_any_element()
}

impl Render for ThreadsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let threads = threads_of(self.store.read(cx));
        self.track_moves(&threads);
        self.prune_stopping(cx);
        let now = now_secs();
        let (running, needs) = lead_counts(self.store.read(cx), &threads);
        let bands = self.bands(&threads);
        let columns: Vec<AnyElement> = ROWS.map(|(c, _)| self.column(c, &bands[&c], &threads, now, &th, cx)).into();
        self.seen.extend(threads.iter().map(|t| t.task.id));
        let chart = self.chart(&threads, now, &th, cx);
        let open = self.open.and_then(|id| threads.iter().find(|t| t.task.id == id).cloned());
        if let Some(t) = open {
            return self.task_page(&t, &threads, now, &th, cx);
        }
        let board = div().id("threads-board").flex().gap(px(12.)).mr(px(40.)).pt(px(16.)).pb(px(28.)).children(columns);
        let body = div().id("threads-body").flex_1().min_h_0().overflow_y_scroll();
        let body = body.child(timeline::view(chart, &th)).child(board);
        let confirm = self.confirm_view(&th, cx);
        let top = section_top(Sec::Work, Some(lead_line(running, needs, &th)), self.stop_all_button(&th, cx), cx);
        let page = div().id("threads-page").relative().flex().flex_col().size_full();
        page.child(top.pr(px(40.))).child(body).children(confirm).into_any_element()
    }
}

/// How far a library drawer travels when it slides; covers its 620 px max width plus gap.
pub(super) const DRAWER_TRAVEL: f32 = 660.;

/// Eased visibility of a sliding drawer: rising when opening, falling when closing.
pub(super) fn shown(closing: bool, d: f32) -> f32 {
    if closing { 1. - ease(d) } else { ease(d) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(kind: &str, payload: Value) -> Event {
        serde_json::from_value(json!({"id":1,"task_id":2,"agent":"builder","kind":kind,
            "payload":payload,"created_at":"2026-11-15T14:40:00.000Z"}))
        .unwrap()
    }

    fn sum(kind: &str, payload: Value) -> (String, bool) {
        summarize(&ev(kind, payload))
    }

    #[test]
    fn assistant_text_and_tool() {
        let p = json!({"type":"assistant","message":{"content":[
            {"type":"text","text":"hi\nthere"},
            {"type":"tool_use","name":"mcp__rui__create_client","input":{"name":"acme"}}]}});
        let (s, err) = sum("assistant", p);
        assert_eq!(s, "hi there  → create_client({\"name\":\"acme\"})");
        assert!(!err);
    }

    #[test]
    fn assistant_truncates_and_missing() {
        let long = "x".repeat(300);
        let p = json!({"message":{"content":[{"type":"text","text":long}]}});
        assert_eq!(sum("assistant", p).0.chars().count(), 200);
        assert_eq!(sum("assistant", json!({})).0, "assistant");
        let big = json!({"message":{"content":[{"type":"tool_use","name":"t","input":{"a":"y".repeat(200)}}]}});
        assert_eq!(sum("assistant", big).0.chars().count(), "→ t()".chars().count() + 80);
    }

    #[test]
    fn toolresult_cases() {
        let ok = json!({"message":{"content":[{"type":"tool_result","content":"fine","is_error":false}]}});
        assert_eq!(sum("toolresult", ok), ("← ok".into(), false));
        let bad = json!({"message":{"content":[{"type":"tool_result",
            "content":[{"type":"text","text":"nope"}],"is_error":true}]}});
        assert_eq!(sum("toolresult", bad), ("← error: nope".into(), true));
        assert_eq!(sum("toolresult", json!({})), ("← ok".into(), false));
    }

    #[test]
    fn init_cases() {
        let p = json!({"type":"system","subtype":"init","model":"claude-opus-5-5"});
        assert_eq!(sum("init", p).0, "started · claude-opus-5-5");
        assert_eq!(sum("init", json!({"session_id":"s"})).0, "started");
    }

    #[test]
    fn result_cases() {
        let ok = json!({"is_error":false,"result":"x","total_cost_usd":0.12});
        assert_eq!(sum("result", ok), ("✓ finished ($0.12)".into(), false));
        assert_eq!(sum("result", json!({"text":"x"})), ("✓ finished".into(), false));
        assert_eq!(sum("result", json!({"is_error":true})), ("✗ failed".into(), true));
    }

    #[test]
    fn done_check_cases() {
        assert_eq!(sum("done_check", json!({"passed":true})), ("check passed".into(), false));
        let bad = json!({"passed":false,"output_tail":"first\nsecond"});
        assert_eq!(sum("done_check", bad), ("check failed first".into(), true));
        assert_eq!(sum("done_check", json!({})), ("check failed".into(), true));
    }

    #[test]
    fn scheduler_kinds() {
        assert_eq!(sum("commit", json!({"committed":true})).0, "committed");
        assert_eq!(sum("rollup", json!({"status":"done"})).0, "task → done");
        assert_eq!(sum("recovered", json!({"from":"a","to":"b"})).0, "recovered a → b");
        assert_eq!(sum("agent_error", json!({"error":"x"})), ("error: x".into(), true));
        assert_eq!(sum("scheduler_error", json!({})), ("error".into(), true));
        assert!(sum("lead_no_outcome", json!({})).1);
        assert_eq!(sum("stuck", json!({"reason":"loop"})), ("stuck: loop".into(), true));
        assert_eq!(sum("stuck", json!({})), ("stuck".into(), true));
        assert_eq!(sum("progress", json!({"note":"n"})).0, "progress: n");
        assert_eq!(sum("progress", json!({})).0, "progress");
        assert_eq!(sum("tool_call", json!({"tool":"t","args":{}})).0, "tool: t");
        assert_eq!(sum("other", json!(null)), ("other".into(), false));
        assert_eq!(sum("weird", json!({})).0, "weird");
    }

    #[test]
    fn busy_count_dedups() {
        let live = |ids: &[&str]| LiveSnapshot {
            tasks: ids.iter().map(|k| (k.to_string(), LiveState::default())).collect(),
            ..Default::default()
        };
        assert_eq!(busy_count(&[1, 2], &live(&[])), 2);
        assert_eq!(busy_count(&[1, 2], &live(&["2"])), 2);
        assert_eq!(busy_count(&[1, 2], &live(&["2", "5"])), 3);
        assert_eq!(busy_count(&[], &live(&["5", "x"])), 1);
    }

    #[test]
    fn live_sub_text() {
        let s = LiveState { tool: Some("Bash".into()), tool_input: r#"{"command":"cargo te"#.into(), ..Default::default() };
        assert_eq!(live_sub(&s), "Running cargo te\u{2026}");
        assert_eq!(live_sub(&LiveState { thinking: true, ..Default::default() }), "Thinking\u{2026}");
    }

    #[test]
    fn elapsed_formats() {
        let t = parse_secs("2026-11-15 09:00:00").unwrap();
        assert_eq!(parse_secs("1970-01-02T00:00:00.000Z"), Some(86_400));
        assert_eq!(elapsed("2026-11-15 09:00:00", t + 30), "just now");
        assert_eq!(elapsed("2026-11-15 09:00:00", t + 14 * 60), "14m");
        assert_eq!(elapsed("2026-11-15 09:00:00", t + 7_800), "2h 10m");
        assert_eq!(elapsed("2026-11-15 09:00:00", t + 7_200), "2h");
        assert_eq!(elapsed("2026-11-15 09:00:00", t + 3 * 86_400 + 5), "3d");
        assert_eq!(elapsed("bad", t), "");
    }
}
