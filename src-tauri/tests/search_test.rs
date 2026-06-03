// M3 validation: embeddings + sqlite-vec KNN retrieve the right note.
// Requires Ollama running with nomic-embed-text pulled.
use serde_json::json;
use tauri_app_lib::db::{self, SaveInput};
use tauri_app_lib::ollama;

fn norm(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

#[tokio::test]
async fn embeds_and_retrieves_relevant_note() {
    let tmp = std::env::temp_dir().join(format!("noted_search_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();

    let notes = [
        ("gym", "bench press 185 for 5 reps, chest day felt strong",
         json!({"exercises":[{"name":"bench","weight":185,"reps":5}]})),
        ("meals", "had a chicken burrito bowl with extra guac for lunch",
         json!({"meal":"chicken burrito bowl"})),
        ("schedule", "spent 3 hours coding the noted app this afternoon",
         json!({"blocks":[{"task":"coding","hours":3}]})),
    ];
    for (i, (cat, text, data)) in notes.iter().enumerate() {
        db::save_note(
            &mut conn,
            SaveInput {
                raw_text: text.to_string(),
                source: "text".into(),
                image_path: None,
                event_date: "2026-06-10".into(),
                entries: vec![db::EntryInput {
                    category: cat.to_string(),
                    description: String::new(),
                    data: data.clone(),
                }],
            },
            "2026-06-10T00:00:00Z",
        )
        .unwrap();
        let embed_text = format!("{}\n{}\n{}", cat, text, data);
        let v = norm(ollama::embed(&embed_text).await.unwrap());
        db::insert_embedding(&conn, i as i64 + 1, &v).unwrap();
    }

    // A bench-press question should surface the gym note first.
    let qv = norm(ollama::embed("how much did I bench last week?").await.unwrap());
    let hits = db::search_notes(&conn, &qv, 3).unwrap();
    assert!(!hits.is_empty(), "search returns hits");
    println!("bench query top: {:?} d={}", hits[0].category, hits[0].distance);
    assert_eq!(hits[0].category.as_deref(), Some("gym"));

    // Full RAG: the text model answers grounded in the retrieved notes.
    let ctx: String = hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            format!("[{}] {} on {}: {}\n", i + 1, h.category.clone().unwrap_or_default(), h.event_date, h.raw_text)
        })
        .collect();
    let answer = ollama::chat_text(
        ollama::TEXT_MODEL,
        "Answer using ONLY these notes. Be specific with numbers and dates.",
        &format!("Notes:\n{ctx}\nQuestion: how much did I bench?"),
    )
    .await
    .unwrap();
    println!("RAG answer: {answer}");
    assert!(answer.contains("185"), "answer should cite the 185 lb bench: {answer}");

    // A food question should surface the meals note first.
    let qv2 = norm(ollama::embed("what did I eat for lunch?").await.unwrap());
    let hits2 = db::search_notes(&conn, &qv2, 3).unwrap();
    println!("food query top: {:?} d={}", hits2[0].category, hits2[0].distance);
    assert_eq!(hits2[0].category.as_deref(), Some("meals"));

    let _ = std::fs::remove_file(&tmp);
}
