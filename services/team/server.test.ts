import { describe, test, expect, afterEach } from "bun:test";
import { TeamStore } from "./store";
import { createHandler } from "./server";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join as joinPath } from "node:path";
import { createMcpHandler, apiClient } from "./mcp";
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
