// The categorize/extract pipeline, factored out of the Tauri command so it can
// be tested headlessly against the real Ollama server. The command in lib.rs is
// a thin wrapper that supplies DB-derived catalog + known category names.
//
// A note is split into sections (PROTOCOL.md §2): a `Section header:` routes its
// section to a category deterministically (the model only extracts data), while
// untagged prose is classified. Each section becomes one entry, so a single note
// can fill several categories. The result is the envelope:
//   { raw_text, event_date, date_was_extracted, entries: [ {category, ...} ] }

use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};

use crate::db::SearchHit;
use crate::ollama;

/// System prompt for the conversational Q&A assistant.
pub fn qa_system(today: &str) -> String {
    format!(
        "You are the user's personal assistant, answering questions about their own life log AND \
their reference knowledge — notes, people, projects, and decisions imported from their knowledge \
base (\"brain\"). \
Today is {today}. You are given the user's recent and most-relevant entries, each tagged with its \
DATE (YYYY-MM-DD) and CATEGORY. Answer using ONLY these entries.\n\
- A \"Knowledge graph\" block may precede the entries: people/projects/topics matched from the \
question, each with its connections (\"linked to\") and dated facts from meetings and notes. Use it \
for who/what relationships and timelines; the entries stay the ground truth for details.\n\
- Resolve relative dates yourself: \"yesterday\" is the day before today; \"last workout\" is the \
most recent entry in a gym/workout category; prefer the most recent matching entry.\n\
- Talk about dates the way a person would: say the month and day like \"June 1st\" — NEVER output \
an ISO date like 2026-06-01, and omit the year unless it is a different year than today. If a date \
is today or the day before today, say \"today\" or \"yesterday\" instead of the date.\n\
- When the question asks about one specific day (\"today\", \"tomorrow\", \"yesterday\", or a \
date), answer ONLY from entries dated that exact day — never pad the answer with other days.\n\
- Be concise and specific; cite the concrete numbers. If the entries don't contain the answer, \
say you don't have that logged."
    )
}

/// Detect a chat question that is explicitly about ONE day ("what's my
/// schedule today?") so retrieval can be pinned to that date — the date filter
/// is code, not the model. Returns (YYYY-MM-DD, label). Conservative:
/// cumulative phrasings ("as of today", "so far today") and questions naming
/// more than one day (comparisons) return None and keep broad retrieval.
pub fn day_scope(question: &str, today: &str) -> Option<(String, &'static str)> {
    let today_d = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;
    let mut q = question.to_lowercase();
    // Cumulative idioms mention "today" without asking about the day itself.
    for idiom in ["as of today", "up to today", "until today", "through today", "so far today", "before today"] {
        q = q.replace(idiom, " ");
    }
    let has = |words: &[&str]| {
        words.iter().any(|w| {
            q.match_indices(w).any(|(at, _)| {
                let before_ok = at == 0 || !q.as_bytes()[at - 1].is_ascii_alphanumeric();
                let after = at + w.len();
                let after_ok = after >= q.len() || !q.as_bytes()[after].is_ascii_alphanumeric();
                before_ok && after_ok
            })
        })
    };
    let today_hit = has(&["today", "tonight", "this morning", "this afternoon", "this evening"]);
    let tomorrow_hit = has(&["tomorrow", "tmrw"]);
    let yesterday_hit = has(&["yesterday"]);
    match (today_hit, tomorrow_hit, yesterday_hit) {
        (true, false, false) => Some((today_d.to_string(), "today")),
        (false, true, false) => today_d.succ_opt().map(|d| (d.to_string(), "tomorrow")),
        (false, false, true) => today_d.pred_opt().map(|d| (d.to_string(), "yesterday")),
        _ => None,
    }
}

/// Format retrieved entries into a compact, dated context block for the LLM.
pub fn qa_context(hits: &[SearchHit]) -> String {
    let mut s = String::new();
    for h in hits {
        let data = h.data.as_ref().map(|d| format!(" | {d}")).unwrap_or_default();
        // Cap each note's text so a long imported brain note can't crowd the
        // context window; captures are short so this rarely touches them.
        let mut text = h.raw_text.replace('\n', " ");
        if text.chars().count() > 1200 {
            text = text.chars().take(1200).collect::<String>() + "…";
        }
        s.push_str(&format!(
            "- {} [{}]: {}{}\n",
            h.event_date,
            h.category.as_deref().unwrap_or("uncategorized"),
            text,
            data
        ));
    }
    s
}

/// JSON schema constraining the agent router's decision. Mirrors `routing_schema`.
pub fn agent_router_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["answer", "create_category", "edit_entry", "create_event"] },
            "category": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" }
                }
            },
            "event": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "date": { "type": "string" },
                    "start": { "type": "string" },
                    "end": { "type": "string" },
                    "guests": { "type": "array", "items": { "type": "string" } },
                    "meet": { "type": "boolean" },
                    "summary": { "type": "string" }
                }
            },
            "edit": {
                "type": "object",
                "properties": {
                    "entry_id": { "type": "integer" },
                    "data": { "type": "object" },
                    "summary": { "type": "string" }
                }
            },
            "clarify": { "type": ["string", "null"] }
        },
        "required": ["action"]
    })
}

/// System prompt for the agent router: classify a chat message as a plain answer,
/// a category creation, or an entry edit — and extract the parameters.
pub fn route_system(today: &str) -> String {
    format!(
        "You are the action router for a personal life-logging app's assistant. Today is {today}. \
Read the user's latest message (with the prior conversation for context) and return ONE JSON object \
deciding what to do.\n\
Set \"action\" to one of:\n\
- \"answer\" — the DEFAULT. Use it for any question, lookup, or chitchat about the log. The answer \
text is generated separately; you only set the action.\n\
- \"create_category\" — ONLY when the user explicitly asks to make/add a new category. Fill \
\"category\" with a short, lowercase \"name\" and a one-line \"description\".\n\
- \"edit_entry\" — ONLY when the user clearly asks to correct or change a value in a specific entry \
they logged. You are given candidate entries, each prefixed `entry #<id>`. Choose the ONE they mean \
and fill \"edit\": \"entry_id\" (the number after #), \"data\", and \"summary\" (a short human \
description, e.g. \"squat reps 8 -> 6\"). CRITICAL: \"data\" must be the entry's CURRENT data object \
reproduced IN FULL — copy EVERY field and EVERY element of EVERY array exactly as given, then change \
ONLY the specific value the user asked about. Do not drop, summarize, or omit any other item. E.g. if \
the entry has exercises [squat, bench] and the user fixes squat's reps, return BOTH squat (with the \
new reps) AND bench unchanged.\n\
- \"create_event\" — ONLY when the user asks to schedule, book, or put a meeting/event on their \
calendar. Fill \"event\": \"title\" (short), \"date\" as YYYY-MM-DD (resolve today/tomorrow/weekday \
words from today's date), \"start\" and \"end\" as 24-hour \"HH:MM\" (omit \"end\" if unstated; omit \
both for an all-day event), \"guests\" = email addresses ONLY if the user explicitly provided them \
(never invent emails), \"meet\" = true only if they asked for a video-call link, and \"summary\" \
(one line, e.g. \"standup tomorrow 09:30\"). Never invent a date or time the user didn't imply; if \
the date is missing, set action=\"answer\" and ask for it in \"clarify\".\n\
If the user wants to edit something but you cannot tell which entry, set action=\"answer\" and put a \
short clarifying question in \"clarify\".\n\
Be conservative: if you are unsure whether they want an action at all, choose \"answer\". Never invent \
an entry_id that is not in the candidates. Return JSON only."
    )
}

/// Format candidate entries (with their row ids) for the router so it can target
/// a specific entry to edit.
pub fn agent_context(entries: &[crate::db::EntryRow]) -> String {
    let mut s = String::new();
    for e in entries {
        s.push_str(&format!(
            "entry #{} {} [{}]: {}\n",
            e.entry_id,
            e.event_date,
            e.category.as_deref().unwrap_or("uncategorized"),
            e.data
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

/// Normalize the model's date guess. Returns (YYYY-MM-DD, was_extracted).
/// A valid ISO date is kept; anything missing/unparseable yields (today, false).
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

/// JSON-schema for a single segment's extraction reply. Constraining decoding to
/// this shape prevents degenerate-JSON loops and keeps the top-level keys
/// reliable, while `data` stays a permissive object so emergent structure is
/// unconstrained.
fn routing_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "category": { "type": "string" },
            "is_new_category": { "type": "boolean" },
            "description": { "type": "string" },
            "event_date": { "type": ["string", "null"] },
            "data": { "type": "object" },
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
        "required": ["category", "is_new_category", "data"]
    })
}

/// Pull the entity candidates {name, type} out of a model reply (best effort).
fn parse_entities(v: &Value) -> Vec<Value> {
    v.get("entities")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let name = e.get("name").and_then(|n| n.as_str())?.trim();
                    let etype = e.get("type").and_then(|t| t.as_str())?.trim().to_lowercase();
                    if name.is_empty() || etype.is_empty() {
                        return None;
                    }
                    let mut out = json!({ "name": name, "type": etype });
                    // Curated person details (best effort): a short fact about them in
                    // this note and their stated relationship to the author.
                    if let Some(fact) = e.get("fact").and_then(|f| f.as_str()).map(str::trim) {
                        if !fact.is_empty() {
                            out["fact"] = json!(fact);
                        }
                    }
                    if let Some(rel) = e.get("relationship").and_then(|r| r.as_str()).map(str::trim) {
                        if !rel.is_empty() {
                            out["relationship"] = json!(rel);
                        }
                    }
                    Some(out)
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Section splitting (deterministic, runs before any LLM call).
// ---------------------------------------------------------------------------

/// A note segment: an explicitly header-tagged section (`hint = Some(label)`) or
/// untagged prose (`hint = None`). `body` is the text that belongs to it.
pub struct Segment {
    pub hint: Option<String>,
    pub body: String,
}

/// Labels that look like headers but never route a category (avoid false
/// positives like "Note:" / "TODO:").
const HEADER_STOPLIST: &[&str] = &[
    "note", "notes", "todo", "ps", "today", "tomorrow", "yesterday", "am", "pm", "re", "update",
];

/// Below these, an untagged block is treated as ONE topic and extracted whole
/// rather than paying an LLM segmentation pass — and, crucially, keeping the full
/// context so a short note's entities (e.g. "lunch with Jake") aren't lost when a
/// small model extracts from a 4-word shard.
const MULTITOPIC_MIN_WORDS: usize = 40;
const MULTITOPIC_MIN_SENTENCES: usize = 3;

/// Is this untagged block long enough to plausibly hold several loggable topics?
/// A cheap, deterministic gate: only a real multi-sentence paragraph is worth a
/// semantic-segmentation call; short scraps ("felt tired today") pass through as
/// one segment.
pub fn is_multi_topic_candidate(body: &str) -> bool {
    let words = body.split_whitespace().count();
    let sentences = body
        .split(|c| matches!(c, '.' | '!' | '?' | '\n'))
        .filter(|s| !s.trim().is_empty())
        .count();
    words >= MULTITOPIC_MIN_WORDS && sentences >= MULTITOPIC_MIN_SENTENCES
}

/// Split a note on `Section header:` lines (PROTOCOL.md §2). A header is a short
/// line (<=3 alphabetic words) ending in a separator (`:` / `—` / trailing `-`)
/// or a leading markdown `#`; text on the header line after the separator, plus
/// every line until the next header, is that section's body. Text before the
/// first header is an untagged segment. Always returns >= 1 segment.
pub fn split_sections(text: &str) -> Vec<Segment> {
    fn flush(hint: Option<String>, body: &str, out: &mut Vec<Segment>) {
        if !body.trim().is_empty() {
            out.push(Segment { hint, body: body.trim().to_string() });
        }
    }

    let mut segments: Vec<Segment> = Vec::new();
    let mut hint: Option<String> = None;
    let mut body = String::new();

    for line in text.lines() {
        if let Some((label, inline)) = parse_header(line) {
            flush(hint.take(), &body, &mut segments);
            body = String::new();
            hint = Some(label);
            if !inline.trim().is_empty() {
                body.push_str(inline.trim());
                body.push('\n');
            }
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(hint.take(), &body, &mut segments);

    if segments.is_empty() {
        segments.push(Segment { hint: None, body: text.trim().to_string() });
    }
    segments
}

/// Recognize a section-header line; returns (lowercased label, inline remainder).
fn parse_header(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    // markdown heading: "## Work"
    if let Some(rest) = t.strip_prefix('#') {
        return header_label(rest.trim_start_matches('#').trim()).map(|l| (l, String::new()));
    }
    // "Label:" — a colon may carry an inline body on the same line
    // (e.g. "Food: rice, chicken"), so split on the first colon.
    if let Some(idx) = t.find(':') {
        if let Some(label) = header_label(&t[..idx]) {
            return Some((label, t[idx + 1..].trim().to_string()));
        }
    }
    // Dash headers ("Gym —", "Schedule -", "Gym –") must be the WHOLE line — no
    // inline body — so a prose line with a mid-sentence dash ("chipotle bowl —
    // chicken, rice, guac") isn't misread as a header that steals the section it
    // sits under.
    for dash in ['—', '–', '-'] {
        if let Some(head) = t.strip_suffix(dash) {
            if let Some(label) = header_label(head.trim()) {
                return Some((label, String::new()));
            }
        }
    }
    None
}

/// Validate a header label: 1-3 alphabetic words, not in the stoplist.
fn header_label(s: &str) -> Option<String> {
    let s = s.trim();
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() || words.len() > 3 {
        return None;
    }
    if !words.iter().all(|w| w.chars().all(|c| c.is_alphabetic())) {
        return None;
    }
    let label = s.to_lowercase();
    if HEADER_STOPLIST.contains(&label.as_str()) {
        return None;
    }
    Some(label)
}

// ---------------------------------------------------------------------------
// Extraction (one LLM call per segment).
// ---------------------------------------------------------------------------

/// Extract one segment into a proposal. `forced` (a header label) fixes the
/// category by code; otherwise the model classifies (and may choose `misc`).
/// Returns (proposal, the model's raw event_date guess, entity candidates).
async fn extract_segment(
    catalog: &str,
    known: &[String],
    body: &str,
    today: &str,
    forced: Option<&str>,
) -> Result<(Value, Option<String>, Vec<Value>)> {
    let system = build_categorize_prompt(catalog, today);
    let hint = forced
        .map(|c| format!("This note is specifically about \"{c}\" — extract its structured data and keep that category.\n\n"))
        .unwrap_or_default();

    let mut last_err = String::new();
    for attempt in 0..2 {
        let user = if attempt == 0 {
            format!("{hint}{body}")
        } else {
            format!(
                "{hint}{body}\n\n(Your previous reply was invalid: {last_err}. Return JSON with keys \
                 category (string), is_new_category (bool), description (string), \
                 event_date (YYYY-MM-DD or null), data (object). JSON only.)"
            )
        };
        match ollama::chat_json(&ollama::text_model(), &system, &user, None, Some(routing_schema())).await {
            Ok(v) => {
                let raw_date = v.get("event_date").and_then(|d| d.as_str()).map(String::from);
                let ents = parse_entities(&v);
                match validate_proposal(v) {
                    Ok(mut proposal) => {
                        let cat = match forced {
                            Some(f) => snap_category(f, known),
                            None => snap_category(proposal["category"].as_str().unwrap_or(""), known),
                        };
                        proposal["is_new_category"] = json!(!known.contains(&cat));
                        proposal["routed_by"] = json!(if forced.is_some() { "header" } else { "classifier" });
                        proposal["category"] = json!(cat);
                        return Ok((proposal, raw_date, ents));
                    }
                    Err(e) => last_err = e,
                }
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(anyhow!("could not extract segment: {last_err}"))
}

// ---------------------------------------------------------------------------
// Stage A: semantic segmentation. One LLM pass reads the whole (often messy,
// transcribed) dump and carves it into discrete loggable ITEMS, each routed to
// a category — so a rambling brain-dump becomes many clean entries instead of
// being lumped under one accidental header. Stage B (extract_segment) then
// extracts each item's structured data, unchanged.
// ---------------------------------------------------------------------------

/// JSON schema for the segmenter: a list of {category, verbatim text} items.
fn segment_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "category": { "type": "string" },
                        "text": { "type": "string" }
                    },
                    "required": ["category", "text"]
                }
            }
        },
        "required": ["items"]
    })
}

fn build_segment_prompt(catalog: &str, today: &str) -> String {
    format!(
        "You are the segmenter for \"noted\", a personal life-logging app. Today is {today}. \
The user dumped a long, messy note (often spoken then transcribed) about their day. Group it into a \
SMALL number of coarse loggable ITEMS — aim for a handful (roughly 3 to 7 for a full day), NOT one \
per sentence. Return JSON: {{\"items\": [{{\"category\": string, \"text\": string}}]}}.\n\n\
HOW TO GROUP:\n\
- Put the day's general flow — waking/sleep, getting ready, errands, work blocks, shifts, \
appointments, and plans for later or tomorrow — TOGETHER into ONE schedule/day item (reuse the \
\"schedule\" category if it exists). Keep the clock times.\n\
- Pull OUT as their own item ONLY things with rich, trackable structure worth their own record: a \
workout; a supplement/medication stack (group ALL supplements taken in ONE session into a SINGLE \
item — a morning stack and a bedtime stack are two items); a notable meeting or call with a named \
person.\n\
- Keep how the user FELT inside the item it happened during. Never make a mood its own item unless a \
whole passage is purely reflection with no activity.\n\n\
COVERAGE: every sentence must land inside some item's \"text\"; don't drop content. \"text\" is the \
user's OWN WORDS copied VERBATIM — you only cut the note into spans (trimming whitespace is fine), \
never rewriting or summarizing. A schedule item's text may gather several sentences from across the \
note.\n\n\
CATEGORY: short, lowercase. REUSE an existing category name from the list below whenever it fits — \
do NOT invent a near-synonym of one that already exists (use \"schedule\", not \"work\" or \"day\"). \
Prefer the catch-all \"misc\" over a shaky new category.\n\n\
A full day usually becomes ~4-6 items: one \"schedule\" item (the day's blocks/errands/plans, with \
times), plus standouts like a \"gym\" workout, a supplement stack, and a meeting with a named person \
— each keeping any mood felt during it.\n\n\
Existing categories (REUSE these when they fit):\n{catalog}\n\n\
Return JSON only."
    )
}

/// Stage A: ask the model to split the dump into routed items. Returns segments
/// whose `hint` is the proposed category (consumed by Stage B as the forced
/// category, exactly like a header label).
async fn segment_note(
    catalog: &str,
    known: &[String],
    text: &str,
    today: &str,
) -> Result<Vec<Segment>> {
    let system = build_segment_prompt(catalog, today);
    let v = ollama::chat_json(&ollama::text_model(), &system, text, None, Some(segment_schema())).await?;
    let items = v
        .get("items")
        .and_then(|i| i.as_array())
        .ok_or_else(|| anyhow!("segmentation returned no items array"))?;

    let mut segs = Vec::new();
    for it in items {
        let body = it.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
        if body.is_empty() {
            continue;
        }
        let cat = it.get("category").and_then(|c| c.as_str()).unwrap_or("").trim().to_lowercase();
        let hint = if cat.is_empty() { None } else { Some(snap_category(&cat, known)) };
        segs.push(Segment { hint, body });
    }
    if segs.is_empty() {
        return Err(anyhow!("segmentation produced no usable items"));
    }
    Ok(segs)
}

/// Split a note into items, extract each, and assemble the envelope. Stage A
/// (semantic segmentation) routes each item to a category; if that LLM call
/// fails we fall back to the deterministic header split. The calendar day is
/// resolved once for the whole note. A single failing item is skipped rather
/// than failing the note; zero entries is an error.
/// Strip a leading list/checkbox marker from each line ("□ ", "- ", "[x] ", …)
/// before the model parses the note, so checklist glyphs aren't echoed verbatim
/// into a task instead of having their times extracted. The original note is
/// still stored as raw_text.
fn strip_line_markers(text: &str) -> String {
    let re = regex::Regex::new(
        r"(?m)^[ \t]*(?:[-*•·▪●□☐▢◻◽◾■☑☒✅✓✔✗✘]|\[[ xX]?\]|\([ xX]?\))[ \t]+",
    )
    .unwrap();
    re.replace_all(text, "").into_owned()
}

async fn extract_note(
    catalog: &str,
    known: &[String],
    text: &str,
    raw_text: &str,
    today: &str,
) -> Result<Value> {
    // Strip leading checklist/box glyphs so the model parses the content, not the
    // markers; raw_text (stored + shown in review) keeps the user's original.
    let text = strip_line_markers(text);
    let text = text.as_str();
    // Hybrid split: run the deterministic header parser FIRST so a section you
    // explicitly tagged (`Gym:`) routes by code, never by the model — you keep
    // control of routing when you ask for it. Only the UNTAGGED prose (the
    // freeform brain-dump, or a note with no headers at all) is handed to the
    // semantic segmenter, which fans it into many routed items. If that LLM call
    // fails, the untagged block falls back to a single classified segment.
    let mut segments: Vec<Segment> = Vec::new();
    for seg in split_sections(text) {
        match seg.hint {
            // Explicit header → deterministic routing, untouched.
            Some(_) => segments.push(seg),
            // Untagged prose: only a long, multi-sentence block is worth a
            // semantic-segmentation pass. A short scrap is extracted whole so its
            // entities survive and we skip a needless LLM call. If segmentation
            // fails, fall back to the single classified segment.
            None if is_multi_topic_candidate(&seg.body) => {
                match segment_note(catalog, known, &seg.body, today).await {
                    Ok(mut segs) if !segs.is_empty() => segments.append(&mut segs),
                    _ => segments.push(seg),
                }
            }
            None => segments.push(seg),
        }
    }
    let mut entries: Vec<Value> = Vec::new();
    let mut model_dates: Vec<Option<String>> = Vec::new();
    // Entity candidates are note-level: collected across all segments, deduped by
    // (normalized name, type) so the same person/place isn't proposed twice.
    let mut entities: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for seg in &segments {
        if let Ok((proposal, raw_date, ents)) =
            extract_segment(catalog, known, &seg.body, today, seg.hint.as_deref()).await
        {
            // Guardrail: drop entries the model couldn't extract any data for
            // (empty `data` object) — they'd just be timeline noise.
            let empty = proposal
                .get("data")
                .and_then(|d| d.as_object())
                .map(|o| o.is_empty())
                .unwrap_or(true);
            if empty {
                continue;
            }
            entries.push(proposal);
            model_dates.push(raw_date);
            for ent in ents {
                let name = ent["name"].as_str().unwrap_or("");
                let etype = ent["type"].as_str().unwrap_or("").to_string();
                if seen.insert((crate::entities::normalize(name), etype)) {
                    entities.push(ent);
                }
            }
        }
    }
    // We read text fine but the structurer found nothing to extract (common for
    // pure schedules, which are handled deterministically elsewhere, or terse
    // notes). Never throw a good transcription away: land it in the reserved
    // `misc` catch-all so the note is preserved and the review UI always has at
    // least one entry to act on. Callers that only want the transcription (the
    // Today schedule flow) ignore entries and re-parse raw_text, so this is inert
    // there.
    if entries.is_empty() {
        entries.push(json!({
            "category": "misc",
            "is_new_category": !known.iter().any(|k| k.eq_ignore_ascii_case("misc")),
            "description": "uncategorized note",
            "routed_by": "classifier",
            "data": { "text": text.trim() },
        }));
        model_dates.push(None);
    }

    // One calendar day for the whole note: first model date that parses, else a
    // date scraped from the text, else today.
    let (mut date, mut extracted) = (today.to_string(), false);
    for d in &model_dates {
        let (dd, ok) = resolve_date(d.as_deref(), today);
        if ok {
            date = dd;
            extracted = true;
            break;
        }
    }
    if !extracted {
        if let Some(d) = extract_date_from_text(text, today) {
            date = d;
            extracted = true;
        }
    }

    Ok(json!({
        "raw_text": raw_text,
        "event_date": date,
        "date_was_extracted": extracted,
        "entries": entries,
        "entities": entities,
    }))
}

/// Transcribe a photo of a note (handwritten/printed) to text, preserving the
/// user's words, line breaks, and any section headers, so the shared text path
/// can route + extract it the same way a typed note is handled.
pub async fn transcribe_photo(image_b64: &str) -> Result<String> {
    let system = "You transcribe a photo of a personal note as faithfully as you can. Preserve the \
        user's exact words, line breaks, and any section headers (lines like \"Food:\" or \"Gym —\"). \
        Do not summarize, translate, or add anything. Return JSON only: {\"raw_text\": string}.";
    let schema = json!({
        "type": "object",
        "properties": { "raw_text": { "type": "string" } },
        "required": ["raw_text"]
    });
    let v = ollama::chat_json(
        &ollama::vision_model(),
        system,
        "Transcribe this note exactly.",
        Some(vec![image_b64.to_string()]),
        Some(schema),
    )
    .await?;
    Ok(v.get("raw_text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string())
}

/// Typed-note path: split into sections, route + extract each, return the
/// envelope `{ raw_text, event_date, date_was_extracted, entries:[...] }`.
pub async fn categorize(
    catalog: &str,
    known_names: &[String],
    text: &str,
    today: &str,
) -> Result<Value> {
    extract_note(catalog, known_names, text, text, today).await
}

/// Photo path: transcribe with the vision model, then route + extract the
/// transcription exactly like a typed note. The envelope's raw_text is the
/// transcription (editable in review).
pub async fn categorize_photo(
    catalog: &str,
    known_names: &[String],
    image_b64: &str,
    today: &str,
) -> Result<Value> {
    let raw_text = transcribe_photo(image_b64).await?;
    if raw_text.is_empty() {
        return Err(anyhow!("could not read any text from the photo"));
    }
    extract_note(catalog, known_names, &raw_text, &raw_text, today).await
}

pub fn build_categorize_prompt(catalog: &str, today: &str) -> String {
    format!(
        "You are the classifier for \"noted\", a personal life-logging app. \
Today's date is {today}. \
The user dumps a messy note about ONE thing (a workout, a meal, a stretch of their day). \
Do these and return ONE JSON object.\n\n\
1) Pick the single best CATEGORY for this note. Reuse an existing category name verbatim when the \
note fits one. Only invent a new category when none fit; use a short, lowercase name \
(e.g. \"gym\", \"schedule\", \"meals\") and set is_new_category=true with a one-line description.\n\
2) event_date: the date the note refers to, resolved to YYYY-MM-DD using today's date for relative \
or partial dates (e.g. \"6/2\" -> the closest such date, \"yesterday\" -> today minus one). If the \
note mentions no date at all, use null.\n\
3) EXTRACT the structured data the note contains into \"data\".\n\
4) ENTITIES: list the concrete things this note refers to — people, places, activities, foods, \
organizations, or recurring topics — as objects {{\"name\", \"type\"}} where type is one of \
person|place|activity|food|item|org|topic. Use the name as written (e.g. \"Jake\", \"Planet Fitness\", \
\"chipotle bowl\"). Skip generic words and anything you aren't sure is a real entity; empty list if none.\n\
   For a \"person\", also add: \"fact\" — a short phrase capturing what happened or what you learned \
about THEM in this note (e.g. \"got engaged\", \"started a new job at Stripe\"); and \"relationship\" \
— how they relate to the author IF the note says so (e.g. \"friend\", \"coworker\", \"brother\"). Omit \
either when the note doesn't provide it. NEVER list the author/narrator (first-person \"I\"/\"me\"/\"my\") \
as a person.\n\
   CRITICAL: if you put a person's name anywhere inside \"data\" (e.g. a \"with\", \"who\", or \
\"attendees\" field), that SAME person MUST also appear in \"entities\" as type \"person\". For example \
the note \"talked to Khai about my trip\" yields data {{\"conversations\":[{{\"with\":\"Khai\",\"topic\":\
\"my trip\"}}]}} AND entities [{{\"name\":\"Khai\",\"type\":\"person\",\"fact\":\"talked about my trip\"}}].\n\n\
Existing categories (reuse if the note fits one):\n{catalog}\n\n\
Rules:\n\
- When reusing a category, match its existing shape so data stays consistent over time.\n\
- Prefer the catch-all category \"misc\" over inventing a brand-new category you're unsure about; \
only create a new category when the note is clearly a recurring, substantial topic.\n\
- Structure repeated things as an ARRAY OF OBJECTS — one object per item, each keeping its own \
attributes together. NEVER use parallel arrays (separate name[], weight[], reps[] arrays). \
For example a workout becomes: \"exercises\": [{{\"name\": \"squat\", \"weight\": 245, \"sets\": 3, \
\"reps\": 5, \"rpe\": 9}}, ...]; a schedule becomes: \"blocks\": [{{\"task\": \"coding\", \"start\": \"09:00\", \
\"end\": \"11:00\", \"duration_min\": 120}}, ...].\n\
- Time periods matter: when an activity has a clock time or time range, capture \"start\" then \
\"end\" (start before end) in 24-hour HH:MM (e.g. \"2-4pm\" -> start \"14:00\", end \"16:00\"; \"9am\" \
-> start \"09:00\"), and \"duration_min\" when you can derive it. Omit start/end when no time is given.\n\
- AM/PM: clock times often omit it. INFER it from the note's chronological flow — a day's note runs \
morning -> night, so use the surrounding times and their order to decide, then output 24-hour HH:MM \
(e.g. wake 8:30 -> 08:30, gym 10:40 -> 10:40, shift at 12 -> 12:00, home 12:45 -> 12:45, evening \
events like 6:30 -> 18:30).\n\
- For a \"schedule\", capture EVERY time block mentioned across the WHOLE note — morning to night — \
each as its own block with start and end. Do not stop partway through the day.\n\
- Numbers as numbers, not strings. Read fractions and decimals literally: \"7 1/2 mg\" -> 7.5 (NEVER \
7500); never concatenate digits. Weights are in POUNDS — record the plain number (\"32lb\" -> \
\"weight\": 32); never convert to kilograms and do not add a unit field. Sanity-check magnitudes (a \
supplement dose in mg is usually < 1000); omit a value rather than emit an absurd one. Omit fields \
you don't know rather than guessing.\n\
- Keep keys short, lowercase, snake_case.\n\
- Feelings matter: whenever the note says how the user felt, add a \"mood\" key inside data (a short \
phrase, e.g. \"satisfied\", \"anxious and unfocused\") — in ANY category.\n\
- If a note is mostly about feelings or mental state with no concrete activity, use category \"mood\".\n\
- event_date is the calendar day only; the clock times within a day go in start/end fields.\n\n\
Return JSON only, exactly this form:\n\
{{\"category\": string, \"is_new_category\": bool, \"description\": string, \"event_date\": string|null, \"data\": object, \"entities\": [{{\"name\": string, \"type\": string, \"fact\": string|null, \"relationship\": string|null}}]}}"
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
