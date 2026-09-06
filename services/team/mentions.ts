// Longest names win; ambiguous names never ping multiple people.
export function findMentions<T extends { id: string; name: string }>(body: string, members: T[]) {
  const aliases = new Map<string, T[]>();
  for (const member of members) {
    for (const name of new Set([member.name, member.name.split(/\s+/)[0]])) {
      const key = name.toLocaleLowerCase();
      aliases.set(key, [...(aliases.get(key) ?? []), member]);
    }
  }
  const names = [...aliases.keys()].filter(Boolean).sort((a, b) => b.length - a.length);
  const found: { start: number; end: number; user: T }[] = [];
  for (let i = 0; i < body.length; i++) {
    if (body[i] !== "@" || (i > 0 && /[\p{L}\p{N}_@]/u.test(body[i - 1]))) continue;
    const rest = body.slice(i + 1).toLocaleLowerCase();
    const name = names.find((n) => rest.startsWith(n) && !/[\p{L}\p{N}_]/u.test(rest[n.length] ?? ""));
    if (!name) continue;
    const matches = aliases.get(name)!;
    if (matches.length === 1) found.push({ start: i, end: i + name.length + 1, user: matches[0] });
    i += name.length;
  }
  return found;
}

// Channel references are exact, case-insensitive slug matches; they never ping
// anyone, so nothing here may ever feed unreadMentions() or mentions().
export function findChannelMentions<T extends { id: string; name: string }>(body: string, channels: T[]) {
  const byName = new Map<string, T>();
  for (const room of channels) if (room.name) byName.set(room.name.toLocaleLowerCase(), room);
  const found: { start: number; end: number; room: T }[] = [];
  for (const match of body.matchAll(/#([A-Za-z0-9][A-Za-z0-9_-]*)/g)) {
    const start = match.index!;
    // The "/" guard keeps URL fragments (https://x.test/#design) plain; the
    // letter/digit guard already covers page#design and C#design.
    if (start > 0 && /[\p{L}\p{N}_#\/]/u.test(body[start - 1])) continue;
    const room = byName.get(match[1].toLocaleLowerCase());
    if (room) found.push({ start, end: start + match[0].length, room });
  }
  return found;
}
