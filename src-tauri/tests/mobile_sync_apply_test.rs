use rusqlite::{params, Connection};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tauri_app_lib::{
    mobile_store::{
        MobileInboxChange, MobileIncomingCategory, MobileIncomingFolder, MobileIncomingNote,
        MobileStore,
    },
    portable::{canonical_sha256, is_uuid_v7, new_uuid_v7},
};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
struct ReplicaFixture {
    library_id: String,
    default_scope_id: String,
}

fn database_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "noted-mobile-sync-apply-{label}-{}-{}.sqlite3",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn remove_database(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

fn replica_fixture(path: &Path) -> ReplicaFixture {
    let connection = Connection::open(path).expect("open mobile database");
    connection
        .query_row(
            "SELECT library_id, default_scope_id
             FROM mobile_replica WHERE singleton = 1",
            [],
            |row| {
                Ok(ReplicaFixture {
                    library_id: row.get(0)?,
                    default_scope_id: row.get(1)?,
                })
            },
        )
        .expect("read mobile replica identity")
}

fn enrolled_store(path: &Path, authority_generation: i64, purge_generation: i64) -> MobileStore {
    let store = MobileStore::open(path).expect("open mobile store");
    let fixture = replica_fixture(path);
    store
        .adopt_staging_library(&fixture.library_id, &fixture.default_scope_id)
        .expect("adopt paired Mac library");
    store
        .activate_sync_enrollment(
            &fixture.library_id,
            &fixture.default_scope_id,
            authority_generation,
            purge_generation,
        )
        .expect("activate sync enrollment");
    store
}

fn note_hash(title: &str, body: &str) -> String {
    canonical_sha256(&json!({
        "body": body,
        "title": title,
    }))
}

fn incoming_note(
    fixture: &ReplicaFixture,
    record_id: &str,
    title: &str,
    body: &str,
    revision: i64,
    lifecycle_state: &str,
    folder_id: Option<&str>,
    authority: &str,
) -> MobileIncomingNote {
    let timestamp = 1_720_000_000_000_i64 + revision;
    MobileIncomingNote {
        record_id: record_id.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        created_at: 1_720_000_000_000,
        updated_at: timestamp,
        accepted_revision: revision,
        accepted_version_id: new_uuid_v7(),
        accepted_content_hash: note_hash(title, body),
        lifecycle_state: lifecycle_state.to_string(),
        trashed_at: matches!(lifecycle_state, "trash" | "tombstone").then_some(timestamp),
        tombstoned_at: (lifecycle_state == "tombstone").then_some(timestamp),
        folder_id: folder_id.map(str::to_string),
        authority: authority.to_string(),
        scope_id: fixture.default_scope_id.clone(),
        scope_class: "personal".to_string(),
    }
}

fn transaction_digest(change: &MobileInboxChange) -> String {
    // The transport digest binds every apply-relevant field while excluding
    // the digest itself. Keeping this helper independent from production code
    // makes a digest implementation that accidentally omits a field observable.
    canonical_sha256(&json!({
        "authorityGeneration": change.authority_generation,
        "categories": change.categories,
        "folders": change.folders,
        "libraryId": change.library_id,
        "notes": change.notes,
        "purgeGeneration": change.purge_generation,
        "sequence": change.sequence,
        "sourceDeviceId": change.source_device_id,
        "transactionId": change.transaction_id,
    }))
}

fn seal_change(mut change: MobileInboxChange) -> MobileInboxChange {
    change.transaction_digest = transaction_digest(&change);
    change
}

fn change(
    fixture: &ReplicaFixture,
    sequence: i64,
    authority_generation: i64,
    purge_generation: i64,
    categories: Vec<MobileIncomingCategory>,
    folders: Vec<MobileIncomingFolder>,
    notes: Vec<MobileIncomingNote>,
) -> MobileInboxChange {
    seal_change(MobileInboxChange {
        sequence,
        transaction_id: new_uuid_v7(),
        transaction_digest: String::new(),
        library_id: fixture.library_id.clone(),
        source_device_id: new_uuid_v7(),
        authority_generation,
        purge_generation,
        categories,
        folders,
        notes,
    })
}

fn accepted_note_state(path: &Path, record_id: &str) -> (String, String, i64, i64, String, String) {
    let connection = Connection::open(path).expect("inspect mobile database");
    connection
        .query_row(
            "SELECT title, body, accepted_revision, working_revision,
                    lifecycle_state, sync_state
             FROM mobile_notes WHERE record_id = ?1",
            [record_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("read accepted note state")
}

fn sync_cursors(path: &Path) -> (i64, i64) {
    let connection = Connection::open(path).expect("inspect mobile database");
    connection
        .query_row(
            "SELECT downloaded_cursor, applied_cursor
             FROM mobile_sync_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read mobile sync cursors")
}

fn eligible_outbox_count(path: &Path, record_id: &str) -> i64 {
    let connection = Connection::open(path).expect("inspect mobile database");
    connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_note_outbox
             WHERE record_id = ?1 AND eligible_for_sync = 1",
            [record_id],
            |row| row.get(0),
        )
        .expect("count eligible outbox mutations")
}

#[test]
fn bootstrap_atomically_applies_category_folder_and_note_then_replays_idempotently() {
    let path = database_path("bootstrap");
    let store = enrolled_store(&path, 7, 3);
    let fixture = replica_fixture(&path);
    let category_id = new_uuid_v7();
    let folder_id = new_uuid_v7();
    let record_id = new_uuid_v7();
    let bootstrap = change(
        &fixture,
        1,
        7,
        3,
        vec![MobileIncomingCategory {
            category_id: category_id.clone(),
            name: "Projects".to_string(),
            schema: json!({"type": "object"}),
            authority: "noted".to_string(),
            updated_at: 1_720_000_000_001,
        }],
        vec![MobileIncomingFolder {
            folder_id: folder_id.clone(),
            name: "Launch".to_string(),
            parent_folder_id: None,
            position: 10,
            authority: "noted".to_string(),
            updated_at: 1_720_000_000_001,
        }],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Bootstrap note",
            "Available offline",
            1,
            "active",
            Some(&folder_id),
            "noted",
        )],
    );

    let first = store
        .apply_inbox_change(&bootstrap)
        .expect("apply valid bootstrap transaction");
    assert_eq!(first.sequence, 1);
    assert_eq!(first.conflict_count, 0);
    assert!(first.applied_count >= 3);
    assert_eq!(sync_cursors(&path), (1, 1));

    let connection = Connection::open(&path).expect("inspect bootstrap state");
    let category_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_note_categories WHERE category_id = ?1",
            [&category_id],
            |row| row.get(0),
        )
        .expect("count applied category");
    let folder_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_note_folders WHERE folder_id = ?1",
            [&folder_id],
            |row| row.get(0),
        )
        .expect("count applied folder");
    let filing: Option<String> = connection
        .query_row(
            "SELECT folder_id FROM mobile_note_filing WHERE record_id = ?1",
            [&record_id],
            |row| row.get(0),
        )
        .expect("read applied filing");
    assert_eq!((category_count, folder_count), (1, 1));
    assert_eq!(filing.as_deref(), Some(folder_id.as_str()));
    drop(connection);

    let before = accepted_note_state(&path, &record_id);
    store
        .apply_inbox_change(&bootstrap)
        .expect("replay exact transaction idempotently");
    assert_eq!(accepted_note_state(&path, &record_id), before);
    assert_eq!(sync_cursors(&path), (1, 1));
    let connection = Connection::open(&path).expect("inspect replay state");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM mobile_sync_inbox WHERE sequence = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count replayed inbox row"),
        1
    );
    remove_database(&path);
}

#[test]
fn sequence_gaps_and_digest_payload_rebinding_fail_without_advancing_cursors() {
    let path = database_path("fail-closed");
    let store = enrolled_store(&path, 4, 1);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    let skipped = change(
        &fixture,
        2,
        4,
        1,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Skipped",
            "Must not apply",
            1,
            "active",
            None,
            "noted",
        )],
    );
    assert!(store.apply_inbox_change(&skipped).is_err());
    assert_eq!(sync_cursors(&path), (0, 0));

    let valid = change(
        &fixture,
        1,
        4,
        1,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Bound",
            "Original payload",
            1,
            "active",
            None,
            "noted",
        )],
    );
    let mut unbound_digest = valid.clone();
    unbound_digest.transaction_digest = "0".repeat(64);
    assert!(store.apply_inbox_change(&unbound_digest).is_err());
    assert_eq!(sync_cursors(&path), (0, 0));

    store
        .apply_inbox_change(&valid)
        .expect("apply correctly bound transaction");

    let mut same_digest_changed_payload = valid.clone();
    same_digest_changed_payload.notes[0].body = "Substituted payload".to_string();
    same_digest_changed_payload.notes[0].accepted_content_hash =
        note_hash("Bound", "Substituted payload");
    assert!(store
        .apply_inbox_change(&same_digest_changed_payload)
        .is_err());

    let mut same_sequence_changed_digest = same_digest_changed_payload;
    same_sequence_changed_digest.transaction_digest =
        transaction_digest(&same_sequence_changed_digest);
    assert!(store
        .apply_inbox_change(&same_sequence_changed_digest)
        .is_err());
    assert_eq!(sync_cursors(&path), (1, 1));
    assert_eq!(accepted_note_state(&path, &record_id).1, "Original payload");
    remove_database(&path);
}

#[test]
fn accepted_remote_heads_fast_forward_without_creating_a_local_outbox_branch() {
    let path = database_path("fast-forward");
    let store = enrolled_store(&path, 2, 0);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    let first = change(
        &fixture,
        1,
        2,
        0,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Remote one",
            "revision one",
            1,
            "active",
            None,
            "noted",
        )],
    );
    store.apply_inbox_change(&first).expect("apply first head");
    let second = change(
        &fixture,
        2,
        2,
        0,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Remote two",
            "revision two",
            2,
            "active",
            None,
            "noted",
        )],
    );
    store
        .apply_inbox_change(&second)
        .expect("fast-forward head");

    assert_eq!(
        accepted_note_state(&path, &record_id),
        (
            "Remote two".to_string(),
            "revision two".to_string(),
            2,
            2,
            "active".to_string(),
            "acknowledged".to_string(),
        )
    );
    assert_eq!(eligible_outbox_count(&path, &record_id), 0);
    assert_eq!(sync_cursors(&path), (2, 2));
    remove_database(&path);
}

#[test]
fn existing_folder_reparent_cycle_is_quarantined_without_partial_graph_changes() {
    let path = database_path("folder-cycle");
    let store = enrolled_store(&path, 14, 0);
    let fixture = replica_fixture(&path);
    let folder_a = new_uuid_v7();
    let folder_b = new_uuid_v7();
    let root_folders = vec![
        MobileIncomingFolder {
            folder_id: folder_a.clone(),
            name: "Alpha".to_string(),
            parent_folder_id: None,
            position: 1,
            authority: "noted".to_string(),
            updated_at: 1_720_000_000_001,
        },
        MobileIncomingFolder {
            folder_id: folder_b.clone(),
            name: "Beta".to_string(),
            parent_folder_id: None,
            position: 2,
            authority: "noted".to_string(),
            updated_at: 1_720_000_000_001,
        },
    ];
    store
        .apply_inbox_change(&change(&fixture, 1, 14, 0, vec![], root_folders, vec![]))
        .expect("apply root folders");

    let cyclic = vec![
        MobileIncomingFolder {
            folder_id: folder_a.clone(),
            name: "Alpha".to_string(),
            parent_folder_id: Some(folder_b.clone()),
            position: 1,
            authority: "noted".to_string(),
            updated_at: 1_720_000_000_002,
        },
        MobileIncomingFolder {
            folder_id: folder_b.clone(),
            name: "Beta".to_string(),
            parent_folder_id: Some(folder_a.clone()),
            position: 2,
            authority: "noted".to_string(),
            updated_at: 1_720_000_000_002,
        },
    ];
    let result = store
        .apply_inbox_change(&change(&fixture, 2, 14, 0, vec![], cyclic, vec![]))
        .expect("quarantine authenticated cyclic graph");
    assert_eq!(result.state, "quarantined");
    assert_eq!(sync_cursors(&path), (2, 2));

    let connection = Connection::open(&path).expect("inspect transactional folder graph");
    let parents = connection
        .prepare(
            "SELECT folder_id, parent_folder_id FROM mobile_note_folders
             WHERE folder_id IN (?1, ?2) ORDER BY folder_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![folder_a, folder_b], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .expect("read unchanged folder graph");
    assert_eq!(parents.len(), 2);
    assert!(parents.iter().all(|(_, parent)| parent.is_none()));
    let inbox_state: (String, Option<String>) = connection
        .query_row(
            "SELECT state, error_code FROM mobile_sync_inbox WHERE sequence = 2",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read semantic quarantine receipt");
    assert_eq!(inbox_state.0, "quarantined");
    assert_eq!(inbox_state.1.as_deref(), Some("semantic_validation_failed"));
    remove_database(&path);
}

#[test]
fn operational_apply_failure_stays_received_and_retries_exact_bytes() {
    let path = database_path("operational-retry");
    let store = enrolled_store(&path, 15, 0);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    let incoming = change(
        &fixture,
        1,
        15,
        0,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Retryable",
            "storage must recover",
            1,
            "active",
            None,
            "noted",
        )],
    );

    let connection = Connection::open(&path).expect("open transient failure fixture");
    let table_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'mobile_note_filing'",
            [],
            |row| row.get(0),
        )
        .expect("capture filing schema");
    let index_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'index' AND name = 'idx_mobile_note_filing_folder'",
            [],
            |row| row.get(0),
        )
        .expect("capture filing index");
    connection
        .execute("DROP TABLE mobile_note_filing", [])
        .expect("simulate unavailable local schema");
    drop(connection);

    let error = store
        .apply_inbox_change(&incoming)
        .expect_err("local storage failure must stay retryable");
    assert!(error.contains("mobile_note_filing"));
    assert_eq!(sync_cursors(&path), (1, 0));
    let connection = Connection::open(&path).expect("inspect retry checkpoint");
    let retry_state: (String, Option<String>) = connection
        .query_row(
            "SELECT state, error_code FROM mobile_sync_inbox WHERE sequence = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read retryable inbox state");
    assert_eq!(retry_state.0, "received");
    assert_eq!(retry_state.1.as_deref(), Some("transient_apply_failed"));
    connection
        .execute(&table_sql, [])
        .expect("restore filing table");
    connection
        .execute(&index_sql, [])
        .expect("restore filing index");
    drop(connection);

    let retried = store
        .apply_inbox_change(&incoming)
        .expect("retry exact authenticated transaction");
    assert_eq!(retried.state, "applied");
    assert_eq!(sync_cursors(&path), (1, 1));
    assert_eq!(accepted_note_state(&path, &record_id).0, "Retryable");
    remove_database(&path);
}

#[test]
fn remote_timestamps_outside_rfc3339_range_are_rejected_before_receipt() {
    let path = database_path("timestamp-range");
    let store = enrolled_store(&path, 16, 0);
    let fixture = replica_fixture(&path);
    let mut note = incoming_note(
        &fixture,
        &new_uuid_v7(),
        "Far future",
        "must not crash date formatting",
        1,
        "active",
        None,
        "noted",
    );
    note.updated_at = i64::MAX;
    let invalid = change(&fixture, 1, 16, 0, vec![], vec![], vec![note]);
    assert!(store.apply_inbox_change(&invalid).is_err());
    assert_eq!(sync_cursors(&path), (0, 0));
    let connection = Connection::open(&path).expect("inspect rejected timestamp");
    let receipts: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_sync_inbox", [], |row| {
            row.get(0)
        })
        .expect("count invalid timestamp receipts");
    assert_eq!(receipts, 0);
    remove_database(&path);
}

#[test]
fn conflict_keeps_pending_branch_then_keep_as_copy_preserves_both_versions() {
    let path = database_path("keep-copy");
    let store = enrolled_store(&path, 5, 2);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    let base = change(
        &fixture,
        1,
        5,
        2,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture, &record_id, "Shared", "base", 1, "active", None, "noted",
        )],
    );
    store.apply_inbox_change(&base).expect("apply base head");
    store
        .update(&record_id, "Local draft", "local pending branch")
        .expect("make offline edit");
    assert_eq!(eligible_outbox_count(&path, &record_id), 1);

    let remote = change(
        &fixture,
        2,
        5,
        2,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Remote accepted",
            "remote revision two",
            2,
            "active",
            None,
            "noted",
        )],
    );
    let applied = store
        .apply_inbox_change(&remote)
        .expect("record accepted-head conflict");
    assert_eq!(applied.conflict_count, 1);
    assert_eq!(eligible_outbox_count(&path, &record_id), 0);
    let conflicted = store
        .workspace(None, Some("all"), None)
        .expect("load conflicted workspace");
    let conflicted_note = conflicted
        .notes
        .iter()
        .find(|note| note.record_id == record_id)
        .expect("find conflicted original");
    assert!(conflicted_note.has_open_conflict);
    assert!(store
        .update(&record_id, "Blocked", "must resolve first")
        .is_err());
    assert!(store.delete(&record_id).is_err());
    assert!(store
        .file_note(&record_id, &conflicted.folders[0].folder_id)
        .is_err());
    assert_eq!(eligible_outbox_count(&path, &record_id), 0);
    let connection = Connection::open(&path).expect("inspect open conflict");
    let preserved: (String, String, String, String, i64, String) = connection
        .query_row(
            "SELECT conflicts.local_title, conflicts.local_body,
                    conflicts.remote_title, conflicts.state,
                    outbox.eligible_for_sync, outbox.state
             FROM mobile_note_conflicts AS conflicts
             JOIN mobile_note_outbox AS outbox
               ON outbox.record_id = conflicts.record_id
             WHERE conflicts.record_id = ?1 AND conflicts.state = 'open'",
            [&record_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("read durable conflict and preserved outbox branch");
    assert_eq!(
        preserved,
        (
            "Local draft".to_string(),
            "local pending branch".to_string(),
            "Remote accepted".to_string(),
            "open".to_string(),
            0,
            "conflict".to_string(),
        )
    );
    drop(connection);

    let copy = store
        .resolve_note_conflict(&record_id, "keepAsCopy")
        .expect("keep local branch as copy");
    assert_ne!(copy.record_id, record_id);
    assert!(is_uuid_v7(&copy.record_id));
    assert_eq!(copy.conflict_of.as_deref(), Some(record_id.as_str()));
    assert!(!copy.has_open_conflict);
    let original = accepted_note_state(&path, &record_id);
    assert_eq!(
        (original.0.as_str(), original.1.as_str()),
        ("Remote accepted", "remote revision two")
    );
    let copied = accepted_note_state(&path, &copy.record_id);
    assert_eq!(
        (copied.0.as_str(), copied.1.as_str()),
        ("Local draft", "local pending branch")
    );
    assert_eq!(eligible_outbox_count(&path, &copy.record_id), 1);

    let connection = Connection::open(&path).expect("inspect resolved conflict");
    let evidence: (String, Option<i64>) = connection
        .query_row(
            "SELECT state, resolved_at FROM mobile_note_conflicts WHERE record_id = ?1",
            [&record_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read kept-copy evidence");
    assert_eq!(evidence.0, "kept_copy");
    assert!(evidence.1.is_some());
    remove_database(&path);
}

#[test]
fn keep_as_copy_preserves_local_lifecycle_scope_authority_and_provenance() {
    let path = database_path("keep-copy-metadata");
    let store = enrolled_store(&path, 12, 1);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    store
        .apply_inbox_change(&change(
            &fixture,
            1,
            12,
            1,
            vec![],
            vec![],
            vec![incoming_note(
                &fixture,
                &record_id,
                "Base",
                "base body",
                1,
                "active",
                None,
                "noted",
            )],
        ))
        .expect("apply base note");
    let folder_id = store
        .workspace(None, Some("all"), None)
        .expect("load local folder")
        .folders[0]
        .folder_id
        .clone();
    store
        .file_note(&record_id, &folder_id)
        .expect("file local branch");
    store
        .update(&record_id, "Local branch", "preserve every local attribute")
        .expect("edit local branch");
    store.delete(&record_id).expect("trash local branch");

    let local_scope_id = new_uuid_v7();
    let local_provenance = json!({
        "source": "registered_external_mirror",
        "sourceId": new_uuid_v7(),
    });
    let connection = Connection::open(&path).expect("seed distinct local metadata");
    connection
        .execute(
            "UPDATE mobile_notes
             SET authority = 'external', scope = 'work', scope_id = ?1,
                 scope_class = 'work', provenance_json = ?2
             WHERE record_id = ?3",
            params![
                local_scope_id,
                serde_json::to_string(&local_provenance).expect("serialize provenance"),
                record_id
            ],
        )
        .expect("assign local branch metadata");
    drop(connection);

    let conflict = store
        .apply_inbox_change(&change(
            &fixture,
            2,
            12,
            1,
            vec![],
            vec![],
            vec![incoming_note(
                &fixture,
                &record_id,
                "Remote branch",
                "remote accepted body",
                2,
                "active",
                None,
                "noted",
            )],
        ))
        .expect("record metadata conflict");
    assert_eq!(conflict.conflict_count, 1);

    let copy = store
        .resolve_note_conflict(&record_id, "keepAsCopy")
        .expect("preserve local branch as copy");
    assert_eq!(copy.lifecycle_state, "trashed");
    assert_eq!(copy.folder_id.as_deref(), Some(folder_id.as_str()));
    assert!(copy.read_only);
    assert!(!copy.has_open_conflict);

    let connection = Connection::open(&path).expect("inspect retained local metadata");
    let retained: (
        String,
        Option<i64>,
        Option<i64>,
        String,
        String,
        String,
        String,
        String,
    ) = connection
        .query_row(
            "SELECT lifecycle_state, trashed_at, tombstoned_at,
                    authority, scope, scope_id, scope_class, provenance_json
             FROM mobile_notes WHERE record_id = ?1",
            [&copy.record_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .expect("read retained local metadata");
    assert_eq!(retained.0, "trash");
    assert!(retained.1.is_some());
    assert_eq!(retained.2, None);
    assert_eq!(retained.3, "external");
    assert_eq!(retained.4, "work");
    assert_eq!(retained.5, local_scope_id);
    assert_eq!(retained.6, "work");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&retained.7).expect("parse retained provenance"),
        local_provenance
    );
    let payload_json: String = connection
        .query_row(
            "SELECT payload_json FROM mobile_note_outbox
             WHERE record_id = ?1 AND eligible_for_sync = 1",
            [&copy.record_id],
            |row| row.get(0),
        )
        .expect("read retained copy mutation");
    let payload: serde_json::Value =
        serde_json::from_str(&payload_json).expect("parse retained copy mutation");
    assert_eq!(payload["proposed_record"]["authority"]["kind"], "external");
    assert_eq!(payload["proposed_record"]["provenance"], local_provenance);
    remove_database(&path);
}

#[test]
fn use_remote_resolves_visible_state_without_erasing_conflict_evidence() {
    let path = database_path("use-remote");
    let store = enrolled_store(&path, 3, 0);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    store
        .apply_inbox_change(&change(
            &fixture,
            1,
            3,
            0,
            vec![],
            vec![],
            vec![incoming_note(
                &fixture,
                &record_id,
                "Base",
                "base body",
                1,
                "active",
                None,
                "noted",
            )],
        ))
        .expect("apply base");
    store
        .update(&record_id, "Local", "local body")
        .expect("make local branch");
    store
        .apply_inbox_change(&change(
            &fixture,
            2,
            3,
            0,
            vec![],
            vec![],
            vec![incoming_note(
                &fixture,
                &record_id,
                "Remote",
                "remote body",
                2,
                "active",
                None,
                "noted",
            )],
        ))
        .expect("record conflict");

    let resolved = store
        .resolve_note_conflict(&record_id, "useRemote")
        .expect("choose remote branch");
    assert_eq!(resolved.record_id, record_id);
    assert_eq!(
        (resolved.title.as_str(), resolved.body.as_str()),
        ("Remote", "remote body")
    );
    assert_eq!(eligible_outbox_count(&path, &record_id), 0);

    let connection = Connection::open(&path).expect("inspect conflict evidence");
    let evidence: (String, String, String, Option<i64>) = connection
        .query_row(
            "SELECT state, local_body, remote_body, resolved_at
             FROM mobile_note_conflicts WHERE record_id = ?1",
            [&record_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read use-remote evidence");
    assert_eq!(evidence.0, "used_remote");
    assert_eq!(evidence.1, "local body");
    assert_eq!(evidence.2, "remote body");
    assert!(evidence.3.is_some());
    remove_database(&path);
}

#[test]
fn external_authority_is_visible_but_remains_read_only_after_sync_apply() {
    let path = database_path("external");
    let store = enrolled_store(&path, 9, 4);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    store
        .apply_inbox_change(&change(
            &fixture,
            1,
            9,
            4,
            vec![],
            vec![],
            vec![incoming_note(
                &fixture,
                &record_id,
                "Imported",
                "owned elsewhere",
                1,
                "active",
                None,
                "external",
            )],
        ))
        .expect("apply external-authority note");
    let workspace = store
        .workspace(None, Some("inbox"), None)
        .expect("load external note");
    let note = workspace
        .notes
        .iter()
        .find(|note| note.record_id == record_id)
        .expect("find external note");
    assert!(note.read_only);
    assert!(store.update(&record_id, "No", "No").is_err());
    assert!(store.delete(&record_id).is_err());
    assert!(store
        .file_note(&record_id, &workspace.folders[0].folder_id)
        .is_err());
    assert!(store
        .resolve_note_conflict(&record_id, "useRemote")
        .is_err());
    assert_eq!(eligible_outbox_count(&path, &record_id), 0);
    let connection = Connection::open(&path).expect("inspect external provenance");
    let provenance_json: String = connection
        .query_row(
            "SELECT provenance_json FROM mobile_notes WHERE record_id = ?1",
            [&record_id],
            |row| row.get(0),
        )
        .expect("read external source provenance");
    let provenance: serde_json::Value =
        serde_json::from_str(&provenance_json).expect("parse external provenance");
    assert_eq!(provenance["source"], "external_authority");
    assert_eq!(provenance["transport"], "direct_sync");
    assert!(provenance["source_device_id"].as_str().is_some());
    remove_database(&path);
}

#[test]
fn remote_trash_and_restore_follow_accepted_lifecycle_without_local_mutations() {
    let path = database_path("lifecycle");
    let store = enrolled_store(&path, 6, 2);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    for (sequence, revision, lifecycle) in [(1, 1, "active"), (2, 2, "trash"), (3, 3, "active")] {
        store
            .apply_inbox_change(&change(
                &fixture,
                sequence,
                6,
                2,
                vec![],
                vec![],
                vec![incoming_note(
                    &fixture,
                    &record_id,
                    "Lifecycle",
                    "remote state",
                    revision,
                    lifecycle,
                    None,
                    "noted",
                )],
            ))
            .expect("apply remote lifecycle head");
        if lifecycle == "trash" {
            let trash = store
                .workspace(None, Some("trash"), None)
                .expect("load trash after remote delete");
            assert_eq!(trash.notes.len(), 1);
            assert_eq!(trash.notes[0].lifecycle_state, "trashed");
        }
    }
    let inbox = store
        .workspace(None, Some("inbox"), None)
        .expect("load restored inbox");
    assert_eq!(inbox.notes.len(), 1);
    assert_eq!(inbox.notes[0].lifecycle_state, "active");
    assert_eq!(eligible_outbox_count(&path, &record_id), 0);
    assert_eq!(sync_cursors(&path), (3, 3));
    remove_database(&path);
}

#[test]
fn library_authority_and_purge_generation_mismatches_are_rejected_atomically() {
    let path = database_path("identity-floors");
    let store = enrolled_store(&path, 11, 5);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    let valid = change(
        &fixture,
        1,
        11,
        5,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Guarded",
            "must stay absent",
            1,
            "active",
            None,
            "noted",
        )],
    );

    let mut wrong_library = valid.clone();
    wrong_library.library_id = new_uuid_v7();
    wrong_library = seal_change(wrong_library);
    assert!(store.apply_inbox_change(&wrong_library).is_err());

    let mut wrong_authority_generation = valid.clone();
    wrong_authority_generation.authority_generation = 10;
    wrong_authority_generation = seal_change(wrong_authority_generation);
    assert!(store
        .apply_inbox_change(&wrong_authority_generation)
        .is_err());

    let mut wrong_purge_generation = valid.clone();
    wrong_purge_generation.purge_generation = 4;
    wrong_purge_generation = seal_change(wrong_purge_generation);
    assert!(store.apply_inbox_change(&wrong_purge_generation).is_err());

    assert_eq!(sync_cursors(&path), (0, 0));
    let connection = Connection::open(&path).expect("inspect rejected state");
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_notes WHERE record_id = ?1",
            [&record_id],
            |row| row.get(0),
        )
        .expect("count rejected notes");
    assert_eq!(count, 0);
    drop(connection);

    store
        .apply_inbox_change(&valid)
        .expect("accept exact library and generations afterward");
    assert_eq!(sync_cursors(&path), (1, 1));
    remove_database(&path);
}

#[test]
fn interrupted_received_transaction_can_resume_after_store_restart() {
    let path = database_path("restart");
    let store = enrolled_store(&path, 8, 1);
    let fixture = replica_fixture(&path);
    let record_id = new_uuid_v7();
    let pending = change(
        &fixture,
        1,
        8,
        1,
        vec![],
        vec![],
        vec![incoming_note(
            &fixture,
            &record_id,
            "Recovered",
            "after interrupted apply",
            1,
            "active",
            None,
            "noted",
        )],
    );
    drop(store);

    let payload_json = serde_json::to_string(&pending).expect("serialize interrupted payload");
    let connection = Connection::open(&path).expect("seed interrupted apply");
    connection
        .execute(
            "INSERT INTO mobile_sync_inbox (
               sequence, transaction_id, transaction_digest, payload_json,
               state, received_at, apply_started_at
             ) VALUES (1, ?1, ?2, ?3, 'applying', 100, 101)",
            params![
                pending.transaction_id,
                pending.transaction_digest,
                payload_json
            ],
        )
        .expect("insert interrupted inbox row");
    connection
        .execute(
            "UPDATE mobile_sync_state
             SET sync_state = 'syncing', downloaded_cursor = 1, applied_cursor = 0
             WHERE singleton = 1",
            [],
        )
        .expect("record downloaded interrupted transaction");
    drop(connection);

    let reopened = MobileStore::open(&path).expect("recover interrupted inbox on restart");
    let connection = Connection::open(&path).expect("inspect recovered marker");
    let recovered: (String, Option<i64>, Option<String>) = connection
        .query_row(
            "SELECT state, apply_started_at, error_code
             FROM mobile_sync_inbox WHERE sequence = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read recovered inbox state");
    assert_eq!(recovered.0, "received");
    assert_eq!(recovered.1, None);
    assert_eq!(recovered.2.as_deref(), Some("interrupted_apply_recovered"));
    drop(connection);

    reopened
        .apply_inbox_change(&pending)
        .expect("resume exact recovered transaction");
    assert_eq!(sync_cursors(&path), (1, 1));
    assert_eq!(accepted_note_state(&path, &record_id).0, "Recovered");
    let connection = Connection::open(&path).expect("inspect resumed inbox");
    let state: String = connection
        .query_row(
            "SELECT state FROM mobile_sync_inbox WHERE sequence = 1",
            [],
            |row| row.get(0),
        )
        .expect("read resumed state");
    assert_eq!(state, "applied");
    remove_database(&path);
}
