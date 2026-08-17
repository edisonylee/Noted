# noted

A local-first personal context system that turns messy capture into structured,
source-backed memory — privately, on your own machine.

You jot, speak, photograph a note, or capture a meeting; Noted turns the source
into context you can search, verify, and use later. Over time it builds a personal
knowledge base, daily schedule, history of decisions and commitments, recaps and
trends, and a lightweight knowledge graph of the people and things in your life.

Meetings are the first high-value wedge, not the boundary of the product. See the
accepted [`PRODUCT_STRATEGY.md`](PRODUCT_STRATEGY.md) and outcome-gated
[`ROADMAP.md`](ROADMAP.md).

Noted supports three complete inference profiles: **Noted Hosted** requires no model downloads, **Use my API keys** routes each capability to providers you choose, and **Local** keeps inference on your Mac through [Ollama](https://ollama.com). The legacy "Balanced" mode remains available for cloud-assisted extraction.

## Features

- **Capture anything** — typed text, voice (offline speech-to-text via whisper.cpp), or photos (vision OCR, including Apple HEIC/HEIF).
- **Automatic structuring** — a note is split into sections; `Header:`-tagged sections route deterministically while untagged prose is classified by the model. One note can fill several categories at once.
- **Spaces** — notes are organized into personal and work spaces, so the two halves of your life stay separate.
- **Semantic search** — local `nomic-embed-text` embeddings in a `sqlite-vec` vector store.
- **Meetings** — a local meeting recorder: system audio + mic captured as separate streams, live offline transcription, per-speaker diarization with voiceprints that learn who's who, optional window video of the call, template-driven summaries (always local), and Live Assist Q&A over the rolling transcript. Meetings are detected automatically from mic use and your calendar.
- **Today** — a daily schedule parsed deterministically (no LLM, never hard-fails), with connected time ranges, inline editing, and optional one-way push to Google Calendar.
- **Calendar** — a day / 3-day / week view aggregating every connected Google account, with event create/edit/move/delete.
- **Journal** — a reflection chat whose entries are saved as notes and feed the personal knowledge graph.
- **Knowledge graph** — entity resolution for people and things, with embedding-based merge suggestions you confirm. The "Knowledge" view surfaces People and a Self graph.
- **Recaps & trends** — auto-generated day/week recaps and per-category trends.
- **Chat** — ask questions over your own knowledge base with a local model.
- **Themes** — search 50 bundled visual systems, import a `DESIGN.md`, or ask the local assistant for a style. Imports are validated data-only packs; no paid MCP or cloud model is required.
- **iPhone companion (in development)** — a signed native build with isolated
  local-only Notes now runs on iPhone; cross-device sync has not shipped. The
  target is an offline-capable app with encrypted Mac sync while the Mac retains
  model processing. The old LAN/PWA remote view is disabled and is not a shipped
  phone product. See the [verified preflight](docs/IPHONE_FEASIBILITY_PREFLIGHT.md),
  [implementation plan](docs/MOBILE_COMPANION_IMPLEMENTATION_PLAN.md), and
  [capability map](docs/mobile/capability-ledger.yaml).

## Tech stack

- **Desktop shell:** [Tauri 2](https://tauri.app) (Rust)
- **Frontend:** React 19 + TypeScript + Vite, `lucide-react`, `recharts`, `react-force-graph-2d`
- **Storage / search:** `rusqlite` (bundled SQLite) + `sqlite-vec` for vector search
- **Local models (via Ollama):** `qwen2.5:7b-instruct` (text), `qwen2.5vl:7b` (vision), `nomic-embed-text` (embeddings, 768-dim)
- **Speech-to-text:** `whisper-rs` (whisper.cpp), in-process and offline
- **Dormant legacy phone bridge:** `tiny_http` over self-signed TLS (`rcgen`);
  disabled in every release profile while the native iPhone companion is built
- **Optional cloud (Balanced mode):** Google Gemini (`gemini-2.5-flash` / `gemini-2.5-flash-lite`)
- **Bring your own keys:** OpenAI, Gemini, Anthropic, Groq speech, Noted Hosted, and HTTPS/loopback OpenAI-compatible endpoints. Intelligence, vision, 768-dimensional embeddings, and transcription are configured independently.

Secrets (the Gemini API key, Google Calendar OAuth tokens) are stored in the macOS Keychain, never in the repo or the database.

## Prerequisites

- **macOS** (the primary supported platform — uses the Keychain and builds whisper.cpp locally; minimum macOS 10.15, with meeting recording gated to newer releases at runtime)
- **[Bun](https://bun.sh)** (the committed lockfile is `bun.lock`; npm also works)
- **Rust** (stable) + the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
- **[Ollama](https://ollama.com)** running locally, with the models pulled:

  ```sh
  ollama pull qwen2.5:7b-instruct
  ollama pull qwen2.5vl:7b
  ollama pull nomic-embed-text
  ```

## Getting started

```sh
bun install
bun run tauri dev      # run the desktop app in development
```

To produce a distributable build:

```sh
bun run tauri build
```

Frontend-only scripts (`vite`) are also available via `bun run dev` / `bun run build`, but the app needs the Tauri backend to do anything useful.

### Development and release targets

“Alpha” has two related but distinct meanings in this repository:

| Target | Identity and data | Intended use |
| --- | --- | --- |
| Standard Noted | `/Applications/noted.app`, `com.noted.app`, normal local database | Everyday development, testing, and real notes. Rebuild and install it with `bun run app:update`. |
| Local Noted Alpha | `Noted Alpha.app`, `com.noted.desktop.alpha`, separate sandbox database | Short-lived release-readiness checks only. Build it with `bun run tauri:alpha` when that validation is explicitly needed. |
| Public prerelease | `Noted`, `com.noted.app`, built by CI with the `alpha` feature profile and `tauri.beta.conf.json` | The consumer-facing prerelease artifact. It is not the local `Noted Alpha.app` sandbox. |

Use standard Noted as the canonical local app. Do not run it alongside `tauri dev` or Noted Alpha: the processes can compete for app-level behavior and duplicate meeting prompts. App variants do not prevent Git conflicts because every build still reads source from the checkout. Parallel coding sessions should use separate Git worktrees or explicitly divided file ownership, then integrate through deliberate commits.

## Optional setup

- **Balanced mode (Gemini):** open Settings, switch to *Balanced*, and paste a Gemini API key. Only OCR/extract calls go to Gemini; chat and embeddings stay local. The connection badge confirms the key is live.
- **Use my API keys:** open Settings → Models, select each provider and model, review the routing summary, then save. Credentials go to macOS Keychain. Anthropic must be paired with another embedding and transcription provider. Changing the embedding provider asks before rebuilding semantic search from your preserved notes.
- **Google Calendar:** in Settings, connect your Google account (OAuth with PKCE). Today can then push the day's schedule one-way into a dedicated "noted" calendar.
- **iPhone companion:** the local-only Notes prototype is installable on a
  development iPhone, but pairing and sync are not shipped. The legacy QR/LAN
  remote view remains disabled; implementation follows the
  [mobile companion plan](docs/MOBILE_COMPANION_IMPLEMENTATION_PLAN.md).
- **Themes:** open Settings → Themes to search 50 bundled presets. You can also paste or upload a `DESIGN.md`; the local text model creates a safe preview before you apply it. See [`THEMES.md`](THEMES.md) for the pack contract.

## Tests

```sh
cd src-tauri && cargo test
```

The section-splitter and parser tests are deterministic and need no model. Some entity-extraction tests exercise a live local model and are expected to vary on smaller 7B models by design.

## Project layout

```
src/            React + TypeScript frontend (views, capture, API bridge)
src-tauri/      Rust backend
  src/
    lib.rs        Tauri commands + app wiring
    pipeline.rs   categorize / extract pipeline (section routing)
    entities.rs   knowledge-graph entity resolution
    db.rs         SQLite schema + sqlite-vec storage
    ollama.rs     local model client (text / vision / embeddings)
    provider.rs   provider selection (local vs Gemini Balanced)
    themes.rs     theme validation, persistence, and local DESIGN.md compilation
    voice.rs      whisper.cpp speech-to-text
    meeting/      meeting recorder (capture, transcription, diarization, video, summaries)
    analytics.rs  recaps + trends aggregation
    gcal.rs       Google Calendar sync (multi-account)
    phone.rs      dormant legacy LAN HTTPS server + RPC bridge
```

## Privacy

noted is local-first by design: in Local mode, your notes, photos, retained audio,
embeddings, and search stay on your machine. Cloud calls happen only when you
explicitly choose Hosted, BYOK, Balanced, or a connected integration, and are
limited to the capabilities and destinations you enabled.
