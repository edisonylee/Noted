# Noted neon: current-master product verification

Date: 2026-09-05 (America/Los_Angeles)

## Corrected baseline

The first installation was mistakenly built from `iphone-companion-foundation` and omitted features already on master. The original visual checks were insufficient: they did not verify the current product feature set. That rollout result is superseded.

Recovery uses a separate checkout at `/Users/edison/Noted-neon-current`, based on freshly fetched `origin/master`, commit `267a57b13f15cb0d44daf5c26f01ab7b9cda2144`. The complete master product is retained. Only theme registration/runtime, Appearance, brand markup, editor default colors, scoped product CSS, design documentation, and installation safeguards are changed.

Verified unchanged against master: Team components and service, desktop Team/notification commands, Documents/Library implementation, calendar, settings, knowledge graph, recording/capture, database, desktop registration and phone bridge. Existing uncommitted work in `/Users/edison/Noted` remains preserved there; it was not substituted for newer master code.

## Installed verification

- Frontend typecheck and production build passed; the generated assets contain the Team workspace and profile bundles.
- Standard native release build passed, using the existing Rust compilation cache. The fresh signed bundle was installed at `/Applications/noted.app` after moving the stale bundle out of the way. Deep signature verification passed.
- 23 neon semantic/runtime tests passed (383 assertions).
- 77 Team, messaging, service, library, and meeting-markdown tests passed (601 assertions), including attachments, private saved messages, pinned messages, unread markers, sharing, search, notifications and authorization boundaries.
- The existing local team service health endpoint returned healthy.
- Native app navigation includes Home, Schedule, Calendar, Documents, Library, Team, and Knowledge.
- Team opened the existing workspace, including direct messages, channels, actual message history, Mentions, Saved messages, Pinned, attachment and thread controls. No messages were sent or edited during verification.
- Documents opened the separate file/folder view with the existing document. Library displayed the existing 77 items and folder organization.
- Citron remains active. Light and dark modes were seen on the corrected installation; final Team view was left in the user's dark mode.

## Design scope

The black identity rail, Soft Cut wordmark, monochrome surfaces, Geist hierarchy and citron accents are applied to the current product. Team layouts and service behavior remain intact; primary actions, selected navigation, and badges use the shared accent roles. Four interchangeable neon palettes and all existing theme packs remain available in Appearance. The separate native iOS companion is outside this Mac rollout.

The earlier palette checks established all four previews, persistent Apply, Cancel restoration, and existing-theme compatibility. Current-master screenshots were freshly inspected for Team, Documents, and Library. This is adaptation of the selected design language, not a pixel match to a fictional reference screen.

## Preventing another rollback

Both standard installation and automatic update scripts check that the checkout contains locally fetched `origin/master`. An older branch cannot replace the installed app automatically. The check was exercised: the original branch's automatic update skipped, its direct installer refused before building, and the current integration checkout passed the ancestry check. Fetch before an intentional install so this ref is current.

## Evidence and limits

Private local screenshots remain outside the repository:

- `/tmp/noted-neon-qa/master-team-restored.png`
- `/tmp/noted-neon-qa/master-documents-restored.png`
- `/tmp/noted-neon-qa/master-library-restored.png`

The app was inspected with real existing content. No live recording, calendar write, external message send, or model generation was performed. The source and service test evidence does not claim every possible UI interaction was manually exercised. The raster wordmark remains concept artwork pending final vector logo work. Existing bundle-size warnings remain.

Final result: passed for current-master feature preservation, installed restoration, and the listed design checks.


## Documents writing index — 2026-09-06

Removed the Documents folder rail and its reveal/collapse control. Documents now always lists the current space's documents, even if an older session retained an inbox, folder, or trash selection. Search, sorting, and New document remain. Work/Personal selection moves into a compact header select. The existing document editor retains its location and Move to controls. Library retains its folders, saved views, and Trash.

Verification: frontend typecheck, production bundle, standard native build, installed signature check, and diff check passed. The refreshed origin/master remains an ancestor; Team components and service still match master. Native screenshots verified Documents without a folder rail, the Work/Personal menu options, search with no results and clearing the query, opening an existing document and returning, and Library with its original folders and 77 items. No document contents were changed. The existing navigation/dock work in the checkout was preserved.

Evidence (private local screenshots): `/tmp/noted-neon-qa/documents-focused.png`, `/tmp/noted-neon-qa/documents-focused-editor.png`, `/tmp/noted-neon-qa/library-after-documents-focus.png`. The document index and editor were inspected at both the larger window and 920 × 760 native size.


## Combined product integration — 2026-09-06

Integrated the finished companion, Aperture app icon, floating dock, Documents simplification, neon themes, and updater controls on the current master baseline. The old bottom assistant bar is removed. Database export now lives in System Settings and retains the native save-location picker. Existing Team components, service, and database implementation remain unchanged from master.

Validation: frontend production build passed; 106 Bun tests passed (1,021 assertions) across neon semantics, companion motion/preferences, Team messaging, and the Team service. Both updater configuration tests, all three native companion tests, and all seven deterministic section tests passed. The standard macOS release build passed and its fresh bundle was installed at `/Applications/noted.app`; deep signature verification passed. The installed icon matches the new source artwork.

Native checks confirmed the seven dock destinations, pet chat launcher, ten-pet customization panel, detached desktop pet, and return to the main app. Documents shows a compact space selector and document list without the folder rail. Library retains its folders and 77 existing items. Team loads the existing workspace, shared meeting, messaging, and People controls. The user's saved electric-blue dark theme remains in place; citron is the default for new installations. No messages, recordings, document edits, or calendar writes were made during these checks. Existing bundle-size and unrelated Rust dead-code warnings remain.
