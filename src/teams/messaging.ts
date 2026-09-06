import type { TeamChatMessage, TeamChatRoom } from "./types";

export function roomLabel(room: TeamChatRoom, user: string) {
  if (room.is_default) return "Team chat";
  return room.kind === "channel"
    ? room.name
    : (room.participants.find((p) => p.id !== user)?.name ?? "Former teammate");
}

// History pages and live edits may arrive out of order. Never restore an older
// body over a newer revision or a deletion that has already been observed.
export function mergeMessages(
  current: TeamChatMessage[],
  incoming: TeamChatMessage[],
) {
  const byId = new Map(current.map((message) => [message.id, message]));
  for (const message of incoming) {
    const previous = byId.get(message.id);
    if (!previous || message.revision >= previous.revision)
      byId.set(message.id, message);
  }
  return [...byId.values()].sort((a, b) => a.created_seq - b.created_seq);
}

// Mirrors the server's previewBody() wording so a sidebar row and a thread
// row never describe the same message differently.
export function messagePreview(message: TeamChatMessage) {
  if (message.deleted_at) return "Message deleted";
  return message.body || "Shared an attachment or meeting";
}

// Today's messages read as a clock time; anything older as a short date.
export function shortTime(iso: string, now = new Date()) {
  const date = new Date(iso);
  return date.toDateString() === now.toDateString()
    ? date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : date.toLocaleDateString([], { month: "short", day: "numeric" });
}
