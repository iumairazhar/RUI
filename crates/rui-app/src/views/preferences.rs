//! Preferences tab of the Knowledge section: the owner's preferences file, shown as
//! markdown, with an editor to change or remove lines.

use super::library::{self as lib, run, toast};
use super::section::{Sec, section_top};
use super::clients::form_buttons;
use super::{chip, markdown, primary_button};
use crate::api;
use crate::store::Store;
use crate::text_editor::{Save, TextEditor};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Entity, Render, Window, div, prelude::*, px};
use serde_json::Value;

const EMPTY: &str = "Nothing saved yet. Tell Rui how you like to work, or add lines here.";
/// Shown under a memory file that is over 80% of what reaches the agents.
pub const FULL_NOTE: &str = "Getting full: only the newest 4,000 characters reach the agents. Trim old lines, or ask for search.";

/// True when `chars` is over 80% of `limit`.
pub fn getting_full(chars: u64, limit: u64) -> bool {
    limit > 0 && chars * 5 > limit * 4
}

/// True when a `{text, chars, limit}` reply is getting full.
pub fn reply_full(v: &Value) -> bool {
    let num = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    getting_full(num("chars"), num("limit"))
}

fn reply_text(v: &Value) -> String {
    v.get("text").and_then(Value::as_str).unwrap_or("").to_string()
}

/// The owner's preferences page.
pub struct PreferencesView {
    store: Entity<Store>,
    /// Current file; `None` while loading.
    text: Option<String>,
    full: bool,
    editing: bool,
    busy: bool,
    editor: Entity<TextEditor>,
}

impl PreferencesView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| TextEditor::new("", cx));
        cx.subscribe(&editor, |this, _, _: &Save, cx| this.save(cx)).detach();
        let mut view = Self { store, text: None, full: false, editing: false, busy: false, editor };
        view.load(cx);
        view
    }

    /// Reload the file from the daemon (not while the owner is editing it).
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if self.editing {
            return;
        }
        run(cx, api::get_preferences, |v, res, cx| v.loaded(res, cx));
    }

    fn loaded(&mut self, res: Result<Value, String>, cx: &mut Context<Self>) {
        let v = res.unwrap_or_default();
        (self.text, self.full) = (Some(reply_text(&v)), reply_full(&v));
        cx.notify();
    }

    fn set_editing(&mut self, on: bool, cx: &mut Context<Self>) {
        self.editing = on;
        let text = if on { self.text.clone().unwrap_or_default() } else { String::new() };
        self.editor.update(cx, |e, cx| e.set_text(&text, cx));
        cx.notify();
    }

    /// Replace the whole file with the editor's text.
    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.editing || self.busy {
            return;
        }
        let text = self.editor.read(cx).text();
        self.busy = true;
        run(cx, move || api::put_preferences(&text), |v, res, cx| v.saved(res, cx));
        cx.notify();
    }

    fn saved(&mut self, res: Result<Value, String>, cx: &mut Context<Self>) {
        self.busy = false;
        match res {
            Ok(v) => {
                self.set_editing(false, cx);
                self.loaded(Ok(v), cx);
                toast(&self.store, "Preferences saved", "", cx);
            }
            Err(e) => toast(&self.store, "Could not save them", &e, cx),
        }
        cx.notify();
    }

    fn current(&self, th: &Theme) -> AnyElement {
        let Some(text) = self.text.as_deref() else { return lib::note("Loading\u{2026}", th).into_any_element() };
        if text.trim().is_empty() {
            return lib::note(EMPTY, th).into_any_element();
        }
        let md = div().p(px(16.)).rounded(px(10.)).bg(th.well).child(markdown::render(text.trim(), th.fg_2, 12.5, th));
        div().flex().flex_col().gap(px(8.)).child(md).children(self.full.then(|| lib::note(FULL_NOTE, th))).into_any_element()
    }

    fn editor_box(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let boxed = div().id("prefs-box").h(px(360.)).rounded(px(10.)).overflow_hidden().border_1().border_color(th.hairline);
        let label = if self.busy { "Saving\u{2026}" } else { "Save" };
        let save = primary_button("prefs-save", label, cx).on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        let cancel = chip("prefs-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| this.set_editing(false, cx)));
        let row = form_buttons(save, cancel).child(lib::note("Or press \u{2318}S", th));
        div().flex().flex_col().gap(px(10.)).child(boxed.child(self.editor.clone())).child(row).into_any_element()
    }
}

impl Render for PreferencesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let lead = "How you like to work. Rui, the Account Leads and every pod member read this. Lines marked (Rui, from chat) were saved by Rui; your own lines win.";
        let lead = div().max_w(px(620.)).text_size(px(13.)).text_color(th.fg_2).child(lead);
        let edit = (!self.editing).then(|| chip("prefs-edit", "Edit", cx).on_click(cx.listener(|this, _, _, cx| this.set_editing(true, cx))).into_any_element());
        let body = if self.editing { self.editor_box(&th, cx) } else { self.current(&th) };
        let page = div().id("prefs-page").flex().flex_col().size_full().overflow_y_scroll().pr(px(40.)).pb(px(28.));
        page.child(section_top(Sec::Knowledge, Some(lead.into_any_element()), edit, cx)).child(div().max_w(px(720.)).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_only_over_eighty_percent() {
        assert!(!getting_full(3200, 4000));
        assert!(getting_full(3201, 4000));
        assert!(!getting_full(10, 0));
        assert!(reply_full(&json!({"text": "x", "chars": 3900, "limit": 4000})));
        assert!(!reply_full(&json!({"text": "x"})));
    }
}
