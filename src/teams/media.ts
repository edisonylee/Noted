import type { TeamMediaItem, TeamMediaPage } from "./types";

export type MediaKind = "images" | "files" | "documents";
export const mediaKinds: { id: MediaKind; label: string; noun: string }[] = [
  { id: "images", label: "Images", noun: "image" },
  { id: "files", label: "Files", noun: "file" },
  { id: "documents", label: "Documents", noun: "document" },
];

// One message can carry several attachments and the same document can be
// shared twice, so neither message_id nor note_id alone identifies a row.
export function mediaKey(item: TeamMediaItem) {
  return item.attachment
    ? `a:${item.attachment.id}`
    : `d:${item.message_id}:${item.document?.note_id ?? ""}`;
}

// Page one is authoritative for its own range so a deleted message's rows
// drop out on refresh; rows older than that range (loaded through "Load
// older") are kept. A continuation page appends only rows not yet shown.
export function mergeMedia(
  old: TeamMediaItem[],
  page: TeamMediaPage,
  refresh: boolean,
) {
  const seen = new Set(page.items.map(mediaKey));
  const kept = old.filter(
    (item) =>
      !seen.has(mediaKey(item)) &&
      (!refresh ||
        (page.next_before != null && item.created_seq <= page.next_before)),
  );
  return refresh ? [...page.items, ...kept] : [...kept, ...page.items];
}

export function mediaCountLabel(kind: MediaKind, count: number) {
  const noun = mediaKinds.find((k) => k.id === kind)!.noun;
  return `${count} ${count === 1 ? noun : `${noun}s`}`;
}
