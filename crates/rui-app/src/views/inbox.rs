//! The Inbox page: Suite-style cards for what needs the owner. Items can be snoozed.

use super::activity::{icon, local_offset};
use super::decision::{self, CardEvent, Cards};
use super::asks::{self, AskInputs};
use super::inbox_kinds::{self as kinds, Kind};
use super::{chip, page_head, primary_button};
use crate::api::InboxItem;
use crate::text_input::{Submit, TextInput};
use crate::store::{self, ItemKey, Store, now_secs};
use crate::theme::{Status, Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, FontWeight, ScrollHandle, Stateful, Window, div, prelude::*, px};
use decision::{Act, CardData};
use rui_core::ChatAsk;
use std::collections::HashMap;

const PAGE_W: f32 = 760.;
const EMPTY_TEXT: &str = "Nothing is waiting on you. Every thread settled below you.";
const NONE_SNOOZED: &str = "Nothing is snoozed or dismissed.";

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    All,
    Decisions,
    Snoozed,
}

/// How long to snooze an item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Snooze {
    Hour,
    /// Until 9:00 tomorrow, local time.
    Tomorrow,
    /// Hidden until the owner brings it back.
    Dismiss,
}

/// The snooze end a dismissed item gets.
pub const DISMISSED: i64 = i64::MAX;

/// Unix time a snooze ends, with `offset` the local UTC offset in seconds.
pub fn snooze_until(s: Snooze, now: i64, offset: i64) -> i64 {
    match s {
        Snooze::Hour => now + 3600,
        Snooze::Tomorrow => ((now + offset).div_euclid(86_400) + 1) * 86_400 + 9 * 3600 - offset,
        Snooze::Dismiss => DISMISSED,
    }
}

/// An item is snoozed while its snooze has not ended.
pub fn is_snoozed(snoozed: &HashMap<ItemKey, i64>, key: &ItemKey, now: i64) -> bool {
    snoozed.get(key).is_some_and(|until| *until > now)
}

pub fn item_key(it: &InboxItem) -> ItemKey {
    (it.kind.clone(), it.id)
}

/// Items that are not snoozed: what the rail badge counts.
pub fn awake_count(items: &[InboxItem], snoozed: &HashMap<ItemKey, i64>, now: i64) -> usize {
    items.iter().filter(|i| !is_snoozed(snoozed, &item_key(i), now)).count()
}

/// "Until 14:05", or "Until tomorrow 09:00" when it ends on a later day.
pub fn until_text(until: i64, now: i64, offset: i64) -> String {
    if until == DISMISSED {
        return "Dismissed".into();
    }
    let day = |t: i64| (t + offset).div_euclid(86_400);
    let secs = (until + offset).rem_euclid(86_400);
    let clock = format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60);
    if day(until) > day(now) { format!("Until tomorrow {clock}") } else { format!("Until {clock}") }
}

/// Open `cur`'s row, or close it when `key` is already the open one.
pub fn toggle_key(cur: Option<ItemKey>, key: ItemKey) -> Option<ItemKey> {
    if cur.as_ref() == Some(&key) { None } else { Some(key) }
}

/// Whether an item shows under a tab: All and Decisions hold awake items, Snoozed the rest.
fn in_tab(tab: Tab, kind: &str, snoozed: bool) -> bool {
    match tab {
        Tab::All => !snoozed,
        Tab::Decisions => !snoozed && kind == "question",
        Tab::Snoozed => snoozed,
    }
}

/// How many items a tab holds.
fn tab_count(tab: Tab, items: &[InboxItem], snoozed: &HashMap<ItemKey, i64>, now: i64) -> usize {
    items.iter().filter(|i| in_tab(tab, &i.kind, is_snoozed(snoozed, &item_key(i), now))).count()
}

/// Kinds whose row opens a detail in place.
fn expandable(kind: &str) -> bool {
    matches!(kind, "question" | "ask" | "stuck" | "review")
}

/// Key of a row's text input: (kind, id).
type InputKey = (&'static str, i64);

/// A task action run from a row's button.
#[derive(Clone, Copy)]
enum TaskAct {
    Retry,
    Approve,
}

/// The Inbox page with its tabs, cards and snooze menus.
pub struct InboxPanel {
    store: Entity<Store>,
    tab: Tab,
    /// Item whose snooze menu is open.
    menu: Option<ItemKey>,
    /// Item whose details are open in its row.
    expanded: Option<ItemKey>,
    cards: Cards,
    ask_inputs: AskInputs,
    /// Inputs of the stuck, review and fallback-question rows.
    notes: HashMap<InputKey, Entity<TextInput>>,
    /// "Ask for changes" input is showing in a review row.
    reveal: bool,
    scroll: ScrollHandle,
    /// Row to scroll into view on the next render.
    scroll_to: Option<ItemKey>,
}

impl InboxPanel {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self {
            store, tab: Tab::All, menu: None, expanded: None, cards: Cards::default(), ask_inputs: AskInputs::default(),
            notes: HashMap::new(), reveal: false, scroll: ScrollHandle::new(), scroll_to: None,
        }
    }

    /// Show the All tab with one item's card open, scrolled into view.
    pub fn open_item(&mut self, kind: &str, id: i64, cx: &mut Context<Self>) {
        (self.menu, self.tab, self.expanded) = (None, Tab::All, None);
        let key = (kind.to_string(), id);
        self.scroll_to = Some(key.clone());
        self.expand(key, cx);
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        (self.tab, self.menu) = (tab, None);
        cx.notify();
    }

    fn toggle_menu(&mut self, key: ItemKey, cx: &mut Context<Self>) {
        self.menu = if self.menu.as_ref() == Some(&key) { None } else { Some(key) };
        cx.notify();
    }

    fn snooze(&mut self, key: ItemKey, s: Snooze, cx: &mut Context<Self>) {
        self.menu = None;
        let until = snooze_until(s, now_secs(), local_offset());
        self.store.update(cx, |st, cx| st.snooze(key, until, cx));
    }

    fn expand(&mut self, key: ItemKey, cx: &mut Context<Self>) {
        self.reveal = false;
        if key.0 == "question" {
            let top = decision::question_thread(self.store.read(cx), key.1);
            self.store.update(cx, |s, cx| top.into_iter().for_each(|id| s.load_thread(id, cx)));
        }
        self.expanded = toggle_key(self.expanded.take(), key);
        cx.notify();
    }

    fn on_card(&mut self, ev: &CardEvent, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| match ev {
            CardEvent::Open { thread_id, .. } => s.open_thread(*thread_id, cx),
            CardEvent::Review { thread_id } => s.open_changes(*thread_id, cx),
        });
    }

    /// Open a task's thread.
    fn open_task(&mut self, task_id: i64, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.open_thread(task_id, cx));
    }

    /// Answer an ask with a picked option, or with what was typed when `text` is `None`.
    fn answer_ask(&mut self, id: i64, text: Option<String>, cx: &mut Context<Self>) {
        let Some(text) = text.or_else(|| self.ask_inputs.take(id, cx)) else { return };
        self.store.update(cx, |s, cx| s.answer_ask(id, text, cx));
        self.expanded = None;
        cx.notify();
    }
}

// ---- row details: ask, stuck, review and fallback-question inputs ----

impl InboxPanel {
    /// The shared input for a row, created on first use; Enter sends it.
    fn note_input(&mut self, key: InputKey, hint: &'static str, task: Option<i64>, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = self.notes.get(&key) {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::new(hint, cx));
        cx.subscribe(&input, move |this, _, _: &Submit, cx| this.send_note(key, task, cx)).detach();
        self.notes.insert(key, input.clone());
        input
    }

    /// Send what was typed in a row's input, then fold the row.
    fn send_note(&mut self, (kind, id): InputKey, task: Option<i64>, cx: &mut Context<Self>) {
        let Some(input) = self.notes.get(&(kind, id)).cloned() else { return };
        let text = input.read(cx).content();
        if text.trim().is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| match (kind, task) {
            ("stuck", Some(t)) => s.instruct_task(t, text, cx),
            ("review", Some(t)) => s.review_task(t, false, Some(text), cx),
            _ => s.answer(id, text, None, cx),
        });
        self.expanded = None;
        cx.notify();
    }

    /// An input with its Send button.
    fn note_row(&mut self, key: InputKey, hint: &'static str, task: Option<i64>, th: &Theme, cx: &mut Context<Self>) -> Div {
        let input = self.note_input(key, hint, task, cx);
        let field = div().flex_1().min_w_0().px(px(10.)).py(px(4.)).rounded(px(8.)).bg(th.well).border_1().border_color(th.hairline);
        let send = chip(format!("inbox-p-send-{}-{}", key.0, key.1), "Send", cx);
        let send = send.on_click(cx.listener(move |this, _, _, cx| this.send_note(key, task, cx)));
        div().flex().items_center().gap(px(8.)).child(field.child(input)).child(send)
    }

    /// Run a task action from a row's button, then fold the row.
    fn task_act(&mut self, task: Option<i64>, act: TaskAct, cx: &mut Context<Self>) {
        let Some(t) = task else { return };
        self.store.update(cx, |s, cx| match act {
            TaskAct::Retry => s.retry_task(t, cx),
            TaskAct::Approve => s.review_task(t, true, None, cx),
        });
        self.expanded = None;
        cx.notify();
    }

    /// Open a row's detail with the "what should change" input showing.
    fn ask_changes(&mut self, key: ItemKey, cx: &mut Context<Self>) {
        self.expanded = None;
        self.expand(key, cx);
        self.reveal = true;
    }

    fn ask_detail(&mut self, id: i64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let a = self.store.read(cx).asks.iter().find(|a| a.id == id && a.status == "open")?.clone();
        Some(asks::ask_card(&a, &self.ask_inputs, Self::answer_ask, cx))
    }

    fn question_detail(&mut self, it: &InboxItem, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(data) = decision::question_card(self.store.read(cx), it.id) else {
            return Some(self.note_row(("question", it.id), "Your answer...", None, th, cx).into_any_element());
        };
        Some(self.cards.get(&data, false, &self.store, Self::on_card, cx).into_any_element())
    }

    /// What an open row shows below its buttons.
    fn detail(&mut self, it: &InboxItem, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        match kinds::kind_of(&it.kind) {
            Kind::Question => self.question_detail(it, th, cx),
            Kind::Ask => self.ask_detail(it.id, cx),
            Kind::Stuck => Some(self.note_row(("stuck", it.id), "New instructions...", it.task_id, th, cx).into_any_element()),
            Kind::Review if self.reveal => Some(self.note_row(("review", it.id), "What should change?", it.task_id, th, cx).into_any_element()),
            _ => None,
        }
    }
}

// ---- rows ----

/// A small round icon button.
fn round_button(id: impl Into<gpui::SharedString>, name: &str, th: &Theme) -> Stateful<Div> {
    let hover = th.surface_2;
    let btn = div().id(id.into()).flex().flex_none().items_center().justify_center().size(px(30.)).rounded(px(15.));
    btn.cursor_pointer().hover(move |s| s.bg(hover)).child(icon(name, 15., th.fg_2))
}

/// The kind chip: a dot and a word on the status colour.
fn kind_chip(word: &str, st: &Status) -> Div {
    let dot = div().flex_none().size(px(6.)).rounded(px(3.)).bg(st.base);
    let chip = div().flex().flex_none().items_center().gap(px(6.)).h(px(22.)).px(px(9.)).rounded(px(11.)).bg(st.dim);
    chip.text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(st.fg).child(dot).child(word.to_string())
}

impl InboxPanel {
    /// The thread an item belongs to: a question's thread, else its task's thread.
    fn thread_of(&self, it: &InboxItem, cx: &Context<Self>) -> Option<i64> {
        let s = self.store.read(cx);
        if kinds::kind_of(&it.kind) == Kind::Question {
            return decision::question_thread(s, it.id);
        }
        let task = s.tasks.iter().find(|t| Some(t.id) == it.task_id)?;
        Some(task.parent_id.unwrap_or(task.id))
    }

    fn thread_name(&self, thread: Option<i64>, cx: &Context<Self>) -> String {
        let s = self.store.read(cx);
        thread.and_then(|id| s.tasks.iter().find(|t| t.id == id)).map(|t| decision::first_line(&t.title)).unwrap_or_default()
    }

    /// Chip, thread name and the moon button.
    fn row_head(&self, i: usize, it: &InboxItem, snoozable: bool, th: &Theme, cx: &mut Context<Self>) -> Div {
        let (word, st) = kinds::sheet_tag(kinds::kind_of(&it.kind), th);
        let name = self.thread_name(self.thread_of(it, cx), cx);
        let name = div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(11.5)).text_color(th.muted).child(name);
        let key = item_key(it);
        let moon = round_button(format!("inbox-p-snooze-{i}"), "moon", th);
        let moon = moon.on_click(cx.listener({
            let key = key.clone();
            move |this, _, _, cx| this.toggle_menu(key.clone(), cx)
        }));
        let close = round_button(format!("inbox-p-dismiss-{i}"), "close", th);
        let close = close.on_click(cx.listener(move |this, _, _, cx| this.snooze(key.clone(), Snooze::Dismiss, cx)));
        let row = div().flex().items_center().gap(px(8.)).child(kind_chip(word, st)).child(name);
        row.children(snoozable.then_some(moon)).children(snoozable.then_some(close))
    }

    /// The one short line under the title: a risk chip, the failure in red, or the reason.
    fn line(&self, it: &InboxItem, th: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let k = kinds::kind_of(&it.kind);
        let decision = (k == Kind::Question).then(|| decision::question_card(self.store.read(cx), it.id)).flatten();
        let risk = decision.and_then(|c| kinds::risk_line(&c.decision.risk, c.decision.reversible).map(|t| (c.decision.risk, t)));
        if let Some((level, text)) = risk {
            let st = match level.as_str() {
                "high" => &th.block,
                "medium" => &th.wait,
                _ => &th.done,
            };
            let chip = div().flex().h(px(22.)).px(px(9.)).items_center().rounded(px(11.)).bg(st.dim).text_size(px(11.5)).text_color(st.fg);
            return Some(div().flex().child(chip.child(text)).into_any_element());
        }
        let color = if k == Kind::Stuck { th.block.fg } else { th.muted };
        kinds::reason(it).map(|r| div().text_size(px(12.)).text_color(color).child(r).into_any_element())
    }

    fn row(&mut self, i: usize, it: &InboxItem, until: Option<i64>, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let key = item_key(it);
        let head = self.row_head(i, it, until.is_none(), th, cx);
        let title = div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).text_color(th.fg).child(decision::first_line(&it.title));
        let bottom = match until {
            Some(u) => self.snoozed_buttons(i, it, u, th, cx),
            None => self.buttons(i, it, th, cx),
        };
        let menu = (until.is_none() && self.menu.as_ref() == Some(&key)).then(|| self.snooze_menu(i, it, cx));
        let detail = if until.is_none() && self.expanded.as_ref() == Some(&key) { self.detail(it, th, cx) } else { None };
        let row = div().id(("inbox-panel-row", i)).flex().flex_col().min_w_0().gap(px(8.)).p(px(14.)).rounded(px(14.)).bg(th.surface);
        let row = row.border_1().border_color(th.hairline).child(head).child(title).children(self.line(it, th, cx));
        row.children(kinds::body(it, th)).child(bottom).children(menu).children(detail).into_any_element()
    }

    /// Action buttons on the left, the round open-or-expand arrow on the right.
    fn buttons(&mut self, i: usize, it: &InboxItem, th: &Theme, cx: &mut Context<Self>) -> Div {
        let main = self.main_buttons(i, it, cx);
        let thread = self.thread_of(it, cx);
        let key = item_key(it);
        let arrow = (thread.is_some() || expandable(&it.kind)).then(|| {
            let btn = round_button(format!("inbox-p-open-{i}"), "arrow", th);
            btn.on_click(cx.listener(move |this, _, _, cx| match thread {
                Some(t) => this.open_task(t, cx),
                None => this.expand(key.clone(), cx),
            }))
        });
        let left = div().flex().flex_1().flex_wrap().items_center().gap(px(8.)).children(main);
        div().flex().items_center().gap(px(8.)).child(left).children(arrow)
    }

    /// A kind's own buttons, main one first.
    fn main_buttons(&mut self, i: usize, it: &InboxItem, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (key, task) = (item_key(it), it.task_id);
        let one = |n: usize, label: &str, cx: &mut Context<Self>| {
            let el_id = format!("inbox-p-act-{i}-{n}");
            if n == 0 { primary_button(el_id, label, cx) } else { chip(el_id, label, cx) }
        };
        match kinds::kind_of(&it.kind) {
            Kind::Question => self.decision_buttons(i, it, cx),
            Kind::Stuck => {
                let retry = one(0, "Retry", cx).on_click(cx.listener(move |this, _, _, cx| this.task_act(task, TaskAct::Retry, cx)));
                let over = one(1, "Take over", cx).on_click(cx.listener(move |this, _, _, cx| match task {
                    Some(t) => this.open_task(t, cx),
                    None => this.expand(key.clone(), cx),
                }));
                vec![retry.into_any_element(), over.into_any_element()]
            }
            Kind::Review => {
                let good = one(0, "Looks good", cx).on_click(cx.listener(move |this, _, _, cx| this.task_act(task, TaskAct::Approve, cx)));
                let changes = one(1, "Ask for changes", cx).on_click(cx.listener(move |this, _, _, cx| this.ask_changes(key.clone(), cx)));
                vec![good.into_any_element(), changes.into_any_element()]
            }
            Kind::Ask => {
                let reply = one(1, "Reply", cx).on_click(cx.listener(move |this, _, _, cx| this.expand(key.clone(), cx)));
                vec![reply.into_any_element()]
            }
            _ => kinds::act_buttons("inbox-p", i, it, &self.store, cx).into_iter().map(IntoElement::into_any_element).collect(),
        }
    }

    /// A decision's picks, Approve and "Ask for changes"; a plain "Answer" when its data is not loaded.
    fn decision_buttons(&mut self, i: usize, it: &InboxItem, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let key = item_key(it);
        let data = decision::question_card(self.store.read(cx), it.id);
        let acts = data.as_ref().map(|c| sheet_acts(&c.decision)).unwrap_or_default();
        let acts = if acts.is_empty() { vec![Act::Answer] } else { acts };
        let mut out = Vec::new();
        for (n, act) in acts.into_iter().enumerate() {
            let label = if act == Act::Answer { "Answer".to_string() } else { act.label() };
            let el_id = format!("inbox-p-act-{i}-{n}");
            let el = if n == 0 { primary_button(el_id, &label, cx) } else { chip(el_id, &label, cx) };
            let (data, key) = (data.clone(), key.clone());
            out.push(el.on_click(cx.listener(move |this, _, _, cx| this.run_act(data.clone(), act.clone(), key.clone(), cx))).into_any_element());
        }
        out
    }

    /// Run a decision button: send a pick, approve, retry, or open the row for typing.
    fn run_act(&mut self, data: Option<CardData>, act: Act, key: ItemKey, cx: &mut Context<Self>) {
        let Some(d) = data else { return self.expand(key, cx) };
        let review = d.decision.review.as_ref().map(|r| r.task_id).filter(|id| *id > 0).unwrap_or(d.thread_id);
        match act {
            Act::Pick(text) => self.store.update(cx, |s, cx| s.answer_decision(d, text, None, cx)),
            Act::Approve => self.store.update(cx, |s, cx| s.review_task(review, true, None, cx)),
            Act::Retry => self.store.update(cx, |s, cx| s.retry_task(d.task_id, cx)),
            _ => return self.expand(key, cx),
        }
        cx.notify();
    }

    fn snooze_menu(&self, i: usize, it: &InboxItem, cx: &mut Context<Self>) -> Div {
        let choice = |n: usize, label: &str, s: Snooze, cx: &mut Context<Self>| {
            let key = item_key(it);
            chip(format!("inbox-p-snooze-{i}-{n}"), label, cx).on_click(cx.listener(move |this, _, _, cx| this.snooze(key.clone(), s, cx)))
        };
        let hour = choice(0, "1 hour", Snooze::Hour, cx);
        let tomorrow = choice(1, "Until tomorrow", Snooze::Tomorrow, cx);
        div().flex().gap(px(8.)).child(hour).child(tomorrow)
    }

    fn snoozed_buttons(&self, i: usize, it: &InboxItem, until: i64, th: &Theme, cx: &mut Context<Self>) -> Div {
        let key = item_key(it);
        let label = if until == DISMISSED { "Bring back" } else { "Unsnooze" };
        let wake = chip(format!("inbox-p-wake-{i}"), label, cx);
        let wake = wake.on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.unsnooze(&key, cx))));
        let when = div().text_size(px(11.5)).text_color(th.muted).child(until_text(until, now_secs(), local_offset()));
        div().flex().items_center().gap(px(10.)).child(wake).child(when)
    }
}

/// The buttons a decision gets in the slide-over: picks, Approve, answer, retry and "Ask for changes".
/// Raising the limit and Stop stay in the opened card.
fn sheet_acts(d: &crate::api::Decision) -> Vec<Act> {
    decision::acts(d).into_iter().filter(|a| !matches!(a, Act::RaiseLimit | Act::Stop)).collect()
}

// ---- page ----

impl InboxPanel {
    /// "All", "Decisions 2" and "Snoozed" as one segmented bar.
    fn tabs(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let n = tab_count(Tab::Decisions, &s.inbox, &s.snoozed, now_secs());
        let decisions = if n > 0 { format!("Decisions {n}") } else { "Decisions".to_string() };
        let tabs = [(Tab::All, "All".to_string()), (Tab::Decisions, decisions), (Tab::Snoozed, "Snoozed & dismissed".to_string())];
        let items: Vec<AnyElement> = tabs.into_iter().enumerate().map(|(i, (tab, label))| self.tab_button(i, tab, label, th, cx)).collect();
        let bar = div().id("inbox-p-tabs").flex().gap(px(2.)).p(px(3.)).rounded(px(10.)).bg(th.well).border_1().border_color(th.hairline);
        div().pb(px(12.)).child(bar.children(items)).into_any_element()
    }

    fn tab_button(&self, i: usize, tab: Tab, label: String, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let on = self.tab == tab;
        let (bg, fg) = if on { (th.ctl, th.fg) } else { (gpui::transparent_black(), th.muted) };
        let el = div().id(("inbox-p-tab", i)).flex().flex_1().items_center().justify_center().h(px(28.)).rounded(px(8.));
        el.text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).cursor_pointer().bg(bg).text_color(fg)
            .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
            .child(label)
            .into_any_element()
    }

    /// Items for the open tab, each with its snooze end when snoozed.
    fn items(&self, cx: &Context<Self>) -> Vec<(InboxItem, Option<i64>)> {
        let s = self.store.read(cx);
        let now = now_secs();
        let until = |it: &InboxItem| s.snoozed.get(&item_key(it)).copied().filter(|u| *u > now);
        let shown = |(it, u): &(InboxItem, Option<i64>)| in_tab(self.tab, &it.kind, u.is_some());
        s.inbox.iter().map(|it| (it.clone(), until(it))).filter(shown).collect()
    }

    fn list(&mut self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let items = self.items(cx);
        let body = div().id("inbox-p-list").track_scroll(&self.scroll).flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(10.)).pr(px(40.)).pb(px(28.));
        if items.is_empty() {
            let text = if self.tab == Tab::Snoozed { NONE_SNOOZED } else { EMPTY_TEXT };
            return body.child(div().pt(px(12.)).text_size(px(12.5)).text_color(th.muted).child(text)).into_any_element();
        }
        if let Some(at) = items.iter().position(|(it, _)| Some(item_key(it)) == self.scroll_to) {
            self.scroll.scroll_to_item(at);
            self.scroll_to = None;
        }
        let rows: Vec<AnyElement> = items.iter().enumerate().map(|(i, (it, u))| self.row(i, it, *u, th, cx)).collect();
        body.children(rows).into_any_element()
    }
}

impl Render for InboxPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let asks: Vec<ChatAsk> = self.store.read(cx).asks.iter().filter(|a| a.status == "open").cloned().collect();
        self.ask_inputs.sync(&asks.iter().collect::<Vec<_>>(), Self::answer_ask, cx);
        let top = div().flex().flex_col().w_full().max_w(px(PAGE_W)).child(page_head("NEEDS A DECISION", "Inbox", cx)).child(self.tabs(&th, cx));
        let col = div().flex().flex_col().flex_1().min_h_0().w_full().max_w(px(PAGE_W)).child(self.list(&th, cx));
        div().id("inbox-page").flex().flex_col().size_full().child(top).child(col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, id: i64) -> InboxItem {
        InboxItem { kind: kind.into(), id, task_id: None, title: "t".into(), reason: String::new(), actions: vec![], created_at: String::new(), detail: None, link: None }
    }

    #[test]
    fn dismissed_items_stay_hidden_and_say_so() {
        let until = snooze_until(Snooze::Dismiss, 1000, 0);
        let mut snoozed = HashMap::new();
        snoozed.insert(("question".to_string(), 1), until);
        assert!(is_snoozed(&snoozed, &("question".to_string(), 1), i64::MAX - 1));
        assert_eq!(until_text(until, 1000, 0), "Dismissed");
    }

    #[test]
    fn snooze_ends_in_an_hour_or_at_nine_tomorrow() {
        let now = 1_763_460_000; // 2025-11-18 10:00:00 UTC
        assert_eq!(snooze_until(Snooze::Hour, now, 0), now + 3600);
        let nine = 1_763_542_800; // 2025-11-19 09:00:00 UTC
        assert_eq!(snooze_until(Snooze::Tomorrow, now, 0), nine);
        assert_eq!(snooze_until(Snooze::Tomorrow, now, 3600), nine - 3600);
        let late = 1_763_506_800; // 2025-11-18 23:00 UTC, already 00:00 on the 19th at +01:00
        assert_eq!(snooze_until(Snooze::Tomorrow, late, 3600), nine + 86_400 - 3600);
    }

    #[test]
    fn snooze_expires_and_badge_counts_awake_items() {
        let items = [item("question", 1), item("undo", 2), item("stuck", 3)];
        let mut snoozed = HashMap::new();
        snoozed.insert(("question".to_string(), 1), 200);
        snoozed.insert(("stuck".to_string(), 3), 50);
        assert!(is_snoozed(&snoozed, &("question".into(), 1), 100));
        assert!(!is_snoozed(&snoozed, &("question".into(), 1), 200));
        assert!(!is_snoozed(&snoozed, &("undo".into(), 2), 100));
        assert_eq!(awake_count(&items, &snoozed, 100), 2);
        assert_eq!(awake_count(&items, &snoozed, 300), 3);
    }

    #[test]
    fn toggle_key_opens_closes_and_switches() {
        let (a, b) = (("ask".to_string(), 1), ("stuck".to_string(), 1));
        assert_eq!(toggle_key(None, a.clone()), Some(a.clone()));
        assert_eq!(toggle_key(Some(a.clone()), a.clone()), None);
        assert_eq!(toggle_key(Some(a), b.clone()), Some(b));
    }

    #[test]
    fn tabs_split_awake_decisions_and_snoozed() {
        assert!(in_tab(Tab::All, "stuck", false) && !in_tab(Tab::All, "stuck", true));
        assert!(in_tab(Tab::Decisions, "question", false) && !in_tab(Tab::Decisions, "stuck", false));
        assert!(in_tab(Tab::Snoozed, "ask", true) && !in_tab(Tab::Snoozed, "ask", false));
        let items = [item("question", 1), item("question", 2), item("stuck", 3)];
        let mut snoozed = HashMap::new();
        snoozed.insert(("question".to_string(), 2), 500);
        assert_eq!(tab_count(Tab::All, &items, &snoozed, 100), 2);
        assert_eq!(tab_count(Tab::Decisions, &items, &snoozed, 100), 1);
        assert_eq!(tab_count(Tab::Snoozed, &items, &snoozed, 100), 1);
    }

    #[test]
    fn sheet_buttons_skip_raise_limit_and_stop() {
        let actions = vec!["approve".into(), "send_back".into(), "raise_limit".into(), "stop".into()];
        let d = crate::api::Decision { actions, ..Default::default() };
        assert_eq!(sheet_acts(&d), [Act::Approve, Act::SendBack]);
    }

    #[test]
    fn until_reads_like_a_clock() {
        let now = 1_763_460_000;
        assert_eq!(until_text(now + 3900, now, 0), "Until 11:05");
        assert_eq!(until_text(1_763_542_800, now, 0), "Until tomorrow 09:00");
    }
}
