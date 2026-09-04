# Noted team service

A separate shared workspace for explicitly published meeting copies. This is an
initial self-hostable implementation, not a deployed hosted service or a claim of
enterprise readiness. The private Noted vault never becomes this service’s data
directory.

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
key, organization name and owner name. Bootstrap works only once. Then create
one-use invitations in **Members & prompts → Members**. Share the invitation
code and server address through your organization’s approved channel.

For other Macs, configure an HTTPS reverse proxy and use its root URL in Noted.
Do not expose the HTTP listener directly to the public internet. The native
client requires HTTPS except for loopback. This repository does not provision
domains, certificates, a production host or an identity provider.

| Setting | Default | Purpose |
| --- | --- | --- |
| `NOTED_TEAM_DB` | `team.sqlite` in current directory | Separate SQLite database |
| `NOTED_TEAM_SETUP_KEY` | Required | Initial one-time owner provisioning |
| `NOTED_TEAM_HOST` | `127.0.0.1` | Listen interface |
| `NOTED_TEAM_PORT` | `8790` | Listen port |
| `NOTED_TEAM_ORIGINS` | Empty | Exact browser origins permitted, comma separated; native requests need none |

Deploy the service as one process over a local SQLite filesystem, behind TLS,
with a process manager. Back up using SQLite’s backup mechanism or stop the
service before copying the database and WAL together. Test restoration before
relying on the service. This implementation does not provide encryption at rest,
backup scheduling, high availability, SSO, SCIM, recovery email or central model
policy. The server operator is a trusted administrator with filesystem access.

## User workflow

1. Connect from **Team**, then create spaces and optional folders. A new custom
   space starts restricted. The initial **Team knowledge** space includes the
   whole workspace. Admins can read every published space.
2. In the private Library, open a completed meeting and choose **Share → Publish
   to team**. Select the audience and summary. The transcript starts unchecked.
   Review content and publish. Background local changes invalidate the preview.
3. Team members can search, select meetings, ask across folders/spaces, open
   sources, and continue private conversations from **Chat history**. Conversations save automatically; individual answers can also be bookmarked. Summaries can be edited by editors;
   concurrent stale edits are rejected. Shared edits do not alter local originals.
4. Use workspace settings to invite members, adjust roles, manage groups, share
   prompts, transfer ownership or revoke access. Removing someone preserves
   company meeting copies while blocking further access by that member.
5. Use **+ Workspace** to create/join another organization on the same server.
   One native server connection is active at a time.

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

An administrator first enables **Allow approved workspace integrations** in a
space’s access settings. Then **Integrations → Create read-only key** chooses
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
