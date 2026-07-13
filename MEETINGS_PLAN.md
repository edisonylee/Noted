# Meetings: a local-first Granola inside Noted

> **Status (2026-07-13):** Phases 0–3 are implemented (capture engine, live ASR,
> summarization + templates, Coming Up strip, meeting page, detection + prompt
> window, auto-stop, settings; transcript search, per-line copy + tap-to-seek
> audio, Markdown export, custom template editor). Phase 4's diarization half
> shipped early — via sherpa-onnx voiceprints (`meeting/diarize.rs`), not
> FluidAudio — plus speaker rename UI, persistent `speaker_profiles`, and
> local-LLM name suggestions. Also fixed from real-call experience: channel
> timelines are wall-anchored (the tap delivers nothing until audio plays) and
> a cross-channel echo suppressor drops mic copies of remote speech.
> Outstanding: Phase 3's stretch goal (inline black/gray merge + per-bullet
> provenance), and Phase 4's engine work — Parakeet via fluidaudio-rs, plus
> VoiceProcessingIO AEC on the mic path (the proper echo fix).

**Goal:** replicate Granola's core functionality (calendar-aware home, "meeting detected — take notes?" prompt, bot-free dual-stream capture, enhanced notes) with PLAUD's note structuring/formatting — running **100% locally**: whisper.cpp (later Parakeet) for ASR, Ollama for summarization, SQLite for storage. Desktop-only by nature (macOS 14.4+ floor; this machine runs 26.5 on an M5 Pro / 64 GB — comfortably above it).

This plan is the synthesis of a research pass (2026-07-12) over Granola's docs/teardowns and app bundle, PLAUD + Read AI's note formats, the macOS audio-capture ecosystem, ASR benchmarks, and Noted's own code. Research findings are compressed into the relevant sections; each phase ends with acceptance criteria.

---

## 1. What the research established (load-bearing facts)

### Granola's actual mechanics (from official docs + its Info.plist on this machine)

- **Capture:** no bot. Two separate local streams — mic + system audio — which is exactly why its speaker labels are a deterministic binary **"Me" (mic, green, right-aligned) vs "Them" (system audio, grey, left-aligned)**. Its Info.plist declares `NSAudioCaptureUsageDescription` (Core Audio process taps, macOS 14.2+) with `NSScreenCaptureUsageDescription` as the ≤14.1 fallback. Audio is never stored — a top user complaint (no way to verify hallucinated transcripts). **We fix that: keep the audio locally, toggleable.**
- **ASR + LLM are 100% cloud** (Deepgram/AssemblyAI streamed over WSS; OpenAI/Anthropic for enhancement). Our local pipeline is the genuine differentiator, not a compromise.
- **There is no true auto-record.** The flow is: a **custom popup window** (their own, top-right of screen — not a macOS notification) fires **1 minute before any calendar event with ≥2 attendees**. Clicking it does three things at once: opens the call URL + opens the note + starts transcribing. The *only* auto-start: if the meeting's note is already open when the scheduled start arrives. A global auto-record toggle is an explicitly declined feature (accidental-capture risk).
- **Ad-hoc detection = mic-in-use watching.** Granola detects that *some app* opened the microphone, attributes it ("Huddle detected" for Slack, "Call detected" for FaceTime, "Meeting detected" for Zoom/Meet/Teams), and prompts. An ad-hoc call starting within **15 min after a scheduled event** adopts that event's identity. Calendar prompts get a solid colored left bar; mic-detection prompts a dashed one.
- **Auto-STOP is where the automation lives:** call-end detection (app released the mic / scheduled end + transcript inactivity), 15 min of silence, system sleep, or manual stop. Then **enhancement fires automatically**.
- **Home screen:** "Coming up" strip (5 upcoming meetings, paging arrows, gear for per-calendar visibility; declined/OOO events hidden) above a past-notes list. **Each upcoming event is a pre-created note page** — click before start to jot prep notes; being in it arms auto-start.
- **The signature merge:** your typed bullets stay **black**, AI additions render **gray**; editing gray text claims it (turns black). Hovering an AI bullet shows a magnifying glass → provenance back to the transcript. Raw and Enhanced are tabs; switching templates regenerates without losing your text. Default "Auto" template = topic-grouped bite-sized bullets, not a fixed schema.
- **Why it feels great (frequency-ranked):** no bot in the call; "my notes, but better" (enhancement *of your bullets*, not generic summaries); the zero-setup calendar habit loop; looks like a notepad, not a meetings platform.

### PLAUD's formatting (what we're copying)

- **Everything is template-driven; a template = a name + ONE free-text prompt** with the sections embedded ("Define your required sections — such as Decisions, Action items, or Open questions — and what the AI should extract for each"). This maps 1:1 onto our Ollama prompt architecture.
- **De facto default output:** `Summary` (one paragraph) → `Key Takeaways` (bullets) → `Action Points` (list, "Owner — verb phrase by date" as inline text) → Mind Map appended last (exports as a Markdown outline).
- **Multidimensional summaries:** a "+" next to the Summary tab regenerates the same recording with a different template into a **new tab**, preserving the original. Transcript and Summary are sibling tabs on the recording.
- **Transcript view:** speaker-labeled paragraphs with timestamps, tap-a-line-to-seek audio, speakers renameable (applies to one paragraph or all; propagates to future summary generations).
- **Worth stealing from Read AI** (PLAUD conspicuously lacks both): **timestamped Chapters** as a first-class summary section, and a standing **Key Questions** section. Also: per-speaker talk-time % (deterministic — no LLM needed). Skip sentiment/charisma/coaching scores.

### Capture + ASR tech (the part that was expected to be hard, and isn't)

- **Pure Rust, no Swift helper.** Hyprnote, Meetily (Tauri 2 + whisper-rs — the closest analog to Noted), and screenpipe all ship Core Audio process taps directly from Rust via the **`cidre`** crate: global tap **excluding our own PID** → aggregate device → IOProc → ring buffer. Per-process taps miss helper processes (a Teams-only tap records silence) — use global-minus-self.
- **Permission:** one Info.plist string (`NSAudioCaptureUsageDescription`) + a one-time grant under Privacy & Security → Screen & System Audio Recording → "just audio". No monthly re-approval nag (that's the ScreenCaptureKit/Screen-Recording path, which we skip entirely).
- **Known gotchas to build for on day one:** (a) taps can silently start delivering all-zero buffers — build a watchdog that tears down and rebuilds tap + aggregate; (b) default-output device changes (AirPods!) require rebuild; (c) read the tap's real format via `kAudioTapPropertyFormat`, never assume 48 kHz stereo; (d) no echo cancellation — without headphones the mic hears the speakers, duplicating remote speech into "Me".
- **Meeting detection costs zero permissions:** `kAudioDevicePropertyDeviceIsRunningSomewhere` listener = instant "mic went hot" trigger; macOS 14+ **process objects** (`kAudioHardwarePropertyProcessObjectList` → per-process `IsRunningInput` + bundle ID) = *which app*, browsers included (Chrome shows up as `com.google.Chrome` — no tab spying). Hyprnote's shipped policy: 15 s sustained mic use before prompting, per-app cooldown (10 min), an ignore list of dictation/recording apps — **superwhisper is literally on their default ignore list, and it's installed on this machine**, so this is not optional.
- **ASR, two phases.** Phase now: **whisper-rs ≥0.16** (dev moved to Codeberg; crates.io current) brings whisper.cpp v1.7.6's built-in **Silero VAD** + reusable `WhisperState` — VAD-segment → `state.full()` on a reused state is the standard realtime pattern. Model: upgrade from today's `ggml-base.en` to **`large-v3-turbo`** (~10–20× realtime on Metal, ~1.6 GB download). Phase later: **`fluidaudio-rs`** — FluidInference's official Rust crate for Parakeet on the Apple Neural Engine.
- **Parakeet vs Whisper — significant, not negligible, and asymmetric:** vs base/small/medium.en it's a class jump on accuracy (6.05–6.3% avg WER vs 8.1–8.6%, and ~11.2% vs 15.9%+ on the AMI meeting corpus); vs large-v3-turbo the accuracy edge is modest (long-form 10.7% vs 11.0%) but it's **5–15× faster on Apple Silicon** (1-hour meeting in ~30–60 s, ~0.5 GB memory, runs on the ANE leaving CPU/GPU free for Ollama), **doesn't hallucinate on silence** (Whisper's classic meeting failure), and has native word timestamps. Costs: Swift toolchain at build time, macOS 14+/Apple Silicon only, young crate. Verdict: **start whisper, upgrade to Parakeet in Phase 4** — the tap already forces the same macOS floor.

---

## 2. Product spec

### 2.1 Home: "Coming up" strip (Granola's habit loop)

On the `today` view, above/beside the composer:

- A compact strip titled **Coming up** showing the next **5** meetings across all connected accounts' enabled calendars (data already flows from `gcal.rs::events_range` — events carry `title, attendees, attendee_count, meet_link, google_meet, start_min/end_min, calendar, color`). Paging arrows for more; declined events already filtered by the backend.
- Per row: time, title, attendee count, calendar color dot, a **Join** button when `meet_link` exists, and a subtle "will offer to record" affordance (mic icon) for ≥2-attendee events.
- **Clicking a row opens (pre-creates) that meeting's page** — jot prep notes before start; having it open arms auto-start at the scheduled time (Granola's exact semantics).
- A currently-recording meeting shows a live indicator (green dancing bars) in the strip and in the top bar.

### 2.2 Detection → prompt (the state machine)

Two independent signals, merged by a resolver:

1. **Calendar timer** (from our own gcal data, no new permissions): T-60 s before any event with ≥2 human attendees → prompt. Solid accent bar. Click = open `meet_link` + open meeting page + start capture. Per-calendar visibility toggles already exist; solo events silently skipped.
2. **Mic-in-use watcher** (CoreAudio listener + process objects, zero TCC): app opened the mic and held it ≥15 s → prompt titled by attribution — "Meeting detected (Zoom)", "Huddle detected (Slack)", "Call detected (FaceTime)". Dashed accent bar, **Take notes** button. Policy: default ignore list (**superwhisper**, Wispr Flow, VoiceInk, OBS, Loom, QuickTime, ChatGPT/Claude desktop, VS Code/Cursor/Warp), 10-min per-app cooldown, suppressed while already recording.
3. **Adjacency rule:** ad-hoc capture starting ≤15 min after a scheduled event's start adopts that event's title/attendees.

The prompt is a **frameless always-on-top Tauri window** in the top-right (Granola draws its own too — native notifications are too constrained). Ignoring it captures nothing, ever; no retroactive buffering. **No global auto-record** (deliberate, same as Granola).

**Auto-stop:** attributed app releases the mic; or scheduled end passed + no speech for 5 min; or 15 min of continuous silence (VAD); or system sleep. On stop → summarization fires automatically.

### 2.3 During the meeting

The meeting page is a normal Noted view (warm/Geist styling per `.impeccable.md`, lucide icons):

- **Header:** title (from event or attribution), time, attendee chips, Join link, green dancing-bars recording indicator, Stop button.
- **Notes editor:** a plain markdown-ish textarea (matches Noted's composer idiom) for terse trigger bullets. Autosaved to the meeting row (`meeting_set_notes`).
- **Live transcript: hidden by default**, toggled by a waveform button — chat-style bubbles, **Me right/accent, Them left/muted**, timestamps per segment, per-chunk copy. (Granola hides it by default for a reason: the notepad is the product.)

### 2.4 After: the note (PLAUD structure, Granola merge)

**Default template — "Meeting" (fixed section order so every export renders identically):**

```
# {title} — {date}

## Summary            one short paragraph
## Key Takeaways      bullets
## Chapters           [mm:ss] Topic — 1–2 line gist        ← stolen from Read AI
## Action Items       Owner — verb phrase by date          ← PLAUD's inline shape
## Key Questions      open questions raised, unresolved    ← stolen from Read AI
```

- **Tabs on the meeting page:** `Notes` (your raw typed notes, always preserved verbatim) | `Transcript` | `Summary` | **`+`** → regenerate with another template into a new tab, originals preserved (PLAUD's multidimensional summaries).
- **Templates = PLAUD's model exactly:** a row of `{name, prompt}`. Seed with: Meeting (default), 1:1, Interview, Standup, Lecture. Users add custom ones (name + free-text prompt describing sections). No per-section form builder — the prompt *is* the template.
- **Typed-notes merge (Granola's "my notes, but better"):** the summarization prompt receives transcript + typed notes + event metadata and must (a) expand the user's bullets with context/quotes, (b) mark AI-added content so the UI renders it **gray vs the user's black**. v1 keeps this coarse (user notes verbatim in `Notes` tab; summary sections styled as AI); the fine-grained inline black/gray merge with provenance-hover is a Phase 3 stretch goal — it's the hardest LLM task in the plan and needs prompt iteration on qwen2.5:7b.
- **Filing:** the summary is saved through the normal pipeline (`db::save_note`) under a `meetings` category with `data_json = {meeting_id, event_id, attendees, duration_min}` → it enters search, embeddings, recaps, and — the big synergy — **attendees flow into the people knowledge graph** (Granola charges for its People directory; Noted already has `PeopleView`/`EntityPage`).
- **Local-only rule:** summarization always uses `chat_json_local` — same convention as Journal. Meeting content never touches the Balanced/Gemini path.
- **Talk-time %** (Me vs Them minutes) computed deterministically from segments — a one-line stat on the meeting page, no LLM.

### 2.5 Settings

New "Meetings" block in `SettingsModal` backed by `meetings.json` in the app data dir (same pattern as `provider.json`/`gcal.json`, no secrets):

- Detection prompts on/off; per-app toggles + editable ignore list (superwhisper pre-seeded).
- Keep audio recordings: **on by default** (our fix for Granola's top complaint); "delete audio after N days" option.
- Default template picker; live-transcript-visible default.
- Model status: whisper model in use, download button for `large-v3-turbo` (reuses the `download_voice_model` pattern).

---

## 3. Architecture

### 3.1 New backend module: `src-tauri/src/meeting/`

```
meeting/
  mod.rs        // state machine: Idle → Armed(event) → Recording → Summarizing → Done
  capture.rs    // cidre global tap (excl. own PID) + cpal mic → two ring buffers
                //   - reads real tap format, resamples to 16 kHz mono f32
                //   - device-change listener → teardown/rebuild
                //   - zero-buffer watchdog → rebuild
  asr.rs        // VAD-chunked live transcription:
                //   Silero VAD (whisper-rs 0.16) per channel → segments →
                //   one WhisperContext, per-channel WhisperState, serialized on
                //   a single blocking worker → insert segment + emit event
  detect.rs     // mic-in-use listener + process-object attribution + policy
                //   (15 s debounce, ignore list, cooldown) + calendar T-60s timer
  summarize.rs  // template prompt assembly → ollama::chat_json_local →
                //   validated markdown sections → save_note (meetings category)
                //   map-reduce chunking for >2 h transcripts (32k ctx budget)
  store.rs      // meetings/segments/summaries/templates DB access
```

New crates: `cidre` (process taps), `cpal ≥0.17` (native mic — capture must not depend on webview window lifetime), `whisper-rs` bumped to 0.16 (VAD + reusable state). `voice.rs` keeps serving quick voice captures; `asr.rs` reuses its resampling.

Audio retention: 16 kHz mono 16-bit WAV per channel in `app_data/meetings/{id}/` (~115 MB/hr for both streams; fine locally, "delete after N days" in settings).

### 3.2 DB (additive only, per convention)

```sql
CREATE TABLE meetings (
  id INTEGER PRIMARY KEY, title TEXT NOT NULL,
  event_id TEXT, event_json TEXT,          -- gcal snapshot: attendees, meet_link, times
  started_at TEXT, ended_at TEXT,
  status TEXT NOT NULL,                    -- recording|summarizing|done|failed
  raw_notes TEXT,                          -- typed during meeting
  audio_me_path TEXT, audio_them_path TEXT,
  note_id INTEGER,                         -- filed summary note
  created_at TEXT NOT NULL
);
CREATE TABLE meeting_segments (
  id INTEGER PRIMARY KEY, meeting_id INTEGER NOT NULL REFERENCES meetings(id),
  channel TEXT NOT NULL,                   -- 'me'|'them'
  t0_ms INTEGER NOT NULL, t1_ms INTEGER NOT NULL,
  text TEXT NOT NULL,
  speaker TEXT                             -- NULL now; Phase-4 diarization fills it
);
CREATE TABLE meeting_summaries (
  id INTEGER PRIMARY KEY, meeting_id INTEGER NOT NULL REFERENCES meetings(id),
  template TEXT NOT NULL, content_md TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE TABLE meeting_templates (
  id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, prompt TEXT NOT NULL,
  builtin INTEGER NOT NULL DEFAULT 0
);
```

Transcript lives in its own table (not `data_json`) because it's large, append-heavy during recording, and queried by time range. The *summary note* still goes through `notes`/`entries` so all existing surfaces work untouched.

### 3.3 Commands (the three-place invariant)

Every command lands in `lib.rs` (`#[tauri::command]`), the `generate_handler![]` list, **and** `phone.rs`'s match. Capture is desktop-only, so phone dispatch returns a clean `"desktop only"` error for start/stop — never a silent 404 — while reads work from the phone:

| Command | Phone |
|---|---|
| `meeting_start(eventId?, source)` / `meeting_stop(id)` | desktop-only error |
| `meeting_state()` — live status, elapsed, latest segments | read OK |
| `meeting_list()` / `meeting_get(id)` — meta + segments + summaries | read OK |
| `meeting_set_notes(id, text)` | OK |
| `meeting_summarize(id, template)` — new tab + (re)file note | desktop-only error |
| `meeting_templates` CRUD, `meetings_settings_get/set` | read OK / desktop-only |
| `meeting_dismiss_prompt(bundleId)` — feeds cooldown | desktop-only |

Push channel: `app.emit` events (`meeting-detected`, `meeting-segment`, `meeting-stopped`, `meeting-summarized`) — same pattern as the existing `note-filed`. Frontend keys stay camelCase (`eventId`) per the invoke-args rule.

### 3.4 Frontend

- `ComingUp.tsx` — the strip on `today` (feeds off `gcal_events_range` for today/tomorrow; already returns everything needed).
- `MeetingPage.tsx` — header / notes editor / transcript toggle / summary tabs; live updates via events with `meeting_state` polling fallback.
- `RecordPrompt` — separate always-on-top frameless window (`WebviewWindowBuilder`, top-right); `App.tsx` branches on window label to render it.
- `App.tsx` — meeting page as a sub-view of `today` (selected meeting id in state), keeping the 4-view nav intact; tray/menu-bar recording indicator is a stretch.
- Mobile: meeting notes are ordinary notes and `meeting_list/get` work over the phone bridge — read-only on phone in v1.

---

## 4. Phases

**Phase 0 — de-risk spike (~1 day).** Add `cidre`; hidden dev command `meeting_capture_probe(seconds)` records N s of global-tap system audio + cpal mic to WAVs. Add the Info.plist usage string; verify the TCC prompt and grant. Bump whisper-rs to 0.16; download `large-v3-turbo`; transcribe both probe WAVs.
*Accept:* two WAVs of a YouTube video playing while speaking; both transcribe; permission appears under "Screen & System Audio Recording" as audio-only; existing voice capture still passes.

**Phase 1 — manual recorder, end to end (~3–5 days).** Schema + `meeting/` module + commands/events (all three places). Manual start/stop from a bare `MeetingPage`. Dual-stream capture → VAD-chunked live transcription → Me/Them segments streaming into the transcript panel. Typed notes autosave. On stop: default-template summarization → tabs → filed note under `meetings` (entities → people graph). Device-change rebuild + zero-buffer watchdog. Talk-time stat.
*Accept:* a real 30-min Meet call yields a live transcript, a structured summary note visible in search/knowledge, and preserved raw notes; deterministic Rust tests for VAD interleaving, section-order validation of summarizer output (live-model tests may vary, per convention), and template CRUD.

**Phase 2 — the Granola loop (~2–3 days).** `ComingUp` strip; pre-created meeting pages arming auto-start at scheduled time; T-60 s calendar prompt (solid bar; click = join link + page + record); mic-in-use detector with attribution, 15 s debounce, ignore list (superwhisper seeded), cooldown (dashed bar, "Take notes"); 15-min adjacency; all auto-stop heuristics; auto-summarize on stop; Settings block + `meetings.json`.
*Accept:* join a Zoom call without touching Noted → correctly-attributed prompt within ~20 s; one click starts recording; leaving the call auto-stops and files the note. Dictating with superwhisper never prompts. Calendar meeting prompts at T-60 s with a working Join.

**Phase 3 — formatting polish (~2–4 days).** Full template set + custom template editor; `+` multidimensional tabs; Chapters/Key Questions prompt tuning on qwen2.5:7b; transcript search + per-chunk ops; export note as Markdown. **Stretch:** inline black/gray merge with per-bullet transcript provenance (Granola's magnifying glass) — segment-id citations from the model, degrade gracefully if the 7B can't cite reliably.

**Phase 4 — Parakeet upgrade (~2–3 days, decided: worth it).** `fluidaudio-rs` behind an `AsrEngine` trait (whisper stays the fallback; Swift toolchain becomes a build dep). Parakeet v3 on the ANE: ~10× faster live chunks, no silence hallucinations, native word timestamps (feeds provenance + chapter anchors). FluidAudio's diarization on the Them stream fills `speaker` (PLAUD-style rename UI, propagating to future regenerations).

Later / if ever published: notarization + entitlements review, macOS 14.4+ floor stated, SCK fallback decision for old OSes, onboarding for the audio permission, consent affordances (Granola's watermark/auto-notice), audio encryption at rest.

## 5. Open decisions (defaults chosen, flag to revisit)

1. **Live model = `large-v3-turbo`** (~1.6 GB download). If live latency annoys on long calls, drop live to `small.en` and re-transcribe with turbo at stop — Phase 4 makes this moot.
2. **Audio retention default = on.** It's the fix for Granola's biggest trust complaint; disk is cheap locally.
3. **Echo without headphones** (mic hears speakers → remote speech duplicated into "Me"): v1 accepts it + a near-duplicate-text suppressor across channels; proper fix is VoiceProcessingIO AEC on the mic path (screenpipe precedent) in Phase 4.
4. **Meeting summaries file under a new `meetings` category** (created on first use, like `misc`) rather than a new top-level nav view — meetings stay reachable from Coming up/past list on `today`.
