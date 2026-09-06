import { test, expect } from "bun:test";
import { TeamStore } from "./store";
test("pins respect DM membership, are idempotent, update live history, and disappear on deletion", () => {
  const s = new TeamStore(":memory:", "setup");
  try {
    const a = s.bootstrap("setup", "Team", "Owner"),
      owner = s.authenticate(a.token),
      peer = s.authenticate(
        s.accept(s.invite(owner, a.org, { name: "Peer", role: "member" }).token)
          .token!,
      ),
      other = s.authenticate(
        s.accept(
          s.invite(owner, a.org, { name: "Other", role: "member" }).token,
        ).token!,
      );
    const room = s.createChatRoom(owner, a.org, {
        kind: "direct",
        member_id: peer,
      }),
      m = s.sendChatMessage(owner, a.org, room.id, {
        body: "Decision",
        client_id: crypto.randomUUID(),
      });
    const pinned = s.pinMessage(peer, a.org, m.id, true);
    expect(pinned.pinned).toBe(true);
    expect(s.pinMessage(owner, a.org, m.id, true).revision).toBe(
      pinned.revision,
    );
    expect(s.pinnedMessages(owner, a.org, room.id)[0].pinned_by).toBe("Peer");
    expect(() => s.pinMessage(other, a.org, m.id, true)).toThrow();
    expect(() => s.pinnedMessages(other, a.org, room.id)).toThrow();
    expect(() =>
      s.pinMessage(owner, s.createOrg(owner, "Foreign"), m.id, true),
    ).toThrow();
    expect(
      s.chatMessages(
        owner,
        a.org,
        room.id,
        new URLSearchParams({ after: String(m.created_seq) }),
      ).messages[0].pinned,
    ).toBe(true);
    s.changeChatMessage(
      owner,
      a.org,
      m.id,
      { revision: pinned.revision },
      true,
    );
    expect(s.pinnedMessages(peer, a.org, room.id)).toHaveLength(0);
    const channel = s.createChatRoom(owner, a.org, {
        kind: "channel",
        name: "archive",
      }),
      n = s.sendChatMessage(owner, a.org, channel.id, {
        body: "note",
        client_id: crypto.randomUUID(),
      });
    s.updateChatRoom(owner, a.org, channel.id, {
      revision: channel.revision,
      archived: true,
    });
    expect(() => s.pinMessage(owner, a.org, n.id, true)).toThrow();
  } finally {
    s.db.close();
  }
});
