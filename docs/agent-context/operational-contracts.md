# Operational contracts

Status: proposed for product-owner acceptance

## Migration baseline and compatibility

The first ordered migration release must:

1. Create a safe pre-migration recovery point using SQLite Online Backup or
   `VACUUM INTO`; never copy a live WAL database file directly.
2. Converge an unversioned database through the legacy initializer once.
3. Verify `quick_check`, `foreign_key_check`, required tables/columns, and a
   canonical-row inventory.
4. Atomically stamp application ID, `schema_version`, `min_reader_version`,
   migration name/checksum/time, and product version.
5. Run later migrations transactionally in expand, backfill, cutover, and delayed
   contract stages.

The synthetic fixture in `phase0_migration_baseline_test.rs` matches the earliest
repository schema and proves current convergence, data preservation, and
idempotent reopen. Before Phase 1 ships, add sanitized private fixtures representing
the daily-driver and every distributed Alpha schema. Private content never enters
the repository.

An older binary opens only schemas within its declared reader range. Same-binary
feature rollback disables new behavior while preserving expanded data. An
incompatible binary downgrade restores the pre-migration dataset and matching
binary; it never guesses at a newer schema.

During active recording, a full cross-file backup is refused or deferred. Database
snapshots may use online backup only when recording latency remains within the
accepted budget. Complete recovery backups inventory media and sensitive biometric
templates, require encryption before being called private, and exclude credentials.

## Raw-capture durability

- Typed/photo capture becomes canonical only when the user explicitly submits.
  Unsent composer text is a UI draft; autosaving it is a separate visible product
  choice, not part of agent-context work.
- Meeting capture becomes canonical when recording start succeeds and the meeting
  row is durably committed. Transcript segments commit incrementally.
- A crash after submission/recording start must recover or visibly queue the raw
  input before classification, embedding, or cloud calls.
- Derived processing failure never deletes the canonical raw input.

## Time

Store instants in UTC. Store the originating IANA timezone when local civil time
affects meaning. Date-only statements remain date-only with timezone context and
must not be coerced to midnight UTC. Ambiguous or nonexistent daylight-saving
times require an explicit disambiguation. Retrieval date filters operate in the
requester's chosen IANA timezone and report it in the receipt. No feature may
hard-code US Eastern time as the portable contract.

## Scope and sensitivity migration

Existing `filing_context` and root spaces seed scope only when values are valid.
`work` and `personal` become stable scope records. Legacy NULL, unknown strings,
unregistered imports, and conflicts become `unknown` and fail closed externally.
Journal/self-knowledge defaults personal and sensitive. Meeting-speaker centroids
and persistent voice profiles are restricted biometric data and are never agent
retrieval content.

## Lifecycle and deletion claims

Trash is reversible. Permanent delete tombstones canonical identity and removes
Noted-controlled FTS, vector, graph, cache, staging, and future-agent reachability.
Tests verify application-level unreachability and absence from rebuilt indexes.
Physical-media erasure is not promised without platform evidence; WAL, freelists,
backups, rollback generations, completed exports, and third-party copies are named
separately in UI copy.

## Rollout and rollback

Every new storage, retrieval, export, and agent behavior is behind an independent
default-off flag. Enable in order: developer fixture, private dogfood, opt-in
preview, release candidate, production default. Advancement requires migration,
correctness, privacy, latency, disk, crash-recovery, and rollback gates.

Disabling semantic retrieval falls back to lexical search. Disabling agent access
revokes new passes without deleting canonical data. Derived generations remain
discardable. Rollback must not require interpreting partially written generated
artifacts as canonical records.
