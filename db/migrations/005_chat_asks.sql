-- Adds chat_asks: questions Rui asks the owner from chat (shown in the Inbox).
-- Apply with:
--   sqlite3 "$HOME/Library/Application Support/RUI/rui.db" < db/migrations/005_chat_asks.sql
BEGIN;
CREATE TABLE chat_asks(
  id INTEGER PRIMARY KEY,
  chat_id INTEGER NOT NULL REFERENCES chats(id),
  message_id INTEGER, -- the owner message of the turn that asked; groups one turn's asks
  text TEXT NOT NULL,
  options TEXT NOT NULL DEFAULT '[]',
  answer TEXT,
  status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open','answered')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX idx_chat_asks_chat_id ON chat_asks(chat_id);
PRAGMA user_version = 5;
COMMIT;
