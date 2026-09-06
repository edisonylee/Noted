import { describe, test, expect, afterEach } from "bun:test";
import { TeamStore } from "./store";
import { createHandler, openServiceStore } from "./server";
import { mkdtempSync, rmSync, readFileSync } from "node:fs";
import { Database } from "bun:sqlite";
import { tmpdir } from "node:os";
import { join as joinPath } from "node:path";
import { createMcpHandler, apiClient } from "./mcp";
import { ensureExampleWorkspace } from "./examples";
import { findChannelMentions } from "./mentions";
const stores: TeamStore[] = [];
afterEach(() => {
  stores.splice(0).forEach((s) => s.db.close());
});
function fixture() {
  const s = new TeamStore(":memory:", "setup-key-for-deterministic-tests-only");
  stores.push(s);
  const setup = s.bootstrap(
    "setup-key-for-deterministic-tests-only",
    "Acme",
    "Owner",
  );
  const owner = s.authenticate(setup.token),
    org = setup.org,
    space = s.spaces(owner, org)[0].id;
  const join = (name: string, role: "member" | "admin" = "member") => {
    const invitation = s.invite(owner, org, { name, role });
    const session = s.accept(invitation.token);
    return { id: s.authenticate(session.token!), token: session.token };
  };
  const publish = (user: string, dest = space, title = "Launch review") =>
    s.publish(user, org, {
      space_id: dest,
      source_key: crypto.randomUUID(),
      title,
      summary: "Launch moves to Friday. Taylor owns the checklist.",
      transcript: "[00:10] Taylor: I will own the launch checklist.",
      occurred_at: "2026-09-04T15:00:00Z",
      folder_ids: [],
    });
  return { s, owner, org, space, join, publish, token: setup.token };
}

describe("threaded chat, reactions, and profiles", () => {
  test("replies remain in one thread, update its summary, paginate, and reject cross-room parents", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0];
    const send = (body: string, thread_id?: string) =>
      s.sendChatMessage(member, org, room.id, {
        body,
        thread_id,
        client_id: crypto.randomUUID(),
      });
    const root = send("Release review");
    const before = s.chatMessages(
      owner,
      org,
      room.id,
      new URLSearchParams(),
    ).cursor;
    const replies = Array.from({ length: 56 }, (_, i) =>
      send(`Reply ${i}`, root.id),
    );
    const main = s.chatMessages(owner, org, room.id, new URLSearchParams());
    expect(main.messages.map((m) => m.id)).toEqual([root.id]);
    expect(main.messages[0].reply_count).toBe(56);
    const thread = s.chatMessages(
      owner,
      org,
      room.id,
      new URLSearchParams({ thread: root.id }),
    );
    expect(thread.parent?.id).toBe(root.id);
    expect(thread.messages).toHaveLength(50);
    const earlier = s.chatMessages(
      owner,
      org,
      room.id,
      new URLSearchParams({
        thread: root.id,
        before: String(thread.older_before),
      }),
    );
    expect(earlier.messages).toHaveLength(6);
    expect(earlier.messages[0].id).toBe(replies[0].id);
    expect(
      s
        .chatMessages(
          owner,
          org,
          room.id,
          new URLSearchParams({ after: String(before) }),
        )
        .messages.every((m) => !m.thread_id),
    ).toBe(true);
    expect(() => send("Nested", replies[0].id)).toThrow("original thread");
    const other = s.createChatRoom(owner, org, {
      kind: "channel",
      name: "Other",
    });
    expect(() =>
      s.sendChatMessage(owner, org, other.id, {
        body: "Cross-room",
        thread_id: root.id,
        client_id: crypto.randomUUID(),
      }),
    ).toThrow();
    const edited = s.changeChatMessage(member, org, replies[0].id, {
      body: "Revised",
      revision: replies[0].revision,
    });
    s.changeChatMessage(
      member,
      org,
      replies[0].id,
      { revision: edited.revision },
      true,
    );
    expect(
      s.chatMessages(owner, org, room.id, new URLSearchParams()).messages[0]
        .reply_count,
    ).toBe(55);
    const currentRoot = s.chatMessages(
      owner,
      org,
      room.id,
      new URLSearchParams(),
    ).messages[0];
    s.changeChatMessage(
      member,
      org,
      root.id,
      { revision: currentRoot.revision },
      true,
    );
    expect(
      s.chatMessages(
        owner,
        org,
        room.id,
        new URLSearchParams({ thread: root.id }),
      ).parent?.deleted_at,
    ).toBeTruthy();
    expect(() => send("New reply", root.id)).toThrow("deleted");
  });
  test("reply retries cannot move between threads and DM threads and reactions stay private", () => {
    const { s, owner, org, join } = fixture();
    const alice = join("Alice").id,
      bob = join("Bob").id;
    const room = s.createChatRoom(alice, org, {
      kind: "direct",
      member_id: bob,
    });
    const root = s.sendChatMessage(alice, org, room.id, {
      body: "Private",
      client_id: crypto.randomUUID(),
    });
    const payload = {
      body: "Private reply",
      client_id: crypto.randomUUID(),
      thread_id: root.id,
    };
    const reply = s.sendChatMessage(bob, org, room.id, payload);
    expect(s.sendChatMessage(bob, org, room.id, payload).id).toBe(reply.id);
    expect(() =>
      s.sendChatMessage(bob, org, room.id, { ...payload, thread_id: null }),
    ).toThrow();
    expect(() =>
      s.chatMessages(
        owner,
        org,
        room.id,
        new URLSearchParams({ thread: root.id }),
      ),
    ).toThrow();
    expect(() =>
      s.reactToMessage(owner, org, reply.id, { emoji: "👍", active: true }),
    ).toThrow();
    s.changeMember(owner, org, bob, "remove");
    expect(() =>
      s.chatMessages(
        bob,
        org,
        room.id,
        new URLSearchParams({ thread: root.id }),
      ),
    ).toThrow();
    expect(() =>
      s.reactToMessage(alice, org, reply.id, { emoji: "👍", active: true }),
    ).toThrow();
  });
  test("reactions are idempotent per person, appear in live changes, and disappear on deletion", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id,
      room = s.chatRooms(owner, org)[0];
    const message = s.sendChatMessage(owner, org, room.id, {
      body: "Ready",
      client_id: crypto.randomUUID(),
    });
    const before = s.chatMessages(
      owner,
      org,
      room.id,
      new URLSearchParams(),
    ).cursor;
    s.reactToMessage(member, org, message.id, { emoji: "🎉", active: true });
    const again = s.reactToMessage(member, org, message.id, {
      emoji: "🎉",
      active: true,
    });
    expect(again.reactions?.[0].count).toBe(1);
    expect(again.reactions?.[0].reacted).toBe(true);
    const both = s.reactToMessage(owner, org, message.id, {
      emoji: "🎉",
      active: true,
    });
    expect(both.reactions?.[0].count).toBe(2);
    expect(both.reactions?.[0].names).toContain("Taylor");
    const onlyMember = s.reactToMessage(owner, org, message.id, {
      emoji: "🎉",
      active: false,
    });
    expect(onlyMember.reactions?.[0].count).toBe(1);
    expect(onlyMember.reactions?.[0].reacted).toBe(false);
    expect(
      s.chatMessages(
        owner,
        org,
        room.id,
        new URLSearchParams({ after: String(before) }),
      ).messages[0].revision,
    ).toBe(onlyMember.revision);
    expect(() =>
      s.reactToMessage(member, org, message.id, {
        emoji: "<script>",
        active: true,
      }),
    ).toThrow();
    s.changeChatMessage(
      owner,
      org,
      message.id,
      { revision: onlyMember.revision },
      true,
    );
    expect(
      s.chatMessages(owner, org, room.id, new URLSearchParams()).messages[0]
        .reactions,
    ).toEqual([]);
    expect(() =>
      s.reactToMessage(member, org, message.id, { emoji: "👍", active: true }),
    ).toThrow();
  });
  test("live HTTP waits wake immediately and recheck membership and session authorization", async () => {
    const { s, owner, org, join, token } = fixture();
    const member = join("Taylor"),
      room = s.chatRooms(owner, org)[0],
      handler = createHandler(s);
    const cursor = s.chatMessages(
      owner,
      org,
      room.id,
      new URLSearchParams(),
    ).cursor;
    const wait = (session: string, after: number, signal?: AbortSignal) =>
      handler(
        new Request(
          `https://team.test/v1/orgs/${org}/chat-rooms/${room.id}/messages?after=${after}&wait=20000`,
          { headers: { authorization: `Bearer ${session}` }, signal },
        ),
      );
    // Flush earlier setup notifications before opening a live request.
    await Promise.resolve();
    const request = wait(member.token!, cursor);
    const message = s.sendChatMessage(owner, org, room.id, {
      body: "Delivered live",
      client_id: crypto.randomUUID(),
    });
    const response = await Promise.race([
      request,
      new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error("Live update did not wake")), 1000),
      ),
    ]);
    expect(response.status).toBe(200);
    const page = await response.json();
    expect(page.messages[0].id).toBe(message.id);
    const revoked = wait(member.token!, page.cursor);
    s.changeMember(owner, org, member.id, "remove");
    expect((await revoked).status).toBe(404);
    const signedOut = wait(token, page.cursor);
    s.signout(token);
    expect((await signedOut).status).toBe(401);
  });
  test("profile edits are self-only, validate uploads, and preserve identity and historical authorship", async () => {
    const { s, owner, org, join, token } = fixture();
    const member = join("Taylor"),
      handler = createHandler(s),
      room = s.chatRooms(owner, org)[0];
    const message = s.sendChatMessage(member.id, org, room.id, {
      body: "Hello",
      client_id: crypto.randomUUID(),
    });
    const photo =
      "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a5VQAAAAASUVORK5CYII=";
    const profile = s.updateProfile(member.id, {
      name: "Taylor Chen",
      title: "Designer",
      about: "Product and research",
      avatar_data: photo,
      revision: 0,
    });
    expect(profile.id).toBe(member.id);
    expect(profile.name).toBe("Taylor Chen");
    expect(
      s.members(owner, org).find((p) => p.id === member.id)?.avatar_version,
    ).toBeTruthy();
    expect(s.profileAvatar(owner, org, member.id).data).toBe(photo);
    expect(
      s
        .chatMessages(owner, org, room.id, new URLSearchParams())
        .messages.find((m) => m.id === message.id)?.author_name,
    ).toBe("Taylor Chen");
    expect(() =>
      s.updateProfile(member.id, { ...profile, revision: 0 }),
    ).toThrow("changed");
    expect(() =>
      s.updateProfile(member.id, {
        ...profile,
        avatar_data: "https://example.com/image.png",
      }),
    ).toThrow();
    expect(() =>
      s.updateProfile(member.id, {
        ...profile,
        avatar_data: "data:image/svg+xml;base64,PHN2Zz4=",
      }),
    ).toThrow();
    const res = await handler(
      new Request("https://team.test/v1/profile", {
        method: "PATCH",
        headers: {
          authorization: `Bearer ${member.token}`,
          "content-type": "application/json",
        },
        body: JSON.stringify({
          ...profile,
          user_id: owner,
          name: "Taylor Updated",
          avatar_data: "",
        }),
      }),
    );
    expect(res.status).toBe(200);
    expect(s.profile(owner).name).toBe("Owner");
    expect(s.profile(member.id).avatar_data).toBe("");
    const other = s.createOrg(owner, "Other");
    expect(() => s.profileAvatar(owner, other, member.id)).toThrow();
    const unauthorized = await handler(
      new Request(
        `https://team.test/v1/orgs/${other}/profiles/${member.id}/avatar`,
        { headers: { authorization: `Bearer ${token}` } },
      ),
    );
    expect(unauthorized.status).toBe(404);
  });
  test("thread replies, reactions and profiles survive a database restart", () => {
    const dir = mkdtempSync(joinPath(tmpdir(), "noted-chat-details-"));
    const path = joinPath(dir, "team.sqlite");
    let s = new TeamStore(path, "test-bootstrap-key");
    const session = s.bootstrap("test-bootstrap-key", "Team", "Owner"),
      owner = s.authenticate(session.token),
      room = s.chatRooms(owner, session.org)[0];
    const root = s.sendChatMessage(owner, session.org, room.id, {
      body: "Root",
      client_id: crypto.randomUUID(),
    });
    const reply = s.sendChatMessage(owner, session.org, room.id, {
      body: "Reply",
      thread_id: root.id,
      client_id: crypto.randomUUID(),
    });
    s.reactToMessage(owner, session.org, reply.id, {
      emoji: "✅",
      active: true,
    });
    const quote = s.sendChatMessage(owner, session.org, room.id, {
      body: "Quote",
      reply_to_id: root.id,
      client_id: crypto.randomUUID(),
    });
    s.updateProfile(owner, { name: "New Name", title: "Founder", revision: 0 });
    s.db.close();
    s = new TeamStore(path);
    try {
      expect(s.authenticate(session.token)).toBe(owner);
      const page = s.chatMessages(
        owner,
        session.org,
        room.id,
        new URLSearchParams({ thread: root.id }),
      );
      expect(page.parent?.reply_count).toBe(1);
      expect(page.messages[0].reactions?.[0].emoji).toBe("✅");
      expect(page.messages[0].author_name).toBe("New Name");
      expect(s.profile(owner).title).toBe("Founder");
      // The thread index is created by the constructor on an existing file.
      expect(
        s.chatThreads(owner, session.org, room.id, new URLSearchParams())
          .items[0].reply_count,
      ).toBe(1);
      // Inline replies: the reply_to_id column survives the reopen guard and
      // quotes resolve the author's current name.
      const quoted = s.messageLocation(owner, session.org, quote.id).message;
      expect(quoted.reply_to_id).toBe(root.id);
      expect(quoted.reply_to?.author_name).toBe("New Name");
      expect(quoted.reply_to?.body).toBe("Root");
      expect(s.chatRoom(owner, session.org, room.id).inline_replies).toBe(true);
    } finally {
      s.db.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("thread listing", () => {
  const query = (value = "") => new URLSearchParams(value);
  const threads = (
    s: TeamStore,
    user: string,
    org: string,
    room: string,
    value = "",
  ) => s.chatThreads(user, org, room, query(value));
  const sender =
    (s: TeamStore, org: string, room: string) =>
    (user: string, body: string, thread_id?: string) =>
      s.sendChatMessage(user, org, room, {
        body,
        thread_id,
        client_id: crypto.randomUUID(),
      });
  test("lists a room's threads newest reply first with counts, participants, last replier, unread and keyset paging", () => {
    const { s, owner, org, join } = fixture();
    const taylor = join("Taylor").id,
      casey = join("Casey").id;
    const room = s.chatRooms(owner, org)[0];
    const send = sender(s, org, room.id);
    const a = send(owner, "A"),
      b = send(owner, "B"),
      c = send(owner, "C");
    send(taylor, "A1", a.id);
    send(taylor, "A2", a.id);
    const c1 = send(taylor, "C1", c.id);
    for (const body of ["B1", "B2", "B3"]) send(casey, body, b.id);
    const page = threads(s, owner, org, room.id);
    expect(page.items.map((t) => t.root.id)).toEqual([b.id, c.id, a.id]);
    expect(page.next_before).toBeNull();
    const [tb, tc, ta] = page.items;
    expect(tb.reply_count).toBe(3);
    expect(tb.participants.map((p) => p.id)).toEqual([casey]);
    expect(tb.participant_count).toBe(1);
    expect(ta.participants.map((p) => p.id)).toEqual([taylor]);
    expect(tc.last_reply_by.name).toBe("Taylor");
    expect(tc.last_reply_at).toBe(c1.created_at);
    expect(tc.last_reply_seq).toBe(c1.created_seq);
    for (const t of page.items) {
      expect(t.root.thread_id).toBeNull();
      expect(t.root.reply_count).toBe(t.reply_count);
    }
    expect([ta, tb, tc].map((t) => t.unread_replies)).toEqual([2, 3, 1]);
    const before = s.chatRoom(owner, org, room.id);
    expect(before.threads_enabled).toBe(true);
    expect(before.unread_threads).toBe(3);
    s.readChat(owner, org, room.id, c1.created_seq, 0);
    const unread = Object.fromEntries(
      threads(s, owner, org, room.id).items.map((t) => [
        t.root.id,
        t.unread_replies,
      ]),
    );
    expect(unread).toEqual({ [a.id]: 0, [b.id]: 3, [c.id]: 0 });
    expect(s.chatRoom(owner, org, room.id).unread_threads).toBe(1);
    expect(
      threads(s, taylor, org, room.id).items.find((t) => t.root.id === a.id)
        ?.unread_replies,
    ).toBe(0);
    for (let i = 0; i < 35; i++)
      send(taylor, `Follow-up ${i}`, send(owner, `Topic ${i}`).id);
    const first = threads(s, owner, org, room.id);
    expect(first.items).toHaveLength(30);
    expect(first.next_before).toBe(first.items[29].last_reply_seq);
    const second = threads(
      s,
      owner,
      org,
      room.id,
      `before=${first.next_before}`,
    );
    expect(second.items).toHaveLength(8);
    expect(second.next_before).toBeNull();
    expect(second.items.map((t) => t.last_reply_seq)).toEqual(
      [...second.items.map((t) => t.last_reply_seq)].sort((x, y) => y - x),
    );
    expect(second.items[0].last_reply_seq).toBeLessThan(first.next_before!);
    expect(
      new Set([...first.items, ...second.items].map((t) => t.root.id)).size,
    ).toBe(38);
    expect(() => threads(s, owner, org, room.id, "before=bad")).toThrow(
      "Invalid threads cursor",
    );
  });
  test("deleted replies and roots keep the listing consistent with reply counts", () => {
    const { s, owner, org, join } = fixture();
    const taylor = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0];
    const send = sender(s, org, room.id);
    const remove = (user: string, id: string) => {
      const current = s.messageLocation(user, org, id).message;
      s.changeChatMessage(user, org, id, { revision: current.revision }, true);
    };
    const lone = send(owner, "Lone");
    const loneReply = send(taylor, "Only reply", lone.id);
    const early = send(owner, "Early");
    const late = send(owner, "Late");
    send(taylor, "Late first", late.id);
    const earlyReply = send(taylor, "Old surviving reply", early.id);
    const lateReply = send(taylor, "Late latest", late.id);
    expect(threads(s, owner, org, room.id).items.map((t) => t.root.id)).toEqual(
      [late.id, early.id, lone.id],
    );
    remove(taylor, loneReply.id);
    expect(
      threads(s, owner, org, room.id).items.some((t) => t.root.id === lone.id),
    ).toBe(false);
    expect(
      s
        .chatMessages(owner, org, room.id, query())
        .messages.find((m) => m.id === lone.id)?.reply_count,
    ).toBe(0);
    remove(owner, early.id);
    const tombstone = threads(s, owner, org, room.id).items.find(
      (t) => t.root.id === early.id,
    )!;
    expect(tombstone.root.deleted_at).toBeTruthy();
    expect(tombstone.root.body).toBe("");
    expect(tombstone.reply_count).toBe(1);
    expect(
      s
        .chatMessages(owner, org, room.id, query(`thread=${early.id}`))
        .messages.map((m) => m.id),
    ).toEqual([earlyReply.id]);
    s.changeChatMessage(taylor, org, earlyReply.id, {
      body: "Edited old reply",
      revision: earlyReply.revision,
    });
    expect(threads(s, owner, org, room.id).items.map((t) => t.root.id)).toEqual(
      [late.id, early.id],
    );
    remove(taylor, lateReply.id);
    expect(threads(s, owner, org, room.id).items.map((t) => t.root.id)).toEqual(
      [early.id, late.id],
    );
  });
  test("thread listing enforces conversation, org and membership access", () => {
    const { s, owner, org, join } = fixture();
    const alice = join("Alice").id,
      bob = join("Bob").id,
      admin = join("Admin", "admin").id;
    const dm = s.createChatRoom(alice, org, { kind: "direct", member_id: bob });
    const send = sender(s, org, dm.id);
    send(bob, "Private reply", send(alice, "Private root").id);
    for (const outsider of [owner, admin])
      expect(() => threads(s, outsider, org, dm.id)).toThrow("access removed");
    expect(threads(s, bob, org, dm.id).items).toHaveLength(1);
    s.changeMember(owner, org, bob, "remove");
    expect(() => threads(s, bob, org, dm.id)).toThrow();
    const remaining = threads(s, alice, org, dm.id);
    expect(remaining.items).toHaveLength(1);
    expect(remaining.items[0].root.can_edit).toBe(false);
    const beta = s.createOrg(owner, "Beta");
    expect(() => threads(s, owner, org, `general-${beta}`)).toThrow();
    const channel = s.createChatRoom(owner, org, {
      kind: "channel",
      name: "Archive me",
    });
    const post = sender(s, org, channel.id);
    post(alice, "Archived reply", post(owner, "Archived root").id);
    s.updateChatRoom(owner, org, channel.id, {
      revision: channel.revision,
      archived: true,
    });
    expect(threads(s, owner, org, channel.id).items).toHaveLength(1);
    expect(threads(s, owner, org, `general-${org}`)).toEqual({
      items: [],
      next_before: null,
    });
  });
  test("HTTP thread listing requires a member session, rejects integration keys and unknown routes", async () => {
    const { s, owner, org, token, space, join } = fixture();
    const handler = createHandler(s);
    const request = (path: string, method = "GET", key = token) =>
      handler(
        new Request(`https://team.test/v1/orgs/${org}/${path}`, {
          method,
          headers: {
            Authorization: `Bearer ${key}`,
            "Content-Type": "application/json",
          },
          body: method === "GET" ? undefined : "{}",
        }),
      );
    const room = s.chatRooms(owner, org)[0].id;
    const taylor = join("Taylor").id;
    const send = sender(s, org, room);
    const root = send(owner, "Seeded root");
    send(taylor, "Seeded reply", root.id);
    s.updateSpace(owner, org, space, {
      ...s.space(owner, org, space),
      api_enabled: true,
    });
    const integration = s.createIntegrationKey(owner, org, {
      name: "Read shared meetings",
      space_ids: [space],
      transcripts: false,
      days: 30,
    });
    const ok = await request(`chat-rooms/${room}/threads`);
    expect(ok.status).toBe(200);
    const json = await ok.json();
    expect(Array.isArray(json.items)).toBe(true);
    expect(json.items.map((t: { root: { id: string } }) => t.root.id)).toEqual([
      root.id,
    ]);
    expect(
      (await request(`chat-rooms/${room}/threads`, "GET", "")).status,
    ).toBe(401);
    expect(
      (
        await request(
          `chat-rooms/${room}/threads`,
          "GET",
          String(integration.token),
        )
      ).status,
    ).toBe(401);
    expect((await request(`chat-rooms/${room}/threads?before=x`)).status).toBe(
      400,
    );
    expect((await request(`chat-rooms/${room}/threadz`)).status).toBe(404);
    expect((await request(`chat-rooms/${room}/threads/extra`)).status).toBe(
      404,
    );
    expect((await request(`chat-rooms/${room}/threads`, "POST")).status).toBe(
      404,
    );
    expect((await request(`chat-rooms/${room}/threads`, "PATCH")).status).toBe(
      404,
    );
    const dm = s.createChatRoom(taylor, org, {
      kind: "direct",
      member_id: join("Morgan").id,
    });
    expect((await request(`chat-rooms/${dm.id}/threads`)).status).toBe(404);
  });
  test("constructor migrations are idempotent across reopens of the same file", () => {
    const dir = mkdtempSync(joinPath(tmpdir(), "noted-thread-migrations-"));
    const path = joinPath(dir, "team.sqlite");
    try {
      new TeamStore(path).db.close();
      const s = new TeamStore(path);
      try {
        const columns = s.all<{ name: string }>(
          "PRAGMA table_info(chat_messages)",
        );
        expect(columns.filter((c) => c.name === "thread_id")).toHaveLength(1);
        expect(columns.filter((c) => c.name === "reply_to_id")).toHaveLength(
          1,
        );
        const indexes = s
          .all<{ name: string }>("PRAGMA index_list(chat_messages)")
          .map((i) => i.name);
        expect(indexes).toContain("chat_thread_history");
        expect(indexes).toContain("chat_thread_replies");
        expect(indexes).toContain("chat_reply_refs");
      } finally {
        s.db.close();
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("inline replies", () => {
  const query = (value = "") => new URLSearchParams(value);
  const sender =
    (s: TeamStore, org: string, room: string) =>
    (
      user: string,
      body: string,
      extra: { thread_id?: string; reply_to_id?: unknown } = {},
    ) =>
      s.sendChatMessage(user, org, room, {
        body,
        ...extra,
        client_id: crypto.randomUUID(),
      });
  test("replies quote a message in the same conversation and read live", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0].id;
    const send = sender(s, org, room);
    const a = send(member, "A body");
    const b = send(owner, "B body", { reply_to_id: a.id });
    expect(b.reply_to_id).toBe(a.id);
    expect(b.reply_to).toEqual({
      id: a.id,
      author_id: member,
      author_name: "Taylor",
      body: "A body",
      deleted_at: null,
      created_seq: a.created_seq,
    });
    // A quote never changes the quoted row.
    expect(s.messageLocation(owner, org, a.id).message.revision).toBe(
      a.revision,
    );
    const page = s.chatMessages(owner, org, room, query());
    expect(page.messages.find((m) => m.id === b.id)?.reply_to?.id).toBe(a.id);
    expect(page.messages.find((m) => m.id === a.id)?.reply_to).toBeNull();
    const around = s.chatMessages(owner, org, room, query(`around=${a.id}`));
    expect(around.messages.find((m) => m.id === b.id)?.reply_to_id).toBe(a.id);
    const long = send(member, "x".repeat(400));
    expect(
      send(owner, "Quoting a long one", { reply_to_id: long.id }).reply_to
        ?.body.length,
    ).toBe(160);
    const file = s.sendChatMessage(member, org, room, {
      body: "",
      client_id: crypto.randomUUID(),
      attachments: [
        { name: "notes.txt", mime: "text/plain", data: btoa("hello") },
      ],
    });
    expect(
      send(owner, "Quoting a file", { reply_to_id: file.id }).reply_to?.body,
    ).toBe("Shared an attachment or meeting");
    expect(send(owner, "Self", { reply_to_id: b.id }).reply_to?.author_id).toBe(
      owner,
    );
    const c = send(member, "C body", { reply_to_id: b.id });
    expect(c.reply_to?.id).toBe(b.id);
    expect(c.reply_to?.body).toBe("B body");
  });
  test("thread scope is enforced", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0].id;
    const send = sender(s, org, room);
    const root = send(member, "Root");
    const t1 = send(member, "In thread", { thread_id: root.id });
    expect(() => send(owner, "Main quoting thread", { reply_to_id: t1.id }))
      .toThrow("in this conversation");
    expect(() =>
      send(owner, "Thread quoting root", {
        thread_id: root.id,
        reply_to_id: root.id,
      }),
    ).toThrow("in this conversation");
    const t2 = send(owner, "Thread quoting t1", {
      thread_id: root.id,
      reply_to_id: t1.id,
    });
    expect(t2.thread_id).toBe(root.id);
    expect(t2.reply_to?.id).toBe(t1.id);
    const thread = s.chatMessages(owner, org, room, query(`thread=${root.id}`));
    expect(thread.messages.map((m) => m.id)).toEqual([t1.id, t2.id]);
    expect(thread.parent?.reply_count).toBe(2);
    expect(
      s
        .chatMessages(owner, org, room, query(`thread=${root.id}&around=${t1.id}`))
        .messages.map((m) => m.id),
    ).toContain(t2.id);
    expect(
      s.chatMessages(owner, org, room, query()).messages[0].reply_count,
    ).toBe(2);
  });
  test("cross-room and private targets are not found", () => {
    const { s, owner, org, join } = fixture();
    const alice = join("Alice").id,
      bob = join("Bob").id;
    const channel = s.chatRooms(owner, org)[0].id;
    const dm = s.createChatRoom(alice, org, { kind: "direct", member_id: bob });
    const secret = sender(s, org, dm.id)(alice, "Private");
    expect(() =>
      sender(s, org, channel)(alice, "Leak", { reply_to_id: secret.id }),
    ).toThrow("Message not found");
    expect(() =>
      sender(s, org, dm.id)(owner, "Intrude", { reply_to_id: secret.id }),
    ).toThrow("access removed");
    const other = s.createChatRoom(owner, org, { kind: "channel", name: "Y" });
    const y = sender(s, org, other.id)(owner, "In Y");
    expect(() =>
      sender(s, org, channel)(owner, "From X", { reply_to_id: y.id }),
    ).toThrow("Message not found");
    expect(() =>
      sender(s, org, channel)(owner, "Unknown", { reply_to_id: "nope" }),
    ).toThrow("Message not found");
  });
  test("deleted originals become tombstones and cannot be targeted", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0].id;
    const send = sender(s, org, room);
    const a = send(member, "Original");
    const b = send(owner, "Quote", { reply_to_id: a.id });
    s.changeChatMessage(member, org, a.id, { revision: a.revision }, true);
    expect(() => send(owner, "Late", { reply_to_id: a.id })).toThrow(
      "replying to was deleted",
    );
    const location = s.messageLocation(owner, org, b.id);
    expect(location.message.reply_to?.deleted_at).toBeTruthy();
    expect(location.message.reply_to?.body).toBe("");
    expect(location.message.reply_to_id).toBe(a.id);
    expect(() => s.messageLocation(owner, org, a.id)).toThrow("deleted");
    s.changeChatMessage(owner, org, b.id, { revision: b.revision }, true);
    const deleted = s
      .chatMessages(owner, org, room, query())
      .messages.find((m) => m.id === b.id);
    expect(deleted?.deleted_at).toBeTruthy();
    expect(deleted?.reply_to).toBeNull();
  });
  test("editing or deleting an original refreshes references live without bumping their revision", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0].id;
    const send = sender(s, org, room);
    const a = send(member, "Original");
    const b = send(owner, "Quote", { reply_to_id: a.id });
    const c = send(owner, "Stale quote", { reply_to_id: a.id });
    s.changeChatMessage(owner, org, c.id, { revision: c.revision }, true);
    const cursor = s.chatMessages(owner, org, room, query()).cursor;
    const unreadBefore = s.chatRoom(member, org, room).unread;
    const edited = s.changeChatMessage(member, org, a.id, {
      body: "Edited",
      revision: a.revision,
    });
    expect(edited.revision).toBe(a.revision + 1);
    const afterEdit = s.chatMessages(owner, org, room, query(`after=${cursor}`));
    expect(afterEdit.messages.map((m) => m.id).sort()).toEqual(
      [a.id, b.id].sort(),
    );
    const quoting = afterEdit.messages.find((m) => m.id === b.id)!;
    expect(quoting.reply_to?.body).toBe("Edited");
    expect(quoting.revision).toBe(b.revision);
    // Fan-out re-emits rows the reader already saw, so it is never unread.
    expect(s.chatRoom(member, org, room).unread).toBe(unreadBefore);
    s.changeChatMessage(member, org, a.id, { revision: edited.revision }, true);
    const afterDelete = s.chatMessages(
      owner,
      org,
      room,
      query(`after=${afterEdit.cursor}`),
    );
    const tombstoned = afterDelete.messages.find((m) => m.id === b.id)!;
    expect(tombstoned.reply_to?.deleted_at).toBeTruthy();
    expect(tombstoned.revision).toBe(b.revision);
    expect(
      afterDelete.messages.find((m) => m.id === a.id)?.revision,
    ).toBe(edited.revision + 1);
    // Thread-level quoting rows reach only the thread panel.
    const root = send(member, "Root");
    const t1 = send(member, "T1", { thread_id: root.id });
    send(owner, "Quoting T1", { thread_id: root.id, reply_to_id: t1.id });
    const mainCursor = s.chatMessages(owner, org, room, query()).cursor;
    s.changeChatMessage(member, org, t1.id, { body: "T1 edited", revision: 1 });
    expect(
      s
        .chatMessages(owner, org, room, query(`after=${mainCursor}`))
        .messages.every((m) => (m.thread_id ?? null) === null),
    ).toBe(true);
    expect(
      s
        .chatMessages(
          owner,
          org,
          room,
          query(`thread=${root.id}&after=${mainCursor}`),
        )
        .messages.find((m) => m.reply_to_id === t1.id)?.reply_to?.body,
    ).toBe("T1 edited");
    // Fan-out is capped: 120 quoting rows produce at most 100 events plus
    // the edited row's own event.
    const popular = send(member, "Popular");
    for (let i = 0; i < 120; i++)
      send(owner, `Quote ${i}`, { reply_to_id: popular.id });
    const seq = Number(
      s.get("SELECT MAX(seq) AS seq FROM chat_events WHERE room_id=?", room)!
        .seq,
    );
    s.changeChatMessage(member, org, popular.id, {
      body: "Popular edited",
      revision: popular.revision,
    });
    expect(
      Number(
        s.get(
          "SELECT COUNT(*) AS n FROM chat_events WHERE room_id=? AND seq>?",
          room,
          seq,
        )!.n,
      ),
    ).toBe(101);
  });
  test("retries cannot change the reply target", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0].id;
    const send = sender(s, org, room);
    const a = send(member, "A");
    const other = send(member, "Other");
    const payload = {
      body: "Quote",
      client_id: crypto.randomUUID(),
      reply_to_id: a.id,
    };
    const first = s.sendChatMessage(owner, org, room, payload);
    expect(s.sendChatMessage(owner, org, room, payload).id).toBe(first.id);
    expect(() =>
      s.sendChatMessage(owner, org, room, { ...payload, reply_to_id: null }),
    ).toThrow("another message");
    expect(() =>
      s.sendChatMessage(owner, org, room, {
        ...payload,
        reply_to_id: other.id,
      }),
    ).toThrow("another message");
    expect(() => send(owner, "Bad", { reply_to_id: 42 })).toThrow(
      "Invalid reply target",
    );
    expect(() =>
      send(owner, "Bad", { reply_to_id: "x".repeat(101) }),
    ).toThrow("Invalid reply target");
  });
  test("search indexes only the reply body, never the quoted text", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0].id;
    const send = sender(s, org, room);
    const original = send(member, "quotedsecretword");
    const reply = send(owner, "replybodyword", { reply_to_id: original.id });
    const hits = (q: string) =>
      s.search(owner, org, query(`q=${q}`)).messages.hits.map((h) => h.id);
    expect(hits("replybodyword")).toEqual([reply.id]);
    expect(hits("quotedsecretword")).toEqual([original.id]);
    expect(
      s.all<{ rowid: number }>(
        "SELECT rowid FROM chat_messages_fts WHERE chat_messages_fts MATCH 'quotedsecretword'",
      ),
    ).toHaveLength(1);
    s.changeChatMessage(member, org, original.id, { revision: 1 }, true);
    expect(hits("quotedsecretword")).toEqual([]);
    expect(hits("replybodyword")).toEqual([reply.id]);
  });
});

describe("team navigation data", () => {
  test("conversation previews stay inside the participant boundary and reflect edits and deletions", () => {
    const { s, owner, org, join } = fixture();
    const alice = join("Alice").id,
      bob = join("Bob").id;
    const dm = s.createChatRoom(alice, org, { kind: "direct", member_id: bob });
    const message = s.sendChatMessage(alice, org, dm.id, {
      body: "Private launch discussion ".repeat(30),
      client_id: crypto.randomUUID(),
    });
    expect(
      s.chatRooms(bob, org).find((room) => room.id === dm.id)?.last_message
        ?.body,
    ).toHaveLength(160);
    expect(JSON.stringify(s.chatRooms(owner, org))).not.toContain(
      "Private launch discussion",
    );
    const edited = s.changeChatMessage(alice, org, message.id, {
      revision: message.revision,
      body: "Revised plan",
    });
    expect(s.chatRoom(bob, org, dm.id).last_message?.body).toBe("Revised plan");
    s.changeChatMessage(
      alice,
      org,
      message.id,
      { revision: edited.revision },
      true,
    );
    expect(s.chatRoom(bob, org, dm.id).last_message?.body).toBe(
      "Message deleted",
    );
    expect(JSON.stringify(s.chatRooms(bob, org))).not.toContain("Revised plan");
    s.changeMember(owner, org, bob, "remove");
    expect(() => s.chatRooms(bob, org)).toThrow();
  });
  test("team names can be changed by admins without changing identity, members, or collections", async () => {
    const { s, owner, org, join, token, space } = fixture();
    const member = join("Member"),
      administrator = join("Admin", "admin");
    const handler = createHandler(s);
    const rename = (session: string, name: string, target = org) =>
      handler(
        new Request(`https://team.test/v1/orgs/${target}`, {
          method: "PATCH",
          headers: {
            authorization: `Bearer ${session}`,
            "content-type": "application/json",
          },
          body: JSON.stringify({ name }),
        }),
      );
    expect((await rename(member.token!, "Unauthorized")).status).toBe(403);
    expect((await rename(administrator.token!, "Fieldwork")).status).toBe(200);
    expect(s.get("SELECT name FROM organizations WHERE id=?", org)?.name).toBe(
      "Fieldwork",
    );
    expect(s.snapshot(owner, org).members).toHaveLength(3);
    expect(s.spaces(owner, org)[0].id).toBe(space);
    expect((await rename(token, " ")).status).toBe(400);
    const other = s.createOrg(owner, "Other team");
    expect(
      (await rename(administrator.token!, "Unauthorized", other)).status,
    ).toBe(404);
    expect((await rename("invalid", "Unauthorized")).status).toBe(401);
  });
});

describe("team member messaging", () => {
  const query = (value = "") => new URLSearchParams(value);
  const message = (body: string) => ({ body, client_id: crypto.randomUUID() });
  test("channels include everyone and enforce creator/admin settings, archive state and revisions", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id,
      peer = join("Morgan").id;
    const general = s.chatRooms(member, org)[0];
    expect(general.name).toBe("general");
    expect(general.is_default).toBe(true);
    const channel = s.createChatRoom(member, org, {
      kind: "channel",
      name: "Project Launch",
      description: "Pilot coordination",
    });
    expect(channel.name).toBe("project-launch");
    expect(s.chatRoom(peer, org, channel.id).can_send).toBe(true);
    expect(() =>
      s.createChatRoom(owner, org, { kind: "channel", name: "PROJECT-LAUNCH" }),
    ).toThrow("already exists");
    expect(() =>
      s.updateChatRoom(peer, org, channel.id, { revision: 1, archived: true }),
    ).toThrow();
    const sent = s.sendChatMessage(
      peer,
      org,
      channel.id,
      message("Pilot is ready"),
    );
    const archived = s.updateChatRoom(member, org, channel.id, {
      revision: 1,
      archived: true,
    });
    expect(archived.can_send).toBe(false);
    expect(
      s.chatMessages(peer, org, channel.id, query()).messages[0].body,
    ).toBe(sent.body);
    expect(() =>
      s.sendChatMessage(peer, org, channel.id, message("Late message")),
    ).toThrow("archived");
    expect(() =>
      s.changeChatMessage(peer, org, sent.id, { revision: 1, body: "Edit" }),
    ).toThrow();
    expect(() =>
      s.updateChatRoom(member, org, channel.id, {
        revision: 1,
        archived: false,
      }),
    ).toThrow("changed");
    expect(
      s.updateChatRoom(owner, org, channel.id, { revision: 2, archived: false })
        .can_send,
    ).toBe(true);
    expect(() =>
      s.updateChatRoom(owner, org, general.id, { revision: 1, archived: true }),
    ).toThrow("general");
  });
  test("channel references are plain text: verbatim, unpinged, searchable, and resolvable by every member", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const design = s.createChatRoom(owner, org, {
      kind: "channel",
      name: "Design Review",
    });
    expect(design.name).toBe("design-review");
    const general = s.chatRooms(member, org).find((r) => r.is_default)!;
    const sent = s.sendChatMessage(
      owner,
      org,
      general.id,
      message("see #design-review and #General"),
    );
    expect(s.messageLocation(member, org, sent.id).message.body).toBe(
      "see #design-review and #General",
    );
    // A channel reference is not a people mention: nothing here may ever
    // count as unread mentions or reach the inbox.
    const room = s.chatRoom(member, org, general.id);
    expect(room.unread).toBe(1);
    expect(room.unread_mentions).toBe(0);
    expect(room.latest_unread_mention_seq ?? 0).toBe(0);
    expect(s.mentions(member, org, query()).items).toHaveLength(0);
    const resolve = (user: string) =>
      findChannelMentions(
        sent.body,
        s.chatRooms(user, org).filter((r) => r.kind === "channel"),
      ).map((h) => h.room.id);
    expect(resolve(member)).toEqual([design.id, general.id]);
    const hits = (q: string) =>
      s.search(member, org, query(`q=${encodeURIComponent(q)}&kind=messages`))
        .messages.hits.some((h) => h.id === sent.id);
    expect(hits("design")).toBe(true);
    expect(hits("#design")).toBe(true);
    const renamed = s.updateChatRoom(owner, org, design.id, {
      name: "Design Ops",
      revision: design.revision,
    });
    expect(renamed.name).toBe("design-ops");
    expect(renamed.name).toMatch(/^[a-z0-9][a-z0-9_-]*$/);
    // Rename rot is the accepted cost of plain text: the old text degrades
    // to readable prose rather than pointing at the wrong room.
    expect(resolve(member)).toEqual([general.id]);
    const archived = s.updateChatRoom(owner, org, design.id, {
      archived: true,
      revision: renamed.revision,
    });
    expect(archived.can_send).toBe(false);
    expect(
      s.chatRooms(member, org).some((r) => r.id === design.id && r.archived_at),
    ).toBe(true);
    const mention = s.sendChatMessage(
      member,
      org,
      general.id,
      message("history lives in #design-ops"),
    );
    const resolveOps = () =>
      findChannelMentions(
        mention.body,
        s.chatRooms(member, org).filter((r) => r.kind === "channel"),
      ).map((h) => [h.room.id, !!h.room.archived_at]);
    expect(resolveOps()).toEqual([[design.id, true]]);
    const restored = s.updateChatRoom(owner, org, design.id, {
      archived: false,
      revision: archived.revision,
    });
    expect(restored.can_send).toBe(true);
    expect(resolveOps()).toEqual([[design.id, false]]);
    s.changeMember(owner, org, member, "remove");
    expect(() => s.chatRooms(member, org)).toThrow("access removed");
  });
  test("direct messages are private even from workspace owners and admins; cross-workspace IDs fail", async () => {
    const { s, owner, org, join, token } = fixture();
    const a = join("Taylor"),
      b = join("Morgan"),
      admin = join("Admin", "admin");
    const dm = s.createChatRoom(a.id, org, { kind: "direct", member_id: b.id });
    expect(
      s.createChatRoom(b.id, org, { kind: "direct", member_id: a.id }).id,
    ).toBe(dm.id);
    const sent = s.sendChatMessage(
      a.id,
      org,
      dm.id,
      message("Private planning detail"),
    );
    expect(s.chatRooms(owner, org).some((r) => r.id === dm.id)).toBe(false);
    for (const outsider of [owner, admin.id]) {
      expect(() => s.chatRoom(outsider, org, dm.id)).toThrow();
      expect(() => s.chatMessages(outsider, org, dm.id, query())).toThrow();
      expect(() =>
        s.sendChatMessage(outsider, org, dm.id, message("Intrusion")),
      ).toThrow();
      expect(() =>
        s.changeChatMessage(outsider, org, sent.id, { revision: 1 }, true),
      ).toThrow();
      expect(() =>
        s.readChat(outsider, org, dm.id, sent.created_seq),
      ).toThrow();
    }
    const other = s.createOrg(owner, "Other workspace");
    expect(() =>
      s.createChatRoom(owner, other, { kind: "direct", member_id: a.id }),
    ).toThrow();
    expect(() =>
      s.createChatRoom(a.id, org, { kind: "direct", member_id: a.id }),
    ).toThrow();
    const handler = createHandler(s);
    for (const path of [
      `chat-rooms/${dm.id}/messages`,
      `chat-rooms/${dm.id}`,
      `chat-rooms/${dm.id}/messages/extra`,
    ]) {
      const response = await handler(
        new Request(`https://team.test/v1/orgs/${org}/${path}`, {
          headers: { Authorization: `Bearer ${token}` },
        }),
      );
      expect(response.status).toBe(404);
      expect(await response.text()).not.toContain("Private planning detail");
    }
    expect(s.chatMessages(b.id, org, dm.id, query()).messages[0].body).toBe(
      "Private planning detail",
    );
  });
  test("send retries are idempotent, authors cannot be forged, and stale edits cannot restore deletions", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id,
      room = s.chatRooms(owner, org)[0].id;
    const payload = message("First version");
    const sent = s.sendChatMessage(member, org, room, {
      ...payload,
      author_id: owner,
    });
    expect(sent.author_id).toBe(member);
    expect(s.sendChatMessage(member, org, room, payload).id).toBe(sent.id);
    expect(() =>
      s.sendChatMessage(member, org, room, { ...payload, body: "Different" }),
    ).toThrow();
    expect(() =>
      s.changeChatMessage(owner, org, sent.id, {
        revision: 1,
        body: "Impersonation",
      }),
    ).toThrow();
    const edited = s.changeChatMessage(member, org, sent.id, {
      revision: 1,
      body: "Corrected",
    });
    expect(edited.revision).toBe(2);
    expect(() =>
      s.changeChatMessage(member, org, sent.id, { revision: 1, body: "Stale" }),
    ).toThrow("changed");
    const deleted = s.changeChatMessage(
      owner,
      org,
      sent.id,
      { revision: 2 },
      true,
    );
    expect(deleted.body).toBe("");
    expect(deleted.deleted_at).not.toBeNull();
    expect(
      s.sendChatMessage(member, org, room, payload).deleted_at,
    ).not.toBeNull();
    expect(s.chatMessages(owner, org, room, query()).messages).toHaveLength(1);
    expect(() =>
      s.changeChatMessage(member, org, sent.id, {
        revision: 3,
        body: "Resurrect",
      }),
    ).toThrow();
    expect(
      s.get("SELECT body FROM chat_messages WHERE id=?", sent.id)!.body,
    ).toBe("");
    for (const body of ["", " ", "x".repeat(10_001)])
      expect(() =>
        s.sendChatMessage(member, org, room, message(body)),
      ).toThrow();
  });
  test("history pages and change cursors include old edits and deletions without skipping messages", () => {
    const { s, owner, org } = fixture();
    const room = s.chatRooms(owner, org)[0].id;
    const sent = Array.from({ length: 125 }, (_, i) =>
      s.sendChatMessage(owner, org, room, message(`Message ${i}`)),
    );
    const latest = s.chatMessages(owner, org, room, query());
    expect(latest.messages).toHaveLength(50);
    expect(latest.messages[0].id).toBe(sent[75].id);
    const older = s.chatMessages(
      owner,
      org,
      room,
      query(`before=${latest.older_before}`),
    );
    expect(older.messages[0].id).toBe(sent[25].id);
    expect(
      s.chatMessages(owner, org, room, query(`before=${older.older_before}`))
        .messages,
    ).toHaveLength(25);
    const firstDelta = s.chatMessages(owner, org, room, query("after=0"));
    expect(firstDelta.has_more).toBe(true);
    const secondDelta = s.chatMessages(
      owner,
      org,
      room,
      query(`after=${firstDelta.cursor}`),
    );
    expect(secondDelta.messages).toHaveLength(25);
    s.changeChatMessage(owner, org, sent[0].id, {
      revision: 1,
      body: "An old correction",
    });
    s.changeChatMessage(owner, org, sent[1].id, { revision: 1 }, true);
    const delta = s.chatMessages(
      owner,
      org,
      room,
      query(`after=${latest.cursor}`),
    );
    expect(delta.messages.map((m) => m.id)).toEqual([sent[0].id, sent[1].id]);
    expect(delta.messages[0].body).toBe("An old correction");
    expect(delta.messages[1].body).toBe("");
    for (const q of [
      "after=-1",
      "after=NaN",
      "before=1.1",
      "after=999999",
      "before=1&after=0",
    ])
      expect(() => s.chatMessages(owner, org, room, query(q))).toThrow();
  });
  test("unread markers are monotonic, edits do not become new messages, and removal immediately denies access", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const room = s.createChatRoom(owner, org, {
      kind: "direct",
      member_id: member,
    }).id;
    const first = s.sendChatMessage(owner, org, room, message("One"));
    const second = s.sendChatMessage(owner, org, room, message("Two"));
    s.sendChatMessage(member, org, room, message("My own reply"));
    expect(s.chatRoom(member, org, room).unread).toBe(2);
    s.readChat(member, org, room, second.created_seq);
    s.readChat(member, org, room, first.created_seq);
    expect(s.chatRoom(member, org, room).unread).toBe(0);
    s.changeChatMessage(owner, org, first.id, {
      revision: 1,
      body: "Edited one",
    });
    expect(s.chatRoom(member, org, room).unread).toBe(0);
    const unread = s.sendChatMessage(owner, org, room, message("Three"));
    expect(s.chatRoom(member, org, room).unread).toBe(1);
    s.changeChatMessage(owner, org, unread.id, { revision: 1 }, true);
    expect(s.chatRoom(member, org, room).unread).toBe(0);
    expect(() =>
      s.readChat(member, org, room, Number.MAX_SAFE_INTEGER),
    ).toThrow();
    s.changeMember(owner, org, member, "remove");
    expect(() => s.chatRooms(member, org)).toThrow();
    expect(() => s.chatMessages(member, org, room, query())).toThrow();
    expect(() =>
      s.sendChatMessage(member, org, room, message("Revoked")),
    ).toThrow();
    expect(s.chatRoom(owner, org, room).can_send).toBe(false);
    expect(() =>
      s.sendChatMessage(owner, org, room, message("No recipient")),
    ).toThrow();
    expect(s.chatMessages(owner, org, room, query()).messages).toHaveLength(4);
  });
  test("HTTP messaging requires a member session and supports send, edit, history and read routes", async () => {
    const { s, owner, org, token, space, join } = fixture();
    const handler = createHandler(s);
    const request = (
      path: string,
      method = "GET",
      body?: unknown,
      key = token,
    ) =>
      handler(
        new Request(`https://team.test/v1/orgs/${org}/${path}`, {
          method,
          headers: {
            Authorization: `Bearer ${key}`,
            "Content-Type": "application/json",
          },
          body: body == null ? undefined : JSON.stringify(body),
        }),
      );
    const room = s.chatRooms(owner, org)[0].id;
    s.updateSpace(owner, org, space, {
      ...s.space(owner, org, space),
      api_enabled: true,
    });
    const integration = s.createIntegrationKey(owner, org, {
      name: "Read shared meetings",
      space_ids: [space],
      transcripts: false,
      days: 30,
    });
    expect(
      (await request(`chat-rooms/${room}/messages`, "GET", undefined, ""))
        .status,
    ).toBe(401);
    expect(
      (
        await request(
          `chat-rooms/${room}/messages`,
          "GET",
          undefined,
          String(integration.token),
        )
      ).status,
    ).toBe(401);
    const send = await request(
      `chat-rooms/${room}/messages`,
      "POST",
      message("Hello team"),
    );
    expect(send.status).toBe(201);
    const saved = await send.json();
    expect(
      (
        await request(`chat-messages/${saved.id}`, "PATCH", {
          body: "Hello everyone",
          revision: 1,
        })
      ).status,
    ).toBe(200);
    const history = await (await request(`chat-rooms/${room}/messages`)).json();
    expect(history.messages[0].body).toBe("Hello everyone");
    expect(
      (
        await request(`chat-rooms/${room}/read`, "POST", {
          cursor: history.cursor,
        })
      ).status,
    ).toBe(200);
    expect(
      (await request(`chat-messages/${saved.id}`, "DELETE", { revision: 2 }))
        .status,
    ).toBe(200);
    const original = await (
      await request(`chat-rooms/${room}/messages`, "POST", message("Original"))
    ).json();
    const other = await (
      await request(`chat-rooms/${room}/messages`, "POST", message("Other"))
    ).json();
    const quote = await request(`chat-rooms/${room}/messages`, "POST", {
      ...message("Quoting"),
      reply_to_id: original.id,
    });
    expect(quote.status).toBe(201);
    const quoted = await quote.json();
    expect(quoted.reply_to.id).toBe(original.id);
    const taylor = join("Taylor").id;
    const dm = s.createChatRoom(owner, org, { kind: "direct", member_id: taylor });
    const secret = s.sendChatMessage(owner, org, dm.id, message("Private"));
    const leak = await request(`chat-rooms/${room}/messages`, "POST", {
      ...message("Leak"),
      reply_to_id: secret.id,
    });
    expect(leak.status).toBe(404);
    expect(await leak.json()).toEqual({ error: "Message not found" });
    // The target is immutable: PATCH ignores reply_to_id.
    const patched = await request(`chat-messages/${quoted.id}`, "PATCH", {
      body: "Quoting, edited",
      revision: quoted.revision,
      reply_to_id: other.id,
    });
    expect(patched.status).toBe(200);
    expect((await patched.json()).reply_to_id).toBe(original.id);
    const located = await (await request(`chat-messages/${quoted.id}`)).json();
    expect(located.message.reply_to.id).toBe(original.id);
    expect(located.message.reply_to.body).toBe("Original");
  });
  test("old workspaces gain one general channel and messages/read state survive a restart", () => {
    const dir = mkdtempSync(joinPath(tmpdir(), "noted-chat-restart-"));
    const path = joinPath(dir, "team.sqlite");
    let s = new TeamStore(path, "setup-key-for-deterministic-tests-only");
    try {
      const setup = s.bootstrap(
        "setup-key-for-deterministic-tests-only",
        "Existing workspace",
        "Owner",
      );
      const user = s.authenticate(setup.token);
      // A pre-chat database has these existing account tables but no chat tables.
      for (const table of [
        "chat_reads",
        "chat_events",
        "chat_messages",
        "chat_participants",
        "chat_rooms",
      ])
        s.db.exec(`DROP TABLE ${table}`);
      s.db.close();
      s = new TeamStore(path);
      const room = s.chatRooms(user, setup.org)[0];
      expect(room.name).toBe("general");
      const sent = s.sendChatMessage(
        user,
        setup.org,
        room.id,
        message("Persistent message"),
      );
      s.readChat(user, setup.org, room.id, sent.created_seq);
      s.db.close();
      s = new TeamStore(path);
      expect(s.authenticate(setup.token)).toBe(user);
      expect(s.chatRooms(user, setup.org)).toHaveLength(1);
      expect(
        s.chatMessages(user, setup.org, room.id, query()).messages[0].body,
      ).toBe("Persistent message");
      expect(
        s.get(
          "SELECT seq FROM chat_reads WHERE room_id=? AND user_id=?",
          room.id,
          user,
        )!.seq,
      ).toBe(sent.created_seq);
    } finally {
      s.db.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
describe("organizational access", () => {
  test("team members read shared meetings; restricted spaces do not leak through lists, direct reads, search, or chat", () => {
    const { s, owner, org, space, join, publish } = fixture();
    const member = join("Taylor").id;
    const restricted = s.createSpace(owner, org, {
      name: "Leadership",
      visibility: "restricted",
    }).id;
    const publicNote = publish(owner),
      privateNote = publish(owner, restricted, "Secret acquisition");
    expect(s.listNotes(member, org).map((n) => n.id)).toEqual([publicNote.id]);
    expect(s.spaces(member, org).map((v) => v.id)).toEqual([space]);
    expect(() => s.note(member, org, privateNote.id)).toThrow();
    expect(s.listNotes(member, org, "Secret")).toEqual([]);
    expect(() =>
      s.context(member, org, {
        question: "Secret?",
        note_ids: [privateNote.id],
      }),
    ).toThrow();
    expect(
      JSON.stringify(s.context(member, org, { question: "Secret?" })),
    ).not.toContain("acquisition");
    expect(() => s.publish(member, org, { space_id: restricted })).toThrow();
  });
  test("membership is checked again on every read and removal preserves shared company knowledge", () => {
    const { s, owner, org, join, publish } = fixture();
    const member = join("Taylor"),
      note = publish(member.id);
    s.changeMember(owner, org, member.id, "remove");
    expect(() => s.listNotes(member.id, org)).toThrow();
    expect(() => s.note(member.id, org, note.id)).toThrow();
    expect(s.note(owner, org, note.id).summary).toContain("Friday");
    expect(() => s.context(member.id, org, { question: "Launch?" })).toThrow();
  });
  test("group membership grants and revokes restricted access without cached permissions", () => {
    const { s, owner, org, join, publish } = fixture();
    const member = join("Taylor").id,
      space = s.createSpace(owner, org, {
        name: "Research",
        visibility: "restricted",
      }).id;
    const note = publish(owner, space),
      group = s.saveGroup(owner, org, {
        name: "Researchers",
        member_ids: [member],
      });
    s.grant(owner, org, space, { kind: "group", id: group.id, role: "viewer" });
    expect(s.note(member, org, note.id).can_edit).toBe(false);
    expect(() =>
      s.updateNote(member, org, note.id, { ...note, summary: "tampered" }),
    ).toThrow();
    s.saveGroup(owner, org, { name: "Researchers", member_ids: [] }, group.id);
    expect(() => s.note(member, org, note.id)).toThrow();
  });
  test("cross-organization IDs cannot be used for folders, notes, members, groups, recipes, or grants", () => {
    const { s, owner, org, space, join, publish } = fixture();
    const member = join("Taylor").id,
      other = s.createOrg(owner, "Other org"),
      otherSpace = s.spaces(owner, other)[0].id;
    const n = publish(owner),
      group = s.saveGroup(owner, other, { name: "Other", member_ids: [owner] });
    expect(() => s.note(member, other, n.id)).toThrow();
    expect(() => s.note(owner, other, n.id)).toThrow();
    expect(() =>
      s.grant(owner, org, space, {
        kind: "group",
        id: group.id,
        role: "editor",
      }),
    ).toThrow();
    const folder = s.saveFolder(owner, other, {
      space_id: otherSpace,
      name: "Other folder",
    });
    expect(() =>
      s.publish(owner, org, { space_id: space, folder_ids: [folder.id] }),
    ).toThrow();
    expect(() => s.listNotes(owner, org, "", "", folder.id)).toThrow();
    const recipe = s.saveRecipe(owner, other, {
      name: "Other",
      prompt: "Analyze",
      kind: "recipe",
    });
    expect(() => s.deleteRecipe(owner, org, String(recipe!.id))).toThrow();
  });
  test("invites are single-use, revocable, expiring, and cannot assign owner", () => {
    const { s, owner, org } = fixture();
    const invite = s.invite(owner, org, { name: "Taylor", role: "member" });
    s.accept(invite.token);
    expect(() => s.accept(invite.token)).toThrow();
    const revoke = s.invite(owner, org, { name: "Dev", role: "member" });
    s.revokeInvite(owner, org, revoke.id);
    expect(() => s.accept(revoke.token)).toThrow();
    const expire = s.invite(owner, org, { name: "Dev", role: "member" });
    s.run("UPDATE invites SET expires_at=0 WHERE id=?", expire.id);
    expect(() => s.accept(expire.token)).toThrow();
    expect(() =>
      s.invite(owner, org, { name: "Dev", role: "owner" }),
    ).toThrow();
    expect(JSON.stringify(s.all("SELECT * FROM invites"))).not.toContain(
      expire.token,
    );
  });
  test("owner cannot be removed; non-admin cannot invite; ownership transfer is atomic", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    expect(() => s.changeMember(owner, org, owner, "remove")).toThrow();
    expect(() =>
      s.invite(member, org, { name: "Other", role: "member" }),
    ).toThrow();
    expect(() => s.transferOwner(member, org, owner)).toThrow();
    s.transferOwner(owner, org, member);
    expect(s.role(member, org)).toBe("owner");
    expect(s.role(owner, org)).toBe("admin");
  });
  test("session expiry and logout invalidate tokens", () => {
    const { s, token, owner } = fixture();
    s.signout(token);
    expect(() => s.authenticate(token)).toThrow();
    const next = s.session(owner);
    s.run("UPDATE sessions SET expires_at=0");
    expect(() => s.authenticate(next)).toThrow();
  });
});
describe("shared meeting workflows", () => {
  test("a changed audience invalidates a publication preview", () => {
    const { s, owner, org, space } = fixture();
    const snapshot = s.snapshot(owner, org);
    s.updateSpace(owner, org, space, {
      name: "Restricted",
      visibility: "restricted",
    });
    expect(() =>
      s.publish(owner, org, {
        space_id: space,
        expected_access_version: snapshot.access_version,
      }),
    ).toThrow("access changed");
    expect(s.listNotes(owner, org)).toEqual([]);
  });
  test("workspace membership, shared records and token hashes survive a service restart", () => {
    const directory = mkdtempSync(joinPath(tmpdir(), "noted-team-test-"));
    let store: TeamStore = new TeamStore(
      joinPath(directory, "team.sqlite"),
      "setup-key-for-persistence-tests-only",
    );
    try {
      const setup = store.bootstrap(
        "setup-key-for-persistence-tests-only",
        "Persistent",
        "Owner",
      );
      const owner = store.authenticate(setup.token),
        space = store.spaces(owner, setup.org)[0].id;
      const note = store.publish(owner, setup.org, {
        space_id: space,
        title: "Durable meeting",
        summary: "A committed decision",
        source_key: "sample",
        occurred_at: "2026-09-04T12:00:00Z",
      });
      store.db.close();
      store = new TeamStore(
        joinPath(directory, "team.sqlite"),
        "another-bootstrap-secret",
      );
      expect(store.authenticate(setup.token)).toBe(owner);
      expect(store.note(owner, setup.org, note.id).summary).toBe(
        "A committed decision",
      );
      expect(() =>
        store.bootstrap("another-bootstrap-secret", "New", "Intruder"),
      ).toThrow("already set up");
      expect(JSON.stringify(store.all("SELECT * FROM sessions"))).not.toContain(
        setup.token,
      );
    } finally {
      store?.db.close();
      rmSync(directory, { recursive: true, force: true });
    }
  });
  test("questions and explicit selections can retrieve meetings older than the first list page", () => {
    const { s, owner, org, space, publish } = fixture();
    const older = s.publish(owner, org, {
      space_id: space,
      source_key: "old",
      title: "Archive",
      summary: "Original engineering discussion",
      transcript: `${"Routine discussion. ".repeat(600)} Quasar requires a blue connector.`,
      occurred_at: "2020-01-01T00:00:00Z",
    });
    for (let i = 0; i < 105; i++) publish(owner);
    expect(s.listNotes(owner, org)).toHaveLength(100);
    const answer = s.context(owner, org, {
      question: "What does Quasar require?",
    });
    expect(answer.sources[0].id).toBe(older.id);
    expect(answer.sources[0].excerpt).toContain("blue connector");
    expect(answer.limited).toBe(true);
    expect(
      s.context(owner, org, { question: "Quasar?", note_ids: [older.id] })
        .sources[0].id,
    ).toBe(older.id);
    const folder = s.saveFolder(owner, org, {
      space_id: space,
      name: "Different scope",
    });
    expect(() =>
      s.context(owner, org, {
        question: "Quasar?",
        folder_id: folder.id,
        note_ids: [older.id],
      }),
    ).toThrow("unavailable");
  });
  test("joining another workspace preserves the existing identity and isolated membership", async () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor"),
      other = s.createOrg(owner, "Other workspace");
    const invite = s.invite(owner, other, { name: "Taylor", role: "member" });
    const handle = createHandler(s);
    const response = await handle(
      new Request("http://localhost/v1/orgs/join", {
        method: "POST",
        headers: { Authorization: `Bearer ${member.token}` },
        body: JSON.stringify({ invitation: invite.token }),
      }),
    );
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ org: other });
    expect(s.orgs(member.id)).toHaveLength(2);
    s.changeMember(owner, other, member.id, "remove");
    expect(s.role(member.id, org)).toBe("member");
  });
  test("access version changes for grants and roles but not private session creation", () => {
    const { s, owner, org, space, join } = fixture();
    const member = join("Taylor").id;
    const before = s.snapshot(member, org).access_version;
    s.session(member);
    expect(s.snapshot(member, org).access_version).toBe(before);
    s.grant(owner, org, space, { kind: "member", id: member, role: "viewer" });
    expect(Number(s.snapshot(member, org).access_version)).toBeGreaterThan(
      Number(before),
    );
    expect(s.space(member, org, space).role).toBe("viewer");
  });
  test("saved answers remain private even to workspace admins and cannot retain revoked sources", () => {
    const { s, owner, org, join, publish } = fixture();
    const member = join("Taylor").id;
    const space = s.createSpace(owner, org, {
      name: "Research",
      visibility: "restricted",
    }).id;
    s.grant(owner, org, space, { kind: "member", id: member, role: "viewer" });
    const note = publish(owner, space);
    const saved = s.saveAnswer(member, org, {
      question: "Private research?",
      answer: "Friday [S1]",
      sources: [{ id: note.id, revision: note.revision, citation: "S1" }],
      limited: false,
    });
    expect(s.answer(member, org, saved.id).answer).toBe("Friday [S1]");
    expect(s.answers(owner, org)).toEqual([]);
    expect(() => s.answer(owner, org, saved.id)).toThrow("not found");
    s.deleteAnswer(owner, org, saved.id);
    expect(s.answers(member, org)).toHaveLength(1);
    s.grant(owner, org, space, { kind: "member", id: member, role: "remove" });
    expect(() => s.answer(member, org, saved.id)).toThrow("no longer access");
    expect(JSON.stringify(s.answers(member, org))).not.toContain(
      "Private research",
    );
    expect(() =>
      s.saveAnswer(member, org, {
        question: "Again",
        answer: "Friday",
        sources: [{ id: note.id, revision: 1, citation: "S1" }],
      }),
    ).toThrow();
    s.deleteAnswer(member, org, saved.id);
    expect(s.answers(member, org)).toEqual([]);
  });
  test("saved answers are invalidated when their evidence is edited or trashed", () => {
    const { s, owner, org, publish } = fixture();
    const note = publish(owner);
    const body = {
      question: "Launch?",
      answer: "Friday [S1]",
      sources: [{ id: note.id, revision: note.revision, citation: "S1" }],
    };
    const saved = s.saveAnswer(owner, org, body);
    s.updateNote(owner, org, note.id, {
      ...note,
      summary: "The launch is Monday",
    });
    expect(() => s.answer(owner, org, saved.id)).toThrow("changed");
    expect(() => s.saveAnswer(owner, org, body)).toThrow("changed");
    expect(s.answers(owner, org)[0].available).toBe(false);
  });
  test("editing rejects stale revisions and keeps local-source transcript immutable", () => {
    const { s, owner, org, join, publish } = fixture();
    const member = join("Taylor").id,
      n = publish(owner);
    const changed = s.updateNote(member, org, n.id, {
      ...n,
      summary: "Launch stays Friday.",
      transcript: "injected",
    });
    expect(changed.revision).toBe(2);
    expect(changed.transcript).toBe(n.transcript);
    expect(() =>
      s.updateNote(owner, org, n.id, { ...n, summary: "stale" }),
    ).toThrow("Reload");
    expect(() => s.trash(member, org, n.id, 2)).toThrow();
    s.trash(owner, org, n.id, 2);
    expect(s.listNotes(owner, org)).toEqual([]);
    expect(s.context(owner, org, { question: "Launch?" }).sources).toEqual([]);
    s.trash(owner, org, n.id, 3, true);
    expect(s.listNotes(owner, org)).toHaveLength(1);
  });
  test("nested folder scope includes descendants and rejects cycles", () => {
    const { s, owner, org, space, publish } = fixture();
    const parent = s.saveFolder(owner, org, {
      space_id: space,
      name: "Product",
    });
    const child = s.saveFolder(owner, org, {
      space_id: space,
      parent_id: parent.id,
      name: "Launch",
    });
    const n = publish(owner);
    s.updateNote(owner, org, n.id, { ...n, folder_ids: [child.id] });
    expect(s.listNotes(owner, org, "", space, parent.id)).toHaveLength(1);
    expect(
      s.context(owner, org, { question: "Launch?", folder_id: parent.id })
        .sources,
    ).toHaveLength(1);
    expect(() =>
      s.saveFolder(owner, org, { ...parent, parent_id: child.id }, parent.id),
    ).toThrow("itself");
  });
  test("shared prompts are workspace-scoped with author and revision protection", () => {
    const { s, owner, org, join } = fixture();
    const member = join("Taylor").id;
    const r = s.saveRecipe(member, org, {
      name: "Decisions",
      prompt: "List decisions with sources",
      kind: "recipe",
    })!;
    expect(s.recipes(owner, org)).toHaveLength(1);
    const r2 = s.saveRecipe(
      owner,
      org,
      { ...r, prompt: "List decisions and owners with sources" },
      String(r.id),
    )!;
    expect(r2.revision).toBe(2);
    expect(() => s.saveRecipe(member, org, r, String(r.id))).toThrow();
    const other = join("Other").id;
    expect(() => s.deleteRecipe(other, org, String(r.id))).toThrow();
  });
  test("HTTP API authenticates and rejects unwanted browser origins and malformed payloads", async () => {
    const { s, token, org } = fixture();
    const handle = createHandler(s);
    const req = (path: string, init: RequestInit = {}) =>
      handle(new Request(`http://localhost${path}`, init));
    expect((await req(`/v1/orgs/${org}`)).status).toBe(401);
    expect(
      (
        await req(`/v1/orgs/${org}`, {
          headers: { Authorization: `Bearer ${token}` },
        })
      ).status,
    ).toBe(200);
    expect(
      (
        await req(`/v1/orgs/${org}`, {
          headers: {
            Authorization: `Bearer ${token}`,
            Origin: "https://evil.example",
          },
        })
      ).status,
    ).toBe(403);
    expect(
      (
        await req(`/v1/orgs/${org}/spaces`, {
          method: "POST",
          headers: { Authorization: `Bearer ${token}` },
          body: "[1]",
        })
      ).status,
    ).toBe(400);
    expect(
      (
        await req(`/v1/orgs/${org}/notes?offset=-1`, {
          headers: { Authorization: `Bearer ${token}` },
        })
      ).status,
    ).toBe(400);
  });
});

describe("explicit integration access", () => {
  function integrationFixture() {
    const f = fixture(),
      { s, owner, org, space } = f;
    s.updateSpace(owner, org, space, {
      ...s.space(owner, org, space),
      api_enabled: true,
    });
    const key = s.createIntegrationKey(owner, org, {
      name: "Research assistant",
      space_ids: [space],
      days: 30,
      transcripts: false,
    });
    return { ...f, key };
  }
  test("integration keys are space-scoped, read-only, and distinct from account sessions", async () => {
    const { s, owner, org, space, join, publish, key, token } =
      integrationFixture();
    const member = join("Taylor").id;
    const hiddenSpace = s.createSpace(owner, org, {
      name: "Leadership",
      visibility: "restricted",
    }).id;
    const visible = publish(owner),
      hidden = publish(owner, hiddenSpace);
    const handle = createHandler(s);
    const request = (path: string, method = "GET", secret = key.token) =>
      handle(
        new Request(`http://localhost${path}`, {
          method,
          headers: { Authorization: `Bearer ${secret}` },
        }),
      );
    expect(() =>
      s.createIntegrationKey(member, org, {
        name: "Escalation",
        space_ids: [space],
      }),
    ).toThrow();
    expect(() =>
      s.createIntegrationKey(owner, org, {
        name: "Unapproved",
        space_ids: [hiddenSpace],
      }),
    ).toThrow("Enable");
    expect((await request(`/v1/orgs/${org}`)).status).toBe(401);
    expect((await request("/v1/api/notes", "GET", token)).status).toBe(401);
    expect((await request("/v1/api/notes", "POST")).status).toBe(405);
    const list = await (await request("/v1/api/notes")).json();
    expect(list.notes.map((n: { id: string }) => n.id)).toEqual([visible.id]);
    expect((await request(`/v1/api/notes/${hidden.id}`)).status).toBe(404);
    const shared = await (await request(`/v1/api/notes/${visible.id}`)).json();
    expect(shared.transcript).toBeUndefined();
    expect(shared.source_key).toBeUndefined();
    expect(shared.summary).toContain("Friday");
    expect(
      (await (await request("/v1/api/notes?q=own%20the%20launch")).json())
        .notes,
    ).toEqual([]);
    const withTranscript = s.createIntegrationKey(owner, org, {
      name: "Approved transcript tool",
      space_ids: [space],
      transcripts: true,
    });
    expect(
      (
        await (
          await request(
            "/v1/api/notes?q=own%20the%20launch",
            "GET",
            withTranscript.token,
          )
        ).json()
      ).notes,
    ).toHaveLength(1);
    expect(JSON.stringify(s.integrationKeys(owner, org))).not.toContain(
      key.token,
    );
    s.revokeIntegrationKey(owner, org, key.id);
    expect((await request("/v1/api/notes")).status).toBe(401);
  });
  test("space-wide disablement and expiry revoke integrations, while creator removal does not change workspace ownership", () => {
    const { s, owner, org, space, join, key } = integrationFixture();
    const nextOwner = join("New owner").id;
    s.transferOwner(owner, org, nextOwner);
    s.changeMember(nextOwner, org, owner, "remove");
    expect(s.authenticateIntegration(key.token).spaces).toEqual([space]);
    s.updateSpace(nextOwner, org, space, {
      ...s.space(nextOwner, org, space),
      api_enabled: false,
    });
    expect(s.authenticateIntegration(key.token).spaces).toEqual([]);
    s.run("UPDATE integration_keys SET expires_at=0 WHERE id=?", key.id);
    expect(() => s.authenticateIntegration(key.token)).toThrow();
  });
  test("MCP uses the real HTTP authorization boundary and bounded source passages", async () => {
    const { s, owner, publish, key } = integrationFixture();
    const n = publish(owner);
    const handler = createHandler(s);
    const server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: (request) => handler(request),
    });
    try {
      const mcp = createMcpHandler(apiClient(server.url.toString(), key.token));
      expect(
        await mcp({ jsonrpc: "2.0", method: "notifications/initialized" }),
      ).toBeNull();
      expect(
        await mcp({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
      ).toMatchObject({ error: { code: -32000 } });
      await mcp({
        jsonrpc: "2.0",
        id: 2,
        method: "initialize",
        params: { protocolVersion: "2025-11-25" },
      });
      const call = (name: string, args: object) =>
        mcp({
          jsonrpc: "2.0",
          id: 3,
          method: "tools/call",
          params: { name, arguments: args },
        });
      const result = await call("get_team_meeting", {
        id: n.id,
        section: "summary",
      });
      expect(JSON.stringify(result)).toContain("Launch moves to Friday");
      const denied = await call("get_team_meeting", {
        id: n.id,
        section: "transcript",
      });
      expect(JSON.stringify(denied)).toContain('"isError":true');
      expect(JSON.stringify(denied)).not.toContain("own the launch");
      expect(
        await call("get_team_meeting", { id: "../secrets" }),
      ).toMatchObject({ error: { code: -32602 } });
      s.revokeIntegrationKey(owner, s.orgs(owner)[0].id as string, key.id);
      expect(
        JSON.stringify(await call("search_team_meetings", { query: "Launch" })),
      ).toContain("revoked");
    } finally {
      server.stop(true);
    }
    expect(() => apiClient("https://example.com/path", key.token)).toThrow();
    expect(() => apiClient("http://example.com", key.token)).toThrow();
    expect(() =>
      apiClient("https://example.com", "a-member-session"),
    ).toThrow();
  });
  test("the stdio MCP process emits only valid protocol messages and reads from the scoped service", async () => {
    const { s, owner, publish, key } = integrationFixture();
    const note = publish(owner);
    const handler = createHandler(s);
    const server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: (request) => handler(request),
    });
    try {
      const child = Bun.spawn(
        [process.execPath, new URL("./mcp.ts", import.meta.url).pathname],
        {
          env: {
            ...process.env,
            NOTED_TEAM_SERVER: server.url.toString(),
            NOTED_TEAM_API_KEY: key.token,
          },
          stdin: "pipe",
          stdout: "pipe",
          stderr: "pipe",
        },
      );
      const requests = [
        {
          jsonrpc: "2.0",
          id: 1,
          method: "initialize",
          params: { protocolVersion: "2025-11-25" },
        },
        { jsonrpc: "2.0", method: "notifications/initialized" },
        { jsonrpc: "2.0", id: 2, method: "tools/list" },
        {
          jsonrpc: "2.0",
          id: 3,
          method: "tools/call",
          params: { name: "get_team_meeting", arguments: { id: note.id } },
        },
      ];
      child.stdin.write(
        requests.map((r) => JSON.stringify(r)).join("\n") + "\n",
      );
      child.stdin.end();
      const [stdout, stderr, code] = await Promise.all([
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
        child.exited,
      ]);
      expect(code).toBe(0);
      expect(stderr).toBe("");
      const replies = stdout
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      expect(replies.map((r) => r.id)).toEqual([1, 2, 3]);
      expect(replies[1].result.tools).toHaveLength(3);
      expect(stdout).toContain("Launch moves to Friday");
      expect(stdout).not.toContain(key.token);
    } finally {
      server.stop(true);
    }
  });
});

describe("private team conversations", () => {
  test("long answers stay intact in history while model input and history pages remain bounded", () => {
    const { s, owner, org, publish } = fixture();
    const note = publish(owner),
      question = "What happened at launch?";
    let current = s.appendConversation(owner, org, {
      question,
      note_ids: [note.id],
      answer: "detail ".repeat(2800),
      sources: s.context(owner, org, { question, note_ids: [note.id] }).sources,
      expected_revision: 0,
    });
    for (let i = 0; i < 4; i++) {
      const context = s.context(owner, org, {
        question,
        conversation_id: current.id,
      });
      current = s.appendConversation(owner, org, {
        question,
        conversation_id: current.id,
        answer: "detail ".repeat(2800),
        sources: context.sources,
        expected_revision: current.revision,
      });
    }
    const longQuestion = "launch ".repeat(850);
    const context = s.context(owner, org, {
      question: longQuestion,
      conversation_id: current.id,
    });
    expect(JSON.stringify(context.history).length).toBeLessThanOrEqual(6000);
    expect(context.limited).toBe(true);
    expect(current.turns[0].answer.length).toBe(19599);
    expect(
      longQuestion.length +
        JSON.stringify(context.history).length +
        context.sources.reduce((sum, source) => sum + source.excerpt.length, 0),
    ).toBeLessThan(20500);
    for (let i = 0; i < 30; i++)
      s.appendConversation(owner, org, {
        question,
        answer: "Friday [S1]",
        sources: s.context(owner, org, { question }).sources,
        expected_revision: 0,
      });
    const first = s.conversations(owner, org),
      second = s.conversations(owner, org, first.next_offset!);
    expect(first.conversations).toHaveLength(30);
    expect(second.conversations).toHaveLength(1);
    expect(second.next_offset).toBeNull();
    expect(
      new Set(
        [...first.conversations, ...second.conversations].map((row) => row.id),
      ).size,
    ).toBe(31);
  });
  test("follow-ups retain scope and history without exposing another account's conversations", () => {
    const { s, owner, org, space, join, publish } = fixture();
    const member = join("Taylor").id;
    const note = publish(owner);
    const question = "What is the launch decision?";
    const context = s.context(member, org, { question, note_ids: [note.id] });
    const first = s.appendConversation(member, org, {
      question,
      answer: "Launch is Friday. [S1]",
      note_ids: [note.id],
      sources: context.sources,
      expected_revision: 0,
    });
    expect(first.revision).toBe(1);
    expect(first.scope.note_ids).toEqual([note.id]);
    expect(s.conversations(owner, org).conversations).toEqual([]);
    expect(() => s.conversation(owner, org, first.id)).toThrow(
      "Conversation not found",
    );
    expect(() => s.deleteConversation(owner, org, first.id)).toThrow(
      "Conversation not found",
    );
    const followup = s.context(member, org, {
      question: "Who owns that?",
      conversation_id: first.id,
      // Changing these cannot silently broaden an existing conversation.
      space_id: space,
      note_ids: [],
    });
    expect(followup.history[0].answer).toContain("Friday");
    expect(followup.sources.map((source) => source.id)).toEqual([note.id]);
    expect(followup.history[0].sources[0].id).toBe(note.id);
    const second = s.appendConversation(member, org, {
      conversation_id: first.id,
      question: "Who owns that?",
      answer: "Taylor. [S1]",
      sources: followup.sources,
      expected_revision: 1,
    });
    expect(second.turns).toHaveLength(2);
    expect(() =>
      s.appendConversation(member, org, {
        conversation_id: first.id,
        question: "Who owns that?",
        answer: "stale",
        sources: followup.sources,
        expected_revision: 1,
      }),
    ).toThrow("changed on another device");
    s.deleteConversation(member, org, first.id);
    expect(s.conversations(member, org).conversations).toEqual([]);
    expect(s.note(member, org, note.id).summary).toContain("Friday");
    expect(s.all("SELECT * FROM conversation_sources")).toHaveLength(0);
  });
  test("revocation and source edits hide the whole conversation including its question", () => {
    const { s, owner, org, join, publish } = fixture();
    const member = join("Taylor").id;
    const restricted = s.createSpace(owner, org, {
      name: "Private research",
      visibility: "restricted",
    }).id;
    s.grant(owner, org, restricted, {
      kind: "member",
      id: member,
      role: "viewer",
    });
    const note = publish(owner, restricted, "Secret research");
    const question = "What are the secret research decisions?";
    const packet = s.context(member, org, { question, space_id: restricted });
    const conversation = s.appendConversation(member, org, {
      question,
      space_id: restricted,
      answer: "Private answer",
      sources: packet.sources,
      expected_revision: 0,
    });
    s.grant(owner, org, restricted, {
      kind: "member",
      id: member,
      role: "remove",
    });
    expect(() => s.conversation(member, org, conversation.id)).toThrow();
    const listing = s.conversations(member, org);
    expect(listing.conversations[0].available).toBe(false);
    expect(JSON.stringify(listing)).not.toContain("secret research");
    expect(() =>
      s.context(member, org, {
        question: "And the owner?",
        conversation_id: conversation.id,
      }),
    ).toThrow();
    expect(() =>
      s.appendConversation(member, org, {
        question,
        conversation_id: conversation.id,
        answer: "late answer",
        sources: packet.sources,
        expected_revision: 1,
      }),
    ).toThrow();
    s.grant(owner, org, restricted, {
      kind: "member",
      id: member,
      role: "viewer",
    });
    expect(s.conversation(member, org, conversation.id).turns).toHaveLength(1);
    s.updateNote(owner, org, note.id, {
      revision: note.revision,
      title: note.title,
      summary: "Revised decision",
    });
    expect(() => s.conversation(member, org, conversation.id)).toThrow(
      "sources changed",
    );
    s.deleteConversation(member, org, conversation.id);
    expect(s.conversations(member, org).conversations).toEqual([]);
  });
  test("appending rechecks exact sources, bounds history, and rejects cross-workspace access", () => {
    const { s, owner, org, publish } = fixture();
    const note = publish(owner);
    const question = "Launch?";
    const packet = s.context(owner, org, { question, note_ids: [note.id] });
    expect(() =>
      s.appendConversation(owner, org, {
        question,
        answer: "forged source",
        expected_revision: 0,
        sources: [{ ...packet.sources[0], revision: 99 }],
        note_ids: [note.id],
      }),
    ).toThrow("Shared sources changed");
    let c = s.appendConversation(owner, org, {
      question,
      answer: "Friday [S1]",
      expected_revision: 0,
      sources: packet.sources,
      note_ids: [note.id],
    });
    for (let i = 1; i < 20; i++) {
      const next = s.context(owner, org, {
        question: `Follow-up ${i}?`,
        conversation_id: c.id,
      });
      expect(next.history.length).toBe(Math.min(6, i));
      c = s.appendConversation(owner, org, {
        question: `Follow-up ${i}?`,
        answer: "Friday [S1]",
        expected_revision: i,
        sources: next.sources,
        conversation_id: c.id,
      });
    }
    expect(c.turns).toHaveLength(20);
    expect(() =>
      s.appendConversation(owner, org, {
        question,
        answer: "too many",
        expected_revision: 20,
        sources: packet.sources,
        conversation_id: c.id,
      }),
    ).toThrow("20 answers");
    const other = s.createOrg(owner, "Elsewhere");
    expect(() => s.conversation(owner, other, c.id)).toThrow(
      "Conversation not found",
    );
    expect(() => s.conversations(owner, org, -1)).toThrow("Invalid offset");
  });
  test("conversation HTTP routes accept member sessions and reject integration keys and malformed routes", async () => {
    const { s, owner, org, space, token, publish } = fixture();
    publish(owner);
    const handler = createHandler(s);
    const call = (path: string, method = "GET", body?: unknown, key = token) =>
      handler(
        new Request(`http://localhost/v1/orgs/${org}/${path}`, {
          method,
          headers: {
            Authorization: `Bearer ${key}`,
            "Content-Type": "application/json",
          },
          body: body ? JSON.stringify(body) : undefined,
        }),
      );
    const packet = s.context(owner, org, { question: "Launch?" });
    const created = await call("conversations", "POST", {
      question: "Launch?",
      answer: "Friday [S1]",
      expected_revision: 0,
      sources: packet.sources,
    });
    expect(created.status).toBe(201);
    const c = await created.json();
    expect((await call(`conversations/${c.id}`)).status).toBe(200);
    expect((await call(`conversations/${c.id}/unexpected`)).status).toBe(404);
    expect((await call("conversations?offset=bad")).status).toBe(400);
    s.updateSpace(owner, org, space, {
      name: "Team knowledge",
      visibility: "team",
      api_enabled: true,
    });
    const key = s.createIntegrationKey(owner, org, {
      name: "Reader",
      space_ids: [space],
    }).token;
    expect(
      (await call(`conversations/${c.id}`, "GET", undefined, key)).status,
    ).toBe(401);
    expect((await call(`conversations/${c.id}`, "DELETE")).status).toBe(200);
    expect((await call(`conversations/${c.id}`)).status).toBe(404);
  });
});

describe("persistent local service startup", () => {
  test("examples are isolated from the real workspace and are not duplicated on reinstall", () => {
    const { s, owner, org } = fixture();
    const sample = ensureExampleWorkspace(s, owner);
    expect(sample).not.toBe(org);
    expect(s.listNotes(owner, org)).toEqual([]);
    expect(s.listNotes(owner, sample)).toHaveLength(3);
    expect(
      s
        .listNotes(owner, sample)
        .every((note) => note.title.startsWith("Example —")),
    ).toBe(true);
    expect(ensureExampleWorkspace(s, owner)).toBe(sample);
    expect(s.listNotes(owner, sample)).toHaveLength(3);
    expect(s.orgs(owner)).toHaveLength(2);
  });
  test("an initialized server restarts with bootstrap disabled and preserves its owner", () => {
    const directory = mkdtempSync(joinPath(tmpdir(), "noted-team-local-"));
    const database = joinPath(directory, "team.sqlite");
    try {
      expect(() => openServiceStore(database)).toThrow("new server needs");
      expect(() => openServiceStore(database, "short")).toThrow("at least 32");
      const first = openServiceStore(
        database,
        "one-time-setup-key-for-local-startup-test",
      );
      const setup = first.bootstrap(
        "one-time-setup-key-for-local-startup-test",
        "Local",
        "Owner",
      );
      const owner = first.authenticate(setup.token);
      first.db.close();
      const next = openServiceStore(database);
      try {
        expect(next.authenticate(setup.token)).toBe(owner);
        expect(next.orgs(owner)[0].id).toBe(setup.org);
        expect(() => next.bootstrap("", "Replacement", "Intruder")).toThrow(
          "Invalid setup key",
        );
        expect(() =>
          next.bootstrap(
            "one-time-setup-key-for-local-startup-test",
            "Replacement",
            "Intruder",
          ),
        ).toThrow("Invalid setup key");
      } finally {
        next.db.close();
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});

test("unread mention indicators follow edits, reads, deletions and conversation access", () => {
  const { s, owner, org, join } = fixture();
  const edison = join("Edison Chen").id;
  const other = join("Taylor").id;
  const room = s.chatRooms(owner, org)[0].id;
  const send = (body: string, roomId = room) => s.sendChatMessage(owner, org, roomId, { body, client_id: crypto.randomUUID() });
  const first = send("Hello @Edison");
  expect(s.chatRoom(edison, org, room).latest_unread_mention_seq).toBe(first.created_seq);
  expect(s.chatRoom(edison, org, room).notification_cursor).toBe(first.created_seq);
  expect(s.chatRoom(edison, org, room).notification_user_id).toBe(edison);
  expect(s.chatRoom(other, org, room).latest_unread_mention_seq).toBe(0);
  expect(s.chatRoom(edison, org, room).unread_mentions).toBe(1);
  expect(s.chatRoom(other, org, room).unread_mentions).toBe(0);
  s.changeChatMessage(owner, org, first.id, { revision: 1, body: "Hello team" });
  expect(s.chatRoom(edison, org, room).unread_mentions).toBe(0);
  const second = send("@Edison Chen, review this");
  s.readChat(edison, org, room, second.created_seq);
  expect(s.chatRoom(edison, org, room).unread_mentions).toBe(0);
  const third = send("@Edison again");
  s.changeChatMessage(owner, org, third.id, { revision: 1 }, true);
  expect(s.chatRoom(edison, org, room).unread_mentions).toBe(0);
  const dm = s.createChatRoom(owner, org, { kind: "direct", member_id: other });
  send("@Edison private", dm.id);
  expect(s.chatRooms(edison, org).some((r) => r.id === dm.id)).toBe(false);
});

test("ordinary message notification cursors exclude own, read and deleted messages", () => {
  const { s, owner, org, join } = fixture();
  const recipient = join("Recipient").id;
  const room = s.chatRooms(owner, org)[0].id;
  const sent = s.sendChatMessage(owner, org, room, { body: "Hello without a mention", client_id: crypto.randomUUID() });
  expect(s.chatRoom(recipient, org, room).latest_unread_message_seq).toBe(sent.created_seq);
  expect(s.chatRoom(recipient, org, room).latest_unread_mention_seq).toBe(0);
  expect(s.chatRoom(owner, org, room).latest_unread_message_seq).toBe(0);
  s.readChat(recipient, org, room, sent.created_seq);
  expect(s.chatRoom(recipient, org, room).latest_unread_message_seq).toBe(0);
  const deleted = s.sendChatMessage(owner, org, room, { body: "Removed", client_id: crypto.randomUUID() });
  s.changeChatMessage(owner, org, deleted.id, { revision: 1 }, true);
  expect(s.chatRoom(recipient, org, room).latest_unread_message_seq).toBe(0);
});

describe("mentions inbox and message destinations", () => {
  test("includes channel mentions and thread replies, with independent read receipts", () => {
    const { s, owner, org, join } = fixture();
    const edison = join("Edison").id;
    const room = s.chatRooms(owner, org)[0].id;
    const send = (body: string, thread_id?: string) => s.sendChatMessage(owner, org, room, { body, thread_id, client_id: crypto.randomUUID() });
    const root = send("@Edison please review");
    const reply = send("@Edison one more detail", root.id);
    const inbox = s.mentions(edison, org, new URLSearchParams("unread=true"));
    expect(inbox.items.map((item) => item.message.id)).toEqual([reply.id, root.id]);
    const quoting = s.sendChatMessage(owner, org, room, { body: "@Edison see above", reply_to_id: root.id, client_id: crypto.randomUUID() });
    expect(s.mentions(edison, org, new URLSearchParams()).items.find((item) => item.message.id === quoting.id)?.message.reply_to?.id).toBe(root.id);
    s.changeChatMessage(owner, org, quoting.id, { revision: quoting.revision }, true);
    expect(inbox.items[0].parent?.id).toBe(root.id);
    expect(s.messageLocation(edison, org, reply.id).room.id).toBe(room);
    s.readMention(edison, org, reply.id);
    expect(s.mentions(edison, org, new URLSearchParams("unread=true")).items.map((i) => i.message.id)).toEqual([root.id]);
    expect(s.chatRoom(edison, org, room).unread_mentions).toBe(1);
    expect(s.chatRoom(edison, org, room).unread).toBe(2);
    expect(s.mentions(edison, org, new URLSearchParams()).items[0].unread).toBe(false);
    s.changeChatMessage(owner, org, root.id, { body: "No mention now", revision: s.messageLocation(owner, org, root.id).message.revision });
    expect(s.mentions(edison, org, new URLSearchParams()).items).toHaveLength(1);
    s.changeChatMessage(owner, org, reply.id, { revision: 1 }, true);
    expect(() => s.messageLocation(edison, org, reply.id)).toThrow("deleted");
    expect(s.mentions(edison, org, new URLSearchParams()).items).toHaveLength(0);
  });
  test("private mentions stay private and HTTP routes recheck access", async () => {
    const { s, owner, org, join, token } = fixture();
    const alice = join("Alice"), bob = join("Bob");
    const dm = s.createChatRoom(alice.id, org, { kind: "direct", member_id: bob.id });
    const message = s.sendChatMessage(alice.id, org, dm.id, { body: "@Bob @Owner private", client_id: crypto.randomUUID() });
    expect(s.mentions(owner, org, new URLSearchParams()).items).toHaveLength(0);
    expect(() => s.messageLocation(owner, org, message.id)).toThrow();
    expect(() => s.readMention(owner, org, message.id)).toThrow();
    const handler = createHandler(s);
    const request = (path: string, auth: string, method = "GET") => handler(new Request(`https://team.test/v1/orgs/${org}/${path}`, { method, headers: { Authorization: `Bearer ${auth}`, "Content-Type": "application/json" }, ...(method === "POST" ? { body: "{}" } : {}) }));
    expect((await request(`chat-messages/${message.id}`, token)).status).toBe(404);
    expect((await request(`chat-messages/${message.id}`, bob.token!)).status).toBe(200);
    expect((await request(`mentions/${message.id}/read`, bob.token!, "POST")).status).toBe(200);
    expect((await (await request("mentions?unread=true", bob.token!)).json()).items).toHaveLength(0);
    s.changeMember(owner, org, bob.id, "remove");
    expect((await request("mentions", bob.token!)).status).toBe(404);
  });
  test("keyset pagination does not skip mentions across nonmatching history", () => {
    const { s, owner, org, join } = fixture();
    const user = join("Edison").id;
    const room = s.chatRooms(owner, org)[0].id;
    const ids: string[] = [];
    for (let i = 0; i < 36; i++) {
      ids.push(s.sendChatMessage(owner, org, room, { body: `@Edison ${i}`, client_id: crypto.randomUUID() }).id);
      s.sendChatMessage(owner, org, room, { body: "Unrelated", client_id: crypto.randomUUID() });
    }
    const first = s.mentions(user, org, new URLSearchParams());
    expect(first.items).toHaveLength(30);
    const second = s.mentions(user, org, new URLSearchParams(`before=${first.next_before}`));
    expect([...first.items, ...second.items].map((i) => i.message.id)).toEqual(ids.reverse());
    expect(second.next_before).toBeNull();
    expect(() => s.mentions(user, org, new URLSearchParams("before=bad"))).toThrow();
  });
});

test("conversation notification settings belong to the viewer, including DMs and archived channels", async () => {
  const { s, owner, org, join } = fixture();
  const alice = join("Alice"), bob = join("Bob");
  const room = s.chatRooms(owner, org)[0].id;
  expect(s.chatRoom(alice.id, org, room).notification_mode).toBe("default");
  expect(s.setConversationNotifications(alice.id, org, room, { mode: "none", user_id: bob.id }).notification_mode).toBe("none");
  expect(s.chatRoom(bob.id, org, room).notification_mode).toBe("default");
  expect(s.setConversationNotifications(alice.id, org, room, { mode: "mentions" }).notification_mode).toBe("mentions");
  expect(s.setConversationNotifications(alice.id, org, room, { mode: "messages" }).notification_mode).toBe("messages");
  expect(s.setConversationNotifications(alice.id, org, room, { mode: "default" }).notification_mode).toBe("default");
  expect(() => s.setConversationNotifications(alice.id, org, room, { mode: "invalid" })).toThrow();
  const dm = s.createChatRoom(alice.id, org, { kind: "direct", member_id: bob.id });
  expect(() => s.setConversationNotifications(owner, org, dm.id, { mode: "none" })).toThrow();
  const handler = createHandler(s);
  const response = await handler(new Request(`https://team.test/v1/orgs/${org}/chat-rooms/${dm.id}/notifications`, {
    method: "PUT", headers: { Authorization: `Bearer ${alice.token}`, "Content-Type": "application/json" }, body: JSON.stringify({ mode: "none" }),
  }));
  expect(response.status).toBe(200);
  expect((await response.json()).notification_mode).toBe("none");
  expect(s.chatRooms(alice.id, org).find((r) => r.id === dm.id)?.notification_mode).toBe("none");
  expect(s.chatRoom(bob.id, org, dm.id).notification_mode).toBe("default");
  const channel = s.createChatRoom(owner, org, { kind: "channel", name: "Archived topic" });
  s.updateChatRoom(owner, org, channel.id, { name: channel.name, description: "", archived: true, revision: channel.revision });
  expect(s.setConversationNotifications(alice.id, org, channel.id, { mode: "none" }).notification_mode).toBe("none");
  s.changeMember(owner, org, alice.id, "remove");
  expect(() => s.setConversationNotifications(alice.id, org, dm.id, { mode: "messages" })).toThrow();
});

describe("document sharing", () => {
  const query = (value = "") => new URLSearchParams(value);
  const publishDocument = (
    s: TeamStore,
    user: string,
    org: string,
    space: string,
    overrides: Record<string, unknown> = {},
  ) =>
    s.publish(user, org, {
      space_id: space,
      source_key: `document:${crypto.randomUUID()}`,
      kind: "document",
      title: "Onboarding guide",
      summary: "# Onboarding\n\nWelcome aboard. Badge pickup is on floor two.",
      transcript: "",
      occurred_at: "2026-09-05T10:00:00Z",
      folder_ids: [],
      ...overrides,
    });
  const reference = (
    s: TeamStore,
    user: string,
    org: string,
    room: string,
    note: { id: string; revision: number },
    start = 0,
    length = 0,
  ) =>
    s.sendChatMessage(user, org, room, {
      body: "",
      client_id: crypto.randomUUID(),
      meeting: { id: note.id, revision: note.revision, start, length },
    });
  test("a source key carries the document prefix exactly when the kind is document", () => {
    const { s, owner, org, space } = fixture();
    expect(() =>
      publishDocument(s, owner, org, space, { source_key: "plain-key" }),
    ).toThrow("Invalid source key for this kind");
    expect(() =>
      publishDocument(s, owner, org, space, {
        kind: "meeting",
        source_key: "document:not-a-document",
      }),
    ).toThrow("Invalid source key for this kind");
    expect(publishDocument(s, owner, org, space).kind).toBe("document");
  });
  test("publishes a document without a transcript, audits it, and words duplicates by kind", () => {
    const { s, owner, org, space, publish } = fixture();
    const key = `document:${crypto.randomUUID()}`;
    const doc = publishDocument(s, owner, org, space, { source_key: key });
    expect(doc.kind).toBe("document");
    expect(doc.transcript).toBe("");
    expect(publish(owner).kind).toBe("meeting");
    expect(
      s
        .activity(owner, org)
        .filter((a) => a.action === "document.published")
        .map((a) => a.target_id),
    ).toEqual([doc.id]);
    expect(() =>
      publishDocument(s, owner, org, space, { transcript: "[00:01] Hi" }),
    ).toThrow("A document has no transcript");
    expect(() =>
      publishDocument(s, owner, org, space, { source_key: key }),
    ).toThrow(
      "This document is already shared in this space. Open the shared copy to update it.",
    );
    const meetingKey = crypto.randomUUID();
    s.publish(owner, org, {
      space_id: space,
      source_key: meetingKey,
      title: "Standup",
      summary: "Notes",
      occurred_at: "2026-09-04T15:00:00Z",
      folder_ids: [],
    });
    expect(() =>
      publishDocument(s, owner, org, space, {
        kind: "meeting",
        source_key: meetingKey,
        transcript: "x",
      }),
    ).toThrow(
      "This meeting is already shared in this space. Open the shared copy to edit it.",
    );
    expect(() =>
      publishDocument(s, owner, org, space, { kind: "memo" }),
    ).toThrow("Invalid note kind");
  });
  test("kind filters listings, picker targets and full-text search; the default returns both", async () => {
    const { s, owner, org, space, publish, token } = fixture();
    const meeting = publish(owner),
      doc = publishDocument(s, owner, org, space);
    const ids = (rows: Record<string, unknown>[]) =>
      rows.map((r) => String(r.id)).sort();
    expect(ids(s.listNotes(owner, org))).toEqual([meeting.id, doc.id].sort());
    expect(ids(s.listNotes(owner, org, "", "", "", false, 100, 0, "meeting")))
      .toEqual([meeting.id]);
    const documents = s.listNotes(
      owner,
      org,
      "",
      "",
      "",
      false,
      100,
      0,
      "document",
    );
    expect(ids(documents)).toEqual([doc.id]);
    expect(documents[0].kind).toBe("document");
    expect(() =>
      s.listNotes(owner, org, "", "", "", false, 100, 0, "memo"),
    ).toThrow("Invalid note kind");
    expect(
      s.listNotes(owner, org, "Badge").map((r) => [r.id, r.kind]),
    ).toEqual([[doc.id, "document"]]);
    const hits = s.search(owner, org, query("q=Badge&kind=meetings")).meetings
      .hits;
    expect(hits.map((h) => [h.id, h.kind])).toEqual([[doc.id, "document"]]);
    expect(
      s
        .search(owner, org, query("q=Launch"))
        .meetings.hits.map((h) => h.kind),
    ).toEqual(["meeting"]);
    const room = s.chatRooms(owner, org)[0].id;
    const targets = (value = "") =>
      s.conversationMeetings(owner, org, room, query(value)).meetings;
    expect(ids(targets())).toEqual([meeting.id, doc.id].sort());
    expect(targets("kind=document").map((r) => [r.id, r.kind])).toEqual([
      [doc.id, "document"],
    ]);
    expect(targets("kind=meeting").map((r) => r.kind)).toEqual(["meeting"]);
    expect(() => targets("kind=memo")).toThrow("Invalid note kind");
    const handler = createHandler(s);
    const request = (path: string) =>
      handler(
        new Request(`https://team.test/v1/orgs/${org}/${path}`, {
          headers: { Authorization: `Bearer ${token}` },
        }),
      );
    const listed = await request("notes?kind=document");
    expect(listed.status).toBe(200);
    expect(
      (await listed.json()).map((r: { id: string; kind: string }) => [
        r.id,
        r.kind,
      ]),
    ).toEqual([[doc.id, "document"]]);
    expect((await request("notes?kind=memo")).status).toBe(400);
    expect(
      (await (await request(`chat-rooms/${room}/meeting-targets?kind=document`)).json())
        .meetings,
    ).toHaveLength(1);
  });
  test("a source key lookup resolves only the caller's own copy, never a teammate's with the same key", async () => {
    const { s, owner, org, space, join } = fixture();
    const peer = join("Peer", "admin"),
      reader = join("Reader");
    // Two Macs each hold local note 7, so both publish `document:7`.
    const key = "document:7";
    const mine = publishDocument(s, owner, org, space, { source_key: key });
    const theirs = publishDocument(s, peer.id, org, space, {
      source_key: key,
      title: "Peer's guide",
    });
    const byKey = (user: string) =>
      s
        .listNotes(user, org, "", "", "", false, 100, 0, "document", key)
        .map((r) => r.id);
    expect(byKey(owner)).toEqual([mine.id]);
    expect(byKey(peer.id)).toEqual([theirs.id]);
    expect(byKey(reader.id)).toEqual([]);
    expect(
      s.listNotes(owner, org, "", "", "", false, 100, 0, "", "document:8"),
    ).toEqual([]);
    const handler = createHandler(s);
    const request = (path: string, token: string) =>
      handler(
        new Request(`https://team.test/v1/orgs/${org}/${path}`, {
          headers: { Authorization: `Bearer ${token}` },
        }),
      );
    const listed = await request(
      `notes?kind=document&source_key=${encodeURIComponent(key)}`,
      peer.token!,
    );
    expect(listed.status).toBe(200);
    expect((await listed.json()).map((r: { id: string }) => r.id)).toEqual([
      theirs.id,
    ]);
    expect(
      await (
        await request(
          `notes?kind=document&source_key=${encodeURIComponent(key)}`,
          reader.token!,
        )
      ).json(),
    ).toEqual([]);
    expect(
      (await request(`notes?source_key=${"x".repeat(201)}`, peer.token!))
        .status,
    ).toBe(400);
  });
  test("references carry kind and a summary excerpt, pin the revision, and go unavailable on trash, revoked access or a foreign org", () => {
    const { s, owner, org, join } = fixture();
    const peer = join("Peer").id;
    const restricted = s.createSpace(owner, org, {
      name: "Handbook",
      visibility: "restricted",
    }).id;
    s.grant(owner, org, restricted, { kind: "member", id: peer, role: "viewer" });
    const dm = s.createChatRoom(owner, org, { kind: "direct", member_id: peer });
    const doc = publishDocument(s, owner, org, restricted);
    const message = reference(s, owner, org, dm.id, doc, 2, 10);
    expect(s.meetingReference(peer, org, message.id)).toEqual({
      available: true,
      id: doc.id,
      kind: "document",
      title: "Onboarding guide",
      excerpt: "Onboarding",
      updated: false,
    });
    const revised = s.updateNote(owner, org, doc.id, {
      revision: doc.revision,
      title: "Onboarding guide v2",
      summary: "# Onboarding v2\n\nBadge pickup moved to reception.",
      folder_ids: [],
    });
    expect(s.meetingReference(peer, org, message.id)).toMatchObject({
      available: true,
      kind: "document",
      title: "Onboarding guide v2",
      excerpt: "",
      updated: true,
    });
    expect(() => reference(s, owner, org, dm.id, doc)).toThrow(
      "The meeting changed",
    );
    s.grant(owner, org, restricted, { kind: "member", id: peer, role: "remove" });
    expect(s.meetingReference(peer, org, message.id)).toEqual({
      available: false,
    });
    expect(s.meetingReference(owner, org, message.id).available).toBe(true);
    s.trash(owner, org, doc.id, revised.revision);
    expect(s.meetingReference(owner, org, message.id)).toEqual({
      available: false,
    });
    const beta = s.createOrg(owner, "Beta");
    expect(() => s.meetingReference(owner, beta, message.id)).toThrow(
      "Message not found",
    );
  });
  test("MCP results report kind and filter by it through the integration boundary", async () => {
    const { s, owner, org, space, publish } = fixture();
    s.updateSpace(owner, org, space, {
      ...s.space(owner, org, space),
      api_enabled: true,
    });
    const key = s.createIntegrationKey(owner, org, {
      name: "Research assistant",
      space_ids: [space],
      days: 30,
      transcripts: false,
    });
    const meeting = publish(owner),
      doc = publishDocument(s, owner, org, space);
    const handler = createHandler(s);
    const server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: (request) => handler(request),
    });
    try {
      const mcp = createMcpHandler(apiClient(server.url.toString(), key.token));
      await mcp({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: { protocolVersion: "2025-11-25" },
      });
      const call = async (name: string, args: object) => {
        const response = (await mcp({
          jsonrpc: "2.0",
          id: 2,
          method: "tools/call",
          params: { name, arguments: args },
        })) as { result?: { content: { text: string }[] }; error?: unknown };
        return response.error
          ? response
          : JSON.parse(response.result!.content[0].text);
      };
      const both = await call("search_team_meetings", {});
      expect(
        both.notes
          .map((n: { id: string; kind: string }) => [n.id, n.kind])
          .sort(),
      ).toEqual(
        [
          [meeting.id, "meeting"],
          [doc.id, "document"],
        ].sort(),
      );
      const filtered = await call("search_team_meetings", { kind: "document" });
      expect(filtered.notes.map((n: { id: string }) => n.id)).toEqual([doc.id]);
      expect(await call("search_team_meetings", { kind: "memo" })).toMatchObject(
        { error: { code: -32602 } },
      );
      const read = await call("get_team_meeting", { id: doc.id });
      expect(read.kind).toBe("document");
      expect(read.text).toContain("Welcome aboard");
      const tools = (await mcp({
        jsonrpc: "2.0",
        id: 3,
        method: "tools/list",
      })) as { result: { tools: { name: string; description: string }[] } };
      expect(
        tools.result.tools.find((t) => t.name === "search_team_meetings")!
          .description,
      ).toContain("meetings and documents");
    } finally {
      server.stop(true);
    }
  });
  test("the notes.kind migration is idempotent and defaults rows that predate it to meeting", () => {
    const dir = mkdtempSync(joinPath(tmpdir(), "noted-document-migrations-"));
    const path = joinPath(dir, "team.sqlite");
    try {
      // A database from before the column existed: schema.sql alone, plus a
      // note row written without a kind.
      const legacy = new Database(path, { create: true, strict: true });
      legacy.exec(readFileSync(new URL("./schema.sql", import.meta.url), "utf8"));
      legacy.exec("PRAGMA foreign_keys=OFF");
      legacy
        .query(
          "INSERT INTO notes(id,space_id,owner_id,source_key,title,summary,transcript,occurred_at,published_at,updated_at) VALUES('n1','s1','u1','k','Old','Body','','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
        )
        .run();
      legacy.close();
      new TeamStore(path).db.close();
      const s = new TeamStore(path);
      try {
        const columns = s.all<{ name: string; dflt_value: string }>(
          "PRAGMA table_info(notes)",
        );
        expect(columns.filter((c) => c.name === "kind")).toHaveLength(1);
        expect(columns.find((c) => c.name === "kind")!.dflt_value).toBe(
          "'meeting'",
        );
        expect(s.get("SELECT kind FROM notes WHERE id='n1'")!.kind).toBe(
          "meeting",
        );
      } finally {
        s.db.close();
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("conversation media", () => {
  const query = (value = "") => new URLSearchParams(value);
  // The smallest PNG validateAttachments accepts: signature, then an IHDR
  // chunk declaring a 1x1 image.
  const png = {
    name: "shot.png",
    data: Buffer.concat([
      Buffer.from([137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13]),
      Buffer.from("IHDR"),
      Buffer.from([0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0, 0, 0, 0]),
    ]).toString("base64"),
  };
  const txt = {
    name: "notes.txt",
    data: Buffer.from("plain notes").toString("base64"),
  };
  const sender =
    (s: TeamStore, org: string, room: string) =>
    (user: string, extra: Record<string, unknown>) =>
      s.sendChatMessage(user, org, room, {
        body: "",
        client_id: crypto.randomUUID(),
        ...extra,
      });
  const media = (
    s: TeamStore,
    user: string,
    org: string,
    room: string,
    value: string,
  ) => s.chatMedia(user, org, room, query(value));
  const publishDocument = (
    s: TeamStore,
    user: string,
    org: string,
    space: string,
    title = "Runbook",
  ) =>
    s.publish(user, org, {
      space_id: space,
      source_key: `document:${crypto.randomUUID()}`,
      kind: "document",
      title,
      summary: "Restart the queue, then the workers.",
      occurred_at: "2026-09-05T10:00:00Z",
      folder_ids: [],
    });
  test("lists images, files and documents newest first with message-boundary keyset paging, skipping deleted messages", () => {
    const { s, owner, org, space, publish, join } = fixture();
    const taylor = join("Taylor").id;
    const room = s.chatRooms(owner, org)[0].id;
    const send = sender(s, org, room);
    const shots = Array.from({ length: 36 }, () =>
      send(owner, { attachments: [png] }),
    );
    s.changeChatMessage(owner, org, shots[0].id, { revision: shots[0].revision }, true);
    const first = media(s, taylor, org, room, "kind=images");
    expect(first.items).toHaveLength(30);
    expect(first.items[0]).toMatchObject({
      message_id: shots[35].id,
      created_seq: shots[35].created_seq,
      created_at: shots[35].created_at,
      author: { id: owner, name: "Owner", avatar_version: "" },
      attachment: { name: "shot.png", mime: "image/png", size: 33 },
    });
    expect(first.items[0].document).toBeUndefined();
    expect(first.next_before).toBe(first.items[29].created_seq);
    const second = media(
      s,
      taylor,
      org,
      room,
      `kind=images&before=${first.next_before}`,
    );
    expect(second.items).toHaveLength(5);
    expect(second.next_before).toBeNull();
    const seen = [...first.items, ...second.items].map((i) => i.message_id);
    expect(new Set(seen).size).toBe(35);
    expect(seen).not.toContain(shots[0].id);
    const seqs = [...first.items, ...second.items].map((i) => i.created_seq);
    expect(seqs).toEqual([...seqs].sort((a, b) => b - a));
    // Files are the non-image attachments of the same messages.
    const mixed = send(taylor, { attachments: [png, png, txt] });
    const files = media(s, owner, org, room, "kind=files");
    expect(files.items).toHaveLength(1);
    expect(files.items[0]).toMatchObject({
      message_id: mixed.id,
      author: { id: taylor, name: "Taylor" },
      attachment: { name: "notes.txt", mime: "text/plain", size: 11 },
    });
    expect(media(s, owner, org, room, "kind=images").items.slice(0, 2)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ message_id: mixed.id }),
      ]),
    );
    // Documents come from references; meeting references never appear.
    const meeting = publish(owner);
    send(owner, { meeting: { id: meeting.id, revision: meeting.revision } });
    const older = publishDocument(s, owner, org, space, "Older runbook"),
      newer = publishDocument(s, owner, org, space, "Newer runbook");
    send(owner, { meeting: { id: older.id, revision: older.revision } });
    const latest = send(taylor, {
      meeting: { id: newer.id, revision: newer.revision },
    });
    const docs = media(s, owner, org, room, "kind=documents");
    expect(docs.items.map((i) => i.document!.title)).toEqual([
      "Newer runbook",
      "Older runbook",
    ]);
    expect(docs.items[0]).toMatchObject({
      message_id: latest.id,
      author: { id: taylor },
      document: { note_id: newer.id, title: "Newer runbook", updated: false },
    });
    expect(docs.items[0].attachment).toBeUndefined();
    expect(docs.next_before).toBeNull();
    s.updateNote(owner, org, newer.id, {
      revision: newer.revision,
      title: "Newer runbook",
      summary: "Restart the workers first.",
      folder_ids: [],
    });
    expect(
      media(s, owner, org, room, "kind=documents").items[0].document!.updated,
    ).toBe(true);
    expect(() => media(s, owner, org, room, "kind=videos")).toThrow(
      "Invalid media kind",
    );
    expect(() => media(s, owner, org, room, "")).toThrow("Invalid media kind");
    expect(() => media(s, owner, org, room, "kind=images&before=-1")).toThrow(
      "Invalid media cursor",
    );
  });
  test("pages never split a message's attachments across a boundary", () => {
    const { s, owner, org } = fixture();
    const channel = s.createChatRoom(owner, org, {
      kind: "channel",
      name: "Screenshots",
    });
    const send = sender(s, org, channel.id);
    const messages = Array.from({ length: 11 }, () =>
      send(owner, { attachments: [png, png, png] }),
    );
    const first = media(s, owner, org, channel.id, "kind=images");
    expect(first.items).toHaveLength(30);
    expect(first.items.map((i) => i.message_id)).not.toContain(messages[0].id);
    expect(first.next_before).toBe(messages[1].created_seq);
    const second = media(
      s,
      owner,
      org,
      channel.id,
      `kind=images&before=${first.next_before}`,
    );
    expect(second.items.map((i) => i.message_id)).toEqual(
      Array(3).fill(messages[0].id),
    );
    expect(second.next_before).toBeNull();
    expect(
      new Set([...first.items, ...second.items].map((i) => i.attachment!.id))
        .size,
    ).toBe(33);
  });
  test("documents the viewer cannot open are left out, and room, membership and org access are enforced", () => {
    const { s, owner, org, join } = fixture();
    const peer = join("Peer").id,
      outsider = join("Outsider").id,
      admin = join("Admin", "admin").id;
    const restricted = s.createSpace(owner, org, {
      name: "Handbook",
      visibility: "restricted",
    }).id;
    s.grant(owner, org, restricted, { kind: "member", id: peer, role: "viewer" });
    const dm = s.createChatRoom(owner, org, { kind: "direct", member_id: peer });
    const doc = publishDocument(s, owner, org, restricted, "Secret handbook");
    sender(s, org, dm.id)(owner, {
      meeting: { id: doc.id, revision: doc.revision },
    });
    expect(media(s, peer, org, dm.id, "kind=documents").items).toHaveLength(1);
    s.grant(owner, org, restricted, { kind: "member", id: peer, role: "remove" });
    const hidden = media(s, peer, org, dm.id, "kind=documents");
    expect(hidden).toEqual({ items: [], next_before: null });
    expect(JSON.stringify(hidden)).not.toContain("Secret handbook");
    expect(media(s, owner, org, dm.id, "kind=documents").items).toHaveLength(1);
    s.trash(owner, org, doc.id, doc.revision);
    expect(media(s, owner, org, dm.id, "kind=documents").items).toHaveLength(0);
    for (const nonParticipant of [outsider, admin])
      expect(() => media(s, nonParticipant, org, dm.id, "kind=images")).toThrow(
        "access removed",
      );
    s.changeMember(owner, org, peer, "remove");
    expect(() => media(s, peer, org, dm.id, "kind=images")).toThrow();
    const beta = s.createOrg(owner, "Beta");
    expect(() => media(s, owner, beta, dm.id, "kind=images")).toThrow();
    expect(() =>
      media(s, owner, org, `general-${beta}`, "kind=images"),
    ).toThrow();
  });
  test("HTTP media listing requires a member session and accepts only GET with a valid kind and cursor", async () => {
    const { s, owner, org, token, space } = fixture();
    const handler = createHandler(s);
    const request = (path: string, method = "GET", key = token) =>
      handler(
        new Request(`https://team.test/v1/orgs/${org}/${path}`, {
          method,
          headers: {
            Authorization: `Bearer ${key}`,
            "Content-Type": "application/json",
          },
          body: method === "GET" ? undefined : "{}",
        }),
      );
    const room = s.chatRooms(owner, org)[0].id;
    sender(s, org, room)(owner, { attachments: [png] });
    s.updateSpace(owner, org, space, {
      ...s.space(owner, org, space),
      api_enabled: true,
    });
    const integration = s.createIntegrationKey(owner, org, {
      name: "Read shared meetings",
      space_ids: [space],
      transcripts: false,
      days: 30,
    });
    const ok = await request(`chat-rooms/${room}/media?kind=images`);
    expect(ok.status).toBe(200);
    const json = await ok.json();
    expect(json.items).toHaveLength(1);
    expect(json.items[0].attachment.mime).toBe("image/png");
    expect(json.next_before).toBeNull();
    expect(
      (await request(`chat-rooms/${room}/media?kind=images`, "GET", "")).status,
    ).toBe(401);
    expect(
      (
        await request(
          `chat-rooms/${room}/media?kind=images`,
          "GET",
          String(integration.token),
        )
      ).status,
    ).toBe(401);
    expect(
      (await request(`chat-rooms/${room}/media?kind=images`, "POST")).status,
    ).toBe(404);
    expect(
      (await request(`chat-rooms/${room}/media?kind=images`, "PATCH")).status,
    ).toBe(404);
    expect((await request(`chat-rooms/${room}/media?kind=videos`)).status).toBe(
      400,
    );
    expect((await request(`chat-rooms/${room}/media`)).status).toBe(400);
    expect(
      (await request(`chat-rooms/${room}/media?kind=images&before=x`)).status,
    ).toBe(400);
    expect((await request(`chat-rooms/${room}/media/extra`)).status).toBe(404);
  });
});
