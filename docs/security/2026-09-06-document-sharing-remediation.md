# Document sharing audit remediation

The three findings in `2026-09-06-document-sharing-audit.md` are addressed in the
working tree. This note describes the fixes; the audit remains an immutable
record of the earlier snapshot and its reproductions.

## DOC-01: exact local source identity

`team_document_identity.rs` maintains an additive local SQLite table mapping a
live document to a stable, random 256-bit `document:v2:` key. Equal row numbers
in independent vaults receive different keys. The native publication command
checks the key against the selected live local document before sending it.

Lookup requires the current account, exact opaque key and document kind. Multiple
matches fail closed instead of choosing the first collection. The dedicated
server publication endpoint independently checks owner, key, collection and
revision when replacing a local copy. Collaborative editing stays separate.

“Open in Library” requires the current viewer to be the publisher and resolves
the opaque key through the local database. Numeric suffixes are never local IDs.
Legacy numeric-key copies remain readable and editable in Team, but the app does
not guess their local origin or replace them automatically. A newly published
verified copy is separate; existing old copies are not silently migrated/deleted.

## DOC-02: reviewed audience on every local release

Creates and updates both use `team_publish_document` and
`POST /document-publications`, which requires a current audience version. A
transaction verifies audience, room eligibility, ownership and content revision
before writing. Missing/stale audience versions fail closed. Old servers cannot
silently ignore the fields because the endpoint is new, and the snapshot
capability `document_publication_review` gates the client workflow.

The publish sheet retains content after rejection and offers **Refresh and
review audience**. It refreshes the destination and requires the user to review
it before retrying; no automatic retry releases the document.

## DOC-03: remove local link destinations

The Markdown exporter keeps display text but omits local filesystem, relative,
internal-app and unsupported URL targets, recording a generic omission without
copying the sensitive target into the omission metadata. HTTP, HTTPS and mailto
remain supported; credential-bearing URLs and control characters are rejected.
Images continue to be omitted. Deliberately written prose is not silently redacted.

## Publication UX and preflight

The composer passes its exact room into the publish flow. Eligible writable
collections are fetched before selection and checked again server-side before
upload persistence. Already-published references must pass share-target
eligibility before staging.

The primary action says **Publish to [collection] now** or **Update published
copy now**. The sheet explains that this immediately uploads the copy, and that
removing the staged reference/cancelling the chat message does not unpublish it.
The collection audience and integration access remain visible in the review.

## Validation and rollout

- Native unit test: stable identity, separate vaults, reverse lookup, legacy key
  rejection, trashed documents and non-document rejection. Native library compiled.
- 139 tests across the team service, meeting references, document export, messaging
  helpers and new security regression tests passed (963 assertions).
- Frontend production build and service TypeScript check passed.
- Browser verification passed: explicit release, path omission, eligible collections,
  stale-audience rejection, manual review/retry, staging and persistent-copy semantics.
  It used a synthetic document and mocked native bridge against the real local team
  service; no real local document was read or uploaded.

Deploy the updated team server before installing the rebuilt desktop app.
No production deployment or installed-app update is performed as part of these
fixes. Existing collection/integration authorization is unchanged; publication
still creates a remotely stored copy rather than a live view into local files.
