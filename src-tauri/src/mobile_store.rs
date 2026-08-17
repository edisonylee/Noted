use rusqlite::{params, Connection};
use serde::Serialize;
use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MobileNote {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct MobileStore {
    connection: Mutex<Connection>,
}

impl MobileStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS mobile_notes (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   title TEXT NOT NULL,
                   body TEXT NOT NULL,
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL,
                   deleted_at INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS idx_mobile_notes_updated
                   ON mobile_notes(updated_at DESC);",
            )
            .map_err(|error| error.to_string())?;
        let has_deleted_at: i64 = connection
            .query_row(
                "SELECT COUNT(*)
                 FROM pragma_table_info('mobile_notes')
                 WHERE name = 'deleted_at'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if has_deleted_at == 0 {
            connection
                .execute("ALTER TABLE mobile_notes ADD COLUMN deleted_at INTEGER", [])
                .map_err(|error| error.to_string())?;
        }

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn list(&self, query: Option<&str>) -> Result<Vec<MobileNote>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let trimmed = query.map(str::trim).filter(|value| !value.is_empty());

        if let Some(query) = trimmed {
            let pattern = format!("%{}%", escape_like(query));
            let mut statement = connection
                .prepare(
                    "SELECT id, title, body, created_at, updated_at
                     FROM mobile_notes
                     WHERE deleted_at IS NULL
                       AND (title LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                         OR body LIKE ?1 ESCAPE '\\' COLLATE NOCASE)
                     ORDER BY updated_at DESC, id DESC",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([pattern], note_from_row)
                .map_err(|error| error.to_string())?;
            return rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string());
        }

        let mut statement = connection
            .prepare(
                "SELECT id, title, body, created_at, updated_at
                 FROM mobile_notes
                 WHERE deleted_at IS NULL
                 ORDER BY updated_at DESC, id DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], note_from_row)
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn create(&self, title: &str, body: &str) -> Result<MobileNote, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let timestamp = next_timestamp(&connection)?;
        connection
            .execute(
                "INSERT INTO mobile_notes (title, body, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![title.trim(), body, timestamp],
            )
            .map_err(|error| error.to_string())?;

        Ok(MobileNote {
            id: connection.last_insert_rowid(),
            title: title.trim().to_string(),
            body: body.to_string(),
            created_at: timestamp,
            updated_at: timestamp,
        })
    }

    pub fn update(&self, id: i64, title: &str, body: &str) -> Result<MobileNote, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let timestamp = next_timestamp(&connection)?;
        let changed = connection
            .execute(
                "UPDATE mobile_notes
                 SET title = ?1, body = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![title.trim(), body, timestamp, id],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err(format!("note {id} does not exist"));
        }

        connection
            .query_row(
                "SELECT id, title, body, created_at, updated_at
                 FROM mobile_notes WHERE id = ?1",
                [id],
                note_from_row,
            )
            .map_err(|error| error.to_string())
    }

    pub fn delete(&self, id: i64) -> Result<(), String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let timestamp = next_timestamp(&connection)?;
        let changed = connection
            .execute(
                "UPDATE mobile_notes
                 SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![timestamp, id],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err(format!("note {id} does not exist"));
        }
        Ok(())
    }
}

fn note_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MobileNote> {
    Ok(MobileNote {
        id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn now_millis() -> Result<i64, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    i64::try_from(duration.as_millis()).map_err(|_| "system time is out of range".to_string())
}

fn next_timestamp(connection: &Connection) -> Result<i64, String> {
    let latest: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(updated_at), 0) FROM mobile_notes",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(now_millis()?.max(latest.saturating_add(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> MobileStore {
        MobileStore::open(Path::new(":memory:")).expect("open in-memory mobile store")
    }

    #[test]
    fn notes_survive_crud_and_sort_by_recent_change() {
        let store = store();
        let first = store.create("First", "alpha").expect("create first");
        let second = store.create("Second", "beta").expect("create second");

        let updated = store
            .update(first.id, "First revised", "alpha revised")
            .expect("update first");
        assert_eq!(updated.created_at, first.created_at);
        assert_eq!(store.list(None).expect("list")[0].id, first.id);

        store.delete(second.id).expect("delete second");
        assert_eq!(store.list(None).expect("list").len(), 1);
        let deleted_at: Option<i64> = store
            .connection
            .lock()
            .expect("lock store")
            .query_row(
                "SELECT deleted_at FROM mobile_notes WHERE id = ?1",
                [second.id],
                |row| row.get(0),
            )
            .expect("read tombstone");
        assert!(deleted_at.is_some());
    }

    #[test]
    fn search_matches_title_and_body_but_escapes_wildcards() {
        let store = store();
        store.create("Launch", "Call Sam").expect("create launch");
        store
            .create("Budget 100%", "Review")
            .expect("create budget");

        assert_eq!(store.list(Some("sam")).expect("body search").len(), 1);
        assert_eq!(store.list(Some("100%")).expect("literal search").len(), 1);
        assert!(store
            .list(Some("100_"))
            .expect("escaped underscore")
            .is_empty());
    }

    #[test]
    fn file_backed_notes_survive_store_reopen() {
        let path = std::env::temp_dir().join(format!(
            "noted-mobile-store-{}-{}.sqlite3",
            std::process::id(),
            now_millis().expect("timestamp")
        ));

        {
            let store = MobileStore::open(&path).expect("open file-backed store");
            store
                .create("Persistent", "Still here")
                .expect("create persistent note");
        }

        let reopened = MobileStore::open(&path).expect("reopen file-backed store");
        let notes = reopened.list(None).expect("list reopened notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Persistent");
        drop(reopened);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    }
}
