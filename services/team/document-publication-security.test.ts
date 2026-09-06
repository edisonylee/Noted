import { expect, test } from "bun:test";
import { TeamStore } from "./store";
import { createHandler } from "./server";

function fixture() {
  const s = new TeamStore(":memory:", "setup");
  const setup = s.bootstrap("setup", "Team", "Owner");
  const owner = s.authenticate(setup.token);
  const peer = s.authenticate(
    s.accept(s.invite(owner, setup.org, { name: "Peer", role: "member" }).token)
      .token!,
  );
  const space = s.spaces(owner, setup.org)[0].id;
  const payload = (key = `document:v2:${"a".repeat(64)}`) => ({
    source_key: key,
    title: "Selected document",
    summary: "Reviewed body",
    space_id: space,
    folder_ids: [],
    occurred_at: "2026-09-06T00:00:00Z",
    expected_access_version: s.snapshot(owner, setup.org).access_version,
  });
  return { s, org: setup.org, token: setup.token, owner, peer, space, payload };
}

test("local publication verifies source owner and identity independently of collaborative edit rights", () => {
  const { s, org, owner, peer, payload } = fixture();
  try {
    const input = payload();
    const other = s.publishDocumentCopy(peer, org, input);
    expect(s.note(owner, org, other.id).can_edit).toBe(true);
    expect(() =>
      s.publishDocumentCopy(owner, org, {
        ...input,
        id: other.id,
        revision: other.revision,
      }),
    ).toThrow("does not belong");
    const own = s.publishDocumentCopy(owner, org, input);
    expect(() =>
      s.publishDocumentCopy(owner, org, {
        ...input,
        source_key: `document:v2:${"b".repeat(64)}`,
        id: own.id,
        revision: own.revision,
      }),
    ).toThrow("does not belong");
    expect(() =>
      s.publishDocumentCopy(owner, org, {
        ...input,
        source_key: "document:42",
      }),
    ).toThrow("verified local identity");
    const updated = s.publishDocumentCopy(owner, org, {
      ...input,
      id: own.id,
      revision: own.revision,
      summary: "New reviewed body",
    });
    expect(updated.summary).toBe("New reviewed body");
    expect(s.note(peer, org, other.id).summary).toBe("Reviewed body");
  } finally {
    s.db.close();
  }
});

test("new and updated local copies reject missing or stale audience consent without modifying content", () => {
  const { s, org, owner, peer, payload } = fixture();
  try {
    const restricted = s.createSpace(owner, org, {
      name: "Restricted",
      visibility: "restricted",
    });
    const input = { ...payload(), space_id: restricted.id };
    const note = s.publishDocumentCopy(owner, org, input);
    for (const version of [undefined, null, -1, "1"]) {
      expect(() =>
        s.publishDocumentCopy(owner, org, {
          ...input,
          expected_access_version: version,
        }),
      ).toThrow("review the audience");
    }
    s.updateSpace(owner, org, restricted.id, {
      name: "Now public",
      visibility: "team",
      api_enabled: true,
    });
    expect(() =>
      s.publishDocumentCopy(owner, org, {
        ...input,
        id: note.id,
        revision: note.revision,
        summary: "Must stay local",
      }),
    ).toThrow("review the audience");
    expect(s.note(peer, org, note.id).summary).toBe("Reviewed body");
    expect(() =>
      s.publishDocumentCopy(owner, org, {
        ...input,
        source_key: `document:v2:${"c".repeat(64)}`,
      }),
    ).toThrow("review the audience");
    const current = s.snapshot(owner, org).access_version;
    expect(
      s.publishDocumentCopy(owner, org, {
        ...input,
        id: note.id,
        revision: note.revision,
        expected_access_version: current,
      }).revision,
    ).toBe(note.revision + 1);
    s.grant(owner, org, restricted.id, {
      kind: "member",
      id: peer,
      role: "viewer",
    });
    expect(() =>
      s.publishDocumentCopy(owner, org, {
        ...input,
        expected_access_version: current,
      }),
    ).toThrow("review the audience");
  } finally {
    s.db.close();
  }
});

test("conversation eligibility is checked before content is stored, including changes after preflight", () => {
  const { s, org, owner, peer, payload } = fixture();
  try {
    const restricted = s.createSpace(owner, org, {
      name: "Private",
      visibility: "restricted",
    });
    const channel = s.chatRooms(owner, org)[0];
    const dm = s.createChatRoom(owner, org, {
      kind: "direct",
      member_id: peer,
    });
    expect(s.documentDestinations(owner, org, channel.id)).not.toContain(
      restricted.id,
    );
    const input = {
      ...payload(),
      space_id: restricted.id,
      room_id: channel.id,
    };
    expect(() => s.publishDocumentCopy(owner, org, input)).toThrow("Everyone");
    expect(s.listNotes(owner, org, "", restricted.id)).toHaveLength(0);
    s.grant(owner, org, restricted.id, {
      kind: "member",
      id: peer,
      role: "viewer",
    });
    expect(s.documentDestinations(owner, org, dm.id)).toContain(restricted.id);
    const accepted = { ...payload(), space_id: restricted.id, room_id: dm.id };
    s.grant(owner, org, restricted.id, {
      kind: "member",
      id: peer,
      role: "remove",
    });
    expect(() => s.publishDocumentCopy(owner, org, accepted)).toThrow(
      "review the audience",
    );
    expect(s.listNotes(owner, org, "", restricted.id)).toHaveLength(0);
  } finally {
    s.db.close();
  }
});

test("the local publication endpoint requires a member and cannot fall back to an unchecked note update", async () => {
  const { s, org, owner, token, payload } = fixture();
  try {
    const handler = createHandler(s);
    const request = (bearer: string, body: unknown) =>
      handler(
        new Request(`https://test/v1/orgs/${org}/document-publications`, {
          method: "POST",
          headers: {
            Authorization: `Bearer ${bearer}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify(body),
        }),
      );
    expect((await request("", payload())).status).toBe(401);
    expect(
      (
        await request(token, {
          ...payload(),
          expected_access_version: undefined,
        })
      ).status,
    ).toBe(409);
    expect((await request(token, payload())).status).toBe(201);
    expect(s.snapshot(owner, org).document_publication_review).toBe(true);
  } finally {
    s.db.close();
  }
});
