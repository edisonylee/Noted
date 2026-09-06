# Unread navigation

## UX and behavior

Provide Jump to unread and Mark unread actions, a stable New messages divider, and keep existing scroll restoration. Explicitly marking unread must not be immediately undone by background polling. Jump to the exact first unread message, including replies in their thread.

## Engineering and security

Expose only the authenticated viewer’s read state. Mark unread relative to an authorized message; never accept another user ID or arbitrary negative cursor. Use a read-state version to prevent stale in-flight acknowledgments overwriting intentional unread marks. Keep notification high-water behavior separate so marks do not replay old alerts. Test race conditions, cross-room targets, deleted messages, and monotonic normal acknowledgments.

## Delivery gate

Additive schema, explicit server capability flags for older servers, formatted source, focused regression tests, frontend build, and synthetic UI review. Include runtime packaging updates for new server files. Commit this feature independently on master; do not push. Server deployment by the owner is required before using new endpoints.
