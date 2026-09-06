# Saved messages

## UX and behavior

Add private Save/Unsave message actions and a Saved entry beside Mentions. List recent bookmarks with author/conversation/excerpt, open the exact message or thread, and offer removal. Make the privacy distinction from shared pins explicit.

## Engineering and security

Bookmarks are scoped to user and organization and reference messages without copying bodies. Reauthorize targets on every list and open; hide revoked or deleted content. Support bounded cursor pagination and a per-user quota. Idempotent save/remove, never accept owner IDs. Test other users, DMs, cross-organization access, revoked membership, deletion, and pagination.

## Delivery gate

Additive schema, explicit server capability flags for older servers, formatted source, focused regression tests, frontend build, and synthetic UI review. Include runtime packaging updates for new server files. Commit this feature independently on master; do not push. Server deployment by the owner is required before using new endpoints.
