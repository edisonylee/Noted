# noted

A local-first personal notes app that turns messy capture into structured, searchable knowledge — privately, on your own machine.

You jot, speak, or photograph a note; a local model categorizes it, extracts the structured bits, and files it. Over time noted builds a personal knowledge base you can search semantically, a daily schedule, recaps and trends, and a lightweight knowledge graph of the people and things in your life.

By default everything runs **100% locally** through [Ollama](https://ollama.com) — text, vision, and embeddings never leave your computer. An optional "Balanced" mode offloads only the latency-sensitive OCR/extract calls to Google Gemini.

## Features

- **Capture anything** — typed text, voice (offline speech-to-text via whisper.cpp), or photos (vision OCR, including Apple HEIC/HEIF).
- **Automatic structuring** — a note is split into sections; `Header:`-tagged sections route deterministically while untagged prose is classified by the model. One note can fill several categories at once.
- **Semantic search** — local `nomic-embed-text` embeddings in a `sqlite-vec` vector store.
- **Today** — a daily schedule parsed deterministically (no LLM, never hard-fails), with connected time ranges, inline editing, and optional one-way push to Google Calendar.
- **Knowledge graph** — entity resolution for people and things, with embedding-based merge suggestions you confirm. The "Knowledge" view surfaces People and a Self graph.
- **Recaps & trends** — auto-generated day/week recaps and per-category trends.
- **Chat** — ask questions over your own knowledge base with a local model.
- **Themes** — swap the full visual system with bundled presets, import a `DESIGN.md`, or ask the local assistant for a style. Imports are validated data-only packs; no paid MCP or cloud model is required.
- **Phone access** — a token-gated LAN HTTPS server serves the full app (or a lightweight capture page) to your phone, installable as a PWA, with every desktop command bridged over HTTP.

## Tech stack

- **Desktop shell:** [Tauri 2](https://tauri.app) (Rust)
- **Frontend:** React 19 + TypeScript + Vite, `lucide-react`, `recharts`, `react-force-graph-2d`
- **Storage / search:** `rusqlite` (bundled SQLite) + `sqlite-vec` for vector search
- **Local models (via Ollama):** `qwen2.5:7b-instruct` (text), `qwen2.5vl:7b` (vision), `nomic-embed-text` (embeddings, 768-dim)
- **Speech-to-text:** `whisper-rs` (whisper.cpp), in-process and offline
- **LAN server:** `tiny_http` over self-signed TLS (`rcgen`) — a secure context is required for phone mic/camera
- **Optional cloud (Balanced mode):** Google Gemini (`gemini-2.5-flash` / `gemini-2.5-flash-lite`)

Secrets (the Gemini API key, Google Calendar OAuth tokens) are stored in the macOS Keychain, never in the repo or the database.

## Prerequisites

- **macOS** (the primary supported platform — uses the Keychain and builds whisper.cpp locally; minimum macOS 10.15)
- **Node.js** 18+ and npm
- **Rust** (stable) + the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
- **[Ollama](https://ollama.com)** running locally, with the models pulled:

  ```sh
  ollama pull qwen2.5:7b-instruct
  ollama pull qwen2.5vl:7b
  ollama pull nomic-embed-text
  ```

## Getting started

```sh
npm install
npm run tauri dev      # run the desktop app in development
```

To produce a distributable build:

```sh
npm run tauri build
```

Frontend-only scripts (`vite`) are also available via `npm run dev` / `npm run build`, but the app needs the Tauri backend to do anything useful.

## Optional setup

- **Balanced mode (Gemini):** open Settings, switch to *Balanced*, and paste a Gemini API key. Only OCR/extract calls go to Gemini; chat and embeddings stay local. The connection badge confirms the key is live.
- **Google Calendar:** in Settings, connect your Google account (OAuth with PKCE). Today can then push the day's schedule one-way into a dedicated "noted" calendar.
- **Phone access:** open the phone panel in the app to get a QR code / URL and token; your phone joins over the LAN and runs the full client.
- **Themes:** open Settings → Themes for bundled presets. You can also paste or upload a `DESIGN.md`; the local text model creates a safe preview before you apply it. See [`THEMES.md`](THEMES.md) for the pack contract.

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
    gcal.rs       Google Calendar sync
    phone.rs      LAN HTTPS server + RPC bridge
```

## Privacy

noted is local-first by design: in the default (local) mode, your notes, photos, audio, embeddings, and search all stay on your machine. Cloud calls happen only if you explicitly enable Balanced mode or Google Calendar sync, and even then are limited to the specific feature you turned on.
