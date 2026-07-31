use serde_json::json;
use tauri_app_lib::db::{self, EntryInput, SaveInput};

fn save(
    conn: &mut rusqlite::Connection,
    text: &str,
    category: &str,
    date: &str,
) -> i64 {
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
    folders
        .iter()
        .find(|folder| folder.name == name)
        .unwrap()
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
    assert_eq!(work.kind, "space");
    assert_eq!(baro.parent_id, Some(work.id));
    assert_eq!(standups.parent_id, Some(baro.id));
    assert_eq!(standups.auto_rule, "daily_standup");

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
    assert!(!standups.note_ids.contains(&unrelated));

    let personal = by_name(&folders, "Personal");
    db::file_note(
        &conn,
        unrelated,
        Some(personal.id),
        "2026-07-31T12:00:00Z",
    )
    .unwrap();
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
    db::file_note(
        &conn,
        unrelated,
        Some(receipts),
        "2026-07-31T12:01:00Z",
    )
    .unwrap();
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
