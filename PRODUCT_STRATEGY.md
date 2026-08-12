# Noted product strategy

Status: active product direction

Direction recorded: 2026-08-06

Review cadence: revisit after each roadmap proof gate, not after every feature idea

This document is the product-level source of truth for what Noted is becoming and
how individual features fit together. [`ROADMAP.md`](ROADMAP.md) owns sequencing
and proof gates. [`docs/COMPETITIVE_LANDSCAPE.md`](docs/COMPETITIVE_LANDSCAPE.md)
owns the evidence behind differentiation and the must-win product contract.
Detailed implementation plans remain authoritative within their technical scope,
and accepted decisions in [`docs/decisions`](docs/decisions) override older plan
defaults when they conflict.

This is a statement of direction, not a claim that every described capability is
already implemented.

## Strategy in one sentence

**Noted turns what happens in a person's life and work into private,
source-grounded context that the person—and any agent they authorize—can use.**

Two-sentence product description:

> Noted is a private context layer that turns your meetings, projects, calendar,
> notes, and daily life into an evolving memory you own. It gives you and the AI
> agents you authorize the right context to understand what matters and act on it.

The concise product hierarchy is:

- **Wedge:** trustworthy meeting capture and follow-through.
- **Product:** a private personal context system.
- **Platform:** a permissioned context layer for agents.
- **Business:** managed continuity, hosted intelligence, and convenience.

“Context vault” is useful shorthand for ownership and durability, but it is not
the whole promise: a vault sounds passive. Noted must also resurface context and
help the user act on it.

Working external language:

- **What it does now:** Noted turns meetings into reliable memory and action.
- **Where it goes:** A private context system for your life.
- **Compact line:** AI memory built from your real life.

## Meetings are the wedge, not a pivot

Starting with meetings narrows execution without narrowing the company. Meetings
are recurring, information-dense events that naturally connect people, calendar
time, projects, decisions, commitments, questions, and changing facts. They also
give Noted original source evidence rather than another copy of already-processed
information.

The durable asset is not a transcript. It is the evolving, source-backed context
created from the transcript:

- who was involved;
- what happened and when;
- what was decided, promised, or left unresolved;
- which people, projects, and prior facts matter;
- what is currently true and what used to be true; and
- where every important claim came from.

The strategic guardrail is that meeting data cannot become a separate product
silo. Transcripts, notes, decisions, commitments, people, and citations must enter
the same record, retrieval, permission, and lifecycle model used by every future
source.

The complete loop is:

```text
Prior context prepares the meeting
              ↓
The meeting creates timestamped evidence
              ↓
Noted extracts decisions, commitments, and changed facts
              ↓
Today, calendar, people, and projects resurface what matters
              ↓
The user or an authorized agent uses the verified context
```

The demo should show this loop, not merely transcription and a generated summary.

## Customer and market sequence

Noted should be a personal product with a prosumer beachhead. The first users are
individual knowledge workers—founders, product builders, consultants, creators,
researchers, and operators—whose context crosses meetings, tools, projects, and
organizations. They feel the problem intensely and can adopt without procurement.

“Consumer” describes the ownership and experience: one person owns the vault,
understands its behavior, and receives value without an administrator. It does
not require launching to everyone at once.

Teams come later, after the personal loop retains users. The sequence is:

1. individual vault ownership;
2. explicit shareable context packets;
3. shared project spaces with clear source ownership;
4. only then administration, organization retention, SSO, compliance, and
   per-seat pricing.

Beginning as a team product would force collaboration and compliance work before
Noted has proven the personal value that makes the context worth sharing.

## Positioning and differentiation

Noted should not lead with “AI chief of staff.” That category promises autonomous
action before the product has earned enough trust and hides the more defensible
asset: accurate longitudinal context.

Noted should also avoid the generic claim “one memory for every AI.” Supermemory
already uses that territory and provides consumer memory, connectors, MCP, and an
agent platform. Obsidian now combines a mature local vault and plugin ecosystem
with CLI and headless agent access. Mem0/OpenMemory provides memory infrastructure
and agent integrations. Agent connectivity by itself is therefore a requirement,
not a moat.

Meeting and relationship systems have converged just as far. Groupthink already
combines private desktop capture, relationship intelligence, reviewable facts,
projects, preparation, and agent access. SavirOS and Flownote connect meetings to
relationship history and follow-through. Granola, TwinMind, Meeting.ai, and
Genspark each cover substantial parts of the meeting-to-context-to-agent vision.
The current evidence and claims Noted must avoid are maintained in
[`docs/COMPETITIVE_LANDSCAPE.md`](docs/COMPETITIVE_LANDSCAPE.md).

Primary references:

- [Obsidian overview](https://obsidian.md/) and
  [headless agent access](https://obsidian.md/help/headless)
- [Supermemory personal product](https://supermemory.ai/personal/) and
  [connectors](https://supermemory.ai/docs/connectors/overview)
- [Mem0](https://docs.mem0.ai/introduction) and
  [Mem0/OpenMemory](https://mem0.ai/openmemory)

Noted will be worth choosing if it can prove a stronger combination of:

1. **Context creation at the source.** Native meetings first, followed
   deliberately by calendar, voice, photos, files, and other demanded sources.
2. **Verifiable memory.** Important claims retain source, timestamp, speaker,
   event, and—when the user chose to keep it—audio evidence.
3. **Context to action.** Memory improves preparation, follow-up, scheduling,
   commitments, and plans rather than ending at search results.
4. **Consumer simplicity.** No manual vault gardening, API setup, or repeated
   memory prompting is necessary for the core experience.
5. **User-controlled access.** Local-first storage, granular retention, scoped
   agent permissions, access history, and portable exports are product features.
6. **Longitudinal truth.** People, projects, promises, preferences, and changing
   facts are reconciled over time instead of accumulating as disconnected text.

Noted does not need to beat Obsidian at Markdown customization or Mem0 at being a
developer API. It should interoperate with those ecosystems while being much
better at turning real life into trustworthy, actionable personal context.

## Product principles

### Broad vision, narrow execution

Only expand capture after the meeting loop is reliable and regularly used. One
excellent new source is more valuable than many shallow connectors.

### Sources are canonical; intelligence is derived

Original notes, transcript segments, calendar snapshots, edits, and approved
memories are durable records. Embeddings, chunks, summaries, inferred relations,
and generated answers are rebuildable. Corrections must improve future behavior.

### Evidence before magic

Answers and extracted context should resolve to inspectable sources. When Noted
is uncertain, it should preserve uncertainty instead of presenting a generated
claim as fact.

### Private and useful without the cloud

Local-only remains a complete mode. Managed cloud services add continuity and
convenience; they do not hold basic export, deletion, or retrieval hostage.

### Access is purpose-bound

An agent receives the smallest useful context packet for a declared task, not
ambient access to the whole vault. Write-back begins as a proposed change that
the user can inspect.

### The model is replaceable

Features depend on capability contracts and quality evaluations, not one model or
provider. The moat is trusted context and the loop around it, not owning a general
foundation model.

## Moat hypotheses

Noted does not yet have a proven moat. The moat hypothesis compounds through:

- longitudinal, source-grounded personal context;
- user-corrected identities, speakers, entities, and relationships;
- temporal understanding of facts that change;
- a trusted local capture layer;
- permissioned retrieval that works across agents;
- context-to-action workflows; and
- product-specific evaluations for attribution, recall, grounding, and leakage.

Portability does not weaken this moat. Users should remain because Noted knows
their context accurately and helps them use it, not because their data is trapped.

## Business model and margin discipline

Noted should monetize managed outcomes, not commodity storage:

- **Local:** local storage and local models or user-provided providers.
- **Noted Pro:** encrypted sync/backup, cross-device access, managed retrieval,
  a reasonable compressed-media allowance, and a hosted-inference allowance.
- **Heavy usage:** transparent storage or inference add-ons only when usage is far
  above the included allowance.
- **Google Drive:** an optional user-owned backup/export destination, not the
  primary playback or sync system.

Storage and inference should have separate internal meters even when presented as
one simple subscription. This prevents one unusually heavy workload from hiding
another and allows pricing to change without changing the product model.

Margin rules:

1. Keep transcript-only as the fresh production default.
2. Compress retained audio and lifecycle-delete temporary sources.
3. Bundle a modest allowance; do not position Noted as a storage reseller.
4. Preserve Local and bring-your-own-provider modes to cap inference cost and
   maintain user choice.
5. Route each task to the smallest model that passes a task-specific evaluation.
6. Cache and incrementally update derived context instead of repeatedly processing
   the entire vault.
7. Self-host a model only when sustained measured utilization makes its fully
   loaded cost lower than alternatives without reducing quality or reliability.
8. Prefer specialized transcription, embedding, extraction, and ranking models
   before considering ownership of a general-purpose model stack.
9. Target greater than 80% gross margin on managed plans before scaling them.

Owning GPUs does not automatically improve margins. At low utilization, idle
capacity, operations, failover, and support can make self-hosting more expensive
than an API. Provider-neutral routing preserves the option without making it a
premature dependency.

## Product scorecard

The north-star metric is **weekly verified context reuse**: a user revisits,
retrieves, cites, corrects, or acts on previously captured context in a later
session.

Supporting measures include:

- successful meeting captures per active user;
- useful decisions and commitments resurfaced later;
- speaker-correction rate after enrollment;
- cited-answer accuracy and source-open rate;
- seven- and thirty-day context reuse;
- agent tasks improved by Noted context without permission leakage;
- cloud attach rate and cost per managed active user; and
- complete export, restore, and deletion success.

Capture volume and stored gigabytes are diagnostics, not success metrics.

## Feature admission rule

Every major feature must do at least one of four things:

1. capture valuable context;
2. improve its truthfulness;
3. retrieve it at the right moment; or
4. let the user or an authorized agent use it safely.

If it does none of these, it is outside the current strategy.

## Explicit non-goals for the current horizon

- Competing with Obsidian's entire editor and plugin ecosystem.
- Becoming a generic blob-storage service.
- Adding many connectors before the core loop retains users.
- Unrestricted agent access to the database or filesystem.
- Autonomous external actions before context quality and approval UX are proven.
- Replacing SQLite with a graph database because the product uses relationships.
- Building team administration before individual product-market pull.
- Self-hosting a general LLM as a branding exercise.

## Related sources of truth

- [`ROADMAP.md`](ROADMAP.md) — outcome-gated product sequence
- [`docs/COMPETITIVE_LANDSCAPE.md`](docs/COMPETITIVE_LANDSCAPE.md) — competitor
  evidence, table stakes, and measurable must-win standards
- [`docs/AGENT_CONTEXT_IMPLEMENTATION_PLAN.md`](docs/AGENT_CONTEXT_IMPLEMENTATION_PLAN.md)
  — detailed context contracts, retrieval, permissions, and implementation phases
- [`docs/decisions/001-meeting-audio-retention.md`](docs/decisions/001-meeting-audio-retention.md)
  — accepted meeting audio policy
- [`docs/decisions/002-context-cloud-boundaries.md`](docs/decisions/002-context-cloud-boundaries.md)
  — accepted local/cloud/agent boundary
- [`MEETINGS_PLAN.md`](MEETINGS_PLAN.md) — meeting implementation history and
  remaining technical work
- [`docs/BYOK_PROVIDER_PLAN.md`](docs/BYOK_PROVIDER_PLAN.md) — provider-neutral
  inference direction
