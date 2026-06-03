// End-to-end M1 acceptance test through the REAL command code path:
//   pipeline::categorize (hits local Ollama) -> db::save_entry -> reuse on 2nd note.
// Requires the Ollama server running with qwen2.5:7b-instruct pulled.
use serde_json::json;
use tauri_app_lib::db::{self, SaveInput};
use tauri_app_lib::pipeline;

fn catalog_and_names(conn: &rusqlite::Connection) -> (String, Vec<String>) {
    let catalog = db::category_catalog(conn).unwrap();
    let names = db::list_categories(conn)
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    (catalog, names)
}

#[tokio::test]
async fn categorize_save_and_reuse() {
    let tmp = std::env::temp_dir().join(format!("noted_pipe_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    // --- Note 1: empty catalog -> must be a brand new category ---
    let (catalog, names) = catalog_and_names(&conn);
    let note1 = "chest day. incline db press 70s 4x10 felt strong. flat bench 185 5,5,4. cable flys 3x15 burnout";
    let p1 = pipeline::categorize(&catalog, &names, note1, "2026-06-02").await.unwrap();

    assert_eq!(p1["is_new_category"], json!(true), "first note creates a category");
    let cat1 = p1["category"].as_str().unwrap().to_string();
    assert!(!cat1.is_empty());
    assert!(p1["data"].is_object(), "data is an object");
    println!("note1 -> category '{cat1}', data: {}", p1["data"]);

    db::save_entry(
        &mut conn,
        SaveInput {
            raw_text: note1.into(),
            source: "text".into(),
            image_path: None,
            category: cat1.clone(),
            description: p1["description"].as_str().unwrap_or("").into(),
            data: p1["data"].clone(),
            event_date: p1["event_date"].as_str().unwrap_or("2026-06-02").into(),
        },
        "2026-06-02T00:00:00Z",
    )
    .unwrap();

    // --- Note 2: same domain -> should REUSE cat1 (catalog-driven), is_new=false ---
    let (catalog2, names2) = catalog_and_names(&conn);
    let note2 = "back day. pullups bodyweight 4x8. barbell row 135 3x10. lat pulldown 120 3x12 solid pump";
    let p2 = pipeline::categorize(&catalog2, &names2, note2, "2026-06-02").await.unwrap();
    println!("note2 -> category '{}', is_new {}", p2["category"], p2["is_new_category"]);

    assert_eq!(
        p2["category"].as_str().unwrap(),
        cat1,
        "second workout note should reuse the existing category, not fork a new one"
    );
    assert_eq!(p2["is_new_category"], json!(false));

    db::save_entry(
        &mut conn,
        SaveInput {
            raw_text: note2.into(),
            source: "text".into(),
            image_path: None,
            category: p2["category"].as_str().unwrap().into(),
            description: String::new(),
            data: p2["data"].clone(),
            event_date: p2["event_date"].as_str().unwrap_or("2026-06-02").into(),
        },
        "2026-06-02T01:00:00Z",
    )
    .unwrap();

    // --- One emergent category, two entries, timeline has both ---
    let cats = db::list_categories(&conn).unwrap();
    assert_eq!(cats.len(), 1, "exactly one category emerged");
    assert_eq!(cats[0].entry_count, 2);
    assert_eq!(db::list_notes(&conn).unwrap().len(), 2);

    let _ = std::fs::remove_file(&tmp);
}
