# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What noted is

A local-first personal-knowledge app (Tauri 2 + React 19). You capture a note by typing, speaking, or photographing it; a local model (Ollama) categorizes it, extracts structured data, and files it. Over time it builds a searchable knowledge base, a daily schedule, recaps/trends, and a people/self knowledge graph. Default mode runs **100% locally**; an optional "Balanced" mode offloads only OCR/extract to Google Gemini. The primary (and only fully supported) platform is **macOS**.

See `README.md` for the user-facing overview, `PROTOCOL.md` for the note-extraction contract, and `.impeccable.md` for the active design direction (note: `DESIGN.md` describes an older "Crisp Data Canvas" theme that has been superseded by the warm/Geist direction in `.impeccable.md` — the latter is authoritative, confirmed by `@fontsource-variable/geist` in `package.json`).

## Commands

The package manager is **bun** (`bun.lock` is committed; `tauri.conf.json` invokes `bun run`). npm also works.

```sh
bun install
bun run tauri dev      # run the desktop app (spawns Vite on :1420 + the Rust shell)
bun run tauri build    # distributable build

bun run dev            # frontend only — the app does nothing useful without the Rust backend
bun run build          # tsc typecheck + vite build (frontend)
```

Rust tests (the meaningful test suite lives in `src-tauri/`):

```sh
cd src-tauri && cargo test                 # all tests
cd src-tauri && cargo test --test schedule_test          # one test file
cd src-tauri && cargo test split_sections                # one test by name
```

Section-splitter, parser, schedule, and date tests are **deterministic and need no model**. Some entity-extraction / vision / chat tests exercise a live local Ollama and are expected to vary or fail on smaller 7B models **by design** — that variance is not a bug (e.g. `surfaces_entities`).

### Prerequisites for running

Ollama must be running locally with these models pulled:
```sh
ollama pull qwen2.5:7b-instruct   # text  (ollama.rs TEXT_MODEL)
ollama pull qwen2.5vl:7b          # vision (VISION_MODEL)
ollama pull nomic-embed-text      # embeddings, 768-dim (EMBED_MODEL)
```

## Architecture

### Two runtimes, one frontend, one command surface

The same React app runs in **two** places, and `src/api.ts` abstracts the difference:
- **Desktop:** Tauri IPC (`invoke`).
- **Phone:** the desktop runs a token-gated LAN HTTPS server (`src-tauri/src/phone.rs`) that serves the built frontend and bridges `POST /api/<cmd>` to the matching Tauri command.

**The critical invariant:** every backend command exists in **three** places that must stay in sync:
1. `#[tauri::command]` fn in `src-tauri/src/lib.rs`
2. registered in the `generate_handler![...]` list at the bottom of `lib.rs`
3. dispatched in `handle_api`'s `match cmd { ... }` in `src-tauri/src/phone.rs`

Adding a command and forgetting #3 means it works on desktop but silently 404s on the phone. (As of writing, `lib.rs` registers ~50 commands; `phone.rs` mirrors them.)

**Invoke arg keys must be camelCase.** Tauri maps JS invoke args to Rust params by name; a snake_case key on the JS side silently arrives as `None`/default in Rust. `phone.rs` reads args from the JSON body with explicit keys (e.g. `sarg(b, "imageBase64")`) that must match the frontend.

### Rust backend (`src-tauri/src/`)

- `lib.rs` — all Tauri commands + app wiring + the `generate_handler!` registry.
- `pipeline.rs` — the capture pipeline. `split_sections()` is a **deterministic** Rust pre-parser (no LLM) that splits a note into segments by `Header:` lines per the grammar in `PROTOCOL.md`; tagged segments route to a fixed category (code decides, not the model — this is the main defense against misclassification) while untagged prose is classified. `snap_category`, `validate_proposal`, `resolve_date`, `extract_date_from_text` run per entry. `is_new_category` is always decided authoritatively from known names, never trusted from the model.
- `provider.rs` — `Mode::Local` vs `Mode::Balanced`. A process-global `OnceLock<RwLock<Config>>` holds the live provider config so `ollama.rs` can consult it without threading app state everywhere. Even in Balanced, **only** extract/OCR go to Gemini (OpenAI-compatible dialect); embeddings and chat stay local.
- `ollama.rs` — local model client; defines the model-name constants and `chat_json` / `embed`.
- `db.rs` — `rusqlite` (bundled SQLite) + `sqlite-vec`. Schema below.
- `entities.rs` — knowledge-graph entity resolution + embedding-based merge suggestions.
- `gcal.rs` — Google Calendar, multi-account (OAuth PKCE, one refresh token per account email in the Keychain). Push: one-way schedule sync into a "noted" calendar in the **first** account. Pull: the Calendar view's range feed across every connected account's enabled calendars, plus event create/edit/move/delete.
- `phone.rs` — LAN HTTPS server (`tiny_http` + self-signed `rcgen` cert) and the RPC bridge. A secure context is required so phone mic/camera work; the cert's SAN must match the host IP, so it's regenerated if the IP changes.
- `voice.rs` — `whisper-rs` (whisper.cpp) in-process offline speech-to-text.
- `meeting/` — the meeting recorder (local Granola; design + research in `MEETINGS_PLAN.md`). `capture.rs`: system audio via a Core Audio process tap (`cidre`, macOS 14.4+ runtime-gated, needs the one-time System Audio Recording permission — `NSAudioCaptureUsageDescription` in `Info.plist`) + mic via `cpal`, kept as **two streams** so speaker labels are a deterministic mic="me"/system="them". `asr.rs`: deterministic energy-gate VAD chunker (unit-tested; silence never reaches whisper) → whisper segments → `meeting_segments` + live events; channel timelines are **wall-anchored** (`advance_gap` — the tap delivers nothing until an app plays audio, so counting frames alone would skew the interleave by minutes), and a cross-channel **echo suppressor** (`is_echo` token containment) drops mic segments that are just the speakers replaying remote speech (no-headphones case; late matches retro-delete + emit `meeting-segment-removed`). `diarize.rs`: per-speaker labels on the "them" stream — a voice embedding per segment (sherpa-onnx WeSpeaker CAM++, `speaker-embed.onnx` downloaded from Settings, statically linked `sherpa-rs`) collected live, then full-context agglomerative clustering **at stop** writes labels into `meeting_segments.speaker` before `meeting-stopped` fires. Naming: a cluster matching a stored voiceprint (`speaker_profiles`, running-mean embedding) gets the person's real name automatically; the rest become "Speaker N"; a lone unknown voice stays NULL ("Them"). During recording, `provisional_labels` reclusters every few embeddings and streams labels live (`meeting-speakers-updated`); the stop pass (`finalize_speakers`, shared policy) resets and rewrites them authoritatively. Crash recovery: `rediarize_from_wav` re-embeds from the wall-anchored them.wav (tolerating an unfinalized RIFF header) — run automatically by `reconcile()` and on demand via `meeting_rediarize` / the "Detect speakers" button. Renaming a speaker (chip UI on the transcript, `meeting_rename_speaker`) relabels the meeting AND seeds/updates the voiceprint via the centroid kept in `meeting_speakers` — profiles are written **only** on explicit rename/confirm, never from a match, so a bad label can't self-reinforce. After each summarize, `suggest_speaker_names` (local model only) mines the transcript + calendar attendees for who "Speaker N" is and stores a *suggestion* the user confirms in the UI. The clustering/naming policy is pure/unit-tested; `diarize_real_meeting` (`--ignored`) is the tuning harness against a real recording. `detect.rs`: zero-permission meeting detection (CoreAudio process objects for mic-in-use + attribution, calendar T-60s prompts, 15s debounce / ignore-list / 10-min cooldown, auto-stop) driving the `record-prompt` always-on-top window. `summarize.rs`: template-driven local summarization (**always** `chat_json_local_ctx`, never Balanced/cloud) — schema-constrained JSON → deterministic markdown; the first summary also files a real note under `meetings` so search/embeddings/KG see it. `video.rs`: window video — ScreenCaptureKit records the call app's WINDOW (desktop-independent filter: covered/other-Space doesn't interrupt) to `meetings/<id>/window.mp4` via `SCRecordingOutput` (macOS 15+ runtime-gated); best-effort, retention sweep by `video_keep_days`. Live Assist: `meeting_assist` answers questions over the rolling transcript (local model only) — design + phases in `LIVE_ASSIST_PLAN.md`. Config in `meetings.json` (`MeetingsCfg`, provider.json pattern). Meeting transcription prefers `ggml-large-v3-turbo.bin`, falls back to `ggml-base.en.bin`.
- `analytics.rs` — trends/recap aggregation.

### Secrets

Never stored in the repo or DB. The Gemini API key and Google OAuth tokens live in the **macOS Keychain**, accessed via the `security` CLI (deliberately, to avoid a new crate). Provider mode + model ids live in `provider.json` in the app data dir (no secrets in it).

### Database (`db.rs`)

SQLite with WAL + foreign keys. Core tables: `categories`, `notes` (raw capture), `entries` (one note → many entries, one per category/segment, each with `data_json` + `event_date`). `embeddings` is a `vec0` virtual table (`FLOAT[768]`, keyed by note). Knowledge graph: `entities` (typed, `UNIQUE(norm, type)`), `entity_mentions` (— **edges are derived from co-mention at query time; there is no edge table**), and `entity_embeddings` (`vec0`). `pending_captures` is a queue: a phone/quick capture lands here, a background worker runs extraction and writes the real note+entries, then deletes the row (`attempts` caps retries on poison rows).

Schema migrations are **additive only** via `ensure_column()` (ALTER adds nullable; reads COALESCE legacy NULLs). The reserved catch-all `misc` category is **not pre-seeded** — the classifier is told its name in the prompt and it's created on first real use, so an unused `misc` never clutters the catalog.

### Frontend (`src/`)

React 19 + TS + Vite. `api.ts` is the single bridge (handles desktop-vs-web transport, `TokenError`/`OfflineError`, token caching). `App.tsx` holds the desktop views (`today` / `calendar` / `journal` / `knowledge` — "today" is the merged home: capture composer + the day's schedule, shown in the nav as "Daily Schedule") and the mobile tabs (`today` / `capture` / `ask`). Notable views: `Today.tsx` (deterministically-parsed daily schedule — never hard-fails, no LLM; its empty state is deliberately quiet because the composer above it is the way to make a schedule), `Calendar.tsx` (day/3-day/week grid over every connected Google account; the `.app` gets a `calmode` class so it can go full-width/full-height), `Journal.tsx` (reflection chat agent; each reflection is saved as a `journal` note whose extracted entities feed the personal knowledge graph — its model call is `chat_json_local`, never the Balanced cloud path), `Knowledge.tsx` / `PeopleView.tsx` / `Self.tsx` / `EntityPage.tsx` (KG), `Trends.tsx` / `Recaps.tsx`, `FloatingChat.tsx` (local-model Q&A over your data), `Settings.tsx` (provider + Google accounts), `PhonePanel.tsx` (QR/token pairing). UI libs: `lucide-react` (icons — no emoji), `recharts`, `react-force-graph-2d`.

## Conventions

- **Commits:** commit at meaningful checkpoints (a reminder hook nudges this — it is not auto-commit). Per global instructions, commits and PRs carry **no** Claude/AI attribution.
- `PROTOCOL.md` documents the intended multi-entry extraction contract; consult it before changing `split_sections`, routing, or the proposal schema.
