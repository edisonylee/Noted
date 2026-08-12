use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags};
use tauri_app_lib::db;

fn temp_db(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "noted_phase0_{label}_{}_{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ))
}

fn remove_sqlite_files(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

fn nearest_chunk(
    conn: &Connection,
    generation: &str,
    scope: &str,
    query: &str,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT chunk_id
         FROM phase0_vectors
         WHERE embedding MATCH ?1
           AND k = 1
           AND generation = ?2
           AND scope = ?3
         ORDER BY distance",
        params![query, generation, scope],
        |row| row.get(0),
    )
}

#[test]
fn sqlite_vec_generation_partition_can_filter_promote_and_retire() {
    let path = temp_db("vec_generation");
    remove_sqlite_files(&path);
    let mut conn = db::init(&path).expect("initialize sqlite with sqlite-vec");

    conn.execute_batch(
        "CREATE TABLE phase0_index_state (
           family TEXT PRIMARY KEY,
           active_generation TEXT NOT NULL
         );
         INSERT INTO phase0_index_state(family, active_generation)
         VALUES ('context-chunks', 'generation-a');
         CREATE VIRTUAL TABLE phase0_vectors USING vec0(
           vector_row_id INTEGER PRIMARY KEY,
           generation TEXT PARTITION KEY,
           chunk_id INTEGER,
           scope TEXT,
           embedding FLOAT[4]
         );",
    )
    .expect("create generation-partitioned spike schema");

    let rows = [
        (1_i64, "generation-a", 101_i64, "work", "[1,0,0,0]"),
        (2, "generation-a", 102, "personal", "[0,1,0,0]"),
        (3, "generation-b", 201, "work", "[0,0,1,0]"),
        (4, "generation-b", 202, "personal", "[0,0,0,1]"),
    ];
    for (row_id, generation, chunk_id, scope, vector) in rows {
        conn.execute(
            "INSERT INTO phase0_vectors
             (vector_row_id, generation, chunk_id, scope, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![row_id, generation, chunk_id, scope, vector],
        )
        .expect("insert spike vector");
    }

    assert_eq!(
        nearest_chunk(&conn, "generation-a", "work", "[1,0,0,0]").unwrap(),
        101,
        "the KNN result must stay inside both generation and scope"
    );
    assert_eq!(
        nearest_chunk(&conn, "generation-b", "work", "[0,0,1,0]").unwrap(),
        201
    );

    let tx = conn.transaction().expect("begin generation promotion");
    tx.execute(
        "UPDATE phase0_index_state
         SET active_generation = 'generation-b'
         WHERE family = 'context-chunks'",
        [],
    )
    .expect("flip active generation");
    tx.commit().expect("commit generation promotion");

    let active: String = conn
        .query_row(
            "SELECT active_generation FROM phase0_index_state
             WHERE family = 'context-chunks'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active, "generation-b");

    conn.execute(
        "DELETE FROM phase0_vectors WHERE generation = 'generation-a'",
        [],
    )
    .expect("retire the inactive generation");
    let remaining: i64 = conn
        .query_row("SELECT count(*) FROM phase0_vectors", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        remaining, 2,
        "retirement must preserve the active generation"
    );

    drop(conn);
    remove_sqlite_files(&path);
}

#[test]
fn wal_read_snapshot_does_not_block_the_single_writer() {
    let path = temp_db("wal_readers");
    remove_sqlite_files(&path);
    let writer = db::init(&path).expect("initialize WAL database");
    writer
        .execute_batch(
            "CREATE TABLE phase0_records (
               id INTEGER PRIMARY KEY,
               body TEXT NOT NULL
             );
             INSERT INTO phase0_records(id, body) VALUES (1, 'before snapshot');",
        )
        .unwrap();

    let reader = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open dedicated read-only connection");
    reader.busy_timeout(Duration::from_secs(2)).unwrap();
    let vec_version: String = reader
        .query_row("SELECT vec_version()", [], |row| row.get(0))
        .expect("sqlite-vec must also load on retrieval connections");
    assert!(vec_version.starts_with('v'));

    let (snapshot_ready_tx, snapshot_ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        reader.execute_batch("BEGIN").unwrap();
        let before: i64 = reader
            .query_row("SELECT count(*) FROM phase0_records", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 1);
        snapshot_ready_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let during: i64 = reader
            .query_row("SELECT count(*) FROM phase0_records", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            during, 1,
            "the open read transaction keeps a stable snapshot"
        );
        reader.execute_batch("COMMIT").unwrap();
    });

    snapshot_ready_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reader established its snapshot");
    writer
        .execute(
            "INSERT INTO phase0_records(id, body) VALUES (2, 'written during snapshot')",
            [],
        )
        .expect("WAL permits the single writer while a reader holds a snapshot");
    release_tx.send(()).unwrap();
    reader_thread.join().unwrap();

    let fresh_reader = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let after: i64 = fresh_reader
        .query_row("SELECT count(*) FROM phase0_records", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        after, 2,
        "a fresh retrieval snapshot sees the committed write"
    );

    drop(fresh_reader);
    drop(writer);
    remove_sqlite_files(&path);
}
