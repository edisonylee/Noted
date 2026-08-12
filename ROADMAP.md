# Noted product roadmap

Status: active, outcome-gated

Last updated: 2026-08-06

This roadmap turns [`PRODUCT_STRATEGY.md`](PRODUCT_STRATEGY.md) into an execution
sequence. The competitive evidence and measurable product contract live in
[`docs/COMPETITIVE_LANDSCAPE.md`](docs/COMPETITIVE_LANDSCAPE.md). Phases are gates,
not calendar promises. A later phase may be explored in a prototype, but it must
not displace the proof required by the active phase.

## Current phase

Phase 1 is active. Some outcomes already exist in partial form, but permission
stability, speaker attribution, retained-audio compression, per-meeting retention,
and a validated user proof gate remain incomplete. Phases 2–6 are direction, not
shipped-product claims.

## North star

**Weekly verified context reuse:** users regularly retrieve, cite, correct, or act
on context captured in an earlier session.

The product must prove this progression:

```text
Capture reliably
      ↓
Create trustworthy context
      ↓
Resurface it in the user's day
      ↓
Share it safely with authorized agents
      ↓
Provide managed continuity and action
```

## Competitive must-win gates

A phase does not advance because its feature list is complete. It advances only
when the relevant must-win standards in the competitive landscape are measured
and pass:

| Roadmap phase | Required must-win standards |
|---|---|
| Phase 1 | Trustworthy capture and speaker identity; meeting evidence for every material memory |
| Phase 2 | Cross-source evidence lifecycle; correctable time-aware truth; complete user ownership |
| Phase 3 | One cross-domain context model and daily loop; consumer-grade zero maintenance |
| Phase 4 | Least-privilege Context Passes for agents |
| Phases 5–6 | Preserve all earlier trust gates while proving managed economics and safe action |

Targets in that document are provisional until baselined, but words such as
“reliable,” “accurate,” “private,” and “better” do not count as proof by
themselves.

## Phase 1 — Trustworthy meeting memory

Goal: make the initial wedge excellent enough to create a weekly habit and prove
that source-backed memory is valuable.

### Product outcomes

- Meeting capture works across supported meeting apps without repeated
  unnecessary permission prompts.
- Speaker enrollment, attribution, correction, and learning are understandable
  and improve across meetings.
- Transcript lines and summary claims can open their timestamped source.
- Fresh production installs use transcript-only by default.
- A remembered global default and per-meeting **Keep audio after meeting** override
  give the user control without another mandatory prompt.
- Retained microphone and system audio are compressed locally and deletable.
- Calendar attendees and relevant prior context prepare the meeting.
- Decisions and commitments flow into Today/schedule rather than remaining inside
  the meeting page.
- Export and deletion work without a cloud account.

### Proof gate

- 10–20 target users capture at least three meetings per week for four weeks.
- The quantitative capture, permission, speaker-attribution, and citation gates
  in Must-wins 1 and 2 of the competitive landscape pass on recorded evaluations.
- Users revisit citations or extracted commitments in later sessions.
- At least half of the cohort says losing Noted would materially disrupt their
  workflow.

### Current technical references

- [`MEETINGS_PLAN.md`](MEETINGS_PLAN.md)
- [`docs/decisions/001-meeting-audio-retention.md`](docs/decisions/001-meeting-audio-retention.md)
- [`docs/COMPETITIVE_LANDSCAPE.md`](docs/COMPETITIVE_LANDSCAPE.md)

## Phase 2 — Unified context foundation

Goal: ensure meetings, notes, calendar, journal, tasks, people, and future sources
can participate in one trustworthy system rather than parallel feature silos.

### Product outcomes

- Stable portable identities, revisions, provenance, and lifecycle state exist
  for addressable source records.
- Person identities support typed, normalized contact methods (email first),
  including source provenance, primary/secondary status, and explicit user
  confirmation instead of relying on untyped aliases.
- Lossless sources are separated from rebuildable summaries, chunks, vectors, and
  inferred relations.
- Notes and transcripts share one hybrid lexical/semantic retrieval contract.
- Citations survive reindexing and resolve to exact source evidence.
- Corrections and approved memories remain canonical and improve later results.
- Backup, verified restore, trash, permanent deletion, and complete export are
  consumer-ready.
- Spaces and sensitivity boundaries are enforced by retrieval, not only by UI.

### Proof gate

- Cross-source questions return accurate, cited answers on a fixed evaluation set.
- Rebuilding every derived index preserves canonical data and citations.
- Backup/restore and deletion pass deterministic end-to-end tests.
- No query or source type requires a model or network for exact/lexical access.
- Corrections measurably improve a repeated extraction or retrieval task.

### Current technical reference

- [`docs/AGENT_CONTEXT_IMPLEMENTATION_PLAN.md`](docs/AGENT_CONTEXT_IMPLEMENTATION_PLAN.md)

## Phase 3 — The daily context loop

Goal: make Noted valuable between meetings and prove it is a personal context
system rather than a recorder.

### Product outcomes

- Today combines calendar, commitments, follow-ups, tasks, and relevant context in
  one calm daily surface.
- Meeting preparation surfaces people, prior decisions, open questions, and
  commitments with citations.
- People and project views reconcile context across meetings, notes, and time.
- Facts can be corrected, superseded, or marked uncertain without erasing history.
- Journal, voice, photos, and manual notes use the same source and retrieval model.
- One additional connector at a time is selected from observed user demand.

### Proof gate

- Users retrieve context more than seven days old every week.
- Context captured in one surface changes an action in another surface.
- Weekly verified context reuse improves beyond meeting-only behavior.
- The daily surface retains users even during a week with few meetings.

## Phase 4 — Permissioned agent context

Goal: let external agents use Noted without turning the vault into an unrestricted
database or filesystem share.

### Product outcomes

- A local read-only MCP/API adapter exposes search, source retrieval,
  commitments, and token-budgeted context packets.
- Results carry stable identity, provenance, citations, and uncertainty.
- Access can be scoped by space, project, source type, time, and sensitivity.
- The user can inspect the exact context packet before external disclosure.
- Agent-specific permission and disclosure receipts are visible.
- Write-back is a proposed change requiring approval, not direct mutation.
- External agents may prepare a context-sharing proposal, but only Noted's
  interactive approval surface may authorize delivery.
- Portable exports keep the user independent of the official service.

### Proof gate

- At least two external agents complete real tasks better with Noted context than
  without it.
- Every returned material assertion remains traceable to a source.
- Permission tests find no cross-space or out-of-scope leakage.
- Users can correctly explain and control what each connected agent can access.

## Phase 5 — Managed Noted Cloud

Goal: monetize continuity, availability, and convenience while preserving the
local-first product.

### Product outcomes

- Account-based encrypted sync and tested restore work across supported devices.
- Remote/mobile access does not require the desktop to be on the same LAN.
- Managed retrieval and hosted inference are available to users who do not want
  local models or provider configuration.
- A paid plan includes a reasonable compressed-media allowance.
- Heavy storage and inference are metered independently and have transparent
  add-ons.
- Google Drive is available as an optional user-owned mirror/export, not a primary
  dependency.
- Backup-only encryption and cloud-readable context are clearly separate consent
  choices.

### Proof gate

- Sync conflict, restore, device loss, deletion, and account closure scenarios
  pass end-to-end tests.
- Managed plans model greater than 80% gross margin at observed usage.
- Users pay for continuity/retrieval value rather than describing the purchase as
  commodity storage.
- Local-only users retain capture, search, export, and deletion functionality.

### Current architectural reference

- [`docs/decisions/002-context-cloud-boundaries.md`](docs/decisions/002-context-cloud-boundaries.md)

## Phase 6 — Focused action and collaboration

Goal: use proven context to help execute work without becoming an ungrounded
generic assistant.

### Product outcomes

- Noted drafts meeting preparation, daily briefs, follow-ups, and scheduling
  suggestions from cited context.
- External actions are previewed, attributable, and user-approved.
- From the global assistant, a user can ask Noted to share notes from a named or
  recent meeting with a known person. Noted deterministically resolves the
  meeting and recipient, then shows the exact sender, recipient, source meeting,
  subject, included content, exclusions, and attachments before delivery.
- Meeting sharing uses an immutable context-packet snapshot. The initial content
  contract is the latest primary summary plus user-authored meeting notes;
  transcripts, audio, alternate summaries, and private annotations are excluded
  unless the user explicitly requests them.
- Email is the first delivery transport: begin with a prefilled native macOS mail
  draft, then add optional direct provider sending after a separate, minimal
  authorization grant. Both paths use the same internal capability rather than
  routing Noted's own assistant through MCP.
- Every delivery has an idempotent local receipt containing the packet hash,
  source IDs, recipient contact, sender identity, transport, status, timestamp,
  and provider message ID when available.
- Shareable context packets and explicit shared project spaces come before broad
  team administration.
- Enterprise controls are added only after clear team pull.

### Proof gate

- Suggested actions are accepted often enough to save measurable time.
- Users can distinguish source facts, inferences, and proposed actions.
- Fixed evaluations confirm that meeting/person resolution selects the intended
  records, ambiguous names or multiple plausible meetings block for
  clarification, source text cannot trigger an outbound action, cancellation
  sends nothing, and retries cannot create duplicate messages.
- In every evaluated delivery, the recipient receives exactly the packet the
  user previewed and approved.
- Shared context has clear ownership, retention, and removal behavior.
- Team demand justifies the compliance and administrative surface.

## Parallel enabling tracks

These do not create separate product phases; they make every phase shippable.

### Trust, privacy, and release quality

- Permission onboarding, consent, encryption, signing, notarization, updater,
  privacy copy, crash recovery, migrations, backup, restore, and deletion.

### Evaluation

- Fixed suites for capture reliability, speaker attribution, extraction,
  retrieval, grounding, citation resolution, permissions, and data lifecycle.

### Inference economics

- Provider-neutral capability routing, local/BYOK modes, smallest-sufficient-model
  selection, caching, incremental indexes, and measured build-versus-buy gates.

### Portability

- Stable public IDs, deterministic exports, import ownership rules, and contracts
  that do not expose macOS paths, SQLite row IDs, or one hosted provider.

## Roadmap decision register

| Decision | Current direction | Revisit gate |
|---|---|---|
| Product category | Personal context system; “context vault” as supporting language | After Phase 3 research |
| Initial market | Individual prosumer knowledge workers | Before team work |
| Wedge | Meetings connected to calendar and commitments | Phase 1 proof gate |
| Production audio default | Transcript only | Phase 1 user research |
| Retained audio | Per-meeting choice, compressed locally | After real storage/playback data |
| Managed storage | Bundled allowance plus heavy-use add-ons | Before paid cloud beta |
| Google Drive | Optional backup/export | Before Phase 5 implementation |
| Agent interface | Permissioned local read-only surface first | Phase 4 threat-model review |
| Outbound context delivery | Email first; native macOS draft before optional direct provider send; exact preview, approval, and receipt required | Phase 6 entry after identity and context-packet contracts exist |
| Hosted inference | Optional; local and BYOK remain supported | Before paid cloud beta |
| Self-hosted models | Only after measured fully loaded cost and quality win | At sustained paid utilization |
| Teams | After personal retention and explicit team pull | Phase 6 entry |

## Ideas that remain parked

The following should not enter the active phase without new evidence:

- broad team administration;
- dozens of connectors;
- autonomous or unreviewed send/edit/delete actions;
- a second writable Markdown truth store;
- graph-database migration;
- unlimited bundled media;
- a proprietary general-purpose foundation model; and
- features that increase capture volume without improving later context reuse.
