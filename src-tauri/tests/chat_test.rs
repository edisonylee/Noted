// Conversational Q&A: recency-aware retrieval answers date/"last X" questions,
// and history enables follow-ups. Requires Ollama (qwen2.5 + nomic).
use std::collections::HashSet;

use serde_json::json;
use tauri_app_lib::db::{self, SaveInput};
use tauri_app_lib::{ollama, pipeline};

const TODAY: &str = "2026-06-02";

fn norm(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

async fn save_and_embed(conn: &mut rusqlite::Connection, id: i64, cat: &str, raw: &str, data: serde_json::Value, date: &str) {
    db::save_note(
        conn,
        SaveInput {
            raw_text: raw.into(),
            source: "text".into(),
            image_path: None,
            event_date: date.into(),
            entries: vec![db::EntryInput { category: cat.into(), description: String::new(), data: data.clone() }],
        },
        &format!("{date}T00:00:00Z"),
    )
    .unwrap();
    let v = norm(ollama::embed(&format!("{cat}\n{raw}\n{data}")).await.unwrap());
    db::insert_embedding(conn, id, &v).unwrap();
}

// Mirror of the `chat` command's retrieval + answer.
async fn answer(conn: &rusqlite::Connection, question: &str, history: &[(&str, &str)]) -> String {
    let qv = norm(ollama::embed(question).await.unwrap());
    let recent = db::recent_entries(conn, 15).unwrap();
    let semantic = db::search_notes(conn, &qv, 8).unwrap();
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for h in recent.into_iter().chain(semantic.into_iter()) {
        if seen.insert(h.note_id) {
            hits.push(h);
        }
    }
    let mut messages = vec![json!({"role":"system","content": pipeline::qa_system(TODAY)})];
    for (r, c) in history {
        messages.push(json!({"role": r, "content": c}));
    }
    messages.push(json!({"role":"user","content": format!("Entries:\n{}\nQuestion: {question}", pipeline::qa_context(&hits))}));
    ollama::chat_messages(ollama::TEXT_MODEL, messages, 0.2).await.unwrap()
}

#[tokio::test]
async fn answers_recency_and_followups() {
    let tmp = std::env::temp_dir().join(format!("noted_chat_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    save_and_embed(&mut conn, 1, "gym", "chest day bench press", json!({"exercises":[{"name":"bench","weight":185}]}), "2026-05-30").await;
    save_and_embed(&mut conn, 2, "gym", "leg day squats", json!({"exercises":[{"name":"squat","weight":245,"reps":5,"sets":5}]}), "2026-06-01").await;
    save_and_embed(&mut conn, 3, "schedule", "spent the day coding and in classes", json!({"blocks":[{"task":"coding","hours":3},{"task":"classes","hours":2}]}), "2026-06-02").await;

    let last = answer(&conn, "what was my last workout?", &[]).await;
    println!("last workout -> {last}");
    let lc = last.to_lowercase();
    assert!(lc.contains("squat") || last.contains("245"), "should be the 6/1 squat: {last}");

    let yest = answer(&conn, "what did I do yesterday?", &[]).await;
    println!("yesterday -> {yest}");
    let yl = yest.to_lowercase();
    assert!(yl.contains("squat") || yl.contains("leg") || yest.contains("245"), "yesterday=6/1 leg day: {yest}");

    // follow-up relies on conversation history
    let followup = answer(
        &conn,
        "how heavy was it?",
        &[("user", "what was my last workout?"), ("assistant", &last)],
    )
    .await;
    println!("followup -> {followup}");
    assert!(followup.contains("245"), "follow-up should cite 245: {followup}");

    let _ = std::fs::remove_file(&tmp);
}
