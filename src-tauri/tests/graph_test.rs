// Phase 2: entity storage (deterministic, no model). Create entities + mentions,
// resolve by exact/alias within a type, and merge a duplicate.
use serde_json::json;
use tauri_app_lib::db::{self, EntryInput, SaveInput};

#[test]
fn entity_create_mention_exact_and_merge() {
    let tmp = std::env::temp_dir().join(format!("noted_graph_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    // a note for the mentions to hang off
    let note_id = db::save_note(
        &mut conn,
        SaveInput {
            raw_text: "leg day with jake at planet fitness".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-06-02".into(),
            entries: vec![EntryInput { category: "gym".into(), description: String::new(), data: json!({}) }],
        },
        "2026-06-02T00:00:00Z",
    )
    .unwrap();

    let jake = db::create_entity(&conn, "Jake", "jake", "person", "[]", "2026-06-02", "now").unwrap();
    let pf = db::create_entity(&conn, "Planet Fitness", "planet fitness", "place", "[]", "2026-06-02", "now").unwrap();
    db::add_mention(&conn, jake, note_id, None, "with jake", "2026-06-02", "now").unwrap();
    db::add_mention(&conn, pf, note_id, None, "at planet fitness", "2026-06-02", "now").unwrap();

    // exact match is type-scoped
    assert_eq!(db::entity_exact(&conn, "jake", "person").unwrap(), Some(jake));
    assert_eq!(db::entity_exact(&conn, "jake", "place").unwrap(), None);

    // a duplicate "PF" of type place gets merged into Planet Fitness
    let pf2 = db::create_entity(&conn, "PF", "pf", "place", "[]", "2026-06-03", "now").unwrap();
    db::add_mention(&conn, pf2, note_id, None, "pf again", "2026-06-03", "now").unwrap();
    db::merge_entities(&mut conn, pf, pf2).unwrap();

    // "PF" now resolves to Planet Fitness via the folded-in alias
    assert_eq!(db::entity_exact(&conn, "pf", "place").unwrap(), Some(pf), "PF alias -> Planet Fitness");

    let count: i64 = conn
        .query_row("SELECT mention_count FROM entities WHERE id = ?1", [pf], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2, "both mentions belong to the kept entity after merge");

    let remaining: i64 = conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0)).unwrap();
    assert_eq!(remaining, 2, "jake + planet fitness; the PF duplicate is gone");

    let _ = std::fs::remove_file(&tmp);
}
