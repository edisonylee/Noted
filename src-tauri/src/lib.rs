pub mod analytics;
pub mod db;
pub mod entities;
pub mod ollama;
pub mod phone;
pub mod pipeline;
pub mod provider;
pub mod voice;

use db::{Db, SaveInput};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

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
async fn health(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
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
    app: tauri::AppHandle,
    text: String,
) -> Result<Value, String> {
    let state = app.state::<Db>();
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
    app: tauri::AppHandle,
    image_base64: String,
) -> Result<Value, String> {
    let state = app.state::<Db>();
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
struct EntryArg {
    category: String,
    #[serde(default)]
    description: String,
    data: Value,
}

#[derive(Deserialize)]
struct EntityArg {
    name: String,
    #[serde(rename = "type")]
    etype: String,
    #[serde(default)]
    fact: Option<String>,
    #[serde(default)]
    relationship: Option<String>,
}

#[derive(Deserialize)]
struct SaveArgs {
    raw_text: String,
    #[serde(default = "default_source")]
    source: String,
    image_path: Option<String>,
    #[serde(default)]
    event_date: String,
    entries: Vec<EntryArg>,
    #[serde(default)]
    entities: Vec<EntityArg>,
}

fn default_source() -> String {
    "text".to_string()
}

/// Commit a reviewed note: writes the note + one entry per category, creating/
/// evolving each category. One embedding per note covers all its entries.
#[tauri::command]
async fn save_entry(app: tauri::AppHandle, args: SaveArgs) -> Result<i64, String> {
    let state = app.state::<Db>();
    let now = chrono::Utc::now().to_rfc3339();
    // Trust an explicit reviewed date; fall back to today if the UI sent none.
    let event_date = {
        let d = args.event_date.trim();
        if d.is_empty() { today_local() } else { d.to_string() }
    };
    if args.entries.is_empty() {
        return Err("no entries to save".into());
    }
    // Compose the text we'll embed for semantic search: the note plus every
    // category name and every entry's data.
    let mut embed_text = args.raw_text.clone();
    for e in &args.entries {
        embed_text.push('\n');
        embed_text.push_str(&e.category);
        embed_text.push('\n');
        embed_text.push_str(&e.data.to_string());
    }

    let entries: Vec<db::EntryInput> = args
        .entries
        .into_iter()
        .map(|e| db::EntryInput {
            category: e.category.trim().to_lowercase(),
            description: e.description,
            data: e.data,
        })
        .collect();

    // Keep a copy of the note text for entity-mention context (raw_text is moved).
    let raw = args.raw_text.clone();

    let note_id = {
        let mut conn = state.0.lock().unwrap();
        db::save_note(
            &mut conn,
            SaveInput {
                raw_text: args.raw_text,
                source: args.source,
                image_path: args.image_path,
                event_date: event_date.clone(),
                entries,
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

    // Persist knowledge-graph entities (best effort — never fails the save). Embed
    // each off-lock, then resolve (exact/alias or near-neighbor) + create + link
    // a mention under the lock.
    if !args.entities.is_empty() {
        // Carry the curated person details (fact, relationship) alongside each
        // embedding so they can be stored once the entity is resolved.
        let mut embedded: Vec<(String, String, Vec<f32>, Option<String>, Option<String>)> = Vec::new();
        for e in &args.entities {
            let name = e.name.trim();
            let etype = e.etype.trim().to_lowercase();
            if name.is_empty() || etype.is_empty() {
                continue;
            }
            if let Ok(v) = entities::embed_entity(name, &etype).await {
                let fact = e.fact.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
                let rel = e.relationship.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
                embedded.push((name.to_string(), etype, v, fact, rel));
            }
        }
        let conn = state.0.lock().unwrap();
        let snippet: String = raw.chars().take(200).collect();
        for (name, etype, emb, fact, rel) in &embedded {
            let id = match entities::resolve_with_embedding(&conn, name, etype, emb) {
                Ok(entities::Resolution::Exact(id)) | Ok(entities::Resolution::Suggest(id, _)) => id,
                Ok(entities::Resolution::New) => {
                    let norm = entities::normalize(name);
                    match db::create_entity(&conn, name, &norm, etype, "[]", &event_date, &now) {
                        Ok(id) => {
                            let _ = db::insert_entity_embedding(&conn, id, emb);
                            id
                        }
                        Err(_) => continue,
                    }
                }
                Err(_) => continue,
            };
            if let Some(r) = rel {
                let _ = db::set_entity_relationship(&conn, id, r);
            }
            // Prefer the curated per-person fact as the mention context; fall back
            // to the note snippet when none was extracted.
            let context = fact.as_deref().unwrap_or(snippet.as_str());
            let _ = db::add_mention(&conn, id, note_id, None, context, &event_date, &now);
        }
    }

    Ok(note_id)
}

/// Instant capture: queue the raw note/photo and return its id immediately (no
/// LLM). A background worker categorizes it and writes a real note later. This
/// is the phone's capture path — it never blocks on the local model.
#[tauri::command]
async fn quick_capture(
    app: tauri::AppHandle,
    raw_text: String,
    source: Option<String>,
    image_path: Option<String>,
    event_date: Option<String>,
) -> Result<i64, String> {
    let source = source.unwrap_or_else(default_source);
    if raw_text.trim().is_empty() && image_path.is_none() {
        return Err("empty capture".into());
    }
    let id = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        db::insert_pending(
            &conn,
            &raw_text,
            &source,
            image_path.as_deref(),
            event_date.as_deref().filter(|s| !s.trim().is_empty()),
            &chrono::Utc::now().to_rfc3339(),
        )
        .map_err(|e| e.to_string())?
    };
    let _ = app.emit("capture-queued", json!({ "id": id }));
    Ok(id)
}

#[tauri::command]
async fn list_notes(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let notes = db::list_notes(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(notes).map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_categories(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let cats = db::list_categories(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(cats).map_err(|e| e.to_string())
}

/// Generate (and store) a recap for "day" (today) or "week" (trailing 7 days),
/// grounded in the entries within that range.
#[tauri::command]
async fn generate_recap(app: tauri::AppHandle, period: String) -> Result<Value, String> {
    let state = app.state::<Db>();
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

const RECAP_SYSTEM: &str = "You write brief, friendly recaps of the user's personal log. Write in \
    second person. Group by category. Highlight concrete numbers (weights, hours, counts) and \
    anything notable like personal records. Keep it tight — a few short sentences or bullets. \
    Do not invent anything not in the entries.";

/// Generate + store a recap for an explicit [start,end] range. Skips if one
/// already exists (unless `force`) or the range has no entries. Emits
/// "recap-generated" on success so the UI refreshes. Returns whether it wrote one.
async fn recap_period(
    app: &tauri::AppHandle,
    period: &str,
    start: &str,
    end: &str,
    force: bool,
) -> Result<bool, String> {
    let state = app.state::<Db>();
    if !force {
        let exists = {
            let conn = state.0.lock().unwrap();
            db::recap_exists(&conn, period, start, end).map_err(|e| e.to_string())?
        };
        if exists {
            return Ok(false);
        }
    }
    let entries = {
        let conn = state.0.lock().unwrap();
        db::entries_between(&conn, start, end).map_err(|e| e.to_string())?
    };
    if entries.is_empty() {
        return Ok(false);
    }
    let entry_count = entries.len() as i64;

    let mut ctx = String::new();
    for (date, cat, data) in &entries {
        ctx.push_str(&format!("- {date} [{cat}]: {}\n", data));
    }
    let span = if period == "week" {
        format!("the week of {start} to {end}")
    } else {
        format!("{end}")
    };
    let user = format!("Period: {span}.\nEntries:\n{ctx}\nWrite the recap.");
    let content = ollama::chat_text(ollama::TEXT_MODEL, RECAP_SYSTEM, &user)
        .await
        .map_err(|e| e.to_string())?
        .trim()
        .to_string();

    {
        let conn = state.0.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        db::upsert_recap(&conn, period, start, end, &content, entry_count, &now)
            .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("recap-generated", json!({ "period": period, "start": start, "end": end }));
    Ok(true)
}

/// (Monday, Sunday) ISO dates of the most recent FULLY-completed calendar week.
pub fn last_completed_week(today: chrono::NaiveDate) -> (String, String) {
    use chrono::{Datelike, Duration};
    let this_monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    (
        (this_monday - Duration::days(7)).to_string(),
        (this_monday - Duration::days(1)).to_string(),
    )
}

/// The last `n` completed days (yesterday back), as ISO date strings.
pub fn recent_completed_days(today: chrono::NaiveDate, n: i64) -> Vec<String> {
    (1..=n).map(|i| (today - chrono::Duration::days(i)).to_string()).collect()
}

/// Auto-fill recaps for COMPLETED periods (the app may be closed at midnight, so
/// this is lazy catch-up): the last few finished days + the last finished
/// calendar week (Mon–Sun). Idempotent — `recap_period` skips ones that exist.
async fn auto_backfill_recaps(app: &tauri::AppHandle) {
    let today = chrono::Local::now().date_naive();
    for d in recent_completed_days(today, 3) {
        let _ = recap_period(app, "day", &d, &d, false).await;
    }
    let (mon, sun) = last_completed_week(today);
    let _ = recap_period(app, "week", &mon, &sun, false).await;
}

/// Manual fallback (e.g. Ollama was down at launch): re-run the auto backfill.
#[tauri::command]
async fn backfill_recaps(app: tauri::AppHandle) -> Result<(), String> {
    auto_backfill_recaps(&app).await;
    Ok(())
}

#[tauri::command]
async fn list_recaps(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let recaps = db::list_recaps(&conn, 20).map_err(|e| e.to_string())?;
    serde_json::to_value(recaps).map_err(|e| e.to_string())
}

/// Discover charts for a category from its (emergent) data shape.
#[tauri::command]
async fn category_trends(app: tauri::AppHandle, category: String) -> Result<Value, String> {
    let state = app.state::<Db>();
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
    app: tauri::AppHandle,
    question: String,
    history: Vec<ChatMsg>,
) -> Result<Value, String> {
    use std::collections::{HashMap, HashSet};
    let state = app.state::<Db>();
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
            "kind": "answer",
            "answer": "I don't have any notes yet — log a few and ask again.",
            "sources": [],
        }));
    }

    // Candidate entries (with row ids + current data) + known category names, for the router.
    let (agent_ctx, valid_ids, cur_data, known) = {
        let conn = state.0.lock().unwrap();
        let mut ctx = String::new();
        let mut ids = HashSet::new();
        let mut cur: HashMap<i64, Value> = HashMap::new();
        for h in &hits {
            for e in db::note_entries(&conn, h.note_id).map_err(|e| e.to_string())? {
                ids.insert(e.entry_id);
                cur.insert(e.entry_id, e.data.clone());
                ctx.push_str(&pipeline::agent_context(std::slice::from_ref(&e)));
            }
        }
        let known: Vec<String> = db::list_categories(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| c.name)
            .collect();
        (ctx, ids, cur, known)
    };

    // sources block for the answer path
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

    // 1) Route the message: answer | create_category | edit_entry.
    let mut convo = String::new();
    for m in &history {
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        convo.push_str(&format!("{role}: {}\n", m.content));
    }
    convo.push_str(&format!("user: {question}"));
    let route_user = format!("Candidate entries:\n{agent_ctx}\nConversation:\n{convo}");
    let routed = ollama::chat_json(
        ollama::TEXT_MODEL,
        &pipeline::route_system(&today_local()),
        &route_user,
        None,
        Some(pipeline::agent_router_schema()),
    )
    .await
    .ok();
    let action = routed
        .as_ref()
        .and_then(|v| v.get("action"))
        .and_then(|a| a.as_str())
        .unwrap_or("answer");

    // 2) Mutating actions return a PROPOSAL (no DB write) for the user to confirm.
    if action == "create_category" {
        if let Some(cat) = routed.as_ref().and_then(|v| v.get("category")) {
            let raw = cat.get("name").and_then(|n| n.as_str()).unwrap_or("").trim();
            if !raw.is_empty() {
                let name = pipeline::snap_category(raw, &known);
                let description = cat
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let already_exists = known.iter().any(|k| k == &name);
                return Ok(json!({
                    "kind": "proposal",
                    "proposal": {
                        "action": "create_category",
                        "name": name,
                        "description": description,
                        "already_exists": already_exists,
                    }
                }));
            }
        }
    } else if action == "edit_entry" {
        if let Some(edit) = routed.as_ref().and_then(|v| v.get("edit")) {
            let entry_id = edit.get("entry_id").and_then(|i| i.as_i64());
            let data = edit.get("data").cloned();
            if let (Some(eid), Some(data)) = (entry_id, data) {
                if valid_ids.contains(&eid) && data.is_object() {
                    // Merge the model's change over the entry's CURRENT data, so a
                    // truncated reply can't silently drop sibling fields. `data` in the
                    // proposal is the exact object that will be written — the UI shows it
                    // so the user can catch any mistake before confirming.
                    let mut preview = cur_data.get(&eid).cloned().unwrap_or_else(|| json!({}));
                    match (preview.as_object_mut(), data.as_object()) {
                        (Some(base), Some(patch)) => {
                            for (k, v) in patch {
                                base.insert(k.clone(), v.clone());
                            }
                        }
                        _ => preview = data.clone(),
                    }
                    let summary = edit
                        .get("summary")
                        .and_then(|s| s.as_str())
                        .unwrap_or("update this entry")
                        .to_string();
                    return Ok(json!({
                        "kind": "proposal",
                        "proposal": {
                            "action": "edit_entry",
                            "entry_id": eid,
                            "data": preview,
                            "summary": summary,
                        }
                    }));
                }
            }
        }
        // fall through to a normal answer if the edit target was invalid/ambiguous
    }

    // 3) Default: grounded free-text answer (unchanged behavior). We deliberately
    // ignore the router's `clarify` field — a 7B over-eagerly fills it even on
    // plain questions, which would hijack normal answers.
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

    Ok(json!({ "kind": "answer", "answer": answer.trim(), "sources": sources }))
}

/// Create a new category by name — the chat agent's confirmed `create_category`
/// action. Idempotent: returns the existing id if the name already exists.
#[tauri::command]
async fn create_category(
    app: tauri::AppHandle,
    name: String,
    description: String,
) -> Result<i64, String> {
    let state = app.state::<Db>();
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return Err("empty category name".into());
    }
    let now = chrono::Utc::now().to_rfc3339();
    let conn = state.0.lock().unwrap();
    db::create_category(&conn, &name, description.trim(), &now).map_err(|e| e.to_string())
}

/// Overwrite one entry's structured data — the chat agent's confirmed `edit_entry`
/// action — then re-embed the affected note so semantic search reflects the fix.
#[tauri::command]
async fn update_entry(
    app: tauri::AppHandle,
    entry_id: i64,
    data: Value,
) -> Result<i64, String> {
    let state = app.state::<Db>();
    if !data.is_object() {
        return Err("entry data must be an object".into());
    }
    let (note_id, text) = {
        let conn = state.0.lock().unwrap();
        let note_id = db::update_entry_data(&conn, entry_id, &data).map_err(|e| e.to_string())?;
        let text = db::note_embed_text(&conn, note_id).map_err(|e| e.to_string())?;
        (note_id, text)
    };
    // re-embed off-lock; insert_embedding REPLACEs the stale vector
    if let Ok(v) = ollama::embed(&text).await {
        let v = normalize(v);
        let conn = state.0.lock().unwrap();
        let _ = db::insert_embedding(&conn, note_id, &v);
    }
    Ok(note_id)
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
async fn reindex(app: tauri::AppHandle) -> Result<i64, String> {
    let state = app.state::<Db>();
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
fn phone_info(app: tauri::AppHandle) -> Value {
    let state = app.state::<phone::PhoneState>();
    json!({ "url": state.url, "lan_url": state.lan_url, "token": state.token, "port": state.port })
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

/// Knowledge-graph entities, most-mentioned first (for the graph view + management).
#[tauri::command]
async fn list_entities(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let ents = db::list_entities(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(ents).map_err(|e| e.to_string())
}

/// Merge one entity into another (manual dedup). Reassigns mentions + aliases.
#[tauri::command]
async fn merge_entities(app: tauri::AppHandle, keep: i64, drop: i64) -> Result<(), String> {
    let state = app.state::<Db>();
    let mut conn = state.0.lock().unwrap();
    db::merge_entities(&mut conn, keep, drop).map_err(|e| e.to_string())
}

/// The whole knowledge graph for the "Self" view: entity nodes + co-mention edges.
#[tauri::command]
async fn entity_graph(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let nodes = db::list_entities(&conn).map_err(|e| e.to_string())?;
    let edges = db::entity_edges(&conn).map_err(|e| e.to_string())?;
    Ok(json!({
        "nodes": serde_json::to_value(nodes).map_err(|e| e.to_string())?,
        "edges": serde_json::to_value(edges).map_err(|e| e.to_string())?,
    }))
}

/// The notes that mention one entity (for the graph's detail panel).
#[tauri::command]
async fn entity_detail(app: tauri::AppHandle, entity_id: i64) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let rows = db::entity_detail(&conn, entity_id, 20).map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

/// People view: every `person` entity with its dated, curated-fact mentions.
#[tauri::command]
async fn list_people(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let people = db::person_profiles(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(people).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// App setup
// ---------------------------------------------------------------------------

// ── Model provider settings (Local / Balanced + Gemini key) ─────────────────
#[tauri::command]
fn get_provider_settings() -> Value {
    let c = provider::get();
    json!({
        "mode": c.mode,
        "gemini_text_model": c.gemini_text_model,
        "gemini_vision_model": c.gemini_vision_model,
        "has_gemini_key": c.gemini_api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false),
    })
}

#[tauri::command]
fn set_provider_settings(
    app: tauri::AppHandle,
    mode: String,
    gemini_api_key: Option<String>,
    gemini_text_model: Option<String>,
    gemini_vision_model: Option<String>,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    provider::update(&dir, &mode, gemini_api_key, gemini_text_model, gemini_vision_model)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn test_provider() -> Result<String, String> {
    provider::test_gemini().await.map_err(|e| e.to_string())
}

// ── Quick-capture background worker ─────────────────────────────────────────
const PENDING_MAX_ATTEMPTS: i64 = 5;
static PENDING_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Read an image file and base64-encode it for the vision pipeline.
fn load_image_b64(path: &str) -> Option<String> {
    use base64::Engine;
    std::fs::read(path)
        .ok()
        .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
}

/// Record a capture-processing failure and notify the UI (recoverable; retried
/// until PENDING_MAX_ATTEMPTS).
fn stamp_pending_error(app: &tauri::AppHandle, id: i64, err: &str) {
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let _ = db::set_pending_error(&conn, id, err);
    }
    let _ = app.emit("capture-needs-attention", json!({ "id": id, "error": err }));
    eprintln!("[noted] capture {id} failed: {err}");
}

/// Drain the quick-capture queue: categorize each pending capture with the local
/// pipeline and write it as a real note via the normal save path, then delete
/// the row (or stamp an error for retry). Re-entrancy-guarded so a slow pass
/// can't overlap the next tick and double-file.
async fn process_pending_captures(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    if PENDING_BUSY.swap(true, Ordering::SeqCst) {
        return; // a previous pass is still running
    }
    if let Err(e) = process_pending_inner(app).await {
        eprintln!("[noted] pending-capture pass error: {e}");
    }
    PENDING_BUSY.store(false, Ordering::SeqCst);
}

async fn process_pending_inner(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<Db>();
    let pending = {
        let conn = state.0.lock().unwrap();
        db::list_pending(&conn, PENDING_MAX_ATTEMPTS).map_err(|e| e.to_string())?
    };
    for p in pending {
        // Load catalog + known category names (lock dropped before any await).
        let (catalog, known) = {
            let conn = state.0.lock().unwrap();
            let catalog = db::category_catalog(&conn).map_err(|e| e.to_string())?;
            let names: Vec<String> = db::list_categories(&conn)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|c| c.name)
                .collect();
            (catalog, names)
        };
        let today = today_local();

        // Categorize: text directly; photos via the vision transcription path.
        let envelope = if p.source == "photo" {
            match p.image_path.as_deref().and_then(load_image_b64) {
                Some(b64) => pipeline::categorize_photo(&catalog, &known, &b64, &today).await,
                None => Err(anyhow::anyhow!("missing or unreadable image")),
            }
        } else {
            pipeline::categorize(&catalog, &known, &p.raw_text, &today).await
        };

        match envelope {
            Ok(env) => {
                // Prefer the user's explicit date, else the extracted one.
                let event_date = p
                    .event_date
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| env.get("event_date").and_then(|d| d.as_str()).map(String::from))
                    .unwrap_or_else(today_local);
                // The envelope's entries/entities already match SaveArgs' shape
                // (category/description/data, name/type/fact/relationship).
                let save_json = json!({
                    "raw_text": env.get("raw_text").cloned().unwrap_or_else(|| json!(p.raw_text)),
                    "source": p.source,
                    "image_path": p.image_path,
                    "event_date": event_date,
                    "entries": env.get("entries").cloned().unwrap_or_else(|| json!([])),
                    "entities": env.get("entities").cloned().unwrap_or_else(|| json!([])),
                });
                match serde_json::from_value::<SaveArgs>(save_json) {
                    Ok(args) => match save_entry(app.clone(), args).await {
                        Ok(_) => {
                            {
                                let conn = state.0.lock().unwrap();
                                let _ = db::delete_pending(&conn, p.id);
                            }
                            let _ = app.emit("note-filed", json!({ "id": p.id }));
                        }
                        Err(e) => stamp_pending_error(app, p.id, &e),
                    },
                    Err(e) => stamp_pending_error(app, p.id, &e.to_string()),
                }
            }
            Err(e) => stamp_pending_error(app, p.id, &e.to_string()),
        }
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::init(&dir.join("noted.db"))?;
            app.manage(Db(Mutex::new(conn)));

            // Load model-provider config (mode + models from disk, key from Keychain).
            provider::init(&dir);

            // Phone capture: tiny LAN upload server gated by a random token.
            let inbox = dir.join("inbox");
            std::fs::create_dir_all(&inbox)?;
            // Stable token + stable hostname so a phone's "Add to Home Screen"
            // icon keeps working across launches (and DHCP IP changes).
            let token = phone::load_or_make_token(&dir);
            let ip = local_ip_address::local_ip()
                .map(|i| i.to_string())
                .unwrap_or_else(|_| "localhost".to_string());
            let host = phone::local_hostname().map(|h| format!("{h}.local"));
            // Cert SANs: the .local name (primary), the LAN IP, and localhost.
            let mut sans = vec![ip.clone(), "localhost".to_string()];
            if let Some(h) = &host {
                sans.insert(0, h.clone());
            }
            if let Some((server, port)) = phone::bind_https(&dir, &sans, 8787) {
                // Prefer the stable .local name; fall back to the raw IP.
                let host_for_url = host.clone().unwrap_or_else(|| ip.clone());
                let url = format!("https://{host_for_url}:{port}/?t={token}");
                let lan_url = format!("https://{ip}:{port}/?t={token}");
                println!("[noted] phone access ready (full app): {url}");
                app.manage(phone::PhoneState { url, lan_url, token: token.clone(), port });
                phone::serve(server, app.handle().clone(), inbox, token);
            } else {
                app.manage(phone::PhoneState {
                    url: String::new(),
                    lan_url: String::new(),
                    token,
                    port: 0,
                });
            }

            // Auto recaps: catch up missing completed-period recaps on launch, then
            // re-check hourly so a day/week rolling over while open gets recapped.
            let h = app.handle().clone();
            tauri::async_runtime::spawn(async move { auto_backfill_recaps(&h).await });
            let h2 = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
                let h3 = h2.clone();
                tauri::async_runtime::spawn(async move { auto_backfill_recaps(&h3).await });
            });

            // Quick-capture worker: drain leftovers at launch, then poll every 5s
            // so phone captures get categorized + filed shortly after they arrive.
            let hp = app.handle().clone();
            tauri::async_runtime::spawn(async move { process_pending_captures(&hp).await });
            let hp2 = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let h = hp2.clone();
                tauri::async_runtime::spawn(async move { process_pending_captures(&h).await });
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            health,
            categorize_note,
            categorize_photo,
            save_image,
            save_entry,
            quick_capture,
            list_notes,
            list_categories,
            chat,
            create_category,
            update_entry,
            speak,
            stop_speaking,
            reindex,
            category_trends,
            generate_recap,
            backfill_recaps,
            list_recaps,
            export_db,
            phone_info,
            read_inbox_image,
            voice_status,
            download_voice_model,
            transcribe,
            list_entities,
            merge_entities,
            entity_graph,
            entity_detail,
            list_people,
            get_provider_settings,
            set_provider_settings,
            test_provider,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
