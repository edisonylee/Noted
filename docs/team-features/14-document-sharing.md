# Document sharing (`/document`)

Status: implemented (phases 1 and 2; phase 3 remains open). Every decision below is settled; the former open
questions were resolved as: a per-conversation Media panel is where shared
images, files and documents are browsed; `search_team_meetings` keeps its name
and returns documents too; the body is Markdown only, with the editor JSON
riding alongside deferred until fidelity is a reported problem.

## Problem

A Library document reaches a teammate today only by exporting it by hand and
dropping the file into a conversation. Meetings have a real path: an explicit
publish into a collection, `/meeting` in the composer, a staged source card, a
per-viewer card in the message, and a reader that respects live permissions.
Documents should have the same path. The composer is where the decision to
share is made, so `/document` is the entry point.

## What already exists and is reused unchanged

Features 04, 12 and 13 built most of this:

- **Composer actions.** `ComposerAction` is `"meeting" | "attach"`;
  `composerActions` supplies label and description; `slashCommands()` matches a
  leading `/` only (never a URL, code, or a slash in prose); the plus button and
  the `/` listbox render the same list, gated by room capability flags.
- **Staged source reference.** One pending `meeting` reference per message,
  shown as a removable card beside staged files, kept in the session store
  across navigation, cleared on successful send, included in the retry key, and
  sent as `meeting: { id, revision, start, length }` on the existing POST.
- **Server reference.** `chat_meeting_refs(message_id, note_id, revision,
  quote_start, quote_length)`; `meetingReference()` derives the card per viewer
  from live permissions and returns `available: false` for trash, revoked
  access or foreign IDs, so the message body never carries a title or excerpt.
- **Publish gate.** `POST /notes` with `source_key` idempotency
  (`UNIQUE(space, owner, source_key)`), collection and audience review, and an
  `accessVersion` recheck that 409s if access changed after the preview.
- **Rendering.** Message bodies render CommonMark + GFM through
  `MessageMarkdown`; the team reader renders note bodies through `MdBlock`
  with a Copy Markdown action. No renderer work is needed for documents.

The sections below describe the delta as built.

## Decision

A shared document is a **published note of kind `document`**, referenced from
a message exactly like a shared meeting. No second publish path, reference
table, card pipeline or reader.

- **Storage: a `kind` column on `notes`, not a new table.** `notes` already
  carries title, a large body, collection membership and group grants,
  folders, trash, revisions, the `notes_fts` index and the MCP surface. A
  separate table would duplicate every one of those and every access check
  with them. `kind` is an additive `ensureColumn` defaulting to `'meeting'`;
  the document's Markdown goes in `summary` (the 300,000-character notes
  field), `transcript` stays empty, `occurred_at` is the last local edit time.
- **Body: Markdown exported deterministically from the TipTap JSON.** It is
  what the reader and the message renderer already display, it is lossless
  plain text for search and MCP, and it is legible on an older client.
  Publishing the editor JSON was considered and rejected: the server would hold
  an editor-specific format that search and MCP cannot read, and the reader
  would need the full editor.
- **Snapshot semantics, like meetings.** The reference pins `revision`. Local
  edits never publish themselves; an explicit **Update shared copy** re-publishes
  through `updateNote`, bumping the revision so cards show "Updated since
  shared" and open the latest published version. Nothing leaves the Mac without
  a deliberate act.
- **Wire compatibility.** The POST field stays `meeting` (documented as "source
  reference"); `chat_meeting_refs` is reused as-is. Renaming either is a
  breaking change with no user-visible gain.
- **Source of documents: the local Library first.** `/meeting` lists notes the
  *team* already holds; `/document` must start from documents that live only
  on this Mac, so its picker is desktop-side (`api.listNotes` filtered by
  `isDocumentNote`) and may need to publish before it can stage. Referencing a
  document someone *else* already shared is phase 3, through the existing
  targets endpoint.

## Data model

`services/team/store.ts` constructor, after the `chat_reply_refs` index:

```ts
this.ensureColumn("notes", "kind", "TEXT NOT NULL DEFAULT 'meeting'");
```

Validate with `choice(kind, ["meeting", "document"])` in code rather than a
schema CHECK, so the additive migration cannot fail on an existing database.
Extend the `schema.sql` comment above `notes` to name the column as a
constructor migration.

Wire types (`src/teams/types.ts`):

```ts
kind: "meeting" | "document";              // TeamNote, TeamNoteRow, TeamMeetingSearchHit, meetingReference() result
document_references_enabled?: boolean;     // TeamChatRoom, beside meeting_references_enabled
```

`ComposerAction` becomes `"meeting" | "document" | "attach"`. The local additive
`team_document_sources` table assigns each document a persistent random
`document:v2:<256-bit random hex>` identity. Lookups are scoped to the current
team account and exact identity, and ambiguous multiple copies fail closed.
Numeric legacy keys are never automatically associated with local documents.

## Markdown export

New pure module `src/editor/documentMarkdown.ts`:

```ts
export type Omitted = { kind: "image" | "unsupported" | "link"; detail: string };
export function documentToMarkdown(doc: JSONContent): { markdown: string; omitted: Omitted[] };
```

Deterministic, no DOM, fixture-tested. Editor node set is StarterKit + Image +
TaskList/TaskItem + TextAlign + TextStyleKit:

| Node / mark | Output |
|---|---|
| paragraph, heading 1–6 | text, `#`…`######` |
| bold, italic, strike, code | `**` `*` `~~` `` ` `` (fixed nesting order: bold outside italic) |
| link | `[text](href)`; `javascript:` / `data:` hrefs emitted as plain text |
| bulletList, orderedList, taskList | `- `, `1. `, `- [ ]` / `- [x]`, two-space nesting |
| blockquote, codeBlock (language), horizontalRule | `> `, fenced with language, `---` |
| hardBreak | two trailing spaces + newline |
| image | **omitted**, replaced by `*[Image not shared]*`; recorded with alt text or file name |
| textAlign, textStyle | dropped silently — no Markdown representation and no content lost |

Images are omitted because they are local files or data URLs; inlining them
would breach the 300,000-character field and put binary content in text. The
publish preview lists every omission so nothing surprises after the fact.

## Server

`services/team/store.ts`

- `publishNote`: accept `kind` (default `'meeting'`); for `document`, require an
  empty `transcript`; audit `document.published`; duplicate copy becomes
  "This document is already shared in this space. Open the shared copy to
  update it."
- `note()` / `notes()`: serialize `kind`; `notes()` accepts `?kind=`. Default
  returns both so search and existing callers are unchanged; the Meetings tab
  passes `kind=meeting`.
- `meetingReference()`: return `kind`; excerpts come from `summary` as today.
- `chatRoom()`: `document_references_enabled: true` beside the meeting flag.
- `GET /chat-rooms/:id/meeting-targets`: accept `?kind=` and return `kind` per
  row (phase 3; same audience predicate, no new endpoint).
- `search`: no change; `notes_fts` already indexes `title/summary/transcript`.
- MCP: `search_team_meetings` / `get_team_meeting` keep their names, gain
  `kind` in results and an optional `kind` filter (default both), and their
  descriptions say "meetings and documents". No aliases.

`services/team/server.ts`: `POST /document-publications` is the dedicated
local-content release endpoint for creates and updates. It requires a current
`expected_access_version`, verifies owner/source/collection for updates, and
checks the optional destination room before storing any content.
`GET /chat-rooms/:id/document-destinations` preflights eligible writable collections.
The snapshot advertises `document_publication_review`; old servers are blocked
from the local publication flow rather than silently ignoring review fields.

## Desktop (Rust)

`team_publish_document` validates the local document identity and accepts the
reviewed `{ title, markdown, accessVersion }`, optional existing copy/revision,
and optional destination room. Both create and update use `/document-publications`.
`team_document_identity` and `team_document_local_id` resolve opaque identities
in the local database; the phone bridge denies these desktop-only commands. The three-place invariant applies — `team_publish_meeting` is
dispatched in `phone.rs` even though the LAN bridge is dormant — so the new
command needs the `#[tauri::command]`, the `generate_handler!` entry (the
registry lives in `desktop.rs`, not the `lib.rs` that `CLAUDE.md` names) and a
`phone.rs` match arm, or it silently 404s on that path.

## Client

### Composer action

Add to `composerActions`: `{ id: "document", label: "Share a document",
description: "Publish a Library document and link it" }`. It appears in the
plus menu and under `/` (`/doc…` filters it) when the room reports
`document_references_enabled` and the client is the desktop (the Library is
local; the web preview has no documents to offer). Choosing it opens the
document picker. Nothing else in `slashCommands`, the menu, or the Escape chain
changes.

### Document picker

`DocumentPicker.tsx`, built like `MeetingPicker` on `TeamDialog`: the
viewer's Library documents from `api.listNotes` filtered by `isDocumentNote`,
title-prefix filtered, newest edit first, rows "Title · edited 2d ago", with
loading, empty ("No documents in your Library yet") and error states. Choosing
a row:

- **already shared in this team** (a `notes` row with this `source_key` and
  `kind=document`, looked up through `GET /notes?kind=document` and cached per
  session) → stage `{ id, revision }` immediately, exactly as the meeting
  picker does;
- **not yet shared** → the publish sheet: collection, audience review, the
  rendered Markdown preview with an explicit **Not included** list when
  `omitted` is non-empty, then **Share document**. On success, stage the new
  reference. Cancel stages nothing and leaves the draft untouched.

Staged card, send, retry key, and clearing after send are the existing
reference path; the card and the send payload do not know or care about kind.

### Cards and reader

`MessageMeetingCard` → `MessageSourceCard`, dispatching on `kind`: the document
card uses `FileText`, the title, "Updated since shared" when the revision
moved, and the excerpt when present. The unavailable card is unchanged.
`SharedMeeting` → `SharedNote` in `TeamWorkspace.tsx`: for `kind:
'document'` the header reads collection · publisher · Updated date · revision
(no meeting date), the body is `MdBlock` without the `[mm:ss]` source jumps,
Copy Markdown is unchanged, and the `can_edit` path is the same PATCH with
"Edit shared copy" / "Markdown" / "Save shared copy" wording. Every reader
mount (conversation card, Search hit, Ask source) is the one component, and a
Search hit opens the reader for any non-message kind. On the desktop, when
the viewer is the publisher and the opaque source key resolves to a live local
document through `team_document_local_id`, **Open in Library** opens that exact
document. A remote numeric suffix is never interpreted as a local row ID.

### Library document page

**Share to team** sits in the document header beside the Move control
(`NotesView.tsx`, desktop only). It is absent until `GET /v1/orgs` answers,
so a disconnected or unreachable team hides it rather than failing; when the
page opens, and again after each publish, the page asks every connected team
for its own `kind=document` note with the exact opaque local identity
(`findSharedDocument`), and the button reads **Update shared copy** once one
exists. Both open `PublishDocument.tsx`, a sheet built like
`PublishMeeting.tsx`: team (preselecting the team that holds the copy),
title, collection (fixed once a copy exists), folders, audience review, the
exported Markdown rendered with `MdBlock`, a **Not included** list when the
export omitted anything, and the character count, which turns into a refusal
above 300,000 characters. A note with no usable editor JSON exports its plain
text one paragraph per line, exactly as the editor would show it. First share
and updates both go through `team_publish_document` and the dedicated reviewed
publication endpoint. The button says **Publish to [collection] now** or
**Update published copy now**. Copy explains that publication is immediate and
cancelling the chat message does not unpublish it. An audience change requires
**Refresh and review audience** before retrying. Server
errors, including the 409 duplicate and stale-revision messages, are shown
verbatim. These make the feature usable without the composer and give the
update path a home.

### Meetings tab

Passes `kind=meeting`; it stays a meetings tab. Shared documents are browsed
from the conversation they were shared in, through the Media panel below.

### Media panel (per conversation)

A **Media** control in the conversation header, beside Pinned and Threads,
opens a panel in the thread-panel slot (the same slot Threads and Pinned use)
listing what this conversation has shared, newest first, with three filter
chips: **Images** (image attachments as a thumbnail grid, reusing the inline
preview machinery), **Files** (non-image attachments: name, size, sender,
date) and **Documents** (referenced `kind=document` notes: title, publisher,
shared date, "Updated since shared" when the revision moved). Activating an
image or file opens its message in place through `jumpToMessage` /
`onOpenMessage`; activating a document opens the shared note reader. The panel
follows the Threads panel's conventions: Escape closes it, focus returns to
the Media button, `resetThreadPanel()` clears it on every exit path, the
main pane is inert while it is open, loading / empty / error states with
Retry, "Load older" keyset paging, and a polite live region for the count.

Server: `GET /chat-rooms/:id/media?kind=images|files|documents&before=<seq>`
returning `{ items, next_before }` paged by the message's `created_seq` (30
per page). Authorization is `chatRoom()` before any listing SQL, exactly like
`/threads`. Images and files come from `chat_attachments` joined to live
messages in the room (`deleted_at IS NULL`), images being `mime LIKE
'image/%'`. Documents come from `chat_meeting_refs` joined to `notes` with
`kind='document'`, and a row is **excluded** — not shown as unavailable —
when the viewer cannot read the note or it is trashed, because listing a
title the viewer cannot open would leak it. Each row carries `message_id`,
`created_seq`, `created_at`, the author, and either an `attachment
{ id, name, mime, size }` or a `document { note_id, title, updated }`. The
room gains `media_enabled: true`; older servers never show the control. The
route joins the `threads` allowlist entry and is per-token rate limited.

## UI behaviour

- `/` listbox shows Share a document beside Attach files and Reference a
  meeting; keyboard and dismissal identical.
- Picker rows and empty states match `MeetingPicker`; the publish sheet's
  primary reads "Share document" (first time) or "Update shared copy".
- Cards share the meeting card's dimensions and tokens; the only accent use is
  the existing `.message-target` flash. Light and dark through tokens.

## Edge cases

- Document deleted locally after sharing: the shared copy stands; **Update
  shared copy** disappears; the card is unaffected.
- Shared copy trashed on the team: card unavailable; a fresh publish is
  possible only once the trash is emptied (`UNIQUE` on `source_key`),
  otherwise the sheet shows the server's 409 message verbatim, as the meeting
  sheet does.
- Viewer loses collection access: `available: false` via the existing branch.
- Excerpt against a stale revision: empty; card says "Updated since shared".
- Export over 300,000 characters: publish refuses with the count and suggests
  splitting; nothing is sent.
- Image-only document: Markdown is a list of omission lines; publish allowed,
  preview makes it obvious.
- Same document shared twice in one conversation: two messages, two
  references, one shared copy.
- Older server (no `kind`, no flag): the action is absent; a document card from
  a newer server on an older client renders as a meeting card with the
  document's title — acceptable, documented.
- Web preview (no Library): the action is absent by client capability.

## Tests

- `tests/document-markdown.test.ts`: every export-table row, nesting order,
  omission recording, hostile hrefs, byte-for-byte determinism.
- `services/team/server.test.ts`: publish with kind (stored, transcript empty,
  audit), duplicate `source_key` wording, `?kind=` filtering, `kind` and
  excerpt from `meetingReference()`, unavailable on trash / revoked grant /
  foreign org, revision pin and "updated" after `updateNote`, FTS finds
  documents by body, `meeting-targets ?kind=`, MCP `kind` filter, constructor
  migration idempotent (`PRAGMA table_info` lists `kind` once, default
  `'meeting'` on existing rows).
- `tests/team-messaging.test.ts`: `slashCommands` lists and filters the new
  action only when available; the document reference participates in the
  retry key like a meeting reference.
- Synthetic check via `team:demo` + `team:preview` for the team-side pieces;
  the desktop picker and publish sheet need the installed app.

## Files

- `services/team/store.ts` (also `chatMedia()`), `server.ts` (`media` route), `schema.sql` (comment), `mcp.ts`, `server.test.ts`
- `src/teams/MediaPanel.tsx` (new), `src/teams/composerCommands.ts`, `ComposerActions.tsx` (icon),
  `TeamMessages.tsx` (action handler, picker mount), `DocumentPicker.tsx`
  (new), `MessageSourceCard.tsx` (from `MessageMeetingCard.tsx`),
  `TeamWorkspace.tsx` (`SharedNote`), `PublishDocument.tsx` (new sheet beside
  `PublishMeeting.tsx`),
  `types.ts`
- `src/editor/documentMarkdown.ts` (new), `src/NotesView.tsx` (Share / Update
  actions in the document header), `src/api.ts` (`teamPublishDocument`)
- `src-tauri/src/desktop.rs`, `src-tauri/src/phone.rs`
- this file (→ implemented status), `README.md` index, `TEAM_DESIGN.md`

## Phasing

1. **Export + publish from the Library page** (done). `documentToMarkdown`,
   `kind`, `team_publish_document`, Share / Update actions, reader header.
   Usable alone and settles every data decision.
2. **`/document`** (done). Action, `DocumentPicker`, publish-then-stage, card
   dispatch.
3. **Reference others' documents** (open). `meeting-targets ?kind=` and a kind switch
   in the existing picker; Documents filter in the Meetings tab; MCP aliases if
   wanted.

## Risks and open questions

- **Fidelity.** Markdown drops alignment and text styling and omits images. The
  omissions list makes it visible; a design-heavy document will look plainer.
  If unacceptable later, publish the editor JSON *alongside* Markdown for a
  richer reader — an addition that does not change this plan.
- **Media panel scope.** It is per conversation, which keeps authorization
  identical to `/threads` and matches Discord's channel media view; a team-wide
  media view would need its own audience predicate across rooms and is not
  part of this change.
- **300,000-character cap** is inherited from the meeting notes field —
  generous for prose, but a hard wall the preview must show before anyone
  hits it.
- **No content recheck in Rust.** `team_publish_meeting` re-derives its payload
  from the local row and compares it to the reviewed content; the document's
  Markdown is exported on the client, so the command can only prove the note is
  a live, untrashed document with a matching `document:` source key. The
  server-side `accessVersion` recheck still applies.
- **Media includes thread replies.** Attachments sent inside a thread belong to
  the conversation and appear in its Media panel; the panel is "what this
  conversation shared", not "what the main timeline shows".
- **Rollout order.** The shared-copy lookup relies on the server's owner-scoped
  `GET /notes?kind=document&source_key=` filter. An older server ignores the
  parameter and returns every readable document, and the client's re-check by
  key would then match another owner's note with the same local id. Deploy the
  team server before the client.
- **Desktop-only source.** The picker reads the local Library, so `/document`
  cannot exist in the web preview or on a second Mac that does not hold the
  document; phase 3 covers the "already shared" case for everyone.

## Security release gate (September 6 audit follow-up)

- Random per-document identities prevent cross-user and cross-vault row-ID collisions.
- Local-copy replacement requires the current publisher, exact source identity,
  collection, note revision and reviewed audience version. General collaborative
  editing remains a separate flow.
- Conversation publication options are preflighted and checked again transactionally
  on the server. Existing references also check share-target eligibility before staging.
- Local/internal/unsupported hyperlinks retain their label, omit their target,
  and appear in **Not included**; HTTP, HTTPS and mailto links remain supported.
- Deploy the updated server before the client. Existing numeric-key copies remain
  readable/editable in Team, but are not automatically replaced from Library.

See `docs/security/2026-09-06-document-sharing-remediation.md` for validation.
