use tauri_app_lib::db;

fn test_db(label: &str) -> (std::path::PathBuf, rusqlite::Connection) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("noted_{label}_{}_{}.db", std::process::id(), nonce));
    let connection = db::init(&path).unwrap();
    (path, connection)
}

#[test]
fn rich_notes_create_in_the_selected_folder_and_round_trip_formatting() {
    let (path, mut connection) = test_db("rich_note_round_trip");
    let work_id = connection
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND lower(name) = 'work'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let folder_id = db::create_note_folder(
        &connection,
        Some(work_id),
        "Writing",
        "folder",
        "",
        "2026-09-01T18:00:00Z",
    )
    .unwrap();
    let first_document = r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"First draft","marks":[{"type":"bold"}]}]}]}"#;

    let note_id = db::create_document_note(
        &mut connection,
        "Launch brief",
        "First draft",
        first_document,
        "work",
        Some(folder_id),
        "2026-09-01T18:01:00Z",
    )
    .unwrap();

    let note = db::list_notes(&connection)
        .unwrap()
        .into_iter()
        .find(|note| note.id == note_id)
        .unwrap();
    assert_eq!(note.title, "Launch brief");
    assert_eq!(note.raw_text, "First draft");
    assert_eq!(note.document_json.as_deref(), Some(first_document));
    assert_eq!(note.note_kind, "document");
    assert_eq!(note.updated_at, "2026-09-01T18:01:00Z");
    assert!(
        note.entries.is_empty(),
        "authored notes do not invent a category"
    );
    let folder = db::list_note_folders(&connection)
        .unwrap()
        .into_iter()
        .find(|folder| folder.id == folder_id)
        .unwrap();
    assert!(folder.note_ids.contains(&note_id));

    let second_document = r#"{"type":"doc","content":[{"type":"taskList","content":[{"type":"taskItem","attrs":{"checked":false},"content":[{"type":"paragraph","content":[{"type":"text","text":"Review draft"}]}]}]}]}"#;
    db::update_note_with_document(
        &connection,
        note_id,
        "Launch plan",
        "- [ ] Review draft",
        Some(second_document),
        "2026-09-01T18:02:00Z",
    )
    .unwrap();
    let updated = db::list_notes(&connection)
        .unwrap()
        .into_iter()
        .find(|note| note.id == note_id)
        .unwrap();
    assert_eq!(updated.title, "Launch plan");
    assert_eq!(updated.raw_text, "- [ ] Review draft");
    assert_eq!(updated.document_json.as_deref(), Some(second_document));
    assert_eq!(updated.updated_at, "2026-09-01T18:02:00Z");

    db::update_note(
        &connection,
        note_id,
        "Plain-text compatibility",
        "Updated by a legacy caller",
        "2026-09-01T18:03:00Z",
    )
    .unwrap();
    let compatible = db::list_notes(&connection)
        .unwrap()
        .into_iter()
        .find(|note| note.id == note_id)
        .unwrap();
    assert_eq!(compatible.document_json.as_deref(), Some(second_document));
    assert_eq!(compatible.note_kind, "document");
    assert_eq!(compatible.updated_at, "2026-09-01T18:03:00Z");

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn legacy_rich_notes_are_backfilled_as_documents_once() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "noted_legacy_document_{}_{}",
        std::process::id(),
        nonce
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("noted.db");
    let legacy = rusqlite::Connection::open(&path).unwrap();
    legacy
        .execute_batch(
            r#"CREATE TABLE notes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 title TEXT NOT NULL DEFAULT '',
                 raw_text TEXT NOT NULL,
                 document_json TEXT,
                 source TEXT NOT NULL DEFAULT 'text',
                 image_path TEXT,
                 category_id INTEGER,
                 created_at TEXT NOT NULL,
                 origin TEXT NOT NULL DEFAULT 'capture',
                 source_path TEXT,
                 content_hash TEXT,
                 synced_at TEXT,
                 filing_context TEXT,
                 trashed_at TEXT
               );
               INSERT INTO notes
                 (id, title, raw_text, document_json, source, created_at)
               VALUES
                 (1, 'Draft', 'Draft body', '{"type":"doc","content":[]}', 'text', '2026-08-30T10:00:00Z'),
                 (2, 'Capture', 'Captured thought', NULL, 'text', '2026-08-30T11:00:00Z');"#,
        )
        .unwrap();
    drop(legacy);

    let connection = db::init(&path).unwrap();
    let kinds = db::list_notes(&connection)
        .unwrap()
        .into_iter()
        .map(|note| (note.id, note.note_kind, note.updated_at))
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            (2, "capture".to_string(), "2026-08-30T11:00:00Z".to_string()),
            (1, "document".to_string(), "2026-08-30T10:00:00Z".to_string()),
        ]
    );
    connection
        .execute(
            "INSERT INTO categories
               (name, description, schema_json, entry_count, created_at)
             VALUES ('journal', '', '{\"shape\":{},\"field_freq\":{}}', 1, '2026-08-30T11:00:00Z')",
            [],
        )
        .unwrap();
    let category_id = connection.last_insert_rowid();
    connection
        .execute(
            "INSERT INTO entries
               (note_id, category_id, data_json, event_date, created_at)
             VALUES (2, ?1, '{}', '2026-08-30', '2026-08-30T11:00:00Z')",
            [category_id],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE notes
             SET document_json = '{\"type\":\"doc\",\"content\":[]}',
                 note_kind = 'document'
             WHERE id = 2",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE notes SET note_kind = 'capture' WHERE id = 1",
            [],
        )
        .unwrap();
    drop(connection);

    let reopened = db::init(&path).unwrap();
    let kind: String = reopened
        .query_row("SELECT note_kind FROM notes WHERE id = 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kind, "capture", "the legacy inference must not run again");
    let rich_capture_kind: String = reopened
        .query_row("SELECT note_kind FROM notes WHERE id = 2", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        rich_capture_kind, "capture",
        "formatting must not turn an extracted capture into a document"
    );
    drop(reopened);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rich_note_creation_rejects_a_folder_from_another_context() {
    let (path, mut connection) = test_db("rich_note_scope");
    let personal_id = connection
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND lower(name) = 'personal'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let personal_folder = db::create_note_folder(
        &connection,
        Some(personal_id),
        "Private",
        "folder",
        "",
        "2026-09-01T18:00:00Z",
    )
    .unwrap();

    let error = db::create_document_note(
        &mut connection,
        "Wrong context",
        "",
        r#"{"type":"doc","content":[{"type":"paragraph"}]}"#,
        "work",
        Some(personal_folder),
        "2026-09-01T18:01:00Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains("selected context"));

    drop(connection);
    let _ = std::fs::remove_file(path);
}
