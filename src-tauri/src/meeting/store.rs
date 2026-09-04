// DB access for the meeting recorder. Transcript segments live in their own
// table (not entries.data_json) because they're large and append-heavy while
// recording. The AI summary is additionally filed as a regular note (see
// summarize.rs) so search/embeddings/KG stay unchanged.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use rusqlite::{types::Value as SqlValue, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const EXPLICIT_RECORDING_CONTEXT_KEY: &str = "_noted_recording_filing_context_v1";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MeetingFilingRule {
    pub email: String,
    pub folder_id: Option<i64>,
    pub folder_name: Option<String>,
    pub folder_path: Option<String>,
    pub priority: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingRoute {
    pub folder_id: Option<i64>,
    pub email: Option<String>,
    pub via: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingFilingDecision {
    pub filing_context: Option<String>,
    pub route: MeetingRoute,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MeetingFilingBackfillItem {
    pub meeting_id: i64,
    pub note_id: i64,
    pub title: String,
    pub status: String,
    pub folder_id: Option<i64>,
    pub folder_name: Option<String>,
    pub folder_path: Option<String>,
    pub email: Option<String>,
    pub via: String,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct MeetingFilingBackfillPreview {
    pub token: String,
    pub eligible: i64,
    pub would_file: i64,
    pub needs_filing: i64,
    pub already_filed: i64,
    pub manual: i64,
    pub items: Vec<MeetingFilingBackfillItem>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct MeetingFilingBackfillApply {
    pub reviewed: i64,
    pub filed: i64,
    pub needs_filing: i64,
    pub skipped: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoteFilingReviewState {
    folder_id: Option<i64>,
    source: Option<String>,
    event_id: Option<i64>,
    filing_context: Option<String>,
    latest_event_id: Option<i64>,
    latest_event_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeetingFilingBackfillReviewItem {
    meeting_id: i64,
    note_id: i64,
    title: String,
    event_json: Option<String>,
    stored_route_status: String,
    stored_route_via: String,
    stored_route_folder_id: Option<i64>,
    stored_route_email: Option<String>,
    stored_filing_context: Option<String>,
    stored_route_updated_at: Option<String>,
    filing: NoteFilingReviewState,
    item: MeetingFilingBackfillItem,
}

#[derive(Debug, Clone)]
struct MeetingFilingBackfillReview {
    database_key: String,
    items: Vec<MeetingFilingBackfillReviewItem>,
}

enum MeetingFilingBackfillInspection {
    Manual,
    AlreadyFiled,
    Eligible(MeetingFilingBackfillReviewItem),
}

static MEETING_FILING_BACKFILL_REVIEWS: OnceLock<
    Mutex<HashMap<String, MeetingFilingBackfillReview>>,
> = OnceLock::new();

fn meeting_filing_backfill_reviews() -> &'static Mutex<HashMap<String, MeetingFilingBackfillReview>>
{
    MEETING_FILING_BACKFILL_REVIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn normalize_owner_email(email: &str) -> Result<String> {
    let email = email.trim().to_lowercase();
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if local.is_empty()
        || domain.is_empty()
        || parts.next().is_some()
        || email.chars().any(char::is_whitespace)
    {
        return Err(anyhow!("enter a valid email address"));
    }
    Ok(email)
}

fn normalized_email(email: &str) -> Option<String> {
    normalize_owner_email(email).ok()
}

fn folder_name_and_path(conn: &Connection, folder_id: i64) -> Result<Option<(String, String)>> {
    let mut current = Some(folder_id);
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err(anyhow!("folder hierarchy contains a cycle"));
        }
        let row = conn
            .query_row(
                "SELECT name, parent_id FROM note_folders WHERE id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .optional()?;
        let Some((name, parent_id)) = row else {
            return Ok(None);
        };
        names.push(name);
        current = parent_id;
    }
    let name = names.first().cloned().unwrap_or_default();
    names.reverse();
    Ok(Some((name, names.join(" / "))))
}

pub fn meeting_filing_rules(conn: &Connection) -> Result<Vec<MeetingFilingRule>> {
    let mut stmt = conn.prepare(
        "SELECT email, folder_id, priority
         FROM meeting_filing_rules ORDER BY priority, email COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    rows.into_iter()
        .map(|(email, folder_id, priority)| {
            let destination = folder_id
                .map(|id| folder_name_and_path(conn, id))
                .transpose()?
                .flatten();
            Ok(MeetingFilingRule {
                email,
                folder_id,
                folder_name: destination.as_ref().map(|(name, _)| name.clone()),
                folder_path: destination.as_ref().map(|(_, path)| path.clone()),
                priority,
                enabled: folder_id.is_some() && destination.is_some(),
            })
        })
        .collect()
}

fn renumber_filing_rules(conn: &Connection, emails: &[String]) -> Result<()> {
    for (priority, email) in emails.iter().enumerate() {
        conn.execute(
            "UPDATE meeting_filing_rules SET priority = ?2 WHERE email = ?1",
            rusqlite::params![email, priority as i64],
        )?;
    }
    Ok(())
}

pub fn set_meeting_filing_rule(
    conn: &Connection,
    email: &str,
    folder_id: i64,
    priority: Option<i64>,
    now: &str,
) -> Result<MeetingFilingRule> {
    let email = normalize_owner_email(email)?;
    let destination_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM note_folders WHERE id = ?1)",
        [folder_id],
        |row| row.get(0),
    )?;
    if !destination_exists {
        return Err(anyhow!("filing destination not found"));
    }
    // Routing must never make recording fail later. Only destinations rooted
    // in the canonical Work/Personal contexts can supply filing provenance.
    crate::db::note_folder_context(conn, folder_id)?;

    let mut emails: Vec<String> = conn
        .prepare(
            "SELECT email FROM meeting_filing_rules
             WHERE email <> ?1 COLLATE NOCASE ORDER BY priority, email COLLATE NOCASE",
        )?
        .query_map([&email], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let insertion = priority.unwrap_or(emails.len() as i64).max(0) as usize;
    emails.insert(insertion.min(emails.len()), email.clone());

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO meeting_filing_rules (email, folder_id, priority, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(email) DO UPDATE SET
           folder_id = excluded.folder_id,
           priority = excluded.priority,
           updated_at = excluded.updated_at",
        rusqlite::params![email, folder_id, insertion as i64, now],
    )?;
    renumber_filing_rules(&tx, &emails)?;
    tx.commit()?;
    repair_one_on_one_speakers(conn)?;

    meeting_filing_rules(conn)?
        .into_iter()
        .find(|rule| rule.email == email)
        .ok_or_else(|| anyhow!("filing rule was not saved"))
}

pub fn delete_meeting_filing_rules(conn: &Connection, emails: &[String]) -> Result<usize> {
    let emails = emails
        .iter()
        .map(|email| normalize_owner_email(email))
        .collect::<Result<HashSet<_>>>()?;
    let tx = conn.unchecked_transaction()?;
    let mut changed = 0usize;
    for email in emails {
        changed += tx.execute(
            "DELETE FROM meeting_filing_rules WHERE email = ?1",
            [&email],
        )?;
    }
    let remaining: Vec<String> = tx
        .prepare("SELECT email FROM meeting_filing_rules ORDER BY priority, email COLLATE NOCASE")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    renumber_filing_rules(&tx, &remaining)?;
    tx.commit()?;
    // This is intentionally unconditional: callers also use it immediately
    // after removing a remembered calendar account, which changes owner
    // identity even when that account never had a filing rule.
    repair_one_on_one_speakers(conn)?;
    Ok(changed)
}

pub fn delete_meeting_filing_rule(conn: &Connection, email: &str) -> Result<bool> {
    Ok(delete_meeting_filing_rules(conn, &[email.to_string()])? > 0)
}

pub fn reorder_meeting_filing_rules(
    conn: &Connection,
    emails: &[String],
) -> Result<Vec<MeetingFilingRule>> {
    let normalized = emails
        .iter()
        .map(|email| normalize_owner_email(email))
        .collect::<Result<Vec<_>>>()?;
    let unique = normalized.iter().cloned().collect::<HashSet<_>>();
    if unique.len() != normalized.len() {
        return Err(anyhow!("filing rule order contains duplicate emails"));
    }
    let existing = meeting_filing_rules(conn)?
        .into_iter()
        .map(|rule| rule.email)
        .collect::<HashSet<_>>();
    if unique != existing {
        return Err(anyhow!(
            "filing rule order must include every saved email exactly once"
        ));
    }
    let tx = conn.unchecked_transaction()?;
    renumber_filing_rules(&tx, &normalized)?;
    tx.commit()?;
    repair_one_on_one_speakers(conn)?;
    meeting_filing_rules(conn)
}

pub fn configured_owner_emails(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT email FROM meeting_filing_rules")?;
    let mut emails = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    emails.extend(crate::gcal::configured_account_emails());
    Ok(emails)
}

fn event_identity_sources(event: &Value) -> HashMap<String, &'static str> {
    let mut identities = HashMap::new();
    let mut add = |email: &str, via: &'static str| {
        if let Some(email) = normalized_email(email) {
            identities.entry(email).or_insert(via);
        }
    };
    if let Some(email) = event.get("account").and_then(Value::as_str) {
        add(email, "source_account");
    }
    if let Some(email) = event
        .get("organizer_email")
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("organizer")
                .and_then(Value::as_str)
                .filter(|s| s.contains('@'))
        })
    {
        add(email, "organizer");
    }
    if let Some(email) = event.get("creator_email").and_then(Value::as_str) {
        add(email, "creator");
    }
    if let Some(emails) = event.get("attendee_emails").and_then(Value::as_array) {
        for email in emails.iter().filter_map(Value::as_str) {
            add(email, "attendee");
        }
    } else if let Some(attendees) = event.get("attendees").and_then(Value::as_array) {
        for attendee in attendees {
            if let Some(email) = attendee
                .as_str()
                .or_else(|| attendee.get("email").and_then(Value::as_str))
            {
                add(email, "attendee");
            }
        }
    }
    if let Some(emails) = event.get("associated_emails").and_then(Value::as_array) {
        for email in emails.iter().filter_map(Value::as_str) {
            add(email, "attendee");
        }
    }
    identities
}

pub fn resolve_meeting_route(conn: &Connection, event: Option<&Value>) -> Result<MeetingRoute> {
    let Some(event) = event else {
        return Ok(MeetingRoute {
            folder_id: None,
            email: None,
            via: "no_event".into(),
            status: "needs_filing".into(),
        });
    };
    let identities = event_identity_sources(event);
    for rule in meeting_filing_rules(conn)?
        .into_iter()
        .filter(|rule| rule.enabled)
    {
        if let Some(via) = identities.get(&rule.email) {
            return Ok(MeetingRoute {
                folder_id: rule.folder_id,
                email: Some(rule.email),
                via: (*via).into(),
                status: "matched".into(),
            });
        }
    }
    Ok(MeetingRoute {
        folder_id: None,
        email: None,
        via: "no_matching_rule".into(),
        status: "needs_filing".into(),
    })
}

fn normalize_filing_context(filing_context: Option<&str>) -> Result<Option<String>> {
    let filing_context = filing_context
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    if filing_context
        .as_deref()
        .is_some_and(|value| !matches!(value, "work" | "personal"))
    {
        return Err(anyhow!("filing context must be work or personal"));
    }
    Ok(filing_context)
}

fn explicit_recording_context(event: Option<&Value>) -> Option<&str> {
    event?
        .get(EXPLICIT_RECORDING_CONTEXT_KEY)
        .and_then(Value::as_str)
}

fn event_json_with_explicit_recording_context(
    event_json: Option<&str>,
    parsed_event: Option<&Value>,
    filing_context: Option<&str>,
) -> Option<String> {
    let Some(filing_context) = filing_context else {
        return event_json.map(str::to_string);
    };
    let mut event = match parsed_event {
        Some(Value::Object(object)) => Value::Object(object.clone()),
        None if event_json.is_none() => json!({}),
        _ => return event_json.map(str::to_string),
    };
    event.as_object_mut().expect("event object").insert(
        EXPLICIT_RECORDING_CONTEXT_KEY.into(),
        Value::String(filing_context.into()),
    );
    Some(event.to_string())
}

/// Resolve account routing and an explicit recording context as one decision.
/// The recording context is a direct user choice, so a rule may refine it
/// within the same context but may never move the recording across contexts.
pub fn resolve_meeting_filing(
    conn: &Connection,
    event: Option<&Value>,
    filing_context: Option<&str>,
) -> Result<MeetingFilingDecision> {
    let filing_context = normalize_filing_context(filing_context)?;
    let mut route = resolve_meeting_route(conn, event)?;
    let route_context = route
        .folder_id
        .map(|folder_id| crate::db::folder_filing_context(conn, folder_id))
        .transpose()?;

    if filing_context
        .as_ref()
        .zip(route_context.as_ref())
        .is_some_and(|(selected, routed)| selected != routed)
    {
        route.folder_id = None;
        route.via = "context_override".into();
        route.status = "needs_filing".into();
    }

    Ok(MeetingFilingDecision {
        filing_context: filing_context.or(route_context),
        route,
    })
}

pub fn create_meeting(
    conn: &Connection,
    title: &str,
    event_id: Option<&str>,
    event_json: Option<&str>,
    now: &str,
) -> Result<i64> {
    create_meeting_row(
        conn, title, event_id, event_json, None, None, None, "online", now,
    )
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
    create_meeting_with_asr_in_context(
        conn, title, event_id, event_json, asr_engine, asr_model, None, now,
    )
}

pub fn create_meeting_with_asr_in_context(
    conn: &Connection,
    title: &str,
    event_id: Option<&str>,
    event_json: Option<&str>,
    asr_engine: &str,
    asr_model: &str,
    filing_context: Option<&str>,
    now: &str,
) -> Result<i64> {
    create_meeting_with_asr_in_context_and_mode(
        conn,
        title,
        event_id,
        event_json,
        asr_engine,
        asr_model,
        filing_context,
        "online",
        now,
    )
}

pub fn create_meeting_with_asr_in_context_and_mode(
    conn: &Connection,
    title: &str,
    event_id: Option<&str>,
    event_json: Option<&str>,
    asr_engine: &str,
    asr_model: &str,
    filing_context: Option<&str>,
    capture_mode: &str,
    now: &str,
) -> Result<i64> {
    create_meeting_row(
        conn,
        title,
        event_id,
        event_json,
        Some(asr_engine),
        Some(asr_model),
        filing_context,
        capture_mode,
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
    filing_context: Option<&str>,
    capture_mode: &str,
    now: &str,
) -> Result<i64> {
    let explicit_filing_context = normalize_filing_context(filing_context)?;
    let parsed_event = event_json.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let decision = resolve_meeting_filing(
        conn,
        parsed_event.as_ref(),
        explicit_filing_context.as_deref(),
    )?;
    let stored_event_json = event_json_with_explicit_recording_context(
        event_json,
        parsed_event.as_ref(),
        explicit_filing_context.as_deref(),
    );
    let route = decision.route;
    let public_id = crate::db::new_public_id();
    conn.execute(
        "INSERT INTO meetings
            (public_id, title, event_id, event_json, started_at, status, asr_engine, asr_model,
             filing_context, route_folder_id, route_email, route_via, route_status,
             route_updated_at, created_at, capture_mode)
         VALUES (?1, ?2, ?3, ?4, ?5, 'recording', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?5, ?5, ?13)",
        rusqlite::params![
            public_id,
            title,
            event_id,
            stored_event_json,
            now,
            asr_engine,
            asr_model,
            decision.filing_context,
            route.folder_id,
            route.email,
            route.via,
            route.status,
            capture_mode,
        ],
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
               (SELECT COUNT(*)
                FROM entries e JOIN notes n ON n.id = e.note_id
                WHERE e.category_id = categories.id AND n.trashed_at IS NULL)",
            [],
        )?;
        tx.execute(
            "UPDATE entities SET
               mention_count =
                 (SELECT COUNT(*)
                  FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                  WHERE m.entity_id = entities.id AND n.trashed_at IS NULL),
               first_seen =
                 (SELECT MIN(m.event_date)
                  FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                  WHERE m.entity_id = entities.id AND n.trashed_at IS NULL),
               last_seen =
                 (SELECT MAX(m.event_date)
                  FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                  WHERE m.entity_id = entities.id AND n.trashed_at IS NULL)",
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

/// Begin one note-generation attempt. Meetings that already own a document
/// stay usable while an additional template is generated; missing projections
/// return to the explicit in-progress state.
pub fn mark_summary_attempt(conn: &Connection, id: i64) -> Result<()> {
    let changed = conn.execute(
        "UPDATE meetings
         SET status = CASE WHEN note_id IS NULL THEN 'summarizing' ELSE status END,
             summary_error = NULL
         WHERE id = ?1 AND trashed_at IS NULL",
        [id],
    )?;
    if changed == 0 {
        return Err(anyhow!("meeting not found"));
    }
    Ok(())
}

/// Persist a failed note-generation attempt instead of disguising it as done.
/// The bounded counter only applies while the primary document is still
/// missing; failures of optional extra templates must not hide an existing one.
pub fn mark_summary_failed(conn: &Connection, id: i64, error: &str) -> Result<()> {
    let error: String = error.chars().take(2_000).collect();
    conn.execute(
        "UPDATE meetings
         SET status = CASE WHEN note_id IS NULL THEN 'failed' ELSE status END,
             summary_error = ?2,
             summary_retry_count = CASE
               WHEN note_id IS NULL THEN COALESCE(summary_retry_count, 0) + 1
               ELSE COALESCE(summary_retry_count, 0)
             END
         WHERE id = ?1",
        rusqlite::params![id, error],
    )?;
    Ok(())
}

pub fn mark_summary_succeeded(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET status = 'done', summary_error = NULL WHERE id = ?1",
        [id],
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

/// Completed-looking meetings whose transcript survived but whose searchable
/// document never did. These were stranded by older builds that converted a
/// summary failure into `done`. Retry only a few times across app launches so
/// an unavailable model cannot create an endless startup loop.
pub fn list_summary_recovery_candidates(
    conn: &Connection,
    max_attempts: i64,
) -> Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT m.id,
                (SELECT COUNT(*) FROM meeting_segments s WHERE s.meeting_id = m.id)
         FROM meetings m
         WHERE m.note_id IS NULL
           AND m.trashed_at IS NULL
           AND m.status IN ('done', 'failed')
           AND COALESCE(m.summary_retry_count, 0) < ?1
           AND EXISTS (
             SELECT 1 FROM meeting_segments s WHERE s.meeting_id = m.id
           )
         ORDER BY m.id",
    )?;
    let rows = stmt
        .query_map([max_attempts], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?
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

/// Persist the user-owned meeting document while retaining a plain-text
/// projection for summaries, search context, exports, and older clients.
pub fn set_notes_document(
    conn: &Connection,
    id: i64,
    notes: &str,
    notes_document_json: Option<&str>,
) -> Result<()> {
    let notes_document_json = notes_document_json
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(document) = notes_document_json {
        let parsed: Value = serde_json::from_str(document)
            .map_err(|error| anyhow!("meeting notes document is not valid JSON: {error}"))?;
        if parsed.get("type").and_then(Value::as_str) != Some("doc") {
            return Err(anyhow!("meeting notes document must be a document"));
        }
    }
    conn.execute(
        "UPDATE meetings SET raw_notes = ?2, notes_document_json = ?3 WHERE id = ?1",
        rusqlite::params![id, notes, notes_document_json],
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

/// Make a meeting's destination an explicit, sticky choice. Before the first
/// summary exists, the route lives on the meeting and is consumed when its
/// library note is created. Afterwards, move that linked note through the
/// normal filing transition so history, context, and provenance stay aligned.
pub fn set_filing_destination(conn: &Connection, id: i64, folder_id: i64, now: &str) -> Result<()> {
    let filing_context = crate::db::note_folder_context(conn, folder_id)?;
    let note_id = conn
        .query_row("SELECT note_id FROM meetings WHERE id = ?1", [id], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .optional()?
        .ok_or_else(|| anyhow!("meeting not found"))?;

    if let Some(note_id) = note_id {
        crate::db::file_note(conn, note_id, Some(folder_id), now)?;
        return Ok(());
    }

    conn.execute(
        "UPDATE meetings SET filing_context = ?2, route_folder_id = ?3,
                route_email = NULL, route_via = 'manual', route_status = 'manual',
                route_updated_at = ?4 WHERE id = ?1",
        rusqlite::params![id, filing_context, folder_id, now],
    )?;
    Ok(())
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

fn note_filing(conn: &Connection, note_id: i64) -> Result<Option<(i64, String)>> {
    conn.query_row(
        "SELECT folder_id, COALESCE(source, 'manual')
         FROM note_folder_items WHERE note_id = ?1
         ORDER BY created_at DESC, folder_id LIMIT 1",
        [note_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn filing_is_sticky(
    conn: &Connection,
    note_id: i64,
    route_status: &str,
    filing: &Option<(i64, String)>,
) -> Result<bool> {
    if route_status == "manual"
        || filing
            .as_ref()
            .is_some_and(|(_, source)| matches!(source.as_str(), "manual" | "undo"))
    {
        return Ok(true);
    }
    let latest_source = conn
        .query_row(
            "SELECT source FROM note_filing_events
             WHERE note_id = ?1 ORDER BY id DESC LIMIT 1",
            [note_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(latest_source
        .as_deref()
        .is_some_and(|source| matches!(source, "manual" | "undo")))
}

fn meeting_route_reason(
    conn: &Connection,
    folder_id: i64,
    route_email: Option<&str>,
    route_via: &str,
) -> Result<String> {
    let destination = folder_name_and_path(conn, folder_id)?
        .map(|(_, path)| path)
        .ok_or_else(|| anyhow!("filing destination not found"))?;
    Ok(match route_email {
        Some(email) => {
            format!("Filed in {destination} because {email} matched the meeting {route_via}.")
        }
        None => format!("Filed in {destination} by its meeting rule."),
    })
}

/// Whether `folder_id` is the route destination itself or a more-specific
/// descendant. Existing automatic organization below an account-level route
/// is deliberate and should not be flattened back to the parent folder.
fn folder_is_within(conn: &Connection, folder_id: i64, ancestor_id: i64) -> Result<bool> {
    let mut current = Some(folder_id);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if id == ancestor_id {
            return Ok(true);
        }
        if !seen.insert(id) {
            return Err(anyhow!("folder hierarchy contains a cycle"));
        }
        current = conn
            .query_row(
                "SELECT parent_id FROM note_folders WHERE id = ?1",
                [id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
    }
    Ok(false)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RoutingParticipant {
    email: String,
    local: String,
    domain: String,
}

#[derive(Debug)]
struct RoutingFolderCandidate {
    id: i64,
    name: String,
    auto_rule: String,
    depth: i64,
    event_json: Option<String>,
}

fn email_parts(value: &str) -> Option<RoutingParticipant> {
    let email = normalized_email(value)?;
    let (local, domain) = email.split_once('@')?;
    Some(RoutingParticipant {
        email: email.clone(),
        local: local.to_string(),
        domain: domain.to_string(),
    })
}

fn routing_participants(conn: &Connection, event: &Value) -> Vec<RoutingParticipant> {
    let mut owners = configured_owner_emails(conn).unwrap_or_default();
    if let Some(account) = event.get("account").and_then(Value::as_str) {
        if let Some(account) = normalized_email(account) {
            owners.insert(account);
        }
    }
    let Some(attendees) = event.get("attendees").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    attendees
        .iter()
        .filter(|attendee| {
            !attendee
                .get("self")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && !attendee
                    .get("resource")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                && !attendee
                    .get("status")
                    .or_else(|| attendee.get("responseStatus"))
                    .or_else(|| attendee.get("response_status"))
                    .and_then(Value::as_str)
                    .is_some_and(|status| status.eq_ignore_ascii_case("declined"))
        })
        .filter_map(|attendee| {
            attendee
                .as_str()
                .or_else(|| attendee.get("email").and_then(Value::as_str))
                .and_then(email_parts)
        })
        .filter(|participant| !owners.contains(&participant.email))
        .filter(|participant| seen.insert(participant.email.clone()))
        .collect()
}

fn public_email_domain(domain: &str) -> bool {
    matches!(
        domain,
        "gmail.com"
            | "googlemail.com"
            | "outlook.com"
            | "hotmail.com"
            | "live.com"
            | "yahoo.com"
            | "icloud.com"
            | "me.com"
            | "proton.me"
            | "protonmail.com"
    )
}

fn domain_label(domain: &str) -> &str {
    domain.split('.').next().unwrap_or(domain)
}

fn normalized_folder_words(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

fn meeting_routing_candidates(
    conn: &Connection,
    broad_folder_id: i64,
    current_note_id: i64,
) -> Result<Vec<RoutingFolderCandidate>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE descendants(id, depth) AS (
           SELECT id, 0 FROM note_folders WHERE id = ?1
           UNION ALL
           SELECT child.id, parent.depth + 1
           FROM note_folders child JOIN descendants parent ON child.parent_id = parent.id
         )
         SELECT folder.id, folder.name, folder.auto_rule, descendants.depth, meeting.event_json
         FROM descendants
         JOIN note_folders folder ON folder.id = descendants.id
         LEFT JOIN note_folder_items filing ON filing.folder_id = folder.id
         LEFT JOIN meetings meeting
           ON meeting.note_id = filing.note_id
          AND meeting.note_id <> ?2
          AND meeting.trashed_at IS NULL
         WHERE descendants.depth > 0
         ORDER BY descendants.depth DESC, folder.position, folder.id",
    )?;
    let rows = stmt.query_map(rusqlite::params![broad_folder_id, current_note_id], |row| {
        Ok(RoutingFolderCandidate {
            id: row.get(0)?,
            name: row.get(1)?,
            auto_rule: row.get(2)?,
            depth: row.get(3)?,
            event_json: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Refine a broad account destination using calendar facts and existing folder
/// organization. Exact participant/company homes win, then semantic meeting
/// folders, while the broad account folder remains the safe fallback.
fn automatic_meeting_destination(
    conn: &Connection,
    note_id: i64,
    broad_folder_id: i64,
    event: Option<&Value>,
) -> Result<i64> {
    let standup_destination =
        crate::db::automatic_rule_destination_for_note(conn, note_id, broad_folder_id)?;
    if standup_destination != broad_folder_id {
        return Ok(standup_destination);
    }
    let Some(event) = event else {
        return Ok(broad_folder_id);
    };
    let participants = routing_participants(conn, event);
    let participant_count = external_attendees_for_event(conn, event).len();
    if participant_count == 0 {
        return Ok(broad_folder_id);
    }
    let account_domain = event
        .get("account")
        .and_then(Value::as_str)
        .and_then(email_parts)
        .map(|participant| participant.domain);
    let emails = participants
        .iter()
        .map(|participant| participant.email.as_str())
        .collect::<HashSet<_>>();
    let company_domains = participants
        .iter()
        .map(|participant| participant.domain.as_str())
        .filter(|domain| !public_email_domain(domain))
        .filter(|domain| account_domain.as_deref() != Some(*domain))
        .collect::<HashSet<_>>();
    let candidates = meeting_routing_candidates(conn, broad_folder_id, note_id)?;
    let mut best: Option<(i64, i64, i64)> = None;
    for candidate in &candidates {
        let words = normalized_folder_words(&candidate.name);
        let domain_name_match = company_domains
            .iter()
            .any(|domain| words.iter().any(|word| word == domain_label(domain)));
        let person_name_match = participant_count == 1
            && participants.iter().any(|participant| {
                participant.local.len() >= 3 && words.iter().any(|word| word == &participant.local)
            });
        let historical = candidate
            .event_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .map(|event| routing_participants(conn, &event))
            .unwrap_or_default();
        let historical_emails = historical
            .iter()
            .map(|participant| participant.email.as_str())
            .collect::<HashSet<_>>();
        let exact_participants = !emails.is_empty() && emails == historical_emails;
        let historical_domains = historical
            .iter()
            .map(|participant| participant.domain.as_str())
            .filter(|domain| !public_email_domain(domain))
            .filter(|domain| account_domain.as_deref() != Some(*domain))
            .collect::<HashSet<_>>();
        let learned_company = !company_domains.is_disjoint(&historical_domains);
        let score = if person_name_match {
            400
        } else if domain_name_match {
            350
        } else if exact_participants {
            300
        } else if learned_company {
            250
        } else {
            0
        };
        if score > 0 {
            let ranked = (score, candidate.depth, -candidate.id);
            if best.is_none_or(|current| ranked > current) {
                best = Some(ranked);
            }
        }
    }
    if let Some((_, _, negative_id)) = best {
        return Ok(-negative_id);
    }
    if participant_count == 1 {
        if let Some(folder) = candidates
            .iter()
            .find(|folder| folder.auto_rule == "one_on_one")
        {
            return Ok(folder.id);
        }
    }
    if !company_domains.is_empty() {
        if let Some(folder) = candidates
            .iter()
            .find(|folder| folder.auto_rule == "external_partner")
        {
            return Ok(folder.id);
        }
    }
    Ok(broad_folder_id)
}

/// Link the first generated note and apply the route captured when recording
/// began. A context inbox is only a provisional home and can be superseded by
/// an identity route; recording-context overrides, manual filings, and undo
/// restorations remain sticky.
pub fn set_note_id_and_apply_route(
    conn: &Connection,
    meeting_id: i64,
    note_id: i64,
    now: &str,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let route = tx
        .query_row(
            "SELECT COALESCE(route_status, 'needs_filing'), route_folder_id,
                    route_email, COALESCE(route_via, 'no_event'), event_json
             FROM meetings WHERE id = ?1",
            [meeting_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((status, folder_id, route_email, route_via, event_json)) = route else {
        return Err(anyhow!("meeting not found"));
    };
    let event = event_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    tx.execute(
        "UPDATE meetings SET note_id = ?2 WHERE id = ?1",
        rusqlite::params![meeting_id, note_id],
    )?;

    let existing_filing = note_filing(&tx, note_id)?;
    if filing_is_sticky(&tx, note_id, &status, &existing_filing)? {
        let existing_folder = existing_filing.as_ref().map(|(folder_id, _)| *folder_id);
        tx.execute(
            "UPDATE meetings SET filing_context =
                        (SELECT filing_context FROM notes WHERE id = ?2),
                    route_folder_id = ?3, route_email = NULL,
                    route_via = 'manual', route_status = 'manual', route_updated_at = ?4
             WHERE id = ?1",
            rusqlite::params![meeting_id, note_id, existing_folder, now],
        )?;
        tx.commit()?;
        return Ok(());
    }

    if status == "matched" {
        if let Some(broad_folder_id) = folder_id {
            let destination_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM note_folders WHERE id = ?1)",
                [broad_folder_id],
                |row| row.get(0),
            )?;
            if destination_exists {
                let refined_folder =
                    automatic_meeting_destination(&tx, note_id, broad_folder_id, event.as_ref())?;
                let destination_folder = match existing_filing.as_ref() {
                    Some((existing_folder, source))
                        if source == "rule"
                            && *existing_folder != broad_folder_id
                            && refined_folder == broad_folder_id
                            && folder_is_within(&tx, *existing_folder, broad_folder_id)? =>
                    {
                        *existing_folder
                    }
                    _ => refined_folder,
                };
                let filing_is_current =
                    existing_filing
                        .as_ref()
                        .is_some_and(|(existing_folder, source)| {
                            source == "rule" && *existing_folder == destination_folder
                        });
                if filing_is_current {
                    let context = crate::db::note_folder_context(&tx, destination_folder)?;
                    tx.execute(
                        "UPDATE notes SET filing_context = ?2 WHERE id = ?1",
                        rusqlite::params![note_id, context],
                    )?;
                    tx.execute(
                        "UPDATE meetings SET filing_context = ?2,
                                route_folder_id = ?3, route_email = ?4,
                                route_via = ?5, route_status = 'matched', route_updated_at = ?6
                         WHERE id = ?1",
                        rusqlite::params![
                            meeting_id,
                            context,
                            destination_folder,
                            route_email,
                            route_via,
                            now,
                        ],
                    )?;
                } else {
                    let reason = meeting_route_reason(
                        &tx,
                        destination_folder,
                        route_email.as_deref(),
                        &route_via,
                    )?;
                    let context = crate::db::note_folder_context(&tx, destination_folder)?;
                    crate::db::filing_transition(
                        &tx,
                        note_id,
                        Some(destination_folder),
                        "rule",
                        &reason,
                        Some(&context),
                        None,
                        now,
                    )?;
                    tx.execute(
                        "UPDATE meetings SET filing_context = ?2,
                                route_folder_id = ?3, route_email = ?4,
                                route_via = ?5, route_status = 'matched', route_updated_at = ?6
                         WHERE id = ?1",
                        rusqlite::params![
                            meeting_id,
                            context,
                            destination_folder,
                            route_email,
                            route_via,
                            now,
                        ],
                    )?;
                }
                tx.commit()?;
                return Ok(());
            }
        }
        tx.execute(
            "UPDATE meetings SET filing_context =
                        (SELECT filing_context FROM notes WHERE id = ?2),
                    route_folder_id = NULL, route_email = ?3,
                    route_via = 'destination_missing', route_status = 'needs_filing',
                    route_updated_at = ?4 WHERE id = ?1",
            rusqlite::params![meeting_id, note_id, route_email, now],
        )?;
    } else if let Some((existing_folder, _)) = existing_filing
        .as_ref()
        .filter(|(_, source)| source == "rule")
    {
        // A prior automatic rule remains a deliberate home when the current
        // identity rules no longer produce a replacement.
        let context = crate::db::note_folder_context(&tx, *existing_folder)?;
        tx.execute(
            "UPDATE notes SET filing_context = ?2 WHERE id = ?1",
            rusqlite::params![note_id, context],
        )?;
        tx.execute(
            "UPDATE meetings SET filing_context = ?2, route_folder_id = ?3,
                    route_email = NULL, route_via = 'filing_rule',
                    route_status = 'matched', route_updated_at = ?4 WHERE id = ?1",
            rusqlite::params![meeting_id, context, existing_folder, now],
        )?;
    } else {
        tx.execute(
            "UPDATE meetings SET filing_context =
                        (SELECT filing_context FROM notes WHERE id = ?2),
                    route_folder_id = ?3, route_email = NULL,
                    route_via = ?4, route_status = 'needs_filing',
                    route_updated_at = ?5 WHERE id = ?1",
            rusqlite::params![
                meeting_id,
                note_id,
                existing_filing.as_ref().map(|(folder_id, _)| *folder_id),
                route_via,
                now,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn filing_backfill_rows(
    conn: &Connection,
) -> Result<
    Vec<(
        i64,
        i64,
        String,
        Option<String>,
        String,
        String,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
    )>,
> {
    let mut stmt = conn.prepare(
        "SELECT id, note_id, title, event_json, COALESCE(route_status, 'needs_filing'),
                COALESCE(route_via, 'no_event'), route_folder_id, route_email,
                filing_context, route_updated_at
         FROM meetings WHERE note_id IS NOT NULL AND trashed_at IS NULL ORDER BY id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn backfill_item(
    conn: &Connection,
    meeting_id: i64,
    note_id: i64,
    title: String,
    route: MeetingRoute,
) -> Result<MeetingFilingBackfillItem> {
    let destination = route
        .folder_id
        .map(|folder_id| folder_name_and_path(conn, folder_id))
        .transpose()?
        .flatten();
    Ok(MeetingFilingBackfillItem {
        meeting_id,
        note_id,
        title,
        status: route.status,
        folder_id: route.folder_id,
        folder_name: destination.as_ref().map(|(name, _)| name.clone()),
        folder_path: destination.map(|(_, path)| path),
        email: route.email,
        via: route.via,
    })
}

fn note_filing_review_state(conn: &Connection, note_id: i64) -> Result<NoteFilingReviewState> {
    let (filing_context, folder_id, source, event_id) = conn
        .query_row(
            "SELECT n.filing_context, i.folder_id, i.source, i.event_id
             FROM notes n
             LEFT JOIN note_folder_items i ON i.note_id = n.id
             WHERE n.id = ?1",
            [note_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow!("backfill note not found"))?;
    let latest_event = conn
        .query_row(
            "SELECT id, source FROM note_filing_events
             WHERE note_id = ?1 ORDER BY id DESC LIMIT 1",
            [note_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(NoteFilingReviewState {
        folder_id,
        source,
        event_id,
        filing_context,
        latest_event_id: latest_event.as_ref().map(|(id, _)| *id),
        latest_event_source: latest_event.map(|(_, source)| source),
    })
}

fn resolve_backfill_route(
    conn: &Connection,
    event: Option<&Value>,
    meeting_filing_context: Option<&str>,
) -> Result<MeetingRoute> {
    let Some(captured_context) = explicit_recording_context(event) else {
        return resolve_meeting_route(conn, event);
    };
    let captured_context = normalize_filing_context(Some(captured_context))?
        .ok_or_else(|| anyhow!("explicit recording context is missing"))?;
    let meeting_filing_context = normalize_filing_context(meeting_filing_context)?
        .ok_or_else(|| anyhow!("explicit recording context is missing"))?;
    if captured_context != meeting_filing_context {
        return Err(anyhow!("explicit recording context provenance changed"));
    }
    Ok(resolve_meeting_filing(conn, event, Some(&meeting_filing_context))?.route)
}

fn inspect_meeting_filing_backfill_row(
    conn: &Connection,
    meeting_id: i64,
    note_id: i64,
    title: String,
    event_json: Option<String>,
    route_status: String,
    route_via: String,
    route_folder_id: Option<i64>,
    route_email: Option<String>,
    meeting_filing_context: Option<String>,
    route_updated_at: Option<String>,
) -> Result<MeetingFilingBackfillInspection> {
    let filing_state = note_filing_review_state(conn, note_id)?;
    let filing = filing_state.folder_id.zip(filing_state.source.clone());
    if filing_is_sticky(conn, note_id, &route_status, &filing)? {
        return Ok(MeetingFilingBackfillInspection::Manual);
    }

    let event = event_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let mut route =
        resolve_backfill_route(conn, event.as_ref(), meeting_filing_context.as_deref())?;
    let broad_route_folder = route.folder_id;
    if route.status == "matched" {
        if let Some(folder_id) = route.folder_id {
            route.folder_id = Some(automatic_meeting_destination(
                conn,
                note_id,
                folder_id,
                event.as_ref(),
            )?);
        }
    }
    let automatic_is_current = match filing.as_ref() {
        Some((_, source)) if source == "rule" && route.status != "matched" => true,
        Some((folder_id, source)) if source == "rule" => match route.folder_id {
            Some(route_folder) if *folder_id == route_folder => true,
            Some(route_folder) if Some(route_folder) == broad_route_folder => {
                let broad = broad_route_folder.expect("matched route folder");
                *folder_id != broad && folder_is_within(conn, *folder_id, broad)?
            }
            Some(_) => false,
            None => false,
        },
        _ => false,
    };
    if automatic_is_current {
        return Ok(MeetingFilingBackfillInspection::AlreadyFiled);
    }
    let item = backfill_item(conn, meeting_id, note_id, title.clone(), route)?;
    Ok(MeetingFilingBackfillInspection::Eligible(
        MeetingFilingBackfillReviewItem {
            meeting_id,
            note_id,
            title,
            event_json,
            stored_route_status: route_status,
            stored_route_via: route_via,
            stored_route_folder_id: route_folder_id,
            stored_route_email: route_email,
            stored_filing_context: meeting_filing_context,
            stored_route_updated_at: route_updated_at,
            filing: filing_state,
            item,
        },
    ))
}

fn inspect_meeting_filing_backfill(
    conn: &Connection,
    meeting_id: i64,
) -> Result<Option<MeetingFilingBackfillInspection>> {
    let row = conn
        .query_row(
            "SELECT note_id, title, event_json,
                    COALESCE(route_status, 'needs_filing'),
                    COALESCE(route_via, 'no_event'), route_folder_id, route_email,
                    filing_context, route_updated_at
             FROM meetings
             WHERE id = ?1 AND note_id IS NOT NULL AND trashed_at IS NULL",
            [meeting_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(
            note_id,
            title,
            event_json,
            route_status,
            route_via,
            route_folder_id,
            route_email,
            filing_context,
            route_updated_at,
        )| {
            inspect_meeting_filing_backfill_row(
                conn,
                meeting_id,
                note_id,
                title,
                event_json,
                route_status,
                route_via,
                route_folder_id,
                route_email,
                filing_context,
                route_updated_at,
            )
        },
    )
    .transpose()
}

fn meeting_filing_backfill_database_key(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare("PRAGMA database_list")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;
    for row in rows {
        let (name, file) = row?;
        if name == "main" {
            return Ok(if file.is_empty() {
                format!("memory:{conn:p}")
            } else {
                file
            });
        }
    }
    Err(anyhow!("main database not found"))
}

fn random_backfill_token() -> String {
    let bytes = rand::random::<[u8; 32]>();
    let mut token = String::with_capacity(64);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    token
}

fn remember_meeting_filing_backfill_review(
    conn: &Connection,
    items: Vec<MeetingFilingBackfillReviewItem>,
) -> Result<String> {
    let database_key = meeting_filing_backfill_database_key(conn)?;
    let mut reviews = meeting_filing_backfill_reviews()
        .lock()
        .map_err(|_| anyhow!("meeting filing review state is unavailable"))?;
    // A newer preview for the same database supersedes older approval. This
    // also bounds memory without invalidating previews in other test/app DBs.
    reviews.retain(|_, review| review.database_key != database_key);
    let mut token = random_backfill_token();
    while reviews.contains_key(&token) {
        token = random_backfill_token();
    }
    reviews.insert(
        token.clone(),
        MeetingFilingBackfillReview {
            database_key,
            items,
        },
    );
    Ok(token)
}

fn take_meeting_filing_backfill_review(
    conn: &Connection,
    token: &str,
) -> Result<MeetingFilingBackfillReview> {
    let token = token.trim();
    if token.is_empty() {
        return Err(anyhow!("filing preview token is required; preview again"));
    }
    let database_key = meeting_filing_backfill_database_key(conn)?;
    let mut reviews = meeting_filing_backfill_reviews()
        .lock()
        .map_err(|_| anyhow!("meeting filing review state is unavailable"))?;
    let review = reviews
        .remove(token)
        .ok_or_else(|| anyhow!("filing preview expired; preview again"))?;
    if review.database_key != database_key {
        return Err(anyhow!(
            "filing preview belongs to another database; preview again"
        ));
    }
    Ok(review)
}

/// Read-only historical review. Context inbox membership is provisional and
/// remains eligible for an identity route; manual choices, recording-context
/// overrides, and undo restorations are sticky.
pub fn meeting_filing_backfill_preview(conn: &Connection) -> Result<MeetingFilingBackfillPreview> {
    let mut preview = MeetingFilingBackfillPreview::default();
    let mut review_items = Vec::new();
    for (
        meeting_id,
        note_id,
        title,
        event_json,
        route_status,
        route_via,
        route_folder_id,
        route_email,
        filing_context,
        route_updated_at,
    ) in filing_backfill_rows(conn)?
    {
        match inspect_meeting_filing_backfill_row(
            conn,
            meeting_id,
            note_id,
            title,
            event_json,
            route_status,
            route_via,
            route_folder_id,
            route_email,
            filing_context,
            route_updated_at,
        )? {
            MeetingFilingBackfillInspection::Manual => preview.manual += 1,
            MeetingFilingBackfillInspection::AlreadyFiled => preview.already_filed += 1,
            MeetingFilingBackfillInspection::Eligible(review) => {
                preview.eligible += 1;
                if review.item.status == "matched" {
                    preview.would_file += 1;
                } else {
                    preview.needs_filing += 1;
                }
                preview.items.push(review.item.clone());
                review_items.push(review);
            }
        }
    }
    preview.token = remember_meeting_filing_backfill_review(conn, review_items)?;
    Ok(preview)
}

/// Apply exactly one reviewed historical batch. The opaque token is one-shot;
/// every previewed row, filing projection, route, and destination is validated
/// before any write. New eligible meetings are intentionally outside the batch.
pub fn meeting_filing_backfill_apply(
    conn: &Connection,
    token: &str,
    now: &str,
) -> Result<MeetingFilingBackfillApply> {
    let review = take_meeting_filing_backfill_review(conn, token)?;
    // Acquire the write reservation before validation so another SQLite
    // connection cannot change a reviewed row between the checks and writes.
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    for expected in &review.items {
        let current = inspect_meeting_filing_backfill(&tx, expected.meeting_id)?;
        match current {
            Some(MeetingFilingBackfillInspection::Eligible(current)) if current == *expected => {}
            _ => {
                return Err(anyhow!(
                    "filing preview is stale; preview again before applying"
                ));
            }
        }
    }

    let mut report = MeetingFilingBackfillApply {
        reviewed: review.items.len() as i64,
        ..MeetingFilingBackfillApply::default()
    };
    for reviewed in review.items {
        let meeting_id = reviewed.meeting_id;
        let note_id = reviewed.note_id;
        let route = reviewed.item;
        if route.status == "matched" {
            let folder_id = route
                .folder_id
                .ok_or_else(|| anyhow!("reviewed filing destination is missing"))?;
            let reason = meeting_route_reason(&tx, folder_id, route.email.as_deref(), &route.via)?;
            let context = crate::db::note_folder_context(&tx, folder_id)?;
            crate::db::filing_transition(
                &tx,
                note_id,
                Some(folder_id),
                "rule",
                &reason,
                Some(&context),
                None,
                now,
            )?;
            tx.execute(
                "UPDATE meetings SET filing_context = ?2, route_folder_id = ?3,
                        route_email = ?4, route_via = ?5, route_status = 'matched',
                        route_updated_at = ?6 WHERE id = ?1",
                rusqlite::params![meeting_id, context, folder_id, route.email, route.via, now],
            )?;
            report.filed += 1;
        } else {
            tx.execute(
                "UPDATE meetings SET filing_context =
                            (SELECT filing_context FROM notes WHERE id = ?2),
                        route_folder_id = ?3, route_email = NULL,
                        route_via = ?4, route_status = 'needs_filing',
                        route_updated_at = ?5 WHERE id = ?1",
                rusqlite::params![
                    meeting_id,
                    note_id,
                    reviewed.filing.folder_id,
                    route.via,
                    now,
                ],
            )?;
            report.needs_filing += 1;
        }
    }
    tx.commit()?;
    Ok(report)
}

pub fn insert_segment(
    conn: &Connection,
    meeting_id: i64,
    channel: &str,
    t0_ms: i64,
    t1_ms: i64,
    text: &str,
) -> Result<i64> {
    insert_segment_with_voice_time(conn, meeting_id, channel, t0_ms, t1_ms, None, text)
}

/// Persist a transcript row with speech-only VAD timing. Keeping this separate
/// from the legacy helper lets deterministic fixtures and repaired historical
/// transcripts remain explicitly unknown instead of fabricating precise pace.
pub fn insert_segment_with_voice_time(
    conn: &Connection,
    meeting_id: i64,
    channel: &str,
    t0_ms: i64,
    t1_ms: i64,
    voiced_ms: Option<i64>,
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
        "INSERT INTO meeting_segments
           (meeting_id, channel, t0_ms, t1_ms, voiced_ms, text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![meeting_id, channel, t0_ms, t1_ms, voiced_ms, normalized],
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
    clear_channel_speakers(conn, meeting_id, "them")
}

pub fn clear_channel_speakers(conn: &Connection, meeting_id: i64, channel: &str) -> Result<()> {
    conn.execute(
        "UPDATE meeting_segments SET speaker = NULL
         WHERE meeting_id = ?1 AND channel = ?2",
        rusqlite::params![meeting_id, channel],
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
    segment_times(conn, meeting_id, "them")
}

pub fn segment_times(
    conn: &Connection,
    meeting_id: i64,
    channel: &str,
) -> Result<Vec<(i64, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, t0_ms, t1_ms FROM meeting_segments
         WHERE meeting_id = ?1 AND channel = ?2 ORDER BY t0_ms",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![meeting_id, channel], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
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
    // A no-op rename must remain a no-op. Without this guard, the merge path
    // finds the same row as both source and destination, then deletes it.
    if from == to {
        return Ok(());
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

pub fn external_attendees_for_event(conn: &Connection, event_json: &Value) -> Vec<String> {
    let owners = configured_owner_emails(conn).unwrap_or_default();
    super::summarize::external_attendees_excluding(event_json, &owners)
}

fn external_attendees_from_raw(conn: &Connection, event_json: Option<&str>) -> Vec<String> {
    let Some(raw) = event_json else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    external_attendees_for_event(conn, &value)
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

fn one_on_one_identity_fingerprint(conn: &Connection) -> Result<String> {
    let mut owners = configured_owner_emails(conn)?
        .into_iter()
        .collect::<Vec<_>>();
    owners.sort();
    let mut hasher = Sha256::new();
    hasher.update(b"meeting-one-on-one-speakers-v2\0");
    for owner in owners {
        hasher.update(owner.as_bytes());
        hasher.update(b"\0");
    }
    let digest = hasher.finalize();
    let mut fingerprint = String::with_capacity(64);
    for byte in digest {
        fingerprint.push_str(&format!("{byte:02x}"));
    }
    Ok(fingerprint)
}

/// Repair historical one-on-ones whenever the set of owner identities changes.
/// Calendar membership is the strongest available ground truth: in a true 1:1
/// every remote line belongs to its sole external attendee, including rows that
/// an old automatic voice profile incorrectly named "Brian". The fingerprint
/// prevents the pass from repeatedly overwriting edits once identities settle.
pub fn repair_one_on_one_speakers(conn: &Connection) -> Result<usize> {
    const FINGERPRINT_KEY: &str = "meeting_one_on_one_speakers_identity_v2";
    let fingerprint = one_on_one_identity_fingerprint(conn)?;
    let repaired_fingerprint: Option<String> = conn
        .query_row(
            "SELECT value FROM app_metadata WHERE key = ?1",
            [FINGERPRINT_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if repaired_fingerprint.as_deref() == Some(&fingerprint) {
        return Ok(0);
    }

    let tx = conn.unchecked_transaction()?;
    let meetings = {
        let mut stmt = tx.prepare("SELECT id, event_json FROM meetings ORDER BY id")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    let mut repaired = 0usize;
    for (meeting_id, event_json) in meetings {
        let attendees = external_attendees_from_raw(&tx, event_json.as_deref());
        if attendees.len() != 1 {
            continue;
        }
        let attendee = &attendees[0];
        let changed_segments = tx.execute(
            "UPDATE meeting_segments SET speaker = ?2
             WHERE meeting_id = ?1 AND channel = 'them'
               AND COALESCE(speaker, '') <> ?2",
            rusqlite::params![meeting_id, attendee],
        )?;
        // These centroids were the source of stale cross-meeting names and no
        // longer describe distinct speakers once the calendar proves a 1:1.
        let changed_speakers = tx.execute(
            "DELETE FROM meeting_speakers WHERE meeting_id = ?1",
            [meeting_id],
        )?;
        if changed_segments > 0 || changed_speakers > 0 {
            repaired += 1;
        }
    }

    tx.execute(
        "INSERT INTO app_metadata (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![FINGERPRINT_KEY, fingerprint],
    )?;
    tx.commit()?;
    Ok(repaired)
}

pub fn initialize_one_on_one_speakers(conn: &Connection) -> Result<()> {
    repair_one_on_one_speakers(conn)?;
    Ok(())
}

/// Full transcript, timeline order, interleaved across channels.
pub fn list_segments(conn: &Connection, meeting_id: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, channel, t0_ms, t1_ms, voiced_ms, text, speaker
         FROM meeting_segments WHERE meeting_id = ?1 ORDER BY t0_ms ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([meeting_id], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "channel": r.get::<_, String>(1)?,
                "t0_ms": r.get::<_, i64>(2)?,
                "t1_ms": r.get::<_, i64>(3)?,
                "voiced_ms": r.get::<_, Option<i64>>(4)?,
                "text": r.get::<_, String>(5)?,
                "speaker": r.get::<_, Option<String>>(6)?,
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
        let attendees = external_attendees_from_raw(conn, event_json.as_deref());
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

fn meeting_types_by_id(conn: &Connection) -> Result<HashMap<i64, String>> {
    let candidates = meeting_search_candidates(conn)?;
    let (_, standup_notes) = folder_search_data(conn)?;
    Ok(candidates
        .iter()
        .map(|meeting| {
            (
                meeting.id,
                meeting_type(meeting, &standup_notes).0.to_string(),
            )
        })
        .collect())
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
                  WHEN COALESCE(m.capture_mode, 'online') = 'in_person'
                    THEN COALESCE(NULLIF(s.speaker, ''), 'Speaker')
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
    content_json: Option<&Value>,
    now: &str,
) -> Result<i64> {
    let content_json = content_json.map(serde_json::to_string).transpose()?;
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
            "UPDATE meeting_summaries
             SET content_md = ?2, content_json = ?3, created_at = ?4
             WHERE id = ?1",
            rusqlite::params![id, content_md, content_json, now],
        )?;
        conn.execute(
            "DELETE FROM meeting_summaries
             WHERE meeting_id = ?1 AND template = ?2 AND id <> ?3",
            rusqlite::params![meeting_id, template, id],
        )?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO meeting_summaries
           (meeting_id, template, content_md, content_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![meeting_id, template, content_md, content_json, now],
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
        "UPDATE meeting_summaries SET content_md = ?2, content_json = NULL WHERE id = ?1",
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
        "SELECT id, template, content_md, content_json, created_at FROM meeting_summaries
         WHERE meeting_id = ?1
           AND id = (SELECT MAX(newer.id) FROM meeting_summaries newer
                     WHERE newer.meeting_id = meeting_summaries.meeting_id
                       AND newer.template = meeting_summaries.template)
         ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map([meeting_id], |r| {
            let content_json = r
                .get::<_, Option<String>>(3)?
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "template": r.get::<_, String>(1)?,
                "content_md": r.get::<_, String>(2)?,
                "content_json": content_json,
                "created_at": r.get::<_, String>(4)?,
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
    let meeting_types = meeting_types_by_id(conn)?;
    let mut stmt = conn.prepare(
        "SELECT m.id, m.title, m.started_at, m.ended_at, m.status, m.note_id, m.event_json,
                (SELECT COUNT(*) FROM meeting_segments s WHERE s.meeting_id = m.id),
                (SELECT COUNT(DISTINCT y.template) FROM meeting_summaries y WHERE y.meeting_id = m.id),
                m.trashed_at, m.route_folder_id, m.route_email,
                COALESCE(m.route_via, 'no_event'),
                COALESCE(m.route_status, 'needs_filing'), m.filing_context,
                COALESCE(m.capture_mode, 'online')
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
                "route_folder_id": r.get::<_, Option<i64>>(10)?,
                "route_email": r.get::<_, Option<String>>(11)?,
                "route_via": r.get::<_, String>(12)?,
                "route_status": r.get::<_, String>(13)?,
                "filing_context": r.get::<_, Option<String>>(14)?,
                "capture_mode": r.get::<_, String>(15)?,
                "meeting_type": meeting_types
                    .get(&r.get::<_, i64>(0)?)
                    .cloned()
                    .unwrap_or_else(|| "other".to_string()),
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Everything the meeting page needs in one call.
pub fn get_meeting(conn: &Connection, id: i64) -> Result<Value> {
    let meeting_type = meeting_types_by_id(conn)?
        .remove(&id)
        .unwrap_or_else(|| "other".to_string());
    let meta = conn.query_row(
        "SELECT id, public_id, title, event_id, event_json, started_at, ended_at, status, raw_notes,
                audio_me_path, audio_them_path, note_id, video_path, trashed_at,
                asr_engine, asr_model, route_folder_id, route_email,
                COALESCE(route_via, 'no_event'), COALESCE(route_status, 'needs_filing'),
                filing_context, COALESCE(capture_mode, 'online'), notes_document_json
         FROM meetings WHERE id = ?1",
        [id],
        |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "public_id": r.get::<_, String>(1)?,
                "title": r.get::<_, String>(2)?,
                "event_id": r.get::<_, Option<String>>(3)?,
                "event_json": r.get::<_, Option<String>>(4)?
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "started_at": r.get::<_, Option<String>>(5)?,
                "ended_at": r.get::<_, Option<String>>(6)?,
                "status": r.get::<_, String>(7)?,
                "raw_notes": r.get::<_, String>(8)?,
                "audio_me_path": r.get::<_, Option<String>>(9)?,
                "audio_them_path": r.get::<_, Option<String>>(10)?,
                "note_id": r.get::<_, Option<i64>>(11)?,
                "video_path": r.get::<_, Option<String>>(12)?,
                "trashed_at": r.get::<_, Option<String>>(13)?,
                "asr_engine": r.get::<_, Option<String>>(14)?,
                "asr_model": r.get::<_, Option<String>>(15)?,
                "route_folder_id": r.get::<_, Option<i64>>(16)?,
                "route_email": r.get::<_, Option<String>>(17)?,
                "route_via": r.get::<_, String>(18)?,
                "route_status": r.get::<_, String>(19)?,
                "filing_context": r.get::<_, Option<String>>(20)?,
                "capture_mode": r.get::<_, String>(21)?,
                "notes_document_json": r.get::<_, Option<String>>(22)?,
                "meeting_type": meeting_type,
            }))
        },
    )?;
    let segments = list_segments(conn, id)?;
    let summaries = list_summaries(conn, id)?;
    let (me_ms, them_ms) = talk_time(conn, id)?;
    let speakers = list_meeting_speakers(conn, id)?;
    let event = &meta["event_json"];
    let expected_remote_speakers = event["attendees"].as_array().map(|attendees| {
        let visible_remote = external_attendees_for_event(conn, event).len();
        let full_count = event["attendee_count"]
            .as_u64()
            .map(|count| count as usize)
            .unwrap_or(attendees.len());
        if full_count > attendees.len() {
            let visible_self = attendees
                .iter()
                .filter(|attendee| attendee["self"].as_bool().unwrap_or(false))
                .count();
            full_count.saturating_sub(visible_self)
        } else {
            visible_remote
        }
    });
    let conversation = super::analytics::build(&segments, expected_remote_speakers);
    let mut out = meta;
    out["segments"] = json!(segments);
    out["summaries"] = json!(summaries);
    out["talk_ms"] = json!({ "me": me_ms, "them": them_ms });
    out["speakers"] = json!(speakers);
    out["conversation"] = conversation;
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
        "General meeting lens. Organize the detailed discussion by each substantive \
         topic or workstream that advanced. Preserve operating sequences, maturity \
         stages, examples, options, constraints, feedback, rationale, and dependencies. \
         Use the workplan when the group established priorities or a definition of \
         progress. Distinguish settled direction from actions and unresolved conditions.",
    ),
    (
        "1:1",
        "One-on-one lens. Organize the detailed discussion around updates and wins, \
         feedback and support, blockers and concerns, career or operating context, and \
         topics to revisit. Preserve the examples and rationale behind feedback. Use \
         the workplan for prioritized objectives and definitions of progress. Capture \
         both participants' commitments with grounded owners.",
    ),
    (
        "Standup",
        "Daily standup lens. Group detailed discussion by person or workstream and \
         preserve since-last-time progress, next work, blockers, dependencies, and \
         cross-team coordination. Keep routine status concise but fully capture material \
         technical reasoning, changes in plan, and follow-ups beyond normal next work.",
    ),
    (
        "Interview",
        "Interview lens (candidate, user research, or journalistic). Organize detailed \
         discussion around background and context, each evidence-backed theme, concrete \
         answers and examples, needs or behaviors, contradictions, gaps, and unanswered \
         questions. Preserve the question context when it changes the meaning. Do not \
         make an unsupported verdict.",
    ),
    (
        "Lecture",
        "Lecture or talk lens. Organize detailed discussion around the thesis, each key \
         concept or argument, definitions, supporting evidence, caveats, examples, \
         references, audience questions, and practical implications. Preserve why each \
         example, paper, book, or tool was introduced.",
    ),
    (
        "Project Update",
        "Project update lens. Organize detailed discussion around current status, \
         completed and measurable progress, product or technical workstreams, scope and \
         sequencing choices, risks, blockers, mitigations, and upcoming milestones. Do \
         not infer on-track status. Use the workplan for ordered milestones and their \
         definitions of progress.",
    ),
    (
        "Client Call",
        "Client or customer call lens. Organize detailed discussion around the client's \
         priorities, goals, needs, constraints, objections, success criteria, evidence, \
         and relationship context. Distinguish mutual agreements from promises by either \
         side, and preserve dates, dependencies, unresolved concerns, and next steps.",
    ),
    (
        "Brainstorm",
        "Brainstorm lens. Organize detailed discussion around the problem or opportunity, \
         every distinct substantive idea, the need it addresses, evidence or advantages, \
         feasibility constraints, tradeoffs, objections, combinations, and deferred \
         ideas. Preserve why ideas were selected or rejected. Use the workplan for \
         experiments and concrete validation criteria.",
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
