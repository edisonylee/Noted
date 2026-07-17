pub mod analytics;
pub mod brain;
pub mod db;
pub mod entities;
pub mod gcal;
pub mod meeting;
pub mod ollama;
pub mod phone;
pub mod pipeline;
pub mod provider;
pub mod themes;
pub mod voice;

use db::{Db, SaveInput};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

/// The app's fixed timezone. "Today" is always an Eastern calendar day, DST-aware
/// (EST↔EDT handled by the tz database), regardless of the machine's own timezone.
const APP_TZ: chrono_tz::Tz = chrono_tz::America::New_York;

/// Current instant as Eastern wall-clock time.
fn now_eastern() -> chrono::DateTime<chrono_tz::Tz> {
    chrono::Utc::now().with_timezone(&APP_TZ)
}

/// Eastern calendar date (YYYY-MM-DD) — the user's "today", not UTC, not the
/// machine's local zone.
fn today_local() -> String {
    now_eastern().date_naive().to_string()
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

/// Persisted theme selection shared by desktop and phone.
#[tauri::command]
async fn theme_state(app: tauri::AppHandle) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    serde_json::to_value(themes::read_state(&dir)).map_err(|e| e.to_string())
}

/// User-imported theme packs. Built-ins ship with the frontend and are not
/// duplicated on disk.
#[tauri::command]
async fn theme_list(app: tauri::AppHandle) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let packs = themes::list(&dir).map_err(|e| e.to_string())?;
    serde_json::to_value(packs).map_err(|e| e.to_string())
}

/// Validate and atomically save a normalized, data-only theme pack.
#[tauri::command]
async fn theme_save(app: tauri::AppHandle, pack: themes::ThemePack) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let pack = themes::save(&dir, pack).map_err(|e| e.to_string())?;
    serde_json::to_value(pack).map_err(|e| e.to_string())
}

#[tauri::command]
async fn theme_activate(app: tauri::AppHandle, theme_id: String, color_mode: Option<String>) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let state = themes::activate(&dir, &theme_id, color_mode.as_deref()).map_err(|e| e.to_string())?;
    serde_json::to_value(state).map_err(|e| e.to_string())
}

#[tauri::command]
async fn theme_set_color_mode(app: tauri::AppHandle, color_mode: String) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let state = themes::set_color_mode(&dir, &color_mode).map_err(|e| e.to_string())?;
    serde_json::to_value(state).map_err(|e| e.to_string())
}

#[tauri::command]
async fn theme_delete(app: tauri::AppHandle, theme_id: String) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let state = themes::delete(&dir, &theme_id).map_err(|e| e.to_string())?;
    serde_json::to_value(state).map_err(|e| e.to_string())
}

/// Compile pasted design guidance with Ollama only. This call deliberately uses
/// `chat_json_local` and therefore cannot route to Gemini in Balanced mode.
#[tauri::command]
async fn theme_compile_design(design_md: String, name: Option<String>) -> Result<Value, String> {
    let pack = themes::compile_design(&design_md, name.as_deref()).await.map_err(|e| e.to_string())?;
    serde_json::to_value(pack).map_err(|e| e.to_string())
}

/// Pick a frontend-supplied built-in/custom candidate for an assistant request.
/// Selection is local-only and returns a proposal; it never activates a theme.
#[tauri::command]
async fn theme_suggest(prompt: String, candidates: Vec<themes::ThemeCandidate>) -> Result<Value, String> {
    themes::suggest(&prompt, &candidates).await.map_err(|e| e.to_string())
}

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

/// Transcribe a photo to text only — the vision/OCR step, skipping the extract
/// pipeline. The Today schedule flow uses this: it re-parses the text itself
/// (parseSchedule), so running extraction would be wasted work and could fail on
/// a pure schedule that has no structured data to pull.
#[tauri::command]
async fn ocr_photo(image_base64: String) -> Result<String, String> {
    pipeline::transcribe_photo(&image_base64).await.map_err(|e| e.to_string())
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

    // Deterministic safety net: promote people the model named in a field (e.g.
    // {"with": "Khai"}) but may have omitted from the `entities` array, so they
    // still reach the People graph. Computed before `entries` moves into save_note.
    let field_people: Vec<String> = entries
        .iter()
        .flat_map(|e| entities::person_names_from_data(&e.data))
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

    // Persist knowledge-graph entities (best effort — never fails the save).
    // Candidates = the model's `entities` ∪ people promoted from entry fields,
    // deduped by (normalized name, type).
    let mut candidates: Vec<EntityCandidate> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for e in &args.entities {
        let name = e.name.trim();
        let etype = e.etype.trim().to_lowercase();
        if name.is_empty() || etype.is_empty() {
            continue;
        }
        if seen.insert((entities::normalize(name), etype.clone())) {
            candidates.push(EntityCandidate {
                name: name.to_string(),
                etype,
                fact: e.fact.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from),
                relationship: e
                    .relationship
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from),
            });
        }
    }
    for name in field_people {
        if seen.insert((entities::normalize(&name), "person".to_string())) {
            candidates.push(EntityCandidate { name, etype: "person".to_string(), fact: None, relationship: None });
        }
    }
    if !candidates.is_empty() {
        let snippet = plain_text(&raw, 200);
        persist_entities(&app, note_id, &event_date, &snippet, &now, candidates, false).await;
    }

    Ok(note_id)
}

/// The Journal's reflection agent. One structured model call — strictly LOCAL,
/// never the Balanced-mode cloud path: reflections are the most private text in
/// the app — both writes a short companion reply and pulls out knowledge-graph
/// entities. The reflection is then persisted as a `journal` note through the
/// normal save path (embedding + entities → the personal graph). A dead model
/// costs the reply and the entities, never the entry.
#[tauri::command]
async fn journal_reflect(
    app: tauri::AppHandle,
    text: String,
    history: Vec<ChatMsg>,
) -> Result<Value, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("write a reflection first".into());
    }

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "reply": { "type": "string" },
            "entities": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "type": { "type": "string" },
                        "fact": { "type": ["string", "null"] },
                        "relationship": { "type": ["string", "null"] }
                    },
                    "required": ["name", "type"]
                }
            }
        },
        "required": ["reply", "entities"]
    });
    let system = "You are the journal inside noted, a private local note-taking app. \
The user writes personal reflections; you respond as a warm, grounded journaling companion.\n\
Return JSON only:\n\
- reply: 2-4 sentences. Be specific to what they actually wrote — mirror the people, places \
and feelings they named. No generic advice. End with at most one gentle follow-up question.\n\
- entities: the people, places, activities, foods, items, orgs and topics the reflection \
mentions (type = person|place|activity|food|item|org|topic). For a person include a short \
fact the reflection reveals about them, and the relationship when stated. [] if none.";

    // A little session context so consecutive reflections read as one sitting.
    let recent: Vec<&ChatMsg> = history.iter().rev().take(6).collect();
    let mut convo = String::new();
    for m in recent.into_iter().rev() {
        let who = if m.role == "assistant" { "journal" } else { "user" };
        convo.push_str(&format!("{who}: {}\n", m.content));
    }
    let user_msg = if convo.is_empty() {
        format!("Reflection:\n{text}")
    } else {
        format!("Earlier this session:\n{convo}\nNew reflection:\n{text}")
    };

    let model_out =
        ollama::chat_json_local(&ollama::text_model(), system, &user_msg, None, Some(schema)).await;

    let (reply, entities): (Option<String>, Vec<EntityArg>) = match &model_out {
        Ok(v) => (
            v.get("reply")
                .and_then(|r| r.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            v.get("entities")
                .and_then(|e| e.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| {
                            Some(EntityArg {
                                name: e.get("name")?.as_str()?.trim().to_string(),
                                etype: e
                                    .get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("topic")
                                    .trim()
                                    .to_lowercase(),
                                fact: e.get("fact").and_then(|f| f.as_str()).map(String::from),
                                relationship: e
                                    .get("relationship")
                                    .and_then(|r| r.as_str())
                                    .map(String::from),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        Err(e) => {
            eprintln!("[noted] journal reflect model failed: {e}");
            (None, Vec::new())
        }
    };

    let entity_count = entities.len();
    let note_id = save_entry(
        app.clone(),
        SaveArgs {
            raw_text: text.clone(),
            source: "journal".to_string(),
            image_path: None,
            event_date: today_local(),
            entries: vec![EntryArg {
                category: "journal".to_string(),
                description: "personal reflection".to_string(),
                data: serde_json::json!({ "reflection": text }),
            }],
            entities,
        },
    )
    .await?;

    Ok(serde_json::json!({
        "reply": reply,
        "note_id": note_id,
        "entity_count": entity_count,
    }))
}

/// A short source snippet for the chat sources list. For an imported brain note
/// the text starts with a YAML frontmatter block — skip it so the snippet shows
/// real content, not `---\nname: …`.
fn note_snippet(raw: &str) -> String {
    plain_text(raw, 140)
}

/// Flatten note text for snippet display: drop YAML frontmatter and markdown
/// syntax (headings, emphasis, bullets, code ticks) so UI chips and stored
/// mention contexts read as prose, not `# Daily Stand Up ## Summary`.
fn plain_text(raw: &str, n: usize) -> String {
    let body = match raw.strip_prefix("---\n") {
        Some(rest) => rest.split_once("\n---").map(|(_, b)| b).unwrap_or(rest),
        None => raw,
    };
    let mut out = String::new();
    for line in body.lines() {
        let line = line.trim_start_matches(['#', '>', ' ']);
        let line = line.strip_prefix("- ").unwrap_or(line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&line.replace(['*', '`'], ""));
    }
    out.chars().take(n).collect()
}

/// A resolved entity to attach to a note: a name + type, plus optional curated
/// person details. Shared by the save path and the backfill command.
struct EntityCandidate {
    name: String,
    etype: String,
    fact: Option<String>,
    relationship: Option<String>,
}

/// Resolve + create + link a batch of entity candidates to a note. Embeds each
/// off-lock, then resolves (exact/alias or near-neighbor) + creates + links a
/// mention under the lock — the single entity-write path for both save + backfill.
/// `guard_existing`: skip a mention if this entity already links to this note
/// (keeps backfill idempotent). Returns the number of mentions added.
async fn persist_entities(
    app: &tauri::AppHandle,
    note_id: i64,
    event_date: &str,
    snippet: &str,
    now: &str,
    candidates: Vec<EntityCandidate>,
    guard_existing: bool,
) -> i64 {
    // Embed off-lock (network), carrying curated details to store post-resolve.
    let mut embedded: Vec<(String, String, Vec<f32>, Option<String>, Option<String>)> = Vec::new();
    for c in candidates {
        if c.name.is_empty() || c.etype.is_empty() {
            continue;
        }
        if let Ok(v) = entities::embed_entity(&c.name, &c.etype).await {
            embedded.push((c.name, c.etype, v, c.fact, c.relationship));
        }
    }
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let mut added = 0;
    for (name, etype, emb, fact, rel) in &embedded {
        let id = match entities::resolve_with_embedding(&conn, name, etype, emb) {
            Ok(entities::Resolution::Exact(id)) => id,
            // Fuzzy snap is for spelling drift ("Sara"/"Sarah") — never for
            // emails: two addresses at one domain embed nearly identically, so
            // snapping would pool different people onto one entity (it did,
            // before this guard).
            Ok(entities::Resolution::Suggest(id, _)) if !name.contains('@') => id,
            Ok(entities::Resolution::Suggest(_, _)) | Ok(entities::Resolution::New) => {
                let norm = entities::normalize(name);
                match db::create_entity(&conn, name, &norm, etype, "[]", event_date, now) {
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
        // Idempotent backfill: don't re-link an entity already mentioned in this note.
        if guard_existing && db::mention_exists(&conn, id, note_id).unwrap_or(false) {
            continue;
        }
        // Prefer the curated per-person fact as context; fall back to the snippet.
        let context = fact.as_deref().unwrap_or(snippet);
        if db::add_mention(&conn, id, note_id, None, context, event_date, now).is_ok() {
            added += 1;
        }
    }
    added
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
    let today = now_eastern().date_naive();
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
    let content = ollama::chat_text(&ollama::text_model(), system, &user)
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
    let content = ollama::chat_text(&ollama::text_model(), RECAP_SYSTEM, &user)
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
    let today = now_eastern().date_naive();
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
    scope: Option<String>,
    entity_id: Option<i64>,
) -> Result<Value, String> {
    use std::collections::{HashMap, HashSet};
    let state = app.state::<Db>();
    if question.trim().is_empty() {
        return Err("empty question".into());
    }

    // Item-scoped ask: pin the answer to ONE entity (project/person/decision) —
    // its curated brain note PLUS every capture that mentions it. No embedding
    // needed (mention-based), read-only.
    if let Some(eid) = entity_id {
        let (name, hits) = {
            let conn = state.0.lock().unwrap();
            let name = db::entity_name_type(&conn, eid)
                .map_err(|e| e.to_string())?
                .map(|(n, _)| n)
                .unwrap_or_else(|| "that".to_string());
            let hits = db::notes_for_entity(&conn, eid, 15).map_err(|e| e.to_string())?;
            (name, hits)
        };
        if hits.is_empty() {
            return Ok(json!({
                "kind": "answer",
                "answer": format!("I don't have any notes about {name} yet."),
                "sources": [],
            }));
        }
        let sources: Vec<Value> = hits
            .iter()
            .take(6)
            .map(|h| {
                let lbl = match h.origin.as_deref() {
                    Some(o) if o.starts_with("brain:") => o.trim_start_matches("brain:").to_string(),
                    _ => h.category.clone().unwrap_or_else(|| "note".to_string()),
                };
                json!({ "note_id": h.note_id, "category": lbl, "event_date": h.event_date, "snippet": note_snippet(&h.raw_text) })
            })
            .collect();
        let context = pipeline::qa_context(&hits);
        let mut messages = vec![json!({ "role": "system", "content": pipeline::qa_system(&today_local()) })];
        for m in &history {
            let role = if m.role == "assistant" { "assistant" } else { "user" };
            messages.push(json!({ "role": role, "content": m.content }));
        }
        messages.push(json!({
            "role": "user",
            "content": format!("Everything I know about \"{name}\":\n{context}\nQuestion: {question}")
        }));
        let answer = ollama::chat_messages(&ollama::text_model(), messages, 0.2)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(json!({ "kind": "answer", "answer": answer.trim(), "sources": sources }));
    }

    let qv = normalize(ollama::embed(&question).await.map_err(|e| e.to_string())?);

    // Vault-scoped ask: restrict retrieval to ONE brain so answers about specific
    // work don't bleed across vaults. Read-only (no edit/create routing).
    if let Some(vault) = scope.as_deref().map(str::trim).filter(|s| !s.is_empty() && *s != "all") {
        let origin = format!("brain:{vault}");
        let hits = {
            let conn = state.0.lock().unwrap();
            db::search_notes_scoped(&conn, &qv, 12, &origin).map_err(|e| e.to_string())?
        };
        if hits.is_empty() {
            return Ok(json!({
                "kind": "answer",
                "answer": format!("I don't have anything about that in your {vault} brain."),
                "sources": [],
            }));
        }
        let sources: Vec<Value> = hits
            .iter()
            .take(6)
            .map(|h| {
                json!({
                    "note_id": h.note_id,
                    "category": vault,
                    "event_date": h.event_date,
                    "snippet": note_snippet(&h.raw_text),
                })
            })
            .collect();
        let context = pipeline::qa_context(&hits);
        let mut messages = vec![json!({ "role": "system", "content": pipeline::qa_system(&today_local()) })];
        for m in &history {
            let role = if m.role == "assistant" { "assistant" } else { "user" };
            messages.push(json!({ "role": role, "content": m.content }));
        }
        messages.push(json!({
            "role": "user",
            "content": format!("Entries (from the {vault} knowledge base only):\n{context}\nQuestion: {question}")
        }));
        let answer = ollama::chat_messages(&ollama::text_model(), messages, 0.2)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(json!({ "kind": "answer", "answer": answer.trim(), "sources": sources }));
    }

    // Day-scoped question ("what's my schedule today?"): pin retrieval to that
    // one date so the answer can't drift into other days — the date filter is
    // code, not the model. Broad questions keep the hybrid retrieval below.
    let day_scope = pipeline::day_scope(&question, &today_local());

    // Label brain notes by their vault ("baro"/"profound") for clear
    // provenance; capture notes keep their category.
    let source_json = |h: &db::SearchHit| {
        let label = match h.origin.as_deref() {
            Some(o) if o.starts_with("brain:") => Some(o.trim_start_matches("brain:").to_string()),
            _ => h.category.clone(),
        };
        json!({
            "note_id": h.note_id,
            "category": label,
            "event_date": h.event_date,
            "snippet": note_snippet(&h.raw_text),
        })
    };

    // Two retrieval sets: recent-by-date (recency questions) and semantic
    // (relevance — this is what surfaces brain/reference notes). Context is
    // recent-first; the SOURCES we attribute are relevance-first, so the note
    // that actually answered (often a brain note) is credited, not the latest
    // bagel. A third, graph-guided set rides along: entities matched from the
    // question pull in their own notes plus a structured relationship digest.
    let (hits, sources, graph_digest, graph_entities) = {
        let conn = state.0.lock().unwrap();
        if let Some((day, _)) = &day_scope {
            // The graph digest is skipped here: its dated facts span other
            // days and would leak them back into the answer.
            let day_hits = db::notes_on_date(&conn, day, 15).map_err(|e| e.to_string())?;
            let sources: Vec<Value> = day_hits.iter().take(6).map(source_json).collect();
            (day_hits, sources, String::new(), Vec::new())
        } else {
            let recent = db::recent_entries(&conn, 15).map_err(|e| e.to_string())?;
            let semantic = db::search_notes(&conn, &qv, 8).map_err(|e| e.to_string())?;
            let (graph_digest, graph_entities, graph_hits) = graph_context(&conn, &question, &qv);
            let mut src_seen = HashSet::new();
            let sources: Vec<Value> = semantic
                .iter()
                .chain(recent.iter())
                .filter(|h| src_seen.insert(h.note_id))
                .take(6)
                .map(source_json)
                .collect();
            let mut seen = HashSet::new();
            let mut hits = Vec::new();
            for h in recent.into_iter().chain(semantic.into_iter()).chain(graph_hits.into_iter()) {
                if seen.insert(h.note_id) {
                    hits.push(h);
                }
            }
            (hits, sources, graph_digest, graph_entities)
        }
    };
    // A day-scoped question with an empty day still goes through the router —
    // "schedule dinner tomorrow at 7" must reach create_event.
    if hits.is_empty() && day_scope.is_none() {
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

    // 1) Route the message: answer | create_category | edit_entry.
    let mut convo = String::new();
    for m in &history {
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        convo.push_str(&format!("{role}: {}\n", m.content));
    }
    convo.push_str(&format!("user: {question}"));
    let route_user = format!("Candidate entries:\n{agent_ctx}\nConversation:\n{convo}");
    let routed = ollama::chat_json(
        &ollama::text_model(),
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
    } else if action == "create_event" {
        // Scheduling: validated proposal only — the UI confirms, then writes to
        // Google Calendar. Malformed model output falls through to an answer.
        if let Some(ev) = routed.as_ref().and_then(|v| v.get("event")) {
            let time_ok = |s: &str| {
                s.len() == 5
                    && s.as_bytes()[2] == b':'
                    && s[..2].chars().all(|c| c.is_ascii_digit())
                    && s[3..].chars().all(|c| c.is_ascii_digit())
            };
            let title = ev.get("title").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
            let date = ev.get("date").and_then(|d| d.as_str()).unwrap_or("").trim().to_string();
            let date_ok = date.len() == 10
                && date
                    .chars()
                    .enumerate()
                    .all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
            let start = ev
                .get("start")
                .and_then(|s| s.as_str())
                .map(str::trim)
                .filter(|s| time_ok(s))
                .map(String::from);
            let end = ev
                .get("end")
                .and_then(|s| s.as_str())
                .map(str::trim)
                .filter(|s| time_ok(s))
                .map(String::from);
            let guests: Vec<String> = ev
                .get("guests")
                .and_then(|g| g.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str())
                        .filter(|s| s.contains('@'))
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            let meet = ev.get("meet").and_then(|m| m.as_bool()).unwrap_or(false);
            if !title.is_empty() && date_ok {
                let summary = ev
                    .get("summary")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                return Ok(json!({
                    "kind": "proposal",
                    "proposal": {
                        "action": "create_event",
                        "title": title,
                        "date": date,
                        "start": start,
                        "end": end,
                        "guests": guests,
                        "meet": meet,
                        "summary": summary,
                    }
                }));
            }
        }
        // fall through to a normal answer if the event was malformed
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
    if let Some((_, label)) = &day_scope {
        // Nothing logged that day: answer deterministically instead of letting
        // the model improvise from an empty context.
        if hits.is_empty() {
            return Ok(json!({
                "kind": "answer",
                "answer": format!("You don't have anything logged for {label}."),
                "sources": [],
            }));
        }
    }
    let context = pipeline::qa_context(&hits);
    let mut messages = vec![json!({ "role": "system", "content": pipeline::qa_system(&today_local()) })];
    for m in &history {
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        messages.push(json!({ "role": role, "content": m.content }));
    }
    let user_turn = if let Some((day, label)) = &day_scope {
        format!(
            "Entries dated {day} ({label}) — the ONLY day the question asks about; \
do not mention any other day:\n{context}\nQuestion: {question}"
        )
    } else if graph_digest.is_empty() {
        format!("Entries:\n{context}\nQuestion: {question}")
    } else {
        format!("Knowledge graph:\n{graph_digest}\nEntries:\n{context}\nQuestion: {question}")
    };
    messages.push(json!({ "role": "user", "content": user_turn }));
    let answer = ollama::chat_messages(&ollama::text_model(), messages, 0.2)
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({
        "kind": "answer",
        "answer": answer.trim(),
        "sources": sources,
        "entities": graph_entities,
    }))
}

/// Match a chat question against the knowledge graph and assemble the graph's
/// contribution to the answer: a compact structured digest (who/what matched,
/// their co-mention neighbors, their most recent dated facts), the matched
/// entities for the UI's "from the graph" chips, and those entities' own notes
/// as extra retrieval hits. Matching is name/alias word-hit first (precise),
/// then embedding nearest-neighbor (fuzzy), capped small so the digest stays a
/// digest. Empty results everywhere when the graph has nothing to say.
fn graph_context(
    conn: &rusqlite::Connection,
    question: &str,
    qv: &[f32],
) -> (String, Vec<Value>, Vec<db::SearchHit>) {
    const MAX_ENTITIES: usize = 4;
    const EMBED_SIM_FLOOR: f32 = 0.55;

    let q = question.to_lowercase();
    let word_hit = |needle: &str| -> bool {
        if needle.len() < 3 {
            return false;
        }
        let mut start = 0;
        while let Some(pos) = q[start..].find(needle) {
            let at = start + pos;
            let before_ok = at == 0 || !q[..at].chars().next_back().unwrap().is_alphanumeric();
            let after = at + needle.len();
            let after_ok = after >= q.len() || !q[after..].chars().next().unwrap().is_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
            start = at + 1;
        }
        false
    };

    let mut matched: Vec<i64> = Vec::new();
    if let Ok(all) = db::entities_for_matching(conn) {
        for (id, name, _t, aliases) in &all {
            if word_hit(&name.to_lowercase())
                || aliases.iter().any(|a| word_hit(&a.to_lowercase()))
            {
                matched.push(*id);
            }
        }
    }
    // Fuzzy fill: embedding neighbors of the question, only above a floor so an
    // unrelated question doesn't drag random entities into every answer.
    if matched.len() < MAX_ENTITIES {
        if let Ok(nn) = db::nearest_entities_any(conn, qv, 6) {
            for (id, dist) in nn {
                let sim = 1.0 - dist * dist / 2.0;
                if sim >= EMBED_SIM_FLOOR && !matched.contains(&id) {
                    matched.push(id);
                }
            }
        }
    }
    matched.truncate(MAX_ENTITIES);
    if matched.is_empty() {
        return (String::new(), Vec::new(), Vec::new());
    }

    let mut digest = String::new();
    let mut chips: Vec<Value> = Vec::new();
    let mut extra_hits: Vec<db::SearchHit> = Vec::new();
    for id in matched {
        let Ok(p) = db::entity_profile(conn, id) else { continue };
        chips.push(json!({ "id": p.id, "name": p.name, "type": p.r#type }));
        digest.push_str(&format!("- {} ({}, {} mentions", p.name, p.r#type, p.mention_count));
        if let Some(last) = &p.last_seen {
            digest.push_str(&format!(", last {last}"));
        }
        digest.push(')');
        if let Ok(neigh) = db::entity_neighbors(conn, id, 6) {
            if !neigh.is_empty() {
                let list: Vec<String> = neigh
                    .iter()
                    .map(|(n, _t, w)| format!("{n} ({w} shared)"))
                    .collect();
                digest.push_str(&format!(" — linked to: {}", list.join(", ")));
            }
        }
        digest.push('\n');
        for m in p.mentions.iter().filter(|m| !m.text.trim().is_empty()).take(3) {
            digest.push_str(&format!("    • {}: {}\n", m.date, m.text.trim()));
        }
        if let Ok(notes) = db::notes_for_entity(conn, id, 3) {
            extra_hits.extend(notes);
        }
    }
    (digest, chips, extra_hits)
}

/// Proactive surfacing: given in-progress capture text, return related brain
/// notes (subject entity + vault + snippet) so the UI can show "related in your
/// brain" as you write. Best-effort; empty until brain notes are embedded.
#[tauri::command]
async fn related_brain(app: tauri::AppHandle, text: String) -> Result<Value, String> {
    let t = text.trim();
    if t.chars().count() < 4 {
        return Ok(json!([]));
    }
    let qv = normalize(ollama::embed(t).await.map_err(|e| e.to_string())?);
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let hits = db::search_notes_brain(&conn, &qv, 5).map_err(|e| e.to_string())?;
    let out: Vec<Value> = hits
        .iter()
        .map(|h| {
            let vault = h
                .origin
                .as_deref()
                .map(|o| o.trim_start_matches("brain:").to_string())
                .unwrap_or_default();
            // The note's subject entity (its brain home), if any.
            let ent: Option<(i64, String)> = conn
                .query_row(
                    "SELECT id, name FROM entities WHERE home_note_id = ?1 LIMIT 1",
                    [h.note_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            json!({
                "note_id": h.note_id,
                "vault": vault,
                "entity_id": ent.as_ref().map(|(id, _)| *id),
                "name": ent.as_ref().map(|(_, n)| n.clone()),
                "snippet": note_snippet(&h.raw_text),
            })
        })
        .collect();
    Ok(json!(out))
}

/// Whether auto-propagation (timed write-back + export) is on.
#[tauri::command]
fn brain_get_auto() -> bool {
    brain::auto_propagate()
}

/// Turn auto-propagation on/off (persisted). Import + embed always run regardless.
#[tauri::command]
fn brain_set_auto(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    brain::set_auto_propagate(&dir, enabled);
    Ok(())
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
/// Embed every note lacking an embedding (captures AND imported brain notes), so
/// semantic search / chat can retrieve them. Best-effort, idempotent. Shared by
/// the `reindex` command and the post-sync / periodic background passes.
async fn embed_missing(app: &tauri::AppHandle) -> i64 {
    let todo = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        db::notes_missing_embeddings(&conn).unwrap_or_default()
    };
    let mut n = 0;
    for (id, text) in todo {
        if let Ok(v) = ollama::embed(&text).await {
            let v = normalize(v);
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            if db::insert_embedding(&conn, id, &v).is_ok() {
                n += 1;
            }
        }
    }
    n
}

#[tauri::command]
async fn reindex(app: tauri::AppHandle) -> Result<i64, String> {
    Ok(embed_missing(&app).await)
}

/// Backfill people from past notes: re-derive person entities from every entry's
/// data fields (the `person_names_from_data` heuristic) and link them to their
/// note, so people the model omitted from the `entities` array appear
/// retroactively. Idempotent — guarded so re-running adds no duplicate mentions.
/// Returns the number of mentions added. Modeled on `reindex`.
#[tauri::command]
async fn backfill_entities(app: tauri::AppHandle) -> Result<i64, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let rows = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        db::all_entry_data(&conn).map_err(|e| e.to_string())?
    };
    let mut added = 0;
    for (note_id, event_date, raw, data) in rows {
        let people = entities::person_names_from_data(&data);
        if people.is_empty() {
            continue;
        }
        let candidates: Vec<EntityCandidate> = people
            .into_iter()
            .map(|name| EntityCandidate { name, etype: "person".to_string(), fact: None, relationship: None })
            .collect();
        let snippet: String = raw.chars().take(200).collect();
        added += persist_entities(&app, note_id, &event_date, &snippet, &now, candidates, true).await;
    }
    Ok(added)
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
    let ts = now_eastern().format("%Y%m%d-%H%M%S");
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

    // The user's custom vocabulary applies to all speech-to-text, not just
    // meetings — quick captures mishear "a16z" the same way.
    let vocab = meeting::cfg().vocabulary;
    let hint = meeting::asr::vocab_hint(&[], &vocab);

    // whisper is CPU/Metal-bound and blocking; run off the async runtime.
    tauri::async_runtime::spawn_blocking(move || {
        voice::transcribe(&model, &samples, hint.as_deref())
            .map(|t| meeting::asr::apply_vocab(&t, &vocab))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Meetings (local Granola: record mic + system audio → live whisper →
// template summarize). Capture commands are desktop-only; reads work anywhere.
// ---------------------------------------------------------------------------

const MEETING_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin";

#[tauri::command]
fn meeting_model_status(app: tauri::AppHandle) -> Value {
    let dir = app
        .path()
        .app_data_dir()
        .map(|d| d.join("models"))
        .unwrap_or_default();
    json!({
        "turbo": dir.join("ggml-large-v3-turbo.bin").exists(),
        "base": dir.join("ggml-base.en.bin").exists(),
        "speaker": dir.join(meeting::diarize::MODEL_FILE).exists(),
        "parakeet": meeting::parakeet_ready(&app),
        "tap_supported": meeting::capture::tap_supported(),
    })
}

/// Download the meeting model (large-v3-turbo, ~1.6GB) — streamed to disk.
#[tauri::command]
async fn download_meeting_model(app: tauri::AppHandle) -> Result<bool, String> {
    use std::io::Write;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("ggml-large-v3-turbo.bin");
    if path.exists() {
        return Ok(true);
    }
    let tmp = dir.join("ggml-large-v3-turbo.bin.part");
    let mut resp = reqwest::get(MEETING_MODEL_URL).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("model download failed: {}", resp.status()));
    }
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        file.write_all(&chunk).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Parakeet-TDT 0.6B v2 int8 (sherpa-onnx export, ~660MB across four files) —
/// the faster, proper-noun-stronger ASR engine. Files are fetched one by one
/// (no archive handling); already-present files are skipped so a failed
/// download resumes where it stopped.
const PARAKEET_BASE_URL: &str =
    "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main";

#[tauri::command]
async fn download_parakeet_model(app: tauri::AppHandle) -> Result<bool, String> {
    use std::io::Write;
    let dir = meeting::parakeet_dir(&app).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for file in meeting::PARAKEET_FILES {
        let path = dir.join(file);
        if path.exists() {
            continue;
        }
        let tmp = dir.join(format!("{file}.part"));
        let mut resp = reqwest::get(format!("{PARAKEET_BASE_URL}/{file}"))
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("parakeet download failed ({file}): {}", resp.status()));
        }
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            out.write_all(&chunk).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        drop(out);
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

/// WeSpeaker CAM++ voice-embedding model (~29MB) — powers per-speaker labels
/// on the system-audio channel. Same explicit-download pattern as whisper.
const SPEAKER_MODEL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_CAM%2B%2B_LM.onnx";

#[tauri::command]
async fn download_speaker_model(app: tauri::AppHandle) -> Result<bool, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(meeting::diarize::MODEL_FILE);
    if path.exists() {
        return Ok(true);
    }
    let bytes = reqwest::get(SPEAKER_MODEL_URL)
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
async fn meeting_start(
    app: tauri::AppHandle,
    title: Option<String>,
    event_id: Option<String>,
    event_json: Option<Value>,
    retain_audio: Option<bool>,
    source_bundle: Option<String>,
) -> Result<i64, String> {
    let title = title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "Meeting".to_string());
    if let Some(ref eid) = event_id {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        if let Some(id) = meeting::store::find_meeting_by_event(&conn, eid)
            .map_err(|e| e.to_string())?
        {
            return Ok(id);
        }
    }
    let retain = retain_audio.unwrap_or_else(|| meeting::cfg().retain_audio);
    meeting::start(&app, title, event_id, event_json, retain, source_bundle)
        .map_err(|e| e.to_string())
}

/// What the record-prompt window should show (it fetches on mount — a fresh
/// webview can't catch an event emitted before it loaded).
#[tauri::command]
fn meeting_prompt_payload(app: tauri::AppHandle) -> Value {
    let pending = app.state::<meeting::detect::PendingPrompt>();
    let v = pending.0.lock().unwrap().clone();
    v.unwrap_or(Value::Null)
}

/// "Not now": close the prompt; the dismissed app stays quiet for 10 minutes.
#[tauri::command]
fn meeting_dismiss_prompt(app: tauri::AppHandle, bundle_id: Option<String>) {
    if let Some(b) = bundle_id {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let state = app.state::<meeting::detect::DetectState>();
        state.0.lock().unwrap().insert(b, now);
    }
    meeting::detect::close_prompt(&app);
}

#[tauri::command]
fn meetings_settings_get() -> Value {
    serde_json::to_value(meeting::cfg()).unwrap_or(Value::Null)
}

#[tauri::command]
fn meetings_settings_set(app: tauri::AppHandle, settings: meeting::MeetingsCfg) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    meeting::cfg_update(&dir, settings).map_err(|e| e.to_string())
}

/// Match the native window chrome to the in-app theme: dark themes get the
/// deep HUD glass, light themes the standard (light) sidebar material — and
/// the window's NSAppearance follows, so the vibrancy renders correctly.
#[tauri::command]
fn set_chrome_theme(app: tauri::AppHandle, dark: bool) {
    #[cfg(target_os = "macos")]
    if let Some(win) = app.get_webview_window("main") {
        use window_vibrancy::{apply_vibrancy, clear_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        let _ = win.set_theme(Some(if dark { tauri::Theme::Dark } else { tauri::Theme::Light }));
        let _ = clear_vibrancy(&win);
        let material = if dark {
            NSVisualEffectMaterial::HudWindow
        } else {
            NSVisualEffectMaterial::Sidebar
        };
        if let Err(e) = apply_vibrancy(
            &win,
            material,
            Some(NSVisualEffectState::FollowsWindowActiveState),
            None,
        ) {
            eprintln!("[noted] vibrancy update failed: {e}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, dark);
}

#[tauri::command]
async fn meeting_stop(app: tauri::AppHandle) -> Result<Option<i64>, String> {
    meeting::stop(app).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn meeting_state(app: tauri::AppHandle) -> Value {
    meeting::state_json(&app)
}

#[tauri::command]
async fn meeting_list(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let rows = meeting::store::list_meetings(&conn, 200).map_err(|e| e.to_string())?;
    Ok(json!(rows))
}

#[tauri::command]
async fn meeting_get(app: tauri::AppHandle, id: i64) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    meeting::store::get_meeting(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn meeting_set_notes(app: tauri::AppHandle, id: i64, notes: String) -> Result<(), String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    meeting::store::set_notes(&conn, id, &notes).map_err(|e| e.to_string())
}

/// Rename a diarized voice ("Speaker 2" → "Mayan"): relabels the transcript
/// and updates the persistent voiceprint so future meetings auto-label them.
#[tauri::command]
async fn meeting_rename_speaker(
    app: tauri::AppHandle,
    id: i64,
    from: String,
    to: String,
) -> Result<(), String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    meeting::store::rename_speaker(&conn, id, &from, &to).map_err(|e| e.to_string())
}

/// On-demand speaker-name suggestions (stop-time runs this automatically).
#[tauri::command]
async fn meeting_suggest_speakers(app: tauri::AppHandle, id: i64) -> Result<usize, String> {
    meeting::summarize::suggest_speaker_names(&app, id)
        .await
        .map_err(|e| e.to_string())
}

/// Delete a meeting's window video now (retention would get it eventually;
/// this is the "free the space today" button).
#[tauri::command]
async fn meeting_video_delete(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let path: Option<String> = conn
        .query_row("SELECT video_path FROM meetings WHERE id = ?1", [id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if let Some(p) = path {
        meeting::video::delete_video(&conn, id, &p);
    }
    Ok(())
}

/// Rebuild a meeting's speaker labels from its retained audio — heals
/// meetings recorded before diarization existed or interrupted by a crash.
/// Returns the voice count (0 = nothing to do / already diarized).
#[tauri::command]
async fn meeting_rediarize(app: tauri::AppHandle, id: i64) -> Result<usize, String> {
    let h = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(model) = meeting::diarize::model_path(&h) else {
            return Err("speaker model not downloaded".to_string());
        };
        let dir = h
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("meetings")
            .join(id.to_string());
        let state = h.state::<Db>();
        let conn = state.0.lock().unwrap();
        meeting::asr::rediarize_from_wav(&conn, Some(&h), &model, &dir, id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Where a meeting export lands: ~/Documents/Notes/Meeting/<title>/<date title>.<ext>,
/// deduped with " (n)" so recurring meetings collect in one folder per title.
fn meeting_export_path(
    app: &tauri::AppHandle,
    meeting: &serde_json::Value,
    ext: &str,
) -> Result<std::path::PathBuf, String> {
    let title: String = meeting["title"]
        .as_str()
        .unwrap_or("Meeting")
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let title = title.trim().to_string();
    let folder = if title.is_empty() { "Meeting" } else { title.as_str() };
    let dir = app
        .path()
        .document_dir()
        .map_err(|e| e.to_string())?
        .join("Notes")
        .join("Meeting")
        .join(folder);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let date: String = meeting["started_at"]
        .as_str()
        .map(|s| s.chars().take(10).collect())
        .unwrap_or_default();
    let base = if date.is_empty() { title } else { format!("{date} {title}") };
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    Ok(path)
}

/// Export the whole meeting (summaries + notes + labeled transcript) as one
/// Markdown file under ~/Documents/Notes/Meeting; returns the written path.
#[tauri::command]
async fn meeting_export_md(app: tauri::AppHandle, id: i64) -> Result<String, String> {
    let meeting = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        meeting::store::get_meeting(&conn, id).map_err(|e| e.to_string())?
    };
    let md = meeting::summarize::export_markdown(&meeting);
    let path = meeting_export_path(&app, &meeting, "md")?;
    std::fs::write(&path, md).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// Render a polished, self-contained meeting PDF under ~/Documents/Notes/Meeting.
#[tauri::command]
async fn meeting_export_pdf(app: tauri::AppHandle, id: i64) -> Result<String, String> {
    let meeting = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        meeting::store::get_meeting(&conn, id).map_err(|e| e.to_string())?
    };
    let path = meeting_export_path(&app, &meeting, "pdf")?;
    meeting::pdf::export(&meeting, &path).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// Generate a summary tab with the given (or default) template. Each template
/// has one refreshable tab; the first summary also files the meeting note.
#[tauri::command]
async fn meeting_summarize(
    app: tauri::AppHandle,
    id: i64,
    template: Option<String>,
) -> Result<String, String> {
    meeting::summarize::run(&app, id, template)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn meeting_templates(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let rows = meeting::store::list_templates(&conn).map_err(|e| e.to_string())?;
    Ok(json!(rows))
}

#[tauri::command]
async fn meeting_template_save(
    app: tauri::AppHandle,
    name: String,
    prompt: String,
) -> Result<(), String> {
    if name.trim().is_empty() || prompt.trim().is_empty() {
        return Err("template needs a name and a prompt".into());
    }
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    meeting::store::save_template(&conn, name.trim(), prompt.trim()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn meeting_template_delete(app: tauri::AppHandle, name: String) -> Result<bool, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    meeting::store::delete_template(&conn, &name).map_err(|e| e.to_string())
}

/// Phase-0 spike: record N seconds of system-audio tap + mic to WAVs so the
/// permission flow and capture path can be verified end to end.
#[tauri::command]
async fn meeting_capture_probe(app: tauri::AppHandle, seconds: Option<u64>) -> Result<Value, String> {
    use std::sync::atomic::Ordering;
    let secs = seconds.unwrap_or(10).clamp(2, 30);
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("probe");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let me = meeting::capture::ChannelBuf::new();
    let them = meeting::capture::ChannelBuf::new();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut threads = Vec::new();
    if meeting::capture::tap_supported() {
        let (b, s) = (them.clone(), stop.clone());
        let log = Some(dir.join("probe-capture.log"));
        threads.push(std::thread::spawn(move || meeting::capture::run_system_tap(b, s, log)));
    }
    {
        let (b, s) = (me.clone(), stop.clone());
        let aec = meeting::cfg().mic_aec;
        threads.push(std::thread::spawn(move || meeting::capture::run_mic(b, s, aec)));
    }
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    stop.store(true, Ordering::Relaxed);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        for t in threads {
            let _ = t.join();
        }
    })
    .await;
    joined.map_err(|e| e.to_string())?;

    let mut report = serde_json::Map::new();
    for (name, buf) in [("me", &me), ("them", &them)] {
        let (raw, rate) = buf.drain();
        let pcm = if rate == 0 { Vec::new() } else { voice::resample_to_16k(&raw, rate) };
        let rms = if pcm.is_empty() {
            0.0
        } else {
            (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32).sqrt()
        };
        let path = dir.join(format!("probe-{name}.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
        for s in &pcm {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
                .map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())?;
        report.insert(
            name.to_string(),
            json!({
                "path": path.to_string_lossy(),
                "seconds": pcm.len() as f32 / 16_000.0,
                "duration_ratio": pcm.len() as f32 / 16_000.0 / secs as f32,
                "native_rate": rate,
                "rms": rms,
            }),
        );
    }
    report.insert("tap_supported".into(), json!(meeting::capture::tap_supported()));
    Ok(Value::Object(report))
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

/// Same-type entity pairs that look like duplicates, for the People view's
/// "possible duplicates" panel. The capture-time resolver treats ≥0.86 as the
/// same entity going forward; this retro pass casts slightly wider (0.82) to
/// surface near-misses already sitting in the catalog. Dismissed pairs stay
/// out (dismissed_merges).
#[tauri::command]
async fn suggest_entity_merges(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let rows = db::suggest_merges(&conn, 0.82, 20).map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

/// "Not the same" — persist the rejection so the pair is never re-suggested.
#[tauri::command]
async fn dismiss_merge_suggestion(app: tauri::AppHandle, a: i64, b: i64) -> Result<(), String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    db::dismiss_merge(&conn, a, b).map_err(|e| e.to_string())
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

/// Full profile for ANY entity (person/place/topic/…): header fields + the
/// complete, uncapped mention timeline — backs the per-entity page.
#[tauri::command]
async fn entity_profile(app: tauri::AppHandle, entity_id: i64) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let p = db::entity_profile(&conn, entity_id).map_err(|e| e.to_string())?;
    serde_json::to_value(p).map_err(|e| e.to_string())
}

/// People view: every `person` entity with its dated, curated-fact mentions.
#[tauri::command]
async fn list_people(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let people = db::person_profiles(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(people).map_err(|e| e.to_string())
}

/// Title-case an email localpart into a name guess: "edison.lee" -> "Edison Lee".
/// The floor the name-suggestion pass never does worse than.
fn name_from_localpart(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    local
        .split(|c: char| c == '.' || c == '_' || c == '-' || c == '+' || c.is_ascii_digit())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Propose display names for person entities still named by a raw email.
/// Deterministic first: a calendar attendee pairing that email with a real
/// display name wins outright. The local model refines the leftovers (email +
/// known speaker names as context) over a localpart-derived floor. Suggestions
/// are stored for the user to confirm — this never renames anything by itself.
#[tauri::command]
async fn suggest_person_names(app: tauri::AppHandle) -> Result<Value, String> {
    // Pool + evidence under one lock.
    let (pool, pairs, known) = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let pool = db::email_named_people(&conn).map_err(|e| e.to_string())?;
        if pool.is_empty() {
            return Ok(json!(0));
        }
        let mut pairs: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for m in meeting::store::list_meetings(&conn, 500).unwrap_or_default() {
            if let Some(atts) = m["event_json"]["attendees"].as_array() {
                for a in atts {
                    let email = a["email"].as_str().unwrap_or("").to_lowercase();
                    let name = a["name"].as_str().unwrap_or("");
                    if !email.is_empty() && !name.is_empty() && !name.contains('@') {
                        pairs.entry(email).or_insert_with(|| name.to_string());
                    }
                }
            }
        }
        let known: Vec<String> = meeting::store::speaker_profiles(&conn)
            .map(|v| v.into_iter().map(|(n, _, _)| n).collect())
            .unwrap_or_default();
        (pool, pairs, known)
    };

    // Start every unresolved email at its localpart floor; the model may refine.
    let mut proposed: Vec<(i64, String, String)> = Vec::new(); // (id, email, name)
    let mut ask_model: Vec<(i64, String)> = Vec::new();
    for (id, email) in &pool {
        match pairs.get(&email.to_lowercase()) {
            Some(name) => proposed.push((*id, email.clone(), name.clone())),
            None => ask_model.push((*id, email.clone())),
        }
    }
    if !ask_model.is_empty() {
        let emails: Vec<String> = ask_model.iter().map(|(_, e)| e.clone()).collect();
        let schema = json!({
            "type": "object",
            "properties": { "suggestions": { "type": "array", "items": {
                "type": "object",
                "properties": { "email": { "type": "string" }, "name": { "type": "string" } },
                "required": ["email", "name"]
            }}},
            "required": ["suggestions"]
        });
        let system = "You match email addresses to people's display names. For each email, \
            propose the person's likely display name. If a known name clearly corresponds to \
            the email's local part (e.g. jasmin@x.com and a known 'Jasmine'), use that known \
            name; otherwise title-case the local part into a plausible name. Names only — \
            never return an email address. JSON: {\"suggestions\":[{\"email\",\"name\"}]}";
        let user = format!("Emails:\n{}\n\nKnown people from meetings: {}",
            emails.join("\n"),
            if known.is_empty() { "(none)".to_string() } else { known.join(", ") });
        let mut by_email: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        if let Ok(v) = ollama::chat_json_local(&ollama::text_model(), system, &user, None, Some(schema)).await {
            if let Some(arr) = v["suggestions"].as_array() {
                for s in arr {
                    let (e, n) = (s["email"].as_str().unwrap_or(""), s["name"].as_str().unwrap_or("").trim());
                    if !e.is_empty() && !n.is_empty() && !n.contains('@') && n.len() <= 40 {
                        by_email.insert(e.to_lowercase(), n.to_string());
                    }
                }
            }
        }
        for (id, email) in ask_model {
            let name = by_email
                .get(&email.to_lowercase())
                .cloned()
                .unwrap_or_else(|| name_from_localpart(&email));
            if !name.is_empty() {
                proposed.push((id, email, name));
            }
        }
    }

    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let mut applied = 0i64;
    for (id, _, name) in &proposed {
        if db::set_suggested_name(&conn, *id, Some(name)).is_ok() {
            applied += 1;
        }
    }
    Ok(json!(applied))
}

/// The user confirmed (or typed) a display name for a person entity. Rename it,
/// folding the old name (usually an email) into the aliases so future filings
/// still resolve to the same person; if another person already owns that name,
/// merge the two first. Voiceprints are untouched — this is KG-side identity.
#[tauri::command]
async fn confirm_person_name(app: tauri::AppHandle, entity_id: i64, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.contains('@') {
        return Err("Enter a display name (not an email).".into());
    }
    let norm = entities::normalize(&name);
    if norm.is_empty() {
        return Err("Enter a display name.".into());
    }
    // Embed off-lock so the rename re-indexes the entity under one lock.
    let emb = entities::embed_entity(&name, "person").await.ok();
    let state = app.state::<Db>();
    let mut conn = state.0.lock().unwrap();
    if let Ok(Some(other)) = db::entity_exact(&conn, &norm, "person") {
        if other != entity_id {
            db::merge_entities(&mut conn, entity_id, other).map_err(|e| e.to_string())?;
        }
    }
    db::rename_entity(&conn, entity_id, &name, &norm).map_err(|e| e.to_string())?;
    if let Some(v) = emb {
        let _ = db::insert_entity_embedding(&conn, entity_id, &v);
    }
    Ok(())
}

/// "Don't call them that" — clear a pending name suggestion.
#[tauri::command]
async fn dismiss_person_name(app: tauri::AppHandle, entity_id: i64) -> Result<(), String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    db::set_suggested_name(&conn, entity_id, None).map_err(|e| e.to_string())
}

/// Rebuild the meeting-fed knowledge layer: run the knowledge-extraction pass
/// over every meeting with a filed note (mention guard keeps it idempotent),
/// then propose display names for email-named people.
#[tauri::command]
async fn kg_reindex_meetings(app: tauri::AppHandle) -> Result<Value, String> {
    let ids: Vec<i64> = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        meeting::store::list_meetings(&conn, 1000)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|m| m["note_id"].as_i64().is_some())
            .filter_map(|m| m["id"].as_i64())
            .collect()
    };
    let (mut meetings_done, mut mentions) = (0i64, 0i64);
    for id in ids {
        match meeting::summarize::extract_knowledge(&app, id).await {
            Ok(n) => {
                meetings_done += 1;
                mentions += n as i64;
            }
            Err(e) => eprintln!("kg_reindex_meetings: meeting {id}: {e}"),
        }
    }
    let suggestions = suggest_person_names(app.clone())
        .await
        .ok()
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    Ok(json!({ "meetings": meetings_done, "mentions": mentions, "name_suggestions": suggestions }))
}

// ---------------------------------------------------------------------------
// Brain sync (Phase 1: Obsidian -> noted, read path). Mirror brain vault files
// into the KG: each file becomes a brain-origin note, its subject an entity, its
// [[wikilinks]] mentions (= co-mention edges). Idempotent via content hash.
// ---------------------------------------------------------------------------

/// People auto-merge into a near-duplicate above this cosine sim (e.g. "Yi" the
/// capture and "yi" the brain note). Higher than the suggest threshold (0.86) so
/// only high-confidence same-person matches fold together automatically.
const BRAIN_AUTO_MERGE_SIM: f32 = 0.92;

#[derive(Default, serde::Serialize)]
struct BrainSyncReport {
    vault: String,
    scanned: usize,
    imported: usize,   // notes (re)processed this run
    unchanged: usize,  // skipped — content hash matched
    entities_created: usize,
    mentions_added: usize,
    errors: Vec<String>,
}

/// Resolve a brain entity by its vault-scoped norm, creating + embedding it when
/// new. People (only) auto-merge into a very close same-type neighbor so the same
/// person unifies across vaults and captures; artifacts dedup by exact norm only
/// (identical embed text across vaults would otherwise wrongly merge them).
/// Returns (entity_id, created_now). Never holds the DB lock across an await.
async fn resolve_or_create_brain_entity(
    app: &tauri::AppHandle,
    vault: &str,
    etype: &str,
    display_name: &str,
    aliases: &[String],
    event_date: &str,
    now: &str,
) -> Option<(i64, bool)> {
    let norm = brain::vault_norm(vault, etype, display_name);
    // Fast path: exact norm/alias hit needs no embedding (keeps re-syncs cheap).
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        if let Ok(Some(id)) = db::entity_exact(&conn, &norm, etype) {
            return Some((id, false));
        }
    }
    // Embed off-lock to index a new entity (and, for people, find a near match).
    let emb = entities::embed_entity(display_name, etype).await.ok()?;
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    if let Ok(Some(id)) = db::entity_exact(&conn, &norm, etype) {
        return Some((id, false));
    }
    if etype == "person" {
        if let Ok(Some((id, dist))) = db::nearest_entity(&conn, &emb, etype) {
            let sim = 1.0 - dist * dist / 2.0;
            if sim >= BRAIN_AUTO_MERGE_SIM {
                return Some((id, false));
            }
        }
    }
    let aliases_json = serde_json::to_string(aliases).unwrap_or_else(|_| "[]".into());
    match db::create_entity(&conn, display_name, &norm, etype, &aliases_json, event_date, now) {
        Ok(id) => {
            let _ = db::insert_entity_embedding(&conn, id, &emb);
            Some((id, true))
        }
        Err(_) => None,
    }
}

/// Sync one vault into the KG. Two passes: parse every file first (so wikilinks
/// can resolve to any note's type/name), then mirror each changed file.
async fn sync_brain_vault(app: &tauri::AppHandle, vault: &str, root: &std::path::Path) -> BrainSyncReport {
    let mut report = BrainSyncReport { vault: vault.to_string(), ..Default::default() };
    let origin = format!("brain:{vault}");
    let now = chrono::Utc::now().to_rfc3339();
    let today = today_local();

    let files = brain::collect_markdown_files(root);
    report.scanned = files.len();
    let parsed: Vec<(String, brain::ParsedNote)> = files
        .into_iter()
        .map(|(rel, raw)| {
            let p = brain::parse_note(vault, &rel, &raw);
            (raw, p)
        })
        .collect();
    // slug -> (type, display name) for resolving [[wikilink]] targets.
    let mut home: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    for (_, p) in &parsed {
        home.entry(p.slug.clone()).or_insert((p.etype.clone(), p.display_name.clone()));
    }

    for (raw, p) in &parsed {
        let event_date = p.event_date.clone().unwrap_or_else(|| today.clone());

        // Change detection + note upsert (unchanged files cost one hash compare).
        let note_id = {
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            let prev = db::brain_note_hash(&conn, &origin, &p.rel_path).ok().flatten();
            if prev.as_deref() == Some(p.hash.as_str()) {
                report.unchanged += 1;
                continue;
            }
            match db::upsert_brain_note(&conn, &origin, &p.rel_path, raw, &p.hash, &now) {
                Ok(id) => id,
                Err(e) => {
                    report.errors.push(format!("{}: {e}", p.rel_path));
                    continue;
                }
            }
        };
        report.imported += 1;

        // The note's own subject entity.
        let home_id = match resolve_or_create_brain_entity(
            app, vault, &p.etype, &p.display_name, &p.aliases, &event_date, &now,
        )
        .await
        {
            Some((id, created)) => {
                if created {
                    report.entities_created += 1;
                }
                id
            }
            None => {
                report.errors.push(format!("{}: entity embed failed (is Ollama running?)", p.rel_path));
                continue;
            }
        };

        {
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            let _ = db::set_entity_home(&conn, home_id, note_id);
            // Rebuild this note's links from scratch so a removed [[link]] drops its edge.
            let _ = db::clear_note_mentions(&conn, note_id);
            if db::add_mention(&conn, home_id, note_id, None, &p.display_name, &event_date, &now).is_ok() {
                report.mentions_added += 1;
            }
        }

        // Each [[wikilink]] -> a mention of the target in this note (co-mention edge).
        for target in &p.wikilinks {
            let Some((tetype, tname)) = home.get(target).cloned() else {
                continue; // unresolved link (target note not in the vault) — skip
            };
            if let Some((tid, created)) =
                resolve_or_create_brain_entity(app, vault, &tetype, &tname, &[], &event_date, &now).await
            {
                if created {
                    report.entities_created += 1;
                }
                let state = app.state::<Db>();
                let conn = state.0.lock().unwrap();
                if db::add_mention(&conn, tid, note_id, None, &p.display_name, &event_date, &now).is_ok() {
                    report.mentions_added += 1;
                }
            }
        }
    }

    // Record the git checkpoint for future diff-based syncs.
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let sha = brain::git_head(root);
        let _ = db::set_vault_synced(&conn, vault, sha.as_deref(), &now);
    }
    report
}

/// Sync every enabled, on-disk vault. Best-effort; missing roots are skipped.
async fn sync_all_brains(app: &tauri::AppHandle) -> Vec<BrainSyncReport> {
    let vaults = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        db::list_brain_vaults(&conn).unwrap_or_default()
    };
    let mut reports = Vec::new();
    for v in vaults {
        // Export-direction vaults (personal) are noted-canonical — never import
        // them, or we'd re-ingest our own generated notes.
        if !v.enabled || v.direction == "export" {
            continue;
        }
        let root = std::path::PathBuf::from(&v.root_path);
        if root.is_dir() {
            reports.push(sync_brain_vault(app, &v.vault, &root).await);
        }
    }
    reports
}

/// Registered brain vaults + live counts (for Settings / the Work tab).
#[tauri::command]
async fn brain_list_vaults(app: tauri::AppHandle) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let v = db::list_brain_vaults(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// Register a vault by path (its folder name becomes the vault id). `direction`
/// defaults to "import" (the only direction wired in Phase 1).
#[tauri::command]
async fn brain_add_vault(app: tauri::AppHandle, path: String, direction: Option<String>) -> Result<Value, String> {
    let root = std::path::PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    let vault = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("vault")
        .to_lowercase();
    let dir = direction.unwrap_or_else(|| "import".to_string());
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        db::upsert_brain_vault(&conn, &vault, &path, &dir).map_err(|e| e.to_string())?;
    }
    brain_list_vaults(app).await
}

#[tauri::command]
async fn brain_remove_vault(app: tauri::AppHandle, vault: String) -> Result<(), String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    db::remove_brain_vault(&conn, &vault).map_err(|e| e.to_string())
}

/// Sync one vault (by name) or all of them; returns a per-vault report.
#[tauri::command]
async fn brain_sync(app: tauri::AppHandle, vault: Option<String>) -> Result<Value, String> {
    let reports = match vault {
        Some(v) => {
            let root = {
                let state = app.state::<Db>();
                let conn = state.0.lock().unwrap();
                db::list_brain_vaults(&conn)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|x| x.vault == v)
                    .map(|x| x.root_path)
            };
            match root {
                Some(rp) => vec![sync_brain_vault(&app, &v, &std::path::PathBuf::from(rp)).await],
                None => return Err(format!("unknown vault: {v}")),
            }
        }
        None => sync_all_brains(&app).await,
    };
    serde_json::to_value(reports).map_err(|e| e.to_string())
}

/// The Work-tab graph: entities a brain vault touches + their brain co-mention
/// edges. `vault` = None for all vaults combined.
#[tauri::command]
async fn work_graph(app: tauri::AppHandle, vault: Option<String>) -> Result<Value, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    let (nodes, edges) = db::work_graph(&conn, vault.as_deref()).map_err(|e| e.to_string())?;
    Ok(json!({
        "nodes": serde_json::to_value(nodes).map_err(|e| e.to_string())?,
        "edges": serde_json::to_value(edges).map_err(|e| e.to_string())?,
    }))
}

// ── Write-back (Phase 2: noted -> Obsidian) ──────────────────────────────────
// Mirror each brain entity's capture mentions into the managed region of its
// home note. noted writes ONLY between the markers (hand-written prose is never
// touched), updates the mirror row's hash (echo suppression), and commits only
// the files it wrote (the git ledger). `brain_write_preview` is the dry run.

struct PlannedWrite {
    vault: String,
    rel_path: String,
    full_path: std::path::PathBuf,
    entity_name: String,
    note_id: i64,
    before: Option<String>, // current managed-region content
    after: String,          // what it would become
    new_raw: String,        // full file after the rewrite
    new_hash: String,
    changed: bool,
}

/// Map of vault name -> root path.
fn vault_roots(app: &tauri::AppHandle) -> std::collections::HashMap<String, String> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    db::list_brain_vaults(&conn)
        .unwrap_or_default()
        .into_iter()
        .map(|v| (v.vault, v.root_path))
        .collect()
}

/// Compute (without writing) the managed-region rewrite for every brain note
/// whose subject has capture mentions. Reads files; never mutates anything.
fn compute_writes(app: &tauri::AppHandle, vault: Option<&str>) -> Result<Vec<PlannedWrite>, String> {
    let targets = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        db::write_targets(&conn, vault).map_err(|e| e.to_string())?
    };
    let roots = vault_roots(app);
    let mut out = Vec::new();
    for t in targets {
        let v = t.origin.strip_prefix("brain:").unwrap_or("").to_string();
        let Some(root) = roots.get(&v) else { continue };
        let full = std::path::PathBuf::from(root).join(&t.source_path);
        let Ok(raw) = std::fs::read_to_string(&full) else { continue };
        let before = brain::extract_managed(&raw);
        let after = brain::render_managed_block(&t.captures);
        let new_raw = brain::apply_managed(&raw, &after);
        let new_hash = brain::content_hash(&new_raw);
        let after = after.trim().to_string();
        let changed = before.as_deref() != Some(after.as_str());
        out.push(PlannedWrite {
            vault: v,
            rel_path: t.source_path,
            full_path: full,
            entity_name: t.entity_name,
            note_id: t.home_note_id,
            before,
            after,
            new_raw,
            new_hash,
            changed,
        });
    }
    Ok(out)
}

/// Dry run: what write-back would change, per file. Reads only — writes nothing.
#[tauri::command]
async fn brain_write_preview(app: tauri::AppHandle, vault: Option<String>) -> Result<Value, String> {
    let writes = compute_writes(&app, vault.as_deref())?;
    let preview: Vec<Value> = writes
        .iter()
        .filter(|w| w.changed)
        .map(|w| {
            json!({
                "vault": w.vault,
                "path": w.rel_path,
                "entity": w.entity_name,
                "before": w.before,
                "after": w.after,
            })
        })
        .collect();
    Ok(json!(preview))
}

/// Apply write-back: rewrite each changed note's managed region, sync the mirror
/// row (echo suppression), and commit only the touched files per vault.
#[tauri::command]
async fn brain_write_back(app: tauri::AppHandle, vault: Option<String>) -> Result<Value, String> {
    let changed: Vec<PlannedWrite> = compute_writes(&app, vault.as_deref())?
        .into_iter()
        .filter(|w| w.changed)
        .collect();
    let now = chrono::Utc::now().to_rfc3339();

    let mut by_vault: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut files_written = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for w in &changed {
        if let Err(e) = std::fs::write(&w.full_path, &w.new_raw) {
            errors.push(format!("{}: {e}", w.rel_path));
            continue;
        }
        {
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            let _ = db::update_brain_note_content(&conn, w.note_id, &w.new_raw, &w.new_hash, &now);
        }
        files_written += 1;
        by_vault.entry(w.vault.clone()).or_default().push(w.rel_path.clone());
    }

    // Commit only the files noted wrote, per vault — the git ledger.
    let roots = vault_roots(&app);
    let mut commits: Vec<Value> = Vec::new();
    for (v, paths) in &by_vault {
        if let Some(root) = roots.get(v) {
            let msg = format!("noted: sync capture mentions into {} note(s)", paths.len());
            if let Some(sha) = brain::git_commit_paths(std::path::Path::new(root), paths, &msg) {
                commits.push(json!({ "vault": v, "sha": sha, "files": paths.len() }));
            }
        }
    }

    Ok(json!({ "files_written": files_written, "commits": commits, "errors": errors }))
}

// ── Personal-brain export (Phase 3: noted -> ~/Brain/personal) ───────────────
// The personal vault is noted-canonical: export each capture-derived person
// (not already owned by a work vault, seen >= a few times) into people/<slug>.md.
// New files are generated whole; existing files only get their managed region
// updated (so hand-written prose survives). Same git-ledger + dry-run model.

/// Don't export one-off people — only those seen at least this many times.
const PERSONAL_MIN_MENTIONS: i64 = 2;

/// The configured export-direction vault (name, root), if any.
fn personal_vault(app: &tauri::AppHandle) -> Option<(String, String)> {
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    db::list_brain_vaults(&conn)
        .ok()?
        .into_iter()
        .find(|v| v.direction == "export")
        .map(|v| (v.vault, v.root_path))
}

/// Compute (without writing) the person notes export would create/update.
fn compute_personal_writes(app: &tauri::AppHandle) -> Result<Vec<PlannedWrite>, String> {
    let (vault, root) = personal_vault(app).ok_or("no personal (export) vault configured")?;
    let people = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        db::people_for_export(&conn, PERSONAL_MIN_MENTIONS).map_err(|e| e.to_string())?
    };
    let today = today_local();
    let mut out = Vec::new();
    for p in people {
        let slug = brain::slugify(&p.name);
        if slug.is_empty() {
            continue;
        }
        let rel_path = format!("people/{slug}.md");
        let full = std::path::PathBuf::from(&root).join(&rel_path);
        let inner = brain::render_managed_block(&p.mentions);
        let (before, new_raw, changed) = match std::fs::read_to_string(&full) {
            Ok(raw) => {
                let before = brain::extract_managed(&raw);
                let new_raw = brain::apply_managed(&raw, &inner);
                let changed = before.as_deref() != Some(inner.trim());
                (before, new_raw, changed)
            }
            Err(_) => (
                None,
                brain::render_new_person_file(&p.name, &slug, p.relationship.as_deref(), &today, &inner),
                true,
            ),
        };
        out.push(PlannedWrite {
            vault: vault.clone(),
            rel_path,
            full_path: full,
            entity_name: p.name,
            note_id: 0, // export files aren't mirror rows (the vault isn't imported)
            before,
            after: inner.trim().to_string(),
            new_raw,
            new_hash: String::new(),
            changed,
        });
    }
    Ok(out)
}

/// Dry run for personal export — what would be created/updated. Writes nothing.
#[tauri::command]
async fn personal_export_preview(app: tauri::AppHandle) -> Result<Value, String> {
    let writes = compute_personal_writes(&app)?;
    let preview: Vec<Value> = writes
        .iter()
        .filter(|w| w.changed)
        .map(|w| {
            json!({
                "vault": w.vault,
                "path": w.rel_path,
                "entity": w.entity_name,
                "before": w.before,
                "after": w.after,
            })
        })
        .collect();
    Ok(json!(preview))
}

/// Apply personal export: write each person note and commit them as one batch.
#[tauri::command]
async fn personal_export(app: tauri::AppHandle) -> Result<Value, String> {
    let changed: Vec<PlannedWrite> =
        compute_personal_writes(&app)?.into_iter().filter(|w| w.changed).collect();
    let mut files_written = 0usize;
    let mut paths: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for w in &changed {
        if let Some(parent) = w.full_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&w.full_path, &w.new_raw) {
            Ok(()) => {
                files_written += 1;
                paths.push(w.rel_path.clone());
            }
            Err(e) => errors.push(format!("{}: {e}", w.rel_path)),
        }
    }
    let commits = match personal_vault(&app) {
        Some((v, root)) => {
            let msg = format!("noted: export {} person note(s)", paths.len());
            brain::git_commit_paths(std::path::Path::new(&root), &paths, &msg)
                .map(|sha| vec![json!({ "vault": v, "sha": sha, "files": paths.len() })])
                .unwrap_or_default()
        }
        None => vec![],
    };
    Ok(json!({ "files_written": files_written, "commits": commits, "errors": errors }))
}

// ---------------------------------------------------------------------------
// App setup
// ---------------------------------------------------------------------------

// ── Model provider settings (Local / Balanced + Gemini key) ─────────────────
#[tauri::command]
fn get_provider_settings() -> Value {
    let c = provider::get();
    let has = |k: &Option<String>| k.as_deref().map(|k| !k.is_empty()).unwrap_or(false);
    json!({
        "mode": c.mode,
        "cloud_provider": c.cloud_provider,
        "text_model": c.text_model,
        "vision_model": c.vision_model,
        "gemini_text_model": c.gemini_text_model,
        "gemini_vision_model": c.gemini_vision_model,
        "openai_base_url": c.openai_base_url,
        "openai_text_model": c.openai_text_model,
        "openai_vision_model": c.openai_vision_model,
        "anthropic_text_model": c.anthropic_text_model,
        "anthropic_vision_model": c.anthropic_vision_model,
        "has_gemini_key": has(&c.gemini_api_key),
        "has_openai_key": has(&c.openai_api_key),
        "has_anthropic_key": has(&c.anthropic_api_key),
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn set_provider_settings(
    app: tauri::AppHandle,
    mode: String,
    cloud_provider: Option<String>,
    gemini_api_key: Option<String>,
    gemini_text_model: Option<String>,
    gemini_vision_model: Option<String>,
    openai_base_url: Option<String>,
    openai_api_key: Option<String>,
    openai_text_model: Option<String>,
    openai_vision_model: Option<String>,
    anthropic_api_key: Option<String>,
    anthropic_text_model: Option<String>,
    anthropic_vision_model: Option<String>,
    text_model: Option<String>,
    vision_model: Option<String>,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    provider::update(
        &dir,
        provider::SettingsPatch {
            mode: Some(mode),
            cloud_provider,
            text_model,
            vision_model,
            gemini_api_key,
            gemini_text_model,
            gemini_vision_model,
            openai_base_url,
            openai_api_key,
            openai_text_model,
            openai_vision_model,
            anthropic_api_key,
            anthropic_text_model,
            anthropic_vision_model,
        },
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn test_provider() -> Result<String, String> {
    provider::test_cloud().await.map_err(|e| e.to_string())
}

// ── Google Calendar sync (one-way push to a dedicated "noted" calendar) ──────
#[tauri::command]
fn gcal_auth_status() -> Value {
    gcal::auth_status()
}

/// Store the user's OAuth client (Desktop-app id + secret) before connecting.
#[tauri::command]
fn gcal_set_client(app: tauri::AppHandle, client_id: String, client_secret: String) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::save_client(&dir, client_id.trim(), client_secret.trim()).map_err(|e| e.to_string())
}

/// Run the OAuth consent flow (opens the browser, catches the loopback redirect).
#[tauri::command]
async fn gcal_begin_auth(app: tauri::AppHandle) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::begin_auth(&dir).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn gcal_disconnect(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::disconnect(&dir);
    Ok(())
}

/// Clear one day: delete only the events noted pushed for that date from its own
/// calendar. Defaults to today (Eastern). Returns the count deleted. Leaves the
/// calendar, other days, other calendars, and the session untouched.
#[tauri::command]
async fn gcal_clear_day(app: tauri::AppHandle, event_date: Option<String>) -> Result<u32, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let date = event_date.filter(|s| !s.trim().is_empty()).unwrap_or_else(today_local);
    gcal::clear_day(&dir, &date).await.map_err(|e| e.to_string())
}

/// Push a day's schedule to Google Calendar. Defaults to today (Eastern).
#[tauri::command]
async fn gcal_sync(app: tauri::AppHandle, event_date: Option<String>) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let date = event_date.filter(|s| !s.trim().is_empty()).unwrap_or_else(today_local);
    let blocks = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        db::schedule_blocks_for(&conn, &date).map_err(|e| e.to_string())?
    };
    let report = gcal::sync(&dir, &date, blocks).await.map_err(|e| e.to_string())?;
    serde_json::to_value(report).map_err(|e| e.to_string())
}

/// Read a day's events back from every (non-noted) Google calendar, so the Today
/// empty state can show what the user already has planned. Defaults to today.
#[tauri::command]
async fn gcal_list_events(app: tauri::AppHandle, event_date: Option<String>) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let date = event_date.filter(|s| !s.trim().is_empty()).unwrap_or_else(today_local);
    let events = gcal::list_events(&dir, &date).await.map_err(|e| e.to_string())?;
    serde_json::to_value(events).map_err(|e| e.to_string())
}

/// Remove one connected Google account (its refresh token leaves the Keychain;
/// other accounts are untouched). Returns the new auth status.
#[tauri::command]
fn gcal_remove_account(app: tauri::AppHandle, email: String) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::remove_account(&dir, email.trim()).map_err(|e| e.to_string())
}

/// Show/hide one calendar in the Calendar view. Returns the new auth status.
#[tauri::command]
fn gcal_set_calendar_enabled(
    app: tauri::AppHandle,
    account: String,
    calendar_id: String,
    enabled: bool,
) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::set_calendar_enabled(&dir, account.trim(), &calendar_id, enabled).map_err(|e| e.to_string())
}

/// Re-pull every connected account's calendar list (new calendars, renames,
/// color changes). Returns the new auth status.
#[tauri::command]
async fn gcal_refresh_calendars(app: tauri::AppHandle) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::refresh_calendars(&dir).await.map_err(|e| e.to_string())
}

/// Pick which account the daily-schedule push targets. Returns auth status.
#[tauri::command]
fn gcal_set_sync_account(app: tauri::AppHandle, email: String) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::set_sync_account(&dir, email.trim()).map_err(|e| e.to_string())
}

/// Guest-autocomplete pool: attendee emails harvested from calendar events.
#[tauri::command]
fn gcal_contacts() -> Value {
    gcal::contacts()
}

/// Events across every connected account's enabled calendars for the inclusive
/// day range [startDate, endDate] — the Calendar view's feed.
#[tauri::command]
async fn gcal_events_range(
    app: tauri::AppHandle,
    start_date: String,
    end_date: String,
) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let events = gcal::events_range(&dir, start_date.trim(), end_date.trim())
        .await
        .map_err(|e| e.to_string())?;
    serde_json::to_value(events).map_err(|e| e.to_string())
}

/// Create an event on any connected account's calendar. `start`/`end` are
/// "HH:MM"; no start means all-day (endDate = inclusive last day).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn gcal_create_event(
    app: tauri::AppHandle,
    account: String,
    calendar_id: String,
    title: String,
    date: String,
    start: Option<String>,
    end: Option<String>,
    end_date: Option<String>,
    location: Option<String>,
    description: Option<String>,
    add_meet: Option<bool>,
    guests: Option<Vec<String>>,
) -> Result<Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::create_event(
        &dir,
        account.trim(),
        &calendar_id,
        &title,
        date.trim(),
        start.as_deref(),
        end.as_deref(),
        end_date.as_deref(),
        location.as_deref(),
        description.as_deref(),
        add_meet.unwrap_or(false),
        &guests.unwrap_or_default(),
    )
    .await
    .map_err(|e| e.to_string())
}

/// Edit an event in place; `moveTo` relocates it to another calendar in the
/// same account first.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn gcal_update_event(
    app: tauri::AppHandle,
    account: String,
    calendar_id: String,
    event_id: String,
    title: String,
    date: String,
    start: Option<String>,
    end: Option<String>,
    end_date: Option<String>,
    location: Option<String>,
    description: Option<String>,
    move_to: Option<String>,
    meet: Option<bool>,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::update_event(
        &dir,
        account.trim(),
        &calendar_id,
        &event_id,
        &title,
        date.trim(),
        start.as_deref(),
        end.as_deref(),
        end_date.as_deref(),
        location.as_deref(),
        description.as_deref(),
        move_to.as_deref(),
        meet,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn gcal_delete_event(
    app: tauri::AppHandle,
    account: String,
    calendar_id: String,
    event_id: String,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    gcal::remove_event(&dir, account.trim(), &calendar_id, &event_id)
        .await
        .map_err(|e| e.to_string())
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
            // Dark-glass chrome: native vibrancy behind the (transparent)
            // webview — the sidebar region lets it show through. Follows the
            // window's active state, so it flattens when unfocused.
            #[cfg(target_os = "macos")]
            if let Some(win) = app.get_webview_window("main") {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
                if let Err(e) = apply_vibrancy(
                    &win,
                    NSVisualEffectMaterial::HudWindow,
                    Some(NSVisualEffectState::FollowsWindowActiveState),
                    None,
                ) {
                    eprintln!("[noted] vibrancy unavailable: {e}");
                }
            }

            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::init(&dir.join("noted.db"))?;
            app.manage(Db(Mutex::new(conn)));
            // Meeting recorder: one-at-a-time session state + builtin templates
            // + detection (mic-in-use watcher, calendar T-60s prompt, auto-stop).
            app.manage(meeting::MeetingState(Mutex::new(None)));
            app.manage(meeting::detect::PendingPrompt(Mutex::new(None)));
            app.manage(meeting::detect::DetectState(Mutex::new(
                std::collections::HashMap::new(),
            )));
            meeting::cfg_init(&dir);
            {
                let state = app.state::<Db>();
                let conn = state.0.lock().unwrap();
                if let Err(e) = meeting::store::seed_templates(&conn) {
                    eprintln!("[noted] template seed failed: {e}");
                }
            }
            meeting::detect::spawn(app.handle().clone());
            // Recover meetings a previous process left mid-recording.
            meeting::reconcile(&app.handle().clone());
            // Retention sweep: expired meeting window videos free their space.
            meeting::video::cleanup_old(&app.handle().clone(), meeting::cfg().video_keep_days);

            // Load model-provider config (mode + models from disk, key from Keychain).
            provider::init(&dir);
            // Load Google Calendar config (client id + calendar id from disk,
            // client secret + refresh token from Keychain).
            gcal::init(&dir);
            brain::init_auto(&dir); // load the auto-propagation preference

            // Brain vaults: auto-register the default ~/Brain/* vaults (idempotent),
            // then mirror them into the KG in the background so they're up to date
            // shortly after launch (Phase 1 = read-only import; never writes vaults).
            {
                let state = app.state::<Db>();
                let conn = state.0.lock().unwrap();
                for (vault, root) in brain::default_vault_roots() {
                    // The personal vault is noted-canonical (export target); work
                    // vaults are Obsidian-canonical (import source).
                    let dir = if vault == "personal" { "export" } else { "import" };
                    let _ = db::upsert_brain_vault(&conn, &vault, &root.to_string_lossy(), dir);
                }
            }
            let hb = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let reports = sync_all_brains(&hb).await;
                for r in &reports {
                    println!(
                        "[noted] brain '{}': {} files, {} imported, {} entities, {} mentions{}",
                        r.vault,
                        r.scanned,
                        r.imported,
                        r.entities_created,
                        r.mentions_added,
                        if r.errors.is_empty() { String::new() } else { format!(", {} errors", r.errors.len()) },
                    );
                }
                // Embed any notes (incl. freshly imported brain notes) so chat /
                // semantic search can answer questions about them.
                let n = embed_missing(&hb).await;
                if n > 0 {
                    println!("[noted] embedded {n} note(s) for search");
                }
            });
            // Keep the brain current automatically: re-import + re-embed every 10
            // minutes (read-only; write-back/export stay manual + confirmed).
            let hbp = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(600));
                let h = hbp.clone();
                tauri::async_runtime::spawn(async move {
                    sync_all_brains(&h).await;
                    embed_missing(&h).await;
                    // Automated propagation (toggleable in Settings): mirror
                    // captures into work-vault notes and refresh the personal
                    // vault. Each is git-committed and a no-op when nothing
                    // changed, so this is safe to run on a loop.
                    if brain::auto_propagate() {
                        let _ = brain_write_back(h.clone(), None).await;
                        let _ = personal_export(h.clone()).await;
                    }
                });
            });

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
            theme_state,
            theme_list,
            theme_save,
            theme_activate,
            theme_set_color_mode,
            theme_delete,
            theme_compile_design,
            theme_suggest,
            health,
            categorize_note,
            ocr_photo,
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
            backfill_entities,
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
            meeting_model_status,
            download_meeting_model,
            download_speaker_model,
            download_parakeet_model,
            meeting_rename_speaker,
            meeting_suggest_speakers,
            meeting_rediarize,
            meeting_video_delete,
            meeting_export_md,
            meeting_export_pdf,
            meeting_start,
            meeting_stop,
            meeting_state,
            meeting_list,
            meeting_get,
            meeting_set_notes,
            meeting_summarize,
            meeting_templates,
            meeting_template_save,
            meeting_template_delete,
            meeting_capture_probe,
            meeting_prompt_payload,
            meeting_dismiss_prompt,
            meetings_settings_get,
            meetings_settings_set,
            set_chrome_theme,
            list_entities,
            merge_entities,
            suggest_entity_merges,
            dismiss_merge_suggestion,
            entity_graph,
            entity_detail,
            entity_profile,
            list_people,
            suggest_person_names,
            confirm_person_name,
            dismiss_person_name,
            kg_reindex_meetings,
            get_provider_settings,
            set_provider_settings,
            test_provider,
            gcal_auth_status,
            gcal_set_client,
            gcal_begin_auth,
            gcal_disconnect,
            gcal_clear_day,
            gcal_sync,
            gcal_list_events,
            gcal_remove_account,
            gcal_set_calendar_enabled,
            gcal_refresh_calendars,
            gcal_set_sync_account,
            gcal_contacts,
            gcal_events_range,
            gcal_create_event,
            gcal_update_event,
            gcal_delete_event,
            journal_reflect,
            brain_list_vaults,
            brain_add_vault,
            brain_remove_vault,
            brain_sync,
            work_graph,
            brain_write_preview,
            brain_write_back,
            personal_export_preview,
            personal_export,
            related_brain,
            brain_get_auto,
            brain_set_auto,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
