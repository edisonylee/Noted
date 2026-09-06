import { TeamError } from "./store";

export const MAX_ATTACHMENT_BYTES = 5 * 1024 * 1024;
export const MAX_ATTACHMENT_COUNT = 3;
export const TEAM_ATTACHMENT_QUOTA = 250 * 1024 * 1024;

export function validateAttachments(value: unknown) {
  if (value == null) return [];
  if (!Array.isArray(value) || value.length > MAX_ATTACHMENT_COUNT)
    throw new TeamError(400, "Attach up to three files");
  let total = 0;
  return value.map((item) => {
    if (
      !item ||
      typeof item !== "object" ||
      typeof item.name !== "string" ||
      typeof item.data !== "string"
    )
      throw new TeamError(400, "Invalid attachment");
    const name = item.name.normalize("NFC");
    if (
      !name.trim() ||
      name.length > 180 ||
      /[\x00-\x1f\x7f/\\\u202a-\u202e\u2066-\u2069]/u.test(name) ||
      name.startsWith(".")
    )
      throw new TeamError(400, "Choose a file with a simple filename");
    if (
      item.data.length > Math.ceil(MAX_ATTACHMENT_BYTES / 3) * 4 ||
      !/^[A-Za-z0-9+/]+={0,2}$/.test(item.data)
    )
      throw new TeamError(400, "Invalid attachment data or size");
    const bytes = Buffer.from(item.data, "base64");
    if (
      !bytes.length ||
      bytes.toString("base64") !== item.data ||
      (total += bytes.length) > MAX_ATTACHMENT_BYTES
    )
      throw new TeamError(413, "Attachments must total 5 MiB or less");
    const ext = name.split(".").at(-1)?.toLowerCase();
    let mime = "";
    if (
      ext === "png" &&
      bytes.length >= 24 &&
      bytes
        .subarray(0, 8)
        .equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
    ) {
      if (bytes.readUInt32BE(16) > 16000 || bytes.readUInt32BE(20) > 16000)
        throw new TeamError(400, "Image dimensions are too large");
      mime = "image/png";
    }
    if (
      ["jpg", "jpeg"].includes(ext ?? "") &&
      bytes[0] === 255 &&
      bytes[1] === 216 &&
      bytes[2] === 255
    )
      mime = "image/jpeg";
    if (ext === "pdf" && bytes.subarray(0, 5).toString() === "%PDF-")
      mime = "application/pdf";
    if (["txt", "md", "csv"].includes(ext ?? "")) {
      try {
        new TextDecoder("utf-8", { fatal: true }).decode(bytes);
        if (bytes.includes(0)) throw new Error();
        mime = "text/plain";
      } catch {
        throw new TeamError(400, "Text attachments must be UTF-8");
      }
    }
    if (!mime)
      throw new TeamError(
        400,
        "Choose PNG, JPEG, PDF, or UTF-8 text files with matching contents",
      );
    return { name, mime, size: bytes.length, bytes };
  });
}
