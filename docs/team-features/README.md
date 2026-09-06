# Team messaging features

Each feature has an implementation plan, UX behavior, security requirements, and
validation gate:

1. [Attachments](01-attachments.md)
2. [Unread navigation](02-unread-navigation.md)
3. [Pinned messages](03-pinned-messages.md)
4. [Meeting sharing](04-meeting-sharing.md)
5. [Saved messages](05-saved-messages.md)
6. [Message formatting](06-message-formatting.md)
7. [Drop attachments](07-drop-attachments.md)
8. [Threads](08-threads.md)
9. [Inline replies](09-inline-replies.md)
10. [Channel mentions](10-channel-mentions.md)
11. [Attachment previews](11-attachment-previews.md)
12. [Full chat Markdown](12-chat-markdown.md)
13. [Composer actions and meeting references](13-composer-actions.md)
14. [Document sharing](14-document-sharing.md)

## Implementation and verification

All fourteen are implemented. The team service suite passes **93 tests** across its
eight files, including HTTP authorization and isolated Docker runtime packaging,
and `tests/team-messaging.test.ts` passes **15**; the two root-suite failures
that remain (alpha bundle packaging and backup destination defaults) predate
these features and are unrelated to team messaging. The frontend build passes;
`cargo check --lib` passes for the native attachment Save command.
Source is formatted with Prettier; the native teams module uses rustfmt.

Synthetic UI checks exercised attachment cards, first-unread history, explicit
unread hold/resume, shared pin confirmation/list, reviewed meeting quote sharing,
source-card navigation, private save/open/remove, and compact layout without
horizontal overflow. Light and dark themes were inspected. The installed app and
native Save dialog still need the user's hands-on testing; no real team data was
used in these checks.

Attachment previews have three additional unit tests and synthetic browser checks
for inline images, PDF navigation/zoom, literal text, compact layout, and access
denial. They reuse the existing attachment endpoint and require only an app update.

## Rollout

Deploy the team server with its additive schema migrations before using the new
client features. Back up the persistent SQLite database first. The client gates
new controls on server capabilities; it does not silently emulate missing
server endpoints. The new endpoints require ordinary authenticated member
sessions and are not exposed to read-only integration credentials.

Attachments add SQLite storage: 5 MiB total per message, three files per message,
250 MiB per team. Draft attachment bytes remain in session memory until sent;
closing the app discards unsent files. File signatures are validated, but the
service does not claim malware scanning. Native downloads use a Save dialog and
macOS quarantine; the app never auto-opens downloaded content.

Manual unread marks are held until explicitly marked read. Older clients cannot
overwrite a newer manual mark with stale read acknowledgments. Meeting sharing
stores references and quote offsets, not copied source content; viewing rechecks
current permissions, and quotes disappear when the source revision changes.
Saved messages are private; pins are shared with the conversation.

Threads add a read endpoint and a partial index created in the store
constructor only, so deploy the team server before the client. Inline replies
add a nullable `reply_to_id` column as an additive constructor migration; back
up the SQLite database first, and note that editing or deleting a quoted
original re-emits at most 100 quoting rows per change. Channel mentions change
neither schema nor endpoints, so they carry no ordering constraint between
server and client. Document sharing adds a `kind` column, the
`/media` route and an owner-scoped `?source_key=` filter the client depends
on, so deploy the team server before the client.

Commits remain local on master for review. Pushing, production server deployment,
and installed-app testing are left to the user.
