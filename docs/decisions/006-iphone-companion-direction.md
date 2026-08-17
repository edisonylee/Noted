# Decision 006: iPhone companion direction

Status: accepted product direction; implementation started

Date: 2026-08-14

Owners: product, mobile, data architecture, and security

Implementation status: M0 accepted; the first M1 quarantine checkpoint is
implemented; M2 preflight is complete and blocked on full Xcode installation.
No synchronized mobile data model has shipped.

## Context

Noted's existing phone code is a dormant LAN-served browser client. It requires
the Mac to be awake and reachable, has no offline replica, and exposes a broad
remote-command boundary that is not suitable for the product.

The desired product is an iPhone companion that provides the nonconversational
Noted experience—Today, Calendar, capture, Notes, Meetings, People/Knowledge,
search, and existing derived artifacts—without requiring a public App Store
listing or moving local intelligence onto the phone.

The detailed product, protocol, migration, security, and rollout contract lives
in [the mobile companion implementation plan](../MOBILE_COMPANION_IMPLEMENTATION_PLAN.md).

## Decision

- Build for iPhone/iOS first using a native Tauri 2 shell that reuses the React
  product and platform-neutral Rust/TypeScript logic.
- Give the iPhone its own local SQLite replica. The interface reads local state
  first and remains useful while the Mac or network is unavailable.
- Keep inference, transcription, OCR, diarization, extraction, semantic indexing,
  and conversational assistance on the Mac initially.
- Do not ship Ask, entity chat, meeting copilot, Live Assist, provider controls,
  or model administration on iPhone.
- Allow bounded, typed, idempotent Mac jobs from the phone, such as processing a
  submitted capture or refreshing a meeting summary.
- Prove one Notes vertical slice over a narrowly scoped, authenticated direct-Mac
  sync adapter before migrating all record families.
- Use a custom application-layer encrypted relay for durable remote sync and
  recovery after the direct protocol converges. Cloud-readable intelligence is a
  separate future consent mode.
- Use Internal TestFlight for the owner/team beta. Paid Ad Hoc distribution is
  the strict off-store fallback; an unlisted App Store release remains a later
  durable option.
- Keep the legacy LAN/PWA bridge disabled in every release profile. It may be
  retained only as an explicitly developer-only diagnostic surface after its
  high-risk behaviors are removed.

## Accepted product defaults

- Notes owns Inbox, Needs filing, Meetings, spaces/folders, and Trash. More owns
  People, Knowledge, Recaps, Trends, and secondary destinations.
- All canonical Noted text is available locally on the iPhone. Calendar uses the
  cache window and media uses the lazy/pinned policy defined in the plan.
- Mobile v1 supports text, photos, handwriting/photo input, dictation, and short
  voice notes. It does not promise multi-hour meeting recording or audio capture
  from other iPhone apps/calls.
- Existing generated artifacts may be viewed. User edits are canonical overlays
  and are never silently overwritten by regeneration.
- Photos sync after cloud enrollment; retained audio is opt-in; video is a
  separate choice; temporary transcription audio and voiceprints do not sync.
- No existing library data uploads until the user sees the inventory, size,
  media, metadata, network, recovery, disable, export, and purge consequences and
  completes recovery setup.
- Conflicts preserve acknowledged user work. Blanket last-write-wins is not an
  approved policy.

## Required gates

- Decisions 003–005 remain independently statused. This decision accepts their
  mobile-relevant requirements; it does not silently enable the agent surface.
- The portable-canonical-layer revisit in Decision 003 occurs before multi-device
  sync ships.
- The single-writer and recording-priority rules in Decision 004 apply to every
  replica.
- Pairing, accepted-head revisions, local branches, encryption, local-at-rest
  protection, recovery, revocation, purge generations, and direct-to-relay
  authority cutover require accepted protocol ADRs and tests before real-data
  rollout.
- The existing meeting-memory roadmap proof remains active and is not displaced
  by exploratory mobile work.

## Consequences

This approach requires more foundation than re-enabling the PWA, but it gives the
phone a durable offline product, preserves Noted's privacy boundary, avoids
synchronizing a live SQLite file, and leaves room for future platforms.

The first implementation checkpoint is safety and feasibility: reconcile product
copy, quarantine the dormant LAN surface, close its known high-severity issues,
and verify the native iOS toolchain on a physical-device-capable Mac.
