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
(lines look like '[mm:ss] Me: ...' — 'Me' is the note-taker's own mic, 'Them' is \
everyone else via system audio), the note-taker's own typed notes, and a template \
describing the sections to produce.\n\
Rules:\n\
- Ground every statement in the transcript or the typed notes. Never invent facts, \
names, numbers, or dates.\n\
- The typed notes are the highest-priority signal: every point in them must be \
reflected and expanded with context from the transcript.\n\
- Timeline sections use kind='timeline' with ts set to a [mm:ss] timestamp that \
actually appears in the transcript.\n\
- Action items use kind='todos', each item shaped 'Owner — verb phrase' with \
'by <date>' appended when a deadline was stated. Owner is Me, Them, or a stated name.\n\
- If the meeting has nothing for a section, omit that section entirely.\n\
- Be concrete and terse. Quote short phrases where wording matters.\n\
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
        ollama::TEXT_MODEL,
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

/// Generate one summary tab for a meeting. Files the meeting note on first
/// summarize; later regenerations only add tabs. Returns the markdown.
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
        ollama::TEXT_MODEL,
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
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;

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
