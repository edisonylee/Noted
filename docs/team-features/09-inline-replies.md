# Inline replies

Add a Reply action to a message row's hover and focus bar, before Reply in
thread, that quotes the message in the same conversation level: a main-view
message from the main composer, a thread reply from that thread's composer.
Choosing it focuses the composer, renames it "Reply to Name", and shows a
quiet bar above the compose box with the author, the first line of the
original (ellipsised, whitespace collapsed) and a Cancel reply control.
Choosing Reply on another message replaces the target. Escape cancels the
reply after the mention picker and before the thread panel: the next Escape
acts on the thread panel as usual (Back when the thread was opened from the
list, Close otherwise). Sending clears the bar; a failed send keeps it under
the usual error, except the specific "The message you are replying to was
deleted" conflict, which clears it because that retry can never succeed. The
target survives room and thread switches like staged attachments and is
dropped on reload; it is never persisted server-side or in local storage.

A sent reply renders one reference line above its author line: the original's
author and a one-line excerpt (plain text, never mention marks or formatting,
since a 160-character prefix can cut a span in half). Activating it scrolls
the original 40px from the top of the pane and paints it with the existing
`.message-target` accent for about 1.6 seconds without losing the loaded page;
when the original is outside the window it goes through the normal open flow
(an `around=` reload, thread panel reopened when the original is a thread
reply). A deleted original becomes a non-interactive italic "Original message
deleted" line, updated live. Quoting never pings the quoted author: no
implicit mention, no unread, no notification (deferred; the cheapest later
path is treating `reply_to.author_id === user` as a mention). Pinned, Saved
and Mentions rows do not show reply context (deferred). Editing a reply never
changes its target: PATCH ignores `reply_to_id`. The Reply action and the
reference line are gated on the room's `inline_replies` flag, so an older
server shows nothing new.

The server adds a nullable `reply_to_id` column to `chat_messages` as a
constructor migration like `thread_id` (`ensureColumn`, after the thread
indexes, with the `chat_reply_refs` partial index), because `schema.sql` runs
before the column exists. `sendChatMessage` enforces that the target is in
the same room and at the same conversation level and is not deleted at send
time; anything outside the room, including a direct message the sender is
not in, is "Message not found". The quoted excerpt rides `chatMessage()`'s
SELECT as a LEFT JOIN guarded by `p.room_id = m.room_id`, so a reference
costs no extra query and a tampered pointer serializes as null. Editing or
deleting an original re-emits the rows quoting it to the live cursor, capped
at 100 per change and room-scoped, without bumping their revision: the client
merges on `revision >= current`, and a bump would 409 anyone mid-edit of a
quoting reply; beyond the cap, older quoting rows refresh on their next full
page load. Quotes are not indexed by full-text search (only the reply body
is) and are not exposed via the MCP surface. `chatRoom()` is untouched;
`inline_replies` is a constant flag. No native code changes: `lib.rs`,
`phone.rs` and `team_notifications.rs` are untouched.

Validation: service tests cover same-conversation quoting and live reads,
thread-level enforcement, cross-room and private targets, tombstones and
deleted targets, live refresh without revision bumps and the fan-out cap,
immutable retry targets, FTS indexing only the reply body, restart
persistence, idempotent constructor migrations, HTTP send/edit/read routes
and mentions carrying `reply_to`. The root suite covers the send key, the
reference and preview helpers, the level rule and equal-revision merging.
Synthetic browser checks cover Reply to bar to Escape with the thread panel
staying open, send and reference rendering, in-window jump and flash, the
`around=` reload for an original outside the window, a live tombstone after
deletion from a second session, replies inside a thread panel, and the
compact layout without overflow. Rollout: the column is additive; deploy the
team server before the client and back up the SQLite database first.
