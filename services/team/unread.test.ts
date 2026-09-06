import { test, expect } from "bun:test";
import { TeamStore } from "./store";
test("manual unread is versioned, durable, and immune to stale acknowledgments", () => {
  const s = new TeamStore(":memory:", "setup");
  try {
    const a = s.bootstrap("setup", "Team", "Owner"),
      owner = s.authenticate(a.token),
      b = s.accept(
        s.invite(owner, a.org, { name: "Peer", role: "member" }).token,
      ),
      peer = s.authenticate(b.token!);
    const room = s.chatRooms(owner, a.org)[0],
      first = s.sendChatMessage(peer, a.org, room.id, {
        body: "first",
        client_id: crypto.randomUUID(),
      }),
      reply = s.sendChatMessage(peer, a.org, room.id, {
        body: "reply",
        thread_id: first.id,
        client_id: crypto.randomUUID(),
      });
    const latest = s.chatRoom(owner, a.org, room.id).notification_cursor!;
    s.readChat(owner, a.org, room.id, latest);
    expect(s.chatRoom(owner, a.org, room.id).unread).toBe(0);
    const marked = s.markChatUnread(owner, a.org, room.id, reply.id);
    expect(marked.first_unread_id).toBe(reply.id);
    expect(marked.first_unread_root_id).toBe(first.id);
    expect(marked.unread).toBe(1);
    expect(() => s.readChat(owner, a.org, room.id, latest, 0)).toThrow(
      "Read state changed",
    );
    expect(
      s.readChat(owner, a.org, room.id, latest, marked.read_version),
    ).toEqual({ held: true });
    expect(s.chatRoom(owner, a.org, room.id).unread).toBe(1);
    expect(s.chatRoom(peer, a.org, room.id).read_held).toBe(false);
    s.readChat(owner, a.org, room.id, latest, marked.read_version, true);
    expect(s.chatRoom(owner, a.org, room.id).unread).toBe(0);
    const other = s.createChatRoom(owner, a.org, {
      kind: "channel",
      name: "another",
    });
    expect(() => s.markChatUnread(owner, a.org, other.id, first.id)).toThrow();
    s.changeChatMessage(
      peer,
      a.org,
      reply.id,
      { revision: reply.revision },
      true,
    );
    expect(() => s.markChatUnread(owner, a.org, room.id, reply.id)).toThrow();
  } finally {
    s.db.close();
  }
});
