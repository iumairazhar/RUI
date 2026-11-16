//! The "New specialist" form: name, two-letter code, title and description.

use super::AgentsView;
use crate::text_input::{Submit, TextInput};
use crate::theme::Theme;
use crate::views::activity::section;
use crate::views::library as lib;
use crate::views::{chip, primary_button};
use gpui::{AnyElement, App, Context, Entity, FontWeight, div, prelude::*, px};
use serde_json::{Value, json};

const FIELDS: [(&str, &str); 4] = [
    ("Name", "lowercase letters, digits and hyphens"),
    ("Code", "two letters, like BE"),
    ("Title", "Data Engineer"),
    ("Description", "One line that tells Rui when to pick this agent"),
];

/// The four inputs and the last error.
pub struct Form {
    inputs: Vec<Entity<TextInput>>,
    pub error: Option<String>,
}

/// Validated JSON body for `POST /roles`, or the message to show.
pub fn body(vals: &[String]) -> Result<Value, String> {
    let [name, code, title, desc] = vals else { return Err("Fill in every field".into()) };
    let (name, code, title, desc) = (name.trim(), code.trim(), title.trim(), desc.trim());
    if [name, code, title, desc].iter().any(|v| v.is_empty()) {
        return Err("Fill in every field".into());
    }
    if code.chars().count() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err("The code is two letters".into());
    }
    Ok(json!({ "name": name.to_lowercase(), "code": code.to_uppercase(), "title": title, "description": desc }))
}

impl Form {
    /// Build the inputs; Enter in any of them calls `submit`.
    pub fn new<V: 'static>(submit: fn(&mut V, &mut Context<V>), cx: &mut Context<V>) -> Self {
        let inputs = FIELDS
            .iter()
            .map(|(_, hint)| {
                let input = cx.new(|cx| TextInput::new(hint, cx));
                cx.subscribe(&input, move |this, _, _: &Submit, cx| submit(this, cx)).detach();
                input
            })
            .collect();
        Self { inputs, error: None }
    }

    /// Empty the inputs and the error.
    pub fn reset(&mut self, cx: &mut App) {
        self.error = None;
        for i in &self.inputs {
            i.update(cx, |i, cx| i.clear(cx));
        }
    }

    /// The request body, or the message to show.
    pub fn values(&self, cx: &App) -> Result<Value, String> {
        let vals: Vec<String> = self.inputs.iter().map(|i| i.read(cx).content()).collect();
        body(&vals)
    }
}

impl AgentsView {
    /// The drawer that holds the form.
    pub(super) fn new_drawer(&mut self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let close = lib::close_button("agents-drawer-close", Self::close, th, cx);
        let eyebrow = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("NEW SPECIALIST");
        let title = div().text_size(px(23.)).font_weight(FontWeight::BOLD).text_color(th.fg).child("New specialist");
        let left = div().flex().flex_col().gap(px(2.)).child(eyebrow).child(title);
        let head = div().flex().items_start().justify_between().px(px(20.)).pt(px(20.)).pb(px(8.)).child(left).child(close);
        let fields: Vec<AnyElement> = FIELDS
            .iter()
            .zip(&self.form.inputs)
            .map(|((label, _), input)| {
                let boxed = div().px(px(12.)).py(px(8.)).rounded(px(9.)).bg(th.well).border_1().border_color(th.hairline);
                section(label, vec![boxed.child(input.clone()).into_any_element()])
            })
            .collect();
        let err = self.form.error.as_ref().map(|e| div().text_size(px(12.)).text_color(th.block.fg).child(e.clone()));
        let note = lib::note("It stays off until you turn it on.", th);
        let body = lib::scroll_body("agents-new-scroll", fields).child(note).children(err);
        let go = primary_button("agents-create", "Create", cx).on_click(cx.listener(|this, _, _, cx| this.create(cx)));
        let no = chip("agents-create-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let foot = lib::drawer_foot(vec![go.into_any_element(), no.into_any_element()], None, th);
        lib::drawer_shell("agents-drawer", th).child(head).child(body).child(foot).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(v: [&str; 4]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn body_needs_every_field_and_a_two_letter_code() {
        let ok = body(&vals(["data-eng", "de", " Data Engineer ", "Builds pipelines"])).unwrap();
        assert_eq!((ok["code"].as_str(), ok["title"].as_str(), ok["name"].as_str()), (Some("DE"), Some("Data Engineer"), Some("data-eng")));
        assert!(body(&vals(["a", "D", "t", "d"])).is_err());
        assert!(body(&vals(["a", "D1", "t", "d"])).is_err());
        assert!(body(&vals(["a", "DE", "", "d"])).is_err());
        assert!(body(&[]).is_err());
    }
}
