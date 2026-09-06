# Messages layout redesign

## Intent

Make choosing a conversation quick and predictable, using Noted's warm surfaces,
Geist typography, quiet selection, and hairline separators. The reference is the
clarity and restraint of a native Mac sidebar, not a copy of another product.

## Problems addressed

The previous sidebar inherited generic meeting-navigation styles, mixed utility
navigation with conversations, repeated create-channel actions, and pushed a
large explanatory footer into the list. Titles, previews, times, and badges
competed for width. Narrow windows stacked two independent panels and could
produce horizontal overflow.

## Structure

1. Fixed Messages header with one compose action for starting a DM.
2. Compact conversation-name filter; content search remains in Team Search.
3. Mentions as a compact utility row with its own count.
4. Direct messages ordered by recent activity.
5. Channels, with Team chat first and other channels alphabetically.
6. Archived channels in a separate collapsed group, when present.

Groups can collapse. Filtering temporarily expands matching groups, including
archived channels. No-results state explains the filter and offers Clear filter.
Empty groups use short copy. Channel creation lives only beside Channels; the
redundant bottom action and explanation are removed.

## Conversation rows

Use a stable three-column layout: avatar/icon, flexible title and preview,
trailing time and unread indicator. Ellipsize names and previews without pushing
the trailing column or creating horizontal scrolling. Show the full name in the
button tooltip. Distinguish unread names through weight. Combine mention and
unread state into one badge with an accessible description. Keep draft and mute
indicators visible without introducing more columns. Peer DMs omit a redundant
sender-name prefix; outgoing messages retain “You:”.

## Responsive behavior and accessibility

The pane responds to its own available width, not just the browser viewport.
Desktop keeps list and conversation side by side. Below 700px, show one pane at
a time with a Conversations back control; choosing a conversation or an external
message destination opens the detail. Filtering and collapses survive navigation
in memory. Native buttons provide keyboard activation; group toggles expose
expanded state and current selection uses aria-current. Provide visible focus
rings, labeled icon buttons, and no hover-only essential actions.

## Verification

Check populated and empty groups, long names/previews, drafts, unread mentions,
muted rooms, archived rooms, collapse/filter behavior, and create actions. Inspect
light/dark themes and wide/narrow widths. Verify list/detail navigation and retain
the existing send, draft, mention, notification, and search flows. Frontend build
and messaging regressions must pass. No backend or authorization changes needed.

Implemented and verified with synthetic team data: light/dark appearance, 1200px
and 650px windows, long labels without horizontal overflow, collapsed groups,
archived filtering, no results, compose/channel dialogs, and compact navigation
with keyboard focus handoff. `bun run build` and all seven messaging regression
tests pass. Installed-app verification remains for local review.
