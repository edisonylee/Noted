# Composer actions and meeting references

## Intended experience

Use one quiet Lucide plus button in the composer for **Attach files** and
**Reference a meeting**. Typing `/` lists the same available actions; `/attach`
and `/meeting` filter them. Arrow keys select, Enter/Tab opens the action, and
Escape or an outside click closes the list. Ordinary slashes, URLs, Markdown
code, and unknown commands remain ordinary message text. Commands do not send.

The meeting picker searches published meeting titles and collection names,
ordered newest first. Each row includes title, date, and collection. Selecting
stages a removable card alongside the existing text and files. The user can
replace the reference, add commentary, then send explicitly. Text-only,
reference-only, and combined messages work in conversations and threads.

## Engineering plan and implementation

1. Add a room capability and authenticated, paginated meeting metadata endpoint.
   Reuse the existing meeting audience predicate, filtering eligible collections
   before selecting note rows. Return at most 30 rows with a continuation offset.
2. Isolate shared action definitions and slash matching from presentation.
   Use Lucide React components, native buttons, listbox/menu semantics, and the
   shared outside-dismissal hook. No extra icon dependency is needed.
3. Reuse TeamDialog for a searchable picker with debounced requests, stale-response
   suppression, loading/empty/error states, retry, and bounded pagination.
4. Stage only source metadata in the existing account/organization/conversation
   session store. Preserve drafts on cancel, navigation, and failed sends; clear
   the reference after a successful send. Like draft files, pending references
   are session-only and do not survive quitting the application.
5. Include source identity and revision in send retry keys. Reuse the existing
   transactional send validation and meeting-reference rendering/opening path.

## Security and rollout

The picker requires an ordinary authenticated member session and permission to
send in the destination. Channel references require a team-visible collection;
DM references require access for every active participant. Unauthorized room
IDs, removed grants, trashed notes, and foreign organizations cannot expose
meeting metadata. Queries and offsets are validated and SQL values are bound.
No summaries, transcripts, or local private meeting content are returned.

Send revalidates current audience permissions and source revision, so a picker
result is never treated as authorization. A stale source produces the existing
review-and-retry error; choose the meeting again to stage its current revision.
Opening sent references continues to check current access. No new database
schema, native commands, or additional external data sharing is introduced.

Deploy the team server with `GET /chat-rooms/:id/meeting-targets` and
`meeting_references_enabled` before installing the updated client. Older servers
retain attachment actions but hide the meeting picker/command. The existing
one-way sharing flow remains available. No push or deployment is performed here.

## Validation

- Service tests: recipient eligibility, revoked grants, foreign organization and
  private DM denial, archived room denial, bounded metadata, pagination, search,
  HTTP routing, and combined text/reference sends.
- Frontend unit tests: command filtering and literal-text boundaries, capability
  gating, source identity/revision retry keys, existing messaging regressions.
- Synthetic browser checks: action menu, slash selection, search/empty states,
  staging across navigation, removal, failed-send retention and retry, explicit
  send, Escape/outside dismissal,
  file chooser, and compact layout. Production build and service typecheck.
