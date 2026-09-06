import type { TeamAttachment } from "./types";

export const MAX_PREVIEW_BYTES = 5 * 1024 * 1024;
export const MAX_TEXT_PREVIEW_CHARACTERS = 200_000;
export type PreviewKind = "image" | "pdf" | "text";
export function attachmentPreviewKind(mime: string): PreviewKind | null {
  if (mime === "image/png" || mime === "image/jpeg") return "image";
  if (mime === "application/pdf") return "pdf";
  if (mime === "text/plain") return "text";
  return null;
}

/** Restrict preview decoding to the server's supported, bounded attachment types. */
export function decodeAttachmentPreview(
  expected: TeamAttachment,
  response: TeamAttachment & { data: string },
) {
  if (
    response.id !== expected.id ||
    response.mime !== expected.mime ||
    response.size !== expected.size ||
    !attachmentPreviewKind(response.mime) ||
    !Number.isInteger(response.size) ||
    response.size <= 0 ||
    response.size > MAX_PREVIEW_BYTES ||
    typeof response.data !== "string" ||
    response.data.length > Math.ceil(MAX_PREVIEW_BYTES / 3) * 4
  )
    throw new Error("This file cannot be previewed.");
  const bytes = Uint8Array.from(atob(response.data), (character) =>
    character.charCodeAt(0),
  );
  if (bytes.length !== response.size)
    throw new Error("The attachment is incomplete. Try again.");
  return bytes;
}
export function textAttachmentPreview(bytes: Uint8Array) {
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  return {
    text: text.slice(0, MAX_TEXT_PREVIEW_CHARACTERS),
    truncated: text.length > MAX_TEXT_PREVIEW_CHARACTERS,
  };
}
