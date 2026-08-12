export type DocumentMark = {
  type: string;
  attrs?: Record<string, unknown>;
};

export type DocumentNode = {
  type: string;
  attrs?: Record<string, unknown>;
  content?: DocumentNode[];
  marks?: DocumentMark[];
  text?: string;
};

export type StructuredDocument = DocumentNode & {
  type: "doc";
};

export type DocumentTask = {
  id: string;
  text: string;
  completed: boolean;
};

export const TASK_DOCUMENT_VERSION = 1;

const EMPTY_PARAGRAPH: DocumentNode = { type: "paragraph" };

function textNode(text: string): DocumentNode[] | undefined {
  return text ? [{ type: "text", text }] : undefined;
}

function taskItem(text: string, completed: boolean): DocumentNode {
  return {
    type: "taskItem",
    attrs: { checked: completed },
    content: [{ type: "paragraph", content: textNode(text) }],
  };
}

export function emptyTaskDocument(): StructuredDocument {
  return {
    type: "doc",
    content: [{ type: "taskList", content: [taskItem("", false)] }],
  };
}

export function todosToTaskDocument(todos: DocumentTask[]): StructuredDocument {
  const populated = todos
    .map((todo) => ({ ...todo, text: todo.text.trim() }))
    .filter((todo) => todo.text);
  if (!populated.length) return emptyTaskDocument();
  return {
    type: "doc",
    content: [
      {
        type: "taskList",
        content: populated.map((todo) => taskItem(todo.text, todo.completed)),
      },
    ],
  };
}

function isDocumentNode(value: unknown): value is DocumentNode {
  if (!value || typeof value !== "object") return false;
  const node = value as Record<string, unknown>;
  if (typeof node.type !== "string" || !node.type) return false;
  if (node.content !== undefined) {
    if (!Array.isArray(node.content) || !node.content.every(isDocumentNode)) return false;
  }
  if (node.text !== undefined && typeof node.text !== "string") return false;
  return true;
}

export function isStructuredDocument(value: unknown): value is StructuredDocument {
  return isDocumentNode(value) && value.type === "doc";
}

export function normalizeTaskDocument(value: unknown, legacyTodos: DocumentTask[]): StructuredDocument {
  return isStructuredDocument(value) ? value : todosToTaskDocument(legacyTodos);
}

function nodeText(node: DocumentNode, includeNestedLists = true): string {
  if (node.type === "text") return node.text ?? "";
  if (!includeNestedLists && ["taskList", "bulletList", "orderedList"].includes(node.type)) return "";
  return (node.content ?? []).map((child) => nodeText(child, includeNestedLists)).join("");
}

function taskText(node: DocumentNode): string {
  return (node.content ?? [])
    .filter((child) => !["taskList", "bulletList", "orderedList"].includes(child.type))
    .map((child) => nodeText(child, false))
    .join("\n")
    .trim();
}

function stableTaskId(text: string, position: number): string {
  let hash = 2166136261;
  const source = `${position}:${text}`;
  for (let index = 0; index < source.length; index += 1) {
    hash ^= source.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return `doc-task-${position}-${(hash >>> 0).toString(36)}`;
}

export function extractDocumentTasks(document: StructuredDocument): DocumentTask[] {
  const tasks: DocumentTask[] = [];
  const visit = (node: DocumentNode) => {
    if (node.type === "taskItem") {
      const text = taskText(node);
      if (text) {
        const position = tasks.length;
        tasks.push({
          id: stableTaskId(text, position),
          text,
          completed: node.attrs?.checked === true,
        });
      }
    }
    for (const child of node.content ?? []) visit(child);
  };
  visit(document);
  return tasks;
}

export function countOpenDocumentTasks(document: StructuredDocument): number {
  return extractDocumentTasks(document).filter((task) => !task.completed).length;
}

function renderListItem(node: DocumentNode, marker: string): string[] {
  const ownText = (node.content ?? [])
    .filter((child) => !["taskList", "bulletList", "orderedList"].includes(child.type))
    .map((child) => child.type === "image" ? renderBlock(child).join(" ") : nodeText(child, false))
    .join(" ")
    .trim();
  const lines = ownText ? [`${marker}${ownText}`] : [];
  for (const child of node.content ?? []) {
    if (["taskList", "bulletList", "orderedList"].includes(child.type)) {
      lines.push(...renderBlock(child).map((line) => `  ${line}`));
    }
  }
  return lines;
}

function renderBlock(node: DocumentNode): string[] {
  if (node.type === "image") {
    const alt = typeof node.attrs?.alt === "string" ? node.attrs.alt.trim() : "";
    return [`[Image${alt ? `: ${alt}` : ""}]`];
  }
  if (node.type === "taskList") {
    return (node.content ?? []).flatMap((item) =>
      renderListItem(item, item.attrs?.checked === true ? "- [x] " : "- [ ] ")
    );
  }
  if (node.type === "bulletList") {
    return (node.content ?? []).flatMap((item) => renderListItem(item, "- "));
  }
  if (node.type === "orderedList") {
    return (node.content ?? []).flatMap((item, index) => renderListItem(item, `${index + 1}. `));
  }
  if (node.type === "listItem") return renderListItem(node, "- ");
  const text = nodeText(node).trim();
  return text ? [text] : [];
}

export function documentPlainText(document: StructuredDocument): string {
  return (document.content ?? [])
    .flatMap(renderBlock)
    .join("\n")
    .trim();
}

export function documentFingerprint(document: StructuredDocument): string {
  const canonicalize = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(canonicalize);
    if (!value || typeof value !== "object") return value;
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, child]) => [key, canonicalize(child)])
    );
  };
  return JSON.stringify(canonicalize(document));
}

export function emptyDocument(): StructuredDocument {
  return { type: "doc", content: [EMPTY_PARAGRAPH] };
}

/**
 * Lift legacy plain-text notes into the shared document shape without
 * interpreting their contents as Markdown. One source line becomes one
 * paragraph, so existing meeting notes keep their exact wording and spacing
 * the first time they are opened in the rich editor.
 */
export function plainTextToDocument(text: string): StructuredDocument {
  const normalized = text.replace(/\r\n?/g, "\n");
  if (!normalized) return emptyDocument();
  return {
    type: "doc",
    content: normalized.split("\n").map((line) => ({
      type: "paragraph",
      content: textNode(line),
    })),
  };
}

export function storedDocumentOrPlainText(
  stored: string | null | undefined,
  fallbackText: string
): StructuredDocument {
  if (stored) {
    try {
      const parsed: unknown = JSON.parse(stored);
      if (isStructuredDocument(parsed)) return parsed;
    } catch {
      // A damaged optional rich representation must never hide the preserved
      // plain-text notes. Fall through to the authoritative fallback.
    }
  }
  return plainTextToDocument(fallbackText);
}
