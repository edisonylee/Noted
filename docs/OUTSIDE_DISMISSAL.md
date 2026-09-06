# Outside-click dismissal

Transient dialogs and action menus dismiss when a primary mouse click or touch
lands outside their content. Clicking the trigger still toggles its menu, and
interacting with content keeps the surface open. Native select menus retain
platform behavior; inline expandable sections (transcripts, filters, and toolbar
rows) remain expanded until explicitly collapsed.

`src/ui/useDismissal.ts` provides two shared behaviors:

- `useBackdropDismiss` requires both the pointer press and the click to start/end
  on the backdrop. Native dialogs additionally check coordinates because their
  backdrop events target the dialog itself, just like clicks on dialog padding.
  This prevents accidental dismissal when selecting text or dragging an image
  from content onto the backdrop. Busy guards and existing close callbacks apply.
- `useOutsideDismiss` includes the trigger and popup in its boundary, supports
  mouse/touch/pen, closes the top registered menu on Escape, and restores focus
  after keyboard dismissal. Outside clicks retain focus on the clicked control.
  It ignores unrelated native modals so interacting with a modal does not dismiss
  the popup beneath it.

Applied to shared Team dialogs (including attachment previews and message actions),
Team options, meeting Share/Add summary menus, recording options, floating chat,
navigation, Settings, phone pairing, Calendar forms, and agent consent. Existing
Library/Today/Calendar outside handlers now use pointer events. Weather and mobile
sheets already have outside dismissal. Outside cancellation of an agent consent
request denies it; it never grants access. Active recording and busy operations
retain their existing in-progress safeguards.

Validation: production frontend build, dialog-boundary regression test, and a
synthetic browser fixture exercising outside click/tap, inside/padding clicks,
content-to-backdrop drag, busy states, nested dialogs, trigger toggles, Escape,
and focus restoration. No real messages, calendar events, or approval requests
were changed during testing.
