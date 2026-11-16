//! In-memory live view of what an agent is typing right now (never stored in the DB).

use crate::runner::ParsedLine;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const MAX_TEXT: usize = 8000;
const MAX_TOOL_INPUT: usize = 2000;

/// What one running turn is doing at this moment.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct LiveState {
    pub text: String,
    pub thinking: bool,
    pub tool: Option<String>,
    pub tool_input: String,
    /// RFC3339 UTC time of the last update.
    pub at: String,
}

/// Which run a live state belongs to.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum LiveKey {
    Task(i64),
    Chat(i64),
}

/// Live states of the turns currently running.
pub type Live = Arc<Mutex<HashMap<LiveKey, LiveState>>>;

/// Keeps only the last `max` characters of `s`.
fn cap_tail(s: &mut String, max: usize) {
    let extra = s.chars().count().saturating_sub(max);
    if extra == 0 {
        return;
    }
    let cut = s.char_indices().nth(extra).map(|(i, _)| i).unwrap_or(0);
    s.replace_range(..cut, "");
}

/// Keeps only the first `max` characters of `s`.
fn cap_head(s: &mut String, max: usize) {
    if let Some((i, _)) = s.char_indices().nth(max) {
        s.truncate(i);
    }
}

fn block_start(state: &mut LiveState, block: &Value) {
    match block.get("type").and_then(Value::as_str) {
        Some("tool_use") => {
            state.tool = block.get("name").and_then(Value::as_str).map(String::from);
            state.tool_input.clear();
        }
        Some("thinking") => state.thinking = true,
        Some("text") => state.thinking = false,
        _ => {}
    }
}

fn block_delta(state: &mut LiveState, delta: &Value) {
    let s = |k: &str| delta.get(k).and_then(Value::as_str).unwrap_or_default();
    match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => {
            state.text.push_str(s("text"));
            cap_tail(&mut state.text, MAX_TEXT);
        }
        Some("thinking_delta") => state.thinking = true,
        Some("input_json_delta") => {
            state.tool_input.push_str(s("partial_json"));
            cap_head(&mut state.tool_input, MAX_TOOL_INPUT);
        }
        _ => {}
    }
}

/// Applies one inner stream event to the state.
pub fn apply(state: &mut LiveState, event: &Value) {
    match event.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            state.text.clear();
            state.thinking = false;
        }
        Some("content_block_start") => block_start(state, &event["content_block"]),
        Some("content_block_delta") => block_delta(state, &event["delta"]),
        _ => {}
    }
}

/// Updates the live state for `key` from one parsed line.
pub fn on_line(live: &Live, key: LiveKey, p: &ParsedLine) {
    let Ok(mut map) = live.lock() else { return };
    match p {
        ParsedLine::Stream(ev) => apply(map.entry(key).or_default(), ev),
        ParsedLine::Assistant(_) => {
            let Some(s) = map.get_mut(&key) else { return };
            s.text.clear();
            s.thinking = false;
        }
        ParsedLine::ToolResult(_) => {
            let Some(s) = map.get_mut(&key) else { return };
            s.tool = None;
            s.tool_input.clear();
        }
        _ => return,
    }
    if let Some(s) = map.get_mut(&key) {
        s.at = now_rfc3339();
    }
}

/// Removes the live state when a turn ends.
pub fn finish(live: &Live, key: LiveKey) {
    if let Ok(mut map) = live.lock() {
        map.remove(&key);
    }
}

/// Calls `finish` when it goes out of scope, so errors and aborts also clear the state.
pub struct LiveGuard(pub Live, pub LiveKey);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        finish(&self.0, self.1);
    }
}

/// All live states as `{"tasks": {"<id>": state}, "chats": {"<id>": state}}`.
pub fn snapshot(live: &Live) -> Value {
    let (mut tasks, mut chats) = (serde_json::Map::new(), serde_json::Map::new());
    if let Ok(map) = live.lock() {
        for (key, state) in map.iter() {
            let v = json!(state);
            match key {
                LiveKey::Task(id) => tasks.insert(id.to_string(), v),
                LiveKey::Chat(id) => chats.insert(id.to_string(), v),
            };
        }
    }
    json!({"tasks": tasks, "chats": chats})
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Converts days since 1970-01-01 to (year, month, day).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_start_clears_text() {
        let mut s = LiveState { text: "old".into(), thinking: true, ..Default::default() };
        apply(&mut s, &json!({"type": "message_start"}));
        assert_eq!((s.text.as_str(), s.thinking), ("", false));
    }

    #[test]
    fn tool_use_start_and_input_deltas() {
        let mut s = LiveState { tool_input: "x".into(), ..Default::default() };
        let start = json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"Bash","input":{}}});
        apply(&mut s, &start);
        assert_eq!(s.tool.as_deref(), Some("Bash"));
        assert_eq!(s.tool_input, "");
        let d = |p: &str| json!({"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":p}});
        apply(&mut s, &d("{\"comm"));
        apply(&mut s, &d("and\"}"));
        assert_eq!(s.tool_input, "{\"command\"}");
        apply(&mut s, &d(&"z".repeat(3000)));
        assert_eq!(s.tool_input.chars().count(), 2000);
    }

    #[test]
    fn text_delta_appends_and_keeps_tail() {
        let mut s = LiveState::default();
        let d = |t: &str| json!({"type":"content_block_delta","delta":{"type":"text_delta","text":t}});
        apply(&mut s, &d("Hel"));
        apply(&mut s, &d("lo"));
        assert_eq!(s.text, "Hello");
        apply(&mut s, &d(&"a".repeat(9000)));
        apply(&mut s, &d("END"));
        assert_eq!(s.text.chars().count(), 8000);
        assert!(s.text.ends_with("aaEND"));
    }

    #[test]
    fn thinking_start_and_delta() {
        let mut s = LiveState::default();
        let start = json!({"type":"content_block_start","content_block":{"type":"thinking"}});
        apply(&mut s, &start);
        assert!(s.thinking);
        apply(&mut s, &json!({"type":"content_block_start","content_block":{"type":"text","text":""}}));
        assert!(!s.thinking);
        let d = json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"secret"}});
        apply(&mut s, &d);
        assert!(s.thinking);
        assert_eq!(s.text, "");
    }

    fn state_of(live: &Live, key: LiveKey) -> Option<LiveState> {
        live.lock().unwrap().get(&key).cloned()
    }

    #[test]
    fn on_line_clears_on_assistant_and_tool_result() {
        let live = Live::default();
        let key = LiveKey::Task(1);
        let line = |v: Value| crate::runner::parse_line(&v.to_string()).unwrap();
        let text = json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}});
        let tool = json!({"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"Read"}}});
        on_line(&live, key, &line(text));
        on_line(&live, key, &line(tool));
        let s = state_of(&live, key).unwrap();
        assert_eq!((s.text.as_str(), s.tool.as_deref()), ("hi", Some("Read")));
        assert!(!s.at.is_empty());
        on_line(&live, key, &line(json!({"type":"assistant"})));
        let s = state_of(&live, key).unwrap();
        assert_eq!((s.text.as_str(), s.tool.as_deref()), ("", Some("Read")));
        on_line(&live, key, &line(json!({"type":"user"})));
        assert_eq!(state_of(&live, key).unwrap().tool, None);
        finish(&live, key);
        assert!(state_of(&live, key).is_none());
    }

    #[test]
    fn guard_clears_key_and_snapshot_shape() {
        let live = Live::default();
        live.lock().unwrap().insert(LiveKey::Task(7), LiveState { text: "a".into(), ..Default::default() });
        live.lock().unwrap().insert(LiveKey::Chat(2), LiveState::default());
        let v = snapshot(&live);
        assert_eq!(v["tasks"]["7"]["text"], "a");
        assert_eq!(v["chats"]["2"]["thinking"], false);
        {
            let _g = LiveGuard(live.clone(), LiveKey::Task(7));
        }
        assert!(snapshot(&live)["tasks"].as_object().unwrap().is_empty());
    }

    #[test]
    fn timestamp_formats_known_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(now_rfc3339().len(), 20);
    }
}
