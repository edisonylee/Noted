# Team meetings and conversations

Implemented September 5, 2026.

## Product model

A **team** is one organization and its members. The Team area has three persistent
destinations:

1. **Meetings** — company knowledge assembled from explicitly published meeting
   notes. Search, source-grounded questions, saved answers, and collection access
   belong here.
2. **Messages** — conversations with people. Team chat includes everyone, direct
   messages connect two teammates, and additional team channels are optional.
3. **People** — the team directory, with a direct Message action and an entry point
   to invitation and membership settings.

**Collections** replace the team UI's ambiguous “Spaces” label. They group meeting
notes by project, client, or topic and show whether access is for all members or
restricted. They are not chat channels. The original uncustomized “Team knowledge”
collection displays as “General meetings.” Existing collection IDs, grants, notes,
and API names remain compatible.

Team names can be changed by admins. A team's identity and content do not change
when its display name changes. Local personal Library spaces remain separate.

## Design decisions

The previous navigation mixed destinations, containers, membership settings, and
model conversation history in one sidebar. Entering human chat replaced that
navigation with a back link. The redesign uses permanent destination navigation
and a sidebar specific to the selected task.

Messages uses a conversation list with recent previews, unread counts, names,
and a consistent composer. DMs are the default action for New message. Channels
remain available for teams that need project conversations; users do not have to
create a channel to begin chatting. The existing general-channel history is shown
as Team chat.

Meeting questions are labeled **Ask meetings** and **Previous questions**, avoiding
confusion with human messages. The question input is more compact. **Share a
meeting** explains how a private meeting becomes company knowledge and opens the
private Library in the desktop app. Publishing still requires a collection,
audience review, content preview, and an explicit publish action.

This applies the familiar separation between destination navigation and
conversation navigation documented in [Slack's sidebar guidance](https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences), while preserving Noted's existing knowledge and privacy model. It is a product-specific design decision, not a claim of Slack feature parity.

## Implementation boundaries

- Keeping Messages mounted within Team preserves unsent drafts when switching
  Meetings, Messages, and People. Drafts do not survive leaving Team or restarting.
- Hidden conversations stop requesting new message bodies and are not marked read.
  An already waiting native request can finish, but its result is discarded. The room
  list refreshes for the navigation's unread count while Team is visible.
- Recent previews are bounded to 160 characters and returned after room access
  checks. Deleted messages show a tombstone. A missing preview field from an older
  server is tolerated.
- Team rename uses an authenticated, admin-only endpoint. Existing organizational
  and DM authorization checks remain authoritative.
- Threads and emoji reactions were added in the subsequent chat update. Group DMs,
  attachments, typing, push notifications, and searching message bodies remain
  outside this implementation. Messages do not enter meeting prompts.

## Verification

- 38 service tests passed, including preview privacy, edit/delete preview updates,
  membership revocation, and admin-only rename without identity/content changes.
- The message merge regression test passed; both TypeScript checks and the standard
  desktop release build passed.
- Synthetic UI checks covered shared-note collections, audience labels, navigation,
  DMs opened from People, sending, previews, unread updates, draft preservation,
  channel creation controls, and team rename. Browser logs showed no warnings or
  errors during these checks.
- Native installed-app checks covered the real team loading, persistent navigation,
  existing meeting/message history, and Share a meeting opening the private Library.
- Visual inspection covered a compact browser layout and the installed dark-theme
  desktop layout. Accessibility names and active-navigation semantics were checked;
  a full screen-reader, contrast, and large-team usability audit was not performed.

## Chat interaction update

Threads now keep replies beside their original message, with counts, history,
editing, deletion, and emoji reactions. The searchable reaction picker supports
64 emoji and shows who reacted. Profiles in main Settings (also accessible from
team settings) add display names,
photos, job titles, and bios, with member-authenticated photo access.

The three-second message polling gap was replaced with authenticated long polling.
React layout effects position new content before the next paint, preserving the
reading position when scrolled into earlier history. The thread panel focuses its
composer on opening and closes with Escape. A Threads control in the
conversation header lists a conversation's threads, newest reply first, in that
same panel. Profile upload validation rejects
remote URLs and active image formats. Identity and history survive display-name
changes.

Inline replies sit beside threads: a Reply action on a message row (before
Reply in thread) starts a quote-reply at the same conversation level, shown as
a quiet bar above the composer with the author, the first line of the original
and a cancel control. The sent reply carries a one-line reference above its
author line; activating it jumps to and briefly highlights the original in
place, or reloads around it when it is outside the loaded window, and a deleted
original becomes a non-interactive tombstone. Escape is layered: it dismisses
the mention picker first, then cancels a pending reply, then acts on the thread
panel as usual (Back when the thread came from the list, Close otherwise).

Channel references round out the composer: `#` opens the same picker as `@`,
labelled "Link a channel", listing Team chat first and then the other open
channels by prefix, and inserts the canonical slug. The body stores the text
as typed; at render time a name matching one of the reader's own channels
becomes a quiet inline chip (hover-soft background, `--line` on hover, muted
when the channel is archived) that closes any thread panel and opens the
channel in place. Unresolved names, code spans and URL fragments stay plain,
and a channel reference never pings anyone.

Library documents travel the same road as meetings. `/document` (or the plus
menu's "Share a document", on the desktop only, when the room reports
`document_references_enabled`) lists the viewer's own Library documents; one
already shared in the team is staged at once, and one that is not opens the
publish sheet first — team, title, collection, audience, the exported Markdown
preview with a "Not included" list for images and anything else the export
dropped, and a 300,000-character refusal — so nothing leaves the Mac without a
deliberate act. The sent message carries only a revision-pinned reference;
the source card is derived per viewer from live permissions, shows "Updated
since shared" when the copy has moved on, and opens the shared note reader,
which for a document reads collection · publisher · last updated. A Media
control in the conversation header lists the conversation's images, files and
documents. The same sheet is reachable from a document's own header as
**Share to team**, which becomes **Update shared copy** once a copy exists.

The service suite now has 44 passing tests (338 assertions), plus the message-merge
regression test. New checks cover thread pagination and isolation, idempotent
reactions, live wake-up/revocation, profile authority and upload validation, and
restart persistence. Synthetic browser checks exercise message/reply sends,
reaction add/remove, profile upload/save, avatar/bio display, and live arrival.
An incoming message preserved an earlier reading position exactly and showed the
new-message control. The native request boundary has three passing Rust tests,
including profile read/update access without exposing session-creation routes.

This follows the discussion pattern in [Slack's thread documentation](https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions-).
Scroll timing follows [React's layout-effect contract](https://react.dev/reference/react/useLayoutEffect).
