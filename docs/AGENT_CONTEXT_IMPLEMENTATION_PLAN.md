# Agent-readable context architecture and implementation plan

Status: owner-reviewed, Fable Max-reviewed, and revised. Phase 0 was explicitly
authorized on 2026-08-06 and is implemented for owner acceptance. Phase 1 is not
yet authorized.

Review sequence:

1. Product-owner plan review — completed.
2. External architecture review using Claude Code Fable at max effort — completed
   read-only on 2026-08-06.
3. Reconcile review findings — completed in this revision.
4. Begin Phase 0 implementation — explicitly authorized and completed for review.
5. Accept Phase 0 contracts and authorize Phase 1a — pending.

External review record:

- docs/AGENT_CONTEXT_FABLE_REVIEW.md

Last updated: 2026-08-06

## Executive decision

Noted should not begin by replacing its storage with a knowledge graph or by making
an agent crawl an unstructured directory for every question.

The recommended architecture is:

1. Keep lossless user records in Noted's transactional store.
2. Give every durable record a portable, stable identity and versioned data contract.
3. Build a derived document-and-chunk retrieval index over those records.
4. Use SQLite FTS5 and chunk embeddings together, with deterministic rank fusion.
5. Use the existing entity graph only as a source-backed, query-specific expansion.
6. Offer an optional, human-readable Noted Library as a manual scoped snapshot.
7. Put Noted Ask and external agents behind the same typed retrieval core.
8. Require an exact, purpose-bound Context Pass before Noted-mediated content
   crosses to an external agent. Manual copy and export remain explicit
   user-directed disclosure paths outside that enforcement boundary.

This preserves Noted's local-first advantage while creating stable seams for a
consumer app, open-source builds, mobile clients, encrypted sync, and a future
hosted service. It also avoids a premature rewrite into a graph database or a
second writable file store.

## North Star

Noted is being prepared as the implementation foundation for a consumer product
that can be open sourced and extended beyond one person's Mac.

The architecture must therefore satisfy these product constraints:

- A normal consumer can capture, read, search, export, back up, and restore their
  information without developer tools.
- The baseline remains useful without a network connection or an installed model.
- Local-only is a supported privacy mode and product advantage, not the limit of
  the eventual product.
- Public product concepts cannot depend on SQLite row IDs, personal folder names,
  one Mac path, Ollama, a private server, or an official-service credential.
- Source records remain lossless, inspectable, correctable, removable, and tied to
  provenance.
- Derived chunks, FTS rows, vectors, generated drafts, unreviewed summaries, and
  inferred graph edges are rebuildable. User-authored or user-approved memories
  are canonical source-linked records and cannot be erased by reindexing.
- Noted-mediated external agent access is a disclosure event. Read-only access is
  necessary but not sufficient; the user must be able to inspect and approve the
  exact packet. Manual copy/export is a separate explicit disclosure path.
- Official hosted services and their operations remain separable from the
  open-source client and core data model.
- Upgrades, rollback, backup, deletion, and recovery are product features rather
  than post-launch maintenance work.

### Relationship to Symphony

Noted is the near-term implementation foundation for Symphony's approved product
loop: source, proposed memory, review, purpose-bound request, exact packet
inspection, approval, disclosure, receipt, and correction. ContextRecordV1 and
Context Pass are the implementation-neutral names for that settled boundary.

Before implementation, Phase 0 must copy the approved record and access contracts
into repository-owned public documentation. Open-source contributors and clean
builds cannot depend on a private external context folder to understand core
behavior.

## What the North Star changes

The long-term direction does not require a different retrieval architecture. It
does require a different implementation order and stricter defaults.

| Earlier temptation | Consumer-production decision |
|---|---|
| Add chunk embeddings immediately | Establish versioned migrations, stable IDs, backup/restore, and lifecycle ownership first |
| Expose the whole database to an agent | Expose a bounded typed interface and only release approved Context Passes |
| Write every note to a visible folder automatically | Make the v1 Noted Library a manual, scoped snapshot with an explicit bulk-disclosure warning |
| Use graph traversal as the main retrieval system | Treat graph expansion as optional and require evidence of benchmark improvement |
| Depend on a local model for search | Make exact and lexical retrieval the offline baseline; semantic retrieval is an enhancement |
| Optimize for the current Mac process | Define portable record, URI, retrieval, and provider contracts that other clients can implement |
| Build hosted access early | Prove the local contracts, permissions, and recovery model before adding remote availability |

Local-only components are not inherently a barrier. SQLite, FTS5, local embeddings,
and a stdio helper all work well in an open-source consumer product. The barriers
are implicit personal defaults, platform-specific concepts leaking into public
contracts, unsafe migrations, silent data duplication, and access that cannot be
explained or revoked.

## Current-state audit

The plan begins from the current system instead of assuming a greenfield rewrite.

### What is already useful

- SQLite with WAL and foreign keys is an appropriate operational store for the
  macOS app.
- Raw notes, structured entries, meeting metadata, transcript segments, summaries,
  entities, and source media already have explicit stores.
- Meeting transcripts already have trigger-maintained FTS5.
- sqlite-vec and the provider layer already establish a 768-dimensional embedding
  contract.
- Local, Hosted, and BYOK capability routing is already provider-neutral at the
  feature boundary.
- The graph already carries entities and mentions that can be used as an optional
  retrieval signal.
- Tauri's Rust library target can host a shared retrieval module and a later
  companion binary without converting the repository into a new workspace first.

### Gaps that block a production agent context surface

- Normal notes have no lexical FTS index.
- One vector represents an entire note, so long notes and transcripts cannot
  return precise evidence reliably.
- Meeting transcript semantic retrieval is incomplete; exact transcript search is
  segment-based and not ranked primarily by relevance.
- Retrieval, graph expansion, prompt assembly, and answer orchestration are tightly
  coupled inside the chat command.
- An embedding failure can prevent retrieval paths that should still work
  lexically.
- Public identifiers are database integers rather than durable resource IDs.
- Schema evolution relies primarily on additive startup checks and metadata
  markers, which will become difficult to audit for a multi-table retrieval index.
- Backup currently covers the database file but not the complete restorable user
  dataset, and there is no verified restore flow. The current checkpoint-then-
  copy command releases the database mutex before copying the live file, leaving
  an auto-checkpoint race during the copy.
- Invalidation and deletion of derived content are spread across write paths.
- Normal notes do not yet have a complete trash, restore, and permanent-delete
  product surface.
- All application queries and writes share one Connection behind one Mutex;
  planned FTS/vector reads need an explicit concurrency design to avoid blocking
  capture.
- There is no deterministic, whole-library portable projection.
- There is no agent permission or Context Pass enforcement layer in Noted.
- There is no MCP implementation, and the broad phone RPC bridge is not an
  appropriate substitute.

### Broader consumer-readiness risks discovered during planning

These are not all part of retrieval, but they affect release order:

- Personal Brain folders and periodic propagation behavior must become explicit,
  user-selected, previewed, and off by default for general consumers.
- Fresh databases also seed personal/product-specific folders such as Baro, Daily
  Standup Meeting Notes, and Symphony; consumer defaults must remove or replace
  those with neutral onboarding choices.
- Hard-coded America/New_York behavior must become device or user timezone
  behavior with timezone-aware persisted events.
- Credential access needs a platform abstraction before a non-macOS port; secrets
  should not be passed in process arguments.
- Speaker centroids and persistent speaker profiles are biometric templates and
  need explicit sensitivity, retention, backup, and deletion treatment.
- Tauri CSP, capabilities, private API use, purpose strings, signing, notarization,
  updater behavior, and minimum macOS targets require a release security review.
- The repository needs an explicit license, contribution and security policies,
  privacy documentation, third-party/model notices, and reproducible release
  checks before public launch.
- Contributor architecture guidance is stale relative to the current command,
  meeting, provider, space, and synchronization surfaces and must be refreshed
  alongside the Phase 0 public contracts.
- Noted Hosted should not be marketed as a production consumer service until its
  tenant isolation, deletion, availability, abuse, observability, incident, and
  commercial requirements are independently complete.

## Research conclusions

The research does not support choosing one of “plain files” or “knowledge graph”
as the universal retrieval mechanism.

1. Long-context models still lose information when relevant evidence is buried
   in the middle of a large prompt. Reading every file for every request is slow,
   expensive, and less reliable than selecting bounded evidence.
2. Lexical and semantic retrieval solve different cases. FTS is strong for names,
   quotes, IDs, and rare tokens; embeddings are strong for paraphrase and thematic
   similarity. Hybrid retrieval is a safer default than either alone.
3. Context added to chunks can improve semantic retrieval, but the quoted evidence
   must remain verbatim and source-addressable.
4. Graph retrieval can improve relationship and multi-hop questions, but graph
   construction and maintenance add cost and can encode extraction mistakes.
   Graph expansion should therefore be conditional and benchmarked.
5. Source provenance and permission boundaries matter more for a personal context
   product than maximizing the number of memories exposed.
6. MCP is a useful interoperability boundary, not a storage design. It should be a
   thin adapter over the product's retrieval and permission contracts.

Load-bearing references:

- Lost in the Middle: https://arxiv.org/abs/2307.03172
- Anthropic contextual retrieval: https://www.anthropic.com/news/contextual-retrieval
- SQLite FTS5: https://www.sqlite.org/fts5.html
- SQLite Online Backup API: https://www.sqlite.org/backup.html
- Microsoft GraphRAG: https://microsoft.github.io/graphrag/
- HippoRAG: https://arxiv.org/abs/2405.14831
- Model Context Protocol specification: https://modelcontextprotocol.io/specification/

## Product and data contracts

These contracts should be reviewed and frozen before schema implementation. They
are the portability boundary for future macOS, mobile, server, and open-source
implementations.

### 1. ContextRecordV1

The portable product contract is a versioned record, even while SQLite remains
the canonical operational store for Noted-owned records in the current app.

Conceptual fields:

~~~text
schema_version
library_id
record_id
record_kind
scope_id
title
body or typed content variants
created_at
updated_at
event_start
event_end
revision
content_hash
source and provenance
sensitivity
attachment metadata
deleted_at or tombstone
derived outputs with generator and source references
~~~

Rules:

- Use UUIDv7 for new public record IDs.
- Keep integer primary keys for efficient local joins.
- Generate missing public IDs exactly once during migration.
- Keep record IDs stable through edits, title changes, reindexing, backup, export,
  restore, and sync.
- Treat scope_id as extensible. Current Personal and Work concepts can map into it,
  while Symphony can later support Work, Health, People, Personal, and nested
  projects without changing the retrieval protocol.
- Treat raw source text, user edits, and user-approved memory overlays as
  authoritative.
- Label generated drafts, unapproved summaries, extracted proposals, inferred
  relationships, and contextual chunk prefixes as derived. Store their generator
  version and source references.
- When a person reviews, edits, or approves a proposed memory, persist that result
  as a canonical source-linked record or overlay. Rebuilds may regenerate the
  proposal but must never erase or silently replace the approved result.
- Carry revision, content hash, tombstone, and provenance fields now so a later
  encrypted sync engine does not require a new identity model.
- Do not sync or export derived indexes as canonical data.

Source authority is explicit per origin:

- Noted-owned records use SQLite as the writable operational authority in v1.
- Registered external sources, including import-direction Brain or Obsidian
  vaults, remain authoritative in their source files. Noted stores a provenance-
  linked mirror/read model and must not overwrite the source without a separately
  approved export or sync rule.
- Generated proposals are derived until a person approves or edits them; the
  approved result is canonical.
- A Noted Library snapshot is an export, never an import authority in v1.

This resolves the apparent conflict between a future file-portable product and
Noted's current database: the record contract is portable; SQLite is the current
transactional materialization for Noted-owned records; external origins retain
their declared authority; and the Noted Library is a manual snapshot. Phase 0
must map every current origin before implementation. After Phase 4, an ADR revisits
whether Symphony's long-term durable layer should move to portable inspectable
files before sync or another platform ships. This staged Noted decision is not a
reversal of that long-term hypothesis.

### 2. Stable resource URIs and citations

Recommended URI grammar:

~~~text
noted://library/<library-id>/notes/<record-id>
noted://library/<library-id>/meetings/<record-id>
noted://library/<library-id>/meetings/<record-id>/transcript
~~~

Citations add stable source anchors:

~~~text
#bytes=<utf8-start>-<utf8-end>
#segments=<first-id>-<last-id>
#t=<start-ms>-<end-ms>
~~~

Text offsets are UTF-8 byte offsets, matching canonical stored bytes and avoiding
Rust UTF-8 versus JavaScript UTF-16 ambiguity. Every citation includes the exact
source revision and content hash. It resolves only against that revision; if the
revision is no longer retained, Noted reports the citation as stale rather than
silently applying old offsets to current text. Chunk IDs are useful internally
but cannot be the sole citation because rechunking must not break references.
Public entity URIs are deferred until a user-reviewed entity identity registry
exists; automatically extracted entities remain derived internal signals.

### 3. Caller-neutral retrieval contract

Noted Ask, a future mobile client, the Noted Library preview, and an external
adapter should call one service rather than each inventing search behavior.

Conceptual request:

~~~text
RetrievalRequest
  query
  caller_context
  requested_scope_ids
  record_kinds
  date range
  entity filters
  lexical or semantic availability
  graph expansion policy
  maximum candidates
  maximum returned bytes or tokens
~~~

Conceptual response:

~~~text
RetrievalResponse
  bounded evidence chunks
  stable resource URI and source anchors
  title, type, dates, speakers
  source revision and content hash
  score components and fallback reason
  neighboring context handles
~~~

CallerContext distinguishes trusted in-app use from an external disclosure
request. It is injected by an authenticated internal entry point and can never be
supplied by a caller. Requested scopes are intersected with server-side effective
permissions; callers cannot claim TrustedInApp or expand their own scopes. Scope
and trash filtering must happen below the adapter layer so a client cannot bypass
them with a different tool. Unknown scope or sensitivity fails closed for export
and external access.

### 4. External agent Context Pass contract

An unrestricted read-only search endpoint would still disclose private content:
search snippets, result titles, and existence metadata all cross the boundary.
The external flow must preserve Symphony's approval model.

Proposed flow:

1. A registered client requests context for a named purpose, query, and scope.
2. Noted performs retrieval locally.
3. Noted freezes an exact candidate packet with a hash, expiry, resource list,
   excerpts, and explicit exclusions.
4. The user sees the exact packet and approves, edits, or denies it.
5. Noted hashes the final post-edit bytes and pins every included source revision
   and range.
6. The adapter releases only those frozen approved bytes. Pagination reads the
   frozen payload, never live resource text.
7. Noted stores a local inspectable disclosure receipt containing the claimed
   client name, credential identity, verification level and attestation state at
   handoff, purpose, scope, resource URIs, source revisions/ranges, inclusion
   classes and counts, exclusions summary, delivery status/transport, time, final
   hash, and revocation state, without retaining excerpt text.
8. Revocation blocks future reads from Noted but is not described as erasing data
   already received by the remote agent.

Candidate and approved payloads are ephemeral sensitive duplicates. Phase 0 sets
strict size and TTL limits; the proposed starting defaults are a 15-minute pending
request and deletion after completed delivery or one hour, whichever comes first.
Denial, expiry, close, or revoke erases payload plaintext and retains only receipt
metadata and the final hash. Crash recovery removes expired payloads. The
encryption/storage ADR must be resolved before this feature ships, and the
approval UI renders source text as inert escaped text rather than interpreted
Markdown or HTML.

If an included source is corrected, removed, or changes revision before delivery
finishes, Noted never mutates the approved packet in place. It closes the pass,
erases undelivered frozen bytes, and requires fresh retrieval and approval. The
receipt truthfully records the old revision and which byte ranges were already
delivered; already received bytes remain non-retractable.

Approval names the receiving client and its verification level. A display name is
shown as claimed, not verified, unless the broker has attested the peer. The UI
also warns that Noted may not know which downstream model or provider that client
uses, and that revoke, local deletion, or pass expiry cannot erase bytes already
received. Broad persistent full-library consent and workflow-level “always allow”
are outside consumer v1.

### 5. Portable Noted Library snapshot contract

Recommended one-way layout:

~~~text
Noted Library/
  .noted/
    library.json
    format-version
  notes/YYYY/MM/<record-id>.md
  meetings/YYYY/MM/<record-id>/
    meeting.md
    transcript.jsonl
    transcript.md
  assets/<sha256>.<extension>
~~~

The manifest records the format version, library ID, generator version, resource
URI, relative path, source revision/hash, output hash, media type, inclusion
scope, and generation time.

Defaults:

- Created only through a manual export after the user selects a destination,
  scope, and data classes.
- Textual notes and transcript text included when selected.
- Audio, video, images, inferred graph data, and sensitive scopes excluded unless
  explicitly enabled.
- Speaker centroids and persistent voice profiles are biometric templates and are
  never included in v1 snapshots.
- UUID-based filenames; human titles live inside files and metadata.
- Atomic snapshot generation into a new destination or explicit replacement
  generation.
- Never delete unknown files. Replacement requires a matching Noted sentinel and
  explicit confirmation; modified managed files are preserved or quarantined for
  user resolution rather than silently removed.
- Export/read-only snapshot in v1; bidirectional editing and continuous mirroring
  are deferred.
- Export errors never block capture, search, or canonical writes.
- The format is designed for later import and migration, even though v1 is export
  only.

This snapshot is an intentional bulk disclosure outside Noted's Context Pass
enforcement. The export UI must say that any application, sync service, backup
tool, Spotlight indexer, or agent with filesystem permission may read it; Noted
cannot revoke or reliably delete copies after export. The Context Pass companion
is the recommended path when the user wants per-request inspection and receipts.

## Retrieval architecture

### Derived document model

Add a unified projection over notes and meetings.

~~~text
retrieval_documents
  internal id
  public document id
  canonical resource URI
  source kind and source row reference
  root resource URI
  scope and visibility
  title and event dates
  source revision and hash
  chunker version and index generation
  deletion and error state

retrieval_chunks
  internal id
  public chunk id
  document id and ordinal
  verbatim text
  contextual embedding text
  UTF-8 byte, segment, or timestamp anchors
  speaker metadata
  content hash
  token estimate
  chunker version and index generation

retrieval_chunks_fts
retrieval_chunk_embeddings
retrieval_index_jobs
retrieval_index_generations
~~~

The exact schema belongs to implementation review, but these invariants do not:

- The source record can rebuild the entire index.
- FTS and vector coverage can be inspected independently.
- A provider or chunker change can build a new generation without corrupting the
  active one.
- A vector result is accepted only when its source hash still matches.
- Delete, trash, restore, and retention transitions update all representations.
- Search remains lexical while semantic indexing is absent, stale, or rebuilding.

### Chunking

Notes:

- Keep short notes intact.
- Split long Markdown on headings and paragraph boundaries.
- Preserve verbatim body text and exact UTF-8 byte offsets.
- Add compact title, date, type, and scope context only to embedding_text.
- Keep structured entry data in small source-linked chunks when it adds retrievable
  facts; do not duplicate the entire note into every chunk.

Transcripts:

- Build speaker-aware, segment-aligned windows.
- Start evaluation around 400–800 tokens with modest overlap; tune from results.
- Preserve stable first/last segment IDs and millisecond bounds.
- Put meeting title, date, and participants in embedding_text, not in the quoted
  transcript body.
- Return adjacent turns after a hit when needed for meaning.
- Debounce indexing during live recording and prioritize capture over backfill.

Every chunker version must be explicit. Rechunking produces a new derived
generation without changing resource identity or canonical citations.

### Hybrid retrieval pipeline

1. Parse deterministic scope, date, type, person, and meeting filters.
2. Run FTS5/BM25 candidate retrieval.
3. Attempt query embedding and vector retrieval in parallel when available.
4. Fuse candidate ranks using reciprocal-rank fusion.
5. Boost exact titles, names, identifiers, and quoted phrases.
6. Collapse overlap and duplicate summary/transcript evidence by root resource.
7. Expand neighboring transcript or note context within a strict budget.
8. Optionally add one evidence-backed graph hop for relationship queries.
9. Return bounded verbatim evidence with resolvable citations.

Raw BM25 and vector distances must not be compared directly. A learned or model
reranker is deferred until deterministic hybrid retrieval is measured and a
privacy/cost decision is explicit.

### Graph policy

The graph is not the source of truth and not the first retrieval phase.

- Co-mention means weak association, not a factual relationship.
- Each derived fact needs source chunks, observed/event time, extraction time,
  confidence, and extractor version.
- Automatically extracted identities, mentions, and edges are derived. A
  user-reviewed entity registry may become canonical; its merges, splits, and
  renames must be reversible and survive graph rebuild.
- Graph facts cannot ground an answer without their supporting source.
- Expansion is limited by query class, scope, hop count, and context budget.
- Graph use ships only when the evaluation set shows a material gain over
  lexical-plus-vector retrieval for the relevant query class.
- Community summaries or a full GraphRAG pipeline are deferred unless broad
  corpus questions remain a demonstrated failure.

## Security, privacy, and lifecycle requirements

### Lifecycle ownership

A single service must own source-to-derived transitions for:

- documents and chunks
- FTS rows
- embeddings
- entity mentions and derived graph facts
- generated/unapproved summaries and contextualization
- active snapshot staging files before they are handed to the user
- disclosure passes and receipts

Required operations:

- create
- edit and revision change
- trash
- restore
- permanent delete
- retention expiry
- reindex
- snapshot generation and cancellation

Every permanent-delete test must prove that no internal FTS, vector, graph, active
snapshot staging, or new agent result exposes the deleted source. A completed
manual export is a user-owned disclosure and cannot be covered by that guarantee;
the UI and receipts must say so.

### Backup, restore, and migrations

Before retrieval schema work:

- Introduce ordered transactional migrations with version, name, checksum, and
  applied time.
- Set a configurable SQLite application ID plus explicit schema_version and
  min_reader_version compatibility rules.
- Refuse a database only when its min_reader_version exceeds the binary's reader
  version or its schema_version exceeds the binary's declared maximum compatible
  schema.
- Preserve fixtures for every released schema version.
- Use expand, backfill, cutover, and contract migrations. Do not perform the
  destructive contract step until the supported rollback window has expired.
- Separate same-binary feature rollback from binary downgrade. A compatible older
  reader may open only a schema permitted by min_reader_version; an incompatible
  downgrade requires restoring the pre-migration dataset together with the prior
  binary.
- Create a pre-migration recovery point only when a migration requires one; do not
  create an unlimited full copy at every launch.
- Use SQLite's Online Backup API or VACUUM INTO for the database snapshot without
  blocking ordinary WAL writers for the duration of the copy.
- Refuse or defer a full cross-file backup while a meeting recording is active.
  After recording closes, use a bounded consistency barrier for the referenced
  media/configuration inventory; do not freeze capture for a multi-gigabyte file
  copy.
- Restore always enters maintenance mode, rejects agent requests, stops
  recording/index/export work, and closes database connections before swap.
- Add a versioned backup manifest that inventories the database, selected source
  media, and non-secret configuration.
- Exclude credentials and tokens deliberately.
- Hash and fsync the manifest and files; retry or fail if referenced media changes
  during capture. A backup omitting source media is explicitly labeled incomplete.
- A complete Noted backup includes all Noted-owned records and referenced media.
  Externally authoritative roots are inventoried and rebound on restore; copying
  them is a separate opt-in snapshot because Noted must not imply ownership or
  overwrite those sources.
- Treat meeting-speaker centroids and persistent voice profiles as sensitive
  biometric data in the backup manifest. They are included only in a complete
  encrypted recovery backup, not in plaintext export snapshots.
- Offer an encrypted backup option before describing backups as private.
- Preflight free space and write permissions. Restore with database connections
  closed into staging; validate manifest hashes, PRAGMA quick_check, and
  foreign_key_check; then atomically swap and retain the replaced dataset as a
  bounded rollback generation.
- Make index rebuilds resumable and safe to discard because indexes are derived.
- Test ENOSPC, write denial, interruption, corrupt manifests, corrupt databases,
  missing media, WAL/SHM handling, and restart recovery.

Identity semantics:

- A full replacement restore preserves library_id and record IDs.
- “Import into this library” is a distinct operation. It keeps the source
  library/record IDs in provenance, deduplicates identical hashes, and assigns a
  new local record ID when the same identity has conflicting content.
- Restoring the same library to a second device preserves library_id but assigns
  a new device_id. Future sync must require explicit device pairing and divergence
  reconciliation before both copies can upload.
- Credentials remain excluded. Restore reports every capability requiring
  Keychain reauthentication rather than silently falling back to another provider.

### Agent boundary

- Off by default.
- Registered clients with local revoke controls.
- Baseline client binding uses a per-client secret plus Unix-socket peer-UID
  validation. Same-user processes remain a documented spoofing boundary.
- Optional macOS hardening may attest the peer audit token/code signature. Until
  attested, approval copy labels the client name as claimed rather than verified.
- Purpose, scope, kind, date, byte, token, and expiry limits.
- No raw SQL, arbitrary paths, audio/video, voice centroids/profiles, note writes,
  calendar writes, shell commands, or unbounded corpus reads.
- Source content is marked untrusted and cannot issue tool instructions.
- No note text, transcript text, query text, embeddings, credentials, or private
  paths in production logs.
- Audit records contain disclosure metadata and packet hashes, not another copy of
  the corpus.
- The MCP helper does not reuse the LAN phone RPC bridge.
- Prefer stdio first. Any later HTTP transport requires loopback binding,
  authentication, Origin validation, CSRF/DNS-rebinding defenses, and a separate
  threat review.

### Local data protection

Restrictive filesystem permissions and complete internal deletion are baseline
requirements. Whether application-level database encryption is a launch promise,
how ephemeral Context Pass bytes are protected, and how plaintext export risk is
presented need an explicit ADR before the agent surface or snapshot export ships.
Until then, product copy must not imply encryption at rest that is not present.

The same ADR must distinguish logical deletion from physical scrubbing. It should
evaluate PRAGMA secure_delete, FTS shadow storage, WAL checkpoint/truncation,
vacuum/scrub behavior, retained rollback generations, backups, and SSD
wear-leveling. Product copy may promise that deleted content is no longer
application-retrievable only when that invariant is tested; it must not promise
forensic erasure from physical media where the platform cannot guarantee it.

## Detailed implementation phases

Each phase has an exit gate. Later phases do not begin merely because code exists;
the gate must pass and any material plan change must be reviewed.

Repository integration constraints:

- Every new backend command must remain synchronized across the Tauri command,
  generate_handler registration, phone API dispatch, and src/api.ts bridge.
- Frontend invoke argument keys remain camelCase.
- If a sensitive command should be desktop-only, first define and document an
  explicit command-availability policy instead of silently breaking the current
  desktop/phone command invariant.
- Database locks must not be held across model, network, or filesystem waits.
- Preserve a single writer, but do not run production retrieval or agent reads on
  that writer mutex. Phase 0 must choose a bounded read-only WAL connection design
  and Phase 2 must measure writer contention during recording.
- Existing additive migrations and legacy query paths remain intact until their
  tested replacement and rollback window are complete.

### Phase 0 — Freeze contracts, threat model, and evaluation baseline

Goal: turn the architecture into testable product contracts before modifying
storage.

Planned artifacts:

- architecture ADR covering canonical storage and derived projections
- ContextRecordV1, resource URI, and public-ID specification, including the
  creation-time leakage of UUIDv7 in URIs/receipts/filenames and the accepted
  alternative if that leakage is not justified
- Noted Library format v1
- retrieval evaluation specification and fixture format
- data-flow map for Local, Hosted, and every BYOK capability
- threat models for local storage, export, MCP, prompt injection, and future sync;
  the MCP model defines per-client secrets, peer-UID checks, optional code-sign
  attestation, and the exact same-user spoofing limit
- database connection/concurrency ADR covering one writer, bounded read-only WAL
  connections, active-recording priority, cancellation, and busy handling
- feature-flag and rollback contract
- source-authority matrix for Noted-owned rows, imported Brain/Obsidian files,
  approved memories, attachments, and generated artifacts
- scope/sensitivity migration map covering folder roots, filing_context, origin,
  legacy NULL values, unknown values, journal/self-knowledge, speaker centroids,
  and persistent voice profiles
- timezone contract using UTC instants plus explicit IANA timezone where local
  civil time matters
- legacy migration-baseline contract: converge through the old init path once,
  inspect the resulting shape, then stamp a known baseline before ordered
  migrations
- raw-capture durability UX contract: define submission/recording boundaries and
  whether unsent typed drafts autosave; do not silently change the current
  review-first composer into keystroke persistence
- refreshed repository contributor architecture documentation after the contracts
  are accepted, preserving any unrelated in-progress documentation changes

Required technical spikes:

1. Prove source-authority and scope mapping against current Brain import/export
   behavior.
2. Exercise migration, min_reader_version, pre-migration recovery, and binary
   downgrade on captured database fixtures.
3. Prove how sqlite-vec can hold, filter, validate, promote, and remove active and
   building generations. If safe parallel generations are not feasible, choose
   lexical availability during an in-place rebuild instead of promising atomic
   vector promotion.
4. Open dedicated read-only WAL connections with sqlite-vec registered, run
   representative FTS/vector queries while recording writes continue, and measure
   busy behavior and writer latency before setting the production pool size.

Evaluation corpus:

- Commit a synthetic/redacted corpus for deterministic CI.
- Keep a private, untracked dogfood corpus for realism.
- Begin with at least 150 questions, at least 15 per load-bearing class, across
  exact, semantic, temporal, transcript, relationship, broad-theme,
  negative/no-answer, and permission cases.
- Keep at least 20% held out from tuning.
- Record the legacy result set and latency before changing queries.

Exit gate:

- Product owner accepts the contracts and defaults.
- External plan review findings are resolved.
- Every test question has expected evidence and scope.
- Privacy/data-flow map names every possible outbound data class.
- Source authority, unknown-scope behavior, timezone semantics, downgrade
  behavior, legacy baseline stamping, public-ID leakage, raw-capture boundaries,
  client-identity limits, database concurrency, and vector-generation feasibility
  have explicit decisions.
- Rollout and rollback rules are agreed.
- Implementation is explicitly authorized.

### Phase 1 — Production-safe storage foundation

Goal: make future indexes, exports, and agent references safe across upgrades.

Likely modules and files:

~~~text
src-tauri/src/migrations.rs
src-tauri/src/context_record.rs
src-tauri/src/source_authority.rs
src-tauri/src/backup.rs
src-tauri/src/lifecycle.rs
src-tauri/src/db.rs
src-tauri/src/lib.rs
src-tauri/tests/migration_test.rs
src-tauri/tests/stable_id_test.rs
src-tauri/tests/backup_restore_test.rs
src-tauri/tests/lifecycle_test.rs
src-tauri/tests/raw_capture_offline_test.rs
~~~

#### Phase 1a — Safe upgrade, identity, and recovery

The first approved product-code change, before any schema migration work, is an
interim backup safety patch:

1. Replace the checkpoint-then-copy implementation with VACUUM INTO or the SQLite
   Online Backup API.
2. Before a plaintext handoff, remove speaker centroids and persistent voice
   profiles from the snapshot, compact the sanitized copy, set restrictive
   permissions, and validate it with quick_check. Alternatively, an encrypted
   recovery artifact may retain those biometric tables.
3. If Online Backup is used without a proven compact/scrub step, warn that the
   artifact may contain logically deleted freelist remnants.
4. Label the artifact database-only, plaintext-sensitive, and incomplete until the
   encrypted manifest/media backup is available.

Remaining Phase 1a work:

1. Converge each unversioned legacy fixture through the old init path once, inspect
   the shape, stamp the accepted baseline, then add the ordered migration runner.
2. Persist one library_id and one installation/device_id with the restore and
   clone semantics above.
3. Add and backfill UUIDv7 IDs for notes, entries, meetings, meeting segments,
   user-approved memories/summaries, attachments, and the scope/folder registry.
   Public entity IDs remain deferred until entity identity is user-reviewed.
4. Add revision, updated-time, content-hash, and minimum identity/provenance fields
   needed for backup, restore, and later authority migration.
5. Build the complete backup inventory, consistent database snapshot, referenced
   media/configuration capture, staged restore, validation, identity handling, and
   bounded rollback generation.
6. Enforce restrictive permissions on app data and non-secret configuration.
7. Add startup integrity, free-space, corruption, min-reader, and incompatible-
   schema refusal behavior.

Phase 1a exit gate:

- The interim database-only backup is a validated SQLite snapshot rather than a
  raw live-file copy.
- A plaintext interim artifact contains no live voice-biometric templates and is
  compacted before handoff; a biometric-complete artifact is encrypted.
- Fresh install, daily-driver, alpha-era, and every released fixture establish the
  correct baseline and migrate without source loss.
- An interrupted migration is transactionally recoverable.
- Stable IDs survive edits, reopen, backup, restore, and future reindex.
- Complete backup/restore round-trip covers notes, entries, meetings, transcripts,
  approved memories, configuration, and referenced Noted-owned media; partial
  backups are labeled.
- Restore/import/clone behavior preserves or regenerates identity exactly as
  specified.

#### Phase 1b — Authority, lifecycle, raw ingress, and time

Work:

1. Add authority-origin, canonical scope, sensitivity, and source ownership to the
   approved record types.
2. Resolve scope in this order: explicit canonical scope; registered source-root
   mapping; reviewed legacy filing_context migration; otherwise unknown. Unknown
   scope or sensitivity remains usable in the trusted app but fails closed for
   export and external agents until reviewed.
3. Classify journal/self-knowledge as conservative personal data and voice
   centroids/profiles as biometric data.
4. Define URI resolution against canonical records and stale-revision behavior.
5. Add a transactional change outbox or database-trigger equivalent so every
   direct SQL mutation emits an unavoidable lifecycle event in the same
   transaction.
6. Build the missing normal-note trash, restore, and permanent-delete commands and
   UI; centralize note, meeting, media, approved-memory, retention, and current
   derived cleanup while keeping the outbox as the enforcement backstop.
7. Add person/voice-profile lifecycle controls so deleting a person offers removal
   of associated biometric templates.
8. Audit every typed, voice, photo, meeting, phone, and quick-capture ingress.
   At the Phase 0-defined submission/recording boundary, durably commit raw input
   or retained-media reference before optional enrichment, then queue enrichment
   with visible retry/failure state.
9. Replace hard-coded timezone behavior with the accepted UTC/IANA contract.

Phase 1b exit gate:

- Authority and sensitivity are explicit for Noted-owned, imported, approved,
  generated, journal/self-knowledge, and voice-biometric records.
- Permanent delete leaves no live/application-retrievable representation on
  Phase 1b-managed surfaces. Retained recovery backups/rollback generations,
  completed exports, already delivered bytes, and unprovable physical-media
  remnants remain explicitly outside that claim. The live-surface invariant
  repeats as each later derived surface is added.
- Direct SQL mutations cannot bypass the transactional lifecycle/outbox contract.
- Notes and meetings both support tested trash, restore, and permanent deletion.
- Raw input at the accepted durability boundary survives model/network failure and
  visibly retries enrichment.
- Temporal behavior follows the accepted UTC/IANA contract.
- Core capture/read/export/restore works without a model or network.

Parallelization boundary:

- After Phase 1a, Phase 2 schema/chunker experiments and one-shot shadow indexing
  may begin on fixtures and development data.
- Live background indexing, freshness claims, external dogfood, and any retrieval
  cutover wait for the Phase 1b exit gate.

### Phase 2 — Unified documents, chunks, and offline lexical retrieval

Goal: make every textual source precisely searchable before adding semantic
complexity.

Likely modules and files:

~~~text
src-tauri/src/retrieval/mod.rs
src-tauri/src/retrieval/types.rs
src-tauri/src/retrieval/document.rs
src-tauri/src/retrieval/chunker.rs
src-tauri/src/retrieval/indexer.rs
src-tauri/src/retrieval/lexical.rs
src-tauri/src/db.rs
src-tauri/src/meeting/store.rs
src-tauri/tests/retrieval_chunk_test.rs
src-tauri/tests/retrieval_fts_test.rs
src-tauri/tests/retrieval_lifecycle_test.rs
~~~

Work:

1. Add versioned retrieval documents, chunks, FTS, generations, and job state.
2. Implement deterministic note and transcript chunkers.
3. Index all current textual records asynchronously in resumable batches.
4. Enqueue canonical changes and make deletion cleanup unavoidable.
5. Add exact phrase, identifier, Unicode, date, type, source, and scope filters.
6. Return stable citations and bounded neighboring context.
7. Add retrieval status and rebuild diagnostics without logging content.
8. Shadow-build alongside current note embeddings and transcript FTS.
9. Define one meeting root resource. Index verbatim transcript chunks and
   user-authored meeting notes once; treat generated linked-note mirrors as
   aliases unless a user edit makes them an independent source. Deduplicate
   summary, linked-note, and transcript evidence deterministically.
10. Open retrieval through the Phase 0-selected bounded read-only WAL connection
    path while preserving the single writer for canonical mutations.

Exit gate:

- All selected note and transcript text is represented or explicitly excluded.
- Exact retrieval meets the global gate: unique quote/identifier Recall@1 is 100%
  and exact/identifier Recall@10 is at least 98% on the accepted held-out corpus.
- Citation anchors resolve to current verbatim evidence 100% of the time.
- Scope isolation and trash exclusion are 100%.
- Save, edit, trash, restore, and delete update FTS idempotently.
- FTS reflects committed note edits within one second p95 and closed transcript
  segment changes within two seconds p95 on the reference Mac, subject to Phase 0
  measurement.
- Permanent delete leaves no FTS, document, chunk, or lifecycle-queue exposure.
- Meeting summary/link/transcript fixtures do not triple-count the same evidence.
- Search works with Ollama stopped and network access disabled.
- App launch does not wait for a full backfill.
- Retrieval p95 and SQLite busy rates meet the accepted targets while an active
  recording writes transcript segments; capture latency takes priority.

### Phase 3 — Chunk embeddings and hybrid rank fusion

Goal: add semantic recall without making it a dependency for access.

Likely modules and files:

~~~text
src-tauri/src/retrieval/embedder.rs
src-tauri/src/retrieval/semantic.rs
src-tauri/src/retrieval/fusion.rs
src-tauri/src/retrieval/worker.rs
src-tauri/src/retrieval/service.rs
src-tauri/src/provider.rs
src-tauri/src/ollama.rs
src-tauri/tests/retrieval_hybrid_test.rs
src-tauri/tests/retrieval_recovery_test.rs
~~~

Work:

1. Add chunk vector storage and active/building index generations.
2. Extend the embedding fingerprint with provider, base URL class, model,
   dimension, normalization, contextualization, chunker, and index schema version.
3. Introduce an injectable Embedder so CI uses deterministic fake vectors.
4. Build vectors asynchronously without holding database locks across provider
   calls.
5. Reject stale in-flight results by content hash and revision.
6. Add FTS and vector candidate retrieval plus reciprocal-rank fusion.
7. Add exact-match boosts, overlap collapse, and bounded context expansion.
8. Degrade to lexical-only on provider failure, quota exhaustion, stale index, or
   no model.
9. Run shadow queries and record rank changes without changing answers.
10. Before Hosted or BYOK backfill, show the provider and destination, selected
    scopes/data classes, estimated records/tokens/cost, provider-retention
    implications, and the fact that semantic queries also leave the device.
    Require explicit confirmation, cancellation, batching, quota/rate controls,
    and resumable progress. A provider switch cannot silently upload historical
    notes or transcripts.
11. Keep the active vector generation until its replacement has complete expected
    coverage and passes validation. Garbage-collect orphan generations only after
    a bounded recovery window and free-space check.

Phase entry requirements:

- Native secure credential storage and truthful microphone/system-audio and
  routing copy are complete for every enabled remote provider.
- Local mode has a tested zero-network-egress guarantee.
- The sqlite-vec generation spike from Phase 0 selected a feasible promotion and
  rollback design.

Exit gate:

- Semantic Recall@10 is at least 85% on the accepted note/transcript cases.
- Overall MRR improves at least 10% relative to the recorded legacy baseline,
  unless the review approves a different evidence-based gate.
- No query class drops more than five percentage points without explicit review.
- Provider changes rebuild resumably and atomically.
- Stale async results cannot overwrite a newer revision.
- Lexical fallback passes for every provider failure mode.
- Scope-filtered vector recall meets the benchmark rather than relying on an
  unverified post-filter.
- Vector freshness meets the accepted p95 queue-lag target when a provider is
  available; the initial proposal is 60 seconds after a committed note edit.
- Remote backfill cancellation, quota exhaustion, cost estimation, and zero-egress
  Local mode pass deterministic tests.
- Permanent delete leaves no vector or orphan-generation exposure.

### Phase 4 — Move Noted Ask onto the shared retrieval service

Goal: prove the public retrieval contract inside the product before exposing it.

Likely modules and files:

~~~text
src-tauri/src/retrieval/query.rs
src-tauri/src/retrieval/citation.rs
src-tauri/src/retrieval/policy.rs
src-tauri/src/retrieval/graph.rs
src-tauri/src/lib.rs
src-tauri/src/pipeline.rs
src/AskView.tsx
src/MeetingPage.tsx
src/Settings.tsx
src/api.ts
~~~

Work:

1. Preserve the existing action router and mutation-confirmation behavior.
2. Replace only evidence retrieval behind a flag.
3. Keep compatibility fields for one release while adding resource URIs, source
   anchors, timestamps, and excerpt bounds.
4. Make source chips open the exact note or meeting timestamp.
5. Delimit evidence as untrusted source data in answer prompts.
6. Put graph expansion behind query policy and an evaluation flag.
7. Add diagnostics for fallback reason, coverage, queue state, and stage timing.
8. Compare legacy and v2 retrieval in shadow mode before cutover.

Exit gate:

- Ask works in Local, Hosted, BYOK, and lexical-only states.
- Date-scoped and exact queries remain deterministic.
- Every answer citation is a member of the retrieved evidence set.
- Source chips resolve exactly.
- Existing writes still require confirmation.
- Prompt-injection fixtures cannot change permissions or invoke mutations.
- Legacy retrieval remains recoverable with one local flag for at least one stable
  release.

### Phase 5 — Permission-gated MCP Context Pass companion

Goal: let compatible agents request useful context without granting them the
library or database. This is the first portable agent object and takes priority
over the broader plaintext snapshot in Phase 6.

Likely modules and files:

~~~text
src-tauri/src/context_pass.rs
src-tauri/src/approval_broker.rs
src-tauri/src/mcp/mod.rs
src-tauri/src/mcp/server.rs
src-tauri/src/mcp/permissions.rs
src-tauri/src/mcp/resources.rs
src-tauri/src/bin/noted-mcp.rs
src-tauri/Cargo.toml
src-tauri/tauri.conf.json
scripts/install-app.sh
src/Settings.tsx
src/api.ts
src-tauri/tests/agent_policy_test.rs
src-tauri/tests/mcp_contract_test.rs
~~~

Process architecture:

1. The running Noted app is the sole database writer, migrator, retriever, packet
   freezer, approval UI, and receipt owner.
2. It exposes an authenticated, user-scoped Unix domain socket with restrictive
   permissions. The socket is unavailable during migration, backup maintenance,
   or restore.
3. The signed stdio helper is a narrow MCP-to-broker adapter. It authenticates the
   configured client, forwards opaque requests, and never opens, migrates, or
   writes the database directly.
4. If Noted is closed, the helper returns a clear approval-required state and may
   offer an explicit app-launch action. It never falls back to unapproved reads.
5. Candidate inspection, edits, exclusions, and receipts exist only in the
   trusted Noted UI.

Initial protocol surface:

~~~text
request_context(purpose, query, requested_scopes, limits)
context_request_status(request_id)
read_context_pass(pass_id, cursor)
~~~

Before approval, context_request_status returns only an identity-bound opaque
state: pending, approved, denied, expired, or app-required. It exposes no
candidate titles, snippets, paths, counts, exclusions, or other corpus metadata.
read_context_pass paginates the final frozen bytes and cannot dereference a live
resource. Request IDs and pass IDs are unguessable and bound to the authenticated
client; the protocol never accepts a client_id override.

Notably absent in v1:

- unrestricted search returning private snippets
- raw SQL
- arbitrary file paths
- full-corpus resources
- audio or video reads
- write, edit, delete, calendar, or shell tools

Work:

1. Resolve the ephemeral packet encryption/storage ADR and maximum payload/TTL.
2. Build the app-owned authenticated broker and maintenance-mode denial behavior.
3. Package and sign the stdio companion for official macOS builds.
4. Add explicit enablement, per-client secrets, peer-UID checks, verification
   state, scopes, and revoke. A caller cannot supply another client_id; optional
   code-signature attestation is a separate hardening layer.
5. Apply the Phase 0 relevance/disclosure budget before presenting candidates;
   optimize for a small sufficient packet rather than recall-driven padding.
6. Freeze retrieval results, final user edits, source revisions, ranges, and hash
   into inspectable Context Pass candidates.
7. Add approve, edit, deny, expiry, source-change invalidation, inspectable
   non-content receipt, cleanup, and crash-recovery flows.
8. Enforce permissions in the retrieval service and broker, not only the protocol
   adapter.
9. Bound responses and paginate only approved frozen bytes.
10. Test concurrent app writes, edit-after-approval races, app-closed behavior,
   maintenance mode, and client reauthentication.
11. Test malformed URIs, socket impersonation, stolen same-user client secrets,
    path traversal, oversized
    requests, prompt injection, expired passes, revoked clients, and cross-scope
    requests.
12. Document how open-source builds package and identify the helper without
    official Hosted credentials.

Exit gate:

- Disabled by default.
- No content crosses before approval of the exact packet.
- Candidate approval names the receiving client, warns that its downstream model
  may be unknown, labels claimed versus attested identity truthfully, and renders
  all content as inert text.
- Candidate precision, irrelevant-evidence rate, and packet size meet the accepted
  disclosure-minimization gate; approval edits/removals are measured in dogfood.
- Scope isolation is 100% across contract and adversarial tests.
- Revoke blocks new reads cleanly.
- Denial, expiry, delivery completion, and revoke delete frozen payload plaintext;
  receipts retain metadata/hash only.
- Post-approval source edits cannot alter the delivered bytes.
- A correction/removal during pagination invalidates the pass, erases undelivered
  bytes, and records partial delivery without claiming retraction.
- Receipts remain inspectable through resource/revision/range and delivery
  metadata, including claimed-versus-attested client identity at handoff, after
  all excerpt text is erased.
- No protocol path can mutate canonical records or exported snapshots.
- The helper cannot read the database directly and fails closed when the app
  broker is unavailable or in maintenance mode.
- At least two target MCP clients pass conformance tests.
- The signed helper survives install and app update validation.
- Receipts describe local disclosure truthfully and do not claim remote erasure.
- Permanent deletion prevents new passes from retrieving the source; previously
  delivered bytes remain correctly described as outside Noted's control.

### Phase 6 — Manual portable Noted Library snapshots

Goal: give humans and tools a durable, inspectable bulk export without creating a
second writable truth or implying per-request Context Pass enforcement.

Phase 6 is an independent sibling after Phase 4 and does not block the safer
permissioned companion in Phase 5.

Likely modules and files:

~~~text
src-tauri/src/library/mod.rs
src-tauri/src/library/manifest.rs
src-tauri/src/library/render.rs
src-tauri/src/library/snapshot.rs
src/Settings.tsx
src/api.ts
docs/NOTED_LIBRARY_FORMAT.md
src-tauri/tests/portable_library_test.rs
~~~

Work:

1. Implement manifest, sentinel, and format-version validation.
2. Add a manual export destination, scopes, data classes, and exact exposure
   preview with the bulk-disclosure warning.
3. Render notes, meeting documents, transcript JSONL, and readable Markdown.
4. Generate into a sibling staging directory, hash/fsync, then atomically promote
   a complete snapshot where the platform permits.
5. For an explicit replacement, require a matching library sentinel and preserve
   or quarantine unknown/externally modified files for user resolution.
6. Keep attachments and derived graph data excluded by default.
   Never include speaker centroids or persistent voice profiles in v1 snapshots.
7. Add a “verify snapshot” action and future portable-import fixtures.
8. Ensure export failure never blocks canonical operations.
9. Do not schedule continuous background refresh in v1.

Snapshot semantics:

- A no-op repeat into the same managed destination preserves the prior manifest
  and generation timestamp and performs no writes.
- A newly requested snapshot has a new generation time.
- After export, the user owns the copy. Noted does not claim that later source
  edits or deletes remove stale exported bytes.
- “Replace,” “export elsewhere,” and “detach and leave files” are explicit
  actions; there is no ambiguous enable/disable background state.

Exit gate:

- An unchanged repeat into the same managed destination is byte-for-byte stable
  and performs no writes.
- Manifest hashes verify and exported scope matches the approved preview.
- Crash recovery leaves an old or new complete snapshot, never a partial promoted
  snapshot.
- Unknown and externally modified files are never silently deleted.
- Conflicting files that contain deleted or out-of-scope data require explicit
  replace, quarantine, or detach resolution; the UI does not falsely claim
  revocation.
- The snapshot is readable on a clean machine without Noted.
- No background export writes occur.

### Phase 7 — Evidence-backed graph augmentation

Goal: improve relationship and multi-hop questions only where the graph proves
valuable.

Work:

1. Add source chunk, time, confidence, and extractor version to derived facts.
2. Distinguish weak co-mention from asserted relationships.
3. Define the user-reviewed canonical entity registry separately from derived
   mentions and edges.
4. Make identity merge, split, and rename reversible.
5. Create relationship and multi-hop evaluation slices.
6. Compare lexical-plus-vector against lexical-plus-vector-plus-graph.
7. Enable bounded graph expansion only for query classes with material gains.
8. Keep global community summaries and a dedicated graph database deferred.

Exit gate:

- Every graph-derived answer claim resolves to canonical source evidence.
- User-confirmed identities are preserved through rebuild.
- False merges are reversible.
- The benchmark shows a pre-agreed gain for enabled query classes.
- Latency and disk cost stay inside the accepted budget.

### Phase 8 — Consumer and open-source release hardening

This track begins during Phase 0 and progresses in parallel; it is not postponed
until retrieval is complete.

Repository and governance:

- Select and add a license.
- Add CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, privacy, and support policies.
- Publish third-party, model, and data-routing notices.
- Define the boundary between community client code and official hosted service
  operations.
- Ensure clean builds do not require personal paths, secrets, or official service
  credentials.
- Make the product identity tuple configurable for forks: bundle ID, display
  name, app-data directory, Keychain service, updater feed/signing identity, MCP
  helper identity, URL scheme, and SQLite application ID.
- Put official Hosted integrations behind an official_services build capability
  that defaults off for forks.
- Keep immutable upstream core migrations in one namespace and fork/extension
  migrations in an extension_id plus version/checksum namespace so independently
  maintained builds cannot collide with upstream migration numbers.

Consumer defaults:

- Remove or gate personal Brain folders and automatic git/background propagation
  before external dogfood, not only before public release.
- Remove personal/product-specific seeded folder trees such as Baro, Daily
  Standup Meeting Notes, and Symphony from fresh consumer databases; use neutral
  onboarding choices or user-selected templates.
- Make every watched folder explicit, previewed, revocable, and off by default.
- Complete the Phase 1 UTC/IANA timezone migration before temporal retrieval
  cutover.
- Make capability routing and outbound data destinations truthful and visible.
- Ensure the offline baseline remains useful with model-dependent features
  clearly marked as enhancements.

Security and release:

- Review CSP, Tauri capabilities, private APIs, secure storage, and purpose strings.
- Complete native credential storage and truthful routing/purpose strings before
  Phase 3 can send historical content to a remote provider.
- Add dependency and license audits, secret scanning, SBOM, and artifact provenance.
- Put tests and migration preflight before packaging in CI.
- Add signed updater rollback and database compatibility checks.
- Validate Developer ID signing, hardened runtime, notarization, stapling,
  codesign, and Gatekeeper.
- Test clean macOS users on Apple Silicon and Intel.
- Set one truthful minimum macOS version.
- Keep macOS capture and credentials behind adapters for later ports without
  delaying the macOS v1.

Hosted boundary:

- Local and BYOK can support a production-oriented open beta.
- Hosted remains disabled or clearly non-production until tenant isolation,
  retention/deletion, quotas, abuse controls, observability, backup, incident
  response, redundancy, billing, and model-license review pass independently.

Exit gate:

- A new consumer can install, use, update, back up, restore, and uninstall without
  developer intervention.
- A clean open-source build works without private infrastructure.
- Product copy matches actual local, BYOK, and hosted data behavior.
- Release artifacts and migrations pass clean-machine validation.
- Security and privacy reporting paths exist before public distribution.

## Rollout and compatibility strategy

Use local, documented flags rather than a proprietary remote flag service:

~~~text
retrieval_v2_shadow_index
retrieval_v2_query
graph_expansion_v2
portable_snapshot_export
mcp_context_pass_enabled
~~~

Rollout:

1. After Phase 1a, allow fixture/dev one-shot shadow builds; after Phase 1b, allow
   live background documents, chunks, and FTS in shadow mode.
2. Build chunk vectors without changing answers.
3. Run shadow queries and compare ranks, citations, latency, and leakage.
4. Enable retrieval v2 for development and dogfood.
5. Enable for release candidates after migration and benchmark gates.
6. Move internal Ask to v2 with legacy fallback.
7. Keep legacy note embeddings and transcript search for at least one stable
   release and a tested same-binary feature rollback window. Binary downgrade
   follows min_reader_version or restores the pre-migration dataset.
8. Ship external Context Pass access only after Noted itself uses the same policy
   and retrieval service.
9. Offer manual portable snapshots independently after the export warning,
   authority, and conflict contracts pass.
10. Remove old derived indexes only after production evidence; never remove
    canonical source content as part of index migration.

At implementation start, re-run a worktree audit. The current repository contains
unrelated in-progress edits in several likely overlap files, including database,
meeting, backend command, API, and UI code. Each phase must preserve or explicitly
integrate those changes rather than treating the current branch as clean.

## Evaluation and validation matrix

### Query classes

| Class | Examples | Required behavior |
|---|---|---|
| Exact | quote, ID, uncommon name | FTS-first precise source |
| Semantic | paraphrase, remembered idea | hybrid result with source evidence |
| Temporal | yesterday, date range, sequence | deterministic filters before ranking |
| Transcript | who said what, meeting moment | speaker and timestamp citation |
| Relationship | person/project association | graph optional, source required |
| Multi-hop | event connected through two records | bounded expansion, no unsupported leap |
| Global | recurring themes across many records | diversified evidence; advanced summaries only if needed |
| Negative | absent or conflicting fact | no fabricated answer; surface uncertainty/conflict |
| Permission | excluded scope or kind | zero leakage, including titles and snippets |
| Lifecycle | edited, trashed, deleted source | fresh results and no stale representation |

### Metrics

- Recall@5 and Recall@10
- Precision@5/10 and irrelevant-evidence rate
- mean reciprocal rank and nDCG
- exact citation-span validity
- scope-filter isolation
- freshness after edit/delete
- answer-source faithfulness
- p50 and p95 stage latency
- indexing throughput and queue lag
- disk growth per 1,000 notes and transcript hour
- estimated and actual remote embedding volume/cost
- candidate Context Pass bytes and user removal/edit rate before approval
- SQLite busy rate and capture-write latency while retrieval is active
- recovery time after interruption
- lexical fallback success rate

Provisional gates to lock during Phase 0:

- Unique exact quote/identifier Recall@1: 100% on the held-out set.
- Exact/identifier Recall@10: at least 98%.
- Semantic Recall@10: at least 85%.
- Context Pass candidate Precision@5: at least 90%, with an irrelevant-evidence
  rate no higher than 10% on the held-out disclosure set; Phase 0 may tighten
  these after baseline measurement.
- Scope isolation: 100%.
- Citation resolves against its exact retained revision or returns an explicit
  stale result: 100%.
- No stale result after permanent deletion: 100%.
- Lexical query p95: under 100 ms at 100,000 chunks on the reference Mac.
- Hybrid database/rank-fusion stage p95: under 250 ms excluding provider latency.
- App capture, read, exact search, export, backup, and restore: functional with
  models and network disabled.
- “Capture works” means raw input is durably committed at the accepted
  submission/recording boundary before enrichment; unsent composer text is not
  implicitly included unless the Phase 0 UX contract deliberately chooses draft
  autosave. Enrichment may remain queued and visibly retry later.
- Phase 0 cannot exit until numerical ceilings for disk growth, initial/backfill
  throughput, steady-state queue lag, remote embedding spend, SQLite busy rate,
  and capture-write latency under retrieval load are recorded on the reference
  corpus and hardware.

Latency numbers are provisional until the reference hardware and corpus are
recorded. They should be tightened or relaxed from measurements, not silently
missed.

Permission, authority, lifecycle, and deletion invariants require property-based
and adversarial tests in addition to curated retrieval questions.

### Deterministic test suites

~~~text
src-tauri/tests/migration_test.rs
src-tauri/tests/legacy_baseline_test.rs
src-tauri/tests/stable_id_test.rs
src-tauri/tests/backup_restore_test.rs
src-tauri/tests/raw_capture_offline_test.rs
src-tauri/tests/db_retrieval_concurrency_test.rs
src-tauri/tests/voice_profile_lifecycle_test.rs
src-tauri/tests/retrieval_chunk_test.rs
src-tauri/tests/retrieval_fts_test.rs
src-tauri/tests/retrieval_hybrid_test.rs
src-tauri/tests/retrieval_lifecycle_test.rs
src-tauri/tests/retrieval_recovery_test.rs
src-tauri/tests/retrieval_scale_test.rs
src-tauri/tests/portable_library_test.rs
src-tauri/tests/agent_policy_test.rs
src-tauri/tests/mcp_contract_test.rs
~~~

Normal CI uses fake deterministic embedders and mocked protocol clients. Live
Ollama, Hosted, and BYOK smoke tests remain opt-in so core correctness never
depends on a downloaded model, network, or customer quota.

## Observability without surveillance

Settings diagnostics should show:

- documents and chunks discovered
- FTS and vector coverage
- active and building index generations
- embedding fingerprint and chunker version
- pending, retrying, and failed job counts
- last successful index/snapshot time
- last snapshot scope and destination
- agent clients, current permissions, outstanding requests, and receipts
- derived index and snapshot disk usage
- rebuild, retry, verify, revoke, and clear-derived-index controls

Structured diagnostics may record durations, candidate counts, fusion counts,
fallback reason, queue lag, and error class. They must not record queries, note
text, transcript text, embeddings, credentials, or private paths. Any aggregate
product telemetry is opt-in.

## Explicit non-goals for the first implementation

- Replacing SQLite with a graph database.
- Making a directory of Markdown files a second live writable truth.
- Syncing or importing edits made to a Noted Library snapshot.
- Continuously mirroring the full corpus into an agent-readable plaintext folder.
- Giving agents raw database or unrestricted filesystem access.
- Agent write/edit/delete tools.
- Always-on full-library consent.
- Syncing derived vectors or FTS indexes.
- Shipping GraphRAG community summaries by default.
- Building cross-platform UI before macOS consumer readiness.
- Treating the existing LAN phone API as an MCP transport.
- Making Noted Hosted a launch dependency.
- Claiming remote revocation of content already disclosed.

## Open decisions and proposed defaults

| Decision | Proposed default | Must be resolved by |
|---|---|---|
| Public ID | UUIDv7, integer IDs internal; explicitly accept timestamp leakage or choose v4 | Phase 0 |
| Source authority | Explicit per origin; SQLite for Noted-owned v1 records, registered files for import-owned sources | Phase 0 |
| Long-term durable layer | Revisit portable-file hypothesis after Phase 4 and before sync/another platform | Phase 0 ADR and revisit gate |
| File export | Manual scoped snapshot with bulk-disclosure warning | Phase 0 |
| Agent disclosure | Exact Context Pass approval per request | Phase 0 |
| MCP transport | Local stdio adapter to authenticated app-owned Unix broker | Phase 0 |
| MCP client identity | Per-client secret plus peer UID; claimed until optional code-sign attestation succeeds | Phase 0 |
| Database concurrency | One writer plus bounded read-only WAL retrieval connections | Phase 0 |
| Scope fallback | Unknown scope/sensitivity fails closed externally | Phase 0 |
| Journal/voice sensitivity | Journal defaults personal; voice templates biometric and excluded externally | Phase 0 |
| Raw capture boundary | Persist after explicit submit/recording boundary; unsent draft autosave is a separate UX choice | Phase 0 |
| Deletion guarantee | Tested application-unreachable deletion; physical scrub claims only where platform-proven | Phase 0 |
| Citation text unit | UTF-8 byte offsets plus revision/hash | Phase 0 |
| Graph role | Disabled by default; selective one-hop expansion | Phase 0 |
| Reranking | Deterministic RRF; no model reranker initially | Phase 0 |
| Vector dimensions | Preserve 768 for v1, version full fingerprint | Phase 0 |
| Attachments | Excluded from snapshots and agents by default | Phase 0 |
| Packet/data encryption | ADR required; protected ephemeral pass payload and encrypted backup minimum | Before Phase 5 |
| Open-source license | Owner decision; permissive license with patent grant is a candidate | Before public repository |
| Official Hosted | Separate capability and operations boundary | Before public beta copy |
| Legacy index removal | No earlier than one stable release after v2 cutover | Phase 4 |

## Definition of done

The agent-context initiative is complete when:

- Every addressable source has a stable portable identity and resolvable citation.
- Upgrades, backup, restore, trash, permanent delete, and recovery are tested.
- Database backup never relies on copying a live mutable SQLite file.
- Notes and transcripts share one document/chunk retrieval model.
- Exact lexical access works without a model or network.
- Hybrid retrieval meets the accepted benchmark and degrades cleanly.
- Retrieval read concurrency meets latency targets without starving capture writes.
- Noted Ask uses the same typed, source-grounded retrieval core planned for agents.
- Source authority is explicit for Noted-owned, externally owned, generated, and
  user-approved records.
- User-approved memories survive every derived-index and graph rebuild.
- The graph is either measurably useful and evidence-backed or remains disabled.
- The optional Noted Library snapshot is deterministic, scoped, inspectable, and
  explicitly described as a non-revocable bulk disclosure.
- External clients receive only exact approved Context Pass content.
- Approval names and verification levels do not overstate client identity.
- No agent tool can mutate records or escape scope.
- Journal/self-knowledge and voice-biometric data follow their conservative
  sensitivity, retention, export, and deletion policies.
- Consumer builds contain no hidden personal paths, credentials, or background
  propagation.
- Clean open-source builds and signed consumer releases pass their respective
  validation paths.
- Documentation truthfully explains storage, outbound routing, deletion,
  disclosure, portability, and limitations.

## Review checklist before implementation

The owner and external reviewer should try to falsify the plan, especially:

- Is SQLite still the right operational source for the expected scale and sync
  horizon?
- Does ContextRecordV1 contain enough identity, revision, tombstone, provenance,
  and sensitivity information for future encrypted sync?
- Can every canonical write, edit, trash, restore, and delete reach lifecycle
  cleanup?
- Can stale async embeddings, pass payloads, or snapshot staging reintroduce old
  private text?
- Can filtered vector retrieval silently lose scoped results?
- Can retrieval readers meet their latency budget without blocking transcript or
  capture writes?
- Does exact-packet approval remain usable enough for the product loop?
- Does the candidate packet minimize irrelevant disclosure as well as maximize
  recall?
- Does approval copy distinguish a claimed client name from an attested peer?
- Can an external client learn sensitive titles or existence metadata before
  approval?
- Are resource URIs stable across restore, import, rename, and rechunk?
- Does a portable plaintext snapshot create unacceptable exposure on synced or
  indexed folders?
- Is the rollback window long enough for real consumer upgrades?
- Can the product remain fully accessible during provider and model outages?
- Are graph facts presented with enough uncertainty and provenance?
- Are voice profiles, journal content, WAL/FTS remnants, and physical-deletion
  limits represented truthfully?
- Does any implementation detail unnecessarily bind future clients to macOS,
  Tauri, Ollama, or one hosted provider?
- Are the benchmark targets strict enough to prevent an impressive demo from
  hiding retrieval regressions?

Suggested external review prompt:

> Review this plan as a skeptical staff-plus storage, retrieval, privacy, and
> consumer-app architect. Do not assume its conclusions are correct. Identify
> data-loss paths, migration hazards, permission bypasses, stale-index races,
> portability traps, open-source/official-service coupling, evaluation blind
> spots, and unnecessary complexity. For every objection, cite the affected
> section, severity, failure scenario, and a concrete correction. End with a
> phase-order recommendation and a ship/no-ship verdict for beginning Phase 0.

## Next action

No product code has changed. The owner should review the reconciled Fable Max
record and explicitly authorize implementation. After authorization, begin Phase
0 contract artifacts; the first product-code change remains the Phase 1a interim
backup safety patch.
