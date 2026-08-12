# ContextRecordV1 and resource URI contract

Status: proposed for product-owner acceptance

`ContextRecordV1` is the stable boundary between canonical Noted data, rebuildable
retrieval projections, portable snapshots, and agent disclosure. It is not a
serialization of the SQLite schema.

## Record envelope

Required fields:

| Field | Contract |
|---|---|
| `contract_version` | Exact string `noted.context-record.v1` |
| `library_id` | UUID identifying one logical library |
| `record_id` | UUIDv7 stable within and across full replacement restores |
| `kind` | Versioned enum such as `note`, `meeting`, `transcript`, `memory` |
| `revision` | Positive, monotonically increasing integer for this record |
| `created_at` / `updated_at` | RFC 3339 UTC instants |
| `event_time` | Optional UTC instant or local civil interval plus IANA timezone |
| `scope` | Stable scope ID plus `work`, `personal`, or future user-defined class |
| `sensitivity` | `standard`, `sensitive`, or `restricted` |
| `authority` | `noted`, `external`, or `derived`, with origin details |
| `content` | Lossless user-visible body and media-independent structure |
| `content_hash` | SHA-256 over canonical versioned serialization |
| `provenance` | Capture/import source, parent IDs, and transformation lineage |
| `lifecycle` | Active/trash/tombstone state and applicable timestamps |

Optional extensions must be namespaced and ignorable. Readers reject an unknown
major contract version and preserve unknown extension data during lossless
round-trips.

## IDs and URIs

SQLite integers never leave the repository boundary as public identities.

~~~text
noted://library/{library_uuid}/record/{record_uuid}
noted://library/{library_uuid}/record/{record_uuid}?revision={positive_integer}
noted://library/{library_uuid}/record/{record_uuid}/chunk/{chunk_key}?generation={generation_id}
~~~

The record URI is canonical. A chunk URI is derived and expires with its index
generation; citations therefore include the record URI and source span even when
a chunk initiated retrieval.

UUIDv7 is selected for offline generation and locality. It reveals approximate
creation time to anyone who sees the identifier. Receipts, logs, snapshots, and
agent clients must treat this as disclosed metadata. If that disclosure becomes
unacceptable before Phase 1, UUIDv4 is the approved alternative; IDs must not be
silently changed after shipping.

## Citations

A citation contains:

- canonical record URI;
- record revision and content hash;
- UTF-8 byte `start` inclusive and `end` exclusive;
- optional transcript first/last segment IDs and millisecond range;
- a short verbatim evidence excerpt; and
- retrieval generation and disclosure-pass ID for diagnostics.

Offsets apply to the exact canonical UTF-8 content bytes for that revision, not
UTF-16 code units, characters, rendered Markdown, or embedding text. A stale
citation resolves to its historical revision when retained; otherwise Noted
reports it as stale rather than pointing at shifted text.

## Canonical versus derived

Canonical: submitted raw capture, user edits, meeting transcript segments, user
corrections, approved memory, organization, lifecycle state, and provenance.

Derived: normalized embedding text, chunks, FTS rows, vectors, entity mentions,
unapproved summaries, graph edges, rank scores, and answer text. Every derived
row names its source record/revision and generation and may be discarded.

