//! Inbox item kinds: the one place that maps a kind string to how it renders, its tag and its buttons.
//! Unknown kinds render generically: title, reason and any detail, with no buttons.

use super::{chip, markdown, primary_button};
use crate::api::InboxItem;
use crate::store::Store;
use crate::theme::{Status, Theme};
use gpui::{AnyElement, App, Div, Entity, FontWeight, Stateful, div, prelude::*, px};

/// Kind strings for email and meeting brief items.
pub const EMAIL_KIND: &str = "email";
pub const BRIEF_KIND: &str = "meeting_brief";
/// Kind string for a PR closed without merging.
pub const PR_CLOSED_KIND: &str = "pr_closed";
/// Kind strings for a deploy waiting for the owner's OK, and one that failed.
pub const DEPLOY_KIND: &str = "deploy";
pub const DEPLOY_FAILED_KIND: &str = "deploy_failed";

/// How an Inbox item renders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Question,
    Ask,
    Stuck,
    Review,
    Undo,
    Checkpoint,
    Seats,
    Proposal,
    Scout,
    Email,
    Brief,
    PrClosed,
    Deploy,
    DeployFailed,
    Other,
}

/// The renderer for a kind string.
pub fn kind_of(kind: &str) -> Kind {
    match kind {
        "question" => Kind::Question,
        "ask" => Kind::Ask,
        "stuck" => Kind::Stuck,
        "review" => Kind::Review,
        "undo" => Kind::Undo,
        "checkpoint" => Kind::Checkpoint,
        "seats" => Kind::Seats,
        "proposal" => Kind::Proposal,
        "scout" => Kind::Scout,
        EMAIL_KIND => Kind::Email,
        BRIEF_KIND => Kind::Brief,
        PR_CLOSED_KIND => Kind::PrClosed,
        DEPLOY_KIND => Kind::Deploy,
        DEPLOY_FAILED_KIND => Kind::DeployFailed,
        _ => Kind::Other,
    }
}

/// Tag text and colour family for a kind.
pub fn tag_of(k: Kind, th: &Theme) -> (&'static str, &Status) {
    match k {
        Kind::Question => ("Question", &th.block),
        Kind::Stuck => ("Stuck", &th.block),
        Kind::Review => ("Review", &th.done),
        Kind::Undo => ("Sending", &th.run),
        Kind::Checkpoint => ("Ready", &th.wait),
        Kind::Ask => ("Asked in chat", &th.think),
        Kind::Seats => ("Seats", &th.block),
        Kind::Proposal => ("Coach", &th.think),
        Kind::Scout => ("Scout", &th.run),
        Kind::Email => ("Email", &th.wait),
        Kind::Brief => ("Meeting", &th.wait),
        Kind::PrClosed => ("PR closed", &th.block),
        Kind::Deploy => ("Deploy", &th.block),
        Kind::DeployFailed => ("Deploy failed", &th.block),
        Kind::Other => ("Inbox", &th.wait),
    }
}

/// Chip text and colour family in the slide-over: a question reads as a "Decision", a stuck task as "Agent failed".
pub fn sheet_tag(k: Kind, th: &Theme) -> (&'static str, &Status) {
    match k {
        Kind::Question => ("Decision", &th.wait),
        Kind::Stuck => ("Agent failed", &th.block),
        _ => tag_of(k, th),
    }
}

/// "Low risk \u{b7} reversible" for a decision with a known risk; `None` when the risk is unknown.
pub fn risk_line(risk: &str, reversible: Option<bool>) -> Option<String> {
    if !matches!(risk, "low" | "medium" | "high") {
        return None;
    }
    let mut line = super::decision::risk_label(risk).to_string();
    match reversible {
        Some(true) => line.push_str(" \u{b7} reversible"),
        Some(false) => line.push_str(" \u{b7} can't be undone"),
        None => {}
    }
    Some(line)
}

/// A button an item offers right in its row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemAct {
    Approve,
    Dismiss,
    MakeTask,
    OpenSeats,
    Undo,
    Release,
    Drop,
    Retry,
    Deploy,
    TryAgain,
    Skip,
}

impl ItemAct {
    pub fn label(self) -> &'static str {
        match self {
            ItemAct::Approve => "Approve",
            ItemAct::Dismiss => "Dismiss",
            ItemAct::MakeTask => "Make a task",
            ItemAct::OpenSeats => "Open seats",
            ItemAct::Undo => "Undo",
            ItemAct::Release => "Release",
            ItemAct::Drop => "Drop it",
            ItemAct::Retry => "Retry",
            ItemAct::Deploy => "Deploy",
            ItemAct::TryAgain => "Try again",
            ItemAct::Skip => "Skip",
        }
    }
}

/// The in-row buttons for a kind, main one first. Kinds that open a drawer have none here.
pub fn acts_of(k: Kind) -> Vec<ItemAct> {
    match k {
        Kind::Proposal => vec![ItemAct::Approve, ItemAct::Dismiss],
        Kind::Scout | Kind::Brief => vec![ItemAct::Dismiss],
        Kind::Email => vec![ItemAct::MakeTask, ItemAct::Dismiss],
        Kind::Seats => vec![ItemAct::OpenSeats],
        Kind::Undo => vec![ItemAct::Undo],
        Kind::Checkpoint => vec![ItemAct::Release],
        Kind::PrClosed => vec![ItemAct::Drop, ItemAct::Retry],
        Kind::Deploy => vec![ItemAct::Deploy, ItemAct::Skip],
        Kind::DeployFailed => vec![ItemAct::TryAgain, ItemAct::Skip],
        _ => vec![],
    }
}

/// The reason line under the title, cut to one short line; deploy kinds show theirs inside the
/// warning box instead. The full text is one tap away in the item's drawer.
pub fn reason(it: &InboxItem) -> Option<String> {
    let own_box = matches!(kind_of(&it.kind), Kind::Deploy | Kind::DeployFailed);
    (!own_box && !it.reason.is_empty()).then(|| super::decision::short_why(&it.reason))
}

/// The item's longer text: markdown for reports, briefs, emails and unknown kinds; a quote for a proposal;
/// a warning box for deploys.
pub fn body(it: &InboxItem, th: &Theme) -> Option<AnyElement> {
    match kind_of(&it.kind) {
        Kind::Deploy => return Some(deploy_box(it, th).into_any_element()),
        Kind::DeployFailed => return Some(failed_box(it, th).into_any_element()),
        _ => {}
    }
    let detail = it.detail.as_deref().filter(|d| !d.trim().is_empty())?;
    match kind_of(&it.kind) {
        Kind::Proposal => Some(quote(detail, th).into_any_element()),
        Kind::Scout | Kind::Brief | Kind::Email | Kind::PrClosed | Kind::Other => Some(markdown::render(detail, th.fg, 12.5, th)),
        _ => None,
    }
}

fn quote(text: &str, th: &Theme) -> Div {
    let label = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("ADDS THIS LESSON");
    let text = div().text_size(px(12.5)).text_color(th.fg_2).child(text.to_string());
    div().flex().flex_col().gap(px(4.)).pl(px(10.)).border_l_2().border_color(th.hairline).child(label).child(text)
}

/// The command a deploy item runs: its reason without the "Runs: " lead.
pub fn deploy_command(reason: &str) -> &str {
    reason.strip_prefix("Runs: ").unwrap_or(reason)
}

/// Text in the monospace font, wrapped, on the well colour.
fn mono(text: &str, th: &Theme) -> Div {
    let el = div().px(px(10.)).py(px(8.)).rounded(px(7.)).bg(th.well).font_family("Menlo").text_size(px(11.5));
    el.text_color(th.fg).child(text.to_string())
}

/// The block-coloured box that holds a deploy's details.
fn warn_box(th: &Theme) -> Div {
    let el = div().flex().flex_col().gap(px(6.)).p(px(10.)).rounded(px(8.)).bg(th.block.dim);
    el.border_1().border_color(th.block.rim)
}

fn warn_line(text: &str, th: &Theme) -> Div {
    div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.block.fg).child(text.to_string())
}

/// The command it will run and the daemon's warning, like "A production deploy cannot be undone."
fn deploy_box(it: &InboxItem, th: &Theme) -> Div {
    let note = it.detail.as_deref().filter(|d| !d.trim().is_empty()).unwrap_or("Nothing runs until you press Deploy.");
    warn_box(th).child(warn_line("Runs", th)).child(mono(deploy_command(&it.reason), th)).child(warn_line(note, th))
}

/// What failed and the last lines the command printed.
fn failed_box(it: &InboxItem, th: &Theme) -> Div {
    let tail = it.detail.as_deref().filter(|d| !d.trim().is_empty()).unwrap_or("No output.");
    warn_box(th).child(warn_line(&it.reason, th)).child(mono(tail.trim_end(), th))
}

/// The kind's buttons; a click runs the act through the store. The first one is the main button.
pub fn act_buttons(prefix: &str, i: usize, it: &InboxItem, store: &Entity<Store>, cx: &App) -> Vec<Stateful<Div>> {
    let k = kind_of(&it.kind);
    let id = it.id;
    let button = |n: usize, act: ItemAct| {
        let el_id = format!("{prefix}-act-{i}-{n}");
        let el = if n == 0 { primary_button(el_id, act.label(), cx) } else { chip(el_id, act.label(), cx) };
        let store = store.clone();
        el.on_click(move |_, _, cx| store.update(cx, |s, cx| s.inbox_act(k, id, act, cx)))
    };
    acts_of(k).into_iter().enumerate().map(|(n, a)| button(n, a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_renderers_and_unknown_is_generic() {
        assert_eq!(kind_of("proposal"), Kind::Proposal);
        assert_eq!(kind_of("scout"), Kind::Scout);
        assert_eq!(kind_of("seats"), Kind::Seats);
        assert_eq!(kind_of(EMAIL_KIND), Kind::Email);
        assert_eq!(kind_of(BRIEF_KIND), Kind::Brief);
        assert_eq!(kind_of(PR_CLOSED_KIND), Kind::PrClosed);
        assert_eq!(kind_of(DEPLOY_KIND), Kind::Deploy);
        assert_eq!(kind_of(DEPLOY_FAILED_KIND), Kind::DeployFailed);
        assert_eq!(kind_of("something_new"), Kind::Other);
        assert!(acts_of(Kind::Other).is_empty());
    }

    #[test]
    fn each_kind_gets_its_buttons() {
        assert_eq!(acts_of(Kind::Proposal), [ItemAct::Approve, ItemAct::Dismiss]);
        assert_eq!(acts_of(Kind::Scout), [ItemAct::Dismiss]);
        assert_eq!(acts_of(Kind::Email), [ItemAct::MakeTask, ItemAct::Dismiss]);
        assert_eq!(acts_of(Kind::Brief), [ItemAct::Dismiss]);
        assert_eq!(acts_of(Kind::Seats), [ItemAct::OpenSeats]);
        assert_eq!(acts_of(Kind::Undo), [ItemAct::Undo]);
        assert_eq!(acts_of(Kind::Checkpoint), [ItemAct::Release]);
        assert_eq!(acts_of(Kind::PrClosed), [ItemAct::Drop, ItemAct::Retry]);
        assert_eq!((ItemAct::Drop.label(), ItemAct::Retry.label()), ("Drop it", "Retry"));
        assert!(acts_of(Kind::Question).is_empty());
        assert_eq!(acts_of(Kind::Deploy), [ItemAct::Deploy, ItemAct::Skip]);
        assert_eq!(acts_of(Kind::DeployFailed), [ItemAct::TryAgain, ItemAct::Skip]);
        assert_eq!((ItemAct::Deploy.label(), ItemAct::TryAgain.label(), ItemAct::Skip.label()), ("Deploy", "Try again", "Skip"));
    }

    #[test]
    fn sheet_chip_words_follow_the_kind() {
        let th = Theme::dark();
        assert_eq!(sheet_tag(Kind::Question, &th).0, "Decision");
        assert_eq!(sheet_tag(Kind::Stuck, &th).0, "Agent failed");
        assert_eq!(sheet_tag(Kind::Ask, &th).0, "Asked in chat");
        assert_eq!(sheet_tag(Kind::Review, &th).0, "Review");
    }

    #[test]
    fn risk_line_needs_a_known_risk() {
        assert_eq!(risk_line("low", Some(true)).as_deref(), Some("Low risk \u{b7} reversible"));
        assert_eq!(risk_line("high", Some(false)).as_deref(), Some("High risk \u{b7} can't be undone"));
        assert_eq!(risk_line("medium", None).as_deref(), Some("Medium risk"));
        assert_eq!(risk_line("unknown", Some(true)), None);
    }

    #[test]
    fn deploy_command_drops_the_lead() {
        assert_eq!(deploy_command("Runs: make deploy"), "make deploy");
        assert_eq!(deploy_command("make deploy"), "make deploy");
    }
}
