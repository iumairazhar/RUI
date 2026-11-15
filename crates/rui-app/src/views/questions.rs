//! Open questions column.

use super::{card, chip, page_head, question_card};
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::theme;
use gpui::{App, Context, Entity, FontWeight, Window, div, prelude::*, px};
use rui_core::{Question, QuestionStatus};
use std::collections::HashMap;

/// Open questions, each with its own answer input.
pub struct QuestionsView {
    store: Entity<Store>,
    inputs: HashMap<i64, Entity<TextInput>>,
}

impl QuestionsView {
    /// Build the questions view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self { store, inputs: HashMap::new() }
    }

    fn sync_inputs(&mut self, questions: &[Question], cx: &mut Context<Self>) {
        self.inputs.retain(|id, _| questions.iter().any(|q| q.id == *id));
        for q in questions {
            if self.inputs.contains_key(&q.id) {
                continue;
            }
            let id = q.id;
            let input = cx.new(|cx| TextInput::new("Your answer...", cx));
            cx.subscribe(&input, move |this, _, _: &Submit, cx| this.answer(id, cx)).detach();
            self.inputs.insert(id, input);
        }
    }

    fn answer(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(input) = self.inputs.get(&id).cloned() else { return };
        let text = input.read(cx).content();
        if text.trim().is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| s.answer(id, text, cx));
    }

    fn open_card(&self, q: &Question, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let id = q.id;
        let th = theme(cx).clone();
        let title = self.store.read(cx).tasks.iter().find(|t| t.id == q.task_id).map(|t| t.title.clone());
        let input = self.inputs.get(&id).cloned().map(|i| i.into_any_element());
        let btn = chip(format!("answer-{id}"), "Answer", cx)
            .on_click(cx.listener(move |this, _, _, cx| this.answer(id, cx)));
        let meta = format!(
            "Task #{} · {}asked by {}",
            q.task_id,
            title.map(|t| format!("{t} · ")).unwrap_or_default(),
            capitalize(q.asked_by.as_str())
        );
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().text_size(px(11.)).text_color(th.muted).child(meta))
            .child(question_card(
                &q.text,
                input.unwrap_or_else(|| div().into_any_element()),
                btn.into_any_element(),
                cx,
            ))
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
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
        .child(div().text_size(px(12.5)).text_color(th.fg_2).child(q.text.clone()))
        .child(div().text_size(px(12.)).text_color(th.accent_text).child(format!("→ {answer}")))
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
        self.sync_inputs(&questions, cx);
        let th = theme(cx).clone();
        let open: Vec<_> = questions.iter().filter(|q| q.status == QuestionStatus::Open).collect();
        let answered: Vec<_> = questions
            .iter()
            .filter(|q| q.status == QuestionStatus::Answered)
            .rev()
            .take(10)
            .collect();
        let cards: Vec<_> = open.iter().map(|q| self.open_card(q, cx).into_any_element()).collect();
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
            .children(empty)
            .children((!answered.is_empty()).then(|| answered_section(&answered, cx)));
        div().flex().flex_col().size_full().child(page_head("NEEDS A DECISION", "Inbox", cx)).child(body)
    }
}
