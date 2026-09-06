import type { JSONContent } from "@tiptap/core";

/**
 * Deterministic Markdown export for the TipTap documents this app produces
 * (StarterKit + Image + TaskList/TaskItem + TextAlign + TextStyleKit). The
 * output is what gets published to a team, so the same document must always
 * serialize to the same bytes, images must never leak local paths or data URLs,
 * and literal text must survive a CommonMark + GFM renderer unchanged.
 *
 * Nothing here touches the DOM or the editor; it runs on the plain JSON.
 */
export type Omitted = {
  kind: "image" | "unsupported" | "link";
  detail: string;
};

const IMAGE_PLACEHOLDER = "*[Image not shared]*";
const LIST_TYPES = new Set(["bulletList", "orderedList", "taskList"]);

/**
 * Wrapping order, outermost first. Fixed so equal content produces equal text
 * no matter which order the editor happened to record the marks in. Code is
 * innermost because a code span cannot contain other delimiters, and the link
 * is outermost so one link over mixed formatting stays one link.
 */
const MARK_ORDER = ["link", "bold", "italic", "strike", "code"] as const;
type MarkName = (typeof MARK_ORDER)[number];
const EMPHASIS_DELIMITERS: Record<
  Exclude<MarkName, "link" | "code">,
  string
> = {
  bold: "**",
  italic: "*",
  strike: "~~",
};

function attribute(node: JSONContent, name: string): unknown {
  return node.attrs?.[name];
}

function isInlineType(node: JSONContent): boolean {
  return node.type === "text" || node.type === "hardBreak";
}

function plainText(node: JSONContent): string {
  if (node.type === "text") return node.text ?? "";
  return (node.content ?? []).map(plainText).join("");
}

/**
 * Characters that change meaning anywhere in a line. `&` only when it would
 * read as an entity, so prose like "R&D" stays readable in the raw text.
 */
function escapeText(text: string): string {
  return text
    .replace(/[\\`*_[\]<~|]/g, "\\$&")
    .replace(/&(?=#?[a-zA-Z0-9]+;)/g, "\\&");
}

/**
 * Characters that only change meaning at the start of a line: headings,
 * quotes, list markers, thematic breaks and setext underlines. Applied to the
 * rendered inline string, so a literal `#` typed after a hard break (which
 * CommonMark would still promote to a heading) is caught as well.
 */
function escapeLineStart(line: string): string {
  return line
    .replace(/^(\s*)([#>+=-])/, "$1\\$2")
    .replace(/^(\s*)(\d{1,9})([.)])(?=\s|$)/, "$1$2\\$3");
}

function imageDetail(node: JSONContent): string {
  const alt = attribute(node, "alt");
  if (typeof alt === "string" && alt.trim()) return alt.trim();
  const src = attribute(node, "src");
  if (typeof src === "string" && src.trim() && !/^data:/i.test(src.trim())) {
    const name = src.trim().split(/[?#]/)[0].split(/[\\/]/).pop();
    if (name) return name;
  }
  return "Image";
}

/**
 * A destination the published Markdown may carry. Control characters are
 * stripped before the scheme check so "java\tscript:" cannot slip past it;
 * spaces and parentheses are encoded so the destination cannot end early.
 */
function safeHref(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const href = value.trim();
  // Local paths and internal app schemes are not portable public destinations.
  // Reject controls instead of normalizing a hidden destination into a new one.
  if (
    /[\u0000-\u001f\u007f]/.test(href) ||
    !/^(https?:\/\/|mailto:)/i.test(href)
  )
    return null;
  try {
    const url = new URL(href);
    if (url.username || url.password) return null;
  } catch {
    return null;
  }
  return href
    .replace(/[<>\\]/g, (character) => encodeURIComponent(character))
    .replace(/[()]/g, "\\$&")
    .replace(/\s/g, "%20");
}

function codeSpan(text: string): string {
  const longest = Math.max(
    0,
    ...(text.match(/`+/g) ?? []).map((run) => run.length),
  );
  const fence = "`".repeat(longest + 1);
  // A span starting or ending with a backtick or space needs padding; the
  // renderer strips exactly one space from each side when both are present.
  const pad = /^[` ]|[` ]$/.test(text) ? " " : "";
  return `${fence}${pad}${text}${pad}${fence}`;
}

function markOf(node: JSONContent, name: MarkName) {
  return node.type === "text"
    ? node.marks?.find((mark) => mark.type === name)
    : undefined;
}

function sameMarkRun(
  node: JSONContent,
  name: MarkName,
  mark: ReturnType<typeof markOf>,
): boolean {
  const own = markOf(node, name);
  if (!own || !mark) return !own && !mark;
  return name !== "link" || own.attrs?.href === mark.attrs?.href;
}

/**
 * Emphasis cannot open before or close after whitespace, so a bold run whose
 * text ends in a space moves that space outside the delimiters.
 */
function wrapEmphasis(delimiter: string, inner: string): string {
  const lead = inner.match(/^\s*/)?.[0] ?? "";
  const trail = inner.match(/\s*$/)?.[0] ?? "";
  const core = inner.slice(lead.length, inner.length - trail.length);
  if (!core) return inner;
  return `${lead}${delimiter}${core}${delimiter}${trail}`;
}

function renderLeaf(
  node: JSONContent,
  hardBreak: string,
  omitted: Omitted[],
): string {
  if (node.type === "text") return escapeText(node.text ?? "");
  if (node.type === "hardBreak") return hardBreak;
  if (node.type === "image") {
    omitted.push({ kind: "image", detail: imageDetail(node) });
    return IMAGE_PLACEHOLDER;
  }
  omitted.push({ kind: "unsupported", detail: node.type ?? "unknown" });
  return escapeText(plainText(node));
}

function renderInline(
  nodes: JSONContent[],
  order: readonly MarkName[],
  hardBreak: string,
  omitted: Omitted[],
): string {
  if (!order.length)
    return nodes.map((node) => renderLeaf(node, hardBreak, omitted)).join("");
  const [name, ...rest] = order;
  let out = "";
  let index = 0;
  while (index < nodes.length) {
    const mark = markOf(nodes[index], name);
    let end = index + 1;
    while (end < nodes.length && sameMarkRun(nodes[end], name, mark)) end += 1;
    const run = nodes.slice(index, end);
    if (!mark) {
      out += renderInline(run, rest, hardBreak, omitted);
    } else if (name === "code") {
      out += codeSpan(run.map((node) => node.text ?? "").join(""));
    } else if (name === "link") {
      const inner = renderInline(run, rest, hardBreak, omitted);
      const href = safeHref(mark.attrs?.href);
      if (!href)
        omitted.push({
          kind: "link",
          detail:
            "Local, internal, or unsupported link destination (display text retained)",
        });
      out += href ? `[${inner}](${href})` : inner;
    } else {
      out += wrapEmphasis(
        EMPHASIS_DELIMITERS[name],
        renderInline(run, rest, hardBreak, omitted),
      );
    }
    index = end;
  }
  return out;
}

/**
 * Turn a paragraph's inline string into clean lines: every line-start escaped,
 * no trailing whitespace, and each hard break marked with two trailing spaces.
 * A hard break on an otherwise empty line uses a backslash instead, because a
 * whitespace-only line would end the paragraph.
 */
function paragraphLines(inline: string): string[] {
  const lines = inline
    .split("\n")
    .map((line) => escapeLineStart(line.replace(/\s+$/, "")));
  while (lines.length && !lines[lines.length - 1]) lines.pop();
  return lines.map((line, index) => {
    if (index === lines.length - 1) return line;
    return line ? `${line}  ` : "\\";
  });
}

function renderParagraph(content: JSONContent[], omitted: Omitted[]): string[] {
  return paragraphLines(renderInline(content, MARK_ORDER, "\n", omitted));
}

function renderHeading(node: JSONContent, omitted: Omitted[]): string[] {
  const level = Math.min(6, Math.max(1, Number(attribute(node, "level")) || 1));
  // A trailing run of hashes after a space would read as the closing sequence
  // and vanish; escaping its first hash keeps it as text.
  const text = renderInline(node.content ?? [], MARK_ORDER, " ", omitted)
    .trim()
    .replace(/(^|\s)(#+)$/, "$1\\$2");
  return text ? [`${"#".repeat(level)} ${text}`] : [];
}

function renderCodeBlock(node: JSONContent): string[] {
  const text = (node.content ?? [])
    .map((child) => child.text ?? "")
    .join("")
    .replace(/\r\n?/g, "\n");
  const longest = Math.max(
    2,
    ...(text.match(/`+/g) ?? []).map((run) => run.length),
  );
  const fence = "`".repeat(longest + 1);
  const language = String(attribute(node, "language") ?? "").replace(
    /[\s`]/g,
    "",
  );
  return [`${fence}${language}`, ...(text ? text.split("\n") : []), fence];
}

function renderBlockquote(node: JSONContent, omitted: Omitted[]): string[] {
  return renderBlocks(node.content ?? [], omitted).map((line) =>
    line ? `> ${line}` : ">",
  );
}

function taskMarker(node: JSONContent): string {
  return attribute(node, "checked") === true ? "- [x] " : "- [ ] ";
}

/**
 * Continuation lines sit at the marker's content column so nested blocks stay
 * inside the item: two spaces under `- ` and `- [ ] ` (the checkbox is part of
 * the item text), three under `1. `, four under `10. `.
 */
function renderListItem(
  node: JSONContent,
  marker: string,
  indentWidth: number,
  omitted: Omitted[],
): string[] {
  const indent = " ".repeat(indentWidth);
  const blocks = (node.content ?? [])
    .map((child) => ({
      list: LIST_TYPES.has(child.type ?? ""),
      // Only a list numbered from 1 may interrupt a paragraph; any other start
      // would be read as more of the item's text unless a blank line precedes it.
      gap: child.type === "orderedList" && listStart(child) !== 1,
      lines: renderBlock(child, omitted),
    }))
    .filter((block) => block.lines.length);
  const out: string[] = [];
  blocks.forEach((block, index) => {
    let rest = block.lines;
    if (index === 0 && !block.list) {
      out.push(`${marker}${block.lines[0]}`);
      rest = block.lines.slice(1);
    } else if (index === 0) {
      out.push(marker.trimEnd());
    } else if (!block.list || block.gap) {
      // A second paragraph in an item needs a blank line; a nested list does not.
      out.push("");
    }
    out.push(...rest.map((line) => (line ? `${indent}${line}` : "")));
  });
  return out.length ? out : [marker.trimEnd()];
}

function listStart(node: JSONContent): number {
  return Math.max(1, Math.floor(Number(attribute(node, "start")) || 1));
}

function renderList(node: JSONContent, omitted: Omitted[]): string[] {
  const start = listStart(node);
  return (node.content ?? []).flatMap((item, index) => {
    if (node.type === "taskList" || item.type === "taskItem") {
      return renderListItem(item, taskMarker(item), 2, omitted);
    }
    const marker = node.type === "orderedList" ? `${start + index}. ` : "- ";
    return renderListItem(item, marker, marker.length, omitted);
  });
}

function renderBlock(node: JSONContent, omitted: Omitted[]): string[] {
  switch (node.type) {
    case "paragraph":
      return renderParagraph(node.content ?? [], omitted);
    case "heading":
      return renderHeading(node, omitted);
    case "bulletList":
    case "orderedList":
    case "taskList":
      return renderList(node, omitted);
    case "listItem":
      return renderListItem(node, "- ", 2, omitted);
    case "taskItem":
      return renderListItem(node, taskMarker(node), 2, omitted);
    case "blockquote":
      return renderBlockquote(node, omitted);
    case "codeBlock":
      return renderCodeBlock(node);
    case "horizontalRule":
      return ["---"];
    case "image":
      omitted.push({ kind: "image", detail: imageDetail(node) });
      return [IMAGE_PLACEHOLDER];
    case "text":
    case "hardBreak":
      return renderParagraph([node], omitted);
    default: {
      // Keep the words, drop the structure, and say so: the reader sees the
      // content and the publish preview lists what did not survive.
      omitted.push({ kind: "unsupported", detail: node.type ?? "unknown" });
      const content = node.content ?? [];
      return content.every(isInlineType)
        ? renderParagraph(content, omitted)
        : renderBlocks(content, omitted);
    }
  }
}

function renderBlocks(nodes: JSONContent[], omitted: Omitted[]): string[] {
  const out: string[] = [];
  for (const node of nodes) {
    const lines = renderBlock(node, omitted);
    if (!lines.length) continue;
    if (out.length) out.push("");
    out.push(...lines);
  }
  return out;
}

export function documentToMarkdown(doc: JSONContent): {
  markdown: string;
  omitted: Omitted[];
} {
  const omitted: Omitted[] = [];
  const lines = renderBlocks(doc.content ?? [], omitted);
  return { markdown: lines.length ? `${lines.join("\n")}\n` : "", omitted };
}
