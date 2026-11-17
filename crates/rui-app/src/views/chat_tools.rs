//! Tool-call rows shown above Rui's replies in a chat, Claude Code style.

use super::activity::parse_secs;
use crate::theme::Theme;
use gpui::{AnyElement, Div, FontWeight, SharedString, Stateful, div, prelude::*, px};
use rui_core::Event;
use serde_json::Value;
use std::collections::HashMap;

const HELD_MARK: &str = "Held for the owner's OK";
const APPROVE_TOOL: &str = "mcp__rui__approve";
const MAX_SUMMARY: usize = 90;
const MAX_LINES: usize = 40;

/// How a tool call ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum CallState {
    Running,
    Ok,
    Error,
    Held,
}

/// One tool call Rui made, with its result once it arrived.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ToolCall {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub output: String,
    pub state: CallState,
}

/// Name shown for a tool: the `mcp__rui__` prefix is dropped, builtins stay as they are.
pub(super) fn display_name(name: &str) -> String {
    name.strip_prefix("mcp__rui__").unwrap_or(name).to_string()
}

/// Cut text to `max` characters with an ellipsis.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}\u{2026}")
}

/// First string value in a tool input, or empty.
fn first_string(input: &Value) -> String {
    let Some(map) = input.as_object() else { return String::new() };
    map.values().find_map(|v| v.as_str()).unwrap_or("").to_string()
}

/// One-line, human summary of what a tool call does.
pub(super) fn summary(name: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    let text = match display_name(name).as_str() {
        "Bash" => s("command").lines().next().unwrap_or("").to_string(),
        "Read" => s("file_path"),
        "Glob" => s("pattern"),
        "Grep" => match s("path") {
            p if p.is_empty() => s("pattern"),
            p => format!("{} in {p}", s("pattern")),
        },
        "WebSearch" => s("query"),
        "WebFetch" => s("url"),
        "staff_task" => s("title"),
        _ => first_string(input).lines().next().unwrap_or("").to_string(),
    };
    cut(&text, MAX_SUMMARY)
}

/// Text of a `tool_result` content: a string, or the joined text blocks.
fn result_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let parts = content.as_array().into_iter().flatten();
    let texts: Vec<&str> = parts.filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
    texts.join("\n")
}

/// State of a call from its result: none means still running.
fn state_of(result: Option<&(String, bool)>) -> CallState {
    match result {
        None => CallState::Running,
        Some((text, true)) if text.contains(HELD_MARK) => CallState::Held,
        Some((_, true)) => CallState::Error,
        Some(_) => CallState::Ok,
    }
}

/// Rui's events of one chat inside a window, as content blocks of the given event kind.
fn blocks_of(events: &[Event], chat_id: i64, from: i64, to: i64, kind: &str) -> Vec<Value> {
    let in_turn = |e: &&Event| {
        let at = parse_secs(&e.created_at).unwrap_or(i64::MIN);
        e.agent == "rui" && e.kind == kind && e.chat_id == Some(chat_id) && (from..=to).contains(&at)
    };
    let content = |e: &Event| e.payload["message"]["content"].as_array().cloned().unwrap_or_default();
    events.iter().filter(in_turn).flat_map(content).collect()
}

/// Results by tool_use id: the text and whether it was an error.
pub(super) fn results_by_id(events: &[Event], chat_id: i64, from: i64, to: i64) -> HashMap<String, (String, bool)> {
    let blocks = blocks_of(events, chat_id, from, to, "toolresult");
    let results = blocks.iter().filter(|b| b["type"] == "tool_result");
    let pair = |b: &Value| (b["tool_use_id"].as_str().unwrap_or("").to_string(), (result_text(&b["content"]), b["is_error"] == true));
    results.map(pair).collect()
}

/// Tool calls Rui made in a chat between two moments (unix seconds, inclusive), in order.
pub(super) fn tool_calls(events: &[Event], chat_id: i64, from: i64, to: i64) -> Vec<ToolCall> {
    let results = results_by_id(events, chat_id, from, to);
    let blocks = blocks_of(events, chat_id, from, to, "assistant");
    let uses = blocks.iter().filter(|b| b["type"] == "tool_use" && b["name"] != APPROVE_TOOL);
    let call = |b: &Value| {
        let id = b["id"].as_str().unwrap_or("").to_string();
        let name = b["name"].as_str().unwrap_or("");
        let result = results.get(&id);
        let output = result.map(|r| r.0.clone()).unwrap_or_default();
        ToolCall { name: display_name(name), summary: summary(name, &b["input"]), output, state: state_of(result), id }
    };
    uses.map(call).collect()
}

/// Output cut to the first lines, "No output" when empty.
fn output_text(output: &str) -> String {
    if output.trim().is_empty() {
        return "No output".to_string();
    }
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= MAX_LINES {
        return output.to_string();
    }
    format!("{}\n\u{2026}", lines[..MAX_LINES].join("\n"))
}

/// Status dot color of a call.
fn dot_color(state: CallState, th: &Theme) -> gpui::Hsla {
    match state {
        CallState::Running => th.run.base,
        CallState::Ok => th.done.base,
        CallState::Error => th.block.base,
        CallState::Held => th.wait.base,
    }
}

/// The clickable header row of one tool call; the caller adds the click handler.
pub(super) fn tool_row_head(id: String, c: &ToolCall, th: &Theme) -> Stateful<Div> {
    let dot = div().size(px(6.)).flex_none().rounded_full().bg(dot_color(c.state, th));
    let name = div().flex_none().font_family("Menlo").text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(c.name.clone());
    let text = div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis();
    let text = text.font_family("Menlo").text_size(px(11.5)).text_color(th.muted).child(c.summary.clone());
    let tag = (c.state == CallState::Held).then(|| div().flex_none().text_size(px(10.5)).text_color(th.wait.fg).child("Held for your OK"));
    let row = div().id(SharedString::from(id)).h(px(26.)).px(px(8.)).flex().items_center().gap(px(8.)).cursor_pointer();
    row.rounded(px(6.)).bg(th.well).border_1().border_color(th.hairline).child(dot).child(name).child(text).children(tag)
}

/// The expanded output block of one tool call.
pub(super) fn tool_output(id: String, c: &ToolCall, th: &Theme) -> AnyElement {
    let body = div().id(SharedString::from(id)).p(px(8.)).rounded(px(6.)).bg(th.stage).font_family("Menlo").text_size(px(11.));
    body.text_color(th.fg_2).child(output_text(&c.output)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(kind: &str, at: &str, content: Value) -> Event {
        Event { id: 1, task_id: None, chat_id: Some(1), agent: "rui".into(), kind: kind.into(), payload: json!({"message": {"content": content}}), created_at: at.into() }
    }

    fn window() -> (i64, i64) {
        (parse_secs("2026-11-15 10:00:00").unwrap(), parse_secs("2026-11-15 10:01:00").unwrap())
    }

    fn call_ev(id: &str, name: &str) -> Event {
        ev("assistant", "2026-11-15T10:00:05Z", json!([{"type":"tool_use","id":id,"name":name,"input":{"command":"ls\nmore"}}]))
    }

    fn res_ev(id: &str, content: Value, err: bool) -> Event {
        ev("toolresult", "2026-11-15T10:00:06Z", json!([{"type":"tool_result","tool_use_id":id,"content":content,"is_error":err}]))
    }

    #[test]
    fn ok_error_held_running_states() {
        let (from, to) = window();
        let events = vec![
            call_ev("a", "Bash"), call_ev("b", "Bash"), call_ev("c", "mcp__rui__staff_task"), call_ev("d", "Read"),
            res_ev("a", json!("done"), false),
            res_ev("b", json!([{"type":"text","text":"boom"}]), true),
            res_ev("c", json!("Held for the owner's OK: x"), true),
        ];
        let calls = tool_calls(&events, 1, from, to);
        let states: Vec<CallState> = calls.iter().map(|c| c.state).collect();
        assert_eq!(states, vec![CallState::Ok, CallState::Error, CallState::Held, CallState::Running]);
        assert_eq!(calls[0].output, "done");
        assert_eq!(calls[1].output, "boom");
    }

    #[test]
    fn keeps_order_matches_ids_and_skips_approve() {
        let (from, to) = window();
        let events = vec![call_ev("a", "Bash"), call_ev("x", APPROVE_TOOL), call_ev("b", "Read"), res_ev("b", json!("two"), false), res_ev("a", json!("one"), false)];
        let calls = tool_calls(&events, 1, from, to);
        let got: Vec<(&str, &str)> = calls.iter().map(|c| (c.name.as_str(), c.output.as_str())).collect();
        assert_eq!(got, vec![("Bash", "one"), ("Read", "two")]);
        assert!(tool_calls(&events, 2, from, to).is_empty());
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("mcp__rui__staff_task"), "staff_task");
        assert_eq!(display_name("Bash"), "Bash");
    }

    #[test]
    fn summaries() {
        assert_eq!(summary("Bash", &json!({"command":"ls -la\nfoo"})), "ls -la");
        assert_eq!(summary("Read", &json!({"file_path":"/a/b"})), "/a/b");
        assert_eq!(summary("Glob", &json!({"pattern":"*.rs"})), "*.rs");
        assert_eq!(summary("Grep", &json!({"pattern":"foo","path":"src"})), "foo in src");
        assert_eq!(summary("Grep", &json!({"pattern":"foo"})), "foo");
        assert_eq!(summary("WebSearch", &json!({"query":"q"})), "q");
        assert_eq!(summary("WebFetch", &json!({"url":"http://x"})), "http://x");
        assert_eq!(summary("mcp__rui__staff_task", &json!({"title":"T","brief":"b"})), "T");
        assert_eq!(summary("mcp__rui__other", &json!({"n":1,"s":"first"})), "first");
        assert_eq!(summary("mcp__rui__other", &json!({})), "");
        let long = summary("Bash", &json!({"command":"x".repeat(200)}));
        assert_eq!(long.chars().count(), MAX_SUMMARY + 1);
        assert!(long.ends_with('\u{2026}'));
    }

    #[test]
    fn output_is_cut_or_placeholder() {
        assert_eq!(output_text("  "), "No output");
        let many = (0..60).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(output_text(&many).lines().count(), MAX_LINES + 1);
    }
}
