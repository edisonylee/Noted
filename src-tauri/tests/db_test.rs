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

#[test]
fn embedding_space_replacement_is_atomic_and_fingerprinted() {
    let tmp = std::env::temp_dir().join(format!("noted_embedding_swap_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();
    save(&mut conn, "work", "", json!({"topic":"routing"}), "2026-06-02T00:00:00Z");
    let note_id: i64 = conn.query_row("SELECT id FROM notes LIMIT 1", [], |r| r.get(0)).unwrap();

    db::replace_embedding_space(&mut conn, "openai|a|768", &[(note_id, vec![0.1; 768])], &[]).unwrap();
    assert_eq!(db::embedding_fingerprint(&conn).unwrap().as_deref(), Some("openai|a|768"));
    assert_eq!(db::embedding_count(&conn).unwrap(), 1);

    // sqlite-vec rejects the wrong dimension after DELETE has run inside the
    // transaction. The rollback must preserve both the old index and marker.
    assert!(db::replace_embedding_space(&mut conn, "openai|bad|768", &[(note_id, vec![0.1; 767])], &[]).is_err());
    assert_eq!(db::embedding_fingerprint(&conn).unwrap().as_deref(), Some("openai|a|768"));
    assert_eq!(db::embedding_count(&conn).unwrap(), 1);

    db::replace_embedding_space(&mut conn, "gemini|b|768", &[(note_id, vec![0.2; 768])], &[]).unwrap();
    assert_eq!(db::embedding_fingerprint(&conn).unwrap().as_deref(), Some("gemini|b|768"));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn suggest_merges_finds_near_duplicates_and_respects_dismissals() {
    let tmp = std::env::temp_dir().join(format!("noted_merge_test_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).expect("db init");
    let now = "2026-06-01T00:00:00Z";

    // Two near-identical people + one unrelated place.
    let a = db::create_entity(&conn, "Sarah", "sarah", "person", "[]", "2026-06-01", now).unwrap();
    let b = db::create_entity(&conn, "Sara", "sara", "person", "[]", "2026-06-01", now).unwrap();
    let c = db::create_entity(&conn, "Gym", "gym", "place", "[]", "2026-06-01", now).unwrap();

    // Hand-built embeddings: a≈b (cosine ~0.98), c orthogonal.
    let mut va = vec![0.0f32; 768];
    va[0] = 1.0;
    let mut vb = vec![0.0f32; 768];
    vb[0] = 0.98;
    vb[1] = 0.2;
    let mut vc = vec![0.0f32; 768];
    vc[1] = 1.0;
    db::insert_entity_embedding(&conn, a, &va).unwrap();
    db::insert_entity_embedding(&conn, b, &vb).unwrap();
    db::insert_entity_embedding(&conn, c, &vc).unwrap();

    let sugg = db::suggest_merges(&conn, 0.82, 20).unwrap();
    assert_eq!(sugg.len(), 1, "only the near-identical same-type pair should surface");
    let pair = (sugg[0].a_id.min(sugg[0].b_id), sugg[0].a_id.max(sugg[0].b_id));
    assert_eq!(pair, (a.min(b), a.max(b)));
    assert!(sugg[0].similarity > 0.9, "cosine should be ~0.98, got {}", sugg[0].similarity);
    assert_eq!(sugg[0].etype, "person");

    // Dismiss (ids deliberately reversed — pair key is order-normalized) → gone.
    db::dismiss_merge(&conn, b, a).unwrap();
    let sugg2 = db::suggest_merges(&conn, 0.82, 20).unwrap();
    assert!(sugg2.is_empty(), "dismissed pair must never resurface");

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn rename_speaker_onto_existing_label_merges_rows() {
    use tauri_app_lib::meeting::{diarize, store};
    let tmp = std::env::temp_dir().join(format!("noted_rename_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).unwrap();
    let id = store::create_meeting(&conn, "Standup", None, None, "2026-07-17T15:00:00Z").unwrap();
    let s1 = store::insert_segment(&conn, id, "them", 0, 5_000, "hello").unwrap();
    let s2 = store::insert_segment(&conn, id, "them", 6_000, 9_000, "yes").unwrap();
    store::set_segment_speakers(&conn, &[(s1, "Brian".into()), (s2, "Speaker 2".into())]).unwrap();
    // Brian's real cluster (10 segments at 1.0) + a small mislabelable one (5 at 0.0).
    store::save_meeting_speakers(
        &conn,
        id,
        &[
            ("Brian".into(), vec![1.0, 0.0], 10),
            ("Speaker 2".into(), vec![0.0, 1.0], 5),
        ],
    )
    .unwrap();

    // The bug: this used to UPDATE OR REPLACE, deleting Brian's real row.
    store::rename_speaker(&conn, id, "Speaker 2", "Brian").unwrap();

    let rows = store::list_meeting_speakers(&conn, id).unwrap();
    assert_eq!(rows.len(), 1, "one merged row, not a vanished one: {rows:?}");
    assert_eq!(rows[0]["label"], "Brian");
    assert_eq!(rows[0]["seg_count"], 15);
    // Weighted centroid: (1.0*10 + 0.0*5)/15, (0.0*10 + 1.0*5)/15
    let cent: Vec<f32> = conn
        .query_row(
            "SELECT centroid FROM meeting_speakers WHERE meeting_id = ?1 AND label = 'Brian'",
            [id],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .map(|b| diarize::blob_to_emb(&b))
        .unwrap();
    assert!((cent[0] - 10.0 / 15.0).abs() < 1e-6 && (cent[1] - 5.0 / 15.0).abs() < 1e-6, "{cent:?}");
    // Both segments carry the merged name.
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM meeting_segments WHERE meeting_id = ?1 AND speaker = 'Brian'",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);
    // Plain rename (no existing target) still works.
    store::save_meeting_speakers(&conn, id, &[("Speaker 9".into(), vec![0.5, 0.5], 3)]).unwrap();
    store::rename_speaker(&conn, id, "Speaker 9", "Vivian").unwrap();
    let labels: Vec<String> = store::list_meeting_speakers(&conn, id)
        .unwrap()
        .iter()
        .map(|r| r["label"].as_str().unwrap().to_string())
        .collect();
    assert!(labels.contains(&"Vivian".to_string()) && labels.contains(&"Brian".to_string()));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn failed_calendar_meeting_does_not_block_retry() {
    use tauri_app_lib::meeting::store;

    let tmp = std::env::temp_dir().join(format!("noted_meeting_retry_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).unwrap();
    let failed = store::create_meeting(
        &conn,
        "Standup",
        Some("calendar-event"),
        None,
        "2026-07-21T15:00:00Z",
    )
    .unwrap();
    store::set_status(&conn, failed, "failed").unwrap();
    assert_eq!(store::find_meeting_by_event(&conn, "calendar-event").unwrap(), None);

    let retry = store::create_meeting(
        &conn,
        "Standup",
        Some("calendar-event"),
        None,
        "2026-07-21T15:01:00Z",
    )
    .unwrap();
    assert_eq!(
        store::find_meeting_by_event(&conn, "calendar-event").unwrap(),
        Some(retry)
    );
    let _ = std::fs::remove_file(&tmp);
}
