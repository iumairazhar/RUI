//! Inbox: owner-level questions, each with its escalation ladder, a reason field and an answer field.

use super::activity::{LEAVE_MS, leave};
use super::{card, chip, page_head, question_card};
use crate::store::{self, Store, pickup_who};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, App, Context, Div, Entity, FontWeight, Stateful, Window, div, prelude::*, px};
use rui_core::{Question, QuestionStatus, Rung};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const WHY_HINT: &str = "Why \u{2014} your reason goes back down with the answer";
/// A view not rendered for this long forgot which questions it showed.
const STALE: Duration = Duration::from_millis(1500);

/// Open questions, each with its own answer and reason inputs.
pub struct QuestionsView {
    store: Entity<Store>,
    inputs: HashMap<i64, Entity<TextInput>>,
    whys: HashMap<i64, Entity<TextInput>>,
    /// Open questions at the last render, to spot ones that were answered.
    prev: HashMap<i64, Question>,
    /// Answered questions still sliding out, and when they started.
    leaving: HashMap<i64, (Question, Instant)>,
    last_render: Instant,
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

fn lead_line(n: usize, th: &Theme) -> Stateful<Div> {
    let base = div().id("inbox-lead").flex().text_size(px(14.)).text_color(th.fg_2).pb(px(16.));
    if n == 0 {
        return base.child("Nothing is waiting on you. Every thread settled below you.");
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

impl QuestionsView {
    /// Build the questions view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let (inputs, whys) = (HashMap::new(), HashMap::new());
        let (prev, leaving) = (HashMap::new(), HashMap::new());
        Self { store, inputs, whys, prev, leaving, last_render: Instant::now() }
    }

    /// Notice questions that left the open list since the last render, and drop finished ghosts.
    fn track_leaving(&mut self, open: &[&Question]) {
        if self.last_render.elapsed() > STALE {
            self.prev.clear();
        }
        self.last_render = Instant::now();
        for (id, q) in &self.prev {
            if !open.iter().any(|o| o.id == *id) {
                self.leaving.entry(*id).or_insert_with(|| (q.clone(), Instant::now()));
            }
        }
        self.prev = open.iter().map(|q| (q.id, (*q).clone())).collect();
        self.leaving.retain(|_, (_, at)| at.elapsed() < Duration::from_millis(LEAVE_MS));
    }

    fn sync_inputs(&mut self, open: &[&Question], cx: &mut Context<Self>) {
        let keep = |id: &i64, gone: &HashMap<i64, (Question, Instant)>| {
            open.iter().any(|q| q.id == *id) || gone.contains_key(id)
        };
        self.inputs.retain(|id, _| keep(id, &self.leaving));
        self.whys.retain(|id, _| keep(id, &self.leaving));
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
    }

    fn open_card(&self, q: &Question, cx: &mut Context<Self>) -> AnyElement {
        let id = q.id;
        let th = theme(cx).clone();
        let s = self.store.read(cx);
        let task = s.tasks.iter().find(|t| t.id == q.task_id);
        let project = task.and_then(|t| s.clients.iter().find(|c| c.id == t.client_id)).map(|c| c.name.clone());
        let title = task.map_or_else(|| format!("Task #{}", q.task_id), |t| t.title.clone());
        let sub = format!("{}{} stopped and passed it up", project.map(|p| format!("{p} \u{b7} ")).unwrap_or_default(), pickup_who(q));
        let head = div().flex().flex_col().gap(px(2.));
        let head = head.child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(title));
        let head = head.child(div().text_size(px(11.)).text_color(th.muted).child(sub));
        let field = |m: &HashMap<i64, Entity<TextInput>>| m.get(&id).cloned().map(|i| i.into_any_element());
        let btn = chip(format!("answer-{id}"), "Send answer", cx)
            .on_click(cx.listener(move |this, _, _, cx| this.answer(id, cx)));
        let input = field(&self.inputs).unwrap_or_else(|| div().into_any_element());
        let card = question_card(&q.text, field(&self.whys), input, btn.into_any_element(), cx);
        let body = div().flex().flex_col().gap(px(8.)).child(head).child(ladder_row(q, &th)).child(card);
        body.id(("inbox-card", id as usize)).into_any_element()
    }
}

fn answered_card(q: &Question, cx: &App) -> impl IntoElement {
    let th = theme(cx);
    let answer = q.answer.clone().unwrap_or_default();
    card(cx)
        .id(("answered", q.id as usize))
        .flex()
        .flex_col()
        .gap(px(4.))
        .max_w(px(660.))
        .child(super::markdown::render(&q.text, th.fg_2, 12.5, th))
        .child(div().text_size(px(12.)).text_color(th.accent_text).child(format!("\u{2192} {answer}")))
}

fn answered_section(answered: &[&Question], cx: &App) -> impl IntoElement {
    let th = theme(cx);
    let head = div()
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(th.muted)
        .child("ANSWERED");
    div()
        .id("answered-section")
        .flex()
        .flex_col()
        .gap(px(8.))
        .mt(px(24.))
        .child(head)
        .children(answered.iter().map(|q| answered_card(q, cx)))
}

impl Render for QuestionsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let questions = self.store.read(cx).questions.clone();
        let th = theme(cx).clone();
        let open: Vec<_> = questions.iter().filter(|q| q.status == QuestionStatus::Open).collect();
        self.track_leaving(&open);
        self.sync_inputs(&open, cx);
        let answered: Vec<_> = questions
            .iter()
            .filter(|q| q.status == QuestionStatus::Answered)
            .rev()
            .take(10)
            .collect();
        let cards: Vec<_> = open.iter().map(|q| self.open_card(q, cx)).collect();
        let gone: Vec<_> = self.leaving.values().map(|(q, _)| q.clone()).collect();
        let ghosts = gone.iter().map(|q| leave(self.open_card(q, cx), ("inbox-out", q.id as usize)));
        let ghosts: Vec<_> = ghosts.collect();
        let empty = open.is_empty().then(|| {
            card(cx).child(div().text_size(px(13.)).text_color(th.fg_2).child("Nothing needs you."))
        });
        let body = div()
            .id("questions-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pr(px(40.))
            .pb(px(28.))
            .children(cards)
            .children(ghosts)
            .children(empty)
            .children((!answered.is_empty()).then(|| answered_section(&answered, cx)));
        let head = page_head("NEEDS A DECISION", "Inbox", cx);
        div().flex().flex_col().size_full().child(head).child(lead_line(open.len(), &th)).child(body)
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
}
