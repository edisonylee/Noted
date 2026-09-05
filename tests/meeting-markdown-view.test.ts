import { expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MdBlock } from "../src/MeetingMarkdownView";

test("meeting subheadings render as headings and source jumps stay accessible", () => {
  const html = renderToStaticMarkup(createElement(MdBlock, {
    md: "## Notes\n### Launch\n#### Dependencies\n- Approval is pending. [01:30-02:00]",
    onSource: () => {},
  }));
  expect(html).toContain("<h3>Notes</h3>");
  expect(html).toContain("<h4>Launch</h4>");
  expect(html).toContain("<h5>Dependencies</h5>");
  expect(html).toContain('aria-label="Open transcript at 01:30"');
  expect(html).not.toContain("####");
});
