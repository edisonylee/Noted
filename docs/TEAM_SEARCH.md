# Team search

Status: implemented in source; hosted rollout requires a team server deployment.

## Purpose and scope

Find messages and published meetings by keyword in one Team search destination.
The conversation-name filter remains in Messages. Search does not read the local
personal Library, search across organizations, or generate an AI answer.

Messages and meetings appear in separate, independently ranked and paginated
groups. The combined view shows five results per group initially; single-type
search shows twenty. A message opens at its exact location, including thread replies. A
meeting opens its existing authorized detail view. Conversation headers offer
**Search this conversation**. Filters cover conversation, sender/publisher, and
inclusive date range. Search text and filters survive navigation in memory;
results and credentials are not stored in browser storage.

This helps people find the discussion and meeting evidence behind a decision.
It makes no claim that competing products cannot do the same.

## Authorization and data lifecycle

The search route requires an existing member session and calls `role(user, org)`
before querying. Integration credentials do not gain access to this route.
Every page applies access predicates in the same SQL statement as MATCH,
ranking, and LIMIT; private content is not retrieved and filtered afterward.

Messages require the requested organization and either a channel or the caller's
DM participation. Every channel, including an archived channel, is readable by
members. The remaining participant can search a departed member's DM history,
matching existing conversation access. A caller removed from the organization
loses access. Room filters and result-opening requests recheck access.

Meetings reuse `noteScope()` for authorized collections and folder scope, plus
an explicit `n.trashed_at IS NULL` predicate. `noteScope()` alone does not exclude
trash. Grant revocation, trash, and restoration take effect at query time.
The existing Trash view can still intentionally search its authorized trash.

Deleted messages have blank bodies and do not match. Index triggers update on
insert, edit, and delete. FTS removal is logical index removal, not a promise of
forensic erasure from SQLite pages, WAL files, or backups. Existing retention and
backup policies still apply.

The UI clears results when filters change, on failed requests, and before focus
or periodic refresh. Responses from superseded requests are ignored. Access
changes observed by the workspace invalidate results. Opening any result uses
the existing authorized detail endpoint, even if access changed after search.

## Indexes and migration

`services/team/search.sql` defines external-content FTS5 indexes:

- `chat_messages_fts(body)` over `chat_messages.rowid`.
- `notes_fts(title, summary, transcript)` over `notes.rowid`.

Both use `unicode61 remove_diacritics 2`, with insert, delete, and update
triggers. This avoids a second full content table; the inverted indexes still
consume disk space. The tokenizer does not provide stemming, semantic matching,
or fuzzy typo correction.

`initializeSearch()` creates a `team_migrations` table and applies `search-v1` in
one transaction: create indexes and triggers, rebuild existing content, then
record the migration. Failure rolls back the migration. It runs before the
server accepts requests and is skipped on subsequent starts.

Do not use `SELECT count(*) FROM ..._fts` to detect an empty index. With an
external-content table, queries without MATCH read the underlying content table;
a nonzero count can coexist with zero indexed matches. See
[SQLite's external-content pitfalls](https://www.sqlite.org/fts5.html#external_content_table_pitfalls).

Future tokenizer/index changes need a new migration and rebuild. Any maintenance
that changes content rowids also requires rebuilding the indexes.

## Query behavior and resource limits

This is **keyword search**. The interface explains that all words must match and
suggests queries such as `pricing` or `Q4 budget`, not natural-language questions.
No raw FTS5 expression or SQL fragment is accepted from the caller.

`searchExpression()` extracts Unicode letters, numbers, and combining marks,
quotes each token, joins with AND, and adds prefix matching to the final token
only when it contains at least two characters. One-character terms remain exact
matches. Short terms such as AI, UI, Q4, and v2 are retained. Quotes, apostrophes,
colons, minus signs, and FTS operators cannot change the query grammar.

Maximums: 256 input characters, 12 tokens, 50 results per group, and an offset of
10,000 per group. Empty/punctuation-only input returns empty results. Oversized
or invalid parameters return 400. Date filters accept real ISO calendar dates
and reject reversed ranges. Results near the pagination bound should be narrowed
with filters. Existing authenticated rate limits, origin checks, and no-store
response headers apply. No new dependencies, credentials, or external services
are introduced.

## Ranking and pagination

Messages use `bm25(chat_messages_fts)`, then descending creation sequence and ID
for stable ties. Meetings use `bm25(notes_fts, 5.0, 2.0, 1.0)`, then descending
meeting date and ID. Lower BM25 scores rank first. Reusing the answer scorer's
column weights does not imply its ranking will be identical.

Do not compare scores between the two indexes. Each group has its own continuation
cursor and More button. Cursors encode bounded offsets bound to the caller,
organization, query, filters, and group. They are not authorization tokens; all
permissions are checked again on every request. They contain no result content.

Offset pagination is not a snapshot: edits, new content, or access changes can
shift the ranking. The client deduplicates rows, and refresh starts a new search.
There is no guarantee of snapshot-stable pagination while the corpus changes.

## API

`GET /v1/orgs/:org/search`

Parameters: `q`, `kind=all|messages|meetings`, `limit` (default 20),
`messages_cursor`, `meetings_cursor`, `room`, `author`, `since`, `until`,
`space`, and `folder`. A room filter restricts search to messages. Sender means
message author or meeting publisher. Message dates use creation dates; meeting
dates use occurrence dates, with inclusive YYYY-MM-DD boundaries.

```ts
type SearchSnippet = { text: string; match: boolean }[];
type TeamSearchPage = {
  messages: { hits: TeamMessageSearchHit[]; cursor: string | null };
  meetings: { hits: TeamMeetingSearchHit[]; cursor: string | null };
};
```

Message hits include ID, room ID and label, author ID and name, creation time,
and snippet. Meeting hits include ID, collection ID, title, occurrence time,
and snippet. Types live in `src/teams/types.ts`.

FTS5 `snippet()` supplies a 40-token excerpt using randomized internal delimiters.
SQL also caps it at 4,096 characters, and response text is limited to 1,600
characters plus an ellipsis so unusually long tokens cannot inflate responses.
The server converts it into text/match parts. React renders text and `<mark>`
elements; the response is never injected as HTML. Matching emphasis uses theme
colors with inherited text color in light and dark mode.

The existing meeting-list search also uses `searchExpression()` and the same
weighted FTS ordering. Empty search preserves the ordinary date-ordered list.

## Opening results and browsing context

Search reuses the message-location endpoint and the navigation introduced for
mentions. History accepts one of:

- `around=<message ID>`: up to 26 messages through the target and 25 after it.
- `before=<sequence>`: existing earlier-history paging.
- `newer=<sequence>`: up to 50 later messages.
- `after=<sequence>`: existing live updates.

Directions cannot be combined. Room, thread, and message access are checked.
The response supplies `older_before` and `newer_after` so users can load context
in either direction. Jump to latest reloads the recent window. While later
history is unloaded, live updates do not splice distant new messages into the
window, and reaching its bottom does not mark the entire conversation read.

## Validation and deployment

Tests in `services/team/search.test.ts` cover:

- Private DM and cross-organization exclusion across pages; departed members.
- Meeting grants, revocation, trash, restore, and existing list search.
- Literal hostile input, short terms, diacritics, input limits, and text snippets.
- Edits, deletion, archived channels, and FTS consistency.
- Independent/filter-bound cursors, dates, author filters, and ranking.
- Backfill from an existing database, restart, and incremental updates.
- Bounded target windows, forward paging, replies, and foreign-room rejection.

The package smoke test stages exactly the Docker runtime files and starts the
service independently of the repository. The image must include `search.ts`
and `search.sql`, in addition to the mention and reaction helpers. Frontend and
service TypeScript builds and existing messaging/service tests are required.

Before production rollout, the Railway owner must back up the persistent SQLite
volume, deploy the updated server package, and verify authenticated search on
existing data. Keep the existing volume and environment configuration. Then test
an old message, a thread reply, a meeting, and permission revocation using the
updated client. Local automated checks do not establish hosted deployment or
largest-workspace migration latency. Time the first migration against a
representative copy before deploying to a large workspace.

## Deliberately deferred

Semantic reranking, natural-language answers, cross-organization search,
attachments, query-language operators, and snapshot pagination are not part of
this implementation. Add them only when real retrieval failures justify the
additional complexity. Keyword search over messages and meetings is complete
without them.
