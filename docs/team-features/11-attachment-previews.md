# Attachment previews

## Experience

Display PNG/JPEG images inline with reserved thumbnail space to keep conversation
scrolling stable. Clicking the image or filename opens a focused preview dialog.
PDF and text attachments open the same dialog from their filename. Keep Save
available both in the conversation and in the dialog.

Image previews support fit and zoom. PDF previews offer previous/next page,
page count, zoom, and a width reset. Text previews are selectable, wrapped UTF-8
with a 200,000-character display limit and an explicit truncation notice. The
original remains downloadable. Dialogs retain native modal keyboard behavior,
Escape to close, and narrow-window layouts.

## Implementation and access

Reuse the authenticated attachment endpoint and its current organization,
conversation, and deletion checks. No new server endpoint, migration, native
command, public URL, or persistent preview cache is introduced. Reauthorize when
opening a dialog and on window focus/visibility changes. Release blob URLs on
close, when a thumbnail leaves the viewport, and on authorization failure;
discard async results after unmount. Do not fetch PDF/text bodies merely because
their message is present in history.

Check attachment identity, MIME, declared size, and decoded size before previewing.
Keep the existing 5 MiB bound and allow only PNG, JPEG, PDF, and plain text.
Render text through React text nodes, including Markdown and CSV attachments;
never interpret HTML or remote image markup.

Use a pinned, lazy-loaded PDF.js distribution with a local worker and bundled
fonts/character maps/decoders. Pass authenticated bytes, not a URL, to the PDF
loader. Render one page at a time to a canvas with bounded canvas/image memory.
Do not initialize PDF scripting, XFA forms, interactive annotation layers, or
external navigation. Resolve PDF support resources from an explicit build-time
asset map only. Cancel page rendering and destroy the document worker on close.
Expose extracted page text to assistive technology. Corrupt/password-protected
PDFs and viewer failures retain a clear explanation and Save fallback.

The implementation follows the [PDF.js rendering API](https://mozilla.github.io/pdf.js/examples/)
and its [document-loading options](https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib.html).

## Verification and rollout

- Unit tests cover supported MIME types, identity/size mismatch, invalid encoding,
  literal text, truncation, and invalid UTF-8.
- Existing server attachment tests cover authorization, revocation, deletion,
  signatures, size limits, and storage quota.
- Synthetic browser checks exercise inline image loading, image zoom/fit,
  two PDF pages and zoom, text containing HTML-like content, Escape/close,
  compact layout, and denied access without stale content.
- Frontend typecheck and production build include the local PDF worker/assets.

Run `bun run app:update` to install this frontend change. No additional team-server
deployment is required if the existing attachment endpoint is already deployed.
The installed macOS WebView still needs a user smoke test; browser validation does
not stand in for an installed-app run. PDF text is exposed for accessibility but
canvas preview does not provide selectable PDF text or interactive forms.
