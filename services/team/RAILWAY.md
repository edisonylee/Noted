# Deploy the team service to Railway

The Docker image runs the same team API as the local service. It contains only
the server, store and schema; it does not contain the private Noted vault, app
credentials, recordings, local databases or example meetings.

## Account and trial

Sign in to Railway and confirm the workspace is on its free trial before
creating the service. The trial currently grants $5 for up to 30 days. It does
not guarantee a month of uninterrupted service: available time depends on
resource usage. Do not add a paid subscription as part of trial setup.

Trial volume data has a limited retention period after credit expiry. Export a
database backup before the trial ends. A backup on the same hosting account
does not replace an independent copy.

References: [trial](https://docs.railway.com/pricing/free-trial),
[volumes](https://docs.railway.com/volumes),
[backups](https://docs.railway.com/volumes/backups),
[HTTPS](https://docs.railway.com/networking/public-networking).

## Service settings

Create a project named **Noted Team** and an empty service named **noted-team**.
Use one service instance and one attached volume mounted at **/data**. The
SQLite database must remain on this volume across deployments. Do not add a
separate database service or additional replicas.

| Setting | Value |
| --- | --- |
| Build source | This directory's Dockerfile and its three source files |
| Database | `/data/team.sqlite` (image default) |
| Listener | `0.0.0.0` (image default) |
| Port | Railway's `PORT`, or explicitly set `NOTED_TEAM_PORT=8790` |
| Health check | `/health` |
| Public networking | Railway-generated HTTPS domain, target the configured port |
| Restart policy | On failure, within the trial's supported limits |
| Persistent volume | `/data` |

Keep browser origins unset for the native Mac app. Set a new cryptographically
random `NOTED_TEAM_SETUP_KEY` of at least 32 characters as a service secret
before the first start. Do not store it in source, a Docker build argument or
an image layer. After the owner is provisioned, remove the variable and
redeploy; the existing database starts with bootstrap disabled.

The current Railway CLI supports `variable set NOTED_TEAM_SETUP_KEY --stdin`
so the secret need not appear in command history. Use the CLI's own authenticated
session, never browser cookies. CLI authentication may need user interaction.

For a local upload, stage only these five files in a clean directory outside
the repository: `Dockerfile`, `.dockerignore`, `server.ts`, `store.ts`, and
`schema.sql`. Link that directory to the intended project/service, then run
`railway up . --path-as-root --detach`. This avoids uploading unrelated source
or development state. Keep the project/service IDs in local deployment records.

The service defaults to HTTP inside the container. Railway terminates HTTPS.
Use the generated HTTPS root URL in Noted; no custom domain is required.

## Connect and invite

In Noted's Team connection screen, enter the HTTPS URL and choose **Set up a
new team server**. Supply the setup key, workspace name and owner name. The
native app stores its account session in macOS Keychain.

Then open **Members & prompts → Members → Invite teammate**. Give each member
their own one-use invitation code and the HTTPS server address. Each needs a
Mac build of Noted containing this branch's Team feature. In the connection
screen they choose **Join with an invitation**. Codes expire after seven days.

Owner sessions, invitations and shared content persist in the volume. Existing
local workspaces are separate; changing the URL does not upload or migrate them.
To move an existing workspace, use a consistent SQLite backup and intentionally
restore it into the hosted volume before initializing a different database.

Account sessions currently expire after 30 days. A signed-in account can create
another account access key before expiry. There is no email/SSO recovery flow
yet, so trial hosting is not a claim that everyday organizational onboarding is
complete. Keep the local service and its data until hosted access is verified.

## Verify before sharing the address

1. Confirm `/health` succeeds over HTTPS and anonymous workspace reads fail.
2. Create a temporary test invitation and verify a separate account can join
   once, read a fictional shared meeting, and cannot access restricted spaces.
3. Replace the deployment and verify the same accounts, invitation state and
   meeting remain available.
4. Remove the test member and verify its next request fails. Remove the test
   data before publishing real meeting copies.
5. Verify bootstrap is disabled after removing its secret. Enable backups if
   available on the trial, and test restoring a consistent database export.
6. Connect the installed Noted app and check the actual Team screen.

The Docker image has been built locally and checked over HTTP for assigned-port
binding, bootstrap, one-use invitations, member reads, anonymous denial,
replacement with the same persistent volume, disabled bootstrap and membership
revocation. Hosted verification must still be performed on the deployed URL.
