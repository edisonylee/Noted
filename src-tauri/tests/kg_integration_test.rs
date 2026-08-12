// End-to-end seam between my Phase-2 write path and the Phase-3 graph/People
// reads: a note that names a person + a place should produce a co-mention edge,
// an entity detail row, and a People profile whose mention text is the curated
// fact. Deterministic (no model) — mirrors exactly what save_entry persists.
use serde_json::json;
use tauri_app_lib::db::{self, EntryInput, SaveInput};

#[test]
fn note_with_people_feeds_graph_and_people_views() {
    let tmp = std::env::temp_dir().join(format!("noted_kg_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    // The note (as save_note writes it).
    let note_id = db::save_note(
        &mut conn,
        SaveInput {
            raw_text: "lunch with Jake at Chipotle — caught up about his new job".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-06-02".into(),
            entries: vec![EntryInput {
                category: "food".into(),
                description: String::new(),
                data: json!({ "meal": "burrito bowl" }),
            }],
        },
        "2026-06-02T00:00:00Z",
    )
    .unwrap();

    // Entities exactly as save_entry persists them: the curated person fact goes
    // into the mention's context; relationship is set on the entity.
    let jake =
        db::create_entity(&conn, "Jake", "jake", "person", "[]", "2026-06-02", "now").unwrap();
    let chipotle = db::create_entity(
        &conn,
        "Chipotle",
        "chipotle",
        "place",
        "[]",
        "2026-06-02",
        "now",
    )
    .unwrap();
    db::set_entity_relationship(&conn, jake, "friend").unwrap();
    db::add_mention(
        &conn,
        jake,
        note_id,
        None,
        "started a new job",
        "2026-06-02",
        "now",
    )
    .unwrap();
    db::add_mention(
        &conn,
        chipotle,
        note_id,
        None,
        "lunch with Jake at Chipotle",
        "2026-06-02",
        "now",
    )
    .unwrap();

    // 1) Co-mention edge links the person and the place (shared note).
    let edges = db::entity_edges(&conn).unwrap();
    assert_eq!(edges.len(), 1, "one co-mention edge");
    assert_eq!(edges[0].weight, 1);
    let pair = (edges[0].source, edges[0].target);
    assert!(
        pair == (jake, chipotle) || pair == (chipotle, jake),
        "edge connects Jake & Chipotle"
    );

    // 2) entity_detail surfaces the originating note for the entity.
    let detail = db::entity_detail(&conn, jake, 10).unwrap();
    assert_eq!(detail.len(), 1);
    assert_eq!(detail[0].note_id, note_id);

    // 3) People view: the person, with relationship + the curated fact as mention
    // text; the place is excluded (not person-typed).
    let people = db::person_profiles(&conn).unwrap();
    assert_eq!(
        people.len(),
        1,
        "only the person-typed entity appears in People"
    );
    let p = &people[0];
    assert_eq!(p.name, "Jake");
    assert_eq!(p.relationship.as_deref(), Some("friend"));
    assert_eq!(p.mentions.len(), 1);
    assert_eq!(
        p.mentions[0].text, "started a new job",
        "curated fact is the mention text"
    );
    assert_eq!(p.mentions[0].note_id, note_id);

    let _ = std::fs::remove_file(&tmp);
}
