//! Rui's asks: the question card shared by the chat and the Inbox, and its answer inputs.

use super::{chip, markdown, tag};
use crate::text_input::{Submit, TextInput};
use crate::theme::theme;
use gpui::{AnyElement, App, Context, Entity, FontWeight, SharedString, div, prelude::*, px, svg};
use rui_core::ChatAsk;
use std::collections::HashMap;

/// What a card calls when the owner answers: the ask id and the text, or `None` to read the typed input.
pub type OnAnswer<V> = fn(&mut V, i64, Option<String>, &mut Context<V>);

/// One free-text input per open ask.
#[derive(Default)]
pub struct AskInputs {
    inputs: HashMap<i64, Entity<TextInput>>,
}

impl AskInputs {
    /// Keep exactly one input per ask in `asks`; Enter calls `on_answer` with `None`.
    pub fn sync<V: 'static>(&mut self, asks: &[&ChatAsk], on_answer: OnAnswer<V>, cx: &mut Context<V>) {
        self.inputs.retain(|id, _| asks.iter().any(|a| a.id == *id));
        for a in asks {
            if self.inputs.contains_key(&a.id) {
                continue;
            }
            let id = a.id;
            let input = cx.new(|cx| TextInput::new("Other\u{2026}", cx));
            cx.subscribe(&input, move |this, _, _: &Submit, cx| on_answer(this, id, None, cx)).detach();
            self.inputs.insert(id, input);
        }
    }

    /// The typed answer of an ask, cleared; `None` when empty.
    pub fn take(&self, id: i64, cx: &mut App) -> Option<String> {
        let input = self.inputs.get(&id)?;
        let text = input.read(cx).content().trim().to_string();
        input.update(cx, |i, cx| i.clear(cx));
        Some(text).filter(|t| !t.is_empty())
    }

    /// The input of an ask.
    pub fn get(&self, id: i64) -> Option<Entity<TextInput>> {
        self.inputs.get(&id).cloned()
    }
}

fn eid(prefix: &str, id: i64) -> (SharedString, usize) {
    (SharedString::from(prefix.to_string()), id as usize)
}

/// The mock's `.ask` card: hand icon, the question, option buttons and an "Other…" input.
/// Answered asks turn into a done card with a tag holding the chosen answer.
pub fn ask_card<V: 'static>(a: &ChatAsk, inputs: &AskInputs, on_answer: OnAnswer<V>, cx: &mut Context<V>) -> AnyElement {
    let th = theme(cx).clone();
    let done = a.status == "answered";
    let st = if done { &th.done } else { &th.wait };
    let icon = if done { "icons/check.svg" } else { "icons/hand.svg" };
    let mark = svg().path(icon).size(px(15.)).flex_none().text_color(st.fg);
    let question = div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).child(markdown::render(&a.text, st.fg, 13., &th));
    let head = div().flex().items_start().gap(px(8.)).child(mark).child(question);
    let foot = match (done, a.answer.as_ref()) {
        (true, Some(answer)) => div().flex().child(tag(&th.done, answer)),
        (true, None) => div(),
        _ => answer_row(a, inputs, on_answer, cx),
    };
    let card = div().id(eid("ask-card", a.id)).flex().flex_col().gap(px(12.)).p(px(16.)).max_w(px(660.));
    card.rounded(px(9.)).bg(st.dim).border_1().border_color(st.rim).child(head).child(foot).into_any_element()
}

fn answer_row<V: 'static>(a: &ChatAsk, inputs: &AskInputs, on_answer: OnAnswer<V>, cx: &mut Context<V>) -> gpui::Div {
    let id = a.id;
    let options: Vec<AnyElement> = a
        .options
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let pick = text.clone();
            let btn = chip(format!("ask-opt-{id}-{i}"), text, cx);
            btn.on_click(cx.listener(move |this, _, _, cx| on_answer(this, id, Some(pick.clone()), cx))).into_any_element()
        })
        .collect();
    let options = (!options.is_empty()).then(|| div().flex().flex_wrap().gap(px(7.)).children(options));
    let send = chip(format!("ask-send-{id}"), "Send", cx).on_click(cx.listener(move |this, _, _, cx| on_answer(this, id, None, cx)));
    let field = inputs.get(id).map(|i| div().flex_1().min_w_0().child(i));
    let typed = div().flex().items_center().gap(px(8.)).children(field).child(send);
    div().flex().flex_col().gap(px(10.)).children(options).child(typed)
}
