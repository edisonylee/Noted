use serde_json::json;
use tauri_app_lib::db::{self, EntryInput, SaveInput};

fn save(conn: &mut rusqlite::Connection, text: &str, category: &str, date: &str) -> i64 {
    db::save_note(
        conn,
        SaveInput {
            raw_text: text.into(),
            source: "text".into(),
            image_path: None,
            event_date: date.into(),
            entries: vec![EntryInput {
                category: category.into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        &format!("{date}T14:00:00Z"),
    )
    .unwrap()
}

fn by_name<'a>(folders: &'a [db::NoteFolderInfo], name: &str) -> &'a db::NoteFolderInfo {
    folders.iter().find(|folder| folder.name == name).unwrap()
}

#[test]
fn seeded_baro_tree_auto_files_standups_and_accepts_manual_filing() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_folder_test_{}_{}.db",
        std::process::id(),
        nonce
    ));
    let mut conn = db::init(&path).unwrap();

    let seeded = db::list_note_folders(&conn).unwrap();
    let work = by_name(&seeded, "Work");
    let baro = by_name(&seeded, "Baro");
    let standups = by_name(&seeded, "Daily Standup Meeting Notes");
    let personal = by_name(&seeded, "Personal");
    assert_eq!(work.kind, "space");
    assert_eq!(baro.parent_id, Some(work.id));
    assert_eq!(standups.parent_id, Some(baro.id));
    assert_eq!(standups.auto_rule, "daily_standup");

    let work_folders: Vec<&str> = seeded
        .iter()
        .filter(|folder| folder.parent_id == Some(work.id))
        .map(|folder| folder.name.as_str())
        .collect();
    assert_eq!(
        work_folders,
        vec!["Baro", "Symphony", "Side Projects", "Career"]
    );
    let personal_folders: Vec<&str> = seeded
        .iter()
        .filter(|folder| folder.parent_id == Some(personal.id))
        .map(|folder| folder.name.as_str())
        .collect();
    assert_eq!(
        personal_folders,
        vec![
            "Health",
            "Finances",
            "Home",
            "Relationships",
            "Travel",
            "Personal Learning"
        ]
    );

    let category_match = save(
        &mut conn,
        "Yesterday I finished the release; today I am reviewing alerts.",
        "daily standup",
        "2026-07-27",
    );
    let text_match = save(
        &mut conn,
        "Baro stand-up: shipped the new capture flow.",
        "meetings",
        "2026-07-28",
    );
    let schedule_match = save(
        &mut conn,
        "11:00 AM-11:15 AM Daily Stand Up",
        "schedule",
        "2026-07-28",
    );
    let unrelated = save(
        &mut conn,
        "Remember to stand up and stretch every hour, then dinner with Maya.",
        "journal",
        "2026-07-28",
    );

    let folders = db::list_note_folders(&conn).unwrap();
    let standups = by_name(&folders, "Daily Standup Meeting Notes");
    assert!(standups.note_ids.contains(&category_match));
    assert!(standups.note_ids.contains(&text_match));
    assert!(!standups.note_ids.contains(&schedule_match));
    assert!(!standups.note_ids.contains(&unrelated));

    let personal = by_name(&folders, "Personal");
    db::file_note(&conn, unrelated, Some(personal.id), "2026-07-31T12:00:00Z").unwrap();
    let filed_space_id: i64 = conn
        .query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [unrelated],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(filed_space_id, personal.id);

    let receipts = db::create_note_folder(
        &conn,
        Some(personal.id),
        "Receipts",
        "folder",
        "",
        "2026-07-31T12:00:00Z",
    )
    .unwrap();
    db::file_note(&conn, unrelated, Some(receipts), "2026-07-31T12:01:00Z").unwrap();
    let folders = db::list_note_folders(&conn).unwrap();
    assert_eq!(by_name(&folders, "Receipts").note_ids, vec![unrelated]);
    let filed_folder_id: i64 = conn
        .query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [unrelated],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(filed_folder_id, receipts);

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn folder_structure_seed_runs_once_and_respects_user_deletions() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_folder_seed_test_{}_{}.db",
        std::process::id(),
        nonce
    ));

    let conn = db::init(&path).unwrap();
    let seeded = db::list_note_folders(&conn).unwrap();
    let side_projects = by_name(&seeded, "Side Projects").id;
    db::delete_note_folder(&conn, side_projects).unwrap();
    drop(conn);

    let conn = db::init(&path).unwrap();
    let reopened = db::list_note_folders(&conn).unwrap();
    assert!(reopened.iter().all(|folder| folder.name != "Side Projects"));
    assert_eq!(
        reopened
            .iter()
            .filter(|folder| folder.name == "Career")
            .count(),
        1
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn folder_structure_upgrade_preserves_existing_folders_and_memberships() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_folder_upgrade_test_{}_{}.db",
        std::process::id(),
        nonce
    ));

    let mut conn = db::init(&path).unwrap();
    let seeded = db::list_note_folders(&conn).unwrap();
    let symphony_id = by_name(&seeded, "Symphony").id;
    let baro_id = by_name(&seeded, "Baro").id;
    let note_id = save(
        &mut conn,
        "Personal product roadmap and release plan.",
        "projects",
        "2026-08-06",
    );
    db::file_note(&conn, note_id, Some(symphony_id), "2026-08-06T12:00:00Z").unwrap();
    let partner_id = db::create_note_folder(
        &conn,
        Some(baro_id),
        "Partner Meetings",
        "folder",
        "",
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    conn.execute(
        "DELETE FROM app_metadata WHERE key = 'note_folders_v2_seeded'",
        [],
    )
    .unwrap();
    drop(conn);

    let conn = db::init(&path).unwrap();
    let upgraded = db::list_note_folders(&conn).unwrap();
    let symphony = by_name(&upgraded, "Symphony");
    let partner = by_name(&upgraded, "Partner Meetings");
    assert_eq!(symphony.id, symphony_id);
    assert_eq!(symphony.note_ids, vec![note_id]);
    assert_eq!(partner.id, partner_id);
    assert_eq!(partner.parent_id, Some(baro_id));

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn legacy_duplicate_memberships_collapse_to_the_latest_home() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_folder_one_home_migration_test_{}_{}.db",
        std::process::id(),
        nonce
    ));

    let mut conn = db::init(&path).unwrap();
    let folders = db::list_note_folders(&conn).unwrap();
    let work = by_name(&folders, "Work").id;
    let personal = by_name(&folders, "Personal").id;
    let note_id = save(
        &mut conn,
        "A legacy note with conflicting folder rows.",
        "misc",
        "2026-08-06",
    );
    conn.execute("DROP INDEX idx_note_folder_items_one_home", [])
        .unwrap();
    conn.execute(
        "INSERT INTO note_folder_items (folder_id, note_id, created_at)
         VALUES (?1, ?2, '2026-08-06T12:00:00Z')",
        rusqlite::params![work, note_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO note_folder_items (folder_id, note_id, created_at)
         VALUES (?1, ?2, '2026-08-06T12:01:00Z')",
        rusqlite::params![personal, note_id],
    )
    .unwrap();
    drop(conn);

    let conn = db::init(&path).unwrap();
    let homes: Vec<i64> = conn
        .prepare("SELECT folder_id FROM note_folder_items WHERE note_id = ?1")
        .unwrap()
        .query_map([note_id], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(homes, vec![personal]);
    assert!(conn
        .execute(
            "INSERT INTO note_folder_items (folder_id, note_id, created_at)
             VALUES (?1, ?2, '2026-08-06T12:02:00Z')",
            rusqlite::params![work, note_id],
        )
        .is_err());

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn folders_can_be_reordered_and_nested_without_creating_cycles() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_folder_move_test_{}_{}.db",
        std::process::id(),
        nonce
    ));
    let conn = db::init(&path).unwrap();
    let seeded = db::list_note_folders(&conn).unwrap();
    let baro = by_name(&seeded, "Baro").id;
    let personal = by_name(&seeded, "Personal").id;
    let standups = by_name(&seeded, "Daily Standup Meeting Notes").id;
    let partner = db::create_note_folder(
        &conn,
        Some(baro),
        "Partner Meetings",
        "folder",
        "",
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let planning = db::create_note_folder(
        &conn,
        Some(baro),
        "Planning",
        "folder",
        "",
        "2026-08-06T12:01:00Z",
    )
    .unwrap();

    db::move_note_folder(&conn, planning, Some(baro), Some(standups)).unwrap();
    let folders = db::list_note_folders(&conn).unwrap();
    let baro_children: Vec<&str> = folders
        .iter()
        .filter(|folder| folder.parent_id == Some(baro))
        .map(|folder| folder.name.as_str())
        .collect();
    assert_eq!(
        baro_children,
        vec![
            "Planning",
            "Daily Standup Meeting Notes",
            "Partner Meetings"
        ]
    );

    db::move_note_folder(&conn, partner, Some(planning), None).unwrap();
    let folders = db::list_note_folders(&conn).unwrap();
    assert_eq!(
        by_name(&folders, "Partner Meetings").parent_id,
        Some(planning)
    );

    let error = db::move_note_folder(&conn, planning, Some(partner), None).unwrap_err();
    assert!(error.to_string().contains("cannot be moved inside itself"));
    let folders = db::list_note_folders(&conn).unwrap();
    assert_eq!(by_name(&folders, "Planning").parent_id, Some(baro));
    assert_eq!(
        by_name(&folders, "Partner Meetings").parent_id,
        Some(planning)
    );

    let error = db::move_note_folder(&conn, planning, Some(personal), None).unwrap_err();
    assert!(error
        .to_string()
        .contains("cannot move between Work and Personal"));

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn new_notes_route_to_the_selected_context_and_only_work_standups_auto_file() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_initial_filing_test_{}_{}.db",
        std::process::id(),
        nonce
    ));
    let mut conn = db::init(&path).unwrap();
    let folders = db::list_note_folders(&conn).unwrap();
    let work = by_name(&folders, "Work").id;
    let personal = by_name(&folders, "Personal").id;
    let baro = by_name(&folders, "Baro").id;
    let standups = by_name(&folders, "Daily Standup Meeting Notes").id;
    let health = by_name(&folders, "Health").id;

    let work_note = db::save_note_with_initial_filing(
        &mut conn,
        SaveInput {
            raw_text: "Daily standup: shipped transcript search.".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "meetings".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "work",
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let personal_note = db::save_note_with_initial_filing(
        &mut conn,
        SaveInput {
            raw_text: "Daily standup: a personal coding check-in.".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "meetings".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "personal",
        None,
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    let reviewed_note = db::save_note_with_initial_filing(
        &mut conn,
        SaveInput {
            raw_text: "Daily standup: health project notes.".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "meetings".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "personal",
        Some(health),
        "2026-08-06T12:02:00Z",
    )
    .unwrap();
    let account_routed_note = db::save_note_with_initial_filing_source(
        &mut conn,
        SaveInput {
            raw_text: "Review the partner launch plan.".into(),
            source: "meeting".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "meetings".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "work",
        Some(baro),
        "rule",
        Some("Matched the approved Baro calendar account rule."),
        "2026-08-06T12:02:30Z",
    )
    .unwrap();
    let account_routed_standup = db::save_note_with_initial_filing_source(
        &mut conn,
        SaveInput {
            raw_text: "Daily standup: shipped the meeting filing repair.".into(),
            source: "meeting".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "meetings".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "work",
        Some(baro),
        "rule",
        Some("Matched the approved Baro calendar account rule."),
        "2026-08-06T12:02:45Z",
    )
    .unwrap();

    let current = |note_id: i64| -> (i64, String, String, Option<i64>) {
        conn.query_row(
            "SELECT folder_id, source, reason, event_id
             FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
    };
    let work_filing = current(work_note);
    assert_eq!(work_filing.0, standups);
    assert_eq!(work_filing.1, "rule");
    assert!(work_filing.2.contains("approved Daily Standup rule"));
    assert!(work_filing.3.is_some());
    assert_eq!(current(personal_note).0, personal);
    assert_eq!(current(personal_note).1, "context");
    assert_eq!(current(reviewed_note).0, health);
    assert_eq!(current(reviewed_note).1, "manual");
    assert_eq!(current(account_routed_note).0, baro);
    assert_eq!(current(account_routed_note).1, "rule");
    assert_eq!(current(account_routed_standup).0, standups);
    assert_eq!(current(account_routed_standup).1, "rule");
    assert!(current(account_routed_standup)
        .2
        .contains("approved Daily Standup rule"));
    assert_eq!(
        current(account_routed_note).2,
        "Matched the approved Baro calendar account rule."
    );

    let contexts: Vec<(i64, Option<String>)> = conn
        .prepare(
            "SELECT id, filing_context FROM notes
             WHERE id IN (?1, ?2, ?3) ORDER BY id",
        )
        .unwrap()
        .query_map(
            rusqlite::params![work_note, personal_note, reviewed_note],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        contexts,
        vec![
            (work_note, Some("work".into())),
            (personal_note, Some("personal".into())),
            (reviewed_note, Some("personal".into())),
        ]
    );

    let listed = db::list_note_folders(&conn).unwrap();
    let standup_folder = by_name(&listed, "Daily Standup Meeting Notes");
    assert!(standup_folder.note_ids.contains(&work_note));
    assert!(!standup_folder.note_ids.contains(&personal_note));
    assert_eq!(
        standup_folder
            .explicit_filings
            .iter()
            .find(|item| item.note_id == work_note)
            .unwrap()
            .source,
        "rule"
    );

    let before_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
        .unwrap();
    let error = db::save_note_with_initial_filing(
        &mut conn,
        SaveInput {
            raw_text: "Wrong context destination".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "projects".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "work",
        Some(health),
        "2026-08-06T12:03:00Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains("selected context"));
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        before_count
    );
    assert_ne!(work, personal);

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn manual_filing_is_sticky_and_undo_restores_folder_and_context() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_filing_undo_test_{}_{}.db",
        std::process::id(),
        nonce
    ));
    let mut conn = db::init(&path).unwrap();
    let folders = db::list_note_folders(&conn).unwrap();
    let standups = by_name(&folders, "Daily Standup Meeting Notes").id;
    let health = by_name(&folders, "Health").id;
    let note_id = db::save_note_with_initial_filing(
        &mut conn,
        SaveInput {
            raw_text: "Daily standup: fixed the routing bug.".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "daily standup".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "work",
        None,
        "2026-08-06T13:00:00Z",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO meetings
           (title, status, note_id, filing_context, route_folder_id, route_via,
            route_status, created_at)
         VALUES ('Routing sync', 'done', ?1, 'work', ?2, 'filing_rule',
                 'matched', '2026-08-06T13:00:00Z')",
        rusqlite::params![note_id, standups],
    )
    .unwrap();

    let moved = db::file_note(&conn, note_id, Some(health), "2026-08-06T13:01:00Z").unwrap();
    assert_eq!(moved.previous_folder_id, Some(standups));
    assert_eq!(moved.folder_id, Some(health));
    assert_eq!(moved.previous_context.as_deref(), Some("work"));
    assert_eq!(moved.filing_context.as_deref(), Some("personal"));
    let moved_meeting: (Option<String>, Option<i64>, String) = conn
        .query_row(
            "SELECT filing_context, route_folder_id, route_status
             FROM meetings WHERE note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        moved_meeting,
        (Some("personal".into()), Some(health), "manual".into())
    );

    let listed = db::list_note_folders(&conn).unwrap();
    assert!(!by_name(&listed, "Daily Standup Meeting Notes")
        .note_ids
        .contains(&note_id));
    let health_item = by_name(&listed, "Health")
        .explicit_filings
        .iter()
        .find(|item| item.note_id == note_id)
        .unwrap();
    assert_eq!(health_item.source, "manual");
    assert_eq!(health_item.filing_context.as_deref(), Some("personal"));

    let undone = db::undo_note_filing(&conn, moved.event_id, "2026-08-06T13:02:00Z").unwrap();
    assert_eq!(undone.folder_id, Some(standups));
    assert_eq!(undone.filing_context.as_deref(), Some("work"));
    assert_eq!(undone.source, "undo");
    let current: (i64, String, Option<String>, i64) = conn
        .query_row(
            "SELECT i.folder_id, i.source, n.filing_context, i.event_id
             FROM note_folder_items i JOIN notes n ON n.id = i.note_id
             WHERE i.note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(current.0, standups);
    assert_eq!(current.1, "undo");
    assert_eq!(current.2.as_deref(), Some("work"));
    assert_eq!(current.3, undone.event_id);
    let restored_meeting: (Option<String>, Option<i64>, String) = conn
        .query_row(
            "SELECT filing_context, route_folder_id, route_status
             FROM meetings WHERE note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        restored_meeting,
        (Some("work".into()), Some(standups), "matched".into())
    );

    let stale = db::undo_note_filing(&conn, moved.event_id, "2026-08-06T13:03:00Z").unwrap_err();
    assert!(stale.to_string().contains("filing changed"));
    let event_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM note_filing_events WHERE note_id = ?1",
            [note_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 4); // context, rule, manual move, undo

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn pending_capture_context_round_trips_without_defaulting_legacy_rows() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_pending_context_test_{}_{}.db",
        std::process::id(),
        nonce
    ));
    let conn = db::init(&path).unwrap();

    db::insert_pending(
        &conn,
        "Work capture",
        "text",
        None,
        None,
        Some("WORK"),
        "2026-08-06T14:00:00Z",
    )
    .unwrap();
    db::insert_pending(
        &conn,
        "Legacy capture",
        "text",
        None,
        None,
        None,
        "2026-08-06T14:01:00Z",
    )
    .unwrap();
    let pending = db::list_pending(&conn, 5).unwrap();
    assert_eq!(pending[0].filing_context.as_deref(), Some("work"));
    assert_eq!(pending[1].filing_context, None);

    let error = db::insert_pending(
        &conn,
        "Unknown context",
        "text",
        None,
        None,
        Some("side-project"),
        "2026-08-06T14:02:00Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains("work or personal"));

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn deleting_a_folder_rehomes_routed_notes_to_their_context_inbox() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_deleted_folder_context_test_{}_{}.db",
        std::process::id(),
        nonce
    ));
    let mut conn = db::init(&path).unwrap();
    let folders = db::list_note_folders(&conn).unwrap();
    let personal = by_name(&folders, "Personal").id;
    let receipts = db::create_note_folder(
        &conn,
        Some(personal),
        "Receipts",
        "folder",
        "",
        "2026-08-06T15:00:00Z",
    )
    .unwrap();
    let note_id = db::save_note_with_initial_filing(
        &mut conn,
        SaveInput {
            raw_text: "Renew the renter's insurance policy.".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![EntryInput {
                category: "finances".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "personal",
        Some(receipts),
        "2026-08-06T15:01:00Z",
    )
    .unwrap();
    let legacy_note_id = save(
        &mut conn,
        "A receipt filed before contexts were persisted.",
        "finances",
        "2026-08-05",
    );
    conn.execute(
        "INSERT INTO note_folder_items (folder_id, note_id, created_at)
         VALUES (?1, ?2, '2026-08-05T15:01:00Z')",
        rusqlite::params![receipts, legacy_note_id],
    )
    .unwrap();

    db::delete_note_folder(&conn, receipts).unwrap();
    let current: (i64, String, String, Option<String>, i64) = conn
        .query_row(
            "SELECT i.folder_id, i.source, i.reason, n.filing_context, i.event_id
             FROM note_folder_items i JOIN notes n ON n.id = i.note_id
             WHERE i.note_id = ?1",
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
    assert_eq!(current.0, personal);
    assert_eq!(current.1, "context");
    assert!(current.2.contains("was deleted"));
    assert_eq!(current.3.as_deref(), Some("personal"));
    let legacy_current: (i64, Option<String>) = conn
        .query_row(
            "SELECT i.folder_id, n.filing_context
             FROM note_folder_items i JOIN notes n ON n.id = i.note_id
             WHERE i.note_id = ?1",
            [legacy_note_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(legacy_current, (personal, Some("personal".into())));
    let undo_error = db::undo_note_filing(&conn, current.4, "2026-08-06T15:02:00Z").unwrap_err();
    assert!(undo_error.to_string().contains("no longer exists"));

    drop(conn);
    let _ = std::fs::remove_file(path);
}
