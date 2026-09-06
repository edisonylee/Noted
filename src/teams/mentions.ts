import { findChannelMentions, findMentions } from "../../services/team/mentions";
export { findMentions, findChannelMentions };

export function mentionQuery(body: string, caret: number) {
  const match = /(?:^|\s)@([^@\n]*)$/.exec(body.slice(0, caret));
  return match ? { start: caret - match[1].length - 1, end: caret, query: match[1] } : null;
}

// No whitespace inside: a space closes the channel picker (names cannot
// contain spaces). Evaluate this before mentionQuery, whose [^@\n]* would
// otherwise swallow "#des" in "@Ed #des".
export function channelQuery(body: string, caret: number) {
  const match = /(?:^|\s)#([A-Za-z0-9_-]*)$/.exec(body.slice(0, caret));
  return match ? { start: caret - match[1].length - 1, end: caret, query: match[1] } : null;
}

export type BodyMention<U, R> =
  | { kind: "member"; start: number; end: number; user: U }
  | { kind: "channel"; start: number; end: number; room: R };

// Union of people and channel hits sorted by start; an earlier hit wins any
// overlap so a member named "Team #1" is never split into a person and a room.
export function bodyMentions<U extends { id: string; name: string }, R extends { id: string; name: string }>(
  body: string,
  members: U[],
  channels: R[],
): BodyMention<U, R>[] {
  const all: BodyMention<U, R>[] = [
    ...findMentions(body, members).map((m) => ({ kind: "member" as const, ...m })),
    ...findChannelMentions(body, channels).map((c) => ({ kind: "channel" as const, ...c })),
  ].sort((a, b) => a.start - b.start);
  const out: BodyMention<U, R>[] = [];
  let end = 0;
  for (const hit of all) {
    if (hit.start < end) continue;
    out.push(hit);
    end = hit.end;
  }
  return out;
}
