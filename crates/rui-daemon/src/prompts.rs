//! Fills `{name}` placeholders in the prompt templates under `prompts/`.
//! The `*_VARS` lists name every placeholder the code fills; a test keeps them in step
//! with the templates, so a new placeholder can't reach an agent as literal text.

/// Placeholders filled in `builder.md`.
pub const BUILDER_VARS: [&str; 6] = ["role", "title", "job", "done_command", "progress_file", "playbook"];
/// Placeholders filled in `decide.md`.
pub const DECIDE_VARS: [&str; 8] =
    ["question", "member_role", "subtask_title", "lead_why", "brief", "client_playbook", "global_playbook", "prior_rulings"];
/// Placeholders filled in `rui.md`.
pub const RUI_VARS: [&str; 1] = ["role_catalog"];

/// Placeholders filled in `account_lead.md`.
pub const ACCOUNT_LEAD_VARS: [&str; 5] = ["client", "event", "client_log", "client_playbook", "pod_roles_and_skills"];

/// What a placeholder gets when there is nothing to say yet.
pub const NONE_YET: &str = "(none yet)";

/// Replace each `{key}` with its value. Unknown placeholders are left as they are.
pub fn fill(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        let value = if value.trim().is_empty() { NONE_YET } else { value.as_str() };
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

/// Names of `{snake_case}` placeholders still in `text`.
pub fn unfilled(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (i, _) in text.match_indices('{') {
        let rest = &text[i + 1..];
        let Some(end) = rest.find('}') else { continue };
        let name = &rest[..end];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            names.push(name.to_string());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(keys: &[&'static str]) -> Vec<(&'static str, String)> {
        keys.iter().map(|k| (*k, "x".to_string())).collect()
    }

    #[test]
    fn fills_known_and_marks_empty_values() {
        let vars = [("a", "1".to_string()), ("b", " ".to_string())];
        assert_eq!(fill("{a} {b} {c} {\"json\": 1}", &vars), "1 (none yet) {c} {\"json\": 1}");
    }

    #[test]
    fn finds_placeholders_only() {
        assert_eq!(unfilled("a {role} b {\"k\": 1} {Not} {x_y}"), vec!["role", "x_y"]);
    }

    #[test]
    fn every_template_placeholder_is_filled() {
        let cases = [
            (include_str!("../prompts/builder.md"), all(&BUILDER_VARS)),
            (include_str!("../prompts/decide.md"), all(&DECIDE_VARS)),
            (include_str!("../prompts/rui.md"), all(&RUI_VARS)),
            (include_str!("../prompts/account_lead.md"), all(&ACCOUNT_LEAD_VARS)),
        ];
        for (template, vars) in cases {
            assert_eq!(unfilled(&fill(template, &vars)), Vec::<String>::new());
        }
    }
}
