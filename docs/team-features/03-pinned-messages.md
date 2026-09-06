# Pinned messages

## UX and behavior

Offer Pin/Unpin from message actions and a compact Pinned panel from the conversation header. Show author, excerpt, who pinned it, and open the exact message/thread. Confirm shared pin changes; cap the collection and provide clear empty/loading/error states.

## Engineering and security

Pins are shared only with the conversation’s existing audience. Active participants may pin/unpin, including DMs; no elevated private-DM access for admins. Reauthorize every list, change, and target resolution. Atomic/idempotent mutations, bounded lists, no copied content retained after deletion. Test membership, organization isolation, archived rooms, deletion, and duplicate changes.

## Delivery gate

Additive schema, explicit server capability flags for older servers, formatted source, focused regression tests, frontend build, and synthetic UI review. Include runtime packaging updates for new server files. Commit this feature independently on master; do not push. Server deployment by the owner is required before using new endpoints.
