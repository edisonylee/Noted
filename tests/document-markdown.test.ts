import { describe, expect, test } from "bun:test";
import type { JSONContent } from "@tiptap/core";
import { documentToMarkdown } from "../src/editor/documentMarkdown";

function doc(...content: JSONContent[]): JSONContent {
  return { type: "doc", content };
}

function text(value: string, ...marks: NonNullable<JSONContent["marks"]>): JSONContent {
  return marks.length ? { type: "text", text: value, marks } : { type: "text", text: value };
}

function paragraph(...content: JSONContent[]): JSONContent {
  return { type: "paragraph", content };
}

function heading(level: number, ...content: JSONContent[]): JSONContent {
  return { type: "heading", attrs: { level }, content };
}

function listItem(...content: JSONContent[]): JSONContent {
  return { type: "listItem", content };
}

function taskItem(checked: boolean, ...content: JSONContent[]): JSONContent {
  return { type: "taskItem", attrs: { checked }, content };
}

function markdown(...content: JSONContent[]): string {
  return documentToMarkdown(doc(...content)).markdown;
}

const bold = { type: "bold" };
const italic = { type: "italic" };
const strike = { type: "strike" };
const code = { type: "code" };
const link = (href: string) => ({ type: "link", attrs: { href, target: "_blank", rel: "noopener" } });

describe("document Markdown export: blocks", () => {
  test("paragraphs are separated by one blank line and the output ends with one newline", () => {
    expect(markdown(paragraph(text("First")), paragraph(text("Second")))).toBe("First\n\nSecond\n");
  });

  test("headings 1 through 6 use the matching number of hashes and clamp out-of-range levels", () => {
    expect(markdown(...[1, 2, 3, 4, 5, 6].map((level) => heading(level, text(`Level ${level}`))))).toBe(
      "# Level 1\n\n## Level 2\n\n### Level 3\n\n#### Level 4\n\n##### Level 5\n\n###### Level 6\n",
    );
    expect(markdown(heading(9, text("Deep")), heading(0, text("Top")))).toBe("###### Deep\n\n# Top\n");
  });

  test("a heading ending in a hash run keeps it as text instead of a closing sequence", () => {
    expect(markdown(heading(2, text("Issue #")))).toBe("## Issue \\#\n");
  });

  test("bullet lists use dashes", () => {
    expect(markdown({
      type: "bulletList",
      content: [listItem(paragraph(text("One"))), listItem(paragraph(text("Two")))],
    })).toBe("- One\n- Two\n");
  });

  test("ordered lists count from the start attribute", () => {
    expect(markdown({
      type: "orderedList",
      attrs: { start: 3 },
      content: [listItem(paragraph(text("Three"))), listItem(paragraph(text("Four")))],
    })).toBe("3. Three\n4. Four\n");
  });

  test("task lists render open and done states", () => {
    expect(markdown({
      type: "taskList",
      content: [
        taskItem(false, paragraph(text("Book the room"))),
        taskItem(true, paragraph(text("Send the invite"))),
        { type: "taskItem", content: [paragraph(text("No checked attribute"))] },
      ],
    })).toBe("- [ ] Book the room\n- [x] Send the invite\n- [ ] No checked attribute\n");
  });

  test("nested lists indent three levels deep under the parent's content column", () => {
    const document = doc({
      type: "bulletList",
      content: [
        listItem(
          paragraph(text("Top")),
          {
            type: "orderedList",
            content: [
              listItem(
                paragraph(text("Middle")),
                {
                  type: "taskList",
                  content: [taskItem(true, paragraph(text("Bottom")))],
                },
              ),
            ],
          },
        ),
        listItem(paragraph(text("Sibling"))),
      ],
    });
    expect(documentToMarkdown(document).markdown).toBe(
      "- Top\n" +
        "  1. Middle\n" +
        "     - [x] Bottom\n" +
        "- Sibling\n",
    );
  });

  test("nested tasks under a task item indent by two spaces", () => {
    expect(markdown({
      type: "taskList",
      content: [
        taskItem(false, paragraph(text("Parent")), {
          type: "taskList",
          content: [taskItem(true, paragraph(text("Child")))],
        }),
      ],
    })).toBe("- [ ] Parent\n  - [x] Child\n");
  });

  test("a nested ordered list not starting at 1 is set off by a blank line so it cannot join the item text", () => {
    expect(markdown({
      type: "bulletList",
      content: [listItem(
        paragraph(text("Top")),
        { type: "orderedList", attrs: { start: 9 }, content: [listItem(paragraph(text("Nine"))), listItem(paragraph(text("Ten")))] },
      )],
    })).toBe("- Top\n\n  9. Nine\n  10. Ten\n");
    expect(markdown({
      type: "bulletList",
      content: [listItem(
        paragraph(text("Top")),
        { type: "orderedList", content: [listItem(paragraph(text("One")))] },
      )],
    })).toBe("- Top\n  1. One\n");
  });

  test("an item whose only content is a nested list gets a bare marker", () => {
    expect(markdown({
      type: "bulletList",
      content: [listItem({ type: "bulletList", content: [listItem(paragraph(text("Inner")))] })],
    })).toBe("-\n  - Inner\n");
  });

  test("a second paragraph in a list item is separated by a blank line and indented", () => {
    expect(markdown({
      type: "bulletList",
      content: [listItem(paragraph(text("First")), paragraph(text("Second")))],
    })).toBe("- First\n\n  Second\n");
  });

  test("blockquotes prefix every line, including blank separators", () => {
    expect(markdown({
      type: "blockquote",
      content: [paragraph(text("Quoted")), paragraph(text("Still quoted"))],
    })).toBe("> Quoted\n>\n> Still quoted\n");
  });

  test("code blocks are fenced with their language and never escaped", () => {
    expect(markdown({
      type: "codeBlock",
      attrs: { language: "ts" },
      content: [text("const a = *b* + _c_;\nreturn `x`;")],
    })).toBe("```ts\nconst a = *b* + _c_;\nreturn `x`;\n```\n");
  });

  test("code blocks without a language and with backtick runs still close correctly", () => {
    expect(markdown({ type: "codeBlock", attrs: { language: null }, content: [text("```\nnot a fence")] })).toBe(
      "````\n```\nnot a fence\n````\n",
    );
    expect(markdown({ type: "codeBlock" })).toBe("```\n```\n");
  });

  test("horizontal rules render as a dash line", () => {
    expect(markdown(paragraph(text("Above")), { type: "horizontalRule" }, paragraph(text("Below")))).toBe(
      "Above\n\n---\n\nBelow\n",
    );
  });

  test("hard breaks become two trailing spaces and a newline, and never trailing whitespace at the end", () => {
    expect(markdown(paragraph(text("Line one "), { type: "hardBreak" }, text("Line two"), { type: "hardBreak" }))).toBe(
      "Line one  \nLine two\n",
    );
  });

  test("consecutive hard breaks use a backslash so the blank line does not end the paragraph", () => {
    expect(markdown(paragraph(text("A"), { type: "hardBreak" }, { type: "hardBreak" }, text("B")))).toBe(
      "A  \n\\\nB\n",
    );
  });

  test("hard breaks inside headings collapse to a space", () => {
    expect(markdown(heading(1, text("Two"), { type: "hardBreak" }, text("words")))).toBe("# Two words\n");
  });

  test("empty paragraphs and headings produce nothing", () => {
    expect(markdown(paragraph(), heading(2), paragraph(text("Only")), { type: "paragraph", content: [] })).toBe(
      "Only\n",
    );
    expect(markdown()).toBe("");
  });
});

describe("document Markdown export: marks", () => {
  test("bold, italic, strike and code use their delimiters", () => {
    expect(markdown(paragraph(
      text("b", bold), text(" "), text("i", italic), text(" "), text("s", strike), text(" "), text("c", code),
    ))).toBe("**b** *i* ~~s~~ `c`\n");
  });

  test("marks nest in a fixed order regardless of how the editor recorded them", () => {
    const boldFirst = markdown(paragraph(text("both", bold, italic)));
    const italicFirst = markdown(paragraph(text("both", italic, bold)));
    expect(boldFirst).toBe("***both***\n");
    expect(italicFirst).toBe(boldFirst);
    expect(markdown(paragraph(text("all", code, strike, italic, bold)))).toBe("***~~`all`~~***\n");
  });

  test("adjacent text with a shared outer mark merges into one span", () => {
    expect(markdown(paragraph(text("Bold ", bold), text("and italic", bold, italic), text(" tail", bold)))).toBe(
      "**Bold *and italic* tail**\n",
    );
  });

  test("whitespace at the edge of emphasis moves outside the delimiters", () => {
    expect(markdown(paragraph(text("Note: ", bold), text("done")))).toBe("**Note:** done\n");
    expect(markdown(paragraph(text("   ", bold)))).toBe("");
  });

  test("code spans are not escaped and pad or lengthen fences around backticks", () => {
    expect(markdown(paragraph(text("a*b_c", code)))).toBe("`a*b_c`\n");
    expect(markdown(paragraph(text("`tick`", code)))).toBe("`` `tick` ``\n");
    expect(markdown(paragraph(text(" padded ", code)))).toBe("`  padded  `\n");
  });

  test("links wrap their text and keep one link across mixed formatting", () => {
    expect(markdown(paragraph(text("plain ", link("https://example.com")), text("bold", link("https://example.com"), bold)))).toBe(
      "[plain **bold**](https://example.com)\n",
    );
  });

  test("link destinations encode spaces and parentheses", () => {
    expect(markdown(paragraph(text("wiki", link("https://en.wikipedia.org/wiki/Foo_(bar) baz"))))).toBe(
      "[wiki](https://en.wikipedia.org/wiki/Foo_\\(bar\\)%20baz)\n",
    );
  });

  test("javascript: and data: hrefs are emitted as plain text, including obfuscated forms", () => {
    expect(markdown(paragraph(text("click", link("javascript:alert(1)"))))).toBe("click\n");
    expect(markdown(paragraph(text("click", link("  JavaScript:alert(1)"))))).toBe("click\n");
    expect(markdown(paragraph(text("click", link("java\tscript:alert(1)"))))).toBe("click\n");
    expect(markdown(paragraph(text("image", link("data:text/html,<script>"))))).toBe("image\n");
    expect(markdown(paragraph(text("empty", link(""))))).toBe("empty\n");
    expect(markdown(paragraph(text("bold link", link("javascript:void(0)"), bold)))).toBe("**bold link**\n");
  });

  test("text alignment, text styles, underline and highlights are dropped without losing text or being recorded", () => {
    const result = documentToMarkdown(doc(
      { type: "paragraph", attrs: { textAlign: "center", indent: 2 }, content: [
        text("Styled", { type: "textStyle", attrs: { color: "#ff0000", fontFamily: "Georgia", fontSize: "24px" } }),
        text(" underlined", { type: "underline" }),
        text(" highlighted", { type: "highlight", attrs: { color: "#dfff00" } }),
      ] },
      { type: "heading", attrs: { level: 1, textAlign: "right" }, content: [text("Right")] },
    ));
    expect(result.markdown).toBe("Styled underlined highlighted\n\n# Right\n");
    expect(result.omitted).toEqual([]);
  });
});

describe("document Markdown export: escaping", () => {
  test("Markdown-significant characters inside text are escaped", () => {
    expect(markdown(paragraph(text("2 * 3 = 6, snake_case, `tick`, [brackets], a<b, ~home, a|b, back\\slash")))).toBe(
      "2 \\* 3 = 6, snake\\_case, \\`tick\\`, \\[brackets\\], a\\<b, \\~home, a\\|b, back\\\\slash\n",
    );
  });

  test("entity-like ampersands are escaped while plain ampersands stay readable", () => {
    expect(markdown(paragraph(text("R&D, &amp; and &#39;")))).toBe("R&D, \\&amp; and \\&#39;\n");
  });

  test("literal markers at the start of a line are escaped", () => {
    expect(markdown(
      paragraph(text("* not a bullet")),
      paragraph(text("_ not emphasis")),
      paragraph(text("# not a heading")),
      paragraph(text("> not a quote")),
      paragraph(text("- not a list")),
      paragraph(text("+ not a list")),
      paragraph(text("1. not a list")),
      paragraph(text("2) not a list")),
      paragraph(text("---")),
      paragraph(text("===")),
    )).toBe(
      "\\* not a bullet\n\n" +
        "\\_ not emphasis\n\n" +
        "\\# not a heading\n\n" +
        "\\> not a quote\n\n" +
        "\\- not a list\n\n" +
        "\\+ not a list\n\n" +
        "1\\. not a list\n\n" +
        "2\\) not a list\n\n" +
        "\\---\n\n" +
        "\\===\n",
    );
  });

  test("line-start escaping also applies after a hard break and inside list items and quotes", () => {
    expect(markdown(paragraph(text("intro"), { type: "hardBreak" }, text("# still text")))).toBe(
      "intro  \n\\# still text\n",
    );
    expect(markdown({ type: "bulletList", content: [listItem(paragraph(text("# item")))] })).toBe("- \\# item\n");
    expect(markdown({ type: "blockquote", content: [paragraph(text("- quoted"))] })).toBe("> \\- quoted\n");
  });

  test("numbers followed by a period mid-sentence are left alone", () => {
    expect(markdown(paragraph(text("Version 1. Done")))).toBe("Version 1. Done\n");
    expect(markdown(paragraph(text("1.5 litres")))).toBe("1.5 litres\n");
  });
});

describe("document Markdown export: omissions", () => {
  test("images are replaced by the placeholder and recorded with their alt text", () => {
    const result = documentToMarkdown(doc(
      paragraph(text("Before")),
      { type: "image", attrs: { src: "images/abc.png", localPath: "images/abc.png", alt: "Whiteboard photo" } },
      paragraph(text("After")),
    ));
    expect(result.markdown).toBe("Before\n\n*[Image not shared]*\n\nAfter\n");
    expect(result.omitted).toEqual([{ kind: "image", detail: "Whiteboard photo" }]);
  });

  test("images without alt text are recorded by file name, and data URLs by a generic label", () => {
    const result = documentToMarkdown(doc(
      { type: "image", attrs: { src: "/Users/me/Library/noted/images/2024-05-01_shot.jpeg?v=2", alt: "  " } },
      { type: "image", attrs: { src: "data:image/png;base64,iVBORw0KGgo=" } },
      { type: "image", attrs: {} },
    ));
    expect(result.markdown).toBe("*[Image not shared]*\n\n*[Image not shared]*\n\n*[Image not shared]*\n");
    expect(result.omitted).toEqual([
      { kind: "image", detail: "2024-05-01_shot.jpeg" },
      { kind: "image", detail: "Image" },
      { kind: "image", detail: "Image" },
    ]);
    expect(result.markdown).not.toContain("base64");
    expect(result.markdown).not.toContain("/Users/");
  });

  test("an inline image inside a paragraph is replaced in place", () => {
    const result = documentToMarkdown(doc(paragraph(text("See "), { type: "image", attrs: { alt: "chart" } }, text(" here"))));
    expect(result.markdown).toBe("See *[Image not shared]* here\n");
    expect(result.omitted).toEqual([{ kind: "image", detail: "chart" }]);
  });

  test("unknown block nodes keep their text and are recorded once per node", () => {
    const result = documentToMarkdown(doc(
      { type: "callout", attrs: { tone: "warn" }, content: [text("Heads up "), text("now", bold)] },
      { type: "details", content: [paragraph(text("Summary")), { type: "bulletList", content: [listItem(paragraph(text("Point")))] }] },
    ));
    expect(result.markdown).toBe("Heads up **now**\n\nSummary\n\n- Point\n");
    expect(result.omitted).toEqual([
      { kind: "unsupported", detail: "callout" },
      { kind: "unsupported", detail: "details" },
    ]);
  });

  test("unknown inline nodes render their text content escaped", () => {
    const result = documentToMarkdown(doc(paragraph(text("Ping "), { type: "mention", attrs: { id: "u1" }, content: [text("@sam_h")] })));
    expect(result.markdown).toBe("Ping @sam\\_h\n");
    expect(result.omitted).toEqual([{ kind: "unsupported", detail: "mention" }]);
  });

  test("an image-only document is a list of omission lines", () => {
    const result = documentToMarkdown(doc(
      { type: "image", attrs: { alt: "one" } },
      { type: "image", attrs: { alt: "two" } },
    ));
    expect(result.markdown).toBe("*[Image not shared]*\n\n*[Image not shared]*\n");
    expect(result.omitted.map((entry) => entry.detail)).toEqual(["one", "two"]);
  });
});

describe("document Markdown export: determinism", () => {
  const fixture = doc(
    heading(1, text("Launch "), text("plan", bold, italic)),
    { type: "paragraph", attrs: { textAlign: "justify" }, content: [
      text("Read the "), text("brief", link("https://example.com/brief (v2)")), text(" first."), { type: "hardBreak" }, text("* Then reply."),
    ] },
    { type: "bulletList", content: [
      listItem(paragraph(text("Scope")), { type: "taskList", content: [taskItem(true, paragraph(text("Draft"))), taskItem(false, paragraph(text("Review")))] }),
    ] },
    { type: "blockquote", content: [paragraph(text("Ship it", strike))] },
    { type: "codeBlock", attrs: { language: "sh" }, content: [text("bun test")] },
    { type: "horizontalRule" },
    { type: "image", attrs: { src: "images/x.png", alt: "Sketch" } },
    { type: "widget", content: [text("Unknown")] },
  );

  test("serializing twice yields byte-identical output and identical omission records", () => {
    const first = documentToMarkdown(fixture);
    const second = documentToMarkdown(fixture);
    expect(second.markdown).toBe(first.markdown);
    expect(Buffer.from(second.markdown)).toEqual(Buffer.from(first.markdown));
    expect(second.omitted).toEqual(first.omitted);
    expect(first.omitted).toEqual([
      { kind: "image", detail: "Sketch" },
      { kind: "unsupported", detail: "widget" },
    ]);
  });

  test("the fixture renders every construct with no trailing whitespace except hard breaks", () => {
    const { markdown: output } = documentToMarkdown(fixture);
    expect(output).toBe(
      "# Launch ***plan***\n\n" +
        "Read the [brief](https://example.com/brief%20\\(v2\\)) first.  \n" +
        "\\* Then reply.\n\n" +
        "- Scope\n" +
        "  - [x] Draft\n" +
        "  - [ ] Review\n\n" +
        "> ~~Ship it~~\n\n" +
        "```sh\nbun test\n```\n\n" +
        "---\n\n" +
        "*[Image not shared]*\n\n" +
        "Unknown\n",
    );
    for (const line of output.split("\n")) {
      if (line.endsWith(" ")) expect(line.endsWith("  ")).toBe(true);
      expect(line).not.toMatch(/[^ ] $/);
      expect(line).not.toMatch(/\t$/);
    }
    expect(output.endsWith("\n")).toBe(true);
    expect(output.endsWith("\n\n")).toBe(false);
  });

  test("does not mutate its input", () => {
    const snapshot = JSON.stringify(fixture);
    documentToMarkdown(fixture);
    expect(JSON.stringify(fixture)).toBe(snapshot);
  });
});
