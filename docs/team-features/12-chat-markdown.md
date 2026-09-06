# CommonMark and GitHub-flavored chat Markdown

## Behavior

Render sent messages and thread replies with react-markdown, remark-gfm, and
remark-breaks. Support CommonMark headings, emphasis (including nesting), ordered
and unordered lists, blockquotes, fenced/indented code, links and reference links,
images, escapes, and thematic breaks. GFM adds tables, task lists, strikethrough,
autolinks, and footnotes. Preserve ordinary chat newlines as line breaks.

Existing message bodies render automatically after the app update. Store the
original Markdown unchanged; editing, drafts, idempotent retries, search, and
older clients retain the same text. The composer remains a source editor with
its existing inline formatting decorations; this change upgrades sent-message
rendering, not the composer into a full document editor. Task checkboxes reflect
`- [x]` and `- [ ]` and are read-only; edit the message to change their state.

Preserve member mentions and workspace-scoped channel navigation in parsed text,
including formatted prose and lists. Do not turn code contents or link labels
into nested interactive mentions. Scope footnote IDs and navigation to the
individual message so two messages can use the same footnote names.

## Safety and performance

Use React nodes and the parser's default escaped handling of raw HTML. Do not
install rehype-raw, execute document scripts, or interpret arbitrary HTML.
Allow ordinary HTTP/HTTPS/mailto links, reject credentials and local/executable
schemes, and open external links through the existing native/browser opener.
Fragment links navigate only inside the current message.

Markdown image URLs require HTTPS and an explicit “Load image from [host]” action;
rendering a message must not silently contact a sender-controlled image host.
Loaded images omit the referrer. Uploaded attachments retain their separately
authorized inline previews. Relative URLs, arbitrary embedded HTML, and extensions
outside CommonMark/GFM (such as executable diagrams) are not enabled.

Memoize the parsed message component by body. Supply the changing mention resolver
through context so polling or typing does not reparse the entire message history.
Wide tables and code scroll within their own containers. Use theme colors and
compact typography at desktop and narrow widths.

## Verification

Rendering tests exercise lists/tasks/tables, nested emphasis, code, line breaks,
reference links, mention exclusions, unique footnote IDs, unsafe schemes, escaped
HTML, and no unsolicited image requests. Synthetic browser checks verify task
states, table layout, code isolation, local footnote focus, channel navigation,
and narrow layouts without page overflow. Frontend production build passes.

This is client-only and requires `bun run app:update`; no server redeployment or
schema migration is needed.

Sources: [react-markdown](https://github.com/remarkjs/react-markdown) and
[remark-gfm](https://github.com/remarkjs/remark-gfm).
