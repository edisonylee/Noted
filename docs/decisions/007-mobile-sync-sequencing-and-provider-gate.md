# Decision 007: Mobile sync sequencing and hosted-provider gate

Status: accepted

Date: 2026-08-16

Owners: product, mobile, data architecture, security, and hosted services

Implementation status: M3 portable Notes records are implemented. M4 now has a
sanitized-fixture Notes UI/store, deterministic convergence application logic,
and the logical pairing/direct-sync cores. The narrow router, scope and replay
rules, TLS/pin evidence, bootstrap/push/pull/checkpoint semantics, and local
conflict preservation are tested without a network. Bootstrap is paged against
one authenticated checkpoint, accepted transactions are guaranteed pullable,
and revocation is serialized with in-flight fixture requests. Production
cryptography, the TLS/HTTP adapter, durable Mac authority/device registry,
external review, and combined physical Mac-to-iPhone validation are not
complete. Direct sync and the hosted relay therefore have not shipped, and the
M4 provider-evaluation gate has not opened.

## Context

Noted needs the iPhone to synchronize with the user's local Mac without making a
hosted account, provider, or monthly bill a prerequisite. It also needs a path to
reliable off-LAN access, recovery, and additional users later, when paying for
managed infrastructure is justified.

Choosing a hosted vendor before the record, conflict, encryption, and recovery
contracts converge would couple foundational data behavior to a provider API.
Building a separate direct-sync format first would create an equally costly
migration when a relay arrives.

This decision refines the mobile direction in
[Decision 006](006-iphone-companion-direction.md) and the detailed milestones in
[the mobile companion implementation plan](../MOBILE_COMPANION_IMPLEMENTATION_PLAN.md).
It does not accept the still-proposed record and concurrency decisions by
implication; the specific Decision 003 and 004 contracts remain an M3 gate.

## Decision

### Direct paired-Mac sync comes first

- M3 creates portable Notes identities, revisions, lifecycle state, a local
  change journal, and safe migrations on both replicas.
- M4 proves encrypted, authenticated Mac-to-iPhone Notes synchronization over a
  narrow local transport.
- The Mac is the accepted-head authority during direct-only operation. The phone
  keeps durable pending branches until the Mac accepts, rejects, or conflicts
  them.
- Pairing uses a one-time invitation, explicit user verification, pinned device
  identity, scoped enrollment, and revocation. No durable credential appears in
  a URL, QR history, browser storage, or log.
- The direct endpoint exposes versioned sync, bootstrap, checkpoint, and blob
  operations only. It cannot invoke arbitrary Tauri commands or read arbitrary
  Mac paths.
- Correctness cannot depend on Bonjour discovery, continuous LAN reachability, or
  iOS background execution. Manual reconnect and foreground sync remain valid.

### One protocol, more than one transport

The direct adapter and future hosted relay implement the same logical contract:

1. negotiate protocol, record-kind, reader, and writer capabilities;
2. enroll or authenticate a device;
3. bootstrap from an authenticated snapshot and cursor;
4. push signed, idempotent mutation transactions against an accepted head;
5. pull ordered accepted mutations and checkpoints;
6. transfer encrypted, content-addressed blob chunks;
7. acknowledge device and purge generations; and
8. revoke devices and rotate key epochs.

Transport code may differ, but record serialization, application-layer
encryption, mutation identity, branch/conflict semantics, tombstones, cursors,
and integrity checks do not. Provider-specific identifiers stay inside the
hosted adapter.

Direct mode records an authority generation and the Mac device that owns it.
Cloud enrollment performs an explicit, checkpointed authority-generation
cutover. After that cutover, no Mac or phone may independently create accepted
heads against the old generation. A direct connection may continue as a faster
transport or recovery path, but it must submit through the active authority
contract so direct and relay histories cannot split.

### Hosted continuity is an intentional later phase

The M7 relay adds capabilities that direct LAN sync cannot reliably provide:

- off-LAN and Mac-asleep durable delivery;
- clean-device encrypted restore;
- account and multi-device enrollment;
- remote revocation and APNs refresh hints;
- encrypted blob durability, backups, export, purge, and account closure; and
- an operational path for additional testers and eventual paid continuity.

The relay stores application-layer ciphertext plus the minimum routing and
integrity metadata required by the protocol. It never receives library keys or
plaintext canonical content in the backup/sync mode. Record kind, opaque library
and device identifiers, revisions, sizes, timing, access patterns, and protocol
versions may remain visible and must be disclosed accurately.

Hosted inference or cloud-readable retrieval is a separate product mode with a
separate consent boundary. Purchasing sync infrastructure does not authorize
personal context to be decrypted by the service.

### No hosted-provider selection or spend now

No hosted sync account, production database, object store, notification service,
or paid plan is required for M3 or M4. Local fixtures, deterministic convergence
tests, and the paired Mac provide the first implementation environment.

Provider evaluation begins only after the M4 Notes gate proves that:

- Mac and iPhone replicas converge under duplicated, reordered, interrupted, and
  conflicting delivery;
- offline edits and tombstones survive termination and restart;
- the accepted-head/local-branch model preserves acknowledged user work;
- pairing, revocation, schema negotiation, and encrypted bootstrap pass their
  threat tests; and
- the transport adapter contract is stable enough to implement twice.

The current sanitized-fixture checkpoint is evidence toward those conditions,
not a pass. It proves the logical store, convergence, pairing state-machine, and
narrow routing contracts in deterministic tests. It does not prove production
key custody or cryptographic interoperability, a pinned TLS 1.3 connection, a
durable Mac authority, real-device background/lifecycle behavior, or an external
security review.

Hosted provisioning and spend belong to M7, when off-LAN access, clean-device
recovery, or an external tester cohort provides a concrete need. Paying for a
provider then is acceptable. A free tier may be used for a disposable prototype,
but price is not allowed to bypass the same data, security, restore, deletion,
and operations gates.

Apple Developer Program or TestFlight spending is a separate distribution
decision. It neither selects nor substitutes for the hosted sync provider.

## Provider evaluation gate

The M7 design review compares providers against the contract, not the other way
around. A candidate must support or permit Noted to implement:

| Area | Required evidence before selection |
|---|---|
| Identity | Scoped account/device sessions, key rotation, revocation, and exportable identity data |
| Mutation authority | Conditional writes, ordered cursors, idempotency, bounded atomic transactions, and safe compaction |
| Blob storage | Resumable encrypted chunks, integrity metadata, range reads, lifecycle controls, and verifiable deletion |
| Operations | Regional choice, encrypted backups, tested restore, rate limits, audit events, metrics, and incident export |
| Privacy | Contractual and technical separation of ciphertext sync from any optional plaintext processing |
| Portability | Standard Postgres-compatible and S3-compatible boundaries or an equivalent adapter with complete export |
| Cost | Measured cost at owner dogfood, small beta, typical paid use, and heavy-media use; no hidden egress dependency |
| Exit | A tested provider migration and service-shutdown export path |

Postgres-compatible control planes, S3-compatible object stores, and managed
auth/compute products are implementation candidates, not accepted vendors. The
selection ADR will name the vendor, regions, cost model, data-processing terms,
backup/restore proof, and exit plan when M7 begins.

## Consequences

### Positive

- The owner gets useful local Mac-to-iPhone sync before cloud operations exist.
- No money or vendor commitment is required to prove the hardest correctness
  properties.
- Hosted continuity can be added without a second record model or destructive
  data migration.
- The same encrypted payload can travel over LAN or through an untrusted relay.
- Provider competition remains available when real load and tester needs are
  measurable.

### Costs and risks

- The direct adapter is real product work, not a throwaway remote-control bridge.
- Authority cutover, recovery, revocation, and purge generations still require
  separate accepted protocol ADRs and adversarial tests.
- Direct-only use cannot guarantee sync while the Mac is asleep, off-LAN, or
  unreachable; the UI must say so without treating local work as failed.
- Application-layer encryption limits server-side inspection and requires strong
  client diagnostics, poison-record quarantine, and recovery design.

## Revisit gates

- Revisit provider candidates after M4 passes, before M7 implementation starts.
- Revisit the direct-only user experience after owner dogfood measures how often
  the Mac is unreachable when sync is needed.
- Revisit CloudKit only if Apple-only scope becomes an intentional long-term
  constraint or the custom relay fails its security, operations, or cost gates.
- Revisit cloud-readable processing only through a separate consent and threat
  decision; it is not implied by this ADR.
