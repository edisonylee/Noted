# Noted mobile companion implementation plan

Status: M0 direction accepted; M1 checkpoint complete; M2 native shell and
local-only Notes proven on physical hardware; the M3 portability implementation
checkpoint is complete; the M4 encrypted sanitized-fixture Notes sync checkpoint
is complete, but native product-network, production, external-review, and
personal-data gates remain closed; no sync has shipped

Date: 2026-08-14

Last updated: 2026-08-20

Scope: iPhone first, while preserving macOS as the primary capture and intelligence platform

Accepted direction: [Decision 006](decisions/006-iphone-companion-direction.md)

Sync and provider sequence: [Decision 007](decisions/007-mobile-sync-sequencing-and-provider-gate.md)

This document defines how Noted becomes a real mobile product. It is intentionally
not a plan to turn the dormant LAN web view back on. The target is an offline-capable
iPhone companion that carries the user's Noted library, accepts durable changes,
syncs safely, and delegates AI-dependent work to the Mac.

It builds on:

- [Product strategy](../PRODUCT_STRATEGY.md)
- [Roadmap](../ROADMAP.md)
- [Decision 002: cloud boundaries](decisions/002-context-cloud-boundaries.md)
- [Decision 003: record authority and portability](decisions/003-context-record-authority-and-portability.md)
- [Decision 004: database concurrency](decisions/004-database-concurrency-and-index-generations.md)
- [Decision 005: Context Pass and client identity](decisions/005-context-pass-and-client-identity.md)
- [Mobile capability ledger](mobile/capability-ledger.yaml)
- [ContextRecordV1](agent-context/context-record-v1.md)
- [Operational contracts](agent-context/operational-contracts.md)
- [Phase 0 gate](agent-context/phase-0-gate.md)

The implementation must not begin by silently treating proposed Decisions 003–005
as accepted. Mobile milestone M0 below reconciles and explicitly accepts the record, migration,
identity, lifecycle, and disclosure defaults needed by mobile sync.

## Executive decision

Build a **native Tauri 2 iPhone companion** that reuses Noted's React experience
but owns a local SQLite replica and a platform-specific iOS capability layer.

The operating model is:

- The iPhone can browse, search, edit, capture, and organize without the Mac or
  network being available.
- The Mac remains the primary meeting recorder and runs transcription, OCR,
  extraction, embeddings, summaries, and the conversational assistant.
- The phone does not show Ask, Live Assist, model controls, or provider settings.
- Model-derived artifacts that already exist may be displayed on the phone.
- New phone captures have a visible lifecycle such as **Saved on this iPhone**,
  **Synced**, **Waiting for Mac processing**, **Processing on Mac**, and **Ready**.
- A custom application-layer encrypted relay is the durable remote sync target.
  It stores encrypted records and blobs, not readable personal context.
- A direct paired Mac transport is implemented first to prove convergence and
  enable private dogfooding before cloud operations are introduced.
- No hosted sync provider is selected, provisioned, or paid for until the direct
  Notes gate passes. Paid managed infrastructure is acceptable later at M7 when
  off-LAN continuity, recovery, or a tester cohort creates a concrete need.
- Internal TestFlight is the default private native distribution channel during
  development. Ad Hoc remains the strict off-store fallback. An unlisted App
  Store release is the durable long-term native channel if desired.

The existing Home Screen web app is useful as a temporary UI or transport
prototype only. It is Mac-dependent, has no offline corpus, and has security
problems that make re-enabling it as-is unacceptable.

## Why this is the recommended shape

The user's request has three independent requirements:

1. **Same product coverage:** Calendar, notes, meetings, Today, knowledge, and
   capture need mobile experiences.
2. **Independent availability:** the phone must remain useful while the Mac is
   asleep, closed, or on another network.
3. **Private delivery:** a public App Store listing is unnecessary.

A responsive web client solves only the first requirement. Private distribution
solves only installation. Neither creates a synchronized data model. The hard
foundation is an offline replica, portable record identities, conflict semantics,
encrypted blobs, and recovery.

Native Tauri is preferred because it keeps the existing React design and much of
the shared Rust/TypeScript domain logic while adding iOS Keychain, background file
transfer, notifications, local SQLite, camera/microphone, local-network discovery,
and a credible long-term installation channel. Tauri explicitly supports iOS and
Swift mobile plugins:

- [Tauri mobile prerequisites](https://v2.tauri.app/start/prerequisites/)
- [Tauri mobile plugin development](https://v2.tauri.app/develop/plugins/develop-mobile/)
- [Tauri iOS signing](https://v2.tauri.app/distribute/sign/ios/)

## Product contract: what “everything except the assistant” means

The phone should match the user's information and organizational capabilities,
not every macOS-only hardware or administration control.

| Surface | iPhone product contract | Offline behavior | Mac or service responsibility |
|---|---|---|---|
| Today / Schedule | Date browsing, daily schedule, commitments, task/schedule changes, upcoming meetings, join links, recent context, and calendar-detail creation | Full local day/range; changes queue | Mac produces AI-derived preparation and extracted commitments |
| Calendar | Day, three-day, and week views; event detail; attendees, Meet links, description and reminders; create, edit, delete; calendar selection and reconnect states | Cached events remain readable; writes show pending state | Direct Google access on each device is the end state |
| Capture | Text, photo, short voice note, dictation, handwriting/photo input, and share-to-Noted | Submit immediately to a durable local outbox | Mac later performs OCR, transcription, extraction, and filing |
| Notes | Inbox, Needs filing, Meetings, Trash, spaces/folders, topics, sorting, transcript facets, search, detail, create/edit, file/move/reorder, undo filing, trash, restore, and guarded purge | All canonical text and structure local; lexical search available | Mac maintains semantic indexes and AI organization |
| Meetings | List/search/filter, detail, transcript search/copy/play-from-time, generated and edited summary, participants, rich user notes, speaker corrections, citations, export/share, media deletion, lifecycle, and retained-media playback | All text local; selected media cached or streamed | Mac captures system audio and produces transcript/summary |
| Knowledge / People | Search/filter, evidence navigation, related meetings/notes, rename, merge, and accept/dismiss suggestions | Approved records and versioned evidence projection local | Mac computes mentions, embeddings, merge suggestions, and refreshed projections |
| Recaps / Trends | Show existing generated recaps and deterministic analytics that are part of the product | Last generated artifact remains available | Mac generates new AI recaps; deterministic aggregates may run on either device |
| Weather | Compact current conditions and forecast in Today, with location, freshness, refresh, and stale state | Last cached snapshot remains visible with its age | iPhone fetches directly; no Mac or sync dependency |
| Search | Exact and local FTS across synced text | Fully available | Semantic search may remain Mac-only initially |
| Settings | Sync, paired devices, storage, calendar accounts/reminders, time zone, appearance, meeting-note preferences, privacy, export request, and about | Local settings always available | Provider, model, Mac recording, vault-source, and system permissions remain desktop-only |

This contract is based on the supported standard profile. Hidden Journal,
Self/Work experiments and release-validation-only Alpha capabilities are not
silently promoted into the mobile scope.

### Capability and AI-action classification

“No assistant” means no conversational assistant surface. It does not mean that
the phone may never request a bounded job from the Mac.

| Class | Mobile rule | Examples |
|---|---|---|
| Conversational surface | Never ship in the iPhone bundle | Ask, entity chat, meeting copilot, Live Assist |
| Existing derived artifact | View and cite; clearly label source/freshness | Summary, recap, extracted commitment, entity projection |
| User-triggered Mac job | Queue a typed idempotent job and show its state | OCR/transcription after capture, generate or refresh summary, rediarize, refresh recap |
| Desktop administration | Hide on iPhone | Provider/model setup, model download, global reindex/backfill, recorder/system permissions |

Generated output and user-authored edits are separate. The immutable generated
artifact retains its source revision and generator lineage. A user edit creates a
canonical editable overlay/replacement with its own revision. Regeneration never
silently overwrites that user-authored content.

### Required parity ledger

The initial machine-readable ledger is maintained in
[mobile/capability-ledger.yaml](mobile/capability-ledger.yaml). It records the
current local Notes prototype, every planned standard-profile action, revived
scope, and every intentional exclusion with a stable contract-test identifier.
The ledger changes with implementation evidence; this section defines the fields
and completeness rule.

Before each surface is called complete, maintain a machine-readable ledger with:

~~~text
desktop capability
standard-profile availability
mobile behavior
offline behavior
Mac/job dependency
target phase
intentional exclusion and reason
contract-test identifier
~~~

The first ledger must include every current non-assistant action in these groups:

- **Notes:** Inbox, Needs filing, Meetings and Trash saved views; spaces; nested
  folder create/rename/move/reorder/delete; file and undo filing; topics; sorting;
  note/transcript facets; create/edit/copy/share/export; trash/restore/purge.
- **Meetings:** list/search/filter; transcript search/copy/play-from-time; rich
  notes; title and generated-summary overlay edits; speaker labeling; citations;
  copy/share/PDF export; retained-media playback/delete; trash/restore/purge.
- **Today/Schedule:** date navigation; schedule/task create/edit/delete/complete;
  join links; calendar details; text, voice-note, and photo/handwriting capture.
- **Calendar:** range and calendar selection; attendees; conferencing links;
  descriptions; reminders; create/edit/delete; OAuth/reconnect and conflict state.
- **People/Knowledge:** search/filter; evidence/source navigation; rename; merge;
  redirect; accept/dismiss suggestions; related notes and meetings.
- **Recaps/Trends:** range/filter selection, deterministic refresh, generated
  refresh job, and stale/empty/error state.
- **Settings:** time zone, appearance, calendar accounts/reminders,
  meeting-note preferences, device/sync/privacy/storage, export, and account
  lifecycle.

A release gate compares this ledger to the registered standard-profile routes and
commands. A missing action must be implemented, explicitly excluded with an owner
decision, or blocked by an iOS platform limitation. Merely having a route does not
count as parity.

### Explicit mobile exclusions

- Conversational Ask and Live Assist.
- Local or hosted model selection and provider-key management.
- Mac system-audio, meeting-app, screen, or window capture.
- Mac-only model downloads, diarization setup, and recorder permissions.
- Raw filesystem/vault administration.
- Voiceprint or speaker-biometric synchronization by default.
- Automatic upload of retained audio or video without a separate user choice.
- Multi-hour in-person meeting recording in v1. Mobile v1 captures short voice
  notes; a full foreground meeting recorder is a later separately tested feature.
- Guaranteed always-on background execution. iOS schedules background work; the
  app must remain correct if a requested background task never runs.

An iPhone must not promise Mac-equivalent capture of audio from calls or other
apps.

### Mobile information architecture

The mobile app should adapt Noted's warm/Geist design rather than compress the
desktop sidebar into a narrow viewport.

Recommended five-tab structure:

1. **Today**
2. **Calendar**
3. **Capture**
4. **Notes**
5. **More**

Notes is the only owner of Inbox, Needs filing, Meetings, spaces/folders, and
Trash. More contains People, Knowledge, Recaps, Trends, and other shipped
standard-profile sources. Settings opens from the profile/sync control, not as a
permanent tab. Ask is absent. Search may be reached globally, but results deep-link
back to the owning destination rather than creating another duplicate hierarchy.

Every data-bearing screen must visibly handle:

- current and last-synced state;
- stale but usable data;
- local unsynced changes;
- waiting for Mac processing;
- retryable errors;
- an edit conflict that preserved both versions;
- content deliberately unavailable on this device; and
- a revoked or expired device session.

The phone should never replace the entire screen with an offline blocker. Last
known data stays usable, and writes enter the outbox.

### Replication and storage default

- All canonical Noted text, transcript text, structure, user corrections, and
  approved people/entity records are stored locally on the phone.
- Initial bootstrap order is: library/device state; Today and the near calendar
  window; note/meeting metadata; all canonical text; thumbnails; then selected
  media.
- The default calendar cache covers the prior 12 months and next 18 months.
  Older ranges load on demand and may be retained within the cache budget.
- Unsynced captures, canonical text, keys, and conflict branches are never evicted
  automatically.
- Under low storage, evict only rebuildable indexes, old calendar cache outside
  the default window, downloaded media, and thumbnails—in that order. Show the
  impact before clearing a user-pinned offline item.
- “Available on this device” means the item can open without network. Remote-only
  media has a distinct state and size before download.
- A bootstrap can pause/cancel safely and resumes from an authenticated cursor.
  The user can use already committed content while lower-priority content arrives.

### Required mobile journeys

Flow specifications and low-fidelity prototypes must be reviewed before broad
screen implementation for:

1. install → local/direct pair or cloud sign-in → recovery setup;
2. initial bootstrap, progress, prioritization, pause/cancel, and low storage;
3. raw capture appearing immediately and later becoming a processed note;
4. offline edit → reconnect → accepted change or preserved conflict;
5. conflict review and resolution;
6. device revocation, reinstall, and re-enrollment;
7. media unavailable → download/stream → keep offline → evict; and
8. existing-library cloud opt-in, pause, disable, export, and purge.

## Current repository findings

### The native iPhone shell and first local slice exist

The repository now has an isolated iOS frontend, Tauri configuration, capability
manifest, Rust entry point, command registry, and SQLite store. A signed build has
been installed and launched on an iPhone 15 Pro. The native Notes prototype can:

- list and open local notes;
- create and edit a title and plain-text body;
- search local title/body text;
- tombstone a note; and
- preserve its file-backed WAL database across process restart.

The iOS command registry contains mobile health plus only the local Notes
commands. Desktop recorder, model, agent, provider, vault, reminder-worker,
sqlite-vec, and legacy phone-server dependencies are target-gated out of the iOS
build. The exact verified environment and commands are recorded in
[the iPhone feasibility preflight](IPHONE_FEASIBILITY_PREFLIGHT.md).

### The native slice is deliberately not syncable yet

The prototype `mobile_notes` table uses local integer IDs and has no library,
device, portable revision, accepted head, branch, outbox, inbox, cursor, conflict,
or media state. Its search is bounded substring matching rather than the planned
local FTS contract. It has no pairing, navigation shell, folders, Trash/restore
UI, bootstrap, background transport, or recovery.

M3 must migrate any retained prototype notes into UUID-backed portable local
records before pairing. Their integer IDs must never be mapped to Mac row IDs.
Until that migration, the iPhone and Mac databases are intentionally separate
sources of local data and the product must not imply that a note is synchronized.

### The dormant browser bridge is quarantined, not a foundation

The historical LAN/PWA path remains in source for a possible developer diagnostic
surface, but release profiles keep it off, application startup cannot activate
it, the frontend browser transport is removed, retained request bodies are
bounded, managed image reads are root-confined, its public manifest carries no
credential, and its command gate permits health only.

The old narrow-layout UI included Today, quick capture, Ask, and Settings and
sent actions directly to the Mac. It has no offline corpus or sync journal and is
not the native product contract. The new sync listener must be separate from
`phone.rs`, expose no arbitrary command dispatcher, and never revive URL/local
storage bearer credentials.

### Current schema cannot support independent writers

Meetings already have UUIDv7 public IDs. Most other canonical objects still use
local SQLite row IDs and lack the fields required for sync:

- stable public IDs;
- library and device identity;
- monotonic record revision;
- complete updated timestamps;
- source revision and content hash;
- tombstone state;
- per-device mutation IDs and counters;
- outbox/inbox state;
- sync checkpoints;
- conflict records; and
- media object identifiers.

Notes, entries, folders, filing rules, meeting segments, corrections, approved
entities, and other user-visible canonical records need a portable identity.

The database also stores image, audio, and video as absolute Mac paths. A path is
not a portable attachment reference and must never cross the sync boundary.

### Remaining native security and lifecycle probes

The physical build proves installation and local persistence, not the production
security boundary. M2 remains open for tested iOS Keychain/Secure Enclave use,
Data Protection across SQLite/WAL/FTS/media, locked-device behavior, App Group
share-inbox isolation, reinstall cleanup, backup exclusion, background encrypted
inbox behavior, VoiceOver/Dynamic Type, and suspend/resume/upgrade lifecycle.

If the legacy diagnostic listener is ever reintroduced, it separately requires
Host/Origin validation, rate limits, scoped revocable sessions, request timeouts,
and HTTP-level security tests. Tailscale membership or a private tunnel is not a
substitute for application authentication and authorization.

## Architecture alternatives

| Option | Offline while Mac sleeps | Native iOS abilities | Privacy and control | Long-term fit | Decision |
|---|---:|---:|---:|---:|---|
| Re-enable LAN PWA | No | Low | Weak in current form | Low | Reject as product target |
| Hardened PWA + cloud API | Partial; storage may be evicted | Medium-low | Can be good | Medium | Optional prototype/fallback |
| Tailscale remote access | No | Low | Strong private network | Low | Developer stopgap only |
| Native iOS + CloudKit | Yes | High | Good; app-layer E2EE still needed for a strict promise | Medium if Apple-only | Credible fallback |
| Native iOS + plaintext hosted database | Yes | High | Expands operator/breach access | Medium | Reject as default |
| Native iOS + custom E2EE relay | Yes | High | Strongest explicit boundary | High | **Recommended** |
| Sync the SQLite file through Drive/iCloud | Unsafe | N/A | Poor conflict behavior | None | Reject |

SQLite's WAL design expects all processes to be on one host. A live database file
must not be placed in iCloud Drive, Google Drive, Dropbox, or a network filesystem:
[SQLite WAL](https://www.sqlite.org/wal.html).

### Why not make CloudKit the primary architecture?

CloudKit is a credible private Apple-only adapter. CKSyncEngine provides local
state coordination, notifications, and sync machinery, but the app still owns
conflicts and engine-state persistence, and scheduling is not deterministic:
[CKSyncEngine](https://developer.apple.com/documentation/cloudkit/cksyncengine-4b4w9).

It is not the default because:

- it binds the product's identity and storage path to Apple/iCloud;
- private records consume the user's iCloud quota;
- future web, Android, agent, and hosted-service access becomes harder;
- a strict server-unreadable promise still benefits from application encryption;
- CloudKit schema deployment is additive-only;
- large media still requires explicit chunking and lifecycle policy; and
- the roadmap already identifies Noted Cloud as a managed continuity product.

CloudKit remains a fallback if the owner intentionally chooses “Apple-only,
iCloud-account-only, no independent managed sync product.”

### Why not make the PWA final?

A Home Screen web app is attractive because it requires no Apple membership or
review. Modern iOS supports installation, notifications, camera/microphone, a
service worker, and IndexedDB:

- [Add a website to the iPhone Home Screen](https://support.apple.com/guide/iphone/iphea86e5236/ios)
- [Web Push for iOS Home Screen apps](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/)

But browser storage is quota-bound and initially best-effort, and WebKit still
does not provide dependable Background Sync:

- [WebKit storage policy](https://webkit.org/blog/14403/updates-to-storage-policy/)
- [WebKit Background Sync issue](https://bugs.webkit.org/show_bug.cgi?id=201866)

It can be a visual-preview or emergency access client, but it should not be the
only durable copy of a personal context library.

## Target system

~~~mermaid
flowchart LR
    subgraph Phone["iPhone companion"]
        PUI["React mobile UI"]
        PDB["Local SQLite replica"]
        PO["Durable outbox / inbox"]
        PK["Keychain + device keys"]
        PM["Encrypted media cache"]
        PUI --> PDB
        PDB <--> PO
        PK --> PO
        PM <--> PDB
    end

    subgraph Relay["Noted encrypted relay"]
        AUTH["Account + device registry"]
        LOG["Opaque mutation log + snapshots"]
        BLOB["Encrypted object storage"]
        PUSH["APNs wake hints"]
    end

    subgraph Mac["Noted on Mac"]
        MUI["Desktop UI"]
        MDB["Canonical SQLite replica"]
        MO["Sync adapter"]
        AI["Local AI processing workers"]
        MM["Local source media"]
        MUI --> MDB
        MDB <--> MO
        MDB <--> AI
        AI <--> MM
    end

    PO <-->|"M4 direct authenticated sync"| MO
    PO <-.->|"M7 hosted encrypted sync"| LOG
    MO <-.->|"M7 hosted encrypted sync"| LOG
    PK <-.-> AUTH
    MO <-.-> AUTH
    PM <-.-> BLOB
    MM <-.-> BLOB
    PUSH -. "refresh hint" .-> PO
~~~

The solid path is built first: the paired Mac is the direct sequencing authority.
The dotted hosted paths arrive at M7 through the explicit authority-generation
cutover in Decision 007. The mutation envelope, encryption, cursors, retry rules,
and local storage do not change when the relay arrives.

### Responsibilities by layer

**Shared product/domain layer**

- record and mutation schemas;
- validation and schema-version negotiation;
- stable public IDs;
- revision and conflict rules;
- canonical serialization and hashes;
- capture lifecycle state;
- media manifests;
- sync engine and convergence tests; and
- platform-neutral repository interfaces.

**macOS layer**

- system and microphone meeting capture;
- Ollama/local/BYOK/hosted model access;
- OCR, transcription, extraction, summaries, embeddings, and semantic retrieval;
- Mac Keychain access;
- current Google Calendar connector until iOS direct OAuth ships;
- source-media retention and compression; and
- desktop-only settings and permissions.

**iOS layer**

- local SQLite replica and FTS;
- iOS Keychain and Secure Enclave integration;
- camera, photo picker, microphone, notifications, and share sheet;
- background URLSession uploads/downloads;
- BGAppRefreshTask wake hints;
- local-network permission and discovery during direct pairing;
- direct Google OAuth/API access in its later calendar phase; and
- mobile-specific capabilities and command allowlist.

**Encrypted relay**

- account authentication;
- device enrollment and revocation;
- idempotent encrypted mutation acceptance;
- monotonic per-library change sequence;
- bounded mutation retention and encrypted bootstrap snapshots;
- pre-signed encrypted blob transfer;
- APNs refresh hints;
- rate limits, abuse controls, metadata-only observability; and
- purge/account-closure workflow.

The relay must not contain inference code in the first mobile release.

## Portable record and migration foundation

### Record envelope

Accept ContextRecordV1 as the basis for sync, with these required properties:

- library_id;
- UUIDv7 record_id;
- kind and schema version;
- positive monotonic revision;
- RFC 3339 UTC created_at and updated_at;
- event time with IANA timezone where civil time matters;
- scope and sensitivity;
- source authority and provenance;
- canonical content;
- canonical serialization content hash; and
- active, trash, tombstone, and purge lifecycle timestamps.

SQLite integers remain internal foreign keys and never appear in network,
notification, deep-link, export, or media identities.

### Noted-owned canonical families

The migration inventory must classify every table before coding. Expected
Noted-owned syncable families include:

- categories/spaces and folders;
- notes, entries, filing state, and user edits;
- schedule/commitment records;
- raw captures and processing state;
- meetings and immutable transcript segments;
- user-authored meeting notes;
- speaker display assignments and explicit corrections;
- approved entity/person facts and merge redirects;
- calendar-display configuration and normalized event references;
- canonical user-authored summary overlays/replacements with lineage to the
  generated artifact they edit;
- nonsecret preferences; and
- media manifests and references.

### Externally authoritative mirrors

Registered Brain/Obsidian files remain authoritative outside Noted. Their mobile
representations are read-only mirrors with source path hidden behind a portable
source ID. An iPhone edit is either:

- a separate Noted-owned annotation; or
- an explicit write-back proposal that the owning Mac applies to the registered
  file after conflict and authority checks.

The phone never silently turns an imported mirror into a second editable source
of truth.

### Synchronized derived display artifacts

These may be synchronized for the phone experience but remain replaceable and
are never classified as canonical:

- immutable generated summary and recap artifacts;
- versioned evidence-backed entity mentions keyed by source record/revision and
  extractor version;
- deterministic trend aggregates; and
- thumbnails or other regenerable display projections.

Every derived artifact identifies its source revision, generation, and producer.
It may be discarded and rebuilt without changing canonical content.

### Local-only or rebuildable families

Do not treat these as canonical synchronized state:

- sqlite-vec tables;
- embeddings;
- FTS tables and transient normalized chunks;
- rank scores and caches;
- inferred graph edges;
- temporary audio used only during transcription;
- local absolute paths;
- model files and model/provider state;
- API keys, OAuth tokens, and refresh tokens;
- agent secrets and access grants;
- voice centroids and biometric speaker profiles by default; and
- machine-specific permissions, window state, and recorder settings.

FTS is rebuilt on each device from synced text. The Mac produces versioned entity
mentions. The phone derives co-mention relationships locally from those mentions,
matching the existing database architecture in which there is no canonical edge
table. If a source has not yet been processed on the Mac, Knowledge shows that
projection as pending/stale rather than inventing an edge. The Mac alone rebuilds
embeddings and other AI-derived projections.

### Aggregate and duplicate-representation boundaries

Before adding IDs, the mobile ADR must decide where one logical record begins and
ends:

- meetings.note_id is a searchable projection of a meeting, not a second
  canonical note;
- current schedule/tasks embedded inside entries.data_json are part of their
  owning entry until a separate task-record migration is explicitly approved;
- meeting summary overlays and meeting notes retain lineage to the meeting rather
  than becoming unlinked duplicates; and
- a projection may have an addressable derived ID, but never independently win a
  conflict against its canonical owner.

ContextRecordV1 is a portable contract, not automatically a new database table.
Existing domain tables remain authoritative unless a separate storage-cutover ADR
is accepted. Immutable change-log snapshots serialize those records for sync;
they do not become a competing canonical payload table.

### Migration mechanics

The first storage phase follows the existing operational contract:

1. Refuse or defer a full migration while a meeting recording is active.
2. Create a verified recovery point with SQLite Online Backup or VACUUM INTO;
   never copy a live WAL database file.
3. Converge legacy additive schema once.
4. Run quick_check, foreign_key_check, required-column checks, and a canonical
   row inventory.
5. Stamp schema_version, min_reader_version, min_writer_version, migration
   checksum, time, and product version. Per-kind writer capability prevents an
   older client from rewriting a record whose unknown fields it cannot preserve
   losslessly.
6. Expand, backfill, validate, cut over, and contract only in a later release.
7. Assign stable IDs deterministically once and prove reopen idempotence.
8. Inventory media and preserve references before replacing local paths.
9. Inventory rich-document JSON as well as table columns. Rewrite embedded src
   and localPath media references transactionally in meeting notes and
   entries.data_json.
10. Keep the future canonical content hash and device-sync timestamps distinct
    from notes.content_hash and notes.synced_at, which currently serve Brain
    echo-suppression/vault-sync semantics.
11. Reconcile transcript correction behavior before calling segments immutable.
    Preserve original ASR evidence plus versioned correction/speaker operations,
    or explicitly model mutable versioned segments with prior-revision retention.
12. Test sanitized daily-driver and every distributed schema variant.
13. Define rollback as restoring the matching pre-migration data and binary, not
    having an old binary guess at a newer schema.

### Proposed local sync tables

Exact names may change during the schema ADR, but the responsibilities must exist:

| Responsibility | Minimum data |
|---|---|
| libraries | library ID, authority generation/owner, purge generation, current key epoch, creation and enrollment state |
| devices | device ID, public keys, capabilities, enrollment and revocation state |
| record_snapshots | immutable serialization of authoritative domain rows for a record/version; not a second canonical table |
| change_log | local sequence, mutation/version ID, base revision/version ID, proposed revision, accepted receipt/commit sequence, device, transaction, time |
| sync_outbox | encrypted mutation, attempts, retry time, acknowledgement state |
| sync_inbox | bounded downloaded ciphertext, source sequence, integrity state, and locked/unapplied status |
| sync_cursors | peer/relay identity, downloaded sequence, applied sequence, checkpoint and snapshot generation |
| conflicts | record, accepted head, pending/rejected branches, preserved variants, resolution |
| media_objects | media ID, hashes, sizes, kind, codec, duration, lifecycle, availability |
| media_refs | owner record, media object, semantic role |

Every write that changes canonical state must write the record revision and
outbox mutation in the same SQLite transaction.

Each replica preserves Decision 004's single-writer rule. UI writes, meeting
recording, processing results, migration, and pulled sync mutations all serialize
through the app-owned writer. Bounded read-only connections may serve UI/search,
and recording/capture writes retain priority over bootstrap, backfill, and sync.

## Sync protocol

### Mutation envelope

Every client mutation contains:

~~~text
protocol_version
library_id
mutation_id
transaction_id
transaction_member_index
transaction_member_count
transaction_manifest_digest
transaction_commit_marker
device_id
device_transaction_counter
authority_generation
purge_generation
record_id
record_kind
base_head_revision
base_head_version_id
proposed_revision
version_id
key_epoch
ciphertext
ciphertext_hash
signature
~~~

mutation_id is globally unique and makes network retries idempotent.
version_id distinguishes two offline branches that both propose revision n+1.
transaction_id groups related changes such as a note, its entries, folder
placement, and media references. One signed transaction manifest commits the
device_transaction_counter, member count, ordered member digests, byte total,
and expiry. A missing/expired transaction is aborted without partial visibility.
Reuse of a mutation_id, transaction_id, or device counter with different signed
bytes is a hard security error.

device_transaction_counter is reserved durably in the same SQLite transaction
that creates the logical transaction and its outbox members. Uploads from one
device are serialized by this counter. Acceptance, conflict rejection, and other
terminal protocol outcomes consume it and return a replayable signed receipt;
retrying the identical transaction returns that receipt. The counter is never
reset under an existing device identity. Restoring or reinstalling creates a new
device identity unless a platform-backed anti-rollback counter can be proved.

The server sequence is not a content conflict policy. It orders accepted opaque
changes and provides a cursor.

### Accepted head and local branch state

Offline local work is durable but is not called an accepted shared revision until
the current sequencing authority accepts it.
ContextRecordV1 exposes the accepted revision; pending work is represented by its
base revision plus version_id/mutation state until accepted or merged.

For each record, a replica stores:

- accepted head: revision, version_id, content hash, authority generation, and
  acceptance checkpoint;
- local working state based on that head;
- pending branches awaiting acknowledgement;
- rejected/conflict branches that remain user-recoverable; and
- a resolved branch emitted against the latest accepted head.

Two devices may both propose revision n+1. The authority accepts at most one head
through compare-and-swap. A rejected device pulls that accepted head, retains its
own n+1 branch, performs the domain merge locally, and emits a newly encrypted
proposal for n+2. The rejected branch is never relabeled or discarded.

During direct sync, the paired Mac is the sequencing authority. Cloud cutover is
an explicit protocol transition: pause acceptance, publish and verify a signed
final Mac checkpoint, enroll the relay as the new authority for a higher sync
generation, and require every client to acknowledge or re-bootstrap. The
authority role never changes merely because an endpoint URL changes.

### Push

1. Authenticate the account and enrolled device.
2. Verify the device is active and its signature/counter is valid.
3. Deduplicate mutation_id only when its signed envelope digest matches exactly.
4. Assemble and verify the complete transaction manifest within count, byte, and
   expiry ceilings.
5. Compare base_head_revision, base_head_version_id, authority generation, and
   purge generation with the current accepted-head metadata.
6. Accept the complete transaction or reject it as a conflict.
7. Assign a monotonic library commit_seq/change_seq and signed acceptance
   receipt.
8. Store the current encrypted envelope and bounded change-log entry.
9. Return accepted revisions, receipts, and the high-water cursor.

Payload and size limits are enforced before buffering. The relay must not need to
decrypt the mutation to validate envelope shape, authorization, size, replay, or
revision preconditions.

### Pull and bootstrap

- Normal pull asks for changes after a durable downloaded cursor. It may place
  bounded authenticated ciphertext in the protected inbox while the device is
  locked without claiming that plaintext state is current.
- After unlock, validate the receipt chain, signatures, ciphertext hash, AEAD,
  schema, and canonical hash; apply domain changes and advance the separate
  applied cursor in the same SQLite transaction.
- A new or very stale device downloads a consistent encrypted snapshot plus its
  high-water cursor, then applies later changes.
- Old mutation logs may be compacted only after an independently restorable
  snapshot is verified.
- A mixed-version client that cannot read or losslessly write a required kind
  stops safely and requests an upgrade; it never discards unknown canonical
  fields.
- Invalid decrypted content is quarantined under strict parser, decompression,
  count, and byte budgets. It cannot permanently block the next cursor: clients
  retain the signed evidence, isolate the sending device, advance only through a
  protocol-defined quarantine receipt, and restore/reconcile from the last known
  good checkpoint.

### Correctness triggers

Sync runs:

- immediately after a local canonical write;
- on app launch;
- on foreground/resume;
- after connectivity changes;
- on pull-to-refresh;
- after a push notification hint;
- during system-granted background refresh; and
- after background media transfer completion.

Correctness cannot depend on any background trigger. Apple states that background
notification delivery is not guaranteed and may be throttled:
[Pushing background updates](https://developer.apple.com/documentation/usernotifications/pushing-background-updates-to-your-app).

## Conflict semantics

Blanket last-write-wins is not acceptable for a personal knowledge system.
Device wall clocks never decide the winner.

| Data shape / operation | Conflict behavior |
|---|---|
| New capture | Append-only UUID; mutation retry deduplicates |
| Transcript segment | Preserve original ASR evidence; correction/speaker operations create a versioned effective segment |
| Generated summary/recap | Immutable generated artifact plus separately revisioned canonical user overlay |
| Disjoint scalar fields | Merge automatically when base versions prove fields are disjoint |
| Same scalar field | Deterministic field operation plus visible audit; preserve overwritten value |
| Note body | Whole-document optimistic concurrency first; preserve conflict copy and offer three-way merge |
| Folder move/order | Domain move operation with deterministic server order and move history |
| Speaker rename/correction | Domain correction operation, never a raw row patch |
| Entity merge | Redirect/alias operation; never delete the losing public identity |
| Calendar event | Respect provider event ID and ETag; refetch and surface a provider conflict |
| Trash | Reversible lifecycle operation |
| Delete vs stale edit | Tombstone wins; the stale edit is preserved for explicit recovery, never resurrected silently |
| Restore | New higher revision that explicitly reactivates the record |
| Permanent purge | Disabled until the purge-generation protocol below is implemented |

A CRDT such as Automerge or Yjs is not required for v1. It becomes justified only
if measured usage shows simultaneous editing of the same note body. Revision-based
conflict copies are simpler, auditable, and sufficient for a personal two-device
product.

### Convergence test harness

Before a network service is written, create a deterministic simulator that:

- generates operations from multiple offline devices;
- reorders, duplicates, delays, and drops network messages;
- crashes between every durable step;
- advances through mixed schema versions;
- applies delete/edit/restore races;
- uploads media partially and resumes; and
- simulates omission, replay, split view, partial transactions, poison payloads,
  counter rollback, snapshot compaction, and key-epoch interruption; and
- asserts that all nonrevoked replicas converge without lost acknowledged edits.

Property tests should minimize failing operation histories and keep those cases as
regression fixtures.

## Encryption, pairing, and recovery

### Threat model

Protect against:

- an honest-but-curious or breached sync operator;
- stolen cloud database/object-store copies;
- network interception and replay;
- leaked account session tokens;
- a lost or stolen phone;
- a revoked device attempting future sync;
- a buggy or compromised enrolled device submitting validly signed poison data;
- malformed encrypted payloads;
- relay omission or rollback when a returning device retains a trusted checkpoint;
- accidental logs containing personal content; and
- deletion resurrecting from a long-offline device.

Application-layer E2EE does not protect content already unlocked on a compromised
device, screenshots, notifications the user chose to reveal, or copies exported
outside Noted. Product copy must state that boundary.

Signatures make historical envelopes authentic but cannot, by themselves, prove
freshness to a brand-new recovery-only device. Each client stores signed,
hash-chained state commitments and its highest trusted checkpoint; trusted-device
pairing transfers that checkpoint and clients detect rollback/fork relative to
what they already know. A recovery-code-only bootstrap cannot distinguish the
relay's latest authentic history from an older authentic snapshot without an
independent witness/transparency service. The first release states that limit
rather than claiming full protection from relay equivocation.

### Relay-visible metadata

“Opaque” refers to content, not all metadata. The relay necessarily sees account,
library and device identifiers; key epoch; protocol version; accepted revision
and change sequence; ciphertext sizes/chunk counts; timing, IP/access patterns;
record kind and object/record routing identifiers. If UUIDv7 is visible, it also
leaks approximate creation time. Participant names, MIME type, duration,
plaintext hashes, titles, text, and semantic media roles remain encrypted unless
a later ADR justifies exposing one. A privacy-hardening ADR may replace visible
record IDs/kinds with stable keyed routing tags, but v1 copy assumes the listed
metadata is visible.

### Local replica protection

E2EE does not protect an unlocked local database. The iOS feasibility ADR must
choose and test:

- iOS Data Protection classes for SQLite, WAL/SHM, FTS, thumbnails, and media;
- whether SQLCipher or equivalent database encryption is required in addition;
- Keychain accessibility and ThisDeviceOnly behavior for vault/device keys;
- app-group and Keychain access-group boundaries for the Share Extension;
- exclusion of plaintext library files from device/cloud backup, or app-layer
  encryption before inclusion;
- uninstall/reinstall cleanup and the rule that reinstall creates a new device
  identity; and
- the user's device-passcode assumption and optional Face ID unlock policy.

Recommended default for the M2 probe: keep vault/content keys in Keychain with
`WhenUnlockedThisDeviceOnly`; protect plaintext SQLite, WAL/SHM, FTS, thumbnails,
and decoded media with complete file protection; and permit only already-encrypted
transport envelopes/chunks plus separately scoped transport authentication under
an `AfterFirstUnlockThisDeviceOnly` boundary if locked background transfer proves
necessary. SQLCipher or equivalent remains a measured decision after physical
extraction, performance, migration, and backup tests; OS Data Protection is not
allowed to remain unspecified while that decision is open.

Foreground-readable canonical plaintext and background-transfer state are
separated. While locked, the system may upload/download already encrypted
transport envelopes and chunks. It does not unlock or apply the plaintext
library. Pulled ciphertext waits in a bounded protected inbox until the user
unlocks the app. Release tests cover background work while plaintext keys are
unavailable.

### Key hierarchy

- Generate one random 256-bit vault key per library.
- Each device creates a device agreement key and signing key. iOS private keys
  live in Keychain/Secure Enclave when the chosen algorithm and operation permit;
  macOS keys live in Keychain.
- The vault key wraps per-record and per-blob data-encryption keys.
- Records and blobs use authenticated encryption with unique nonces.
- Associated authenticated data includes library ID, record ID, kind,
  base/proposed revision, version ID, schema version, and key epoch.
- Mutations are signed and include a monotonically increasing device counter.
- Cloud account authentication grants access to ciphertext; it is not a
  decryption key.

Apple identifies Keychain as the appropriate store for small secrets and
cryptographic keys:
[Storing keys in Keychain](https://developer.apple.com/documentation/security/storing-keys-in-the-keychain).

Use a reviewed standard envelope construction such as HPKE for wrapping material:
[RFC 9180](https://www.rfc-editor.org/rfc/rfc9180.html).
HPKE is a primitive, not the pairing protocol.

Before any real-data pairing, accept a pairing-protocol ADR that fixes the
algorithm suite and Secure Enclave compatibility; canonical transcript; role,
library, environment and protocol-version binding; proof of possession; session
expiry/replay storage; short-authentication-string derivation; key confirmation;
signed enrollment receipt; device-key pinning; downgrade resistance; and failure
behavior. The cryptographic design receives external review before real-data
write dogfood.

### Pairing

1. The existing trusted device creates a single-use, short-lived pairing session.
2. The iPhone scans a QR code containing only session identity, expiry,
   rendezvous information, and an ephemeral public key.
3. Both devices authenticate the handshake and display the same short
   verification string.
4. The user confirms the match on both devices.
5. The existing device wraps the vault key to the new device public key.
6. In direct mode, the Mac-owned device registry records the keys, pinned channel
   identity, scopes, and signed enrollment receipt. In cloud mode, the account
   registry records the corresponding relay enrollment.
7. The iPhone performs an encrypted bootstrap and verifies a known library
   fingerprint before showing content.

The QR code never contains the vault key or a permanent bearer token. Direct
discovery is rendezvous only: the session uses pinned TLS/device identity or an
equivalent transcript-authenticated secure channel, key confirmation, strict
endpoint scope, and no credential in a URL. Migrating an existing direct device
to cloud is a new signed enrollment tied to the final direct-mode checkpoint; it
is not an automatic trust expansion.

### Recovery

True E2EE requires an explicit recovery choice:

- user-held recovery code that wraps the vault key; or
- separately consented escrow with clearly different privacy semantics.

Recommended default: generate a high-entropy checksummed recovery code, verify
that the user saved it, and allow a new code/key epoch to be created from any
surviving trusted device. The recovery ADR fixes entropy, human encoding,
checksum, wrapping algorithm, salts, versioned KDF parameters, attempt controls,
rotation, removal of obsolete wrappers, and treatment of old snapshots/backups.
Passphrase recovery, if added, uses a memory-hard KDF such as Argon2id with
versioned parameters; a normal account password is never silently substituted.

Without a trusted device or recovery secret, encrypted data is intentionally
unrecoverable. Account-password reset cannot decrypt it.

### Revocation and rotation

- Revocation prevents the device from downloading future envelopes or wrapped
  keys.
- Rotate the library key epoch after revocation.
- New records and blobs receive new-epoch data keys. The next write to an
  existing record/blob rotates its data key; merely rewrapping a data key already
  known to the revoked device does not restore confidentiality.
- Previously downloaded plaintext or keys cannot be remotely clawed back.
- A revoked device cannot decrypt content created or rekeyed after epoch cutover
  even if it later obtains that ciphertext. Historical access requires complete
  data-key/content rotation and still cannot erase past copies.

### Purge-generation protocol

Permanent purge does not ship with direct-only write dogfood. The managed relay
must first define:

- active, inactive, lost and revoked device acknowledgement rules;
- trash retention deadline and undo policy;
- a signed purge/snapshot generation barrier;
- the minimum accepted base generation after that barrier;
- forced wipe/re-bootstrap for a device returning from before the cutoff;
- rejection/quarantine of pre-purge edits so they cannot resurrect content;
- signed client-issued media retain/release operations and acknowledgement
  barriers;
- object-store, snapshot and backup deletion SLAs; and
- explicit disclosure that plaintext already downloaded elsewhere cannot be
  remotely erased.

## Media architecture

Replace path-shaped sync contracts with immutable media manifests. Split them
into encrypted semantic metadata and minimal relay routing metadata.

~~~text
encrypted:
media_id
owner_record_id
semantic_role
mime_type
codec
duration_ms
byte_count
keyed_plaintext_integrity_digest
source_retention_policy_snapshot
source_track_role
compression_and_verification_state
created_at
deleted_at

relay-visible:
opaque_object_id
ciphertext_byte_count
encrypted_chunk_count
key_epoch
lifecycle_state
~~~

Media is chunk-encrypted for resumable upload and range-oriented playback.
Uploads use short-lived pre-signed URLs. The client verifies chunk hashes,
complete length, and final manifest before marking remote storage verified.

Lifecycle:

~~~text
local_only
  → queued
  → uploading
  → verified_remote
  → available
  → deleting
  → deleted
~~~

Policy defaults:

- Photos: sync original plus encrypted thumbnail.
- Temporary transcript input audio: never sync.
- Retained meeting audio: opt-in; preserve separate mic/system roles and the
  immutable per-meeting retention snapshot; compress to the accepted AAC/M4A
  target and verify before upload; lazy download/stream.
- Video: separate opt-in and quota; never fully prefetch by default.
- Voiceprints: local-only unless a future biometric-specific design is approved.
- Local source deletion: offered only after verified remote availability and
  recovery policy checks.

The content-blind relay does not infer semantic reference counts. Authorized
clients emit signed retain/release operations for opaque object IDs, and the
relay deletes only after retention and generation barriers. A keyed per-library
digest remains encrypted; never expose a raw plaintext hash that enables content
guessing.

Native background URLSession is the correct large-transfer mechanism because it
can continue system-managed transfers while the app is suspended:
[Apple background downloads](https://developer.apple.com/documentation/foundation/downloading-files-in-the-background).

## Calendar architecture

Google Calendar is already cloud data, so copying the Mac's OAuth refresh token
to the phone is the wrong abstraction.

### Initial behavior

- Add persistent normalized calendar-cache tables because the current connector
  fetches ranges live and has no snapshot to replicate.
- Sync a normalized event snapshot needed by Today and Calendar, including
  provider event ID, calendar ID, ETag, recurrence identity/exception, timezone,
  attendees, conferencing, reminders, visibility and last provider update.
- Cache enough history/future range for offline use.
- Queue create/edit/delete operations when direct provider access is unavailable.
- Show **Pending calendar update** until Google acknowledges the mutation.
- The Mac connector applies queued writes initially using provider event IDs and
  ETags.

### End-state behavior

- Create a distinct iOS OAuth client for the same Google project and bundle ID.
- Use Authorization Code + PKCE through the system browser.
- Store the phone's refresh token only in iOS Keychain.
- Request the minimum Calendar scopes needed by shipped behavior.
- Call Google Calendar directly from the phone for current data and queued writes.
- Normalize provider IDs/ETags into the shared calendar model.
- Treat Google as authoritative and resolve 412/precondition failures by refetching
  and surfacing a conflict.
- Disconnect and revoke credentials per device; do not sync OAuth secrets.

Google's installed-app guidance says installed apps cannot keep a client secret,
supports iOS clients, and documents PKCE and per-platform credentials:
[Google OAuth for iOS and desktop apps](https://developers.google.com/identity/protocols/oauth2/native-app).

EventKit may later offer device-calendar integration, but it is a separate source.
It must not silently duplicate Google events already loaded through the Google
connector.

## Tauri and repository restructuring

The existing Rust target assumes macOS: local Ollama, macOS Keychain through the
security process, Core Audio, screen capture, desktop window chrome, model files,
and a LAN web server. A native iPhone app should not compile or expose those
modules.

### Target boundaries

Refactor toward:

- a platform-neutral domain/sync/storage core;
- a macOS application layer;
- an iOS application layer;
- explicitly platform-gated Cargo dependencies and modules; and
- separate Tauri capability manifests for desktop and mobile.

The current crate already produces staticlib/cdylib/rlib, which is directionally
compatible, but it is not evidence that the backend compiles for iOS. The mobile
feasibility phase must prove each dependency and replace or gate unsupported
ones.

The feasibility inventory explicitly covers current unconditional startup and
build behavior:

- global shortcut registration;
- provider and Google Keychain initialization;
- agent broker and Brain source startup;
- meeting and pending-capture workers;
- sqlite-vec registration in db::init;
- macOS diarizer helper build;
- desktop resource bundling;
- macOS-specific Tauri features/plugins; and
- Cargo dependencies that do not support iOS.

iOS receives a separate Tauri config, build script, entry/command registry,
capability manifest, and mobile schema that does not require vec0. Physical-device
tests must prove rusqlite/FTS, migrations, Tauri plugins, and every retained Rust
dependency compile and run on the selected minimum iOS version.

The selected minimum is iOS 17.0. This is encoded in the source Tauri iOS
configuration because the accepted pairing suite depends on CryptoKit HPKE;
generated Xcode project files are not the authority for this setting.

Share to Noted is a native Share Extension, not a browser shortcut. It requires
an App Group durable inbox, extension-safe and least-privilege Keychain access,
strict item/byte limits, handoff receipts, and crash-safe import by the main app.
The extension never opens the main SQLite writer directly.

### Frontend boundary

Replace “Tauri versus browser HTTP” as the primary data abstraction with typed
repositories:

~~~text
NotesRepository
MeetingsRepository
CalendarRepository
KnowledgeRepository
CaptureRepository
SyncStatusRepository
MediaRepository
~~~

Desktop implementations call the local macOS command layer. iOS implementations
call the local mobile command layer. Both screens read local state first; sync
runs below the UI. The web transport, if retained for diagnostics, is a third
adapter with an intentionally small command set.

Every API has:

- a versioned request/response type;
- an explicit capability declaration;
- pagination and byte/count ceilings;
- typed unavailable/offline/conflict errors; and
- deterministic fixtures for mobile screen development.

Do not expose the desktop's complete 155-command registry to an iPhone session.

## Private distribution strategy

Distribution and data sync are separate. The preferred release ladder is:

1. Xcode device build for the feasibility spike.
2. Internal TestFlight for private daily-driver development.
3. Optional Ad Hoc build if no App Store infrastructure is acceptable.
4. Unlisted App Store distribution for a durable finished native product.

| Route | Practical behavior | Use for Noted |
|---|---|---|
| Home Screen web app | No Apple membership/review; no signing expiry; foreground-oriented sync and quota-bound browser storage | Visual prototype or emergency client |
| Free Personal Team | Own-device development; provisioning expires after seven days | Compile/install spike only |
| Internal TestFlight | Paid Developer Program; up to 100 App Store Connect team members; each build lasts up to 90 days | **Default owner/team beta** |
| External TestFlight | Paid program; invite-only cohort up to Apple's limit; first group build receives Beta App Review | Broader private tester cohort |
| Ad Hoc | Paid program; registered UDIDs; up to 100 iPhones/year; manual install and expiring profiles | Strict off-store fallback |
| Unlisted App Store | Full review; hidden from search; anyone with link can download, so app auth still matters | Durable private-ish release |
| Custom App / Enterprise | Organization/MDM requirements | Not appropriate for a personal product |

Apple's current terms and limits:

- [Membership comparison and fees](https://developer.apple.com/support/compare-memberships/)
- [TestFlight overview](https://developer.apple.com/help/app-store-connect/test-a-beta-version/testflight-overview)
- [Registered device distribution](https://developer.apple.com/documentation/xcode/distributing-your-app-to-registered-devices)
- [Registered device limits](https://developer.apple.com/help/account/devices/devices-overview)
- [Unlisted app distribution](https://developer.apple.com/support/unlisted-app-distribution)

Unlisted is not a security boundary. The app still requires account/device
authorization. A future App Review build also needs a sanitized demo mode or
reachable review environment; a reviewer cannot be expected to connect to the
owner's private Mac.

Internal TestFlight still uses App Store infrastructure and is not a permanent
install: upload a fresh build at least every 90 days. Standard iPhones have no
ordinary permanent, maintenance-free, completely off-store native channel.
Ad Hoc avoids App Review but requires registered devices and periodic
re-provisioning. The Home Screen web app is the only no-signing/no-review route,
with the capability and durability trade-offs already documented.

## Mobile implementation milestones

Mobile milestones are acceptance gates, not promises based on elapsed time. They
use the prefix M so they cannot be confused with the authoritative product
roadmap, whose Phase 1 meeting-memory proof is still active and must not be
displaced.

Roadmap relationship:

- M0–M5 supply portability and multi-device prerequisites for Roadmap Phase 2.
- M4–M6 also prototype the daily mobile loop described by Roadmap Phase 3.
- M7 and M9 implement encrypted continuity from Roadmap Phase 5.
- None changes the proof gate or shipped-status language of those roadmap phases.

A later milestone may be spiked, but it does not ship around a failed earlier
gate. Physical-iPhone feasibility precedes broad schema conversion, and one Notes
vertical slice must work end to end before other record families migrate.

### M0 — Owner decisions and frozen contracts

**Purpose:** make product, privacy, and data decisions explicit before schema work.

Status: accepted as the implementation baseline in
[Decision 006](decisions/006-iphone-companion-direction.md). Detailed pairing,
storage, recovery, and purge protocol ADRs remain required at their named gates.

Deliverables:

- Accept or amend the relevant proposed record, lifecycle, time, scope,
  migration, and recovery contracts.
- Approve the capability matrix in this plan.
- Decide the first supported iOS range and device classes.
- Confirm Internal TestFlight as the default beta lane or select Ad Hoc.
- Confirm that AI stays Mac-only and that phone-derived artifacts may be displayed.
- Confirm that bounded user-triggered Mac jobs are allowed from mobile while
  conversational assistant surfaces remain prohibited.
- Confirm the media defaults: photos on, retained audio opt-in, video separate,
  voiceprints off.
- Confirm short voice notes—not full multi-hour meeting recording—for mobile v1.
- Confirm Notes owns Meetings/Needs filing/Trash and More owns the secondary
  knowledge/analytics destinations.
- Confirm all Noted text is local, the proposed calendar cache window, and the
  media eviction order.
- Confirm owner/team Internal TestFlight versus an External TestFlight cohort.
- Maintain [Decision 007](decisions/007-mobile-sync-sequencing-and-provider-gate.md),
  which records direct-first sync, the provider-neutral relay seam, the M4
  evaluation gate, M7 spending boundary, and why CloudKit remains a fallback.
- Revisit Decision 003's portable-canonical-layer gate before a second primary
  platform and explicitly retain per-device SQLite replicas or approve a change.
- Accept pairing, recovery, local-at-rest, metadata-leakage, purge-generation,
  direct-to-relay authority-cutover, and initial-cloud-consent ADR requirements.
- Produce a complete canonical/derived/secret/biometric/media data inventory.
- Define measurable budgets for startup, local query latency, initial bootstrap,
  sync freshness, mobile storage, and recorder write latency.

Gate:

- Owner accepts the contracts and threat boundaries.
- No unresolved data family is implicitly synchronized.
- Phase 0 of the existing context plan is either accepted or explicitly amended.

Rollback:

- Documentation only; no production data change.

### M1 — Quarantine the old bridge and establish safety rails

**Purpose:** make accidental exposure impossible while the new boundary is built.

Status: first quarantine checkpoint implemented. Application startup and the
frontend transport can no longer activate the browser bridge; retained requests
are bounded, the public manifest contains no credential, only health is allowed
through the diagnostic command gate, and managed image reads are root-confined.
Host/Origin validation, rate limiting, and scoped device sessions remain deferred
unless a diagnostic listener is intentionally reintroduced.

Deliverables:

- Keep phoneLan off in every release profile.
- Add a deterministic registry/dispatcher/capability contract test.
- Remove the token-bearing dynamic manifest or isolate the entire bridge behind a
  developer-only build.
- Constrain inbox/media reads to canonical managed roots.
- Add request limits, timeouts, Origin/Host validation, rate limiting, and scoped
  device sessions to any retained diagnostic endpoint.
- Remove secrets and full connection URLs from logs.
- Update README, PhonePanel, release docs, and comments to reflect actual status.
- Create security regression tests for path traversal, token leakage, body size,
  destructive commands, and revoked sessions.

Gate:

- No production profile starts a broad phone RPC server.
- A LAN client cannot obtain credentials from an unauthenticated static route.
- Arbitrary Mac files cannot be read through any phone-facing endpoint.

Rollback:

- Disable the developer bridge flag; desktop behavior is unchanged.

### M2 — Native iOS feasibility shell

**Purpose:** retire unknowns before building screens.

Status: the isolated shell is generated and verified in an iPhone simulator and
on an iPhone 15 Pro. Full Xcode/iPhone SDKs, iOS Rust targets, CocoaPods,
target-gated dependencies, the mobile-only frontend, the minimal command
registry, reproducible development-team configuration, and signed physical-device
installation are in place. A first local-only Notes slice now proves on-device
SQLite persistence and implements create, edit, delete, and lexical search on
the phone. It does not yet establish the portable M3 record schema or sync.
Keychain/Data Protection, lifecycle, accessibility, and capability-isolation
probes remain pending. See [the dated preflight](IPHONE_FEASIBILITY_PREFLIGHT.md).

Deliverables:

- Generate the Tauri iOS target in a dedicated implementation branch.
- Compile and install a signed shell on a physical iPhone.
- Identify every nonmobile Rust dependency and either gate, replace, or move it.
- Prove the separate iOS config/build/entry registry and target-gate all current
  unconditional macOS startup services and dependencies.
- Open a local rusqlite/FTS database without requiring sqlite-vec and run a
  migration/schema compatibility smoke test.
- Implement minimal Swift plugins for Keychain/Secure Enclave and background file
  transfer; stub notifications and local-network pairing.
- Prove OS Data Protection, locked-device encrypted inbox behavior, App Group
  share inbox, ThisDeviceOnly key behavior, reinstall cleanup, and backup
  exclusion on a physical device.
- Prove the React design system, routing, safe areas, dynamic type, dark/light
  appearance, keyboard, and VoiceOver work in the iOS webview.
- Establish separate iOS capabilities and permissions; no model/recorder/provider
  command is present.
- Upload an internal TestFlight build if the distribution choice permits.

Gate:

- Cold install, upgrade, launch, suspend/resume, and local persistence pass on
  physical hardware.
- A production iOS build contains no macOS recorder, model, broad LAN server, or
  desktop secret-management surface.

Rollback:

- The iOS target is isolated; macOS release output remains unchanged.

### M3 — Ordered migration framework and Notes portability

**Purpose:** prove safe migration and portable identity on one bounded record
family before converting the library horizontally.

Status: implementation checkpoint complete. Desktop and iPhone Notes now have
ordered schema history, UUID-backed portable state, accepted heads and local
branches, atomic shadow journals/outboxes, non-destructive lifecycle, rebuildable
FTS, path-free media manifests, and verified export/restore. Migration recovery,
WAL-active snapshots, authority enforcement, writer floors, failure rollback,
and sanitized fixture reopening are covered by deterministic tests. The release
gate remains held for the physical-device security/lifecycle probes from M2 and
an accepted recorder-load measurement; no transport or personal-data sync is
enabled by this checkpoint.

Deliverables:

- Ship safe pre-migration backup/verification first.
- Introduce schema versioning plus reader and writer compatibility floors.
- Backfill library ID and UUIDv7 public IDs for Notes and the category/folder
  records required to organize them.
- Migrate any retained `mobile_notes` prototype rows into new UUID-backed local
  Notes as new creates; preserve their text and timestamps, never interpret their
  integer IDs as Mac identities, and provide a verified export/reset fallback.
- Add accepted-head revision state, local branch IDs, timestamps, canonical
  hashes, lifecycle/tombstones, provenance, and device attribution.
- Define Brain/imported-note authority and make imported mirrors read-only or
  proposal-based on mobile.
- Replace Notes public media paths—including embedded rich-document JSON
  references—with media IDs/manifests while retaining local paths internally.
- Introduce the local change journal/outbox in shadow mode.
- Make every in-scope canonical Notes write emit a deterministic mutation in the
  same SQLite transaction through the single writer.
- Gate current permanent-delete paths for portable families behind the later
  purge-generation contract; direct sync may trash/restore and retain tombstones,
  but it may not promise permanent erasure.
- Prove Notes export, verified restore, trash, and rebuildable FTS.

Gate:

- Sanitized legacy/daily-driver fixtures migrate, reopen idempotently, and roll
  back through the documented recovery process.
- Notes public contracts return no SQLite row ID or absolute Mac path.
- An old client that cannot losslessly write the record kind is blocked safely.
- Active recording latency stays within its accepted budget.

Rollback:

- Feature flags stop the shadow outbox while preserving expanded schema/data.
- Incompatible downgrade restores the verified pre-migration dataset and binary.

### M4 — Notes vertical slice on a paired iPhone

**Purpose:** prove one complete path—migration, local replica, mobile UI, offline
edit, conflict, and direct sync—before expanding record families.

Status: encrypted sanitized-fixture Notes sync checkpoint complete; milestone
gate not passed. The branch now contains the portable local
Notes workspace and mobile UI,
including Inbox, Needs filing, spaces/folders, search, create/edit, filing/undo,
Trash/restore, read-only external-authority records, conflict preservation and
resolution choices. Stable `noted://library/<library-id>/notes/<record-id>` deep
links use public UUIDv7 identities. The local store applies validated logical
bootstrap and incremental transactions, retains accepted heads and phone working
branches, quarantines invalid inbox work, and is covered by deterministic
convergence tests for replay, sequence gaps, payload rebinding, interruption,
restart, fast-forward, conflicts, lifecycle changes, and authority/purge
generation mismatches.

The pairing state machine and narrow direct-sync router contract are also
implemented for generated or sanitized fixtures. They enforce the accepted
transcript, explicit Notes scopes, invitation and receipt replay rules, bounded
parsing, TLS 1.3/no-0-RTT evidence, SPKI pin binding, signed request/response
boundaries, checkpoint-bound paged bootstrap, byte- and member-bounded atomic
transactions, serialized revocation, and exactly the allowed versioned sync
operations. The phone outbox uses the same encrypted-byte ceiling, including
AEAD overhead, so locally accepted batches remain sendable. These are logical
cores plus a sanitized-fixture-only native network surface, not a personal-data
service. Fixture cryptography is deliberately unable to enroll a personal
library.

The production-facing crypto boundary is now implemented behind an iOS-only
native plugin. Secure Enclave P-256 signing, ThisDeviceOnly Keychain X25519 and
bootstrap storage, and CryptoKit authenticated HPKE expose only public keys and
opaque handles to Rust. The HPKE sender operation is atomic: one sender context
produces its encapsulated key, ciphertext, and exporter; signatures and digests
bind the complete envelope. The authenticated fixed-size bootstrap package now
delivers the library key directly into native-only custody and binds the exact
pairing/sync protocol versions, receipt, library, device, default unknown scope,
Notes capabilities, authority/purge/key generations, record cipher suite,
durable-sync SPKI pin, and transcript digest. Both Swift and Rust reject
wrong-device or byte-different recovery metadata before mutation. The SAS uses
the frozen RFC 5869 HKDF-SHA256 construction, and Rust/Swift golden vectors cover
canonical transcripts, P1363 signatures, RFC 9180 authenticated HPKE, exporter
bytes, envelope digests, bootstrap metadata, and SAS output. The plugin has no
JavaScript crypto commands and its fixture mode requires the exact
debug/simulator/runtime gate.

The local replica also has a crash-safe protected-data and pairing boundary. It
closes SQLite behind the same operation mutex, returns a typed locked error to
every command, reopens through the full recovery/migration/verifier path, and
uses in-memory SQLite temporary storage. The ordered v5 migration durably stores
only exact protocol bytes, authenticated public metadata, public identity
bindings, and opaque native handles. Pairing activation is one immediate SQLite
transaction that adopts local staging records without changing their public
IDs, enrolls the replica, mirrors the authenticated activation, and advances the
exact active checkpoint. Exact replay is idempotent; byte-different replay,
partial activation, legacy active-v4 state, and Keychain/SQLite divergence fail
closed. A rejection is durably marked cancellation-pending before native cleanup,
so restart can only finish the discard and cannot resurrect confirmation.
Native callbacks enforce the lifecycle epoch, and existing plus newly created
SQLite, WAL, SHM, and recovery files are hardened to complete protection and
excluded from backup before the store is published ready. Source-owned iOS
configuration pins iOS 17 and declares only the local-network/Bonjour purpose
required by direct sync.

On the Mac side, an isolated fixture authority now provisions a create-new,
symlink-safe, mode-0600 database containing generated sanitized Note, Category,
and Folder data only. Its immutable marker and digests fail closed on tampering;
reopen is read-only and restart-stable. One serialized runtime owns pairing,
sync, and revocation and persists invitations, receipts, device enrollment,
replay evidence, counters, authority generation, and checkpoints. The six-route
exact-wire coordinator atomically prepares, signs, serializes, and finalizes
responses, including byte-identical replay after restart. A strict pinned-TLS
1.3 client/server adapter enforces the exact P-256 SPKI pin, disables 0-RTT and
resumption, and bounds hosts, origins, bodies, headers, time, and concurrency.
Sanitized-fixture builds now also expose a separate private-IPv4-only listener
type with an address-bound ephemeral certificate and matching Bonjour
advertiser. Its TXT contract contains only protocol, numeric address, and port;
it cannot carry a pin, library/device/receipt identifier, token, or credential.
The private listener can now bind the same serialized fixture authority to
exactly three pairing routes and six direct-sync routes. Route-specific request
and response ceilings are enforced before dispatch, TLS facts are constructed
inside the native listener, sync-only listeners reject pairing paths, and no
route reaches Tauri or JavaScript. The ordinary loopback harness cannot widen
its bind scope, and neither listener mode has a personal-data constructor or
desktop lifecycle/Tauri surface. Immutable
replay evidence is retained for audit; only the trusted
five-minute window counts toward replay admission, so the protocol is not
permanently locked after the cap, but on-disk evidence is not yet compacted or
size-bounded.

The sanitized fixture path now also proves the complete encrypted Notes data
cycle. Canonical NRC1 Note, Category, and Folder records are encrypted with fresh
AES-256-GCM nonces, signed by their exact writer, and verified against the active
pairing profile before local application. The phone durably journals each exact
signed request before its first socket write, stages bounded ciphertext bootstrap
pages, resumes them by authenticated checkpoint, and performs incremental
push/pull through all six pinned-TLS routes. One cross-layer test exercises real
pairing, encrypted bootstrap, an offline edit, convergence, crash/restart exact
request recovery, tamper rejection, and authority revocation without exposing a
library key to Rust or JavaScript in the production iPhone path.

The paired iPhone now has a product-facing direct-sync driver. A native
`NWBrowser` one-shot discovery adapter reduces Bonjour TXT metadata to at most
16 versioned private numeric IPv4 address hints; Rust validates them again and
constructs pinned TLS exclusively from the durable authenticated activation.
The manual fallback accepts only a numeric private socket address and uses the
same activation pin. The mobile UI can start either path, but JavaScript never
receives or supplies a certificate pin, credential, protocol message, route, or
response body. The driver runs the bounded six-route Notes orchestrator and
refreshes the protected local workspace after success. The generated app plist
now carries the same local-network purpose and `_noted-sync._tcp` declaration as
the source-owned iOS configuration.

Authenticated revocation is now a single durable phone-store transition. It
records immutable evidence, marks the enrollment and sync profile revoked,
quarantines unfinished inbound work, rejects open push bindings, and retires
unsafe outbox entries as conflicts while preserving working branches for export.
The revoked state remains terminal after restart. The product runtime now
immediately converts the exact active Keychain identity to a secret-free
tombstone after that durable transition, retries the retirement on resume, and
allows only a same-library invitation with a higher authority generation to
start replacement pairing. Finalizing that replacement now atomically swaps the
revoked activation, enrollment, sync profile, and active checkpoint only when
the new receipt, identity, public keys, and authority generation satisfy the
authenticated re-enrollment invariants. Existing Note identities survive the
swap, exact replay remains idempotent, rollback is rejected, and restart tests
verify the new activation remains authoritative.

The following work still blocks the M4 gate and any personal-data sync:

- the authenticated pairing half of the native iPhone network driver and
  desktop lifecycle wiring that owns the shared sanitized
  listener/advertiser/runtime as one unit. The private-LAN listener now routes
  pairing and sync through one fixture authority instance, and the minimal
  Bonjour advertiser exists as a fixture-only type, while
  post-activation Bonjour discovery, strict manual connect, and six-route Notes
  sync are implemented without trusting discovery metadata or passing protocol
  messages through JavaScript. The existing fixture pairing command that
  accepts claimed SPKI evidence from its caller remains a harness only and
  cannot satisfy this gate;
- production Mac key custody/signing and structural enforcement that pairing,
  revocation, and sync share one authority instance;
- external review of the implemented pairing, cryptography, transport, and
  convergence boundaries;
- end-to-end pairing, encrypted bootstrap, push/pull, conflict, restart, and
  airplane-mode validation between the Mac and a physical iPhone; and
- the outstanding physical-device Secure Enclave, Data Protection, Keychain,
  backup, reinstall, locked-device, lifecycle, Bonjour/manual-connect,
  accessibility, and recorder-load gates.

No hosted provider has been selected or provisioned, and no hosted-sync spend is
authorized by this checkpoint. Provider evaluation begins only after the full
M4 gate below passes.

Checkpoint verification includes the complete Rust library suite, focused
migration/pairing/direct-sync/fixture-authority suites, the encrypted cross-layer
Notes test, 32 Swift native-security tests, 14 Rust Apple-security tests, shared
Rust/Swift golden vectors, frontend/iOS contract checks, and Rust library
compilation for both the iPhone device and simulator targets. These prove the
bounded fixture contracts; they do not substitute for the product-network,
physical-device, personal-data, and external-review gates above.

Deliverables:

- Implement Notes Inbox, Needs filing, folders/spaces, search, create/edit,
  filing/undo, trash/restore, and conflict-copy UI against the local iOS database.
- Implement mutation envelopes, accepted-head/local-branch state, signatures,
  outbox/inbox, cursors, schema negotiation, and the deterministic convergence
  simulator for Notes.
- Add a narrow direct /sync/v1 Mac adapter that implements the same authority
  contract planned for the relay and cannot invoke arbitrary Tauri commands.
- Pair through the accepted protocol ADR using pinned endpoint identity,
  verification string, device keys, explicit scopes, Mac-owned registry, and
  revocation.
- Support encrypted Notes bootstrap and incremental pull/push over LAN.
- Complete the external cryptographic design review before using real personal
  data; prior tests use generated or sanitized fixtures.
- Add public-ID deep links, accessibility identifiers, and visual baselines.

Gate:

- Two Notes replicas converge under reordering, duplication, conflict,
  interruption, crash, and restart.
- An offline edit remains durable, and a rejected branch is preserved rather
  than misrepresented as the accepted head.
- Direct discovery spoofing, credential-in-URL, arbitrary command/path access,
  and unbounded bodies are impossible.
- The Notes experience remains usable in airplane mode.
- No hosted sync provider is selected, provisioned, or paid for before this gate.
  Passing M4 opens provider evaluation; it does not itself authorize M7 spend.

Rollback:

- Revoke the paired device and disable Notes sync; the Mac's canonical Notes
  remain intact and the phone can export its pending branches.

### M5 — Expand identity and the offline read-only mobile product

**Purpose:** expand only after the Notes slice proves the runtime and protocol.

Deliverables:

- Backfill portable identity/revision/lifecycle for meetings, transcript
  corrections, entries/schedule aggregates, captures, approved people/entities,
  derived display artifacts, settings, and media references one family at a time.
- Resolve meeting-note and schedule aggregate boundaries before each family syncs.
- Add persistent normalized Google Calendar cache tables and sync the read-only
  event snapshot needed by Today/Calendar.
- Implement Today, Calendar, Meetings, Knowledge/People, global lexical search,
  More, and mobile Settings against real local repositories.
- Treat Recaps/Trends as explicitly revived/new standard mobile scope until the
  desktop navigation decision is made; do not call their current components
  desktop parity.
- Apply the capability/AI-action gates inside reused MeetingPage and EntityPage,
  not only at top-level navigation.
- Implement versioned entity mentions and local co-mention graph derivation.
- Complete every read/action row in the parity ledger before enabling its family.
- Implement bootstrap priority, low-storage eviction, content-availability,
  stale-data, last-synced, local-only, and remote-media states.
- Use local discovery on trusted LAN; optionally use Tailscale Serve for private
  off-LAN dogfood after the endpoint passes its threat review:
  [Tailscale Serve](https://tailscale.com/docs/reference/tailscale-cli/serve).
- Add device list, last seen, revoke, and full re-bootstrap controls.

Gate:

- Every in-scope standard-profile read action in the parity ledger passes against
  an offline local replica.
- Mac sleep leaves the phone's prior data usable.
- No canonical family is duplicated through a projection.
- Rebuilding FTS, entity edges, and other derived state changes no canonical row.
- Accessibility, dynamic type, and the required mobile journeys pass review.

Rollback:

- Disable each family independently; Notes remains the known-good vertical slice.

### M6 — Remaining offline writes, capture, and Mac processing handshake

**Purpose:** make the direct-paired companion useful while keeping real-data risk
bounded until relay recovery and purge barriers exist.

Deliverables:

- Expand create/edit/file/trash/restore from Notes to the remaining approved
  canonical families. Permanent purge remains disabled.
- Enable meeting user notes, title edits, speaker corrections, and organizational
  actions. Participant editing is new scope unless the capability ledger proves a
  current canonical desktop operation and contract.
- Enable Today/commitment changes supported by the desktop product.
- Implement durable text/photo/short-voice-note capture and the native Share
  Extension.
- Ship the minimum capture-blob pipeline now: media ID, encrypted chunking,
  bounded/resumable direct-Mac transfer, integrity verification, retry state and
  idempotent ownership. M9 adds retained meeting media, streaming and quotas.
- Define processing jobs as records, not transient UI state.
- Mac receives jobs at least once, claims them idempotently, and publishes
  exactly one visible committed result per job ID, source revision, and processor
  version.
- Show Saved locally, Synced, Waiting for Mac, Processing, Ready, and Failed with
  retry states.
- Implement domain conflict UI and preserve both note-body variants.
- Show readiness after foreground/local sync. True remote wake/readiness
  notifications arrive with APNs in M7/M10.

Gate:

- A phone capture survives app termination, device restart, and seven days offline.
- Retried jobs never create duplicate notes or summaries.
- Concurrent offline edits lose no acknowledged user content.
- A delete cannot silently resurrect from a stale device.
- The phone performs no inference and holds no model/provider secret.
- Direct-only dogfood uses a verified Mac recovery backup. Daily-driver remote
  writes do not graduate beyond a tightly controlled owner fixture until M7
  recovery and security gates pass.

Rollback:

- Disable mobile mutation upload while retaining the local outbox for export or a
  later retry.

### M7 — Text-first managed E2EE relay and recovery

**Purpose:** make the phone independent of Mac reachability and provide encrypted
continuity before advanced calendar/media work.

Deliverables:

- Run the provider gate in Decision 007 against measured direct-sync behavior;
  record vendor, region, privacy terms, cost model, restore proof, and exit plan
  in a selection ADR before provisioning durable infrastructure.
- Account and device registry.
- Opaque Postgres-compatible mutation log/current-state index.
- Minimal S3-compatible encrypted object storage for capture blobs.
- Snapshot/bootstrap compaction and restoration.
- APNs refresh hints.
- Relay pairing/enrollment, direct-to-relay authority cutover, recovery code,
  revocation, data-key rotation, and key epochs.
- Signed/hash-chained checkpoints, retained-client rollback detection, conflict
  branches, poison-mutation quarantine, and purge-generation barriers.
- Metadata-only rate limits, metrics, audit events, backups, and disaster recovery.
- Account export, purge, and closure.
- A provider adapter so infrastructure choice does not leak into record contracts.
- Separate feature and consent boundary for any future cloud-readable/hosted-AI
  mode.
- External review of the implemented pairing, crypto, revision, recovery and
  deletion protocol before real daily-driver write rollout.

Existing-library enablement is an explicit consent flow before the first upload.
It shows record families, estimated text/media size, media defaults, relay-visible
metadata, Wi-Fi/cellular policy, recovery consequences, what pause/disable does,
and export/purge/account-close behavior. Decision 002's no-retroactive-upload rule
is enforced in data state, not only copy. Recovery-code verification must finish
before the first existing-library envelope leaves the Mac.

Gate:

- Server-side tests cannot decrypt E2EE fixtures.
- A clean phone and clean Mac can restore independently using the recovery path.
- A revoked device cannot decrypt content created or rekeyed after epoch cutover,
  even if it later obtains the ciphertext; historical limitations are disclosed.
- Mixed app versions either converge or stop safely with a required-upgrade state.
- Relay loss can be recovered from tested encrypted backups without record or
  blob mismatch.
- Mac-off phone capture reaches durable remote storage, is delivered at least
  once, and yields exactly one visible committed processing result when the Mac
  returns.
- An offline device older than the purge generation is forced to wipe/re-bootstrap
  and cannot resurrect deleted content.

Rollback:

- Clients fall back to local operation and keep durable outboxes.
- The direct paired-Mac adapter remains available for recovery/dogfood.

### M8 — Calendar independence

**Purpose:** make Calendar and Today current while the Mac is asleep.

Deliverables:

- Use the normalized calendar snapshots and offline ranges introduced in M5.
- Implement queued Mac-applied writes first.
- Add the iOS Google OAuth client, callback URL scheme, PKCE, product-owned Google
  project/test-user or verification plan, and per-device Keychain token.
- Perform direct phone reads/writes using provider IDs, recurrence IDs and ETags.
- Handle revoked scope, partial scope grants, rate limits, stale ETags, and account
  disconnect.
- Prevent duplicate events when Google and a future EventKit source overlap.
- Update Today immediately from optimistic local event state, then reconcile.

Gate:

- Calendar remains readable offline and becomes current without waking the Mac.
- Provider conflicts preserve the user's attempted edit.
- OAuth credentials never appear in sync records, logs, backups, or exports.

Rollback:

- Disable direct iOS provider access and fall back to cached events plus the
  visible Mac write queue.

### M9 — Retained media sync and playback

**Purpose:** extend the proven capture-blob path to retained meeting evidence
without turning Noted into an automatic video archive.

Deliverables:

- Encrypted photo originals and thumbnails on the relay.
- Resumable, chunked retained-audio upload with separate mic/system track roles,
  accepted retention snapshot, AAC/M4A compression and verification.
- Lazy phone streaming/download and explicit keep-offline control.
- Signed retain/release lifecycle, purge barriers and remote verification before
  local cleanup.
- Storage/quota UI split by photos, audio, and video.
- Video upload/playback behind a separate opt-in flag and quota.
- Background URLSession integration and foreground fallback.

Gate:

- Interrupted transfers resume without corrupting or duplicating a blob.
- Keyed plaintext integrity checks match after decrypt/download.
- Range playback works without fetching complete long recordings.
- Temporary transcript audio and voiceprints never enter the relay.
- Revocation invalidates outstanding transfer authorization.
- Remote delete, account close, backup deletion and retention-policy tests pass.

Rollback:

- Stop new media uploads; synced text remains functional and verified remote blobs
  remain readable until an explicit cleanup.

### M10 — Background behavior, final security review, and private beta

**Purpose:** make the system trustworthy under real iOS and network behavior.

Deliverables:

- Sync on launch/resume/connectivity/write plus best-effort background refresh.
- Silent-push refresh hints with no correctness dependency.
- Background media transfers and user-visible retry controls.
- Face ID policy and local-library unlock behavior selected in M0/M2.
- Final application penetration test and remediation; cryptographic design and
  implementation reviews have already gated M4 and M7.
- Privacy-safe crash reporting and support diagnostics.
- Battery, cellular-data, storage, bootstrap, and large-library profiling.
- TestFlight release automation, encryption/export-compliance review, and
  sanitized demo mode.
- Privacy policy, App Privacy disclosures, privacy manifest and required-reason
  API review.
- In-app account deletion when accounts ship; permission-purpose copy; support
  and contact metadata.
- Sign in with Apple policy review if any third-party social account login is
  offered for the Noted account.
- In-app device/recovery education and loss simulation.
- Staged rollout: developer fixture, personal dogfood, opt-in preview, release
  candidate, then durable distribution decision.

Gate:

- All release scenarios below pass.
- No high-severity security issue remains open.
- Battery and data use meet the accepted budgets.
- Recovery has been performed, not merely documented.
- At least four weeks of daily-driver use show no lost acknowledged capture/edit.
- Every parity-ledger action is implemented, owner-excluded, or blocked by a
  documented iOS limitation with accurate product copy.

Rollback:

- Remote feature flags can stop pairing, writes, media, or background activity
  independently without making local content unavailable.

## Release verification matrix

### Install, migration, and restore

- Fresh Mac + fresh phone.
- Existing daily-driver Mac + fresh phone.
- Fresh Mac restored from relay + existing phone.
- Fresh phone restored with recovery code while Mac is absent.
- Upgrade from every distributed desktop schema.
- Mobile upgrade with an outbox containing old-protocol mutations.
- App downgrade attempt against a newer schema.
- Interrupted migration at every durable stage.
- Reinstall with surviving Keychain items, and device-data restore with a rolled
  back counter; both must create or safely reconcile a new device identity.
- Locked-device background download followed by foreground unlock/apply.

### Offline and network

- Mac awake/asleep/off/switched networks.
- Phone offline for minutes, days, and beyond log retention.
- Captive portal, flapping Wi-Fi, cellular-only, low-data mode.
- Duplicate, delayed, reordered, and partially acknowledged requests.
- Push notification never delivered.
- Background task never scheduled.
- TLS/session expiry during a large upload.

### Conflict and lifecycle

- Both devices edit the same note body.
- Both edit disjoint metadata.
- One trashes while the other edits.
- Restore after delete.
- Permanent purge while a device is long offline.
- Offline device return from before the purge-generation cutoff; force
  re-bootstrap and quarantine its stale edit.
- Folder move races.
- Speaker correction and entity merge races.
- Duplicate capture submission and duplicate processing claim.

### Calendar and time

- Google token revoked or scope partially granted.
- Provider 401, 403, 409/412, 429, and transient 5xx.
- Event edited simultaneously in Google, Mac, and phone.
- Recurring event exception.
- All-day event and floating local time.
- Travel across timezones.
- Ambiguous/nonexistent daylight-saving time.
- Calendar removed or renamed.

### Media

- Photo upload interrupted at each chunk.
- Audio playback while remaining chunks are absent.
- Corrupt chunk/hash mismatch.
- Remote verified, then local source cleanup.
- Shared blob referenced by multiple records.
- Record deletion while upload is running.
- Quota exhaustion.
- Video disabled after a partial upload.
- Presigned upload/download authorization retained across device revocation.
- Crash during key-epoch rekey of an existing retained-media object.

### Security and privacy

- Pairing QR replay and expiry.
- Two simultaneous pairing sessions and enrollment-receipt replay.
- Mismatched verification string.
- Revoked-device push/pull.
- Device-counter rollback.
- Concurrent out-of-order counters and mutation-ID reuse with different bytes.
- Tampered ciphertext/signature/associated data.
- Malformed oversized envelope.
- Decrypt-valid but semantically invalid/zip-bomb mutation at the next cursor.
- Partial multi-record transaction, wrong aggregate digest, and expired commit.
- Snapshot compaction concurrent with accepted writes.
- Relay omission, authentic old-snapshot replay, and split-view/equivocation
  relative to retained client checkpoints.
- Recovery with an obsolete rotated code.
- Crash during partial library-key/data-key epoch rotation.
- Cross-library record and blob request.
- Logged errors contain no titles, transcript, note text, token, path, or key.
- Account password reset cannot bypass E2EE.
- Notification previews respect the user's privacy setting.

### Product and accessibility

- Empty, single-item, ordinary, and very large libraries.
- Large text and VoiceOver.
- Reduce Motion, dark/light appearance, landscape, keyboard, and safe areas.
- One-handed capture.
- Clear status for local-only, pending, waiting for Mac, conflict, and unavailable
  media.
- No dead Ask/provider/model entry point in the mobile bundle.

## Observability and operations

Operational metrics may include:

- device/app/protocol version;
- change cursor lag and count;
- encrypted envelope/blob sizes;
- sync attempt outcome and duration;
- conflict type count;
- bootstrap duration;
- background-task and push delivery diagnostics;
- job state transitions;
- storage/quota totals; and
- crash/energy/network performance.

Never log:

- note or transcript content;
- titles or participant names;
- calendar event content;
- search queries;
- OAuth/session tokens;
- device/vault/data-encryption keys;
- plaintext content hashes that enable dictionary attacks on small values;
- local filesystem paths; or
- decrypted media metadata beyond explicitly approved aggregate telemetry.

Required runbooks:

- account/session compromise;
- lost device and key rotation;
- lost recovery secret;
- stuck outbox and cursor repair;
- relay outage and client backoff;
- corrupt snapshot;
- provider token revocation;
- object-store partial failure;
- mistaken delete/purge request;
- app-version protocol cutoff; and
- full service shutdown with user export.

## Suggested delivery bands

These are provisional engineering-effort lower-bound ranges, not calendar
commitments:

| Outcome | Included milestones | Solo engineering range |
|---|---|---:|
| Architectural proof plus Notes migration foundation | M0–M3 | 5–10 person-weeks |
| Notes vertical slice and broad offline read-only companion | M0–M5 | 16–28 person-weeks |
| Direct-paired writable owner preview | M0–M6 | 22–36 person-weeks |
| Recoverable text-first encrypted daily driver | M0–M7 | 30–48 person-weeks |
| Calendar, retained media, and hardened private beta | M0–M10 | 45–70 person-weeks |

Parallel mobile, data/sync, and service engineers can shorten calendar time, but
the migration, protocol, and recovery gates remain sequential. Treat these as
lower bounds for a custom E2EE protocol and broad schema conversion. Formal
re-estimation gates follow M2's physical-device spike and M4's first complete
sync slice.

## Accepted decisions and deferred gates

Decision 006 accepts the product defaults below. Decision 007 accepts the
direct-first sync sequence and hosted-provider spending gate. Protocol details
that have their own named ADR gates remain deferred rather than silently
accepted.

1. **Client:** native Tauri iOS, not a permanent PWA.
2. **Beta delivery:** Internal TestFlight, not a public listing.
3. **Strict off-store fallback:** paid Ad Hoc, accepting manual installs and
   expiring provisioning.
4. **Sync:** direct paired Mac first, then custom application-layer E2EE relay;
   no provider selection or spend before M4 passes, and hosted provisioning
   belongs to M7 when off-LAN continuity, recovery, or testers justify it.
5. **Cloud privacy:** content is server-unreadable while documented routing,
   revision, size, timing and access metadata remain visible; hosted intelligence
   is a separate future opt-in mode.
6. **Calendar:** cached/queued first; direct per-device Google OAuth on iPhone next.
7. **Media:** photos sync; retained audio opt-in; video separate and lazy;
   temporary audio and voiceprints do not sync.
8. **Conflict policy:** preserve user work; no blanket last-write-wins.
9. **Assistant:** no mobile Ask/entity chat/meeting copilot/Live Assist or local
   inference; display artifacts and allow bounded queued Mac jobs.
10. **Recording:** short voice notes in v1, not multi-hour meeting sessions.
11. **Navigation:** Notes owns Meetings/Needs filing/Trash; More owns
    People/Knowledge/Recaps/Trends.
12. **Replication:** all Noted text local; bounded calendar window; media on
    demand unless pinned.
13. **Cloud enrollment:** no existing record uploads before the explicit
    inventory/size/metadata/recovery consent flow.
14. **Tester audience:** owner/team Internal TestFlight first; External
    TestFlight only when a broader cohort is intended.

Blocking points are milestone-specific:

- M1 bridge quarantine needs approval of this safety scope, not a distribution or
  recovery choice.
- M2 device feasibility can use a local Xcode build; TestFlight/Ad Hoc blocks only
  the beta lane.
- M3 foundational schema work requires acceptance or amendment of the portable
  record, authority, migration, aggregate and local-at-rest contracts.
- M4 real-data pairing requires the reviewed pairing/revision protocol.
- M7 cloud enrollment requires the E2EE recovery, metadata and upload-consent
  decisions.

Infrastructure vendor, object-store provider, exact pricing, video quota, hosted
AI, Android, and public/unlisted release do not block foundational
implementation. Paying for managed infrastructure later is acceptable, but the
provider must pass Decision 007's evidence gate and must not redefine the sync
or encryption contract.

## Definition of done

The mobile companion is complete when:

- every in-scope standard-profile capability ledger row is implemented or has an
  explicit owner-approved/platform-limited exclusion;
- the phone opens and searches its library without the Mac or network;
- phone captures and edits are durable before any network or AI work;
- Mac and phone converge after long offline periods without lost acknowledged
  changes;
- duplicate delivery and processing are harmless;
- deletes do not resurrect;
- calendar can become current without the Mac;
- media never depends on a Mac path and follows explicit retention policy;
- the relay cannot decrypt E2EE content;
- imported Brain/Obsidian mirrors retain their external authority;
- local database/media protection and locked-device behavior pass extraction and
  background tests;
- pairing, recovery, revocation, and clean-device restore have been exercised;
- private installation and updates are sustainable;
- the assistant and Mac-only controls are absent on iPhone; and
- local-only desktop use remains complete and unaffected.

## Research references

Primary sources consulted for this plan:

- [Apple Developer Program membership comparison](https://developer.apple.com/support/compare-memberships/)
- [Apple TestFlight overview](https://developer.apple.com/help/app-store-connect/test-a-beta-version/testflight-overview)
- [Apple registered-device distribution](https://developer.apple.com/documentation/xcode/distributing-your-app-to-registered-devices)
- [Apple unlisted distribution](https://developer.apple.com/support/unlisted-app-distribution)
- [Apple background tasks](https://developer.apple.com/documentation/backgroundtasks/refreshing-and-maintaining-your-app-using-background-tasks)
- [Apple background notifications](https://developer.apple.com/documentation/usernotifications/pushing-background-updates-to-your-app)
- [Apple Keychain](https://developer.apple.com/documentation/security/storing-keys-in-the-keychain)
- [Apple privacy manifests and required-reason APIs](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api)
- [Apple in-app account deletion](https://developer.apple.com/support/offering-account-deletion-in-your-app/)
- [Apple App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/)
- [Apple CKSyncEngine](https://developer.apple.com/documentation/cloudkit/cksyncengine-4b4w9)
- [WebKit iOS Home Screen Web Push](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/)
- [WebKit storage policy](https://webkit.org/blog/14403/updates-to-storage-policy/)
- [Tauri mobile prerequisites](https://v2.tauri.app/start/prerequisites/)
- [Tauri mobile plugins](https://v2.tauri.app/develop/plugins/develop-mobile/)
- [Tauri iOS signing](https://v2.tauri.app/distribute/sign/ios/)
- [Google OAuth for installed iOS apps](https://developers.google.com/identity/protocols/oauth2/native-app)
- [SQLite WAL](https://www.sqlite.org/wal.html)
- [Tailscale Serve](https://tailscale.com/docs/reference/tailscale-cli/serve)
- [HPKE, RFC 9180](https://www.rfc-editor.org/rfc/rfc9180.html)
