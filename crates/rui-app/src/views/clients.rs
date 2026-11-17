//! Clients page: one card per client.

use super::status::{Word, word_of};
use crate::api::OwnerState;
use std::collections::HashMap;
use super::card;
use super::character::{CharState, character};
use super::section::{Sec, section_top};
use crate::store::StoreHandle;
use crate::theme::{Theme, theme};
use gpui::{AnyElement, App, Div, FontWeight, div, prelude::*, px};
use rui_core::{Client, Task};

/// Top-level tasks of a client.
fn mine(client: i64, tasks: &[Task]) -> impl Iterator<Item = &Task> {
    tasks.iter().filter(move |t| t.client_id == client && t.parent_id.is_none())
}

/// Counts for the card: (working, waiting, needs you, stopped) of its top-level tasks.
fn counts(client: i64, tasks: &[Task], states: &HashMap<i64, OwnerState>) -> [usize; 4] {
    let n = |w: Word| mine(client, tasks).filter(|t| word_of(states, t.id) == w).count();
    [n(Word::Working), n(Word::Waiting), n(Word::NeedsYou), n(Word::Stopped)]
}

/// A count as text, with a dash for zero.
fn dash_zero(n: usize) -> String {
    if n == 0 { "—".to_string() } else { n.to_string() }
}

/// "Client since Nov 2026" from "YYYY-MM-..." text; plain "Client" when it does not parse.
fn since(created_at: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let head = created_at.get(..7).unwrap_or("");
    let (year, month) = head.split_once('-').unwrap_or(("", ""));
    let month = month.parse::<usize>().ok().filter(|m| (1..=12).contains(m));
    match (year.parse::<u32>().is_ok(), month) {
        (true, Some(m)) => format!("Client since {} {year}", MONTHS[m - 1]),
        _ => "Client".to_string(),
    }
}

/// Lead line: "1 client." or "N clients." plus the ownership sentence.
fn lead_text(n: usize) -> String {
    let noun = if n == 1 { "client" } else { "clients" };
    format!("{n} {noun}. Every thread, task and seat charge belongs to exactly one of them.")
}

/// One label/value row of a client card.
fn kv(label: &str, value: String, th: &Theme) -> AnyElement {
    let name = div().text_size(px(11.5)).text_color(th.muted).child(label.to_string());
    let num = div().font_family("Menlo").text_size(px(12.5)).text_color(th.fg).child(value);
    div().flex().justify_between().child(name).child(num).into_any_element()
}

/// The four stat rows of a client card.
fn stats(c: [usize; 4], th: &Theme) -> Div {
    let rows = [("Working", c[0]), ("Waiting", c[1]), ("Needs you", c[2]), ("Stopped", c[3])];
    let kvs = rows.map(|(label, n)| kv(label, dash_zero(n), th));
    div().flex().flex_col().gap(px(4.)).children(kvs)
}

fn client_card(c: &Client, tasks: &[Task], states: &HashMap<i64, OwnerState>, cx: &App) -> AnyElement {
    let th = theme(cx);
    let n = counts(c.id, tasks, states);
    let state = match (n[0], n[2] > 0) {
        (0, true) => CharState::Blocked,
        (0, false) => CharState::Standing,
        _ => CharState::Doing,
    };
    let face = character(state, px(34.), ("client-rui", c.id as usize), th.client_color(c.id), th);
    let name = div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(c.name.clone());
    let head = div().flex().items_center().gap(px(10.)).child(face).child(name);
    let foot = div().truncate().text_size(px(11.)).text_color(th.muted).child(since(&c.created_at));
    let body = div().flex().flex_col().gap(px(12.)).child(head).child(stats(n, th)).child(foot);
    let card = card(cx).id(("client-card", c.id as usize)).w(px(280.)).p(px(16.));
    card.child(body).into_any_element()
}

/// Last folder name of a path, ignoring a trailing slash.
pub fn folder_name(path: &str) -> Option<String> {
    let name = path.trim_end_matches('/').rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// The Clients tab of the Clients section; `add` is the "+ Add client" button.
pub fn page(add: AnyElement, cx: &App) -> AnyElement {
    let store = cx.global::<StoreHandle>().0.read(cx);
    let cards: Vec<AnyElement> = store.clients.iter().map(|c| client_card(c, &store.tasks, &store.states, cx)).collect();
    let grid = div().id("clients-grid").flex().flex_wrap().gap(px(12.)).pb(px(28.)).children(cards);
    let scroll = div().id("clients-scroll").flex_1().min_h_0().overflow_y_scroll().child(grid);
    let lead = div().id("clients-lead").text_size(px(14.)).text_color(theme(cx).fg_2).child(lead_text(store.clients.len()));
    let top = section_top(Sec::Clients, Some(lead.into_any_element()), Some(add), cx);
    div().id("clients-page").flex().flex_col().size_full().pr(px(40.)).child(top).child(scroll).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(id: i64, parent: Option<i64>, status: &str) -> Task {
        serde_json::from_value(json!({"id":id,"client_id":1,"parent_id":parent,"title":"t","body":"",
            "status":status,"done_command":null,"max_loops":5,"loops_used":0,"role":"lead",
            "branch":null,"source_kind":"user","source_ref":null,"depends_on":[],"agent":null,
            "pod":[],"created_at":"a","updated_at":"b"}))
        .unwrap()
    }

    #[test]
    fn folder_name_handles_trailing_slash() {
        assert_eq!(folder_name("/a/b/acme"), Some("acme".into()));
        assert_eq!(folder_name("/a/b/acme/"), Some("acme".into()));
        assert_eq!(folder_name("/"), None);
    }

    #[test]
    fn since_reads_year_and_month() {
        assert_eq!(since("2026-11-15 11:54:41"), "Client since Nov 2026");
        assert_eq!(since("2025-01-02T03:04:05Z"), "Client since Jan 2025");
        assert_eq!(since("garbage"), "Client");
        assert_eq!(since("2026-13-01"), "Client");
        assert_eq!(since(""), "Client");
    }

    #[test]
    fn lead_pluralises() {
        assert!(lead_text(1).starts_with("1 client. "));
        assert!(lead_text(3).starts_with("3 clients. "));
    }

    #[test]
    fn counts_only_top_level_tasks_by_word() {
        let tasks = vec![task(1, None, "in_progress"), task(2, Some(1), "in_progress"), task(3, None, "in_progress"), task(4, None, "open")];
        let state = |id: i64, word: &str| (id, OwnerState { task_id: id, word: word.into(), reason: String::new(), since: String::new() });
        let states: HashMap<i64, OwnerState> = [state(1, "Working"), state(2, "Working"), state(3, "Stopped")].into();
        assert_eq!(counts(1, &tasks, &states), [1, 1, 0, 1]);
        assert_eq!(counts(2, &tasks, &states), [0, 0, 0, 0]);
        assert_eq!(dash_zero(0), "—");
        assert_eq!(dash_zero(2), "2");
    }
}
