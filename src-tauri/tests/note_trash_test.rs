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
    let conn = db::init(&path).unwrap();
    (path, conn)
}

fn save_schedule_note(conn: &mut rusqlite::Connection, image_path: &str) -> i64 {
    db::save_note(
        conn,
        SaveInput {
            raw_text: "Plan the launch review with Maya".into(),
            source: "photo".into(),
            image_path: Some(image_path.into()),
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "schedule".into(),
                description: "daily schedule".into(),
                data: json!({
                    "blocks": [{"task": "Launch review", "start": "14:00"}]
                }),
            }],
        },
        "2026-08-06T13:00:00Z",
    )
    .unwrap()
}

fn scalar(conn: &rusqlite::Connection, sql: &str, id: i64) -> i64 {
    conn.query_row(sql, [id], |row| row.get(0)).unwrap()
}

#[test]
fn ordinary_note_trash_is_reversible_hidden_everywhere_and_dependency_safe() {
    let (path, mut conn) = test_db("ordinary_note_trash");
    let image_path = std::env::temp_dir()
        .join("noted-managed-image.png")
        .to_string_lossy()
        .to_string();
    let note_id = save_schedule_note(&mut conn, &image_path);
    let entry_id: i64 = conn
        .query_row(
            "SELECT id FROM entries WHERE note_id = ?1",
            [note_id],
            |row| row.get(0),
        )
        .unwrap();

    let folders = db::list_note_folders(&conn).unwrap();
    let folder_id = folders
        .iter()
        .find(|folder| folder.kind == "folder")
        .unwrap()
        .id;
    db::file_note(&conn, note_id, Some(folder_id), "2026-08-06T13:01:00Z").unwrap();
    db::insert_embedding(&conn, note_id, &vec![0.1; 768]).unwrap();

    let maya = db::create_entity(
        &conn,
        "Maya",
        "maya",
        "person",
        "[]",
        "2026-08-06",
        "2026-08-06T13:00:00Z",
    )
    .unwrap();
    let launch = db::create_entity(
        &conn,
        "Launch",
        "launch",
        "topic",
        "[]",
        "2026-08-06",
        "2026-08-06T13:00:00Z",
    )
    .unwrap();
    for entity_id in [maya, launch] {
        db::add_mention(
            &conn,
            entity_id,
            note_id,
            Some(entry_id),
            "Launch review",
            "2026-08-06",
            "2026-08-06T13:00:00Z",
        )
        .unwrap();
    }
    conn.execute(
        "UPDATE entities SET home_note_id = ?2 WHERE id = ?1",
        rusqlite::params![launch, note_id],
    )
    .unwrap();

    let visible_notes = db::list_notes(&conn).unwrap();
    assert_eq!(visible_notes.len(), 1);
    assert!(visible_notes[0].trashed_at.is_none());
    assert!(db::list_trashed_notes(&conn).unwrap().is_empty());
    assert_eq!(db::category_entries(&conn, "schedule").unwrap().len(), 1);
    assert_eq!(
        db::entries_between(&conn, "2026-08-06", "2026-08-06")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db::recent_entries(&conn, 10).unwrap().len(), 1);
    assert_eq!(db::notes_on_date(&conn, "2026-08-06", 10).unwrap().len(), 1);
    assert_eq!(db::notes_for_entity(&conn, maya, 10).unwrap().len(), 1);
    assert_eq!(db::entity_edges(&conn).unwrap().len(), 1);
    assert_eq!(
        db::search_notes(&conn, &vec![0.1; 768], 10).unwrap().len(),
        1
    );
    assert!(db::delete_note_forever(&mut conn, note_id)
        .unwrap()
        .is_none());
    assert!(!db::restore_note(&conn, note_id).unwrap());

    assert!(db::trash_note(&conn, note_id, "2026-08-06T14:00:00Z").unwrap());
    assert!(!db::trash_note(&conn, note_id, "2026-08-06T14:01:00Z").unwrap());
    assert!(db::list_notes(&conn).unwrap().is_empty());
    let trashed_notes = db::list_trashed_notes(&conn).unwrap();
    assert_eq!(trashed_notes[0].id, note_id);
    assert_eq!(
        trashed_notes[0].trashed_at.as_deref(),
        Some("2026-08-06T14:00:00Z")
    );
    assert!(db::list_note_folders(&conn)
        .unwrap()
        .iter()
        .all(|folder| !folder.note_ids.contains(&note_id)));
    assert!(db::category_entries(&conn, "schedule").unwrap().is_empty());
    assert!(db::entries_between(&conn, "2026-08-06", "2026-08-06")
        .unwrap()
        .is_empty());
    assert!(db::recent_entries(&conn, 10).unwrap().is_empty());
    assert!(db::notes_on_date(&conn, "2026-08-06", 10)
        .unwrap()
        .is_empty());
    assert!(db::notes_for_entity(&conn, maya, 10).unwrap().is_empty());
    assert!(db::entity_detail(&conn, maya, 10).unwrap().is_empty());
    assert!(db::mentions_for(&conn, maya).unwrap().is_empty());
    assert!(db::entity_edges(&conn).unwrap().is_empty());
    assert!(db::search_notes(&conn, &vec![0.1; 768], 10)
        .unwrap()
        .is_empty());
    assert!(db::all_entry_data(&conn).unwrap().is_empty());
    assert!(db::note_entries(&conn, note_id).unwrap().is_empty());
    assert!(db::all_note_embedding_inputs(&conn).unwrap().is_empty());
    assert!(db::schedule_blocks_for(&conn, "2026-08-06")
        .unwrap()
        .is_empty());
    assert!(db::update_note(&conn, note_id, "Hidden", "Hidden").is_err());
    assert!(db::update_entry_data(&conn, entry_id, &json!({"task": "Hidden"})).is_err());
    assert!(db::file_note(&conn, note_id, Some(folder_id), "2026-08-06T14:02:00Z").is_err());
    assert_eq!(
        db::embedding_count(&conn).unwrap(),
        1,
        "Trash preserves the index for restore"
    );
    assert_eq!(
        db::list_categories(&conn)
            .unwrap()
            .into_iter()
            .find(|category| category.name == "schedule")
            .unwrap()
            .entry_count,
        0
    );
    assert_eq!(
        scalar(
            &conn,
            "SELECT mention_count FROM entities WHERE id = ?1",
            maya
        ),
        0
    );

    assert!(db::restore_note(&conn, note_id).unwrap());
    assert_eq!(db::list_notes(&conn).unwrap().len(), 1);
    assert!(db::list_trashed_notes(&conn).unwrap().is_empty());
    assert_eq!(
        db::search_notes(&conn, &vec![0.1; 768], 10).unwrap().len(),
        1
    );
    assert_eq!(db::mentions_for(&conn, maya).unwrap().len(), 1);
    assert_eq!(
        scalar(
            &conn,
            "SELECT mention_count FROM entities WHERE id = ?1",
            maya
        ),
        1
    );

    assert!(db::trash_note(&conn, note_id, "2026-08-06T15:00:00Z").unwrap());
    let deleted = db::delete_note_forever(&mut conn, note_id)
        .unwrap()
        .unwrap();
    assert_eq!(deleted.image_path.as_deref(), Some(image_path.as_str()));
    for (table, column) in [
        ("notes", "id"),
        ("entries", "note_id"),
        ("entity_mentions", "note_id"),
        ("embeddings", "note_id"),
        ("note_folder_items", "note_id"),
        ("note_filing_events", "note_id"),
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1");
        assert_eq!(
            scalar(&conn, &sql, note_id),
            0,
            "{table} dependency remains"
        );
    }
    assert_eq!(
        scalar(
            &conn,
            "SELECT home_note_id IS NOT NULL FROM entities WHERE id = ?1",
            launch
        ),
        0
    );
    assert_eq!(
        scalar(
            &conn,
            "SELECT mention_count FROM entities WHERE id = ?1",
            maya
        ),
        0
    );
    assert_eq!(
        db::list_categories(&conn)
            .unwrap()
            .into_iter()
            .find(|category| category.name == "schedule")
            .unwrap()
            .entry_count,
        0
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn meeting_backed_notes_are_rejected_by_the_ordinary_note_lifecycle() {
    let (path, mut conn) = test_db("meeting_note_trash_rejection");
    let note_id = save_schedule_note(&mut conn, "/tmp/meeting-note.png");
    let meeting_id =
        store::create_meeting(&conn, "Launch review", None, None, "2026-08-06T13:00:00Z").unwrap();
    store::set_status(&conn, meeting_id, "done").unwrap();
    store::set_note_id(&conn, meeting_id, note_id).unwrap();

    let trash_error = db::trash_note(&conn, note_id, "2026-08-06T14:00:00Z")
        .unwrap_err()
        .to_string();
    assert!(trash_error.contains("meeting Trash lifecycle"));
    assert!(db::restore_note(&conn, note_id)
        .unwrap_err()
        .to_string()
        .contains("meeting Trash lifecycle"));

    conn.execute(
        "UPDATE notes SET trashed_at = '2026-08-06T14:00:00Z' WHERE id = ?1",
        [note_id],
    )
    .unwrap();
    assert!(db::delete_note_forever(&mut conn, note_id)
        .unwrap_err()
        .to_string()
        .contains("meeting Trash lifecycle"));
    assert_eq!(
        scalar(&conn, "SELECT COUNT(*) FROM notes WHERE id = ?1", note_id),
        1
    );
    assert_eq!(
        scalar(
            &conn,
            "SELECT COUNT(*) FROM meetings WHERE id = ?1",
            meeting_id
        ),
        1
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn trashed_vectors_do_not_consume_semantic_search_results() {
    let (path, mut conn) = test_db("trashed_vector_search_budget");
    let trashed_id = save_schedule_note(&mut conn, "/tmp/trashed-search-note.png");
    let visible_id = save_schedule_note(&mut conn, "/tmp/visible-search-note.png");

    let query = vec![0.1; 768];
    db::insert_embedding(&conn, trashed_id, &query).unwrap();
    db::insert_embedding(&conn, visible_id, &vec![0.2; 768]).unwrap();
    db::trash_note(&conn, trashed_id, "2026-08-06T16:00:00Z").unwrap();

    let hits = db::search_notes(&conn, &query, 1).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].note_id, visible_id);

    drop(conn);
    let _ = std::fs::remove_file(path);
}
