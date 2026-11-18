//! The Inspector: a glass panel that slides in from the right with one Flow node's story and controls.

use super::flow::node_style;
use super::{ThreadsView, mono, section, summarize};
use crate::api::{self, HandBack, Inspector, Node, TakeOver, Thread};
use crate::theme::Theme;
use crate::views::decision::{CardData, first_line};
use crate::views::library::{after_slide, close_button, scrim, slide_overlay};
use crate::views::status::thread_word;
use crate::views::{chip, primary_button, tag};
use gpui::{AnyElement, ClipboardItem, Context, Div, FontWeight, Stateful, div, prelude::*, px};
use rui_core::Event;

const PANEL_W: f32 = 420.;

/// The open inspector: which node, its loaded events, and any take-over reply.
pub(super) struct Inspect {
    thread_id: i64,
    node_id: String,
    page: Option<Inspector>,
    error: Option<String>,
    take: Option<TakeOver>,
    closing: bool,
}

/// Add a newer page to what is loaded; without `append` the new page replaces it.
pub fn merge(old: Option<Inspector>, new: Inspector, append: bool) -> Inspector {
    let Some(mut old) = old.filter(|_| append) else { return new };
    old.events.extend(new.events);
    old.next_after_id = new.next_after_id;
    (old.cost_usd, old.done_checks) = (new.cost_usd, new.done_checks);
    old
}

/// Events split into what happened and the tool calls behind it.
pub fn split_events(evs: &[Event]) -> (Vec<&Event>, Vec<&Event>) {
    evs.iter().partition(|e| e.kind != "tool_call")
}

/// "$0.25"
pub fn cost_text(c: f64) -> String {
    format!("${c:.2}")
}

/// "log_decision({"line":"chose sqlite"})", the arguments cut to 100 characters.
pub fn tool_line(e: &Event) -> String {
    let name = e.payload["tool"].as_str().unwrap_or("tool");
    let args = &e.payload["args"];
    let args: String = if args.is_null() { String::new() } else { args.to_string().chars().take(100).collect() };
    format!("{name}({args})")
}

/// Header word for a node; the owner's own step reads as theirs.
fn node_word(status: &str) -> &'static str {
    if status == "owner" { "You're doing this step" } else { thread_word(status).label() }
}

/// The Scout can be asked about a step that failed, stalled or needs the owner.
pub fn can_diagnose(status: &str) -> bool {
    matches!(status, "failed" | "stalled" | "needs_you")
}

/// Run a daemon call off the main thread, then hand the result (or error text) back to the view.
fn call<T: Send + 'static>(
    cx: &mut Context<ThreadsView>,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    done: impl FnOnce(&mut ThreadsView, Result<T, String>, &mut Context<ThreadsView>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let res = cx.background_executor().spawn(async move { f() }).await;
        this.update(cx, |v, cx| done(v, res.map_err(|e| e.to_string()), cx)).ok();
    })
    .detach();
}

impl ThreadsView {
    /// Open the inspector on one node and load its first page.
    pub(super) fn inspect(&mut self, thread_id: i64, node_id: String, cx: &mut Context<Self>) {
        self.inspect = Some(Inspect { thread_id, node_id, page: None, error: None, take: None, closing: false });
        self.note.update(cx, |i, cx| i.clear(cx));
        self.fetch_page(None, cx);
        cx.notify();
    }

    /// Load events after `after` (appending), or reload from the start with `None`.
    fn fetch_page(&mut self, after: Option<i64>, cx: &mut Context<Self>) {
        let Some(i) = &self.inspect else { return };
        let (tid, nid) = (i.thread_id, i.node_id.clone());
        let want = nid.clone();
        call(cx, move || api::get_thread_node(tid, &nid, after), move |v, res, cx| {
            let Some(i) = v.inspect.as_mut().filter(|i| i.node_id == want) else { return };
            match res {
                Ok(p) => (i.page, i.error) = (Some(merge(i.page.take(), p, after.is_some())), None),
                Err(e) => i.error = Some(e),
            }
            cx.notify();
        });
    }

    pub(super) fn close_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.inspect.as_mut() else { return };
        i.closing = true;
        cx.notify();
        after_slide(cx, |v, cx| {
            if v.inspect.as_ref().is_some_and(|i| i.closing) {
                v.inspect = None;
                cx.notify();
            }
        });
    }

    /// After an action: toast, reload the thread and this node's page.
    fn acted(&mut self, title: &str, chip: String, cx: &mut Context<Self>) {
        let chips = if chip.is_empty() { vec![] } else { vec![chip] };
        let tid = self.inspect.as_ref().map(|i| i.thread_id);
        self.store.update(cx, |s, cx| {
            s.show_toast(title, chips, cx);
            if let Some(id) = tid {
                s.load_thread(id, cx);
            }
        });
        self.fetch_page(None, cx);
    }

    fn failed(&mut self, err: String, cx: &mut Context<Self>) {
        let short: String = err.chars().take(80).collect();
        self.store.update(cx, |s, cx| s.show_toast("Couldn't do that", vec![short], cx));
    }

    fn set_checkpoint(&mut self, sid: i64, on: bool, cx: &mut Context<Self>) {
        call(cx, move || api::set_checkpoint(sid, on), move |v, res, cx| match res {
            Ok(()) => v.acted(if on { "Held before this step" } else { "Released this step" }, String::new(), cx),
            Err(e) => v.failed(e, cx),
        });
    }

    fn take_over(&mut self, sid: i64, cx: &mut Context<Self>) {
        call(cx, move || api::take_over(sid), |v, res, cx| match res {
            Ok(t) => {
                let branch = t.branch.clone();
                if let Some(i) = v.inspect.as_mut() {
                    i.take = Some(t);
                }
                v.acted("You have this step", branch, cx);
            }
            Err(e) => v.failed(e, cx),
        });
    }

    pub(super) fn hand_back(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(sid) = self.inspected_subtask(cx) else { return };
        let note = self.note.read(cx).content().trim().to_string();
        call(cx, move || api::hand_back(sid, &note, force), |v, res: Result<HandBack, String>, cx| match res {
            Ok(h) => {
                v.note.update(cx, |i, cx| i.clear(cx));
                if let Some(i) = v.inspect.as_mut() {
                    i.take = None;
                }
                v.acted("Handed back", format!("Check {}", if h.passed { "passed" } else { "failed" }), cx);
            }
            Err(e) => v.failed(e, cx),
        });
    }

    fn retry(&mut self, sid: i64, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.retry_task(sid, cx));
        self.fetch_page(None, cx);
    }

    /// The subtask behind the open node, from the cached thread.
    fn inspected_subtask(&self, cx: &Context<Self>) -> Option<i64> {
        let i = self.inspect.as_ref()?;
        let s = self.store.read(cx);
        let t = s.thread_cache.get(&i.thread_id)?;
        t.nodes.iter().find(|n| n.id == i.node_id)?.subtask_id
    }

    /// The sliding panel over the task page, when a node is open.
    pub(super) fn inspector_view(&mut self, thread: Option<&Thread>, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = thread?;
        let node = self.inspected_node(t, cx)?;
        let card = CardData::from_node(t, &node).map(|d| self.cards.get(&d, false, &self.store, Self::on_card, cx).into_any_element());
        let i = self.inspect.as_ref()?;
        let body = div().id("inspector-body").flex_1().min_h_0().overflow_y_scroll().px(px(20.)).pb(px(20.));
        let body = body.flex().flex_col().gap(px(18.)).children(card).children(self.controls(&node, i, th, cx));
        let body = body.children(self.sections(&node, i, th, cx));
        let panel = panel_shell(th).child(self.inspector_head(&node, th, cx)).child(body);
        let back = scrim("inspector-scrim", Self::close_inspector, cx);
        Some(slide_overlay("inspector", back, panel.into_any_element(), i.closing))
    }

    /// The open node; a question just answered is gone from the thread, so it comes from the held answer.
    fn inspected_node(&self, t: &Thread, cx: &Context<Self>) -> Option<Node> {
        let id = &self.inspect.as_ref()?.node_id;
        if let Some(n) = t.nodes.iter().find(|n| &n.id == id) {
            return Some(n.clone());
        }
        let held = self.store.read(cx).sending.values().find(|s| s.card.thread_id == t.id && &s.card.node_id == id)?;
        let d = held.card.decision.clone();
        let title = first_line(&d.ask);
        Some(Node { id: id.clone(), kind: "decision".into(), status: "needs_you".into(), title, decision: Some(d), ..Default::default() })
    }

    fn inspector_head(&self, n: &Node, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let title = div().text_size(px(16.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(n.title.clone());
        let stalled = (n.status == "stalled").then(|| "No activity for 10 min".to_string());
        let reason = n.reason.clone().or(stalled).map(|r| div().text_size(px(11.5)).text_color(th.fg_2).child(r));
        let word = tag(node_style(&n.status, th), node_word(&n.status));
        let agent = n.agent.clone().map(|a| mono(10.5, th.muted, a));
        let meta = div().flex().items_center().gap(px(8.)).child(word).children(agent);
        let info = div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).child(title).child(meta).children(reason);
        let close = close_button("inspector-close", Self::close_inspector, th, cx);
        div().flex().items_start().gap(px(12.)).p(px(20.)).child(info).child(close).into_any_element()
    }

    fn sections(&self, n: &Node, i: &Inspect, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(p) = &i.page else {
            let text = i.error.clone().unwrap_or_else(|| "Loading\u{2026}".into());
            return vec![div().text_size(px(12.)).text_color(th.muted).child(text).into_any_element()];
        };
        let (happened, tools) = split_events(&p.events);
        let mut rows: Vec<AnyElement> = happened.iter().map(|e| event_row(e, th)).collect();
        rows.extend(p.next_after_id.map(|a| self.load_more(a, cx)));
        let tools = tools.iter().map(|e| mono(11., th.fg_2, tool_line(e)).into_any_element()).collect();
        let checks = p.done_checks.iter().enumerate().map(|(k, c)| check_row(k, c, th)).collect();
        let cost = vec![mono(12., th.fg, cost_text(p.cost_usd)).into_any_element()];
        let mut out = vec![section("What happened", or_none(rows, th))];
        out.push(section("What it produced", or_none(produced(n, th), th)));
        out.extend([section("Tool calls", or_none(tools, th)), section("Done check", or_none(checks, th))]);
        out.push(section("Cost", cost));
        out
    }

    fn load_more(&self, after: i64, cx: &mut Context<Self>) -> AnyElement {
        let more = chip("inspector-more", "Load more", cx);
        more.on_click(cx.listener(move |this, _, _, cx| this.fetch_page(Some(after), cx))).into_any_element()
    }

    /// Owner controls for a subtask node, by its status.
    fn controls(&self, n: &Node, i: &Inspect, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sid = n.subtask_id?;
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.extend(self.checkpoint_toggle(n, sid, th, cx));
        rows.extend(i.take.as_ref().map(|t| take_box(t, th, cx)));
        if n.status == "owner" {
            rows.push(self.hand_back_box(cx));
        } else if n.kind == "subtask" && n.status != "done" {
            let take = chip("inspector-take", "Take over", cx).on_click(cx.listener(move |this, _, _, cx| this.take_over(sid, cx)));
            rows.push(take.into_any_element());
        }
        if n.status == "failed" {
            let retry = chip("inspector-retry", "Retry", cx).on_click(cx.listener(move |this, _, _, cx| this.retry(sid, cx)));
            rows.push(retry.into_any_element());
        }
        if can_diagnose(&n.status) {
            let ask = chip("inspector-diagnose", "Ask Scout to diagnose", cx);
            let ask = ask.on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.diagnose(sid, cx))));
            rows.push(ask.into_any_element());
        }
        (!rows.is_empty()).then(|| section("Your controls", rows))
    }

    /// "Wait for me before this step", only before the step runs; on while held at a checkpoint.
    fn checkpoint_toggle(&self, n: &Node, sid: i64, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if n.kind != "subtask" || !matches!(n.status.as_str(), "planned" | "waiting") {
            return None;
        }
        let on = n.status == "waiting" && n.reason.is_some();
        let fill = if on { th.accent_fill } else { gpui::transparent_black() };
        let mark = div().size(px(14.)).rounded(px(4.)).border_1().border_color(th.accent_text).bg(fill);
        let text = div().text_size(px(12.5)).text_color(th.fg).child("Wait for me before this step");
        let row = div().id("inspector-checkpoint").flex().items_center().gap(px(8.)).cursor_pointer().child(mark).child(text);
        Some(row.on_click(cx.listener(move |this, _, _, cx| this.set_checkpoint(sid, !on, cx))).into_any_element())
    }

    fn hand_back_box(&self, cx: &mut Context<Self>) -> AnyElement {
        let back = primary_button("inspector-hand-back", "Hand back", cx).on_click(cx.listener(|this, _, _, cx| this.hand_back(false, cx)));
        let force = chip("inspector-force", "Force done", cx).on_click(cx.listener(|this, _, _, cx| this.hand_back(true, cx)));
        let buttons = div().flex().items_center().gap(px(8.)).child(back).child(force);
        div().flex().flex_col().gap(px(8.)).child(self.note.clone()).child(buttons).into_any_element()
    }
}

fn panel_shell(th: &Theme) -> Stateful<Div> {
    let side = div().id("inspector-panel").occlude().absolute().right(px(10.)).top(px(10.)).bottom(px(10.)).w(px(PANEL_W));
    let side = side.flex().flex_col().overflow_hidden().rounded(px(16.)).bg(th.glass);
    side.shadow(th.card_shadow()).border_1().border_color(th.glass_rim)
}

fn or_none(rows: Vec<AnyElement>, th: &Theme) -> Vec<AnyElement> {
    if !rows.is_empty() {
        return rows;
    }
    vec![div().text_size(px(12.)).text_color(th.muted).child("Nothing yet.").into_any_element()]
}

/// "10:42  started · opus"
fn event_row(e: &Event, th: &Theme) -> AnyElement {
    let (text, bad) = summarize(e);
    let time = mono(10.5, th.muted, e.created_at.get(11..16).unwrap_or("").to_string());
    let text = div().flex_1().min_w_0().text_size(px(12.)).text_color(if bad { th.block.fg } else { th.fg_2 }).child(text);
    div().flex().items_start().gap(px(10.)).child(time.flex_none()).child(text).into_any_element()
}

/// The node's output: label, link and preview.
fn produced(n: &Node, th: &Theme) -> Vec<AnyElement> {
    let Some(o) = &n.output else { return vec![] };
    let label = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(o.label.clone());
    let url = o.url.clone().map(|u| mono(11., th.accent_text, u).into_any_element());
    let preview = o.preview.clone().map(|p| div().text_size(px(12.)).text_color(th.fg_2).child(p).into_any_element());
    std::iter::once(label.into_any_element()).chain(url).chain(preview).collect()
}

fn check_row(k: usize, c: &api::DoneCheck, th: &Theme) -> AnyElement {
    let st = if c.passed { &th.done } else { &th.block };
    let word = tag(st, if c.passed { "Passed" } else { "Failed" });
    let tail = (!c.output_tail.trim().is_empty()).then(|| {
        let block = div().id(("inspector-check", k)).max_h(px(160.)).overflow_y_scroll().p(px(8.)).rounded(px(8.)).bg(th.well);
        block.child(mono(11., th.fg_2, c.output_tail.clone()).whitespace_normal())
    });
    div().flex().flex_col().gap(px(6.)).child(word).children(tail).into_any_element()
}

/// Where the owner works after a take-over, with a copy button.
fn take_box(t: &TakeOver, th: &Theme, cx: &mut Context<ThreadsView>) -> AnyElement {
    let line = match &t.worktree {
        Some(w) => format!("cd {w}  # branch {}", t.branch),
        None => format!("git checkout {}", t.branch),
    };
    let text = mono(11.5, th.fg, line.clone()).flex_1().min_w_0();
    let copy = chip("inspector-copy", "Copy", cx).on_click(cx.listener(move |_, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(line.clone()))));
    let row = div().flex().items_center().gap(px(8.)).p(px(8.)).rounded(px(8.)).bg(th.well).child(text).child(copy);
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::DoneCheck;
    use serde_json::json;

    fn ev(id: i64, kind: &str, payload: serde_json::Value) -> Event {
        serde_json::from_value(json!({"id":id,"task_id":2,"agent":"b","kind":kind,"payload":payload,"created_at":"2026-11-18 10:42:00"})).unwrap()
    }

    fn page(ids: &[i64], next: Option<i64>, cost: f64) -> Inspector {
        let events = ids.iter().map(|&i| ev(i, "progress", json!({}))).collect();
        Inspector { node_id: "n".into(), events, cost_usd: cost, done_checks: vec![DoneCheck::default()], next_after_id: next }
    }

    #[test]
    fn merge_appends_or_replaces() {
        let m = merge(Some(page(&[1, 2], Some(2), 0.1)), page(&[3], None, 0.3), true);
        assert_eq!(m.events.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!((m.next_after_id, m.cost_usd), (None, 0.3));
        let r = merge(Some(page(&[1, 2], Some(2), 0.1)), page(&[1], None, 0.2), false);
        assert_eq!(r.events.len(), 1);
        assert_eq!(merge(None, page(&[5], None, 0.), true).events[0].id, 5);
    }

    #[test]
    fn tool_calls_split_out() {
        let evs = vec![ev(1, "init", json!({})), ev(2, "tool_call", json!({"tool":"log_decision","args":{"line":"x"}}))];
        let (happened, tools) = split_events(&evs);
        assert_eq!((happened.len(), tools.len()), (1, 1));
        assert_eq!(tool_line(tools[0]), r#"log_decision({"line":"x"})"#);
        assert_eq!(tool_line(&ev(3, "tool_call", json!({}))), "tool()");
    }

    #[test]
    fn cost_and_words() {
        assert_eq!((cost_text(0.0), cost_text(1.256)), ("$0.00".to_string(), "$1.26".to_string()));
        assert_eq!(node_word("owner"), "You're doing this step");
        assert_eq!(node_word("failed"), "Needs you");
    }

    #[test]
    fn scout_is_offered_for_failed_stalled_and_needs_you() {
        assert!(["failed", "stalled", "needs_you"].iter().all(|s| can_diagnose(s)));
        assert!(!can_diagnose("done") && !can_diagnose("running"));
    }
}
