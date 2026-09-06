import { describe, expect, test } from "bun:test";
import {
  attachmentPreviewKind,
  decodeAttachmentPreview,
  textAttachmentPreview,
  MAX_PREVIEW_BYTES,
  MAX_TEXT_PREVIEW_CHARACTERS,
} from "../src/teams/attachmentPreviewData";
const file = {
  id: "attachment",
  name: "notes.txt",
  mime: "text/plain",
  size: 5,
};
const data = { ...file, data: btoa("hello") };
describe("attachment previews", () => {
  test("only explicitly supported passive formats can be previewed", () => {
    expect(attachmentPreviewKind("image/png")).toBe("image");
    expect(attachmentPreviewKind("image/jpeg")).toBe("image");
    expect(attachmentPreviewKind("application/pdf")).toBe("pdf");
    expect(attachmentPreviewKind("text/plain")).toBe("text");
    for (const mime of [
      "text/html",
      "image/svg+xml",
      "application/javascript",
      "application/octet-stream",
    ])
      expect(attachmentPreviewKind(mime)).toBeNull();
  });
  test("validates attachment identity, metadata, size, and base64 before allocating preview URLs", () => {
    expect(new TextDecoder().decode(decodeAttachmentPreview(file, data))).toBe(
      "hello",
    );
    for (const patch of [
      { id: "other" },
      { mime: "text/html" },
      { size: 6 },
      { size: MAX_PREVIEW_BYTES + 1 },
      { data: "broken!" },
      { data: btoa("shortened") },
    ])
      expect(() =>
        decodeAttachmentPreview(file, { ...data, ...patch }),
      ).toThrow();
    expect(() =>
      decodeAttachmentPreview(file, {
        ...data,
        data: "a".repeat(Math.ceil(MAX_PREVIEW_BYTES / 3) * 4 + 1),
      }),
    ).toThrow();
  });
  test("text remains literal and oversized previews are bounded with disclosure", () => {
    const source = '<script>alert("literal")</script>\n**not interpreted**';
    expect(textAttachmentPreview(new TextEncoder().encode(source))).toEqual({
      text: source,
      truncated: false,
    });
    const long = textAttachmentPreview(
      new TextEncoder().encode("a".repeat(MAX_TEXT_PREVIEW_CHARACTERS + 1)),
    );
    expect(long.text.length).toBe(MAX_TEXT_PREVIEW_CHARACTERS);
    expect(long.truncated).toBe(true);
    expect(() => textAttachmentPreview(new Uint8Array([255, 255]))).toThrow();
  });
});
