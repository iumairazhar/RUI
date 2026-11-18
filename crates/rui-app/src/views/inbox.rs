//! The Inbox slide-over: what needs the owner, over the current page. Items can be snoozed.

use super::activity::local_offset;
use super::decision::{self, CardEvent, Cards};
use super::library::{after_slide, close_button, scrim, slide_overlay, tab_bar};
use super::{chip, primary_button, tag};
use crate::api::InboxItem;
use crate::store::{self, ItemKey, Store, now_secs};
use crate::theme::{Status, Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, EventEmitter, FontWeight, Stateful, Window, div, prelude::*, px};
use std::collections::HashMap;

const PANEL_W: f32 = 440.;
const EMPTY_TEXT: &str = "Nothing is waiting on you. Every thread settled below you.";
const NONE_SNOOZED: &str = "Nothing is snoozed.";

/// What the panel asks the root to do.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelEvent {
    /// The slide-out finished; remove the panel.
    Close,
    /// Go to the full Inbox page, opening this item's drawer when given.
    OpenFull(Option<ItemKey>),
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Decisions,
    Snoozed,
}

/// How long to snooze an item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Snooze {
    Hour,
    /// Until 9:00 tomorrow, local time.
    Tomorrow,
}

/// Unix time a snooze ends, with `offset` the local UTC offset in seconds.
pub fn snooze_until(s: Snooze, now: i64, offset: i64) -> i64 {
    match s {
        Snooze::Hour => now + 3600,
        Snooze::Tomorrow => ((now + offset).div_euclid(86_400) + 1) * 86_400 + 9 * 3600 - offset,
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
    let day = |t: i64| (t + offset).div_euclid(86_400);
    let secs = (until + offset).rem_euclid(86_400);
    let clock = format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60);
    if day(until) > day(now) { format!("Until tomorrow {clock}") } else { format!("Until {clock}") }
}

/// Tag text and colour family for an item kind.
fn kind_tag<'a>(kind: &str, th: &'a Theme) -> (&'static str, &'a Status) {
    match kind {
        "question" => ("Question", &th.block),
        "stuck" => ("Stuck", &th.block),
        "review" => ("Review", &th.done),
        "undo" => ("Sending", &th.run),
        "checkpoint" => ("Ready", &th.wait),
        "ask" => ("Asked in chat", &th.think),
        _ => ("Seats", &th.wait),
    }
}

/// The slide-over panel with its tabs, rows and snooze menus.
pub struct InboxPanel {
    store: Entity<Store>,
    tab: Tab,
    /// Item whose snooze menu is open.
    menu: Option<ItemKey>,
    /// Question whose decision card is open in its row.
    expanded: Option<i64>,
    cards: Cards,
    closing: bool,
}

impl EventEmitter<PanelEvent> for InboxPanel {}

impl InboxPanel {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self { store, tab: Tab::Decisions, menu: None, expanded: None, cards: Cards::default(), closing: false }
    }

    /// Reset for a fresh slide-in.
    pub fn opened(&mut self, cx: &mut Context<Self>) {
        (self.closing, self.menu, self.tab) = (false, None, Tab::Decisions);
        cx.notify();
    }

    /// Slide out, then tell the root to remove the panel.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        cx.notify();
        after_slide(cx, |v, cx| {
            if v.closing {
                cx.emit(PanelEvent::Close);
            }
        });
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

    fn expand(&mut self, qid: i64, cx: &mut Context<Self>) {
        self.expanded = if self.expanded == Some(qid) { None } else { Some(qid) };
        let top = decision::question_thread(self.store.read(cx), qid);
        self.store.update(cx, |s, cx| top.into_iter().for_each(|id| s.load_thread(id, cx)));
        cx.notify();
    }

    fn on_card(&mut self, ev: &CardEvent, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| match ev {
            CardEvent::Open { thread_id, .. } => s.open_thread(*thread_id, cx),
            CardEvent::Review { thread_id } => s.open_changes(*thread_id, cx),
        });
        cx.emit(PanelEvent::Close);
    }
}

// ---- rows ----

impl InboxPanel {
    fn row(&mut self, i: usize, it: &InboxItem, until: Option<i64>, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (word, st) = kind_tag(&it.kind, th);
        let title = div().flex_1().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg);
        let top = div().flex().items_center().gap(px(8.)).child(title.child(decision::first_line(&it.title))).child(tag(st, word));
        let reason = (!it.reason.is_empty()).then(|| div().text_size(px(11.5)).text_color(th.muted).child(it.reason.clone()));
        let buttons = match until {
            Some(u) => self.snoozed_buttons(i, it, u, th, cx),
            None => self.buttons(i, it, cx),
        };
        let menu = (until.is_none() && self.menu == Some(item_key(it))).then(|| self.snooze_menu(i, it, cx));
        let card = self.row_card(it, until, cx);
        let row = div().id(("inbox-panel-row", i)).flex().flex_col().gap(px(8.)).p(px(12.)).rounded(px(10.)).bg(th.surface);
        row.border_1().border_color(th.hairline).child(top).children(reason).child(buttons).children(menu).children(card).into_any_element()
    }

    fn row_card(&mut self, it: &InboxItem, until: Option<i64>, cx: &mut Context<Self>) -> Option<AnyElement> {
        if until.is_some() || it.kind != "question" || self.expanded != Some(it.id) {
            return None;
        }
        let data = decision::question_card(self.store.read(cx), it.id)?;
        Some(self.cards.get(&data, false, &self.store, Self::on_card, cx).into_any_element())
    }

    /// The kind's own button, then "Snooze".
    fn buttons(&self, i: usize, it: &InboxItem, cx: &mut Context<Self>) -> Div {
        let id = it.id;
        let main = match it.kind.as_str() {
            "question" => Some(chip(format!("inbox-p-answer-{i}"), "Answer", cx).on_click(cx.listener(move |this, _, _, cx| this.expand(id, cx)))),
            "undo" => Some(primary_button(format!("inbox-p-undo-{i}"), "Undo", cx)
                .on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.undo_answer(id, cx))))),
            "checkpoint" => Some(primary_button(format!("inbox-p-release-{i}"), "Release", cx)
                .on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.release_step(id, cx))))),
            "ask" | "stuck" | "review" => Some(self.open_button(i, it, cx)),
            _ => None,
        };
        let key = item_key(it);
        let snooze = chip(format!("inbox-p-snooze-{i}"), "Snooze", cx).on_click(cx.listener(move |this, _, _, cx| this.toggle_menu(key.clone(), cx)));
        div().flex().flex_wrap().items_center().gap(px(8.)).children(main).child(snooze)
    }

    fn open_button(&self, i: usize, it: &InboxItem, cx: &mut Context<Self>) -> Stateful<Div> {
        let key = item_key(it);
        let open = chip(format!("inbox-p-open-{i}"), "Open", cx);
        open.on_click(cx.listener(move |_, _, _, cx| cx.emit(PanelEvent::OpenFull(Some(key.clone())))))
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
        let wake = chip(format!("inbox-p-wake-{i}"), "Unsnooze", cx);
        let wake = wake.on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.unsnooze(&key, cx))));
        let when = div().text_size(px(11.5)).text_color(th.muted).child(until_text(until, now_secs(), local_offset()));
        div().flex().items_center().gap(px(10.)).child(wake).child(when)
    }
}

// ---- panel ----

impl InboxPanel {
    fn head(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let title = div().flex_1().text_size(px(17.)).font_weight(FontWeight::BOLD).text_color(th.fg).child("Inbox");
        let full = div().id("inbox-p-full").text_size(px(12.)).text_color(th.run.fg).cursor_pointer().child("Open full Inbox");
        let full = full.on_click(cx.listener(|_, _, _, cx| cx.emit(PanelEvent::OpenFull(None))));
        let close = close_button("inbox-p-close", Self::close, th, cx);
        div().flex().items_center().gap(px(12.)).px(px(20.)).pt(px(18.)).child(title).child(full).child(close).into_any_element()
    }

    /// Items for the open tab, each with its snooze end when snoozed.
    fn items(&self, cx: &Context<Self>) -> Vec<(InboxItem, Option<i64>)> {
        let s = self.store.read(cx);
        let now = now_secs();
        let until = |it: &InboxItem| s.snoozed.get(&item_key(it)).copied().filter(|u| *u > now);
        let want = self.tab == Tab::Snoozed;
        s.inbox.iter().map(|it| (it.clone(), until(it))).filter(|(_, u)| u.is_some() == want).collect()
    }

    fn list(&mut self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let items = self.items(cx);
        let body = div().id("inbox-p-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(8.)).px(px(20.)).pb(px(20.));
        if items.is_empty() {
            let text = if self.tab == Tab::Snoozed { NONE_SNOOZED } else { EMPTY_TEXT };
            return body.child(div().pt(px(12.)).text_size(px(12.5)).text_color(th.muted).child(text)).into_any_element();
        }
        let rows: Vec<AnyElement> = items.iter().enumerate().map(|(i, (it, u))| self.row(i, it, *u, th, cx)).collect();
        body.children(rows).into_any_element()
    }
}

impl Render for InboxPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let tabs = tab_bar("inbox-p-tabs", &[(Tab::Decisions, "Decisions"), (Tab::Snoozed, "Snoozed")], self.tab, Self::set_tab, &th, cx);
        let shell = div().id("inbox-p-panel").occlude().absolute().right(px(10.)).top(px(10.)).bottom(px(10.)).w(px(PANEL_W));
        let shell = shell.flex().flex_col().overflow_hidden().rounded(px(16.)).bg(th.stage).shadow(th.card_shadow());
        let panel = shell.border_1().border_color(th.glass_rim).child(self.head(&th, cx)).child(tabs).child(self.list(&th, cx));
        let back = scrim("inbox-p-scrim", Self::close, cx);
        slide_overlay("inbox-p", back, panel.into_any_element(), self.closing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, id: i64) -> InboxItem {
        InboxItem { kind: kind.into(), id, task_id: None, title: "t".into(), reason: String::new(), actions: vec![], created_at: String::new() }
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
    fn until_reads_like_a_clock() {
        let now = 1_763_460_000;
        assert_eq!(until_text(now + 3900, now, 0), "Until 11:05");
        assert_eq!(until_text(1_763_542_800, now, 0), "Until tomorrow 09:00");
    }
}
