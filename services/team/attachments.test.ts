import { test, expect } from "bun:test";
import { TeamStore } from "./store";
import { createHandler } from "./server";
import { validateAttachments, TEAM_ATTACHMENT_QUOTA } from "./attachments";
const file = {
  name: "notes.txt",
  data: Buffer.from("<script>untrusted text</script>").toString("base64"),
};
function fixture() {
  const s = new TeamStore(":memory:", "setup");
  const a = s.bootstrap("setup", "Team", "Owner");
  const owner = s.authenticate(a.token);
  const memberSession = s.accept(
    s.invite(owner, a.org, { name: "Member", role: "member" }).token,
  );
  const member = s.authenticate(memberSession.token!);
  const outsider = s.authenticate(
    s.accept(s.invite(owner, a.org, { name: "Other", role: "member" }).token)
      .token!,
  );
  const room = s.createChatRoom(owner, a.org, {
    kind: "direct",
    member_id: member,
  });
  return { s, a, owner, member, outsider, room };
}
test("attachments are atomic, idempotent, private, and removed with messages", () => {
  const { s, a, owner, member, outsider, room } = fixture();
  try {
    const body = {
      body: "",
      client_id: crypto.randomUUID(),
      attachments: [file],
    };
    const m = s.sendChatMessage(owner, a.org, room.id, body);
    const id = m.attachments![0].id;
    expect(s.sendChatMessage(owner, a.org, room.id, body).id).toBe(m.id);
    expect(s.attachment(member, a.org, id).data).toBe(file.data);
    expect(() => s.attachment(outsider, a.org, id)).toThrow();
    const other = s.createOrg(owner, "Other org");
    expect(() => s.attachment(owner, other, id)).toThrow();
    expect(() =>
      s.sendChatMessage(owner, a.org, room.id, {
        ...body,
        attachments: [{ ...file, name: "different.txt" }],
      }),
    ).toThrow();
    s.changeMember(owner, a.org, member, "remove");
    expect(() => s.attachment(member, a.org, id)).toThrow();
    s.changeChatMessage(owner, a.org, m.id, { revision: m.revision }, true);
    expect(() => s.attachment(owner, a.org, id)).toThrow();
    expect(s.get("SELECT count(*) n FROM chat_attachments")!.n).toBe(0);
  } finally {
    s.db.close();
  }
});
test("attachment validation rejects traversal, active formats, mismatched signatures, oversized data and quota overflow", () => {
  for (const name of [
    "../secret.txt",
    "a/b.txt",
    ".hidden.txt",
    "bad\n.txt",
    "a.svg",
    "a.html",
    "fake.pdf",
  ])
    expect(() => validateAttachments([{ ...file, name }])).toThrow();
  expect(() => validateAttachments(Array(4).fill(file))).toThrow();
  expect(() =>
    validateAttachments([
      {
        name: "large.txt",
        data: Buffer.alloc(5 * 1024 * 1024 + 1, 65).toString("base64"),
      },
    ]),
  ).toThrow();
  const { s, a, owner, room } = fixture();
  try {
    const m = s.sendChatMessage(owner, a.org, room.id, {
      body: "file",
      client_id: crypto.randomUUID(),
      attachments: [file],
    });
    // Simulate existing allocation at the team quota without allocating 250 MiB.
    for (let i = 0; i < 50; i++)
      s.run(
        "INSERT INTO chat_attachments(id,message_id,name,mime,size,data) VALUES(?,?,?,?,?,?)",
        crypto.randomUUID(),
        m.id,
        "used.txt",
        "text/plain",
        TEAM_ATTACHMENT_QUOTA / 50,
        new Uint8Array([1]),
      );
    expect(() =>
      s.sendChatMessage(owner, a.org, room.id, {
        body: "new",
        client_id: crypto.randomUUID(),
        attachments: [file],
      }),
    ).toThrow("storage limit");
  } finally {
    s.db.close();
  }
});
test("HTTP downloads require authentication and request bodies are bounded", async () => {
  const { s, a, owner, room } = fixture();
  try {
    const handler = createHandler(s);
    const m = s.sendChatMessage(owner, a.org, room.id, {
      body: "file",
      client_id: crypto.randomUUID(),
      attachments: [file],
    });
    const url = `https://test/v1/orgs/${a.org}/attachments/${m.attachments![0].id}`;
    expect((await handler(new Request(url))).status).toBe(401);
    const response = await handler(
      new Request(url, { headers: { Authorization: `Bearer ${a.token}` } }),
    );
    expect(response.status).toBe(200);
    expect(response.headers.get("Cache-Control")).toBe("no-store");
    expect(
      (
        await handler(
          new Request(
            `https://test/v1/orgs/${a.org}/chat-rooms/${room.id}/messages`,
            {
              method: "POST",
              headers: {
                Authorization: `Bearer ${a.token}`,
                "Content-Length": "7100001",
              },
              body: "{}",
            },
          ),
        )
      ).status,
    ).toBe(413);
  } finally {
    s.db.close();
  }
});
