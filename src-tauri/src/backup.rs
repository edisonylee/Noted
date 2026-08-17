//! Consistent database-only snapshots.
//!
//! This module deliberately preserves the complete SQLite database, including
//! sensitive speaker embeddings. The resulting file is therefore plaintext,
//! sensitive, and incomplete without referenced media. Encryption, media
//! manifests, and a staged restore workflow belong to the later recovery phase;
//! this checkpoint only replaces the unsafe live-file copy.

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

// Canonical and user-owned rows whose counts must survive a database snapshot.
// Rebuildable FTS/vector tables are intentionally excluded from this inventory;
// SQLite's own quick_check still validates the complete database file.
const INVENTORY_TABLES: &[&str] = &[
    "categories",
    "notes",
    "entries",
    "note_folders",
    "note_folder_items",
    "note_filing_events",
    "meeting_filing_rules",
    "app_metadata",
    "recaps",
    "entities",
    "entity_mentions",
    "dismissed_merges",
    "pending_captures",
    "brain_vaults",
    "meetings",
    "meeting_segments",
    "transcript_vocabulary",
    "transcript_correction_batches",
    "transcript_correction_items",
    "meeting_summaries",
    "meeting_speakers",
    "speaker_profiles",
    "meeting_templates",
    "agent_context_receipts",
];

struct StagingFile {
    path: PathBuf,
    renamed: bool,
}

impl Drop for StagingFile {
    fn drop(&mut self) {
        if !self.renamed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Create a consistent, validated, database-only snapshot at `destination`.
///
/// The caller must hold Noted's database-writer mutex for this entire call. The
/// destination and staging file must not already exist. `VACUUM INTO` creates a
/// standalone SQLite file from one source snapshot, after which an independent
/// read-only connection validates integrity, foreign keys, and canonical counts.
/// Only a validated and fsynced staging file is atomically renamed into place.
pub fn create_database_snapshot(source: &Connection, destination: &Path) -> Result<()> {
    if destination.exists() {
        bail!(
            "backup destination already exists: {}",
            destination.display()
        );
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("backup destination has no parent directory"))?;
    if !parent.is_dir() {
        bail!(
            "backup destination directory is unavailable: {}",
            parent.display()
        );
    }

    let source_inventory = canonical_inventory(source)?;
    let staging_path = unique_staging_path(destination)?;
    let mut staging = StagingFile {
        path: staging_path,
        renamed: false,
    };
    let staging_text = staging
        .path
        .to_str()
        .ok_or_else(|| anyhow!("backup staging path is not valid UTF-8"))?;

    source
        .execute("VACUUM INTO ?1", [staging_text])
        .context("create consistent SQLite snapshot")?;

    #[cfg(unix)]
    std::fs::set_permissions(&staging.path, std::fs::Permissions::from_mode(0o600))
        .context("restrict snapshot permissions")?;

    validate_snapshot(&staging.path, &source_inventory)?;

    File::open(&staging.path)
        .context("open snapshot for fsync")?
        .sync_all()
        .context("fsync snapshot")?;
    sync_directory(parent).context("fsync snapshot directory before rename")?;

    // A same-directory hard link publishes the validated inode atomically and
    // fails if another process created the destination. `rename` would silently
    // overwrite on Unix after a check/use race.
    std::fs::hard_link(&staging.path, destination).context("publish validated snapshot")?;
    std::fs::remove_file(&staging.path).context("remove published staging link")?;
    staging.renamed = true;
    sync_directory(parent).context("fsync snapshot directory after rename")?;
    Ok(())
}

fn validate_snapshot(path: &Path, expected_inventory: &BTreeMap<String, i64>) -> Result<()> {
    let snapshot = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("open snapshot independently")?;

    let mut quick_check = snapshot.prepare("PRAGMA quick_check")?;
    let results = quick_check
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if results.as_slice() != ["ok"] {
        bail!("snapshot quick_check failed: {}", results.join("; "));
    }

    let mut foreign_keys = snapshot.prepare("PRAGMA foreign_key_check")?;
    if foreign_keys.query([])?.next()?.is_some() {
        bail!("snapshot foreign_key_check failed");
    }

    let actual_inventory = canonical_inventory(&snapshot)?;
    if &actual_inventory != expected_inventory {
        bail!(
            "snapshot canonical inventory mismatch: expected {expected_inventory:?}, got {actual_inventory:?}"
        );
    }
    Ok(())
}

fn canonical_inventory(connection: &Connection) -> Result<BTreeMap<String, i64>> {
    let mut inventory = BTreeMap::new();
    for table in INVENTORY_TABLES {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("canonical inventory table is missing: {table}");
        }
        // Table names come exclusively from the fixed allowlist above.
        let count =
            connection.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get::<_, i64>(0)
            })?;
        inventory.insert((*table).to_string(), count);
    }
    Ok(inventory)
}

fn unique_staging_path(destination: &Path) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("backup destination has no parent directory"))?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("backup destination filename is not valid UTF-8"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..32 {
        let counter = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}.staging-{}-{nonce}-{counter}",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!("could not allocate a unique backup staging path")
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
