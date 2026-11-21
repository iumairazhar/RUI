//! The thread composer. When the team waits on the owner it shows the question above the field,
//! with its options as quick answers and a switch: "Answer the question" or "Send as a message".
//! Where the text goes follows that switch only.

use super::{ThreadsView, icon};
use crate::api::{Node, OpenQuestion};
use crate::theme::Theme;
use crate::views::chip;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Stateful, div, prelude::*, px};
use rui_core::QuestionStatus;

/// Where the owner's text goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Answer,
    Message,
}

/// The call a send makes: answer a question by id, or message the team.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Route {
    Answer(i64),
    Message,
}

/// The composer's own state on the open task page.
#[derive(Debug, Default)]
pub struct Compose {
    /// The open question the owner switched to "Send as a message" for.
    message_for: Option<i64>,
    /// The long question text is shown in full.
    show_all: bool,
    /// Thread and its reply count when the owner last sent, for the "Sent" note.
    sent: Option<(i64, usize)>,
}

/// Answer while a question is open, unless the owner switched this question to a message.
pub fn mode_of(q: Option<&OpenQuestion>, message_for: Option<i64>) -> Mode {
    match q {
        Some(q) if message_for != Some(q.id) => Mode::Answer,
        _ => Mode::Message,
    }
}

pub fn send_label(m: Mode) -> &'static str {
    match m {
        Mode::Answer => "Send answer",
        Mode::Message => "Send message",
    }
}

pub fn placeholder(m: Mode) -> &'static str {
    match m {
        Mode::Answer => "Type your answer\u{2026}",
        Mode::Message => "Write to the team\u{2026}",
    }
}

/// Only Answer mode with an open question answers it; everything else is a message.
pub fn route(m: Mode, q: Option<&OpenQuestion>) -> Route {
    match (m, q) {
        (Mode::Answer, Some(q)) => Route::Answer(q.id),
        _ => Route::Message,
    }
}

/// How many "reply" nodes the Lead has posted in a thread.
pub fn reply_count(nodes: &[Node]) -> usize {
    nodes.iter().filter(|n| n.kind == "reply").count()
}

/// The "Sent" note stays until a new reply lands in the same thread.
pub fn awaiting_reply(sent: Option<(i64, usize)>, thread_id: i64, replies: usize) -> bool {
    sent.is_some_and(|(id, at)| id == thread_id && replies <= at)
}

/// A question longer than about four lines gets a "Show all" link.
pub fn is_long(text: &str) -> bool {
    text.lines().count() > 4 || text.chars().count() > 320
}

/// An option that asks the owner to write something, not an answer to send as-is.
pub fn types_instead(option: &str) -> bool {
    option.trim().to_lowercase().starts_with("give new instructions")
}

impl ThreadsView {
    /// The thread's open question: from the thread map, else the task's own open owner question.
    pub(super) fn page_question(&self, id: i64, cx: &Context<Self>) -> Option<OpenQuestion> {
        let s = self.store.read(cx);
        if let Some(q) = s.thread_cache.get(&id).and_then(|t| t.open_question.clone()) {
            return Some(q);
        }
        let open = |q: &&rui_core::Question| q.task_id == id && q.status == QuestionStatus::Open && q.level == "owner";
        s.questions.iter().find(open).map(|q| OpenQuestion { id: q.id, text: q.text.clone(), ..Default::default() })
    }

    fn reply_count_of(&self, id: i64, cx: &Context<Self>) -> usize {
        self.store.read(cx).thread_cache.get(&id).map_or(0, |t| reply_count(&t.nodes))
    }

    pub(super) fn reset_compose(&mut self) {
        self.compose = Compose::default();
    }

    /// Send the field's text where the switch says: the open question, or a message to the team.
    pub(super) fn send(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.open else { return };
        let text = self.input.read(cx).content();
        if text.trim().is_empty() {
            return;
        }
        let q = self.page_question(id, cx);
        let to = route(mode_of(q.as_ref(), self.compose.message_for), q.as_ref());
        self.input.update(cx, |i, cx| i.clear(cx));
        match to {
            Route::Answer(qid) => self.store.update(cx, |s, cx| s.answer(qid, text, None, cx)),
            Route::Message => {
                let files = self.picked.take_ids();
                self.store.update(cx, |s, cx| s.instruct_with_files(id, text, files, cx));
            }
        }
        self.mark_sent(id, cx);
    }

    /// Answer the open question with one of its options.
    fn pick_option(&mut self, qid: i64, answer: String, cx: &mut Context<Self>) {
        // "Give new instructions" means "let me type them": switch to answering instead of sending the label.
        if types_instead(&answer) {
            self.set_mode(Mode::Answer, qid, cx);
            self.store.update(cx, |s, cx| s.show_toast("Type your instructions below, then Send answer", vec![], cx));
            return;
        }
        self.store.update(cx, |s, cx| s.answer(qid, answer, None, cx));
        if let Some(id) = self.open {
            self.mark_sent(id, cx);
        }
    }

    fn mark_sent(&mut self, id: i64, cx: &mut Context<Self>) {
        self.compose.sent = Some((id, self.reply_count_of(id, cx)));
        cx.notify();
    }

    fn set_mode(&mut self, m: Mode, qid: i64, cx: &mut Context<Self>) {
        self.compose.message_for = (m == Mode::Message).then_some(qid);
        cx.notify();
    }

    fn toggle_show_all(&mut self, cx: &mut Context<Self>) {
        self.compose.show_all = !self.compose.show_all;
        cx.notify();
    }

    /// The question strip (when one is open), the "Sent" note, then the field with the switch and Send.
    pub(super) fn composer(&self, id: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let q = self.page_question(id, cx);
        let mode = mode_of(q.as_ref(), self.compose.message_for);
        self.input.update(cx, |i, _| _ = i.set_placeholder(placeholder(mode)));
        let strip = q.as_ref().map(|q| self.question_strip(q, th, cx));
        let note = awaiting_reply(self.compose.sent, id, self.reply_count_of(id, cx)).then(|| sent_note(th));
        let field = div().flex_1().min_w_0().child(self.input.clone());
        let send = chip("thread-send", send_label(mode), cx).on_click(cx.listener(|this, _, _, cx| this.send(cx)));
        let attach = (mode == Mode::Message).then(|| crate::views::attach::attach_button("thread-attach", &self.store, Self::picked, cx));
        let chips = crate::views::attach::picked_row("thread-file", self.picked.items(), Self::picked, th, cx);
        let switch = q.as_ref().map(|q| self.mode_switch(mode, q.id, th, cx));
        let bar = div().flex().items_end().gap(px(10.)).p(px(6.)).rounded(px(14.)).bg(th.surface_2).border_1().border_color(th.hairline);
        let bar = bar.child(field).children(attach).children(switch).child(send);
        let wrap = div().id("threads-composer").flex_none().flex().flex_col().gap(px(8.)).px(px(14.)).pb(px(14.)).pt(px(4.));
        let wrap = wrap.children(strip).children(note);
        crate::views::attach::on_drop(wrap, &self.store, Self::picked, cx).children(chips).child(bar).into_any_element()
    }

    /// "The team needs your answer", the question (clamped, with "Show all") and its options.
    fn question_strip(&self, q: &OpenQuestion, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let head = div().flex().items_center().gap(px(8.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(th.wait.fg);
        let head = head.child(icon("hand", 13., th.wait.fg)).child("The team needs your answer");
        let long = is_long(&q.text);
        let mut text = div().text_size(px(13.)).text_color(th.fg).child(q.text.clone());
        if long && !self.compose.show_all {
            text = text.line_clamp(4);
        }
        let more = long.then(|| self.show_all_link(th, cx));
        let options = self.option_buttons(q, cx);
        let strip = div().id("thread-question").flex().flex_col().gap(px(6.)).px(px(14.)).py(px(10.)).rounded(px(10.));
        let strip = strip.bg(th.wait.dim).border_1().border_color(th.wait.rim);
        strip.child(head).child(text).children(more).children(options).into_any_element()
    }

    fn show_all_link(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let label = if self.compose.show_all { "Show less" } else { "Show all" };
        let hover = th.fg;
        let link = div().id("thread-question-more").text_size(px(11.5)).text_color(th.muted).cursor_pointer();
        let link = link.hover(move |s| s.text_color(hover)).on_click(cx.listener(|this, _, _, cx| this.toggle_show_all(cx)));
        link.child(label).into_any_element()
    }

    /// One button per option; a click answers the question with it right away.
    fn option_buttons(&self, q: &OpenQuestion, cx: &mut Context<Self>) -> Option<AnyElement> {
        if q.options.is_empty() {
            return None;
        }
        let qid = q.id;
        let button = |(i, opt): (usize, &String)| {
            let answer = opt.clone();
            let b = chip(SharedString::from(format!("thread-option-{i}")), opt, cx);
            b.on_click(cx.listener(move |this, _, _, cx| this.pick_option(qid, answer.clone(), cx)))
        };
        let row = div().flex().flex_wrap().gap(px(6.)).pt(px(2.)).children(q.options.iter().enumerate().map(button));
        Some(row.into_any_element())
    }

    /// "Answer the question" / "Send as a message", the picked one raised.
    fn mode_switch(&self, mode: Mode, qid: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let seg = |m: Mode, label: &str, el_id: &str| -> Stateful<Div> {
            let on = m == mode;
            let el = div().id(SharedString::from(el_id.to_string())).flex().items_center().h(px(26.)).px(px(10.)).rounded(px(7.)).text_size(px(12.));
            let el = if on { el.bg(th.surface).font_weight(FontWeight::SEMIBOLD).text_color(th.fg) } else { el.text_color(th.muted).cursor_pointer() };
            el.on_click(cx.listener(move |this, _, _, cx| this.set_mode(m, qid, cx))).child(label.to_string())
        };
        let answer = seg(Mode::Answer, "Answer the question", "thread-mode-answer");
        let message = seg(Mode::Message, "Send as a message", "thread-mode-message");
        div().flex().flex_none().items_center().p(px(3.)).rounded(px(9.)).bg(th.well).child(answer).child(message).into_any_element()
    }
}

/// "Sent — the Lead will reply here.", muted, until the next reply lands.
fn sent_note(th: &Theme) -> AnyElement {
    let text = "Sent \u{2014} the Lead will reply here.";
    div().px(px(4.)).text_size(px(12.)).text_color(th.muted).child(text).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: i64) -> OpenQuestion {
        OpenQuestion { id, text: "Which db?".into(), ..Default::default() }
    }

    #[test]
    fn answers_by_default_only_while_a_question_is_open() {
        assert_eq!(mode_of(Some(&q(3)), None), Mode::Answer);
        assert_eq!(mode_of(Some(&q(3)), Some(3)), Mode::Message);
        // Switching to a message for one question does not carry over to the next.
        assert_eq!(mode_of(Some(&q(4)), Some(3)), Mode::Answer);
        assert_eq!(mode_of(None, None), Mode::Message);
    }

    #[test]
    fn labels_follow_the_mode() {
        assert_eq!((send_label(Mode::Answer), send_label(Mode::Message)), ("Send answer", "Send message"));
        assert_eq!(placeholder(Mode::Answer), "Type your answer\u{2026}");
        assert!(types_instead("Give new instructions"));
        assert!(!types_instead("Retry with more tries"));
        assert!(!types_instead("Abandon the task"));
        assert_eq!(placeholder(Mode::Message), "Write to the team\u{2026}");
    }

    #[test]
    fn routing_follows_the_switch_only() {
        assert_eq!(route(Mode::Answer, Some(&q(3))), Route::Answer(3));
        assert_eq!(route(Mode::Message, Some(&q(3))), Route::Message);
        assert_eq!(route(Mode::Message, None), Route::Message);
        assert_eq!(route(Mode::Answer, None), Route::Message);
    }

    #[test]
    fn sent_note_waits_for_a_new_reply_in_that_thread() {
        let reply = Node { id: "r".into(), kind: "reply".into(), ..Default::default() };
        let other = Node { id: "s".into(), kind: "step".into(), ..Default::default() };
        assert_eq!(reply_count(&[reply.clone(), other]), 1);
        assert!(awaiting_reply(Some((7, 1)), 7, 1));
        assert!(!awaiting_reply(Some((7, 1)), 7, 2));
        assert!(!awaiting_reply(Some((7, 1)), 8, 1));
        assert!(!awaiting_reply(None, 7, 0));
    }

    #[test]
    fn long_questions_fold() {
        assert!(!is_long("Which db?"));
        assert!(is_long("a\nb\nc\nd\ne"));
        assert!(is_long(&"x".repeat(400)));
    }
}
