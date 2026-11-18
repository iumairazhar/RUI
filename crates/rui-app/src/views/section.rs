//! Chrome shared by the sectioned pages: eyebrow, title, lead line and the sub-tab bar.

use super::page_head;
use crate::store::StoreHandle;
use crate::theme::theme;
use gpui::{AnyElement, App, Div, FontWeight, SharedString, div, prelude::*, px};

/// A rail section that has a page head.
#[derive(Clone, Copy, PartialEq)]
pub enum Sec {
    Work,
    Clients,
    Knowledge,
    Cost,
    Analytics,
    Settings,
}

impl Sec {
    /// Key under which the selected sub-tab is kept in the store.
    pub fn key(self) -> &'static str {
        match self {
            Sec::Work => "work",
            Sec::Clients => "clients",
            Sec::Knowledge => "knowledge",
            Sec::Cost => "cost",
            Sec::Analytics => "analytics",
            Sec::Settings => "settings",
        }
    }

    fn eyebrow(self) -> &'static str {
        match self {
            Sec::Work => "LIVE WORK",
            Sec::Clients => "WHO THE WORK IS FOR",
            Sec::Knowledge => "WHAT THE AGENTS KNOW",
            Sec::Cost => "WHAT IT COSTS TO RUN",
            Sec::Analytics => "WHERE THE TIME GOES",
            Sec::Settings => "HOW THE CONSOLE BEHAVES",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Sec::Work => "Work",
            Sec::Clients => "Clients",
            Sec::Knowledge => "Knowledge",
            Sec::Cost => "Cost",
            Sec::Analytics => "Analytics",
            Sec::Settings => "Settings",
        }
    }

    /// Sub-tab names; a section with one page has none.
    pub fn tabs(self) -> &'static [&'static str] {
        match self {
            Sec::Work => &["Board", "Apps"],
            Sec::Clients => &["Clients", "Pods"],
            Sec::Knowledge => &["Skills", "Playbook"],
            Sec::Cost => &["Spend", "Seats"],
            Sec::Analytics | Sec::Settings => &[],
        }
    }
}

/// Index of the selected sub-tab of a section.
pub fn active_tab(sec: Sec, cx: &App) -> usize {
    cx.global::<StoreHandle>().0.read(cx).tabs.get(sec.key()).copied().unwrap_or(0)
}

/// The segmented control (mock `.tabs--page`).
fn tab_bar(sec: Sec, cx: &App) -> impl IntoElement {
    let th = theme(cx);
    let store = cx.global::<StoreHandle>().0.clone();
    let active = active_tab(sec, cx);
    let ov = if th.dark { gpui::white() } else { gpui::black() };
    let tabs = sec.tabs().iter().enumerate().map(|(i, name)| {
        let store = store.clone();
        let tab = div()
            .id(SharedString::from(format!("subtab-{}-{i}", sec.key())))
            .flex()
            .items_center()
            .h(px(28.))
            .px(px(15.))
            .rounded(px(7.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer();
        let tab = if i == active {
            let hover = th.accent_fill_hover;
            tab.bg(th.accent_fill).text_color(th.on_accent).hover(move |s| s.bg(hover))
        } else {
            let (hover, fg) = (ov.opacity(0.14), th.fg);
            tab.text_color(th.fg_2).hover(move |s| s.bg(hover).text_color(fg))
        };
        tab.child(*name).on_click(move |_, _, cx| {
            store.update(cx, |s, cx| {
                s.tabs.insert(sec.key(), i);
                cx.notify();
            })
        })
    });
    let bar = div().id(SharedString::from(format!("subtabs-{}", sec.key()))).flex().gap(px(2.)).p(px(3.));
    bar.rounded(px(9.)).bg(ov.opacity(0.09)).border_1().border_color(th.hairline).children(tabs)
}

/// Eyebrow, title, optional lead line and action button, then the sub-tab bar.
pub fn section_top(sec: Sec, lead: Option<AnyElement>, action: Option<AnyElement>, cx: &App) -> Div {
    let head = page_head(sec.eyebrow(), sec.title(), cx).pb(px(0.));
    let row = div().flex().items_end().justify_between().child(head).children(action);
    let lead = lead.map(|l| div().pt(px(16.)).child(l));
    let tabs = (sec.tabs().len() > 1).then(|| div().flex().pt(px(16.)).child(tab_bar(sec, cx)));
    div().flex().flex_col().flex_none().child(row).children(lead).children(tabs).child(div().h(px(16.)))
}

/// A section page with nothing to show yet.
pub fn empty_page(sec: Sec, text: &str, cx: &App) -> AnyElement {
    let th = theme(cx);
    let note = div().text_size(px(13.)).text_color(th.muted).max_w(px(520.)).child(text.to_string());
    div()
        .id(SharedString::from(format!("{}-empty-page", sec.key())))
        .flex()
        .flex_col()
        .size_full()
        .pr(px(40.))
        .child(section_top(sec, None, None, cx))
        .child(note)
        .into_any_element()
}
