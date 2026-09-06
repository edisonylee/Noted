# Threads

Add a Threads control to the conversation header, between Pinned and Search,
for channels, direct messages, and archived channels. It shows a muted count of
threads with new replies and opens a list inside the existing thread panel slot:
newest reply first, each row with the author, the root's time, a three-line
excerpt, the reply count, the participants ("Taylor, Alex and you"), the time of
the last reply, and an unread badge. Activating a row swaps the slot to the
ordinary thread view with a Back to threads chevron before the close control.
Back returns to a refreshed list with the originating row focused; Escape means
Back when the thread was opened from the list and Close otherwise; Close always
returns focus to the main composer. Threads reached from a message row, search,
mentions, pins, saved messages, or a notification behave exactly as before.
Opening from the list never scrolls or highlights the main timeline and writes
nothing. Rows move with ArrowUp, ArrowDown, Home, and End; a polite live region
announces the count after each refresh.

A thread is any root with at least one live reply, the same predicate the
timeline uses for reply counts, so the list and the timeline never disagree. A
deleted root whose replies survive stays listed as a tombstone ("Message
deleted"); opening it shows the deleted parent with a disabled composer. A
thread whose replies are all deleted drops out on the next refresh. Unread is
room-level: `unread_replies` and the header count come from the viewer's
existing room read cursor, so reaching the bottom of the main view or of any
thread clears every thread's count, a held manual unread mark restores them,
and your own replies never count. Per-thread read state is deferred.

The server adds `GET /v1/orgs/:org/chat-rooms/:id/threads` returning
`{ items, next_before }`, paged by keyset on the last reply's sequence (30 per
page). It requires an ordinary member session, rejects integration keys, and
runs under the normal per-token rate limit. The room carries `threads_enabled`
and `unread_threads`; `unread_threads` is derived from rows `chatRoom()` already
loads, so sidebar polling gains no SQL. The `chat_thread_replies` partial index
(`thread_id, created_seq WHERE thread_id IS NOT NULL`) is created in the store
constructor, after the `thread_id` column migration, because `schema.sql` runs
before that column exists; anything reading `schema.sql` alone will not see the
column or either thread index. The client refreshes an open list every ten
seconds while visible, on window focus, and when the long poll re-emits a
thread root; page-one refreshes are spaced at least two seconds apart. Older
servers without `threads_enabled` never show the control. No native code
changes: the endpoint is served by the team service only, so `lib.rs` and
`phone.rs` are untouched.

Validation: service tests cover ordering, counts, participants, unread and read
cursors, keyset paging, deleted replies and roots, conversation and membership
access, HTTP authorization including integration keys and unsupported methods,
idempotent constructor migrations across reopens, and restart persistence. The
root suite covers thread merging, participant labels, previews, and short
times. Synthetic browser checks cover the header control, list rows and badges,
row to thread and Back, Escape layering, docked and overlay layouts, and the
empty, loading, and error states. Deferred: per-thread read receipts, a
"threads I'm in" filter, a cross-conversation threads view, sidebar nesting,
and push-based refresh.
