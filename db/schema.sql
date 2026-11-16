-- Apply with:
--   sqlite3 "$HOME/Library/Application Support/RUI/rui.db" < db/schema.sql
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS clients (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  repo_path TEXT NOT NULL,
  test_command TEXT,
  allowed_commands TEXT NOT NULL DEFAULT '[]', -- JSON array
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS tasks (
  id INTEGER PRIMARY KEY,
  client_id INTEGER NOT NULL REFERENCES clients(id),
  parent_id INTEGER REFERENCES tasks(id),
  title TEXT NOT NULL,
  body TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'new'
    CHECK (status IN ('new','clarifying','waiting_answer','split','open','in_progress','done','stuck')),
  done_command TEXT,
  max_loops INTEGER NOT NULL DEFAULT 5,
  loops_used INTEGER NOT NULL DEFAULT 0,
  role TEXT NOT NULL DEFAULT 'lead' CHECK (role IN ('rui','lead','builder')),
  branch TEXT,
  source_kind TEXT NOT NULL CHECK (source_kind IN ('chat','user')),
  source_ref TEXT,
  skill TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS task_deps (
  task_id INTEGER NOT NULL REFERENCES tasks(id),
  depends_on INTEGER NOT NULL REFERENCES tasks(id),
  PRIMARY KEY (task_id, depends_on)
);

CREATE TABLE IF NOT EXISTS questions (
  id INTEGER PRIMARY KEY,
  task_id INTEGER NOT NULL REFERENCES tasks(id),
  asked_by TEXT NOT NULL CHECK (asked_by IN ('rui','lead','builder')),
  text TEXT NOT NULL,
  answer TEXT,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','answered')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS chats (
  id INTEGER PRIMARY KEY,
  title TEXT NOT NULL DEFAULT 'New chat',
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS chat_messages (
  id INTEGER PRIMARY KEY,
  chat_id INTEGER NOT NULL DEFAULT 1 REFERENCES chats(id),
  role TEXT NOT NULL CHECK (role IN ('user','rui')),
  content TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY,
  task_id INTEGER REFERENCES tasks(id),
  chat_id INTEGER REFERENCES chats(id),
  agent TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS agent_sessions (
  role TEXT NOT NULL CHECK (role IN ('rui','lead','builder')),
  task_id INTEGER REFERENCES tasks(id),
  chat_id INTEGER REFERENCES chats(id),
  session_id TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (role, task_id)
);

CREATE INDEX IF NOT EXISTS idx_tasks_client_id ON tasks(client_id);
CREATE INDEX IF NOT EXISTS idx_tasks_parent_id ON tasks(parent_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_questions_task_id ON questions(task_id);
CREATE INDEX IF NOT EXISTS idx_events_task_id ON events(task_id);
CREATE INDEX IF NOT EXISTS idx_chat_messages_chat_id ON chat_messages(chat_id);
CREATE INDEX IF NOT EXISTS idx_events_chat_id ON events(chat_id);

PRAGMA user_version = 3;
