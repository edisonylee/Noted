import { expect, test } from "bun:test";
import { documentToMarkdown } from "../src/editor/documentMarkdown";
const exportLink = (href: string) =>
  documentToMarkdown({
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          {
            type: "text",
            text: "Source",
            marks: [{ type: "link", attrs: { href } }],
          },
        ],
      },
    ],
  });
test("local paths and internal link targets are omitted without exposing them in omission metadata", () => {
  for (const href of [
    "file:///Users/alice/Secret/plan.pdf",
    "/Users/alice/Secret",
    "../Secret/file",
    "C:\\Secret\\file",
    "\\\\server\\Secret",
    "noted://document/42",
    "asset://localhost/Secret",
    "//server/Secret",
    "https://user:password@example.com",
    "java\tscript:alert(1)",
  ]) {
    const output = exportLink(href);
    expect(output.markdown).toBe("Source\n");
    expect(output.omitted[0].kind).toBe("link");
    expect(JSON.stringify(output)).not.toContain(href);
  }
});
test("shareable web and email links remain intact", () => {
  for (const href of [
    "https://example.com/plan",
    "http://example.com/plan",
    "mailto:team@example.com",
  ]) {
    expect(exportLink(href).markdown).toBe(`[Source](${href})\n`);
    expect(exportLink(href).omitted).toHaveLength(0);
  }
});
