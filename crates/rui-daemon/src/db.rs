//! SQLite access layer. Plain SQL, one query per function. Never creates tables.

use anyhow::{Result, anyhow, bail};
use rui_core::{
    ChatMessage, ChatRole, Client, Event, Question, QuestionStatus, Role, SCHEMA_VERSION,
    SourceKind, Task, TaskStatus,
};
use rusqlite::{Connection, OptionalExtension, Row, params, types::Type};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";
const TASK_COLS: &str = "id, client_id, parent_id, title, body, status, done_command, max_loops, \
     loops_used, role, branch, source_kind, source_ref, created_at, updated_at";
const CLIENT_COLS: &str = "id, name, repo_path, test_command, allowed_commands, created_at";
const QUESTION_COLS: &str = "id, task_id, asked_by, text, answer, status, created_at";

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
    })
}

fn chat_from_row(r: &Row) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: r.get(0)?,
        role: parse_enum(1, r.get(1)?, ChatRole::parse)?,
        content: r.get(2)?,
        created_at: r.get(3)?,
    })
}

fn event_from_row(r: &Row) -> rusqlite::Result<Event> {
    let payload: String = r.get(4)?;
    Ok(Event {
        id: r.get(0)?,
        task_id: r.get(1)?,
        agent: r.get(2)?,
        kind: r.get(3)?,
        payload: serde_json::from_str(&payload).map_err(|e| bad(4, e.to_string()))?,
        created_at: r.get(5)?,
    })
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

fn fetch_client(conn: &Connection, id: i64) -> Result<Option<Client>> {
    let sql = format!("SELECT {CLIENT_COLS} FROM clients WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], client_from_row).optional()?)
}

impl Db {
    /// Open an existing, correctly versioned database. Never creates tables.
    pub fn open(path: &Path) -> Result<Db> {
        if !path.exists() {
            bail!("database not found at {}; apply db/schema.sql with sqlite3", path.display());
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            bail!("schema version {version}, expected {SCHEMA_VERSION}; apply db/schema.sql");
        }
        Ok(Db { conn: Mutex::new(conn) })
    }

    /// In-memory database with the schema applied. Tests only.
    #[cfg(test)]
    pub fn open_test() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../../db/schema.sql")).unwrap();
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

    // ---- tasks ----

    /// Insert a task and return it.
    pub fn create_task(&self, t: &NewTask) -> Result<Task> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO tasks (client_id, parent_id, title, body, status, done_command, max_loops, role, source_kind, source_ref) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                t.source_ref
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
    pub fn answer_question(&self, id: i64, answer: &str) -> Result<Option<Question>> {
        let conn = self.conn()?;
        let n = conn.execute(
            "UPDATE questions SET answer = ?1, status = 'answered' WHERE id = ?2",
            params![answer, id],
        )?;
        if n == 0 {
            return Ok(None);
        }
        fetch_question(&conn, id)
    }

    // ---- chat ----

    /// Append a chat message and return it.
    pub fn add_chat_message(&self, role: ChatRole, content: &str) -> Result<ChatMessage> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO chat_messages (role, content) VALUES (?1, ?2)",
            params![role.as_str(), content],
        )?;
        let id = conn.last_insert_rowid();
        Ok(conn.query_row(
            "SELECT id, role, content, created_at FROM chat_messages WHERE id = ?1",
            [id],
            chat_from_row,
        )?)
    }

    /// The last `limit` chat messages, oldest first.
    pub fn list_chat(&self, limit: i64) -> Result<Vec<ChatMessage>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, role, content, created_at FROM chat_messages ORDER BY id DESC LIMIT ?1",
        )?;
        let mut msgs = stmt.query_map([limit], chat_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
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
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO events (task_id, agent, kind, payload) VALUES (?1, ?2, ?3, ?4)",
            params![task_id, agent, kind, payload.to_string()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// The last `limit` events (optionally for one task), oldest first.
    pub fn list_events(&self, task_id: Option<i64>, limit: i64) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, agent, kind, payload, created_at FROM ( \
               SELECT * FROM events WHERE (?1 IS NULL OR task_id = ?1) ORDER BY id DESC LIMIT ?2 \
             ) ORDER BY id",
        )?;
        Ok(stmt.query_map(params![task_id, limit], event_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Events with id > `after_id`, oldest first, at most `limit`.
    pub fn list_events_after(&self, task_id: Option<i64>, after_id: i64, limit: i64) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, agent, kind, payload, created_at FROM events \
             WHERE id > ?1 AND (?2 IS NULL OR task_id = ?2) ORDER BY id LIMIT ?3",
        )?;
        Ok(stmt.query_map(params![after_id, task_id, limit], event_from_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// The newest "init" event of each task, newest task id last.
    pub fn latest_init_per_task(&self) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, agent, kind, payload, created_at FROM events \
             WHERE id IN (SELECT MAX(id) FROM events WHERE kind = 'init' AND task_id IS NOT NULL GROUP BY task_id) \
             ORDER BY task_id",
        )?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    fn db_with_client() -> (Db, Client) {
        let db = Db::open_test();
        let c = db.create_client("acme", "/repo", Some("cargo test"), &["ls".into()]).unwrap();
        (db, c)
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
        let ans = db.answer_question(q1.id, "because").unwrap().unwrap();
        assert_eq!(ans.status, QuestionStatus::Answered);
        assert_eq!(ans.answer.as_deref(), Some("because"));
        assert_eq!(db.answer_question(999, "x").unwrap(), None);
        let open = db.list_questions(Some(QuestionStatus::Open)).unwrap();
        assert_eq!(open.iter().map(|q| q.id).collect::<Vec<_>>(), vec![q2.id]);
        assert_eq!(db.list_questions(None).unwrap().len(), 2);
        assert_eq!(db.questions_for_task(t.id).unwrap().len(), 2);
    }

    #[test]
    fn chat_order_and_limit() {
        let db = Db::open_test();
        for i in 0..5 {
            db.add_chat_message(ChatRole::User, &format!("m{i}")).unwrap();
        }
        let last = db.list_chat(3).unwrap();
        let texts: Vec<_> = last.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(texts, vec!["m2", "m3", "m4"]);
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

    #[test]
    fn open_missing_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = Db::open(&dir.path().join("nope.db")).err().unwrap();
        assert!(err.to_string().contains("database not found"));
    }

    #[test]
    fn open_wrong_version_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        Connection::open(&path).unwrap();
        let err = Db::open(&path).err().unwrap();
        assert!(err.to_string().contains("schema version 0"));
    }
}
