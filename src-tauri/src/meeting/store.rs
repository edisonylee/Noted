// DB access for the meeting recorder. Transcript segments live in their own
// table (not entries.data_json) because they're large and append-heavy while
// recording. The AI summary is additionally filed as a regular note (see
// summarize.rs) so search/embeddings/KG stay unchanged.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{anyhow, Result};
use rusqlite::{types::Value as SqlValue, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub fn create_meeting(
    conn: &Connection,
    title: &str,
    event_id: Option<&str>,
    event_json: Option<&str>,
    now: &str,
) -> Result<i64> {
    create_meeting_row(conn, title, event_id, event_json, None, None, now)
}

pub fn create_meeting_with_asr(
    conn: &Connection,
    title: &str,
    event_id: Option<&str>,
    event_json: Option<&str>,
    asr_engine: &str,
    asr_model: &str,
    now: &str,
) -> Result<i64> {
    create_meeting_row(
        conn,
        title,
        event_id,
        event_json,
        Some(asr_engine),
        Some(asr_model),
        now,
    )
}

fn create_meeting_row(
    conn: &Connection,
    title: &str,
    event_id: Option<&str>,
    event_json: Option<&str>,
    asr_engine: Option<&str>,
    asr_model: Option<&str>,
    now: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO meetings
            (title, event_id, event_json, started_at, status, asr_engine, asr_model, created_at)
         VALUES (?1, ?2, ?3, ?4, 'recording', ?5, ?6, ?4)",
        rusqlite::params![title, event_id, event_json, now, asr_engine, asr_model],
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
    // Vocabulary is a deterministic post-ASR layer. Capture must never be
    // dropped because a preference lookup failed, so fall back to the engine's
    // original text while retaining a useful diagnostic.
    let normalized = normalize_transcript_text(conn, text).unwrap_or_else(|error| {
        eprintln!("[noted] transcript vocabulary lookup failed: {error}");
        text.to_string()
    });
    conn.execute(
        "INSERT INTO meeting_segments (meeting_id, channel, t0_ms, t1_ms, text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![meeting_id, channel, t0_ms, t1_ms, normalized],
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
             WHERE meeting_id = ?1 AND channel = 'them'
               AND (speaker IS NULL OR speaker = '' OR speaker = 'Them')",
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

fn external_attendees_from_raw(event_json: Option<&str>) -> Vec<String> {
    let Some(raw) = event_json else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    super::summarize::external_attendees(&value)
        .into_iter()
        .filter(|name| seen.insert(name.to_lowercase()))
        .collect()
}

fn anonymous_speaker(label: &str) -> bool {
    let trimmed = label.trim();
    trimmed.is_empty()
        || trimmed == "Them"
        || trimmed
            .strip_prefix("Speaker ")
            .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
}

/// Calendar ground truth for an actual one-on-one: every remote line belongs
/// to the sole external attendee. This is intentionally stronger than voice
/// clustering, which can split one person into several anonymous clusters.
pub fn set_one_on_one_speaker(conn: &Connection, meeting_id: i64, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("one-on-one attendee name cannot be empty"));
    }
    conn.execute(
        "UPDATE meeting_segments SET speaker = ?2
         WHERE meeting_id = ?1 AND channel = 'them'",
        rusqlite::params![meeting_id, name],
    )?;
    Ok(())
}

/// One-time repair for saved one-on-ones created before calendar-aware speaker
/// naming. Only anonymous labels are rewritten; a name the user entered by hand
/// remains authoritative.
pub fn initialize_one_on_one_speakers(conn: &Connection) -> Result<()> {
    let initialized: Option<String> = conn
        .query_row(
            "SELECT value FROM app_metadata WHERE key = 'meeting_one_on_one_speakers_v1'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if initialized.is_some() {
        return Ok(());
    }

    let meetings = {
        let mut stmt = conn.prepare("SELECT id, event_json FROM meetings ORDER BY id")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    for (meeting_id, event_json) in meetings {
        let attendees = external_attendees_from_raw(event_json.as_deref());
        if attendees.len() != 1 {
            continue;
        }
        let attendee = &attendees[0];
        let mut labels = BTreeSet::new();
        {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT COALESCE(NULLIF(speaker, ''), 'Them')
                 FROM meeting_segments
                 WHERE meeting_id = ?1 AND channel = 'them'",
            )?;
            labels.extend(
                stmt.query_map([meeting_id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
        {
            let mut stmt =
                conn.prepare("SELECT label FROM meeting_speakers WHERE meeting_id = ?1")?;
            labels.extend(
                stmt.query_map([meeting_id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
        for label in labels.into_iter().filter(|label| anonymous_speaker(label)) {
            rename_speaker(conn, meeting_id, &label, attendee)?;
        }
    }

    conn.execute(
        "INSERT INTO app_metadata (key, value)
         VALUES ('meeting_one_on_one_speakers_v1', '1')",
        [],
    )?;
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

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TranscriptSearchFilters {
    pub people: Vec<String>,
    pub folder_ids: Vec<i64>,
    pub meeting_types: Vec<String>,
}

impl TranscriptSearchFilters {
    fn active(&self) -> bool {
        !self.people.is_empty() || !self.folder_ids.is_empty() || !self.meeting_types.is_empty()
    }
}

#[derive(Debug, Serialize)]
pub struct TranscriptFacetValue {
    pub value: String,
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct TranscriptSearchFacets {
    pub people: Vec<TranscriptFacetValue>,
    pub folders: Vec<TranscriptFacetValue>,
    pub meeting_types: Vec<TranscriptFacetValue>,
}

struct MeetingSearchCandidate {
    id: i64,
    title: String,
    note_id: Option<i64>,
    people: Vec<String>,
    external_attendee_count: usize,
}

#[derive(Clone)]
struct FolderSearchNode {
    id: i64,
    parent_id: Option<i64>,
    name: String,
    kind: String,
    auto_rule: String,
    direct_note_ids: HashSet<i64>,
}

struct FolderSearchData {
    id: i64,
    label: String,
    note_ids: HashSet<i64>,
}

fn named_person(label: &str) -> bool {
    let trimmed = label.trim();
    !trimmed.is_empty() && trimmed != "Me" && !anonymous_speaker(trimmed)
}

fn meeting_search_candidates(conn: &Connection) -> Result<Vec<MeetingSearchCandidate>> {
    let mut spoken_names: HashMap<i64, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT s.meeting_id, s.speaker
             FROM meeting_segments s
             JOIN meetings m ON m.id = s.meeting_id
             WHERE m.trashed_at IS NULL AND s.channel = 'them'
               AND s.speaker IS NOT NULL AND s.speaker <> ''",
        )?;
        for row in stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (meeting_id, name) = row?;
            if named_person(&name) {
                spoken_names.entry(meeting_id).or_default().push(name);
            }
        }
    }

    let mut stmt = conn.prepare(
        "SELECT id, title, event_json, note_id
         FROM meetings WHERE trashed_at IS NULL ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut candidates = Vec::new();
    for row in rows {
        let (id, title, event_json, note_id) = row?;
        let attendees = external_attendees_from_raw(event_json.as_deref());
        let mut people = attendees.clone();
        people.extend(spoken_names.remove(&id).unwrap_or_default());
        let mut seen = HashSet::new();
        people.retain(|name| named_person(name) && seen.insert(name.to_lowercase()));
        candidates.push(MeetingSearchCandidate {
            id,
            title,
            note_id,
            people,
            external_attendee_count: attendees.len(),
        });
    }
    Ok(candidates)
}

fn collect_folder_notes(
    id: i64,
    nodes: &HashMap<i64, FolderSearchNode>,
    children: &HashMap<i64, Vec<i64>>,
    memo: &mut HashMap<i64, HashSet<i64>>,
    path: &mut HashSet<i64>,
) -> HashSet<i64> {
    if let Some(cached) = memo.get(&id) {
        return cached.clone();
    }
    if !path.insert(id) {
        return HashSet::new();
    }
    let mut notes = nodes
        .get(&id)
        .map(|node| node.direct_note_ids.clone())
        .unwrap_or_default();
    for child in children.get(&id).into_iter().flatten() {
        notes.extend(collect_folder_notes(*child, nodes, children, memo, path));
    }
    path.remove(&id);
    memo.insert(id, notes.clone());
    notes
}

fn folder_search_data(conn: &Connection) -> Result<(Vec<FolderSearchData>, HashSet<i64>)> {
    let folders = crate::db::list_note_folders(conn)?;
    let nodes: HashMap<i64, FolderSearchNode> = folders
        .into_iter()
        .map(|folder| {
            (
                folder.id,
                FolderSearchNode {
                    id: folder.id,
                    parent_id: folder.parent_id,
                    name: folder.name,
                    kind: folder.kind,
                    auto_rule: folder.auto_rule,
                    direct_note_ids: folder.note_ids.into_iter().collect(),
                },
            )
        })
        .collect();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for node in nodes.values() {
        if let Some(parent_id) = node.parent_id {
            children.entry(parent_id).or_default().push(node.id);
        }
    }
    let mut memo = HashMap::new();
    let mut data = Vec::new();
    let mut standup_notes = HashSet::new();
    let mut ids = nodes.keys().copied().collect::<Vec<_>>();
    ids.sort_unstable();
    for id in ids {
        let Some(node) = nodes.get(&id) else {
            continue;
        };
        if node.auto_rule == "daily_standup" {
            standup_notes.extend(node.direct_note_ids.iter().copied());
        }
        if node.kind != "folder" {
            continue;
        }
        let note_ids = collect_folder_notes(id, &nodes, &children, &mut memo, &mut HashSet::new());
        let mut names = Vec::new();
        let mut current = Some(id);
        let mut seen = HashSet::new();
        while let Some(current_id) = current {
            if !seen.insert(current_id) {
                break;
            }
            let Some(current_node) = nodes.get(&current_id) else {
                break;
            };
            if current_node.kind != "space" {
                names.push(current_node.name.clone());
            }
            current = current_node.parent_id;
        }
        names.reverse();
        data.push(FolderSearchData {
            id,
            label: names.join(" / "),
            note_ids,
        });
    }
    Ok((data, standup_notes))
}

fn title_is_standup(title: &str) -> bool {
    let lower = title.to_lowercase();
    let spaced = lower.replace(['-', '_'], " ");
    lower.contains("standup")
        || lower.contains("stand-up")
        || spaced.contains("stand up")
        || spaced.contains("daily scrum")
}

fn meeting_type<'a>(
    meeting: &MeetingSearchCandidate,
    standup_notes: &HashSet<i64>,
) -> (&'a str, &'a str) {
    if title_is_standup(&meeting.title)
        || meeting
            .note_id
            .is_some_and(|id| standup_notes.contains(&id))
    {
        ("daily_standup", "Daily stand-up")
    } else if meeting.external_attendee_count == 1 {
        ("one_on_one", "One-on-one")
    } else if meeting.external_attendee_count > 1 {
        ("group", "Group meeting")
    } else {
        ("other", "Other meeting")
    }
}

pub fn transcript_search_facets(conn: &Connection) -> Result<TranscriptSearchFacets> {
    let candidates = meeting_search_candidates(conn)?;
    let (folders, standup_notes) = folder_search_data(conn)?;

    let mut people_map: BTreeMap<String, (String, HashSet<i64>)> = BTreeMap::new();
    for meeting in &candidates {
        for person in &meeting.people {
            let entry = people_map
                .entry(person.to_lowercase())
                .or_insert_with(|| (person.clone(), HashSet::new()));
            entry.1.insert(meeting.id);
        }
    }
    let mut people = people_map
        .into_values()
        .map(|(label, meetings)| TranscriptFacetValue {
            value: label.clone(),
            label,
            count: meetings.len() as i64,
        })
        .collect::<Vec<_>>();
    people.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));

    let mut folder_values = folders
        .iter()
        .filter_map(|folder| {
            let count = candidates
                .iter()
                .filter(|meeting| {
                    meeting
                        .note_id
                        .is_some_and(|id| folder.note_ids.contains(&id))
                })
                .count() as i64;
            (count > 0).then(|| TranscriptFacetValue {
                value: folder.id.to_string(),
                label: folder.label.clone(),
                count,
            })
        })
        .collect::<Vec<_>>();
    folder_values.sort_by(|a, b| a.label.cmp(&b.label));

    let mut type_counts: BTreeMap<&str, (&str, i64)> = BTreeMap::new();
    for meeting in &candidates {
        let (value, label) = meeting_type(meeting, &standup_notes);
        type_counts
            .entry(value)
            .and_modify(|entry| entry.1 += 1)
            .or_insert((label, 1));
    }
    let type_order = ["daily_standup", "one_on_one", "group", "other"];
    let meeting_types = type_order
        .into_iter()
        .filter_map(|value| {
            type_counts
                .get(value)
                .map(|(label, count)| TranscriptFacetValue {
                    value: value.to_string(),
                    label: (*label).to_string(),
                    count: *count,
                })
        })
        .collect();

    Ok(TranscriptSearchFacets {
        people,
        folders: folder_values,
        meeting_types,
    })
}

fn filtered_meeting_ids(
    conn: &Connection,
    filters: &TranscriptSearchFilters,
) -> Result<Option<HashSet<i64>>> {
    if !filters.active() {
        return Ok(None);
    }
    let candidates = meeting_search_candidates(conn)?;
    let (folders, standup_notes) = folder_search_data(conn)?;
    let selected_people = filters
        .people
        .iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();
    let selected_folders = filters.folder_ids.iter().copied().collect::<HashSet<_>>();
    let selected_types = filters
        .meeting_types
        .iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();

    let folder_by_id = folders
        .iter()
        .map(|folder| (folder.id, &folder.note_ids))
        .collect::<HashMap<_, _>>();
    let mut eligible = HashSet::new();
    for meeting in candidates {
        let person_matches = selected_people.is_empty()
            || meeting
                .people
                .iter()
                .any(|person| selected_people.contains(&person.to_lowercase()));
        let folder_matches = selected_folders.is_empty()
            || meeting.note_id.is_some_and(|note_id| {
                selected_folders.iter().any(|folder_id| {
                    folder_by_id
                        .get(folder_id)
                        .is_some_and(|note_ids| note_ids.contains(&note_id))
                })
            });
        let meeting_type_matches = selected_types.is_empty()
            || selected_types.contains(meeting_type(&meeting, &standup_notes).0);
        if person_matches && folder_matches && meeting_type_matches {
            eligible.insert(meeting.id);
        }
    }
    Ok(Some(eligible))
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
    search_transcripts_filtered(conn, query, limit, &TranscriptSearchFilters::default())
}

pub fn search_transcripts_filtered(
    conn: &Connection,
    query: &str,
    limit: i64,
    filters: &TranscriptSearchFilters,
) -> Result<Vec<TranscriptSearchHit>> {
    search_transcripts_filtered_sorted(conn, query, limit, filters, "date_desc")
}

pub fn search_transcripts_filtered_sorted(
    conn: &Connection,
    query: &str,
    limit: i64,
    filters: &TranscriptSearchFilters,
    sort: &str,
) -> Result<Vec<TranscriptSearchHit>> {
    let fts_query = transcript_fts_query(query.trim());
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 200);
    let eligible = filtered_meeting_ids(conn, filters)?;
    if eligible.as_ref().is_some_and(HashSet::is_empty) {
        return Ok(Vec::new());
    }
    let mut sql = String::from(
        "SELECT s.id, m.id, m.title, m.started_at, s.t0_ms,
                CASE
                  WHEN s.channel = 'me' THEN 'Me'
                  ELSE COALESCE(NULLIF(s.speaker, ''), 'Them')
                END AS speaker,
                s.text
         FROM meeting_segments_fts
         JOIN meeting_segments s ON s.id = meeting_segments_fts.rowid
         JOIN meetings m ON m.id = s.meeting_id
         WHERE meeting_segments_fts MATCH ? AND m.trashed_at IS NULL",
    );
    let mut params = vec![SqlValue::Text(fts_query)];
    if let Some(ids) = eligible {
        let mut ids = ids.into_iter().collect::<Vec<_>>();
        ids.sort_unstable();
        sql.push_str(" AND m.id IN (");
        sql.push_str(&vec!["?"; ids.len()].join(","));
        sql.push(')');
        params.extend(ids.into_iter().map(SqlValue::Integer));
    }
    sql.push_str(match sort {
        "date_asc" => " ORDER BY COALESCE(m.started_at, m.created_at) ASC, s.t0_ms ASC, s.id ASC",
        "title_asc" => {
            " ORDER BY m.title COLLATE NOCASE ASC, COALESCE(m.started_at, m.created_at) DESC,
                       s.t0_ms ASC, s.id ASC"
        }
        "title_desc" => {
            " ORDER BY m.title COLLATE NOCASE DESC, COALESCE(m.started_at, m.created_at) DESC,
                       s.t0_ms ASC, s.id ASC"
        }
        _ => " ORDER BY COALESCE(m.started_at, m.created_at) DESC, s.t0_ms ASC, s.id ASC",
    });
    sql.push_str(" LIMIT ?");
    params.push(SqlValue::Integer(limit));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
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

#[derive(Debug, Serialize)]
pub struct TranscriptVocabularyRule {
    pub id: i64,
    pub heard: String,
    pub preferred: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_batch_id: Option<i64>,
    pub last_changed_segments: Option<i64>,
    pub last_applied_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TranscriptVocabularyPreview {
    pub matching_segments: i64,
    pub occurrences: i64,
}

#[derive(Debug, Serialize)]
pub struct TranscriptVocabularyApplyResult {
    pub rule: TranscriptVocabularyRule,
    pub batch_id: Option<i64>,
    pub changed_segments: i64,
    pub changed_occurrences: i64,
}

#[derive(Debug, Serialize)]
pub struct TranscriptVocabularyUndoResult {
    pub restored_segments: i64,
    pub skipped_segments: i64,
}

fn term_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn replace_whole_term(text: &str, heard: &str, preferred: &str) -> Result<(String, usize)> {
    let heard = heard.trim();
    if heard.is_empty() {
        return Ok((text.to_string(), 0));
    }
    let matcher = regex::RegexBuilder::new(&regex::escape(heard))
        .case_insensitive(true)
        .unicode(true)
        .build()?;
    let require_left_boundary = heard.chars().next().is_some_and(term_character);
    let require_right_boundary = heard.chars().next_back().is_some_and(term_character);
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut replacements = 0;
    for matched in matcher.find_iter(text) {
        let left_ok = !require_left_boundary
            || matched.start() == 0
            || text[..matched.start()]
                .chars()
                .next_back()
                .is_none_or(|character| !term_character(character));
        let right_ok = !require_right_boundary
            || matched.end() == text.len()
            || text[matched.end()..]
                .chars()
                .next()
                .is_none_or(|character| !term_character(character));
        if !left_ok || !right_ok {
            continue;
        }
        output.push_str(&text[cursor..matched.start()]);
        output.push_str(preferred);
        cursor = matched.end();
        replacements += 1;
    }
    if replacements == 0 {
        return Ok((text.to_string(), 0));
    }
    output.push_str(&text[cursor..]);
    Ok((output, replacements))
}

fn validate_vocabulary_terms(heard: &str, preferred: &str) -> Result<(String, String)> {
    let heard = heard.trim();
    let preferred = preferred.trim();
    if heard.is_empty() || preferred.is_empty() {
        return Err(anyhow!("both transcript spellings are required"));
    }
    if heard == preferred {
        return Err(anyhow!("the preferred spelling is already identical"));
    }
    if heard.chars().count() > 120 || preferred.chars().count() > 120 {
        return Err(anyhow!(
            "transcript vocabulary entries must be 120 characters or fewer"
        ));
    }
    Ok((heard.to_string(), preferred.to_string()))
}

pub fn normalize_transcript_text(conn: &Connection, text: &str) -> Result<String> {
    let rules = {
        let mut stmt = conn.prepare(
            "SELECT heard, preferred FROM transcript_vocabulary
             WHERE enabled = 1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut normalized = text.to_string();
    for (heard, preferred) in rules {
        normalized = replace_whole_term(&normalized, &heard, &preferred)?.0;
    }
    Ok(normalized)
}

pub fn list_transcript_vocabulary(conn: &Connection) -> Result<Vec<TranscriptVocabularyRule>> {
    let mut stmt = conn.prepare(
        "SELECT v.id, v.heard, v.preferred, v.created_at, v.updated_at,
                b.id, b.changed_segments, b.created_at
         FROM transcript_vocabulary v
         LEFT JOIN transcript_correction_batches b ON b.id = (
           SELECT id FROM transcript_correction_batches latest
           WHERE latest.vocabulary_id = v.id AND latest.undone_at IS NULL
           ORDER BY latest.id DESC LIMIT 1
         )
         WHERE v.enabled = 1
         ORDER BY v.updated_at DESC, v.id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(TranscriptVocabularyRule {
            id: row.get(0)?,
            heard: row.get(1)?,
            preferred: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
            last_batch_id: row.get(5)?,
            last_changed_segments: row.get(6)?,
            last_applied_at: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn preview_transcript_vocabulary(
    conn: &Connection,
    heard: &str,
) -> Result<TranscriptVocabularyPreview> {
    let heard = heard.trim();
    if heard.is_empty() {
        return Ok(TranscriptVocabularyPreview {
            matching_segments: 0,
            occurrences: 0,
        });
    }
    let mut stmt = conn.prepare("SELECT text FROM meeting_segments ORDER BY id")?;
    let mut matching_segments = 0;
    let mut occurrences = 0;
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        let text = row?;
        let count = replace_whole_term(&text, heard, heard)?.1 as i64;
        if count > 0 {
            matching_segments += 1;
            occurrences += count;
        }
    }
    Ok(TranscriptVocabularyPreview {
        matching_segments,
        occurrences,
    })
}

pub fn apply_transcript_vocabulary(
    conn: &mut Connection,
    heard: &str,
    preferred: &str,
    now: &str,
) -> Result<TranscriptVocabularyApplyResult> {
    let (heard, preferred) = validate_vocabulary_terms(heard, preferred)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO transcript_vocabulary
           (heard, preferred, enabled, created_at, updated_at)
         VALUES (?1, ?2, 1, ?3, ?3)
         ON CONFLICT(heard) DO UPDATE SET
           preferred = excluded.preferred,
           enabled = 1,
           updated_at = excluded.updated_at",
        rusqlite::params![heard, preferred, now],
    )?;
    let vocabulary_id: i64 = tx.query_row(
        "SELECT id FROM transcript_vocabulary WHERE heard = ?1 COLLATE NOCASE",
        [&heard],
        |row| row.get(0),
    )?;
    let segments = {
        let mut stmt = tx.prepare("SELECT id, text FROM meeting_segments ORDER BY id")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut changes = Vec::new();
    let mut changed_occurrences = 0i64;
    for (segment_id, before) in segments {
        let (after, occurrences) = replace_whole_term(&before, &heard, &preferred)?;
        if occurrences > 0 && after != before {
            changed_occurrences += occurrences as i64;
            changes.push((segment_id, before, after));
        }
    }
    let batch_id = if changes.is_empty() {
        None
    } else {
        tx.execute(
            "INSERT INTO transcript_correction_batches
               (vocabulary_id, heard, preferred, changed_segments,
                changed_occurrences, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                vocabulary_id,
                heard,
                preferred,
                changes.len() as i64,
                changed_occurrences,
                now
            ],
        )?;
        let batch_id = tx.last_insert_rowid();
        for (segment_id, before, after) in &changes {
            tx.execute(
                "UPDATE meeting_segments SET text = ?2 WHERE id = ?1",
                rusqlite::params![segment_id, after],
            )?;
            tx.execute(
                "INSERT INTO transcript_correction_items
                   (batch_id, segment_id, before_text, after_text)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![batch_id, segment_id, before, after],
            )?;
        }
        Some(batch_id)
    };
    let changed_segments = changes.len() as i64;
    tx.commit()?;
    let rule = list_transcript_vocabulary(conn)?
        .into_iter()
        .find(|rule| rule.id == vocabulary_id)
        .ok_or_else(|| anyhow!("saved transcript vocabulary rule is unavailable"))?;
    Ok(TranscriptVocabularyApplyResult {
        rule,
        batch_id,
        changed_segments,
        changed_occurrences,
    })
}

pub fn remove_transcript_vocabulary(conn: &Connection, id: i64, now: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE transcript_vocabulary SET enabled = 0, updated_at = ?2
         WHERE id = ?1 AND enabled = 1",
        rusqlite::params![id, now],
    )?;
    if changed == 0 {
        return Err(anyhow!("transcript vocabulary rule not found"));
    }
    Ok(())
}

pub fn undo_transcript_vocabulary(
    conn: &mut Connection,
    batch_id: i64,
    now: &str,
) -> Result<TranscriptVocabularyUndoResult> {
    let tx = conn.transaction()?;
    let vocabulary_id: Option<i64> = tx
        .query_row(
            "SELECT vocabulary_id FROM transcript_correction_batches
             WHERE id = ?1 AND undone_at IS NULL",
            [batch_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let batch_exists: bool = tx.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM transcript_correction_batches
           WHERE id = ?1 AND undone_at IS NULL
         )",
        [batch_id],
        |row| row.get(0),
    )?;
    if !batch_exists {
        return Err(anyhow!("correction batch is unavailable or already undone"));
    }
    let items = {
        let mut stmt = tx.prepare(
            "SELECT segment_id, before_text, after_text
             FROM transcript_correction_items WHERE batch_id = ?1 ORDER BY segment_id",
        )?;
        let rows = stmt
            .query_map([batch_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut restored_segments = 0;
    let mut skipped_segments = 0;
    for (segment_id, before, after) in items {
        let current = tx
            .query_row(
                "SELECT text FROM meeting_segments WHERE id = ?1",
                [segment_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if current.as_deref() == Some(after.as_str()) {
            tx.execute(
                "UPDATE meeting_segments SET text = ?2 WHERE id = ?1",
                rusqlite::params![segment_id, before],
            )?;
            restored_segments += 1;
        } else {
            skipped_segments += 1;
        }
    }
    tx.execute(
        "UPDATE transcript_correction_batches SET undone_at = ?2 WHERE id = ?1",
        rusqlite::params![batch_id, now],
    )?;
    if let Some(vocabulary_id) = vocabulary_id {
        tx.execute(
            "UPDATE transcript_vocabulary SET enabled = 0, updated_at = ?2 WHERE id = ?1",
            rusqlite::params![vocabulary_id, now],
        )?;
    }
    tx.commit()?;
    Ok(TranscriptVocabularyUndoResult {
        restored_segments,
        skipped_segments,
    })
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
                audio_me_path, audio_them_path, note_id, video_path, trashed_at,
                asr_engine, asr_model
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
                "asr_engine": r.get::<_, Option<String>>(13)?,
                "asr_model": r.get::<_, Option<String>>(14)?,
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
