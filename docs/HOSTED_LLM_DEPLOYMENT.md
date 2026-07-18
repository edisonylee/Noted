# Hosted LLM deployment reference

Last consolidated: July 17, 2026

This document records the current hosted-LLM setup intended for Noted. It deliberately excludes API keys, license keys, private network addresses, and other credentials. Secrets belong in the operating-system credential store and should never be committed to this repository.

## Current deployment

- Public API base: `https://api.entersymphony.com`
- Exposure: Cloudflare Tunnel; the host does not require publicly forwarded inbound ports and its residential IP remains hidden.
- Gateway host: Windows desktop at `C:\Users\ediso\noted-gateway`
- Gateway process: Python/FastAPI listening only on `127.0.0.1:8080`
- Ollama: listening only on the host loopback interface
- Authentication: scoped API keys with `ntd_test_…` and `ntd_live_…` prefixes. Only salted hashes are retained by the gateway, and the plaintext key is displayed once when created.
- Billing: Stripe integration is prepared but was last reported in test mode.

On July 17, 2026, a health request from the Noted Mac returned `status: healthy` with both the gateway and Ollama reporting `ok`. The response showed no models kept warm at that moment (`models_loaded: []`), which is normal if models load on demand but means the first request may have extra latency.

Tailscale is not part of the public API path and is not required for this architecture. Cloudflare Tunnel provides the controlled public ingress. Tailscale can remain absent unless private administrator access is wanted later.

## Available API routes

- `POST /v1/chat/completions`
- `POST /v1/embeddings`
- `POST /v1/noted/extract`
- `POST /v1/noted/ocr`
- `POST /v1/noted/meeting-summary`
- `POST /v1/noted/recap`
- `POST /v1/noted/answer`

The gateway implements authentication, scopes, request limits, and an append-only usage ledger. Cloudflare provides TLS, DDoS protection, WAF controls, and edge rate limiting.

## Models and routing

- `gemma3:12b`: quality-oriented generation
- `gemma3:4b`: faster extraction, OCR, and lightweight work
- `nomic-embed-text`: 768-dimensional embeddings

The deployed names are Gemma 3 model tags, not Gemma 4. Model names in Noted and API requests must match the tags exposed by the gateway.

## Last reported capacity

The host has an RTX 3070 with 8 GB of VRAM.

- Gemma 3 4B: approximately 117 requests per minute in the reported test, approximately two-second responses, and no reported errors.
- Gemma 3 12B: approximately seven requests per minute; typical responses took 12–30 seconds, with a long meeting summary taking about 60 seconds. The model partially spills to CPU on this GPU.
- Early paid-beta estimate: roughly three to five simultaneously active users with the current routing. This is a planning estimate, not a service-level guarantee.

For beta capacity, route latency-sensitive and high-volume work to 4B and reserve 12B for work where quality materially matters. Add queueing, per-key quotas, timeout handling, and overload responses before offering a firm commercial SLA.

## Startup and resilience

The Windows host was configured with scheduled tasks named:

- `NotedGateway`
- `NotedTunnel`
- `NotedWatchdog`
- `NotedBackup`

These were last described as starting at user logon. True pre-login Windows services require a one-time elevated setup. Reboot-survival verification remained an explicit follow-up item in the supplied deployment history.

## Noted connection plan

The present Noted application has two provider modes:

- Local: all work uses the Mac's local Ollama instance.
- Balanced: extraction and OCR can use an OpenAI-compatible endpoint; embeddings, chat, meeting summaries, and other features remain local.

The existing Balanced mode can use the hosted gateway for extraction and OCR with:

- Base URL: `https://api.entersymphony.com/v1`
- Text model: `gemma3:12b` or `gemma3:4b`
- Vision/OCR model: `gemma3:4b`
- API key: an appropriately scoped test or live key entered directly into Noted so it is stored in macOS Keychain

Do not paste a production key into documentation, source control, logs, screenshots, or support conversations.

To run every Noted AI feature through the hosted service, the app still needs a dedicated hosted-provider mode that routes chat, embeddings, meeting summaries, recaps, and answers through the gateway's `/v1` endpoints. Until that is implemented, Balanced mode is only a partial connection.

## Remaining checks

1. Create a limited-scope test API key on the Windows gateway and enter it directly into Noted Settings.
2. Confirm the public health check and make one authenticated test request from outside the home network.
3. Verify the four scheduled tasks after a Windows reboot and confirm Ollama and the gateway still bind only to loopback.
4. Test 4B and 12B routing under concurrent load, then set per-plan quotas and timeouts.
5. Complete legal review of the selected model license before charging customers.
6. Move Stripe from test mode only after metering, cancellation, refund, abuse, and key-revocation flows are verified.

## Related host documentation

The supplied history says the Windows deployment also contains API documentation, a deployment journal, `docs/CAPACITY_REPORT.md`, and `docs/PRICING_WORKSHEET.md` under `C:\Users\ediso\noted-gateway`. Those host files should remain the operational source of truth; this document is the Noted-side reference.
