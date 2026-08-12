# Decision 005: Context Pass and local agent identity

Status: proposed for product-owner acceptance

Date: 2026-08-06

Owners: product, security, and agent platform

Implementation status: Phase 0 contract only; the agent surface remains off.

## Context

Giving an agent raw SQLite or filesystem access would bypass scope, sensitivity,
deletion, and disclosure controls. Local process names are also not trustworthy
identities on macOS when another process runs as the same user.

## Decision

- Agent access is off by default and read-only at launch.
- MCP is an adapter over an app-owned Context Pass service, not the storage API.
- The first transport is a local stdio helper connected to an authenticated
  app-owned Unix-domain broker. It does not reuse the phone HTTP bridge.
- Each registered client receives a revocable secret. The broker also validates
  the peer user ID. This stops cross-user access but does not prove which same-user
  executable owns the secret.
- Client names are labeled **claimed** until optional audit-token/code-signature
  attestation succeeds. Approval UI never represents an unattested name as
  verified.
- Every request declares purpose, allowed scopes and kinds, date bounds, result
  count, byte/token ceilings, and expiry. The user approves the exact candidate
  packet before release unless a future narrowly scoped standing grant is
  separately designed.
- Returned content is source-grounded, bounded, and accompanied by resolvable
  citations. Noted records a disclosure receipt containing metadata and packet
  hashes, not another plaintext corpus copy.
- Source text is untrusted data. It cannot modify tool policy or issue agent
  instructions.
- No raw SQL, arbitrary file paths, attachments, audio/video, biometric voice
  templates, shell commands, writes, or unbounded corpus enumeration.

## Revocation and deletion boundary

Revocation prevents new passes. It cannot claw back bytes already released to an
agent or included in a completed manual export. Product copy and receipts must say
this plainly. Permanent-delete tests promise application-unreachable content from
Noted-controlled indexes and future results, not forensic erasure from SSDs or
third-party copies.

## Deferred hardening

Code-signature attestation, standing grants, and any network transport require
separate threat review and explicit product approval.
