//! Chats page: the list of chats beside the selected chat.

use super::live::live_action;
use super::{chip, markdown, page_head, primary_button, question_card};
use crate::api::LiveState;
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Theme, theme};
use gpui::{
    AnyElement, Context, Entity, FontWeight, ScrollHandle, Window, div, prelude::*, px, svg,
};
use super::activity::summarize;
use rui_core::{Chat, ChatMessage, ChatRole, Event, Question};
use std::collections::HashMap;

/// Chat list, messages of the selected chat, open questions and a composer.
pub struct ChatView {
    store: Entity<Store>,
    input: Entity<TextInput>,
    answers: HashMap<i64, Entity<TextInput>>,
    scroll: ScrollHandle,
    last_len: usize,
    last_chat: Option<i64>,
}

impl ChatView {
    /// Build the chat view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let input = cx.new(|cx| TextInput::new("Tell Rui what to set up…", cx));
        cx.subscribe(&input, |this, _, _: &Submit, cx| this.send(cx)).detach();
        Self { store, input, answers: HashMap::new(), scroll: ScrollHandle::new(), last_len: 0, last_chat: None }
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
        self.store.update(cx, |s, cx| s.answer(id, text, cx));
    }

    fn question(&self, q: &Question, cx: &mut Context<Self>) -> AnyElement {
        let id = q.id;
        let input = match self.answers.get(&id) {
            Some(i) => i.clone().into_any_element(),
            None => div().into_any_element(),
        };
        let btn = chip(format!("answer-{id}"), "Answer", cx)
            .on_click(cx.listener(move |this, _, _, cx| this.answer(id, cx)));
        question_card(&q.text, input, btn.into_any_element(), cx).into_any_element()
    }

    fn message_list(
        &mut self,
        chat: &[ChatMessage],
        questions: &[Question],
        live: Option<Vec<String>>,
        state: Option<&LiveState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        let open: Vec<AnyElement> = questions.iter().map(|q| self.question(q, cx)).collect();
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
            .children(chat.iter().map(|m| message(m, &th)))
            .children(working)
            .children(open);
        if chat.is_empty() && questions.is_empty() {
            list = list.child(div().text_size(px(13.)).text_color(th.muted).child("Tell Rui what to set up."));
        }
        list.into_any_element()
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
    let lines: Vec<AnyElement> = match (state, steps.is_empty()) {
        (Some(l), _) => live_lines(l, th),
        (None, true) => vec![div().child("Thinking…").into_any_element()],
        (None, false) => steps.iter().map(|s| div().child(s.clone()).into_any_element()).collect(),
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

fn chat_row(c: &Chat, selected: bool, today: i64, th: &Theme, cx: &mut Context<ChatView>) -> AnyElement {
    let id = c.id;
    let (fg, hover_bg) = (th.fg, th.fg.opacity(0.08));
    let sub_color = if selected { th.on_accent.opacity(0.72) } else { th.muted };
    let title = div()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .child(c.title.clone());
    let sub = div()
        .mt(px(3.))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_size(px(10.5))
        .text_color(sub_color)
        .child(chat_sub(c, today));
    let row = div()
        .id(("chat-row", id as usize))
        .px(px(10.))
        .py(px(9.))
        .mb(px(3.))
        .rounded(px(9.))
        .cursor_pointer()
        .child(title)
        .child(sub)
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
        .h(px(34.))
        .px(px(14.))
        .rounded(px(9.))
        .flex()
        .items_center()
        .gap(px(8.))
        .bg(base)
        .text_color(th.fg)
        .text_size(px(12.5))
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
    let head = div().id("chats-head").px(px(12.)).pt(px(16.)).pb(px(12.)).child(new_chat_button(&th, cx));
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
        .w(px(296.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .bg(th.well)
        .border_r_1()
        .border_color(th.hairline)
        .child(head)
        .child(scroll)
        .into_any_element()
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (chat, chats, current, questions, live, state) = {
            let s = self.store.read(cx);
            let live = s.current_chat.and_then(|id| live_steps(&s.chat, &s.events, id));
            let state = s.current_chat.and_then(|id| s.live.chat(id)).cloned();
            (s.chat.clone(), s.chats.clone(), s.current_chat, s.questions.clone(), live, state)
        };
        self.sync_inputs(&questions, cx);
        if current != self.last_chat {
            self.last_chat = current;
            self.last_len = 0;
        }
        let text_len = state.as_ref().map_or(0, |l| l.text.len() + 1);
        let len = chat.len() * (1 << 20) + live.as_ref().map_or(0, |l| l.len() + 1) + text_len;
        if len != self.last_len {
            self.last_len = len;
            self.scroll.scroll_to_bottom();
        }
        let list = self.message_list(&chat, &questions, live, state.as_ref(), cx);
        let composer = self.composer(cx);
        let main = div()
            .id("chat-main")
            .flex_1()
            .min_w_0()
            .h_full()
            .pl(px(20.))
            .flex()
            .flex_col()
            .child(list)
            .child(composer);
        let body = div()
            .id("chat-body")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(chat_list(&chats, current, cx))
            .child(main);
        let head = page_head("TALK TO THE SYSTEM", "Rui", cx).pr(px(40.));
        div().id("chat-root").flex().flex_col().size_full().child(head).child(body)
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
