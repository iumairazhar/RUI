//! "Automatic tasks" in the client drawer: the client's triggers, a delete per row, and an add form.
//! Every change posts the whole list; the daemon checks it and answers 400 with a message when wrong.

use super::library::{self as lib, run, toast};
use super::{chip, primary_button};
use crate::api::{self, Trigger};
use crate::store::Store;
use crate::text_input::TextInput;
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, FontWeight, Render, SharedString, Window, div, prelude::*, px};

const TITLE: &str = "Automatic tasks";
const SUBTITLE: &str = "Start a task when something happens.";
const EMPTY: &str = "None yet. Add one below.";

/// What starts the task, as picked in the add form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum When {
    PrMerged,
    Email,
    Schedule,
}

impl When {
    const ALL: [When; 3] = [When::PrMerged, When::Email, When::Schedule];

    /// The daemon's `on` value.
    fn key(self) -> &'static str {
        match self {
            When::PrMerged => "pr_merged",
            When::Email => "email",
            When::Schedule => "schedule",
        }
    }

    /// Text on the form's choice button.
    fn choice(self) -> &'static str {
        match self {
            When::PrMerged => "A PR merges",
            When::Email => "This client emails",
            When::Schedule => "Every day",
        }
    }
}

/// "When a PR merges", "When this client emails", "Every day at 09:00" and so on.
pub fn when_label(t: &Trigger) -> String {
    match t.on.as_str() {
        "pr_merged" => "When a PR merges".into(),
        "email" => "When this client emails".into(),
        "schedule" => cron_label(t.cron.as_deref().unwrap_or("")),
        other => format!("When {other}"),
    }
}

/// Plain words for the daemon's tiny cron: "HH:MM daily", "weekdays HH:MM" or "<day> HH:MM".
fn cron_label(cron: &str) -> String {
    let parts: Vec<String> = cron.split_whitespace().map(str::to_lowercase).collect();
    match parts.as_slice() {
        [t, d] if d == "daily" => format!("Every day at {t}"),
        [d, t] if d == "weekdays" => format!("Every weekday at {t}"),
        [d, t] => match day_name(d) {
            Some(day) => format!("Every {day} at {t}"),
            None => format!("On schedule {cron}"),
        },
        _ => format!("On schedule {cron}"),
    }
}

fn day_name(d: &str) -> Option<&'static str> {
    let names = [("mon", "Monday"), ("tue", "Tuesday"), ("wed", "Wednesday"), ("thu", "Thursday"), ("fri", "Friday"), ("sat", "Saturday"), ("sun", "Sunday")];
    names.iter().find(|(k, _)| d.starts_with(k)).map(|(_, n)| *n)
}

/// "9:05" or "09:05" as "09:05"; `None` when it is not a real time.
fn clock(s: &str) -> Option<String> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

/// First "t<n>" id not already used.
fn next_id(list: &[Trigger]) -> String {
    (1..).map(|n| format!("t{n}")).find(|id| list.iter().all(|t| &t.id != id)).unwrap_or_default()
}

/// The add form as a trigger entry, or what is wrong with it.
pub fn entry(when: When, time: &str, title: &str, body: &str, list: &[Trigger]) -> Result<Trigger, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Give it a title".into());
    }
    let cron = match when {
        When::Schedule => Some(format!("{} daily", clock(time).ok_or("Time must look like 09:00")?)),
        _ => None,
    };
    Ok(Trigger { id: next_id(list), on: when.key().into(), cron, title: title.into(), body: body.trim().into() })
}

/// The triggers section of the client drawer.
pub struct TriggerPanel {
    store: Entity<Store>,
    client: Option<i64>,
    /// Current list; `None` while loading.
    list: Option<Vec<Trigger>>,
    when: When,
    time: Entity<TextInput>,
    title: Entity<TextInput>,
    body: Entity<TextInput>,
    busy: bool,
}

impl TriggerPanel {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let time = cx.new(|cx| TextInput::new("09:00", cx));
        let title = cx.new(|cx| TextInput::new("Title, like \u{201c}Check the release notes\u{201d}", cx));
        let body = cx.new(|cx| TextInput::new("What should the task do? (optional)", cx));
        Self { store, client: None, list: None, when: When::PrMerged, time, title, body, busy: false }
    }

    /// Show a client's triggers and clear the form.
    pub fn open(&mut self, id: i64, cx: &mut Context<Self>) {
        (self.client, self.list, self.when, self.busy) = (Some(id), None, When::PrMerged, false);
        self.clear_form(cx);
        run(cx, move || api::get_triggers(id), move |v, res, cx| v.loaded(id, res, cx));
        cx.notify();
    }

    fn clear_form(&mut self, cx: &mut Context<Self>) {
        for input in [&self.time, &self.title, &self.body] {
            input.update(cx, |i, cx| i.clear(cx));
        }
    }

    fn loaded(&mut self, id: i64, res: Result<Vec<Trigger>, String>, cx: &mut Context<Self>) {
        if self.client != Some(id) {
            return;
        }
        if let Err(e) = &res {
            toast(&self.store, "Could not load automatic tasks", e, cx);
        }
        self.list = Some(res.unwrap_or_default());
        cx.notify();
    }

    fn pick(&mut self, when: When, cx: &mut Context<Self>) {
        self.when = when;
        cx.notify();
    }

    /// Check the form, then save the list with the new entry added.
    fn add(&mut self, cx: &mut Context<Self>) {
        let Some(list) = self.list.clone() else { return };
        let read = |e: &Entity<TextInput>, cx: &Context<Self>| e.read(cx).content();
        let (time, title, body) = (read(&self.time, cx), read(&self.title, cx), read(&self.body, cx));
        match entry(self.when, &time, &title, &body, &list) {
            Ok(t) => self.save([list, vec![t]].concat(), true, cx),
            Err(e) => toast(&self.store, "Could not add it", &e, cx),
        }
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(list) = self.list.clone() else { return };
        self.save(list.into_iter().filter(|t| t.id != id).collect(), false, cx);
    }

    /// Post the whole list; `added` clears the form on success.
    fn save(&mut self, list: Vec<Trigger>, added: bool, cx: &mut Context<Self>) {
        let Some(id) = self.client else { return };
        if self.busy {
            return;
        }
        self.busy = true;
        run(cx, move || api::set_triggers(id, &list), move |v, res, cx| v.saved(id, added, res, cx));
        cx.notify();
    }

    fn saved(&mut self, id: i64, added: bool, res: Result<Vec<Trigger>, String>, cx: &mut Context<Self>) {
        self.busy = false;
        match res {
            Ok(list) if self.client == Some(id) => {
                self.list = Some(list);
                if added {
                    self.clear_form(cx);
                }
                toast(&self.store, "Automatic tasks saved", "", cx);
            }
            Ok(_) => toast(&self.store, "Automatic tasks saved", "", cx),
            Err(e) => toast(&self.store, "Could not save automatic tasks", &e, cx),
        }
        cx.notify();
    }

    fn row(&self, i: usize, t: &Trigger, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let when = div().text_size(px(11.5)).text_color(th.muted).child(when_label(t));
        let title = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(t.title.clone());
        let text = div().flex().flex_col().gap(px(2.)).flex_1().min_w_0().child(when).child(title);
        let del_id = SharedString::from(format!("trigger-del-{i}"));
        let id = t.id.clone();
        let del = lib::danger_button(del_id, "Delete", !self.busy, th);
        let del = del.on_click(cx.listener(move |this, _, _, cx| this.delete(id.clone(), cx)));
        let row = div().flex().items_center().gap(px(10.)).px(px(12.)).py(px(10.)).rounded(px(10.)).bg(th.well);
        row.child(text).child(del).into_any_element()
    }

    fn rows(&self, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match self.list.as_deref() {
            None => vec![lib::note("Loading\u{2026}", th).into_any_element()],
            Some([]) => vec![lib::note(EMPTY, th).into_any_element()],
            Some(list) => list.iter().enumerate().map(|(i, t)| self.row(i, t, th, cx)).collect(),
        }
    }

    fn choices(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = |(i, w): (usize, &When)| {
            let (w, id) = (*w, SharedString::from(format!("trigger-when-{i}")));
            let el = if w == self.when { primary_button(id, w.choice(), cx) } else { chip(id, w.choice(), cx) };
            el.on_click(cx.listener(move |this, _, _, cx| this.pick(w, cx))).into_any_element()
        };
        let items: Vec<AnyElement> = When::ALL.iter().enumerate().map(button).collect();
        div().flex().flex_wrap().items_center().gap(px(6.)).children(items).into_any_element()
    }

    fn form(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let time = (self.when == When::Schedule).then(|| labeled("At", field(self.time.clone(), th), th));
        let label = if self.busy { "Saving\u{2026}" } else { "Add" };
        let add = primary_button("trigger-add", label, cx).on_click(cx.listener(|this, _, _, cx| this.add(cx)));
        let col = div().flex().flex_col().gap(px(10.)).child(labeled("When", self.choices(cx), th)).children(time);
        let col = col.child(labeled("Title", field(self.title.clone(), th), th));
        col.child(labeled("Task", field(self.body.clone(), th), th)).child(div().flex().child(add)).into_any_element()
    }
}

fn field(input: Entity<TextInput>, th: &Theme) -> AnyElement {
    let f = div().px(px(10.)).py(px(4.)).rounded(px(8.)).bg(th.well).border_1().border_color(th.hairline);
    f.child(input).into_any_element()
}

fn labeled(label: &str, el: AnyElement, th: &Theme) -> Div {
    let head = div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(label.to_string());
    div().flex().flex_col().gap(px(4.)).child(head).child(el)
}

impl Render for TriggerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let title = div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(TITLE);
        let head = div().flex().flex_col().gap(px(2.)).child(title).child(lib::note(SUBTITLE, &th));
        let rows = div().flex().flex_col().gap(px(6.)).children(self.rows(&th, cx));
        div().flex().flex_col().gap(px(12.)).child(head).child(rows).child(self.form(&th, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, on: &str, cron: Option<&str>) -> Trigger {
        Trigger { id: id.into(), on: on.into(), cron: cron.map(Into::into), title: "x".into(), body: String::new() }
    }

    #[test]
    fn labels_say_when_it_fires() {
        assert_eq!(when_label(&t("a", "pr_merged", None)), "When a PR merges");
        assert_eq!(when_label(&t("a", "email", None)), "When this client emails");
        assert_eq!(when_label(&t("a", "schedule", Some("09:00 daily"))), "Every day at 09:00");
        assert_eq!(when_label(&t("a", "schedule", Some("weekdays 08:30"))), "Every weekday at 08:30");
        assert_eq!(when_label(&t("a", "schedule", Some("Mon 09:00"))), "Every Monday at 09:00");
    }

    #[test]
    fn form_becomes_an_entry() {
        let list = vec![t("t1", "email", None)];
        let e = entry(When::Schedule, "9:05", " Standup ", " notes ", &list).unwrap();
        assert_eq!((e.id.as_str(), e.on.as_str(), e.cron.as_deref()), ("t2", "schedule", Some("09:05 daily")));
        assert_eq!((e.title.as_str(), e.body.as_str()), ("Standup", "notes"));
        let e = entry(When::PrMerged, "ignored", "Review", "", &[]).unwrap();
        assert_eq!((e.id.as_str(), e.on.as_str(), e.cron), ("t1", "pr_merged", None));
    }

    #[test]
    fn bad_forms_are_refused() {
        assert_eq!(entry(When::Email, "", "  ", "", &[]).unwrap_err(), "Give it a title");
        assert!(entry(When::Schedule, "25:00", "x", "", &[]).is_err());
        assert!(entry(When::Schedule, "", "x", "", &[]).is_err());
    }
}
