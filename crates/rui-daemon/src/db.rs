//! SQLite access layer. Plain SQL, one query per function. Schema comes from db/migrations.

use anyhow::{Result, anyhow, bail};
use rui_core::{
    Chat, ChatAsk, ChatMessage, ChatRole, Client, Event, Question, QuestionStatus, Role, Rung, SourceKind, Task, TaskStatus,
};
use rusqlite::{Connection, OptionalExtension, Row, params, types::Type};
use rusqlite_migration::{M, Migrations};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";
const TASK_COLS: &str = "id, client_id, parent_id, title, body, status, done_command, max_loops, \
     loops_used, role, branch, source_kind, source_ref, created_at, updated_at, skill, agent, pod";
const CLIENT_COLS: &str = "id, name, repo_path, test_command, allowed_commands, created_at";
const EVENT_COLS: &str = "id, task_id, chat_id, agent, kind, payload, created_at";
const CHAT_COLS: &str = "id, chat_id, role, content, created_at";
const QUESTION_COLS: &str = "id, task_id, asked_by, text, answer, status, created_at, level, ladder, why";

/// One `result` event's spend, as read for the cost report.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultCost {
    pub task_id: Option<i64>,
    pub created_at: String,
    pub usd: f64,
    pub tokens: i64,
    pub seat_id: Option<String>,
}

/// Handle to the SQLite database, safe to share across threads.
pub struct Db {
    conn: Mutex<Connection>,
}

/// Fields needed to insert a new task.
pub struct NewTask {
    pub client_id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
    pub body: String,
    pub status: TaskStatus,
    pub done_command: Option<String>,
    pub max_loops: i64,
    pub role: Role,
    pub source_kind: SourceKind,
    pub source_ref: Option<String>,
    pub agent: Option<String>,
    pub pod: Vec<String>,
}

fn bad(idx: usize, msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, msg.into())
}

fn parse_enum<T>(idx: usize, s: String, f: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    f(&s).ok_or_else(|| bad(idx, format!("unknown enum value '{s}'")))
}

fn client_from_row(r: &Row) -> rusqlite::Result<Client> {
    let cmds: String = r.get(4)?;
    Ok(Client {
        id: r.get(0)?,
        name: r.get(1)?,
        repo_path: r.get(2)?,
        test_command: r.get(3)?,
        allowed_commands: serde_json::from_str(&cmds).map_err(|e| bad(4, e.to_string()))?,
        created_at: r.get(5)?,
    })
}

fn json_list<T: serde::de::DeserializeOwned>(idx: usize, raw: Option<String>) -> rusqlite::Result<Vec<T>> {
    let Some(text) = raw else { return Ok(Vec::new()) };
    serde_json::from_str(&text).map_err(|e| bad(idx, e.to_string()))
}

fn task_from_row(r: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        client_id: r.get(1)?,
        parent_id: r.get(2)?,
        title: r.get(3)?,
        body: r.get(4)?,
        status: parse_enum(5, r.get(5)?, TaskStatus::parse)?,
        done_command: r.get(6)?,
        max_loops: r.get(7)?,
        loops_used: r.get(8)?,
        role: parse_enum(9, r.get(9)?, Role::parse)?,
        branch: r.get(10)?,
        source_kind: parse_enum(11, r.get(11)?, SourceKind::parse)?,
        source_ref: r.get(12)?,
        depends_on: Vec::new(),
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
        skill: r.get(15)?,
        agent: r.get(16)?,
        pod: json_list(17, r.get(17)?)?,
    })
}

fn question_from_row(r: &Row) -> rusqlite::Result<Question> {
    Ok(Question {
        id: r.get(0)?,
        task_id: r.get(1)?,
        asked_by: parse_enum(2, r.get(2)?, Role::parse)?,
        text: r.get(3)?,
        answer: r.get(4)?,
        status: parse_enum(5, r.get(5)?, QuestionStatus::parse)?,
        created_at: r.get(6)?,
        level: r.get(7)?,
        ladder: json_list::<Rung>(8, r.get(8)?)?,
        why: r.get(9)?,
    })
}

fn chat_from_row(r: &Row) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        role: parse_enum(2, r.get(2)?, ChatRole::parse)?,
        content: r.get(3)?,
        created_at: r.get(4)?,
    })
}

fn chat_row(r: &Row) -> rusqlite::Result<Chat> {
    Ok(Chat {
        id: r.get(0)?,
        title: r.get(1)?,
        created_at: r.get(2)?,
        updated_at: r.get(3)?,
        last: r.get(4)?,
    })
}

const DEFAULT_CHAT_TITLE: &str = "New chat";
const TITLE_MAX_CHARS: usize = 44;

/// Chat title taken from the first user message: trimmed, cut at 44 chars.
pub fn chat_title(content: &str) -> String {
    let text = content.trim();
    if text.chars().count() <= TITLE_MAX_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(TITLE_MAX_CHARS).collect();
    format!("{}…", head.trim_end())
}

fn fetch_asks(conn: &Connection, cond: &str, args: impl rusqlite::Params) -> Result<Vec<ChatAsk>> {
    let sql = format!(
        "SELECT a.id, a.chat_id, c.title, a.text, a.options, a.answer, a.status, a.created_at \
         FROM chat_asks a JOIN chats c ON c.id = a.chat_id WHERE {cond} ORDER BY a.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(args, |r| {
        let options: String = r.get(4)?;
        Ok(ChatAsk {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            chat_title: r.get(2)?,
            text: r.get(3)?,
            options: serde_json::from_str(&options).unwrap_or_default(),
            answer: r.get(5)?,
            status: r.get(6)?,
            created_at: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn event_from_row(r: &Row) -> rusqlite::Result<Event> {
    let payload: String = r.get(5)?;
    Ok(Event {
        id: r.get(0)?,
        task_id: r.get(1)?,
        chat_id: r.get(2)?,
        agent: r.get(3)?,
        kind: r.get(4)?,
        payload: serde_json::from_str(&payload).map_err(|e| bad(5, e.to_string()))?,
        created_at: r.get(6)?,
    })
}

/// Task or event ids as a SQL list body. Integers only, so safe to inline.
fn id_list(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// Fill `depends_on` for each task from `task_deps`.
fn fill_deps(conn: &Connection, tasks: &mut [Task]) -> Result<()> {
    let mut stmt = conn.prepare("SELECT depends_on FROM task_deps WHERE task_id = ?1 ORDER BY depends_on")?;
    for t in tasks {
        t.depends_on = stmt
            .query_map([t.id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok(())
}

fn query_tasks(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(sql)?;
    let mut tasks = stmt.query_map(p, task_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    fill_deps(conn, &mut tasks)?;
    Ok(tasks)
}

fn fetch_task(conn: &Connection, id: i64) -> Result<Option<Task>> {
    let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1");
    Ok(query_tasks(conn, &sql, [id])?.pop())
}

fn fetch_question(conn: &Connection, id: i64) -> Result<Option<Question>> {
    let sql = format!("SELECT {QUESTION_COLS} FROM questions WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], question_from_row).optional()?)
}

const CHAT_SELECT: &str = "SELECT c.id, c.title, c.created_at, c.updated_at, \
     (SELECT m.content FROM chat_messages m WHERE m.chat_id = c.id ORDER BY m.id DESC LIMIT 1) FROM chats c";

fn fetch_chat(conn: &Connection, id: i64) -> Result<Option<Chat>> {
    let sql = format!("{CHAT_SELECT} WHERE c.id = ?1");
    Ok(conn.query_row(&sql, [id], chat_row).optional()?)
}

fn fetch_chat_message(conn: &Connection, id: i64) -> Result<Option<ChatMessage>> {
    let sql = format!("SELECT {CHAT_COLS} FROM chat_messages WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], chat_from_row).optional()?)
}

fn fetch_client(conn: &Connection, id: i64) -> Result<Option<Client>> {
    let sql = format!("SELECT {CLIENT_COLS} FROM clients WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], client_from_row).optional()?)
}

/// Migration steps in order. Step N leaves `user_version` at N.
const MIGRATION_SQL: [&str; 5] = [
    include_str!("../../../db/migrations/001_base.sql"),
    include_str!("../../../db/migrations/002_chats.sql"),
    include_str!("../../../db/migrations/003_skills.sql"),
    include_str!("../../../db/migrations/004_teams.sql"),
    include_str!("../../../db/migrations/005_chat_asks.sql"),
];

pub fn migrations() -> Migrations<'static> {
    Migrations::new(MIGRATION_SQL.iter().map(|sql| M::up(sql)).collect())
}

fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Write a full copy of the DB next to itself before it is migrated.
/// `VACUUM INTO` includes anything still in the WAL file, unlike a plain file copy.
fn backup_before_migrating(conn: &Connection, path: &Path, from: i64, to: i64) -> Result<()> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".bak-v{from}-{secs}"));
    let backup = PathBuf::from(name);
    conn.execute("VACUUM INTO ?1", [backup.to_string_lossy()])?;
    eprintln!("migrating database v{from} -> v{to}; backup at {}", backup.display());
    Ok(())
}

impl Db {
    /// Open the database, creating it if missing and applying pending migrations.
    /// An existing file that needs migrating is backed up first.
    pub fn open(path: &Path) -> Result<Db> {
        let existed = path.exists();
        let mut conn = Connection::open(path)?;
        let (version, latest) = (user_version(&conn)?, MIGRATION_SQL.len() as i64);
        if version > latest {
            bail!("database is newer than this daemon (schema v{version}, daemon knows v{latest})");
        }
        if existed && version < latest {
            backup_before_migrating(&conn, path, version, latest)?;
        }
        // SQLite refuses ALTER TABLE ADD COLUMN ... REFERENCES on non-empty tables while foreign keys are on.
        conn.execute_batch("PRAGMA foreign_keys = OFF")?;
        migrations().to_latest(&mut conn)?;
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    /// In-memory database with the migrations applied. Tests only.
    #[cfg(test)]
    pub fn open_test() -> Db {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_latest(&mut conn).unwrap();
        // Migration 002 seeds chat 1 for old databases; tests start with no chats.
        conn.execute("DELETE FROM chats", []).unwrap();
        Db { conn: Mutex::new(conn) }
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| anyhow!("database lock poisoned"))
    }

    // ---- clients ----

    /// Insert a client and return it.
    pub fn create_client(
        &self,
        name: &str,
        repo_path: &str,
        test_command: Option<&str>,
        allowed_commands: &[String],
    ) -> Result<Client> {
        let conn = self.conn()?;
        let cmds = serde_json::to_string(allowed_commands)?;
        conn.execute(
            "INSERT INTO clients (name, repo_path, test_command, allowed_commands) VALUES (?1, ?2, ?3, ?4)",
            params![name, repo_path, test_command, cmds],
        )?;
        fetch_client(&conn, conn.last_insert_rowid())?.ok_or_else(|| anyhow!("client vanished"))
    }

    /// All clients, oldest first.
    pub fn list_clients(&self) -> Result<Vec<Client>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!("SELECT {CLIENT_COLS} FROM clients ORDER BY id"))?;
        Ok(stmt.query_map([], client_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Client by id, if it exists.
    pub fn get_client(&self, id: i64) -> Result<Option<Client>> {
        fetch_client(&*self.conn()?, id)
    }

    /// Client by unique name, if it exists.
    pub fn get_client_by_name(&self, name: &str) -> Result<Option<Client>> {
        let sql = format!("SELECT {CLIENT_COLS} FROM clients WHERE name = ?1");
        Ok(self.conn()?.query_row(&sql, [name], client_from_row).optional()?)
    }

    /// Change a client's name and repo folder; returns the updated client.
    pub fn update_client(&self, id: i64, name: &str, repo_path: &str) -> Result<Client> {
        let conn = self.conn()?;
        conn.execute("UPDATE clients SET name = ?1, repo_path = ?2 WHERE id = ?3", params![name, repo_path, id])?;
        fetch_client(&conn, id)?.ok_or_else(|| anyhow!("client not found"))
    }

    /// Delete a client with its tasks, subtasks and everything that points at those tasks, in one
    /// transaction. Returns false when the client does not exist.
    pub fn delete_client_cascade(&self, id: i64) -> Result<bool> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let mine = "SELECT id FROM tasks WHERE client_id = ?1";
        for table in ["questions", "events", "agent_sessions"] {
            tx.execute(&format!("DELETE FROM {table} WHERE task_id IN ({mine})"), [id])?;
        }
        tx.execute(&format!("DELETE FROM task_deps WHERE task_id IN ({mine}) OR depends_on IN ({mine})"), [id])?;
        tx.execute("DELETE FROM tasks WHERE client_id = ?1", [id])?;
        let deleted = tx.execute("DELETE FROM clients WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(deleted > 0)
    }

    // ---- tasks ----

    /// Insert a task and return it.
    pub fn create_task(&self, t: &NewTask) -> Result<Task> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO tasks (client_id, parent_id, title, body, status, done_command, max_loops, role, source_kind, source_ref, agent, pod) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                t.client_id,
                t.parent_id,
                t.title,
                t.body,
                t.status.as_str(),
                t.done_command,
                t.max_loops,
                t.role.as_str(),
                t.source_kind.as_str(),
                t.source_ref,
                t.agent,
                serde_json::to_string(&t.pod)?
            ],
        )?;
        fetch_task(&conn, conn.last_insert_rowid())?.ok_or_else(|| anyhow!("task vanished"))
    }

    /// Task by id, if it exists.
    pub fn get_task(&self, id: i64) -> Result<Option<Task>> {
        fetch_task(&*self.conn()?, id)
    }

    /// All tasks, oldest first.
    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks ORDER BY id");
        query_tasks(&*self.conn()?, &sql, [])
    }

    /// Tasks currently in the given status.
    pub fn tasks_with_status(&self, status: TaskStatus) -> Result<Vec<Task>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE status = ?1 ORDER BY id");
        query_tasks(&*self.conn()?, &sql, [status.as_str()])
    }

    /// Direct children of a task.
    pub fn subtasks(&self, parent_id: i64) -> Result<Vec<Task>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE parent_id = ?1 ORDER BY id");
        query_tasks(&*self.conn()?, &sql, [parent_id])
    }

    /// Change a task's status.
    pub fn set_task_status(&self, id: i64, status: TaskStatus) -> Result<()> {
        let sql = format!("UPDATE tasks SET status = ?1, updated_at = {NOW} WHERE id = ?2");
        self.conn()?.execute(&sql, params![status.as_str(), id])?;
        Ok(())
    }

    /// Bump loops_used and return the new value.
    pub fn increment_loops(&self, id: i64) -> Result<i64> {
        let sql = format!(
            "UPDATE tasks SET loops_used = loops_used + 1, updated_at = {NOW} WHERE id = ?1 RETURNING loops_used"
        );
        Ok(self.conn()?.query_row(&sql, [id], |r| r.get(0))?)
    }

    /// Record the git branch used for a task.
    pub fn set_task_branch(&self, id: i64, branch: &str) -> Result<()> {
        let sql = format!("UPDATE tasks SET branch = ?1, updated_at = {NOW} WHERE id = ?2");
        self.conn()?.execute(&sql, params![branch, id])?;
        Ok(())
    }

    /// Change a task's loop limit.
    pub fn set_max_loops(&self, id: i64, max_loops: i64) -> Result<()> {
        let sql = format!("UPDATE tasks SET max_loops = ?1, updated_at = {NOW} WHERE id = ?2");
        self.conn()?.execute(&sql, params![max_loops, id])?;
        Ok(())
    }

    /// Change a task's done command.
    pub fn set_done_command(&self, id: i64, done_command: &str) -> Result<()> {
        let sql = format!("UPDATE tasks SET done_command = ?1, updated_at = {NOW} WHERE id = ?2");
        self.conn()?.execute(&sql, params![done_command, id])?;
        Ok(())
    }

    /// Make `task_id` depend on `depends_on`.
    pub fn add_task_dep(&self, task_id: i64, depends_on: i64) -> Result<()> {
        let sql = "INSERT OR IGNORE INTO task_deps (task_id, depends_on) VALUES (?1, ?2)";
        self.conn()?.execute(sql, params![task_id, depends_on])?;
        Ok(())
    }

    // ---- questions ----

    /// Insert an open question and return it.
    pub fn create_question(&self, task_id: i64, asked_by: Role, text: &str) -> Result<Question> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO questions (task_id, asked_by, text) VALUES (?1, ?2, ?3)",
            params![task_id, asked_by.as_str(), text],
        )?;
        fetch_question(&conn, conn.last_insert_rowid())?.ok_or_else(|| anyhow!("question vanished"))
    }

    /// Insert an open question at a given level with its ladder so far.
    pub fn create_ladder_question(
        &self,
        task_id: i64,
        asked_by: Role,
        level: &str,
        ladder: &[Rung],
        text: &str,
    ) -> Result<Question> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO questions (task_id, asked_by, text, level, ladder) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, asked_by.as_str(), text, level, serde_json::to_string(ladder)?],
        )?;
        fetch_question(&conn, conn.last_insert_rowid())?.ok_or_else(|| anyhow!("question vanished"))
    }

    /// Question by id, if it exists.
    pub fn get_question(&self, id: i64) -> Result<Option<Question>> {
        fetch_question(&*self.conn()?, id)
    }

    /// Move a question to another level, replacing its ladder.
    pub fn set_question_level(&self, id: i64, level: &str, ladder: &[Rung]) -> Result<()> {
        let sql = "UPDATE questions SET level = ?1, ladder = ?2 WHERE id = ?3";
        self.conn()?.execute(sql, params![level, serde_json::to_string(ladder)?, id])?;
        Ok(())
    }

    /// Questions, optionally filtered by status, oldest first.
    pub fn list_questions(&self, status: Option<QuestionStatus>) -> Result<Vec<Question>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT {QUESTION_COLS} FROM questions WHERE (?1 IS NULL OR status = ?1) ORDER BY id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([status.map(|s| s.as_str())], question_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// All questions for one task, oldest first.
    pub fn questions_for_task(&self, task_id: i64) -> Result<Vec<Question>> {
        let conn = self.conn()?;
        let sql = format!("SELECT {QUESTION_COLS} FROM questions WHERE task_id = ?1 ORDER BY id");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_map([task_id], question_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Store an answer and mark answered. None if the id does not exist.
    pub fn answer_question(&self, id: i64, answer: &str, why: Option<&str>) -> Result<Option<Question>> {
        let conn = self.conn()?;
        let n = conn.execute(
            "UPDATE questions SET answer = ?1, why = ?2, status = 'answered' WHERE id = ?3",
            params![answer, why, id],
        )?;
        if n == 0 {
            return Ok(None);
        }
        fetch_question(&conn, id)
    }

    /// Clear a question's answer and reason and mark it open again. None if the id does not exist.
    pub fn reopen_question(&self, id: i64) -> Result<Option<Question>> {
        let conn = self.conn()?;
        let sql = "UPDATE questions SET answer = NULL, why = NULL, status = 'open' WHERE id = ?1";
        if conn.execute(sql, [id])? == 0 {
            return Ok(None);
        }
        fetch_question(&conn, id)
    }

    // ---- chat ----

    /// Create an empty chat with the default title.
    pub fn create_chat(&self) -> Result<Chat> {
        let conn = self.conn()?;
        conn.execute("INSERT INTO chats (title) VALUES (?1)", [DEFAULT_CHAT_TITLE])?;
        fetch_chat(&conn, conn.last_insert_rowid())?.ok_or_else(|| anyhow!("chat vanished"))
    }

    /// Chat by id, if it exists.
    pub fn get_chat(&self, id: i64) -> Result<Option<Chat>> {
        fetch_chat(&*self.conn()?, id)
    }

    // ---- chat asks ----

    /// The chat a source ref like `chat:<message id>` points at (the id is a chat MESSAGE id).
    pub fn chat_id_for_source_ref(&self, source_ref: &str) -> Result<Option<i64>> {
        let Some(id) = source_ref.strip_prefix("chat:").and_then(|i| i.parse::<i64>().ok()) else { return Ok(None) };
        Ok(self.get_chat_message(id)?.map(|m| m.chat_id))
    }

    /// Stores a new open ask of the turn that answered `message_id`.
    pub fn create_chat_ask(&self, chat_id: i64, message_id: Option<i64>, text: &str, options: &[String]) -> Result<ChatAsk> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO chat_asks (chat_id, message_id, text, options) VALUES (?1, ?2, ?3, ?4)",
            params![chat_id, message_id, text, serde_json::to_string(options)?],
        )?;
        let id = conn.last_insert_rowid();
        fetch_asks(&conn, "a.id = ?1", params![id])?.pop().ok_or_else(|| anyhow!("ask vanished"))
    }

    pub fn get_chat_ask(&self, id: i64) -> Result<Option<ChatAsk>> {
        Ok(fetch_asks(&*self.conn()?, "a.id = ?1", params![id])?.pop())
    }

    /// Asks, oldest first, optionally only those with `status`.
    pub fn list_chat_asks(&self, status: Option<&str>) -> Result<Vec<ChatAsk>> {
        fetch_asks(&*self.conn()?, "(?1 IS NULL OR a.status = ?1)", params![status])
    }

    /// How many asks this turn (the owner message `message_id`) already made.
    pub fn count_asks_of_message(&self, message_id: i64) -> Result<i64> {
        let sql = "SELECT COUNT(*) FROM chat_asks WHERE message_id = ?1";
        Ok(self.conn()?.query_row(sql, [message_id], |r| r.get(0))?)
    }

    /// Records the answer to an open ask. None if it is missing or already answered.
    pub fn answer_chat_ask(&self, id: i64, answer: &str) -> Result<Option<ChatAsk>> {
        let conn = self.conn()?;
        let n = conn.execute("UPDATE chat_asks SET answer = ?1, status = 'answered' WHERE id = ?2 AND status = 'open'", params![answer, id])?;
        if n == 0 {
            return Ok(None);
        }
        Ok(fetch_asks(&conn, "a.id = ?1", params![id])?.pop())
    }

    /// The asks made in the same turn as ask `id` (just itself when it has no turn).
    pub fn asks_of_same_turn(&self, id: i64) -> Result<Vec<ChatAsk>> {
        let cond = "a.id = ?1 OR (a.message_id IS NOT NULL AND a.message_id = (SELECT message_id FROM chat_asks WHERE id = ?1))";
        fetch_asks(&*self.conn()?, cond, params![id])
    }

    /// The owner replied in the chat itself: its open asks count as answered there.
    pub fn close_chat_asks(&self, chat_id: i64) -> Result<usize> {
        let sql = "UPDATE chat_asks SET status = 'answered', answer = '(answered in chat)' WHERE chat_id = ?1 AND status = 'open'";
        Ok(self.conn()?.execute(sql, [chat_id])?)
    }

    /// Delete a chat and its messages, asks and sessions; events stay, detached from the chat.
    pub fn delete_chat(&self, id: i64) -> Result<bool> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute("UPDATE events SET chat_id = NULL WHERE chat_id = ?1", [id])?;
        tx.execute("DELETE FROM agent_sessions WHERE chat_id = ?1", [id])?;
        tx.execute("DELETE FROM chat_asks WHERE chat_id = ?1", [id])?;
        tx.execute("DELETE FROM chat_messages WHERE chat_id = ?1", [id])?;
        let deleted = tx.execute("DELETE FROM chats WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(deleted > 0)
    }

    /// All chats, most recently active first.
    pub fn list_chats(&self) -> Result<Vec<Chat>> {
        let conn = self.conn()?;
        let sql = format!("{CHAT_SELECT} ORDER BY c.updated_at DESC, c.id DESC");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_map([], chat_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Append a message to a chat, bump the chat, and title it from the first user message.
    pub fn add_chat_message(&self, chat_id: i64, role: ChatRole, content: &str) -> Result<ChatMessage> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO chat_messages (chat_id, role, content) VALUES (?1, ?2, ?3)",
            params![chat_id, role.as_str(), content],
        )?;
        let id = conn.last_insert_rowid();
        let sql = format!("UPDATE chats SET updated_at = {NOW} WHERE id = ?1");
        conn.execute(&sql, [chat_id])?;
        if role == ChatRole::User {
            conn.execute(
                "UPDATE chats SET title = ?1 WHERE id = ?2 AND title = ?3",
                params![chat_title(content), chat_id, DEFAULT_CHAT_TITLE],
            )?;
        }
        fetch_chat_message(&conn, id)?.ok_or_else(|| anyhow!("message vanished"))
    }

    /// One chat message by id.
    pub fn get_chat_message(&self, id: i64) -> Result<Option<ChatMessage>> {
        fetch_chat_message(&*self.conn()?, id)
    }

    /// The last `limit` messages of a chat, oldest first.
    pub fn list_chat(&self, chat_id: i64, limit: i64) -> Result<Vec<ChatMessage>> {
        let conn = self.conn()?;
        let sql = format!("SELECT {CHAT_COLS} FROM chat_messages WHERE chat_id = ?1 ORDER BY id DESC LIMIT ?2");
        let mut stmt = conn.prepare(&sql)?;
        let mut msgs = stmt.query_map(params![chat_id, limit], chat_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        msgs.reverse();
        Ok(msgs)
    }

    // ---- events ----

    /// Record an event and return its id.
    pub fn add_event(
        &self,
        task_id: Option<i64>,
        agent: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<i64> {
        self.insert_event(task_id, None, agent, kind, payload)
    }

    /// Record an event that belongs to a chat and return its id.
    pub fn add_event_in_chat(
        &self,
        chat_id: i64,
        agent: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<i64> {
        self.insert_event(None, Some(chat_id), agent, kind, payload)
    }

    fn insert_event(
        &self,
        task_id: Option<i64>,
        chat_id: Option<i64>,
        agent: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<i64> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO events (task_id, chat_id, agent, kind, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, chat_id, agent, kind, payload.to_string()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// The last `limit` events (optionally for one task), oldest first.
    pub fn list_events(&self, task_id: Option<i64>, limit: i64) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT {EVENT_COLS} FROM ( \
               SELECT * FROM events WHERE (?1 IS NULL OR task_id = ?1) ORDER BY id DESC LIMIT ?2 \
             ) ORDER BY id"
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_map(params![task_id, limit], event_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Ids of stuck tasks that were stopped (a `stopped` event exists), not stuck on their own.
    pub fn stopped_task_ids(&self) -> Result<Vec<i64>> {
        let conn = self.conn()?;
        let sql = "SELECT t.id FROM tasks t WHERE t.status = 'stuck' \
                   AND EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND e.kind = 'stopped') ORDER BY t.id";
        let mut stmt = conn.prepare(sql)?;
        Ok(stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    /// Every event whose kind is in `kinds`, oldest first.
    pub fn events_of_kinds(&self, kinds: &[&str]) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let marks = vec!["?"; kinds.len()].join(",");
        let sql = format!("SELECT {EVENT_COLS} FROM events WHERE kind IN ({marks}) ORDER BY id");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_map(rusqlite::params_from_iter(kinds.iter()), event_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Events with id > `after_id`, oldest first, at most `limit`.
    pub fn list_events_after(&self, task_id: Option<i64>, after_id: i64, limit: i64) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT {EVENT_COLS} FROM events \
             WHERE id > ?1 AND (?2 IS NULL OR task_id = ?2) ORDER BY id LIMIT ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_map(params![after_id, task_id, limit], event_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    // ---- thread reads (read-only, scoped to a set of task ids) ----

    /// Events of the given tasks and kinds with id > `after_id`, oldest first, at most `limit`.
    pub fn events_for_tasks(&self, ids: &[i64], kinds: &[&str], after_id: i64, limit: i64) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let marks = vec!["?"; kinds.len()].join(",");
        let sql = format!(
            "SELECT {EVENT_COLS} FROM events WHERE task_id IN ({}) AND kind IN ({marks}) AND id > {after_id} \
             ORDER BY id LIMIT {limit}",
            id_list(ids)
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_map(rusqlite::params_from_iter(kinds.iter()), event_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// The events with these ids, oldest first.
    pub fn events_by_ids(&self, ids: &[i64]) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let sql = format!("SELECT {EVENT_COLS} FROM events WHERE id IN ({}) ORDER BY id", id_list(ids));
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_map([], event_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Epoch seconds of the newest activity per task: its newest event (heartbeat
    /// bookkeeping like `stalled` and `heartbeat` does not count), or its own update.
    pub fn last_activity(&self, ids: &[i64]) -> Result<std::collections::HashMap<i64, i64>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT t.id, CAST(strftime('%s', MAX(t.updated_at, \
               COALESCE((SELECT MAX(created_at) FROM events WHERE task_id = t.id \
               AND kind NOT IN ('stalled', 'heartbeat')), t.updated_at))) AS INTEGER) \
             FROM tasks t WHERE t.id IN ({})",
            id_list(ids)
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().filter_map(|(id, t)| Some((id, t?))).collect())
    }

    /// Sum of `total_cost_usd` over the `result` events of each task.
    pub fn cost_by_task(&self, ids: &[i64]) -> Result<std::collections::HashMap<i64, f64>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT task_id, SUM(COALESCE(json_extract(payload, '$.total_cost_usd'), 0)) FROM events \
             WHERE kind = 'result' AND task_id IN ({}) GROUP BY task_id",
            id_list(ids)
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every `result` event since `since` (an ISO time) as (task_id, created_at, usd, tokens, seat_id).
    /// Tokens add up input, output and both cache counts from the result's `usage`.
    pub fn result_costs_since(&self, since: &str) -> Result<Vec<ResultCost>> {
        let conn = self.conn()?;
        let tok = |k: &str| format!("COALESCE(json_extract(payload, '$.usage.{k}'), 0)");
        let sql = format!(
            "SELECT task_id, created_at, COALESCE(json_extract(payload, '$.total_cost_usd'), 0), {} + {} + {} + {}, \
             json_extract(payload, '$.seat_id') FROM events WHERE kind = 'result' AND created_at >= ?1 ORDER BY id",
            tok("input_tokens"), tok("output_tokens"), tok("cache_read_input_tokens"), tok("cache_creation_input_tokens")
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![since], |r| {
            Ok(ResultCost { task_id: r.get(0)?, created_at: r.get(1)?, usd: r.get(2)?, tokens: r.get(3)?, seat_id: r.get(4)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Cost of one seat's turns since `since` (an ISO time), and when the oldest of them ran.
    pub fn seat_usage_since(&self, seat_id: &str, since: &str) -> Result<(f64, Option<String>)> {
        let conn = self.conn()?;
        let sql = "SELECT COALESCE(SUM(COALESCE(json_extract(payload, '$.total_cost_usd'), 0)), 0), MIN(created_at) \
                   FROM events WHERE kind = 'result' AND json_extract(payload, '$.seat_id') = ?1 AND created_at >= ?2";
        Ok(conn.query_row(sql, params![seat_id, since], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    /// Highest event id among these tasks, or 0.
    pub fn max_event_id(&self, ids: &[i64]) -> Result<i64> {
        let conn = self.conn()?;
        let sql = format!("SELECT COALESCE(MAX(id), 0) FROM events WHERE task_id IN ({})", id_list(ids));
        Ok(conn.query_row(&sql, [], |r| r.get(0))?)
    }

    /// The newest `story` line of these tasks, else the newest `log_decision` line.
    pub fn latest_story_line(&self, ids: &[i64]) -> Result<Option<String>> {
        let conn = self.conn()?;
        let list = id_list(ids);
        let story = format!("SELECT json_extract(payload, '$.line') FROM events WHERE task_id IN ({list}) AND kind = 'story' ORDER BY id DESC LIMIT 1");
        let logged = format!(
            "SELECT json_extract(payload, '$.args.line') FROM events WHERE task_id IN ({list}) AND kind = 'tool_call' \
             AND json_extract(payload, '$.tool') = 'log_decision' ORDER BY id DESC LIMIT 1"
        );
        for sql in [story, logged] {
            let line: Option<Option<String>> = conn.query_row(&sql, [], |r| r.get(0)).optional()?;
            if let Some(l) = line.flatten() {
                return Ok(Some(l));
            }
        }
        Ok(None)
    }

    /// How often each skill was installed in the last `days` days, from `skill_used` events.
    pub fn skill_use_counts(&self, days: i64) -> Result<std::collections::HashMap<String, i64>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT json_extract(payload, '$.skill'), COUNT(*) FROM events \
             WHERE kind = 'skill_used' AND created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1) GROUP BY 1",
        )?;
        let rows = stmt.query_map(params![format!("-{days} days")], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)))?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().filter_map(|(k, n)| Some((k?, n))).collect())
    }

    /// Owner instructions (id, text) not yet delivered, oldest first.
    pub fn pending_instructions(&self, task_id: i64) -> Result<Vec<(i64, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, json_extract(payload, '$.text') FROM events \
             WHERE task_id = ?1 AND kind = 'instruction' AND id > ( \
               SELECT COALESCE(MAX(json_extract(payload, '$.upto')), 0) FROM events \
               WHERE task_id = ?1 AND kind = 'instruction_delivered') \
             ORDER BY id",
        )?;
        let rows = stmt.query_map(params![task_id], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?)))?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().map(|(id, t)| (id, t.unwrap_or_default())).collect())
    }

    /// The newest "init" event of each task, newest task id last.
    pub fn latest_init_per_task(&self) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT {EVENT_COLS} FROM events \
             WHERE id IN (SELECT MAX(id) FROM events WHERE kind = 'init' AND task_id IS NOT NULL GROUP BY task_id) \
             ORDER BY task_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_map([], event_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// The model string of the newest "init" event that has one.
    pub fn latest_model(&self) -> Result<Option<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT json_extract(payload, '$.model') AS m FROM events \
             WHERE kind = 'init' AND typeof(json_extract(payload, '$.model')) = 'text' \
             ORDER BY id DESC LIMIT 1",
        )?;
        Ok(stmt.query_row([], |r| r.get(0)).ok())
    }

    // ---- sessions ----

    /// Saved agent session id for a role and optional task.
    pub fn get_session(&self, role: Role, task_id: Option<i64>) -> Result<Option<String>> {
        let sql = "SELECT session_id FROM agent_sessions WHERE role = ?1 AND task_id IS ?2";
        Ok(self.conn()?.query_row(sql, params![role.as_str(), task_id], |r| r.get(0)).optional()?)
    }

    /// Upsert the session id (delete then insert, since NULL task ids are distinct in UNIQUE).
    pub fn set_session(&self, role: Role, task_id: Option<i64>, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM agent_sessions WHERE role = ?1 AND task_id IS ?2",
            params![role.as_str(), task_id],
        )?;
        conn.execute(
            "INSERT INTO agent_sessions (role, task_id, session_id) VALUES (?1, ?2, ?3)",
            params![role.as_str(), task_id, session_id],
        )?;
        Ok(())
    }

    /// Saved Rui session id for one chat.
    pub fn get_rui_session(&self, chat_id: i64) -> Result<Option<String>> {
        let sql = "SELECT session_id FROM agent_sessions WHERE role = 'rui' AND chat_id = ?1";
        Ok(self.conn()?.query_row(sql, [chat_id], |r| r.get(0)).optional()?)
    }

    /// Upsert the Rui session id for one chat.
    pub fn set_rui_session(&self, chat_id: i64, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM agent_sessions WHERE role = 'rui' AND chat_id = ?1", [chat_id])?;
        conn.execute(
            "INSERT INTO agent_sessions (role, chat_id, session_id) VALUES ('rui', ?1, ?2)",
            params![chat_id, session_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_core::SCHEMA_VERSION;

    fn new_task(client_id: i64, title: &str) -> NewTask {
        NewTask {
            client_id,
            parent_id: None,
            title: title.into(),
            body: String::new(),
            status: TaskStatus::New,
            done_command: None,
            max_loops: 5,
            role: Role::Lead,
            source_kind: SourceKind::User,
            source_ref: None,
            agent: None,
            pod: vec![],
        }
    }

    fn db_with_client() -> (Db, Client) {
        let db = Db::open_test();
        let c = db.create_client("acme", "/repo", Some("cargo test"), &["ls".into()]).unwrap();
        (db, c)
    }

    #[test]
    fn delete_client_cascade_removes_only_that_client() {
        let (db, a) = db_with_client();
        let b = db.create_client("other", "/repo2", None, &[]).unwrap();
        let ta = db.create_task(&new_task(a.id, "a1")).unwrap();
        let sub = db.create_task(&NewTask { parent_id: Some(ta.id), ..new_task(a.id, "a2") }).unwrap();
        let tb = db.create_task(&new_task(b.id, "b1")).unwrap();
        db.create_question(ta.id, Role::Lead, "why?").unwrap();
        db.create_question(tb.id, Role::Lead, "keep?").unwrap();
        db.add_event(Some(sub.id), "lead", "note", &serde_json::json!({})).unwrap();
        db.add_event(Some(tb.id), "lead", "note", &serde_json::json!({})).unwrap();
        db.set_session(Role::Lead, Some(ta.id), "s1").unwrap();
        assert!(db.delete_client_cascade(a.id).unwrap());
        assert!(db.get_client(a.id).unwrap().is_none());
        let ids: Vec<i64> = db.list_tasks().unwrap().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![tb.id]);
        assert_eq!(db.list_questions(None).unwrap().len(), 1);
        assert_eq!(db.list_events(None, 100).unwrap().len(), 1);
        assert!(db.get_client(b.id).unwrap().is_some());
        assert!(!db.delete_client_cascade(a.id).unwrap());
    }

    #[test]
    fn delete_chat_keeps_events_detached() {
        let db = Db::open_test();
        let chat = db.create_chat().unwrap();
        db.add_chat_message(chat.id, ChatRole::User, "hi").unwrap();
        let eid = db.add_event_in_chat(chat.id, "rui", "text", &serde_json::json!({})).unwrap();
        assert!(db.delete_chat(chat.id).unwrap());
        assert!(db.get_chat(chat.id).unwrap().is_none());
        assert!(!db.delete_chat(chat.id).unwrap());
        let ev = db.list_events(None, 10).unwrap().into_iter().find(|e| e.id == eid).unwrap();
        assert_eq!(ev.chat_id, None);
    }

    #[test]
    fn client_create_list_get() {
        let (db, c) = db_with_client();
        assert_eq!(c.allowed_commands, vec!["ls".to_string()]);
        assert_eq!(c.test_command.as_deref(), Some("cargo test"));
        assert_eq!(db.list_clients().unwrap(), vec![c.clone()]);
        assert_eq!(db.get_client(c.id).unwrap(), Some(c.clone()));
        assert_eq!(db.get_client_by_name("acme").unwrap(), Some(c));
        assert_eq!(db.get_client(999).unwrap(), None);
        assert_eq!(db.get_client_by_name("nope").unwrap(), None);
    }

    #[test]
    fn task_deps_round_trip() {
        let (db, c) = db_with_client();
        let a = db.create_task(&new_task(c.id, "a")).unwrap();
        let b = db.create_task(&new_task(c.id, "b")).unwrap();
        db.add_task_dep(b.id, a.id).unwrap();
        assert_eq!(db.get_task(b.id).unwrap().unwrap().depends_on, vec![a.id]);
        assert!(db.get_task(a.id).unwrap().unwrap().depends_on.is_empty());
        let mut child = new_task(c.id, "child");
        child.parent_id = Some(a.id);
        let child = db.create_task(&child).unwrap();
        assert_eq!(db.subtasks(a.id).unwrap()[0].id, child.id);
        assert_eq!(db.list_tasks().unwrap().len(), 3);
    }

    #[test]
    fn status_loops_and_branch() {
        let (db, c) = db_with_client();
        let t = db.create_task(&new_task(c.id, "t")).unwrap();
        db.set_task_status(t.id, TaskStatus::Open).unwrap();
        assert_eq!(db.tasks_with_status(TaskStatus::Open).unwrap().len(), 1);
        assert!(db.tasks_with_status(TaskStatus::New).unwrap().is_empty());
        assert_eq!(db.increment_loops(t.id).unwrap(), 1);
        assert_eq!(db.increment_loops(t.id).unwrap(), 2);
        db.set_task_branch(t.id, "rui/t").unwrap();
        let t = db.get_task(t.id).unwrap().unwrap();
        assert_eq!((t.status, t.loops_used, t.branch.as_deref()), (TaskStatus::Open, 2, Some("rui/t")));
    }

    #[test]
    fn questions_create_answer_filter() {
        let (db, c) = db_with_client();
        let t = db.create_task(&new_task(c.id, "t")).unwrap();
        let q1 = db.create_question(t.id, Role::Lead, "why?").unwrap();
        let q2 = db.create_question(t.id, Role::Builder, "how?").unwrap();
        let ans = db.answer_question(q1.id, "because", Some("r")).unwrap().unwrap();
        assert_eq!(ans.status, QuestionStatus::Answered);
        assert_eq!(ans.answer.as_deref(), Some("because"));
        assert_eq!((ans.level.as_str(), ans.why.as_deref()), ("owner", Some("r")));
        assert_eq!(db.answer_question(999, "x", None).unwrap(), None);
        let open = db.list_questions(Some(QuestionStatus::Open)).unwrap();
        assert_eq!(open.iter().map(|q| q.id).collect::<Vec<_>>(), vec![q2.id]);
        assert_eq!(db.list_questions(None).unwrap().len(), 2);
        assert_eq!(db.questions_for_task(t.id).unwrap().len(), 2);
    }

    #[test]
    fn chat_order_and_limit() {
        let db = Db::open_test();
        let chat = db.create_chat().unwrap();
        for i in 0..5 {
            db.add_chat_message(chat.id, ChatRole::User, &format!("m{i}")).unwrap();
        }
        let last = db.list_chat(chat.id, 3).unwrap();
        let texts: Vec<_> = last.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(texts, vec!["m2", "m3", "m4"]);
    }

    #[test]
    fn chat_title_rule() {
        assert_eq!(chat_title("  hello  "), "hello");
        let long = "é".repeat(50);
        assert_eq!(chat_title(&long), format!("{}…", "é".repeat(44)));
        let exact = "a".repeat(44);
        assert_eq!(chat_title(&exact), exact);
        let spaced = format!("{} tail words", "a".repeat(43));
        assert_eq!(chat_title(&spaced), format!("{}…", "a".repeat(43)));
    }

    #[test]
    fn chats_list_title_last_and_isolation() {
        let db = Db::open_test();
        let a = db.create_chat().unwrap();
        let b = db.create_chat().unwrap();
        assert_eq!((a.title.as_str(), a.last.clone()), ("New chat", None));
        db.add_chat_message(a.id, ChatRole::Rui, "welcome").unwrap();
        assert_eq!(db.get_chat(a.id).unwrap().unwrap().title, "New chat");
        db.add_chat_message(a.id, ChatRole::User, "set up acme").unwrap();
        db.add_chat_message(a.id, ChatRole::User, "second").unwrap();
        let m = db.add_chat_message(b.id, ChatRole::User, "other").unwrap();
        assert_eq!(db.get_chat_message(m.id).unwrap().unwrap().chat_id, b.id);
        assert_eq!(db.get_chat_message(999).unwrap(), None);
        let chats = db.list_chats().unwrap();
        assert_eq!(chats.len(), 2);
        let a = chats.iter().find(|c| c.id == a.id).unwrap();
        assert_eq!((a.title.as_str(), a.last.as_deref()), ("set up acme", Some("second")));
        assert_eq!(db.list_chat(a.id, 10).unwrap().len(), 3);
        assert_eq!(db.list_chat(b.id, 10).unwrap().len(), 1);
    }

    #[test]
    fn rui_sessions_are_per_chat_and_events_carry_chat() {
        let db = Db::open_test();
        let (a, b) = (db.create_chat().unwrap(), db.create_chat().unwrap());
        db.set_rui_session(a.id, "s1").unwrap();
        db.set_rui_session(a.id, "s2").unwrap();
        assert_eq!(db.get_rui_session(a.id).unwrap().as_deref(), Some("s2"));
        assert_eq!(db.get_rui_session(b.id).unwrap(), None);
        db.add_event_in_chat(a.id, "rui", "k", &serde_json::json!({})).unwrap();
        assert_eq!(db.list_events(None, 10).unwrap()[0].chat_id, Some(a.id));
    }

    const V1_SCHEMA: &str = "\
        CREATE TABLE chat_messages (id INTEGER PRIMARY KEY, role TEXT NOT NULL, content TEXT NOT NULL, \
          created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))); \
        CREATE TABLE events (id INTEGER PRIMARY KEY, task_id INTEGER, agent TEXT NOT NULL, kind TEXT NOT NULL, \
          payload TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))); \
        CREATE TABLE agent_sessions (role TEXT NOT NULL, task_id INTEGER, session_id TEXT NOT NULL, \
          updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), UNIQUE (role, task_id)); \
        INSERT INTO chat_messages (role, content) VALUES ('user', 'hi'); \
        INSERT INTO events (agent, kind) VALUES ('rui', 'x'), ('lead', 'y'); \
        INSERT INTO agent_sessions (role, session_id) VALUES ('rui', 's'); \
        PRAGMA user_version = 1;";

    #[test]
    fn chat_asks_status_is_checked() {
        let db = Db::open_test();
        let conn = db.conn().unwrap();
        conn.execute("INSERT INTO chats (id, title) VALUES (2, 't')", []).unwrap();
        conn.execute("INSERT INTO chat_asks (chat_id, text) VALUES (2, 'q')", []).unwrap();
        assert!(conn.execute("INSERT INTO chat_asks (chat_id, text, status) VALUES (2, 'q', 'bogus')", []).is_err());
    }

    #[test]
    fn migration_002_upgrades_v1_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        conn.execute_batch(MIGRATION_SQL[1]).unwrap();
        let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(one("SELECT chat_id FROM chat_messages"), 1);
        assert_eq!(one("SELECT chat_id FROM agent_sessions WHERE role = 'rui'"), 1);
        assert_eq!(one("SELECT chat_id FROM events WHERE agent = 'rui'"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM events WHERE chat_id IS NULL"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM chats WHERE id = 1 AND title = 'Rui'"), 1);
    }

    #[test]
    fn migrations_are_valid_and_match_schema_version() {
        migrations().validate().unwrap();
        assert_eq!(MIGRATION_SQL.len() as i64, SCHEMA_VERSION);
    }

    /// Table and index definitions with comments dropped and words sorted, so column
    /// order (ALTER TABLE appends columns) and formatting do not count as drift.
    fn schema_of(conn: &Connection) -> Vec<(String, String, Vec<String>)> {
        let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_master ORDER BY name").unwrap();
        let rows = stmt.query_map([], |r| {
            let sql: Option<String> = r.get(2)?;
            let code: Vec<&str> = sql.as_deref().unwrap_or("").lines().map(|l| l.split("--").next().unwrap()).collect();
            let flat = code.join(" ").replace(['(', ')', ','], " ");
            let mut words: Vec<String> = flat.split_whitespace().map(String::from).collect();
            words.sort();
            Ok((r.get(0)?, r.get(1)?, words))
        });
        rows.unwrap().map(|r| r.unwrap()).collect()
    }

    #[test]
    fn migrations_match_schema_sql() {
        let from_sql = Connection::open_in_memory().unwrap();
        from_sql.execute_batch(include_str!("../../../db/schema.sql")).unwrap();
        let mut from_migrations = Connection::open_in_memory().unwrap();
        migrations().to_latest(&mut from_migrations).unwrap();
        assert_eq!(schema_of(&from_sql), schema_of(&from_migrations));
    }

    fn version_of(path: &Path) -> i64 {
        user_version(&Connection::open(path).unwrap()).unwrap()
    }

    fn backups_in(dir: &Path, prefix: &str) -> usize {
        let names = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned());
        names.filter(|n| n.contains(prefix)).count()
    }

    #[test]
    fn open_creates_a_fresh_file_at_latest_without_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rui.db");
        Db::open(&path).unwrap();
        assert_eq!(version_of(&path), SCHEMA_VERSION);
        assert_eq!(backups_in(dir.path(), ".bak-"), 0);
    }

    #[test]
    fn open_migrates_an_old_file_and_backs_it_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rui.db");
        let mut conn = Connection::open(&path).unwrap();
        Migrations::new(MIGRATION_SQL[..3].iter().map(|sql| M::up(sql)).collect()).to_latest(&mut conn).unwrap();
        drop(conn);
        Db::open(&path).unwrap();
        assert_eq!(version_of(&path), SCHEMA_VERSION);
        assert_eq!(backups_in(dir.path(), "rui.db.bak-v3-"), 1);
    }

    #[test]
    fn open_current_file_makes_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rui.db");
        Db::open(&path).unwrap();
        Db::open(&path).unwrap();
        assert_eq!(backups_in(dir.path(), ".bak-"), 0);
    }

    #[test]
    fn open_rejects_a_newer_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rui.db");
        Connection::open(&path).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
        let err = Db::open(&path).err().unwrap().to_string();
        assert!(err.contains("newer than this daemon"));
        assert_eq!(backups_in(dir.path(), ".bak-"), 0);
    }

    #[test]
    fn events_filter_by_task() {
        let (db, c) = db_with_client();
        let t = db.create_task(&new_task(c.id, "t")).unwrap();
        let p = serde_json::json!({"k": 1});
        db.add_event(Some(t.id), "lead", "start", &p).unwrap();
        db.add_event(None, "rui", "boot", &p).unwrap();
        assert_eq!(db.list_events(None, 10).unwrap().len(), 2);
        let only = db.list_events(Some(t.id), 10).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].payload, p);
        assert_eq!(db.list_events(None, 1).unwrap()[0].kind, "boot");
    }

    #[test]
    fn pending_instructions_respect_delivered_marker() {
        let (db, c) = db_with_client();
        let t = db.create_task(&new_task(c.id, "t")).unwrap();
        assert!(db.pending_instructions(t.id).unwrap().is_empty());
        let a = db.add_event(Some(t.id), "owner", "instruction", &serde_json::json!({"text": "a"})).unwrap();
        let b = db.add_event(Some(t.id), "owner", "instruction", &serde_json::json!({"text": "b"})).unwrap();
        let both = vec![(a, "a".to_string()), (b, "b".to_string())];
        assert_eq!(db.pending_instructions(t.id).unwrap(), both);
        db.add_event(Some(t.id), "owner", "instruction_delivered", &serde_json::json!({"upto": a})).unwrap();
        assert_eq!(db.pending_instructions(t.id).unwrap(), vec![(b, "b".to_string())]);
    }

    #[test]
    fn session_upsert_keeps_one_row() {
        let (db, c) = db_with_client();
        let t = db.create_task(&new_task(c.id, "t")).unwrap();
        for tid in [Some(t.id), None] {
            db.set_session(Role::Lead, tid, "s1").unwrap();
            db.set_session(Role::Lead, tid, "s2").unwrap();
            assert_eq!(db.get_session(Role::Lead, tid).unwrap().as_deref(), Some("s2"));
        }
        let n: i64 = db
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM agent_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(db.get_session(Role::Rui, None).unwrap(), None);
    }
}
