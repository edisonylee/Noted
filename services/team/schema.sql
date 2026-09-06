PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS organizations (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS members (org_id TEXT NOT NULL REFERENCES organizations(id), user_id TEXT NOT NULL REFERENCES users(id), role TEXT NOT NULL CHECK(role IN ('owner','admin','member')), PRIMARY KEY(org_id,user_id));
CREATE TABLE IF NOT EXISTS invites (id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id), hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL, role TEXT NOT NULL CHECK(role IN ('admin','member')), expires_at INTEGER NOT NULL, created_by TEXT NOT NULL REFERENCES users(id));
CREATE TABLE IF NOT EXISTS spaces (id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id), name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', visibility TEXT NOT NULL CHECK(visibility IN ('team','restricted')));
CREATE TABLE IF NOT EXISTS space_members (space_id TEXT NOT NULL REFERENCES spaces(id), user_id TEXT NOT NULL REFERENCES users(id), role TEXT NOT NULL CHECK(role IN ('viewer','editor')), PRIMARY KEY(space_id,user_id));
CREATE TABLE IF NOT EXISTS groups (id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id), name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS group_members (group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE, user_id TEXT NOT NULL REFERENCES users(id), PRIMARY KEY(group_id,user_id));
CREATE TABLE IF NOT EXISTS space_groups (space_id TEXT NOT NULL REFERENCES spaces(id), group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE, role TEXT NOT NULL CHECK(role IN ('viewer','editor')), PRIMARY KEY(space_id,group_id));
CREATE TABLE IF NOT EXISTS folders (id TEXT PRIMARY KEY, space_id TEXT NOT NULL REFERENCES spaces(id), parent_id TEXT REFERENCES folders(id), name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS notes (
 id TEXT PRIMARY KEY, space_id TEXT NOT NULL REFERENCES spaces(id), owner_id TEXT NOT NULL REFERENCES users(id),
 source_key TEXT NOT NULL, title TEXT NOT NULL, summary TEXT NOT NULL, transcript TEXT NOT NULL DEFAULT '',
 occurred_at TEXT NOT NULL, published_at TEXT NOT NULL, updated_at TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 1,
 trashed_at TEXT, UNIQUE(space_id,owner_id,source_key)
);
CREATE TABLE IF NOT EXISTS note_folders (note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE, folder_id TEXT NOT NULL REFERENCES folders(id), PRIMARY KEY(note_id,folder_id));
CREATE TABLE IF NOT EXISTS recipes (id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id), owner_id TEXT NOT NULL REFERENCES users(id), name TEXT NOT NULL, prompt TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('recipe','template')), revision INTEGER NOT NULL DEFAULT 1);
CREATE TABLE IF NOT EXISTS audit (id INTEGER PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id), actor_id TEXT NOT NULL REFERENCES users(id), action TEXT NOT NULL, target_id TEXT NOT NULL, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS saved_answers (id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id), user_id TEXT NOT NULL REFERENCES users(id), question TEXT NOT NULL, answer TEXT NOT NULL, limited INTEGER NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS answer_sources (answer_id TEXT NOT NULL REFERENCES saved_answers(id) ON DELETE CASCADE, note_id TEXT NOT NULL REFERENCES notes(id), revision INTEGER NOT NULL, citation TEXT NOT NULL, PRIMARY KEY(answer_id,note_id));
CREATE TABLE IF NOT EXISTS integration_keys (id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id), name TEXT NOT NULL, hash TEXT NOT NULL UNIQUE, transcripts INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, expires_at INTEGER NOT NULL, revoked_at TEXT);
CREATE TABLE IF NOT EXISTS integration_spaces (key_id TEXT NOT NULL REFERENCES integration_keys(id) ON DELETE CASCADE, space_id TEXT NOT NULL REFERENCES spaces(id), PRIMARY KEY(key_id,space_id));
CREATE INDEX IF NOT EXISTS answers_owner ON saved_answers(org_id,user_id,created_at);
CREATE TABLE IF NOT EXISTS conversations (id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id), user_id TEXT NOT NULL REFERENCES users(id), scope_json TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS conversation_turns (id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE, position INTEGER NOT NULL, question TEXT NOT NULL, answer TEXT NOT NULL, limited INTEGER NOT NULL, created_at TEXT NOT NULL, UNIQUE(conversation_id,position));
CREATE TABLE IF NOT EXISTS conversation_sources (turn_id TEXT NOT NULL REFERENCES conversation_turns(id) ON DELETE CASCADE, note_id TEXT NOT NULL REFERENCES notes(id), revision INTEGER NOT NULL, citation TEXT NOT NULL, PRIMARY KEY(turn_id,note_id));
CREATE INDEX IF NOT EXISTS conversations_owner ON conversations(org_id,user_id,updated_at);
CREATE INDEX IF NOT EXISTS notes_space_date ON notes(space_id,occurred_at);
CREATE INDEX IF NOT EXISTS members_user ON members(user_id);
CREATE INDEX IF NOT EXISTS spaces_org ON spaces(org_id);
CREATE INDEX IF NOT EXISTS audit_org ON audit(org_id,id);
CREATE TABLE IF NOT EXISTS chat_rooms (
 id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(id),
 kind TEXT NOT NULL CHECK(kind IN ('channel','direct')), name TEXT NOT NULL,
 description TEXT NOT NULL DEFAULT '', direct_key TEXT, created_by TEXT NOT NULL REFERENCES users(id),
 created_at TEXT NOT NULL, archived_at TEXT, revision INTEGER NOT NULL DEFAULT 1,
 UNIQUE(org_id,direct_key)
);
CREATE UNIQUE INDEX IF NOT EXISTS chat_channel_name ON chat_rooms(org_id,name COLLATE NOCASE) WHERE kind='channel';
CREATE TABLE IF NOT EXISTS chat_participants (room_id TEXT NOT NULL REFERENCES chat_rooms(id), user_id TEXT NOT NULL REFERENCES users(id), PRIMARY KEY(room_id,user_id));
CREATE TABLE IF NOT EXISTS chat_messages (
 id TEXT PRIMARY KEY, room_id TEXT NOT NULL REFERENCES chat_rooms(id), author_id TEXT NOT NULL REFERENCES users(id),
 client_id TEXT NOT NULL, original_hash TEXT NOT NULL, body TEXT NOT NULL,
 created_at TEXT NOT NULL, edited_at TEXT, deleted_at TEXT, revision INTEGER NOT NULL DEFAULT 1,
 created_seq INTEGER NOT NULL DEFAULT 0, UNIQUE(room_id,author_id,client_id)
);
CREATE TABLE IF NOT EXISTS chat_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, room_id TEXT NOT NULL REFERENCES chat_rooms(id), message_id TEXT NOT NULL REFERENCES chat_messages(id));
CREATE TABLE IF NOT EXISTS chat_reads (room_id TEXT NOT NULL REFERENCES chat_rooms(id), user_id TEXT NOT NULL REFERENCES users(id), seq INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(room_id,user_id));
CREATE INDEX IF NOT EXISTS chat_rooms_org ON chat_rooms(org_id,kind);
CREATE INDEX IF NOT EXISTS chat_participant_user ON chat_participants(user_id,room_id);
CREATE INDEX IF NOT EXISTS chat_message_history ON chat_messages(room_id,created_seq);
CREATE INDEX IF NOT EXISTS chat_room_events ON chat_events(room_id,seq);

CREATE TABLE IF NOT EXISTS user_profiles (
 user_id TEXT PRIMARY KEY REFERENCES users(id), title TEXT NOT NULL DEFAULT '', about TEXT NOT NULL DEFAULT '',
 avatar_data TEXT NOT NULL DEFAULT '', avatar_version TEXT NOT NULL DEFAULT '', revision INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS chat_reactions (
 message_id TEXT NOT NULL REFERENCES chat_messages(id), user_id TEXT NOT NULL REFERENCES users(id),
 emoji TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(message_id,user_id,emoji)
);
CREATE TABLE IF NOT EXISTS chat_mention_reads (
 user_id TEXT NOT NULL REFERENCES users(id),
 message_id TEXT NOT NULL REFERENCES chat_messages(id),
 PRIMARY KEY(user_id,message_id)
);
CREATE TABLE IF NOT EXISTS chat_notification_preferences (
 room_id TEXT NOT NULL REFERENCES chat_rooms(id),
 user_id TEXT NOT NULL REFERENCES users(id),
 mode TEXT NOT NULL CHECK(mode IN ('messages','mentions','none')),
 PRIMARY KEY(room_id,user_id)
);
