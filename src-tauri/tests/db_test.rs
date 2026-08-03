// Runtime validation for M0/M1 without the GUI:
//  - sqlite-vec actually loads (vec_version + vec0 table) at runtime
//  - save_entry creates a category, then grows its schema additively
use serde_json::json;
use tauri_app_lib::db::{self, SaveInput};

fn save(conn: &mut rusqlite::Connection, cat: &str, desc: &str, data: serde_json::Value, ts: &str) {
    db::save_note(
        conn,
        SaveInput {
            raw_text: format!("note about {cat}"),
            source: "text".into(),
            image_path: None,
            event_date: ts[..10].to_string(), // YYYY-MM-DD from the timestamp
            entries: vec![db::EntryInput { category: cat.into(), description: desc.into(), data, }],
        },
        ts,
    )
    .unwrap();
}

#[test]
fn vec_loads_and_schema_evolves() {
    let tmp = std::env::temp_dir().join(format!("noted_test_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);

    let mut conn = db::init(&tmp).expect("db init (loads sqlite-vec, creates vec0 table)");

    // sqlite-vec is live at runtime
    let v: String = conn
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .expect("vec_version() works -> extension loaded");
    assert!(v.starts_with('v'), "unexpected vec_version: {v}");

    // First gym note creates the category from scratch.
    save(
        &mut conn,
        "gym",
        "workouts",
        json!({ "exercises": [{ "name": "bench", "weight": 185, "reps": 5, "sets": 3 }] }),
        "2026-06-02T00:00:00Z",
    );

    // Second gym note introduces a novel field (rpe) -> schema must grow.
    save(
        &mut conn,
        "gym",
        "",
        json!({ "exercises": [{ "name": "squat", "weight": 225, "reps": 5, "sets": 5, "rpe": 8 }] }),
        "2026-06-02T01:00:00Z",
    );

    // A different note creates a second, separate category.
    save(
        &mut conn,
        "schedule",
        "daily time blocks",
        json!({ "blocks": [{ "task": "coding", "duration_min": 120 }] }),
        "2026-06-02T02:00:00Z",
    );

    let cats = db::list_categories(&conn).unwrap();
    assert_eq!(cats.len(), 2, "two emergent categories");

    let gym = cats.iter().find(|c| c.name == "gym").unwrap();
    assert_eq!(gym.entry_count, 2);
    let freq = gym.schema.get("field_freq").unwrap();
    assert_eq!(freq.get("exercises.weight").and_then(|x| x.as_i64()), Some(2));
    assert_eq!(
        freq.get("exercises.rpe").and_then(|x| x.as_i64()),
        Some(1),
        "novel field grew the schema"
    );
    // shape is the union of both entries
    let shape = gym.schema.get("shape").unwrap();
    assert!(shape["exercises"][0].get("rpe").is_some(), "shape merged rpe");

    let notes = db::list_notes(&conn).unwrap();
    assert_eq!(notes.len(), 3);
    assert!(notes.iter().all(|n| n.event_date == "2026-06-02"), "every entry is dated");

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn note_titles_and_bodies_are_user_editable() {
    let tmp = std::env::temp_dir().join(format!(
        "noted_note_edit_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();
    save(
        &mut conn,
        "journal",
        "",
        json!({"mood":"good"}),
        "2026-07-31T12:00:00Z",
    );
    let note_id: i64 = conn
        .query_row("SELECT id FROM notes LIMIT 1", [], |r| r.get(0))
        .unwrap();
    db::insert_embedding(&conn, note_id, &vec![0.1; 768]).unwrap();

    db::update_note(&conn, note_id, "A title I chose", "A body I rewrote.").unwrap();

    let notes = db::list_notes(&conn).unwrap();
    assert_eq!(notes[0].title, "A title I chose");
    assert_eq!(notes[0].raw_text, "A body I rewrote.");
    assert_eq!(db::embedding_count(&conn).unwrap(), 0);
    let embed_text = db::note_embed_text(&conn, note_id).unwrap();
    assert!(embed_text.contains("A title I chose"));
    assert!(embed_text.contains("A body I rewrote."));

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn meeting_title_and_generated_notes_are_editable() {
    use tauri_app_lib::meeting::store;

    let tmp = std::env::temp_dir().join(format!(
        "noted_meeting_edit_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();
    let meeting_id =
        store::create_meeting(&conn, "Original title", None, None, "2026-07-31T12:00:00Z").unwrap();
    store::set_notes(&conn, meeting_id, "My verbatim note").unwrap();
    save(
        &mut conn,
        "meetings",
        "generated meeting note",
        json!({"meeting_id": meeting_id, "title": "Original title"}),
        "2026-07-31T12:30:00Z",
    );
    let note_id: i64 = conn
        .query_row("SELECT id FROM notes ORDER BY id DESC LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    store::set_note_id(&conn, meeting_id, note_id).unwrap();
    let summary_id = store::insert_summary(
        &conn,
        meeting_id,
        store::DEFAULT_TEMPLATE,
        "## Summary\n\nOriginal summary.",
        "2026-07-31T12:31:00Z",
    )
    .unwrap();

    assert_eq!(
        store::set_title(&conn, meeting_id, "Weekly product review").unwrap(),
        Some(note_id)
    );
    let refreshed = store::set_summary_content(
        &conn,
        meeting_id,
        summary_id,
        "## Summary\n\nEdited by me.",
        store::DEFAULT_TEMPLATE,
    )
    .unwrap()
    .unwrap();
    assert_eq!(refreshed.0, note_id);
    assert!(refreshed.1.contains("Edited by me."));
    assert!(refreshed.1.contains("My verbatim note"));

    let detail = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(detail["title"], "Weekly product review");
    assert_eq!(
        detail["summaries"][0]["content_md"],
        "## Summary\n\nEdited by me."
    );
    let note = db::list_notes(&conn).unwrap().pop().unwrap();
    assert_eq!(note.title, "Weekly product review");
    assert!(note.raw_text.contains("Edited by me."));
    assert!(note.raw_text.contains("My verbatim note"));

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn transcript_search_is_indexed_live_and_returns_each_matching_line() {
    use tauri_app_lib::meeting::store;

    let tmp = std::env::temp_dir().join(format!(
        "noted_transcript_search_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).unwrap();
    let meeting_id =
        store::create_meeting(&conn, "Fundraising review", None, None, "2026-07-31T14:00:00Z")
            .unwrap();
    store::set_status(&conn, meeting_id, "done").unwrap();
    let first = store::insert_segment(
        &conn,
        meeting_id,
        "me",
        2_000,
        5_000,
        "The investor asked about retention.",
    )
    .unwrap();
    store::insert_segment(
        &conn,
        meeting_id,
        "them",
        8_000,
        12_000,
        "Our investors want the updated model next week.",
    )
    .unwrap();
    store::insert_segment(
        &conn,
        meeting_id,
        "them",
        13_000,
        15_000,
        "The product demo is ready.",
    )
    .unwrap();

    let hits = store::search_transcripts(&conn, "invest", 200).unwrap();
    assert_eq!(hits.len(), 2, "prefix search returns every matching transcript line");
    assert_eq!(hits[0].meeting_title, "Fundraising review");
    assert_eq!(hits[0].started_at.as_deref(), Some("2026-07-31T14:00:00Z"));
    assert_eq!(hits[0].speaker, "Me");
    assert_eq!(hits[1].speaker, "Them");

    store::delete_segment(&conn, first).unwrap();
    let after_delete = store::search_transcripts(&conn, "investor", 200).unwrap();
    assert_eq!(after_delete.len(), 1, "the delete trigger removes stale FTS rows");

    store::trash_meeting(&conn, meeting_id, "2026-07-31T15:00:00Z").unwrap();
    assert!(
        store::search_transcripts(&conn, "investor", 200).unwrap().is_empty(),
        "trashed meetings stay out of global transcript search"
    );

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn transcript_vocabulary_corrects_existing_and_future_lines_with_safe_undo() {
    use tauri_app_lib::meeting::store;

    let tmp = std::env::temp_dir().join(format!(
        "noted_transcript_vocabulary_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();
    let meeting_id =
        store::create_meeting(&conn, "Company review", None, None, "2026-08-03T14:00:00Z")
            .unwrap();
    store::set_status(&conn, meeting_id, "done").unwrap();
    let segment_id = store::insert_segment(
        &conn,
        meeting_id,
        "them",
        1_000,
        4_000,
        "Borrow said BORROW, but we never borrowed the deck.",
    )
    .unwrap();

    let preview = store::preview_transcript_vocabulary(&conn, "borrow").unwrap();
    assert_eq!(preview.matching_segments, 1);
    assert_eq!(preview.occurrences, 2, "whole-word matching excludes borrowed");

    let applied = store::apply_transcript_vocabulary(
        &mut conn,
        "borrow",
        "BARO",
        "2026-08-03T14:05:00Z",
    )
    .unwrap();
    assert_eq!(applied.changed_segments, 1);
    assert_eq!(applied.changed_occurrences, 2);
    let corrected: String = conn
        .query_row(
            "SELECT text FROM meeting_segments WHERE id = ?1",
            [segment_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(corrected, "BARO said BARO, but we never borrowed the deck.");
    assert_eq!(store::search_transcripts(&conn, "baro", 200).unwrap().len(), 1);

    let undone = store::undo_transcript_vocabulary(
        &mut conn,
        applied.batch_id.unwrap(),
        "2026-08-03T14:06:00Z",
    )
    .unwrap();
    assert_eq!(undone.restored_segments, 1);
    assert_eq!(undone.skipped_segments, 0);
    assert!(store::list_transcript_vocabulary(&conn).unwrap().is_empty());
    let restored: String = conn
        .query_row(
            "SELECT text FROM meeting_segments WHERE id = ?1",
            [segment_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(restored, "Borrow said BORROW, but we never borrowed the deck.");

    store::apply_transcript_vocabulary(
        &mut conn,
        "borrow",
        "BARO",
        "2026-08-03T14:07:00Z",
    )
    .unwrap();
    let future = store::insert_segment(
        &conn,
        meeting_id,
        "me",
        5_000,
        8_000,
        "BORROW is presenting next.",
    )
    .unwrap();
    let future_text: String = conn
        .query_row(
            "SELECT text FROM meeting_segments WHERE id = ?1",
            [future],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(future_text, "BARO is presenting next.");

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn transcript_filters_are_dynamic_and_one_on_ones_use_the_attendee_name() {
    use tauri_app_lib::meeting::{diarize, store};

    let tmp = std::env::temp_dir().join(format!(
        "noted_transcript_facets_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();
    let one_on_one_event = json!({
        "attendees": [
            {"email":"edison@heybaro.com","self":true},
            {"email":"brian@heybaro.com","self":false}
        ]
    })
    .to_string();
    let one_on_one = store::create_meeting(
        &conn,
        "Brian/Edison",
        None,
        Some(&one_on_one_event),
        "2026-08-03T15:00:00Z",
    )
    .unwrap();
    store::set_status(&conn, one_on_one, "done").unwrap();
    let brian_a = store::insert_segment(
        &conn,
        one_on_one,
        "them",
        1_000,
        3_000,
        "The investor update is ready.",
    )
    .unwrap();
    let brian_b = store::insert_segment(
        &conn,
        one_on_one,
        "them",
        4_000,
        6_000,
        "The investor asked about timing.",
    )
    .unwrap();
    store::set_segment_speakers(
        &conn,
        &[(brian_a, "Speaker 1".into()), (brian_b, "Speaker 2".into())],
    )
    .unwrap();
    store::save_meeting_speakers(
        &conn,
        one_on_one,
        &[
            ("Speaker 1".into(), vec![1.0, 0.0], 8),
            ("Speaker 2".into(), vec![0.0, 1.0], 2),
        ],
    )
    .unwrap();

    let standup_event = json!({
        "attendees": [
            {"email":"edison@heybaro.com","self":true},
            {"email":"brian@heybaro.com","self":false},
            {"email":"max@heybaro.com","self":false}
        ]
    })
    .to_string();
    let standup = store::create_meeting(
        &conn,
        "Daily Stand Up",
        None,
        Some(&standup_event),
        "2026-08-03T16:00:00Z",
    )
    .unwrap();
    store::set_status(&conn, standup, "done").unwrap();
    store::insert_segment(
        &conn,
        standup,
        "them",
        1_000,
        3_000,
        "The investor pipeline moved forward.",
    )
    .unwrap();
    save(
        &mut conn,
        "meetings",
        "generated meeting note",
        json!({"meeting_id": standup, "title": "Daily Stand Up"}),
        "2026-08-03T16:05:00Z",
    );
    let standup_note: i64 = conn
        .query_row("SELECT id FROM notes ORDER BY id DESC LIMIT 1", [], |row| row.get(0))
        .unwrap();
    conn.execute(
        "UPDATE notes SET raw_text = 'Daily standup meeting notes' WHERE id = ?1",
        [standup_note],
    )
    .unwrap();
    store::set_note_id(&conn, standup, standup_note).unwrap();

    conn.execute(
        "DELETE FROM app_metadata WHERE key = 'meeting_one_on_one_speakers_v1'",
        [],
    )
    .unwrap();
    store::initialize_one_on_one_speakers(&conn).unwrap();
    let labels: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT speaker FROM meeting_segments
             WHERE meeting_id = ?1 AND channel = 'them' ORDER BY speaker",
        )
        .unwrap()
        .query_map([one_on_one], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(labels, vec!["Brian"]);
    let merged: (i64, Vec<f32>) = conn
        .query_row(
            "SELECT seg_count, centroid FROM meeting_speakers
             WHERE meeting_id = ?1 AND label = 'Brian'",
            [one_on_one],
            |row| {
                Ok((
                    row.get(0)?,
                    diarize::blob_to_emb(&row.get::<_, Vec<u8>>(1)?),
                ))
            },
        )
        .unwrap();
    assert_eq!(merged.0, 10);
    assert!((merged.1[0] - 0.8).abs() < 1e-6);

    let facets = store::transcript_search_facets(&conn).unwrap();
    assert!(facets.people.iter().any(|value| value.label == "Brian" && value.count == 2));
    assert!(facets.people.iter().any(|value| value.label == "Max" && value.count == 1));
    assert!(facets.meeting_types.iter().any(|value| value.value == "one_on_one"));
    assert!(facets.meeting_types.iter().any(|value| value.value == "daily_standup"));
    let baro = facets
        .folders
        .iter()
        .find(|value| value.label == "Baro")
        .expect("parent company folder includes its stand-up child");

    let brian_hits = store::search_transcripts_filtered(
        &conn,
        "investor",
        200,
        &store::TranscriptSearchFilters {
            people: vec!["Brian".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(brian_hits.len(), 3);
    let max_hits = store::search_transcripts_filtered(
        &conn,
        "investor",
        200,
        &store::TranscriptSearchFilters {
            people: vec!["Max".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(max_hits.len(), 1);
    assert_eq!(max_hits[0].meeting_id, standup);
    let folder_hits = store::search_transcripts_filtered(
        &conn,
        "investor",
        200,
        &store::TranscriptSearchFilters {
            folder_ids: vec![baro.value.parse().unwrap()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(folder_hits.len(), 1);
    assert_eq!(folder_hits[0].meeting_id, standup);

    let oldest_hits = store::search_transcripts_filtered_sorted(
        &conn,
        "investor",
        200,
        &store::TranscriptSearchFilters::default(),
        "date_asc",
    )
    .unwrap();
    assert_eq!(oldest_hits.first().unwrap().meeting_id, one_on_one);

    let title_hits = store::search_transcripts_filtered_sorted(
        &conn,
        "investor",
        200,
        &store::TranscriptSearchFilters::default(),
        "title_asc",
    )
    .unwrap();
    assert_eq!(title_hits.first().unwrap().meeting_id, one_on_one);
    assert_eq!(title_hits.last().unwrap().meeting_id, standup);

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn embedding_space_replacement_is_atomic_and_fingerprinted() {
    let tmp = std::env::temp_dir().join(format!("noted_embedding_swap_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();
    save(&mut conn, "work", "", json!({"topic":"routing"}), "2026-06-02T00:00:00Z",);
    let note_id: i64 = conn.query_row("SELECT id FROM notes LIMIT 1", [], |r| r.get(0)).unwrap();

    db::replace_embedding_space(&mut conn, "openai|a|768", &[(note_id, vec![0.1; 768])], &[]).unwrap();
    assert_eq!(db::embedding_fingerprint(&conn).unwrap().as_deref(), Some("openai|a|768"));
    assert_eq!(db::embedding_count(&conn).unwrap(), 1);

    // sqlite-vec rejects the wrong dimension after DELETE has run inside the
    // transaction. The rollback must preserve both the old index and marker.
    assert!(db::replace_embedding_space(&mut conn, "openai|bad|768", &[(note_id, vec![0.1; 767])], &[]).is_err());
    assert_eq!(db::embedding_fingerprint(&conn).unwrap().as_deref(), Some("openai|a|768"));
    assert_eq!(db::embedding_count(&conn).unwrap(), 1);

    db::replace_embedding_space(&mut conn, "gemini|b|768", &[(note_id, vec![0.2; 768])], &[]).unwrap();
    assert_eq!(db::embedding_fingerprint(&conn).unwrap().as_deref(), Some("gemini|b|768"));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn suggest_merges_finds_near_duplicates_and_respects_dismissals() {
    let tmp = std::env::temp_dir().join(format!("noted_merge_test_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).expect("db init");
    let now = "2026-06-01T00:00:00Z";

    // Two near-identical people + one unrelated place.
    let a = db::create_entity(&conn, "Sarah", "sarah", "person", "[]", "2026-06-01", now).unwrap();
    let b = db::create_entity(&conn, "Sara", "sara", "person", "[]", "2026-06-01", now).unwrap();
    let c = db::create_entity(&conn, "Gym", "gym", "place", "[]", "2026-06-01", now).unwrap();

    // Hand-built embeddings: a≈b (cosine ~0.98), c orthogonal.
    let mut va = vec![0.0f32; 768];
    va[0] = 1.0;
    let mut vb = vec![0.0f32; 768];
    vb[0] = 0.98;
    vb[1] = 0.2;
    let mut vc = vec![0.0f32; 768];
    vc[1] = 1.0;
    db::insert_entity_embedding(&conn, a, &va).unwrap();
    db::insert_entity_embedding(&conn, b, &vb).unwrap();
    db::insert_entity_embedding(&conn, c, &vc).unwrap();

    let sugg = db::suggest_merges(&conn, 0.82, 20).unwrap();
    assert_eq!(sugg.len(), 1, "only the near-identical same-type pair should surface");
    let pair = (sugg[0].a_id.min(sugg[0].b_id), sugg[0].a_id.max(sugg[0].b_id),);
    assert_eq!(pair, (a.min(b), a.max(b)));
    assert!(sugg[0].similarity > 0.9, "cosine should be ~0.98, got {}", sugg[0].similarity);
    assert_eq!(sugg[0].etype, "person");

    // Dismiss (ids deliberately reversed — pair key is order-normalized) → gone.
    db::dismiss_merge(&conn, b, a).unwrap();
    let sugg2 = db::suggest_merges(&conn, 0.82, 20).unwrap();
    assert!(sugg2.is_empty(), "dismissed pair must never resurface");

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn rename_speaker_onto_existing_label_merges_rows() {
    use tauri_app_lib::meeting::{diarize, store};
    let tmp = std::env::temp_dir().join(format!("noted_rename_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).unwrap();
    let id = store::create_meeting(&conn, "Standup", None, None, "2026-07-17T15:00:00Z").unwrap();
    let s1 = store::insert_segment(&conn, id, "them", 0, 5_000, "hello").unwrap();
    let s2 = store::insert_segment(&conn, id, "them", 6_000, 9_000, "yes").unwrap();
    store::set_segment_speakers(&conn, &[(s1, "Brian".into()), (s2, "Speaker 2".into())]).unwrap();
    // Brian's real cluster (10 segments at 1.0) + a small mislabelable one (5 at 0.0).
    store::save_meeting_speakers(
        &conn,
        id,
        &[
            ("Brian".into(), vec![1.0, 0.0], 10),
            ("Speaker 2".into(), vec![0.0, 1.0], 5),
        ],
    )
    .unwrap();

    // The bug: this used to UPDATE OR REPLACE, deleting Brian's real row.
    store::rename_speaker(&conn, id, "Speaker 2", "Brian").unwrap();

    let rows = store::list_meeting_speakers(&conn, id).unwrap();
    assert_eq!(rows.len(), 1, "one merged row, not a vanished one: {rows:?}");
    assert_eq!(rows[0]["label"], "Brian");
    assert_eq!(rows[0]["seg_count"], 15);
    // Weighted centroid: (1.0*10 + 0.0*5)/15, (0.0*10 + 1.0*5)/15
    let cent: Vec<f32> = conn
        .query_row(
            "SELECT centroid FROM meeting_speakers WHERE meeting_id = ?1 AND label = 'Brian'",
            [id],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .map(|b| diarize::blob_to_emb(&b))
        .unwrap();
    assert!((cent[0] - 10.0 / 15.0).abs() < 1e-6 && (cent[1] - 5.0 / 15.0).abs() < 1e-6, "{cent:?}");
    // Both segments carry the merged name.
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM meeting_segments WHERE meeting_id = ?1 AND speaker = 'Brian'",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);
    // Plain rename (no existing target) still works.
    store::save_meeting_speakers(&conn, id, &[("Speaker 9".into(), vec![0.5, 0.5], 3)]).unwrap();
    store::rename_speaker(&conn, id, "Speaker 9", "Vivian").unwrap();
    let labels: Vec<String> = store::list_meeting_speakers(&conn, id)
        .unwrap()
        .iter()
        .map(|r| r["label"].as_str().unwrap().to_string())
        .collect();
    assert!(labels.contains(&"Vivian".to_string()) && labels.contains(&"Brian".to_string()));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn failed_calendar_meeting_does_not_block_retry() {
    use tauri_app_lib::meeting::store;

    let tmp = std::env::temp_dir().join(format!("noted_meeting_retry_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).unwrap();
    let failed = store::create_meeting(
        &conn,
        "Standup",
        Some("calendar-event"),
        None,
        "2026-07-21T15:00:00Z",
    )
    .unwrap();
    store::set_status(&conn, failed, "failed").unwrap();
    assert_eq!(store::find_meeting_by_event(&conn, "calendar-event").unwrap(), None);

    let retry = store::create_meeting(
        &conn,
        "Standup",
        Some("calendar-event"),
        None,
        "2026-07-21T15:01:00Z",
    )
    .unwrap();
    assert_eq!(
        store::find_meeting_by_event(&conn, "calendar-event").unwrap(),
        Some(retry)
    );
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn meeting_trash_is_reversible_and_required_before_delete() {
    use tauri_app_lib::meeting::store;

    let tmp = std::env::temp_dir().join(format!("noted_meeting_trash_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::init(&tmp).unwrap();
    let id = store::create_meeting(&conn, "Important call", None, None, "2026-07-21T16:00:00Z").unwrap();
    store::set_status(&conn, id, "done").unwrap();
    save(
        &mut conn,
        "meetings",
        "generated meeting note",
        json!({"meeting_id": id}),
        "2026-07-21T16:30:00Z",
    );
    let note_id: i64 = conn.query_row("SELECT id FROM notes ORDER BY id DESC LIMIT 1", [], |r| { r.get(0)
        }).unwrap();
    store::set_note_id(&conn, id, note_id).unwrap();
    db::insert_embedding(&conn, note_id, &vec![0.1; 768]).unwrap();
    db::refresh_note_text(&conn, note_id, "# Important call\n\nCorrected notes").unwrap();
    let refreshed: String = conn.query_row("SELECT raw_text FROM notes WHERE id = ?1", [note_id], |r| { r.get(0)
        }).unwrap();
    assert_eq!(refreshed, "# Important call\n\nCorrected notes");
    assert_eq!(db::embedding_count(&conn).unwrap(), 0, "stale semantic index is removed");
    db::insert_embedding(&conn, note_id, &vec![0.2; 768]).unwrap();

    assert_eq!(store::list_meetings(&conn, 20).unwrap().len(), 1);
    assert!(store::list_trashed_meetings(&conn, 20).unwrap().is_empty());
    assert!(!store::delete_meeting_forever(&mut conn, id).unwrap(), "visible meetings cannot be permanently deleted");

    assert!(store::trash_meeting(&conn, id, "2026-07-21T17:00:00Z").unwrap());
    assert!(store::list_meetings(&conn, 20).unwrap().is_empty());
    assert_eq!(store::list_trashed_meetings(&conn, 20).unwrap().len(), 1);

    assert!(store::restore_meeting(&conn, id).unwrap());
    assert_eq!(store::list_meetings(&conn, 20).unwrap().len(), 1);

    assert!(store::trash_meeting(&conn, id, "2026-07-21T18:00:00Z").unwrap());
    assert!(store::delete_meeting_forever(&mut conn, id).unwrap());
    assert!(store::list_trashed_meetings(&conn, 20).unwrap().is_empty());
    let exists: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?1)", [id], |r| r.get(0),)
        .unwrap();
    assert!(!exists);
    let note_exists: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM notes WHERE id = ?1)", [note_id], |r| r.get(0),)
        .unwrap();
    assert!(!note_exists, "the generated note is deleted with its meeting");
    assert_eq!(db::embedding_count(&conn).unwrap(), 0);

    let _ = std::fs::remove_file(&tmp);
}
