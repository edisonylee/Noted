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

## Implementation and verification

All five are implemented. The full Team regression suite passes **75 tests**,
including HTTP authorization and isolated Docker runtime packaging. The frontend
build passes; `cargo check --lib` passes for the native attachment Save command.
Source is formatted with Prettier; the native teams module uses rustfmt.

Synthetic UI checks exercised attachment cards, first-unread history, explicit
unread hold/resume, shared pin confirmation/list, reviewed meeting quote sharing,
source-card navigation, private save/open/remove, and compact layout without
horizontal overflow. Light and dark themes were inspected. The installed app and
native Save dialog still need the user's hands-on testing; no real team data was
used in these checks.

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

Commits remain local on master for review. Pushing, production server deployment,
and installed-app testing are left to the user.
