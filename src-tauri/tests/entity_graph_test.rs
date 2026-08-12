// "Self" graph queries: co-mention edges + entity detail (db-only, no model).
use serde_json::json;
use tauri_app_lib::db::{self, EntryInput, SaveInput};

fn note(conn: &mut rusqlite::Connection, cat: &str, date: &str) -> i64 {
    db::save_note(
        conn,
        SaveInput {
            raw_text: format!("a {cat} note"),
            source: "text".into(),
            image_path: None,
            event_date: date.into(),
            entries: vec![EntryInput {
                category: cat.into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        &format!("{date}T00:00:00Z"),
    )
    .unwrap()
}

#[test]
fn graph_nodes_and_edges() {
    let tmp = std::env::temp_dir().join(format!("noted_egraph_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    let n1 = note(&mut conn, "gym", "2026-06-01");
    let n2 = note(&mut conn, "meals", "2026-06-02");

    let gym = db::create_entity(
        &conn,
        "Planet Fitness",
        "planet fitness",
        "place",
        "[]",
        "2026-06-01",
        "now",
    )
    .unwrap();
    let squat = db::create_entity(
        &conn,
        "squat",
        "squat",
        "activity",
        "[]",
        "2026-06-01",
        "now",
    )
    .unwrap();
    let burrito = db::create_entity(
        &conn,
        "burrito",
        "burrito",
        "food",
        "[]",
        "2026-06-02",
        "now",
    )
    .unwrap();

    // gym + squat co-occur in note 1; burrito is alone in note 2.
    db::add_mention(&conn, gym, n1, None, "ctx", "2026-06-01", "now").unwrap();
    db::add_mention(&conn, squat, n1, None, "ctx", "2026-06-01", "now").unwrap();
    db::add_mention(&conn, burrito, n2, None, "ctx", "2026-06-02", "now").unwrap();

    let nodes = db::list_entities(&conn).unwrap();
    assert_eq!(nodes.len(), 3, "three entity nodes");

    let edges = db::entity_edges(&conn).unwrap();
    assert_eq!(edges.len(), 1, "only gym↔squat share a note");
    let e = &edges[0];
    assert!(
        (e.source == gym && e.target == squat) || (e.source == squat && e.target == gym),
        "edge connects gym and squat"
    );
    assert_eq!(e.weight, 1);
    assert!(
        !edges
            .iter()
            .any(|x| x.source == burrito || x.target == burrito),
        "burrito has no co-mention edges"
    );

    let det = db::entity_detail(&conn, gym, 20).unwrap();
    assert_eq!(det.len(), 1);
    assert_eq!(det[0].note_id, n1);

    let _ = std::fs::remove_file(&tmp);
}
