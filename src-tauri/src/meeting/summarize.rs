// Meeting summarization: template prompt (PLAUD's model — one free-text prompt
// describing sections) → schema-constrained local LLM call → deterministic
// markdown render → a summary tab + (once per meeting) a searchable note
// projection filed under the 'meetings' category. The meeting remains canonical;
// the projection lets the current search/embeddings/entities paths see it.
//
// ALWAYS chat_json_local_ctx — meeting content never touches the Balanced
// cloud path (same rule as the Journal).

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use super::store;
use crate::db::Db;
use crate::ollama;

/// Long meetings benefit from a coverage pass even when they technically fit in
/// the model context. Smaller, line-aligned chunks keep the beginning, middle,
/// and end from collapsing into one generic paragraph.
const SINGLE_PASS_CHARS: usize = 36_000;
const CHUNK_CHARS: usize = 28_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CoverageLevel {
    Brief,
    Detailed,
    Comprehensive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CoveragePlan {
    level: CoverageLevel,
    duration_min: i64,
    transcript_words: usize,
    minimum_summary_words: usize,
    minimum_detail_points: usize,
}

impl CoveragePlan {
    fn for_segments(segments: &[Value]) -> Self {
        let duration_min = segments
            .iter()
            .filter_map(|segment| segment["t1_ms"].as_i64())
            .max()
            .map(|ms| ((ms.max(0) + 59_999) / 60_000).max(1))
            .unwrap_or(1);
        let transcript_words = segments
            .iter()
            .filter_map(|segment| segment["text"].as_str())
            .map(|text| text.split_whitespace().count())
            .sum();

        if duration_min >= 35 || transcript_words >= 5_000 {
            Self {
                level: CoverageLevel::Comprehensive,
                duration_min,
                transcript_words,
                minimum_summary_words: 650,
                minimum_detail_points: 10,
            }
        } else if duration_min >= 15 || transcript_words >= 1_800 {
            Self {
                level: CoverageLevel::Detailed,
                duration_min,
                transcript_words,
                minimum_summary_words: 350,
                minimum_detail_points: 6,
            }
        } else {
            Self {
                level: CoverageLevel::Brief,
                duration_min,
                transcript_words,
                minimum_summary_words: 0,
                minimum_detail_points: 0,
            }
        }
    }

    fn instructions(self) -> String {
        let depth = match self.level {
            CoverageLevel::Brief => {
                "Keep the result proportionate, but include every decision, commitment, and unresolved question."
            }
            CoverageLevel::Detailed => {
                "Produce detailed notes (roughly 500-900 words when the evidence supports it). Cover at least 6 distinct substantive points across the meeting rather than merging separate topics."
            }
            CoverageLevel::Comprehensive => {
                "Produce comprehensive notes (roughly 900-1,500 words when the evidence supports it). Cover at least 10 distinct substantive points across the opening, middle, and end rather than merging separate topics or workstreams."
            }
        };
        format!(
            "COVERAGE CONTRACT: This meeting is about {} minutes and contains about {} transcript words. {} Every non-overview section must use bullets (or todos/timeline when appropriate). Each discussion bullet should preserve the concrete point plus its stated rationale, constraint, example, or implication; never use vague topic labels. Do not pad, repeat, or invent information to reach a length target.",
            self.duration_min, self.transcript_words, depth
        )
    }

    fn requires_expansion(self, stats: SummaryStats) -> bool {
        self.level != CoverageLevel::Brief
            && (stats.words < self.minimum_summary_words
                || stats.detail_points < self.minimum_detail_points)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SummaryStats {
    words: usize,
    detail_points: usize,
}

pub fn mmss(ms: i64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Render segments as prompt-friendly transcript lines.
fn transcript_text(segments: &[Value], in_person: bool) -> String {
    segments
        .iter()
        .map(|s| {
            let t0 = s["t0_ms"].as_i64().unwrap_or(0);
            let who = if in_person {
                s["speaker"].as_str().unwrap_or("Unassigned")
            } else {
                match s["channel"].as_str().unwrap_or("them") {
                    "me" => "Me",
                    _ => s["speaker"].as_str().unwrap_or("Them"),
                }
            };
            format!(
                "[{}] {}: {}",
                mmss(t0),
                who,
                s["text"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split only between transcript lines so a speaker turn and its timestamp stay
/// together for the evidence-ledger pass.
fn transcript_chunks(transcript: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in transcript.lines() {
        let added = line.len() + usize::from(!current.is_empty());
        if !current.is_empty() && current.len() + added > CHUNK_CHARS {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn summary_stats(sections: &Value) -> SummaryStats {
    let mut stats = SummaryStats::default();
    for (index, section) in sections["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(paragraph) = section["paragraph"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
        {
            stats.words += paragraph.split_whitespace().count();
            if index > 0 {
                stats.detail_points += 1;
            }
        }
        for item in section["items"].as_array().into_iter().flatten() {
            if let Some(text) = item
                .as_str()
                .or_else(|| item["text"].as_str())
                .filter(|text| !text.trim().is_empty())
            {
                stats.words += text.split_whitespace().count();
                stats.detail_points += 1;
            }
        }
        for item in section["timeline"].as_array().into_iter().flatten() {
            if let Some(text) = item["text"].as_str().filter(|text| !text.trim().is_empty()) {
                stats.words += text.split_whitespace().count();
                stats.detail_points += 1;
            }
        }
    }
    stats
}

fn richer_summary(first: Value, candidate: Value) -> Value {
    let first_stats = summary_stats(&first);
    let candidate_stats = summary_stats(&candidate);
    let first_score = first_stats.words + first_stats.detail_points * 35;
    let candidate_score = candidate_stats.words + candidate_stats.detail_points * 35;
    if candidate_score > first_score {
        candidate
    } else {
        first
    }
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
    let in_person = meeting["capture_mode"].as_str() == Some("in_person");

    let mut md = format!("# {title}\n\n");
    let mut meta: Vec<String> = Vec::new();
    if !date.is_empty() {
        meta.push(date);
    }
    if !attendees.is_empty() {
        meta.push(attendees.join(", "));
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
        md.push_str(&format!(
            "\n---\n\n## Your Notes (verbatim)\n\n{}\n",
            raw_notes.trim()
        ));
    }
    if !segments.is_empty() {
        md.push_str("\n---\n\n## Transcript\n\n");
        for s in segments {
            let who = if in_person {
                s["speaker"].as_str().unwrap_or("Unassigned")
            } else {
                match s["channel"].as_str().unwrap_or("them") {
                    "me" => "Me",
                    _ => s["speaker"].as_str().unwrap_or("Them"),
                }
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
    external_attendees_excluding(event_json, &HashSet::new())
}

/// Presentable remote participants, excluding every identity known to belong
/// to the note-taker. Google only marks one attendee `self` when the same
/// invitation spans multiple connected accounts, so the source account and
/// user-configured filing identities are equally authoritative exclusions.
pub fn external_attendees_excluding(
    event_json: &Value,
    configured_owner_emails: &HashSet<String>,
) -> Vec<String> {
    let Some(arr) = event_json.get("attendees").and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    let mut owner_emails = configured_owner_emails.clone();
    if let Some(account) = event_json.get("account").and_then(|a| a.as_str()) {
        let account = account.trim().to_lowercase();
        if !account.is_empty() {
            owner_emails.insert(account);
        }
    }
    for attendee in arr {
        if attendee
            .get("self")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
        {
            if let Some(email) = attendee.get("email").and_then(|e| e.as_str()) {
                owner_emails.insert(email.trim().to_lowercase());
            }
        }
    }

    let mut seen = HashSet::new();
    arr.iter()
        .filter(|a| !a.get("self").and_then(|s| s.as_bool()).unwrap_or(false))
        .filter(|a| !a.get("resource").and_then(Value::as_bool).unwrap_or(false))
        .filter(|a| {
            !a.get("status")
                .or_else(|| a.get("responseStatus"))
                .or_else(|| a.get("response_status"))
                .and_then(Value::as_str)
                .is_some_and(|status| status.eq_ignore_ascii_case("declined"))
        })
        .filter_map(|a| {
            if let Some(s) = a.as_str() {
                let normalized = s.trim().to_lowercase();
                if s.contains('@') && owner_emails.contains(&normalized) {
                    return None;
                }
                return Some((normalized, humanize(s)));
            }
            let email = a
                .get("email")
                .and_then(|v| v.as_str())
                .map(|email| email.trim().to_lowercase())
                .unwrap_or_default();
            if !email.is_empty() && owner_emails.contains(&email) {
                return None;
            }
            let name = a
                .get("name")
                .or_else(|| a.get("displayName"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(humanize)
                .or_else(|| a.get("email").and_then(|v| v.as_str()).map(humanize))?;
            let key = if email.is_empty() {
                name.to_lowercase()
            } else {
                email
            };
            Some((key, name))
        })
        .filter(|(key, name)| !name.is_empty() && seen.insert(key.clone()))
        .map(|(_, name)| name)
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
                        "sources": { "type": "array", "items": { "type": "string" } },
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string" },
                                    "source": { "type": "string" }
                                },
                                "required": ["text", "source"]
                            }
                        },
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

fn normalized_source(
    raw: &str,
    valid_timestamps: Option<&HashSet<String>>,
    allow_notes: bool,
) -> Option<String> {
    let source = raw.trim();
    if source.eq_ignore_ascii_case("notes") {
        return allow_notes.then(|| "[notes]".to_string());
    }
    let timestamp = source.trim_matches(['[', ']']);
    if timestamp.is_empty() || valid_timestamps.is_some_and(|valid| !valid.contains(timestamp)) {
        return None;
    }
    Some(format!("[{timestamp}]"))
}

fn item_line(
    item: &Value,
    valid_timestamps: Option<&HashSet<String>>,
    allow_notes: bool,
) -> Option<String> {
    // String items are retained for custom templates and summaries generated by
    // older builds. New structured output always uses { text, source }.
    if let Some(text) = item.as_str().map(str::trim).filter(|text| !text.is_empty()) {
        return Some(text.to_string());
    }
    let text = item.get("text")?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    let source = item
        .get("source")
        .and_then(Value::as_str)
        .and_then(|source| normalized_source(source, valid_timestamps, allow_notes));
    Some(match source {
        Some(source) => format!("{text} {source}"),
        None => text.to_string(),
    })
}

fn clean_action_owner_prefix(text: &str) -> String {
    let text = text.trim();
    let lower = text.to_lowercase();
    for prefix in ["owner — ", "owner – ", "owner - ", "owner: "] {
        if lower.starts_with(prefix) {
            let remainder = text[prefix.len()..].trim();
            if split_owner_task(remainder).is_some() {
                return remainder.to_string();
            }
        }
    }
    text.to_string()
}

fn split_owner_task(text: &str) -> Option<(&str, &str)> {
    for separator in [" — ", " – ", " - "] {
        if let Some((owner, task)) = text.split_once(separator) {
            let owner = owner.trim();
            let task = task.trim();
            if !owner.is_empty() && !task.is_empty() {
                return Some((owner, task));
            }
        }
    }
    None
}

fn canonical_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| !word.is_empty())
        .collect()
}

fn item_source(
    item: &Value,
    valid_timestamps: Option<&HashSet<String>>,
    allow_notes: bool,
) -> Option<String> {
    item.get("source")
        .and_then(Value::as_str)
        .and_then(|source| normalized_source(source, valid_timestamps, allow_notes))
}

fn decision_repeats_action(decision: &str, action: &str) -> bool {
    let Some((owner, task)) = split_owner_task(action) else {
        return false;
    };
    let decision = canonical_words(decision);
    if decision.len() < 4 {
        return false;
    }
    decision == canonical_words(&format!("{owner} {task}"))
        || decision == canonical_words(&format!("{owner} will {task}"))
}

/// Small local models sometimes repeat a commitment as a decision even when the
/// prompt forbids it. Remove only exact, source-aligned wording variants and clean
/// a redundant literal schema label; broader meaning stays exactly as written.
fn enforce_summary_quality(
    sections: &mut Value,
    valid_timestamps: Option<&HashSet<String>>,
    allow_notes: bool,
) {
    let Some(list) = sections.get_mut("sections").and_then(Value::as_array_mut) else {
        return;
    };

    for section in list
        .iter_mut()
        .filter(|section| section.get("kind").and_then(Value::as_str) == Some("todos"))
    {
        let Some(items) = section.get_mut("items").and_then(Value::as_array_mut) else {
            continue;
        };
        for item in items {
            let Some(text) = item.get("text").and_then(Value::as_str) else {
                continue;
            };
            item["text"] = Value::String(clean_action_owner_prefix(text));
        }
    }

    let actions = list
        .iter()
        .filter(|section| section.get("kind").and_then(Value::as_str) == Some("todos"))
        .flat_map(|section| {
            section
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|item| {
            let text = item.get("text").and_then(Value::as_str)?.to_string();
            let source = item_source(item, valid_timestamps, allow_notes)?;
            Some((text, source))
        })
        .collect::<Vec<_>>();

    if actions.is_empty() {
        return;
    }

    for section in list.iter_mut().filter(|section| {
        section
            .get("heading")
            .and_then(Value::as_str)
            .is_some_and(|heading| {
                matches!(
                    heading.trim().to_lowercase().as_str(),
                    "decision" | "decisions"
                )
            })
            && section.get("kind").and_then(Value::as_str) != Some("todos")
    }) {
        let Some(items) = section.get_mut("items").and_then(Value::as_array_mut) else {
            continue;
        };
        items.retain(|item| {
            let Some(text) = item.get("text").and_then(Value::as_str) else {
                return true;
            };
            let Some(source) = item_source(item, valid_timestamps, allow_notes) else {
                return true;
            };
            !actions.iter().any(|(action_text, action_source)| {
                source == *action_source && decision_repeats_action(text, action_text)
            })
        });
    }
}

/// Deterministic markdown from the model's sections. Empty/degenerate sections
/// are dropped, so a template section with nothing to say simply vanishes
/// (matching the prompt's "omit if nothing" rule even when the model doesn't).
pub fn render_markdown(sections: &Value) -> String {
    render_markdown_with_sources(sections, None, true)
}

fn render_markdown_with_sources(
    sections: &Value,
    valid_timestamps: Option<&HashSet<String>>,
    allow_notes: bool,
) -> String {
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
            "paragraph" => {
                let paragraph = sec["paragraph"].as_str().unwrap_or("").trim();
                if paragraph.is_empty() {
                    String::new()
                } else {
                    let sources = sec["sources"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter_map(|source| {
                            normalized_source(source, valid_timestamps, allow_notes)
                        })
                        .take(4)
                        .collect::<Vec<_>>();
                    if sources.is_empty() {
                        paragraph.to_string()
                    } else {
                        format!("{paragraph} {}", sources.join(" "))
                    }
                }
            }
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
                            } else {
                                let source = normalized_source(ts, valid_timestamps, allow_notes);
                                Some(match source {
                                    Some(source) => format!("- {source} {text}"),
                                    None => format!("- {text}"),
                                })
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
                        .filter_map(|item| item_line(item, valid_timestamps, allow_notes))
                        .map(|text| format!("- [ ] {text}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            _ => sec["items"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item_line(item, valid_timestamps, allow_notes))
                        .map(|text| format!("- {text}"))
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
- Write for usefulness, not coverage theater. Lead with the conclusion a person can \
act on, then preserve the evidence, rationale, constraint, or consequence that makes \
it meaningful. Avoid empty openings such as 'the team discussed', 'the group reviewed', \
or a list of topics with no outcome.\n\
- The opening paragraph is the meeting's readout. State what materially changed or \
became clearer, why that matters when the source says why, and the most consequential \
next move or unresolved condition. If nothing changed, say that plainly.\n\
- Surface grounded insight: explicit tradeoffs, tensions between options, hidden \
dependencies, contradictions, second-order consequences, and patterns across separate \
moments in the meeting. Connect two facts only when the source supports the relationship; \
otherwise keep them separate. Never manufacture a clever take.\n\
- Every bullets/todos item is an object {\"text\",\"source\"}. source is either an \
exact mm:ss timestamp printed at the start of a transcript line, or the word notes \
when the claim comes only from typed notes. Never invent or approximate a timestamp.\n\
- A paragraph section includes sources with up to four exact timestamps/notes that \
best support its claims. Timeline ts follows the same exact-timestamp rule.\n\
- Use kind='paragraph' for the opening overview/summary/status/outcome/thesis. Use \
kind='bullets' for other non-action sections unless the template requests a timeline.\n\
- Sections that list actions, commitments, follow-ups, next steps, or experiments \
use kind='todos'.\n\
- Action-item text is '<actual owner name> — verb phrase', with 'by <date>' only when \
a deadline was explicitly stated. Never emit the literal label 'Owner'. A speaker saying \
'I will' owns that task. Otherwise use 'Unassigned' — never use Them or guess an owner. \
Include both explicit commitments \
and concrete follow-up work described as needed, required, still to do, check, confirm, \
review, send, or follow up, even when nobody committed and no owner exists.\n\
- A decision is chosen direction, not merely a task or promise. Do not copy an \
action item into Decisions just because its owner committed to it.\n\
- If the meeting has nothing for a section, omit that section entirely.\n\
- Start with the outcome or current state, then decisions, commitments, important \
reasoning, and unresolved questions. The opening overview may synthesize the most \
important outcomes; after it, place each detail in one best section only. Never put a \
scheduled or owned follow-up under Open Questions & Risks — describe only the unresolved \
condition it mitigates there.\n\
- The notes must stand alone and be complete enough to replace replaying the meeting. \
Preserve concrete facts, examples, reasoning, tradeoffs, constraints, objections, and \
unresolved issues. Do not collapse separate topics or workstreams into a generic recap.\n\
- Scale detail to the meeting: a 5-minute check-in earns a few lines; a 40-60 minute \
discussion normally earns multiple substantive bullets for every topic that materially \
advanced. Length must come from grounded specificity, never filler or transcript rewriting.\n\
- Describe what was said and decided. Do not infer mood, enthusiasm, engagement, \
agreement, or other internal states.\n\
Respond ONLY with JSON: {\"sections\":[{\"heading\",\"kind\":\"paragraph|bullets|timeline|todos\",\
\"paragraph\"?,\"sources\"?:[\"mm:ss|notes\"],\"items\"?:[{\"text\",\"source\"}],\
\"timeline\"?:[{\"ts\",\"text\"}]}]}";

/// Turn one long-meeting chunk into a dense evidence ledger. The final pass can
/// then synthesize without losing topics that appeared far apart in the source.
async fn condense_chunk(chunk: &str) -> Result<String> {
    let minimum_points = (chunk.split_whitespace().count() / 350).clamp(8, 18);
    let schema = json!({
        "type": "object",
        "properties": { "facts": {
            "type": "array",
            "minItems": minimum_points,
            "maxItems": minimum_points + 10,
            "items": { "type": "string" }
        } },
        "required": ["facts"]
    });
    let instruction = format!(
        "Build a dense evidence ledger from this portion of a longer meeting. Return \
         {minimum_points}-{} specific facts, covering the beginning, middle, and end \
         of this chunk. Every fact starts with an exact [mm:ss] timestamp copied from \
         the source. Preserve speaker names when relevant, concrete examples, numbers, \
         options, reasoning, constraints, decisions, commitments with owners, and open \
         questions. Keep separate topics separate. Do not write an overview or merge the \
         chunk into a few generic themes. Respond ONLY with JSON {{\"facts\":[...]}}.",
        minimum_points + 10
    );
    let out = ollama::chat_json_local_ctx(
        &ollama::text_model(),
        &instruction,
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

/// Generate or refresh one template summary for a meeting. Files the meeting
/// note on first summarize; later runs replace that template's existing tab.
pub async fn run(
    app: &tauri::AppHandle,
    meeting_id: i64,
    template_name: Option<String>,
) -> Result<String> {
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
    let coverage = CoveragePlan::for_segments(&segments);
    let coverage_instructions = coverage.instructions();

    let mut transcript = transcript_text(
        &segments,
        meeting["capture_mode"].as_str() == Some("in_person"),
    );
    if transcript.len() > SINGLE_PASS_CHARS {
        // Map-reduce: make a timestamped evidence ledger, then synthesize it.
        let mut condensed = String::new();
        for (index, chunk) in transcript_chunks(&transcript).into_iter().enumerate() {
            condensed.push_str(&format!("\nSOURCE PART {}:\n", index + 1));
            condensed.push_str(&condense_chunk(&chunk).await?);
            condensed.push('\n');
        }
        transcript = format!(
            "TIMESTAMPED SOURCE LEDGER (extracted across the complete transcript; \
             treat each line as meeting evidence):\n{condensed}"
        );
    }

    let user = format!(
        "{coverage_instructions}\n\n\
         TEMPLATE — produce these sections, in this order:\n{prompt}\n\n\
         MEETING: {title}{}{}\n\n\
         MY TYPED NOTES:\n{}\n\n\
         TRANSCRIPT OR SOURCE LEDGER:\n{transcript}\n\n\
         END OF SOURCE.\n\n\
         FINAL COVERAGE CHECK: {coverage_instructions} Before responding, verify that \
         you covered distinct substantive material from the full time range, captured \
         every grounded decision/action/open question, and used bullets for every \
         non-overview section. Return the replacement notes only.",
        if date.is_empty() {
            String::new()
        } else {
            format!(" ({date})")
        },
        if attendees.is_empty() {
            String::new()
        } else {
            format!("\nATTENDEES: {}", attendees.join(", "))
        },
        if raw_notes.trim().is_empty() {
            "(none)"
        } else {
            raw_notes.trim()
        },
    );

    let mut out = ollama::chat_json_local_ctx(
        &ollama::text_model(),
        SYSTEM,
        &user,
        None,
        Some(sections_schema()),
        24_576,
    )
    .await?;
    let first_stats = summary_stats(&out);
    if coverage.requires_expansion(first_stats) {
        let repair_user = format!(
            "{user}\n\nYour first draft was too compressed: it contained about {} words \
             and {} substantive detail points. Regenerate it from the complete source \
             as a fuller replacement. Preserve more concrete rationale, examples, \
             constraints, topic progression, decisions, commitments, and unresolved \
             questions. Do not add filler or unsupported claims.",
            first_stats.words, first_stats.detail_points
        );
        match ollama::chat_json_local_ctx(
            &ollama::text_model(),
            SYSTEM,
            &repair_user,
            None,
            Some(sections_schema()),
            24_576,
        )
        .await
        {
            Ok(candidate) => out = richer_summary(out, candidate),
            Err(error) => eprintln!("[noted] meeting summary expansion pass failed: {error}"),
        }
    }
    let valid_timestamps = segments
        .iter()
        .filter_map(|segment| segment["t0_ms"].as_i64())
        .map(mmss)
        .collect::<HashSet<_>>();
    enforce_summary_quality(
        &mut out,
        Some(&valid_timestamps),
        !raw_notes.trim().is_empty(),
    );
    let md =
        render_markdown_with_sources(&out, Some(&valid_timestamps), !raw_notes.trim().is_empty());
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

    let mut note_text = format!("# {title}\n\n{md}");
    if !raw_notes.trim().is_empty() {
        note_text.push_str(&format!(
            "\n\n## Your Notes (verbatim)\n\n{}",
            raw_notes.trim()
        ));
    }

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
        let entities: Vec<Value> = attendees
            .iter()
            .map(|n| json!({ "name": n, "type": "person" }))
            .collect();
        let route_status = meeting["route_status"].as_str();
        let route_folder_id = matches!(route_status, Some("matched" | "manual"))
            .then(|| meeting["route_folder_id"].as_i64())
            .flatten();
        // An exact approved meeting rule owns context as well as destination.
        // Without one, use the Work/Personal choice captured when recording
        // began; never consult whichever context happens to be active now.
        let filing_context = if let Some(folder_id) = route_folder_id {
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            Some(crate::db::note_folder_context(&conn, folder_id)?)
        } else {
            meeting["filing_context"].as_str().map(String::from)
        };
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
            "filing_context": filing_context,
            "folder_id": route_folder_id,
            "filing_source": route_folder_id.map(|_| {
                if route_status == Some("manual") { "manual" } else { "rule" }
            }),
        });
        match serde_json::from_value::<crate::SaveArgs>(save_json) {
            Ok(args) => match crate::save_entry(app.clone(), args).await {
                Ok(note_id) => {
                    let state = app.state::<Db>();
                    let conn = state.0.lock().unwrap();
                    if let Err(error) =
                        store::set_note_id_and_apply_route(&conn, meeting_id, note_id, &now)
                    {
                        eprintln!("[noted] meeting route filing failed: {error}");
                        // Preserve the core meeting → note link even if a
                        // destination disappeared during summarization.
                        let _ = store::set_note_id(&conn, meeting_id, note_id);
                    }
                }
                Err(e) => eprintln!("[noted] meeting note filing failed: {e}"),
            },
            Err(e) => eprintln!("[noted] meeting note args invalid: {e}"),
        }
    } else if template == super::cfg().default_template {
        // A refreshed primary summary (including a speaker repair) must also
        // refresh the searchable note; otherwise a bad old name survives in
        // Notes, semantic search, and the knowledge graph.
        if let Some(note_id) = meeting["note_id"].as_i64() {
            {
                let state = app.state::<Db>();
                let conn = state.0.lock().unwrap();
                crate::db::refresh_note_text(&conn, note_id, &note_text)?;
            }
            if let Ok(v) = ollama::embed(&note_text).await {
                let state = app.state::<Db>();
                let conn = state.0.lock().unwrap();
                let _ = crate::db::insert_embedding(&conn, note_id, &crate::normalize(v));
            }
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

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// Compose one knowledge source for one meeting. The generated summary and the
/// user's own notes have different provenance, but they share a meeting root
/// and must never become two competing graph sources.
fn knowledge_source(summary_md: &str, raw_notes: &str) -> String {
    let summary = bounded_text(summary_md.trim(), 12_000);
    let notes = bounded_text(raw_notes.trim(), 6_000);
    match (summary.is_empty(), notes.is_empty()) {
        (false, false) => format!(
            "GENERATED MEETING SUMMARY:\n{summary}\n\nUSER-AUTHORED MEETING NOTES (verbatim):\n{notes}"
        ),
        (false, true) => format!("GENERATED MEETING SUMMARY:\n{summary}"),
        (true, false) => format!("USER-AUTHORED MEETING NOTES (verbatim):\n{notes}"),
        (true, true) => String::new(),
    }
}

/// Post-summarize knowledge pass: mine the generated summary and the user's
/// meeting-owned notes for projects, orgs, and topics, then attach every mention
/// to the meeting's single filed note. Local model only, like everything
/// meeting-side.
pub async fn extract_knowledge(app: &tauri::AppHandle, meeting_id: i64) -> Result<usize> {
    let (note_id, title, date, attendees, speakers, summary_md, raw_notes) = {
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
            .filter(|l| !l.is_empty() && *l != "Me" && *l != "Them" && !l.starts_with("Speaker "))
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
        let raw_notes = meeting["raw_notes"].as_str().unwrap_or("").to_string();
        (note_id, title, date, attendees, speakers, md, raw_notes)
    };
    let body = knowledge_source(&summary_md, &raw_notes);
    if body.is_empty() {
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
    let system = "You mine one meeting record for knowledge-graph entities. The input may \
        contain a generated summary and the user's verbatim notes; both belong to the same \
        meeting, not separate records. Extract the \
        distinct projects, organizations/companies/products, and recurring topics the \
        meeting actually discussed. Rules: 3-12 entities total, most important first; \
        names are short canonical noun phrases of 1-4 words (never a sentence); skip \
        generic words (meeting, update, team, discussion), dates, and people — people \
        are handled separately. For each entity give a one-sentence fact stating what \
        THIS meeting said about it. JSON: {\"entities\":[{\"name\",\"type\",\"fact\"}]}";
    let user = format!(
        "Meeting: {title} ({date})\nAttendees: {}\n\n{body}",
        if attendees.is_empty() {
            "(unknown)".to_string()
        } else {
            attendees.join(", ")
        }
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
                    fact: e["fact"]
                        .as_str()
                        .map(|f| f.trim().to_string())
                        .filter(|f| !f.is_empty()),
                    relationship: None,
                })
            }),
    );
    if candidates.is_empty() {
        return Ok(0);
    }
    let now = chrono::Utc::now().to_rfc3339();
    let added = crate::persist_entities(app, note_id, &date, &title, &now, candidates, true).await;
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
        let ev3 = json!({ "attendees": [
            { "name": "Declined", "status": "declined" },
            { "name": "Room", "resource": true },
            { "name": "Present", "status": "accepted" }
        ]});
        assert_eq!(external_attendees(&ev3), vec!["Present"]);
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
        assert!(md.contains("*2026-07-13 · Mayan, Jasmine*"));
        assert!(md.contains("## Meeting"));
        assert!(
            md.contains("### Summary"),
            "summary headings demote under the tab name"
        );
        assert!(md.contains("- [00:02] **Mayan**: hello"));
        assert!(md.contains("## Your Notes (verbatim)\n\nremember the demo"));
    }

    #[test]
    fn knowledge_source_keeps_user_notes_inside_the_meeting_record() {
        let source = knowledge_source(
            "The launch remains on Tuesday.",
            "Project Atlas is the risk I want to remember.",
        );
        assert!(source.contains("GENERATED MEETING SUMMARY:\nThe launch remains on Tuesday."));
        assert!(source.contains(
            "USER-AUTHORED MEETING NOTES (verbatim):\nProject Atlas is the risk I want to remember."
        ));
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
        assert!(md.contains("- [00:12] Kickoff"));
        assert!(md.contains("- [05:30] Demo"), "brackets normalized: {md}");
        assert!(md.contains("- [ ] Me — write the plan by Friday"));
    }

    #[test]
    fn renders_only_real_transcript_sources() {
        let sections = json!({ "sections": [
            {
                "heading": "Overview", "kind": "paragraph",
                "paragraph": "The rollout stays on Friday.",
                "sources": ["00:12", "99:99", "notes"]
            },
            {
                "heading": "Decisions", "kind": "bullets",
                "items": [
                    { "text": "Keep the Friday rollout.", "source": "[00:12]" },
                    { "text": "Do not invent evidence.", "source": "01:30" }
                ]
            },
            {
                "heading": "Actions", "kind": "todos",
                "items": [{ "text": "Unassigned — confirm the owner", "source": "notes" }]
            }
        ]});
        let valid = HashSet::from(["00:12".to_string()]);
        let md = render_markdown_with_sources(&sections, Some(&valid), true);
        assert!(md.contains("Friday. [00:12] [notes]"));
        assert!(md.contains("Keep the Friday rollout. [00:12]"));
        assert!(md.contains("Do not invent evidence."));
        assert!(!md.contains("99:99"));
        assert!(!md.contains("01:30"));
        assert!(md.contains("Unassigned — confirm the owner [notes]"));

        let without_typed_notes = render_markdown_with_sources(&sections, Some(&valid), false);
        assert!(!without_typed_notes.contains("[notes]"));
        assert!(without_typed_notes.contains("Unassigned — confirm the owner"));
    }

    #[test]
    fn quality_guard_removes_only_duplicate_actions_from_decisions() {
        let mut sections = json!({ "sections": [
            {
                "heading": "Decisions", "kind": "bullets",
                "items": [
                    {
                        "text": "Keep Friday as the release date for two partners.",
                        "source": "00:00"
                    },
                    {
                        "text": "Priya will finish the onboarding copy and send it for final review by Thursday at noon.",
                        "source": "03:50"
                    }
                ]
            },
            {
                "heading": "Action Items", "kind": "todos",
                "items": [
                    {
                        "text": "Owner — Priya — Finish the onboarding copy and send it for final review by Thursday at noon.",
                        "source": "03:50"
                    },
                    {
                        "text": "Unassigned — Confirm migration capacity with engineering.",
                        "source": "notes"
                    }
                ]
            }
        ]});

        let valid = HashSet::from(["00:00".to_string(), "03:50".to_string()]);
        enforce_summary_quality(&mut sections, Some(&valid), true);
        let md = render_markdown(&sections);

        assert!(md.contains("Keep Friday as the release date"));
        assert_eq!(md.matches("Priya — Finish the onboarding copy").count(), 1);
        assert!(!md.contains("Owner — Priya"));
        assert!(md.contains("Unassigned — Confirm migration capacity"));
    }

    #[test]
    fn quality_guard_keeps_related_but_distinct_decisions() {
        let mut sections = json!({ "sections": [
            {
                "heading": "Decisions", "kind": "bullets",
                "items": [{
                    "text": "The migration capacity check is a release-critical path.",
                    "source": "02:30"
                }]
            },
            {
                "heading": "Action Items", "kind": "todos",
                "items": [{
                    "text": "Unassigned — Confirm migration capacity with engineering tomorrow.",
                    "source": "02:30"
                }]
            }
        ]});

        let valid = HashSet::from(["02:30".to_string()]);
        enforce_summary_quality(&mut sections, Some(&valid), false);
        let md = render_markdown(&sections);

        assert!(md.contains("migration capacity check is a release-critical path"));
        assert!(md.contains("Confirm migration capacity with engineering"));
    }

    #[test]
    fn quality_guard_requires_same_valid_source_and_plain_decisions_heading() {
        let mut sections = json!({ "sections": [
            {
                "heading": "Decisions", "kind": "bullets",
                "items": [{
                    "text": "Priya will finish the onboarding copy.",
                    "source": "00:10"
                }]
            },
            {
                "heading": "Decisions & Commitments", "kind": "bullets",
                "items": [{
                    "text": "Priya will finish the onboarding copy.",
                    "source": "00:20"
                }]
            },
            {
                "heading": "Action Items", "kind": "todos",
                "items": [{
                    "text": "Priya — Finish the onboarding copy.",
                    "source": "00:20"
                }]
            }
        ]});
        let valid = HashSet::from(["00:10".to_string(), "00:20".to_string()]);

        enforce_summary_quality(&mut sections, Some(&valid), false);
        let md = render_markdown(&sections);

        assert_eq!(
            md.matches("Priya will finish the onboarding copy").count(),
            2
        );
        assert!(md.contains("Priya — Finish the onboarding copy"));
    }

    #[test]
    fn quality_guard_leaves_malformed_owner_labels_untouched() {
        let mut sections = json!({ "sections": [{
            "heading": "Action Items", "kind": "todos",
            "items": [
                { "text": "Owner — Confirm capacity", "source": "00:10" },
                { "text": "Owner: Unassigned — Confirm the migration capacity", "source": "00:20" }
            ]
        }]});
        let valid = HashSet::from(["00:10".to_string(), "00:20".to_string()]);

        enforce_summary_quality(&mut sections, Some(&valid), false);
        let md = render_markdown(&sections);

        assert!(md.contains("Owner — Confirm capacity"));
        assert!(md.contains("Unassigned — Confirm the migration capacity"));
        assert!(!md.contains("Owner: Unassigned"));
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
        let t = transcript_text(&segs, false);
        assert!(t.contains("[00:01] Me: hi"));
        assert!(t.contains("[00:02] Them: hello"));
        assert!(
            t.contains("[00:03] Ana: renamed"),
            "diarized name wins: {t}"
        );

        let room = transcript_text(&segs, true);
        assert!(room.contains("[00:01] Unassigned: hi"));
        assert!(room.contains("[00:03] Ana: renamed"));
    }

    #[test]
    fn long_meetings_receive_comprehensive_coverage_requirements() {
        let segments = vec![json!({
            "t0_ms": 0,
            "t1_ms": 2_993_880,
            "text": "word ".repeat(8_108),
        })];
        let coverage = CoveragePlan::for_segments(&segments);

        assert_eq!(coverage.level, CoverageLevel::Comprehensive);
        assert_eq!(coverage.duration_min, 50);
        assert_eq!(coverage.transcript_words, 8_108);
        assert!(coverage.instructions().contains("900-1,500 words"));
        assert!(coverage.instructions().contains("at least 10 distinct"));

        let sparse = json!({ "sections": [
            { "heading": "Overview", "kind": "paragraph", "paragraph": "A very short overview." },
            { "heading": "Actions", "kind": "todos", "items": [
                { "text": "Franek — revise the hero", "source": "40:18" }
            ] }
        ]});
        assert!(coverage.requires_expansion(summary_stats(&sparse)));
    }

    #[test]
    fn short_meetings_do_not_get_padded_by_the_expansion_guard() {
        let segments = vec![json!({
            "t0_ms": 0,
            "t1_ms": 240_000,
            "text": "Quick status and one follow-up."
        })];
        let coverage = CoveragePlan::for_segments(&segments);
        assert_eq!(coverage.level, CoverageLevel::Brief);
        assert!(!coverage.requires_expansion(SummaryStats::default()));
    }

    #[test]
    fn long_transcript_chunks_keep_complete_timestamped_lines() {
        let line = format!("[00:01] Ana: {}", "specific detail ".repeat(260));
        let transcript = std::iter::repeat(line.clone())
            .take(9)
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = transcript_chunks(&transcript);

        assert!(chunks.len() > 1);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.lines().all(|part| part == line)));
    }
}
