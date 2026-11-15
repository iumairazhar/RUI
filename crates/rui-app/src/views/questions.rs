//! Open questions column.

use super::primary_button;
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::theme;
use gpui::{Context, Entity, Window, div, prelude::*, px};
use rui_core::Question;
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

    fn card(&self, q: &Question, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let id = q.id;
        let input = self.inputs.get(&id).cloned();
        let th = theme(cx).clone();
        let btn = primary_button("answer", "Answer", cx).id(("answer", id as usize));
        let btn = btn.on_click(cx.listener(move |this, _, _, cx| this.answer(id, cx)));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p(px(10.))
            .rounded_md()
            .bg(th.surface)
            .child(div().text_xs().text_color(th.wait.fg).child(format!("Task #{}", q.task_id)))
            .child(div().text_sm().child(q.text.clone()))
            .children(input)
            .child(btn)
    }
}

impl Render for QuestionsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let questions = self.store.read(cx).questions.clone();
        self.sync_inputs(&questions, cx);
        let mut cards = Vec::new();
        for q in &questions {
            cards.push(self.card(q, cx));
        }
        div().flex().flex_col().size_full().p(px(14.)).gap_3().child(
            div()
                .id("questions-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_3()
                .children(cards),
        )
    }
}
