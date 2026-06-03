// M2.5: canonical event-date resolution on the text path (real Ollama).
// A note with an explicit date keeps it; a dateless note defaults to today;
// a relative date resolves against today.
use tauri_app_lib::pipeline::{self, extract_date_from_text, resolve_date, snap_category};

const TODAY: &str = "2026-06-15";

#[test]
fn resolve_date_defaults_and_validates() {
    // pure unit checks, no model
    assert_eq!(resolve_date(Some("2026-06-02"), TODAY), ("2026-06-02".into(), true));
    assert_eq!(resolve_date(None, TODAY), (TODAY.into(), false));
    assert_eq!(resolve_date(Some(""), TODAY), (TODAY.into(), false));
    assert_eq!(resolve_date(Some("null"), TODAY), (TODAY.into(), false));
    assert_eq!(resolve_date(Some("not a date"), TODAY), (TODAY.into(), false));
}

#[test]
fn snaps_category_to_existing() {
    let known = vec!["gym".to_string(), "schedule".to_string()];
    // model echoed the description -> snaps back to gym (reuse, not duplicate)
    assert_eq!(snap_category("gym: workout logs", &known), "gym");
    assert_eq!(snap_category("Gym", &known), "gym");
    assert_eq!(snap_category("schedule - daily blocks", &known), "schedule");
    assert_eq!(snap_category("gyms", &known), "gym"); // plural tolerance
    // genuinely unrelated -> stays new
    assert_eq!(snap_category("movies", &known), "movies");
    assert_eq!(snap_category("books: what I read", &known), "books");
}

#[test]
fn scrapes_dates_from_text() {
    // the exact case from the gym-log photo
    assert_eq!(
        extract_date_from_text("LEG DAY 6/2\nsquat 225 x5 x5 x4", TODAY).as_deref(),
        Some("2026-06-02")
    );
    assert_eq!(extract_date_from_text("ran on 2026-06-09", TODAY).as_deref(), Some("2026-06-09"));
    assert_eq!(extract_date_from_text("6/2/26 workout", TODAY).as_deref(), Some("2026-06-02"));
    // no year + lands in the future -> assume last year
    assert_eq!(extract_date_from_text("12/30 party", "2026-01-05").as_deref(), Some("2025-12-30"));
    assert_eq!(extract_date_from_text("no date here, just squats", TODAY), None);
}

#[tokio::test]
async fn extracts_explicit_date() {
    let p = pipeline::categorize("(none yet)", &[], "ran 5k on 2026-06-09, felt good, 26 min", TODAY)
        .await
        .unwrap();
    println!("explicit -> {} ({})", p["event_date"], p["date_was_extracted"]);
    assert_eq!(p["event_date"], serde_json::json!("2026-06-09"));
    assert_eq!(p["date_was_extracted"], serde_json::json!(true));
}

#[tokio::test]
async fn dateless_defaults_to_today() {
    let p = pipeline::categorize("(none yet)", &[], "had oatmeal and eggs for breakfast", TODAY)
        .await
        .unwrap();
    println!("dateless -> {} ({})", p["event_date"], p["date_was_extracted"]);
    assert_eq!(p["event_date"], serde_json::json!(TODAY), "no date in note -> today");
    assert_eq!(p["date_was_extracted"], serde_json::json!(false));
}

#[tokio::test]
async fn relative_date_resolves() {
    let p = pipeline::categorize("(none yet)", &[], "yesterday I biked 12 miles", TODAY)
        .await
        .unwrap();
    println!("relative -> {} ({})", p["event_date"], p["date_was_extracted"]);
    // "yesterday" relative to 2026-06-15 = 2026-06-14
    assert_eq!(p["event_date"], serde_json::json!("2026-06-14"));
}
