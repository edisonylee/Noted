// Categorization robustness (the schema + temp 0.3 + feelings fix): diverse notes
// route to sensible categories, gym is still reused, and feelings get captured.
use tauri_app_lib::pipeline;

const CATALOG: &str = "- gym: workout logs. shape: {\"exercises\":[{\"name\":\"squat\",\"weight\":245}]}\n\
- schedule: daily time blocks. shape: {\"blocks\":[{\"task\":\"coding\",\"hours\":3}]}";

fn known() -> Vec<String> {
    vec!["gym".into(), "schedule".into()]
}

#[tokio::test]
async fn food_note_makes_a_new_category() {
    let p = pipeline::categorize(
        CATALOG,
        &known(),
        "had a chipotle bowl for lunch — chicken, extra rice and guac, felt good after",
        "2026-06-02",
    )
    .await
    .unwrap();
    let e = &p["entries"][0];
    let cat = e["category"].as_str().unwrap();
    println!("food -> {cat} | data {}", e["data"]);
    assert_ne!(cat, "gym", "a meal must not be filed under gym");
    assert_ne!(cat, "schedule");
    assert_eq!(e["is_new_category"], serde_json::json!(true));
}

#[tokio::test]
async fn gym_note_reused_with_feeling_captured() {
    let p = pipeline::categorize(
        CATALOG,
        &known(),
        "leg day, squats 245 for 5x5, felt strong and energized",
        "2026-06-02",
    )
    .await
    .unwrap();
    let e = &p["entries"][0];
    println!("gym -> {} | data {}", e["category"], e["data"]);
    assert_eq!(e["category"], serde_json::json!("gym"));
    assert_eq!(e["is_new_category"], serde_json::json!(false));
    let blob = e["data"].to_string().to_lowercase();
    assert!(
        blob.contains("mood") || blob.contains("strong") || blob.contains("energ"),
        "the feeling should be captured in data: {blob}"
    );
}

#[tokio::test]
async fn feeling_note_is_not_gym_or_schedule() {
    let p = pipeline::categorize(
        CATALOG,
        &known(),
        "felt really anxious and unfocused all day, kept procrastinating and couldn't settle",
        "2026-06-02",
    )
    .await
    .unwrap();
    let e = &p["entries"][0];
    let cat = e["category"].as_str().unwrap();
    let blob = format!("{}{}", cat, e["data"]).to_lowercase();
    println!("feeling -> {cat} | data {}", e["data"]);
    assert_ne!(cat, "gym");
    assert_ne!(cat, "schedule");
    assert!(blob.contains("mood") || blob.contains("anx") || blob.contains("feel"), "mood captured: {blob}");
}
