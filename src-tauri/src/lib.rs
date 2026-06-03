pub mod analytics;
pub mod db;
pub mod ollama;
pub mod phone;
pub mod pipeline;
pub mod voice;

use db::{Db, SaveInput};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::Manager;

/// Local calendar date (YYYY-MM-DD) — the user's "today", not UTC.
fn today_local() -> String {
    chrono::Local::now().date_naive().to_string()
}

/// L2-normalize a vector so the vec0 default L2 distance ranks like cosine.
fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// M0 health check: which models are pulled, plus a sqlite-vec smoke test.
#[tauri::command]
async fn health(state: tauri::State<'_, Db>) -> Result<Value, String> {
    let tags = ollama::tags().await.map_err(|e| e.to_string())?;
    let models: Vec<String> = tags
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let vec_version: String = {
        let conn = state.0.lock().unwrap();
        conn.query_row("SELECT vec_version()", [], |r| r.get(0))
            .map_err(|e| e.to_string())?
    };

    Ok(json!({ "models": models, "vec_version": vec_version }))
}

/// Take a messy note, return a *proposal* { category, is_new_category, description, data }.
/// Nothing is written — the UI reviews this before save_entry.
#[tauri::command]
async fn categorize_note(
    state: tauri::State<'_, Db>,
    text: String,
) -> Result<Value, String> {
    // Read the current catalog + known names, then drop the lock before any await.
    let (catalog, known_names) = {
        let conn = state.0.lock().unwrap();
        let catalog = db::category_catalog(&conn).map_err(|e| e.to_string())?;
        let names: Vec<String> = db::list_categories(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| c.name)
            .collect();
        (catalog, names)
    };

    pipeline::categorize(&catalog, &known_names, &text, &today_local())
        .await
        .map_err(|e| e.to_string())
}

/// One-shot vision path: a photo (base64, no data: prefix) is transcribed +
/// categorized + extracted by the local vision model. Returns a proposal that
/// also includes `raw_text` (the transcription) for review.
#[tauri::command]
async fn categorize_photo(
    state: tauri::State<'_, Db>,
    image_base64: String,
) -> Result<Value, String> {
    let (catalog, known_names) = {
        let conn = state.0.lock().unwrap();
        let catalog = db::category_catalog(&conn).map_err(|e| e.to_string())?;
        let names: Vec<String> = db::list_categories(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| c.name)
            .collect();
        (catalog, names)
    };

    pipeline::categorize_photo(&catalog, &known_names, &image_base64, &today_local())
        .await
        .map_err(|e| e.to_string())
}

/// Persist an uploaded image (base64) under app_data/images and return its path,
/// so save_entry can reference it. `ext` is e.g. "png" | "jpg".
#[tauri::command]
async fn save_image(
    app: tauri::AppHandle,
    image_base64: String,
    ext: String,
) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image_base64.as_bytes())
        .map_err(|e| e.to_string())?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("images");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe_ext = if ext.chars().all(|c| c.is_ascii_alphanumeric()) && !ext.is_empty() {
        ext
    } else {
        "png".to_string()
    };
    let name = format!("{}.{}", chrono::Utc::now().timestamp_micros(), safe_ext);
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[derive(Deserialize)]
struct SaveArgs {
    raw_text: String,
    #[serde(default = "default_source")]
    source: String,
    image_path: Option<String>,
    category: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    event_date: String,
    data: Value,
}

fn default_source() -> String {
    "text".to_string()
}

/// Commit a reviewed proposal: writes note + entry, creates/evolves the category.
#[tauri::command]
async fn save_entry(state: tauri::State<'_, Db>, args: SaveArgs) -> Result<i64, String> {
    let now = chrono::Utc::now().to_rfc3339();
    // Trust an explicit reviewed date; fall back to today if the UI sent none.
    let event_date = {
        let d = args.event_date.trim();
        if d.is_empty() { today_local() } else { d.to_string() }
    };
    // Compose the text we'll embed for semantic search (category + note + data).
    let embed_text = format!("{}\n{}\n{}", args.category, args.raw_text, args.data);

    let note_id = {
        let mut conn = state.0.lock().unwrap();
        db::save_entry(
            &mut conn,
            SaveInput {
                raw_text: args.raw_text,
                source: args.source,
                image_path: args.image_path,
                category: args.category.trim().to_lowercase(),
                description: args.description,
                data: args.data,
                event_date,
            },
            &now,
        )
        .map_err(|e| e.to_string())?
    };

    // Best-effort: index for "ask my notes". A failed embed never fails the save
    // (the note is still recoverable via reindex()).
    if let Ok(v) = ollama::embed(&embed_text).await {
        let v = normalize(v);
        let conn = state.0.lock().unwrap();
        let _ = db::insert_embedding(&conn, note_id, &v);
    }
    Ok(note_id)
}

#[tauri::command]
async fn list_notes(state: tauri::State<'_, Db>) -> Result<Value, String> {
    let conn = state.0.lock().unwrap();
    let notes = db::list_notes(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(notes).map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_categories(state: tauri::State<'_, Db>) -> Result<Value, String> {
    let conn = state.0.lock().unwrap();
    let cats = db::list_categories(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(cats).map_err(|e| e.to_string())
}

/// Generate (and store) a recap for "day" (today) or "week" (trailing 7 days),
/// grounded in the entries within that range.
#[tauri::command]
async fn generate_recap(state: tauri::State<'_, Db>, period: String) -> Result<Value, String> {
    let today = chrono::Local::now().date_naive();
    let (start, end) = match period.as_str() {
        "week" => ((today - chrono::Duration::days(6)).to_string(), today.to_string()),
        _ => (today.to_string(), today.to_string()),
    };

    let entries = {
        let conn = state.0.lock().unwrap();
        db::entries_between(&conn, &start, &end).map_err(|e| e.to_string())?
    };
    let entry_count = entries.len() as i64;

    if entries.is_empty() {
        let label = if period == "week" { "this week" } else { "today" };
        return Ok(json!({
            "content": format!("Nothing logged {label} yet."),
            "period": period, "period_start": start, "period_end": end, "entry_count": 0,
        }));
    }

    let mut ctx = String::new();
    for (date, cat, data) in &entries {
        ctx.push_str(&format!("- {date} [{cat}]: {}\n", data));
    }
    let span = if period == "week" {
        format!("the week of {start} to {end}")
    } else {
        format!("{end}")
    };
    let system = "You write brief, friendly recaps of the user's personal log. Write in second \
        person. Group by category. Highlight concrete numbers (weights, hours, counts) and \
        anything notable like personal records. Keep it tight — a few short sentences or bullets. \
        Do not invent anything not in the entries.";
    let user = format!("Period: {span}.\nEntries:\n{ctx}\nWrite the recap.");
    let content = ollama::chat_text(ollama::TEXT_MODEL, system, &user)
        .await
        .map_err(|e| e.to_string())?;
    let content = content.trim().to_string();

    {
        let conn = state.0.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        db::upsert_recap(&conn, &period, &start, &end, &content, entry_count, &now)
            .map_err(|e| e.to_string())?;
    }

    Ok(json!({
        "content": content, "period": period,
        "period_start": start, "period_end": end, "entry_count": entry_count,
    }))
}

#[tauri::command]
async fn list_recaps(state: tauri::State<'_, Db>) -> Result<Value, String> {
    let conn = state.0.lock().unwrap();
    let recaps = db::list_recaps(&conn, 20).map_err(|e| e.to_string())?;
    serde_json::to_value(recaps).map_err(|e| e.to_string())
}

/// Discover charts for a category from its (emergent) data shape.
#[tauri::command]
async fn category_trends(state: tauri::State<'_, Db>, category: String) -> Result<Value, String> {
    let entries = {
        let conn = state.0.lock().unwrap();
        db::category_entries(&conn, &category).map_err(|e| e.to_string())?
    };
    let trends = analytics::build_trends(&entries);
    serde_json::to_value(trends).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct ChatMsg {
    role: String,
    content: String,
}

/// Conversational Q&A over the user's log. Hybrid retrieval (recent-by-date +
/// semantic) so date/recency questions work, plus prior `history` for follow-ups.
#[tauri::command]
async fn chat(
    state: tauri::State<'_, Db>,
    question: String,
    history: Vec<ChatMsg>,
) -> Result<Value, String> {
    use std::collections::HashSet;
    if question.trim().is_empty() {
        return Err("empty question".into());
    }
    let qv = normalize(ollama::embed(&question).await.map_err(|e| e.to_string())?);

    // recent-by-date ∪ semantic, deduped by note_id (recent first).
    let hits = {
        let conn = state.0.lock().unwrap();
        let recent = db::recent_entries(&conn, 15).map_err(|e| e.to_string())?;
        let semantic = db::search_notes(&conn, &qv, 8).map_err(|e| e.to_string())?;
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for h in recent.into_iter().chain(semantic.into_iter()) {
            if seen.insert(h.note_id) {
                hits.push(h);
            }
        }
        hits
    };
    if hits.is_empty() {
        return Ok(json!({
            "answer": "I don't have any notes yet — log a few and ask again.",
            "sources": [],
        }));
    }

    let context = pipeline::qa_context(&hits);
    let mut messages = vec![json!({ "role": "system", "content": pipeline::qa_system(&today_local()) })];
    for m in &history {
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        messages.push(json!({ "role": role, "content": m.content }));
    }
    messages.push(json!({
        "role": "user",
        "content": format!("Entries:\n{context}\nQuestion: {question}")
    }));

    let answer = ollama::chat_messages(ollama::TEXT_MODEL, messages, 0.2)
        .await
        .map_err(|e| e.to_string())?;

    let sources: Vec<Value> = hits
        .iter()
        .take(6)
        .map(|h| {
            json!({
                "note_id": h.note_id,
                "category": h.category,
                "event_date": h.event_date,
                "snippet": h.raw_text.chars().take(140).collect::<String>(),
            })
        })
        .collect();

    Ok(json!({ "answer": answer.trim(), "sources": sources }))
}

/// Speak text aloud via macOS `say` (free, on-device). Cancels any prior speech.
#[tauri::command]
fn speak(text: String) -> Result<(), String> {
    let _ = std::process::Command::new("pkill").args(["-x", "say"]).status();
    if text.trim().is_empty() {
        return Ok(());
    }
    std::process::Command::new("say")
        .arg(text)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn stop_speaking() {
    let _ = std::process::Command::new("pkill").args(["-x", "say"]).status();
}

/// Backfill embeddings for any notes that don't have one (e.g. saved while the
/// embed model was unavailable). Returns how many were indexed.
#[tauri::command]
async fn reindex(state: tauri::State<'_, Db>) -> Result<i64, String> {
    let todo = {
        let conn = state.0.lock().unwrap();
        db::notes_missing_embeddings(&conn).map_err(|e| e.to_string())?
    };
    let mut n = 0;
    for (id, text) in todo {
        if let Ok(v) = ollama::embed(&text).await {
            let v = normalize(v);
            let conn = state.0.lock().unwrap();
            if db::insert_embedding(&conn, id, &v).is_ok() {
                n += 1;
            }
        }
    }
    Ok(n)
}

/// One-click backup: checkpoint the WAL and copy the DB to a timestamped file
/// on the Desktop. Returns the destination path.
#[tauri::command]
async fn export_db(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let db_path = dir.join("noted.db");
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let mut dest_dir = std::path::PathBuf::from(&home).join("Desktop");
    if !dest_dir.exists() {
        dest_dir = std::path::PathBuf::from(&home);
    }
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest = dest_dir.join(format!("noted-backup-{ts}.db"));
    std::fs::copy(&db_path, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

/// LAN URL + token for the phone capture page.
#[tauri::command]
fn phone_info(state: tauri::State<'_, phone::PhoneState>) -> Value {
    json!({ "url": state.url, "token": state.token, "port": state.port })
}

/// Read an inbox image (from a phone upload) as base64 for the vision pipeline.
#[tauri::command]
async fn read_inbox_image(path: String) -> Result<Value, String> {
    use base64::Engine;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let ext = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg")
        .to_string();
    Ok(json!({ "base64": b64, "ext": ext }))
}

// ---------------------------------------------------------------------------
// Voice (local speech-to-text via whisper.cpp)
// ---------------------------------------------------------------------------

const VOICE_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";

fn voice_model_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("ggml-base.en.bin"))
}

#[tauri::command]
fn voice_status(app: tauri::AppHandle) -> Value {
    let ready = voice_model_path(&app).map(|p| p.exists()).unwrap_or(false);
    json!({ "ready": ready })
}

/// Download the whisper model (~148MB) once, into app data.
#[tauri::command]
async fn download_voice_model(app: tauri::AppHandle) -> Result<bool, String> {
    let path = voice_model_path(&app)?;
    if path.exists() {
        return Ok(true);
    }
    let bytes = reqwest::get(VOICE_MODEL_URL)
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Transcribe audio sent from the UI as base64 of little-endian f32 PCM samples.
#[tauri::command]
async fn transcribe(
    app: tauri::AppHandle,
    audio_b64: String,
    sample_rate: u32,
) -> Result<String, String> {
    use base64::Engine;
    let model = voice_model_path(&app)?;
    if !model.exists() {
        return Err("voice model not downloaded".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_b64.as_bytes())
        .map_err(|e| e.to_string())?;
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let samples = voice::resample_to_16k(&samples, sample_rate);

    // whisper is CPU/Metal-bound and blocking; run off the async runtime.
    tauri::async_runtime::spawn_blocking(move || voice::transcribe(&model, &samples))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// App setup
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::init(&dir.join("noted.db"))?;
            app.manage(Db(Mutex::new(conn)));

            // Phone capture: tiny LAN upload server gated by a random token.
            let inbox = dir.join("inbox");
            std::fs::create_dir_all(&inbox)?;
            let token = format!("{:016x}", rand::random::<u64>());
            let ip = local_ip_address::local_ip()
                .map(|i| i.to_string())
                .unwrap_or_else(|_| "localhost".to_string());
            if let Some((server, port)) = phone::bind(8787) {
                let url = format!("http://{ip}:{port}/?t={token}");
                println!("[noted] phone capture ready: {url}");
                app.manage(phone::PhoneState { url, token: token.clone(), port });
                phone::serve(server, app.handle().clone(), inbox, token);
            } else {
                app.manage(phone::PhoneState {
                    url: String::new(),
                    token,
                    port: 0,
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            health,
            categorize_note,
            categorize_photo,
            save_image,
            save_entry,
            list_notes,
            list_categories,
            chat,
            speak,
            stop_speaking,
            reindex,
            category_trends,
            generate_recap,
            list_recaps,
            export_db,
            phone_info,
            read_inbox_image,
            voice_status,
            download_voice_model,
            transcribe,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
