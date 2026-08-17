use serde_json::json;
use tauri_app_lib::{
    db::{self, EntryInput, SaveInput},
    meeting::store,
};

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

fn scalar(connection: &rusqlite::Connection, sql: &str) -> i64 {
    connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

fn note_revision(connection: &rusqlite::Connection, note_id: i64) -> i64 {
    connection
        .query_row(
            "SELECT h.accepted_revision
             FROM portable_records p
             JOIN record_heads h ON h.record_id = p.record_id
             WHERE p.source_table = 'notes' AND p.source_row_id = ?1",
            [note_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn portable_revision(connection: &rusqlite::Connection, table: &str, row_id: i64) -> i64 {
    connection
        .query_row(
            "SELECT h.accepted_revision
             FROM portable_records p
             JOIN record_heads h ON h.record_id = p.record_id
             WHERE p.source_table = ?1 AND p.source_row_id = ?2",
            rusqlite::params![table, row_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn note_change_count(connection: &rusqlite::Connection) -> i64 {
    scalar(
        connection,
        "SELECT COUNT(*) FROM change_log WHERE record_kind = 'note'",
    )
}

fn ordinary_input(raw_text: &str, source: &str) -> SaveInput {
    SaveInput {
        raw_text: raw_text.into(),
        source: source.into(),
        image_path: None,
        event_date: "2026-08-16".into(),
        entries: vec![EntryInput {
            category: "journal".into(),
            description: "A journal entry".into(),
            data: json!({"body": raw_text}),
        }],
    }
}

#[test]
fn ordinary_note_outer_writes_emit_exactly_one_mutation_each() {
    let (path, mut connection) = test_db("note_journal_boundaries");
    let note_id = db::save_note(
        &mut connection,
        ordinary_input("Original body", "text"),
        "2026-08-16T10:00:00Z",
    )
    .unwrap();
    assert_eq!(note_revision(&connection, note_id), 1);
    assert_eq!(note_change_count(&connection), 1);

    db::update_note(
        &connection,
        note_id,
        "Changed title",
        "Changed body",
        "2026-08-16T11:00:00Z",
    )
    .unwrap();
    assert_eq!(note_revision(&connection, note_id), 2);

    assert!(db::trash_note(&connection, note_id, "2026-08-16T12:00:00Z").unwrap());
    assert_eq!(note_revision(&connection, note_id), 3);
    assert_eq!(
        connection
            .query_row(
                "SELECT lifecycle_state FROM portable_records
                 WHERE source_table = 'notes' AND source_row_id = ?1",
                [note_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "trash"
    );

    assert!(db::restore_note(&connection, note_id, "2026-08-16T13:00:00Z").unwrap());
    assert_eq!(note_revision(&connection, note_id), 4);

    let personal_id = connection
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND name = 'Personal'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let projects_id = db::create_note_folder(
        &connection,
        Some(personal_id),
        "Portable project",
        "folder",
        "",
        "2026-08-16T13:30:00Z",
    )
    .unwrap();
    let filing = db::file_note(
        &connection,
        note_id,
        Some(projects_id),
        "2026-08-16T14:00:00Z",
    )
    .unwrap();
    assert_eq!(note_revision(&connection, note_id), 5);
    db::undo_note_filing(&connection, filing.event_id, "2026-08-16T15:00:00Z").unwrap();
    assert_eq!(note_revision(&connection, note_id), 6);

    let entry_id = connection
        .query_row(
            "SELECT id FROM entries WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    db::update_entry_data(
        &connection,
        entry_id,
        &json!({"status": "reviewed"}),
        "2026-08-16T16:00:00Z",
    )
    .unwrap();
    assert_eq!(note_revision(&connection, note_id), 7);
    assert_eq!(note_change_count(&connection), 7);
    assert_eq!(
        scalar(&connection, "SELECT COUNT(*) FROM change_transactions"),
        8
    );
    assert_eq!(
        scalar(
            &connection,
            "SELECT COUNT(*) FROM change_log WHERE record_kind = 'folder'"
        ),
        1
    );
    assert_eq!(
        scalar(
            &connection,
            "SELECT COUNT(*)
             FROM sync_outbox o JOIN change_log l ON l.mutation_id = o.mutation_id
             WHERE l.record_kind = 'note'"
        ),
        7
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT v.accepted_at
                 FROM portable_records p
                 JOIN record_heads h ON h.record_id = p.record_id
                 JOIN record_versions v ON v.version_id = h.accepted_version_id
                 WHERE p.source_table = 'notes' AND p.source_row_id = ?1",
                [note_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "2026-08-16T16:00:00.000Z"
    );

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn standalone_category_and_folder_writes_share_their_domain_transaction() {
    let (path, connection) = test_db("dependency_write_boundaries");

    let category_id = db::create_category(
        &connection,
        "research",
        "Reference material",
        "2026-08-20T09:00:00Z",
    )
    .unwrap();
    assert_eq!(portable_revision(&connection, "categories", category_id), 1);
    let category_changes = scalar(
        &connection,
        "SELECT COUNT(*) FROM change_log WHERE record_kind = 'category'",
    );
    assert_eq!(
        db::create_category(
            &connection,
            "research",
            "Ignored duplicate",
            "2026-08-20T09:01:00Z",
        )
        .unwrap(),
        category_id
    );
    assert_eq!(
        scalar(
            &connection,
            "SELECT COUNT(*) FROM change_log WHERE record_kind = 'category'"
        ),
        category_changes
    );

    let personal_id = connection
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND name = 'Personal'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let folder_id = db::create_note_folder(
        &connection,
        Some(personal_id),
        "Reading",
        "folder",
        "",
        "2026-08-20T09:02:00Z",
    )
    .unwrap();
    assert_eq!(portable_revision(&connection, "note_folders", folder_id), 1);
    db::rename_note_folder(
        &connection,
        folder_id,
        "Research reading",
        "2026-08-20T09:03:00Z",
    )
    .unwrap();
    assert_eq!(portable_revision(&connection, "note_folders", folder_id), 2);

    connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_dependency_change_log
             BEFORE INSERT ON change_log WHEN NEW.record_kind IN ('category', 'folder')
             BEGIN
               SELECT RAISE(ABORT, 'injected dependency journal failure');
             END;",
        )
        .unwrap();
    let rename_error = db::rename_note_folder(
        &connection,
        folder_id,
        "Must roll back",
        "2026-08-20T09:04:00Z",
    )
    .unwrap_err()
    .to_string();
    assert!(rename_error.contains("injected dependency journal failure"));
    assert_eq!(
        connection
            .query_row(
                "SELECT name FROM note_folders WHERE id = ?1",
                [folder_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "Research reading"
    );
    assert_eq!(portable_revision(&connection, "note_folders", folder_id), 2);

    let category_error = db::create_category(
        &connection,
        "blocked-category",
        "Must roll back",
        "2026-08-20T09:05:00Z",
    )
    .unwrap_err()
    .to_string();
    assert!(category_error.contains("injected dependency journal failure"));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM categories WHERE name = 'blocked-category'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn meeting_source_projection_never_enters_the_portable_note_journal() {
    let (path, mut connection) = test_db("meeting_projection_exclusion");
    let note_id = db::save_note(
        &mut connection,
        ordinary_input("Generated meeting summary", "meeting"),
        "2026-08-16T10:00:00Z",
    )
    .unwrap();

    let meeting_id = store::create_meeting(
        &connection,
        "Generated meeting summary",
        None,
        None,
        "2026-08-16T10:01:00Z",
    )
    .unwrap();
    store::set_note_id(&connection, meeting_id, note_id).unwrap();
    let personal_id = connection
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND name = 'Personal'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let filing = db::file_note(
        &connection,
        note_id,
        Some(personal_id),
        "2026-08-16T10:02:00Z",
    )
    .unwrap();
    db::undo_note_filing(&connection, filing.event_id, "2026-08-16T10:03:00Z").unwrap();
    let entry_id = connection
        .query_row(
            "SELECT id FROM entries WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    db::update_entry_data(
        &connection,
        entry_id,
        &json!({"projection": "corrected"}),
        "2026-08-16T10:04:00Z",
    )
    .unwrap();

    let portable_count = connection
        .query_row(
            "SELECT COUNT(*) FROM portable_records
             WHERE source_table = 'notes' AND source_row_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(portable_count, 0);
    assert_eq!(scalar(&connection, "SELECT COUNT(*) FROM change_log"), 0);
    assert_eq!(scalar(&connection, "SELECT COUNT(*) FROM sync_outbox"), 0);

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn journal_source_defaults_to_personal_sensitive_portability() {
    let (path, mut connection) = test_db("journal_scope_sensitivity");
    let note_id = db::save_note(
        &mut connection,
        ordinary_input("Private reflection", "journal"),
        "2026-08-16T10:00:00Z",
    )
    .unwrap();

    let portable: (String, String, String) = connection
        .query_row(
            "SELECT s.scope_class, p.sensitivity,
                    json_extract(v.snapshot_json, '$.sensitivity')
             FROM portable_records p
             JOIN library_scopes s ON s.scope_id = p.scope_id
             JOIN record_heads h ON h.record_id = p.record_id
             JOIN record_versions v ON v.version_id = h.accepted_version_id
             WHERE p.source_table = 'notes' AND p.source_row_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        portable,
        ("personal".into(), "sensitive".into(), "sensitive".into())
    );

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn journal_failure_rolls_back_the_authoritative_note_edit() {
    let (path, mut connection) = test_db("note_journal_atomicity");
    let note_id = db::save_note(
        &mut connection,
        ordinary_input("Original body", "text"),
        "2026-08-16T10:00:00Z",
    )
    .unwrap();
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_note_change_log
             BEFORE INSERT ON change_log
             BEGIN
               SELECT RAISE(ABORT, 'injected journal failure');
             END;",
        )
        .unwrap();

    let error = db::update_note(
        &connection,
        note_id,
        "Changed title",
        "Changed body",
        "2026-08-16T11:00:00Z",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("injected journal failure"));

    let stored = connection
        .query_row(
            "SELECT title, raw_text FROM notes WHERE id = ?1",
            [note_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(stored, (String::new(), "Original body".to_string()));
    assert_eq!(note_revision(&connection, note_id), 1);
    assert_eq!(note_change_count(&connection), 1);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM record_versions v
                 JOIN portable_records p ON p.record_id = v.record_id
                 WHERE p.source_table = 'notes' AND p.source_row_id = ?1",
                [note_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        scalar(
            &connection,
            "SELECT last_transaction_counter FROM portable_devices WHERE role = 'authority'"
        ),
        1
    );

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn filing_journal_failure_rolls_back_membership_context_and_history() {
    let (path, mut connection) = test_db("filing_journal_atomicity");
    let note_id = db::save_note(
        &mut connection,
        ordinary_input("File this atomically", "text"),
        "2026-08-16T10:00:00Z",
    )
    .unwrap();
    let personal_id = connection
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND name = 'Personal'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let folder_id = db::create_note_folder(
        &connection,
        Some(personal_id),
        "Atomic filing",
        "folder",
        "",
        "2026-08-16T10:30:00Z",
    )
    .unwrap();
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_filing_note_change_log
             BEFORE INSERT ON change_log WHEN NEW.record_kind = 'note'
             BEGIN
               SELECT RAISE(ABORT, 'injected filing journal failure');
             END;",
        )
        .unwrap();

    let error = db::file_note(
        &connection,
        note_id,
        Some(folder_id),
        "2026-08-16T11:00:00Z",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("injected filing journal failure"));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM note_folder_items WHERE note_id = ?1",
                [note_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM note_filing_events WHERE note_id = ?1",
                [note_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT filing_context FROM notes WHERE id = ?1",
                [note_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap(),
        None
    );
    assert_eq!(note_revision(&connection, note_id), 1);
    assert_eq!(note_change_count(&connection), 1);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM portable_records
                 WHERE source_table = 'note_folders' AND source_row_id = ?1",
                [folder_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn undo_journal_failure_preserves_the_observed_filing_receipt() {
    let (path, mut connection) = test_db("undo_journal_atomicity");
    let note_id = db::save_note(
        &mut connection,
        ordinary_input("Undo this atomically", "text"),
        "2026-08-16T10:00:00Z",
    )
    .unwrap();
    let personal_id = connection
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND name = 'Personal'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let receipt = db::file_note(
        &connection,
        note_id,
        Some(personal_id),
        "2026-08-16T11:00:00Z",
    )
    .unwrap();
    assert_eq!(note_revision(&connection, note_id), 2);
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_undo_note_change_log
             BEFORE INSERT ON change_log WHEN NEW.record_kind = 'note'
             BEGIN
               SELECT RAISE(ABORT, 'injected undo journal failure');
             END;",
        )
        .unwrap();

    let error = db::undo_note_filing(&connection, receipt.event_id, "2026-08-16T12:00:00Z")
        .unwrap_err()
        .to_string();
    assert!(error.contains("injected undo journal failure"));
    assert_eq!(
        connection
            .query_row(
                "SELECT folder_id, event_id FROM note_folder_items WHERE note_id = ?1",
                [note_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
        (personal_id, receipt.event_id)
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM note_filing_events WHERE note_id = ?1",
                [note_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(note_revision(&connection, note_id), 2);
    assert_eq!(note_change_count(&connection), 2);

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn entry_journal_failure_rolls_back_the_structured_entry_patch() {
    let (path, mut connection) = test_db("entry_journal_atomicity");
    let note_id = db::save_note(
        &mut connection,
        ordinary_input("Keep the original entry", "text"),
        "2026-08-16T10:00:00Z",
    )
    .unwrap();
    let (entry_id, original): (i64, String) = connection
        .query_row(
            "SELECT id, data_json FROM entries WHERE note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_entry_note_change_log
             BEFORE INSERT ON change_log WHEN NEW.record_kind = 'note'
             BEGIN
               SELECT RAISE(ABORT, 'injected entry journal failure');
             END;",
        )
        .unwrap();

    let error = db::update_entry_data(
        &connection,
        entry_id,
        &json!({"body": "Changed", "status": "reviewed"}),
        "2026-08-16T11:00:00Z",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("injected entry journal failure"));
    assert_eq!(
        connection
            .query_row(
                "SELECT data_json FROM entries WHERE id = ?1",
                [entry_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        original
    );
    assert_eq!(note_revision(&connection, note_id), 1);
    assert_eq!(note_change_count(&connection), 1);

    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[test]
fn ordinary_note_edit_rolls_back_content_and_derived_cleanup_together() {
    let (path, mut connection) = test_db("note_edit_atomicity");
    let note_id = db::save_note(
        &mut connection,
        ordinary_input("Original body", "text"),
        "2026-08-16T10:00:00Z",
    )
    .unwrap();
    let entry_id = connection
        .query_row(
            "SELECT id FROM entries WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let entity_id = db::create_entity(
        &connection,
        "Maya",
        "maya",
        "person",
        "[]",
        "2026-08-16",
        "2026-08-16T10:00:00Z",
    )
    .unwrap();
    db::add_mention(
        &connection,
        entity_id,
        note_id,
        Some(entry_id),
        "Original body",
        "2026-08-16",
        "2026-08-16T10:00:00Z",
    )
    .unwrap();
    db::insert_embedding(&connection, note_id, &vec![0.1; 768]).unwrap();

    connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_mention_cleanup
             BEFORE DELETE ON entity_mentions
             BEGIN
               SELECT RAISE(ABORT, 'injected mention cleanup failure');
             END;",
        )
        .unwrap();

    let error = db::update_note(
        &connection,
        note_id,
        "Changed title",
        "Changed body",
        "2026-08-16T11:00:00Z",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("injected mention cleanup failure"));

    let stored = connection
        .query_row(
            "SELECT title, raw_text FROM notes WHERE id = ?1",
            [note_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(stored, (String::new(), "Original body".to_string()));
    assert_eq!(db::embedding_count(&connection).unwrap(), 1);
    assert_eq!(db::mentions_for(&connection, entity_id).unwrap().len(), 1);

    drop(connection);
    let _ = std::fs::remove_file(path);
}
