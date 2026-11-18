//! Inbox: a compact list of what needs the owner; a row opens a drawer to read it and answer.

use super::asks::AskInputs;
use super::decision::{self, CardEvent, Cards, first_line};
use super::character::{CharState, character};
use super::library::{after_slide, close_button, drawer_shell, scroll_body, scrim, slide_overlay};
use super::motion::rise;
use super::status::{Word, char_state, style};
use super::{chip, markdown, page_head, primary_button, tag};
use crate::api::InboxItem;
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Theme, theme};
use gpui::{
    AnyElement, Context, Div, Entity, FocusHandle, FontWeight, Hsla, KeyDownEvent, Stateful, Window, div,
    prelude::*, px,
};
use rui_core::{ChatAsk, Question, QuestionStatus, Task};
use std::collections::HashMap;

const ASK_SUB: &str = "Rui \u{b7} asked in chat";
const NONE_TEXT: &str = "Nothing needs you right now.";

/// What the drawer shows.
#[derive(Clone, Copy, PartialEq)]
enum Open {
    Question(i64),
    Ask(i64),
    Stuck(i64),
    Review(i64),
}

/// Key of a drawer's text input: (kind, id).
type InputKey = (&'static str, i64);

/// Open questions and asks as rows; the open one slides in as a drawer.
pub struct QuestionsView {
    store: Entity<Store>,
    /// Decision cards for question drawers.
    cards: Cards,
    ask_inputs: AskInputs,
    /// Inputs of the stuck, review and fallback-question drawers.
    notes: HashMap<InputKey, Entity<TextInput>>,
    /// "Ask for changes" input is showing in the review drawer.
    reveal: bool,
    /// "Drop it" was clicked once and waits for a second click.
    confirm_drop: bool,
    open: Option<Open>,
    /// Drawer is playing its slide-out before it closes.
    closing: bool,
    /// The option picked in an ask's drawer.
    pick: Option<String>,
    focus: FocusHandle,
    want_focus: bool,
}

/// Text after the bold count in the lead line.
fn lead_tail(n: usize) -> &'static str {
    match n {
        1 => " item needs you.",
        _ => " items need you.",
    }
}

fn lead_line(n: usize, th: &Theme) -> Stateful<Div> {
    let base = div().id("inbox-lead").flex().text_size(px(14.)).text_color(th.fg_2).pb(px(16.));
    if n == 0 {
        return base.child(NONE_TEXT);
    }
    let bold = div().font_weight(FontWeight::BOLD).text_color(th.fg).child(n.to_string());
    base.child(bold).child(lead_tail(n))
}

/// Where a question came from: its task and the agent that asked.
struct Origin {
    task: Option<Task>,
    agent: String,
}

impl Origin {
    fn new(q: &Question, s: &Store) -> Self {
        let task = s.tasks.iter().find(|t| t.id == q.task_id).cloned();
        let agent = task.as_ref().and_then(|t| t.agent.clone()).unwrap_or_else(|| "Pod Lead".to_string());
        Self { task, agent }
    }

    fn color(&self, th: &Theme) -> Hsla {
        th.client_color(self.task.as_ref().map_or(0, |t| t.client_id))
    }
}

fn muted_line(text: String, size: f32, th: &Theme) -> Div {
    div().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(size)).text_color(th.muted).child(text)
}

fn heading(text: &str, th: &Theme) -> Div {
    div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(text.to_uppercase())
}

impl QuestionsView {
    /// Build the questions view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let focus = cx.focus_handle();
        Self {
            store, cards: Cards::default(), ask_inputs: AskInputs::default(), notes: HashMap::new(), reveal: false, confirm_drop: false, open: None, closing: false, pick: None, focus, want_focus: false,
        }
    }

    // ---- opening and closing ----

    /// Open the drawer for an inbox item by kind and id (from the Inbox slide-over).
    pub fn open_kind(&mut self, kind: &str, id: i64, cx: &mut Context<Self>) {
        if let Some((open, ..)) = kind_parts(kind, id) {
            self.open_item(open, cx);
        }
    }

    fn open_item(&mut self, item: Open, cx: &mut Context<Self>) {
        if let Open::Question(qid) = item {
            let top = decision::question_thread(self.store.read(cx), qid);
            self.store.update(cx, |s, cx| top.into_iter().for_each(|id| s.load_thread(id, cx)));
        }
        (self.open, self.closing, self.pick, self.want_focus) = (Some(item), false, None, true);
        (self.reveal, self.confirm_drop) = (false, false);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open.is_none() || self.closing {
            return;
        }
        self.closing = true;
        cx.notify();
        after_slide(cx, Self::finish_close);
    }

    fn finish_close(&mut self, cx: &mut Context<Self>) {
        if !self.closing {
            return;
        }
        (self.closing, self.open, self.pick) = (false, None, None);
        (self.reveal, self.confirm_drop) = (false, false);
        cx.notify();
    }

    fn key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        if ev.keystroke.key == "escape" {
            self.close(cx);
        }
    }

    fn open_thread(&mut self, task_id: Option<i64>, cx: &mut Context<Self>) {
        (self.open, self.closing) = (None, false);
        match task_id {
            Some(id) => self.store.update(cx, |s, cx| s.open_thread(id, cx)),
            None => self.store.update(cx, |s, cx| s.go_to_threads(cx)),
        }
    }

    /// Answer an ask with a picked option, or with what was typed when `text` is `None`.
    fn answer_ask(&mut self, id: i64, text: Option<String>, cx: &mut Context<Self>) {
        let typed = || self.ask_inputs.take(id, cx);
        let Some(text) = text.or_else(typed).or_else(|| self.pick.clone()) else { return };
        self.store.update(cx, |s, cx| s.answer_ask(id, text, cx));
        self.close(cx);
    }

    // ---- rows ----

    fn row(&self, i: usize, r: Row, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let (item, n) = (r.open, r.id as usize);
        let hover = th.surface_2;
        let name = div().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(13.));
        let name = name.font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(r.title);
        let text = div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(name).child(muted_line(r.sub, 11.5, &th));
        let face = character(r.face, px(36.), (r.key, n), r.color, &th);
        let row = div().id((r.key, n)).flex().items_center().gap(px(12.)).px(px(14.)).py(px(11.)).mb(px(6.));
        let row = row.rounded(px(10.)).bg(th.surface).cursor_pointer().hover(move |s| s.bg(hover));
        let row = row.on_click(cx.listener(move |this, _, _, cx| this.open_item(item, cx)));
        let row = row.child(face).child(text).child(tag(style(r.word, &th), r.tag));
        rise(row, (r.rise, n), i).into_any_element()
    }

    fn rows(&self, items: &[InboxItem], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let th = theme(cx).clone();
        let rows: Vec<Row> = items.iter().filter_map(|it| self.item_row(it, &th, cx)).collect();
        rows.into_iter().enumerate().map(|(i, r)| self.row(i, r, cx)).collect()
    }

    fn item_row(&self, it: &InboxItem, th: &Theme, cx: &Context<Self>) -> Option<Row> {
        let (open, key, rise, tag, word) = kind_parts(&it.kind, it.id)?;
        let s = self.store.read(cx);
        let task = it.task_id.and_then(|id| s.tasks.iter().find(|t| t.id == id));
        let color = th.client_color(task.map_or(0, |t| t.client_id));
        let face = char_state(word);
        Some(Row { open, id: it.id, key, rise, color, face, title: first_line(&it.title), sub: it.reason.clone(), tag, word })
    }
}

/// Drawer target, element keys, tag text and word for an inbox kind.
fn kind_parts(kind: &str, id: i64) -> Option<(Open, &'static str, &'static str, &'static str, Word)> {
    Some(match kind {
        "question" => (Open::Question(id), "inbox-q", "inbox-q-rise", "Question", Word::NeedsYou),
        "ask" => (Open::Ask(id), "inbox-a", "inbox-a-rise", "Asked in chat", Word::NeedsYou),
        "stuck" => (Open::Stuck(id), "inbox-s", "inbox-s-rise", "Stuck", Word::NeedsYou),
        "review" => (Open::Review(id), "inbox-r", "inbox-r-rise", "Review", Word::Done),
        _ => return None,
    })
}

/// What one list row shows.
struct Row {
    open: Open,
    id: i64,
    key: &'static str,
    rise: &'static str,
    color: Hsla,
    face: CharState,
    title: String,
    sub: String,
    tag: &'static str,
    word: Word,
}

fn answered_row(q: &Question, th: &Theme) -> AnyElement {
    let title = div().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(12.5)).text_color(th.fg_2);
    let answer = format!("You: {}", q.answer.clone().unwrap_or_default());
    let col = div().flex().flex_col().gap(px(2.)).px(px(14.)).py(px(7.));
    col.id(("answered", q.id as usize)).child(title.child(first_line(&q.text))).child(muted_line(answer, 11.5, th)).into_any_element()
}

fn answered_section(answered: &[&Question], th: &Theme) -> AnyElement {
    let head = div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).px(px(14.)).child("Answered");
    let col = div().id("answered-section").flex().flex_col().gap(px(2.)).mt(px(24.));
    col.child(head).children(answered.iter().map(|q| answered_row(q, th))).into_any_element()
}

// ---- drawer ----

impl QuestionsView {
    fn overlay(&mut self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let drawer = match self.open? {
            Open::Question(id) => self.question_drawer(id, th, cx)?,
            Open::Ask(id) => self.ask_drawer(id, th, cx)?,
            Open::Stuck(id) => self.stuck_drawer(id, th, cx)?,
            Open::Review(id) => self.review_drawer(id, th, cx)?,
        };
        let scrim = scrim("inbox-scrim", Self::close, cx);
        Some(slide_overlay("inbox", scrim, drawer, self.closing))
    }

    fn drawer_head(&self, eyebrow: String, title: String, th: &Theme, cx: &mut Context<Self>) -> Div {
        let eyebrow = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(eyebrow.to_uppercase());
        let name = div().text_size(px(17.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(title);
        let left = div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(eyebrow).child(name);
        let close = close_button("inbox-drawer-close", Self::close, th, cx);
        div().flex().items_start().justify_between().gap(px(12.)).px(px(20.)).pt(px(20.)).child(left).child(close)
    }

    fn field(input: Option<Entity<TextInput>>, th: &Theme) -> Div {
        let field = div().px(px(10.)).py(px(4.)).rounded(px(8.)).bg(th.well).border_1().border_color(th.hairline);
        field.children(input)
    }

    fn composer(&self, fields: Vec<Div>, send: AnyElement, hint: Option<String>, th: &Theme) -> AnyElement {
        let hint = hint.map(|h| div().text_size(px(11.)).text_color(th.muted).child(h));
        let row = div().flex().items_center().gap(px(10.)).children(hint.map(|h| h.flex_1())).child(send);
        let foot = div().id("inbox-composer").flex().flex_col().flex_none().gap(px(8.)).px(px(20.)).py(px(14.));
        foot.border_t_1().border_color(th.hairline).children(fields).child(row).into_any_element()
    }

    fn question_drawer(&mut self, id: i64, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(data) = decision::question_card(self.store.read(cx), id) else { return self.question_fallback(id, th, cx) };
        let q = self.store.read(cx).questions.iter().find(|q| q.id == id).cloned();
        let head = self.drawer_head("Inbox".into(), "Needs a decision".into(), th, cx);
        let card = self.cards.get(&data, false, &self.store, Self::on_card, cx);
        let mut body = vec![card.into_any_element()];
        body.extend(q.map(|q| self.thread_section(&Origin::new(&q, self.store.read(cx)), th, cx)));
        let body = scroll_body("inbox-drawer-scroll", body);
        Some(drawer_shell("inbox-drawer", th).child(head).child(body).into_any_element())
    }

    fn on_card(&mut self, ev: &CardEvent, cx: &mut Context<Self>) {
        match ev {
            CardEvent::Open { thread_id, .. } => self.open_thread(Some(*thread_id), cx),
            CardEvent::Review { thread_id } => {
                (self.open, self.closing) = (None, false);
                let id = *thread_id;
                self.store.update(cx, |s, cx| s.open_changes(id, cx));
            }
        }
    }

    fn thread_section(&self, o: &Origin, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(task) = &o.task else { return div().into_any_element() };
        let task_id = task.id;
        let face = character(CharState::Blocked, px(32.), ("inbox-thread-rui", task.id as usize), o.color(th), th);
        let name = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(o.agent.clone());
        let text = div().flex().flex_col().flex_1().min_w_0().child(name).child(muted_line(task.title.clone(), 11.5, th));
        let hover = th.surface_2;
        let row = div().id("inbox-thread").flex().items_center().gap(px(10.)).px(px(12.)).py(px(9.)).rounded(px(10.));
        let row = row.bg(th.well).cursor_pointer().hover(move |s| s.bg(hover));
        let row = row.on_click(cx.listener(move |this, _, _, cx| this.open_thread(Some(task_id), cx))).child(face).child(text);
        let col = div().flex().flex_col().gap(px(8.)).child(heading("The thread waiting on this", th));
        col.child(row).into_any_element()
    }

    fn ask_drawer(&mut self, id: i64, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let a = self.store.read(cx).asks.iter().find(|a| a.id == id && a.status == "open")?.clone();
        let head = self.drawer_head(ASK_SUB.to_string(), first_line(&a.text), th, cx);
        let mut body = vec![markdown::render(&a.text, th.fg, 13., th)];
        body.extend((!a.options.is_empty()).then(|| self.options(&a, th, cx)));
        let body = scroll_body("inbox-drawer-scroll", body);
        let fields = vec![Self::field(self.ask_inputs.get(id), th)];
        let send = primary_button("inbox-send", "Send answer", cx).on_click(cx.listener(move |this, _, _, cx| this.answer_ask(id, None, cx)));
        let foot = self.composer(fields, send.into_any_element(), None, th);
        Some(drawer_shell("inbox-drawer", th).child(head).child(body).child(foot).into_any_element())
    }

    fn options(&self, a: &ChatAsk, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = a.options.iter().enumerate().map(|(i, text)| self.option(i, text, th, cx)).collect();
        div().flex().flex_col().gap(px(8.)).child(heading("Pick one", th)).children(rows).into_any_element()
    }

    fn option(&self, i: usize, text: &str, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let on = self.pick.as_deref() == Some(text);
        let (bg, ring) = if on { (th.accent_fill.opacity(0.14), th.accent_fill) } else { (gpui::transparent_black(), th.hairline) };
        let pick = text.to_string();
        let el = div().id(("inbox-opt", i)).w_full().px(px(12.)).py(px(10.)).rounded(px(8.)).border_1().border_color(ring);
        let el = el.bg(bg).cursor_pointer().text_size(px(12.5)).text_color(th.fg).child(text.to_string());
        el.on_click(cx.listener(move |this, _, _, cx| {
            this.pick = Some(pick.clone());
            cx.notify();
        }))
        .into_any_element()
    }
}

// ---- stuck, review and fallback drawers ----

impl QuestionsView {
    /// The shared input for a drawer, created on first use; Enter sends it.
    fn note_input(&mut self, key: InputKey, hint: &'static str, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = self.notes.get(&key) {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::new(hint, cx));
        cx.subscribe(&input, move |this, _, _: &Submit, cx| this.send_note(key, cx)).detach();
        self.notes.insert(key, input.clone());
        input
    }

    /// Send what was typed in a drawer's input, then close the drawer.
    fn send_note(&mut self, (kind, id): InputKey, cx: &mut Context<Self>) {
        let Some(input) = self.notes.get(&(kind, id)).cloned() else { return };
        let text = input.read(cx).content();
        if text.trim().is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.clear(cx));
        let task_id = self.task_of(id, kind, cx);
        self.store.update(cx, |s, cx| match (kind, task_id) {
            ("stuck", Some(t)) => s.instruct_task(t, text, cx),
            ("review", Some(t)) => s.review_task(t, false, Some(text), cx),
            _ => s.answer(id, text, None, cx),
        });
        self.close(cx);
    }

    fn item(&self, kind: &str, id: i64, cx: &Context<Self>) -> Option<InboxItem> {
        self.store.read(cx).inbox.iter().find(|i| i.kind == kind && i.id == id).cloned()
    }

    fn task_of(&self, id: i64, kind: &str, cx: &Context<Self>) -> Option<i64> {
        self.item(kind, id, cx).and_then(|i| i.task_id)
    }

    fn open_link(&self, task_id: Option<i64>, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let link = div().id("inbox-open-task").text_size(px(12.5)).text_color(th.run.fg).cursor_pointer();
        let link = link.child("Open the task");
        Some(link.on_click(cx.listener(move |this, _, _, cx| this.open_thread(task_id, cx))).into_any_element())
            .filter(|_| task_id.is_some())
    }

    fn question_fallback(&mut self, id: i64, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let it = self.item("question", id, cx)?;
        let head = self.drawer_head("Question".into(), first_line(&it.title), th, cx);
        let reason = markdown::render(&it.reason, th.fg, 13., th);
        let body = scroll_body("inbox-drawer-scroll", vec![reason]);
        let input = self.note_input(("question", id), "Your answer...", cx);
        let send = primary_button("inbox-send", "Send answer", cx).on_click(cx.listener(move |this, _, _, cx| this.send_note(("question", id), cx)));
        let foot = self.composer(vec![Self::field(Some(input), th)], send.into_any_element(), None, th);
        Some(drawer_shell("inbox-drawer", th).child(head).child(body).child(foot).into_any_element())
    }

    fn stuck_drawer(&mut self, id: i64, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let it = self.item("stuck", id, cx)?;
        let task = it.task_id;
        let head = self.drawer_head("Stuck".into(), first_line(&it.title), th, cx);
        let mut body = vec![markdown::render(&it.reason, th.fg, 13., th)];
        body.extend(self.open_link(task, th, cx));
        body.push(self.stuck_buttons(task, cx));
        let body = scroll_body("inbox-drawer-scroll", body);
        let input = self.note_input(("stuck", id), "New instructions...", cx);
        let send = primary_button("inbox-send", "Send instructions", cx).on_click(cx.listener(move |this, _, _, cx| this.send_note(("stuck", id), cx)));
        let foot = self.composer(vec![Self::field(Some(input), th)], send.into_any_element(), None, th);
        Some(drawer_shell("inbox-drawer", th).child(head).child(body).child(foot).into_any_element())
    }

    fn stuck_buttons(&mut self, task: Option<i64>, cx: &mut Context<Self>) -> AnyElement {
        let drop_label = if self.confirm_drop { "Click again to drop" } else { "Drop it" };
        let retry = primary_button("inbox-retry", "Retry", cx).on_click(cx.listener(move |this, _, _, cx| {
            let Some(t) = task else { return };
            this.store.update(cx, |s, cx| s.retry_task(t, cx));
            this.close(cx);
        }));
        let drop = chip("inbox-drop", drop_label, cx).on_click(cx.listener(move |this, _, _, cx| this.drop_click(task, cx)));
        div().flex().gap(px(8.)).child(retry).child(drop).into_any_element()
    }

    /// First click arms the drop; the second one drops the task.
    fn drop_click(&mut self, task: Option<i64>, cx: &mut Context<Self>) {
        let Some(t) = task else { return };
        if !self.confirm_drop {
            self.confirm_drop = true;
            cx.notify();
            return;
        }
        self.store.update(cx, |s, cx| s.drop_task(t, cx));
        self.close(cx);
    }

    fn review_drawer(&mut self, id: i64, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let it = self.item("review", id, cx)?;
        let task = it.task_id;
        let head = self.drawer_head("Review".into(), first_line(&it.title), th, cx);
        let mut body = vec![markdown::render(&it.reason, th.fg, 13., th)];
        body.extend(self.open_link(task, th, cx));
        body.push(self.review_buttons(task, cx));
        let body = scroll_body("inbox-drawer-scroll", body);
        let drawer = drawer_shell("inbox-drawer", th).child(head).child(body);
        if !self.reveal {
            return Some(drawer.into_any_element());
        }
        let input = self.note_input(("review", id), "What should change?", cx);
        let send = primary_button("inbox-send", "Send changes", cx).on_click(cx.listener(move |this, _, _, cx| this.send_note(("review", id), cx)));
        let foot = self.composer(vec![Self::field(Some(input), th)], send.into_any_element(), None, th);
        Some(drawer.child(foot).into_any_element())
    }

    fn review_buttons(&mut self, task: Option<i64>, cx: &mut Context<Self>) -> AnyElement {
        let good = primary_button("inbox-good", "Looks good", cx).on_click(cx.listener(move |this, _, _, cx| {
            let Some(t) = task else { return };
            this.store.update(cx, |s, cx| s.review_task(t, true, None, cx));
            this.close(cx);
        }));
        let changes = chip("inbox-changes", "Ask for changes", cx).on_click(cx.listener(|this, _, _, cx| {
            this.reveal = true;
            cx.notify();
        }));
        div().flex().gap(px(8.)).child(good).child(changes).into_any_element()
    }
}

impl Render for QuestionsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.want_focus {
            self.want_focus = false;
            window.focus(&self.focus);
        }
        let (questions, asks, inbox) = {
            let s = self.store.read(cx);
            (s.questions.clone(), s.asks.clone(), s.inbox.clone())
        };
        let th = theme(cx).clone();
        let open_asks: Vec<&ChatAsk> = asks.iter().filter(|a| a.status == "open").collect();
        self.ask_inputs.sync(&open_asks, Self::answer_ask, cx);
        let answered: Vec<_> = questions.iter().filter(|q| q.status == QuestionStatus::Answered).rev().take(10).collect();
        let rows = self.rows(&inbox, cx);
        let body = div().id("questions-scroll").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        let body = body.pr(px(40.)).pb(px(28.)).children(rows);
        let body = body.children((!answered.is_empty()).then(|| answered_section(&answered, &th)));
        let page = div().flex().flex_col().size_full().child(page_head("NEEDS A DECISION", "Inbox", cx));
        let page = page.child(lead_line(inbox.len(), &th)).child(body);
        let overlay = self.overlay(&th, cx);
        let root = div().id("inbox-page").relative().size_full().track_focus(&self.focus);
        root.on_key_down(cx.listener(|this, ev, _, cx| this.key_down(ev, cx))).child(page).children(overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_tail_is_singular_for_one() {
        assert!(lead_tail(1).starts_with(" item needs"));
        assert!(lead_tail(3).starts_with(" items need"));
    }
}
