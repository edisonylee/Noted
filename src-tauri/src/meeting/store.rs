// DB access for the meeting recorder. Transcript segments live in their own
// table (not entries.data_json) because they're large and append-heavy while
// recording. The AI summary is additionally filed as a regular note (see
// summarize.rs) so search/embeddings/KG stay unchanged.

use anyhow::{anyhow,Result};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
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

/// Remove a row that never became a recording. This is only used while
/// `meeting::start` still owns the unpublished row, before capture threads,
/// segments, summaries, or UI state can exist.
pub fn delete_meeting(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM meetings WHERE id = ?1", [id])?;
    Ok(())
}

pub fn trash_meeting(conn: &Connection, id: i64, now: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE meetings SET trashed_at = ?2
         WHERE id = ?1 AND trashed_at IS NULL
           AND status NOT IN ('recording', 'summarizing')",
        rusqlite::params![id, now],
    )?;
    Ok(changed > 0)
}

pub fn restore_meeting(conn: &Connection, id: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE meetings SET trashed_at = NULL WHERE id = ?1 AND trashed_at IS NOT NULL",
        [id],
    )?;
    Ok(changed > 0)
}

/// Permanently remove a trashed meeting and its generated note. The caller
/// deletes retained media after this transaction commits.
pub fn delete_meeting_forever(conn: &mut Connection, id: i64) -> Result<bool> {
    let tx = conn.transaction()?;
    let note_id = tx
        .query_row(
            "SELECT note_id FROM meetings WHERE id = ?1 AND trashed_at IS NOT NULL",
            [id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?1 AND trashed_at IS NOT NULL)",
        [id],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(false);
    }

    tx.execute("DELETE FROM meeting_summaries WHERE meeting_id = ?1", [id])?;
    tx.execute("DELETE FROM meeting_segments WHERE meeting_id = ?1", [id])?;
    tx.execute("DELETE FROM meeting_speakers WHERE meeting_id = ?1", [id])?;
    tx.execute("DELETE FROM meetings WHERE id = ?1", [id])?;

    if let Some(note_id) = note_id {
        tx.execute(
            "UPDATE entities SET home_note_id = NULL WHERE home_note_id = ?1",
            [note_id],
        )?;
        tx.execute("DELETE FROM entity_mentions WHERE note_id = ?1", [note_id])?;
        tx.execute("DELETE FROM embeddings WHERE note_id = ?1", [note_id])?;
        tx.execute("DELETE FROM entries WHERE note_id = ?1", [note_id])?;
        tx.execute("DELETE FROM notes WHERE id = ?1", [note_id])?;
        tx.execute(
            "UPDATE categories SET entry_count =
               (SELECT COUNT(*) FROM entries e WHERE e.category_id = categories.id)",
            [],
        )?;
        tx.execute(
            "UPDATE entities SET
               mention_count = (SELECT COUNT(*) FROM entity_mentions m WHERE m.entity_id = entities.id),
               first_seen = (SELECT MIN(event_date) FROM entity_mentions m WHERE m.entity_id = entities.id),
               last_seen = (SELECT MAX(event_date) FROM entity_mentions m WHERE m.entity_id = entities.id)",
            [],
        )?;
    }
    tx.commit()?;
    Ok(true)
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

/// Rename the meeting and its linked library note. The generated body remains
/// untouched: a custom title is user-owned metadata, not a rewrite of history.
pub fn set_title(conn: &Connection, id: i64, title: &str) -> Result<Option<i64>> {
    let title = title.trim();
    if title.is_empty() {
        return Err(anyhow!("meeting title cannot be empty"));
    }
    let changed = conn.execute(
        "UPDATE meetings SET title = ?2 WHERE id = ?1",
        rusqlite::params![id, title],
    )?;
    if changed == 0 {
        return Err(anyhow!("meeting not found"));
    }
    let note_id = conn.query_row("SELECT note_id FROM meetings WHERE id = ?1", [id], |r| {
        r.get::<_, Option<i64>>(0)
    })?;
    if let Some(note_id) = note_id {
        conn.execute(
            "UPDATE notes SET title = ?2 WHERE id = ?1",
            rusqlite::params![note_id, title],
        )?;
        conn.execute(
            "UPDATE entries SET data_json = json_set(data_json, '$.title', ?2)
             WHERE note_id = ?1 AND json_valid(data_json)",
            rusqlite::params![note_id, title],
        )?;
        conn.execute("DELETE FROM embeddings WHERE note_id = ?1", [note_id])?;
    }
    Ok(note_id)
}

pub fn set_audio_paths(
    conn: &Connection,
    id: i64,
    me: Option<&str>,
    them: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET audio_me_path = ?2, audio_them_path = ?3 WHERE id = ?1",
        rusqlite::params![id, me, them],
    )?;
    Ok(())
}

/// Window-video mp4 path for a meeting; None clears it (file deleted).
pub fn set_video_path(conn: &Connection, id: i64, path: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET video_path = ?2 WHERE id = ?1",
        rusqlite::params![id, path],
    )?;
    Ok(())
}

/// (id, video_path) of every meeting still holding a window video — feeds
/// the launch-time retention sweep.
pub fn meetings_with_video(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, video_path FROM meetings WHERE video_path IS NOT NULL")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
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
/// Reset every them-segment's speaker — the stop pass calls this before
/// writing final labels so no provisional live label survives it.
pub fn clear_them_speakers(conn: &Connection, meeting_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE meeting_segments SET speaker = NULL
         WHERE meeting_id = ?1 AND channel = 'them'",
        [meeting_id],
    )?;
    Ok(())
}

/// Remove the saved cluster rows before rebuilding diarization. Without this,
/// a label that disappears on a later pass can remain visible in the UI.
pub fn clear_meeting_speakers(conn: &Connection, meeting_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM meeting_speakers WHERE meeting_id = ?1",
        [meeting_id],
    )?;
    Ok(())
}

/// (id, t0_ms, t1_ms) of a meeting's them-segments in timeline order — feeds
/// recovery diarization from the retained WAV.
pub fn them_segment_times(conn: &Connection, meeting_id: i64) -> Result<Vec<(i64, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, t0_ms, t1_ms FROM meeting_segments
         WHERE meeting_id = ?1 AND channel = 'them' ORDER BY t0_ms",
    )?;
    let rows = stmt
        .query_map([meeting_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_segment_speakers(conn: &Connection, labels: &[(i64, String)]) -> Result<()> {
    let mut stmt = conn.prepare("UPDATE meeting_segments SET speaker = ?2 WHERE id = ?1")?;
    for (id, speaker) in labels {
        stmt.execute(rusqlite::params![id, speaker])?;
    }
    Ok(())
}

/// Record a meeting's anonymous diarized voices (label + centroid). An
/// unlabeled lone voice is stored under "Them" for manual renaming.
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

/// Rename a diarized voice within one meeting. `from` = "Them" renames the
/// lone-voice case (speaker still NULL on the segments).
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
    // Renaming onto a label that already exists in this meeting ("Speaker 2"
    // recognized as an already-named voice) must MERGE the two rows. UPDATE
    // OR REPLACE would resolve the key conflict by silently DELETING the
    // existing row — destroying the named voice's centroid (learned the hard
    // way when a mis-confirmed suggestion vaporized a meeting's real cluster).
    let existing: Option<(Vec<u8>, i64)> = conn
        .query_row(
            "SELECT centroid, seg_count FROM meeting_speakers
             WHERE meeting_id = ?1 AND label = ?2",
            rusqlite::params![meeting_id, to],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((to_blob, to_n)) = existing {
        let merged = super::diarize::merge_centroid(
            &super::diarize::blob_to_emb(&to_blob),
            to_n,
            &super::diarize::blob_to_emb(&blob),
            seg_count,
        );
        conn.execute(
            "UPDATE meeting_speakers SET centroid = ?3, seg_count = ?4, suggested = NULL
             WHERE meeting_id = ?1 AND label = ?2",
            rusqlite::params![
                meeting_id,
                to,
                super::diarize::emb_to_blob(&merged),
                to_n + seg_count
            ],
        )?;
        conn.execute(
            "DELETE FROM meeting_speakers WHERE meeting_id = ?1 AND label = ?2",
            rusqlite::params![meeting_id, from],
        )?;
    } else {
        conn.execute(
            "UPDATE meeting_speakers SET label = ?3, suggested = NULL
             WHERE meeting_id = ?1 AND label = ?2",
            rusqlite::params![meeting_id, from, to],
        )?;
    }
    Ok(())
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

#[derive(Debug, Serialize)]
pub struct TranscriptSearchHit {
    pub segment_id: i64,
    pub meeting_id: i64,
    pub meeting_title: String,
    pub started_at: Option<String>,
    pub t0_ms: i64,
    pub speaker: String,
    pub text: String,
}

fn transcript_fts_query(query: &str) -> String {
    query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{term}\"*"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Prefix-search every visible meeting transcript using the content-linked
/// FTS index. One hit represents one spoken segment, so repeated mentions in
/// separate transcript lines remain separate results.
pub fn search_transcripts(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<TranscriptSearchHit>> {
    let fts_query = transcript_fts_query(query.trim());
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 200);
    let mut stmt = conn.prepare(
        "SELECT s.id, m.id, m.title, m.started_at, s.t0_ms,
                CASE
                  WHEN s.channel = 'me' THEN 'Me'
                  ELSE COALESCE(NULLIF(s.speaker, ''), 'Them')
                END AS speaker,
                s.text
         FROM meeting_segments_fts
         JOIN meeting_segments s ON s.id = meeting_segments_fts.rowid
         JOIN meetings m ON m.id = s.meeting_id
         WHERE meeting_segments_fts MATCH ?1
           AND m.trashed_at IS NULL
         ORDER BY COALESCE(m.started_at, m.created_at) DESC, s.t0_ms ASC, s.id ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![fts_query, limit], |row| {
            Ok(TranscriptSearchHit {
                segment_id: row.get(0)?,
                meeting_id: row.get(1)?,
                meeting_title: row.get(2)?,
                started_at: row.get(3)?,
                t0_ms: row.get(4)?,
                speaker: row.get(5)?,
                text: row.get(6)?,
            })
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
    let existing = conn
        .query_row(
            "SELECT id FROM meeting_summaries WHERE meeting_id = ?1 AND template = ?2
             ORDER BY id ASC LIMIT 1",
            rusqlite::params![meeting_id, template],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(id) = existing {
        conn.execute(
            "UPDATE meeting_summaries SET content_md = ?2, created_at = ?3 WHERE id = ?1",
            rusqlite::params![id, content_md, now],
        )?;
        conn.execute(
            "DELETE FROM meeting_summaries
             WHERE meeting_id = ?1 AND template = ?2 AND id <> ?3",
            rusqlite::params![meeting_id, template, id],
        )?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO meeting_summaries (meeting_id, template, content_md, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![meeting_id, template, content_md, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Save an edited generated summary. Editing the primary summary also refreshes
/// the linked searchable note while preserving the user's verbatim meeting notes.
pub fn set_summary_content(
    conn: &Connection,
    meeting_id: i64,
    summary_id: i64,
    content_md: &str,
    primary_template: &str,
) -> Result<Option<(i64, String)>> {
    let summary = conn
        .query_row(
            "SELECT s.template, m.title, m.raw_notes, m.note_id
             FROM meeting_summaries s
             JOIN meetings m ON m.id = s.meeting_id
             WHERE s.id = ?1 AND s.meeting_id = ?2",
            rusqlite::params![summary_id, meeting_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((template, title, raw_notes, note_id)) = summary else {
        return Err(anyhow!("meeting summary not found"));
    };
    conn.execute(
        "UPDATE meeting_summaries SET content_md = ?2 WHERE id = ?1",
        rusqlite::params![summary_id, content_md],
    )?;

    if template != primary_template {
        return Ok(None);
    }
    let Some(note_id) = note_id else {
        return Ok(None);
    };
    let mut note_text = format!("# {title}\n\n{content_md}");
    if !raw_notes.trim().is_empty() {
        note_text.push_str(&format!(
            "\n\n## Your Notes (verbatim)\n\n{}",
            raw_notes.trim()
        ));
    }
    crate::db::refresh_note_text(conn, note_id, &note_text)?;
    Ok(Some((note_id, note_text)))
}

pub fn find_meeting_by_event(conn: &Connection, event_id: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM meetings
             WHERE event_id = ?1 AND status <> 'failed' AND trashed_at IS NULL
             ORDER BY id DESC LIMIT 1",
            [event_id],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn list_summaries(conn: &Connection, meeting_id: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, template, content_md, created_at FROM meeting_summaries
         WHERE meeting_id = ?1
           AND id = (SELECT MAX(newer.id) FROM meeting_summaries newer
                     WHERE newer.meeting_id = meeting_summaries.meeting_id
                       AND newer.template = meeting_summaries.template)
         ORDER BY id ASC",
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
    list_meetings_by_trash(conn, limit, false)
}

pub fn list_trashed_meetings(conn: &Connection, limit: i64) -> Result<Vec<Value>> {
    list_meetings_by_trash(conn, limit, true)
}

fn list_meetings_by_trash(conn: &Connection, limit: i64, trashed: bool) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.title, m.started_at, m.ended_at, m.status, m.note_id, m.event_json,
                (SELECT COUNT(*) FROM meeting_segments s WHERE s.meeting_id = m.id),
                (SELECT COUNT(DISTINCT y.template) FROM meeting_summaries y WHERE y.meeting_id = m.id),
                m.trashed_at
         FROM meetings m
         WHERE (?2 = 1 AND m.trashed_at IS NOT NULL)
            OR (?2 = 0 AND m.trashed_at IS NULL)
         ORDER BY m.id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![limit, trashed], |r| {
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
                "trashed_at": r.get::<_, Option<String>>(9)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Everything the meeting page needs in one call.
pub fn get_meeting(conn: &Connection, id: i64) -> Result<Value> {
    let meta = conn.query_row(
        "SELECT id, title, event_id, event_json, started_at, ended_at, status, raw_notes,
                audio_me_path, audio_them_path, note_id, video_path, trashed_at
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
                "video_path": r.get::<_, Option<String>>(11)?,
                "trashed_at": r.get::<_, Option<String>>(12)?,
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
