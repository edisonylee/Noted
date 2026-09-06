# Message attachments

## UX and behavior

Choose up to three PNG/JPEG images, PDF, or plain-text files from the composer. Show filename, size, remove action, and errors before sending. Keep pending files when a send fails; send once with an idempotency key. File-only messages are supported. Received files use explicit Save actions; never auto-open documents.

## Engineering and security

Store bounded attachment bytes and metadata transactionally alongside the message in SQLite. Limit each file to 5 MiB, total message files to 5 MiB, three files per message, and team storage to 250 MiB. Accept only matching extension/signature types; disallow paths, controls, SVG, HTML, and executables. Downloads recheck live team/DM permissions and deletion. Return bytes through the authenticated native bridge, not public URLs. Save through a native picker and quarantine on macOS. No inline PDF execution, extraction, or external fetches. Metadata only in message lists. Delete bytes with the message. Stream-limit HTTP bodies before JSON decoding; allow larger bodies only on message creation. Test bounds, forged types, cross-organization/DM access, revocation, deletion, duplicate retries, and storage quota.

## Delivery gate

Additive schema, explicit server capability flags for older servers, formatted source, focused regression tests, frontend build, and synthetic UI review. Include runtime packaging updates for new server files. Commit this feature independently on master; do not push. Server deployment by the owner is required before using new endpoints.


The follow-up [attachment preview plan](11-attachment-previews.md) adds local image,
PDF canvas, and literal text previews, superseding the initial download-only UX.
