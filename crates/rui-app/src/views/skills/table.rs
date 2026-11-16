//! The skills table: filters, search, the "attached to" label and the rows.

use super::SkillsView;
use crate::api::{RoleInfo, SkillInfo};
use crate::theme::Theme;
use crate::views::library::{SKILL_TOOLS, status_cell};
use crate::views::roster::{capitalize, role_title};
use gpui::{AnyElement, Context, Div, FontWeight, Stateful, div, prelude::*, px};

const ATTACHED_W: f32 = 150.;
const TOOLS_W: f32 = 56.;
const AUTHOR_W: f32 = 76.;
const WHEN_W: f32 = 112.;
const STATUS_W: f32 = 104.;
const DASH: &str = "\u{2014}";

/// Status filter chips above the table.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Filter {
    All,
    On,
    Review,
    Off,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Filter::All, Filter::On, Filter::Review, Filter::Off];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::On => "On",
            Filter::Review => "Needs review",
            Filter::Off => "Off",
        }
    }

    fn matches(self, s: &SkillInfo) -> bool {
        match self {
            Filter::All => true,
            Filter::On => s.status == "on",
            Filter::Review => s.status == "review",
            Filter::Off => s.status == "off",
        }
    }
}

/// Skills per filter, in `Filter::ALL` order.
pub fn counts(skills: &[SkillInfo]) -> [usize; 4] {
    Filter::ALL.map(|f| skills.iter().filter(|s| f.matches(s)).count())
}

/// Skills that pass the filter and whose name or description holds the query.
pub fn visible<'a>(skills: &'a [SkillInfo], filter: Filter, query: &str) -> Vec<&'a SkillInfo> {
    let q = query.trim().to_lowercase();
    let hit = |s: &SkillInfo| q.is_empty() || s.name.to_lowercase().contains(&q) || s.description.to_lowercase().contains(&q);
    skills.iter().filter(|s| filter.matches(s) && hit(s)).collect()
}

/// "All agents" when every role has the skill, else role titles then client names; a dash when none.
pub fn attached_label(skill_roles: &[String], clients: &[String], roles: &[RoleInfo]) -> String {
    if !roles.is_empty() && roles.iter().all(|r| skill_roles.contains(&r.name)) {
        return "All agents".into();
    }
    let titles = skill_roles.iter().map(|n| role_title(roles, n));
    let names: Vec<String> = titles.chain(clients.iter().cloned()).collect();
    if names.is_empty() { DASH.into() } else { names.join(", ") }
}

/// "You", "Shipped" or "Imported".
pub fn author_label(author: &str) -> String {
    if author.is_empty() { DASH.into() } else { capitalize(author) }
}

/// Date and who of the newest history line.
pub fn last_change(s: &SkillInfo) -> (String, String) {
    match s.hist.last() {
        Some(h) => (h.when.clone(), h.who.clone()),
        None => (DASH.into(), String::new()),
    }
}

fn head_cell(text: &str, width: Option<f32>, th: &Theme) -> Div {
    let cell = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(text.to_uppercase());
    match width {
        Some(w) => cell.flex_none().w(px(w)),
        None => cell.flex_1().min_w(px(180.)),
    }
}

/// Column headings.
pub fn head(th: &Theme) -> Stateful<Div> {
    let row = div().id("skills-head").flex().gap(px(14.)).px(px(12.)).pb(px(8.)).pt(px(4.));
    let row = row.border_b_1().border_color(th.hairline);
    row.child(head_cell("Skill", None, th))
        .child(head_cell("Attached to", Some(ATTACHED_W), th))
        .child(head_cell("Tools", Some(TOOLS_W), th))
        .child(head_cell("Author", Some(AUTHOR_W), th))
        .child(head_cell("Last change", Some(WHEN_W), th))
        .child(head_cell("Status", Some(STATUS_W), th))
}

fn cell(text: String, width: f32, color: gpui::Hsla) -> Div {
    div().flex_none().w(px(width)).min_w_0().text_size(px(12.)).text_color(color).child(text)
}

/// One table row.
pub fn row(i: usize, s: &SkillInfo, roles: &[RoleInfo], selected: bool, th: &Theme, cx: &mut Context<SkillsView>) -> AnyElement {
    let name = s.name.clone();
    let title = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(s.name.clone());
    let desc = div().text_size(px(11.5)).text_color(th.muted).line_height(px(16.)).child(s.description.clone());
    let first = div().flex().flex_col().flex_1().min_w(px(180.)).gap(px(3.)).child(title).child(desc);
    let attached = attached_label(&s.attach.roles, &s.attach.clients, roles);
    let (when, who) = last_change(s);
    let last = div().flex().flex_col().flex_none().w(px(WHEN_W)).child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(when));
    let last = last.child(div().text_size(px(11.5)).text_color(th.muted).child(who));
    let tools = format!("{} of {}", s.tools.len(), SKILL_TOOLS.len());
    let hover = th.surface_2;
    let el = div().id(("skills-row", i)).flex().items_center().gap(px(14.)).px(px(12.)).py(px(11.));
    let el = el.border_b_1().border_color(th.hairline).cursor_pointer().hover(move |st| st.bg(hover));
    let el = if selected { el.bg(th.surface_2) } else { el };
    el.on_click(cx.listener(move |this, _, _, cx| this.open_skill(&name, cx)))
        .child(first)
        .child(cell(attached, ATTACHED_W, th.fg_2))
        .child(cell(tools, TOOLS_W, th.muted).font_family("Menlo").text_size(px(11.)))
        .child(cell(author_label(&s.author), AUTHOR_W, th.fg_2))
        .child(last)
        .child(status_cell(&s.status, STATUS_W, th))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Hist;

    fn skill(name: &str, desc: &str, status: &str) -> SkillInfo {
        SkillInfo { name: name.into(), description: desc.into(), status: status.into(), ..Default::default() }
    }

    fn role(name: &str, title: &str) -> RoleInfo {
        RoleInfo { name: name.into(), title: title.into(), ..Default::default() }
    }

    fn names(list: &[&SkillInfo]) -> Vec<String> {
        list.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn counts_follow_status() {
        let all = [skill("a", "", "on"), skill("b", "", "review"), skill("c", "", "off"), skill("d", "", "on")];
        assert_eq!(counts(&all), [4, 2, 1, 1]);
        assert_eq!(counts(&[]), [0, 0, 0, 0]);
    }

    #[test]
    fn search_matches_name_or_description_in_any_case() {
        let all = [skill("deploy-check", "Use before shipping", "on"), skill("tidy", "Cleans the DEPLOY notes", "off"), skill("x", "y", "on")];
        assert_eq!(names(&visible(&all, Filter::All, "deploy")), ["deploy-check", "tidy"]);
        assert_eq!(names(&visible(&all, Filter::On, " DEPLOY ")), ["deploy-check"]);
        assert_eq!(names(&visible(&all, Filter::Off, "")), ["tidy"]);
        assert!(visible(&all, Filter::Review, "").is_empty());
    }

    #[test]
    fn attached_label_cases() {
        let roles = [role("lead", "Lead"), role("backend", "Backend Engineer")];
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(attached_label(&own(&["lead", "backend"]), &[], &roles), "All agents");
        assert_eq!(attached_label(&own(&["backend"]), &[], &roles), "Backend Engineer");
        assert_eq!(attached_label(&own(&["lead"]), &own(&["acme"]), &roles), "Lead, acme");
        assert_eq!(attached_label(&[], &own(&["acme", "zed"]), &roles), "acme, zed");
        assert_eq!(attached_label(&[], &[], &roles), "\u{2014}");
        assert_eq!(attached_label(&[], &[], &[]), "\u{2014}");
    }

    #[test]
    fn author_and_last_change() {
        assert_eq!((author_label("you"), author_label("imported"), author_label("")), ("You".into(), "Imported".into(), "\u{2014}".into()));
        let mut s = skill("a", "", "on");
        assert_eq!(last_change(&s), ("\u{2014}".into(), String::new()));
        let h = |when: &str| Hist { when: when.into(), who: "You".into(), what: "x".into(), why: None, from: None };
        s.hist = vec![h("2026-01-01 10:00"), h("2026-02-02 11:00")];
        assert_eq!(last_change(&s), ("2026-02-02 11:00".into(), "You".into()));
    }
}
