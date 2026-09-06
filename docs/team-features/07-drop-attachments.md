# Drag-and-drop message attachments

Accept file drops across the active conversation, including its composer. Show a
quiet drop target and stage files through the same picker validation and send
path. A drop never sends a message. PNG and JPEG drafts have local thumbnails;
every file has its name, size, and a removal action before sending.

Retain the existing server policy: at most three PNG, JPEG, PDF, TXT, MD, or CSV
files and 5 MiB combined. Server validation and current conversation permissions
remain authoritative. Prevent browser navigation on file drops, reject
unsupported types, serialize file reads, and ignore results after unmounting.
Read-only conversations and conversations without server attachment support
cannot stage drops. Temporary files follow the existing session-only draft
lifecycle; no new persistence or network endpoint is introduced.

Validation: synthetic browser checks cover file drops over history and directly
into the editor, staging without sending, local image previews, removal,
unsupported-file rejection, and sending. Existing attachment authorization,
validation, and messaging regression tests pass. Standard macOS WebView native
file dragging still needs an installed-app smoke test. The standard window
already enables HTML file-drop handling (`dragDropEnabled: false`).
