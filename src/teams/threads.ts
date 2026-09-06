import type { TeamThreadSummary, TeamUser } from "./types";

// Page-one refreshes and older pages are not a snapshot: a reply deleted or
// added between requests can move a thread. Keep the summary whose latest
// reply is newest so an older page never rolls a thread backwards.
export function mergeThreads(
  current: TeamThreadSummary[],
  incoming: TeamThreadSummary[],
) {
  const byRoot = new Map(current.map((thread) => [thread.root.id, thread]));
  for (const thread of incoming) {
    const previous = byRoot.get(thread.root.id);
    if (!previous || thread.last_reply_seq >= previous.last_reply_seq)
      byRoot.set(thread.root.id, thread);
  }
  return [...byRoot.values()].sort(
    (a, b) => b.last_reply_seq - a.last_reply_seq,
  );
}

// "Taylor, Alex and you" / "Taylor, Alex and 3 others". The viewer reads as
// "you" and goes last; the server caps the named participants at five.
export function threadParticipants(
  participants: TeamUser[],
  count: number,
  user: string,
) {
  if (count <= 0) return "";
  const names = participants
    .filter((person) => person.id !== user)
    .map((person) => person.name);
  if (participants.some((person) => person.id === user)) names.push("you");
  const others = count - participants.length;
  if (others > 0) names.push(`${others} ${others === 1 ? "other" : "others"}`);
  return names.length > 1
    ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`
    : (names[0] ?? "");
}
