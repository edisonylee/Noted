use serde_json::json;
use tauri_app_lib::{db, meeting::store};

fn test_db(name: &str) -> (std::path::PathBuf, rusqlite::Connection) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noted_meeting_filing_{name}_{}_{}.db",
        std::process::id(),
        nonce
    ));
    let conn = db::init(&path).unwrap();
    let work = folder_id(&conn, "Work");
    let personal = folder_id(&conn, "Personal");
    let baro = create_folder(&conn, work, "Baro");
    db::create_note_folder(
        &conn,
        Some(baro),
        "Daily Standup Meeting Notes",
        "folder",
        "daily_standup",
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    create_folder(&conn, personal, "Health");
    (path, conn)
}

fn folder_id(conn: &rusqlite::Connection, name: &str) -> i64 {
    conn.query_row(
        "SELECT id FROM note_folders WHERE name = ?1 COLLATE NOCASE",
        [name],
        |row| row.get(0),
    )
    .unwrap()
}

fn note(conn: &rusqlite::Connection, title: &str) -> i64 {
    conn.execute(
        "INSERT INTO notes (title, raw_text, source, created_at, origin)
         VALUES (?1, ?1, 'meeting', '2026-08-06T12:00:00Z', 'capture')",
        [title],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn create_folder(conn: &rusqlite::Connection, parent: i64, name: &str) -> i64 {
    db::create_note_folder(
        conn,
        Some(parent),
        name,
        "folder",
        "",
        "2026-08-06T12:00:00Z",
    )
    .unwrap()
}

fn broad_rule_filing(conn: &rusqlite::Connection, note_id: i64, folder_id: i64) {
    conn.execute(
        "UPDATE notes SET filing_context = 'work' WHERE id = ?1",
        [note_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO note_folder_items
           (folder_id, note_id, source, reason, created_at)
         VALUES (?1, ?2, 'rule', 'Broad account route.', '2026-08-06T12:01:00Z')",
        rusqlite::params![folder_id, note_id],
    )
    .unwrap();
}

fn work_event(attendees: serde_json::Value) -> serde_json::Value {
    json!({
        "account": "edison@heybaro.com",
        "attendees": attendees
    })
}

#[test]
fn priority_routes_exact_normalized_identity_and_owner_aliases_are_not_attendees() {
    let (path, conn) = test_db("priority");
    let baro = folder_id(&conn, "Baro");
    let personal = folder_id(&conn, "Personal");
    store::set_meeting_filing_rule(
        &conn,
        " Personal@Example.com ",
        personal,
        Some(1),
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    store::set_meeting_filing_rule(
        &conn,
        "EDISON@HEYBARO.COM",
        baro,
        Some(0),
        "2026-08-06T12:00:01Z",
    )
    .unwrap();

    let rules = store::meeting_filing_rules(&conn).unwrap();
    assert_eq!(rules[0].email, "edison@heybaro.com");
    assert_eq!(rules[0].folder_path.as_deref(), Some("Work / Baro"));
    assert_eq!(rules[1].email, "personal@example.com");

    let event = json!({
        "account": "personal@example.com",
        "organizer_email": "edison@heybaro.com",
        "creator_email": "assistant@vendor.com",
        "attendee_emails": [
            "personal@example.com",
            "edison@heybaro.com",
            "zach@example.com"
        ],
        "attendees": [
            {"email":"personal@example.com","name":"Personal Edison","self":true},
            {"email":"edison@heybaro.com","name":"Work Edison"},
            {"email":"zach@example.com","name":"Zach Rossman"}
        ]
    });
    let meeting_id = store::create_meeting(
        &conn,
        "Zach / Edison",
        Some("event-1"),
        Some(&event.to_string()),
        "2026-08-06T12:00:02Z",
    )
    .unwrap();
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["route_status"], "matched");
    assert_eq!(meeting["route_folder_id"], baro);
    assert_eq!(meeting["route_email"], "edison@heybaro.com");
    assert_eq!(meeting["route_via"], "organizer");
    assert_eq!(meeting["meeting_type"], "one_on_one");
    assert_eq!(
        store::external_attendees_for_event(&conn, &event),
        vec!["Zach Rossman"]
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn broad_account_route_refines_to_named_person_and_company_folders() {
    let (path, conn) = test_db("specific_participant_folders");
    let baro = folder_id(&conn, "Baro");
    let one_on_ones = create_folder(&conn, baro, "One-on-ones");
    let brian = create_folder(&conn, one_on_ones, "Brian");
    let partners = create_folder(&conn, baro, "Partner Meetings");
    let tonik = create_folder(&conn, partners, "Tonik - Design Partner");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();

    for (title, attendee, expected) in [
        ("Brian / Edison", "brian@heybaro.com", brian),
        ("Baro x Tonik", "maria@tonik.com", tonik),
    ] {
        let event = work_event(json!([
            {"email":"edison@heybaro.com","self":true},
            {"email":attendee,"self":false}
        ]));
        let meeting_id = store::create_meeting(
            &conn,
            title,
            None,
            Some(&event.to_string()),
            "2026-08-06T12:01:00Z",
        )
        .unwrap();
        let note_id = note(&conn, title);
        broad_rule_filing(&conn, note_id, baro);
        store::set_note_id_and_apply_route(&conn, meeting_id, note_id, "2026-08-06T12:02:00Z")
            .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
                [note_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            expected
        );
    }

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn broad_account_route_learns_company_destination_from_prior_filing() {
    let (path, conn) = test_db("learned_company_folder");
    let baro = folder_id(&conn, "Baro");
    let partners = create_folder(&conn, baro, "Partner Meetings");
    let design_partner = create_folder(&conn, partners, "Brand research");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();

    let prior_event = work_event(json!([
        {"email":"edison@heybaro.com","self":true},
        {"email":"maria@tonik.com","self":false}
    ]));
    let prior_meeting = store::create_meeting(
        &conn,
        "Prior partner call",
        None,
        Some(&prior_event.to_string()),
        "2026-08-05T12:00:00Z",
    )
    .unwrap();
    let prior_note = note(&conn, "Prior partner call");
    store::set_note_id(&conn, prior_meeting, prior_note).unwrap();
    db::file_note(
        &conn,
        prior_note,
        Some(design_partner),
        "2026-08-05T12:01:00Z",
    )
    .unwrap();

    let current_event = work_event(json!([
        {"email":"edison@heybaro.com","self":true},
        {"email":"franek@tonik.com","self":false},
        {"email":"maria@tonik.com","self":false}
    ]));
    let meeting_id = store::create_meeting(
        &conn,
        "Another partner call",
        None,
        Some(&current_event.to_string()),
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let note_id = note(&conn, "Another partner call");
    broad_rule_filing(&conn, note_id, baro);
    store::set_note_id_and_apply_route(&conn, meeting_id, note_id, "2026-08-06T12:02:00Z").unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        design_partner
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn broad_account_route_uses_semantic_fallbacks_without_misfiling_internal_groups() {
    let (path, conn) = test_db("semantic_fallbacks");
    let baro = folder_id(&conn, "Baro");
    let one_on_ones = create_folder(&conn, baro, "One-on-ones");
    let partners = create_folder(&conn, baro, "Partner Meetings");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();

    let cases = [
        (
            "New one-on-one",
            json!([
                {"email":"edison@heybaro.com","self":true},
                {"email":"limayk@stanford.edu","self":false}
            ]),
            one_on_ones,
        ),
        (
            "New external group",
            json!([
                {"email":"edison@heybaro.com","self":true},
                {"email":"max@heybaro.com","self":false},
                {"email":"luca@skarlo.co","self":false}
            ]),
            partners,
        ),
        (
            "Internal group",
            json!([
                {"email":"edison@heybaro.com","self":true},
                {"email":"max@heybaro.com","self":false},
                {"email":"brian@heybaro.com","self":false}
            ]),
            baro,
        ),
    ];
    for (title, attendees, expected) in cases {
        let event = work_event(attendees);
        let meeting_id = store::create_meeting(
            &conn,
            title,
            None,
            Some(&event.to_string()),
            "2026-08-06T12:01:00Z",
        )
        .unwrap();
        let note_id = note(&conn, title);
        broad_rule_filing(&conn, note_id, baro);
        store::set_note_id_and_apply_route(&conn, meeting_id, note_id, "2026-08-06T12:02:00Z")
            .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
                [note_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            expected,
            "{title}"
        );
    }

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn backfill_previews_broad_rule_filing_against_specific_destination() {
    let (path, conn) = test_db("specific_backfill");
    let baro = folder_id(&conn, "Baro");
    let one_on_ones = create_folder(&conn, baro, "One-on-ones");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = work_event(json!([
        {"email":"edison@heybaro.com","self":true},
        {"email":"new.person@gmail.com","self":false}
    ]));
    let meeting_id = store::create_meeting(
        &conn,
        "Historical one-on-one",
        None,
        Some(&event.to_string()),
        "2026-07-01T12:00:00Z",
    )
    .unwrap();
    let note_id = note(&conn, "Historical one-on-one");
    broad_rule_filing(&conn, note_id, baro);
    store::set_note_id(&conn, meeting_id, note_id).unwrap();

    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();
    let item = preview
        .items
        .iter()
        .find(|item| item.meeting_id == meeting_id)
        .expect("broad filing should be eligible for refinement");
    assert_eq!(item.folder_id, Some(one_on_ones));
    let report =
        store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:03:00Z")
            .unwrap();
    assert_eq!(report.filed, 1);
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        one_on_ones
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn explicit_recording_context_suppresses_only_conflicting_account_routes_atomically() {
    let (path, conn) = test_db("explicit_context");
    let baro = folder_id(&conn, "Baro");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = json!({"account":"edison@heybaro.com","attendees":[]});

    let conflicting =
        store::resolve_meeting_filing(&conn, Some(&event), Some(" Personal ")).unwrap();
    assert_eq!(conflicting.filing_context.as_deref(), Some("personal"));
    assert_eq!(conflicting.route.folder_id, None);
    assert_eq!(
        conflicting.route.email.as_deref(),
        Some("edison@heybaro.com")
    );
    assert_eq!(conflicting.route.via, "context_override");
    assert_eq!(conflicting.route.status, "needs_filing");

    let personal_meeting = store::create_meeting_with_asr_in_context(
        &conn,
        "Personal recording of a work-account event",
        Some("event-personal"),
        Some(&event.to_string()),
        "whisper",
        "test-model",
        Some("Personal"),
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    let meeting = store::get_meeting(&conn, personal_meeting).unwrap();
    assert_eq!(meeting["filing_context"], "personal");
    assert!(meeting["route_folder_id"].is_null());
    assert_eq!(meeting["route_email"], "edison@heybaro.com");
    assert_eq!(meeting["route_via"], "context_override");
    assert_eq!(meeting["route_status"], "needs_filing");

    let same_context = store::resolve_meeting_filing(&conn, Some(&event), Some("WORK")).unwrap();
    assert_eq!(same_context.filing_context.as_deref(), Some("work"));
    assert_eq!(same_context.route.folder_id, Some(baro));
    assert_eq!(same_context.route.via, "source_account");
    assert_eq!(same_context.route.status, "matched");

    let work_meeting = store::create_meeting_with_asr_in_context(
        &conn,
        "Work recording of a work-account event",
        Some("event-work"),
        Some(&event.to_string()),
        "whisper",
        "test-model",
        Some("work"),
        "2026-08-06T12:02:00Z",
    )
    .unwrap();
    let meeting = store::get_meeting(&conn, work_meeting).unwrap();
    assert_eq!(meeting["filing_context"], "work");
    assert_eq!(meeting["route_folder_id"], baro);
    assert_eq!(meeting["route_via"], "source_account");
    assert_eq!(meeting["route_status"], "matched");

    let count_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM meetings", [], |row| row.get(0))
        .unwrap();
    let error = store::create_meeting_with_asr_in_context(
        &conn,
        "Invalid context",
        None,
        None,
        "whisper",
        "test-model",
        Some("shared"),
        "2026-08-06T12:03:00Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains("work or personal"));
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM meetings", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        count_before
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn explicit_context_without_an_initial_rule_blocks_later_cross_context_backfill() {
    let (path, mut conn) = test_db("late_context_rule");
    let personal = folder_id(&conn, "Personal");
    let health = folder_id(&conn, "Health");
    let baro = folder_id(&conn, "Baro");
    let event = json!({"account":"edison@heybaro.com","attendees":[]});
    let meeting_id = store::create_meeting_with_asr_in_context(
        &conn,
        "Personal project discussion",
        Some("event-late-rule"),
        Some(&event.to_string()),
        "whisper",
        "test-model",
        Some("personal"),
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["filing_context"], "personal");
    assert_eq!(meeting["route_status"], "needs_filing");
    assert_eq!(
        meeting["event_json"]["_noted_recording_filing_context_v1"],
        "personal"
    );

    let note_id = db::save_note_with_initial_filing(
        &mut conn,
        db::SaveInput {
            raw_text: "Personal project discussion".into(),
            source: "meeting".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![db::EntryInput {
                category: "projects".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "personal",
        None,
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    store::set_note_id_and_apply_route(&conn, meeting_id, note_id, "2026-08-06T12:02:00Z").unwrap();

    // A Work rule learned after recording must not cross the user's captured
    // Personal boundary.
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:03:00Z",
    )
    .unwrap();
    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();
    assert_eq!(preview.eligible, 1);
    assert_eq!(preview.would_file, 0);
    assert_eq!(preview.needs_filing, 1);
    assert_eq!(preview.items[0].folder_id, None);
    let report =
        store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:04:00Z")
            .unwrap();
    assert_eq!(report.filed, 0);
    assert_eq!(report.needs_filing, 1);
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        personal
    );

    // Repointing the same account rule inside Personal is a valid refinement.
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        health,
        None,
        "2026-08-06T12:05:00Z",
    )
    .unwrap();
    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();
    assert_eq!(preview.eligible, 1);
    assert_eq!(preview.would_file, 1);
    assert_eq!(preview.needs_filing, 0);
    assert_eq!(preview.items[0].folder_id, Some(health));
    let report =
        store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:06:00Z")
            .unwrap();
    assert_eq!(report.filed, 1);
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        health
    );
    assert_eq!(
        store::get_meeting(&conn, meeting_id).unwrap()["filing_context"],
        "personal"
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn first_summary_files_to_saved_route_and_manual_filing_is_permanent() {
    let (path, conn) = test_db("manual");
    let baro = folder_id(&conn, "Baro");
    let personal = folder_id(&conn, "Personal");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = json!({"account":"edison@heybaro.com","attendees":[]});
    let meeting_id = store::create_meeting(
        &conn,
        "Baro planning",
        Some("event-2"),
        Some(&event.to_string()),
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    let note_id = note(&conn, "Baro planning");
    store::set_note_id_and_apply_route(&conn, meeting_id, note_id, "2026-08-06T12:30:00Z").unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        baro
    );

    db::file_note(&conn, note_id, Some(personal), "2026-08-06T12:31:00Z").unwrap();
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["route_status"], "manual");
    assert_eq!(meeting["route_folder_id"], personal);
    assert_eq!(meeting["route_via"], "manual");

    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();
    assert_eq!(preview.manual, 1);
    assert_eq!(preview.eligible, 0);
    let applied =
        store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T13:00:00Z")
            .unwrap();
    assert_eq!(applied.reviewed, 0);
    assert_eq!(applied.filed, 0);
    assert_eq!(applied.skipped, 0);
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        personal
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn live_destination_is_manual_and_survives_the_first_note_link() {
    let (path, conn) = test_db("live_manual_destination");
    let health = folder_id(&conn, "Health");
    let meeting_id = store::create_meeting(
        &conn,
        "Weekly reflection",
        None,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();

    store::set_filing_destination(&conn, meeting_id, health, "2026-08-06T12:01:00Z").unwrap();
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["route_folder_id"], health);
    assert_eq!(meeting["route_status"], "manual");
    assert_eq!(meeting["route_via"], "manual");
    assert_eq!(meeting["filing_context"], "personal");

    let note_id = note(&conn, "Weekly reflection");
    store::set_note_id(&conn, meeting_id, note_id).unwrap();
    db::file_note(&conn, note_id, Some(health), "2026-08-06T12:02:00Z").unwrap();

    let filing: (i64, String) = conn
        .query_row(
            "SELECT folder_id, source FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(filing, (health, "manual".into()));
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["route_folder_id"], health);
    assert_eq!(meeting["route_status"], "manual");

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn completed_meeting_destination_moves_its_linked_note() {
    let (path, conn) = test_db("completed_manual_destination");
    let baro = folder_id(&conn, "Baro");
    let health = folder_id(&conn, "Health");
    let meeting_id =
        store::create_meeting(&conn, "Planning", None, None, "2026-08-06T12:00:00Z").unwrap();
    let note_id = note(&conn, "Planning");
    store::set_note_id(&conn, meeting_id, note_id).unwrap();
    db::file_note(&conn, note_id, Some(baro), "2026-08-06T12:01:00Z").unwrap();

    store::set_filing_destination(&conn, meeting_id, health, "2026-08-06T12:02:00Z").unwrap();

    let filing: (i64, String) = conn
        .query_row(
            "SELECT folder_id, source FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(filing, (health, "manual".into()));
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["route_folder_id"], health);
    assert_eq!(meeting["filing_context"], "personal");

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn identity_route_supersedes_context_inbox_and_sets_work_provenance() {
    let (path, conn) = test_db("context_to_identity");
    let baro = folder_id(&conn, "Baro");
    let personal = folder_id(&conn, "Personal");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = json!({"account":"edison@heybaro.com","attendees":[]});
    let meeting_id = store::create_meeting(
        &conn,
        "Baro context override",
        Some("event-context"),
        Some(&event.to_string()),
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    assert_eq!(
        store::get_meeting(&conn, meeting_id).unwrap()["filing_context"],
        "work"
    );

    let note_id = note(&conn, "Baro context override");
    conn.execute(
        "UPDATE notes SET filing_context = 'personal' WHERE id = ?1",
        [note_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO note_folder_items
           (folder_id, note_id, source, reason, created_at)
         VALUES (?1, ?2, 'context', 'Captured in Personal.', '2026-08-06T12:02:00Z')",
        rusqlite::params![personal, note_id],
    )
    .unwrap();

    store::set_note_id_and_apply_route(&conn, meeting_id, note_id, "2026-08-06T12:03:00Z").unwrap();
    let filing: (i64, String) = conn
        .query_row(
            "SELECT folder_id, source FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(filing, (baro, "rule".into()));
    assert_eq!(
        conn.query_row(
            "SELECT filing_context FROM notes WHERE id = ?1",
            [note_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "work"
    );
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["filing_context"], "work");
    assert_eq!(meeting["route_folder_id"], baro);

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn undo_of_automatic_route_is_sticky_against_historical_backfill() {
    let (path, mut conn) = test_db("undo_sticky");
    let baro = folder_id(&conn, "Baro");
    let personal = folder_id(&conn, "Personal");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = json!({"account":"edison@heybaro.com","attendees":[]});
    let meeting_id = store::create_meeting(
        &conn,
        "Undo automatic Baro route",
        Some("event-undo"),
        Some(&event.to_string()),
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    let note_id = db::save_note_with_initial_filing(
        &mut conn,
        db::SaveInput {
            raw_text: "Undo automatic Baro route".into(),
            source: "meeting".into(),
            image_path: None,
            event_date: "2026-08-06".into(),
            entries: vec![db::EntryInput {
                category: "projects".into(),
                description: String::new(),
                data: json!({}),
            }],
        },
        "personal",
        None,
        "2026-08-06T12:02:00Z",
    )
    .unwrap();
    store::set_note_id_and_apply_route(&conn, meeting_id, note_id, "2026-08-06T12:03:00Z").unwrap();
    let automatic_event_id: i64 = conn
        .query_row(
            "SELECT event_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        baro
    );

    let undo = db::undo_note_filing(&conn, automatic_event_id, "2026-08-06T12:04:00Z").unwrap();
    assert_eq!(undo.folder_id, Some(personal));
    assert_eq!(undo.source, "undo");
    let restored: (i64, String) = conn
        .query_row(
            "SELECT folder_id, source FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(restored, (personal, "undo".into()));
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["route_status"], "needs_filing");
    assert_eq!(meeting["route_via"], "context_inbox");

    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();
    assert_eq!(preview.manual, 1);
    assert_eq!(preview.eligible, 0);
    let report =
        store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:05:00Z")
            .unwrap();
    assert_eq!(report.reviewed, 0);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.filed, 0);
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        personal
    );
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["route_status"], "needs_filing");
    assert_eq!(meeting["route_via"], "context_inbox");

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn identity_route_preserves_a_more_specific_automatic_subfolder() {
    let (path, conn) = test_db("specific_auto");
    let baro = folder_id(&conn, "Baro");
    let daily = folder_id(&conn, "Daily Standup Meeting Notes");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = json!({"account":"edison@heybaro.com","attendees":[]});
    let meeting_id = store::create_meeting(
        &conn,
        "Daily standup",
        Some("event-standup"),
        Some(&event.to_string()),
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    let note_id = note(&conn, "Daily standup");
    conn.execute(
        "UPDATE notes SET filing_context = 'work' WHERE id = ?1",
        [note_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO note_folder_items
           (folder_id, note_id, source, reason, created_at)
         VALUES (?1, ?2, 'rule', 'Specific meeting rule.', '2026-08-06T12:02:00Z')",
        rusqlite::params![daily, note_id],
    )
    .unwrap();

    store::set_note_id_and_apply_route(&conn, meeting_id, note_id, "2026-08-06T12:03:00Z").unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        daily
    );
    assert_eq!(
        store::get_meeting(&conn, meeting_id).unwrap()["route_folder_id"],
        daily
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM note_filing_events WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn identity_fingerprint_repairs_stale_named_one_on_one_once_per_owner_set() {
    let (path, conn) = test_db("speaker_identity_repair");
    let baro = folder_id(&conn, "Baro");
    let event = json!({
        "attendees": [
            {"email":"primary@example.com","name":"Edison","self":true},
            {"email":"alias@example.com","name":"Work Edison"},
            {"email":"zach@example.com","name":"Zach Rossman"}
        ]
    });
    let meeting_id = store::create_meeting(
        &conn,
        "Zach / Edison",
        None,
        Some(&event.to_string()),
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let first = store::insert_segment(&conn, meeting_id, "them", 0, 1_000, "Hello").unwrap();
    let second = store::insert_segment(&conn, meeting_id, "them", 1_100, 2_000, "Update").unwrap();
    store::set_segment_speakers(
        &conn,
        &[(first, "Brian".into()), (second, "Speaker 2".into())],
    )
    .unwrap();
    store::save_meeting_speakers(&conn, meeting_id, &[("Brian".into(), vec![1.0, 0.0], 4)])
        .unwrap();

    // Learning that the alias is also "me" turns the apparent group call into
    // a true 1:1 and reruns the v2 identity migration automatically.
    store::set_meeting_filing_rule(
        &conn,
        "alias@example.com",
        baro,
        None,
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    let labels: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT speaker FROM meeting_segments
             WHERE meeting_id = ?1 AND channel = 'them' ORDER BY speaker",
        )
        .unwrap()
        .query_map([meeting_id], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(labels, vec!["Zach Rossman"]);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM meeting_speakers WHERE meeting_id = ?1",
            [meeting_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );

    conn.execute(
        "UPDATE meeting_segments SET speaker = 'Manual label' WHERE id = ?1",
        [first],
    )
    .unwrap();
    assert_eq!(store::repair_one_on_one_speakers(&conn).unwrap(), 0);
    assert_eq!(
        conn.query_row(
            "SELECT speaker FROM meeting_segments WHERE id = ?1",
            [first],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "Manual label"
    );

    store::set_meeting_filing_rule(
        &conn,
        "another-owner@example.com",
        baro,
        None,
        "2026-08-06T12:02:00Z",
    )
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT speaker FROM meeting_segments WHERE id = ?1",
            [first],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "Zach Rossman"
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn backfill_is_previewable_and_only_moves_eligible_unfiled_notes() {
    let (path, conn) = test_db("backfill");
    let baro = folder_id(&conn, "Baro");
    let standups = folder_id(&conn, "Daily Standup Meeting Notes");
    let personal = folder_id(&conn, "Personal");
    let event = json!({
        "account":"edison@heybaro.com",
        "attendee_emails":["edison@heybaro.com","zach@example.com"],
        "attendees":[{"email":"zach@example.com","name":"Zach"}]
    });
    let eligible = store::create_meeting(
        &conn,
        "Daily standup: historical Baro call",
        Some("old-1"),
        Some(&event.to_string()),
        "2026-07-01T12:00:00Z",
    )
    .unwrap();
    let eligible_note = note(&conn, "Daily standup: historical Baro call");
    store::set_note_id(&conn, eligible, eligible_note).unwrap();

    let unmatched = store::create_meeting(
        &conn,
        "Offline recording",
        None,
        None,
        "2026-07-02T12:00:00Z",
    )
    .unwrap();
    store::set_note_id(&conn, unmatched, note(&conn, "Offline recording")).unwrap();

    let protected = store::create_meeting(
        &conn,
        "Already organized",
        None,
        Some(&event.to_string()),
        "2026-07-03T12:00:00Z",
    )
    .unwrap();
    let protected_note = note(&conn, "Already organized");
    store::set_note_id(&conn, protected, protected_note).unwrap();
    // A context inbox is only a provisional capture home. Historical identity
    // routing must be allowed to supersede it.
    conn.execute(
        "INSERT INTO note_folder_items
           (folder_id, note_id, source, reason, created_at)
         VALUES (?1, ?2, 'context', 'Captured in Personal.', '2026-07-03T12:10:00Z')",
        rusqlite::params![personal, protected_note],
    )
    .unwrap();
    conn.execute(
        "UPDATE notes SET filing_context = 'personal' WHERE id = ?1",
        [protected_note],
    )
    .unwrap();

    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();
    assert_eq!(preview.eligible, 3);
    assert_eq!(preview.would_file, 2);
    assert_eq!(preview.needs_filing, 1);
    assert_eq!(preview.already_filed, 0);
    assert_eq!(preview.manual, 0);
    assert_eq!(
        preview
            .items
            .iter()
            .find(|item| item.meeting_id == eligible)
            .and_then(|item| item.folder_id),
        Some(standups)
    );

    let report =
        store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:01:00Z")
            .unwrap();
    assert_eq!(report.reviewed, 3);
    assert_eq!(report.filed, 2);
    assert_eq!(report.needs_filing, 1);
    assert_eq!(report.skipped, 0);
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [eligible_note],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        standups
    );
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [protected_note],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        baro
    );
    assert_eq!(
        store::get_meeting(&conn, unmatched).unwrap()["route_status"],
        "needs_filing"
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn backfill_apply_uses_only_the_previewed_batch_and_token_is_one_shot() {
    let (path, conn) = test_db("backfill_exact_batch");
    let baro = folder_id(&conn, "Baro");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = json!({"account":"edison@heybaro.com","attendees":[]});
    let first = store::create_meeting(
        &conn,
        "First historical call",
        None,
        Some(&event.to_string()),
        "2026-07-01T12:00:00Z",
    )
    .unwrap();
    let first_note = note(&conn, "First historical call");
    store::set_note_id(&conn, first, first_note).unwrap();
    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();
    assert_eq!(preview.items.len(), 1);
    assert!(!preview.token.is_empty());

    // This meeting did not exist in the reviewed snapshot and must not be
    // picked up by apply even though it is eligible by then.
    let second = store::create_meeting(
        &conn,
        "Second historical call",
        None,
        Some(&event.to_string()),
        "2026-07-02T12:00:00Z",
    )
    .unwrap();
    let second_note = note(&conn, "Second historical call");
    store::set_note_id(&conn, second, second_note).unwrap();

    let report =
        store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:01:00Z")
            .unwrap();
    assert_eq!(report.reviewed, 1);
    assert_eq!(report.filed, 1);
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [first_note],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        baro
    );
    assert!(!conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM note_folder_items WHERE note_id = ?1)",
            [second_note],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());

    let error = store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:02:00Z")
        .unwrap_err();
    assert!(error.to_string().contains("expired"));
    let next = store::meeting_filing_backfill_preview(&conn).unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].meeting_id, second);

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn backfill_apply_rejects_a_stale_route_without_writing() {
    let (path, conn) = test_db("backfill_stale_route");
    let baro = folder_id(&conn, "Baro");
    let personal = folder_id(&conn, "Personal");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = json!({"account":"edison@heybaro.com","attendees":[]});
    let meeting_id = store::create_meeting(
        &conn,
        "Historical route change",
        None,
        Some(&event.to_string()),
        "2026-07-01T12:00:00Z",
    )
    .unwrap();
    let note_id = note(&conn, "Historical route change");
    store::set_note_id(&conn, meeting_id, note_id).unwrap();
    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();
    assert_eq!(preview.items[0].folder_id, Some(baro));

    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        personal,
        None,
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    let error = store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:02:00Z")
        .unwrap_err();
    assert!(error.to_string().contains("stale"));
    assert!(!conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM note_folder_items WHERE note_id = ?1)",
            [note_id],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn backfill_apply_rejects_a_new_manual_filing_and_preserves_it() {
    let (path, conn) = test_db("backfill_stale_manual");
    let baro = folder_id(&conn, "Baro");
    let personal = folder_id(&conn, "Personal");
    store::set_meeting_filing_rule(
        &conn,
        "edison@heybaro.com",
        baro,
        None,
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let event = json!({"account":"edison@heybaro.com","attendees":[]});
    let meeting_id = store::create_meeting(
        &conn,
        "Historical manual move",
        None,
        Some(&event.to_string()),
        "2026-07-01T12:00:00Z",
    )
    .unwrap();
    let note_id = note(&conn, "Historical manual move");
    store::set_note_id(&conn, meeting_id, note_id).unwrap();
    let preview = store::meeting_filing_backfill_preview(&conn).unwrap();

    db::file_note(&conn, note_id, Some(personal), "2026-08-06T12:01:00Z").unwrap();
    let error = store::meeting_filing_backfill_apply(&conn, &preview.token, "2026-08-06T12:02:00Z")
        .unwrap_err();
    assert!(error.to_string().contains("stale"));
    assert_eq!(
        conn.query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1",
            [note_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        personal
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn deleting_a_destination_fails_closed_without_changing_rules_or_routes() {
    let (path, conn) = test_db("deleted_destination");
    let work = folder_id(&conn, "Work");
    let destination = db::create_note_folder(
        &conn,
        Some(work),
        "Temporary team",
        "folder",
        "",
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    store::set_meeting_filing_rule(
        &conn,
        "team@example.com",
        destination,
        None,
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    let event = json!({"account":"team@example.com","attendees":[]});
    let meeting_id = store::create_meeting(
        &conn,
        "Team call",
        None,
        Some(&event.to_string()),
        "2026-08-06T12:02:00Z",
    )
    .unwrap();
    assert_eq!(
        store::get_meeting(&conn, meeting_id).unwrap()["route_status"],
        "matched"
    );

    let before_rule = store::meeting_filing_rules(&conn).unwrap().remove(0);
    let before_meeting = store::get_meeting(&conn, meeting_id).unwrap();
    let error = db::delete_note_folder(&conn, destination)
        .unwrap_err()
        .to_string();
    assert!(error.contains("folder deletion is unavailable"));
    let rule = store::meeting_filing_rules(&conn).unwrap().remove(0);
    assert_eq!(rule.enabled, before_rule.enabled);
    assert_eq!(rule.folder_id, before_rule.folder_id);
    let meeting = store::get_meeting(&conn, meeting_id).unwrap();
    assert_eq!(meeting["route_status"], before_meeting["route_status"]);
    assert_eq!(
        meeting["route_folder_id"],
        before_meeting["route_folder_id"]
    );
    assert_eq!(meeting["route_via"], before_meeting["route_via"]);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM note_folders WHERE id = ?1",
            [destination],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn rule_rejects_a_destination_outside_work_or_personal() {
    let (path, conn) = test_db("invalid_context");
    let archive =
        db::create_note_folder(&conn, None, "Archive", "space", "", "2026-08-06T12:00:00Z")
            .unwrap();
    let error = store::set_meeting_filing_rule(
        &conn,
        "archive@example.com",
        archive,
        None,
        "2026-08-06T12:01:00Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains("work or personal"));

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn rules_can_be_reordered_and_deleted_without_ambiguous_priority() {
    let (path, conn) = test_db("reorder");
    let baro = folder_id(&conn, "Baro");
    for email in ["a@example.com", "b@example.com", "c@example.com"] {
        store::set_meeting_filing_rule(&conn, email, baro, None, "2026-08-06T12:00:00Z").unwrap();
    }
    let order = vec![
        "c@example.com".to_string(),
        "a@example.com".to_string(),
        "b@example.com".to_string(),
    ];
    let rules = store::reorder_meeting_filing_rules(&conn, &order).unwrap();
    assert_eq!(
        rules
            .iter()
            .map(|rule| rule.email.as_str())
            .collect::<Vec<_>>(),
        vec!["c@example.com", "a@example.com", "b@example.com"]
    );
    assert!(store::delete_meeting_filing_rule(&conn, "A@EXAMPLE.COM").unwrap());
    let rules = store::meeting_filing_rules(&conn).unwrap();
    assert_eq!(rules[0].priority, 0);
    assert_eq!(rules[1].priority, 1);
    assert_eq!(rules.len(), 2);

    drop(conn);
    let _ = std::fs::remove_file(path);
}
