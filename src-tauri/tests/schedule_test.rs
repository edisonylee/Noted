// Schedule entries capture time periods (start/end in 24h) — real Ollama.
use tauri_app_lib::pipeline;

const CATALOG: &str = "- gym: workout logs. shape: {\"exercises\":[{\"name\":\"squat\",\"weight\":245}]}\n\
- schedule: daily time blocks. shape: {\"blocks\":[{\"task\":\"coding\",\"start\":\"09:00\",\"end\":\"11:00\",\"duration_min\":120}]}";

#[tokio::test]
async fn schedule_captures_time_periods() {
    let known = vec!["gym".to_string(), "schedule".to_string()];
    let p = pipeline::categorize(
        CATALOG,
        &known,
        "Schedule: coding from 9 to 11am, then gym from 2 to 4pm",
        "2026-06-02",
    )
    .await
    .unwrap();

    let entries = p["entries"].as_array().unwrap();
    let sched = entries
        .iter()
        .find(|e| e["category"] == "schedule")
        .or_else(|| entries.first())
        .unwrap();
    let blob = sched["data"].to_string();
    println!("schedule data: {blob}");

    assert!(blob.contains("start"), "blocks should carry start: {blob}");
    assert!(blob.contains("end"), "blocks should carry end: {blob}");
    // 2-4pm must be stored in 24-hour form
    assert!(blob.contains("14:00") || blob.contains("16:00"), "afternoon should be 24h: {blob}");
}
