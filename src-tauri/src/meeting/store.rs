// DB access for the meeting recorder. Transcript segments live in their own
// table (not entries.data_json) because they're large and append-heavy while
// recording. The AI summary is additionally filed as a regular note (see
// summarize.rs) so search/embeddings/KG stay unchanged.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

pub fn create_meeting(
    conn: &Connection,
    title: &str,
    event_id: Option<&str>,
    event_json: Option<&str>,
    now: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO meetings (title, event_id, event_json, started_at, status, created_at)
         VALUES (?1, ?2, ?3, ?4, 'recording', ?4)",
        rusqlite::params![title, event_id, event_json, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn set_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET status = ?2 WHERE id = ?1",
        rusqlite::params![id, status],
    )?;
    Ok(())
}

pub fn set_ended(conn: &Connection, id: i64, ended_at: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET ended_at = ?2, status = ?3 WHERE id = ?1",
        rusqlite::params![id, ended_at, status],
    )?;
    Ok(())
}

/// Meetings a dead process left mid-flight ("recording"/"summarizing"),
/// with their segment counts — startup reconciliation input.
pub fn list_stuck(conn: &Connection) -> Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, (SELECT COUNT(*) FROM meeting_segments s WHERE s.meeting_id = m.id)
         FROM meetings m WHERE m.status IN ('recording', 'summarizing')",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Stamp an interrupted meeting: keep any real ended_at, set the new status.
pub fn mark_interrupted(conn: &Connection, id: i64, now: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET ended_at = COALESCE(ended_at, ?2), status = ?3 WHERE id = ?1",
        rusqlite::params![id, now, status],
    )?;
    Ok(())
}

pub fn set_notes(conn: &Connection, id: i64, notes: &str) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET raw_notes = ?2 WHERE id = ?1",
        rusqlite::params![id, notes],
    )?;
    Ok(())
}

pub fn set_audio_paths(conn: &Connection, id: i64, me: Option<&str>, them: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET audio_me_path = ?2, audio_them_path = ?3 WHERE id = ?1",
        rusqlite::params![id, me, them],
    )?;
    Ok(())
}

pub fn set_note_id(conn: &Connection, id: i64, note_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET note_id = ?2 WHERE id = ?1",
        rusqlite::params![id, note_id],
    )?;
    Ok(())
}

pub fn insert_segment(
    conn: &Connection,
    meeting_id: i64,
    channel: &str,
    t0_ms: i64,
    t1_ms: i64,
    text: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO meeting_segments (meeting_id, channel, t0_ms, t1_ms, text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![meeting_id, channel, t0_ms, t1_ms, text],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Echo suppression: a mic segment recognized (late) as the speakers' copy of
/// remote speech is removed outright — it was never real "Me" speech.
pub fn delete_segment(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM meeting_segments WHERE id = ?1", [id])?;
    Ok(())
}

/// Stamp diarization labels ("Speaker 1..N") onto them-channel segments.
pub fn set_segment_speakers(conn: &Connection, labels: &[(i64, String)]) -> Result<()> {
    let mut stmt =
        conn.prepare("UPDATE meeting_segments SET speaker = ?2 WHERE id = ?1")?;
    for (id, speaker) in labels {
        stmt.execute(rusqlite::params![id, speaker])?;
    }
    Ok(())
}

/// Saved voiceprints: (name, embedding, samples). Reserved internal profiles
/// (double-underscore names, e.g. the note-taker's own __me__) are excluded —
/// they must never label a "them" speaker.
pub fn speaker_profiles(conn: &Connection) -> Result<Vec<(String, Vec<f32>, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT name, embedding, samples FROM speaker_profiles WHERE substr(name, 1, 2) != '__'",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                super::diarize::blob_to_emb(&r.get::<_, Vec<u8>>(1)?),
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One profile by exact name (reserved names included).
pub fn get_profile(conn: &Connection, name: &str) -> Result<Option<(Vec<f32>, i64)>> {
    Ok(conn
        .query_row(
            "SELECT embedding, samples FROM speaker_profiles WHERE name = ?1",
            [name],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok()
        .map(|(blob, n)| (super::diarize::blob_to_emb(&blob), n)))
}

/// Fold `centroid` (over `n` segments) into the named voiceprint as a
/// samples-weighted running mean; creates the profile if new.
pub fn merge_profile(conn: &Connection, name: &str, centroid: &[f32], n: i64) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let (emb, samples) = match get_profile(conn, name)? {
        Some((old, old_n)) => (
            super::diarize::merge_centroid(&old, old_n, centroid, n),
            old_n + n,
        ),
        None => (centroid.to_vec(), n),
    };
    conn.execute(
        "INSERT OR REPLACE INTO speaker_profiles (name, embedding, samples, updated_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![name, super::diarize::emb_to_blob(&emb), samples, now],
    )?;
    Ok(())
}

/// The calendar-event snapshot captured at start, if this meeting had one.
pub fn meeting_event_json(conn: &Connection, id: i64) -> Result<Option<Value>> {
    let raw: Option<String> = conn.query_row(
        "SELECT event_json FROM meetings WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
}

/// 1:1 meetings: them-segments too short to voice-fingerprint never joined a
/// cluster — but with exactly one external attendee they can only be them.
pub fn label_unlabeled_them(conn: &Connection, meeting_id: i64, name: &str) -> Result<usize> {
    let n = conn.execute(
        "UPDATE meeting_segments SET speaker = ?2
         WHERE meeting_id = ?1 AND channel = 'them' AND speaker IS NULL",
        rusqlite::params![meeting_id, name],
    )?;
    Ok(n)
}

/// Record a meeting's diarized voices (label + centroid) so a later rename can
/// seed a voiceprint. An unlabeled lone voice is stored under "Them".
pub fn save_meeting_speakers(
    conn: &Connection,
    meeting_id: i64,
    speakers: &[(String, Vec<f32>, i64)], // (label, centroid, seg_count)
) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO meeting_speakers (meeting_id, label, centroid, seg_count)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (label, centroid, seg_count) in speakers {
        stmt.execute(rusqlite::params![
            meeting_id,
            label,
            super::diarize::emb_to_blob(centroid),
            seg_count
        ])?;
    }
    Ok(())
}

pub fn list_meeting_speakers(conn: &Connection, meeting_id: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT label, suggested, seg_count FROM meeting_speakers
         WHERE meeting_id = ?1 ORDER BY seg_count DESC",
    )?;
    let rows = stmt
        .query_map([meeting_id], |r| {
            Ok(json!({
                "label": r.get::<_, String>(0)?,
                "suggested": r.get::<_, Option<String>>(1)?,
                "seg_count": r.get::<_, i64>(2)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_speaker_suggestion(
    conn: &Connection,
    meeting_id: i64,
    label: &str,
    suggested: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE meeting_speakers SET suggested = ?3 WHERE meeting_id = ?1 AND label = ?2",
        rusqlite::params![meeting_id, label, suggested],
    )?;
    Ok(())
}

/// Rename a diarized voice: relabels its transcript lines and folds the
/// meeting's cluster centroid into the persistent voiceprint for `to` (running
/// mean), so future meetings auto-label this person. `from` = "Them" renames
/// the lone-voice case (speaker still NULL on the segments).
pub fn rename_speaker(conn: &Connection, meeting_id: i64, from: &str, to: &str) -> Result<()> {
    let to = to.trim();
    if to.is_empty() || to == "Me" || to == "Them" || to.starts_with("__") {
        return Err(anyhow::anyhow!("invalid speaker name"));
    }
    if from == "Them" {
        conn.execute(
            "UPDATE meeting_segments SET speaker = ?2
             WHERE meeting_id = ?1 AND channel = 'them' AND speaker IS NULL",
            rusqlite::params![meeting_id, to],
        )?;
    } else {
        conn.execute(
            "UPDATE meeting_segments SET speaker = ?3
             WHERE meeting_id = ?1 AND speaker = ?2",
            rusqlite::params![meeting_id, from, to],
        )?;
    }
    let row: Option<(Vec<u8>, i64)> = conn
        .query_row(
            "SELECT centroid, seg_count FROM meeting_speakers
             WHERE meeting_id = ?1 AND label = ?2",
            rusqlite::params![meeting_id, from],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((blob, seg_count)) = row else {
        return Ok(()); // pre-voiceprint meeting: relabel only
    };
    conn.execute(
        "UPDATE OR REPLACE meeting_speakers SET label = ?3, suggested = NULL
         WHERE meeting_id = ?1 AND label = ?2",
        rusqlite::params![meeting_id, from, to],
    )?;
    merge_profile(conn, to, &super::diarize::blob_to_emb(&blob), seg_count)
}

/// Full transcript, timeline order, interleaved across channels.
pub fn list_segments(conn: &Connection, meeting_id: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, channel, t0_ms, t1_ms, text, speaker
         FROM meeting_segments WHERE meeting_id = ?1 ORDER BY t0_ms ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([meeting_id], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "channel": r.get::<_, String>(1)?,
                "t0_ms": r.get::<_, i64>(2)?,
                "t1_ms": r.get::<_, i64>(3)?,
                "text": r.get::<_, String>(4)?,
                "speaker": r.get::<_, Option<String>>(5)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Per-channel speaking time in ms — deterministic, no LLM (Read AI's one good
/// metric worth keeping).
pub fn talk_time(conn: &Connection, meeting_id: i64) -> Result<(i64, i64)> {
    let me: i64 = conn.query_row(
        "SELECT COALESCE(SUM(t1_ms - t0_ms), 0) FROM meeting_segments
         WHERE meeting_id = ?1 AND channel = 'me'",
        [meeting_id],
        |r| r.get(0),
    )?;
    let them: i64 = conn.query_row(
        "SELECT COALESCE(SUM(t1_ms - t0_ms), 0) FROM meeting_segments
         WHERE meeting_id = ?1 AND channel = 'them'",
        [meeting_id],
        |r| r.get(0),
    )?;
    Ok((me, them))
}

pub fn insert_summary(
    conn: &Connection,
    meeting_id: i64,
    template: &str,
    content_md: &str,
    now: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO meeting_summaries (meeting_id, template, content_md, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![meeting_id, template, content_md, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_summaries(conn: &Connection, meeting_id: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, template, content_md, created_at FROM meeting_summaries
         WHERE meeting_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map([meeting_id], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "template": r.get::<_, String>(1)?,
                "content_md": r.get::<_, String>(2)?,
                "created_at": r.get::<_, String>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Recent meetings, newest first, with enough for a list row.
pub fn list_meetings(conn: &Connection, limit: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.title, m.started_at, m.ended_at, m.status, m.note_id, m.event_json,
                (SELECT COUNT(*) FROM meeting_segments s WHERE s.meeting_id = m.id),
                (SELECT COUNT(*) FROM meeting_summaries y WHERE y.meeting_id = m.id)
         FROM meetings m ORDER BY m.id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "title": r.get::<_, String>(1)?,
                "started_at": r.get::<_, Option<String>>(2)?,
                "ended_at": r.get::<_, Option<String>>(3)?,
                "status": r.get::<_, String>(4)?,
                "note_id": r.get::<_, Option<i64>>(5)?,
                "event_json": r.get::<_, Option<String>>(6)?
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "segment_count": r.get::<_, i64>(7)?,
                "summary_count": r.get::<_, i64>(8)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Everything the meeting page needs in one call.
pub fn get_meeting(conn: &Connection, id: i64) -> Result<Value> {
    let meta = conn.query_row(
        "SELECT id, title, event_id, event_json, started_at, ended_at, status, raw_notes,
                audio_me_path, audio_them_path, note_id
         FROM meetings WHERE id = ?1",
        [id],
        |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "title": r.get::<_, String>(1)?,
                "event_id": r.get::<_, Option<String>>(2)?,
                "event_json": r.get::<_, Option<String>>(3)?
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "started_at": r.get::<_, Option<String>>(4)?,
                "ended_at": r.get::<_, Option<String>>(5)?,
                "status": r.get::<_, String>(6)?,
                "raw_notes": r.get::<_, String>(7)?,
                "audio_me_path": r.get::<_, Option<String>>(8)?,
                "audio_them_path": r.get::<_, Option<String>>(9)?,
                "note_id": r.get::<_, Option<i64>>(10)?,
            }))
        },
    )?;
    let segments = list_segments(conn, id)?;
    let summaries = list_summaries(conn, id)?;
    let (me_ms, them_ms) = talk_time(conn, id)?;
    let speakers = list_meeting_speakers(conn, id)?;
    let mut out = meta;
    out["segments"] = json!(segments);
    out["summaries"] = json!(summaries);
    out["talk_ms"] = json!({ "me": me_ms, "them": them_ms });
    out["speakers"] = json!(speakers);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Templates: PLAUD's model — a template is a name + ONE free-text prompt that
// describes the sections to produce. Builtins are re-seeded (overwritten) on
// startup so prompt improvements ship; user templates are never touched.
// ---------------------------------------------------------------------------

pub const DEFAULT_TEMPLATE: &str = "Meeting";

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (
        "Meeting",
        "General meeting notes. Sections, in this order: \
         'Summary' — a full paragraph (4-6 sentences): what the meeting was about, \
         who drove it, the main threads, and how each was left. \
         'Discussion' — the heart of the notes and the LONGEST section: detailed \
         bullets covering every topic discussed, in order. Open each topic with a \
         bold lead ('**Pricing** — …'), then one bullet per substantive point: \
         decisions and the reasoning behind them, options considered and rejected, \
         numbers, dates, names, disagreements, and how each thread was left. \
         'Key Takeaways' — the 5-10 points that matter most, each a complete \
         sentence carrying its own specifics. \
         'Chapters' — the conversation's phases as a timeline: each item gets the \
         timestamp where the topic started and a 1-2 line gist. \
         'Action Items' — every task, commitment, deadline, or follow-up as \
         'Owner — verb phrase by date' (use Me/Them or a stated name as owner). \
         'Key Questions' — questions raised that were NOT resolved in the meeting.",
    ),
    (
        "1:1",
        "One-on-one meeting notes. Sections, in this order: \
         'Summary' — a full paragraph capturing the tone and main threads. \
         'Updates' — progress and wins each person shared, as detailed bullets that \
         keep the specifics (project names, numbers, dates, who was involved). \
         'Blockers & Concerns' — every problem raised, the context behind it, and \
         how it landed. \
         'Feedback' — feedback exchanged in either direction, quoted where sharp. \
         'Discussion' — anything substantive outside the above, topic by topic, \
         with the reasoning and details preserved. \
         'Action Items' — commitments as 'Owner — verb phrase by date'. \
         'Key Questions' — open questions to revisit next time.",
    ),
    (
        "Standup",
        "Daily standup notes. Sections, in this order: \
         'Summary' — two sentences max. \
         'Progress' — what was done, per person where stated. \
         'Next' — what each person is doing next. \
         'Blockers' — anything blocking, and who owns unblocking it. \
         'Action Items' — as 'Owner — verb phrase by date'.",
    ),
    (
        "Interview",
        "Interview notes (candidate, user research, or journalistic). Sections: \
         'Summary' — a full paragraph: who was interviewed, the ground covered, \
         and the overall read. \
         'Background' — the experience and context the interviewee gave, as \
         detailed bullets (roles, companies, dates, scope). \
         'Highlights' — the strongest answers or moments: what was asked, how they \
         answered, and why it landed, with short quotes. \
         'Concerns' — weak answers, risks, or open doubts, each with the moment \
         that raised it. \
         'Chapters' — question areas as a timeline with timestamps. \
         'Action Items' — follow-ups as 'Owner — verb phrase by date'.",
    ),
    (
        "Lecture",
        "Lecture or talk notes. Sections, in this order: \
         'Summary' — a full paragraph: the thesis of the talk and the arc of its \
         argument. \
         'Key Concepts' — each concept as a bullet with the explanation actually \
         given: definitions, numbers, and the examples used to make the point. \
         'Chapters' — the talk's arc as a timeline with timestamps. \
         'Examples & References' — concrete examples, papers, books, or tools \
         mentioned, each with why it came up. \
         'Key Questions' — audience questions and any left unanswered.",
    ),
];

pub fn seed_templates(conn: &Connection) -> Result<()> {
    for (name, prompt) in BUILTIN_TEMPLATES {
        conn.execute(
            "INSERT INTO meeting_templates (name, prompt, builtin) VALUES (?1, ?2, 1)
             ON CONFLICT(name) DO UPDATE SET prompt = excluded.prompt, builtin = 1",
            rusqlite::params![name, prompt],
        )?;
    }
    Ok(())
}

pub fn list_templates(conn: &Connection) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT name, prompt, builtin FROM meeting_templates ORDER BY builtin DESC, name ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "name": r.get::<_, String>(0)?,
                "prompt": r.get::<_, String>(1)?,
                "builtin": r.get::<_, i64>(2)? == 1,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_template(conn: &Connection, name: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT prompt FROM meeting_templates WHERE name = ?1",
            [name],
            |r| r.get(0),
        )
        .ok())
}

pub fn save_template(conn: &Connection, name: &str, prompt: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meeting_templates (name, prompt, builtin) VALUES (?1, ?2, 0)
         ON CONFLICT(name) DO UPDATE SET prompt = excluded.prompt",
        rusqlite::params![name, prompt],
    )?;
    Ok(())
}

/// Builtins can't be deleted (they'd re-seed on launch anyway).
pub fn delete_template(conn: &Connection, name: &str) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM meeting_templates WHERE name = ?1 AND builtin = 0",
        [name],
    )?;
    Ok(n > 0)
}
