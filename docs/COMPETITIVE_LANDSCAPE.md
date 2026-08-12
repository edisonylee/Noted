# Noted competitive landscape and must-win product contract

Status: active research snapshot

Last verified: 2026-08-06

Next required review: before external positioning, fundraising, or entry into a
new roadmap phase

This document records the competitive evidence behind Noted's product strategy.
It is deliberately stricter than marketing copy. A capability is "confirmed"
only when a vendor currently describes it in an official product page or
documentation; that does not mean it was independently tested. Private-beta and
coming-soon claims are labeled when they materially affect the comparison.

The market changes quickly. Recheck the linked primary sources before making an
external claim, and date any new evidence added here.

This is a decision-oriented map of the competitors that constrain Noted's
positioning, not an exhaustive directory of every transcription or CRM product.

## Executive verdict

The category is real, valuable, and crowded. Several products already combine
meeting capture, relationship memory, calendar context, commitments, preparation,
and agent access. In particular, Groupthink, SavirOS, and Flownote make "meeting
notetaker plus relationship memory" an existing category, not empty whitespace.
Meeting.ai and Genspark also make "meetings become memory that agents can use" an
existing direction.

Three conclusions follow:

1. **Consumer-oriented is a market and ownership choice, not a sufficient
   differentiator.** Mesh, Dex, TwinMind, Hallway, Groupthink, and others already
   serve individuals or mix personal and professional contexts.
2. **No single feature in the current vision is unique.** Local capture, speaker
   labels, source links, relationship histories, commitments, pre-meeting briefs,
   calendar views, broad memory, export, and MCP access all have precedents.
3. **Noted can still win, but it must win a product contract rather than a feature
   checklist.** The credible opening is user-controlled chain of custody for
   context: local capture, inspectable evidence, durable corrections, time-aware
   memory, daily reuse, portability, and least-privilege disclosure to agents.

The working position is:

> **Noted is the user-owned context system that turns conversations and daily
> work into source-verifiable memory, then gives each AI only the context the
> user approves.**

This is a direction and a testable promise, not a claim that the complete product
already exists. No reviewed public product demonstrated the entire contract as
of the verification date, but competitors cover every component individually
and some cover most of the loop.

## The defining difference to build

The root security and product identity in Noted is the person, not an employer,
workspace, CRM record, model vendor, or agent. That distinction must produce
observable behavior:

```text
The user captures a real source locally
                    ↓
Noted preserves who said what, when, and with what confidence
                    ↓
The user can correct identity, meaning, and what is currently true
                    ↓
The correction improves later preparation, schedule, and retrieval
                    ↓
An agent receives a purpose-bound Context Pass, not the whole vault
                    ↓
The user can inspect the disclosure, trace the answer, and revoke access
```

This is stronger than saying "private," "personal memory," "relationship OS,"
or "AI chief of staff." Each of those phrases is broad enough for competitors to
claim without implementing the full trust boundary.

Noted's current local-first architecture is a credible starting advantage. It is
not yet a defensible product difference while capture reliability and speaker
attribution have user-reported failures, the cross-domain context model remains
in progress, and permissioned agent access is planned rather than shipped.

## Competitive map

### Direct competitors

| Product | Publicly supported or claimed overlap | Most honest opening for Noted | Threat |
|---|---|---|---|
| [Groupthink](https://groupthink.com/) | Bot and private desktop capture; meetings, relationships, open threads, daily briefs, projects, reviewable AI facts, source timestamps, and read/write MCP | A complete local mode without account dependence; portable canonical records; per-agent and per-purpose disclosure instead of account-wide token access; a daily model spanning schedule, decisions, projects, notes, and self | Critical |
| [SavirOS](https://saviros.com/) | Unified calendar, automatic meeting bot, preparation, transcription, speaker ID, promises, relationship memory, going-cold signals, and action-oriented internal agents | User-owned local context, exact evidence behind inferred promises and facts, open external-agent portability, and a broader personal context model | High |
| [Flownote](https://www.flownote.ai/relationship-memory) | Bot-free capture, speaker identification, synchronized audio, decisions and actions, person histories, unresolved questions, pre-meeting context, and cross-note chat | A source model beyond meetings, a stronger schedule and commitment loop, full local operation, and scoped external-agent access | High |
| [Hallway](https://hallwayai.app/) | Consumer conversation capture, automatic contacts, commitments, calendar briefs, search, JSON export, and audio deletion after transcription | Ship the reliable native Mac and broad-context loop while Hallway remains invite-only and lists native apps, speaker identification, and meeting integrations as coming soon | Medium, but exact concept |

Groupthink is the closest current product and the most important benchmark. Its
official documentation describes private desktop capture, a personal CRM built
from calendar and meetings, verified versus suggested relationship facts,
source-meeting timestamps, Projects for work/personal/family material, and MCP
tools that can read and write the relationship record. Noted must not describe
those capabilities as unique.

### Near-direct competitors

| Product | Publicly supported or claimed overlap | Most honest opening for Noted | Threat |
|---|---|---|---|
| [Granola](https://www.granola.ai/) | Bot-free capture, source-linked meeting answers, decisions/actions, People and Companies histories, preparation, shared context, API, and MCP | Individual-owned context across life domains, true local completeness, daily schedule and commitment execution, and least-privilege agent disclosure | High; scaled incumbent |
| [TwinMind](https://twinmind.com/) | Consumer meeting and ambient capture, summaries and tasks, archive chat, calendar/email context, audio deletion by default, optional on-device notes, and read-only MCP | Durable source-backed temporal memory, Mac-local completeness, relationship/commitment UX, and more granular agent control | High |
| [Meeting.ai](https://meeting.ai/en/p) | Before/during/after assistance, capture, voice identity, exact-source answers, calendar and contacts, compounding meeting memory, and agent-produced decks/docs/sheets | Local and portable context, a longitudinal model beyond meetings, an actual daily schedule, and interoperability with outside agents rather than one proprietary agent | High |
| [Genspark SecondBrain](https://www.genspark.ai/blog/genspark-ai-workspace-6) | Persistent context from email, meetings, chats, docs, apps, and projects; conversation-capture hardware; an agent that uses the combined context to produce work | Focus, transparency, local ownership, source/correction semantics, relationship and commitment UX, and permissioned access for any agent | High to long-term vision |

Granola explicitly describes conversation transcripts as company context and now
offers Spaces, API, and MCP. TwinMind already combines a consumer memory story,
transcript-only default, an on-device storage option on iOS, and read-only agent
access. Genspark explicitly calls SecondBrain a memory layer across meetings,
email, documents, apps, and projects. These facts remove "context layer," "audio
deleted by default," and "MCP for my memories" as standalone differentiation.

### Meeting incumbents and capture hardware

| Product | Capability that establishes table stakes | Strategic implication |
|---|---|---|
| [Otter](https://help.otter.ai/hc/en-us/articles/360035266494-What-is-Otter) | Transcript/audio playback, learned speaker profiles, corrections that improve later labeling, cited action items, calendar, and AI chat/actions | Speaker learning, editable attribution, playback, and transcript-linked tasks are mature expectations |
| [Fireflies](https://guide.fireflies.ai/articles/1193528158-what-is-fireflies-ai) | Bot or bot-free system-audio capture, live notes, tasks, daily digest, meeting prep, cross-meeting AskFred, and broad integrations | A meeting product can already look like a daily assistant; a schedule and digest alone will not separate Noted |
| [Fathom](https://fathom.video/pricing) | Recording storage, summaries, actions, follow-up email, Ask Fathom, clips/playlists, global search, and CRM sync | Polished capture, shareable evidence, and downstream workflows define the incumbent bar |
| [PLAUD](https://global.plaud.ai/products/plaud-note-pro) | Hardware capture for calls and rooms, speaker-labeled transcripts, referenced answers that trace to audio, cross-file search, and exports | Hardware can own the in-person capture habit and produce strong evidence without a desktop app |
| [Limitless](https://help.limitless.ai/en/articles/9124757-pendant-faq) | Ambient wearable capture across meetings and daily life, summaries, lifelog retrieval, and tasks/reminders | Meeting capture may expand into all-day personal memory through a different form factor |

These products are not necessarily direct substitutes for the entire Noted
vision. They still remove basic capture, speaker correction, clips, playback,
daily digest, and ambient memory from the list of novel claims.

### Relationship-first and memory-layer competitors

| Product | Layer it already owns | Opening that remains |
|---|---|---|
| [Mesh, formerly Clay](https://me.sh/) | Automatically organized personal/professional relationships, interaction history, reminders, updates, search, and team network effects | First-party conversation evidence, speaker/timestamp provenance, commitments in the day, and scoped agent access |
| [Dex](https://getdex.com/integrations/mcp-server/) and Dana | Personal CRM histories, reminders, preparation, voice notes, and read/write MCP | Automatic full-conversation capture tied to evidence, then context beyond contacts |
| [Orvo](https://www.getorvo.com/) | Stakeholder profiles, relationship health, open items, preparation, email/calendar sync, and voice-note extraction | Native meeting evidence, broader personal context, local ownership, and external-agent access |
| [Supermemory](https://supermemory.ai/personal/) | Memory across AI tools, connectors, graph/evolving facts, contradiction handling, MCP/API, portability, and self-hosting options | Own the end-user habit: capture a real event, understand it, plan the day, and act with evidence |
| [Obsidian](https://obsidian.md/) | Mature local user-owned vault, Markdown portability, plugins, and agent/headless access | Automatic capture, reliable structuring, source semantics, correction learning, and zero-maintenance resurfacing |
| [Mem0/OpenMemory](https://mem0.ai/openmemory) | Agent memory infrastructure and developer integrations | A complete consumer capture, review, daily-use, and permission experience |

Relationship products do not need full meeting transcription to compete for the
same outcome. They already own contact history, reminders, preparation, and habit.
Memory infrastructure products do not need a consumer UI to commoditize agent
retrieval. Noted must therefore be better at the complete user loop, not merely
add CRM fields or an MCP server.

## Table stakes, not differentiation

Noted may need every item below, but none should be used alone as a defining
claim:

- AI meeting transcription, summaries, decisions, and action items;
- private or bot-free desktop capture;
- speaker labels, timestamps, and optional audio playback;
- a people or company directory built from meetings;
- relationship history, open loops, and going-cold reminders;
- an automatic or self-building personal CRM;
- pre-meeting briefs based on calendar attendees and prior meetings;
- a before/during/after meeting lifecycle;
- unified calendar, daily schedule, tasks, or commitment reminders;
- chat or search across all prior meetings;
- source links or citations in generated answers;
- reviewable AI facts and uncertainty;
- transcript-only retention or audio deletion after transcription;
- export, deletion, privacy, or user ownership stated only in general terms;
- local or self-hosted stated without defining exactly what still requires cloud;
- MCP, API, connectors, or "works with any AI";
- memory that compounds, learns, or gets smarter over time;
- meetings, email, calendar, documents, and projects in one memory;
- meeting-derived drafts, documents, slides, or other finished work; and
- "personal AI relationship manager," "relationship OS," "AI chief of staff,"
  or another new acronym.

The category language itself is not a moat. "PRM" is already used for personal
relationship management and for enterprise partner relationship management.
Noted should describe a concrete user outcome and trust contract rather than
claim category invention.

## Must-win product contract

These are pass/fail product standards, not a backlog ordered by novelty. The
initial quantitative thresholds are deliberately demanding. They may be refined
after a recorded baseline, but they must not be loosened merely because a demo
does not pass.

### 1. Trustworthy capture and speaker identity

**Promise:** Noted records when the user expects it to, stops when asked, does not
repeat avoidable operating-system permission prompts, and learns who is speaking.

**Why it matters:** A context system built on misattributed or missing source data
becomes less valuable with every meeting. Current user-reported failures make
this the first must-win, regardless of broader vision.

**Initial proof gate:**

- at least 99% successful start, continuous capture, and stop/finalize sessions
  across the declared meeting-app support matrix over at least 200 real beta
  sessions, with zero unrecoverable source loss;
- zero Noted-created repeat authorization prompts across 50 consecutive sessions
  after permissions are granted and unchanged;
- at least 90% word-weighted named-speaker attribution across the real-meeting
  evaluation set for enrolled recurring speakers, and at least 95% on the clean
  enrolled two-speaker set; and
- at least a 50% reduction in correction rate by the fifth recorded conversation
  with an enrolled speaker.

**Closest benchmarks:** Groupthink, Granola, Flownote, TwinMind, and Meeting.ai.

**Current state:** partial; the user has reported repeated prompts and incorrect
attribution, so this gate is not met.

### 2. Evidence for every material memory

**Promise:** A decision, commitment, relationship fact, or changed preference can
always be opened at the exact source span. Retained audio is optional evidence,
not a requirement for trust.

**Why it matters:** Source links are already table stakes. Noted must make
provenance complete, stable through reindexing/export/restore, and useful for
correction.

**Initial proof gate:**

- 100% of material extracted decisions, commitments, and durable facts carry a
  resolvable source record and span;
- citation resolution survives reindex, export, restore, and source revision;
- at least 95% of sampled claims are supported by the cited source; and
- zero unsupported high-impact claims are presented as verified in the fixed
  evaluation set.

**Closest benchmarks:** Groupthink, Granola, and Meeting.ai.

**Current state:** partial; meeting timestamps exist in the direction, but the
cross-source citation contract and deterministic lifecycle gates remain in
progress.

### 3. Correctable, time-aware truth

**Promise:** Noted distinguishes source fact, inference, uncertainty, correction,
and superseded belief. A correction changes later behavior without erasing what
the original source said.

**Why it matters:** This converts a pile of accurate historical notes into a
reliable current model. Groupthink already separates suggested and verified facts;
Supermemory already describes evolving and contradiction-aware memory. Noted must
go beyond labels and prove durable temporal behavior.

**Initial proof gate:**

- every correction persists through reindex, export, and restore;
- no superseded fact is presented as current in the temporal evaluation set;
- at least 95% of repeated evaluated outputs honor an approved correction; and
- the UI can show the source, change, actor, time, confidence, and current status
  of an important claim.

**Closest benchmarks:** Groupthink and Supermemory.

**Current state:** foundation/planned; entity and source concepts exist, but the
complete temporal correction contract is not yet a shipped distinction.

### 4. One cross-domain context model and daily loop

**Promise:** Meetings, notes, calendar events, projects, people, decisions,
commitments, journal/self context, and schedule are not separate silos. Context
captured in one place changes preparation or action somewhere else.

**Why it matters:** Genspark already claims broad multi-source memory, while
SavirOS and Meeting.ai already claim before/during/after action. Noted must prove
both coherence and a calm consumer daily loop.

**Initial proof gate:**

- the same stable people, project, decision, commitment, and source identities
  are used across every supported surface;
- a commitment accepted from a meeting appears in Today with owner, date, and
  source and can be completed or corrected without losing provenance;
- at least half of the pilot cohort uses context captured in one surface from a
  different surface each week; and
- users retrieve or act on context older than seven days every week.

**Closest benchmarks:** SavirOS, Genspark, Meeting.ai, Groupthink, and Flownote.

**Current state:** partial; the schedule, calendar, notes, meetings, and knowledge
surfaces exist, but the shared context/action contract is not yet complete.

### 5. Complete user ownership, not privacy copy

**Promise:** The supported Mac product can capture, transcribe, search, correct,
export, and delete without an account or cloud dependency. Cloud is an optional
continuity service, not the authority over the user's memory.

**Why it matters:** Many competitors say private, exportable, or local. The
difference is an auditable end-to-end boundary and a portable canonical record.

**Initial proof gate:**

- a clean Mac can complete the core loop offline after installing required local
  models, without sign-in;
- a deterministic export/restore round trip preserves canonical IDs, sources,
  corrections, citations, and lifecycle state;
- deletion tests remove application-owned source data, derivatives, media, and
  credentials from every declared storage location; and
- the UI states exactly which data crosses the device for every non-local mode.

**Closest benchmarks:** Obsidian, TwinMind's on-device mode, and self-hostable
memory infrastructure.

**Current state:** strongest architectural starting point, but complete export,
restore, disclosure, and deletion proof remains a roadmap gate.

### 6. Least-privilege Context Passes for agents

**Promise:** An agent gets the smallest useful, token-budgeted packet for a stated
purpose—not ambient database, filesystem, workspace, or account access.

**Why it matters:** MCP is already common. Groupthink documents bearer tokens with
the same data reach as the user's account; TwinMind offers read-only access. The
opening is understandable, inspectable, purpose-bound disclosure.

**Initial proof gate:**

- the user can preview the exact Context Pass before first disclosure;
- scope can combine agent/client, project or space, source type, person, time,
  sensitivity, operation, and expiry;
- every disclosure produces a human-readable receipt and can be revoked;
- every material assertion returned to an agent retains a source citation; and
- adversarial permission tests produce zero cross-scope leakage.

**Closest benchmarks:** Groupthink, TwinMind, Granola, Dex, and Supermemory.

**Current state:** specified in agent-context plans; not shipped and therefore not
yet an external differentiator.

### 7. Consumer-grade zero maintenance

**Promise:** Noted becomes useful without CRM gardening, taxonomy design, prompt
engineering, provider configuration, or routine cleanup.

**Why it matters:** Local-first systems often transfer operations work to the
user. Consumer ownership only matters if the experience is simpler than the
cloud incumbents.

**Initial proof gate:**

- a new user reaches the first useful captured-and-resurfaced context in under
  ten minutes, excluding OS permission and local-model download time;
- no category, folder, tag, prompt, or API key is required for the core loop;
- at least 80% of evaluated meetings attach the correct calendar event and known
  people without manual filing; and
- a four-week pilot shows repeated weekly use without staff-assisted maintenance.

**Closest benchmarks:** Granola, Groupthink, Mesh, and Hallway's stated experience.

**Current state:** a product principle, not yet cohort-validated.

## What may compound into a moat

Noted does not currently have a proven moat. The moat hypothesis is a trust and
quality flywheel:

1. reliable local capture creates high-quality original evidence;
2. user corrections improve identities, speakers, entities, temporal facts, and
   retrieval rather than patching one note;
3. daily reuse reveals which context is important and whether it is still true;
4. provenance-specific evaluations improve extraction and retrieval quality;
5. scoped agent use makes the vault more useful without forcing lock-in; and
6. longitudinal accuracy and habit make Noted hard to replace even though export
   remains easy.

The moat is not storage volume, a proprietary acronym, an MCP server, connector
count, or a general-purpose foundation model. Portability is compatible with the
moat: users should stay because the context is unusually accurate and useful,
not because it is trapped.

## Demo and product test

The strategic demo must show one continuous trust loop:

1. Noted prepares a real meeting using an earlier decision and open commitment.
2. It captures the meeting without a visible bot or repeat permission prompt.
3. A summary claim opens the exact speaker, transcript span, and retained audio
   timestamp when audio was kept.
4. The user corrects a speaker or fact, and Noted records the correction.
5. A decision and commitment update the relevant person/project and appear in
   Today.
6. Later, an external agent receives a narrow Context Pass, produces a cited
   result, and leaves a disclosure receipt.

A demo that ends at a transcript and summary makes Noted look interchangeable.
A demo that omits correction, later reuse, or the disclosure boundary does not
prove the proposed difference.

## Strategic rules for future work

- Do not say Noted is the first, only, or creator of this category.
- Distinguish **shipped**, **planned**, **hypothesis**, and **vendor-claimed** in
  every strategy or comparison artifact.
- Treat Groupthink as the closest direct benchmark until evidence changes.
- Recheck Groupthink, Granola, TwinMind, Genspark, Meeting.ai, SavirOS, and
  Flownote before major positioning or roadmap decisions.
- Do not add a feature solely to match a competitor. Tie it to a must-win proof
  gate or leave it parked.
- Do not describe "consumer" as the moat. Translate it into root ownership,
  cross-organization continuity, no-admin setup, and individual control.
- Do not expose raw SQLite rows, local file paths, or an account-wide bearer
  token as the long-term agent interface.
- Keep the meeting wedge narrow until capture, attribution, evidence, and daily
  follow-through pass Phase 1.
- Re-run this study at least quarterly in an active fundraising or launch period,
  and immediately when a direct competitor ships local completeness, temporal
  correction, granular agent scopes, or a cross-domain daily loop.

## Primary source index

### Meeting and relationship systems

- Groupthink: [home](https://groupthink.com/),
  [private desktop capture](https://groupthink.com/docs/private_desktop_recording/),
  [relationships](https://groupthink.com/docs/relationships/),
  [relationship intelligence](https://groupthink.com/releases/relationship-intelligence/),
  [Projects](https://groupthink.com/docs/projects/), and
  [MCP](https://groupthink.com/docs/mcp_server/)
- SavirOS: [home](https://saviros.com/),
  [product](https://saviros.com/product), and
  [calendar intelligence](https://saviros.com/calendar-intelligence)
- Flownote: [home](https://www.flownote.ai/),
  [relationship memory](https://www.flownote.ai/relationship-memory), and
  [contextual chat](https://www.flownote.ai/contextual-ai-chat)
- Hallway: [product and beta status](https://hallwayai.app/)
- Granola: [home](https://www.granola.ai/),
  [People and Companies](https://docs.granola.ai/help-center/people-and-companies),
  [MCP](https://help.granola.ai/article/granola-mcp), and
  [company-context direction](https://www.granola.ai/blog/series-c)
- TwinMind: [home](https://twinmind.com/),
  [MCP](https://twinmind.com/blogs/twinmind-mcp), and
  [privacy and storage modes](https://twinmind.com/legal/privacy-policy)
- Meeting.ai: [product](https://meeting.ai/en/p) and
  [documentation](https://meeting.ai/en/docs)

### Meeting incumbents and capture hardware

- Otter: [product overview](https://help.otter.ai/hc/en-us/articles/360035266494-What-is-Otter),
  [speaker identification](https://help.otter.ai/hc/en-us/articles/21665587209367-Speaker-Identification-Overview),
  and [action items](https://help.otter.ai/hc/en-us/articles/25983095114519-Action-Items-Overview)
- Fireflies: [product overview](https://guide.fireflies.ai/articles/1193528158-what-is-fireflies-ai),
  [desktop capture and personal assistant](https://guide.fireflies.ai/articles/1208704416-getting-started-with-the-fireflies-desktop-app),
  and [global AskFred](https://guide.fireflies.ai/articles/1512776728-global-askfred-get-answers-from-past-meetings-and-web-searches)
- Fathom: [plans and capabilities](https://fathom.video/pricing)
- PLAUD: [Note Pro](https://global.plaud.ai/products/plaud-note-pro)
- Limitless: [Pendant](https://help.limitless.ai/en/articles/9124757-pendant-faq)

### Broad memory and relationship layers

- Genspark: [AI Workspace 6 and SecondBrain](https://www.genspark.ai/blog/genspark-ai-workspace-6)
  and [SecondBrain Note](https://shop.genspark.ai/)
- Mesh: [product](https://me.sh/) and [Nexus](https://me.sh/nexus)
- Dex: [product](https://getdex.com/) and
  [MCP](https://getdex.com/integrations/mcp-server/)
- Orvo: [product](https://www.getorvo.com/) and
  [overview](https://www.getorvo.com/what-is-orvo)
- Supermemory: [personal product](https://supermemory.ai/personal/),
  [platform](https://supermemory.ai/), and
  [memory graph](https://supermemory.ai/memory-graph/)
- Obsidian: [overview](https://obsidian.md/) and
  [headless/agent access](https://obsidian.md/help/headless)
- Mem0/OpenMemory: [Mem0](https://docs.mem0.ai/introduction) and
  [OpenMemory](https://mem0.ai/openmemory)

## Related internal sources

- [`../PRODUCT_STRATEGY.md`](../PRODUCT_STRATEGY.md) — product thesis and business
  model
- [`../ROADMAP.md`](../ROADMAP.md) — phase sequence and proof gates
- [`AGENT_CONTEXT_IMPLEMENTATION_PLAN.md`](AGENT_CONTEXT_IMPLEMENTATION_PLAN.md)
  — canonical context, retrieval, and permission design
- [`agent-context/README.md`](agent-context/README.md) — Phase 0 context contracts
  and evidence gates
- [`decisions/001-meeting-audio-retention.md`](decisions/001-meeting-audio-retention.md)
  — meeting audio policy
- [`decisions/002-context-cloud-boundaries.md`](decisions/002-context-cloud-boundaries.md)
  — local, cloud, and agent boundary
- [`decisions/003-context-record-authority-and-portability.md`](decisions/003-context-record-authority-and-portability.md)
  — proposed canonical record and portability contract
- [`decisions/005-context-pass-and-client-identity.md`](decisions/005-context-pass-and-client-identity.md)
  — proposed Context Pass and local client-identity contract
