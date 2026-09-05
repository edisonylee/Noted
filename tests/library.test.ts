import { describe, expect, test } from "bun:test";
import { isDocumentNote } from "../src/library";

describe("library content types", () => {
  test("documents use their persisted type instead of formatting heuristics", () => {
    expect(isDocumentNote({ note_kind: "document" })).toBe(true);
    expect(isDocumentNote({ note_kind: "capture" })).toBe(false);
  });
});
