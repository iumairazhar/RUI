//! Skills: read-only list of SKILL.md files with a detail drawer.

use super::activity::{MONO, icon, muted_status, section};
use super::{markdown, page_head, tag};
use crate::api::SkillInfo;
use crate::store::{self, Store};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, FontWeight, Stateful, Window, div, prelude::*, px, relative};

const NOTE: &str = "Edit these files in ~/Library/Application Support/RUI/skills. Editing in the app comes later.";
const SKILL_W: f32 = 260.;
const RUN_W: f32 = 200.;

/// Skills page: a table of skills and a drawer for the open one.
pub struct SkillsView {
    store: Entity<Store>,
    open: Option<String>,
}

/// Bash patterns joined with " · ".
fn commands_text(skill: &SkillInfo) -> String {
    skill.bash.join(" \u{b7} ")
}

fn mono_cell(text: String, width: Option<f32>, th: &Theme) -> Div {
    let cell = div().font_family(MONO).text_size(px(11.)).text_color(th.fg_2).min_w_0().child(text);
    match width {
        Some(w) => cell.flex_none().w(px(w)),
        None => cell.flex_1(),
    }
}

fn head_cell(text: &str, width: Option<f32>, th: &Theme) -> Div {
    let cell = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(text.to_uppercase());
    match width {
        Some(w) => cell.flex_none().w(px(w)),
        None => cell.flex_1(),
    }
}

fn table_head(th: &Theme) -> Stateful<Div> {
    div()
        .id("skills-head")
        .flex()
        .gap(px(16.))
        .px(px(14.))
        .pb(px(8.))
        .child(head_cell("Skill", Some(SKILL_W), th))
        .child(head_cell("Commands", None, th))
        .child(head_cell("Run", Some(RUN_W), th))
}

fn name_cell(skill: &SkillInfo, th: &Theme) -> Div {
    let name = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(skill.name.clone());
    let desc = div().text_size(px(11.5)).text_color(th.muted).child(skill.description.clone());
    div().flex().flex_col().flex_none().w(px(SKILL_W)).gap(px(2.)).child(name).child(desc)
}

impl SkillsView {
    /// Build the skills view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self { store, open: None }
    }

    fn set_open(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.open = name;
        cx.notify();
    }

    fn row(&self, i: usize, skill: &SkillInfo, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let name = skill.name.clone();
        let run = skill.run_hint.clone().unwrap_or_else(|| "\u{2014}".into());
        let hover = th.surface_2;
        div()
            .id(("skills-row", i))
            .flex()
            .gap(px(16.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .bg(th.surface)
            .shadow(th.card_shadow())
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| this.set_open(Some(name.clone()), cx)))
            .child(name_cell(skill, th))
            .child(mono_cell(commands_text(skill), None, th))
            .child(mono_cell(run, Some(RUN_W), th))
            .into_any_element()
    }

    fn close_button(&self, th: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let hover = th.surface_2;
        div()
            .id("skills-drawer-close")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(34.))
            .rounded(px(9.))
            .bg(th.ctl)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(|this, _, _, cx| this.set_open(None, cx)))
            .child(icon("close", 16., th.fg_2))
    }

    fn drawer_head(&self, skill: &SkillInfo, th: &Theme, cx: &mut Context<Self>) -> Div {
        let eyebrow = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("SKILL");
        let top = div().flex().items_start().justify_between().child(eyebrow).child(self.close_button(th, cx));
        let title = div().text_size(px(23.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(skill.name.clone());
        div().flex().flex_col().flex_none().px(px(20.)).pt(px(20.)).pb(px(8.)).child(top).child(title)
    }

    fn drawer_body(&self, skill: &SkillInfo, th: &Theme) -> Stateful<Div> {
        let desc = div().text_size(px(12.5)).text_color(th.fg_2).child(skill.description.clone());
        let muted = muted_status(th);
        let tags: Vec<AnyElement> = skill.bash.iter().map(|b| tag(&muted, b).font_family(MONO).into_any_element()).collect();
        let tags = div().flex().flex_wrap().gap(px(6.)).children(tags).into_any_element();
        let body = markdown::render(&skill.body, th.fg_2, 12.5, th);
        div()
            .id("skills-drawer-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(20.))
            .p(px(20.))
            .child(section("When agents use it", vec![desc.into_any_element()]))
            .child(section("Commands it can run", vec![tags]))
            .child(section("SKILL.md", vec![body]))
    }

    fn drawer(&self, skill: &SkillInfo, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let scrim = div()
            .id("skills-scrim")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .bg(gpui::black().opacity(0.3))
            .on_click(cx.listener(|this, _, _, cx| this.set_open(None, cx)));
        let drawer = div()
            .id("skills-drawer")
            .occlude()
            .absolute()
            .right(px(10.))
            .top(px(10.))
            .bottom(px(10.))
            .w(relative(0.52))
            .max_w(px(620.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(16.))
            .bg(th.surface)
            .shadow(th.card_shadow())
            .border_1()
            .border_color(th.glass_rim)
            .child(self.drawer_head(skill, th, cx))
            .child(self.drawer_body(skill, th));
        div().id("skills-overlay").absolute().top_0().left_0().right_0().bottom_0().child(scrim).child(drawer).into_any_element()
    }
}

impl Render for SkillsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let skills = self.store.read(cx).skills.clone();
        let rows: Vec<AnyElement> = skills.iter().enumerate().map(|(i, s)| self.row(i, s, &th, cx)).collect();
        let open = self.open.as_ref().and_then(|n| skills.iter().find(|s| &s.name == n));
        let overlay = open.map(|s| self.drawer(s, &th, cx));
        let list = div().id("skills-list").flex().flex_col().gap(px(6.)).children(rows);
        let note = div().id("skills-note").mt(px(14.)).text_size(px(11.5)).text_color(th.muted).child(NOTE);
        let page = div()
            .id("skills-scroll")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .pr(px(40.))
            .pb(px(28.))
            .child(page_head("TEAM", "Skills", cx))
            .child(table_head(&th))
            .child(list)
            .child(note);
        div().id("skills-page").relative().size_full().child(page).children(overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_join_with_dot() {
        let s = SkillInfo { name: "x".into(), description: "d".into(), bash: vec!["uv *".into(), "pytest *".into()], run_hint: None, body: "b".into() };
        assert_eq!(commands_text(&s), "uv * \u{b7} pytest *");
    }
}
