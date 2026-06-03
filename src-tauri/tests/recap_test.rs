// M5: recap storage + range query (db-only, deterministic) and model-backed gen.
use serde_json::json;
use tauri_app_lib::db::{self, SaveInput};
use tauri_app_lib::ollama;

fn save(conn: &mut rusqlite::Connection, cat: &str, data: serde_json::Value, date: &str) {
    db::save_note(
        conn,
        SaveInput {
            raw_text: format!("{cat} note"),
            source: "text".into(),
            image_path: None,
            event_date: date.into(),
            entries: vec![db::EntryInput { category: cat.into(), description: String::new(), data }],
        },
        &format!("{date}T00:00:00Z"),
    )
    .unwrap();
}

#[test]
fn recap_range_and_storage() {
    let tmp = std::env::temp_dir().join(format!("noted_recap_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    save(&mut conn, "gym", json!({"exercises":[{"name":"bench","weight":185}]}), "2026-06-09");
    save(&mut conn, "gym", json!({"exercises":[{"name":"squat","weight":245}]}), "2026-06-11");
    save(&mut conn, "meals", json!({"meal":"salad"}), "2026-06-20"); // outside the window

    let within = db::entries_between(&conn, "2026-06-09", "2026-06-11").unwrap();
    assert_eq!(within.len(), 2, "only the two in-range entries");
    assert!(within.iter().all(|(_, cat, _)| cat == "gym"));

    // upsert replaces an existing recap for the same period+range
    db::upsert_recap(&conn, "week", "2026-06-09", "2026-06-11", "draft", 2, "2026-06-11T00:00:00Z").unwrap();
    db::upsert_recap(&conn, "week", "2026-06-09", "2026-06-11", "you trained twice — bench 185, squat 245", 2, "2026-06-11T01:00:00Z").unwrap();
    let recaps = db::list_recaps(&conn, 10).unwrap();
    assert_eq!(recaps.len(), 1, "upsert replaced, not duplicated");
    assert!(recaps[0].content.contains("185"));
    assert_eq!(recaps[0].entry_count, 2);

    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn generates_a_grounded_recap() {
    let tmp = std::env::temp_dir().join(format!("noted_recapgen_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    save(&mut conn, "gym", json!({"exercises":[{"name":"bench","weight":185,"reps":5}]}), "2026-06-09");
    save(&mut conn, "gym", json!({"exercises":[{"name":"squat","weight":245,"reps":5}]}), "2026-06-11");
    save(&mut conn, "schedule", json!({"blocks":[{"task":"coding","hours":4}]}), "2026-06-11");

    let entries = db::entries_between(&conn, "2026-06-09", "2026-06-11").unwrap();
    let mut ctx = String::new();
    for (date, cat, data) in &entries {
        ctx.push_str(&format!("- {date} [{cat}]: {data}\n"));
    }
    let system = "You write brief, friendly recaps of the user's personal log. Write in second \
        person. Group by category. Highlight concrete numbers. Do not invent anything.";
    let user = format!("Period: 2026-06-09 to 2026-06-11.\nEntries:\n{ctx}\nWrite the recap.");
    let content = ollama::chat_text(ollama::TEXT_MODEL, system, &user).await.unwrap();
    println!("--- recap ---\n{content}");

    let lc = content.to_lowercase();
    assert!(lc.contains("bench") || content.contains("185"), "mentions the workout");
    assert!(lc.contains("squat") || content.contains("245") || lc.contains("cod"), "covers more than one thing");

    let _ = std::fs::remove_file(&tmp);
}
