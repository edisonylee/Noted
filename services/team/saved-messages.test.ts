import { test, expect } from "bun:test";
import { TeamStore } from "./store";
import { createHandler } from "./server";
test("saved messages are private, paginated, idempotent and permission checked", async () => {
  const s = new TeamStore(":memory:", "setup");
  try {
    const a = s.bootstrap("setup", "Team", "Owner"),
      owner = s.authenticate(a.token),
      peerSession = s.accept(
        s.invite(owner, a.org, { name: "Peer", role: "member" }).token,
      ),
      peer = s.authenticate(peerSession.token!),
      outsider = s.authenticate(
        s.accept(
          s.invite(owner, a.org, { name: "Third", role: "member" }).token,
        ).token!,
      );
    const room = s.createChatRoom(owner, a.org, {
      kind: "direct",
      member_id: peer,
    });
    const messages = Array.from({ length: 35 }, (_, i) =>
      s.sendChatMessage(peer, a.org, room.id, {
        body: `Message ${i}`,
        client_id: crypto.randomUUID(),
      }),
    );
    for (const m of messages) {
      s.saveMessage(owner, a.org, m.id, true);
      s.saveMessage(owner, a.org, m.id, true);
    }
    const first = s.savedMessages(owner, a.org, new URLSearchParams());
    expect(first.items).toHaveLength(30);
    const second = s.savedMessages(
      owner,
      a.org,
      new URLSearchParams({ before: String(first.next_before) }),
    );
    expect(second.items).toHaveLength(5);
    expect(
      new Set([...first.items, ...second.items].map((i) => i.message.id)).size,
    ).toBe(35);
    expect(
      s.savedMessages(peer, a.org, new URLSearchParams()).items,
    ).toHaveLength(0);
    expect(s.messageLocation(peer, a.org, messages[0].id).message.saved).toBe(
      false,
    );
    expect(() =>
      s.saveMessage(outsider, a.org, messages[0].id, true),
    ).toThrow();
    const other = s.createOrg(owner, "Other team");
    expect(() => s.saveMessage(owner, other, messages[0].id, true)).toThrow();
    expect(
      s.savedMessages(owner, other, new URLSearchParams()).items,
    ).toHaveLength(0);
    s.saveMessage(owner, a.org, messages[0].id, false);
    expect(s.messageLocation(owner, a.org, messages[0].id).message.saved).toBe(
      false,
    );
    s.changeChatMessage(
      peer,
      a.org,
      messages[34].id,
      { revision: messages[34].revision },
      true,
    );
    expect(
      s
        .savedMessages(owner, a.org, new URLSearchParams())
        .items.map((i) => i.message.id),
    ).not.toContain(messages[34].id);
    const handler = createHandler(s);
    const base = `https://test/v1/orgs/${a.org}`;
    const response = await handler(
      new Request(`${base}/saved-messages`, {
        headers: { Authorization: `Bearer ${peerSession.token}` },
      }),
    );
    expect(response.status).toBe(200);
    expect((await response.json()).items).toHaveLength(0);
    expect(
      (
        await handler(
          new Request(`${base}/chat-messages/${messages[1].id}/saved`, {
            method: "PUT",
            headers: { Authorization: `Bearer ${peerSession.token}` },
            body: JSON.stringify({ active: true, user_id: owner }),
          }),
        )
      ).status,
    ).toBe(200);
    expect(
      s.savedMessages(peer, a.org, new URLSearchParams()).items,
    ).toHaveLength(1);
    s.changeMember(owner, a.org, peer, "remove");
    expect(() => s.savedMessages(peer, a.org, new URLSearchParams())).toThrow();
  } finally {
    s.db.close();
  }
});
