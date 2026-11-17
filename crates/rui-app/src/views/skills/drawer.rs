//! The skill drawer: head, tabs, the five tab bodies and the footer buttons.

use super::table::{attached_label, author_label};
use super::{SkillsView, Tab};
use crate::api::{RoleInfo, SkillFile, SkillInfo};
use crate::theme::Theme;
use crate::views::activity::{icon, section};
use crate::views::library::{self as lib, EditActs, SKILL_TOOLS};
use crate::views::{chip, markdown, primary_button, tag};
use gpui::{AnyElement, App, ClickEvent, Context, FontWeight, Window, div, prelude::*, px};

const TABS: [(Tab, &str); 6] =
    [(Tab::Review, "Review"), (Tab::Overview, "Overview"), (Tab::Edit, "Edit"), (Tab::Attach, "Attach"), (Tab::Tools, "Tools"), (Tab::History, "History")];
const MAX_LINES: usize = 200;
const EDIT_HINT: &str = "Name and description are required. The description tells agents when to use the skill.";
const REVIEW_TEXT: &str =
    "Imported skills can include instructions and scripts you didn\u{2019}t write. Read them before your agents use them.";

fn text(s: String, th: &Theme) -> AnyElement {
    div().text_size(px(12.5)).text_color(th.fg_2).child(s).into_any_element()
}

fn review_bar(cx: &mut Context<SkillsView>, th: &Theme) -> AnyElement {
    let head = div().text_size(px(13.)).font_weight(FontWeight::BOLD).text_color(th.wait.fg).child("Review before turning on");
    let body = div().text_size(px(12.5)).text_color(th.fg_2).line_height(px(18.)).child(REVIEW_TEXT);
    let read = chip("skill-read", "Read the SKILL.md", cx).on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Review, cx)));
    let bar = div().id("skill-review-bar").flex().flex_col().gap(px(8.)).p(px(16.)).rounded(px(10.));
    bar.bg(th.wait.dim).border_1().border_color(th.wait.rim).child(head).child(body).child(div().flex().child(read)).into_any_element()
}

/// "512 B", "2.1 KB", "1.5 MB".
fn human_size(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.),
    }
}

/// First `max` lines of `s`, and how many lines were cut.
fn cap_lines(s: &str, max: usize) -> (String, usize) {
    let total = s.lines().count();
    let kept: Vec<&str> = s.lines().take(max).collect();
    (kept.join("\n"), total.saturating_sub(max))
}

/// Files under scripts/ can run code.
fn runs_code(path: &str) -> bool {
    path.starts_with("scripts/")
}

fn mono(s: String, th: &Theme) -> AnyElement {
    div().font_family("Menlo").text_size(px(11.5)).text_color(th.fg_2).child(s).into_any_element()
}

fn unlocks(d: &SkillInfo, th: &Theme) -> Vec<AnyElement> {
    let bash = d.bash.iter().map(|b| format!("Bash: {b}"));
    let lines: Vec<AnyElement> = bash.chain(d.tools.iter().cloned()).map(|l| mono(l, th)).collect();
    if lines.is_empty() { vec![text("No extra permissions.".into(), th)] } else { lines }
}

fn file_body(f: &SkillFile, th: &Theme) -> AnyElement {
    let Some(t) = &f.text else { return lib::note("Binary or too large, not shown.", th).into_any_element() };
    let (shown, hidden) = cap_lines(t, MAX_LINES);
    let more = (hidden > 0).then(|| mono(format!("\u{2026} {hidden} more lines"), th));
    let body = div().flex().flex_col().gap(px(4.)).p(px(10.)).rounded(px(8.)).bg(th.well);
    body.child(mono(shown, th)).children(more).into_any_element()
}

fn used_line(n: i64, th: &Theme) -> AnyElement {
    let num = div().text_size(px(17.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(n.to_string());
    let rest = div().text_size(px(12.5)).text_color(th.fg_2).child("times in the last 7 days");
    div().flex().items_baseline().gap(px(6.)).child(num).child(rest).into_any_element()
}

impl SkillsView {
    pub(super) fn drawer(&mut self, d: &SkillInfo, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let roles = self.store.read(cx).roles.clone();
        let attached = attached_label(&d.attach.roles, &d.attach.clients, &roles);
        let eyebrow = format!("{} \u{b7} {attached}", author_label(&d.author));
        let close = lib::close_button("skills-drawer-close", Self::close, th, cx);
        let head = lib::drawer_top(&eyebrow, &d.name, close, &d.status, false, th);
        let tabs = lib::tab_bar("skills-tabs", &TABS, self.tab, Self::set_tab, th, cx);
        let body = lib::scroll_body("skills-drawer-scroll", self.tab_body(d, &attached, &roles, th, cx));
        let foot = self.foot(d, th, cx);
        lib::drawer_shell("skills-drawer", th).child(head).child(tabs).child(body).child(foot).into_any_element()
    }

    fn tab_body(&mut self, d: &SkillInfo, attached: &str, roles: &[RoleInfo], th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match self.tab {
            Tab::Review => self.review(d, th, cx),
            Tab::Overview => self.overview(d, attached, th, cx),
            Tab::Edit => {
                let acts = EditActs { save: Self::save, preview: Self::toggle_preview, discard: Self::discard };
                self.edit.view(EDIT_HINT, acts, th, cx)
            }
            Tab::Attach => self.attach_tab(roles, th, cx),
            Tab::Tools => self.tools_tab(d, th, cx),
            Tab::History => lib::history_tab(&d.hist, &d.versions, Self::ask_restore, th, cx),
        }
    }

    fn review(&mut self, d: &SkillInfo, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let body = markdown::render(&d.body, th.fg_2, 12.5, th);
        vec![
            section("What it unlocks", unlocks(d, th)),
            section("Instructions", vec![body]),
            section("Files", self.file_rows(th, cx)),
        ]
    }

    fn file_rows(&mut self, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(files) = self.files.clone() else { return vec![text("Loading files\u{2026}".into(), th)] };
        files.iter().enumerate().map(|(i, f)| self.file_row(i, f, th, cx)).collect()
    }

    fn file_row(&self, i: usize, f: &SkillFile, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let path = f.path.clone();
        let name = div().font_family("Menlo").text_size(px(12.)).text_color(th.fg).child(f.path.clone());
        let size = div().text_size(px(11.)).text_color(th.muted).child(human_size(f.size));
        let code = runs_code(&f.path).then(|| tag(&th.wait, "runs code"));
        let head = div().flex().items_center().gap(px(8.)).child(name).child(size).children(code);
        let row = div().id(("skill-file", i)).flex().flex_col().gap(px(6.)).py(px(4.)).cursor_pointer();
        let row = row.on_click(cx.listener(move |this, _, _, cx| this.toggle_file(path.clone(), cx)));
        row.child(head).children(self.open_files.contains(&f.path).then(|| file_body(f, th))).into_any_element()
    }

    fn overview(&self, d: &SkillInfo, attached: &str, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if d.status == "review" {
            out.push(review_bar(cx, th));
        }
        out.push(section("When agents should use this skill", vec![text(d.description.clone(), th)]));
        out.push(section("Attached to", vec![text(attached.to_string(), th)]));
        out.push(section("Used", vec![used_line(d.used_7d, th)]));
        out
    }

    fn attach_tab(&self, roles: &[RoleInfo], th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let clients = self.store.read(cx).clients.clone();
        let role_rows: Vec<AnyElement> = roles
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let (name, on) = (r.name.clone(), self.sel_roles.contains(&r.name));
                let row = lib::check_row(("skill-role", i), format!("All {} agents", r.title), on, th);
                row.on_click(cx.listener(move |this, _, _, cx| this.toggle_role(name.clone(), cx))).into_any_element()
            })
            .collect();
        let client_rows: Vec<AnyElement> = clients
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (name, on) = (c.name.clone(), self.sel_clients.contains(&c.name));
                let row = lib::check_row(("skill-client", i), c.name.clone(), on, th);
                row.on_click(cx.listener(move |this, _, _, cx| this.toggle_client(name.clone(), cx))).into_any_element()
            })
            .collect();
        let client_rows = if client_rows.is_empty() { vec![lib::note("No clients yet.", th).into_any_element()] } else { client_rows };
        let apply = primary_button("skill-apply", "Apply", cx).on_click(cx.listener(|this, _, _, cx| this.apply_attach(cx)));
        vec![
            section("Who gets this skill", role_rows),
            section("Only these clients", client_rows),
            lib::note("Client skills never leave that client.", th).into_any_element(),
            div().flex().child(apply).into_any_element(),
        ]
    }

    fn tools_tab(&self, d: &SkillInfo, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let grid = lib::tool_grid("skill-tools", &SKILL_TOOLS, &d.tools, Self::toggle_tool, th, cx);
        let lock = icon("lock", 12., th.muted);
        let locked = div().flex().items_center().gap(px(6.)).child(lock);
        let locked = locked.child(lib::note("Deploying to production is not on this list and cannot be added.", th));
        let stored = lib::note("Stored now. Enforced from the tools round.", th);
        vec![section("Allowed tools", vec![grid]), locked.into_any_element(), stored.into_any_element()]
    }

    fn turn_on(cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, _, _, cx| this.set_status("on", "Turned on", "Agents use it from the next loop", cx))
    }

    fn foot(&self, d: &SkillInfo, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut btns: Vec<AnyElement> = Vec::new();
        if d.status == "on" {
            let off = chip("skill-off", "Turn off", cx);
            btns.push(off.on_click(cx.listener(|this, _, _, cx| this.set_status("off", "Turned off", "", cx))).into_any_element());
        } else if self.tab == Tab::Edit {
            let on = Self::turn_on(cx);
            btns.push(chip("skill-on", "Turn on", cx).on_click(on).into_any_element());
        } else {
            let on = Self::turn_on(cx);
            btns.push(primary_button("skill-on", "Turn on", cx).on_click(on).into_any_element());
        }
        if d.status == "review" {
            let keep = chip("skill-keep", "Keep off", cx);
            btns.push(keep.on_click(cx.listener(|this, _, _, cx| this.set_status("off", "Stays off", "No agent reads it", cx))).into_any_element());
        }
        let del = lib::danger_button("skill-delete", "Delete", true, th).ml_auto();
        btns.push(del.on_click(cx.listener(|this, _, _, cx| this.ask_delete(cx))).into_any_element());
        let hint = (d.status == "review").then_some("Nothing reads it until you turn it on.");
        lib::drawer_foot(btns, hint, th)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_picks_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2150), "2.1 KB");
        assert_eq!(human_size(1_572_864), "1.5 MB");
    }

    #[test]
    fn cap_lines_reports_hidden() {
        assert_eq!(cap_lines("a\nb\nc", 2), ("a\nb".to_string(), 1));
        assert_eq!(cap_lines("a\nb", 5), ("a\nb".to_string(), 0));
    }

    #[test]
    fn runs_code_checks_scripts_dir() {
        assert!(runs_code("scripts/run.sh"));
        assert!(!runs_code("SKILL.md"));
    }
}
