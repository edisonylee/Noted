import { test, expect } from "bun:test";
import { TeamStore } from "./store";
import { createHandler } from "./server";
test("meeting sharing validates recipient access, quote revision, and live source permissions", async () => {
  const s = new TeamStore(":memory:", "setup");
  try {
    const a = s.bootstrap("setup", "Team", "Owner"),
      owner = s.authenticate(a.token),
      peer = s.authenticate(
        s.accept(s.invite(owner, a.org, { name: "Peer", role: "member" }).token)
          .token!,
      );
    const channel = s.chatRooms(owner, a.org)[0],
      dm = s.createChatRoom(owner, a.org, { kind: "direct", member_id: peer });
    const privateSpace = s.createSpace(owner, a.org, {
      name: "Private",
      visibility: "restricted",
    });
    const publish = (space: string, title: string) =>
      s.publish(owner, a.org, {
        space_id: space,
        source_key: crypto.randomUUID(),
        title,
        summary: "Decision: launch Friday",
        transcript: "",
        occurred_at: "2026-09-04T00:00:00Z",
        folder_ids: [],
      });
    const note = publish(privateSpace.id, "Confidential launch");
    const payload = {
      body: "",
      client_id: crypto.randomUUID(),
      meeting: { id: note.id, revision: note.revision, start: 10, length: 13 },
    };
    expect(() => s.sendChatMessage(owner, a.org, channel.id, payload)).toThrow(
      "cannot access",
    );
    expect(() => s.sendChatMessage(owner, a.org, dm.id, payload)).toThrow(
      "cannot access",
    );
    const group = s.saveGroup(owner, a.org, {
      name: "Reviewers",
      member_ids: [peer],
    });
    s.grant(owner, a.org, privateSpace.id, {
      kind: "group",
      id: group.id,
      role: "viewer",
    });
    expect(
      s.meetingShareTargets(owner, a.org, note.id).map((t) => t.id),
    ).toContain(dm.id);
    expect(
      s.meetingShareTargets(owner, a.org, note.id).map((t) => t.id),
    ).not.toContain(channel.id);
    const m = s.sendChatMessage(owner, a.org, dm.id, payload);
    expect(m.has_meeting).toBe(true);
    expect(s.sendChatMessage(owner, a.org, dm.id, payload).id).toBe(m.id);
    expect(s.meetingReference(peer, a.org, m.id).excerpt).toBe("launch Friday");
    expect(s.get("SELECT body FROM chat_messages WHERE id=?", m.id)!.body).toBe(
      "",
    );
    expect(
      JSON.stringify(
        s.get("SELECT * FROM chat_meeting_refs WHERE message_id=?", m.id),
      ),
    ).not.toContain("Confidential");
    const other = publish(privateSpace.id, "Other source");
    expect(() =>
      s.sendChatMessage(owner, a.org, dm.id, {
        ...payload,
        meeting: { ...payload.meeting, id: other.id },
      }),
    ).toThrow("already belongs");
    s.grant(owner, a.org, privateSpace.id, {
      kind: "group",
      id: group.id,
      role: "remove",
    });
    expect(s.meetingReference(peer, a.org, m.id)).toEqual({ available: false });
    expect(() => s.meetingShareTargets(peer, a.org, note.id)).toThrow();
    s.updateNote(owner, a.org, note.id, {
      revision: note.revision,
      title: note.title,
      summary: "Changed source",
      folder_ids: [],
    });
    expect(s.meetingReference(owner, a.org, m.id).updated).toBe(true);
    expect(s.meetingReference(owner, a.org, m.id).excerpt).toBe("");
    expect(() =>
      s.sendChatMessage(owner, a.org, dm.id, {
        ...payload,
        client_id: crypto.randomUUID(),
      }),
    ).toThrow("meeting changed");
    const current = s.note(owner, a.org, note.id);
    s.trash(owner, a.org, note.id, current.revision);
    expect(s.meetingReference(owner, a.org, m.id)).toEqual({
      available: false,
    });
    const publicNote = publish(
      s.spaces(owner, a.org).find((x) => x.visibility === "team")!.id,
      "Public note",
    );
    const publicMessage = s.sendChatMessage(owner, a.org, channel.id, {
      body: "",
      client_id: crypto.randomUUID(),
      meeting: { id: publicNote.id, revision: publicNote.revision },
    });
    expect(s.meetingReference(peer, a.org, publicMessage.id).available).toBe(
      true,
    );
    const handler = createHandler(s);
    expect(
      (
        await handler(
          new Request(
            `https://test/v1/orgs/${a.org}/notes/${publicNote.id}/share-targets`,
            { headers: { Authorization: `Bearer ${a.token}` } },
          ),
        )
      ).status,
    ).toBe(200);
    expect(() =>
      s.meetingReference(
        owner,
        s.createOrg(owner, "Foreign"),
        publicMessage.id,
      ),
    ).toThrow();
  } finally {
    s.db.close();
  }
});
