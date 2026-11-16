//! Apps: what each project's team has running, with links and logs.

use super::activity::{chevron, mono_block, text_lines};
use super::{card, chip, page_head};
use crate::api::AppInfo;
use crate::store::{self, Store};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, FontWeight, Hsla, Window, div, prelude::*, px};
use std::collections::HashSet;

const LOG_LINES: usize = 20;
const EMPTY: &str = "No apps running yet. When a team finishes a task, Rui starts its apps here and posts the links in chat.";

/// Apps page: one card per project.
pub struct AppsView {
    store: Entity<Store>,
    /// (project, app name) pairs whose log is open.
    open_logs: HashSet<(String, String)>,
}

/// Dot color for an app status.
fn status_color(status: &str, th: &Theme) -> Hsla {
    match status {
        "running" => th.run.base,
        "starting" => th.wait.base,
        "failed" => th.block.base,
        _ => th.muted,
    }
}

/// Apps grouped by project, keeping the daemon's order.
fn by_project(apps: &[AppInfo]) -> Vec<(String, Vec<AppInfo>)> {
    let mut groups: Vec<(String, Vec<AppInfo>)> = Vec::new();
    for app in apps {
        match groups.last_mut().filter(|(p, _)| *p == app.project) {
            Some((_, list)) => list.push(app.clone()),
            None => groups.push((app.project.clone(), vec![app.clone()])),
        }
    }
    groups
}

/// The newest `LOG_LINES` lines of a log.
fn log_tail(log: &[String]) -> &[String] {
    &log[log.len().saturating_sub(LOG_LINES)..]
}

impl AppsView {
    /// Build the apps view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self { store, open_logs: HashSet::new() }
    }

    fn toggle_log(&mut self, key: (String, String), cx: &mut Context<Self>) {
        if !self.open_logs.remove(&key) {
            self.open_logs.insert(key);
        }
        cx.notify();
    }

    fn project_buttons(&self, gi: usize, project: &str, cx: &mut Context<Self>) -> Div {
        let (p1, p2) = (project.to_string(), project.to_string());
        let restart = chip(format!("apps-restart-{gi}"), "Restart", cx)
            .on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.restart_apps(p1.clone(), cx))));
        let stop = chip(format!("apps-stop-{gi}"), "Stop", cx)
            .on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.stop_apps(p2.clone(), cx))));
        div().flex().gap(px(8.)).child(restart).child(stop)
    }

    fn card_head(&self, gi: usize, project: &str, th: &Theme, cx: &mut Context<Self>) -> Div {
        let name = div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(project.to_string());
        div().flex().items_center().justify_between().child(name).child(self.project_buttons(gi, project, cx))
    }

    fn app_row(&self, idx: usize, app: &AppInfo, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let dot = div().flex_none().size(px(8.)).rounded(px(4.)).bg(status_color(&app.status, th));
        let name = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(app.name.clone());
        let accent = th.accent_text;
        let url = app.url.clone();
        let link = div()
            .id(("apps-link", idx))
            .text_size(px(12.5))
            .text_color(accent)
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
            .child(app.url.clone());
        let url = app.url.clone();
        let open = chip(format!("apps-open-{idx}"), "Open", cx).on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)));
        let head = div().flex().items_center().gap(px(10.)).child(dot).child(name).child(div().flex_1().child(link)).child(open);
        let key = (app.project.clone(), app.name.clone());
        div()
            .id(("apps-row", idx))
            .flex()
            .flex_col()
            .gap(px(8.))
            .py(px(8.))
            .child(head)
            .child(self.log_section(idx, key, app, th, cx))
            .into_any_element()
    }

    fn log_section(&self, idx: usize, key: (String, String), app: &AppInfo, th: &Theme, cx: &mut Context<Self>) -> Div {
        let open = self.open_logs.contains(&key);
        let label = div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("Log");
        let head = div()
            .id(("apps-log-head", idx))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_log(key.clone(), cx)))
            .child(chevron(open, th))
            .child(label);
        let body = open.then(|| log_body(idx, &app.log, th));
        div().flex().flex_col().gap(px(6.)).child(head).children(body)
    }

    fn project_card(&self, gi: usize, project: &str, apps: &[AppInfo], base: usize, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = apps.iter().enumerate().map(|(i, a)| self.app_row(base + i, a, th, cx)).collect();
        card(cx)
            .id(("apps-card", gi))
            .p(px(16.))
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.card_head(gi, project, th, cx))
            .children(rows)
            .into_any_element()
    }
}

fn log_body(idx: usize, log: &[String], th: &Theme) -> AnyElement {
    if log.is_empty() {
        return div().text_size(px(12.)).text_color(th.muted).child("No output yet.").into_any_element();
    }
    mono_block(idx, text_lines(&log_tail(log).join("\n"), th.fg_2), th)
}

impl Render for AppsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let apps = self.store.read(cx).apps.clone();
        let mut base = 0;
        let mut cards: Vec<AnyElement> = Vec::new();
        for (gi, (project, list)) in by_project(&apps).iter().enumerate() {
            cards.push(self.project_card(gi, project, list, base, &th, cx));
            base += list.len();
        }
        let empty = apps.is_empty().then(|| div().id("apps-empty").text_size(px(13.)).text_color(th.muted).max_w(px(520.)).child(EMPTY));
        div()
            .id("apps-page")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .pr(px(40.))
            .pb(px(28.))
            .child(page_head("RUNNING", "Apps", cx))
            .child(div().id("apps-list").flex().flex_col().gap(px(12.)).children(cards))
            .children(empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(project: &str, name: &str) -> AppInfo {
        AppInfo { project: project.into(), name: name.into(), port: 1, url: String::new(), status: "running".into(), log: vec![] }
    }

    #[test]
    fn groups_by_project_in_order() {
        let apps = [app("a", "x"), app("a", "y"), app("b", "z")];
        let groups = by_project(&apps);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].1.len(), 2);
    }

    #[test]
    fn log_tail_keeps_last_20() {
        let log: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        assert_eq!(log_tail(&log).len(), 20);
        assert_eq!(log_tail(&log)[0], "10");
        assert_eq!(log_tail(&log[..3]).len(), 3);
    }
}
