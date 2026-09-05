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
- Hidden conversations are not polled for message bodies or marked read. The room
  list refreshes for the navigation's unread count while Team is visible.
- Recent previews are bounded to 160 characters and returned after room access
  checks. Deleted messages show a tombstone. A missing preview field from an older
  server is tolerated.
- Team rename uses an authenticated, admin-only endpoint. Existing organizational
  and DM authorization checks remain authoritative.
- Group DMs, attachments, threads, typing, push notifications, and searching message
  bodies remain outside this implementation. Messages do not enter meeting prompts.

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
