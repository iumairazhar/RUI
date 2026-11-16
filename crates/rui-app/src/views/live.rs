//! Turns the daemon's live state into a short "what is it doing" label. No UI here.

use super::trace::{clean_command, rel_path};
use crate::api::LiveState;

/// Value of a string field in possibly incomplete JSON: up to the next unescaped quote or the end.
fn partial_field(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            },
            _ => out.push(c),
        }
    }
    Some(out).filter(|s| !s.is_empty())
}

fn first_field(json: &str, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| partial_field(json, k))
}

/// Last segment of an `mcp__server__tool` name, underscores as spaces.
fn mcp_short(name: &str) -> String {
    name.rsplit("__").next().unwrap_or(name).replace('_', " ")
}

fn tool_action(name: &str, input: &str) -> (String, Option<String>) {
    match name {
        "Bash" => ("Running".into(), partial_field(input, "command").map(|c| clean_command(&c)).filter(|c| !c.is_empty())),
        "Edit" | "Write" | "MultiEdit" => ("Editing".into(), partial_field(input, "file_path").map(|p| rel_path(&p))),
        "Read" | "Glob" | "Grep" => ("Reading".into(), first_field(input, &["file_path", "pattern"]).map(|p| rel_path(&p))),
        n if n.starts_with("mcp__") => ("Using".into(), Some(mcp_short(n))),
        n => ("Using".into(), Some(n.to_string())),
    }
}

/// Label and optional code for what a running turn is doing right now.
pub fn live_action(s: &LiveState) -> (String, Option<String>) {
    if let Some(tool) = s.tool.as_deref() {
        return tool_action(tool, &s.tool_input);
    }
    if s.thinking {
        return ("Thinking".into(), None);
    }
    if !s.text.trim().is_empty() {
        return ("Writing".into(), None);
    }
    ("Working".into(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, input: &str) -> LiveState {
        LiveState { tool: Some(name.into()), tool_input: input.into(), ..Default::default() }
    }

    fn act(s: &LiveState) -> (String, Option<String>) {
        live_action(s)
    }

    #[test]
    fn bash_command_partial_and_complete() {
        let want = |c: &str| ("Running".to_string(), Some(c.to_string()));
        assert_eq!(act(&tool("Bash", r#"{"command":"cargo te"#)), want("cargo te"));
        assert_eq!(act(&tool("Bash", r#"{"command":"cargo test","timeout":5}"#)), want("cargo test"));
        assert_eq!(act(&tool("Bash", r#"{"command":"echo \"a\" && ls"}"#)), want(r#"echo "a" && ls"#));
        assert_eq!(act(&tool("Bash", r#"{"command":"cd /x/worktrees/7 && ls\nmore"}"#)), want("ls"));
        assert_eq!(act(&tool("Bash", r#"{"comm"#)), ("Running".into(), None));
        assert_eq!(act(&tool("Bash", "")), ("Running".into(), None));
    }

    #[test]
    fn edit_shortens_worktree_path() {
        let p = r#"{"file_path":"/Users/u/RUI/worktrees/7/src/lib.rs","old_string":"x"}"#;
        let want = ("Editing".to_string(), Some("src/lib.rs".to_string()));
        assert_eq!(act(&tool("Edit", p)), want);
        assert_eq!(act(&tool("Write", p)), want);
        assert_eq!(act(&tool("MultiEdit", p)), want);
        assert_eq!(act(&tool("Edit", r#"{"file_path":"/a/b.rs"#)), ("Editing".into(), Some("/a/b.rs".into())));
    }

    #[test]
    fn reading_uses_path_or_pattern() {
        let read = ("Reading".to_string(), Some("a.rs".to_string()));
        assert_eq!(act(&tool("Read", r#"{"file_path":"a.rs"}"#)), read);
        let grep = ("Reading".to_string(), Some("fn main".to_string()));
        assert_eq!(act(&tool("Grep", r#"{"pattern":"fn main","path":"."}"#)), grep);
        assert_eq!(act(&tool("Glob", "{")), ("Reading".into(), None));
    }

    #[test]
    fn mcp_and_other_tools() {
        assert_eq!(act(&tool("mcp__rui__create_task", "{}")), ("Using".into(), Some("create task".into())));
        assert_eq!(act(&tool("WebFetch", "{}")), ("Using".into(), Some("WebFetch".into())));
    }

    #[test]
    fn no_tool_states() {
        let s = LiveState { thinking: true, text: "x".into(), ..Default::default() };
        assert_eq!(act(&s), ("Thinking".into(), None));
        let s = LiveState { text: "hello".into(), ..Default::default() };
        assert_eq!(act(&s), ("Writing".into(), None));
        let s = LiveState { text: "  \n".into(), ..Default::default() };
        assert_eq!(act(&s), ("Working".into(), None));
        assert_eq!(act(&LiveState::default()), ("Working".into(), None));
    }
}
