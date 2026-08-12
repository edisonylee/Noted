# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

## What noted is

A local-first personal context system (Tauri 2 + React 19). Meeting capture is
the initial product wedge, while notes, calendar, schedule, journal, people, and
future sources build one source-grounded context layer for the user and agents
they explicitly authorize. Local mode runs fully on the Mac; Hosted, BYOK, and
legacy Balanced profiles are optional. The primary (and only fully supported)
platform is **macOS**.

See `PRODUCT_STRATEGY.md` and `ROADMAP.md` for the product direction and phase
gates, `docs/COMPETITIVE_LANDSCAPE.md` for competitor evidence and must-win
standards, `README.md` for the user-facing overview, `PROTOCOL.md` for the
note-extraction contract, and `.impeccable.md` for the active design direction
(note: `DESIGN.md` describes an older "Crisp Data Canvas" theme that has been
superseded by the warm/Geist direction in `.impeccable.md` — the latter is
authoritative, confirmed by `@fontsource-variable/geist` in `package.json`).

Before changing product positioning, describing a capability as differentiated,
or adding parity work because a competitor shipped it, consult the competitive
landscape. Never claim that Noted is the first or only product in this category;
separate shipped capabilities, plans, hypotheses, and vendor claims.

## Commands

The package manager is **bun** (`bun.lock` is committed; `tauri.conf.json` invokes `bun run`). npm also works.

### App target

Use the standard Noted app for routine development, testing, real local data, and local installation: `bun run tauri dev`, `bun run tauri build`, or `bun run app:update` (`/Applications/noted.app`, bundle id `com.noted.app`). `Noted Alpha.app` and `bun run tauri:alpha` are a separate local QA sandbox reserved for explicitly requested release-readiness or distribution validation; they are not the public artifact. The public prerelease uses the `alpha` feature profile but is packaged as `Noted` through `tauri.beta.conf.json`.

Do not build, install, or launch Noted Alpha unless the user specifically asks for release validation, and never leave Alpha, `tauri dev`, and the installed standard app running together. App variants isolate runtime data, not source edits: parallel coding sessions must use separate Git worktrees or clearly divided file ownership. See “Development and release targets” in `README.md`.

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

Adding a command and forgetting #3 means it works on desktop but silently 404s on the phone. (As of writing, `lib.rs` registers ~41 commands; `phone.rs` mirrors them.)

**Invoke arg keys must be camelCase.** Tauri maps JS invoke args to Rust params by name; a snake_case key on the JS side silently arrives as `None`/default in Rust. `phone.rs` reads args from the JSON body with explicit keys (e.g. `sarg(b, "imageBase64")`) that must match the frontend.

### Rust backend (`src-tauri/src/`)

- `lib.rs` — all Tauri commands + app wiring + the `generate_handler!` registry.
- `pipeline.rs` — the capture pipeline. `split_sections()` is a **deterministic** Rust pre-parser (no LLM) that splits a note into segments by `Header:` lines per the grammar in `PROTOCOL.md`; tagged segments route to a fixed category (code decides, not the model — this is the main defense against misclassification) while untagged prose is classified. `snap_category`, `validate_proposal`, `resolve_date`, `extract_date_from_text` run per entry. `is_new_category` is always decided authoritatively from known names, never trusted from the model.
- `provider.rs` — `Mode::Local` vs `Mode::Balanced`. A process-global `OnceLock<RwLock<Config>>` holds the live provider config so `ollama.rs` can consult it without threading app state everywhere. Even in Balanced, **only** extract/OCR go to Gemini (OpenAI-compatible dialect); embeddings and chat stay local.
- `ollama.rs` — local model client; defines the model-name constants and `chat_json` / `embed`.
- `db.rs` — `rusqlite` (bundled SQLite) + `sqlite-vec`. Schema below.
- `entities.rs` — knowledge-graph entity resolution + embedding-based merge suggestions.
- `gcal.rs` — Google Calendar one-way sync (OAuth PKCE).
- `phone.rs` — LAN HTTPS server (`tiny_http` + self-signed `rcgen` cert) and the RPC bridge. A secure context is required so phone mic/camera work; the cert's SAN must match the host IP, so it's regenerated if the IP changes.
- `voice.rs` — `whisper-rs` (whisper.cpp) in-process offline speech-to-text.
- `analytics.rs` — trends/recap aggregation.

### Secrets

Never stored in the repo or DB. The Gemini API key and Google OAuth tokens live in the **macOS Keychain**, accessed via the `security` CLI (deliberately, to avoid a new crate). Provider mode + model ids live in `provider.json` in the app data dir (no secrets in it).

### Database (`db.rs`)

SQLite with WAL + foreign keys. Core tables: `categories`, `notes` (raw capture), `entries` (one note → many entries, one per category/segment, each with `data_json` + `event_date`). `embeddings` is a `vec0` virtual table (`FLOAT[768]`, keyed by note). Knowledge graph: `entities` (typed, `UNIQUE(norm, type)`), `entity_mentions` (— **edges are derived from co-mention at query time; there is no edge table**), and `entity_embeddings` (`vec0`). `pending_captures` is a queue: a phone/quick capture lands here, a background worker runs extraction and writes the real note+entries, then deletes the row (`attempts` caps retries on poison rows).

Schema migrations are **additive only** via `ensure_column()` (ALTER adds nullable; reads COALESCE legacy NULLs). The reserved catch-all `misc` category is **not pre-seeded** — the classifier is told its name in the prompt and it's created on first real use, so an unused `misc` never clutters the catalog.

### Frontend (`src/`)

React 19 + TS + Vite. `api.ts` is the single bridge (handles desktop-vs-web transport, `TokenError`/`OfflineError`, token caching). `App.tsx` holds the desktop views (`today` / `log` / `timeline` / `knowledge`) and the mobile tabs (`today` / `capture` / `timeline` / `ask`). Notable views: `Today.tsx` (deterministically-parsed daily schedule — never hard-fails, no LLM), `Knowledge.tsx` / `PeopleView.tsx` / `Self.tsx` / `EntityPage.tsx` (KG), `Trends.tsx` / `Recaps.tsx`, `FloatingChat.tsx` (local-model Q&A over your data), `Settings.tsx` (provider + gcal), `PhonePanel.tsx` (QR/token pairing). UI libs: `lucide-react` (icons — no emoji), `recharts`, `react-force-graph-2d`.

## Conventions

- **Commits:** commit at meaningful checkpoints (a reminder hook nudges this — it is not auto-commit). Per global instructions, commits and PRs carry **no** Codex/AI attribution.
- `PROTOCOL.md` documents the intended multi-entry extraction contract; consult it before changing `split_sections`, routing, or the proposal schema.
