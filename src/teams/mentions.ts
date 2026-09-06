export { findMentions } from "../../services/team/mentions";

export function mentionQuery(body: string, caret: number) {
  const match = /(?:^|\s)@([^@\n]*)$/.exec(body.slice(0, caret));
  return match ? { start: caret - match[1].length - 1, end: caret, query: match[1] } : null;
}
