// Runtime validation for M0/M1 without the GUI:
//  - sqlite-vec actually loads (vec_version + vec0 table) at runtime
//  - save_entry creates a category, then grows its schema additively
use serde_json::json;
use tauri_app_lib::db::{self, SaveInput};

fn save(conn: &mut rusqlite::Connection, cat: &str, desc: &str, data: serde_json::Value, ts: &str) {
    db::save_note(
        conn,
        SaveInput {
            raw_text: format!("note about {cat}"),
            source: "text".into(),
            image_path: None,
            event_date: ts[..10].to_string(), // YYYY-MM-DD from the timestamp
            entries: vec![db::EntryInput { category: cat.into(), description: desc.into(), data }],
        },
        ts,
    )
    .unwrap();
}

#[test]
fn vec_loads_and_schema_evolves() {
    let tmp = std::env::temp_dir().join(format!("noted_test_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);

    let mut conn = db::init(&tmp).expect("db init (loads sqlite-vec, creates vec0 table)");

    // sqlite-vec is live at runtime
    let v: String = conn
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .expect("vec_version() works -> extension loaded");
    assert!(v.starts_with('v'), "unexpected vec_version: {v}");

    // First gym note creates the category from scratch.
    save(
        &mut conn,
        "gym",
        "workouts",
        json!({ "exercises": [{ "name": "bench", "weight": 185, "reps": 5, "sets": 3 }] }),
        "2026-06-02T00:00:00Z",
    );

    // Second gym note introduces a novel field (rpe) -> schema must grow.
    save(
        &mut conn,
        "gym",
        "",
        json!({ "exercises": [{ "name": "squat", "weight": 225, "reps": 5, "sets": 5, "rpe": 8 }] }),
        "2026-06-02T01:00:00Z",
    );

    // A different note creates a second, separate category.
    save(
        &mut conn,
        "schedule",
        "daily time blocks",
        json!({ "blocks": [{ "task": "coding", "duration_min": 120 }] }),
        "2026-06-02T02:00:00Z",
    );

    let cats = db::list_categories(&conn).unwrap();
    assert_eq!(cats.len(), 2, "two emergent categories");

    let gym = cats.iter().find(|c| c.name == "gym").unwrap();
    assert_eq!(gym.entry_count, 2);
    let freq = gym.schema.get("field_freq").unwrap();
    assert_eq!(freq.get("exercises.weight").and_then(|x| x.as_i64()), Some(2));
    assert_eq!(
        freq.get("exercises.rpe").and_then(|x| x.as_i64()),
        Some(1),
        "novel field grew the schema"
    );
    // shape is the union of both entries
    let shape = gym.schema.get("shape").unwrap();
    assert!(shape["exercises"][0].get("rpe").is_some(), "shape merged rpe");

    let notes = db::list_notes(&conn).unwrap();
    assert_eq!(notes.len(), 3);
    assert!(notes.iter().all(|n| n.event_date == "2026-06-02"), "every entry is dated");

    let _ = std::fs::remove_file(&tmp);
}
