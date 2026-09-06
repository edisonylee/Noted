//! Opaque document identities belong to the local database, never a remote row number.
use anyhow::{bail, Result};
use rusqlite::{Connection, OptionalExtension};

fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS team_document_sources (
        local_note_id INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
        source_key TEXT NOT NULL UNIQUE
    )",
    )?;
    Ok(())
}

pub fn identity(conn: &Connection, id: i64) -> Result<String> {
    initialize(conn)?;
    let live: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM notes WHERE id=?1 AND note_kind='document' AND trashed_at IS NULL)",
        [id], |row| row.get(0),
    )?;
    if !live {
        bail!("Only live Library documents can be shared");
    }
    conn.execute(
        "INSERT OR IGNORE INTO team_document_sources(local_note_id,source_key)
        VALUES(?1,'document:v2:'||lower(hex(randomblob(32))))",
        [id],
    )?;
    Ok(conn.query_row(
        "SELECT source_key FROM team_document_sources WHERE local_note_id=?1",
        [id],
        |row| row.get(0),
    )?)
}

pub fn local_id(conn: &Connection, key: &str) -> Result<Option<i64>> {
    initialize(conn)?;
    if !key.starts_with("document:v2:") {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT n.id FROM team_document_sources s JOIN notes n ON n.id=s.local_note_id
        WHERE s.source_key=?1 AND n.note_kind='document' AND n.trashed_at IS NULL",
            [key],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE notes(id INTEGER PRIMARY KEY,note_kind TEXT,trashed_at TEXT);
            INSERT INTO notes VALUES(42,'document',NULL),(43,'capture',NULL);").unwrap();
        conn
    }
    #[test]
    fn opaque_document_identity_is_stable_and_vault_specific() {
        let first = db();
        let second = db();
        let key = identity(&first, 42).unwrap();
        assert_eq!(key, identity(&first, 42).unwrap());
        assert_ne!(key, identity(&second, 42).unwrap());
        assert_eq!(local_id(&first, &key).unwrap(), Some(42));
        assert_eq!(local_id(&second, &key).unwrap(), None);
        assert_eq!(local_id(&first, "document:42").unwrap(), None);
        assert!(identity(&first, 43).is_err());
        first
            .execute("UPDATE notes SET trashed_at='now' WHERE id=42", [])
            .unwrap();
        assert_eq!(local_id(&first, &key).unwrap(), None);
        assert!(identity(&first, 42).is_err());
    }
}
