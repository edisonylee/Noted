import { describe, expect, test } from "bun:test";
import {
  countOpenDocumentTasks,
  documentFingerprint,
  documentPlainText,
  extractDocumentTasks,
  normalizeTaskDocument,
  plainTextToDocument,
  storedDocumentOrPlainText,
  todosToTaskDocument,
  type StructuredDocument,
} from "../src/editor/document";

describe("structured task documents", () => {
  test("lifts legacy todos into a task-list document", () => {
    const document = normalizeTaskDocument(undefined, [
      { id: "one", text: "Book the room", completed: false },
      { id: "two", text: "Send the invite", completed: true },
    ]);

    expect(document.type).toBe("doc");
    expect(document.content?.[0]?.type).toBe("taskList");
    expect(extractDocumentTasks(document).map(({ text, completed }) => ({ text, completed }))).toEqual([
      { text: "Book the room", completed: false },
      { text: "Send the invite", completed: true },
    ]);
    expect(countOpenDocumentTasks(document)).toBe(1);
  });

  test("keeps paragraphs, bullets, numbers, and checklists in one document", () => {
    const document: StructuredDocument = {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "Launch notes" }] },
        {
          type: "bulletList",
          content: [
            { type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "Context" }] }] },
          ],
        },
        {
          type: "orderedList",
          content: [
            { type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "Draft" }] }] },
            { type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "Review" }] }] },
          ],
        },
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { checked: false },
              content: [{ type: "paragraph", content: [{ type: "text", text: "Publish" }] }],
            },
          ],
        },
      ],
    };

    expect(documentPlainText(document)).toBe("Launch notes\n- Context\n1. Draft\n2. Review\n- [ ] Publish");
    expect(extractDocumentTasks(document)[0]?.text).toBe("Publish");
  });

  test("extracts nested task items without folding child text into the parent", () => {
    const document: StructuredDocument = {
      type: "doc",
      content: [
        {
          type: "taskList",
          content: [
            {
              type: "taskItem",
              attrs: { checked: false },
              content: [
                { type: "paragraph", content: [{ type: "text", text: "Plan launch" }] },
                {
                  type: "taskList",
                  content: [
                    {
                      type: "taskItem",
                      attrs: { checked: true },
                      content: [{ type: "paragraph", content: [{ type: "text", text: "Pick date" }] }],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    };

    expect(extractDocumentTasks(document).map(({ text, completed }) => ({ text, completed }))).toEqual([
      { text: "Plan launch", completed: false },
      { text: "Pick date", completed: true },
    ]);
    expect(documentPlainText(document)).toBe("- [ ] Plan launch\n  - [x] Pick date");
  });

  test("uses a writable empty checklist for a new task document", () => {
    const document = todosToTaskDocument([]);
    expect(document.content?.[0]?.type).toBe("taskList");
    expect(extractDocumentTasks(document)).toEqual([]);
    expect(documentPlainText(document)).toBe("");
  });

  test("fingerprints equivalent JSON regardless of object key order", () => {
    const frontendOrder: StructuredDocument = {
      type: "doc",
      content: [{ type: "paragraph", attrs: { align: "left", level: 1 } }],
    };
    const backendOrder = {
      content: [{ attrs: { level: 1, align: "left" }, type: "paragraph" }],
      type: "doc",
    } as StructuredDocument;

    expect(documentFingerprint(frontendOrder)).toBe(documentFingerprint(backendOrder));
  });

  test("keeps image bytes out of document text while preserving useful context", () => {
    const document: StructuredDocument = {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "Reference" }] },
        {
          type: "image",
          attrs: {
            src: "/managed/images/123.png",
            localPath: "/managed/images/123.png",
            alt: "whiteboard.png",
          },
        },
      ],
    };

    expect(documentPlainText(document)).toBe("Reference\n[Image: whiteboard.png]");
    expect(documentFingerprint(document)).not.toContain("data:image");
  });

  test("treats an image inside an empty checklist row as saveable content", () => {
    const document: StructuredDocument = {
      type: "doc",
      content: [{
        type: "taskList",
        content: [{
          type: "taskItem",
          attrs: { checked: false },
          content: [
            { type: "paragraph" },
            {
              type: "image",
              attrs: {
                src: "/managed/images/receipt.jpg",
                localPath: "/managed/images/receipt.jpg",
                alt: "receipt.jpg",
              },
            },
          ],
        }],
      }],
    };

    expect(documentPlainText(document)).toBe("- [ ] [Image: receipt.jpg]");
    expect(extractDocumentTasks(document)).toEqual([]);
  });
});

describe("shared document storage", () => {
  test("lifts legacy meeting notes without losing line breaks", () => {
    const document = plainTextToDocument("Decision: ship Tuesday\n\nOwner: Maya");

    expect(document.content).toHaveLength(3);
    expect(documentPlainText(document)).toBe("Decision: ship Tuesday\nOwner: Maya");
  });

  test("prefers a valid stored rich document", () => {
    const stored = JSON.stringify({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "Bold decision", marks: [{ type: "bold" }] }],
        },
      ],
    });

    const document = storedDocumentOrPlainText(stored, "Legacy fallback");
    expect(document.content?.[0]?.content?.[0]?.marks).toEqual([{ type: "bold" }]);
    expect(documentPlainText(document)).toBe("Bold decision");
  });

  test("falls back to preserved text when rich JSON is damaged", () => {
    expect(documentPlainText(storedDocumentOrPlainText("{broken", "Still here"))).toBe("Still here");
  });
});
