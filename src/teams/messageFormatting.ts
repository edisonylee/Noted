/** A deliberately small, text-only Markdown dialect. Never interprets HTML. */
export type MessageSpan = {
  start: number;
  end: number;
  contentStart: number;
  contentEnd: number;
  kind: "bold" | "italic" | "strike" | "code";
};
export function messageFormatting(text: string): MessageSpan[] {
  const spans: MessageSpan[] = [];
  const pattern =
    /```[\s\S]+?```|`[^`\n]+`|\*\*[^*\n]+\*\*|__[^_\n]+__|~~[^~\n]+~~|\*[^*\n]+\*|_[^_\n]+_/g;
  for (const match of text.matchAll(pattern)) {
    const start = match.index!;
    let slashes = 0;
    for (let i = start - 1; i >= 0 && text[i] === "\\"; i--) slashes++;
    if (slashes % 2) continue;
    const value = match[0];
    const marker = value.startsWith("```")
      ? 3
      : /^(\*\*|__|~~)/.test(value)
        ? 2
        : 1;
    if (value[0] === "_" && /[\p{L}\p{N}]/u.test(text[start - 1] ?? ""))
      continue;
    spans.push({
      start,
      end: start + value.length,
      contentStart: start + marker,
      contentEnd: start + value.length - marker,
      kind:
        value[0] === "`"
          ? "code"
          : value.startsWith("~~")
            ? "strike"
            : marker === 2
              ? "bold"
              : "italic",
    });
  }
  return spans;
}
