// Live Assist A0: the meeting_assist prompt assembly + a real local-model
// answer over a real meeting transcript. Mirrors the command in lib.rs (which
// is AppHandle-bound) the way chat_test mirrors `chat`. Requires Ollama.
//
// The default test builds its own tiny meeting; the ignored variant points at
// a real DB copy:
//   NOTED_DB=…/copy.db MEETING_ID=8 QUESTION="what did Brian say?" \
//   cargo test --test assist_test real_meeting -- --ignored --nocapture

use serde_json::json;
use tauri_app_lib::{meeting::store, ollama};

async fn assist(conn: &rusqlite::Connection, id: i64, question: &str) -> String {
    let meeting = store::get_meeting(conn, id).expect("meeting");
    let title = meeting["title"].as_str().unwrap_or("Meeting").to_string();
    let notes = meeting["raw_notes"].as_str().unwrap_or("").to_string();
    let mut lines: Vec<String> = Vec::new();
    if let Some(segs) = meeting["segments"].as_array() {
        for s in segs {
            let t0 = s["t0_ms"].as_i64().unwrap_or(0);
            let who = if s["channel"].as_str() == Some("me") {
                "Me".to_string()
            } else {
                s["speaker"].as_str().unwrap_or("Them").to_string()
            };
            lines.push(format!(
                "[{:02}:{:02}] {who}: {}",
                t0 / 60_000,
                (t0 / 1_000) % 60,
                s["text"].as_str().unwrap_or("")
            ));
        }
    }
    let mut budget = 8_000usize;
    let mut tail: Vec<&String> = Vec::new();
    for l in lines.iter().rev() {
        if budget < l.len() {
            break;
        }
        budget -= l.len();
        tail.push(l);
    }
    tail.reverse();
    let transcript = tail.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
    let system = "You are the user's live meeting copilot. You see the transcript of the \
meeting so far — 'Me' is the user; named speakers or 'Them' are the other participants — plus \
the user's own typed notes. Answer the question from that context only. Be concise and \
specific; quote who said something when it matters; give ready-to-say wording when the user \
asks how to respond. If the transcript doesn't contain the answer, say so plainly.";
    let notes_block = if notes.trim().is_empty() {
        String::new()
    } else {
        format!("\nMy notes:\n{notes}\n")
    };
    let user =
        format!("Meeting: {title}\nTranscript so far:\n{transcript}\n{notes_block}\nQuestion: {question}");
    ollama::chat_messages(
        &ollama::text_model(),
        vec![
            json!({ "role": "system", "content": system }),
            json!({ "role": "user", "content": user }),
        ],
        0.2,
    )
    .await
    .expect("ollama")
}

#[tokio::test]
async fn answers_from_the_transcript() {
    let tmp = std::env::temp_dir().join(format!("noted_assist_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = tauri_app_lib::db::init(&tmp).unwrap();
    let id = store::create_meeting(&conn, "Roadmap sync", None, None, "2026-07-17T16:00:00Z").unwrap();
    for (ch, t0, text, speaker) in [
        ("me", 1_000, "Morning! Where did we land on the launch date?", None),
        ("them", 6_000, "We're moving the launch to September 3rd because QA needs two more weeks.", Some("Brian")),
        ("me", 14_000, "Okay. And the pricing page?", None),
        ("them", 19_000, "Jasmine owns the pricing page copy, due Friday.", Some("Brian")),
    ] {
        let sid = store::insert_segment(&conn, id, ch, t0, t0 + 4_000, text).unwrap();
        if let Some(name) = speaker {
            store::set_segment_speakers(&conn, &[(sid, name.to_string())]).unwrap();
        }
    }
    let a = assist(&conn, id, "when is the launch now and why?").await;
    println!("assist -> {a}");
    let al = a.to_lowercase();
    assert!(al.contains("september") || a.contains("3"), "should cite the new date: {a}");
    assert!(al.contains("qa") || al.contains("two more weeks"), "should cite the reason: {a}");
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
#[ignore]
async fn real_meeting() {
    let db = std::env::var("NOTED_DB").expect("NOTED_DB");
    let id: i64 = std::env::var("MEETING_ID").expect("MEETING_ID").parse().unwrap();
    let q = std::env::var("QUESTION").unwrap_or_else(|_| "what was this meeting about?".into());
    let conn = rusqlite::Connection::open(&db).expect("db");
    let a = assist(&conn, id, &q).await;
    println!("assist({id}) -> {a}");
    assert!(a.len() > 20);
}
