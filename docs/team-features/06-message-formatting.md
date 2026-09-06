# Inline message formatting

Use a text-only editor document with visual Markdown decorations. Keep the exact
source in the existing per-conversation draft and message body so mentions,
retries, edits, and older clients retain readable content. Render supported
formatting in received messages through React text nodes; never interpret HTML.

The toolbar-free composer supports `**bold**`, `__bold__`, `*italic*`, `_italic_`,
`~~strikethrough~~`, inline backticks, and fenced code. Markers stay visible and
editable while composing. This is a small inline Markdown dialect, not a full
Markdown document editor. It does not add links, embedded HTML, or remote images.

Preserve Enter to send, Shift+Enter for a newline, keyboard mention selection,
IME composition, plain-text paste, and undo/redo. Enforce the existing 10,000
character limit and clear editor history after a successful send.

Validation: parser regression tests cover literal HTML, incomplete and escaped
syntax, code, and Unicode offsets. Synthetic browser checks cover formatting,
newlines, mentions, navigation draft restoration, sending, undo, and plain-text
paste. Frontend typecheck and production build pass. No server migration is needed.
