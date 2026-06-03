// The categorize/extract pipeline, factored out of the Tauri command so it can
// be tested headlessly against the real Ollama server. The command in lib.rs is
// a thin wrapper that supplies DB-derived catalog + known category names.

use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};

use crate::db::SearchHit;
use crate::ollama;

/// System prompt for the conversational Q&A assistant.
pub fn qa_system(today: &str) -> String {
    format!(
        "You are the user's personal assistant, answering questions about their own life log. \
Today is {today}. You are given the user's recent and most-relevant entries, each tagged with its \
DATE and CATEGORY. Answer using ONLY these entries. Resolve relative dates yourself (\"yesterday\" \
is the day before today; \"last workout\" is the most recent entry in a workout/gym category; etc.) \
and prefer the most recent matching entry. Be concise and specific — cite the date and the concrete \
numbers from the entry. If the entries don't contain the answer, say you don't have that logged."
    )
}

/// Format retrieved entries into a compact, dated context block for the LLM.
pub fn qa_context(hits: &[SearchHit]) -> String {
    let mut s = String::new();
    for h in hits {
        let data = h.data.as_ref().map(|d| format!(" | {d}")).unwrap_or_default();
        s.push_str(&format!(
            "- {} [{}]: {}{}\n",
            h.event_date,
            h.category.as_deref().unwrap_or("uncategorized"),
            h.raw_text.replace('\n', " "),
            data
        ));
    }
    s
}

/// Best-effort date scrape from the note text itself — a reliable backstop for
/// when the (vision) model fails to populate event_date from a written date like
/// "6/2". Handles ISO (2026-06-02) and US slash dates (6/2, 6/2/26). A slash date
/// with no year that lands in the future is assumed to be last year (you log the
/// past, not the future).
pub fn extract_date_from_text(text: &str, today: &str) -> Option<String> {
    let today_d = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;

    if let Some(m) = regex::Regex::new(r"\b(\d{4})-(\d{2})-(\d{2})\b").unwrap().find(text) {
        if let Ok(d) = NaiveDate::parse_from_str(m.as_str(), "%Y-%m-%d") {
            return Some(d.to_string());
        }
    }

    let re = regex::Regex::new(r"\b(\d{1,2})/(\d{1,2})(?:/(\d{2,4}))?\b").unwrap();
    let c = re.captures(text)?;
    let month: u32 = c[1].parse().ok()?;
    let day: u32 = c[2].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (year, had_year) = match c.get(3) {
        Some(y) => {
            let n: i32 = y.as_str().parse().ok()?;
            (if n < 100 { 2000 + n } else { n }, true)
        }
        None => (today_d.year(), false),
    };
    let mut cand = NaiveDate::from_ymd_opt(year, month, day)?;
    if !had_year && cand > today_d {
        cand = NaiveDate::from_ymd_opt(year - 1, month, day)?;
    }
    Some(cand.to_string())
}

/// Normalize the model's date guess. Returns (YYYY-MM-DD, was_extracted).
/// A valid ISO date from the note is kept; anything missing/unparseable falls
/// back to `today` (so every entry is always dated).
/// Snap a model-returned category onto an existing one when it's really the same
/// (the model sometimes echoes the description, e.g. "gym: workout logs" -> "gym",
/// or pluralizes). Otherwise returns the cleaned name as a genuinely new category.
pub fn snap_category(raw: &str, known: &[String]) -> String {
    let c = raw.trim().to_lowercase();
    if known.iter().any(|k| k == &c) {
        return c;
    }
    // strip a trailing ": description" / " - description" / " (description)"
    let head = c
        .split([':', '-', '—', '(', ','])
        .next()
        .unwrap_or(&c)
        .trim()
        .to_string();
    if known.iter().any(|k| k == &head) {
        return head;
    }
    // tolerate a trailing plural 's' (gyms -> gym, meals -> meal mismatch is fine)
    if let Some(k) = known.iter().find(|k| format!("{k}s") == head || **k == format!("{head}s")) {
        return k.clone();
    }
    head
}

pub fn resolve_date(raw: Option<&str>, today: &str) -> (String, bool) {
    if let Some(s) = raw {
        let s = s.trim();
        if !s.is_empty()
            && !s.eq_ignore_ascii_case("null")
            && NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
        {
            return (s.to_string(), true);
        }
    }
    (today.to_string(), false)
}

/// JSON-schema for the routing reply. Constraining decoding to this shape both
/// prevents degenerate-JSON loops and keeps the top-level keys reliable, while
/// `data` stays a permissive object so emergent structure is unconstrained.
fn routing_schema(with_raw_text: bool) -> Value {
    let mut props = json!({
        "category": { "type": "string" },
        "is_new_category": { "type": "boolean" },
        "description": { "type": "string" },
        "event_date": { "type": ["string", "null"] },
        "data": { "type": "object" }
    });
    let mut required = vec!["category", "is_new_category", "data"];
    if with_raw_text {
        props["raw_text"] = json!({ "type": "string" });
        required.insert(0, "raw_text");
    }
    json!({ "type": "object", "properties": props, "required": required })
}

/// Run a messy note through the local text model and return a normalized
/// proposal: { category, is_new_category, description, data }.
/// `is_new_category` is decided authoritatively from `known_names`, not the model.
pub async fn categorize(
    catalog: &str,
    known_names: &[String],
    text: &str,
    today: &str,
) -> Result<Value> {
    let system = build_categorize_prompt(catalog, today);

    let mut last_err = String::new();
    for attempt in 0..2 {
        let user = if attempt == 0 {
            text.to_string()
        } else {
            format!(
                "{text}\n\n(Your previous reply was invalid: {last_err}. \
                 Return JSON with keys category (string), is_new_category (bool), \
                 description (string), event_date (YYYY-MM-DD or null), data (object). JSON only.)"
            )
        };

        match ollama::chat_json(ollama::TEXT_MODEL, &system, &user, None, Some(routing_schema(false))).await {
            Ok(v) => {
                let raw_date = v.get("event_date").and_then(|d| d.as_str()).map(String::from);
                match validate_proposal(v) {
                    Ok(mut proposal) => {
                        let cat = snap_category(proposal["category"].as_str().unwrap_or(""), known_names);
                        proposal["category"] = json!(cat);
                        proposal["is_new_category"] = json!(!known_names.contains(&cat));
                        let (mut date, mut extracted) = resolve_date(raw_date.as_deref(), today);
                        if !extracted {
                            if let Some(d) = extract_date_from_text(text, today) {
                                date = d;
                                extracted = true;
                            }
                        }
                        proposal["event_date"] = json!(date);
                        proposal["date_was_extracted"] = json!(extracted);
                        return Ok(proposal);
                    }
                    Err(e) => last_err = e,
                }
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(anyhow!("could not get a valid proposal: {last_err}"))
}

/// One-shot vision path: a photo of a (often handwritten) note goes straight to
/// the vision model, which transcribes it AND routes/extracts in a single call.
/// Returns the same proposal shape plus `raw_text` (the transcription).
pub async fn categorize_photo(
    catalog: &str,
    known_names: &[String],
    image_b64: &str,
    today: &str,
) -> Result<Value> {
    let system = build_photo_prompt(catalog, today);

    let mut last_err = String::new();
    for attempt in 0..2 {
        let user = if attempt == 0 {
            "Transcribe this note, then categorize and extract it.".to_string()
        } else {
            format!(
                "Your previous reply was invalid: {last_err}. Return JSON with keys \
                 raw_text (string), category (string), is_new_category (bool), \
                 description (string), event_date (YYYY-MM-DD or null), data (object). JSON only."
            )
        };

        match ollama::chat_json(
            ollama::VISION_MODEL,
            &system,
            &user,
            Some(vec![image_b64.to_string()]),
            Some(routing_schema(true)),
        )
        .await
        {
            Ok(v) => {
                let raw_text = v
                    .get("raw_text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let raw_date = v.get("event_date").and_then(|d| d.as_str()).map(String::from);
                match validate_proposal(v) {
                    Ok(mut proposal) => {
                        let cat = snap_category(proposal["category"].as_str().unwrap_or(""), known_names);
                        proposal["category"] = json!(cat);
                        proposal["is_new_category"] = json!(!known_names.contains(&cat));
                        proposal["raw_text"] = json!(raw_text);
                        let (mut date, mut extracted) = resolve_date(raw_date.as_deref(), today);
                        // Vision often misses a handwritten date; scrape the transcription.
                        if !extracted {
                            if let Some(d) = extract_date_from_text(&raw_text, today) {
                                date = d;
                                extracted = true;
                            }
                        }
                        proposal["event_date"] = json!(date);
                        proposal["date_was_extracted"] = json!(extracted);
                        return Ok(proposal);
                    }
                    Err(e) => last_err = e,
                }
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(anyhow!("could not read the photo: {last_err}"))
}

fn build_photo_prompt(catalog: &str, today: &str) -> String {
    format!(
        "You are the classifier for \"noted\", a personal life-logging app. \
Today's date is {today}. \
You are given a PHOTO of a note — often messy handwriting (a gym log, a daily schedule, meals). \
Return ONE JSON object with these steps:\n\
1) raw_text: transcribe the note as faithfully as you can, preserving the user's own words and line breaks.\n\
2) category: reuse an existing category name verbatim when the note fits one; only invent a short \
lowercase name when none fit (set is_new_category=true with a one-line description).\n\
3) event_date: the date the note refers to, resolved to YYYY-MM-DD using today's date for relative \
or partial dates (e.g. \"6/2\" -> the closest such date, \"yesterday\" -> today minus one). If the \
note shows no date at all, use null.\n\
4) data: extract the structured content.\n\n\
Existing categories (reuse if the note fits one):\n{catalog}\n\n\
Rules:\n\
- When reusing a category, match its existing shape so data stays consistent over time.\n\
- Structure repeated things as an ARRAY OF OBJECTS — one object per item, each keeping its own \
attributes together. NEVER use parallel arrays. \
e.g. \"exercises\": [{{\"name\": \"squat\", \"weight\": 245, \"sets\": 3, \"reps\": 5, \"rpe\": 9}}, ...].\n\
- Numbers as numbers, not strings. Omit fields you can't read rather than guessing.\n\
- Keep keys short, lowercase, snake_case.\n\
- Feelings matter: whenever the note says how the user felt, add a \"mood\" key inside data (a short \
phrase, e.g. \"satisfied\", \"anxious and unfocused\") — in ANY category.\n\
- If a note is mostly about feelings or mental state with no concrete activity, use category \"mood\".\n\
- event_date is the calendar day only; intra-day times (e.g. a 2-4pm block) belong inside data.\n\n\
Return JSON only, exactly this form:\n\
{{\"raw_text\": string, \"category\": string, \"is_new_category\": bool, \"description\": string, \"event_date\": string|null, \"data\": object}}"
    )
}

pub fn build_categorize_prompt(catalog: &str, today: &str) -> String {
    format!(
        "You are the classifier for \"noted\", a personal life-logging app. \
Today's date is {today}. \
The user dumps a messy note (a workout, a daily schedule, meals, anything). \
Do these and return ONE JSON object.\n\n\
1) Pick the single best CATEGORY. Reuse an existing category name verbatim when the \
note fits one. Only invent a new category when none fit; use a short, lowercase name \
(e.g. \"gym\", \"schedule\", \"meals\") and set is_new_category=true with a one-line description.\n\
2) event_date: the date the note refers to, resolved to YYYY-MM-DD using today's date for relative \
or partial dates (e.g. \"6/2\" -> the closest such date, \"yesterday\" -> today minus one). If the \
note mentions no date at all, use null.\n\
3) EXTRACT the structured data the note contains into \"data\".\n\n\
Existing categories (reuse if the note fits one):\n{catalog}\n\n\
Rules:\n\
- When reusing a category, match its existing shape so data stays consistent over time.\n\
- Structure repeated things as an ARRAY OF OBJECTS — one object per item, each keeping its own \
attributes together. NEVER use parallel arrays (separate name[], weight[], reps[] arrays). \
For example a workout becomes: \"exercises\": [{{\"name\": \"squat\", \"weight\": 245, \"sets\": 3, \
\"reps\": 5, \"rpe\": 9}}, ...]; a day becomes: \"blocks\": [{{\"task\": \"coding\", \"duration_min\": 120}}, ...].\n\
- Numbers as numbers, not strings. Omit fields you don't know rather than guessing.\n\
- Keep keys short, lowercase, snake_case.\n\
- Feelings matter: whenever the note says how the user felt, add a \"mood\" key inside data (a short \
phrase, e.g. \"satisfied\", \"anxious and unfocused\") — in ANY category.\n\
- If a note is mostly about feelings or mental state with no concrete activity, use category \"mood\".\n\
- event_date is the calendar day only; intra-day times belong inside data.\n\n\
Return JSON only, exactly this form:\n\
{{\"category\": string, \"is_new_category\": bool, \"description\": string, \"event_date\": string|null, \"data\": object}}"
    )
}

/// Ensure the model's reply has the keys we need, normalizing types.
pub fn validate_proposal(v: Value) -> std::result::Result<Value, String> {
    let category = v
        .get("category")
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .ok_or("missing 'category'")?;
    let is_new = v.get("is_new_category").and_then(|b| b.as_bool()).unwrap_or(false);
    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    // Coerce rather than fail: a 7B model occasionally returns data as an array
    // or omits it. Preserve a list under a generic key; never crash the pipeline.
    let data = match v.get("data") {
        Some(Value::Object(o)) => Value::Object(o.clone()),
        Some(arr @ Value::Array(_)) => json!({ "items": arr }),
        _ => json!({}),
    };
    Ok(json!({
        "category": category,
        "is_new_category": is_new,
        "description": description,
        "data": data,
    }))
}
