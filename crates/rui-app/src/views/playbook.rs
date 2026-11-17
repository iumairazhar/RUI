//! Playbook page: lessons in force, by scope.

use super::section::{Sec, section_top};
use crate::store::StoreHandle;
use crate::theme::{Theme, theme};
use gpui::{AnyElement, App, Div, FontWeight, SharedString, div, prelude::*, px};

/// One static scope chip (mock `.seg`): green when selected.
fn scope_chip(id: SharedString, name: &str, on: bool, th: &Theme) -> Div {
    let (bg, fg) = if on { (th.accent_fill, th.on_accent) } else { (gpui::transparent_black(), th.fg_2) };
    let count = div().font_family("Menlo").text_size(px(10.5)).opacity(0.7).child("0");
    let chip = div().id(id).flex().items_center().gap(px(5.)).h(px(24.)).px(px(10.)).rounded(px(6.));
    let chip = chip.bg(bg).text_color(fg).text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD);
    div().child(chip.child(name.to_string()).child(count))
}

/// The segment group: "Everywhere" first, then one chip per client.
fn scope_row(names: &[String], th: &Theme) -> Div {
    let first = scope_chip("playbook-scope-all".into(), "Everywhere", true, th);
    let rest = names.iter().enumerate().map(|(i, n)| scope_chip(format!("playbook-scope-{i}").into(), n, false, th));
    let group = div().id("playbook-scopes").flex().gap(px(2.)).p(px(3.)).rounded(px(9.)).bg(th.well);
    div().flex().child(group.child(first).children(rest))
}

/// The Playbook tab of the Knowledge section.
pub fn page(cx: &App) -> AnyElement {
    let th = theme(cx);
    let names: Vec<String> = cx.global::<StoreHandle>().0.read(cx).clients.iter().map(|c| c.name.clone()).collect();
    let text = "0 lessons in force. Every one came from a correction you made or a decision you took \u{2014} never from an agent deciding it had learned something.";
    let lead = div().id("playbook-lead").max_w(px(620.)).text_size(px(14.)).text_color(th.fg_2).child(text);
    let note = "Nothing has been learned at this scope yet. Lessons appear here when you correct something.";
    let note = div().id("playbook-note").mt(px(16.)).text_size(px(13.)).text_color(th.muted).child(note);
    let top = section_top(Sec::Knowledge, Some(lead.into_any_element()), None, cx);
    div().id("playbook-page").flex().flex_col().size_full().pr(px(40.)).child(top).child(scope_row(&names, th)).child(note).into_any_element()
}
