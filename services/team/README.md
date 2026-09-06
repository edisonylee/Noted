# Noted team service

A separate shared workspace for explicitly published meeting copies, channels,
and direct messages. It can run locally or on a hosted server such as Railway.
The private Noted vault never becomes this service’s data directory. This is an
initial self-hostable implementation, not a claim of enterprise readiness.

## Persistent workspace on your Mac

For a workspace that remains available after the terminal closes or the Mac
restarts, run this from the repository root:

```sh
bun run team:local --owner "Your name" --workspace "Your workspace" --examples
```

This creates a persistent database in `~/Library/Application Support/Noted Team`,
installs a user LaunchAgent named `com.noted.team`, and connects the standard
Noted app to `http://127.0.0.1:8790`. The service listens only on this Mac. It is
not a hosted service that coworkers can reach. Bun must remain installed at the
path recorded in the LaunchAgent.

Open Home, then Team in an already-running Noted app to refresh its connection.
`--examples` adds a separate **Noted examples** workspace with clearly fictional
meetings; it never imports or publishes private notes, recordings, or calendar
data. Your named workspace starts empty. Re-running the command preserves both
workspaces and existing content. It refuses to replace a connection to another
server or take ownership of a transferred workspace.

The runtime is copied outside the checkout, so changing branches or closing this
task does not stop it. Credentials are saved in macOS Keychain; the LaunchAgent
and connection file contain no account token or setup key. The initial setup key
is discarded after provisioning, and bootstrap is disabled on subsequent starts.
Account sessions expire after 30 days; rerunning `team:local` renews the local
owner's connection. This is a development installation, not enterprise hosting.

To stop it without deleting any workspace data:

```sh
launchctl bootout gui/$(id -u)/com.noted.team
```

To start it again, rerun the setup command. Back up `team.sqlite` with SQLite's
backup API while running, or stop the service before copying its database/WAL.
This installer does not schedule backups or provide remote access.

## Try it locally

From the repository root, with Bun installed:

```sh
bun install
bun run team:demo
```

Open the printed preview URL, choose **Sign in with an access key**, and use the
printed owner or member key. The sample organization lives only in memory.
Restarting removes all sample changes and invalidates those keys. The preview
uses real requests and authorization; its Ask action demonstrates retrieval,
not model-generated answers. Native Noted uses its configured provider to answer.

## Run a persistent service

Use a dedicated machine/account and a writable data directory outside the repo.
Set `NOTED_TEAM_SETUP_KEY` to a cryptographically random secret of at least 32
characters in the service manager’s secret configuration. Do not commit it.

```sh
NOTED_TEAM_DB=/absolute/private/path/team.sqlite bun run team:serve
```

The default listener is `127.0.0.1:8790`. In Noted’s **Team** view, use
`http://127.0.0.1:8790`, choose **Set up a new team server**, and enter the setup
key, organization name and owner name. Bootstrap works only once. An initialized service can restart without the setup key; omitting it disables bootstrap. Then create
one-use invitations in **Team settings → Members**. Share the invitation
code and server address through your organization’s approved channel.

For other Macs, configure an HTTPS reverse proxy and use its root URL in Noted.
Do not expose the HTTP listener directly to the public internet. The native
client requires HTTPS except for loopback. This repository does not provision
domains, certificates, a production host or an identity provider.

| Setting | Default | Purpose |
| --- | --- | --- |
| `NOTED_TEAM_DB` | `team.sqlite` in current directory | Separate SQLite database |
| `NOTED_TEAM_SETUP_KEY` | Required for a new database | Initial one-time owner provisioning; omit after setup to disable bootstrap |
| `NOTED_TEAM_HOST` | `127.0.0.1` | Listen interface |
| `NOTED_TEAM_PORT` | `PORT`, otherwise `8790` | Listen port; supports hosting providers' assigned port |
| `NOTED_TEAM_ORIGINS` | Empty | Exact browser origins permitted, comma separated; native requests need none |

Deploy the service as one process over a local SQLite filesystem, behind TLS,
with a process manager. Back up using SQLite’s backup mechanism or stop the
service before copying the database and WAL together. Test restoration before
relying on the service. This implementation does not provide encryption at rest,
backup scheduling, high availability, SSO, SCIM, recovery email or central model
policy. The server operator is a trusted administrator with filesystem access.

## User workflow

For the container deployment and free-trial setup, see [Railway deployment](RAILWAY.md).

1. Connect from **Team**. **Meetings**, **Messages**, and **People** remain visible
   as the three main destinations. A team represents one organization and its
   members; admins can change its name in **Team settings**.
2. **Meetings** brings together explicitly shared notes from everyone with access.
   **Collections** group meetings by project, client, or topic. **General meetings**
   includes all team members; custom collections start restricted. Folders inherit
   their collection's permissions. Admins can read every published collection.
3. Choose **Share a meeting** for the publishing steps. In your private Library,
   open a completed meeting and choose **Share → Publish to team**. Select a
   collection and summary, review the audience and content, and publish. The
   transcript starts unchecked. Background local changes invalidate the preview.
4. Search or select meeting notes and use **Ask meetings** for source-grounded
   answers. **Previous questions** resumes your private question history. Answers
   can also be bookmarked. Editors can update shared summaries; stale edits are
   rejected, and shared edits do not alter private originals.
5. **People** lists teammates and opens private conversations. **Invite & manage**
   opens settings for invitations, roles, groups, collections, prompts, and
   integrations. Removing a member preserves published company knowledge while
   blocking that member's further access.
6. The menu beside the team name includes **Create or join a team**. Teams on the
   same server have separate membership and content. One native server connection
   is active at a time.

The former **Spaces** label is now **Collections**. Existing IDs, grants, folders,
and API paths are unchanged. The original uncustomized **Team knowledge** starter
collection displays as **General meetings** without rewriting its stored name.

## Team chat

Open **Team → Messages** for conversations with people. **Team chat** includes
all current team members and stays available. It is backed by the existing
`general` channel, so earlier history is preserved. Additional **Team channels**
are optional conversations for projects or topics; all team members can read and
send in them. A creator or team admin can rename, describe, archive, or restore a
custom channel. Archiving preserves history and pauses sending.

Choose **New message**, then a teammate, or **People → Message**, for a private
two-person conversation. Reopening the same pair returns the same history. Every
request checks membership and participation: even team owners and admins cannot
open someone else's DM through the API. If a participant leaves, the remaining
member retains read-only history. The server operator has database access;
messages are not end-to-end encrypted.

The conversation list includes latest-message previews, unread counts, and
search by conversation name. Drafts survive switching rooms and moving between
Meetings, Messages, and People within Team; they are not persisted across app
restarts or leaving the Team area. Chat supports earlier history, editing your
own messages, and deletion. Channel admins can delete channel messages. Messages
are plain text, limited to 10,000 characters. Failed sends can be retried with the
same request identifier without creating a second copy. Edits reject stale
revisions, and deletion removes the body while leaving a visible tombstone.
Backups may retain earlier content.

While Messages is visible, the open conversation receives live updates through
an authenticated waiting request. The conversation list refreshes every ten
seconds while Team is visible, keeping
the navigation unread badge up to date. Focus triggers an immediate refresh.
Read markers sync across devices when the conversation is viewed at the bottom;
hidden conversations are not marked read. This release does not include group
DMs, typing indicators, attachments, message-body search, or push notifications. Messages are not included in meeting search, model prompts,
read-only integration keys, or MCP responses.

The service database gains additive `chat_*` tables on startup and a general
channel for each existing team. The navigation refresh requires no additional
schema migration; the later thread/profile release adds the fields described below. Conversation previews are limited to 160 characters and are
returned only after the room's membership/participant checks. Older service
versions without previews still render the conversation list. Use the normal
service backup and deployment process; no separate chat server or model
subscription is needed.

## Threads, reactions, and profiles

Hover or focus a message and choose **Reply in thread**. Replies stay attached to
that message in a separate panel in channels and DMs. The parent shows a reply
count; reopening it restores the discussion. A compact window shows the thread
in the conversation area, while wide windows show it alongside the main chat.
**Escape** or **Close thread** returns to the conversation. Unsent reply drafts
stay in memory while Team remains open. Replies can be edited, deleted, and reacted
to under the same rules as other messages. Deleted parents retain existing reply
history but cannot receive new replies. Nested replies use the original thread.

Choose **Add reaction** for a searchable set of 64 emoji. Click a reaction pill to
add yours, or click one you already added to remove it. Counts and names are
visible to conversation participants; reactions on private messages do not grant
any new access. Repeated requests set the desired state without double-counting.

Open **Settings → Profile** to edit your display name, job title, bio, and
photo. **Team settings → Profile** opens the same editor and account. Your profile
is shared across your teams on the connected server. Click a
person's avatar or message author to view it. Photos are cropped and resized on
the device before upload; the server accepts only bounded JPEG/PNG image data.
It stores photos in the team database and serves them through member-authenticated
requests, rather than public image URLs. Removing a photo restores initials.
Avatar requests are deduplicated within the team view. Name changes preserve the
account ID, memberships, message authorship, and DM history.

The open conversation now uses authenticated HTTP long polling: after the initial
history request, a request waits for up to 20 seconds and wakes when a message,
reply, reaction, edit, or deletion changes. Authorization is checked again before
returning data, including after membership removal or sign-out. A client using an
older server falls back to the previous polling interval and does not expose
unsupported thread/reaction actions. There is no required WebSocket endpoint or
separate chat service. Network transit time still applies.

Scrolling runs immediately after React commits new messages, before paint. The
view follows the latest message when already at the bottom and after sending;
reading earlier history keeps its position and exposes a **New messages** button.
The conversation list and member directory retain their existing periodic refresh.

Database changes are additive: a nullable thread-parent column, a thread-history
index, reaction records, and profile records. Back up before deploying. The
container and local installer include the shared reaction catalog.

## Data and authority boundaries

- Published fields: title, selected summary, meeting date, optional formatted
  transcript, destination/folders and a random local publication identifier.
  Audio/video, My Notes, event JSON, local paths and attendee metadata are excluded.
  A summary itself can contain private-note information; the preview explains this.
- Every direct read, search, context request and mutation checks organization
  membership and space grants. UI visibility is not the authorization mechanism.
- Explicit viewer grants narrow default team-space editor access. When multiple
  explicit grants apply, editor wins. Removing a grant from a team-visible space
  restores its default editor access; restrict the space to remove default access.
- Invitations last seven days and work once. They identify the recipient by
  possession, not verified email. Sessions last 30 days; only token hashes are
  stored server-side. Native sessions are stored in macOS Keychain, not the vault.
- An access key for another Mac is a full account session, **not** an integration
  or agent key. Membership changes still apply to it on every request.
- Answers send authorized excerpts to the native app’s selected model provider.
  Local mode uses the local model. Hosted/BYOK behavior follows existing provider
  settings; there is currently no workspace policy to constrain that selection.
- Conversations retain their original workspace/folder/selection scope. Each supports 20 answers, with up to six recent turns supplied to the model. Source revisions and access are rechecked before generation, after generation and transactionally when appending. Concurrent updates from another device are rejected. History is paginated; deleting a conversation keeps shared meetings and separately saved answers. Changed, trashed, moved-out-of-scope or inaccessible evidence blocks reopening or continuing that conversation.
- Conversations and saved answers belong to the querying account; other members and workspace
  admins cannot read them through the API. The server operator can inspect its
  database. Opening an answer rechecks every source and its revision. Changed,
  trashed or inaccessible evidence hides the answer and its question in the list.
- The client checks access on focus and every 30 seconds while visible, and again
  after model generation. Revocation cannot erase content already copied/exported.
- Sign out revokes the current session when online and removes local credentials.
  If offline, the server session remains valid until expiry. Member removal is the
  admin’s immediate workspace-level revocation mechanism.
- This API does not revive the retired phone web transport or grant external
  agents access to personal context. Desktop publication is a dedicated command.

## Checks

```sh
bun run team:test
bun test tests/team-messaging.test.ts
bun install --cwd services/team
bun run --cwd services/team check
bun run build
cd src-tauri
cargo test --lib teams::tests --offline
cargo test --lib meeting::detect::tests --offline
```

The service uses [Bun’s SQLite API](https://bun.sh/docs/runtime/sqlite) and
[HTTP server](https://bun.sh/docs/runtime/http/server). It adds no runtime package
dependency. Service-only development dependencies have their own lockfile.
See [the parity audit](../../docs/GRANOLA_PARITY.md) for explicit remaining work.

## Read-only API and MCP

An administrator first enables **Allow approved team integrations to read this collection** in a
collection’s access settings. Then **Integrations → Create read-only key** chooses
specific spaces, an expiry, and whether transcripts are included. Both approvals
are required. Turning space access off blocks all keys for that space immediately;
revoking one key affects only that integration. A workspace key survives its
creator leaving, so offboarding should also review the workspace’s integrations.
The publication preview discloses when a destination permits integrations.

Use `Authorization: Bearer <integration-key>` with these GET endpoints:

- `/v1/api/spaces`: approved spaces only.
- `/v1/api/folders`: folders in those spaces.
- `/v1/api/notes?q=phrase&space=id&folder=id&offset=0`: metadata and summary
  excerpts, with `next_offset` pagination. Folder scope includes descendants.
  Without transcript permission, transcript-only matches are excluded.
- `/v1/api/notes/{id}`: a published summary and, if separately granted, transcript.
  Trashed or out-of-scope meetings return 404. Member/session/admin routes reject
  integration keys; integration routes reject writes and member session keys.

For a client that supports local stdio MCP servers, configure Bun to run the
absolute path to `services/team/mcp.ts`, with `NOTED_TEAM_SERVER` and
`NOTED_TEAM_API_KEY` provided through the client’s protected environment/secret
configuration. Keep the key out of tracked configuration. This adapter exposes
`list_team_spaces`, `search_team_meetings` and `get_team_meeting`. Source reads
are bounded to 15,000 characters per passage with a continuation offset.

The adapter follows the official [stdio transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
and [tool contract](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).
It does not install itself in any client, access the local vault, use a member’s
Keychain session, or expose write tools. Hosted OAuth MCP and webhook delivery
are not implemented.
