//! Pieces the Skills and Agents drawers share: history, edit tab, tool grid, tabs, confirm and slide-in.

use super::activity::{DRAWER_MS, DRAWER_TRAVEL, SCRIM_MS, icon, section, shown};
use super::motion::{self, ease, spring};
use super::{chip, markdown, tag};
use crate::api::{Hist, Version};
use crate::store::Store;
use crate::text_editor::{Save, TextEditor};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Status, Theme, theme};
use gpui::{
    AnimationExt, AnyElement, App, Context, Div, ElementId, Entity, FontWeight, Render, SharedString, Stateful,
    Window, div, prelude::*, px, relative,
};
use std::time::Duration;

/// Tools a skill may declare, in the mock's order.
pub const SKILL_TOOLS: [&str; 7] = ["Read", "Search", "Edit", "Run tests", "Shell", "Fetch a URL", "Browser"];
/// Tool groups of a role.
pub const ROLE_TOOLS: [&str; 6] = ["Read", "Search", "Edit", "Shell", "Run tests", "Fetch URL"];

/// Tooltip bubble.
pub struct Tooltip(pub SharedString);

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx);
        let bubble = div().px(px(10.)).py(px(6.)).rounded(px(8.)).bg(th.ctl);
        bubble.text_size(px(12.)).text_color(th.fg).child(self.0.clone())
    }
}

// ---- pure helpers ----

/// (year, month, day) of a day count since 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// "2023-11-14 22:13" (UTC) for unix milliseconds.
pub fn stamp(ms: u64) -> String {
    let (y, m, d) = civil((ms / 86_400_000) as i64);
    let mins = ms / 60_000 % 1440;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", mins / 60, mins % 60)
}

/// "2026-11-16 10:45 · You edited the SKILL.md".
pub fn hist_title(h: &Hist) -> String {
    format!("{} \u{b7} {} {}", h.when, h.who, h.what)
}

/// "Why: tidy · From: https://..." or `None` when there is neither.
pub fn hist_detail(h: &Hist) -> Option<String> {
    let why = h.why.as_ref().map(|w| format!("Why: {w}"));
    let from = h.from.as_ref().map(|f| format!("From: {f}"));
    let parts: Vec<String> = [why, from].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// The daemon logs oldest first; the drawer shows newest first.
pub fn newest_first(hist: &[Hist]) -> Vec<&Hist> {
    hist.iter().rev().collect()
}

/// Frontmatter as (key, value) rows plus the trimmed body below it.
pub fn preview_parts(text: &str) -> (Vec<(String, String)>, String) {
    let plain = || (Vec::new(), text.trim().to_string());
    let Some(rest) = text.strip_prefix("---\n") else { return plain() };
    let Some(end) = rest.find("\n---") else { return plain() };
    let row = |l: &str| l.split_once(": ").map(|(k, v)| (k.trim().to_string(), v.trim().to_string()));
    let fields = rest[..end].lines().filter_map(row).filter(|(_, v)| !v.is_empty()).collect();
    (fields, rest[end + 4..].trim().to_string())
}

/// The list without `item` when it is in, else with it added at the end.
pub fn toggled(list: &[String], item: &str) -> Vec<String> {
    if list.iter().any(|t| t == item) {
        return list.iter().filter(|t| *t != item).cloned().collect();
    }
    list.iter().cloned().chain([item.to_string()]).collect()
}

/// "On", "Off" or "Needs review".
pub fn status_word(status: &str) -> &'static str {
    match status {
        "on" => "On",
        "review" => "Needs review",
        _ => "Off",
    }
}

/// One line saying what the status means.
pub fn status_note(status: &str, agent: bool) -> &'static str {
    match (status, agent) {
        ("on", false) => "Agents use this skill",
        ("review", false) => "Imported skills stay off until you review them",
        (_, false) => "Agents don\u{2019}t use this skill",
        ("on", true) => "Pods can use this agent",
        (_, true) => "Rui does not offer this agent to pods",
    }
}

/// On is the run family, review the wait family, off the done family.
pub fn status_color<'a>(status: &str, th: &'a Theme) -> &'a Status {
    match status {
        "on" => &th.run,
        "review" => &th.wait,
        _ => &th.done,
    }
}

/// Colored status tag.
pub fn status_tag(status: &str, th: &Theme) -> Div {
    tag(status_color(status, th), status_word(status))
}

/// A table cell of fixed width holding the status tag.
pub fn status_cell(status: &str, width: f32, th: &Theme) -> Div {
    div().flex().flex_none().w(px(width)).child(status_tag(status, th))
}

// ---- building blocks ----

/// Run a daemon call in the background, then hand its result (or the error text) to `done`.
pub fn run<V: 'static, T: Send + 'static>(
    cx: &mut Context<V>,
    call: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    done: impl FnOnce(&mut V, Result<T, String>, &mut Context<V>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let res = cx.background_executor().spawn(async move { call() }).await;
        this.update(cx, |v, cx| done(v, res.map_err(|e| e.to_string()), cx)).ok();
    })
    .detach();
}

/// Show a toast with at most one chip.
pub fn toast<V: 'static>(store: &Entity<Store>, title: &str, chip: &str, cx: &mut Context<V>) {
    let chips = if chip.is_empty() { vec![] } else { vec![chip.to_string()] };
    store.update(cx, |s, cx| s.show_toast(title, chips, cx));
}

/// Run `done` once the drawer's slide-out has finished.
pub fn after_slide<V: 'static>(cx: &mut Context<V>, done: fn(&mut V, &mut Context<V>)) {
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(Duration::from_millis(DRAWER_MS)).await;
        this.update(cx, done).ok();
    })
    .detach();
}

/// Right-hand drawer container.
pub fn drawer_shell(id: &'static str, th: &Theme) -> Stateful<Div> {
    let side = div().id(id).occlude().absolute().right(px(10.)).top(px(10.)).bottom(px(10.));
    let side = side.w(relative(0.52)).max_w(px(620.)).flex().flex_col().overflow_hidden().rounded(px(16.));
    side.bg(th.surface).shadow(th.card_shadow()).border_1().border_color(th.glass_rim)
}

/// Dimmed scrim plus the drawer sliding in, or out when `closing`.
pub fn slide_overlay(id: &'static str, scrim: Stateful<Div>, drawer: AnyElement, closing: bool) -> AnyElement {
    let out = closing;
    let scrim_id = (SharedString::from(format!("{id}-scrim")), out as usize);
    let scrim = scrim.with_animation(scrim_id, motion::once(SCRIM_MS), move |el, d| el.opacity(shown(out, d)));
    let slide = div().absolute().top_0().bottom_0().left_0().right_0().child(drawer);
    let slide_id = (SharedString::from(format!("{id}-slide")), out as usize);
    let slide = slide.with_animation(slide_id, motion::once(DRAWER_MS), move |el, d| {
        let off = DRAWER_TRAVEL * (1. - shown(out, d));
        el.left(px(off)).right(px(-off))
    });
    let root = div().id(SharedString::from(format!("{id}-overlay")));
    root.absolute().top_0().left_0().right_0().bottom_0().child(scrim).child(slide).into_any_element()
}

/// Dimmed click-away layer behind a drawer.
pub fn scrim<V: 'static>(id: &'static str, on: fn(&mut V, &mut Context<V>), cx: &mut Context<V>) -> Stateful<Div> {
    let layer = div().id(id).absolute().top_0().left_0().right_0().bottom_0().bg(gpui::black().opacity(0.3));
    layer.on_click(cx.listener(move |this, _, _, cx| on(this, cx)))
}

/// Square close button.
pub fn close_button<V: 'static>(id: &'static str, on: fn(&mut V, &mut Context<V>), th: &Theme, cx: &mut Context<V>) -> AnyElement {
    let hover = th.surface_2;
    let btn = div().id(id).flex().flex_none().items_center().justify_center().size(px(34.)).rounded(px(9.));
    btn.bg(th.ctl)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .on_click(cx.listener(move |this, _, _, cx| on(this, cx)))
        .child(icon("close", 16., th.fg_2))
        .into_any_element()
}

/// Drawer eyebrow, title, close button and the status line.
pub fn drawer_top(eyebrow: &str, title: &str, close: AnyElement, status: &str, agent: bool, th: &Theme) -> Div {
    let eyebrow = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(eyebrow.to_uppercase());
    let name = div().text_size(px(23.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(title.to_string());
    let left = div().flex().flex_col().gap(px(2.)).min_w_0().child(eyebrow).child(name);
    let top = div().flex().items_start().justify_between().gap(px(12.)).child(left).child(close);
    let note = div().text_size(px(12.)).text_color(th.muted).child(status_note(status, agent));
    let line = div().flex().items_center().gap(px(8.)).mt(px(12.)).child(status_tag(status, th)).child(note);
    div().flex().flex_col().flex_none().px(px(20.)).pt(px(20.)).child(top).child(line)
}

/// Equal-width tab bar like the Threads drawer's.
pub fn tab_bar<V: 'static, T: Copy + PartialEq + 'static>(
    id: &'static str,
    tabs: &[(T, &'static str)],
    cur: T,
    on: fn(&mut V, T, &mut Context<V>),
    th: &Theme,
    cx: &mut Context<V>,
) -> AnyElement {
    let items: Vec<AnyElement> = tabs
        .iter()
        .enumerate()
        .map(|(i, (tab, name))| {
            let (tab, on_it) = (*tab, *tab == cur);
            let (bg, fg) = if on_it { (th.accent_fill, th.on_accent) } else { (gpui::transparent_black(), th.muted) };
            let el = div().id((SharedString::from(format!("{id}-tab")), i)).flex().flex_1().items_center().justify_center();
            el.h(px(28.)).rounded(px(7.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).cursor_pointer().bg(bg).text_color(fg)
                .on_click(cx.listener(move |this, _, _, cx| on(this, tab, cx)))
                .child(*name)
                .into_any_element()
        })
        .collect();
    let bar = div().id(id).flex().gap(px(2.)).p(px(3.)).rounded(px(9.)).border_1().border_color(th.hairline);
    div().px(px(20.)).pt(px(16.)).pb(px(8.)).child(bar.children(items)).into_any_element()
}

/// Scrolling body of a drawer.
pub fn scroll_body(id: &'static str, children: Vec<AnyElement>) -> Stateful<Div> {
    let body = div().id(id).flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
    body.gap(px(20.)).p(px(20.)).children(children)
}

/// Footer strip holding the drawer's buttons.
pub fn drawer_foot(buttons: Vec<AnyElement>, hint: Option<&str>, th: &Theme) -> AnyElement {
    let row = div().flex().flex_wrap().items_center().gap(px(8.)).children(buttons);
    let hint = hint.map(|h| div().text_size(px(11.5)).text_color(th.muted).child(h.to_string()));
    let foot = div().flex().flex_col().flex_none().gap(px(8.)).px(px(20.)).py(px(14.));
    foot.border_t_1().border_color(th.hairline).child(row).children(hint).into_any_element()
}

/// Red button for destructive actions; `enabled` false dims it and makes it inert.
pub fn danger_button(id: impl Into<SharedString>, label: &str, enabled: bool, th: &Theme) -> Stateful<Div> {
    let b = div().id(id.into()).flex().items_center().h(px(30.)).px(px(12.)).rounded(px(7.));
    let b = b.text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).bg(th.block.dim).text_color(th.block.fg);
    let b = b.border_1().border_color(th.block.rim).child(label.to_string());
    if enabled { b.cursor_pointer() } else { b.opacity(0.4) }
}

/// Check box row; the caller adds the click handler.
pub fn check_row(id: impl Into<ElementId>, label: String, on: bool, th: &Theme) -> Stateful<Div> {
    let (bg, ring) = if on { (th.accent_fill, th.accent_fill) } else { (gpui::transparent_black(), th.muted) };
    let tick = on.then(|| icon("check", 11., th.on_accent));
    let boxed = div().flex().flex_none().items_center().justify_center().size(px(16.)).rounded(px(5.));
    let boxed = boxed.bg(bg).border_1().border_color(ring).children(tick);
    let name = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(label);
    let row = div().id(id).flex().items_center().gap(px(10.)).px(px(12.)).py(px(10.)).rounded(px(10.));
    let hover = th.surface_2;
    row.bg(th.well).cursor_pointer().hover(move |s| s.bg(hover)).child(boxed).child(name)
}

/// Grid of tool toggles; `toggle` gets the tool name.
pub fn tool_grid<V: 'static>(
    id: &'static str,
    all: &[&'static str],
    on: &[String],
    toggle: fn(&mut V, String, &mut Context<V>),
    th: &Theme,
    cx: &mut Context<V>,
) -> AnyElement {
    let rows: Vec<AnyElement> = all
        .iter()
        .enumerate()
        .map(|(i, tool)| {
            let on_it = on.iter().any(|t| t == tool);
            let row = check_row((SharedString::from(format!("{id}-tool")), i), tool.to_string(), on_it, th);
            let name = tool.to_string();
            let row = row.on_click(cx.listener(move |this, _, _, cx| toggle(this, name.clone(), cx)));
            div().w(relative(0.5)).p(px(3.)).child(row).into_any_element()
        })
        .collect();
    div().id(id).flex().flex_wrap().children(rows).into_any_element()
}

/// Muted explanatory line.
pub fn note(text: &str, th: &Theme) -> Div {
    div().text_size(px(11.5)).text_color(th.muted).child(text.to_string())
}

/// The History tab: entries newest first, then older versions to restore.
pub fn history_tab<V: 'static>(
    hist: &[Hist],
    versions: &[Version],
    restore: fn(&mut V, u64, &mut Context<V>),
    th: &Theme,
    cx: &mut Context<V>,
) -> Vec<AnyElement> {
    let entry = |h: &Hist| {
        let title = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(hist_title(h));
        let sub = hist_detail(h).map(|d| note(&d, th));
        div().flex().flex_col().gap(px(2.)).child(title).children(sub).into_any_element()
    };
    let entries: Vec<AnyElement> = newest_first(hist).into_iter().map(entry).collect();
    let mut out = vec![section("History", entries)];
    if versions.is_empty() {
        return out;
    }
    let rows: Vec<AnyElement> = versions.iter().map(|v| version_row(v.ts, restore, th, cx)).collect();
    out.push(section("Older versions", rows));
    out
}

fn version_row<V: 'static>(ts: u64, restore: fn(&mut V, u64, &mut Context<V>), th: &Theme, cx: &mut Context<V>) -> AnyElement {
    let when = div().flex_1().text_size(px(12.5)).text_color(th.fg_2).child(stamp(ts));
    let btn = chip(format!("restore-{ts}"), "Restore this version", cx);
    let btn = btn.on_click(cx.listener(move |this, _, _, cx| restore(this, ts, cx)));
    div().flex().items_center().gap(px(10.)).child(when).child(btn).into_any_element()
}

/// Small modal with Cancel and a confirm button, popping in over a dimmed layer.
#[allow(clippy::too_many_arguments)]
pub fn confirm_modal<V: 'static>(
    title: &str,
    body: &str,
    label: &str,
    danger: bool,
    cancel: fn(&mut V, &mut Context<V>),
    accept: fn(&mut V, &mut Context<V>),
    th: &Theme,
    cx: &mut Context<V>,
) -> AnyElement {
    let ghost = chip("confirm-cancel", "Cancel", cx).on_click(cx.listener(move |this, _, _, cx| cancel(this, cx)));
    let yes = if danger {
        danger_button("confirm-yes", label, true, th)
    } else {
        super::primary_button("confirm-yes", label, cx)
    };
    let yes = yes.on_click(cx.listener(move |this, _, _, cx| accept(this, cx)));
    let head = div().text_size(px(15.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(title.to_string());
    let text = div().text_size(px(12.5)).text_color(th.fg_2).child(body.to_string());
    let acts = div().flex().justify_end().gap(px(8.)).mt(px(6.)).child(ghost).child(yes);
    let boxed = div().id("confirm-box").occlude().w(px(380.)).flex().flex_col().gap(px(8.)).p(px(20.)).rounded(px(14.));
    let boxed = boxed.bg(th.surface).border_1().border_color(th.glass_rim).shadow(th.card_shadow());
    let boxed = boxed.child(head).child(text).child(acts);
    let boxed = boxed.with_animation("confirm-pop", motion::once(260), |el, d| {
        el.relative().top(px(10. * (1. - spring(d)))).opacity(ease(d))
    });
    let layer = div().id("confirm-layer").occlude().absolute().top_0().left_0().right_0().bottom_0();
    let layer = layer.flex().items_center().justify_center().bg(gpui::black().opacity(0.45));
    layer.on_click(cx.listener(move |this, _, _, cx| cancel(this, cx))).child(boxed).into_any_element()
}

// ---- edit tab ----

/// The editor, the required reason, and the flags the edit tab shows.
pub struct EditState {
    pub editor: Entity<TextEditor>,
    pub why: Entity<TextInput>,
    pub preview: bool,
    /// Error text from the daemon, shown under the reason.
    pub error: Option<String>,
    pub why_missing: bool,
    file: &'static str,
}

impl EditState {
    /// Build the editor and reason input; cmd-S and Enter in the reason call `save`.
    pub fn new<V: 'static>(file: &'static str, save: fn(&mut V, &mut Context<V>), cx: &mut Context<V>) -> Self {
        let editor = cx.new(|cx| TextEditor::new("", cx));
        let why = cx.new(|cx| TextInput::new("Why are you changing it? (required)", cx));
        cx.subscribe(&editor, move |this, _, _: &Save, cx| save(this, cx)).detach();
        cx.subscribe(&why, move |this, _, _: &Submit, cx| save(this, cx)).detach();
        Self { editor, why, preview: false, error: None, why_missing: false, file }
    }

    /// Put `text` in the editor as the clean copy and reset the reason and flags.
    pub fn load(&mut self, text: &str, cx: &mut App) {
        self.editor.update(cx, |e, cx| {
            e.set_text(text, cx);
            e.mark_clean();
        });
        self.why.update(cx, |w, cx| w.clear(cx));
        (self.preview, self.error, self.why_missing) = (false, None, false);
    }

    /// After a successful save: the text is clean and the reason is spent.
    pub fn saved(&mut self, cx: &mut App) {
        self.editor.update(cx, |e, _| e.mark_clean());
        self.why.update(cx, |w, cx| w.clear(cx));
        (self.error, self.why_missing) = (None, false);
    }

    /// Current text of the editor.
    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).text()
    }

    /// Current reason, trimmed.
    pub fn reason(&self, cx: &App) -> String {
        self.why.read(cx).content().trim().to_string()
    }

    /// The edit tab body: editor or preview, hint, reason and the three buttons.
    pub fn view<V: 'static>(&self, hint: &str, acts: EditActs<V>, th: &Theme, cx: &mut Context<V>) -> Vec<AnyElement> {
        let body = match self.preview {
            true => preview_view(&self.text(cx), th),
            false => self.editor_box(th),
        };
        let field = div().px(px(12.)).py(px(8.)).rounded(px(9.)).bg(th.well).border_1().border_color(th.hairline);
        let field = field.child(self.why.clone());
        let flag = |text: &str| div().text_size(px(12.)).text_color(th.block.fg).child(text.to_string());
        let missing = self.why_missing.then(|| flag("Add a reason first"));
        let error = self.error.as_ref().map(|e| flag(e));
        let why = div().flex().flex_col().gap(px(6.)).child(field).children(missing).children(error);
        let first = section(self.file, vec![body, note(hint, th).into_any_element()]);
        vec![first, section("Why", vec![why.into_any_element()]), self.buttons(acts, cx)]
    }

    fn editor_box(&self, th: &Theme) -> AnyElement {
        let boxed = div().id("edit-box").h(px(340.)).rounded(px(10.)).overflow_hidden().border_1().border_color(th.hairline);
        boxed.child(self.editor.clone()).into_any_element()
    }

    fn buttons<V: 'static>(&self, acts: EditActs<V>, cx: &mut Context<V>) -> AnyElement {
        let save = super::primary_button("edit-save", "Save", cx).on_click(cx.listener(move |t, _, _, cx| (acts.save)(t, cx)));
        let label = if self.preview { "Edit" } else { "Preview" };
        let prev = chip("edit-preview", label, cx).on_click(cx.listener(move |t, _, _, cx| (acts.preview)(t, cx)));
        let undo = chip("edit-discard", "Discard", cx).on_click(cx.listener(move |t, _, _, cx| (acts.discard)(t, cx)));
        div().flex().flex_wrap().gap(px(8.)).child(save).child(prev).child(undo).into_any_element()
    }
}

/// What the edit tab's buttons do.
pub struct EditActs<V: 'static> {
    pub save: fn(&mut V, &mut Context<V>),
    pub preview: fn(&mut V, &mut Context<V>),
    pub discard: fn(&mut V, &mut Context<V>),
}

impl<V: 'static> Clone for EditActs<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V: 'static> Copy for EditActs<V> {}

/// Frontmatter as label/value rows, then the body as markdown.
fn preview_view(text: &str, th: &Theme) -> AnyElement {
    let (fields, body) = preview_parts(text);
    let label = |k: &str| {
        let l = div().w(px(96.)).flex_none().pt(px(2.)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD);
        l.text_color(th.muted).child(k.to_uppercase())
    };
    let row = |(k, v): &(String, String)| {
        let value = div().flex_1().min_w_0().text_size(px(12.5)).text_color(th.fg_2).child(v.clone());
        div().flex().gap(px(12.)).child(label(k)).child(value)
    };
    let table = (!fields.is_empty()).then(|| {
        let rows = div().flex().flex_col().gap(px(6.)).children(fields.iter().map(row));
        div().pb(px(12.)).border_b_1().border_color(th.hairline).child(rows)
    });
    let card = div().id("edit-preview-box").flex().flex_col().gap(px(12.)).p(px(16.)).rounded(px(10.)).bg(th.well);
    card.children(table).child(markdown::render(&body, th.fg_2, 12.5, th)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(why: Option<&str>, from: Option<&str>) -> Hist {
        let (why, from) = (why.map(String::from), from.map(String::from));
        Hist { when: "2026-11-16 10:45".into(), who: "You".into(), what: "edited it".into(), why, from }
    }

    #[test]
    fn history_lines() {
        assert_eq!(hist_title(&hist(None, None)), "2026-11-16 10:45 \u{b7} You edited it");
        assert_eq!(hist_detail(&hist(Some("tidy"), None)).as_deref(), Some("Why: tidy"));
        assert_eq!(hist_detail(&hist(Some("tidy"), Some("a.b"))).as_deref(), Some("Why: tidy \u{b7} From: a.b"));
        assert_eq!(hist_detail(&hist(None, Some("a.b"))).as_deref(), Some("From: a.b"));
        assert_eq!(hist_detail(&hist(None, None)), None);
    }

    #[test]
    fn history_runs_newest_first() {
        let mut a = hist(None, None);
        a.what = "created it".into();
        let list = [a, hist(None, None)];
        assert_eq!(newest_first(&list)[0].what, "edited it");
    }

    #[test]
    fn stamps_are_utc() {
        assert_eq!(stamp(0), "1970-01-01 00:00");
        assert_eq!(stamp(1_700_000_000_000), "2023-11-14 22:13");
        assert_eq!(stamp(1_709_164_800_000 + 3_600_000), "2024-02-29 01:00");
    }

    #[test]
    fn preview_splits_frontmatter_from_body() {
        let (fields, body) = preview_parts("---\nname: x\ndescription: Use when asked\nempty:\n---\n# x\n\ntext\n");
        assert_eq!(fields, [("name".to_string(), "x".to_string()), ("description".to_string(), "Use when asked".to_string())]);
        assert_eq!(body, "# x\n\ntext");
        let (none, plain) = preview_parts("just text\n");
        assert!(none.is_empty());
        assert_eq!(plain, "just text");
        assert!(preview_parts("---\nname: x\nno end").0.is_empty());
    }

    #[test]
    fn toggling_adds_and_removes() {
        let list = vec!["Read".to_string(), "Edit".to_string()];
        assert_eq!(toggled(&list, "Edit"), ["Read"]);
        assert_eq!(toggled(&list, "Shell"), ["Read", "Edit", "Shell"]);
    }

    #[test]
    fn status_words() {
        assert_eq!((status_word("on"), status_word("review"), status_word("off")), ("On", "Needs review", "Off"));
        let th = Theme::dark();
        assert_eq!(status_color("review", &th).base, th.wait.base);
        assert_eq!(status_color("on", &th).base, th.run.base);
    }
}
