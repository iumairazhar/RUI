//! The decision card: what a thread needs the owner to decide. Shared by the task page, Home and the Inbox.

use super::library::danger_button;
use super::{chip, markdown, primary_button, tag};
use crate::api::{Decision, Node, Thread};
use crate::store::{self, Store, now_secs};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Status, Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, EventEmitter, FontWeight, Stateful, Window, div, prelude::*, px};
use rui_core::{Question, QuestionStatus, Rung};
use std::collections::HashMap;

pub const WHY_HINT: &str = "Why \u{2014} this becomes a lesson in the Playbook, so the team doesn\u{2019}t ask again";
const ANSWER_HINT: &str = "Answer it, or pass down new instructions\u{2026}";

/// One decision with where it lives: its thread, the task to act on, its node and client.
#[derive(Debug, Clone, PartialEq)]
pub struct CardData {
    pub thread_id: i64,
    /// The subtask behind the decision, or the thread itself.
    pub task_id: i64,
    pub node_id: String,
    pub client: String,
    pub decision: Decision,
    /// When the decision came up, for newest-first order.
    pub at: String,
    /// The step's attempt limit, when known.
    pub max_loops: Option<i64>,
}

impl CardData {
    /// The card for a node, when the node carries a decision.
    pub fn from_node(t: &Thread, n: &Node) -> Option<Self> {
        let decision = n.decision.clone()?;
        let task_id = n.subtask_id.unwrap_or(t.id);
        let (node_id, at) = (n.id.clone(), n.started_at.clone());
        let max_loops = n.attempts.as_ref().map(|a| a.max).filter(|m| *m > 0);
        Some(Self { thread_id: t.id, task_id, node_id, client: t.client.name.clone(), decision, at, max_loops })
    }

    /// A plain card for an open question whose thread has not loaded yet.
    fn from_question(q: &Question, s: &Store) -> Self {
        let task = s.tasks.iter().find(|t| t.id == q.task_id);
        let thread_id = task.and_then(|t| t.parent_id).unwrap_or(q.task_id);
        let client = task.and_then(|t| s.clients.iter().find(|c| c.id == t.client_id)).map(|c| c.name.clone());
        let why = q.ladder.last().map(|r| r.why.clone()).unwrap_or_default();
        let decision = Decision {
            source: "question".into(), question_id: Some(q.id), ask: q.text.clone(), why, ladder: q.ladder.clone(),
            risk: "unknown".into(), actions: vec!["answer".into()], ..Default::default()
        };
        let (node_id, at) = (format!("q:{}", q.id), q.created_at.clone());
        Self { thread_id, task_id: q.task_id, node_id, client: client.unwrap_or_default(), decision, at, max_loops: None }
    }

    /// Key that keeps one card entity per decision.
    fn key(&self) -> String {
        format!("{}:{}", self.thread_id, self.node_id)
    }
}

/// Every open decision in a thread.
pub fn decisions_of(t: &Thread) -> Vec<CardData> {
    let open = t.nodes.iter().filter(|n| n.status == "needs_you");
    open.filter_map(|n| CardData::from_node(t, n)).collect()
}

/// The card for a question: from its loaded thread, else from the question, else the held answer.
pub fn question_card(s: &Store, qid: i64) -> Option<CardData> {
    let cached = s.thread_cache.values().flat_map(decisions_of).find(|c| c.decision.question_id == Some(qid));
    let open = || s.questions.iter().find(|q| q.id == qid && q.status == QuestionStatus::Open);
    cached.or_else(|| open().map(|q| CardData::from_question(q, s))).or_else(|| s.sending.get(&qid).map(|x| x.card.clone()))
}

/// The thread a question belongs to, to load it.
pub fn question_thread(s: &Store, qid: i64) -> Option<i64> {
    let q = s.questions.iter().find(|q| q.id == qid)?;
    let task = s.tasks.iter().find(|t| t.id == q.task_id)?;
    Some(task.parent_id.unwrap_or(task.id))
}

/// Add held answers that `keep` accepts and that are not in `cards` already.
pub fn with_sending(mut cards: Vec<CardData>, s: &Store, keep: impl Fn(&CardData) -> bool) -> Vec<CardData> {
    let held: Vec<CardData> = s.sending.values().map(|x| x.card.clone()).filter(|c| keep(c)).collect();
    for c in held {
        if !cards.iter().any(|k| k.key() == c.key()) {
            cards.push(c);
        }
    }
    cards
}

/// High first, then medium, low and unknown.
pub fn risk_rank(risk: &str) -> u8 {
    match risk {
        "high" => 0,
        "medium" => 1,
        "low" => 2,
        _ => 3,
    }
}

/// Highest risk first, then newest.
pub fn sort_cards(cards: &mut [CardData]) {
    cards.sort_by(|a, b| risk_rank(&a.decision.risk).cmp(&risk_rank(&b.decision.risk)).then(b.at.cmp(&a.at)));
}

pub fn risk_label(risk: &str) -> &'static str {
    match risk {
        "high" => "High risk",
        "medium" => "Medium risk",
        "low" => "Low risk",
        _ => "Risk unknown",
    }
}

fn risk_style(risk: &str, th: &Theme) -> Status {
    match risk {
        "high" => th.block.clone(),
        "medium" => th.wait.clone(),
        "low" => th.done.clone(),
        _ => Status { base: th.muted, fg: th.muted, dim: th.hairline, rim: th.hairline },
    }
}

pub fn reversible_text(r: Option<bool>) -> Option<&'static str> {
    r.map(|r| if r { "Reversible" } else { "Can't be undone" })
}

/// One button a decision offers.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// A quick answer from the question's options.
    Pick(String),
    /// The typed answer.
    Answer,
    Approve,
    SendBack,
    Retry,
    RaiseLimit,
    Stop,
}

impl Act {
    pub fn label(&self) -> String {
        match self {
            Act::Pick(o) => o.clone(),
            Act::Answer => "Send down".into(),
            Act::Approve => "Approve".into(),
            Act::SendBack => "Ask for changes".into(),
            Act::Retry => "Retry".into(),
            Act::RaiseLimit => "Give it more tries".into(),
            Act::Stop => "Stop".into(),
        }
    }
}

/// The decision's actions as buttons; "answer" becomes its options plus a typed answer.
pub fn acts(d: &Decision) -> Vec<Act> {
    let one = |a: &String| match a.as_str() {
        "answer" => d.options.iter().cloned().map(Act::Pick).chain([Act::Answer]).collect(),
        "approve" => vec![Act::Approve],
        "send_back" => vec![Act::SendBack],
        "retry" => vec![Act::Retry],
        "raise_limit" => vec![Act::RaiseLimit],
        "stop" => vec![Act::Stop],
        _ => vec![],
    };
    d.actions.iter().flat_map(one).collect()
}

/// Highest attempt limit "+3 tries" may set.
const LIMIT_CAP: i64 = 20;

/// New limit for "+3 tries": three more, capped at 20. None when unknown or already at the cap.
pub fn raised_limit(max: Option<i64>) -> Option<i64> {
    let max = max?;
    let to = (max + 3).min(LIMIT_CAP);
    (to > max).then_some(to)
}

/// "+3 tries" when the limit can be raised; otherwise the retry wording.
pub fn raise_label(max: Option<i64>) -> String {
    match (raised_limit(max), max) {
        (Some(to), Some(m)) => format!("+{} tries", to - m),
        _ => Act::RaiseLimit.label(),
    }
}

/// Seconds left to undo an answer that runs at `run_at`.
pub fn undo_left(run_at: i64, now: i64) -> i64 {
    (run_at - now).max(0)
}

/// "Undo · 4s"
pub fn undo_label(left: i64) -> String {
    format!("Undo \u{b7} {left}s")
}

/// The ladder as (who, why) rungs, ending with "You" and no reason.
pub fn ladder_parts(ladder: &[Rung]) -> Vec<(String, Option<String>)> {
    let rungs = ladder.iter().map(|r| (r.who.clone(), Some(r.why.clone())));
    rungs.chain([("You".to_string(), None)]).collect()
}

/// "Backend Engineer 7 · which db? → Lead · unsure → You".
#[cfg(test)]
pub fn ladder_text(ladder: &[Rung]) -> String {
    let rung = |(who, why): (String, Option<String>)| match why {
        Some(w) => format!("{who} \u{b7} {w}"),
        None => who,
    };
    ladder_parts(ladder).into_iter().map(rung).collect::<Vec<_>>().join(" \u{2192} ")
}

/// One rung: bold who, then the reason.
pub fn rung_chip(who: String, why: Option<String>, th: &Theme) -> Div {
    let who = div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(who);
    // Never wider than the card it sits in; the why text wraps instead of pushing the card wide.
    let chip = div().flex().min_w_0().gap(px(5.)).px(px(9.)).py(px(4.)).rounded(px(7.)).max_w(gpui::relative(1.));
    let chip = chip.bg(th.fg.opacity(0.08)).text_size(px(11.)).text_color(th.muted).child(who);
    let why = why.map(|w| div().flex_1().min_w_0().whitespace_normal().child(format!("\u{b7} {w}")));
    chip.children(why)
}

/// "Pod · why → Account Lead · why → You" as chips.
pub fn ladder_row(ladder: &[Rung], th: &Theme) -> Div {
    let parts = ladder_parts(ladder).into_iter().map(|(who, why)| (who, why.map(|w| short_why(&w))));
    let parts: Vec<_> = parts.collect();
    let last = parts.len() - 1;
    let mut row = div().flex().flex_wrap().items_center().gap(px(6.));
    for (i, (who, why)) in parts.into_iter().enumerate() {
        row = row.child(rung_chip(who, why, th));
        if i != last {
            row = row.child(div().text_size(px(11.)).text_color(th.muted.opacity(0.6)).child("\u{2192}"));
        }
    }
    row
}

/// A rung's reason on one line: its first line, cut to 80 characters with an ellipsis.
pub fn short_why(why: &str) -> String {
    let line = why.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() <= 80 {
        return line.to_string();
    }
    let cut: String = line.chars().take(80).collect();
    format!("{}\u{2026}", cut.trim_end())
}

/// First non-empty line, without markdown marks.
pub fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().filter(|c| !"#*_".contains(*c)).collect::<String>().trim().to_string()
}

/// What a card asks its host to do.
#[derive(Debug, Clone, PartialEq)]
pub enum CardEvent {
    /// Open the thread page on this decision's node.
    Open { thread_id: i64, node_id: String },
    /// Open the thread page's Changes tab.
    Review { thread_id: i64 },
}

/// One decision card. `compact` is the Home strip variant: no buttons, a click opens the thread.
pub struct DecisionCard {
    store: Entity<Store>,
    data: Option<CardData>,
    compact: bool,
    answer: Entity<TextInput>,
    why: Entity<TextInput>,
    /// "Ask for changes" was clicked and its input is showing.
    reveal: bool,
    /// The "Show details" fold is open.
    details: bool,
}

impl EventEmitter<CardEvent> for DecisionCard {}

impl DecisionCard {
    pub fn new(store: Entity<Store>, compact: bool, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let answer = cx.new(|cx| TextInput::new(ANSWER_HINT, cx));
        cx.subscribe(&answer, |this, _, _: &Submit, cx| this.submit(cx)).detach();
        let why = cx.new(|cx| TextInput::new(WHY_HINT, cx));
        cx.subscribe(&why, |this, _, _: &Submit, cx| this.submit(cx)).detach();
        Self { store, data: None, compact, answer, why, reveal: false, details: false }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let act = if self.reveal { Act::SendBack } else { Act::Answer };
        self.run(act, cx);
    }

    fn run(&mut self, act: Act, cx: &mut Context<Self>) {
        let Some(d) = self.data.clone() else { return };
        let review = d.decision.review.as_ref().map(|r| r.task_id).filter(|id| *id > 0).unwrap_or(d.thread_id);
        match act {
            Act::Pick(text) => self.send(d, Some(text), cx),
            Act::Answer => self.send(d, None, cx),
            Act::Approve => self.store.update(cx, |s, cx| s.review_task(review, true, None, cx)),
            Act::SendBack => self.send_back(review, cx),
            Act::Retry => self.store.update(cx, |s, cx| s.retry_task(d.task_id, cx)),
            Act::RaiseLimit => self.raise_limit(&d, cx),
            Act::Stop => self.store.update(cx, |s, cx| s.stop_task(d.task_id, "Decision card", None, cx)),
        }
    }

    /// Raise the step's attempt limit by three; retry when the limit is unknown or capped.
    fn raise_limit(&mut self, d: &CardData, cx: &mut Context<Self>) {
        let task = d.task_id;
        match raised_limit(d.max_loops) {
            Some(to) => self.store.update(cx, |s, cx| s.set_limit(task, to, cx)),
            None => self.store.update(cx, |s, cx| s.retry_task(task, cx)),
        }
    }

    /// Send a picked option, or what was typed when `pick` is `None`.
    fn send(&mut self, d: CardData, pick: Option<String>, cx: &mut Context<Self>) {
        let text = pick.unwrap_or_else(|| self.answer.read(cx).content().trim().to_string());
        if text.is_empty() {
            return;
        }
        let why = Some(self.why.read(cx).content().trim().to_string()).filter(|w| !w.is_empty());
        self.answer.update(cx, |i, cx| i.clear(cx));
        self.why.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| s.answer_decision(d, text, why, cx));
    }

    /// First click shows the input; the next one sends the change request.
    fn send_back(&mut self, task: i64, cx: &mut Context<Self>) {
        if !self.reveal {
            self.reveal = true;
            cx.notify();
            return;
        }
        let text = self.answer.read(cx).content().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.answer.update(cx, |i, cx| i.clear(cx));
        self.reveal = false;
        self.store.update(cx, |s, cx| s.review_task(task, false, Some(text), cx));
    }

    fn review_click(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.data else { return };
        let Some(r) = &d.decision.review else { return };
        match &r.url {
            Some(url) => cx.open_url(url),
            None => cx.emit(CardEvent::Review { thread_id: d.thread_id }),
        }
    }

    fn open_click(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.data else { return };
        cx.emit(CardEvent::Open { thread_id: d.thread_id, node_id: d.node_id.clone() });
    }

    /// Seconds left on this card's held answer, if one is held.
    fn held(&self, cx: &Context<Self>) -> Option<(i64, i64)> {
        let qid = self.data.as_ref()?.decision.question_id?;
        let s = self.store.read(cx).sending.get(&qid)?;
        Some((s.undo.event_id, undo_left(s.undo.run_at, now_secs())))
    }
}

// ---- rendering ----

fn heading(text: &str, th: &Theme) -> Div {
    div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(text.to_uppercase())
}

fn eyebrow(client: &str, th: &Theme) -> Div {
    let client = if client.is_empty() { "General" } else { client };
    heading(&format!("{client} \u{b7} needs a decision"), th)
}

/// Risk chip plus "Reversible" / "Can't be undone".
fn meta_row(d: &Decision, th: &Theme) -> Div {
    let risk = tag(&risk_style(&d.risk, th), risk_label(&d.risk));
    let rev = reversible_text(d.reversible).map(|t| tag(&risk_style(if d.reversible == Some(true) { "low" } else { "medium" }, th), t));
    div().flex().flex_wrap().items_center().gap(px(6.)).child(risk).children(rev)
}

fn if_approved(steps: &[String], th: &Theme) -> Option<Div> {
    if steps.is_empty() {
        return None;
    }
    let line = |(i, s): (usize, &String)| div().text_size(px(12.5)).text_color(th.fg_2).child(format!("{}. {s}", i + 1));
    Some(div().flex().flex_col().gap(px(4.)).child(heading("If approved:", th)).children(steps.iter().enumerate().map(line)))
}

fn field(input: Entity<TextInput>, th: &Theme) -> Div {
    div().min_w_0().overflow_hidden().px(px(10.)).py(px(4.)).rounded(px(8.)).bg(th.well).border_1().border_color(th.hairline).child(input)
}

fn card_shell(id: String, th: &Theme) -> Stateful<Div> {
    let card = div().id(gpui::SharedString::from(id)).flex().flex_col().gap(px(12.)).p(px(16.)).rounded(px(12.));
    card.bg(th.surface).border_1().border_color(th.block.rim)
}

impl DecisionCard {
    fn undo_row(&self, event_id: i64, left: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let text = div().flex_1().text_size(px(12.5)).text_color(th.fg_2).child("Sending your answer\u{2026}");
        let undo = chip("decision-undo", &undo_label(left), cx).on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.store.update(cx, |s, cx| s.undo_answer(event_id, cx));
        }));
        div().flex().items_center().gap(px(10.)).child(text).child(undo).into_any_element()
    }

    fn button(&self, i: usize, act: Act, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let label = match act {
            Act::RaiseLimit => raise_label(self.data.as_ref().and_then(|d| d.max_loops)),
            _ => act.label(),
        };
        let el = match act {
            Act::Approve | Act::Answer => primary_button(format!("decision-act-{i}"), &label, cx),
            Act::Stop => danger_button(format!("decision-act-{i}"), &label, true, th),
            _ => chip(format!("decision-act-{i}"), &label, cx),
        };
        el.on_click(cx.listener(move |this, _, _, cx| this.run(act.clone(), cx))).into_any_element()
    }

    /// Option chips and buttons, with the answer fields above them when the decision takes an answer.
    fn actions(&self, d: &Decision, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let acts = acts(d);
        let mut out = Vec::new();
        if acts.contains(&Act::Answer) {
            out.push(field(self.answer.clone(), th).into_any_element());
            out.push(field(self.why.clone(), th).into_any_element());
        } else if self.reveal {
            out.push(field(self.answer.clone(), th).into_any_element());
        }
        let buttons: Vec<AnyElement> = acts.into_iter().enumerate().map(|(i, a)| self.button(i, a, th, cx)).collect();
        out.push(div().flex().flex_wrap().items_center().gap(px(8.)).children(buttons).into_any_element());
        out
    }

    fn review_link(&self, d: &Decision, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        d.review.as_ref()?;
        let link = div().id("decision-review").text_size(px(12.5)).text_color(th.run.fg).cursor_pointer().child("Review the change");
        Some(link.on_click(cx.listener(|this, _, _, cx| this.review_click(cx))).into_any_element())
    }

    fn details_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = if self.details { "Hide details" } else { "Show details" };
        let chip = chip("decision-details", label, cx);
        let chip = chip.on_click(cx.listener(|this, _, _, cx| {
            this.details = !this.details;
            cx.notify();
        }));
        div().flex().child(chip).into_any_element()
    }

    /// The folded part: why the Lead couldn't decide, the ladder, the long text, the plan and the review link.
    fn details_block(&self, d: &Decision, th: &Theme, cx: &mut Context<Self>) -> Div {
        let why = (!d.why.trim().is_empty()).then(|| {
            let text = div().text_size(px(12.5)).text_color(th.fg_2).child(d.why.clone());
            div().flex().flex_col().gap(px(4.)).child(heading("Why the Lead couldn't decide", th)).child(text)
        });
        let ladder = (!d.ladder.is_empty()).then(|| ladder_row(&d.ladder, th));
        let long = (!d.details.trim().is_empty()).then(|| div().max_w(px(660.)).child(markdown::render(&d.details, th.fg_2, 12.5, th)));
        let block = div().flex().flex_col().gap(px(12.)).children(why).children(ladder).children(long);
        block.children(if_approved(&d.if_approved, th)).children(self.review_link(d, th, cx))
    }

    fn full(&self, c: &CardData, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let d = &c.decision;
        let ask = markdown::render(&d.ask, th.fg, 13.5, th);
        let card = card_shell(format!("decision-{}", c.key()), th).child(eyebrow(&c.client, th)).child(ask);
        let card = card.child(meta_row(d, th));
        // Picks sit right under the question; the long story folds away below them.
        let card = match self.held(cx) {
            Some((event_id, left)) => card.child(self.undo_row(event_id, left, th, cx)),
            None => card.children(self.actions(d, th, cx)),
        };
        let card = card.child(self.details_chip(cx));
        let card = if self.details { card.child(self.details_block(d, th, cx)) } else { card };
        card.into_any_element()
    }

    fn compact(&self, c: &CardData, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let d = &c.decision;
        let ask = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).truncate().child(first_line(&d.ask));
        let hover = th.surface_2;
        let card = card_shell(format!("decision-mini-{}", c.key()), th).w(px(300.)).gap(px(8.)).p(px(12.));
        let card = card.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(|this, _, _, cx| this.open_click(cx)));
        let card = card.child(eyebrow(&c.client, th)).child(ask).child(meta_row(d, th));
        let held = self.held(cx).map(|(event_id, left)| self.undo_row(event_id, left, th, cx));
        card.children(held).into_any_element()
    }
}

impl Render for DecisionCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let Some(c) = self.data.clone() else { return div().into_any_element() };
        if self.compact { self.compact(&c, &th, cx) } else { self.full(&c, &th, cx) }
    }
}

/// What a host does with a card's events.
pub type OnCard<V> = fn(&mut V, &CardEvent, &mut Context<V>);

/// One card entity per decision, kept while the decision is shown.
#[derive(Default)]
pub struct Cards {
    map: HashMap<(String, bool), Entity<DecisionCard>>,
}

impl Cards {
    /// The card for `data`, created on first use and refreshed with the latest data.
    pub fn get<V: 'static>(&mut self, data: &CardData, compact: bool, store: &Entity<Store>, on: OnCard<V>, cx: &mut Context<V>) -> Entity<DecisionCard> {
        let key = (data.key(), compact);
        let card = match self.map.get(&key) {
            Some(c) => c.clone(),
            None => {
                let store = store.clone();
                let card = cx.new(|cx| DecisionCard::new(store, compact, cx));
                cx.subscribe(&card, move |this, _, ev: &CardEvent, cx| on(this, ev, cx)).detach();
                self.map.insert(key, card.clone());
                card
            }
        };
        let data = data.clone();
        card.update(cx, |c, _| c.data = Some(data));
        card
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rung(who: &str, why: &str) -> Rung {
        Rung { who: who.into(), why: why.into() }
    }

    fn card(risk: &str, at: &str) -> CardData {
        let decision = Decision { risk: risk.into(), ..Default::default() };
        CardData { thread_id: 1, task_id: 1, node_id: format!("{risk}{at}"), client: String::new(), decision, at: at.into(), max_loops: None }
    }

    #[test]
    fn ladder_ends_with_you() {
        let ladder = [rung("Backend Engineer 7", "which db?"), rung("Lead", "unsure")];
        assert_eq!(ladder_text(&ladder), "Backend Engineer 7 \u{b7} which db? \u{2192} Lead \u{b7} unsure \u{2192} You");
        let parts = ladder_parts(&ladder);
        assert_eq!((parts.len(), parts[2].clone()), (3, ("You".into(), None)));
        assert_eq!(ladder_text(&[]), "You");
    }

    #[test]
    fn risk_sorts_high_first_then_newest() {
        let mut v = vec![card("low", "2"), card("unknown", "9"), card("high", "1"), card("medium", "3"), card("high", "5"), card("odd", "4")];
        sort_cards(&mut v);
        let got: Vec<&str> = v.iter().map(|c| c.node_id.as_str()).collect();
        assert_eq!(got, ["high5", "high1", "medium3", "low2", "unknown9", "odd4"]);
    }

    #[test]
    fn actions_map_to_buttons() {
        let d = |actions: &[&str], options: &[&str]| Decision {
            actions: actions.iter().map(|a| a.to_string()).collect(),
            options: options.iter().map(|o| o.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(acts(&d(&["answer"], &["Yes", "No"])), [Act::Pick("Yes".into()), Act::Pick("No".into()), Act::Answer]);
        assert_eq!(acts(&d(&["approve", "send_back"], &[])), [Act::Approve, Act::SendBack]);
        assert_eq!(acts(&d(&["raise_limit", "stop", "nope"], &[])), [Act::RaiseLimit, Act::Stop]);
        assert_eq!(acts(&d(&["retry"], &[])), [Act::Retry]);
        assert_eq!(Act::SendBack.label(), "Ask for changes");
        assert_eq!(Act::Answer.label(), "Send down");
    }

    #[test]
    fn raise_limit_adds_three_capped_at_twenty() {
        assert_eq!(raised_limit(Some(5)), Some(8));
        assert_eq!(raised_limit(Some(19)), Some(20));
        assert_eq!(raised_limit(Some(20)), None);
        assert_eq!(raised_limit(None), None);
        assert_eq!(raise_label(Some(5)), "+3 tries");
        assert_eq!(raise_label(None), "Give it more tries");
    }

    #[test]
    fn undo_counts_down_to_zero() {
        assert_eq!(undo_left(110, 100), 10);
        assert_eq!(undo_left(100, 105), 0);
        assert_eq!(undo_label(4), "Undo \u{b7} 4s");
    }

    #[test]
    fn words_for_risk_and_reversible() {
        assert_eq!((risk_label("high"), risk_label("")), ("High risk", "Risk unknown"));
        assert_eq!(reversible_text(Some(false)), Some("Can't be undone"));
        assert_eq!(reversible_text(None), None);
    }

    #[test]
    fn short_why_takes_first_line_and_cuts_at_80() {
        assert_eq!(short_why("\n  commercial call\nsecond line"), "commercial call");
        assert_eq!(short_why(&"a".repeat(80)), "a".repeat(80));
        assert_eq!(short_why(&"a".repeat(81)), format!("{}\u{2026}", "a".repeat(80)));
    }

    #[test]
    fn first_line_strips_markdown() {
        assert_eq!(first_line("\n## **Use** _Postgres_?\nmore"), "Use Postgres?");
        assert_eq!(first_line(""), "");
    }
}
