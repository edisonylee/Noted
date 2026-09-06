# Document sharing security and privacy audit

Follow-up: the working-tree fixes and their validation are documented in
[the remediation note](2026-09-06-document-sharing-remediation.md). Findings below
describe the original audited snapshot.

## Executive assessment

**Do not release the local-document sharing workflow in this snapshot.** Two
high-priority defects can put local content into the wrong shared copy or release
new content to a broader audience than the one reviewed. A third defect preserves
local filesystem paths in exported hyperlinks.

Scope: the uncommitted document-sharing and Media changes on top of `a526405`,
reviewed September 6, 2026. These are work in progress, not claims about deployed
production behavior. Attribution to Claude follows the user's description; the
audit observes repository changes and the checked-in design plan, not Claude's
private reasoning or live session. No product code was modified by this audit.

“Public channel” currently means **team-wide**, not anonymous Internet access.
Publishing uploads a persistent Markdown copy to the team service. Access can
include approved integrations when the collection permits them. The copy is not
served from the author's Mac and does not remain local-only.

## Findings

### DOC-01 — High / P1: Local numeric IDs can select and overwrite someone else's shared document

Locations: `src/teams/PublishDocument.tsx:31`, `:49`, `:197`;
`src/teams/TeamWorkspace.tsx:1417`; `src-tauri/src/desktop.rs:2919`.

`documentSourceKey()` is only `document:<local integer>`. The lookup scans all
readable team documents and picks the first matching key, without checking the
publisher, vault identity, or an explicit local-to-remote mapping. Integers are
independently allocated on each Mac. Server uniqueness is scoped to collection
and owner, so it does not make this lookup unambiguous. Team editors can edit
one another's shared notes, so `can_edit` does not establish source identity.

**Impact:** updating a local document can overwrite another person's shared
copy and publish the local content into that copy's existing audience. A member
can also deliberately create a colliding source key; this does not require
bypassing the server's ordinary collection permissions.

Reproduction (synthetic in-memory database): Bob publishes `document:42` in a
team-visible collection. Alice's lookup for her local document 42 returns Bob's
row with `can_edit=true`. The same PATCH used by the UI replaces Bob's summary
with Alice's local content, which Bob can immediately read. Confirmed.

The reverse “Open in Library” mapping also trusts the remote numeric suffix and
can open an unrelated local row, without checking that it is a document from this
vault. This is a false source association, not demonstrated remote file reading.

Remediation: use a persistent, opaque per-document identity scoped to the vault,
and a local mapping keyed by team server, organization, account and remote note.
Require exact ownership/source association for the local “update shared copy”
workflow. Keep intentional collaborative editing separate. Do not silently
migrate ambiguous numeric mappings; have the user verify the destination.

Required regression cases: same local ID across two users and two vaults under
one account; deliberately colliding keys; multiple collections; forged reverse
Library links; no remote writes until an exact association is established.

### DOC-02 — High / P1: Updating a shared copy bypasses audience-version review

Locations: `src/teams/PublishDocument.tsx:197`–`:215`;
`services/team/store.ts:812`–`:827`.

New publication sends the reviewed access version. The update branch sends only
title, body, folders and note revision. `updateNote()` checks the note revision
and write access but does not compare the reviewed audience version. Collection
visibility/grant/API changes do not bump the document's content revision.

**Impact:** fresh local content can be uploaded after a restricted destination
becomes team-visible or integration-readable while the user still sees the old
preview. Existing shared content becoming visible after an authorized admin
change is expected; releasing additional local content under a stale preview is
the defect here.

Reproduction: preview a restricted document, record the access version, then
change its collection to team visibility with API access. Submit the update with
the original note revision. The update succeeds and another team member reads
the new private content. Even supplying the stale `expected_access_version` in
the PATCH is ignored. Confirmed.

Remediation: carry the reviewed audience version through the local-copy update
flow and enforce it server-side before applying the update. A changed audience
must require a fresh preview/review. Include recipient grants, membership,
visibility and integration-readability changes in that version contract. Keep
backward compatibility for generic editors explicit rather than silently
weakening the local-content release endpoint.

Required regression cases: restricted-to-team transition, added member/group,
API enablement, unchanged content revision, missing/stale audience version, and
retry after a newly reviewed destination.

### DOC-03 — Medium / P2: Exported hyperlinks disclose local paths

Location: `src/editor/documentMarkdown.ts:82`–`:86`.

The exporter removes image sources and blocks three dangerous URL schemes, but
preserves `file:` links and absolute/relative filesystem destinations. A link's
local destination is metadata that can be hidden behind a harmless label in the
editor. It becomes part of the stored, searchable, integration-readable Markdown.

Reproduction: a text node labelled “Local source” with a link to
`file:///Users/alice/Clients/Secret-Acquisition/plan.pdf` exports that entire path
unchanged. Confirmed. This is path disclosure; no file contents were fetched and
no code-execution claim is made. The current preview renders the destination as
text, which can help a careful user notice it, but does not enforce omission.

Remediation: define an explicit outbound-link policy. Strip filesystem/internal
resource destinations while retaining their display text and list them among the
preview's omissions. Allow only intentionally shareable schemes; test `file:`,
absolute paths, Windows/UNC paths, relative local resources and internal app
schemes. Do not treat arbitrary prose containing a path as automatically safe to
redact; that requires a separate content-review policy.

## Privacy behavior needing clear product wording

The composer first publishes the document, then stages a reference. The **Share
document** button is the actual release point; the later message Send button is
not. Removing the staged card or closing the conversation does not revoke the
published copy. The current sheet does explicitly describe publishing a copy,
so this is not classified as a hidden-upload vulnerability. Make the irreversible
point unmistakable: “Publish to [collection] now”, with the audience and integration
access beside it, and explain that cancelling the message leaves the copy shared.

The picker is not scoped to a destination room and may publish a copy into a
collection that the conversation cannot reference. Send still rejects the
reference, but that rejection does not undo publication. Preflight eligibility
before uploading to avoid that confusing partial completion.

## Controls verified or positively observed

- Restricted-source references are rejected for team-wide channels. A synthetic
  direct send attempt confirmed this; eligible DMs use per-participant access.
- Shared source reads and document media listings check current collection,
  organization and conversation permissions. Revoked sources are filtered on the
  server rather than exposing titles through the media endpoint.
- Publication is explicit; opening the local picker does not itself upload the
  document body. New publication includes an audience-version check.
- Images are replaced with placeholders; document publication rejects transcripts.
  Raw HTML is rendered as React text by the current shared-note renderer.
- Integration reads remain restricted to explicitly approved collections; the
  new document content becomes readable through those existing approved scopes.
- The phone dispatcher rejects the new native local-document publication command.
  This is not a general assurance about every existing phone/agent permission path.

## Validation and limits

A disposable in-memory `TeamStore` reproduced DOC-01 and DOC-02, exercised the
restricted-channel rejection, and the pure Markdown exporter reproduced DOC-03.
No real documents, credentials, production team, or external recipients were used.
The reproduction script is `/tmp/noted-document-audit.ts` for this session.

The existing document/server/meeting-sharing suites passed **116 tests** with
780 assertions after allowing their loopback test listeners (the initial sandbox
run could not bind three listeners). Their success is not sufficient to catch the source-identity and update-consent cases above.
This is a targeted code and local-test audit, not a production penetration test,
full native runtime review, dependency audit, or certification. Future uncommitted
changes require re-review. No fixes, commit, push, or deployment were performed.

## Snapshot fingerprints

SHA-256 for the principal files at the final audit snapshot:

```
6ee5982a116b04c456f783a84f841da8381ac751ca7a10a278042cab19ea1c37  src/teams/PublishDocument.tsx
52adfc3f236c3d8a00934f79590aaae1f1af0f1b2eef9ac8036f4fe967da8ba5  src/teams/DocumentPicker.tsx
54c48fe79cd33925046bd382259506d6ae0f6bc6d76ee39315dcde7a63053b71  src/editor/documentMarkdown.ts
9f58edd6e5f78d6cc66f9e44248a183f8e4070c9a400b3b5558951b22ffe5bee  services/team/store.ts
3dda08907d21b39bd2a8b86e74bf26a2dde284d6d1ce0ae7e8beb4c1e41daad4  src-tauri/src/desktop.rs
97166e3015e74b9268b1198db874ea2b069f68733a273e6ebf33850d0c87ed99  src/teams/TeamWorkspace.tsx
```
