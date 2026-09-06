# Working together on Noted

Git only shares work after it has been committed and pushed. A file changed on
one person's Mac is invisible to everyone else until then.

## Everyday flow

1. Fetch before starting and create a branch for one focused change.
2. Commit meaningful checkpoints and push the branch regularly.
3. Open a pull request early, as a draft if the work is not ready.
4. Request the other active collaborator as a reviewer. That request is the
   reliable GitHub notification that new work is available.
5. Resolve review and automated checks, then merge into `master`.
6. The other person can use **Fetch origin** and **Pull origin** in GitHub
   Desktop to receive the merged change.

Do not share unfinished source changes through an installed app build. App
updates replace the signed application bundle; they do not merge Git branches
or touch either collaborator's working checkout.

## Avoiding collisions

- Keep unrelated work on separate branches or Git worktrees.
- Put database migrations and backend commands in their own clearly described
  commits. Backend commands must remain mirrored in the desktop registry and
  phone bridge as documented in `AGENTS.md`.
- Before merging, update the branch from `master` and run `bun run build`, the
  relevant Rust tests, and any feature-specific checks named in the pull request.
- Never commit credentials, updater signing keys, or repository-local
  `.codex/` state.

## Tester builds

Installed beta builds update through published GitHub Releases. A draft release
is safe to test manually, but it does not become visible to the in-app updater
until it is published. See `docs/BETA_DISTRIBUTION.md` for the release checklist.
