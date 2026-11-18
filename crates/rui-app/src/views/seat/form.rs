//! The "Add seat" form: kind, label, login folder or environment variable name, budget, window and order.
//! It never asks for a key value: an API key seat only names the variable that holds the key.

use super::SeatView;
use crate::text_input::{Submit, TextInput};
use crate::theme::Theme;
use crate::views::activity::section;
use crate::views::{card, chip, primary_button};
use gpui::{AnyElement, App, Context, Div, Entity, div, prelude::*, px};
use serde_json::{Value, json};

/// The two seat kinds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Subscription,
    ApiKey,
}

impl Kind {
    fn wire(self) -> &'static str {
        match self {
            Kind::Subscription => "subscription",
            Kind::ApiKey => "api_key",
        }
    }

    fn place(self) -> (&'static str, &'static str) {
        match self {
            Kind::Subscription => ("Login folder", "~/.claude-work \u{2014} empty for the default login"),
            Kind::ApiKey => ("Environment variable name", "ANTHROPIC_API_KEY_WORK"),
        }
    }
}

/// The form's text, as typed.
pub struct Fields<'a> {
    pub label: &'a str,
    pub place: &'a str,
    pub budget: &'a str,
    pub window: &'a str,
    pub order: &'a str,
}

/// An environment variable name: a letter or `_`, then letters, digits or `_`.
pub fn is_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    let first = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    first && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Seat id from its label: lowercase letters and digits, other runs become one `-`.
pub fn seat_id(label: &str) -> String {
    let mapped: String = label.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    mapped.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// The JSON body for `POST /seats`, or the message to show.
pub fn seat_body(kind: Kind, f: &Fields) -> Result<Value, String> {
    let (label, id) = (f.label.trim(), seat_id(f.label));
    if label.is_empty() || id.is_empty() {
        return Err("Give the seat a label with a letter or digit".into());
    }
    let mut body = json!({ "id": id, "kind": kind.wire(), "label": label });
    place(kind, f.place.trim(), &mut body)?;
    body["budget_usd"] = budget(f.budget.trim())?;
    body["window_hours"] = json!(number(f.window.trim(), 5u32).filter(|h| *h >= 1).ok_or("The window is a whole number of hours, 1 or more")?);
    body["order"] = json!(number(f.order.trim(), 0i32).ok_or("The order is a whole number")?);
    Ok(body)
}

/// The login folder (subscription) or the variable's name (API key).
fn place(kind: Kind, text: &str, body: &mut Value) -> Result<(), String> {
    match kind {
        Kind::Subscription if text.is_empty() => Ok(()),
        Kind::Subscription if text.starts_with('/') || text.starts_with('~') => Ok(body["config_dir"] = json!(text)),
        Kind::Subscription => Err("The login folder starts with / or ~".into()),
        Kind::ApiKey if is_env_name(text) => Ok(body["env_var"] = json!(text)),
        Kind::ApiKey => Err("Enter the variable's name, like ANTHROPIC_API_KEY_WORK \u{2014} never the key itself".into()),
    }
}

/// Empty means no limit.
fn budget(text: &str) -> Result<Value, String> {
    if text.is_empty() {
        return Ok(Value::Null);
    }
    let b: f64 = text.trim_start_matches('$').parse().map_err(|_| "The budget is a number of dollars")?;
    if !b.is_finite() || b < 0.0 {
        return Err("The budget can't be negative".into());
    }
    Ok(json!(b))
}

/// Parsed number, or `default` when empty.
fn number<T: std::str::FromStr>(text: &str, default: T) -> Option<T> {
    if text.is_empty() { Some(default) } else { text.parse().ok() }
}

const LABELS: [(&str, &str); 4] = [
    ("Label", "Work login"),
    ("Budget per window (USD)", "Empty for no limit"),
    ("Window (hours)", "5"),
    ("Order", "0 \u{2014} lower goes first"),
];

/// The inputs, the picked kind and the last error.
pub struct SeatForm {
    pub kind: Kind,
    /// Label, budget, window, order.
    inputs: Vec<Entity<TextInput>>,
    /// Login folder or variable name; rebuilt when the kind changes so its hint matches.
    place: Entity<TextInput>,
    pub error: Option<String>,
}

impl SeatForm {
    pub fn new(cx: &mut Context<SeatView>) -> Self {
        let inputs = LABELS.iter().map(|(_, hint)| input(hint, cx)).collect();
        let place = input(Kind::Subscription.place().1, cx);
        Self { kind: Kind::Subscription, inputs, place, error: None }
    }

    /// Switch kind; the place field starts empty so a path never goes in as a variable name.
    pub fn set_kind(&mut self, kind: Kind, cx: &mut Context<SeatView>) {
        if self.kind != kind {
            (self.kind, self.place) = (kind, input(kind.place().1, cx));
        }
    }

    pub fn reset(&mut self, cx: &mut App) {
        self.error = None;
        for i in self.inputs.iter().chain([&self.place]) {
            i.update(cx, |i, cx| i.clear(cx));
        }
    }

    /// The request body, or the message to show.
    pub fn values(&self, cx: &App) -> Result<Value, String> {
        let v: Vec<String> = self.inputs.iter().map(|i| i.read(cx).content()).collect();
        let place = self.place.read(cx).content();
        seat_body(self.kind, &Fields { label: &v[0], place: &place, budget: &v[1], window: &v[2], order: &v[3] })
    }
}

fn input(hint: &str, cx: &mut Context<SeatView>) -> Entity<TextInput> {
    let input = cx.new(|cx| TextInput::new(hint, cx));
    cx.subscribe(&input, |this, _, _: &Submit, cx| this.save(cx)).detach();
    input
}

fn boxed(input: Entity<TextInput>, th: &Theme) -> AnyElement {
    let field = div().px(px(12.)).py(px(8.)).rounded(px(9.)).bg(th.well).border_1().border_color(th.hairline);
    field.child(input).into_any_element()
}

impl SeatView {
    /// The form card: kind toggle, fields, error, then Save and Cancel.
    pub(super) fn form_card(&self, th: &Theme, cx: &mut Context<Self>) -> Div {
        let f = &self.form;
        let mut fields = vec![section("Kind", vec![self.kind_toggle(cx)])];
        fields.push(section(LABELS[0].0, vec![boxed(f.inputs[0].clone(), th)]));
        fields.push(section(f.kind.place().0, vec![boxed(f.place.clone(), th)]));
        fields.extend(LABELS[1..].iter().zip(&f.inputs[1..]).map(|((l, _), i)| section(l, vec![boxed(i.clone(), th)])));
        let err = f.error.as_ref().map(|e| div().text_size(px(12.)).text_color(th.block.fg).child(e.clone()));
        let save = primary_button("seat-save", "Save seat", cx).on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        let cancel = chip("seat-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| this.show_form(false, cx)));
        let buttons = div().flex().gap(px(8.)).child(save).child(cancel);
        card(cx).p(px(16.)).flex().flex_col().gap(px(14.)).max_w(px(560.)).children(fields).children(err).child(buttons)
    }

    fn kind_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let one = |k: Kind, label: &str, cx: &mut Context<Self>| {
            let id = format!("seat-kind-{}", k.wire());
            let el = if self.form.kind == k { primary_button(id, label, cx) } else { chip(id, label, cx) };
            el.on_click(cx.listener(move |this, _, _, cx| {
                this.form.set_kind(k, cx);
                cx.notify();
            }))
        };
        let sub = one(Kind::Subscription, "Subscription", cx);
        let key = one(Kind::ApiKey, "API key", cx);
        div().flex().gap(px(8.)).child(sub).child(key).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f<'a>(label: &'a str, place: &'a str, budget: &'a str, window: &'a str, order: &'a str) -> Fields<'a> {
        Fields { label, place, budget, window, order }
    }

    #[test]
    fn env_var_name_pattern() {
        assert!(is_env_name("ANTHROPIC_API_KEY_WORK") && is_env_name("_K1"));
        assert!(!is_env_name("") && !is_env_name("1KEY") && !is_env_name("sk-ant-abc123") && !is_env_name("MY KEY"));
    }

    #[test]
    fn api_key_seat_takes_a_name_never_a_key() {
        let ok = seat_body(Kind::ApiKey, &f("Work key", "ANTHROPIC_API_KEY_WORK", "$20", "", "")).unwrap();
        assert_eq!((ok["id"].as_str(), ok["kind"].as_str(), ok["env_var"].as_str()), (Some("work-key"), Some("api_key"), Some("ANTHROPIC_API_KEY_WORK")));
        assert_eq!((ok["budget_usd"].as_f64(), ok["window_hours"].as_u64(), ok["order"].as_i64()), (Some(20.0), Some(5), Some(0)));
        assert!(ok.get("config_dir").is_none());
        assert!(seat_body(Kind::ApiKey, &f("Work", "sk-ant-api03-abc", "", "", "")).is_err());
        assert!(seat_body(Kind::ApiKey, &f("Work", "", "", "", "")).is_err());
    }

    #[test]
    fn subscription_and_number_checks() {
        let ok = seat_body(Kind::Subscription, &f("Second login", "~/.claude-two", "", "8", "2")).unwrap();
        assert_eq!((ok["config_dir"].as_str(), ok["budget_usd"].is_null(), ok["window_hours"].as_u64()), (Some("~/.claude-two"), true, Some(8)));
        assert!(seat_body(Kind::Subscription, &f("Default", "", "", "", "")).unwrap().get("config_dir").is_none());
        assert!(seat_body(Kind::Subscription, &f("S", "rel/dir", "", "", "")).is_err());
        assert!(seat_body(Kind::Subscription, &f("  ", "", "", "", "")).is_err());
        assert!(seat_body(Kind::Subscription, &f("S", "", "-1", "", "")).is_err());
        assert!(seat_body(Kind::Subscription, &f("S", "", "", "0", "")).is_err());
        assert!(seat_body(Kind::Subscription, &f("S", "", "", "", "x")).is_err());
    }

    #[test]
    fn id_comes_from_the_label() {
        assert_eq!(seat_id(" Work  Key #2 "), "work-key-2");
        assert_eq!(seat_id("!!"), "");
    }
}
