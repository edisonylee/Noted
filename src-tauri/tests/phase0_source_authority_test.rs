use std::time::{SystemTime, UNIX_EPOCH};

use tauri_app_lib::db;

fn temp_db() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "noted_phase0_authority_{}_{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

#[test]
fn registered_brain_files_are_externally_authoritative_mirrors() {
    let path = temp_db();
    cleanup(&path);
    let conn = db::init(&path).unwrap();
    db::upsert_brain_vault(&conn, "example", "/synthetic/ExampleBrain", "import").unwrap();

    let note_id = db::upsert_brain_note(
        &conn,
        "brain:example",
        "people/Ada.md",
        "External version one",
        "hash-one",
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    conn.execute(
        "UPDATE notes SET raw_text = 'local mirror edit' WHERE id = ?1",
        [note_id],
    )
    .unwrap();

    let refreshed_id = db::upsert_brain_note(
        &conn,
        "brain:example",
        "people/Ada.md",
        "External version two",
        "hash-two",
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    assert_eq!(
        refreshed_id, note_id,
        "the external source path is the mirror key"
    );

    let mirror: (String, String, String, Option<String>, i64) = conn
        .query_row(
            "SELECT raw_text, origin, source_path, filing_context,
                    (SELECT count(*) FROM entries WHERE note_id = notes.id)
             FROM notes WHERE id = ?1",
            [note_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(mirror.0, "External version two");
    assert_eq!(mirror.1, "brain:example");
    assert_eq!(mirror.2, "people/Ada.md");
    assert_eq!(
        mirror.3, None,
        "current imports do not prove a disclosure scope"
    );
    assert_eq!(mirror.4, 0, "brain file content is not a Noted-owned entry");

    db::remove_brain_vault(&conn, "example").unwrap();
    let remaining: i64 = conn
        .query_row(
            "SELECT count(*) FROM notes WHERE id = ?1",
            [note_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        remaining, 1,
        "unregistering a root must not silently destroy its mirror"
    );

    drop(conn);
    cleanup(&path);
}
