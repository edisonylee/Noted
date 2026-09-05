# Granola functionality audit and Noted implementation

Research date: September 4, 2026. Branch: `total-granola-note-parity`.

“Team” means an organization’s shared knowledge and access controls, not
Microsoft Teams. This audit uses Granola’s current official help center. Vendor
documentation is evidence of advertised behavior, not an independent quality
benchmark. The older competitive landscape remains useful context; this document
records newer findings without relabeling planned capabilities as shipped.

## Product decision

Noted keeps its private local library, richer personal context, multiple summary
formats, native capture, and explicit agent permissions. Organizational knowledge
is an additional, separately authenticated service. Local folder names are not
authorization boundaries, so the existing Work/Personal folders cannot stand in
for real team membership.

The request authorizes organizational features beyond the earlier team-admin
non-goal in `PRODUCT_STRATEGY.md`. This branch implements that expansion without
changing local mode into an account-required product. A shared meeting is a
reviewed copy. Publishing never grants access to the publisher’s entire vault.

## Organizational functionality

“Implemented” below means present on this branch. The standard native app is
installed locally, with a persistent service on this Mac and an automatic
connection. Coworkers on different Macs still require a hosted deployment.
See [the service runbook](../services/team/README.md).

| Capability | Current Granola evidence | Noted on this branch | Remaining difference |
| --- | --- | --- | --- |
| Workspaces and membership | [Workspaces](https://docs.granola.ai/help-center/workspaces): separate memberships, invitations, admins, workspace switching | Implemented: multiple organizations per account, invitation codes, owner/admin/member roles, ownership transfer, removal and leave | Google/Microsoft sign-in, email-verified invitations, SSO, billing and domain membership policies remain open |
| Private by default | [Sharing controls](https://docs.granola.ai/help-center/consent-security-privacy/sharing-controls) | Local vault remains private. Publication requires a destination and preview; transcript sharing is opt-in | Organization-wide default sharing policies and approved automatic publication rules remain open |
| Shared spaces and folders | [Spaces & Folders](https://docs.granola.ai/help-center/sharing/folders/spaces-and-folders): spaces, subfolders, multi-folder notes, permissions and recurring filing | Implemented: team-visible/restricted spaces, nested folders, multi-folder shared notes, folder editing and moving, viewer/editor grants | Grants currently apply at space level; folder-specific overrides, recurring filing, cross-workspace transfer and deletion of folders/spaces remain open |
| User groups | [User Groups](https://docs.granola.ai/help-center/sharing/user-groups): group-based sharing and directory synchronization | Implemented: manual groups, membership updates, group grants, immediate authorization changes | Directory sync/SCIM and identity-provider groups remain open |
| Shared company knowledge | [Workspaces](https://docs.granola.ai/help-center/workspaces) | Removing a member revokes their access while preserving their published company knowledge | Offboarding export and retention policy administration remain open |
| Shared note editing | [Sharing notes](https://docs.granola.ai/help-center/sharing/sharing-notes) | Implemented: editable shared summaries, immutable source transcript, optimistic revisions, copy Markdown, Trash/restore | Concurrent live cursors, granular note grants and public web links remain open |
| Shared recipes and templates | [Recipes](https://docs.granola.ai/help-center/getting-more-from-your-notes/recipes), [Templates](https://docs.granola.ai/help-center/taking-notes/customise-notes-with-templates) | Implemented: workspace prompts, author/admin editing, revision checks, recipe shortcuts; shared templates can be installed into local summary formats | Automatic propagation to installed templates and recipe attachment inputs remain open |
| Ask across team knowledge | [Chat](https://docs.granola.ai/help-center/getting-more-from-your-notes/chatting-with-your-meetings) | Implemented: selected-meeting, folder, space and workspace scopes; transcript search, evidence links, configured local/hosted/BYOK model; all authorized history participates in ranking; follow-up conversations retain scope and source mappings | Retrieval uses keyword ranking and bounded excerpts. Each conversation supports 20 answers with up to six recent turns and a bounded model input budget; attachments and per-chat model choice remain open |
| Private answer history | [Chat](https://docs.granola.ai/help-center/getting-more-from-your-notes/chatting-with-your-meetings) | Implemented: automatic private conversation history with pagination and resume/delete; individual answers can also be bookmarked. Reopening or continuing rechecks all source revisions and permissions | Conversations with changed or unavailable evidence must be restarted; bookmarks currently show the latest 100 |
| Administration | [Workspaces](https://docs.granola.ai/help-center/workspaces), [Security FAQs](https://docs.granola.ai/help-center/consent-security-privacy/security-privacy-data-faqs) | Implemented: member roles, grants, ownership, invitation revocation and metadata-only activity | SSO/SCIM, centrally enforced capture settings, retention, audit export, account recovery and compliance certifications are not implemented |

## The rest of the product surface

These findings prevent “team parity” from being mistaken for complete Granola
parity. They also distinguish functionality already present in Noted from new
work required after the organizational foundation.

| Area | Evidence and comparison | Status / next acceptance gate |
| --- | --- | --- |
| Bot-free recording | [Transcription](https://docs.granola.ai/help-center/taking-notes/transcription) | Existing Noted native system/mic capture, live notes, pause and recovery need a real-call benchmark. This branch fixes early auto-stop when a muted call still has remote speech, and a stop-state race. No real meeting was recorded for QA. |
| Notes and summaries | [Writing notes](https://docs.granola.ai/help-center/taking-notes/taking-notes-in-granola), [Enhanced notes](https://docs.granola.ai/help-center/taking-notes/ai-enhanced-notes) | Existing rich notes, transcript-grounded summaries, templates and multiple summary tabs. Preserve these. Benchmark missed decisions and action ownership with consented fixtures rather than claiming model equivalence. |
| Speaker attribution | [Speaker tags](https://docs.granola.ai/help-center/taking-notes/speaker-attribution) | Existing Noted speaker attribution and correction remain. Platform-specific automatic names and attribution accuracy require separate verification. |
| Calendar context | [Calendar sync](https://docs.granola.ai/help-center/getting-started/syncing-your-calendars) | Existing Google and native calendar paths. Direct Outlook account integration and calendar parity need an explicit audit; the user’s team request does not mean a Microsoft Teams integration request. |
| Pre-meeting preparation | [Pre-meeting briefs](https://docs.granola.ai/help-center/taking-notes/pre-meeting-briefs): overnight external-meeting briefs can combine previous/shared notes, calendar, web and opt-in Gmail | Noted has calendar/personal context foundations, but this complete automatic briefing workflow is not implemented by the team work. Acceptance requires grounded sources, access checks, useful empty states and no speculative filler. |
| Follow-up email | [Follow-up emails](https://docs.granola.ai/help-center/taking-notes/follow-up-emails): eligible Google-connected meetings can receive editable Gmail follow-up drafts | Noted can generate follow-up text through prompts. Gmail connection, eligibility, reviewed delivery, attachments and undo are separate missing workflows. No mail was sent during this task. |
| Chat actions | The current [documentation index](https://docs.granola.ai/llms.txt) lists reviewed email, Slack and calendar actions | Not implemented by team chat. Research of the detailed workflow page was unavailable in this session, so exact behavior is not treated as verified. |
| Integrations | [Integrations](https://docs.granola.ai/help-center/sharing/integrations/integrations-with-granola): Slack, Notion, Zapier and CRM connectors | Markdown copying is present. Native OAuth integrations, retryable delivery, status history and automatic folder destinations remain open. A generic HTTP endpoint is not a finished CRM integration. |
| API and MCP | [Granola API](https://docs.granola.ai/help-center/sharing/integrations/granola-api), [MCP](https://docs.granola.ai/help-center/sharing/integrations/mcp) | Noted already has explicit local agent-access foundations. Implemented: separate read-only workspace integration keys, explicit space approval, expiry/revocation, independent transcript scope, HTTP API and a stdio MCP adapter. Keys do not grant member sessions, personal vault access or writes. Remote OAuth MCP, webhooks and packaged client setup remain open. |
| People and companies | [People and Companies](https://docs.granola.ai/help-center/people-and-companies) | Existing Noted people/knowledge graph remains local. A permission-filtered shared directory and company histories are not yet implemented. |
| Import/export | [Historical export](https://docs.granola.ai/help-center/sharing/exporting-notes), [Workspace transfer](https://docs.granola.ai/help-center/transfer-notes-between-workspaces) | Existing local exports and new shared Markdown copy. Granola migration, bulk organizational export and audited transfer remain open. |
| Mobile and Watch | [Mobile](https://docs.granola.ai/help-center/ios/getting-started), [Apple Watch](https://docs.granola.ai/help-center/ios/apple-watch) | Native iPhone companion work exists on the original checkout, including uncommitted work not imported into this branch. Organizational mobile sync, Android and Watch parity are not claimed. |
| Privacy and transparency | [Security standards](https://docs.granola.ai/help-center/consent-security-privacy/our-security-standards), [Transparency](https://docs.granola.ai/help-center/consent-security-privacy/transparency-solutions/introduction), [Retention](https://docs.granola.ai/help-center/consent-security-privacy/transcript-auto-deletion) | Local capture controls remain. Team-server operators can read their database; no end-to-end encryption or certification is claimed. Retention policies, centrally managed notices and enterprise deployment remain separate work. |

## Verification record

- Synthetic organizational fixtures cover cross-workspace isolation, restricted
  reads/search/context, member removal, viewer grants, groups, invitation expiry
  and replay, ownership, session revocation, stale summary edits, nested folders,
  older-meeting retrieval, shared prompts and private saved answers.
- Native tests cover server-origin restrictions, opt-in transcript publication,
  private-field exclusion and exact publication-preview matching.
- Production frontend compilation, service typechecking and all 335 active Rust unit tests pass (three manual/live tests remain ignored). The service suite has 29 tests with 187 assertions, including restart persistence, read-only integration scopes, MCP through a real HTTP listener and stdio process, private conversation continuity, source revocation, stale appends, fixed scope, bounded model history and history pagination.
- The dedicated browser preview uses the real local HTTP service and synthetic
  meetings. Browser checks cover organizational access changes, nested folders, scoped retrieval, follow-up history, saved answers, source jumps, summary edits, integration keys, responsive layouts and keyboard focus. It verifies retrieval and permissioned workflows, while generated
  answers use the configured model only in the native app.
- The final native-browser deletion prompt stalled the preview automation. Chat deletion now uses an in-app confirmation dialog; its replacement compiles, but its final click-through was not verified in that stalled browser session. Backend conversation deletion and cascades are covered by tests.
- The installed native app was checked end to end with a separate fictional workspace: automatic connection, workspace switching, a cited answer and follow-up using the configured model, saving the answer, opening its source, and jumping to a transcript timestamp. A forced service restart preserved both workspaces, three examples, two chat turns, the saved answer, and the authenticated connection. This is a small functional check, not a broad model-quality benchmark.
- Local setup now installs a persistent service through a user LaunchAgent, stores the account session in Keychain, and creates a separate optional example workspace. The deployed runtime is independent of the source checkout; bootstrap is disabled after provisioning.
- Real organizational deployment, identity-provider integration, production
  operations, native end-to-end publication and live-model answer quality remain
  unverified. Complete Granola functionality parity has **not** been achieved.

The isolated worktree starts from the current committed Noted state; the user’s
uncommitted work in the original checkout is preserved and must be reconciled
before integrating this branch.
