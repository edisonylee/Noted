# Agent context architecture — Phase 0

Phase 0 freezes the public contracts and produces evidence before Noted changes
its canonical storage. The consumer-production North Star is: local-only remains
a complete product, while stable records and permissions can map cleanly to open
source builds, managed sync, and other clients later.

## Contracts

- [ContextRecordV1 and resource URIs](context-record-v1.md)
- [Noted Library v1](noted-library-v1.md)
- [Data flows and threat model](data-flow-and-threat-model.md)
- [Retrieval evaluation](retrieval-evaluation.md)
- [Migration, lifecycle, time, rollout, and rollback](operational-contracts.md)
- [Phase 0 gate](phase-0-gate.md)

## Decisions

- [Canonical records, authority, and portability](../decisions/003-context-record-authority-and-portability.md)
- [Database concurrency and index generations](../decisions/004-database-concurrency-and-index-generations.md)
- [Context Pass and client identity](../decisions/005-context-pass-and-client-identity.md)

These documents specify behavior; they do not claim the behavior is implemented.
The Phase 0 gate distinguishes evidence already collected from owner decisions and
Phase 1 work that remain open.

