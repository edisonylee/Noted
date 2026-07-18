# Bring Your Own Provider plan

Status: planned after the Hosted-mode stabilization pass.

## Product goal

A user can choose either:

1. **Noted Hosted** — one activation, no model setup, Noted handles every capability.
2. **Bring Your Own Key** — the user supplies provider credentials and pays that provider directly.
3. **Local** — optional offline/private models managed on the user's device.

BYOK must not require one vendor to provide every capability. Text/vision, embeddings, transcription, and speech output are separate provider slots.

## Provider slots

| Slot | Used for | Required fallback |
|---|---|---|
| Intelligence | chat, extraction, summaries, recaps, answers, journal, themes | Noted Hosted or Local |
| Vision | handwriting/photo OCR and image understanding | Intelligence provider when vision-capable |
| Embeddings | 768-dimensional indexing and retrieval | Noted Hosted or Local `nomic-embed-text` |
| Transcription | dictation and meeting audio to text | Noted Hosted Parakeet or Local |
| Speech output | optional read-aloud/voice replies | macOS system speech |

The Settings UI should show a compatibility warning rather than silently using a different paid provider. Users must explicitly approve fallback behavior.

## Initial provider support

### First-party adapters

- **OpenAI**: intelligence, vision, embeddings, transcription, and optional text-to-speech.
- **Google Gemini**: intelligence, vision, embeddings, and audio understanding/transcription. For low-latency dedicated speech recognition, Google Cloud Speech-to-Text is a distinct credential/product and should be modeled separately.
- **Anthropic**: intelligence and vision. Anthropic's public API should not be treated as an embeddings or speech-to-text provider; pair it with another configured slot.
- **Noted Hosted**: all required capabilities through the existing activation credential.
- **Local**: Ollama plus optional local speech models.

### Compatibility adapter

Add a configurable **OpenAI-compatible** adapter with base URL, API key, and model IDs. This provides broad support without one-off code for:

- Groq
- OpenRouter
- Together AI
- Fireworks AI
- Mistral-compatible deployments
- LM Studio
- llama.cpp servers
- vLLM
- other services implementing the required OpenAI endpoint shapes

Compatibility must be capability-tested. An OpenAI-compatible chat endpoint does not imply embeddings, vision, structured output, or audio support.

### Dedicated speech adapter

Support **Groq Speech-to-Text** as an early optional transcription adapter because it exposes an OpenAI-compatible `/audio/transcriptions` endpoint with Whisper models. This is separate from Groq chat configuration even when both use one key.

## Configuration model

Replace the single provider mode with a versioned capability configuration:

```json
{
  "version": 2,
  "profile": "byok",
  "intelligence": { "provider": "anthropic", "model": "..." },
  "vision": { "provider": "anthropic", "model": "..." },
  "embeddings": { "provider": "gemini", "model": "gemini-embedding-2", "dimensions": 768 },
  "transcription": { "provider": "openai", "model": "gpt-4o-mini-transcribe" },
  "speech": { "provider": "system" }
}
```

The JSON stores provider IDs, base URLs, model IDs, and non-secret preferences only. Every API key remains in macOS Keychain under a distinct account name.

Existing Local, Balanced, and Hosted configurations need an additive migration into this structure. Never delete existing keys during migration.

## Adapter interface

Backend adapters should implement explicit capabilities rather than expose vendor APIs to feature code:

```text
generate_text(messages, options) -> text
generate_json(messages, schema, options) -> JSON
understand_image(images, prompt, schema) -> JSON
embed(texts, dimensions=768) -> vectors
transcribe(wav, language, vocabulary, timestamps) -> transcript
speak(text, voice, format) -> audio  [optional]
list_models() -> models             [optional]
test_capability(capability) -> result
```

Feature code must call these interfaces. It must never branch on provider names.

## Embedding safety

The current SQLite vector tables require 768 dimensions, but equal dimensions do not mean equal vector spaces. Switching embedding providers makes existing vectors incomparable.

When the embedding provider or model changes, Noted must:

1. Show that the search index requires rebuilding.
2. Confirm the operation with the user.
3. Re-embed every note and entity using the new provider.
4. Atomically replace or version the index.
5. Preserve the source text so rebuilding is always possible.

Store an embedding-space fingerprint with every index: provider, base URL class, model ID, dimensions, and normalization policy.

## Transcription behavior

Keep Noted's local capture, two-channel timing, VAD, echo suppression, and transcript merge logic. Swap only the ASR adapter.

- OpenAI: `/v1/audio/transcriptions`; support transcription-specific models and diarized output when selected.
- Groq: OpenAI-compatible transcription endpoint with Whisper models.
- Gemini: audio understanding can transcribe long recordings, but dedicated live transcription should be treated separately from general audio prompting.
- Anthropic: require a separate transcription slot.
- Noted Hosted: existing Parakeet batch and chunk-session endpoints.
- Local: existing Whisper/Parakeet engines.

For vendors without idempotent session APIs, Noted should upload closed VAD chunks and persist per-chunk request state locally. Never assume a chat streaming protocol can safely replace transcription.

## Settings experience

### Profile picker

- Noted Hosted — recommended; everything included
- Use my API keys — advanced
- Local — offline/private

### BYOK setup wizard

1. Select intelligence provider and paste key.
2. Fetch or enter text and vision model IDs.
3. Run a structured-output and vision test.
4. Select an embedding provider; test exactly 768 dimensions and warn about reindexing.
5. Select transcription provider; upload a bundled, non-personal sample for testing.
6. Review fallback choices and estimated data destinations.
7. Save only after every required capability passes.

Each key field must show its own remove/replace action and Keychain status. Never echo keys back to the frontend after saving.

## Privacy and billing clarity

Before activation, show a routing summary such as:

```text
Meeting audio -> OpenAI
Meeting transcript and summaries -> Anthropic
Notes for semantic indexing -> Google Gemini Embeddings
Credentials -> macOS Keychain
```

BYOK usage is billed by the selected providers, not by Noted, except for separately purchased Noted services. Provider errors and quota exhaustion must name the affected capability without exposing request content or credentials.

## Delivery phases

### Phase 1 — provider core

- Capability interfaces and routing registry
- Versioned configuration and migrations
- Keychain credential registry
- Per-capability test results and error mapping
- Preserve current Hosted and Local behavior

### Phase 2 — primary adapters

- OpenAI full adapter
- Anthropic intelligence/vision adapter
- Gemini intelligence/vision/embedding adapter
- OpenAI-compatible intelligence adapter
- Groq transcription adapter

### Phase 3 — UX and index migration

- BYOK setup wizard
- Model discovery with manual-entry fallback
- Routing/privacy summary
- Embedding-space fingerprint and safe reindex workflow
- Quota, invalid-key, unsupported-capability, and rate-limit UX

### Phase 4 — validation

- Contract tests with mocked provider responses
- Opt-in live tests for each provider
- Structured-output schema tests
- Vision fixture tests
- 768-dimensional embedding and full reindex tests
- Transcription fixtures, retries, timestamps, and long-meeting tests
- Secret scanning and Keychain deletion tests
- Ensure no provider is contacted when Local mode is selected

## Definition of done

- A new user can activate Noted Hosted without model downloads.
- A BYOK user can configure supported capabilities using only keys and model selections.
- Anthropic users are clearly prompted to choose separate transcription and embedding providers.
- Changing embedding providers cannot silently corrupt semantic search.
- No secret appears in JSON, SQLite, logs, crash reports, source control, or frontend state after save.
- Every outbound data category and destination is visible before the user enables the profile.
