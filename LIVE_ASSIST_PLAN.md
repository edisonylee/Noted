# Live Assist: an always-on, local-first meeting copilot

> **Status (2026-07-17):** design document + Phase A0 groundwork. A0 ships with
> this commit: `meeting_assist` — ask questions against the live rolling
> transcript from the meeting page, local model only. The prerequisites this
> plan leans on all exist as of today: two-stream capture with wall-anchored
> timelines (`meeting/capture.rs`, `asr.rs`), **live provisional speaker
> labels** streamed during the call (`meeting-speakers-updated`), the
> meeting-fed knowledge graph, sqlite-vec embeddings, and the confirm-first
> chat agent. Later phases are design, not commitment — revisit ordering when
> A0/A1 have real-call mileage.

**Goal:** a Cluely-class real-time assistant — live insights, "help me answer
this", instant recall over past meetings — with none of the cloud dependence
and none of the stealth posture. Everything runs on the Mac that is already
recording the meeting; the assistant is a *participant tool*, not a cheating
device.

This plan synthesizes a research pass (2026-07-17) over the open-source
Cluely-alternative field: **Glass** (pickle-com/glass, the reference
implementation), **cheating-daddy** (sohzm — best concrete numbers, read at
source), **Pluely** (Tauri-based, closest architecture), **Natively**
(feature-richest, local RAG), plus the local-first notetakers **Hyprnote** and
**Meetily**. Repo links at the bottom.

---

## 1. What the research established (load-bearing facts)

### The converged architecture (all five projects independently arrived here)

1. **Two-stream capture** (mic = me, system audio = them). *We already do this
   better than any of them*: every Electron project bundles `SystemAudioDump`
   (ScreenCaptureKit), which drags in the **Screen Recording** permission and
   its macOS 15 re-approval nags; our Core Audio process tap needs only the
   one-time **System Audio Recording** grant. This is a real privacy/UX edge —
   say it in the UI ("no bot joins; no screen access needed to listen").
2. **Streaming STT feeding a rolling context.** cheating-daddy streams 100 ms
   PCM frames to Gemini Live; the local projects VAD-chunk exactly like our
   energy-gate chunker. Nothing to change — `meeting-segment` events already
   stream our transcript live.
3. **Insight generation on a THROTTLE, never per-segment.** Glass regenerates
   its live summary **every 5 conversation turns** (turn-count, not wall
   clock); Meetily makes summarization manual; cheating-daddy lets the model
   itself decide when to speak (`proactiveAudio`). On a local 7B this is the
   difference between usable and molasses — batch by turns.
4. **Decouple transcription from answer generation.** cheating-daddy
   transcribes on one model and answers with another, faster one "for faster
   responses". Ours are already decoupled (whisper vs qwen); keep the answer
   path on the smallest model that holds up.
5. **Optional periodic screen-OCR context** (~5 s cadence, JPEG q0.5–0.7 in
   cheating-daddy). The only capture capability we lack — and now
   half-possess: `meeting/video.rs` already records the call window via
   ScreenCaptureKit; a still-frame → local vision model OCR is a small step.

### The interaction model worth copying (and the one to reject)

- **Glass's three-verb frame: Listen / Ask / Summarize.** The cleanest mental
  model in the space. Ours maps: Listen = the live transcript (shipped), Ask =
  `meeting_assist` (A0), Summarize = live insight cards (A1) + the existing
  post-meeting templates.
- **Pluely's three-way auto-suggest trigger** is the best-articulated answer
  to "when should an assistant speak": fire **(a)** when someone asks a
  question, **(b)** after every pause, or **(c)** only on a manual "Suggest"
  tap — as a user setting. Add cheating-daddy's model-decides "proactive" as a
  possible fourth later. **Default to manual**; auto modes are opt-in.
- **UI vocabulary:** answer cards + suggested-reply / follow-up chips
  (Natively, Pluely); a small always-on-top panel rather than a full window
  (all of them); `Cmd+K`-style summon; Natively's "Eager Code Expansion"
  (pre-size the answer card before render) is a nice perceived-latency trick.
- **Overlay tech for Tauri:** Pluely uses **`tauri-nspanel`** (non-activating
  NSPanel — floats without stealing focus). We already own a simpler variant:
  the `record-prompt` always-on-top window (`detect.rs`). Extend that family
  before adding a dependency.
- **Screen-share invisibility** (`setContentProtection(true)` /
  `set_content_protected` in Tauri): legitimate for keeping *your* notes out
  of *your* screen share — offer as a toggle on the assist panel. The rest of
  the stealth kit — randomized process names, OS masquerading as Terminal,
  "stealth levels", interview-cheating framing (cheating-daddy, Natively) —
  is explicitly rejected. Noted is permission-honest and bot-free; that's the
  brand.
- **Emergency dismiss** (cheating-daddy's panic hotkey, minus the paranoia):
  one keystroke hides the panel and stops generation. Keep.

### Recall over history (the highest-leverage lift)

- **Natively's "Smart Scope":** classify whether a question targets the
  *current meeting* or *meeting history* BEFORE retrieving; then RAG over
  SQLite + sqlite-vec (their stack is literally ours). Their chunking:
  sliding window with **50-token overlap**.
- We already have the pieces: meeting summaries are filed as real notes
  (embedded, KG-extracted), `graph_context()` pulls entity digests into chat,
  and the day-scope work gave us deterministic question-scoping precedent
  (`pipeline::day_scope`). Smart Scope is a small deterministic classifier in
  the same spirit: "this meeting" words (just said / earlier in this call /
  they mentioned) vs history words (last time / previous meeting with X).

### Local-model viability

- Hyprnote ships a **Qwen3-1.7B fine-tune** as its entire summarizer —
  evidence that live insight cards do NOT need the 7B. If qwen2.5:7b latency
  ever hurts the live loop, a ~2B side-model for A1 cards is a proven
  fallback (keep the 7B for final summaries).
- Natively claims <500 ms end-to-end with local ONNX STT + native capture.
  Ours: whisper on Metal is the latency floor for transcript lines; the
  assist answer itself is token-generation-bound. Budget: first token < 2 s,
  full card < 8 s on the M5 Pro.

---

## 2. Product spec

### 2.1 The Assist panel (the "always-on" part)

A small always-on-top card, same family as the record-prompt window: shows
during recording (auto-appears when a meeting starts, dismissible), summoned
any time with a global hotkey. Three zones, Glass's triad:

- **Listen** (ambient): last 2–3 transcript lines with live speaker names —
  proof it's hearing correctly, and the anchor for "what did they just say?"
- **Insights** (A1): a card refreshed every N=6 remote turns — "Where we are"
  (one line), open questions directed at *me*, action items so far. Generated
  locally, throttled, never blocking the transcript.
- **Ask** (A0): one input. Answers ground in (in priority order) the rolling
  transcript → my raw notes → Smart-Scope'd history/KG. Answer card with
  copyable text; follow-up chips later (A3).

Toggles on the panel: content-protection (hide from screen share), auto-mode
(off / after-pause / question-detected), Esc hides.

### 2.2 The `meeting_assist` command (A0 — shipped with this plan)

`meeting_assist(id, question)` → `{ answer }`. Works during recording AND on
finished meetings (the meeting page's transcript tab gets an ask box). Prompt
= last ~10 min / ~8k chars of transcript (speaker-labeled, wall-clock
timestamps) + the user's typed notes + the question. `chat_json_local` path
only — like Journal and summaries, assist NEVER routes to the Balanced cloud
provider (the transcript is the most sensitive text in the app).

### 2.3 Personas / templates (A4)

Natively's `SKILL.md`-file personas and cheating-daddy's struct-composed
prompt presets both reduce to: a persona = one editable free-text prompt.
That is *exactly* our existing meeting-template architecture
(`store.rs::BUILTIN_TEMPLATES`) — extend it with assist personas (Meeting /
1:1 / Standup / Interviewing-as-interviewer / Sales) rather than inventing a
new mechanism.

---

## 3. Architecture notes

- **Feed:** the assist worker consumes the same `meeting-segment` /
  `meeting-speakers-updated` events the UI does — no second transcript path.
  Backend-side it reads `meeting_segments` directly; segments are already
  wall-anchored and echo-suppressed.
- **Throttle state** lives with the meeting worker (a turn counter, like
  `LIVE_RELABEL_EVERY`); insight generation is a `tauri::async_runtime`
  task so whisper never waits on the LLM.
- **Smart Scope** = deterministic classifier first (current-meeting vs
  history phrasing), embeddings only as tiebreak — "code decides, not the
  model", same as `split_sections` and `day_scope`.
- **Screen OCR context (A4):** grab a frame from the already-running window
  video stream (an `SCScreenshotManager` call scoped to the same window
  filter — no new permission), OCR via the existing vision path, append as
  `[on screen]` context. Never store the frames.
- **Command surface:** every new command lands in all three places
  (`lib.rs` fn, `generate_handler!`, `phone.rs::handle_api`) — assist answers
  should work from the phone as a remote control, like `meeting_summarize`.

## 4. Phases

- **A0 (now): meeting-scoped ask.** `meeting_assist` + ask box on the meeting
  page (live and finished). Acceptance: during a recording, "what did
  Brian just say about X?" answers in <8 s from the transcript alone.
- **A1: live insight cards.** Turn-counter throttle (N=6 remote turns),
  insight JSON schema (where-we-are / open-questions / actions), card in the
  meeting page; measure 7B latency, drop to a ~2B side-model if needed.
  Acceptance: cards refresh through a 30-min call without ever delaying a
  transcript line.
- **A2: the panel.** Assist card joins the record-prompt window family
  (always-on-top, non-activating, global hotkey, Esc hides, optional
  content-protection). Listen + Insights + Ask zones.
- **A3: triggers + chips.** Pluely's trigger setting (manual default /
  after-pause / question-detected — question detection is deterministic: a
  them-segment ending in "?" or starting with an interrogative, directed at
  short silence). Suggested-reply chips on question detection.
- **A4: context extras.** Persona templates via the template system;
  screen-frame OCR context; Smart-Scope'd history RAG with 50-token-overlap
  chunking of past transcripts (today only summaries are embedded — decide
  then whether raw-transcript embedding is worth the index size).

## 5. Explicit non-goals

- No process masquerading, no anti-detection, no "invisible to interviewers"
  marketing — rejected on ethics and because it's Noted's differentiator NOT
  to be that.
- No cloud STT/LLM in the assist loop, even in Balanced mode.
- No bot joining calls, ever.

---

Research sources: [glass](https://github.com/pickle-com/glass) ·
[cheating-daddy](https://github.com/sohzm/cheating-daddy) ·
[pluely](https://github.com/iamsrikanthnani/pluely) ·
[natively](https://github.com/Natively-AI-assistant/natively-cluely-ai-assistant) ·
[hyprnote](https://github.com/fastrepl/hyprnote) ·
[meetily](https://github.com/Zackriya-Solutions/meetily)
