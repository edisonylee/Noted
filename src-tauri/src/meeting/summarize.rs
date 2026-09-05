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
const SINGLE_PASS_CHARS: usize = 24_000;
const CHUNK_CHARS: usize = 12_000;

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
                // A hard 800-word floor still prevents a one-page collapse while
                // leaving room for compact tables and source ranges. The prompt
                // continues to target 1,400-2,200 words when the evidence supports it.
                minimum_summary_words: 800,
                minimum_detail_points: 18,
            }
        } else if duration_min >= 15 || transcript_words >= 1_800 {
            Self {
                level: CoverageLevel::Detailed,
                duration_min,
                transcript_words,
                minimum_summary_words: 500,
                minimum_detail_points: 8,
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
                "Produce detailed notes (roughly 700-1,200 words when the evidence supports it). Cover at least 8 distinct substantive points across the meeting rather than merging separate topics."
            }
            CoverageLevel::Comprehensive => {
                "Produce a comprehensive meeting pack (normally 1,400-2,200 words, and longer when the source warrants it). Cover at least 18 distinct substantive points across the opening, middle, and end rather than merging separate topics or workstreams. There is no page limit."
            }
        };
        format!(
            "COVERAGE CONTRACT: This meeting is about {} minutes and contains about {} transcript words. {} Each detailed discussion point must preserve the concrete fact plus its stated rationale, constraint, example, or implication; never use vague topic labels. Do not pad, repeat, or invent information to reach a length target.",
            self.duration_min, self.transcript_words, depth
        )
    }

    #[cfg(test)]
    fn requires_expansion(self, stats: SummaryStats) -> bool {
        self.level != CoverageLevel::Brief
            && (stats.words < self.minimum_summary_words
                || stats.detail_points < self.minimum_detail_points)
    }
}

#[cfg(test)]
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
#[cfg(test)]
fn transcript_text(segments: &[Value], in_person: bool) -> String {
    transcript_text_with_remote_alias(segments, in_person, None)
}

fn transcript_text_with_remote_alias(
    segments: &[Value],
    in_person: bool,
    remote_alias: Option<&str>,
) -> String {
    segments
        .iter()
        .map(|s| {
            let t0 = s["t0_ms"].as_i64().unwrap_or(0);
            let who = if in_person {
                s["speaker"].as_str().unwrap_or("Unassigned")
            } else {
                match s["channel"].as_str().unwrap_or("them") {
                    "me" => "Me",
                    _ => remote_alias
                        .or_else(|| s["speaker"].as_str())
                        .unwrap_or("Them"),
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

#[cfg(test)]
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

/// A title such as "1:1 with Chris" is explicit participant metadata even when
/// the recording was started outside a calendar event. In that narrow case it
/// is safer and more useful than carrying several diarization fragments through
/// the notes as Speaker 1, Speaker 2, and so on.
fn one_on_one_title_participant(title: &str) -> Option<String> {
    let trimmed = title.trim();
    let lower = trimmed.to_lowercase();
    let marker = "1:1 with ";
    let start = lower.find(marker)? + marker.len();
    let candidate = trimmed[start..]
        .split(['|', '—', '-', ':'])
        .next()
        .unwrap_or("")
        .trim();
    let mut words = candidate.split_whitespace().collect::<Vec<_>>();
    while words.last().is_some_and(|word| {
        matches!(
            word.trim_matches(|ch: char| !ch.is_alphanumeric())
                .to_lowercase()
                .as_str(),
            "onboarding" | "meeting" | "sync" | "checkin" | "catchup"
        )
    }) {
        words.pop();
    }
    let candidate = words.join(" ");
    if candidate.is_empty()
        || candidate.split_whitespace().count() > 4
        || candidate.eq_ignore_ascii_case("me")
    {
        None
    } else {
        Some(candidate)
    }
}

fn meeting_participants(title: &str, event_json: &Value) -> (Vec<String>, Option<String>) {
    let attendees = attendee_names(event_json);
    if !attendees.is_empty() {
        let sole = (attendees.len() == 1).then(|| attendees[0].clone());
        return (attendees, sole);
    }
    match one_on_one_title_participant(title) {
        Some(name) => (vec![name.clone()], Some(name)),
        None => (Vec::new(), None),
    }
}

fn timestamp_schema() -> Value {
    // Keep structured decoding on the same canonical shape as transcript_text.
    // The deterministic normalizer below remains the final authority because
    // not every local model honors every JSON Schema keyword consistently.
    json!({
        "type": "string",
        "pattern": "^([0-9]{2,}:[0-5][0-9]|notes)$"
    })
}

fn source_range_schema() -> Value {
    let timestamp = timestamp_schema();
    json!({
        "type": "object",
        "properties": {
            "start": timestamp.clone(),
            "end": timestamp
        },
        "required": ["start", "end"]
    })
}

/// Canonical Meeting Pack output. Every template uses this shared information
/// architecture; the template prompt changes the lens and topic grouping, not
/// the amount of source-grounded coverage the user receives.
fn meeting_pack_schema(coverage: CoveragePlan) -> Value {
    let (
        minimum_topics,
        minimum_points,
        minimum_timeline,
        maximum_topics,
        maximum_points,
        maximum_timeline,
        maximum_register,
    ) = match coverage.level {
        CoverageLevel::Comprehensive => (5, 3, 6, 8, 8, 12, 10),
        CoverageLevel::Detailed => (3, 2, 3, 7, 6, 10, 10),
        CoverageLevel::Brief => (1, 1, 0, 4, 5, 5, 6),
    };
    let source = source_range_schema();
    json!({
        "type": "object",
        "properties": {
            "executive_summary": { "type": "string" },
            "executive_sources": { "type": "array", "maxItems": 4, "items": source.clone() },
            "success_definition": { "type": "string" },
            "success_sources": { "type": "array", "maxItems": 4, "items": source.clone() },
            "at_glance": {
                "type": "array", "maxItems": 8,
                "items": {
                    "type": "object",
                    "properties": {
                        "item": { "type": "string" },
                        "details": { "type": "string" },
                        "sources": { "type": "array", "maxItems": 4, "items": source.clone() }
                    },
                    "required": ["item", "details", "sources"]
                }
            },
            "timeline": {
                "type": "array",
                "minItems": minimum_timeline,
                "maxItems": maximum_timeline,
                "items": {
                    "type": "object",
                    "properties": {
                        "start": timestamp_schema(),
                        "end": timestamp_schema(),
                        "topic": { "type": "string" },
                        "details": { "type": "string" }
                    },
                    "required": ["start", "end", "topic", "details"]
                }
            },
            "discussion": {
                "type": "array",
                "minItems": minimum_topics,
                "maxItems": maximum_topics,
                "items": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "summary": { "type": "string" },
                        "points": {
                            "type": "array",
                            "minItems": minimum_points,
                            "maxItems": maximum_points,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string" },
                                    "sources": { "type": "array", "maxItems": 4, "items": source.clone() }
                                },
                                "required": ["text", "sources"]
                            }
                        }
                    },
                    "required": ["title", "summary", "points"]
                }
            },
            "decisions": {
                "type": "array", "maxItems": maximum_register,
                "items": {
                    "type": "object",
                    "properties": {
                        "decision": { "type": "string" },
                        "detail": { "type": "string" },
                        "sources": { "type": "array", "maxItems": 4, "items": source.clone() }
                    },
                    "required": ["decision", "detail", "sources"]
                }
            },
            "workplan": {
                "type": "array", "maxItems": 8,
                "items": {
                    "type": "object",
                    "properties": {
                        "priority": { "type": "string" },
                        "objective": { "type": "string" },
                        "definition_of_progress": { "type": "string" },
                        "sources": { "type": "array", "maxItems": 4, "items": source.clone() }
                    },
                    "required": ["priority", "objective", "definition_of_progress", "sources"]
                }
            },
            "actions": {
                "type": "array", "maxItems": maximum_register,
                "items": {
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "action": { "type": "string" },
                        "timing": { "type": "string" },
                        "dependency": { "type": "string" },
                        "sources": { "type": "array", "maxItems": 4, "items": source.clone() }
                    },
                    "required": ["owner", "action", "timing", "dependency", "sources"]
                }
            },
            "open_questions": {
                "type": "array", "maxItems": maximum_register,
                "items": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" },
                        "sources": { "type": "array", "maxItems": 4, "items": source.clone() }
                    },
                    "required": ["text", "sources"]
                }
            },
            "risks": {
                "type": "array", "maxItems": maximum_register,
                "items": {
                    "type": "object",
                    "properties": {
                        "risk": { "type": "string" },
                        "impact": { "type": "string" },
                        "response": { "type": "string" },
                        "sources": { "type": "array", "maxItems": 4, "items": source.clone() }
                    },
                    "required": ["risk", "impact", "response", "sources"]
                }
            }
        },
        "required": [
            "executive_summary", "executive_sources", "success_definition",
            "success_sources", "at_glance", "timeline", "discussion",
            "decisions", "workplan", "actions", "open_questions", "risks"
        ]
    })
}

fn pack_array_len(pack: &Value, key: &str) -> usize {
    pack[key].as_array().map(Vec::len).unwrap_or(0)
}

fn pack_discussion_points(pack: &Value) -> usize {
    pack["discussion"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|topic| pack_array_len(topic, "points"))
        .sum()
}

fn semantic_word_count(value: &Value, key: Option<&str>) -> usize {
    match value {
        Value::String(text) if !matches!(key, Some("start" | "end" | "quality_warnings")) => {
            text.split_whitespace().count()
        }
        Value::Array(items) => items
            .iter()
            .map(|item| semantic_word_count(item, key))
            .sum(),
        Value::Object(map) => map
            .iter()
            .map(|(name, item)| semantic_word_count(item, Some(name)))
            .sum(),
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MeetingPackStats {
    words: usize,
    discussion_topics: usize,
    discussion_points: usize,
    timeline_points: usize,
    covered_time_buckets: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LedgerCounts {
    decisions: usize,
    actions: usize,
    open_questions: usize,
    risks: usize,
}

fn chapter_ledger_counts(ledgers: &[Value]) -> LedgerCounts {
    LedgerCounts {
        decisions: ledgers
            .iter()
            .map(|ledger| pack_array_len(ledger, "decisions"))
            .sum(),
        actions: ledgers
            .iter()
            .map(|ledger| pack_array_len(ledger, "actions"))
            .sum(),
        open_questions: ledgers
            .iter()
            .map(|ledger| pack_array_len(ledger, "open_questions"))
            .sum(),
        risks: ledgers
            .iter()
            .map(|ledger| pack_array_len(ledger, "risks"))
            .sum(),
    }
}

fn minimum_retained_count(extracted: usize) -> usize {
    // Chapter ledgers intentionally overlap at topic boundaries and often phrase
    // one unresolved issue several ways. Preserve a meaningful register while
    // allowing the final composer to consolidate those duplicates.
    extracted.div_ceil(3)
}

fn timestamp_seconds(value: &str) -> Option<i64> {
    let value = value.trim().trim_matches(['[', ']']);
    let (minutes, seconds) = value.split_once(':')?;
    if minutes.is_empty()
        || seconds.len() != 2
        || !minutes.chars().all(|character| character.is_ascii_digit())
        || !seconds.chars().all(|character| character.is_ascii_digit())
    {
        return None;
    }
    let minutes = minutes.parse::<i64>().ok()?;
    let seconds = seconds.parse::<i64>().ok()?;
    if seconds >= 60 {
        return None;
    }
    minutes.checked_mul(60)?.checked_add(seconds)
}

/// Recover timestamp shapes emitted by small local models without guessing at
/// arbitrary prose. Real examples include `:826` for `08:26`, `:13:38`, and
/// `]==40:02`. Anything that cannot be read unambiguously is discarded later.
fn loose_timestamp_seconds(value: &str) -> Option<i64> {
    let trimmed = value.trim().trim_matches(['[', ']', '=', '<', '>']).trim();
    if let Some(seconds) = timestamp_seconds(trimmed) {
        return Some(seconds);
    }

    // Only repair known wrapper noise. Prose and ambiguous hours:minutes:seconds
    // must not be reinterpreted as a different transcript location.
    let compact = trimmed.strip_prefix(':')?;
    if let Some(seconds) = timestamp_seconds(compact) {
        return Some(seconds);
    }

    // Some constrained decodes keep the colon but collapse the leading zero
    // and the separator: `:16` -> 00:16, `:826` -> 08:26, `:1248` -> 12:48.
    if compact.is_empty() || !compact.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let (minutes, seconds) = if compact.len() <= 2 {
        (0, compact.parse::<i64>().ok()?)
    } else {
        let split = compact.len() - 2;
        (
            compact[..split].parse::<i64>().ok()?,
            compact[split..].parse::<i64>().ok()?,
        )
    };
    if seconds >= 60 {
        return None;
    }
    minutes.checked_mul(60)?.checked_add(seconds)
}

fn nearest_valid_timestamp(value: &str, valid: &HashSet<String>) -> Option<String> {
    const MAX_SNAP_SECONDS: i64 = 90;
    let value = value.trim();
    if valid.contains(value) {
        return Some(value.to_string());
    }
    if value.eq_ignore_ascii_case("notes") && valid.contains("notes") {
        return Some("notes".to_string());
    }
    let target = loose_timestamp_seconds(value)?;
    valid
        .iter()
        .filter_map(|candidate| timestamp_seconds(candidate).map(|seconds| (candidate, seconds)))
        .min_by_key(|(_, seconds)| (seconds.abs_diff(target), *seconds))
        .filter(|(_, seconds)| seconds.abs_diff(target) <= MAX_SNAP_SECONDS as u64)
        .map(|(candidate, _)| candidate.clone())
}

/// Models commonly cite a meaningful second inside a transcript turn rather
/// than the turn's exact starting second. Snap those in-range citations to the
/// nearest real turn so every link remains clickable and validation stays
/// deterministic instead of rejecting otherwise grounded notes.
fn normalize_pack_source_timestamps(value: &mut Value, valid: &HashSet<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_pack_source_timestamps(item, valid);
            }
        }
        Value::Object(map) => {
            let start = map.get("start").and_then(Value::as_str);
            let end = map.get("end").and_then(Value::as_str);
            if let (Some(start), Some(end)) = (start, end) {
                let normalized_start = nearest_valid_timestamp(start, valid);
                let normalized_end = nearest_valid_timestamp(end, valid);
                match (normalized_start, normalized_end) {
                    (Some(start), Some(end))
                        if (start == "notes" && end == "notes")
                            || timestamp_seconds(&start)
                                .zip(timestamp_seconds(&end))
                                .is_some_and(|(start_seconds, end_seconds)| {
                                    start_seconds <= end_seconds
                                }) =>
                    {
                        map.insert("start".to_string(), json!(start));
                        map.insert("end".to_string(), json!(end));
                    }
                    (Some(timestamp), None) | (None, Some(timestamp)) => {
                        map.insert("start".to_string(), json!(timestamp));
                        map.insert("end".to_string(), json!(timestamp));
                    }
                    // Never let malformed model syntax leak into Markdown.
                    // Empty source ranges are ignored by scoring and rendering.
                    (Some(_), Some(_)) | (None, None) => {
                        map.insert("start".to_string(), json!(""));
                        map.insert("end".to_string(), json!(""));
                    }
                }
            }
            for (key, item) in map {
                if key != "start" && key != "end" {
                    normalize_pack_source_timestamps(item, valid);
                }
            }
        }
        _ => {}
    }
}

fn semantic_key(text: &str) -> String {
    let mut key = String::with_capacity(text.len());
    let mut separated = false;
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            key.push(character);
            separated = false;
        } else if !separated && !key.is_empty() {
            key.push(' ');
            separated = true;
        }
    }
    key.trim().to_string()
}

fn object_key(value: &Value, fields: &[&str]) -> String {
    fields
        .iter()
        .filter_map(|field| value[*field].as_str())
        .map(semantic_key)
        .collect::<Vec<_>>()
        .join("|")
}

fn deduplicate_object_array(value: &mut Value, fields: &[&str]) {
    let Some(items) = value.as_array_mut() else {
        return;
    };
    let mut seen = HashSet::new();
    items.retain(|item| {
        let key = object_key(item, fields);
        !key.is_empty() && seen.insert(key)
    });
}

fn looks_like_source_range(value: &str) -> bool {
    value.split_once('-').is_some_and(|(start, end)| {
        looks_like_timestamp_fragment(start) && looks_like_timestamp_fragment(end)
    })
}

fn looks_like_timestamp_fragment(value: &str) -> bool {
    let value = value.trim();
    value.contains(':')
        && value.chars().any(|character| character.is_ascii_digit())
        && value.chars().all(|character| {
            character.is_ascii_digit()
                || character.is_ascii_whitespace()
                || matches!(character, ':' | '[' | ']' | '=' | '<' | '>')
        })
}

/// Remove exact semantic repetition before quality scoring. A small local model
/// can otherwise satisfy a length floor by cloning one valid topic or turning
/// actions completed during the call into a follow-up register.
fn sanitize_meeting_pack(pack: &mut Value) {
    for (key, fields) in [
        ("at_glance", &["item", "details"][..]),
        ("timeline", &["topic", "details"][..]),
        ("decisions", &["decision", "detail"][..]),
        ("workplan", &["objective", "definition_of_progress"][..]),
        ("open_questions", &["text"][..]),
        ("risks", &["risk", "impact"][..]),
    ] {
        deduplicate_object_array(&mut pack[key], fields);
    }

    if let Some(actions) = pack["actions"].as_array_mut() {
        for item in actions.iter_mut() {
            if item["timing"].as_str().is_some_and(looks_like_source_range) {
                // A transcript citation is not a deadline. Keep the grounded
                // action, but do not display source syntax in the timing column.
                item["timing"] = json!("");
            }
        }
        actions.retain(|item| {
            let action = semantic_key(item["action"].as_str().unwrap_or(""));
            !action.is_empty()
                && ![
                    "start of meeting",
                    "check audio connection",
                    "confirm audio connection established",
                    "share screen for demonstration",
                    "record meeting notes",
                    "confirm understanding",
                ]
                .iter()
                .any(|mechanic| action.contains(mechanic))
                && action != "share screen"
        });
    }
    deduplicate_object_array(&mut pack["actions"], &["owner", "action"]);

    let Some(topics) = pack["discussion"].as_array_mut() else {
        return;
    };
    let mut seen_titles = HashSet::new();
    let mut seen_points = HashSet::new();
    topics.retain_mut(|topic| {
        let title = object_key(topic, &["title"]);
        if title.is_empty() || !seen_titles.insert(title) {
            return false;
        }
        let Some(points) = topic["points"].as_array_mut() else {
            return false;
        };
        points.retain(|point| {
            let key = object_key(point, &["text"]);
            !key.is_empty() && seen_points.insert(key)
        });
        !points.is_empty()
    });
}

fn ledger_source(item: &Value) -> Option<Value> {
    let start = item["start"].as_str()?.trim();
    let end = item["end"].as_str()?.trim();
    if start.is_empty() || end.is_empty() {
        return None;
    }
    Some(json!({ "start": start, "end": end }))
}

fn ledger_fact_to_point(fact: &Value) -> Option<Value> {
    let text = fact["text"].as_str()?.trim();
    let source = ledger_source(fact)?;
    (!text.is_empty()).then(|| json!({ "text": text, "sources": [source] }))
}

fn append_unique_points(topic: &mut Value, candidates: &[Value]) {
    let Some(points) = topic.get_mut("points").and_then(Value::as_array_mut) else {
        return;
    };
    let mut seen = points
        .iter()
        .filter_map(|point| point["text"].as_str())
        .map(semantic_key)
        .collect::<HashSet<_>>();
    for candidate in candidates {
        let Some(text) = candidate["text"].as_str() else {
            continue;
        };
        if seen.insert(semantic_key(text)) {
            points.push(candidate.clone());
        }
    }
}

fn merge_chapter_discussion(pack: &mut Value, ledgers: &[Value]) {
    let mut topics = pack["discussion"].as_array().cloned().unwrap_or_default();
    for ledger_topic in ledgers
        .iter()
        .flat_map(|ledger| ledger["topics"].as_array().into_iter().flatten())
    {
        let title = ledger_topic["title"]
            .as_str()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("Meeting chapter");
        let facts = ledger_topic["facts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(ledger_fact_to_point)
            .collect::<Vec<_>>();
        if facts.is_empty() {
            continue;
        }

        let title_key = semantic_key(title);
        let title_match = topics.iter().position(|topic| {
            semantic_key(topic["title"].as_str().unwrap_or("")) == title_key
                && topic["points"].is_array()
        });
        let target = if let Some(index) = title_match {
            index
        } else {
            topics.push(json!({
                "title": title,
                "summary": "",
                "points": []
            }));
            topics.len() - 1
        };
        append_unique_points(&mut topics[target], &facts);
    }
    pack["discussion"] = Value::Array(topics);
}

fn merge_chapter_timeline(pack: &mut Value, ledgers: &[Value]) {
    let mut timeline = Vec::new();
    let mut seen = HashSet::new();

    for item in pack["timeline"].as_array().into_iter().flatten() {
        let title = item["topic"].as_str().unwrap_or("");
        let start = item["start"].as_str().unwrap_or("");
        let key = format!("{}|{}", semantic_key(title), start.trim());
        if seen.insert(key) {
            timeline.push(item.clone());
        }
    }

    for topic in ledgers
        .iter()
        .flat_map(|ledger| ledger["topics"].as_array().into_iter().flatten())
    {
        let Some(start) = topic["start"].as_str() else {
            continue;
        };
        let Some(end) = topic["end"].as_str() else {
            continue;
        };
        let Some(title) = topic["title"].as_str() else {
            continue;
        };
        let details = topic["facts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|fact| fact["text"].as_str())
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let key = format!("{}|{}", semantic_key(title), start.trim());
        if !details.is_empty() && seen.insert(key) {
            timeline.push(json!({
                "start": start,
                "end": end,
                "topic": title,
                "details": details
            }));
        }
    }

    timeline.sort_by_key(|item| {
        item["start"]
            .as_str()
            .and_then(timestamp_seconds)
            .unwrap_or(i64::MAX)
    });
    pack["timeline"] = Value::Array(timeline);
}

fn ledger_register_item(key: &str, item: &Value) -> Option<Value> {
    let source = ledger_source(item)?;
    match key {
        "decisions" => Some(json!({
            "decision": item["decision"].as_str()?,
            "detail": item["detail"].as_str()?,
            "sources": [source]
        })),
        "actions" => {
            let owner = item["owner"].as_str()?.trim();
            let owner_key = semantic_key(owner);
            let owner =
                if owner_key == "me" || owner_key == "them" || owner_key.starts_with("speaker ") {
                    "Unassigned"
                } else {
                    owner
                };
            Some(json!({
                "owner": owner,
                "action": item["action"].as_str()?,
                "timing": item["timing"].as_str()?,
                "dependency": item["dependency"].as_str()?,
                "sources": [source]
            }))
        }
        "open_questions" => Some(json!({
            "text": item["text"].as_str()?,
            "sources": [source]
        })),
        "risks" => Some(json!({
            "risk": item["risk"].as_str()?,
            "impact": item["impact"].as_str()?,
            "response": item["response"].as_str()?,
            "sources": [source]
        })),
        _ => None,
    }
}

fn merge_chapter_register(
    pack: &mut Value,
    ledgers: &[Value],
    key: &str,
    identity_fields: &[&str],
) {
    let mut merged = pack[key].as_array().cloned().unwrap_or_default();
    merged.extend(
        ledgers
            .iter()
            .flat_map(|ledger| ledger[key].as_array().into_iter().flatten())
            .filter_map(|item| ledger_register_item(key, item)),
    );
    let mut seen = HashSet::new();
    merged.retain(|item| {
        let identity = object_key(item, identity_fields);
        !identity.is_empty() && seen.insert(identity)
    });
    pack[key] = Value::Array(merged);
}

/// The final local-model composition is useful for organization and prose, but
/// it must not be allowed to forget evidence already extracted from long-meeting
/// chapters. Model response limits keep generation bounded; they must not
/// truncate already-extracted evidence or attach later facts to unrelated topics.
fn backfill_pack_from_chapter_ledgers(pack: &mut Value, ledgers: &[Value]) {
    if ledgers.is_empty() {
        return;
    }
    merge_chapter_timeline(pack, ledgers);
    merge_chapter_discussion(pack, ledgers);
    merge_chapter_register(pack, ledgers, "decisions", &["decision", "detail"]);
    merge_chapter_register(pack, ledgers, "actions", &["owner", "action"]);
    merge_chapter_register(pack, ledgers, "open_questions", &["text"]);
    merge_chapter_register(pack, ledgers, "risks", &["risk", "impact"]);
}

fn collect_pack_source_starts(value: &Value, out: &mut Vec<i64>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_pack_source_starts(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(start) = map
                .get("start")
                .and_then(Value::as_str)
                .and_then(timestamp_seconds)
            {
                out.push(start);
            }
            for (key, item) in map {
                if key != "start" && key != "end" {
                    collect_pack_source_starts(item, out);
                }
            }
        }
        _ => {}
    }
}

fn meeting_pack_stats(
    pack: &Value,
    duration_min: i64,
    valid_timestamps: Option<&HashSet<String>>,
) -> MeetingPackStats {
    let mut starts = Vec::new();
    collect_pack_source_starts(pack, &mut starts);
    if let Some(valid) = valid_timestamps {
        starts.retain(|seconds| {
            let value = format!("{:02}:{:02}", seconds / 60, seconds % 60);
            valid.contains(&value)
        });
    }
    let duration_seconds = (duration_min.max(1) * 60) as f64;
    let covered_time_buckets = starts
        .into_iter()
        .map(|seconds| ((seconds as f64 / duration_seconds) * 5.0).floor() as usize)
        .map(|bucket| bucket.min(4))
        .collect::<HashSet<_>>()
        .len();
    MeetingPackStats {
        words: semantic_word_count(pack, None),
        discussion_topics: pack_array_len(pack, "discussion"),
        discussion_points: pack_discussion_points(pack),
        timeline_points: pack_array_len(pack, "timeline"),
        covered_time_buckets,
    }
}

#[cfg(test)]
fn invalid_pack_source_count(value: &Value, valid: &HashSet<String>) -> usize {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| invalid_pack_source_count(item, valid))
            .sum(),
        Value::Object(map) => {
            let own = match (map.get("start"), map.get("end")) {
                (Some(start), Some(end)) => {
                    let start = start.as_str().map(|text| text.trim_matches(['[', ']']));
                    let end = end.as_str().map(|text| text.trim_matches(['[', ']']));
                    usize::from(
                        start.is_none_or(|timestamp| !valid.contains(timestamp))
                            || end.is_none_or(|timestamp| !valid.contains(timestamp)),
                    )
                }
                _ => 0,
            };
            own + map
                .iter()
                .filter(|(key, _)| *key != "start" && *key != "end")
                .map(|(_, item)| invalid_pack_source_count(item, valid))
                .sum::<usize>()
        }
        _ => 0,
    }
}

fn meeting_pack_deficiencies(
    pack: &Value,
    coverage: CoveragePlan,
    valid_timestamps: Option<&HashSet<String>>,
    ledger_counts: Option<LedgerCounts>,
) -> Vec<String> {
    let stats = meeting_pack_stats(pack, coverage.duration_min, valid_timestamps);
    let mut missing = Vec::new();
    if pack["executive_summary"]
        .as_str()
        .is_none_or(|text| text.trim().is_empty())
    {
        missing.push("an executive summary".to_string());
    }
    if pack_array_len(pack, "at_glance") < 3 && coverage.level != CoverageLevel::Brief {
        missing.push("at least 3 at-a-glance facts".to_string());
    }
    let (topics, points, timeline, buckets) = match coverage.level {
        // Three distinct time bands reject beginning-only summaries while not
        // forcing small talk or setup chatter into otherwise substantive notes.
        CoverageLevel::Comprehensive => (5, 18, 6, 3),
        CoverageLevel::Detailed => (3, 8, 3, 3),
        CoverageLevel::Brief => (1, 1, 0, 1),
    };
    if stats.discussion_topics < topics {
        missing.push(format!("at least {topics} detailed discussion topics"));
    }
    if stats.discussion_points < points {
        missing.push(format!("at least {points} substantive discussion points"));
    }
    if stats.timeline_points < timeline {
        missing.push(format!("at least {timeline} timeline chapters"));
    }
    if stats.covered_time_buckets < buckets {
        missing.push(format!(
            "evidence spanning at least {buckets} fifths of the meeting"
        ));
    }
    if stats.words < coverage.minimum_summary_words {
        missing.push(format!(
            "at least {} grounded words",
            coverage.minimum_summary_words
        ));
    }
    // Parseable model citations are snapped to real transcript turns before
    // this check. Any remaining malformed citation is omitted by the renderer;
    // it must not invalidate an otherwise grounded, chapter-covered pack.
    if let Some(expected) = ledger_counts {
        for (key, label, extracted) in [
            ("decisions", "decisions", expected.decisions),
            ("actions", "actions", expected.actions),
            ("open_questions", "open questions", expected.open_questions),
            ("risks", "risks", expected.risks),
        ] {
            let required = minimum_retained_count(extracted);
            let actual = pack_array_len(pack, key);
            if actual < required {
                missing.push(format!(
                    "retention of {required} {label} from the chapter ledgers (found {actual})"
                ));
            }
        }
    }
    missing
}

fn richer_meeting_pack(
    first: Value,
    candidate: Value,
    coverage: CoveragePlan,
    valid_timestamps: Option<&HashSet<String>>,
) -> Value {
    let score = |pack: &Value| {
        let stats = meeting_pack_stats(pack, coverage.duration_min, valid_timestamps);
        stats.words
            + stats.discussion_points * 45
            + stats.discussion_topics * 80
            + stats.timeline_points * 25
            + stats.covered_time_buckets * 100
    };
    if score(&candidate) > score(&first) {
        candidate
    } else {
        first
    }
}

fn protect_existing_summary(
    summaries: &Value,
    template: &str,
    candidate: &Value,
    coverage: CoveragePlan,
    valid_timestamps: &HashSet<String>,
) -> Result<()> {
    let Some(previous) = summaries
        .as_array()
        .into_iter()
        .flatten()
        .find(|summary| summary["template"].as_str() == Some(template))
    else {
        return Ok(());
    };
    // Only used for an incomplete regeneration. A manually edited summary has
    // no structured pack and must not be replaced by a partial model result.
    let old_pack = &previous["content_json"];
    if old_pack.is_object()
        && old_pack != candidate
        && richer_meeting_pack(
            old_pack.clone(),
            candidate.clone(),
            coverage,
            Some(valid_timestamps),
        ) == *candidate
    {
        return Ok(());
    }
    Err(anyhow!("The new notes were incomplete and did not improve on the existing notes. Your existing notes were kept."))
}

fn normalized_pack_sources(
    sources: &Value,
    valid_timestamps: Option<&HashSet<String>>,
) -> Vec<String> {
    sources
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|source| {
            let start = source["start"].as_str()?.trim_matches(['[', ']']);
            let end = source["end"].as_str()?.trim_matches(['[', ']']);
            if (start != "notes" && timestamp_seconds(start).is_none())
                || valid_timestamps.is_some_and(|valid| !valid.contains(start))
            {
                return None;
            }
            let end =
                if end.is_empty() || valid_timestamps.is_some_and(|valid| !valid.contains(end)) {
                    start
                } else {
                    end
                };
            let end = timestamp_seconds(start)
                .zip(timestamp_seconds(end))
                .filter(|(start_seconds, end_seconds)| start_seconds <= end_seconds)
                .map(|_| end)
                .unwrap_or(start);
            Some(if end == start {
                format!("[{start}]")
            } else {
                format!("[{start}-{end}]")
            })
        })
        .take(4)
        .collect()
}

fn sourced_pack_text(
    text: &str,
    sources: &Value,
    valid_timestamps: Option<&HashSet<String>>,
) -> String {
    let references = normalized_pack_sources(sources, valid_timestamps);
    if references.is_empty() {
        text.trim().to_string()
    } else {
        format!("{} {}", text.trim(), references.join(" "))
    }
}

fn table_cell(text: &str) -> String {
    text.trim().replace('|', "/").replace('\n', " ")
}

fn renderable_timestamp<'a>(
    value: &'a str,
    valid_timestamps: Option<&HashSet<String>>,
) -> Option<&'a str> {
    let timestamp = value.trim().trim_matches(['[', ']']);
    if timestamp_seconds(timestamp).is_none()
        || valid_timestamps.is_some_and(|valid| !valid.contains(timestamp))
    {
        None
    } else {
        Some(timestamp)
    }
}

fn push_markdown_table(out: &mut String, headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }
    out.push_str(&format!("| {} |\n", headers.join(" | ")));
    out.push_str(&format!(
        "|{}|\n",
        headers
            .iter()
            .map(|_| " --- ")
            .collect::<Vec<_>>()
            .join("|")
    ));
    for row in rows {
        out.push_str(&format!(
            "| {} |\n",
            row.iter()
                .map(|cell| table_cell(cell))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    out.push('\n');
}

fn render_meeting_pack(pack: &Value, valid_timestamps: Option<&HashSet<String>>) -> String {
    let mut out = String::new();
    let summary = pack["executive_summary"].as_str().unwrap_or("").trim();
    if !summary.is_empty() {
        out.push_str("## Executive Summary\n\n");
        out.push_str(&sourced_pack_text(
            summary,
            &pack["executive_sources"],
            valid_timestamps,
        ));
        out.push_str("\n\n");
    }
    let success = pack["success_definition"].as_str().unwrap_or("").trim();
    if !success.is_empty() {
        out.push_str("### Success Definition\n\n");
        out.push_str(&sourced_pack_text(
            success,
            &pack["success_sources"],
            valid_timestamps,
        ));
        out.push_str("\n\n");
    }

    let at_glance = pack["at_glance"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(vec![
                row["item"].as_str()?.to_string(),
                sourced_pack_text(row["details"].as_str()?, &row["sources"], valid_timestamps),
            ])
        })
        .collect::<Vec<_>>();
    if !at_glance.is_empty() {
        out.push_str("## Meeting at a Glance\n\n");
        push_markdown_table(&mut out, &["Item", "Details"], &at_glance);
    }

    let timeline = pack["timeline"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let start = renderable_timestamp(row["start"].as_str()?, valid_timestamps)?;
            let end = renderable_timestamp(row["end"].as_str()?, valid_timestamps).unwrap_or(start);
            if timestamp_seconds(start)
                .zip(timestamp_seconds(end))
                .is_none_or(|(start_seconds, end_seconds)| start_seconds > end_seconds)
            {
                return None;
            }
            Some(vec![
                if start == end {
                    start.to_string()
                } else {
                    format!("{start}-{end}")
                },
                row["topic"].as_str()?.to_string(),
                row["details"].as_str()?.to_string(),
            ])
        })
        .collect::<Vec<_>>();
    if !timeline.is_empty() {
        out.push_str("## Discussion Timeline\n\n");
        push_markdown_table(&mut out, &["Time", "Topic", "What was covered"], &timeline);
    }

    if let Some(topics) = pack["discussion"]
        .as_array()
        .filter(|topics| !topics.is_empty())
    {
        out.push_str("## Key Discussion Notes\n\n");
        for (index, topic) in topics.iter().enumerate() {
            let title = topic["title"].as_str().unwrap_or("Topic").trim();
            out.push_str(&format!("### {}. {}\n\n", index + 1, title));
            if let Some(summary) = topic["summary"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
            {
                out.push_str(summary.trim());
                out.push_str("\n\n");
            }
            for point in topic["points"].as_array().into_iter().flatten() {
                if let Some(text) = point["text"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                {
                    out.push_str("- ");
                    out.push_str(&sourced_pack_text(
                        text,
                        &point["sources"],
                        valid_timestamps,
                    ));
                    out.push('\n');
                }
            }
            out.push('\n');
        }
    }

    let decisions = pack["decisions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(vec![
                row["decision"].as_str()?.to_string(),
                row["detail"].as_str()?.to_string(),
                normalized_pack_sources(&row["sources"], valid_timestamps).join(" "),
            ])
        })
        .collect::<Vec<_>>();
    if !decisions.is_empty() {
        out.push_str("## Decisions and Working Agreements\n\n");
        let include_detail = decisions.iter().any(|row| !row[1].trim().is_empty());
        let include_reference = decisions.iter().any(|row| !row[2].trim().is_empty());
        let mut headers = vec!["Decision / agreement"];
        if include_detail {
            headers.push("Detail");
        }
        if include_reference {
            headers.push("Reference");
        }
        let rows = decisions
            .into_iter()
            .map(|row| {
                let mut rendered = vec![row[0].clone()];
                if include_detail {
                    rendered.push(row[1].clone());
                }
                if include_reference {
                    rendered.push(row[2].clone());
                }
                rendered
            })
            .collect::<Vec<_>>();
        push_markdown_table(&mut out, &headers, &rows);
    }

    let workplan = pack["workplan"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(vec![
                row["priority"].as_str()?.to_string(),
                row["objective"].as_str()?.to_string(),
                sourced_pack_text(
                    row["definition_of_progress"].as_str()?,
                    &row["sources"],
                    valid_timestamps,
                ),
            ])
        })
        .collect::<Vec<_>>();
    if !workplan.is_empty() {
        out.push_str("## Workplan\n\n");
        push_markdown_table(
            &mut out,
            &["Priority", "Objective", "Definition of progress"],
            &workplan,
        );
    }

    let actions = pack["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(vec![
                row["owner"].as_str()?.to_string(),
                sourced_pack_text(row["action"].as_str()?, &row["sources"], valid_timestamps),
                row["timing"].as_str()?.to_string(),
                row["dependency"].as_str()?.to_string(),
            ])
        })
        .collect::<Vec<_>>();
    if !actions.is_empty() {
        out.push_str("## Action Items\n\n");
        let include_timing = actions.iter().any(|row| !row[2].trim().is_empty());
        let include_dependency = actions.iter().any(|row| !row[3].trim().is_empty());
        let mut headers = vec!["Owner", "Action"];
        if include_timing {
            headers.push("Timing / priority");
        }
        if include_dependency {
            headers.push("Dependency or note");
        }
        let rows = actions
            .into_iter()
            .map(|row| {
                let mut rendered = vec![row[0].clone(), row[1].clone()];
                if include_timing {
                    rendered.push(row[2].clone());
                }
                if include_dependency {
                    rendered.push(row[3].clone());
                }
                rendered
            })
            .collect::<Vec<_>>();
        push_markdown_table(&mut out, &headers, &rows);
    }

    if let Some(questions) = pack["open_questions"]
        .as_array()
        .filter(|items| !items.is_empty())
    {
        out.push_str("## Open Questions and Dependencies\n\n");
        for question in questions {
            if let Some(text) = question["text"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
            {
                out.push_str("- ");
                out.push_str(&sourced_pack_text(
                    text,
                    &question["sources"],
                    valid_timestamps,
                ));
                out.push('\n');
            }
        }
        out.push('\n');
    }

    let risks = pack["risks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(vec![
                row["risk"].as_str()?.to_string(),
                row["impact"].as_str()?.to_string(),
                sourced_pack_text(row["response"].as_str()?, &row["sources"], valid_timestamps),
            ])
        })
        .collect::<Vec<_>>();
    if !risks.is_empty() {
        out.push_str("## Risks and Operating Considerations\n\n");
        let include_impact = risks.iter().any(|row| !row[1].trim().is_empty());
        let include_response = risks.iter().any(|row| !row[2].trim().is_empty());
        let mut headers = vec!["Risk / constraint"];
        if include_impact {
            headers.push("Impact");
        }
        if include_response {
            headers.push("Practical response");
        }
        let rows = risks
            .into_iter()
            .map(|row| {
                let mut rendered = vec![row[0].clone()];
                if include_impact {
                    rendered.push(row[1].clone());
                }
                if include_response {
                    rendered.push(row[2].clone());
                }
                rendered
            })
            .collect::<Vec<_>>();
        push_markdown_table(&mut out, &headers, &rows);
    }
    out.trim_end().to_string()
}

fn finalize_meeting_pack(
    pack: &mut Value,
    coverage: CoveragePlan,
    valid_timestamps: &HashSet<String>,
    ledger_counts: Option<LedgerCounts>,
) -> Result<(String, Option<String>)> {
    if !pack.is_object() {
        return Err(anyhow!("model did not return a Meeting Pack object"));
    }
    let md = render_meeting_pack(pack, Some(valid_timestamps));
    if md.trim().is_empty() {
        return Err(anyhow!("model produced an empty summary"));
    }
    let deficiencies =
        meeting_pack_deficiencies(pack, coverage, Some(valid_timestamps), ledger_counts);
    let warning = (!deficiencies.is_empty()).then(|| {
        "Notes were saved, but may be incomplete. Review them against the transcript.".to_string()
    });
    pack["quality_warnings"] = json!(deficiencies);
    Ok((md, warning))
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn canonical_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| !word.is_empty())
        .collect()
}

#[cfg(test)]
fn item_source(
    item: &Value,
    valid_timestamps: Option<&HashSet<String>>,
    allow_notes: bool,
) -> Option<String> {
    item.get("source")
        .and_then(Value::as_str)
        .and_then(|source| normalized_source(source, valid_timestamps, allow_notes))
}

#[cfg(test)]
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
#[cfg(test)]
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

const SYSTEM: &str = "You create a comprehensive, source-grounded Meeting Pack. The input is \
either a timestamped transcript or typed chapter ledgers extracted from the complete transcript, \
plus the note-taker's own notes and a template lens. The Meeting Pack has one shared information \
architecture for every template: executive readout, success definition when grounded, at-a-glance \
context, discussion timeline, detailed topic notes, decisions and working agreements, optional \
workplan, complete action register, open questions and dependencies, and risks.\n\
Rules:\n\
- There is no page limit. Scale detail to information density. A 40-60 minute substantive meeting \
normally needs multiple detailed points for every topic and may require several pages.\n\
- Ground every statement in the supplied source. Never invent facts, names, numbers, dates, owners, \
deadlines, dependencies, impacts, or recommended responses.\n\
- Preserve every point from the note-taker's typed notes and expand it with transcript context.\n\
- The executive summary is a concise readout; the detailed discussion is deliberately comprehensive. \
Never use the executive summary as a reason to omit detail later.\n\
- Preserve concrete examples, rationale, constraints, tradeoffs, maturity stages, operating sequences, \
dependencies, and implications. Do not collapse separate workstreams into generic bullets.\n\
- Never repeat the same fact in multiple discussion topics to create length. Each topic title and each \
detailed point must add distinct information; consolidate overlaps under the best-fitting topic.\n\
- Use content-specific structure: at_glance for compact context; timeline for navigation; discussion \
topics for full understanding; workplan for prioritized objectives; and typed registers for decisions, \
actions, questions, and risks. Leave an array empty only when the complete source truly has no content \
for it. success_definition may be an empty string when none is grounded.\n\
- Every source is {start,end} using exact mm:ss timestamps present in the supplied evidence. Use the \
same value for both when only one line supports a claim. For a point supported only by the note-taker's \
typed notes, use notes for both values. Never approximate timestamps.\n\
- A decision is a chosen direction or working agreement, not merely a task. Capture its stated reasoning.\n\
- Capture every explicit post-meeting commitment and every concrete follow-up described as needed, \
required, still to do, check, confirm, review, send, or follow up. Exclude agenda steps and mechanics \
already completed during the call, such as starting the meeting, checking audio, sharing a screen, \
explaining a workflow, or confirming understanding. Owner is an actual participant name only when grounded; \
otherwise use Unassigned. Never use Them or Speaker N as an owner.\n\
- Meeting-title and attendee metadata are authoritative participant context. When the title explicitly \
says this is a 1:1 with one person, generic remote-speaker labels refer to that person unless the source \
clearly introduces another participant.\n\
- Do not silently normalize uncertain names or transcript errors. Say that a reference name is unclear \
when the evidence is inconsistent.\n\
- A practical risk response must be stated or directly entailed by the source; otherwise leave it empty.\n\
- Describe what was said and decided. Do not infer mood, enthusiasm, engagement, or internal states.\n\
Return only JSON matching the supplied schema.";

/// Turn one bounded portion of a long meeting into a typed chapter ledger. The
/// final composer receives topic boundaries and distinct decision/action/risk
/// registers rather than a lossy flat list of fact strings.
fn model_object(value: Value) -> Result<Value> {
    if !value.is_object() {
        return Err(anyhow!(
            "model returned JSON without the requested object structure"
        ));
    }
    Ok(value)
}

async fn condense_chunk(chunk: &str) -> Result<Value> {
    let timestamp = timestamp_schema();
    let source_fields = json!({
        "start": timestamp.clone(),
        "end": timestamp
    });
    let schema = json!({
        "type": "object",
        "properties": {
            "topics": {
                "type": "array", "minItems": 2, "maxItems": 6,
                "items": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": timestamp_schema(),
                        "end": timestamp_schema(),
                        "facts": {
                            "type": "array", "minItems": 3, "maxItems": 10,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string" },
                                    "start": timestamp_schema(),
                                    "end": timestamp_schema()
                                },
                                "required": ["text", "start", "end"]
                            }
                        }
                    },
                    "required": ["title", "start", "end", "facts"]
                }
            },
            "decisions": {
                "type": "array", "maxItems": 8,
                "items": {
                    "type": "object", "properties": {
                        "decision": { "type": "string" }, "detail": { "type": "string" },
                        "start": source_fields["start"].clone(), "end": source_fields["end"].clone()
                    }, "required": ["decision", "detail", "start", "end"]
                }
            },
            "actions": {
                "type": "array", "maxItems": 10,
                "items": {
                    "type": "object", "properties": {
                        "owner": { "type": "string" }, "action": { "type": "string" },
                        "timing": { "type": "string" }, "dependency": { "type": "string" },
                        "start": source_fields["start"].clone(), "end": source_fields["end"].clone()
                    }, "required": ["owner", "action", "timing", "dependency", "start", "end"]
                }
            },
            "open_questions": {
                "type": "array", "maxItems": 8,
                "items": {
                    "type": "object", "properties": {
                        "text": { "type": "string" },
                        "start": source_fields["start"].clone(), "end": source_fields["end"].clone()
                    }, "required": ["text", "start", "end"]
                }
            },
            "risks": {
                "type": "array", "maxItems": 8,
                "items": {
                    "type": "object", "properties": {
                        "risk": { "type": "string" }, "impact": { "type": "string" },
                        "response": { "type": "string" },
                        "start": source_fields["start"].clone(), "end": source_fields["end"].clone()
                    }, "required": ["risk", "impact", "response", "start", "end"]
                }
            }
        },
        "required": ["topics", "decisions", "actions", "open_questions", "risks"]
    });
    let instruction = "Build a dense, typed chapter ledger from this portion of a longer meeting. \
        Preserve every substantive workstream and keep separate topics separate. For each topic, \
        capture concrete facts, examples, rationale, constraints, numbers, options, and implications. \
        Separately capture every actual decision, post-meeting commitment or needed follow-up, unresolved \
        question, and risk. Do not treat agenda steps, audio checks, screen sharing, explanations, or \
        anything already completed inside this chapter as a future action. Copy exact mm:ss timestamps from \
        the source for start and end; use the same timestamp \
        for both when only one line supports the point. Use a real participant name when the supplied \
        transcript does; otherwise use Unassigned, never Speaker N. If a proper noun is inconsistent or \
        uncertain, describe it as uncertain rather than silently choosing a spelling. A risk response \
        must be stated or directly entailed by the source; otherwise leave it empty. Do not summarize \
        this chunk into a short overview. Respond only with the requested JSON.";
    ollama::chat_json_local_ctx(
        &ollama::text_model(),
        instruction,
        chunk,
        None,
        Some(schema),
        24_576,
    )
    .await
    .and_then(model_object)
}

/// Ensure every captured transcript has a real, searchable library projection
/// before model work begins. Generated notes replace this deterministic
/// transcript fallback once usable notes are available. Incomplete notes retain
/// a visible warning, and the original transcript remains on the meeting.
pub async fn ensure_note_projection(app: &tauri::AppHandle, meeting_id: i64) -> Result<i64> {
    let (meeting, segments) = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let meeting = store::get_meeting(&conn, meeting_id)?;
        if let Some(note_id) = meeting["note_id"].as_i64() {
            return Ok(note_id);
        }
        let segments = store::list_segments(&conn, meeting_id)?;
        (meeting, segments)
    };
    if segments.is_empty() {
        return Err(anyhow!("no transcript to file"));
    }

    let title = meeting["title"].as_str().unwrap_or("Meeting").to_string();
    let raw_notes = meeting["raw_notes"].as_str().unwrap_or("").to_string();
    let (attendees, remote_alias) = meeting_participants(&title, &meeting["event_json"]);
    let date = meeting["started_at"]
        .as_str()
        .map(|value| value.chars().take(10).collect::<String>())
        .unwrap_or_default();
    let transcript = transcript_text_with_remote_alias(
        &segments,
        meeting["capture_mode"].as_str() == Some("in_person"),
        remote_alias.as_deref(),
    );
    let mut note_text = format!(
        "# {title}\n\n_Transcript saved. Generated meeting notes will replace this view when ready._"
    );
    if !raw_notes.trim().is_empty() {
        note_text.push_str(&format!(
            "\n\n## Your Notes (verbatim)\n\n{}",
            raw_notes.trim()
        ));
    }
    note_text.push_str(&format!("\n\n## Transcript\n\n{transcript}"));

    let (me_ms, them_ms) = {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        store::talk_time(&conn, meeting_id)?
    };
    let duration_min = segments
        .iter()
        .filter_map(|segment| segment["t1_ms"].as_i64())
        .max()
        .unwrap_or(0)
        / 60_000;
    let entities: Vec<Value> = attendees
        .iter()
        .map(|name| json!({ "name": name, "type": "person" }))
        .collect();
    let route_status = meeting["route_status"].as_str();
    let route_folder_id = matches!(route_status, Some("matched" | "manual"))
        .then(|| meeting["route_folder_id"].as_i64())
        .flatten();
    // An exact approved meeting rule owns context as well as destination.
    // Without one, use the Work/Personal choice captured when recording began;
    // never consult whichever context happens to be active now.
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
        "event_date": if date.is_empty() { crate::today_local() } else { date },
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
    let args = serde_json::from_value::<crate::SaveArgs>(save_json)
        .map_err(|error| anyhow!("meeting note data was invalid: {error}"))?;
    let note_id = crate::save_entry(app.clone(), args)
        .await
        .map_err(|error| anyhow!("meeting note could not be saved: {error}"))?;
    let now = chrono::Utc::now().to_rfc3339();
    let state = app.state::<Db>();
    let conn = state.0.lock().unwrap();
    if let Err(error) = store::set_note_id_and_apply_route(&conn, meeting_id, note_id, &now) {
        eprintln!("[noted] meeting route filing failed: {error}");
        // The route application is transactional. Preserve the projection link
        // so a retry cannot create a duplicate document; its broad save
        // destination remains visible and can be re-filed separately.
        store::set_note_id(&conn, meeting_id, note_id).map_err(|link_error| {
            anyhow!(
                "meeting note was saved but could not be linked: {link_error}; route error: {error}"
            )
        })?;
    }
    drop(conn);
    let _ = app.emit("note-filed", json!({ "id": note_id }));
    Ok(note_id)
}

/// Generate or refresh one template summary for a meeting. The transcript
/// projection already exists; a successful primary summary refreshes it.
pub async fn run(
    app: &tauri::AppHandle,
    meeting_id: i64,
    template_name: Option<String>,
) -> Result<String> {
    if let Err(error) = ensure_note_projection(app, meeting_id).await {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        let _ = store::mark_summary_failed(&conn, meeting_id, &error.to_string());
        return Err(error);
    }
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        store::mark_summary_attempt(&conn, meeting_id)?;
    }
    let result = run_inner(app, meeting_id, template_name).await;
    if let Err(error) = &result {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        if let Err(persist_error) =
            store::mark_summary_failed(&conn, meeting_id, &error.to_string())
        {
            eprintln!(
                "[noted] failed to persist meeting summary error for {meeting_id}: {persist_error}"
            );
        }
    }
    result
}

async fn run_inner(
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
    let (attendees, remote_alias) = meeting_participants(&title, &meeting["event_json"]);
    let date = meeting["started_at"]
        .as_str()
        .map(|s| s.chars().take(10).collect::<String>())
        .unwrap_or_default();
    let coverage = CoveragePlan::for_segments(&segments);
    let coverage_instructions = coverage.instructions();
    let mut valid_timestamps = segments
        .iter()
        .filter_map(|segment| segment["t0_ms"].as_i64())
        .map(mmss)
        .collect::<HashSet<_>>();
    if !raw_notes.trim().is_empty() {
        valid_timestamps.insert("notes".to_string());
    }

    let mut transcript = transcript_text_with_remote_alias(
        &segments,
        meeting["capture_mode"].as_str() == Some("in_person"),
        remote_alias.as_deref(),
    );
    let mut expected_ledger_counts = None;
    let mut chapter_ledgers = Vec::new();
    if transcript.len() > SINGLE_PASS_CHARS {
        // Chapter-preserving map/reduce: keep topic boundaries and typed facts
        // instead of handing the composer a lossy flat list.
        let mut condensed = String::new();
        for (index, chunk) in transcript_chunks(&transcript).into_iter().enumerate() {
            let ledger = match condense_chunk(&chunk).await {
                Ok(ledger) => Ok(ledger),
                Err(first_error) => {
                    eprintln!(
                        "[noted] meeting {meeting_id} chapter {} extraction failed; retrying once: {first_error}",
                        index + 1
                    );
                    condense_chunk(&chunk).await.map_err(|second_error| {
                        anyhow!(
                            "chapter extraction failed twice: first attempt: {first_error}; retry: {second_error}"
                        )
                    })
                }
            };
            match ledger {
                Ok(mut ledger) => {
                    // Repair or discard chapter citations before they become
                    // source material for the final composer. Otherwise one
                    // malformed ledger can propagate the same broken syntax
                    // through every summary repair pass.
                    normalize_pack_source_timestamps(&mut ledger, &valid_timestamps);
                    condensed.push_str(&format!("\nSOURCE CHAPTER {}:\n", index + 1));
                    condensed.push_str(&serde_json::to_string_pretty(&ledger)?);
                    condensed.push('\n');
                    chapter_ledgers.push(ledger);
                }
                Err(error) => {
                    eprintln!(
                        "[noted] meeting {meeting_id} chapter {} extraction failed; preserving the raw chapter: {error}",
                        index + 1
                    );
                    condensed.push_str(&format!(
                        "\nSOURCE CHAPTER {} (VERBATIM FALLBACK):\n{chunk}\n",
                        index + 1
                    ));
                }
            }
        }
        if !chapter_ledgers.is_empty() {
            expected_ledger_counts = Some(chapter_ledger_counts(&chapter_ledgers));
        }
        transcript = format!(
            "TYPED CHAPTER LEDGERS (extracted in order across the complete transcript; \
             every object remains source evidence):\n{condensed}"
        );
    }

    let user = format!(
        "{coverage_instructions}\n\n\
         TEMPLATE LENS — use this to organize the detailed discussion and emphasize \
         the most useful domain-specific material, while still returning the complete \
         shared Meeting Pack:\n{prompt}\n\n\
         MEETING: {title}{}{}\n\n\
         MY TYPED NOTES:\n{}\n\n\
         TRANSCRIPT OR SOURCE LEDGER:\n{transcript}\n\n\
         END OF SOURCE.\n\n\
         FINAL COVERAGE CHECK: {coverage_instructions} Before responding, verify that \
         every source chapter appears in the timeline and detailed discussion; all \
         grounded decisions, actions, questions, and risks survive; and evidence spans \
         the complete time range. Return the Meeting Pack JSON only.",
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
    let first_attempt = ollama::chat_json_local_ctx(
        &ollama::text_model(),
        SYSTEM,
        &user,
        None,
        Some(meeting_pack_schema(coverage)),
        24_576,
    )
    .await
    .and_then(model_object);
    let mut out = match first_attempt {
        Ok(pack) => pack,
        Err(first_error) => {
            eprintln!(
                "[noted] meeting {meeting_id} initial Meeting Pack composition failed; retrying once: {first_error}"
            );
            let retry_user = format!(
                "{user}\n\nRECOVERY RETRY: The first structured response failed to parse or complete. \
                 Return one complete, valid Meeting Pack JSON object now. Keep the content grounded \
                 and concise enough to finish every required field."
            );
            ollama::chat_json_local_ctx(
                &ollama::text_model(),
                SYSTEM,
                &retry_user,
                None,
                Some(meeting_pack_schema(coverage)),
                24_576,
            )
            .await
            .and_then(model_object)
            .map_err(|second_error| {
                anyhow!(
                    "Meeting Pack generation failed twice: first attempt: {first_error}; retry: {second_error}"
                )
            })?
        }
    };
    normalize_pack_source_timestamps(&mut out, &valid_timestamps);
    sanitize_meeting_pack(&mut out);
    backfill_pack_from_chapter_ledgers(&mut out, &chapter_ledgers);
    normalize_pack_source_timestamps(&mut out, &valid_timestamps);
    sanitize_meeting_pack(&mut out);
    for attempt in 1..=2 {
        let deficiencies = meeting_pack_deficiencies(
            &out,
            coverage,
            Some(&valid_timestamps),
            expected_ledger_counts,
        );
        if deficiencies.is_empty() {
            break;
        }
        let stats = meeting_pack_stats(&out, coverage.duration_min, Some(&valid_timestamps));
        let repair_user = format!(
            "{user}\n\nQUALITY REPAIR {attempt}: The previous Meeting Pack contained about \
             {} grounded words, {} detailed discussion points, and {} timeline chapters. \
             It is not acceptable because it is missing: {}. Regenerate the complete \
             Meeting Pack from the full supplied source. Add grounded specificity and \
             missing source chapters; do not add filler or unsupported claims.",
            stats.words,
            stats.discussion_points,
            stats.timeline_points,
            deficiencies.join(", ")
        );
        match ollama::chat_json_local_ctx(
            &ollama::text_model(),
            SYSTEM,
            &repair_user,
            None,
            Some(meeting_pack_schema(coverage)),
            24_576,
        )
        .await
        .and_then(model_object)
        {
            Ok(mut candidate) => {
                normalize_pack_source_timestamps(&mut candidate, &valid_timestamps);
                sanitize_meeting_pack(&mut candidate);
                backfill_pack_from_chapter_ledgers(&mut candidate, &chapter_ledgers);
                normalize_pack_source_timestamps(&mut candidate, &valid_timestamps);
                sanitize_meeting_pack(&mut candidate);
                out = richer_meeting_pack(out, candidate, coverage, Some(&valid_timestamps))
            }
            Err(error) => eprintln!("[noted] meeting summary expansion pass failed: {error}"),
        }
    }
    backfill_pack_from_chapter_ledgers(&mut out, &chapter_ledgers);
    normalize_pack_source_timestamps(&mut out, &valid_timestamps);
    sanitize_meeting_pack(&mut out);
    let (md, quality_warning) = finalize_meeting_pack(
        &mut out,
        coverage,
        &valid_timestamps,
        expected_ledger_counts,
    )?;
    if quality_warning.is_some() {
        eprintln!(
            "[noted] meeting {meeting_id} summary quality warnings: {}",
            out["quality_warnings"]
        );
        protect_existing_summary(
            &meeting["summaries"],
            &template,
            &out,
            coverage,
            &valid_timestamps,
        )?;
    }

    let now = chrono::Utc::now().to_rfc3339();
    let first_summary = meeting["summaries"]
        .as_array()
        .is_none_or(|summaries| summaries.is_empty());
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        store::insert_summary(&conn, meeting_id, &template, &md, Some(&out), &now)?;
    }

    let mut note_text = format!("# {title}\n\n{md}");
    if !raw_notes.trim().is_empty() {
        note_text.push_str(&format!(
            "\n\n## Your Notes (verbatim)\n\n{}",
            raw_notes.trim()
        ));
    }

    if first_summary || template == super::cfg().default_template {
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
        store::mark_summary_succeeded(&conn, meeting_id, quality_warning.as_deref())?;
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
        assert!(coverage.instructions().contains("1,400-2,200 words"));
        assert!(coverage.instructions().contains("at least 18 distinct"));

        let sparse = json!({ "sections": [
            { "heading": "Overview", "kind": "paragraph", "paragraph": "A very short overview." },
            { "heading": "Actions", "kind": "todos", "items": [
                { "text": "Franek — revise the hero", "source": "40:18" }
            ] }
        ]});
        assert!(coverage.requires_expansion(summary_stats(&sparse)));
    }

    #[test]
    fn comprehensive_pack_schema_caps_repeating_sections() {
        let coverage = CoveragePlan {
            level: CoverageLevel::Comprehensive,
            duration_min: 50,
            transcript_words: 6_400,
            minimum_summary_words: 1_200,
            minimum_detail_points: 18,
        };
        let schema = meeting_pack_schema(coverage);
        let properties = &schema["properties"];

        assert_eq!(properties["timeline"]["maxItems"], 12);
        assert_eq!(
            properties["timeline"]["items"]["properties"]["start"]["pattern"],
            "^([0-9]{2,}:[0-5][0-9]|notes)$"
        );
        assert_eq!(properties["discussion"]["maxItems"], 8);
        assert_eq!(
            properties["discussion"]["items"]["properties"]["points"]["maxItems"],
            8
        );
        assert_eq!(properties["actions"]["maxItems"], 10);
        assert_eq!(
            properties["actions"]["items"]["properties"]["sources"]["maxItems"],
            4
        );
    }

    #[test]
    fn one_on_one_title_supplies_missing_participant_identity() {
        assert_eq!(
            one_on_one_title_participant("1:1 with Chris Onboarding"),
            Some("Chris".to_string())
        );
        assert_eq!(
            meeting_participants("1:1 with Chris", &json!({})),
            (vec!["Chris".to_string()], Some("Chris".to_string()))
        );
        assert_eq!(one_on_one_title_participant("Project review"), None);
    }

    #[test]
    fn remote_alias_replaces_fragmented_speaker_labels_for_clear_one_on_one() {
        let segments = vec![
            json!({ "t0_ms": 1_000, "channel": "me", "text": "Welcome", "speaker": null }),
            json!({ "t0_ms": 2_000, "channel": "them", "text": "Thanks", "speaker": "Speaker 4" }),
        ];
        let transcript = transcript_text_with_remote_alias(&segments, false, Some("Chris"));
        assert!(transcript.contains("[00:02] Chris: Thanks"));
        assert!(!transcript.contains("Speaker 4"));
    }

    #[test]
    fn comprehensive_pack_requires_depth_time_coverage_and_grounded_length() {
        let coverage = CoveragePlan {
            level: CoverageLevel::Comprehensive,
            duration_min: 50,
            transcript_words: 6_400,
            minimum_summary_words: 1_200,
            minimum_detail_points: 18,
        };
        let source = |start: &str| json!({ "start": start, "end": start });
        let topic_starts = ["02:00", "12:00", "22:00", "32:00", "42:00"];
        let discussion = topic_starts
            .iter()
            .enumerate()
            .map(|(index, start)| json!({
                "title": format!("Topic {}", index + 1),
                "summary": "A grounded topic summary with concrete operating context.",
                "points": (0..4).map(|point| json!({
                    "text": format!("{} {}", "specific evidence rationale constraint example implication ".repeat(13), point),
                    "sources": [source(start)]
                })).collect::<Vec<_>>()
            }))
            .collect::<Vec<_>>();
        let pack = json!({
            "executive_summary": "The meeting established a concrete direction and the work required to carry it forward.",
            "executive_sources": [source("02:00"), source("42:00")],
            "success_definition": "Progress means completing the stated work while resolving its dependencies.",
            "success_sources": [source("32:00")],
            "at_glance": [
                {"item":"Purpose","details":"Plan the work.","sources":[source("02:00")]},
                {"item":"Focus","details":"Execution and dependencies.","sources":[source("12:00")]},
                {"item":"Stage","details":"Early implementation.","sources":[source("22:00")]}
            ],
            "timeline": [
                {"start":"02:00","end":"08:00","topic":"Opening","details":"Context"},
                {"start":"10:00","end":"16:00","topic":"Area one","details":"Details"},
                {"start":"18:00","end":"24:00","topic":"Area two","details":"Details"},
                {"start":"26:00","end":"32:00","topic":"Area three","details":"Details"},
                {"start":"34:00","end":"40:00","topic":"Plan","details":"Details"},
                {"start":"42:00","end":"49:00","topic":"Close","details":"Details"}
            ],
            "discussion": discussion,
            "decisions": [], "workplan": [], "actions": [],
            "open_questions": [], "risks": []
        });
        assert!(meeting_pack_deficiencies(&pack, coverage, None, None).is_empty());

        let mut sparse = pack.clone();
        sparse["discussion"] = json!([sparse["discussion"][0].clone()]);
        let deficiencies = meeting_pack_deficiencies(&sparse, coverage, None, None).join(" ");
        assert!(deficiencies.contains("detailed discussion topics"));
        assert!(deficiencies.contains("substantive discussion points"));
    }

    #[test]
    fn meeting_pack_markdown_preserves_hierarchy_tables_and_source_ranges() {
        let pack = json!({
            "executive_summary": "The launch plan is clear.",
            "executive_sources": [{"start":"01:00","end":"02:00"}],
            "success_definition": "Ship the pilot.",
            "success_sources": [{"start":"02:00","end":"02:00"}],
            "at_glance": [{"item":"Focus","details":"Pilot","sources":[{"start":"01:00","end":"02:00"}]}],
            "timeline": [{"start":"01:00","end":"02:00","topic":"Plan","details":"Pilot scope"}],
            "discussion": [{"title":"Pilot design","summary":"The design is bounded.","points":[{"text":"Use twelve customers.","sources":[{"start":"01:00","end":"02:00"}]}]}],
            "decisions": [], "workplan": [], "actions": [], "open_questions": [], "risks": []
        });
        let valid = HashSet::from(["01:00".to_string(), "02:00".to_string()]);
        assert_eq!(invalid_pack_source_count(&pack, &valid), 0);
        let mut invalid = pack.clone();
        invalid["discussion"][0]["points"][0]["sources"][0]["start"] = json!("01:01");
        invalid["discussion"][0]["points"][0]["sources"][0]["end"] = json!("unknown");
        assert_eq!(invalid_pack_source_count(&invalid, &valid), 1);
        normalize_pack_source_timestamps(&mut invalid, &valid);
        assert_eq!(invalid_pack_source_count(&invalid, &valid), 0);
        assert_eq!(
            invalid["discussion"][0]["points"][0]["sources"][0]["start"],
            "01:00"
        );
        assert_eq!(
            invalid["discussion"][0]["points"][0]["sources"][0]["end"],
            "01:00"
        );
        let markdown = render_meeting_pack(&pack, Some(&valid));
        assert!(markdown.contains("## Meeting at a Glance"));
        assert!(markdown.contains("| Item | Details |"));
        assert!(markdown.contains("### 1. Pilot design"));
        assert!(markdown.contains("[01:00-02:00]"));
    }

    #[test]
    fn malformed_model_timestamps_are_repaired_or_hidden_before_rendering() {
        assert_eq!(loose_timestamp_seconds(":16"), Some(16));
        assert_eq!(loose_timestamp_seconds(":826"), Some(8 * 60 + 26));
        assert_eq!(loose_timestamp_seconds(":13:38"), Some(13 * 60 + 38));
        assert_eq!(loose_timestamp_seconds("]==40:02"), Some(40 * 60 + 2));
        assert_eq!(loose_timestamp_seconds(":296"), None);
        assert_eq!(timestamp_seconds("13:99"), None);
        assert_eq!(loose_timestamp_seconds("recorded at 40:02"), None);
        assert_eq!(loose_timestamp_seconds("01:02:03"), None);
        assert_eq!(timestamp_seconds("9223372036854775807:59"), None);
        assert_eq!(loose_timestamp_seconds(":922337203685477580759"), None);

        let valid = HashSet::from([
            "00:16".to_string(),
            "00:41".to_string(),
            "00:58".to_string(),
            "03:26".to_string(),
            "08:26".to_string(),
            "09:58".to_string(),
            "13:38".to_string(),
            "26:51".to_string(),
            "40:02".to_string(),
            "41:58".to_string(),
        ]);
        let mut pack = json!({
            "executive_summary":"Grounded summary",
            "executive_sources":[{"start":":16","end":":58"}],
            "success_definition":"", "success_sources":[], "at_glance":[],
            "timeline":[
                {"start":":16","end":":58","topic":"Opening","details":"Context"},
                {"start":":826","end":":958","topic":"Dashboard","details":"Test data"},
                {"start":":13:38","end":":26:51","topic":"Engineering","details":"Sentry"},
                {"start":"]==40:02","end":"]==41:58","topic":"Delivery","details":"Options"},
                {"start":":326","end":":41","topic":"Reversed citation","details":"Do not render"},
                {"start":":296","end":":397","topic":"Broken citation","details":"Do not render"}
            ],
            "discussion":[],
            "decisions":[{"decision":"Keep the plan","detail":"","sources":[]}],
            "workplan":[],
            "actions":[
                {"owner":"Unassigned","action":"Check the account","timing":":296-:304","dependency":"","sources":[]},
                {"owner":"Chris","action":"Review the plan","timing":"Tomorrow","dependency":"","sources":[{"start":"]==40:02","end":"]==41:58"}]}
            ],
            "open_questions":[],
            "risks":[{"risk":"A delay","impact":"Launch moves","response":"","sources":[]}]
        });

        normalize_pack_source_timestamps(&mut pack, &valid);
        sanitize_meeting_pack(&mut pack);

        assert_eq!(pack["timeline"][0]["start"], "00:16");
        assert_eq!(pack["timeline"][1]["start"], "08:26");
        assert_eq!(pack["timeline"][2]["start"], "13:38");
        assert_eq!(pack["timeline"][3]["start"], "40:02");
        assert_eq!(pack["timeline"][4]["start"], "");
        assert_eq!(pack["timeline"][5]["start"], "");
        assert_eq!(pack["actions"][0]["timing"], "");

        let markdown = render_meeting_pack(&pack, Some(&valid));
        assert!(markdown.contains("00:16-00:58"));
        assert!(markdown.contains("08:26-09:58"));
        assert!(markdown.contains("40:02-41:58"));
        assert!(!markdown.contains("Reversed citation"));
        assert!(!markdown.contains("Broken citation"));
        assert!(!markdown.contains(":826"));
        assert!(!markdown.contains("==40:02"));
        assert!(!markdown.contains("Dependency or note"));
        assert!(!markdown.contains("| Reference |"));
        assert!(!markdown.contains("Practical response"));
    }

    #[test]
    fn citation_repairs_are_bounded_and_break_ties_consistently() {
        let valid = HashSet::from([
            "01:00".to_string(),
            "02:00".to_string(),
            "notes".to_string(),
        ]);
        assert_eq!(
            nearest_valid_timestamp("01:30", &valid).as_deref(),
            Some("01:00")
        );
        assert_eq!(nearest_valid_timestamp("30:00", &valid), None);
        let mut sources = json!([
            {"start": "notes", "end": "01:00"},
            {"start": "30:00", "end": "31:00"},
            {"start": "notes", "end": "notes"}
        ]);
        normalize_pack_source_timestamps(&mut sources, &valid);
        assert_eq!(
            normalized_pack_sources(&sources, Some(&valid)),
            vec!["[notes]"]
        );
        assert!(
            normalized_pack_sources(&json!([{"start": ":296", "end": ":397"}]), None).is_empty()
        );
    }

    #[test]
    fn incomplete_regeneration_keeps_existing_notes_unless_it_adds_substance() {
        let coverage =
            CoveragePlan::for_segments(&[json!({"t1_ms": 3_600_000, "text": "Launch planning"})]);
        let valid = HashSet::from(["01:00".to_string()]);
        let poor = json!({"executive_summary": "Launch planning."});
        let rich = json!({"executive_summary": "The launch requires approval and a named owner before the release can proceed."});
        let existing = json!([{"template": "Meeting", "content_json": rich}]);
        assert!(protect_existing_summary(&existing, "Meeting", &poor, coverage, &valid).is_err());
        assert!(protect_existing_summary(&existing, "Sales", &poor, coverage, &valid).is_ok());
        let edited = json!([{"template": "Meeting", "content_json": null, "content_md": "My corrected notes"}]);
        assert!(protect_existing_summary(&edited, "Meeting", &rich, coverage, &valid).is_err());
        let existing = json!([{"template": "Meeting", "content_json": poor}]);
        assert!(protect_existing_summary(&existing, "Meeting", &rich, coverage, &valid).is_ok());
        let mut unchanged = poor.clone();
        unchanged["quality_warnings"] =
            json!(["Warnings must not inflate the substance score.".repeat(50)]);
        assert!(
            protect_existing_summary(&existing, "Meeting", &unchanged, coverage, &valid).is_err()
        );
    }

    #[test]
    fn usable_incomplete_notes_are_saved_with_a_warning_but_empty_output_is_rejected() {
        let coverage =
            CoveragePlan::for_segments(&[json!({"t1_ms": 3_600_000, "text": "Plan the launch"})]);
        let valid = HashSet::from(["01:00".to_string()]);
        let mut sparse = json!({
            "executive_summary": "The launch depends on approval.",
            "executive_sources": [{"start": "01:00", "end": "01:00"}]
        });
        let (markdown, warning) =
            finalize_meeting_pack(&mut sparse, coverage, &valid, None).unwrap();
        assert!(markdown.contains("The launch depends on approval. [01:00]"));
        assert!(warning.unwrap().contains("may be incomplete"));
        assert!(!sparse["quality_warnings"].as_array().unwrap().is_empty());
        assert!(finalize_meeting_pack(&mut json!({}), coverage, &valid, None).is_err());
        for malformed in [json!([]), json!("summary"), Value::Null] {
            assert!(model_object(malformed).is_err());
        }

        let brief =
            CoveragePlan::for_segments(&[json!({"t1_ms": 90_000, "text": "Plan the launch"})]);
        sparse["discussion"] = json!([{
            "title": "Launch", "summary": "", "points": [{
                "text": "Request approval before shipping.",
                "sources": [{"start": "01:00", "end": "01:00"}]
            }]
        }]);
        let (_, warning) = finalize_meeting_pack(&mut sparse, brief, &valid, None).unwrap();
        assert!(warning.is_none());
        assert_eq!(sparse["quality_warnings"], json!([]));
    }

    #[test]
    fn meeting_pack_sanitizer_removes_repetition_and_in_meeting_mechanics() {
        let point = |text: &str| json!({"text": text, "sources": []});
        let mut pack = json!({
            "at_glance": [
                {"item":"Goal", "details":"Onboard a brand"},
                {"item":"Goal", "details":"Onboard a brand"}
            ],
            "timeline": [],
            "discussion": [
                {"title":"Onboarding", "summary":"Plan", "points":[point("Use the playbook"), point("Set up the site")]},
                {"title":"Onboarding", "summary":"Repeated", "points":[point("Use the playbook")]},
                {"title":"Security", "summary":"Review", "points":[point("Use the playbook"), point("Review the agent findings")]}
            ],
            "decisions": [],
            "workplan": [],
            "actions": [
                {"owner":"Me", "action":"Share screen for demonstration", "timing":"00:11-08:21"},
                {"owner":"Chris", "action":"Review the security findings", "timing":"This week"},
                {"owner":"Chris", "action":"Review the security findings", "timing":"This week"}
            ],
            "open_questions": [],
            "risks": []
        });

        sanitize_meeting_pack(&mut pack);

        assert_eq!(pack_array_len(&pack, "at_glance"), 1);
        assert_eq!(pack_array_len(&pack, "discussion"), 2);
        assert_eq!(pack_discussion_points(&pack), 3);
        assert_eq!(pack_array_len(&pack, "actions"), 1);
        assert_eq!(pack["actions"][0]["action"], "Review the security findings");
    }

    #[test]
    fn sanitizer_clears_source_ranges_from_timing_and_removes_screen_share_actions() {
        assert!(looks_like_source_range("[00:29] - [00:56]"));
        assert!(looks_like_source_range(":296-:304"));
        let mut pack = json!({
            "at_glance": [], "timeline": [], "discussion": [], "decisions": [],
            "workplan": [],
            "actions": [
                {"owner":"Me","action":"Share screen","timing":"[00:29] - [00:56]"},
                {"owner":"Chris","action":"Check the account","timing":":296-:304"},
                {"owner":"Chris","action":"Audit the payment adapter","timing":"Tomorrow"},
                {"owner":"Chris","action":"Share screen recording with the customer","timing":"Tomorrow"}
            ],
            "open_questions": [], "risks": []
        });
        sanitize_meeting_pack(&mut pack);
        assert_eq!(pack_array_len(&pack, "actions"), 3);
        assert_eq!(pack["actions"][0]["action"], "Check the account");
        assert_eq!(pack["actions"][0]["timing"], "");
        assert_eq!(pack["actions"][1]["action"], "Audit the payment adapter");
        assert_eq!(
            pack["actions"][2]["action"],
            "Share screen recording with the customer"
        );
    }

    #[test]
    fn chapter_backfill_keeps_late_topics_and_actions_without_replacing_composed_detail() {
        let ledgers = (1..=15)
            .map(|number| {
                let start = format!("{:02}:00", number * 3);
                let title = format!("Workstream {number}");
                json!({
                    "topics": [{"title": title, "start": start, "end": start, "facts": [{
                        "text": format!("Preserve the decision for workstream {number}."),
                        "start": start, "end": start
                    }]}],
                    "actions": [{"owner": "Chris", "action": format!("Deliver workstream {number}"),
                        "timing": "Tomorrow", "dependency": "", "start": start, "end": start}]
                })
            })
            .collect::<Vec<_>>();
        let mut pack = json!({
            "timeline": [{"topic": "Workstream 1", "start": "03:00", "end": "03:00",
                "details": "Existing composed detail with the rationale."}],
            "actions": [{"owner": "Chris", "action": "Deliver workstream 1", "timing": "Tomorrow",
                "dependency": "Approved credentials", "sources": [{"start": "03:00", "end": "03:00"}]}]
        });
        sanitize_meeting_pack(&mut pack);
        backfill_pack_from_chapter_ledgers(&mut pack, &ledgers);
        sanitize_meeting_pack(&mut pack);
        assert_eq!(pack_array_len(&pack, "timeline"), 15);
        assert_eq!(pack_array_len(&pack, "discussion"), 15);
        assert_eq!(pack_array_len(&pack, "actions"), 15);
        assert_eq!(
            pack["timeline"][0]["details"],
            "Existing composed detail with the rationale."
        );
        assert_eq!(pack["actions"][0]["dependency"], "Approved credentials");
        assert_eq!(pack["discussion"][14]["title"], "Workstream 15");
        assert_eq!(pack["actions"][14]["action"], "Deliver workstream 15");
        let once = pack.clone();
        backfill_pack_from_chapter_ledgers(&mut pack, &ledgers);
        sanitize_meeting_pack(&mut pack);
        assert_eq!(pack, once);
    }

    #[test]
    fn chapter_ledgers_are_deterministically_restored_into_a_sparse_pack() {
        let chapter = |title: &str, start: &str, end: &str, number: usize| {
            json!({
                "title": title,
                "start": start,
                "end": end,
                "facts": (0..3).map(|fact| json!({
                    "text": format!(
                        "Distinct grounded fact {number}-{fact} with rationale, constraint, implementation detail, concrete example, dependency, tradeoff, operating implication, and next-step context. {}",
                        "The chapter preserves additional source-specific evidence without replacing another workstream. ".repeat(3)
                    ),
                    "start": start,
                    "end": end
                })).collect::<Vec<_>>()
            })
        };
        let ledgers = vec![
            json!({
                "topics": [
                    chapter("Onboarding", "01:00", "08:00", 1),
                    chapter("Payments", "10:00", "16:00", 2),
                    chapter("Email lifecycle", "18:00", "24:00", 3)
                ],
                "decisions": [{"decision":"Use adapters","detail":"Reuse integrations","start":"10:00","end":"16:00"}],
                "actions": [{"owner":"Chris","action":"Audit payment wiring","timing":"Tomorrow","dependency":"Access","start":"10:00","end":"16:00"}],
                "open_questions": [{"text":"Which provider is active?","start":"10:00","end":"16:00"}],
                "risks": [{"risk":"Missing wiring","impact":"Launch delay","response":"Audit configuration","start":"01:00","end":"08:00"}]
            }),
            json!({
                "topics": [
                    chapter("Experiments", "26:00", "32:00", 4),
                    chapter("Documentation", "34:00", "40:00", 5),
                    chapter("Delivery plan", "42:00", "48:00", 6)
                ],
                "decisions": [], "actions": [], "open_questions": [], "risks": []
            }),
        ];
        let mut pack = json!({
            "executive_summary":"Summary", "executive_sources":[],
            "success_definition":"Success", "success_sources":[],
            "at_glance":[
                {"item":"Purpose","details":"Plan implementation","sources":[]},
                {"item":"Focus","details":"Onboarding and architecture","sources":[]},
                {"item":"Timing","details":"Immediate delivery","sources":[]}
            ],
            "timeline":[], "discussion":[], "decisions":[], "workplan":[],
            "actions":[{"owner":"Me","action":"Share screen","timing":"[00:29] - [00:56]","dependency":"","sources":[]}],
            "open_questions":[], "risks":[]
        });

        backfill_pack_from_chapter_ledgers(&mut pack, &ledgers);
        sanitize_meeting_pack(&mut pack);

        assert_eq!(pack_array_len(&pack, "timeline"), 6);
        assert_eq!(pack_array_len(&pack, "discussion"), 6);
        assert_eq!(pack_discussion_points(&pack), 18);
        assert_eq!(pack_array_len(&pack, "decisions"), 1);
        assert_eq!(pack_array_len(&pack, "actions"), 1);
        assert_eq!(pack["actions"][0]["action"], "Audit payment wiring");
        assert_eq!(pack_array_len(&pack, "open_questions"), 1);
        assert_eq!(pack_array_len(&pack, "risks"), 1);
        assert_eq!(meeting_pack_stats(&pack, 50, None).covered_time_buckets, 5);
        let coverage = CoveragePlan {
            level: CoverageLevel::Comprehensive,
            duration_min: 50,
            transcript_words: 5_000,
            minimum_summary_words: 800,
            minimum_detail_points: 18,
        };
        assert!(meeting_pack_deficiencies(
            &pack,
            coverage,
            None,
            Some(chapter_ledger_counts(&ledgers))
        )
        .is_empty());
    }

    #[test]
    fn ledger_registers_keep_a_consolidated_retention_floor() {
        let ledgers = vec![
            json!({"decisions":[{},{}],"actions":[{},{},{}],"open_questions":[{}],"risks":[{}]}),
            json!({"decisions":[{}],"actions":[{},{}],"open_questions":[{},{}],"risks":[{},{}]}),
        ];
        let counts = chapter_ledger_counts(&ledgers);
        assert_eq!(
            counts,
            LedgerCounts {
                decisions: 3,
                actions: 5,
                open_questions: 3,
                risks: 3,
            }
        );
        assert_eq!(minimum_retained_count(counts.actions), 2);
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
