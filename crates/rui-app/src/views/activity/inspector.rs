//! The Inspector: a docked side panel on the thread page with one Flow node's story and controls.

use super::flow::node_style;
use super::{ThreadsView, icon, is_empty_turn, mono, parse_secs, section, summarize};
use crate::api::{self, HandBack, Inspector, Node, TakeOver, Thread};
use crate::theme::Theme;
use crate::views::decision::{CardData, DecisionCard, first_line, reversible_text, risk_label, risk_style};
use crate::views::status::thread_word;
use crate::views::{chip, primary_button, tag};
use gpui::{AnyElement, ClipboardItem, Context, Div, Entity, FontWeight, Stateful, div, prelude::*, px};
use rui_core::Event;

const PANEL_W: f32 = 380.;

/// The open inspector: which node, its loaded events, and any take-over reply.
pub(super) struct Inspect {
    thread_id: i64,
    node_id: String,
    page: Option<Inspector>,
    error: Option<String>,
    take: Option<TakeOver>,
    /// The owner clicked this node; otherwise it is the thread's default node.
    picked: bool,
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
/// (what happened, tool calls); turns with nothing to show are left out.
pub fn split_events(evs: &[Event]) -> (Vec<&Event>, Vec<&Event>) {
    evs.iter().filter(|e| !is_empty_turn(e)).partition(|e| e.kind != "tool_call")
}

/// The node the panel shows when nothing is clicked: one that needs the owner, else a running one, else the last done.
pub fn default_node(nodes: &[Node]) -> Option<&Node> {
    let needs = nodes.iter().find(|n| n.status == "needs_you" || n.kind == "decision");
    let running = || nodes.iter().find(|n| matches!(n.status.as_str(), "running" | "working"));
    let done = || nodes.iter().rev().find(|n| n.status == "done");
    needs.or_else(running).or_else(done).or(nodes.first())
}

/// (year, month, day) of a day count since 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    (yoe + era * 400 + i64::from(mp >= 10), if mp < 10 { mp + 3 } else { mp - 9 }, d)
}

/// "today 10:24" when `at` falls on the same local day as `now`, else "11-18 10:24".
/// Both are unix seconds in UTC; `offset` is the local offset east of UTC in seconds.
pub fn when_text(at: i64, now: i64, offset: i64) -> String {
    let (local, today) = (at + offset, now + offset);
    let m = local.rem_euclid(86_400) / 60;
    let clock = format!("{:02}:{:02}", m / 60, m % 60);
    if local.div_euclid(86_400) == today.div_euclid(86_400) {
        return format!("today {clock}");
    }
    let (_, month, day) = civil(local.div_euclid(86_400));
    format!("{month:02}-{day:02} {clock}")
}

/// "Builder (agent) \u{b7} today 10:24"; an older day reads "11-18 10:24". Unknown parts are left out.
pub fn who_when(n: &Node, now: i64, offset: i64) -> String {
    let who = match &n.agent {
        _ if n.status == "owner" => Some("You".to_string()),
        Some(a) if !a.is_empty() => Some(format!("{a} (agent)")),
        _ => None,
    };
    let when = parse_secs(&n.started_at).map(|at| when_text(at, now, offset));
    [who, when].into_iter().flatten().collect::<Vec<_>>().join(" \u{b7} ")
}

/// "Low risk \u{b7} reversible": how risky the decision is and whether it can be undone.
pub fn risk_chip_text(risk: &str, reversible: Option<bool>) -> String {
    let rev = reversible_text(reversible).map(str::to_lowercase);
    [Some(risk_label(risk).to_string()), rev].into_iter().flatten().collect::<Vec<_>>().join(" \u{b7} ")
}

/// The "What happened" paragraph of a decision: why it came up, else the full ask, else the node's reason.
pub fn happened_text(n: &Node, d: &crate::api::Decision) -> Option<String> {
    let why = Some(d.why.trim().to_string()).filter(|w| !w.is_empty());
    let ask = Some(d.ask.trim().to_string()).filter(|a| !a.is_empty());
    why.or(ask).or_else(|| n.reason.clone().filter(|r| !r.trim().is_empty()))
}

/// The last loaded event's id, once every page is loaded; new events come after it.
fn tail_after(i: &Inspect) -> Option<i64> {
    let p = i.page.as_ref().filter(|p| p.next_after_id.is_none())?;
    Some(p.events.last().map_or(0, |e| e.id))
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
    /// The node the side panel shows for thread `tid`, if it is open on that thread.
    pub(super) fn selected_node(&self, tid: i64) -> Option<&str> {
        self.inspect.as_ref().filter(|i| i.thread_id == tid).map(|i| i.node_id.as_str())
    }

    /// Show the clicked node in the panel and load its first page.
    pub(super) fn inspect(&mut self, thread_id: i64, node_id: String, cx: &mut Context<Self>) {
        self.show_node(thread_id, node_id, true, cx);
        cx.notify();
    }

    fn show_node(&mut self, thread_id: i64, node_id: String, picked: bool, cx: &mut Context<Self>) {
        self.inspect = Some(Inspect { thread_id, node_id, page: None, error: None, take: None, picked });
        self.note.update(cx, |i, cx| i.clear(cx));
        self.fetch_page(None, cx);
        self.follow(cx);
    }

    /// Unless the owner clicked a node, follow the thread's default node as it changes.
    fn sync_default(&mut self, t: &Thread, cx: &mut Context<Self>) {
        let cur = self.inspect.as_ref().filter(|i| i.thread_id == t.id);
        if cur.is_some_and(|i| i.picked) {
            return;
        }
        let Some(id) = default_node(&t.nodes).map(|n| n.id.clone()) else { return };
        if cur.is_some_and(|i| i.node_id == id) {
            return;
        }
        self.show_node(t.id, id, false, cx);
    }

    /// While this node stays open, load its new events every few seconds.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let Some(want) = self.inspect.as_ref().map(|i| i.node_id.clone()) else { return };
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
            let open = this.update(cx, |v, cx| {
                let Some(i) = v.inspect.as_ref().filter(|i| i.node_id == want) else { return false };
                if let Some(after) = tail_after(i) {
                    v.fetch_page(Some(after), cx);
                }
                true
            });
            if !open.unwrap_or(false) {
                break;
            }
        })
        .detach();
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

    /// The docked panel beside the task page, showing the clicked or default node.
    pub(super) fn inspector_view(&mut self, thread: Option<&Thread>, now: i64, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = thread?;
        self.sync_default(t, cx);
        let node = self.inspected_node(t, cx)?;
        let data = CardData::from_node(t, &node);
        let card = data.as_ref().map(|d| self.cards.bare(d, &self.store, Self::on_card, cx));
        let i = self.inspect.as_ref()?;
        let rows = match (&data, card) {
            (Some(d), Some(card)) => self.decision_rows(&node, d, card, i, th, cx),
            _ => self.node_rows(&node, i, th, cx),
        };
        let body = div().id("inspector-body").flex_1().min_h_0().overflow_y_scroll().px(px(20.)).py(px(18.));
        let body = body.flex().flex_col().gap(px(18.)).children(rows);
        let panel = panel_shell(th).child(self.inspector_head(&node, now, th)).child(body);
        Some(panel.into_any_element())
    }

    /// A step that isn't a decision: its controls, then the log, output, tool calls, check and cost.
    fn node_rows(&self, n: &Node, i: &Inspect, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = self.controls(n, i, th, cx).into_iter().collect();
        rows.extend(self.sections(n, i, th, cx));
        rows
    }

    /// A decision: what happened, then the amber "If approved" box with its buttons.
    fn decision_rows(&self, n: &Node, d: &CardData, card: Entity<DecisionCard>, i: &Inspect, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let story = happened_text(n, &d.decision).map(|text| {
            let label = div().text_size(px(11.5)).text_color(th.muted).child("What happened");
            let text = div().mt(px(6.)).text_size(px(13.5)).text_color(th.fg).child(text);
            div().child(label).child(text).into_any_element()
        });
        let mut rows: Vec<AnyElement> = story.into_iter().collect();
        rows.push(self.approve_box(n, d, card, th, cx));
        rows.extend(self.controls(n, i, th, cx));
        rows
    }

    /// "If approved, <agent> will", the numbered steps, the review link and the buttons.
    fn approve_box(&self, n: &Node, d: &CardData, card: Entity<DecisionCard>, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let steps = &d.decision.if_approved;
        let agent = n.agent.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| "the team".into());
        let heading = div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(th.wait.fg).child(format!("If approved, {agent} will"));
        let line = |(k, s): (usize, &String)| div().mt(px(4.)).text_size(px(13.)).text_color(th.fg).child(format!("{}. {s}", k + 1));
        let list = (!steps.is_empty()).then(|| div().child(heading).children(steps.iter().enumerate().map(line)));
        let box_ = div().id("inspector-decision").flex().flex_col().gap(px(12.)).p(px(14.)).rounded(px(12.));
        let box_ = box_.bg(th.wait.dim).border_1().border_color(th.wait.rim);
        box_.children(list).children(self.review_row(d, th, cx)).child(card).into_any_element()
    }

    /// "Review the change first": opens the pull request, or the Changes tab when there is no link.
    fn review_row(&self, d: &CardData, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = d.decision.review.as_ref()?;
        let (url, tid) = (r.url.clone(), d.thread_id);
        let sub = url.clone().unwrap_or_else(|| "See what it changed".into());
        let mark = div().flex().flex_none().items_center().justify_center().size(px(28.)).rounded(px(8.)).bg(th.done.dim);
        let mark = mark.child(icon("diff", 14., th.done.fg));
        let title = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child("Review the change first");
        let sub = div().truncate().text_size(px(11.5)).text_color(th.muted).child(sub);
        let text = div().flex().flex_col().flex_1().min_w_0().child(title).child(sub);
        let hover = th.surface_2;
        let row = div().id("inspector-review").flex().items_center().gap(px(10.)).p(px(10.)).rounded(px(10.));
        let row = row.bg(th.surface).cursor_pointer().hover(move |s| s.bg(hover));
        let row = row.child(mark).child(text).child(icon("chevron", 12., th.fg_2));
        let open = move |this: &mut Self, cx: &mut Context<Self>| match &url {
            Some(u) => cx.open_url(u),
            None => this.show_changes(tid, cx),
        };
        Some(row.on_click(cx.listener(move |this, _, _, cx| open(this, cx))).into_any_element())
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

    /// Chips (state and risk), the title, and "who \u{b7} when" for the open node.
    fn inspector_head(&self, n: &Node, now: i64, th: &Theme) -> AnyElement {
        let decision = n.decision.as_ref();
        let ask = n.status == "needs_you" && decision.is_some();
        let label = if n.status == "needs_you" { "Needs a decision" } else { node_word(&n.status) };
        let style = if ask { &th.wait } else { node_style(&n.status, th) };
        let risk = decision.map(|d| tag(&risk_style(&d.risk, th), &risk_chip_text(&d.risk, d.reversible)));
        let chips = div().flex().flex_wrap().items_center().gap(px(6.)).child(tag(style, label)).children(risk);
        let title = div().mt(px(10.)).text_size(px(20.)).line_height(px(26.)).font_weight(FontWeight::BOLD).text_color(th.fg);
        let title = title.line_clamp(3).child(n.title.clone());
        let who = Some(who_when(n, now, super::timeline::local_offset())).filter(|w| !w.is_empty()).map(|w| self.who_row(n, w, th));
        let stalled = (n.status == "stalled").then(|| "No activity for 10 min".to_string());
        let reason = n.reason.clone().filter(|_| decision.is_none()).or(stalled);
        let reason = reason.map(|r| div().mt(px(8.)).text_size(px(11.5)).text_color(th.fg_2).child(r));
        let head = div().px(px(20.)).pt(px(18.)).pb(px(14.)).border_b_1().border_color(th.hairline);
        head.child(chips).child(title).children(who).children(reason).into_any_element()
    }

    /// The agent's initial in a small square, then "Builder (agent) \u{b7} today 10:24".
    fn who_row(&self, n: &Node, text: String, th: &Theme) -> Div {
        let initial = n.agent.as_deref().and_then(|a| a.chars().next()).map(|c| c.to_uppercase().to_string());
        let badge = div().flex().flex_none().items_center().justify_center().size(px(22.)).rounded(px(6.)).bg(th.run.dim);
        let badge = initial.map(|c| badge.text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(th.run.fg).child(c));
        let text = div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(th.muted).child(text);
        div().mt(px(8.)).flex().items_center().gap(px(8.)).children(badge).child(text)
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

/// The docked right column: a rounded card beside the main column.
fn panel_shell(th: &Theme) -> Stateful<Div> {
    let side = div().id("inspector-panel").flex_none().w(px(PANEL_W)).my(px(10.)).mr(px(10.));
    side.flex().flex_col().overflow_hidden().rounded(px(16.)).bg(th.surface).shadow(th.card_shadow())
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
    fn empty_assistant_turns_are_hidden() {
        let think = json!({"message": {"content": [{"type": "thinking", "thinking": "hmm"}]}});
        let said = json!({"message": {"content": [{"type": "text", "text": "Splitting it"}]}});
        let used = json!({"message": {"content": [{"type": "tool_use", "name": "mark_split", "input": {}}]}});
        let evs = vec![ev(1, "assistant", think), ev(2, "assistant", said), ev(3, "assistant", used)];
        let (happened, _) = split_events(&evs);
        assert_eq!(happened.iter().map(|e| e.id).collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn cost_and_words() {
        assert_eq!((cost_text(0.0), cost_text(1.256)), ("$0.00".to_string(), "$1.26".to_string()));
        assert_eq!(node_word("owner"), "You're doing this step");
        assert_eq!(node_word("failed"), "Needs you");
    }

    fn node(id: &str, kind: &str, status: &str) -> Node {
        Node { id: id.into(), kind: kind.into(), status: status.into(), ..Default::default() }
    }

    #[test]
    fn default_node_prefers_decision_then_running_then_last_done() {
        let ns = [node("a", "subtask", "done"), node("b", "subtask", "running"), node("c", "decision", "needs_you"), node("d", "subtask", "done")];
        assert_eq!(default_node(&ns).unwrap().id, "c");
        assert_eq!(default_node(&ns[..2]).unwrap().id, "b");
        assert_eq!(default_node(&[ns[0].clone(), ns[3].clone()]).unwrap().id, "d");
        assert!(default_node(&[]).is_none());
    }

    #[test]
    fn who_when_says_today_for_this_day() {
        let mut n = node("a", "subtask", "done");
        n.agent = Some("Builder".into());
        n.started_at = "2026-11-18 10:24:00".into();
        let at = parse_secs(&n.started_at).unwrap();
        assert_eq!(who_when(&n, at + 60, 0), "Builder (agent) \u{b7} today 10:24");
        assert_eq!(who_when(&n, at + 2 * 86_400, 0), "Builder (agent) \u{b7} 11-18 10:24");
        n.status = "owner".into();
        assert_eq!(who_when(&n, at, 0), "You \u{b7} today 10:24");
        assert_eq!(who_when(&node("a", "", ""), at, 0), "");
    }

    #[test]
    fn when_uses_the_local_day() {
        // 23:14 UTC on 11-19 is 00:14 on 11-20 at +01:00, so with "now" on 11-20 it is today.
        let at = parse_secs("2026-11-19 23:14:00").unwrap();
        let now = parse_secs("2026-11-20 05:40:00").unwrap();
        assert_eq!(when_text(at, now, 3600), "today 00:14");
        assert_eq!(when_text(at, now, 0), "11-19 23:14");
        // West of UTC the same moment is still the evening before.
        assert_eq!(when_text(at, now, -5 * 3600), "11-19 18:14");
        assert_eq!(when_text(at, now + 86_400, 3600), "11-20 00:14");
    }

    #[test]
    fn risk_chip_joins_risk_and_reversible() {
        assert_eq!(risk_chip_text("low", Some(true)), "Low risk \u{b7} reversible");
        assert_eq!(risk_chip_text("high", Some(false)), "High risk \u{b7} can't be undone");
        assert_eq!(risk_chip_text("unknown", None), "Risk unknown");
    }

    #[test]
    fn happened_prefers_why_then_reason() {
        let mut n = node("a", "decision", "needs_you");
        let mut d = crate::api::Decision { why: " Ready to ship ".into(), ..Default::default() };
        assert_eq!(happened_text(&n, &d).as_deref(), Some("Ready to ship"));
        d.why.clear();
        assert_eq!(happened_text(&n, &d), None);
        d.ask = "Merge it?\nIt only adds a folder.".into();
        assert_eq!(happened_text(&n, &d).as_deref(), Some("Merge it?\nIt only adds a folder."));
        d.ask.clear();
        n.reason = Some("Waiting on you".into());
        assert_eq!(happened_text(&n, &d).as_deref(), Some("Waiting on you"));
    }

    #[test]
    fn scout_is_offered_for_failed_stalled_and_needs_you() {
        assert!(["failed", "stalled", "needs_you"].iter().all(|s| can_diagnose(s)));
        assert!(!can_diagnose("done") && !can_diagnose("running"));
    }
}
