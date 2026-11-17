//! Inbox: a compact list of what needs the owner; a row opens a drawer to read it and answer.

use super::asks::AskInputs;
use super::character::{CharState, character};
use super::library::{after_slide, close_button, drawer_shell, scroll_body, scrim, slide_overlay};
use super::motion::rise;
use super::{markdown, page_head, primary_button, tag};
use crate::store::{self, Store, inbox_count};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Theme, theme};
use gpui::{
    AnyElement, Context, Div, Entity, FocusHandle, FontWeight, Hsla, KeyDownEvent, Stateful, Window, div,
    prelude::*, px,
};
use rui_core::{ChatAsk, Question, QuestionStatus, Rung, Task};
use std::collections::HashMap;

const WHY_HINT: &str = "Why \u{2014} this becomes a lesson in the Playbook, so the team doesn\u{2019}t ask again";
const ASK_SUB: &str = "Rui \u{b7} asked in chat";
const NONE_TEXT: &str = "Nothing is waiting on you. Every thread settled below you.";

/// What the drawer shows.
#[derive(Clone, Copy, PartialEq)]
enum Open {
    Question(i64),
    Ask(i64),
}

/// Open questions and asks as rows; the open one slides in as a drawer.
pub struct QuestionsView {
    store: Entity<Store>,
    inputs: HashMap<i64, Entity<TextInput>>,
    whys: HashMap<i64, Entity<TextInput>>,
    ask_inputs: AskInputs,
    open: Option<Open>,
    /// Drawer is playing its slide-out before it closes.
    closing: bool,
    /// The option picked in an ask's drawer.
    pick: Option<String>,
    focus: FocusHandle,
    want_focus: bool,
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

/// Text after the bold count in the lead line.
fn lead_tail(n: usize) -> &'static str {
    match n {
        1 => " thread stopped rather than guess. Everything else settled below you.",
        _ => " threads stopped rather than guess. Everything else settled below you.",
    }
}

/// First non-empty line of a question, without markdown marks.
fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().filter(|c| !"#*_".contains(*c)).collect::<String>().trim().to_string()
}

/// "Client · Agent stopped and passed it up".
fn sub_line(client: Option<&str>, agent: &str) -> String {
    format!("{} \u{b7} {agent} stopped and passed it up", client.unwrap_or("General"))
}

fn lead_line(n: usize, th: &Theme) -> Stateful<Div> {
    let base = div().id("inbox-lead").flex().text_size(px(14.)).text_color(th.fg_2).pb(px(16.));
    if n == 0 {
        return base.child(NONE_TEXT);
    }
    let bold = div().font_weight(FontWeight::BOLD).text_color(th.fg).child(n.to_string());
    base.child(bold).child(lead_tail(n))
}

/// One rung: bold who, then the reason.
fn rung_chip(who: String, why: Option<String>, th: &Theme) -> Div {
    let who = div().font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(who);
    let chip = div().flex().gap(px(5.)).px(px(9.)).py(px(4.)).rounded(px(7.)).max_w(px(560.));
    let chip = chip.bg(th.fg.opacity(0.08)).text_size(px(11.)).text_color(th.muted).child(who);
    chip.children(why.map(|w| format!("\u{b7} {w}")))
}

fn ladder_row(q: &Question, th: &Theme) -> Div {
    let parts = ladder_parts(&q.ladder);
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

/// Where a question came from: its task, client name and the agent that asked.
struct Origin {
    task: Option<Task>,
    client: Option<String>,
    agent: String,
}

impl Origin {
    fn new(q: &Question, s: &Store) -> Self {
        let task = s.tasks.iter().find(|t| t.id == q.task_id).cloned();
        let client = task.as_ref().and_then(|t| s.clients.iter().find(|c| c.id == t.client_id));
        let client = client.map(|c| c.name.clone());
        let agent = task.as_ref().and_then(|t| t.agent.clone()).unwrap_or_else(|| "Pod Lead".to_string());
        Self { task, client, agent }
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
        let (inputs, whys) = (HashMap::new(), HashMap::new());
        let focus = cx.focus_handle();
        Self {
            store, inputs, whys, ask_inputs: AskInputs::default(), open: None, closing: false, pick: None, focus, want_focus: false,
        }
    }

    // ---- opening and closing ----

    fn open_item(&mut self, item: Open, cx: &mut Context<Self>) {
        (self.open, self.closing, self.pick, self.want_focus) = (Some(item), false, None, true);
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
        cx.notify();
    }

    fn key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        if ev.keystroke.key == "escape" {
            self.close(cx);
        }
    }

    fn open_thread(&mut self, cx: &mut Context<Self>) {
        (self.open, self.closing) = (None, false);
        self.store.update(cx, |s, cx| s.go_to_threads(cx));
    }

    // ---- answering ----

    fn sync_inputs(&mut self, open: &[&Question], cx: &mut Context<Self>) {
        self.inputs.retain(|id, _| open.iter().any(|q| q.id == *id));
        self.whys.retain(|id, _| open.iter().any(|q| q.id == *id));
        for q in open {
            if !self.inputs.contains_key(&q.id) {
                let input = self.new_input("Your answer...", q.id, cx);
                self.inputs.insert(q.id, input);
            }
            if !self.whys.contains_key(&q.id) {
                let why = self.new_input(WHY_HINT, q.id, cx);
                self.whys.insert(q.id, why);
            }
        }
    }

    fn new_input(&self, hint: &'static str, id: i64, cx: &mut Context<Self>) -> Entity<TextInput> {
        let input = cx.new(|cx| TextInput::new(hint, cx));
        cx.subscribe(&input, move |this, _, _: &Submit, cx| this.answer(id, cx)).detach();
        input
    }

    fn answer(&mut self, id: i64, cx: &mut Context<Self>) {
        let (Some(input), Some(why)) = (self.inputs.get(&id).cloned(), self.whys.get(&id).cloned()) else { return };
        let (text, reason) = (input.read(cx).content(), why.read(cx).content());
        if text.trim().is_empty() {
            return;
        }
        if reason.trim().is_empty() {
            self.store.update(cx, |s, cx| s.show_toast("Add your reason first", vec![], cx));
            return;
        }
        input.update(cx, |i, cx| i.clear(cx));
        why.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| s.answer(id, text, Some(reason), cx));
        self.close(cx);
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
        let face = character(CharState::Blocked, px(36.), (r.key, n), r.color, &th);
        let row = div().id((r.key, n)).flex().items_center().gap(px(12.)).px(px(14.)).py(px(11.)).mb(px(6.));
        let row = row.rounded(px(10.)).bg(th.surface).cursor_pointer().hover(move |s| s.bg(hover));
        let row = row.on_click(cx.listener(move |this, _, _, cx| this.open_item(item, cx)));
        let row = row.child(face).child(text).child(tag(&th.block, "Needs you"));
        rise(row, (r.rise, n), i).into_any_element()
    }

    fn rows(&self, questions: &[&Question], asks: &[&ChatAsk], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let th = theme(cx).clone();
        let mut out: Vec<Row> = questions.iter().map(|q| self.question_row(q, &th, cx)).collect();
        out.extend(asks.iter().map(|a| ask_row(a, &th)));
        out.into_iter().enumerate().map(|(i, r)| self.row(i, r, cx)).collect()
    }

    fn question_row(&self, q: &Question, th: &Theme, cx: &Context<Self>) -> Row {
        let o = Origin::new(q, self.store.read(cx));
        let sub = sub_line(o.client.as_deref(), &o.agent);
        Row { open: Open::Question(q.id), id: q.id, key: "inbox-q", rise: "inbox-q-rise", color: o.color(th), title: first_line(&q.text), sub }
    }
}

/// What one list row shows.
struct Row {
    open: Open,
    id: i64,
    key: &'static str,
    rise: &'static str,
    color: Hsla,
    title: String,
    sub: String,
}

fn ask_row(a: &ChatAsk, th: &Theme) -> Row {
    let (open, sub) = (Open::Ask(a.id), ASK_SUB.to_string());
    Row { open, id: a.id, key: "inbox-a", rise: "inbox-a-rise", color: th.client_color(0), title: first_line(&a.text), sub }
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
        let q = self.store.read(cx).questions.iter().find(|q| q.id == id && q.status == QuestionStatus::Open)?.clone();
        let o = Origin::new(&q, self.store.read(cx));
        let eyebrow = format!("{} \u{b7} needs a decision", o.client.clone().unwrap_or_else(|| "General".into()));
        let head = self.drawer_head(eyebrow, first_line(&q.text), th, cx);
        let text = markdown::render(&q.text, th.fg, 13., th);
        let body = vec![ladder_row(&q, th).into_any_element(), text, self.thread_section(&o, th, cx)];
        let body = scroll_body("inbox-drawer-scroll", body);
        let fields = vec![Self::field(self.inputs.get(&id).cloned(), th), Self::field(self.whys.get(&id).cloned(), th)];
        let send = primary_button("inbox-send", "Send answer", cx).on_click(cx.listener(move |this, _, _, cx| this.answer(id, cx)));
        let hint = format!("Goes back down to {}.", if o.task.is_some() { o.agent.as_str() } else { "the Pod" });
        let foot = self.composer(fields, send.into_any_element(), Some(hint), th);
        Some(drawer_shell("inbox-drawer", th).child(head).child(body).child(foot).into_any_element())
    }

    fn thread_section(&self, o: &Origin, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(task) = &o.task else { return div().into_any_element() };
        let face = character(CharState::Blocked, px(32.), ("inbox-thread-rui", task.id as usize), o.color(th), th);
        let name = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(o.agent.clone());
        let text = div().flex().flex_col().flex_1().min_w_0().child(name).child(muted_line(task.title.clone(), 11.5, th));
        let hover = th.surface_2;
        let row = div().id("inbox-thread").flex().items_center().gap(px(10.)).px(px(12.)).py(px(9.)).rounded(px(10.));
        let row = row.bg(th.well).cursor_pointer().hover(move |s| s.bg(hover));
        let row = row.on_click(cx.listener(|this, _, _, cx| this.open_thread(cx))).child(face).child(text);
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

impl Render for QuestionsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.want_focus {
            self.want_focus = false;
            window.focus(&self.focus);
        }
        let (questions, asks) = {
            let s = self.store.read(cx);
            (s.questions.clone(), s.asks.clone())
        };
        let th = theme(cx).clone();
        let open_asks: Vec<&ChatAsk> = asks.iter().filter(|a| a.status == "open").collect();
        self.ask_inputs.sync(&open_asks, Self::answer_ask, cx);
        let open: Vec<_> = questions.iter().filter(|q| q.status == QuestionStatus::Open).collect();
        self.sync_inputs(&open, cx);
        let answered: Vec<_> = questions.iter().filter(|q| q.status == QuestionStatus::Answered).rev().take(10).collect();
        let rows = self.rows(&open, &open_asks, cx);
        let empty = rows.is_empty().then(|| muted_line("Nothing needs a decision right now.".into(), 13., &th));
        let body = div().id("questions-scroll").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        let body = body.pr(px(40.)).pb(px(28.)).children(rows).children(empty);
        let body = body.children((!answered.is_empty()).then(|| answered_section(&answered, &th)));
        let page = div().flex().flex_col().size_full().child(page_head("NEEDS A DECISION", "Inbox", cx));
        let page = page.child(lead_line(inbox_count(&questions, &asks), &th)).child(body);
        let overlay = self.overlay(&th, cx);
        let root = div().id("inbox-page").relative().size_full().track_focus(&self.focus);
        root.on_key_down(cx.listener(|this, ev, _, cx| this.key_down(ev, cx))).child(page).children(overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rung(who: &str, why: &str) -> Rung {
        Rung { who: who.into(), why: why.into() }
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
    fn lead_tail_is_singular_for_one() {
        assert!(lead_tail(1).starts_with(" thread stopped"));
        assert!(lead_tail(3).starts_with(" threads stopped"));
    }

    #[test]
    fn first_line_strips_markdown() {
        assert_eq!(first_line("\n## **Use** _Postgres_?\nmore"), "Use Postgres?");
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn sub_line_names_client_and_agent() {
        assert_eq!(sub_line(Some("Fide"), "Backend 7"), "Fide \u{b7} Backend 7 stopped and passed it up");
        assert!(sub_line(None, "Pod Lead").starts_with("General \u{b7} Pod Lead"));
    }
}
