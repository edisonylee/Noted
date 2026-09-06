import { describe, expect, test } from "bun:test";
import { messageFormatting } from "../src/teams/messageFormatting";

describe("message formatting", () => {
  test("recognizes inline formatting with exact source ranges", () => {
    const text = "**bold** *italic* ~~old~~ `literal`";
    const spans = messageFormatting(text);
    expect(
      spans.map((span) => [
        span.kind,
        text.slice(span.contentStart, span.contentEnd),
      ]),
    ).toEqual([
      ["bold", "bold"],
      ["italic", "italic"],
      ["strike", "old"],
      ["code", "literal"],
    ]);
  });
  test("code is opaque, supports newlines, and never interprets HTML", () => {
    const text = "```\n**literal** <script>alert(1)</script>\n```";
    expect(messageFormatting(text)).toEqual([
      {
        start: 0,
        end: text.length,
        contentStart: 3,
        contentEnd: text.length - 3,
        kind: "code",
      },
    ]);
    expect(messageFormatting("<img src=x onerror=alert(1)>")).toEqual([]);
  });
  test("leaves incomplete syntax, escaped markers, and identifiers alone", () => {
    expect(messageFormatting("**unfinished")).toEqual([]);
    expect(messageFormatting("\\*literal*")).toEqual([]);
    expect(messageFormatting("snake_case_name")).toEqual([]);
  });
  test("retains Unicode source offsets for mentions and caret decorations", () => {
    const text = "🧑 **@Édison**";
    const [span] = messageFormatting(text);
    expect(text.slice(span.contentStart, span.contentEnd)).toBe("@Édison");
    expect(span.start).toBe(3);
  });
});
