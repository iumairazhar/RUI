//! The agent drawer: head, six tabs and the footer buttons.

use super::{AgentsView, LIMITS, Tab, attachable, owns_section, pods_with};
use crate::api::RoleInfo;
use crate::theme::Theme;
use crate::views::activity::section;
use crate::views::library::{self as lib, EditActs, ROLE_TOOLS, Tooltip};
use crate::views::{chip, markdown, primary_button};
use gpui::{AnyElement, App, ClickEvent, Context, FontWeight, SharedString, Window, div, prelude::*, px};

const TABS: [(Tab, &str); 6] = [
    (Tab::Overview, "Overview"),
    (Tab::Edit, "Edit"),
    (Tab::Skills, "Skills"),
    (Tab::Tools, "Tools"),
    (Tab::Limit, "Loop limit"),
    (Tab::History, "History"),
];
const EDIT_HINT: &str = "Name, title and description are required. The description tells Rui when to pick this agent.";
const LEAD_TIP: &str = "The Lead heads every pod";

fn text(s: String, th: &Theme) -> AnyElement {
    div().text_size(px(12.5)).text_color(th.fg_2).line_height(px(18.)).child(s).into_any_element()
}

fn tip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    move |_, cx| cx.new(|_| Tooltip(SharedString::from(text))).into()
}

impl AgentsView {
    pub(super) fn drawer(&mut self, d: &RoleInfo, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let eyebrow = format!("{} \u{b7} {}", d.code, super::super::roster::capitalize(&d.author));
        let close = lib::close_button("agents-drawer-close", Self::close, th, cx);
        let head = lib::drawer_top(&eyebrow, &d.title, close, &d.status, true, th);
        let tabs = lib::tab_bar("agents-tabs", &TABS, self.tab, Self::set_tab, th, cx);
        let body = lib::scroll_body("agents-drawer-scroll", self.tab_body(d, th, cx));
        let foot = self.foot(d, th, cx);
        lib::drawer_shell("agents-drawer", th).child(head).child(tabs).child(body).child(foot).into_any_element()
    }

    fn tab_body(&mut self, d: &RoleInfo, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match self.tab {
            Tab::Overview => self.overview(d, th, cx),
            Tab::Edit => {
                let acts = EditActs { save: Self::save, preview: Self::toggle_preview, discard: Self::discard };
                self.edit.view(EDIT_HINT, acts, th, cx)
            }
            Tab::Skills => self.skills_tab(d, th, cx),
            Tab::Tools => {
                let grid = lib::tool_grid("agent-tools", &ROLE_TOOLS, &d.tools, Self::toggle_tool, th, cx);
                vec![section("Allowed tools", vec![grid]), lib::note("These decide what the agent may do.", th).into_any_element()]
            }
            Tab::Limit => self.limit_tab(d, th, cx),
            Tab::History => lib::history_tab(&d.hist, &d.versions, Self::ask_restore, th, cx),
        }
    }

    fn overview(&self, d: &RoleInfo, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let owns = owns_section(&d.text);
        let owns = if owns.is_empty() { text("Nothing written yet.".into(), th) } else { markdown::render(&owns, th.fg_2, 12.5, th) };
        let s = self.store.read(cx);
        let pods: Vec<AnyElement> = pods_with(&d.name, &s.tasks)
            .iter()
            .map(|p| {
                let project = s.clients.iter().find(|c| c.id == p.client_id).map(|c| format!(" \u{b7} {}", c.name)).unwrap_or_default();
                text(format!("{}{project}", p.title), th)
            })
            .collect();
        let pods = if pods.is_empty() { vec![text("Not in any pod right now.".into(), th)] } else { pods };
        vec![
            section("What it does", vec![text(d.description.clone(), th)]),
            section("What it owns", vec![owns]),
            section("In pods now", pods),
        ]
    }

    fn skills_tab(&self, d: &RoleInfo, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let all = self.store.read(cx).skills.clone();
        let have: Vec<AnyElement> = d.skills.iter().enumerate().map(|(i, k)| self.skill_chip(i, k, th, cx)).collect();
        let have = if have.is_empty() { vec![lib::note("No skills attached.", th).into_any_element()] } else { have };
        let picks: Vec<AnyElement> = attachable(&all, d).iter().enumerate().map(|(i, k)| self.pick_chip(i, &k.name, cx)).collect();
        let picks = if picks.is_empty() { vec![lib::note("Every skill that is on is already attached.", th).into_any_element()] } else { picks };
        let wrap = |items: Vec<AnyElement>| div().flex().flex_wrap().gap(px(8.)).children(items).into_any_element();
        vec![section("Attached skills", vec![wrap(have)]), section("Attach a skill", vec![wrap(picks)])]
    }

    fn skill_chip(&self, i: usize, name: &str, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let skill = name.to_string();
        let label = div().font_family("Menlo").text_size(px(11.5)).text_color(th.fg_2).child(name.to_string());
        let x = div().id(("agent-skill-x", i)).cursor_pointer().px(px(4.)).text_size(px(13.)).text_color(th.muted).child("\u{d7}");
        let x = x.on_click(cx.listener(move |this, _, _, cx| this.set_skill(skill.clone(), false, cx)));
        let pill = div().flex().items_center().gap(px(6.)).h(px(26.)).pl(px(10.)).pr(px(6.)).rounded(px(7.));
        pill.bg(th.well).border_1().border_color(th.hairline).child(label).child(x).into_any_element()
    }

    fn pick_chip(&self, i: usize, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let skill = name.to_string();
        let btn = chip(format!("agent-pick-{i}"), &format!("+ {name}"), cx).h(px(26.));
        btn.on_click(cx.listener(move |this, _, _, cx| this.set_skill(skill.clone(), true, cx))).into_any_element()
    }

    fn limit_tab(&self, d: &RoleInfo, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let seg: Vec<AnyElement> = LIMITS
            .iter()
            .map(|n| {
                let (n, on) = (*n, *n == d.max_loops);
                let (bg, fg) = if on { (th.accent_fill, th.on_accent) } else { (gpui::transparent_black(), th.muted) };
                let el = div().id(("agent-limit", n as usize)).flex().flex_1().items_center().justify_center().h(px(30.));
                let el = el.rounded(px(7.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).cursor_pointer().bg(bg).text_color(fg);
                el.on_click(cx.listener(move |this, _, _, cx| this.set_limit(n, cx))).child(n.to_string()).into_any_element()
            })
            .collect();
        let bar = div().flex().gap(px(2.)).p(px(3.)).rounded(px(9.)).border_1().border_color(th.hairline).max_w(px(320.)).children(seg);
        let copy = "A task escalates to its Lead rather than looping past this.";
        vec![section("Loop limit", vec![bar.into_any_element()]), lib::note(copy, th).into_any_element()]
    }

    fn turn_on(cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, _, _, cx| this.set_status("on", "Turned on", "Rui can staff it in pods", cx))
    }

    fn foot(&self, d: &RoleInfo, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let lead = d.name == "lead";
        let mut btns: Vec<AnyElement> = Vec::new();
        if d.status == "on" {
            let off = chip("agent-off", "Turn off", cx);
            let off = match lead {
                true => off.opacity(0.4).tooltip(tip(LEAD_TIP)),
                false => off.on_click(cx.listener(|this, _, _, cx| this.set_status("off", "Turned off", "", cx))),
            };
            btns.push(off.into_any_element());
        } else {
            let on = Self::turn_on(cx);
            btns.push(primary_button("agent-on", "Turn on", cx).on_click(on).into_any_element());
        }
        let del = lib::danger_button("agent-delete", "Delete", !lead, th).ml_auto();
        let del = match lead {
            true => del.tooltip(tip(LEAD_TIP)),
            false => del.on_click(cx.listener(|this, _, _, cx| this.ask_delete(cx))),
        };
        btns.push(del.into_any_element());
        lib::drawer_foot(btns, None, th)
    }
}
