# Decision 002: Context, cloud, agent, and monetization boundaries

Status: accepted direction; provider and pricing details deferred

Date: 2026-08-06

Owners: product, data architecture, and hosted services

Implementation status: partly represented by the local/provider architecture;
managed sync, cloud media storage, Drive backup, and external-agent access are not
yet shipped.

## Context

Noted is starting with meeting notes but is intended to become a personal context
system. Productization introduces four concerns that should not be conflated:

1. canonical personal records;
2. rebuildable search and intelligence artifacts;
3. storage/sync availability; and
4. model inference.

Bundling all four behind one hosted database would weaken the local-first product,
make agent access hard to explain, and create unnecessary provider lock-in.
Treating cloud storage as the product would also commoditize Noted's value.

## Decision

### Canonical and derived data

- Lossless source records and user-approved corrections remain canonical.
- Chunks, FTS rows, vectors, summaries, inferred graph relations, and generated
  answers are derived and rebuildable.
- Public identities, provenance, revision, deletion, export, and permission
  contracts must not depend on SQLite row IDs, Mac paths, or one cloud provider.
- All capture surfaces eventually feed one retrieval and lifecycle core.

The detailed contracts and implementation sequence live in
[`../AGENT_CONTEXT_IMPLEMENTATION_PLAN.md`](../AGENT_CONTEXT_IMPLEMENTATION_PLAN.md).

### Local product

Local-only remains useful without an account, network, or official model service.
Users can capture, search, inspect, export, and delete their information. Local
models and bring-your-own-provider profiles remain supported.

### Managed Noted Cloud

Noted Cloud is an opt-in convenience layer for:

- account and device sync;
- encrypted backup and verified restore;
- remote/mobile availability;
- managed retrieval for authorized agents;
- hosted inference for users who do not want local setup; and
- a reasonable retained-media allowance.

Cloud enrollment does not retroactively upload local data without an explicit
choice. Backup-only encryption and server-readable context are distinct modes: a
service cannot promise that it cannot read encrypted data while also searching
that data on the server without a separate user-authorized processing design.

### Google Drive

Google Drive is a future optional user-owned backup/export destination, not the
primary source for Noted playback, retrieval, or sync.

When implemented, it should:

- request the narrow `drive.file` scope;
- create a visible user-owned Noted folder;
- upload compressed media asynchronously and resumably;
- preserve local capture and notes when offline or disconnected;
- verify a remote upload before offering to free local space; and
- store Drive file identifiers rather than assuming stable paths.

Drive lowers Noted's blob cost but introduces quota, OAuth, revocation, support,
and remote-file lifecycle complexity. It is a portability option, not a free
replacement for a managed product.

Primary implementation references:

- [Drive scope guidance](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)
- [Resumable uploads](https://developers.google.com/workspace/drive/api/guides/manage-uploads)

### Agent boundary

External agents use a typed, permissioned retrieval surface rather than raw
database or filesystem access. The first interface is local and read-only. A
purpose-bound context packet contains the smallest useful set of source-grounded
records, can be inspected before disclosure, and produces an access receipt.

Agent write-back begins as a proposed record or correction that requires user
approval. MCP or another protocol is an adapter over this contract, not the
storage architecture itself.

### Inference boundary

Product capabilities call provider-neutral interfaces. Local, BYOK, third-party
API, and Noted-hosted models remain interchangeable when they pass the same
feature evaluation.

Noted will not self-host a general-purpose model merely to claim ownership or
assume higher margins. A workload moves in-house only when measured sustained
utilization, quality, latency, reliability, operations, and fully loaded cost beat
the available alternative. Specialized transcription, embedding, extraction,
and ranking workloads are more plausible early candidates.

## Commercial model

The current planning model is:

- **Local:** user-owned local storage with local or BYOK inference.
- **Noted Pro:** managed sync/backup, remote access, retrieval, hosted-inference
  allowance, and bundled compressed-media storage.
- **Heavy usage:** separate, transparent storage or inference add-ons.
- **Drive:** optional user-owned mirror/export.

Noted sells reliable context continuity and use, not raw gigabytes. A demo may
show retained audio explicitly, but production defaults remain transcript-only.

At approximately 40 MB per retained meeting-hour:

| Allowance | Approximate retained audio |
|---|---:|
| 5 GB | 125 hours |
| 20 GB | 500 hours |
| 100 GB | 2,500 hours |

At current standard storage rates, 20 GB of stored data is roughly $0.14 per
month on Backblaze B2 or $0.30 per month on Cloudflare R2 before processing,
support, transaction patterns, and atypical egress. B2 currently includes free
egress up to three times average monthly storage; R2 Standard currently has no
internet egress charge. These are planning inputs, not provider commitments.

Primary pricing references:

- [Backblaze B2 pricing](https://www.backblaze.com/cloud-storage/pricing)
- [Backblaze transaction and egress pricing](https://www.backblaze.com/cloud-storage/transaction-pricing)
- [Cloudflare R2 pricing](https://developers.cloudflare.com/r2/pricing/)

The initial hypothesis is a modest bundled allowance in a paid plan and a
roughly $5/month 100 GB add-on for unusually heavy users. Exact pricing is deferred
until observed storage, replay, inference, support, payment, and churn data exist.
The managed plan should model greater than 80% gross margin before scale.

## Consequences

### Positive

- Local-first remains real rather than marketing language.
- Cloud can be added without replacing the canonical data model.
- Storage, inference, and product value can be priced and optimized separately.
- Users can choose portability without making Drive a runtime dependency.
- The same permission contract can serve multiple agents and protocols.
- Provider neutrality preserves leverage as model economics change.

### Costs and risks

- Offline/local and managed-cloud modes require explicit sync and conflict
  semantics.
- E2EE backup and cloud-side retrieval cannot be collapsed into one vague promise.
- Drive adds support burden even when byte storage is user-funded.
- Local, BYOK, and hosted inference increase the evaluation matrix.
- A permissioned agent interface is slower to build than exposing a database, but
  avoids an unacceptable privacy boundary.

## Deferred decisions

- Managed object-store provider and regional layout.
- Sync protocol, conflict model, account system, and key architecture.
- Exact bundled storage and inference allowances.
- Media lifecycle and overage UX.
- Whether cloud retrieval uses client-side, trusted server-side, or hybrid
  processing for each data class.
- Google OAuth token separation from the existing Calendar integration.
- Commercial SLA, support tier, and team/enterprise packaging.
