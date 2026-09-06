import type {
  TeamChatMessage,
  TeamChatRoom,
  TeamReplyReference,
} from "./types";

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

// Attachments and a reply target change what a send means, so they join the
// idempotency key; a plain body keeps a plain key so existing retries match.
export function sendAttemptKey(
  body: string,
  attachmentIds: string[],
  replyToId: string | null,
  meeting?: { id: string; revision: number } | null,
) {
  if (meeting)
    return JSON.stringify([
      body,
      attachmentIds,
      replyToId,
      meeting.id,
      meeting.revision,
    ]);
  return attachmentIds.length || replyToId
    ? JSON.stringify([body, attachmentIds, replyToId])
    : body;
}

// The pending target shown in the compose bar, shaped like the server's
// reply_to so the bar and the sent row read the same before and after send.
export function replyReference(message: TeamChatMessage): TeamReplyReference {
  return {
    id: message.id,
    author_id: message.author_id,
    author_name: message.author_name,
    body: message.deleted_at ? "" : messagePreview(message).slice(0, 160),
    deleted_at: message.deleted_at,
    created_seq: message.created_seq,
  };
}

// One line of quoted text: newlines collapse so a reference never grows the
// row, and a deleted original reads as a tombstone rather than stale text.
export function quotePreview(
  ref: TeamReplyReference | null | undefined,
  limit = 120,
) {
  if (!ref) return "";
  if (ref.deleted_at) return "Original message deleted";
  const text = ref.body.replace(/\s+/g, " ").trim();
  return text.length > limit ? `${text.slice(0, limit)}…` : text;
}

// Mirrors the server rule: a quote stays at the composer's conversation level
// so the jump target is always inside the timeline the reply is shown in.
export function canReplyInline(
  message: TeamChatMessage,
  threadId: string | undefined,
) {
  return (
    !message.deleted_at && (message.thread_id ?? null) === (threadId ?? null)
  );
}
