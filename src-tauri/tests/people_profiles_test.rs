// People view (deterministic, no model): person entities + their curated-fact
// mentions group into profiles, dedup by normalized name, and span first/last seen.
use serde_json::json;
use tauri_app_lib::db::{self, EntryInput, SaveInput};
use tauri_app_lib::entities::normalize;

fn note(conn: &mut rusqlite::Connection, text: &str, date: &str) -> i64 {
    db::save_note(
        conn,
        SaveInput {
            raw_text: text.into(),
            source: "text".into(),
            image_path: None,
            event_date: date.into(),
            entries: vec![EntryInput {
                category: "misc".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        &format!("{date}T00:00:00Z"),
    )
    .unwrap()
}

#[test]
fn person_profiles_group_dedup_and_span() {
    let tmp = std::env::temp_dir().join(format!("noted_people_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    // normalization is the grouping contract the read path relies on
    assert_eq!(
        normalize("Mike"),
        normalize("mike"),
        "case-insensitive grouping"
    );
    assert_ne!(
        normalize("Sarah"),
        normalize("Sarah Chen"),
        "different names stay separate"
    );

    let n1 = note(
        &mut conn,
        "coffee with Mike, he just got engaged",
        "2026-06-01",
    );
    let n2 = note(
        &mut conn,
        "lunch w mike, he started a new job at Stripe",
        "2026-06-03",
    );
    let n3 = note(&mut conn, "Sarah moved to Austin", "2026-06-02");

    // "Mike" / "mike" resolve to one entity via the normalized key
    let mike = db::create_entity(
        &conn,
        "Mike",
        &normalize("Mike"),
        "person",
        "[]",
        "2026-06-01",
        "now",
    )
    .unwrap();
    assert_eq!(
        db::entity_exact(&conn, &normalize("mike"), "person").unwrap(),
        Some(mike)
    );
    db::set_entity_relationship(&conn, mike, "friend").unwrap();
    db::add_mention(&conn, mike, n1, None, "got engaged", "2026-06-01", "now").unwrap();
    db::add_mention(
        &conn,
        mike,
        n2,
        None,
        "started a new job at Stripe",
        "2026-06-03",
        "now",
    )
    .unwrap();

    let sarah = db::create_entity(
        &conn,
        "Sarah",
        &normalize("Sarah"),
        "person",
        "[]",
        "2026-06-02",
        "now",
    )
    .unwrap();
    db::add_mention(
        &conn,
        sarah,
        n3,
        None,
        "moved to Austin",
        "2026-06-02",
        "now",
    )
    .unwrap();

    let people = db::person_profiles(&conn).unwrap();
    assert_eq!(people.len(), 2, "Mike (merged) + Sarah");

    let m = people.iter().find(|p| p.id == mike).expect("Mike profile");
    assert_eq!(m.mention_count, 2, "two mentions of Mike");
    assert_eq!(m.relationship.as_deref(), Some("friend"));
    assert_eq!(m.first_seen.as_deref(), Some("2026-06-01"));
    assert_eq!(m.last_seen.as_deref(), Some("2026-06-03"));
    let facts = m
        .mentions
        .iter()
        .map(|x| x.text.clone())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(facts.contains("got engaged"), "fact present: {facts}");
    assert!(facts.contains("Stripe"), "fact present: {facts}");
    // most-recent first
    assert_eq!(m.mentions[0].date, "2026-06-03");

    let s = people
        .iter()
        .find(|p| p.id == sarah)
        .expect("Sarah profile");
    assert_eq!(s.mention_count, 1);
    assert_eq!(s.relationship, None);

    // most-mentioned first ordering
    assert_eq!(
        people[0].id, mike,
        "Mike (2 mentions) ranks before Sarah (1)"
    );

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn relationship_latest_nonempty_wins_and_blank_ignored() {
    let tmp = std::env::temp_dir().join(format!("noted_people_rel_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).unwrap();

    let tom = db::create_entity(
        &conn,
        "Tom",
        &normalize("Tom"),
        "person",
        "[]",
        "2026-06-01",
        "now",
    )
    .unwrap();
    db::set_entity_relationship(&conn, tom, "brother").unwrap();
    db::set_entity_relationship(&conn, tom, "   ").unwrap(); // blank must not clobber
    let rel: Option<String> = conn
        .query_row(
            "SELECT relationship FROM entities WHERE id = ?1",
            [tom],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rel.as_deref(), Some("brother"));

    let _ = std::fs::remove_file(&tmp);
}
