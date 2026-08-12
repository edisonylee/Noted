# Decision 004: Database concurrency and index generations

Status: proposed for product-owner acceptance; Phase 0 spikes passed

Date: 2026-08-06

Owners: data architecture and retrieval

Implementation status: contract and isolated tests only.

## Context

The current application serializes all database work through one mutex-protected
connection. Agent retrieval must not delay recording writes, and model upgrades
must not make semantic search unavailable or mix incompatible vector spaces.

## Decision

- Preserve one application writer.
- Production retrieval uses a small, bounded set of dedicated read-only SQLite
  connections in WAL mode. Start with at most two concurrent retrieval readers;
  tune only from writer-latency measurements.
- Register sqlite-vec on every process connection. Give retrieval a short busy
  timeout, cancellation, query deadline, row/byte/token ceilings, and no access to
  the writer mutex while model, network, or filesystem work is pending.
- Active recording and capture writes have priority. Backfills, exports, and
  broad retrieval pause or yield under write pressure.
- FTS remains available when vector generation is unavailable or rebuilding.
- A regular relational table owns index-family state. New vector rows are built
  under an immutable generation ID, validated for coverage and fingerprint, then
  made active by a short transaction. The old generation is retired later.
- The full embedding fingerprint includes provider, model, dimension, normalization,
  chunker version, and preprocessing version. Different vector dimensions require
  a separate vec0 table/schema generation because vec0 fixes dimensions at table
  creation.
- With pinned sqlite-vec 0.1.9, KNN SQL uses either `k = ?` or `LIMIT`, never both.

## Phase 0 evidence

`phase0_storage_spike_test.rs` proves that the pinned sqlite-vec can filter KNN
results by an immutable generation partition and scope, transactionally promote a
generation, and retire the inactive rows. It also proves that sqlite-vec loads on
a read-only connection and a held WAL read snapshot does not block the writer.

These are feasibility proofs, not production load measurements. Phase 2 must run
recording-write contention and latency tests before enabling agent retrieval.

## Failure behavior

- Busy or deadline exceeded: return a bounded retryable error; do not fall back to
  an unbounded writer query.
- Missing or invalid active vector generation: lexical retrieval continues.
- Interrupted build: keep the prior active generation and resume or discard the
  building generation.
- Fingerprint mismatch: never query across spaces; schedule a rebuild.

