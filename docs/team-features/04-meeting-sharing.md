# Meeting-to-message sharing

## UX and behavior

From a shared meeting, choose a conversation, optionally select a decision/excerpt, and review the destination audience before sharing. Show a source card in messages that opens the exact published meeting. Only share published sources, never silently publish local notes.

## Engineering and security

Use structured source references rather than arbitrary URLs. Require current source access and destination send permission. Recheck the destination audience can read the source before sharing, including every active DM recipient. Do not persist sensitive titles/excerpts in the message: derive cards per viewer from live source permissions and revisions. Inaccessible/deleted sources show an unavailable card with no leaked content. Validate selected excerpt against the current source. Idempotent creation; test restricted collections, group grants, revoked access, trash, edits, and foreign IDs.

## Delivery gate

Additive schema, explicit server capability flags for older servers, formatted source, focused regression tests, frontend build, and synthetic UI review. Include runtime packaging updates for new server files. Commit this feature independently on master; do not push. Server deployment by the owner is required before using new endpoints.
