# Channel mentions

Typing `#` at the start of the text or after a space opens the same listbox
the `@` picker uses, labelled "Link a channel": up to eight non-archived
channels, Team chat's channel first and then alphabetical, filtered by the
slug's prefix as you type. The default room reads `#general` with a muted
"Team chat" hint. ArrowUp and ArrowDown move, Enter or Tab insert the
canonical lowercase name plus a space and place the caret after it, Escape
dismisses until the next keystroke, and any space or character outside the
slug grammar closes it. Only one picker is ever visible and `#` wins when
both could apply, so `@Ed #de` lists channels and `#design @Ed` lists people.
The picker never appears while the room list is still loading, and the hint
under the composer reads "Markdown · @ mention · # channel · Enter to send ·
Shift + Enter for a new line" unconditionally, because the client resolves
names on its own and needs nothing from the server.

A sent message stores exactly what was typed: `#Design` stays `#Design` in
the body, in edit, in the sidebar preview, in search snippets, in the mentions
inbox, in saved and pinned lists, on older clients and in the MCP surface. At
render time each `#slug` that exactly matches a channel in the reader's own
room list becomes an inline button showing the text as typed, with a quiet
hover-soft background, a `--line` hover and underline, and a title of "Open
#design", "Open Team chat", or "Open #design · Archived channel". The token is
the maximal run of `[A-Za-z0-9_-]`, so `#design-system` never partially links
to `design`, `#designs` stays plain, and `#design.` or `(#design)` resolve; a
`#` preceded by a letter, digit, underscore, another `#`, or `/` never matches,
which keeps `C#design`, `issue#12`, `##design` and URL fragments plain. Code
spans and fenced blocks stay plain; references inside bold, italic and strike
resolve. Chips render in the main pane, in thread replies and on the thread
parent; the inline-reply reference line stays plain text. Activating a chip
closes any open thread panel or thread list, selects the channel through the
same path as a sidebar click (the row becomes current, its group expands, the
filter clears, the compact layout shows the detail pane), and in the wide
layout focuses the new room's composer. An archived channel resolves as a
muted chip and opens its read-only history; un-archiving it needs no client
work because the chip only reads the room's current `archived_at`.

The server stores plain text and never resolves names: no schema change, no
lookup-by-name endpoint, no server-side detection. `findChannelMentions` lives
beside `findMentions` in `services/team/mentions.ts` so both matchers share
one grammar, but nothing on the server calls it; a channel reference is not a
people mention and never touches `unread_mentions`, the mentions inbox, read
marks or desktop notifications (a service test pins `unread_mentions` at 0
and an empty inbox). Resolution runs only against the reader's own fetched
`/chat-rooms`, which the server already filters by membership and which is
keyed per workspace on the client, so a name can never link across
organizations and a removed member sees nothing resolve. Renames are the
accepted cost of plain text: after `#design` becomes `#design-ops`, old text
degrades to readable prose and the new name resolves everywhere after the next
room poll, exactly as `@Old Name` degrades after a profile rename. If that ever
becomes a complaint, the deferred follow-up is a `chat_channel_aliases(org_id,
name COLLATE NOCASE, room_id, retired_at)` table written by `updateChatRoom`
on every rename, returned with each room as `aliases: string[]`, and consulted
by `findChannelMentions` after the live names; it costs a schema migration, a
server-before-client rollout, one extra query per `chatRooms()` call, and a
chip whose label no longer matches its destination, which is why it is not
part of this change. No native code changes: `lib.rs`, `phone.rs` and
`team_notifications.rs` are untouched.

Validation: the root suite covers the matcher (exact names, typed-case
ranges, archived rooms, word and URL guards, empty input), the channel query
(whitespace rule and precedence over the mention query) and the merged
people-plus-channel hits with overlap dropping. The service test covers a
verbatim body, zero unread mentions and an empty inbox, resolution by another
member, full-text hits for `design` and `#design`, rename rot, archived and
un-archived resolution, and access removal. Synthetic browser checks covered
the picker order and prefix filter, Enter insertion, Escape, the `@Alex #`
precedence and the index reset after Escape, a chip beside a plain `#nope`,
chip navigation from another channel with the filter cleared and the composer
focused, a chip in a thread reply opened from the Threads list closing the
panel and switching rooms, a muted archived chip opening read-only history,
Escape cancelling a pending inline reply after the picker, the compact layout,
the hint fitting one line at 700px with no horizontal overflow, and dark-theme
contrast on the hover background. Deferred: chips in the mentions inbox,
saved, pinned and search surfaces, and the alias table above.
