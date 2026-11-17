-- Migrates an existing v1 database to v2 (multiple chats).

CREATE TABLE chats (
  id INTEGER PRIMARY KEY,
  title TEXT NOT NULL DEFAULT 'New chat',
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
INSERT INTO chats(id, title) VALUES (1, 'Rui');

ALTER TABLE chat_messages ADD COLUMN chat_id INTEGER NOT NULL DEFAULT 1 REFERENCES chats(id);
CREATE INDEX idx_chat_messages_chat_id ON chat_messages(chat_id);

ALTER TABLE agent_sessions ADD COLUMN chat_id INTEGER REFERENCES chats(id);
UPDATE agent_sessions SET chat_id = 1 WHERE role = 'rui';

ALTER TABLE events ADD COLUMN chat_id INTEGER REFERENCES chats(id);
CREATE INDEX idx_events_chat_id ON events(chat_id);
UPDATE events SET chat_id = 1 WHERE agent = 'rui';

