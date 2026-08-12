# Decision 003: Context records, source authority, and portability

Status: proposed for product-owner acceptance

Date: 2026-08-06

Owners: product and data architecture

Implementation status: Phase 0 contract only; no storage cutover has occurred.

## Context

Agents need stable, citable access to Noted information, but the current database
mixes canonical user data with rebuildable indexes and uses local integer IDs.
Some Brain/Obsidian files are authoritative outside Noted. A filesystem-only
architecture would weaken transactions and lifecycle control; a graph-first
architecture would make lossy inferences look canonical.

## Decision

- SQLite remains the transactional canonical store for Noted-owned records in v1.
- Registered external files remain authoritative for import-owned content. Noted
  stores a mirror, provenance, and derived indexes and never silently overwrites
  the source.
- The portable contract is `ContextRecordV1`, not a table layout. It separates
  stable identity and verbatim content from derived chunks, vectors, graph facts,
  and generated answers.
- Public resource IDs use UUIDv7. Internal SQLite row IDs remain implementation
  details. UUIDv7 leaks coarse creation time; this is accepted for sortable,
  offline-generated IDs and must be disclosed anywhere IDs appear externally.
- Citations use UTF-8 byte offsets plus a source revision and content hash.
- The first file format is a manually generated, scoped Noted Library snapshot.
  It is not a second live source of truth.
- Knowledge-graph relations are optional derived projections. They may expand a
  result by one evidence-backed hop only after the retrieval benchmark shows a
  gain for that query class.

## Authority matrix

| Data class | Authority | Derived copies | External disclosure default |
|---|---|---|---|
| Typed/spoken/photo capture after submission | Noted SQLite | chunks, FTS, vectors, mentions | Context Pass only |
| Meeting transcript and user corrections | Noted SQLite | windows, FTS, vectors, summaries | Context Pass only |
| Registered Brain/Obsidian file | Registered file | SQLite mirror and indexes | excluded until root and scope are approved |
| User-approved memory/correction | Noted SQLite | indexes and graph projection | Context Pass only |
| Generated summary not approved by user | Derived artifact | caches | excluded by default |
| Attachment/audio/video | Referenced local media | metadata and optional thumbnails | excluded by default |
| Voice centroid/profile | Sensitive biometric template | none required | always excluded from agents and plaintext snapshots |
| Completed manual export | User-owned snapshot | none controlled by Noted | outside later Noted revocation guarantees |

Unknown authority, scope, or sensitivity fails closed for external disclosure.
Journal and self-knowledge default to personal/sensitive. Imported roots require an
explicit authority and scope registration rather than path-name inference.

## Consequences

This preserves local-first behavior and creates a contract that can later map to
managed sync or another platform. It requires lifecycle events to fan out to all
derived projections and requires migration work before stable IDs can ship.

## Revisit gate

Reconsider a portable-file canonical layer after Phase 4, and before multi-device
sync or a second primary platform, using measured portability, conflict, and
transaction requirements. Do not change authority merely to make agent ingestion
look simpler.

