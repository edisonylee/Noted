use rusqlite::{params, Connection};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tauri_app_lib::{mobile_store::MobileStore, portable::new_uuid_v7};

#[path = "support/mobile_pairing.rs"]
mod mobile_pairing_support;
use mobile_pairing_support::finalize_fixture_pairing;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
const ARCHIVE_FOLDER_ID: &str = "018f47a0-7b80-7000-8000-000000000101";
const INBOX_TRANSACTION_ID: &str = "018f47a0-7b80-7000-8000-000000000102";
const INBOX_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn database_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "noted-mobile-workspace-{label}-{}-{}.sqlite3",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn remove_database(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

fn add_archive_folder(path: &Path) {
    let connection = Connection::open(path).expect("open mobile database");
    let library_id: String = connection
        .query_row(
            "SELECT library_id FROM mobile_replica WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read mobile library");
    connection
        .execute(
            "INSERT INTO mobile_note_folders (
               folder_id, library_id, parent_folder_id, name, normalized_name,
               position, authority, lifecycle_state, created_at, updated_at
             ) VALUES (?1, ?2, NULL, 'Archive', 'archive', 10,
                       'noted', 'active', 100, 100)",
            params![ARCHIVE_FOLDER_ID, library_id],
        )
        .expect("insert second portable folder");
}

#[test]
fn schema_v5_is_ordered_and_keeps_the_prior_export_contract_path_free() {
    let path = database_path("schema");
    let export = {
        let store = MobileStore::open(&path).expect("migrate fresh store");
        store.create("Portable", "Body").expect("create note");
        store.export_notes().expect("export notes")
    };

    let connection = Connection::open(&path).expect("inspect migrated database");
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read schema version");
    assert_eq!(user_version, 5);
    let history = connection
        .prepare("SELECT version FROM mobile_schema_migrations ORDER BY version")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .expect("read ordered migration history");
    assert_eq!(history, vec![1, 2, 3, 4, 5]);
    let state: (String, String) = connection
        .query_row(
            "SELECT enrollment_state, sync_state FROM mobile_sync_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read initial sync state");
    assert_eq!(
        state,
        ("not_enrolled".to_string(), "not_enrolled".to_string())
    );
    drop(connection);

    let export_value: serde_json::Value = serde_json::from_str(&export).expect("parse export");
    let exported_note = &export_value["payload"]["notes"][0];
    assert!(exported_note.get("folderId").is_none());
    assert!(exported_note.get("path").is_none());

    let restored_path = database_path("schema-restore");
    let restored = MobileStore::open(&restored_path).expect("open restore target");
    assert_eq!(
        restored
            .restore_notes_export(&export)
            .expect("restore prior export shape"),
        1
    );
    assert_eq!(restored.list(None).expect("list restored note").len(), 1);
    remove_database(&path);
    remove_database(&restored_path);
}

#[test]
fn offline_filing_undo_and_counts_survive_restart() {
    let path = database_path("filing");
    let (record_id, notes_folder_id) = {
        let store = MobileStore::open(&path).expect("open store");
        let note = store.create("Plan", "Offline first").expect("create note");
        let workspace = store
            .workspace(None, Some("inbox"), None)
            .expect("load initial workspace");
        let health = store.health().expect("load store-backed health");
        assert_eq!(health.storage, "ready");
        assert_eq!(health.sync, "local");
        assert_eq!(workspace.sync.state, "local");
        assert_eq!(workspace.counts.inbox, 1);
        assert_eq!(workspace.counts.needs_filing, 1);
        assert_eq!(workspace.counts.trash, 0);
        assert!(workspace.capabilities.filing);
        assert!(workspace.capabilities.undo_filing);
        assert!(workspace.capabilities.trash);
        assert!(workspace.capabilities.restore);
        assert!(!workspace.capabilities.conflict_resolution);
        assert_eq!(workspace.folders.len(), 1);
        assert_eq!(workspace.folders[0].path.as_deref(), Some("Notes"));
        (note.record_id, workspace.folders[0].folder_id.clone())
    };
    add_archive_folder(&path);

    {
        let store = MobileStore::open(&path).expect("reopen for filing");
        let first = store
            .file_note(&record_id, &notes_folder_id)
            .expect("file in Notes");
        assert_eq!(first.folder_id.as_deref(), Some(notes_folder_id.as_str()));
        assert!(!first.needs_filing);
        let moved = store
            .file_note(&record_id, ARCHIVE_FOLDER_ID)
            .expect("move to Archive");
        assert_eq!(moved.folder_name.as_deref(), Some("Archive"));
    }

    {
        let store = MobileStore::open(&path).expect("restart after offline filing");
        let archive = store
            .workspace(None, Some("folder"), Some(ARCHIVE_FOLDER_ID))
            .expect("load Archive");
        assert_eq!(archive.notes.len(), 1);
        assert_eq!(
            archive
                .folders
                .iter()
                .map(|folder| folder.note_count)
                .sum::<i64>(),
            1
        );
        assert_eq!(archive.counts.inbox, 1);
        assert_eq!(archive.counts.needs_filing, 0);

        let undone = store
            .undo_note_filing(&record_id)
            .expect("undo last filing");
        assert_eq!(undone.folder_id.as_deref(), Some(notes_folder_id.as_str()));
        assert_eq!(undone.folder_name.as_deref(), Some("Notes"));
        assert!(!undone.needs_filing);
    }

    {
        let connection = Connection::open(&path).expect("inspect filing outbox");
        let (eligible, member_index, member_count, payload_json): (i64, i64, i64, String) =
            connection
                .query_row(
                    "SELECT COUNT(*) OVER (), transaction_member_index,
                            transaction_member_count, payload_json
                     FROM mobile_note_outbox
                     WHERE record_id = ?1 AND eligible_for_sync = 1",
                    [&record_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read current atomic filing mutation");
        assert_eq!((eligible, member_index, member_count), (1, 0, 1));
        let payload: serde_json::Value =
            serde_json::from_str(&payload_json).expect("parse filing payload");
        assert_eq!(payload["organization"]["action"], "undoFiling");
        assert_eq!(payload["organization"]["folderId"], notes_folder_id);
        assert_eq!(
            payload["organization"]["previousFolderId"],
            ARCHIVE_FOLDER_ID
        );
    }

    let store = MobileStore::open(&path).expect("restart after undo");
    let notes = store
        .workspace(None, Some("folder"), Some(&notes_folder_id))
        .expect("load restored folder");
    assert_eq!(notes.notes.len(), 1);
    let serialized = serde_json::to_value(&notes).expect("serialize workspace");
    assert_eq!(
        serialized["folders"][0]["parentId"],
        serde_json::Value::Null
    );
    assert!(serialized["folders"][0].get("parentFolderId").is_none());
    remove_database(&path);
}

#[test]
fn trash_and_restore_are_recoverable_and_never_purge_the_row() {
    let path = database_path("trash");
    let record_id = {
        let store = MobileStore::open(&path).expect("open store");
        store
            .create("Keep me", "recoverable")
            .expect("create note")
            .record_id
    };
    {
        let store = MobileStore::open(&path).expect("open for trash");
        store.delete(&record_id).expect("move to trash");
        let trash = store
            .workspace(None, Some("trash"), None)
            .expect("load trash");
        assert_eq!(trash.notes.len(), 1);
        assert_eq!(trash.notes[0].lifecycle_state, "trashed");
        assert_eq!(trash.counts.inbox, 0);
        assert_eq!(trash.counts.trash, 1);
    }
    {
        let connection = Connection::open(&path).expect("inspect retained row");
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mobile_notes WHERE record_id = ?1",
                [&record_id],
                |row| row.get(0),
            )
            .expect("count retained row");
        assert_eq!(count, 1);
    }
    let store = MobileStore::open(&path).expect("restart trashed store");
    store.restore(&record_id).expect("restore note");
    let active = store
        .workspace(None, Some("inbox"), None)
        .expect("load restored inbox");
    assert_eq!(active.notes.len(), 1);
    assert_eq!(active.notes[0].lifecycle_state, "active");
    assert_eq!(active.counts.trash, 0);
    remove_database(&path);
}

#[test]
fn external_notes_are_visible_but_all_local_mutations_fail_closed() {
    let path = database_path("external");
    let (record_id, trashed_record_id, folder_id) = {
        let store = MobileStore::open(&path).expect("open store");
        let record_id = store
            .create("External", "read only")
            .expect("create fixture")
            .record_id;
        let trashed_record_id = store
            .create("External trash", "also read only")
            .expect("create trashed fixture")
            .record_id;
        store
            .delete(&trashed_record_id)
            .expect("trash before assigning external authority");
        let folder_id = store
            .workspace(None, Some("inbox"), None)
            .expect("load folder")
            .folders[0]
            .folder_id
            .clone();
        (record_id, trashed_record_id, folder_id)
    };
    let connection = Connection::open(&path).expect("open authority fixture");
    connection
        .execute(
            "UPDATE mobile_notes SET authority = 'external'
             WHERE record_id IN (?1, ?2)",
            params![record_id, trashed_record_id],
        )
        .expect("mark external authority");
    drop(connection);

    let store = MobileStore::open(&path).expect("reopen external note");
    let workspace = store
        .workspace(None, Some("inbox"), None)
        .expect("show external note");
    assert!(workspace.notes[0].read_only);
    assert!(store.update(&record_id, "No", "No").is_err());
    assert!(store.file_note(&record_id, &folder_id).is_err());
    assert!(store.delete(&record_id).is_err());
    assert!(store.restore(&trashed_record_id).is_err());
    remove_database(&path);
}

#[test]
fn interrupted_inbox_application_returns_to_received_on_restart() {
    let path = database_path("inbox-recovery");
    drop(MobileStore::open(&path).expect("create schema"));
    {
        let connection = Connection::open(&path).expect("open inbox fixture");
        connection
            .execute(
                "INSERT INTO mobile_sync_inbox (
                   sequence, transaction_id, transaction_digest, payload_json,
                   state, received_at, apply_started_at
                 ) VALUES (1, ?1, ?2, '{}', 'applying', 100, 101)",
                params![INBOX_TRANSACTION_ID, INBOX_DIGEST],
            )
            .expect("seed interrupted inbox transaction");
    }

    drop(MobileStore::open(&path).expect("recover interrupted apply"));
    let connection = Connection::open(&path).expect("inspect recovered inbox");
    let recovered: (String, Option<i64>, Option<String>) = connection
        .query_row(
            "SELECT state, apply_started_at, error_code
             FROM mobile_sync_inbox WHERE sequence = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read recovered inbox row");
    assert_eq!(recovered.0, "received");
    assert_eq!(recovered.1, None);
    assert_eq!(recovered.2.as_deref(), Some("interrupted_apply_recovered"));
    remove_database(&path);
}

#[test]
fn staging_adoption_chunks_outbox_transactions_at_protocol_member_ceiling() {
    let path = database_path("adoption-chunks");
    let store = MobileStore::open(&path).expect("open staging store");
    for index in 0..129 {
        store
            .create(&format!("Staging {index}"), "offline")
            .expect("create staging note");
    }
    let mac_library_id = new_uuid_v7();
    let mac_scope_id = new_uuid_v7();
    assert_eq!(
        finalize_fixture_pairing(&store, &mac_library_id, &mac_scope_id, 1, 0),
        129
    );

    let connection = Connection::open(&path).expect("inspect adoption transactions");
    let transaction_shapes = connection
        .prepare(
            "SELECT transaction_id, transaction_member_count, COUNT(*)
             FROM mobile_note_outbox
             WHERE eligible_for_sync = 1
             GROUP BY transaction_id, transaction_member_count
             ORDER BY transaction_member_count DESC",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .expect("read chunked adoption transactions");
    assert_eq!(transaction_shapes.len(), 2);
    assert_eq!(transaction_shapes[0].1, 128);
    assert_eq!(transaction_shapes[0].2, 128);
    assert_eq!(transaction_shapes[1].1, 1);
    assert_eq!(transaction_shapes[1].2, 1);
    remove_database(&path);
}

#[test]
fn staging_adoption_also_chunks_by_encrypted_transaction_byte_ceiling() {
    let path = database_path("adoption-byte-chunks");
    let store = MobileStore::open(&path).expect("open staging store");
    let nearly_maximal_body = "b".repeat(255 * 1024);
    for index in 0..2 {
        store
            .create(&format!("Large staging {index}"), &nearly_maximal_body)
            .expect("create individually sendable staging note");
    }
    finalize_fixture_pairing(&store, &new_uuid_v7(), &new_uuid_v7(), 1, 0);

    let connection = Connection::open(&path).expect("inspect byte-packed transactions");
    let transaction_shapes = connection
        .prepare(
            "SELECT transaction_id, COUNT(*),
                    SUM(length(CAST(payload_json AS BLOB)) + 28)
             FROM mobile_note_outbox
             WHERE eligible_for_sync = 1
             GROUP BY transaction_id
             ORDER BY transaction_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .expect("read byte-packed adoption transactions");
    assert_eq!(transaction_shapes.len(), 2);
    assert!(transaction_shapes.iter().all(|shape| shape.1 == 1));
    assert!(transaction_shapes.iter().all(|shape| shape.2 <= 512 * 1024));
    remove_database(&path);
}

#[test]
fn unsendable_note_text_and_total_mutation_payloads_roll_back() {
    let path = database_path("mutation-byte-ceilings");
    let store = MobileStore::open(&path).expect("open bounded store");
    let oversized_body = "b".repeat(256 * 1024 + 1);
    let body_error = store
        .create("Too large", &oversized_body)
        .expect_err("reject body over direct-sync string ceiling");
    assert!(body_error.contains("at most"));

    let max_title = "t".repeat(256 * 1024);
    let max_body = "b".repeat(256 * 1024);
    let payload_error = store
        .create(&max_title, &max_body)
        .expect_err("reject aggregate mutation over upload ceiling");
    assert!(payload_error.contains("upload ceiling"));

    let connection = Connection::open(&path).expect("inspect rolled-back oversized notes");
    let state: (i64, i64) = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM mobile_notes),
               (SELECT COUNT(*) FROM mobile_note_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("count bounded records");
    assert_eq!(state, (0, 0));
    remove_database(&path);
}
