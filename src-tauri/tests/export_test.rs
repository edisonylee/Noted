// Backup mechanism: WAL-checkpoint then copy yields a consistent single-file DB
// (mirrors what the export_db command does, minus the Desktop path resolution).
use serde_json::json;
use tauri_app_lib::db::{self, SaveInput};

#[test]
fn backup_copy_is_consistent() {
    let src = std::env::temp_dir().join(format!("noted_exp_src_{}.db", std::process::id()));
    let dst = std::env::temp_dir().join(format!("noted_exp_dst_{}.db", std::process::id()));
    for p in [&src, &dst] {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(format!("{}-wal", p.display()));
        let _ = std::fs::remove_file(format!("{}-shm", p.display()));
    }

    let mut conn = db::init(&src).unwrap();
    db::save_note(
        &mut conn,
        SaveInput {
            raw_text: "bench 185".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-06-09".into(),
            entries: vec![db::EntryInput {
                category: "gym".into(),
                description: String::new(),
                data: json!({"exercises":[{"name":"bench","weight":185}]}),
            }],
        },
        "2026-06-09T00:00:00Z",
    )
    .unwrap();

    // export: flush WAL into the main file, then copy it.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    std::fs::copy(&src, &dst).unwrap();

    // the backup opens independently and has the data.
    let copy = db::init(&dst).unwrap();
    let notes = db::list_notes(&copy).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].entries[0].category.as_deref(), Some("gym"));
    assert_eq!(notes[0].event_date, "2026-06-09");

    for p in [&src, &dst] {
        let _ = std::fs::remove_file(p);
    }
}
