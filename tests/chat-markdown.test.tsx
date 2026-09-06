import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { MessageMarkdown } from "../src/teams/MessageMarkdown";
import { chatMarkdownUrl } from "../src/teams/chatMarkdown";
const render = (body: string) =>
  renderToStaticMarkup(<MessageMarkdown body={body} />);

describe("chat CommonMark and GFM", () => {
  test("renders task lists, nested lists, headings, quotes, tables, and nested inline formatting", () => {
    const html = render(
      "# Plan\n\n- [x] Rich text\n- [ ] Preview\n  - nested\n\n1. First\n2. Second\n\n> A **bold and *italic*** quote\n\n| Item | State |\n| --- | --- |\n| Chat | Done |",
    );
    for (const tag of [
      "<h1>Plan</h1>",
      'type="checkbox"',
      'checked=""',
      "<ul>",
      "<ol>",
      "<blockquote>",
      "<table>",
      "<em>italic</em>",
    ])
      expect(html).toContain(tag);
    expect(html).toContain('aria-label="Incomplete task"');
  });
  test("preserves code and chat line breaks and resolves reference links", () => {
    const html = render(
      'one\ntwo\n\n```ts\nconst raw = "<script> @Edison #general";\n```\n\n[Docs][docs]\n\n[docs]: https://example.com/docs',
    );
    expect(html).toContain("<br/>");
    expect(html).toContain("language-ts");
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain('href="https://example.com/docs"');
  });
  test("decorates mentions in formatted prose but never in code or link labels", () => {
    const html = renderToStaticMarkup(
      <MessageMarkdown
        body={
          "**@Edison** and #general\n\n`@literal` [@link](https://example.com)"
        }
        renderText={(text) => <mark>{text}</mark>}
      />,
    );
    expect(html).toContain("<strong><mark>@Edison</mark></strong>");
    expect(html).toContain("<code>@literal</code>");
    expect(html).not.toContain("<mark>@link</mark>");
  });
  test("isolates footnote IDs across messages", () => {
    const body = "A note[^one].\n\n[^one]: Footnote content.";
    const html = renderToStaticMarkup(
      <>
        <MessageMarkdown body={body} />
        <MessageMarkdown body={body} />
      </>,
    );
    const ids = [...html.matchAll(/\sid="([^"]+)"/g)].map((match) => match[1]);
    expect(ids.length).toBeGreaterThanOrEqual(6);
    expect(new Set(ids).size).toBe(ids.length);
  });
  test("does not execute HTML, allow dangerous links, or fetch remote images on render", () => {
    const html = render(
      "<script>alert(1)</script>\n\n[bad](javascript:alert%281%29)\n\n![Photo](https://example.com/tracker.png)",
    );
    expect(html).not.toContain("<script>");
    expect(html).not.toContain('href="javascript:');
    expect(html).not.toContain("<img");
    expect(html).toContain("Load image from example.com");
    for (const url of [
      "javascript:alert(1)",
      "data:text/html,hi",
      "file:///etc/passwd",
      "https://user:password@example.com",
      "/local",
    ])
      expect(chatMarkdownUrl(url)).toBe("");
    expect(chatMarkdownUrl("http://example.com/image.png", true)).toBe("");
    expect(chatMarkdownUrl("https://example.com/image.png", true)).toBe(
      "https://example.com/image.png",
    );
  });
});
