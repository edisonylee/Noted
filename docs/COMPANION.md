# Companion movement

The same motion system animates all built-in pets and uploaded artwork. CSS
animations move, tilt, and squash the transparent character image.

| State | Trigger |
| --- | --- |
| Idle | Resting between interactions |
| Greeting | Hover or open chat |
| Drag left / right | Horizontal movement while held |
| Dragged | Held on the desktop without horizontal movement |
| Pulling | Outward movement in the window's 80-point edge zone |
| Jumping | Landing after a drag, pressing J, or an occasional idle hop |
| Working | Waiting for a chat response or applying a confirmed action |
| Waiting | Recording or a proposal awaiting confirmation |
| Review | Reading the assistant's response |
| Failed | Chat/action failure or a failed desktop handoff |

Dragging starts after 6 points of pointer movement. A click still opens chat.
Arrow keys move the focused pet; Shift increases the step. Escape cancels an
in-app drag. Normalized positions survive window resizing, and Settings →
Assistant provides a home corner and a Reset position action.

The edge indicator fills across a 72-point pull. Detaching also requires at
least 24 points of outward movement since pointer-down, preventing incidental
movement from detaching a pet already near an edge. The desktop-only Move to
desktop control provides a keyboard-accessible alternative.

On macOS, a transparent 168 × 184 point webview renders only DesktopCompanion.
Native pointer tracking carries the current grab point across the window
boundary. Cursor and window coordinates are normalized to desktop points
before comparison because their backing scales can differ. Releasing over
Noted on a return drag sends a normalized landing position to the main window.
The first detach cannot immediately reattach until the pointer has actually
left Noted. Back to Noted and Escape also return the desktop pet.

The native worker sleeps on a condition variable while the pet is in-app.
While detached, transparent margins pass mouse events through to other apps.
Preference and assistant-state events keep the two webviews synchronized.
Closing the main window destroys the companion window. Detachment is a
session state: the pet starts inside Noted after restarting the application.

Reduced motion, or disabling Pet animations, stops decorative animation.
Pointer movement and the edge progress indicator remain available.

## Validation

```sh
bun run build
bun test tests/companion-motion.test.ts tests/companion-preferences.test.ts
cargo test --manifest-path src-tauri/Cargo.toml --lib companion::tests
```

Native manual checks should cover detaching on every edge, dropping back into
Noted, changing pets and sizes while detached, clicking through transparent
margins, minimizing/restoring the main window, and dragging between monitors
with different backing scales. Use a single standard Noted instance.
