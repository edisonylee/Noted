# Data flows and threat model

Status: proposed for product-owner acceptance

## Trust boundaries

1. User and macOS account.
2. Noted application, SQLite library, referenced media, and Keychain.
3. Local model runtimes and registered same-user agent clients.
4. User-selected third-party/BYOK model providers.
5. Future Noted managed services, storage providers, and paired devices.
6. User-created exports outside Noted's control.

Content entering from notes, transcripts, imports, OCR, calendars, agents, and
model output is untrusted data. It never changes permission policy by instruction.

## Capability/data-flow matrix

| Mode or capability | May leave the Noted process/device | Must not leave | User control |
|---|---|---|---|
| Local capture/search/chat | Requests to local Ollama/Whisper on loopback or in process | Corpus to internet, secrets | default; works offline |
| Balanced extract/OCR | Submitted text or image required for that extraction, request metadata, provider response | unrelated notes, retrieval corpus, embeddings, chat history, voice templates | explicit provider mode and key |
| BYOK feature | Minimum prompt/evidence needed for the named feature | unrelated scopes, attachments by default, secrets other than selected credential | per-provider disclosure and disable |
| Local Context Pass | Exact approved bounded packet, citations, claimed client metadata | raw DB/path access, restricted records, voice templates, attachments/media | client registration, preview, approve, revoke |
| Manual Noted Library | Selected record envelopes/content and explicitly selected attachments | tokens, credentials, indexes, biometrics, absolute paths | scoped export confirmation |
| Future encrypted backup | Ciphertext, manifest metadata required by design | plaintext content or keys if advertised unreadable | explicit enrollment and recovery-key UX |
| Future managed retrieval/sync | Only enrolled scopes and server-readable classes under the eventual contract | non-enrolled local data | separate enrollment, device and scope controls |
| Google Drive mirror | Explicit snapshot/media bytes and minimal Drive metadata | live retrieval queries, credentials beyond Drive token | opt-in and disconnect |

Production logs contain no note/transcript/query text, embeddings, credentials,
private paths, raw provider payloads, or Context Pass contents. Diagnostic events
use operation IDs, counts, timings, error classes, provider/model identifiers, and
packet/file hashes where needed.

## Threats and required controls

| Threat | Control and residual risk |
|---|---|
| Local database or media theft | restrictive permissions, Keychain secrets, optional encrypted backups; app database encryption remains an explicit pre-ship decision, and FileVault/user-account compromise remains outside app control |
| Inconsistent live database copy | SQLite Online Backup or `VACUUM INTO`, hashes, fsync, checks; media inventory barrier after recording |
| Export oversharing | scope/kind/date preview, counts/bytes, plaintext warning, fail-closed sensitivity, private staging, receipt; user can still redistribute the completed copy |
| Agent enumerates corpus | purpose-bound Context Pass, result/byte/token/date/scope ceilings, approval, expiry, receipts, rate limits |
| Same-user client impersonation | per-client revocable secret plus peer UID; name remains claimed without code-sign attestation; malware running as the user is residual risk |
| Prompt injection in a note/transcript | source text is quoted/untrusted, policy and tool schemas are out of band, no content-triggered permission widening or writes |
| Path traversal/symlink escape | registered roots, canonical path checks, no arbitrary-path API, relative sanitized snapshot paths, reject links escaping roots |
| Deleted content survives in an index | centralized lifecycle fan-out, generation-aware deletion tests, rebuild tests, staging cleanup; physical remnants/backups require distinct claims |
| Cross-scope inference | scope filter before retrieval, partition/filter every candidate source, fail closed on unknown, enforce again before disclosure |
| Malicious/compromised model provider | data minimization, provider-specific mode, timeouts, response validation, no secrets in prompts; provider receives the explicitly sent content |
| Future sync replay/conflict | stable IDs/revisions/hashes, device identity, authenticated encryption, monotonic protocol state, explicit conflict semantics before implementation |
| Resource exhaustion | bounded readers, query deadlines, cancellation, candidate/byte/token ceilings, active-recording priority, resumable derived work |

## Security decisions still required before shipping affected features

- Database and ephemeral Context Pass encryption-at-rest promise.
- Signing/encryption and key recovery for managed sync and backup.
- macOS code-sign attestation requirements for any standing local agent grant.
- Physical-deletion copy and the exact retention of backup/rollback generations.
- Network threat model before any HTTP agent transport or managed retrieval.

