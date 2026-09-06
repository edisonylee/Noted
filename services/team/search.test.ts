import { test, expect, afterEach } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { TeamStore } from "./store";
import { createHandler } from "./server";
import { searchExpression } from "./search";
const stores: TeamStore[] = [];
afterEach(() => { for (const store of stores.splice(0)) store.db.close(); });
function fixture(path = ":memory:") {
  const s = new TeamStore(path, "deterministic-search-test-setup-key"); stores.push(s);
  const session = s.bootstrap("deterministic-search-test-setup-key", "Search team", "Owner");
  const org = session.org, owner = s.authenticate(session.token), room = s.chatRooms(owner, org)[0].id;
  const member = (name: string) => { const invite = s.invite(owner, org, { name, role: "member" }); const login = s.accept(invite.token); return { id: s.authenticate(login.token!), token: login.token! }; };
  const send = (body: string, who = owner, where = room, thread_id?: string) => s.sendChatMessage(who, org, where, { body, thread_id, client_id: crypto.randomUUID() });
  const search = (q: string, who = owner, filters: Record<string, string> = {}) => s.search(who, org, new URLSearchParams({ q, ...filters }));
  const publish = (title: string, transcript = "", space = s.spaces(owner, org)[0].id) => s.publish(owner, org, { space_id: space, source_key: crypto.randomUUID(), title, summary: "Meeting notes", transcript, occurred_at: "2026-09-04T15:00:00Z", folder_ids: [] });
  return { s, org, owner, room, member, send, search, publish, session };
}

test("search isolates private DMs and organizations on every page and rechecks revocation", async () => {
  const { s, org, owner, member, send, search } = fixture();
  const alice = member("Alice"), bob = member("Bob"), third = member("Third");
  const dm = s.createChatRoom(alice.id, org, { kind: "direct", member_id: bob.id });
  for (let i = 0; i < 5; i++) { send(`pricing secret ${i}`, alice.id, dm.id); send(`pricing public ${i}`); }
  const otherOrg = s.createOrg(owner, "Other team");
  s.sendChatMessage(owner, otherOrg, s.chatRooms(owner, otherOrg)[0].id, { body: "pricing other-org-private", client_id: crypto.randomUUID() });
  const hits = []; let cursor = "";
  do { const page = search("pricing", third.id, { limit: "2", messages_cursor: cursor }); hits.push(...page.messages.hits); cursor = page.messages.cursor ?? ""; } while (cursor);
  expect(hits).toHaveLength(5);
  expect(JSON.stringify(hits)).not.toContain("secret");
  expect(JSON.stringify(hits)).not.toContain("other-org-private");
  expect(() => search("pricing", owner, { room: dm.id })).toThrow();
  expect(search("pricing", bob.id, { room: dm.id }).messages.hits).toHaveLength(5);
  s.changeMember(owner, org, alice.id, "remove");
  expect(search("pricing", bob.id, { room: dm.id }).messages.hits).toHaveLength(5);
  expect(() => search("pricing", alice.id)).toThrow();
  const handler = createHandler(s);
  const request = (token: string) => handler(new Request(`https://test.invalid/v1/orgs/${org}/search?q=pricing`, { headers: { Authorization: `Bearer ${token}` } }));
  expect((await request("invalid")).status).toBe(401);
  expect((await request(alice.token)).status).toBe(404);
  expect((await request(bob.token)).status).toBe(200);
});

test("meeting search honors grants, trash and restore, and existing list search uses FTS", () => {
  const { s, org, owner, member, publish, search } = fixture();
  const user = member("Viewer");
  const space = s.createSpace(owner, org, { name: "Restricted", visibility: "restricted" }).id;
  const hidden = publish("pricing hidden", "", space);
  const publicNote = publish("pricing public");
  expect(search("pricing", user.id).meetings.hits.map((h) => h.id)).toEqual([publicNote.id]);
  s.grant(owner, org, space, { kind: "member", id: user.id, role: "viewer" });
  expect(search("pricing", user.id).meetings.hits).toHaveLength(2);
  s.grant(owner, org, space, { kind: "member", id: user.id, role: "remove" });
  expect(JSON.stringify(search("pricing", user.id))).not.toContain(hidden.id);
  s.trash(owner, org, publicNote.id, publicNote.revision);
  expect(search("pricing", user.id).meetings.hits).toHaveLength(0);
  s.trash(owner, org, publicNote.id, s.note(owner, org, publicNote.id).revision, true);
  expect(search("pricing", user.id).meetings.hits).toHaveLength(1);
  expect(s.listNotes(user.id, org, "pricing publ").map((n) => n.id)).toEqual([publicNote.id]);
});

test("short terms, hostile syntax and accented text are literal keywords; highlights are text", () => {
  const { send, search } = fixture();
  send('AI UI Q4 v2 café pricing <img src=x onerror=alert(1)>');
  for (const q of ["AI", "UI", "Q4:", "v2*", "cafe", "-pricing", '"pricing']) expect(search(q).messages.hits).toHaveLength(1);
  for (const q of ["what's the \"plan", "^", "NEAR(", "a OR", "pricing AND", "' OR 1=1 --"]) expect(() => search(q)).not.toThrow();
  const hit = search("pricing").messages.hits[0];
  expect(hit.snippet.some((part) => part.match && part.text === "pricing")).toBe(true);
  expect(hit.snippet.map((part) => part.text).join("")).toContain("<img");
  expect(search("^").messages.hits).toHaveLength(0);
  expect(() => search("a".repeat(257))).toThrow();
  expect(() => search(Array(13).fill("term").join(" "))).toThrow();
  expect(searchExpression("Q4 budget")).toBe('"Q4" AND "budget"*');
});

test("edits and deletes update the index; archived history remains searchable", () => {
  const { s, owner, org, send, search } = fixture();
  const message = send("oldword");
  s.changeChatMessage(owner, org, message.id, { body: "newword", revision: message.revision });
  expect(search("oldword").messages.hits).toHaveLength(0);
  expect(search("newword").messages.hits).toHaveLength(1);
  s.changeChatMessage(owner, org, message.id, { revision: message.revision + 1 }, true);
  expect(search("newword").messages.hits).toHaveLength(0);
  const room = s.createChatRoom(owner, org, { kind: "channel", name: "Archive" });
  send("archivedword", owner, room.id);
  s.updateChatRoom(owner, org, room.id, { revision: room.revision, archived: true });
  expect(search("archivedword").messages.hits).toHaveLength(1);
  s.db.exec("INSERT INTO chat_messages_fts(chat_messages_fts,rank) VALUES('integrity-check',1)");
});

test("group cursors are independent and filter-bound, with stable ties and validated dates", () => {
  const { owner, member, send, publish, search } = fixture();
  const other = member("Other");
  for (let i = 0; i < 5; i++) { send(`pricing ${i}`); publish(`pricing ${i}`); }
  const first = search("pricing", owner, { limit: "2" });
  expect(first.messages.hits).toHaveLength(2); expect(first.meetings.hits).toHaveLength(2);
  const next = search("pricing", owner, { kind: "messages", limit: "2", messages_cursor: first.messages.cursor! });
  expect(next.meetings.hits).toHaveLength(0);
  expect(new Set([...first.messages.hits, ...next.messages.hits].map((h) => h.id)).size).toBe(4);
  expect(() => search("different", owner, { messages_cursor: first.messages.cursor! })).toThrow();
  expect(() => search("pricing", other.id, { messages_cursor: first.messages.cursor! })).toThrow();
  expect(() => search("pricing", owner, { meetings_cursor: first.messages.cursor! })).toThrow();
  for (const filters of ([{ since: "2026-02-30" }, { since: "2026-09-06", until: "2026-09-01" }, { limit: "NaN" }, { messages_cursor: "bad" }, { kind: "invalid" }] as Record<string, string>[])) expect(() => search("pricing", owner, filters)).toThrow();
  expect(search("pricing", owner, { author: other.id }).messages.hits).toHaveLength(0);
  expect(search("pricing", owner, { since: "2026-09-04", until: "2026-09-04" }).meetings.hits).toHaveLength(5);
  expect(search("pricing", owner, { since: "2026-09-05" }).meetings.hits).toHaveLength(0);
});

test("title matches outrank transcript matches and note edits are indexed", () => {
  const { s, owner, org, publish, search } = fixture();
  const title = publish("pricing", "Other content");
  publish("Other content", "pricing");
  expect(search("pricing").meetings.hits[0].id).toBe(title.id);
  s.updateNote(owner, org, title.id, { title: "Changed", summary: "Meeting notes", revision: title.revision, folder_ids: [] });
  expect(search("pricing").meetings.hits).toHaveLength(1);
  s.db.exec("INSERT INTO notes_fts(notes_fts,rank) VALUES('integrity-check',1)");
});

test("legacy databases backfill on migration and keep incremental updates after restart", () => {
  const dir = mkdtempSync(join(tmpdir(), "noted-search-migrate-"));
  try {
    const path = join(dir, "team.sqlite");
    const { s, org, owner, send, publish } = fixture(path);
    send("legacyword"); publish("legacyword");
    for (const table of ["chat_messages", "notes"]) {
      for (const suffix of ["ai", "ad", "au"]) s.db.exec(`DROP TRIGGER ${table}_fts_${suffix}`);
      s.db.exec(`DROP TABLE ${table}_fts`);
    }
    s.db.exec("DELETE FROM team_migrations WHERE id='search-v1'");
    s.db.close(); stores.splice(stores.indexOf(s), 1);
    const migrated = new TeamStore(path); stores.push(migrated);
    const page = migrated.search(owner, org, new URLSearchParams({ q: "legacyword" }));
    expect(page.messages.hits).toHaveLength(1); expect(page.meetings.hits).toHaveLength(1);
    expect(migrated.all("SELECT * FROM team_migrations")).toHaveLength(1);
    const message = page.messages.hits[0];
    migrated.changeChatMessage(owner, org, message.id, { body: "updatedword", revision: 1 });
    migrated.db.close(); stores.splice(stores.indexOf(migrated), 1);
    const reopened = new TeamStore(path); stores.push(reopened);
    expect(reopened.search(owner, org, new URLSearchParams({ q: "updatedword" })).messages.hits).toHaveLength(1);
    expect(reopened.search(owner, org, new URLSearchParams({ q: "legacyword" })).messages.hits).toHaveLength(0);
    reopened.db.exec("INSERT INTO chat_messages_fts(chat_messages_fts,rank) VALUES('integrity-check',1)");
    reopened.db.close(); stores.splice(stores.indexOf(reopened), 1);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("bounded message windows include the target, page forward, and reject foreign threads", () => {
  const { s, owner, org, room, member, send } = fixture();
  const messages = Array.from({ length: 150 }, (_, i) => send(`Message ${i}`));
  const page = s.chatMessages(owner, org, room, new URLSearchParams({ around: messages[40].id }));
  expect(page.messages).toHaveLength(51);
  expect(page.messages.some((m) => m.id === messages[40].id)).toBe(true);
  expect(page.older_before).not.toBeNull(); expect(page.newer_after).not.toBeNull();
  const next = s.chatMessages(owner, org, room, new URLSearchParams({ newer: String(page.newer_after) }));
  expect(next.messages[0].created_seq).toBeGreaterThan(page.messages.at(-1)!.created_seq);
  expect(next.messages).toHaveLength(50);
  const reply = send("Thread reply", owner, room, messages[0].id);
  expect(s.chatMessages(owner, org, room, new URLSearchParams({ around: reply.id, thread: messages[0].id })).messages[0].id).toBe(reply.id);
  expect(() => s.chatMessages(owner, org, room, new URLSearchParams({ around: reply.id }))).toThrow();
  expect(() => s.chatMessages(owner, org, room, new URLSearchParams({ around: reply.id, after: "0" }))).toThrow();
  const alice = member("Alice"), bob = member("Bob");
  const dm = s.createChatRoom(alice.id, org, { kind: "direct", member_id: bob.id });
  const hidden = send("private", alice.id, dm.id);
  expect(() => s.chatMessages(owner, org, room, new URLSearchParams({ around: hidden.id }))).toThrow();
  expect(() => s.chatMessages(owner, org, dm.id, new URLSearchParams({ around: hidden.id }))).toThrow();
});

test("excerpt responses stay bounded even when a matching token is very long", () => {
  const { send, search } = fixture();
  send("oversized" + "x".repeat(7000));
  const parts = search("oversized").messages.hits[0].snippet;
  expect(parts.map((p) => p.text).join("").length).toBeLessThanOrEqual(1601);
});

test("a failed migration leaves neither a completion marker nor partial indexes", () => {
  const { Database } = require("bun:sqlite");
  const { readFileSync } = require("node:fs");
  const { initializeSearch } = require("./search");
  const db = new Database(":memory:");
  try {
    db.exec(readFileSync(new URL("./schema.sql", import.meta.url), "utf8"));
    // Simulate a conflicting preexisting schema so the second rebuild fails.
    db.exec("CREATE TABLE notes_fts(body TEXT)");
    expect(() => initializeSearch(db)).toThrow();
    expect(db.query("SELECT name FROM sqlite_master WHERE name='chat_messages_fts'").get()).toBeNull();
    expect(db.query("SELECT name FROM sqlite_master WHERE name='team_migrations'").get()).toBeNull();
  } finally { db.close(); }
});
