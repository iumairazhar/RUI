//! Chats page: the list of chats beside the selected chat.

use super::live::live_action;
use super::asks::{AskInputs, ask_card};
use super::chat_tools::{ToolCall, results_by_id, tool_calls, tool_output, tool_row_head};
use super::library::confirm_modal;
use super::{chip, markdown, primary_button, question_card};
use crate::api::LiveState;
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Theme, theme};
use gpui::{
    AnyElement, Context, Entity, FontWeight, ScrollHandle, Window, div, prelude::*, px, svg,
};
use super::activity::{icon, parse_secs, summarize};
use super::roster::role_title;
use crate::api::RoleInfo;
use serde_json::Value;
use rui_core::{Chat, ChatAsk, ChatMessage, ChatRole, Event, Question};
use std::collections::{HashMap, HashSet};

/// Chat list, messages of the selected chat, open questions and a composer.
pub struct ChatView {
    store: Entity<Store>,
    input: Entity<TextInput>,
    answers: HashMap<i64, Entity<TextInput>>,
    ask_inputs: AskInputs,
    scroll: ScrollHandle,
    last_len: usize,
    last_chat: Option<i64>,
    confirm_delete: Option<(i64, String)>,
    open_tools: HashSet<String>,
}

impl ChatView {
    /// Build the chat view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let input = cx.new(|cx| TextInput::new("Tell Rui what to set up…", cx));
        cx.subscribe(&input, |this, _, _: &Submit, cx| this.send(cx)).detach();
        Self { store, input, answers: HashMap::new(), ask_inputs: AskInputs::default(), scroll: ScrollHandle::new(), last_len: 0, last_chat: None, confirm_delete: None, open_tools: HashSet::new() }
    }

    fn ask_delete(&mut self, id: i64, title: String, cx: &mut Context<Self>) {
        self.confirm_delete = Some((id, title));
        cx.notify();
    }

    fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        cx.notify();
    }

    fn accept_delete(&mut self, cx: &mut Context<Self>) {
        let Some((id, _)) = self.confirm_delete.take() else { return };
        self.store.update(cx, |s, cx| s.delete_chat(id, cx));
        cx.notify();
    }

    fn delete_modal(&self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, title) = self.confirm_delete.as_ref()?;
        let head = format!("Delete \"{}\"?", cut_title(title));
        let body = "The conversation and its open questions go away. Teams it started keep running and their work stays.";
        Some(confirm_modal(&head, body, "Delete", true, Self::cancel_delete, Self::accept_delete, th, cx))
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).content();
        if text.trim().is_empty() {
            return;
        }
        self.input.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| s.send_chat(text, cx));
    }

    fn sync_inputs(&mut self, questions: &[Question], cx: &mut Context<Self>) {
        self.answers.retain(|id, _| questions.iter().any(|q| q.id == *id));
        for q in questions {
            if self.answers.contains_key(&q.id) {
                continue;
            }
            let id = q.id;
            let input = cx.new(|cx| TextInput::new("Your answer...", cx));
            cx.subscribe(&input, move |this, _, _: &Submit, cx| this.answer(id, cx)).detach();
            self.answers.insert(id, input);
        }
    }

    fn answer(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(input) = self.answers.get(&id).cloned() else { return };
        let text = input.read(cx).content();
        if text.trim().is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| s.answer(id, text, None, cx));
    }

    /// Answer an ask with a picked option, or with what was typed when `text` is `None`.
    fn answer_ask(&mut self, id: i64, text: Option<String>, cx: &mut Context<Self>) {
        let Some(text) = text.or_else(|| self.ask_inputs.take(id, cx)) else { return };
        self.store.update(cx, |s, cx| s.answer_ask(id, text, cx));
    }

    fn question(&self, q: &Question, cx: &mut Context<Self>) -> AnyElement {
        let id = q.id;
        let input = match self.answers.get(&id) {
            Some(i) => i.clone().into_any_element(),
            None => div().into_any_element(),
        };
        let btn = chip(format!("answer-{id}"), "Answer", cx)
            .on_click(cx.listener(move |this, _, _, cx| this.answer(id, cx)));
        question_card(&q.text, None, input, btn.into_any_element(), cx).into_any_element()
    }

    fn message_list(
        &mut self,
        chat: &[ChatMessage],
        questions: &[Question],
        asks: &[ChatAsk],
        cards: &HashMap<i64, Vec<ActCard>>,
        tools: &HashMap<i64, Vec<ToolCall>>,
        live_tools: &[ToolCall],
        live: Option<Vec<String>>,
        state: Option<&LiveState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        let open: Vec<AnyElement> = questions.iter().map(|q| self.question(q, cx)).collect();
        let open_asks = asks.iter().filter(|a| a.status == "open");
        let ask_cards: Vec<AnyElement> = open_asks.map(|a| ask_card(a, &self.ask_inputs, Self::answer_ask, cx)).collect();
        let placed = place_asks(chat, asks);
        let steps = live.or_else(|| state.map(|_| vec![]));
        let working = steps.map(|steps| working_bubble(&steps, state, &th));
        let mut list = div()
            .id("chat-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .pr(px(20.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .children(chat.iter().map(|m| self.message_with_asks(m, placed.get(&m.id), cards, tools, &th, cx)))
            .children(ask_cards)
            .children((!live_tools.is_empty()).then(|| self.tool_rows(live_tools, &th, cx)))
            .children(working)
            .children(open);
        if chat.is_empty() && questions.is_empty() {
            list = list.child(div().text_size(px(13.)).text_color(th.muted).child("Tell Rui what to set up."));
        }
        list.into_any_element()
    }

    /// A message with its cards, then the answered asks that were put to the owner right after it.
    fn message_with_asks(&self, m: &ChatMessage, asks: Option<&Vec<ChatAsk>>, cards: &HashMap<i64, Vec<ActCard>>, tools: &HashMap<i64, Vec<ToolCall>>, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let el = self.message_with_cards(m, cards, tools, th, cx);
        let Some(asks) = asks else { return el };
        let lines: Vec<AnyElement> = asks.iter().map(|a| answered_line(a, th)).collect();
        div().flex().flex_col().gap(px(8.)).child(el).children(lines).into_any_element()
    }

    fn message_with_cards(&self, m: &ChatMessage, cards: &HashMap<i64, Vec<ActCard>>, tools: &HashMap<i64, Vec<ToolCall>>, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let acts = cards.get(&m.id).map(|v| v.as_slice()).unwrap_or(&[]);
        let calls = tools.get(&m.id).map(|v| v.as_slice()).unwrap_or(&[]);
        if acts.is_empty() && calls.is_empty() {
            return message(m, th);
        }
        let rows = (!calls.is_empty()).then(|| div().mb(px(8.)).child(self.tool_rows(calls, th, cx)));
        let els: Vec<AnyElement> = acts.iter().enumerate().map(|(k, a)| self.act_card(m.id, k, a, th, cx)).collect();
        let cards = (!els.is_empty()).then(|| div().flex().flex_col().gap(px(8.)).mt(px(8.)).children(els));
        div().flex().flex_col().children(rows).child(message(m, th)).children(cards).into_any_element()
    }

    /// Show or hide the output of one tool call.
    fn toggle_tool(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.open_tools.remove(id) {
            self.open_tools.insert(id.to_string());
        }
        cx.notify();
    }

    /// Compact rows for a turn's tool calls, each expandable to its output.
    fn tool_rows(&self, calls: &[ToolCall], th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = calls.iter().map(|c| self.tool_row(c, th, cx)).collect();
        div().max_w(px(620.)).flex().flex_col().gap(px(4.)).children(rows).into_any_element()
    }

    /// One tool call row, with its output below when expanded.
    fn tool_row(&self, c: &ToolCall, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = c.id.clone();
        let head = tool_row_head(format!("tool-row-{id}"), c, th);
        let head = head.on_click(cx.listener(move |this, _, _, cx| this.toggle_tool(&id, cx)));
        let out = self.open_tools.contains(&c.id).then(|| tool_output(format!("tool-out-{}", c.id), c, th));
        div().flex().flex_col().gap(px(2.)).child(head).children(out).into_any_element()
    }

    fn act_card(&self, mid: i64, k: usize, a: &ActCard, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let tool = div().font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child("Set up Pod");
        let head = div().flex().items_center().gap(px(8.)).px(px(14.)).py(px(9.)).border_b_1().border_color(th.hairline);
        let head = head.font_family("Menlo").text_size(px(11.)).text_color(th.muted);
        let head = head.child(icon("term", 12., th.muted)).child(tool).child("\u{b7} tool run by Rui");
        let body = div().px(px(14.)).pt(px(10.)).text_size(px(12.5)).text_color(th.fg_2).child(a.body.clone());
        let scope = div().px(px(14.)).text_size(px(11.5)).text_color(th.muted).child(a.scope.clone());
        let open = a.task_id.map(|id| {
            let chip = chip(format!("act-open-{mid}-{k}"), "Open the task", cx).h(px(26.));
            chip.on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.open_thread(id, cx))))
        });
        let foot = div().flex().px(px(14.)).pb(px(11.)).children(open);
        let card = div().id(("act-card", mid as usize * 16 + k)).max_w(px(620.)).rounded(px(12.)).bg(th.well);
        card.border_1().border_color(th.hairline).flex().flex_col().gap(px(6.)).child(head).child(body).child(scope).child(foot).into_any_element()
    }

    fn composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let send = primary_button("send", "Send", cx).on_click(cx.listener(|this, _, _, cx| this.send(cx)));
        div()
            .id("chat-composer-wrap")
            .pr(px(20.))
            .pt(px(12.))
            .pb(px(28.))
            .child(
                div()
                    .id("chat-composer")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .p(px(8.))
                    .pl(px(14.))
                    .rounded(px(14.))
                    .bg(th.surface)
                    .shadow(th.card_shadow())
                    .child(div().flex_1().child(self.input.clone()))
                    .child(send),
            )
            .into_any_element()
    }
}

fn message(m: &ChatMessage, th: &Theme) -> AnyElement {
    match m.role {
        ChatRole::User => user_message(&m.content, th),
        _ => rui_message(m, th),
    }
}

fn user_message(text: &str, th: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .justify_end()
        .child(
            div()
                .max_w(px(560.))
                .px(px(16.))
                .py(px(12.))
                .rounded(px(12.))
                .bg(th.accent_fill)
                .child(markdown::render(text, th.on_accent, 13.5, th)),
        )
        .into_any_element()
}

fn rui_message(m: &ChatMessage, th: &Theme) -> AnyElement {
    let meta = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child("Rui"))
        .child(
            div()
                .font_family("Menlo")
                .text_size(px(10.5))
                .text_color(th.muted)
                .child(hhmm(&m.created_at)),
        );
    let bubble = div()
        .max_w(px(620.))
        .p(px(16.))
        .rounded(px(12.))
        .bg(th.surface)
        .shadow(th.card_shadow())
        .child(markdown::render(&m.content, th.fg, 13.5, th));
    let col = div().flex().flex_col().gap(px(7.)).child(meta).child(div().flex().child(bubble));
    col.into_any_element()
}

const STAFF_TOOL: &str = "mcp__rui__staff_task";

/// Text of a "Set up Pod" card: the task with its pod roles, and the project line.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ActCard {
    pub body: String,
    pub scope: String,
    /// The task the call created, from the tool result.
    pub task_id: Option<i64>,
}

/// Card text from a `staff_task` input `{project?, title, brief, pod: [..]}`.
pub(super) fn staff_card(input: &Value, roles: &[RoleInfo]) -> ActCard {
    let title = input["title"].as_str().unwrap_or("Untitled");
    let pod: Vec<String> = input["pod"].as_array().into_iter().flatten().filter_map(|r| r.as_str()).map(|r| role_title(roles, r)).collect();
    let body = if pod.is_empty() { title.to_string() } else { format!("{title} \u{2014} {}", pod.join(", ")) };
    let scope = match input["project"].as_str().filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => "General task".to_string(),
    };
    ActCard { body, scope, task_id: None }
}

/// The `task_id` in a `staff_task` result, which is the JSON the tool returned.
fn task_id_of(result: &str) -> Option<i64> {
    serde_json::from_str::<Value>(result).ok()?["task_id"].as_i64()
}

/// `staff_task` inputs Rui called in a chat between two moments (unix seconds, inclusive), with the task each created.
fn staff_calls(events: &[Event], chat_id: i64, from: i64, to: i64) -> Vec<(Value, Option<i64>)> {
    let in_turn = |e: &&Event| {
        let at = parse_secs(&e.created_at).unwrap_or(i64::MIN);
        e.agent == "rui" && e.kind == "assistant" && e.chat_id == Some(chat_id) && (from..=to).contains(&at)
    };
    let blocks = events.iter().filter(in_turn).flat_map(|e| e.payload["message"]["content"].as_array().cloned().unwrap_or_default());
    let results = results_by_id(events, chat_id, from, to);
    let task = |b: &Value| results.get(b["id"].as_str().unwrap_or("")).filter(|r| !r.1).and_then(|r| task_id_of(&r.0));
    blocks.filter(|b| b["type"] == "tool_use" && b["name"] == STAFF_TOOL).map(|b| (b["input"].clone(), task(&b))).collect()
}

/// Cards per Rui message id: the staff_task calls between the previous message and that one.
fn all_cards(chat: &[ChatMessage], events: &[Event], chat_id: i64, roles: &[RoleInfo]) -> HashMap<i64, Vec<ActCard>> {
    let mut out = HashMap::new();
    for (i, m) in chat.iter().enumerate().filter(|(i, m)| *i > 0 && m.role == ChatRole::Rui) {
        let (Some(from), Some(to)) = (parse_secs(&chat[i - 1].created_at), parse_secs(&m.created_at)) else { continue };
        let card = |(v, task_id): &(Value, Option<i64>)| ActCard { task_id: *task_id, ..staff_card(v, roles) };
        let cards: Vec<ActCard> = staff_calls(events, chat_id, from, to).iter().map(card).collect();
        if !cards.is_empty() {
            out.insert(m.id, cards);
        }
    }
    out
}

/// Tool calls per Rui message id: the calls between the previous message and that one.
fn all_tools(chat: &[ChatMessage], events: &[Event], chat_id: i64) -> HashMap<i64, Vec<ToolCall>> {
    let mut out = HashMap::new();
    for (i, m) in chat.iter().enumerate().filter(|(i, m)| *i > 0 && m.role == ChatRole::Rui) {
        let (Some(from), Some(to)) = (parse_secs(&chat[i - 1].created_at), parse_secs(&m.created_at)) else { continue };
        let calls = tool_calls(events, chat_id, from, to);
        if !calls.is_empty() {
            out.insert(m.id, calls);
        }
    }
    out
}

/// Tool calls of the in-progress turn: since the owner's last message, while it has no reply.
fn live_tools(chat: &[ChatMessage], events: &[Event], chat_id: i64) -> Vec<ToolCall> {
    let Some(last) = chat.last().filter(|m| m.role == ChatRole::User) else { return vec![] };
    let Some(from) = parse_secs(&last.created_at) else { return vec![] };
    tool_calls(events, chat_id, from, i64::MAX)
}

/// Rui's steps in one chat since the owner's last message, while that message has no reply yet.
fn live_steps(chat: &[ChatMessage], events: &[Event], chat_id: i64) -> Option<Vec<String>> {
    let last = chat.last().filter(|m| m.role == ChatRole::User)?;
    let steps: Vec<String> = events
        .iter()
        .filter(|e| e.agent == "rui" && e.chat_id == Some(chat_id) && e.created_at >= last.created_at)
        .map(|e| summarize(e).0)
        .filter(|s| !s.trim().is_empty())
        .collect();
    let skip = steps.len().saturating_sub(5);
    Some(steps.into_iter().skip(skip).collect())
}

/// Streaming text, plus a muted line for the tool or thinking state.
fn live_lines(l: &LiveState, th: &Theme) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let has_text = !l.text.trim().is_empty();
    if has_text {
        out.push(div().text_color(th.fg).child(markdown::render(&l.text, th.fg, 13.5, th)).into_any_element());
    }
    let (label, code) = live_action(l);
    let note = match (&l.tool, l.thinking && !has_text) {
        (Some(_), _) => Some(format!("Using {}\u{2026}", code.unwrap_or(label))),
        (None, true) => Some("Thinking\u{2026}".to_string()),
        _ => None,
    };
    out.extend(note.map(|n| div().child(n).into_any_element()));
    if out.is_empty() {
        out.push(div().child("Working\u{2026}").into_any_element());
    }
    out
}

/// A Rui bubble showing what it is doing right now.
fn working_bubble(steps: &[String], state: Option<&LiveState>, th: &Theme) -> AnyElement {
    let meta = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child("Rui"))
        .child(div().text_size(px(11.)).text_color(th.think.fg).child("working…"));
    // Tool calls already show as rows above the bubble, so raw step names are not repeated here.
    let lines: Vec<AnyElement> = match (state, steps.is_empty()) {
        (Some(l), _) => live_lines(l, th),
        (None, true) => vec![div().child("Thinking…").into_any_element()],
        (None, false) => vec![div().child("Working…").into_any_element()],
    };
    let bubble = div()
        .max_w(px(620.))
        .p(px(14.))
        .rounded(px(12.))
        .bg(th.surface)
        .border_1()
        .border_color(th.think.rim)
        .flex()
        .flex_col()
        .gap(px(4.))
        .text_size(px(12.5))
        .line_height(px(18.))
        .text_color(th.muted)
        .children(lines);
    div().flex().flex_col().gap(px(7.)).child(meta).child(div().flex().child(bubble)).into_any_element()
}

/// Answered asks by the id of the message they sit after: the last message sent at or before the ask.
/// An ask before the first message goes after the first one; an ask with no readable time goes last.
fn place_asks(chat: &[ChatMessage], asks: &[ChatAsk]) -> HashMap<i64, Vec<ChatAsk>> {
    let mut out: HashMap<i64, Vec<ChatAsk>> = HashMap::new();
    for a in asks.iter().filter(|a| a.status == "answered") {
        let at = parse_secs(&a.created_at);
        let before = |m: &ChatMessage| at.is_some_and(|t| parse_secs(&m.created_at).is_some_and(|s| s <= t));
        let idx = match at {
            Some(_) => chat.iter().rposition(before).or(Some(0)),
            None => chat.len().checked_sub(1),
        };
        if let Some(m) = idx.and_then(|i| chat.get(i)) {
            out.entry(m.id).or_default().push(a.clone());
        }
    }
    out
}

/// "✓ question — You chose: answer".
fn answered_text(a: &ChatAsk) -> String {
    match a.answer.as_deref().filter(|t| !t.is_empty()) {
        Some(answer) => format!("\u{2713} {} \u{2014} You chose: {answer}", a.text),
        None => format!("\u{2713} {}", a.text),
    }
}

/// An answered ask as one muted line.
fn answered_line(a: &ChatAsk, th: &Theme) -> AnyElement {
    let line = div().id(("ask-line", a.id as usize)).max_w(px(660.)).text_size(px(12.)).text_color(th.muted);
    line.child(answered_text(a)).into_any_element()
}

/// Asks of one chat, answered ones first, oldest first.
fn asks_of(chat: Option<i64>, open: &[ChatAsk], answered: &[ChatAsk]) -> Vec<ChatAsk> {
    let mut asks: Vec<ChatAsk> = answered.iter().chain(open).filter(|a| Some(a.chat_id) == chat).cloned().collect();
    asks.sort_by_key(|a| a.id);
    asks
}

/// "HH:MM" from "YYYY-MM-DD HH:MM:SS"; empty when too short.
pub(super) fn hhmm(created_at: &str) -> String {
    created_at.get(11..16).unwrap_or("").to_string()
}

/// Days since 1970-01-01 for a civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Day number of a "YYYY-MM-DD..." timestamp.
fn day_of(ts: &str) -> Option<i64> {
    let mut parts = ts.get(..10)?.split('-').map(|p| p.parse::<i64>());
    let (y, m, d) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    Some(days_from_civil(y, m, d))
}

/// Today as days since 1970-01-01 (UTC).
fn today_days() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    secs / 86_400
}

/// "HH:MM" for today, "Yesterday", else "N days ago".
fn when_label(updated_at: &str, today: i64) -> String {
    let Some(day) = day_of(updated_at) else { return String::new() };
    match today - day {
        ..=0 => hhmm(updated_at),
        1 => "Yesterday".into(),
        n => format!("{n} days ago"),
    }
}

/// Second line of a chat row: when, then the first 48 chars of the last message.
fn chat_sub(c: &Chat, today: i64) -> String {
    let last = match c.last.as_deref() {
        Some(l) => l.chars().take(48).map(|ch| if ch == '\n' { ' ' } else { ch }).collect(),
        None => "Empty".to_string(),
    };
    format!("{} \u{b7} {last}", when_label(&c.updated_at, today))
}

/// A chat title cut to 40 chars with "…".
fn cut_title(title: &str) -> String {
    match title.chars().count() > 40 {
        true => format!("{}\u{2026}", title.chars().take(40).collect::<String>()),
        false => title.to_string(),
    }
}

fn delete_button(c: &Chat, th: &Theme, cx: &mut Context<ChatView>) -> AnyElement {
    let (id, title) = (c.id, c.title.clone());
    let (muted, fg) = (th.muted, th.fg);
    let group = format!("chat-row-{id}");
    div()
        .id(("chat-delete", id as usize))
        .flex_none()
        .size(px(20.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .invisible()
        .group_hover(group, |s| s.visible())
        .child(svg().path("icons/close.svg").size(px(12.)).text_color(muted))
        .hover(move |s| s.bg(fg.opacity(0.12)))
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.ask_delete(id, title.clone(), cx);
        }))
        .into_any_element()
}

fn chat_row(c: &Chat, selected: bool, today: i64, th: &Theme, cx: &mut Context<ChatView>) -> AnyElement {
    let id = c.id;
    let (fg, hover_bg) = (th.fg, th.fg.opacity(0.08));
    let sub_color = if selected { th.on_accent.opacity(0.72) } else { th.muted };
    let title = div()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(c.title.clone());
    let sub = div()
        .mt(px(2.))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_size(px(10.))
        .text_color(sub_color)
        .child(chat_sub(c, today));
    let text = div().flex_1().min_w_0().child(title).child(sub);
    let row = div()
        .id(("chat-row", id as usize))
        .group(format!("chat-row-{id}"))
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(7.))
        .mb(px(2.))
        .rounded(px(9.))
        .cursor_pointer()
        .child(text)
        .child(delete_button(c, th, cx))
        .on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.select_chat(id, cx))));
    match selected {
        true => row.bg(th.sel).text_color(th.on_accent),
        false => row.text_color(th.fg_2).hover(move |s| s.bg(hover_bg).text_color(fg)),
    }
    .into_any_element()
}

fn new_chat_button(th: &Theme, cx: &mut Context<ChatView>) -> AnyElement {
    let (base, hover) = (th.fg.opacity(0.10), th.fg.opacity(0.20));
    div()
        .id("chat-new")
        .w_full()
        .h(px(30.))
        .px(px(14.))
        .rounded(px(9.))
        .flex()
        .items_center()
        .gap(px(8.))
        .bg(base)
        .text_color(th.fg)
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(svg().path("icons/plus.svg").size(px(14.)).flex_none().text_color(th.fg))
        .child("New chat")
        .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.new_chat(cx))))
        .into_any_element()
}

fn chat_list(chats: &[Chat], current: Option<i64>, cx: &mut Context<ChatView>) -> AnyElement {
    let th = theme(cx).clone();
    let today = today_days();
    let rows: Vec<AnyElement> =
        chats.iter().map(|c| chat_row(c, current == Some(c.id), today, &th, cx)).collect();
    let eyebrow = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("TALK TO THE SYSTEM");
    let name = div().text_size(px(26.)).font_weight(FontWeight::BOLD).text_color(th.fg).child("Rui");
    let top = div().id("chats-title").px(px(14.)).pt(px(12.)).pb(px(12.)).flex().flex_col().gap(px(4.)).child(eyebrow).child(name);
    let head = div().id("chats-head").px(px(12.)).pb(px(10.)).child(new_chat_button(&th, cx));
    let scroll = div()
        .id("chats-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(px(12.))
        .pb(px(20.))
        .children(rows);
    div()
        .id("chats-list")
        .w(px(232.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .bg(th.well)
        .border_r_1()
        .border_color(th.hairline)
        .child(top)
        .child(head)
        .child(scroll)
        .into_any_element()
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (chat, chats, current, questions, live, state, cards, asks, tools, live_calls) = {
            let s = self.store.read(cx);
            let live = s.current_chat.and_then(|id| live_steps(&s.chat, &s.events, id));
            let state = s.current_chat.and_then(|id| s.live.chat(id)).cloned();
            let cards = s.current_chat.map(|id| all_cards(&s.chat, &s.events, id, &s.roles)).unwrap_or_default();
            let asks = asks_of(s.current_chat, &s.asks, &s.answered_asks);
            let tools = s.current_chat.map(|id| all_tools(&s.chat, &s.events, id)).unwrap_or_default();
            let live_calls = s.current_chat.map(|id| live_tools(&s.chat, &s.events, id)).unwrap_or_default();
            (s.chat.clone(), s.chats.clone(), s.current_chat, s.questions.clone(), live, state, cards, asks, tools, live_calls)
        };
        self.sync_inputs(&questions, cx);
        let open_asks: Vec<&ChatAsk> = asks.iter().filter(|a| a.status == "open").collect();
        self.ask_inputs.sync(&open_asks, Self::answer_ask, cx);
        if current != self.last_chat {
            self.last_chat = current;
            self.last_len = 0;
        }
        let text_len = state.as_ref().map_or(0, |l| l.text.len() + 1);
        let len = chat.len() * (1 << 20) + open_asks.len() * (1 << 10) + live.as_ref().map_or(0, |l| l.len() + 1) + text_len + live_calls.len();
        if len != self.last_len {
            self.last_len = len;
            self.scroll.scroll_to_bottom();
        }
        let list = self.message_list(&chat, &questions, &asks, &cards, &tools, &live_calls, live, state.as_ref(), cx);
        let composer = self.composer(cx);
        let main = div()
            .id("chat-main")
            .flex_1()
            .min_w_0()
            .h_full()
            .pl(px(20.))
            .pt(px(12.))
            .flex()
            .flex_col()
            .child(list)
            .child(composer);
        let th = theme(cx).clone();
        let modal = self.delete_modal(&th, cx);
        let root = div().id("chat-root").relative().flex().flex_row().size_full();
        root.child(chat_list(&chats, current, cx)).child(main).children(modal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_steps_only_while_waiting_in_this_chat() {
        let msg = |role, at: &str| ChatMessage { id: 1, role, content: "hi".into(), chat_id: 1, created_at: at.into() };
        let ev = |agent: &str, chat_id: Option<i64>, at: &str| Event {
            id: 1,
            task_id: None,
            chat_id,
            agent: agent.into(),
            kind: "progress".into(),
            payload: serde_json::json!({"note": "looking"}),
            created_at: at.into(),
        };
        let events = vec![
            ev("rui", Some(1), "2026-11-15T10:00:00Z"),
            ev("rui", Some(1), "2026-11-15T10:00:05Z"),
            ev("rui", Some(2), "2026-11-15T10:00:06Z"),
            ev("lead", Some(1), "2026-11-15T10:00:06Z"),
        ];
        let waiting = vec![msg(ChatRole::User, "2026-11-15T10:00:03Z")];
        assert_eq!(live_steps(&waiting, &events, 1).map(|s| s.len()), Some(1));
        assert_eq!(live_steps(&waiting, &events, 2).map(|s| s.len()), Some(1));
        assert_eq!(live_steps(&waiting, &events, 3).map(|s| s.len()), Some(0));
        let answered = vec![msg(ChatRole::User, "2026-11-15T10:00:03Z"), msg(ChatRole::Rui, "2026-11-15T10:00:09Z")];
        assert_eq!(live_steps(&answered, &events, 1), None);
    }

    #[test]
    fn staff_card_text() {
        let role = |n: &str, t: &str| RoleInfo { name: n.into(), code: "X".into(), title: t.into(), max_loops: 5, ..Default::default() };
        let roles = [role("backend", "Backend Engineer")];
        let input = serde_json::json!({"project":"acme","title":"Add login","brief":"b","pod":["backend","tester"]});
        let c = staff_card(&input, &roles);
        assert_eq!((c.body.as_str(), c.scope.as_str()), ("Add login \u{2014} Backend Engineer, Tester", "acme"));
        let general = staff_card(&serde_json::json!({"title":"T","brief":"b","pod":[]}), &roles);
        assert_eq!((general.body.as_str(), general.scope.as_str()), ("T", "General task"));
    }

    #[test]
    fn staff_inputs_found_in_turn_window() {
        let ev = |at: &str, name: &str| Event {
            id: 1, task_id: None, chat_id: Some(1), agent: "rui".into(), kind: "assistant".into(),
            payload: serde_json::json!({"message":{"content":[{"type":"tool_use","name":name,"input":{"title":"x"}}]}}),
            created_at: at.into(),
        };
        let events = vec![ev("2026-11-15T10:00:05.000Z", STAFF_TOOL), ev("2026-11-15T10:00:06.000Z", "mcp__rui__use_repo"), ev("2026-11-15T09:00:00.000Z", STAFF_TOOL)];
        let from = parse_secs("2026-11-15 10:00:03").unwrap();
        let to = parse_secs("2026-11-15 10:00:09").unwrap();
        assert_eq!(staff_calls(&events, 1, from, to).len(), 1);
        assert_eq!(staff_calls(&events, 2, from, to).len(), 0);
    }

    #[test]
    fn staff_call_reads_task_id_from_result() {
        let use_ev = Event {
            id: 1, task_id: None, chat_id: Some(1), agent: "rui".into(), kind: "assistant".into(),
            payload: serde_json::json!({"message":{"content":[{"type":"tool_use","id":"t1","name":STAFF_TOOL,"input":{"title":"x"}}]}}),
            created_at: "2026-11-15T10:00:05.000Z".into(),
        };
        let res = r#"{"project":"acme","repo_path":"/r","task_id":42,"pod":["lead"]}"#;
        let res_ev = Event {
            kind: "toolresult".into(),
            payload: serde_json::json!({"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":res,"is_error":false}]}}),
            ..use_ev.clone()
        };
        let (from, to) = (parse_secs("2026-11-15 10:00:00").unwrap(), parse_secs("2026-11-15 10:01:00").unwrap());
        assert_eq!(staff_calls(&[use_ev.clone(), res_ev], 1, from, to)[0].1, Some(42));
        assert_eq!(staff_calls(&[use_ev], 1, from, to)[0].1, None);
        assert_eq!(task_id_of("Held for the owner's OK"), None);
    }

    fn placed_ask(id: i64, at: &str) -> ChatAsk {
        ChatAsk { id, chat_id: 1, chat_title: "t".into(), text: "Which?".into(), options: vec![], answer: Some("A".into()), status: "answered".into(), created_at: at.into() }
    }

    #[test]
    fn asks_sit_after_the_message_they_followed() {
        let msg = |id: i64, at: &str| ChatMessage { id, role: ChatRole::Rui, content: "x".into(), chat_id: 1, created_at: at.into() };
        let chat = [msg(1, "2026-11-15 10:00:10"), msg(2, "2026-11-15 10:00:20"), msg(3, "2026-11-15 10:00:20")];
        let asks = [placed_ask(1, "2026-11-15 10:00:05"), placed_ask(2, "2026-11-15 10:00:15"), placed_ask(3, "2026-11-15 10:00:20"), placed_ask(4, "2026-11-15 10:09:00")];
        let got = place_asks(&chat, &asks);
        let ids = |m: i64| got.get(&m).map(|v| v.iter().map(|a| a.id).collect::<Vec<_>>());
        assert_eq!(ids(1), Some(vec![1, 2]));
        assert_eq!(ids(2), None);
        assert_eq!(ids(3), Some(vec![3, 4]));
        let open = ChatAsk { status: "open".into(), ..placed_ask(9, "2026-11-15 10:00:12") };
        assert!(place_asks(&chat, &[open]).is_empty());
    }

    #[test]
    fn answered_line_text() {
        assert_eq!(answered_text(&placed_ask(1, "a")), "\u{2713} Which? \u{2014} You chose: A");
    }

    #[test]
    fn asks_of_keeps_one_chat_in_order() {
        let ask = |id: i64, chat_id: i64, status: &str| ChatAsk {
            id, chat_id, chat_title: "t".into(), text: "q".into(), options: vec![], answer: None, status: status.into(), created_at: "a".into(),
        };
        let got = asks_of(Some(1), &[ask(5, 1, "open"), ask(6, 2, "open")], &[ask(2, 1, "answered")]);
        assert_eq!(got.iter().map(|a| a.id).collect::<Vec<_>>(), [2, 5]);
        assert!(asks_of(None, &[ask(5, 1, "open")], &[]).is_empty());
    }

    #[test]
    fn cut_title_limits_to_40_chars() {
        assert_eq!(cut_title("short"), "short");
        assert_eq!(cut_title(&"a".repeat(40)), "a".repeat(40));
        assert_eq!(cut_title(&"b".repeat(41)), format!("{}\u{2026}", "b".repeat(40)));
    }

    #[test]
    fn hhmm_extracts_time() {
        assert_eq!(hhmm("2026-11-15 09:41:07"), "09:41");
        assert_eq!(hhmm("short"), "");
        assert_eq!(hhmm(""), "");
    }

    #[test]
    fn day_numbers() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 15), 20_772);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        assert_eq!(day_of("2026-11-15 09:41:07"), Some(20_772));
        assert_eq!(day_of("bad"), None);
    }

    #[test]
    fn when_labels() {
        let today = 20_772;
        assert_eq!(when_label("2026-11-15 09:41:07", today), "09:41");
        assert_eq!(when_label("2026-11-14 23:00:00", today), "Yesterday");
        assert_eq!(when_label("2026-11-10 23:00:00", today), "5 days ago");
        assert_eq!(when_label("", today), "");
    }

    #[test]
    fn chat_sub_text() {
        let mut c = Chat { id: 1, title: "t".into(), created_at: "a".into(), updated_at: "2026-11-15 09:41:07".into(), last: None };
        assert_eq!(chat_sub(&c, 20_772), "09:41 \u{b7} Empty");
        c.last = Some("x".repeat(60));
        assert_eq!(chat_sub(&c, 20_772), format!("09:41 \u{b7} {}", "x".repeat(48)));
    }
}
