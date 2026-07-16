// Meeting summarization: template prompt (PLAUD's model — one free-text prompt
// describing sections) → schema-constrained local LLM call → deterministic
// markdown render → a summary tab + (once per meeting) a real note filed under
// the 'meetings' category so search/embeddings/entities all see it.
//
// ALWAYS chat_json_local_ctx — meeting content never touches the Balanced
// cloud path (same rule as the Journal).

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use super::store;
use crate::db::Db;
use crate::ollama;

/// Above this transcript size we condense in chunks first (7B context budget).
const SINGLE_PASS_CHARS: usize = 60_000;
const CHUNK_CHARS: usize = 40_000;

pub fn mmss(ms: i64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Render segments as prompt-friendly transcript lines.
fn transcript_text(segments: &[Value]) -> String {
    segments
        .iter()
        .map(|s| {
            let t0 = s["t0_ms"].as_i64().unwrap_or(0);
            let who = match s["channel"].as_str().unwrap_or("them") {
                "me" => "Me",
                _ => s["speaker"].as_str().unwrap_or("Them"),
            };
            format!("[{}] {}: {}", mmss(t0), who, s["text"].as_str().unwrap_or(""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One self-contained Markdown document for a meeting: header, every summary
/// tab (headings demoted one level under the tab name), the user's verbatim
/// notes, and the full speaker-labeled transcript. Deterministic — no model.
pub fn export_markdown(meeting: &Value) -> String {
    let title = meeting["title"].as_str().unwrap_or("Meeting");
    let date: String = meeting["started_at"]
        .as_str()
        .map(|s| s.chars().take(10).collect())
        .unwrap_or_default();
    let attendees = attendee_names(&meeting["event_json"]);
    let empty = Vec::new();
    let segments = meeting["segments"].as_array().unwrap_or(&empty);
    let summaries = meeting["summaries"].as_array().unwrap_or(&empty);
    let raw_notes = meeting["raw_notes"].as_str().unwrap_or("");

    let mut md = format!("# {title}\n\n");
    let mut meta: Vec<String> = Vec::new();
    if !date.is_empty() {
        meta.push(date);
    }
    if !attendees.is_empty() {
        meta.push(attendees.join(", "));
    }
    let (me_ms, them_ms) = (
        meeting["talk_ms"]["me"].as_i64().unwrap_or(0),
        meeting["talk_ms"]["them"].as_i64().unwrap_or(0),
    );
    if me_ms + them_ms > 0 {
        meta.push(format!("you spoke {}%", me_ms * 100 / (me_ms + them_ms)));
    }
    if !meta.is_empty() {
        md.push_str(&format!("*{}*\n", meta.join(" · ")));
    }

    for s in summaries {
        let tpl = s["template"].as_str().unwrap_or("Summary");
        md.push_str(&format!("\n---\n\n## {tpl}\n\n"));
        for line in s["content_md"].as_str().unwrap_or("").lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                md.push_str(&format!("### {rest}\n"));
            } else {
                md.push_str(line);
                md.push('\n');
            }
        }
    }
    if !raw_notes.trim().is_empty() {
        md.push_str(&format!("\n---\n\n## Your Notes (verbatim)\n\n{}\n", raw_notes.trim()));
    }
    if !segments.is_empty() {
        md.push_str("\n---\n\n## Transcript\n\n");
        for s in segments {
            let who = match s["channel"].as_str().unwrap_or("them") {
                "me" => "Me",
                _ => s["speaker"].as_str().unwrap_or("Them"),
            };
            md.push_str(&format!(
                "- [{}] **{}**: {}\n",
                mmss(s["t0_ms"].as_i64().unwrap_or(0)),
                who,
                s["text"].as_str().unwrap_or("")
            ));
        }
    }
    md
}

/// "brian@heybaro.com" → "Brian"; display names pass through untouched.
fn humanize(name: &str) -> String {
    let base = name.split('@').next().unwrap_or(name).trim();
    let mut c = base.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Attendees who are NOT the note-taker, as presentable names — the candidate
/// pool for speaker naming (a lone entry = the 1:1 rule's ground truth).
pub fn external_attendees(event_json: &Value) -> Vec<String> {
    let Some(arr) = event_json.get("attendees").and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter(|a| !a.get("self").and_then(|s| s.as_bool()).unwrap_or(false))
        .filter_map(|a| {
            if let Some(s) = a.as_str() {
                return Some(humanize(s));
            }
            a.get("name")
                .or_else(|| a.get("displayName"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(humanize)
                .or_else(|| a.get("email").and_then(|v| v.as_str()).map(humanize))
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn attendee_names(event_json: &Value) -> Vec<String> {
    let arr = event_json.get("attendees").and_then(|a| a.as_array());
    let Some(arr) = arr else { return Vec::new() };
    arr.iter()
        .filter_map(|a| {
            if let Some(s) = a.as_str() {
                return Some(s.to_string());
            }
            a.get("name")
                .or_else(|| a.get("displayName"))
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| {
                    a.get("email")
                        .and_then(|v| v.as_str())
                        .map(|e| e.split('@').next().unwrap_or(e).to_string())
                })
        })
        .collect()
}

fn sections_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "heading": { "type": "string" },
                        "kind": { "type": "string", "enum": ["paragraph", "bullets", "timeline", "todos"] },
                        "paragraph": { "type": "string" },
                        "items": { "type": "array", "items": { "type": "string" } },
                        "timeline": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "ts": { "type": "string" },
                                    "text": { "type": "string" }
                                },
                                "required": ["ts", "text"]
                            }
                        }
                    },
                    "required": ["heading", "kind"]
                }
            }
        },
        "required": ["sections"]
    })
}

/// Deterministic markdown from the model's sections. Empty/degenerate sections
/// are dropped, so a template section with nothing to say simply vanishes
/// (matching the prompt's "omit if nothing" rule even when the model doesn't).
pub fn render_markdown(sections: &Value) -> String {
    let mut out = String::new();
    let list = sections
        .get("sections")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    for sec in list.iter().take(12) {
        let heading = sec["heading"].as_str().unwrap_or("").trim();
        if heading.is_empty() {
            continue;
        }
        let body = match sec["kind"].as_str().unwrap_or("bullets") {
            "paragraph" => sec["paragraph"].as_str().unwrap_or("").trim().to_string(),
            "timeline" => sec["timeline"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| {
                            let ts = i["ts"].as_str().unwrap_or("").trim();
                            let text = i["text"].as_str().unwrap_or("").trim();
                            if text.is_empty() {
                                None
                            } else if ts.is_empty() {
                                Some(format!("- {text}"))
                            } else {
                                Some(format!("- **[{}]** {}", ts.trim_matches(['[', ']']), text))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            "todos" => sec["items"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| i.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| format!("- [ ] {}", s.trim()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            _ => sec["items"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| i.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| format!("- {}", s.trim()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        };
        if body.is_empty() {
            continue;
        }
        out.push_str(&format!("## {heading}\n\n{body}\n\n"));
    }
    out.trim_end().to_string()
}

const SYSTEM: &str = "You write meeting notes. You are given a meeting transcript \
(lines look like '[mm:ss] Me: ...' — 'Me' is the note-taker's own mic; other lines \
carry the speaker's name when identified, or 'Speaker N'/'Them' when not), the \
note-taker's own typed notes, and a template describing the sections to produce.\n\
Rules:\n\
- Ground every statement in the transcript or the typed notes. Never invent facts, \
names, numbers, or dates.\n\
- The typed notes are the highest-priority signal: every point in them must be \
reflected and expanded with context from the transcript.\n\
- Timeline sections use kind='timeline' with ts set to a [mm:ss] timestamp that \
actually appears in the transcript — the moment that topic starts.\n\
- Action items use kind='todos', each item shaped 'Owner — verb phrase' with \
'by <date>' appended when a deadline was stated. Owner is Me or a speaker/stated name.\n\
- If the meeting has nothing for a section, omit that section entirely.\n\
- The notes must stand alone: someone who missed the meeting should get everything \
that mattered without reading the transcript. Err on the side of MORE detail, not less.\n\
- Every point carries its specifics — who said it, the numbers, dates, names, and the \
reasoning or disagreement behind it. Never collapse distinct points into one vague bullet.\n\
- Scale detail to the meeting: a 5-minute check-in earns a few lines; an hour of \
discussion earns thorough notes on every topic raised.\n\
- Be concrete. Quote short phrases where the exact wording matters.\n\
Respond ONLY with JSON: {\"sections\":[{\"heading\",\"kind\":\"paragraph|bullets|timeline|todos\",\
\"paragraph\"?,\"items\"?,\"timeline\"?:[{\"ts\",\"text\"}]}]}";

/// Condense one oversized transcript chunk into timestamped fact lines.
async fn condense_chunk(chunk: &str) -> Result<String> {
    let schema = json!({
        "type": "object",
        "properties": { "facts": { "type": "array", "items": { "type": "string" } } },
        "required": ["facts"]
    });
    let out = ollama::chat_json_local_ctx(
        &ollama::text_model(),
        "Condense this meeting transcript chunk into dense factual notes. Each fact \
         starts with the [mm:ss] timestamp it came from and preserves names, numbers, \
         decisions, tasks, and questions verbatim. Respond ONLY with JSON {\"facts\":[...]}.",
        chunk,
        None,
        Some(schema),
        24_576,
    )
    .await?;
    Ok(out["facts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Speaker-name suggestions: after diarization, mine the transcript (plus the
// calendar attendee list as candidates) for who "Speaker N" actually is.
// Suggestions are stored on meeting_speakers.suggested and require an explicit
// confirm (= rename) in the UI — a 7B model's guess never silently becomes a
// label. Local model always; meeting content never leaves the machine.
// ---------------------------------------------------------------------------

const SUGGEST_CONFIDENCE: f64 = 0.7;
const EVIDENCE_CAP_CHARS: usize = 12_000;

/// Deterministic evidence packing: the meeting intro, each unnamed speaker's
/// first/last lines, and every line mentioning a candidate first name.
fn name_evidence(segments: &[Value], unnamed: &[String], attendees: &[String]) -> String {
    let who_of = |s: &Value| -> String {
        match s["channel"].as_str().unwrap_or("them") {
            "me" => "Me".into(),
            _ => s["speaker"].as_str().unwrap_or("Them").to_string(),
        }
    };
    let firsts: Vec<String> = attendees
        .iter()
        .filter_map(|a| a.split_whitespace().next())
        .map(|s| s.to_lowercase())
        .collect();
    let mut picked: Vec<usize> = (0..segments.len().min(12)).collect(); // intro window
    for label in unnamed {
        let theirs: Vec<usize> = segments
            .iter()
            .enumerate()
            .filter(|(_, s)| who_of(s) == *label)
            .map(|(i, _)| i)
            .collect();
        picked.extend(theirs.iter().take(4));
        picked.extend(theirs.iter().rev().take(2));
    }
    for (i, s) in segments.iter().enumerate() {
        let text = s["text"].as_str().unwrap_or("").to_lowercase();
        if firsts.iter().any(|f| !f.is_empty() && text.contains(f.as_str())) {
            picked.push(i);
        }
    }
    picked.sort_unstable();
    picked.dedup();
    let mut out = String::new();
    for i in picked {
        let s = &segments[i];
        let line = format!(
            "[{}] {}: {}\n",
            mmss(s["t0_ms"].as_i64().unwrap_or(0)),
            who_of(s),
            s["text"].as_str().unwrap_or("")
        );
        if out.len() + line.len() > EVIDENCE_CAP_CHARS {
            break;
        }
        out.push_str(&line);
    }
    out
}

/// Propose real names for this meeting's "Speaker N" (and lone "Them") voices.
/// Returns how many suggestions were stored; emits meeting-speakers-suggested.
pub async fn suggest_speaker_names(app: &tauri::AppHandle, meeting_id: i64) -> Result<usize> {
    let (segments, unnamed, attendees) = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let meeting = store::get_meeting(&conn, meeting_id)?;
        let segments = store::list_segments(&conn, meeting_id)?;
        let unnamed: Vec<String> = store::list_meeting_speakers(&conn, meeting_id)?
            .iter()
            .filter(|s| {
                s["suggested"].is_null()
                    && s["label"]
                        .as_str()
                        .map(|l| l.starts_with("Speaker ") || l == "Them")
                        .unwrap_or(false)
            })
            .filter_map(|s| s["label"].as_str().map(String::from))
            .collect();
        (segments, unnamed, external_attendees(&meeting["event_json"]))
    };
    if unnamed.is_empty() || segments.is_empty() {
        return Ok(0);
    }

    let evidence = name_evidence(&segments, &unnamed, &attendees);
    let schema = json!({
        "type": "object",
        "properties": {
            "mappings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "speaker": { "type": "string" },
                        "name": { "type": "string" },
                        "confidence": { "type": "number" },
                        "evidence": { "type": "string" }
                    },
                    "required": ["speaker", "name", "confidence"]
                }
            }
        },
        "required": ["mappings"]
    });
    let user = format!(
        "UNIDENTIFIED SPEAKERS: {}\n\nCANDIDATE NAMES (calendar attendees): {}\n\n\
         TRANSCRIPT EXCERPTS:\n{evidence}",
        unnamed.join(", "),
        if attendees.is_empty() { "(none listed — use names stated in the transcript)".into() } else { attendees.join(", ") },
    );
    let out = ollama::chat_json_local_ctx(
        &ollama::text_model(),
        "You map a meeting's unidentified speakers to real names, using only evidence \
         in the transcript: being addressed by name right after speaking, answering when \
         a name is called, self-introductions, or presenting work attributed to a name. \
         'Me' is the note-taker, never a candidate name. Include a mapping ONLY when the \
         evidence clearly supports it and give the exact speaker label; omit speakers you \
         are unsure about — a missing mapping is better than a wrong one. Respond ONLY \
         with JSON {\"mappings\":[{\"speaker\",\"name\",\"confidence\":0..1,\"evidence\"}]}.",
        &user,
        None,
        Some(schema),
        16_384,
    )
    .await?;

    let empty = Vec::new();
    let mappings = out["mappings"].as_array().unwrap_or(&empty);
    let mut applied = 0usize;
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        for m in mappings {
            let speaker = m["speaker"].as_str().unwrap_or("");
            let name = m["name"].as_str().unwrap_or("").trim();
            let conf = m["confidence"].as_f64().unwrap_or(0.0);
            if conf < SUGGEST_CONFIDENCE
                || name.is_empty()
                || name == "Me"
                || name == "Them"
                || name.starts_with("Speaker ")
                || !unnamed.iter().any(|l| l == speaker)
            {
                continue;
            }
            if store::set_speaker_suggestion(&conn, meeting_id, speaker, name).is_ok() {
                applied += 1;
            }
        }
    }
    if applied > 0 {
        let _ = app.emit(
            "meeting-speakers-suggested",
            json!({ "meetingId": meeting_id, "count": applied }),
        );
    }
    Ok(applied)
}

/// Generate or refresh one template summary for a meeting. Files the meeting
/// note on first summarize; later runs replace that template's existing tab.
pub async fn run(app: &tauri::AppHandle, meeting_id: i64, template_name: Option<String>) -> Result<String> {
    // Snapshot everything under one short lock.
    let (meeting, segments, template, prompt) = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let meeting = store::get_meeting(&conn, meeting_id)?;
        let segments = store::list_segments(&conn, meeting_id)?;
        let name = template_name.unwrap_or_else(|| super::cfg().default_template);
        let prompt = store::get_template(&conn, &name)?
            .ok_or_else(|| anyhow!("unknown template '{name}'"))?;
        (meeting, segments, name, prompt)
    };
    if segments.is_empty() {
        return Err(anyhow!("no transcript to summarize"));
    }

    let title = meeting["title"].as_str().unwrap_or("Meeting").to_string();
    let raw_notes = meeting["raw_notes"].as_str().unwrap_or("").to_string();
    let attendees = attendee_names(&meeting["event_json"]);
    let date = meeting["started_at"]
        .as_str()
        .map(|s| s.chars().take(10).collect::<String>())
        .unwrap_or_default();

    let mut transcript = transcript_text(&segments);
    if transcript.len() > SINGLE_PASS_CHARS {
        // Map-reduce: condense chunks, then summarize the condensed notes.
        let chars: Vec<char> = transcript.chars().collect();
        let mut condensed = String::new();
        for chunk in chars.chunks(CHUNK_CHARS) {
            let chunk: String = chunk.iter().collect();
            condensed.push_str(&condense_chunk(&chunk).await?);
            condensed.push('\n');
        }
        transcript = format!("(condensed from a long transcript)\n{condensed}");
    }

    let user = format!(
        "TEMPLATE — produce these sections, in this order:\n{prompt}\n\n\
         MEETING: {title}{}{}\n\n\
         MY TYPED NOTES:\n{}\n\n\
         TRANSCRIPT:\n{transcript}",
        if date.is_empty() { String::new() } else { format!(" ({date})") },
        if attendees.is_empty() {
            String::new()
        } else {
            format!("\nATTENDEES: {}", attendees.join(", "))
        },
        if raw_notes.trim().is_empty() { "(none)" } else { raw_notes.trim() },
    );

    let out = ollama::chat_json_local_ctx(
        &ollama::text_model(),
        SYSTEM,
        &user,
        None,
        Some(sections_schema()),
        24_576,
    )
    .await?;
    let md = render_markdown(&out);
    if md.is_empty() {
        return Err(anyhow!("model produced an empty summary"));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let first_note = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        store::insert_summary(&conn, meeting_id, &template, &md, &now)?;
        meeting["note_id"].is_null()
    };

    // First summary → file a real note under 'meetings' (search/embeddings/KG).
    if first_note {
        let (me_ms, them_ms) = {
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            store::talk_time(&conn, meeting_id)?
        };
        let duration_min = segments
            .iter()
            .filter_map(|s| s["t1_ms"].as_i64())
            .max()
            .unwrap_or(0)
            / 60_000;
        let mut note_text = format!("# {title}\n\n{md}");
        if !raw_notes.trim().is_empty() {
            note_text.push_str(&format!("\n\n## Your Notes (verbatim)\n\n{}", raw_notes.trim()));
        }
        let entities: Vec<Value> = attendees
            .iter()
            .map(|n| json!({ "name": n, "type": "person" }))
            .collect();
        let save_json = json!({
            "raw_text": note_text,
            "source": "meeting",
            "event_date": if date.is_empty() { crate::today_local() } else { date.clone() },
            "entries": [{
                "category": "meetings",
                "description": title,
                "data": {
                    "meeting_id": meeting_id,
                    "title": title,
                    "attendees": attendees,
                    "duration_min": duration_min,
                    "talk_ms_me": me_ms,
                    "talk_ms_them": them_ms,
                },
            }],
            "entities": entities,
        });
        match serde_json::from_value::<crate::SaveArgs>(save_json) {
            Ok(args) => match crate::save_entry(app.clone(), args).await {
                Ok(note_id) => {
                    let state = app.state::<Db>();
                    let conn = state.0.lock().unwrap();
                    let _ = store::set_note_id(&conn, meeting_id, note_id);
                }
                Err(e) => eprintln!("[noted] meeting note filing failed: {e}"),
            },
            Err(e) => eprintln!("[noted] meeting note args invalid: {e}"),
        }
    }

    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        store::set_status(&conn, meeting_id, "done")?;
    }
    let _ = app.emit(
        "meeting-summarized",
        json!({ "meetingId": meeting_id, "template": template }),
    );

    // Feed the knowledge graph from the fresh summary (mention guard keeps it
    // idempotent across re-summarizes), then propose display names for any
    // attendee people still filed under a raw email. Both best-effort — a
    // knowledge hiccup never fails the summary the user is waiting on.
    if let Err(e) = extract_knowledge(app, meeting_id).await {
        eprintln!("[noted] knowledge extraction failed: {e}");
    }
    let _ = crate::suggest_person_names(app.clone()).await;

    Ok(md)
}

/// Post-summarize knowledge pass: mine the (already distilled) summary for the
/// projects, orgs, and topics the meeting discussed and link them to the
/// meeting's filed note as entity mentions — the meeting-fed food source for
/// the knowledge graph. Local model only, like everything meeting-side.
pub async fn extract_knowledge(app: &tauri::AppHandle, meeting_id: i64) -> Result<usize> {
    let (note_id, title, date, attendees, speakers, summary_md) = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let meeting = store::get_meeting(&conn, meeting_id)?;
        let Some(note_id) = meeting["note_id"].as_i64() else {
            return Ok(0); // nothing filed yet — the first summarize will loop back here
        };
        let title = meeting["title"].as_str().unwrap_or("Meeting").to_string();
        let date: String = meeting["started_at"]
            .as_str()
            .map(|s| s.chars().take(10).collect())
            .unwrap_or_else(crate::today_local);
        let attendees = attendee_names(&meeting["event_json"]);
        // Named speakers are people the KG must know even when they weren't on
        // the invite (voiceprint-matched or user-renamed — never "Speaker N").
        let speakers: Vec<String> = meeting["speakers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["label"].as_str())
            .filter(|l| {
                !l.is_empty() && *l != "Me" && *l != "Them" && !l.starts_with("Speaker ")
            })
            .map(|l| l.to_string())
            .collect();
        // Richest tab wins: the longest summary carries the most extractable detail.
        let md = meeting["summaries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["content_md"].as_str())
            .max_by_key(|c| c.len())
            .unwrap_or("")
            .to_string();
        (note_id, title, date, attendees, speakers, md)
    };
    if summary_md.is_empty() {
        return Ok(0);
    }

    let schema = json!({
        "type": "object",
        "properties": { "entities": { "type": "array", "items": {
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "type": { "type": "string", "enum": ["project", "org", "topic"] },
                "fact": { "type": "string" }
            },
            "required": ["name", "type", "fact"]
        }}},
        "required": ["entities"]
    });
    let system = "You mine a meeting summary for knowledge-graph entities. Extract the \
        distinct projects, organizations/companies/products, and recurring topics the \
        meeting actually discussed. Rules: 3-12 entities total, most important first; \
        names are short canonical noun phrases of 1-4 words (never a sentence); skip \
        generic words (meeting, update, team, discussion), dates, and people — people \
        are handled separately. For each entity give a one-sentence fact stating what \
        THIS meeting said about it. JSON: {\"entities\":[{\"name\",\"type\",\"fact\"}]}";
    let mut body = summary_md;
    body.truncate(12_000);
    let user = format!(
        "Meeting: {title} ({date})\nAttendees: {}\n\nSummary:\n{body}",
        if attendees.is_empty() { "(unknown)".to_string() } else { attendees.join(", ") }
    );
    // People first — attendees AND named speakers — deduped by normalized name.
    // These file even when the content extraction below has a bad day, and the
    // mention guard keeps re-runs (reindex, re-summarize) from double-counting.
    let mut candidates: Vec<crate::EntityCandidate> = Vec::new();
    let mut seen_people = std::collections::HashSet::new();
    for name in attendees.iter().chain(speakers.iter()) {
        let name = name.trim();
        if name.is_empty() || !seen_people.insert(crate::entities::normalize(name)) {
            continue;
        }
        candidates.push(crate::EntityCandidate {
            name: name.to_string(),
            etype: "person".into(),
            // Curated context so their card reads "Meeting: <title>", not a
            // raw-markdown snippet of the filed note.
            fact: Some(format!("Meeting: {title}")),
            relationship: None,
        });
    }

    // Content extraction is best-effort: a failed model call must not cost the
    // meeting its people.
    let extracted =
        match ollama::chat_json_local(&ollama::text_model(), system, &user, None, Some(schema))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[noted] knowledge extraction model call failed: {e}");
                json!({})
            }
        };
    candidates.extend(
        extracted["entities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let name = e["name"].as_str()?.trim();
                let etype = e["type"].as_str()?.trim().to_lowercase();
                if name.is_empty() || name.len() > 60 {
                    return None;
                }
                Some(crate::EntityCandidate {
                    name: name.to_string(),
                    etype,
                    fact: e["fact"].as_str().map(|f| f.trim().to_string()).filter(|f| !f.is_empty()),
                    relationship: None,
                })
            }),
    );
    if candidates.is_empty() {
        return Ok(0);
    }
    let now = chrono::Utc::now().to_rfc3339();
    let added =
        crate::persist_entities(app, note_id, &date, &title, &now, candidates, true).await;
    Ok(added as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_attendees_humanized_and_self_excluded() {
        let ev = json!({ "attendees": [
            { "email": "edison@heybaro.com", "self": true },
            { "email": "brian@heybaro.com" },
            { "name": "Jasmine Wu", "email": "j@x.com" },
        ]});
        assert_eq!(external_attendees(&ev), vec!["Brian", "Jasmine Wu"]);
        // Plain-string attendee lists humanize too.
        let ev2 = json!({ "attendees": ["mayan@heybaro.com"] });
        assert_eq!(external_attendees(&ev2), vec!["Mayan"]);
        assert!(external_attendees(&json!({})).is_empty());
    }

    #[test]
    fn export_markdown_composes_all_parts() {
        let meeting = json!({
            "title": "Standup",
            "started_at": "2026-07-13T18:00:00Z",
            "raw_notes": "remember the demo",
            "event_json": { "attendees": [{ "name": "Mayan" }, { "name": "Jasmine" }] },
            "talk_ms": { "me": 250, "them": 750 },
            "segments": [
                { "t0_ms": 1000, "channel": "me", "text": "hi", "speaker": null },
                { "t0_ms": 2000, "channel": "them", "text": "hello", "speaker": "Mayan" },
            ],
            "summaries": [
                { "template": "Meeting", "content_md": "## Summary\nshort." },
            ],
        });
        let md = export_markdown(&meeting);
        assert!(md.starts_with("# Standup"));
        assert!(md.contains("*2026-07-13 · Mayan, Jasmine · you spoke 25%*"));
        assert!(md.contains("## Meeting"));
        assert!(md.contains("### Summary"), "summary headings demote under the tab name");
        assert!(md.contains("- [00:02] **Mayan**: hello"));
        assert!(md.contains("## Your Notes (verbatim)\n\nremember the demo"));
    }

    #[test]
    fn renders_all_section_kinds_in_order() {
        let sections = json!({ "sections": [
            { "heading": "Summary", "kind": "paragraph", "paragraph": "We shipped it." },
            { "heading": "Key Takeaways", "kind": "bullets", "items": ["fast", "local"] },
            { "heading": "Chapters", "kind": "timeline",
              "timeline": [ { "ts": "00:12", "text": "Kickoff" }, { "ts": "[05:30]", "text": "Demo" } ] },
            { "heading": "Action Items", "kind": "todos", "items": ["Me — write the plan by Friday"] },
        ]});
        let md = render_markdown(&sections);
        let idx = |s: &str| md.find(s).unwrap_or(usize::MAX);
        assert!(idx("## Summary") < idx("## Key Takeaways"));
        assert!(idx("## Key Takeaways") < idx("## Chapters"));
        assert!(md.contains("- **[00:12]** Kickoff"));
        assert!(md.contains("- **[05:30]** Demo"), "brackets normalized: {md}");
        assert!(md.contains("- [ ] Me — write the plan by Friday"));
    }

    #[test]
    fn drops_empty_and_degenerate_sections() {
        let sections = json!({ "sections": [
            { "heading": "", "kind": "paragraph", "paragraph": "orphan" },
            { "heading": "Empty", "kind": "bullets", "items": [] },
            { "heading": "Blank", "kind": "paragraph", "paragraph": "  " },
            { "heading": "Real", "kind": "bullets", "items": ["one thing"] },
        ]});
        let md = render_markdown(&sections);
        assert!(!md.contains("orphan"));
        assert!(!md.contains("## Empty"));
        assert!(!md.contains("## Blank"));
        assert!(md.contains("## Real"));
    }

    #[test]
    fn mmss_formats() {
        assert_eq!(mmss(0), "00:00");
        assert_eq!(mmss(65_000), "01:05");
        assert_eq!(mmss(3_725_000), "62:05");
    }

    #[test]
    fn transcript_lines_carry_channel_labels() {
        let segs = vec![
            json!({ "t0_ms": 1000, "channel": "me", "text": "hi", "speaker": null }),
            json!({ "t0_ms": 2000, "channel": "them", "text": "hello", "speaker": null }),
            json!({ "t0_ms": 3000, "channel": "them", "text": "renamed", "speaker": "Ana" }),
        ];
        let t = transcript_text(&segs);
        assert!(t.contains("[00:01] Me: hi"));
        assert!(t.contains("[00:02] Them: hello"));
        assert!(t.contains("[00:03] Ana: renamed"), "diarized name wins: {t}");
    }
}
