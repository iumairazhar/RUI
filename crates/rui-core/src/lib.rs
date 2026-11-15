use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: i64 = 1;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some(Self::$variant),)+ _ => None }
            }

            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}

string_enum!(TaskStatus {
    New => "new",
    Clarifying => "clarifying",
    WaitingAnswer => "waiting_answer",
    Split => "split",
    Open => "open",
    InProgress => "in_progress",
    Done => "done",
    Stuck => "stuck",
});
string_enum!(Role { Rui => "rui", Lead => "lead", Builder => "builder" });
string_enum!(SourceKind { Chat => "chat", User => "user" });
string_enum!(QuestionStatus { Open => "open", Answered => "answered" });
string_enum!(ChatRole { User => "user", Rui => "rui" });

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Client {
    pub id: i64,
    pub name: String,
    pub repo_path: String,
    pub test_command: Option<String>,
    pub allowed_commands: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub client_id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
    pub body: String,
    pub status: TaskStatus,
    pub done_command: Option<String>,
    pub max_loops: i64,
    pub loops_used: i64,
    pub role: Role,
    pub branch: Option<String>,
    pub source_kind: SourceKind,
    pub source_ref: Option<String>,
    pub depends_on: Vec<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: i64,
    pub task_id: i64,
    pub asked_by: Role,
    pub text: String,
    pub answer: Option<String>,
    pub status: QuestionStatus,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub role: ChatRole,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub task_id: Option<i64>,
    pub agent: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub role: Role,
    pub task_id: Option<i64>,
    pub session_id: String,
}

/// Which account the agent CLI is signed in with, plus the last model used.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SeatInfo {
    pub logged_in: bool,
    pub auth_method: Option<String>,
    pub email: Option<String>,
    pub subscription_type: Option<String>,
    pub org_name: Option<String>,
    pub last_model: Option<String>,
    pub error: Option<String>,
}

/// The latest agent run for a task: which model and seat kind it used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRun {
    pub task_id: i64,
    pub agent: String,
    pub model: Option<String>,
    /// "subscription", "api_key" or "unknown".
    pub seat: String,
    pub at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! check_enum {
        ($t:ident) => {{
            for v in $t::ALL {
                assert_eq!($t::parse(v.as_str()), Some(*v));
                let json = serde_json::to_string(v).unwrap();
                assert_eq!(json.trim_matches('"'), v.as_str());
            }
            assert_eq!($t::parse("bogus"), None);
        }};
    }

    #[test]
    fn enums_round_trip() {
        check_enum!(TaskStatus);
        check_enum!(Role);
        check_enum!(SourceKind);
        check_enum!(QuestionStatus);
        check_enum!(ChatRole);
    }
}
