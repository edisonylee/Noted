import type { TeamChatMessage, TeamChatRoom } from "./types";

export function roomLabel(room: TeamChatRoom, user: string) {
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
