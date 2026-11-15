//! Activity panel: a live feed of agent events.

use crate::store::{self, Store};
use super::{page_head, tag};
use crate::theme::{Status, Theme, theme};
use gpui::{Context, Entity, ScrollHandle, Window, div, prelude::*, px};
use rui_core::Event;
use serde_json::Value;

/// Scrolling list of recent events, pinned to the bottom as new ones arrive.
pub struct ActivityView {
    store: Entity<Store>,
    scroll: ScrollHandle,
    last_id: Option<i64>,
}

impl ActivityView {
    /// Build the activity view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self { store, scroll: ScrollHandle::new(), last_id: None }
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn one_line(s: &str, max: usize) -> String {
    truncate(&s.replace(['\n', '\r'], " "), max)
}

fn content_blocks(payload: &Value) -> Vec<&Value> {
    payload["message"]["content"].as_array().map(|a| a.iter().collect()).unwrap_or_default()
}

fn tool_use_part(block: &Value) -> String {
    let name = block["name"].as_str().unwrap_or("tool");
    let name = name.strip_prefix("mcp__rui__").unwrap_or(name);
    let input = if block["input"].is_null() { String::new() } else { block["input"].to_string() };
    format!("→ {name}({})", truncate(&input, 80))
}

fn summarize_assistant(payload: &Value) -> String {
    let blocks = content_blocks(payload);
    let text: Vec<&str> = blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(one_line(&text.join(" "), 200));
    }
    parts.extend(blocks.iter().filter(|b| b["type"] == "tool_use").map(|b| tool_use_part(b)));
    if parts.is_empty() { "assistant".into() } else { parts.join("  ") }
}

fn result_text(block: &Value) -> String {
    match &block["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" ")
        }
        _ => String::new(),
    }
}

fn summarize_toolresult(payload: &Value) -> (String, bool) {
    let blocks = content_blocks(payload);
    let Some(b) = blocks.iter().find(|b| b["is_error"] == true) else {
        return ("← ok".into(), false);
    };
    (format!("← error: {}", one_line(&result_text(b), 120)), true)
}

fn summarize_result(p: &Value) -> (String, bool) {
    if p["is_error"] == true {
        return ("✗ failed".into(), true);
    }
    match p["total_cost_usd"].as_f64() {
        Some(c) => (format!("✓ finished (${c:.2})"), false),
        None => ("✓ finished".into(), false),
    }
}

fn summarize_done_check(p: &Value) -> (String, bool) {
    if p["passed"] == true {
        return ("check passed".into(), false);
    }
    let tail = p["output_tail"].as_str().and_then(|s| s.lines().next()).unwrap_or("");
    if tail.is_empty() {
        return ("check failed".into(), true);
    }
    (format!("check failed {}", truncate(tail, 100)), true)
}

fn str_or<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p[key].as_str().filter(|s| !s.is_empty())
}

fn with_opt(prefix: &str, value: Option<&str>) -> String {
    match value {
        Some(v) => format!("{prefix}: {v}"),
        None => prefix.to_string(),
    }
}

/// Turn an event into a one-line summary plus whether it should be shown as an error.
pub fn summarize(e: &Event) -> (String, bool) {
    let p = &e.payload;
    match e.kind.as_str() {
        "assistant" => (summarize_assistant(p), false),
        "toolresult" => summarize_toolresult(p),
        "init" => match str_or(p, "model") {
            Some(m) => (format!("started · {m}"), false),
            None => ("started".into(), false),
        },
        "result" => summarize_result(p),
        "done_check" => summarize_done_check(p),
        "commit" => ("committed".into(), false),
        "rollup" => (format!("task → {}", str_or(p, "status").unwrap_or("?")), false),
        "recovered" => {
            let from = str_or(p, "from").unwrap_or("?");
            let to = str_or(p, "to").unwrap_or("?");
            (format!("recovered {from} → {to}"), false)
        }
        "agent_error" | "scheduler_error" => (with_opt("error", str_or(p, "error")), true),
        "lead_no_outcome" => ("lead ended without asking or splitting".into(), true),
        "stuck" => (with_opt("stuck", str_or(p, "reason")), true),
        "progress" => (with_opt("progress", str_or(p, "note")), false),
        "tool_call" => (with_opt("tool", str_or(p, "tool")), false),
        other => (other.to_string(), false),
    }
}

fn agent_status<'a>(agent: &str, th: &'a Theme) -> &'a Status {
    match agent {
        "rui" => &th.think,
        "lead" => &th.wait,
        "builder" => &th.run,
        _ => &th.done,
    }
}

fn clock(created_at: &str) -> String {
    created_at.get(11..19).unwrap_or("--:--:--").to_string()
}

fn row(e: &Event, th: &Theme) -> impl IntoElement + use<> {
    let (text, is_error) = summarize(e);
    let task = e.task_id.map(|t| format!("#{t}")).unwrap_or_default();
    let body = if is_error { th.block.fg } else { th.fg };
    div()
        .id(("event", e.id as usize))
        .flex()
        .items_start()
        .gap(px(10.))
        .py(px(6.))
        .px(px(10.))
        .rounded(px(8.))
        .hover(|s| s.bg(th.surface_2))
        .child(
            div()
                .flex_none()
                .font_family("Menlo")
                .text_size(px(10.5))
                .text_color(th.muted)
                .child(clock(&e.created_at)),
        )
        .child(tag(agent_status(&e.agent, th), &e.agent))
        .child(
            div()
                .flex_none()
                .min_w(px(32.))
                .font_family("Menlo")
                .text_size(px(11.))
                .text_color(th.muted)
                .child(task),
        )
        .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(body).child(text))
}

impl Render for ActivityView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let events = self.store.read(cx).events.clone();
        let newest = events.last().map(|e| e.id);
        if newest != self.last_id {
            self.last_id = newest;
            self.scroll.scroll_to_bottom();
        }
        let list = div()
            .flex()
            .flex_col()
            .bg(th.surface)
            .rounded(px(12.))
            .shadow(th.card_shadow())
            .p(px(8.))
            .children(events.iter().map(|e| row(e, &th)));
        let body = div()
            .id("activity-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .mr(px(40.))
            .mb(px(28.))
            .child(list);
        div().flex().flex_col().size_full().child(page_head("LIVE", "Threads", cx)).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(kind: &str, payload: Value) -> Event {
        Event {
            id: 1,
            task_id: Some(2),
            agent: "builder".into(),
            kind: kind.into(),
            payload,
            created_at: "2026-11-15T14:40:00.000Z".into(),
        }
    }

    fn sum(kind: &str, payload: Value) -> (String, bool) {
        summarize(&ev(kind, payload))
    }

    #[test]
    fn assistant_text_and_tool() {
        let p = json!({"type":"assistant","message":{"content":[
            {"type":"text","text":"hi\nthere"},
            {"type":"tool_use","name":"mcp__rui__create_client","input":{"name":"acme"}}]}});
        let (s, err) = sum("assistant", p);
        assert_eq!(s, "hi there  → create_client({\"name\":\"acme\"})");
        assert!(!err);
    }

    #[test]
    fn assistant_truncates_and_missing() {
        let long = "x".repeat(300);
        let p = json!({"message":{"content":[{"type":"text","text":long}]}});
        assert_eq!(sum("assistant", p).0.chars().count(), 200);
        assert_eq!(sum("assistant", json!({})).0, "assistant");
        let big = json!({"message":{"content":[{"type":"tool_use","name":"t","input":{"a":"y".repeat(200)}}]}});
        assert_eq!(sum("assistant", big).0.chars().count(), "→ t()".chars().count() + 80);
    }

    #[test]
    fn toolresult_cases() {
        let ok = json!({"message":{"content":[{"type":"tool_result","content":"fine","is_error":false}]}});
        assert_eq!(sum("toolresult", ok), ("← ok".into(), false));
        let bad = json!({"message":{"content":[{"type":"tool_result",
            "content":[{"type":"text","text":"nope"}],"is_error":true}]}});
        assert_eq!(sum("toolresult", bad), ("← error: nope".into(), true));
        assert_eq!(sum("toolresult", json!({})), ("← ok".into(), false));
    }

    #[test]
    fn init_cases() {
        let p = json!({"type":"system","subtype":"init","model":"claude-opus-5-5"});
        assert_eq!(sum("init", p).0, "started · claude-opus-5-5");
        assert_eq!(sum("init", json!({"session_id":"s"})).0, "started");
    }

    #[test]
    fn result_cases() {
        let ok = json!({"is_error":false,"result":"x","total_cost_usd":0.12});
        assert_eq!(sum("result", ok), ("✓ finished ($0.12)".into(), false));
        assert_eq!(sum("result", json!({"text":"x"})), ("✓ finished".into(), false));
        assert_eq!(sum("result", json!({"is_error":true})), ("✗ failed".into(), true));
    }

    #[test]
    fn done_check_cases() {
        assert_eq!(sum("done_check", json!({"passed":true})), ("check passed".into(), false));
        let bad = json!({"passed":false,"output_tail":"first\nsecond"});
        assert_eq!(sum("done_check", bad), ("check failed first".into(), true));
        assert_eq!(sum("done_check", json!({})), ("check failed".into(), true));
    }

    #[test]
    fn scheduler_kinds() {
        assert_eq!(sum("commit", json!({"committed":true})).0, "committed");
        assert_eq!(sum("rollup", json!({"status":"done"})).0, "task → done");
        assert_eq!(sum("recovered", json!({"from":"a","to":"b"})).0, "recovered a → b");
        assert_eq!(sum("agent_error", json!({"error":"x"})), ("error: x".into(), true));
        assert_eq!(sum("scheduler_error", json!({})), ("error".into(), true));
        assert!(sum("lead_no_outcome", json!({})).1);
        assert_eq!(sum("stuck", json!({"reason":"loop"})), ("stuck: loop".into(), true));
        assert_eq!(sum("stuck", json!({})), ("stuck".into(), true));
        assert_eq!(sum("progress", json!({"note":"n"})).0, "progress: n");
        assert_eq!(sum("progress", json!({})).0, "progress");
        assert_eq!(sum("tool_call", json!({"tool":"t","args":{}})).0, "tool: t");
        assert_eq!(sum("other", json!(null)), ("other".into(), false));
        assert_eq!(sum("weird", json!({})).0, "weird");
    }

    #[test]
    fn clock_slices_time() {
        assert_eq!(clock("2026-11-15T14:40:00.000Z"), "14:40:00");
        assert_eq!(clock("bad"), "--:--:--");
    }
}
