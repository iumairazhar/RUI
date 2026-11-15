//! Chat column plus the "Since you last looked" panel.

use super::{avatar, chip, page_head, primary_button, question_card, task_status};
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Theme, theme};
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, ScrollHandle, Window, div, prelude::*, px,
};
use rui_core::{ChatMessage, ChatRole, Question, Task, TaskStatus};
use std::collections::HashMap;

/// Chat messages, open questions, a composer and the exceptions panel.
pub struct ChatView {
    store: Entity<Store>,
    input: Entity<TextInput>,
    answers: HashMap<i64, Entity<TextInput>>,
    scroll: ScrollHandle,
    last_len: usize,
}

impl ChatView {
    /// Build the chat view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let input = cx.new(|cx| TextInput::new("Tell Rui what to set up…", cx));
        cx.subscribe(&input, |this, _, _: &Submit, cx| this.send(cx)).detach();
        Self { store, input, answers: HashMap::new(), scroll: ScrollHandle::new(), last_len: 0 }
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

    fn message_list(&mut self, chat: &[ChatMessage], questions: &[Question], cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let open: Vec<AnyElement> = questions.iter().map(|q| self.question(q, cx)).collect();
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
                .text_color(th.on_accent)
                .text_size(px(13.5))
                .line_height(px(20.))
                .child(text.to_string()),
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
        .text_size(px(13.5))
        .line_height(px(20.))
        .text_color(th.fg)
        .child(m.content.clone());
    let col = div().flex().flex_col().gap(px(7.)).child(meta).child(div().flex().child(bubble));
    col.into_any_element()
}

/// "HH:MM" from "YYYY-MM-DD HH:MM:SS"; empty when too short.
fn hhmm(created_at: &str) -> String {
    created_at.get(11..16).unwrap_or("").to_string()
}

/// Civil (year, month, day) for days since 1970-01-01.
fn civil_date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Today's UTC date as "YYYY-MM-DD".
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_date(secs / 86_400);
    format!("{y:04}-{m:02}-{d:02}")
}

fn needs_you(tasks: &[Task]) -> Vec<&Task> {
    let wanted = |s: &TaskStatus| matches!(s, TaskStatus::WaitingAnswer | TaskStatus::Stuck);
    tasks.iter().filter(|t| wanted(&t.status)).collect()
}

fn landed_today<'a>(tasks: &'a [Task], today: &str) -> Vec<&'a Task> {
    tasks.iter().filter(|t| t.status == TaskStatus::Done && t.updated_at.starts_with(today)).collect()
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn status_words(s: &TaskStatus) -> &'static str {
    match s {
        TaskStatus::WaitingAnswer => "Needs you",
        TaskStatus::Stuck => "Stuck",
        TaskStatus::Done => "Landed",
        _ => "",
    }
}

fn group_header(label: &str, count: usize, th: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(20.))
        .mt(px(20.))
        .mb(px(8.))
        .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(label.to_string()))
        .child(div().font_family("Menlo").text_size(px(11.)).text_color(th.muted).child(count.to_string()))
        .into_any_element()
}

fn row_meta(t: &Task, store: &Store) -> String {
    let client = store.clients.iter().find(|c| c.id == t.client_id).map_or("—", |c| c.name.as_str());
    let model = store.runs.get(&t.id).and_then(|r| r.model.as_deref()).unwrap_or("—");
    format!("{client} · {} · {model}", status_words(&t.status))
}

fn task_row(t: &Task, store: &Store, th: &Theme, cx: &App) -> AnyElement {
    let title = format!("{} · {}", capitalize(t.role.as_str()), t.title);
    let text = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(th.fg)
                .child(title),
        )
        .child(
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(11.))
                .text_color(th.muted)
                .child(row_meta(t, store)),
        );
    let bot = avatar(th.client_color(t.client_id), task_status(&t.status, th).base, px(34.), cx);
    let surface_2 = th.surface_2;
    div()
        .id(("since-row", t.id as usize))
        .mx(px(12.))
        .mb(px(6.))
        .px(px(10.))
        .py(px(9.))
        .rounded(px(10.))
        .bg(th.surface)
        .shadow(th.card_shadow())
        .hover(move |s| s.bg(surface_2))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(bot)
        .child(text)
        .into_any_element()
}

fn group(label: &str, tasks: &[&Task], store: &Store, th: &Theme, cx: &App) -> AnyElement {
    let mut col = div().flex().flex_col().child(group_header(label, tasks.len(), th));
    if tasks.is_empty() {
        col = col.child(div().px(px(20.)).text_size(px(11.5)).text_color(th.muted).child("Nothing here."));
    }
    col.children(tasks.iter().map(|t| task_row(t, store, th, cx))).into_any_element()
}

fn side_panel(cx: &App, store: &Entity<Store>) -> AnyElement {
    let th = theme(cx).clone();
    let store = store.read(cx);
    let (need, landed) = (needs_you(&store.tasks), landed_today(&store.tasks, &today()));
    let head = div()
        .pt(px(28.))
        .px(px(20.))
        .pb(px(16.))
        .child(
            div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child("Since you last looked"),
        )
        .child(
            div()
                .mt(px(4.))
                .text_size(px(11.5))
                .text_color(th.muted)
                .child("Only the exceptions. The whole picture lives in Threads."),
        );
    let g1 = group("NEEDS YOU", &need, store, &th, cx);
    let g2 = group("LANDED TODAY", &landed, store, &th, cx);
    div()
        .id("since-panel")
        .w(px(340.))
        .flex_none()
        .h_full()
        .bg(th.well)
        .border_l_1()
        .border_color(th.hairline)
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .child(head)
        .child(g1)
        .child(g2)
        .into_any_element()
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (chat, questions) = {
            let s = self.store.read(cx);
            (s.chat.clone(), s.questions.clone())
        };
        self.sync_inputs(&questions, cx);
        if chat.len() != self.last_len {
            self.last_len = chat.len();
            self.scroll.scroll_to_bottom();
        }
        let list = self.message_list(&chat, &questions, cx);
        let composer = self.composer(cx);
        let store = self.store.clone();
        let main = div()
            .id("chat-main")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(page_head("TALK TO THE SYSTEM", "Rui", cx).pr(px(40.)))
            .child(list)
            .child(composer);
        div().id("chat-root").flex().flex_row().size_full().child(main).child(side_panel(cx, &store))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hhmm_extracts_time() {
        assert_eq!(hhmm("2026-11-15 09:41:07"), "09:41");
        assert_eq!(hhmm("short"), "");
        assert_eq!(hhmm(""), "");
    }

    #[test]
    fn civil_date_known_days() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(20_772), (2026, 11, 15));
        assert_eq!(civil_date(19_782), (2024, 2, 29));
        assert_eq!(civil_date(-1), (1969, 12, 31));
    }

    #[test]
    fn today_is_formatted() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert_eq!(t.as_bytes()[4], b'-');
    }

    #[test]
    fn capitalize_works() {
        assert_eq!(capitalize("lead"), "Lead");
        assert_eq!(capitalize(""), "");
    }
}
