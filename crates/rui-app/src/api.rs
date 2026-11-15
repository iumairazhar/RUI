//! Blocking HTTP client for the RUI daemon.

use anyhow::Result;
use rui_core::{ChatMessage, Client, Event, Question, SeatInfo, Task, TaskRun};
use std::time::Duration;

/// Base URL of the daemon (`RUI_URL` or the default local address).
pub fn base_url() -> String {
    std::env::var("RUI_URL").unwrap_or_else(|_| "http://127.0.0.1:7878".into())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_recv_response(Some(Duration::from_secs(5)))
        .timeout_recv_body(Some(Duration::from_secs(5)))
        .build()
        .into()
}

fn get(path: &str) -> Result<serde_json::Value> {
    let url = format!("{}{}", base_url(), path);
    Ok(agent().get(&url).call()?.body_mut().read_json()?)
}

fn post(path: &str, body: serde_json::Value) -> Result<()> {
    let url = format!("{}{}", base_url(), path);
    agent().post(&url).send_json(body)?;
    Ok(())
}

/// Fetch all clients.
pub fn get_clients() -> Result<Vec<Client>> {
    Ok(serde_json::from_value(get("/clients")?)?)
}

/// Fetch all tasks.
pub fn get_tasks() -> Result<Vec<Task>> {
    Ok(serde_json::from_value(get("/tasks")?)?)
}

/// Fetch open questions.
pub fn get_questions_open() -> Result<Vec<Question>> {
    Ok(serde_json::from_value(get("/questions?status=open")?)?)
}

/// Fetch the latest 200 chat messages.
pub fn get_chat() -> Result<Vec<ChatMessage>> {
    Ok(serde_json::from_value(get("/chat?limit=200")?)?)
}

/// Fetch the signed-in seat info.
pub fn get_seat() -> Result<SeatInfo> {
    Ok(serde_json::from_value(get("/seat")?)?)
}

/// Fetch the latest run (model and seat) per task.
pub fn get_task_runs() -> Result<Vec<TaskRun>> {
    Ok(serde_json::from_value(get("/tasks/runs")?)?)
}

/// Fetch events oldest first: those after `after_id`, or the newest `limit` if `None`.
pub fn get_events_after(after_id: Option<i64>, limit: usize) -> Result<Vec<Event>> {
    let path = match after_id {
        Some(id) => format!("/events?after_id={id}&limit={limit}"),
        None => format!("/events?limit={limit}"),
    };
    Ok(serde_json::from_value(get(&path)?)?)
}

/// Post a user chat message.
pub fn post_chat(content: &str) -> Result<()> {
    post("/chat", serde_json::json!({ "content": content }))
}

/// Answer a question by id.
pub fn post_answer(question_id: i64, answer: &str) -> Result<()> {
    let path = format!("/questions/{question_id}/answer");
    post(&path, serde_json::json!({ "answer": answer }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_core::{ChatRole, QuestionStatus, TaskStatus};

    #[test]
    fn parses_client() {
        let j = r#"[{"id":1,"name":"acme","repo_path":"/repo","test_command":null,
            "allowed_commands":["cargo test"],"created_at":"2026-01-01T00:00:00Z"}]"#;
        let v: Vec<Client> = serde_json::from_str(j).unwrap();
        assert_eq!(v[0].name, "acme");
        assert_eq!(v[0].allowed_commands, vec!["cargo test"]);
    }

    #[test]
    fn parses_task() {
        let j = r#"[{"id":2,"client_id":1,"parent_id":null,"title":"t","body":"",
            "status":"in_progress","done_command":null,"max_loops":5,"loops_used":1,
            "role":"lead","branch":null,"source_kind":"user","source_ref":null,
            "depends_on":[1],"created_at":"a","updated_at":"b"}]"#;
        let v: Vec<Task> = serde_json::from_str(j).unwrap();
        assert_eq!(v[0].status, TaskStatus::InProgress);
        assert_eq!(v[0].depends_on, vec![1]);
    }

    #[test]
    fn parses_question_and_chat() {
        let q = r#"[{"id":3,"task_id":2,"asked_by":"lead","text":"why?","answer":null,
            "status":"open","created_at":"a"}]"#;
        let v: Vec<Question> = serde_json::from_str(q).unwrap();
        assert_eq!(v[0].status, QuestionStatus::Open);
        let c = r#"[{"id":1,"role":"user","content":"hi","created_at":"a"}]"#;
        let v: Vec<ChatMessage> = serde_json::from_str(c).unwrap();
        assert_eq!(v[0].role, ChatRole::User);
    }
}
