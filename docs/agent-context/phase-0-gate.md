# Phase 0 gate report

Date: 2026-08-06

Status: implementation work complete for review; gate remains open pending
product-owner acceptance of the proposed contracts and defaults.

Phase 0 changed no production schema, command, retrieval path, export behavior,
or agent-access default. Its Rust additions are isolated deterministic tests.

## Contract checklist

| Gate | Status | Evidence |
|---|---|---|
| Canonical/derived boundary and source authority | proposed | Decision 003 and `phase0_source_authority_test.rs` |
| ContextRecordV1, public IDs, UUIDv7 leakage, citations | proposed | `context-record-v1.md` |
| Noted Library v1 and bulk-disclosure boundary | proposed | `noted-library-v1.md` |
| Local, Balanced, BYOK, agent, export, and future-cloud data flows | proposed | `data-flow-and-threat-model.md` |
| Local storage/export/MCP/injection/sync threats | proposed | `data-flow-and-threat-model.md` and Decision 005 |
| One writer, bounded WAL readers, recording priority | feasible and proposed | Decision 004; storage spike passed |
| Vector generation build/filter/promote/retire | feasible and proposed | storage spike passed with sqlite-vec 0.1.9 |
| Source authority and unknown-scope behavior | characterized and proposed | authority spike passed; Decision 003 |
| Timezone and raw-capture boundaries | proposed | `operational-contracts.md` |
| Legacy convergence and baseline stamping | feasible and proposed | earliest-schema and migration-protocol spikes passed |
| Recovery point, reader floor, binary downgrade | feasible and proposed | migration-protocol spike passed |
| Scope/sensitivity, journal, and biometric defaults | proposed | Decision 003 and operational contracts |
| Rollout and rollback | proposed | operational contracts |
| Retrieval fixture and numerical ceilings | contract passed | 150 questions, 15 per class, 30 held out; fixture test passed |
| External architecture review | complete | `../AGENT_CONTEXT_FABLE_REVIEW.md`; findings folded into contracts |

## Phase 0 evidence commands

~~~sh
cd src-tauri
cargo test --test phase0_migration_baseline_test
cargo test --test phase0_migration_protocol_spike_test
cargo test --test phase0_source_authority_test
cargo test --test phase0_storage_spike_test
cargo test --test retrieval_fixture_contract_test
~~~

Observed results on 2026-08-06: six tests passed across five targets. The storage
spike established that sqlite-vec 0.1.9 accepts generation/scope-filtered KNN,
with the important SQL constraint that callers choose `k = ?` or `LIMIT`, not
both. A read-only sqlite-vec WAL connection held a stable snapshot while the
single writer committed successfully.

## Interpretation and remaining evidence

- The 150-question committed corpus is synthetic and intentionally small in
  source-document diversity. It is a contract/CI seed, not a quality claim.
- Before Phase 2 changes retrieval, capture current result IDs and latency for
  this fixture and add a private untracked dogfood suite using the same schema.
- Before setting the production reader-pool size, run representative meeting
  recording writes with concurrent FTS/vector queries and enforce the written
  writer-latency ceiling.
- Before Phase 1 migrations ship, validate sanitized private copies representing
  the current daily-driver database and every actually distributed Alpha schema.
- Before any export or agent surface ships, resolve the documented database and
  ephemeral Context Pass encryption-at-rest decision.

These are later implementation/enablement gates and do not require changing the
Phase 0 architecture. A failure that changes authority, identity, disclosure, or
rollback semantics returns the design to product review.

## Owner acceptance requested

Acceptance means approving these defaults for implementation:

1. SQLite canonical for Noted-owned v1 records; registered files authoritative
   for import-owned records; graph derived and disabled by default.
2. UUIDv7 public record IDs with disclosed coarse creation-time leakage.
3. Unknown scope fails closed; journal/self-knowledge sensitive; voice templates
   restricted and never disclosed to agents/plaintext snapshots.
4. One writer plus at most two initial read-only WAL retrieval connections.
5. Local agent access off by default through exact, inspectable Context Passes;
   same-user client identity remains claimed without code-sign attestation.
6. Manual Noted Library snapshots are scoped plaintext disclosures, not a live
   canonical directory.
7. Raw typed capture persists on submit; meeting capture persists when recording
   start succeeds; draft autosave is a separate product choice.
8. Product deletion claims application unreachability, not forensic SSD erasure.
9. Ordered migrations begin only after safe backup replaces the current live-file
   copy behavior.

After explicit acceptance, Phase 1a begins with that backup-safety patch. No
Phase 1 product code should land before this gate is accepted.
