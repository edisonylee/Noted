# Agent-context architecture: Fable Max review record

Status: completed and reconciled into the implementation plan.

Date: 2026-08-06

Reviewer configuration:

- Claude Code 2.1.212
- model alias: fable
- effort: max
- mode: read-only plan review
- scope: the implementation plan, selected Noted implementation, repository
  guidance, and the shared Symphony product context

No product code was changed during this review.

## Verdict

The external review found no contradiction that invalidated the core
architecture:

- SQLite remains the operational authority for Noted-owned records.
- External source origins retain their declared authority.
- A versioned portable record contract provides the cross-platform boundary.
- FTS plus chunk embeddings is the primary retrieval path.
- The graph remains derived and evidence-gated.
- Noted-mediated external access uses exact approved Context Passes.
- Plaintext library snapshots remain manual bulk disclosures.

Its verdict was:

> Ready to begin Phase 0 after owner approval, conditional on incorporating the
> high-priority findings into the plan first.

Those findings and the accepted corrections are recorded below. Implementation
still requires a separate owner authorization.

## Findings and resolutions

### H1 — Current database backup has an unsafe runway

Evidence:

- export_db checkpoints/truncates WAL while holding the database mutex.
- It releases the mutex before copying the live noted.db path.
- Background writes can then append WAL and trigger a checkpoint while the raw
  copy is in progress.

Risk:

A user may trust a stale or inconsistent database-only backup before the complete
Phase 1 backup/restore system exists.

Resolution:

- The first approved storage change is an interim consistent SQLite snapshot
  using VACUUM INTO or the Online Backup API.
- Validate the snapshot before reporting success.
- Before plaintext handoff, remove voice-biometric tables, compact the sanitized
  copy, set restrictive permissions, and label it database-only,
  plaintext-sensitive, and incomplete.
- A biometric-complete recovery artifact must be encrypted.
- If the selected snapshot path does not prove freelist cleanup, warn that
  logically deleted remnants may remain.
- Do not implement this change until implementation is explicitly authorized.

### H2 — Retrieval needs a connection-concurrency contract

Evidence:

- The app currently shares one rusqlite Connection behind one Mutex.
- FTS and vec0 queries over the planned corpus can hold that mutex long enough to
  interfere with capture writes and later agent traffic.

Resolution:

- Phase 0 gains a connection/concurrency ADR and feasibility spike.
- Preserve a single writer.
- Run retrieval on dedicated read-only WAL connections or a bounded reader pool.
- Phase 2 latency and active-recording tests must include writer-contention
  measurements.

### H3 — MCP client identity cannot be overstated

Risk:

A same-user process may copy a client secret or impersonate a configured display
name. Exact packet approval limits disclosure but does not prove recipient
identity.

Resolution:

- Phase 0 defines the identity mechanism and spoofing limits.
- Baseline: per-client secret plus Unix-socket peer-UID validation.
- Optional macOS hardening: peer code-signature attestation.
- The approval UI labels identity as claimed unless attested and never presents a
  display name as cryptographically verified without evidence.

### H4 — Phase 1 is too concentrated

Resolution:

- Split Phase 1 into:
  - 1a: interim backup safety, migrations, stable identity, and full
    backup/restore.
  - 1b: authority/scope, lifecycle outbox, deletion, raw-ingress durability, and
    timezone.
- Fixture-only or one-shot shadow-index work may begin after 1a.
- Live background indexing and any retrieval cutover wait for 1b.

### M1 — Legacy migration bootstrap

Resolution:

- Converge unversioned databases through the legacy init path once.
- Inspect/stamp a known baseline before ordered migrations begin.
- Include real daily-driver and alpha-era fixture shapes.

### M2 — Backup during active recording

Resolution:

- Online database snapshots do not pause ordinary writers.
- Full cross-file backups refuse or defer while a meeting recording is active.
- Restore always uses maintenance mode and closed database connections.

### M3 — Voice profiles are biometric data

Resolution:

- Voice centroids and speaker profiles enter the sensitivity map, backup
  inventory, deletion model, and threat model.
- They are excluded from snapshots and agents by default.
- Person deletion offers deletion of associated voice templates.

### M4 — Logical deletion differs from physical scrubbing

Resolution:

- The local-data-protection ADR evaluates secure_delete, WAL/FTS cleanup, and
  vacuum/scrub behavior.
- Product copy distinguishes application-unreachable deletion from physical
  erasure, especially on SSDs and backups.

### M5 — Retrieval must minimize disclosure

Resolution:

- Add precision@k, irrelevant-evidence rate, packet size, and removed-before-
  approval rate to evaluation.
- Context Pass gates consider relevance and disclosure minimization, not recall
  alone.

### Low-level corrections

Accepted:

- Use UTF-8 byte offsets consistently.
- Scope the no-import non-goal to Noted Library snapshots.
- Record the UUIDv7 timestamp-leak tradeoff.
- Remove personal seeded folder defaults from consumer builds.
- Define the raw-capture durability boundary without autosaving unsent text by
  accident.
- Refresh stale contributor architecture documentation during Phase 0.
- Give journal/self-knowledge a conservative explicit sensitivity classification.

## Falsification checks that passed

The reviewer verified that the plan accurately describes:

- hard-coded Eastern timezone behavior
- one 768-dimensional vector per normal note
- meeting-only FTS and non-relevance transcript ordering
- embedding failure before deterministic chat routing
- additive unversioned migrations
- integer public identity
- database-only backup and absent restore
- automatic Brain folder registration/propagation
- current provider routing and rebuild confirmation
- disabled CSP, private macOS APIs, and secret values in Keychain CLI arguments
- the existing Rust library target needed by a later companion binary
- alignment with Symphony's exact-packet and manual-export product contract

The reviewer also confirmed that normal notes currently lack a complete
trash/restore/permanent-delete surface, so Phase 1b includes creating that product
surface rather than merely refactoring existing deletion paths.

## Final review state

After the accepted corrections are incorporated, the plan is ready for owner
implementation authorization. This review does not itself authorize Phase 0 or
any product-code change.
