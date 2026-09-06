import { validateAttachments, TEAM_ATTACHMENT_QUOTA } from "./attachments";
import { initializeSearch, searchExpression, snippetParts } from "./search";
import { REACTION_NAMES } from "./reactions";
import { findMentions } from "./mentions";
import { Database, type SQLQueryBindings } from "bun:sqlite";
import { readFileSync } from "node:fs";
import { randomBytes, createHash, timingSafeEqual } from "node:crypto";
import type {
  TeamRole,
  TeamSpace,
  TeamNote,
  TeamNoteRow,
  TeamSource,
  TeamRecipe,
  TeamConversation,
  TeamTurn,
  TeamChatRoom,
  TeamChatMessage,
  TeamChatPage,
  TeamUser,
  TeamProfile,
} from "../../src/teams/types";

type Row = Record<string, string | number | null>;
export type IntegrationAccess = {
  id: string;
  org: string;
  spaces: string[];
  transcripts: boolean;
};
export class TeamError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
function fail(status: number, message: string): never {
  throw new TeamError(status, message);
}
const uid = () => crypto.randomUUID();
const secret = () => randomBytes(32).toString("base64url");
const hash = (value: string) =>
  createHash("sha256").update(value).digest("hex");
const now = () => new Date().toISOString();
export function text(
  value: unknown,
  name: string,
  max = 200,
  optional = false,
): string {
  if (
    typeof value !== "string" ||
    value.length > max ||
    (!optional && !value.trim())
  )
    fail(400, `Invalid ${name}`);
  return (value as string).trim();
}
function choice<T extends string>(
  value: unknown,
  allowed: readonly T[],
  name: string,
): T {
  if (!allowed.includes(value as T)) fail(400, `Invalid ${name}`);
  return value as T;
}
function ids(value: unknown, name: string, max = 100): string[] {
  if (!Array.isArray(value) || value.length > max) fail(400, `Invalid ${name}`);
  return [...new Set((value as unknown[]).map((v) => text(v, name, 100)))];
}
const admin = (role: TeamRole) => role === "owner" || role === "admin";

function validateAvatar(value: string) {
  const match = /^data:image\/(png|jpeg);base64,([A-Za-z0-9+/]+={0,2})$/.exec(
    value,
  );
  if (!match) fail(400, "Choose a JPEG or PNG profile photo");
  const bytes = Buffer.from(match[2], "base64");
  if (bytes.toString("base64") !== match[2]) fail(400, "Invalid profile photo");
  let width = 0,
    height = 0;
  if (
    match[1] === "png" &&
    bytes.length >= 33 &&
    bytes
      .subarray(0, 8)
      .equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])) &&
    bytes.toString("ascii", 12, 16) === "IHDR"
  ) {
    width = bytes.readUInt32BE(16);
    height = bytes.readUInt32BE(20);
  } else if (
    match[1] === "jpeg" &&
    bytes[0] === 255 &&
    bytes[1] === 216 &&
    bytes.at(-2) === 255 &&
    bytes.at(-1) === 217
  ) {
    let at = 2;
    while (at + 8 < bytes.length) {
      if (bytes[at++] !== 255) break;
      while (bytes[at] === 255) at++;
      const marker = bytes[at++];
      if (marker === 218 || marker === 217 || at + 2 > bytes.length) break;
      const length = bytes.readUInt16BE(at);
      if (length < 2 || at + length > bytes.length) break;
      if ([192, 193, 194].includes(marker) && length >= 8) {
        height = bytes.readUInt16BE(at + 3);
        width = bytes.readUInt16BE(at + 5);
        break;
      }
      at += length;
    }
  }
  if (!width || !height || width > 512 || height > 512)
    fail(400, "Profile photos must be JPEG or PNG images up to 512 pixels");
}

export class TeamStore {
  readonly db: Database;
  private chatWaiters = new Map<string, Set<() => void>>();
  constructor(
    path = ":memory:",
    private bootstrapKey = "",
  ) {
    this.db = new Database(path, { create: true, strict: true });
    this.db.exec(
      readFileSync(new URL("./schema.sql", import.meta.url), "utf8"),
    );
    if (
      !this.all<{ name: string }>("PRAGMA table_info(spaces)").some(
        (c) => c.name === "api_enabled",
      )
    )
      this.db.exec(
        "ALTER TABLE spaces ADD COLUMN api_enabled INTEGER NOT NULL DEFAULT 0",
      );
    if (
      !this.all<{ name: string }>("PRAGMA table_info(chat_messages)").some(
        (c) => c.name === "thread_id",
      )
    )
      this.db.exec(
        "ALTER TABLE chat_messages ADD COLUMN thread_id TEXT REFERENCES chat_messages(id)",
      );
    this.db.exec(
      "CREATE INDEX IF NOT EXISTS chat_thread_history ON chat_messages(room_id,thread_id,created_seq)",
    );
    initializeSearch(this.db);
    this
      .run(`INSERT OR IGNORE INTO chat_rooms(id,org_id,kind,name,description,created_by,created_at)
      SELECT 'general-' || o.id,o.id,'channel','general','A place for everyone in the workspace.',m.user_id,o.created_at
      FROM organizations o JOIN members m ON m.org_id=o.id AND m.role='owner'`);
  }
  all<T = Row>(sql: string, ...values: SQLQueryBindings[]): T[] {
    return this.db.query(sql).all(...values) as T[];
  }
  get<T = Row>(sql: string, ...values: SQLQueryBindings[]): T | null {
    return this.db.query(sql).get(...values) as T | null;
  }
  run(sql: string, ...values: SQLQueryBindings[]) {
    return this.db.query(sql).run(...values);
  }
  private notifyChat(room?: string) {
    queueMicrotask(() => {
      if (room) this.chatWaiters.get(room)?.forEach((wake) => wake());
      else
        for (const listeners of this.chatWaiters.values())
          listeners.forEach((wake) => wake());
    });
  }
  async waitForChat(
    room: string,
    after: number,
    signal: AbortSignal,
    wait: number,
  ) {
    if (signal.aborted || this.latestChatCursor(room) !== after) return;
    await new Promise<void>((resolve) => {
      const listeners = this.chatWaiters.get(room) ?? new Set<() => void>();
      if (listeners.size >= 100)
        fail(429, "Too many open conversations. Try again shortly.");
      this.chatWaiters.set(room, listeners);
      const finish = () => {
        clearTimeout(timer);
        signal.removeEventListener("abort", finish);
        listeners.delete(finish);
        if (!listeners.size) this.chatWaiters.delete(room);
        resolve();
      };
      const timer = setTimeout(finish, wait);
      listeners.add(finish);
      signal.addEventListener("abort", finish, { once: true });
      if (signal.aborted || this.latestChatCursor(room) !== after) finish();
    });
  }
  audit(org: string, actor: string, action: string, target: string) {
    this.notifyChat();
    this.run(
      "INSERT INTO audit(org_id,actor_id,action,target_id,at) VALUES(?,?,?,?,?)",
      org,
      actor,
      action,
      target,
      now(),
    );
  }
  session(user: string) {
    const token = secret();
    this.run("DELETE FROM sessions WHERE expires_at < ?", Date.now());
    this.run(
      "INSERT INTO sessions VALUES(?,?,?)",
      hash(token),
      user,
      Date.now() + 30 * 86400_000,
    );
    return token;
  }
  authenticate(token: string): string {
    if (token.length > 200) fail(401, "Sign in again");
    const row = this.get(
      "SELECT user_id FROM sessions WHERE hash=? AND expires_at>?",
      hash(token),
      Date.now(),
    );
    return row ? String(row.user_id) : fail(401, "Sign in again");
  }
  signout(token: string) {
    this.notifyChat();
    this.run("DELETE FROM sessions WHERE hash=?", hash(token));
  }
  bootstrap(key: string, organization: unknown, name: unknown) {
    const a = Buffer.from(hash(key)),
      b = Buffer.from(hash(this.bootstrapKey));
    if (!this.bootstrapKey || !timingSafeEqual(a, b))
      fail(403, "Invalid setup key");
    return this.db.transaction(() => {
      if (this.get("SELECT id FROM organizations LIMIT 1"))
        fail(409, "This server is already set up. Use an invitation to join.");
      const user = uid();
      this.run("INSERT INTO users VALUES(?,?)", user, text(name, "name"));
      const org = this.createOrg(user, organization);
      return { token: this.session(user), org };
    })();
  }
  createOrg(user: string, name: unknown) {
    const id = uid();
    this.db.transaction(() => {
      this.run(
        "INSERT INTO organizations VALUES(?,?,?)",
        id,
        text(name, "workspace name"),
        now(),
      );
      this.run("INSERT INTO members VALUES(?,?,'owner')", id, user);
      this.createSpace(user, id, {
        name: "General meetings",
        description:
          "Meetings and decisions shared with everyone in this workspace.",
        visibility: "team",
      });
      this.audit(id, user, "workspace.created", id);
      this.run(
        "INSERT INTO chat_rooms(id,org_id,kind,name,description,created_by,created_at) VALUES(?,?,'channel','general',?,?,?)",
        `general-${id}`,
        id,
        "A place for everyone in the workspace.",
        user,
        now(),
      );
    })();
    return id;
  }
  renameOrg(user: string, org: string, name: unknown) {
    if (!admin(this.role(user, org)))
      fail(403, "Only an admin can rename the team");
    const value = text(name, "team name");
    this.run("UPDATE organizations SET name=? WHERE id=?", value, org);
    this.audit(org, user, "team.renamed", org);
    return {};
  }
  role(user: string, org: string): TeamRole {
    const row = this.get(
      "SELECT role FROM members WHERE org_id=? AND user_id=?",
      org,
      user,
    );
    return row
      ? (row.role as TeamRole)
      : fail(404, "Workspace not found or access removed");
  }
  requireAdmin(user: string, org: string) {
    if (!admin(this.role(user, org)))
      fail(403, "A workspace admin is required");
  }
  orgs(user: string) {
    return this.all(
      "SELECT o.id,o.name,m.role FROM organizations o JOIN members m ON m.org_id=o.id WHERE m.user_id=? ORDER BY o.name",
      user,
    );
  }
  members(user: string, org: string) {
    this.role(user, org);
    return this.all(
      "SELECT u.id,u.name,m.role,COALESCE(p.title,'') AS title,COALESCE(p.about,'') AS about,COALESCE(p.avatar_version,'') AS avatar_version FROM members m JOIN users u ON u.id=m.user_id LEFT JOIN user_profiles p ON p.user_id=u.id WHERE m.org_id=? ORDER BY u.name",
      org,
    );
  }
  invite(user: string, org: string, body: Record<string, unknown>) {
    this.requireAdmin(user, org);
    const token = secret(),
      id = uid(),
      role = choice(body.role, ["member", "admin"], "role");
    const expires = Date.now() + 7 * 86400_000;
    this.run(
      "INSERT INTO invites VALUES(?,?,?,?,?,?,?)",
      id,
      org,
      hash(token),
      text(body.name, "invitee name"),
      role,
      expires,
      user,
    );
    this.audit(org, user, "member.invited", id);
    return { id, token, expires_at: new Date(expires).toISOString() };
  }
  accept(token: unknown, existingUser?: string) {
    const key = text(token, "invitation", 200);
    return this.db.transaction(() => {
      const invitation = this.get(
        "SELECT * FROM invites WHERE hash=? AND expires_at>?",
        hash(key),
        Date.now(),
      );
      if (!invitation)
        fail(404, "Invitation expired, revoked, or already used");
      // Revoking the inviter also invalidates their outstanding invitations.
      if (
        !admin(
          this.role(String(invitation.created_by), String(invitation.org_id)),
        )
      )
        fail(403, "Invitation no longer valid");
      const user = existingUser ?? uid();
      if (!existingUser)
        this.run("INSERT INTO users VALUES(?,?)", user, invitation.name);
      this.run(
        "INSERT OR IGNORE INTO members VALUES(?,?,?)",
        invitation.org_id,
        user,
        invitation.role,
      );
      this.run("DELETE FROM invites WHERE id=?", invitation.id);
      this.audit(String(invitation.org_id), user, "member.joined", user);
      return {
        token: existingUser ? undefined : this.session(user),
        org: invitation.org_id,
      };
    })();
  }
  invitations(user: string, org: string) {
    this.requireAdmin(user, org);
    return this.all(
      "SELECT id,name,role,expires_at FROM invites WHERE org_id=? AND expires_at>? ORDER BY name",
      org,
      Date.now(),
    );
  }
  revokeInvite(user: string, org: string, id: string) {
    this.requireAdmin(user, org);
    this.run("DELETE FROM invites WHERE org_id=? AND id=?", org, id);
    this.audit(org, user, "invitation.revoked", id);
  }
  changeMember(user: string, org: string, target: string, role: unknown) {
    const actorRole = this.role(user, org),
      targetRole = this.role(target, org);
    const remove = role === "remove";
    if (user !== target || !remove) this.requireAdmin(user, org);
    if (targetRole === "owner")
      fail(409, "Transfer ownership before changing the owner");
    if (actorRole !== "owner" && targetRole === "admin" && user !== target)
      fail(403, "Only the owner can change another admin");
    if (!remove) choice(role, ["admin", "member"], "role");
    this.db.transaction(() => {
      if (remove) {
        this.run(
          "DELETE FROM members WHERE org_id=? AND user_id=?",
          org,
          target,
        );
        this.run(
          "DELETE FROM space_members WHERE user_id=? AND space_id IN(SELECT id FROM spaces WHERE org_id=?)",
          target,
          org,
        );
        this.run(
          "DELETE FROM group_members WHERE user_id=? AND group_id IN(SELECT id FROM groups WHERE org_id=?)",
          target,
          org,
        );
        this.run(
          "DELETE FROM invites WHERE org_id=? AND created_by=?",
          org,
          target,
        );
      } else
        this.run(
          "UPDATE members SET role=? WHERE org_id=? AND user_id=?",
          role as string,
          org,
          target,
        );
      this.audit(
        org,
        user,
        remove ? "member.removed" : "member.role_changed",
        target,
      );
    })();
  }
  transferOwner(user: string, org: string, target: string) {
    if (this.role(user, org) !== "owner")
      fail(403, "Only the owner can transfer ownership");
    this.role(target, org);
    if (target === user) fail(400, "Choose another member");
    this.db.transaction(() => {
      this.run(
        "UPDATE members SET role='admin' WHERE org_id=? AND user_id=?",
        org,
        user,
      );
      this.run(
        "UPDATE members SET role='owner' WHERE org_id=? AND user_id=?",
        org,
        target,
      );
      this.audit(org, user, "workspace.owner_changed", target);
    })();
  }
  spaceRole(
    user: string,
    org: string,
    space: string,
  ): "viewer" | "editor" | null {
    const role = this.role(user, org);
    const s = this.get(
      "SELECT visibility FROM spaces WHERE org_id=? AND id=?",
      org,
      space,
    );
    if (!s) return null;
    if (admin(role)) return "editor";
    const grants = this.all(
      "SELECT role FROM space_members WHERE space_id=? AND user_id=? UNION ALL SELECT sg.role FROM space_groups sg JOIN group_members gm ON gm.group_id=sg.group_id WHERE sg.space_id=? AND gm.user_id=?",
      space,
      user,
      space,
      user,
    );
    // An explicit viewer grant narrows access in a team-visible space.
    if (grants.length)
      return grants.some((g) => g.role === "editor") ? "editor" : "viewer";
    return s.visibility === "team" ? "editor" : null;
  }
  space(user: string, org: string, id: string, write = false): TeamSpace {
    const role = this.spaceRole(user, org, id);
    if (!role) fail(404, "Space not found or access removed");
    if (write && role !== "editor")
      fail(403, "This space is read-only for you");
    const row = this.get("SELECT * FROM spaces WHERE id=?", id)!;
    return { ...row, api_enabled: !!row.api_enabled, role } as TeamSpace;
  }
  spaces(user: string, org: string): TeamSpace[] {
    this.role(user, org);
    return this.all(
      "SELECT * FROM spaces WHERE org_id=? ORDER BY name",
      org,
    ).flatMap((s) => {
      const role = this.spaceRole(user, org, String(s.id));
      return role
        ? [{ ...s, api_enabled: !!s.api_enabled, role } as TeamSpace]
        : [];
    });
  }
  createSpace(user: string, org: string, body: Record<string, unknown>) {
    this.requireAdmin(user, org);
    const id = uid();
    this.run(
      "INSERT INTO spaces(id,org_id,name,description,visibility) VALUES(?,?,?,?,?)",
      id,
      org,
      text(body.name, "space name"),
      text(body.description ?? "", "description", 4000, true),
      choice(body.visibility, ["team", "restricted"], "visibility"),
    );
    this.audit(org, user, "space.created", id);
    return this.space(user, org, id);
  }
  updateSpace(
    user: string,
    org: string,
    id: string,
    body: Record<string, unknown>,
  ) {
    this.requireAdmin(user, org);
    const previous = this.space(user, org, id);
    this.run(
      "UPDATE spaces SET name=?,description=?,visibility=? WHERE id=?",
      text(body.name, "space name"),
      text(body.description ?? "", "description", 4000, true),
      choice(body.visibility, ["team", "restricted"], "visibility"),
      id,
    );
    this.run(
      "UPDATE spaces SET api_enabled=? WHERE id=?",
      body.api_enabled == null
        ? previous.api_enabled
          ? 1
          : 0
        : body.api_enabled === true
          ? 1
          : 0,
      id,
    );
    this.audit(org, user, "space.updated", id);
    return this.space(user, org, id);
  }
  grants(user: string, org: string, space: string) {
    this.requireAdmin(user, org);
    this.space(user, org, space);
    return this.all(
      "SELECT u.id,u.name,'member' AS kind,sm.role FROM space_members sm JOIN users u ON u.id=sm.user_id WHERE sm.space_id=? UNION ALL SELECT g.id,g.name,'group' AS kind,sg.role FROM space_groups sg JOIN groups g ON g.id=sg.group_id WHERE sg.space_id=?",
      space,
      space,
    );
  }
  grant(
    user: string,
    org: string,
    space: string,
    body: Record<string, unknown>,
  ) {
    this.requireAdmin(user, org);
    this.space(user, org, space);
    const kind = choice(body.kind, ["member", "group"], "grant kind"),
      id = text(body.id, "recipient", 100);
    if (kind === "member") this.role(id, org);
    else if (
      !this.get("SELECT id FROM groups WHERE id=? AND org_id=?", id, org)
    )
      fail(404, "Group not found");
    const table = kind === "member" ? "space_members" : "space_groups",
      key = kind === "member" ? "user_id" : "group_id";
    if (body.role === "remove")
      this.run(`DELETE FROM ${table} WHERE space_id=? AND ${key}=?`, space, id);
    else
      this.run(
        `INSERT INTO ${table} VALUES(?,?,?) ON CONFLICT(space_id,${key}) DO UPDATE SET role=excluded.role`,
        space,
        id,
        choice(body.role, ["viewer", "editor"], "space role"),
      );
    this.audit(org, user, "space.access_changed", space);
    return this.grants(user, org, space);
  }
  groups(user: string, org: string) {
    this.requireAdmin(user, org);
    return this.all(
      "SELECT id,name FROM groups WHERE org_id=? ORDER BY name",
      org,
    ).map((g) => ({
      ...g,
      member_ids: this.all(
        "SELECT user_id FROM group_members WHERE group_id=?",
        g.id,
      ).map((m) => m.user_id),
    }));
  }
  saveGroup(
    user: string,
    org: string,
    body: Record<string, unknown>,
    groupId?: string,
  ) {
    this.requireAdmin(user, org);
    const name = text(body.name, "group name"),
      members = ids(body.member_ids, "members");
    members.forEach((id) => this.role(id, org));
    if (
      groupId &&
      !this.get("SELECT id FROM groups WHERE id=? AND org_id=?", groupId, org)
    )
      fail(404, "Group not found");
    const id = groupId ?? uid();
    this.db.transaction(() => {
      if (groupId) this.run("UPDATE groups SET name=? WHERE id=?", name, id);
      else this.run("INSERT INTO groups VALUES(?,?,?)", id, org, name);
      this.run("DELETE FROM group_members WHERE group_id=?", id);
      members.forEach((member) =>
        this.run("INSERT INTO group_members VALUES(?,?)", id, member),
      );
      this.audit(org, user, "group.updated", id);
    })();
    return { id, name, member_ids: members };
  }
  deleteGroup(user: string, org: string, id: string) {
    this.requireAdmin(user, org);
    this.run("DELETE FROM groups WHERE org_id=? AND id=?", org, id);
    this.audit(org, user, "group.deleted", id);
  }
  folders(user: string, org: string) {
    const visible = new Set(this.spaces(user, org).map((s) => s.id));
    return this.all(
      "SELECT f.* FROM folders f JOIN spaces s ON s.id=f.space_id WHERE s.org_id=? ORDER BY f.name",
      org,
    ).filter((f) => visible.has(String(f.space_id)));
  }
  saveFolder(
    user: string,
    org: string,
    body: Record<string, unknown>,
    folderId?: string,
  ) {
    const space = text(body.space_id, "space", 100);
    this.space(user, org, space, true);
    const parent =
      body.parent_id == null
        ? null
        : text(body.parent_id, "parent folder", 100);
    const previous = folderId
      ? this.get(
          "SELECT * FROM folders WHERE id=? AND space_id=?",
          folderId,
          space,
        )
      : null;
    if (folderId && !previous) fail(404, "Folder not found");
    if (
      parent &&
      !this.get(
        "SELECT id FROM folders WHERE id=? AND space_id=?",
        parent,
        space,
      )
    )
      fail(404, "Parent folder not found");
    if (folderId && parent) {
      const descendants = this.all(
        "WITH RECURSIVE tree(id) AS(SELECT ? UNION SELECT f.id FROM folders f JOIN tree t ON f.parent_id=t.id) SELECT id FROM tree",
        folderId,
      );
      if (descendants.some((d) => d.id === parent))
        fail(400, "A folder cannot contain itself");
    }
    const id = folderId ?? uid(),
      name = text(body.name, "folder name"),
      description = text(body.description ?? "", "description", 4000, true);
    if (folderId)
      this.run(
        "UPDATE folders SET name=?,description=?,parent_id=? WHERE id=?",
        name,
        description,
        parent,
        id,
      );
    else
      this.run(
        "INSERT INTO folders VALUES(?,?,?,?,?)",
        id,
        space,
        parent,
        name,
        description,
      );
    this.audit(org, user, "folder.saved", id);
    return { id, space_id: space, parent_id: parent, name, description };
  }
  note(user: string, org: string, id: string): TeamNote {
    const n = this.get(
      "SELECT n.*,u.name AS owner_name FROM notes n JOIN users u ON u.id=n.owner_id WHERE n.id=?",
      id,
    );
    if (!n) fail(404, "Meeting not found");
    const space = this.space(user, org, String(n.space_id));
    return {
      ...n,
      folder_ids: this.all(
        "SELECT folder_id FROM note_folders WHERE note_id=?",
        id,
      ).map((f) => String(f.folder_id)),
      can_edit: space.role === "editor",
      can_manage:
        space.role === "editor" &&
        (n.owner_id === user || admin(this.role(user, org))),
    } as TeamNote;
  }
  validateFolders(
    user: string,
    org: string,
    space: string,
    values: unknown,
  ): string[] {
    this.space(user, org, space, true);
    const folders = ids(values ?? [], "folders");
    for (const id of folders)
      if (
        !this.get("SELECT id FROM folders WHERE id=? AND space_id=?", id, space)
      )
        fail(400, "Folder must belong to the destination space");
    return folders;
  }
  publish(user: string, org: string, body: Record<string, unknown>) {
    const space = text(body.space_id, "space", 100),
      folders = this.validateFolders(user, org, space, body.folder_ids);
    if (
      body.expected_access_version != null &&
      body.expected_access_version !== this.accessVersion(org)
    )
      fail(
        409,
        "Workspace access changed after the preview opened. Review the destination again before publishing.",
      );
    const key = text(body.source_key, "source key", 200),
      title = text(body.title, "title", 500);
    const summary = text(body.summary, "notes", 300_000),
      transcript = text(body.transcript ?? "", "transcript", 1_000_000, true);
    const occurredInput = text(body.occurred_at, "meeting date", 50);
    if (!Number.isFinite(Date.parse(occurredInput)))
      fail(400, "Invalid meeting date");
    const occurred = new Date(occurredInput).toISOString();
    const previous = this.get(
      "SELECT id FROM notes WHERE space_id=? AND owner_id=? AND source_key=?",
      space,
      user,
      key,
    );
    if (previous)
      fail(
        409,
        "This meeting is already shared in this space. Open the shared copy to edit it.",
      );
    const id = uid(),
      at = now();
    this.db.transaction(() => {
      this.run(
        "INSERT INTO notes(id,space_id,owner_id,source_key,title,summary,transcript,occurred_at,published_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
        id,
        space,
        user,
        key,
        title,
        summary,
        transcript,
        occurred,
        at,
        at,
      );
      folders.forEach((folder) =>
        this.run("INSERT INTO note_folders VALUES(?,?)", id, folder),
      );
      this.audit(org, user, "meeting.published", id);
    })();
    return this.note(user, org, id);
  }
  updateNote(
    user: string,
    org: string,
    id: string,
    body: Record<string, unknown>,
  ) {
    const n = this.note(user, org, id);
    if (!n.can_edit) fail(403, "This meeting is read-only for you");
    if (n.trashed_at) fail(409, "Restore this meeting before editing it");
    if (body.revision !== n.revision)
      fail(409, "Someone updated this meeting. Reload it before saving.");
    const title = text(body.title, "title", 500),
      summary = text(body.summary, "notes", 300_000);
    const folders = this.validateFolders(
      user,
      org,
      n.space_id,
      body.folder_ids,
    );
    this.db.transaction(() => {
      this.run(
        "UPDATE notes SET title=?,summary=?,updated_at=?,revision=revision+1 WHERE id=?",
        title,
        summary,
        now(),
        id,
      );
      this.run("DELETE FROM note_folders WHERE note_id=?", id);
      folders.forEach((folder) =>
        this.run("INSERT INTO note_folders VALUES(?,?)", id, folder),
      );
      this.audit(org, user, "meeting.edited", id);
    })();
    return this.note(user, org, id);
  }
  trash(
    user: string,
    org: string,
    id: string,
    revision: unknown,
    restore = false,
  ) {
    const n = this.note(user, org, id);
    if (!n.can_manage)
      fail(
        403,
        "Only the publisher or an admin can remove or restore this meeting",
      );
    if (revision !== n.revision)
      fail(409, "This meeting changed. Reload it before continuing.");
    this.run(
      "UPDATE notes SET trashed_at=?,updated_at=?,revision=revision+1 WHERE id=?",
      restore ? null : now(),
      now(),
      id,
    );
    this.audit(org, user, restore ? "meeting.restored" : "meeting.trashed", id);
  }
  private noteScope(
    user: string,
    org: string,
    spaceId: string,
    folderId: string,
  ) {
    let spaces = this.spaces(user, org).map((s) => s.id);
    if (spaceId) {
      this.space(user, org, spaceId);
      spaces = [spaceId];
    }
    if (folderId) {
      const folder = this.get(
        "SELECT space_id FROM folders WHERE id=?",
        folderId,
      );
      if (!folder) fail(404, "Folder not found");
      this.space(user, org, String(folder.space_id));
      if (!spaces.includes(String(folder.space_id)))
        fail(400, "Folder is outside this scope");
      spaces = [String(folder.space_id)];
    }
    return {
      sql: `n.space_id IN(${spaces.length ? spaces.map(() => "?").join(",") : "NULL"})
      AND (?='' OR n.id IN( WITH RECURSIVE tree(id) AS(SELECT ? UNION SELECT f.id FROM folders f JOIN tree t ON f.parent_id=t.id) SELECT nf.note_id FROM note_folders nf JOIN tree t ON nf.folder_id=t.id))
      `,
      values: [...spaces, folderId, folderId],
    };
  }
  search(user: string, org: string, query: URLSearchParams) {
    this.role(user, org);
    const q = query.get("q") ?? "";
    let expression: string;
    try {
      expression = searchExpression(q);
    } catch (e) {
      fail(400, String((e as Error).message));
    }
    const kind = choice(
      query.get("kind") ?? "all",
      ["all", "messages", "meetings"],
      "search kind",
    );
    const bounded = (key: string, max = 200) => {
      const value = query.get(key) ?? "";
      if (value.length > max) fail(400, `Invalid ${key}`);
      return value;
    };
    const room = bounded("room"),
      author = bounded("author"),
      space = bounded("space"),
      folder = bounded("folder");
    if (room) this.chatRoom(user, org, room);
    const date = (key: string) => {
      const value = bounded(key, 10);
      if (
        value &&
        (!/^\d{4}-\d{2}-\d{2}$/.test(value) ||
          !Number.isFinite(Date.parse(value)) ||
          new Date(value).toISOString().slice(0, 10) !== value)
      )
        fail(400, `Invalid ${key} date`);
      return value;
    };
    const since = date("since"),
      until = date("until");
    if (since && until && since > until)
      fail(400, "Start date must be before end date");
    const limitValue = query.get("limit") ?? "20";
    if (!/^\d{1,3}$/.test(limitValue) || Number(limitValue) < 1)
      fail(400, "Invalid search limit");
    const limit = Math.min(50, Number(limitValue));
    const scope = this.noteScope(user, org, space, folder);
    // Cursors contain only bounded offsets. Scope binding prevents accidental
    // reuse across filters/accounts; authorization is rerun for every page.
    const fingerprint = createHash("sha256")
      .update(
        JSON.stringify([
          user,
          org,
          expression,
          room,
          author,
          space,
          folder,
          since,
          until,
        ]),
      )
      .digest("hex");
    const offset = (key: string) => {
      const value = bounded(key, 512);
      if (!value) return 0;
      try {
        const parsed = JSON.parse(Buffer.from(value, "base64url").toString());
        if (
          parsed.scope !== fingerprint ||
          parsed.group !== key ||
          !Number.isSafeInteger(parsed.offset) ||
          parsed.offset < 0 ||
          parsed.offset > 10000
        )
          throw new Error();
        return parsed.offset as number;
      } catch {
        fail(400, "Invalid search cursor. Start a new search.");
      }
    };
    const messageOffset = offset("messages_cursor"),
      meetingOffset = offset("meetings_cursor");
    const cursor = (group: string, count: number, start: number) =>
      count > limit && start + limit <= 10000
        ? Buffer.from(
            JSON.stringify({
              scope: fingerprint,
              group,
              offset: start + limit,
            }),
          ).toString("base64url")
        : null;
    const start = `\u0001${randomBytes(16).toString("hex")}\u0002`,
      end = `\u0001${randomBytes(16).toString("hex")}\u0002`;
    const page: import("../../src/teams/types").TeamSearchPage = {
      messages: { hits: [], cursor: null },
      meetings: { hits: [], cursor: null },
    };
    if (!expression) return page;
    if (kind !== "meetings") {
      const rows = this.all<{
        id: string;
        room_id: string;
        author_id: string;
        author_name: string;
        created_at: string;
        excerpt: string;
      }>(
        `SELECT m.id,m.room_id,m.author_id,u.name AS author_name,m.created_at,
         substr(snippet(chat_messages_fts,0,?,?,'…',40),1,4096) AS excerpt
         FROM chat_messages_fts JOIN chat_messages m ON m.rowid=chat_messages_fts.rowid
         JOIN chat_rooms r ON r.id=m.room_id JOIN users u ON u.id=m.author_id
         WHERE chat_messages_fts MATCH ? AND r.org_id=? AND m.deleted_at IS NULL
         AND (r.kind='channel' OR EXISTS(SELECT 1 FROM chat_participants p WHERE p.room_id=r.id AND p.user_id=?))
         AND (?='' OR r.id=?) AND (?='' OR m.author_id=?)
         AND (?='' OR substr(m.created_at,1,10)>=?) AND (?='' OR substr(m.created_at,1,10)<=?)
         ORDER BY bm25(chat_messages_fts),m.created_seq DESC,m.id LIMIT ? OFFSET ?`,
        start,
        end,
        expression,
        org,
        user,
        room,
        room,
        author,
        author,
        since,
        since,
        until,
        until,
        limit + 1,
        messageOffset,
      );
      const labels = new Map<string, string>();
      page.messages.hits = rows.slice(0, limit).map(({ excerpt, ...hit }) => {
        let label = labels.get(hit.room_id);
        if (!label) {
          const r = this.chatRoom(user, org, hit.room_id);
          label =
            r.kind === "channel"
              ? r.is_default
                ? "Team chat"
                : `#${r.name}`
              : r.participants
                  .filter((p) => p.id !== user)
                  .map((p) => p.name)
                  .join(", ") || "Direct message";
          labels.set(hit.room_id, label);
        }
        return {
          ...hit,
          kind: "message",
          room_label: label,
          snippet: snippetParts(excerpt, start, end),
        };
      });
      page.messages.cursor = cursor(
        "messages_cursor",
        rows.length,
        messageOffset,
      );
    }
    if (kind !== "messages" && !room) {
      const rows = this.all<{
        id: string;
        space_id: string;
        title: string;
        occurred_at: string;
        excerpt: string;
      }>(
        `SELECT n.id,n.space_id,n.title,n.occurred_at,substr(snippet(notes_fts,-1,?,?,'…',40),1,4096) AS excerpt
         FROM notes_fts JOIN notes n ON n.rowid=notes_fts.rowid
         WHERE notes_fts MATCH ? AND ${scope.sql} AND n.trashed_at IS NULL
         AND (?='' OR n.owner_id=?) AND (?='' OR substr(n.occurred_at,1,10)>=?) AND (?='' OR substr(n.occurred_at,1,10)<=?)
         ORDER BY bm25(notes_fts,5.0,2.0,1.0),n.occurred_at DESC,n.id LIMIT ? OFFSET ?`,
        start,
        end,
        expression,
        ...scope.values,
        author,
        author,
        since,
        since,
        until,
        until,
        limit + 1,
        meetingOffset,
      );
      page.meetings.hits = rows.slice(0, limit).map(({ excerpt, ...hit }) => ({
        ...hit,
        kind: "meeting",
        snippet: snippetParts(excerpt, start, end),
      }));
      page.meetings.cursor = cursor(
        "meetings_cursor",
        rows.length,
        meetingOffset,
      );
    }
    return page;
  }
  listNotes(
    user: string,
    org: string,
    query = "",
    spaceId = "",
    folderId = "",
    trash = false,
    limit = 100,
    offset = 0,
  ): TeamNoteRow[] {
    const scope = this.noteScope(user, org, spaceId, folderId);
    let expression: string;
    try {
      expression = searchExpression(query);
    } catch (e) {
      fail(400, (e as Error).message);
    }
    if (query.trim() && !expression) return [];
    const rows = this.all<{ id: string }>(
      `SELECT n.id FROM notes n ${expression ? "JOIN notes_fts ON notes_fts.rowid=n.rowid" : ""}
       WHERE ${scope.sql} AND n.trashed_at IS ${trash ? "NOT " : ""}NULL
       ${expression ? "AND notes_fts MATCH ?" : ""}
       ORDER BY ${expression ? "bm25(notes_fts,5.0,2.0,1.0)," : ""}n.occurred_at DESC,n.id LIMIT ? OFFSET ?`,
      ...scope.values,
      ...(expression ? [expression] : []),
      Math.min(100, limit),
      offset,
    );
    return rows.map(({ id }) => {
      const n = this.note(user, org, id),
        { summary, transcript, ...meta } = n;
      const content =
        query && !summary.toLowerCase().includes(query.toLowerCase())
          ? transcript
          : summary;
      const start = query
        ? Math.max(0, content.toLowerCase().indexOf(query.toLowerCase()) - 80)
        : 0;
      return {
        ...meta,
        excerpt: content.slice(start, start + 240),
        has_transcript: !!transcript,
      };
    });
  }
  context(user: string, org: string, body: Record<string, unknown>) {
    const question = text(body.question, "question", 6000);
    const conversation = body.conversation_id
      ? this.conversation(
          user,
          org,
          text(body.conversation_id, "conversation", 100),
        )
      : null;
    const requested = conversation?.scope ?? body;
    const selected =
      requested.note_ids == null ? [] : ids(requested.note_ids, "meetings", 40);
    const space =
        typeof requested.space_id === "string" ? requested.space_id : "",
      folder =
        typeof requested.folder_id === "string" ? requested.folder_id : "";
    const history = conversation?.turns.slice(-6) ?? [];
    const promptHistory: {
      question: string;
      answer: string;
      sources: { id: string; citation: string }[];
    }[] = [];
    let historyLimited = (conversation?.turns.length ?? 0) > history.length;
    for (const turn of [...history].reverse()) {
      const previous = {
        question: turn.question.slice(0, 800),
        answer: turn.answer.slice(0, 1800),
        sources: turn.sources.map((source) => ({
          id: source.id,
          citation: source.citation,
        })),
      };
      if (JSON.stringify([previous, ...promptHistory]).length > 6000) {
        historyLimited = true;
        break;
      }
      historyLimited ||=
        previous.question.length < turn.question.length ||
        previous.answer.length < turn.answer.length;
      promptHistory.unshift(previous);
    }
    const previousIds = [
      ...new Set(history.flatMap((t) => t.sources.map((s) => s.id))),
    ];
    // Resolve scope before reading any content. Exact selected IDs never bypass it.
    const scope = this.noteScope(user, org, space, folder);
    const stop = new Set(
      "the and for with from what when where which who how are was were this that these those your our any all have has had will would could should list find meeting meetings shared sources cite original conversation".split(
        " ",
      ),
    );
    const terms = [
      ...new Set(
        [question, ...history.slice(-2).map((t) => t.question)]
          .join(" ")
          .toLowerCase()
          .match(/[\p{L}\p{N}]{3,}/gu) ?? [],
      ),
    ]
      .filter((t) => !stop.has(t))
      .slice(0, 24);
    const scores = terms.map(
      () =>
        "(5*(instr(lower(n.title),?)>0)+2*(instr(lower(n.summary),?)>0)+(instr(lower(n.transcript),?)>0))",
    );
    const where = `${scope.sql} AND n.trashed_at IS NULL ${selected.length ? `AND n.id IN(${selected.map(() => "?").join(",")})` : ""}`;
    const total = Number(
      this.get(
        `SELECT count(*) AS count FROM notes n WHERE ${where}`,
        ...scope.values,
        ...selected,
      )!.count,
    );
    if (selected.length && total !== selected.length)
      fail(404, "Selected meeting is unavailable in this scope");
    // Rank inside the authorized SQL scope across the whole library, not just
    // the latest list page. Fetch full text only for the selected candidates.
    const candidates = this.all<{ id: string }>(
      `SELECT n.id, ${scores.join("+") || "0"}${previousIds.length ? `+(n.id IN(${previousIds.map(() => "?").join(",")}))` : ""} AS score FROM notes n WHERE ${where} ORDER BY score DESC,n.occurred_at DESC,n.id LIMIT 12`,
      ...terms.flatMap((t) => [t, t, t]),
      ...previousIds,
      ...scope.values,
      ...selected,
    );
    // Reserve room for the question and bounded history in the local model's
    // context window. Stored conversations keep complete answers separately.
    let remaining = Math.max(
      4000,
      20_000 - question.length - JSON.stringify(promptHistory).length,
    );
    let truncated = false;
    const sources: TeamSource[] = [];
    for (const { id } of candidates) {
      if (remaining < 300) break;
      const n = this.note(user, org, id);
      const budget = Math.min(4500, remaining);
      const passage = (content: string, size: number) => {
        if (content.length <= size) return content;
        truncated = true;
        const positions = terms
          .map((t) => content.toLowerCase().indexOf(t))
          .filter((i) => i >= 0);
        const start = Math.max(
          0,
          (positions.length ? Math.min(...positions) : 0) - 250,
        );
        return `${start ? "…" : ""}${content.slice(start, start + size - 2)}…`;
      };
      const summaryBudget = n.transcript
        ? Math.min(2200, Math.floor(budget / 2))
        : budget - 80;
      const summary = passage(n.summary, summaryBudget);
      const transcript = n.transcript
        ? passage(n.transcript, Math.max(100, budget - summary.length - 80))
        : "";
      const excerpt = `${n.occurred_at}\nSummary:\n${summary}${transcript ? `\nTranscript excerpt:\n${transcript}` : ""}`;
      remaining -= excerpt.length;
      sources.push({
        id: n.id,
        title: n.title,
        revision: n.revision,
        citation: `S${sources.length + 1}`,
        excerpt,
      });
    }
    return {
      sources,
      limited: total > sources.length || truncated || historyLimited,
      conversation_revision: conversation?.revision ?? 0,
      // Old citations refer to that turn's source map, not the current excerpts.
      history: promptHistory,
    };
  }
  conversation(user: string, org: string, id: string): TeamConversation {
    this.role(user, org);
    const row = this.get(
      "SELECT * FROM conversations WHERE id=? AND org_id=? AND user_id=?",
      id,
      org,
      user,
    );
    if (!row) fail(404, "Conversation not found");
    const scope = JSON.parse(
      String(row.scope_json),
    ) as TeamConversation["scope"];
    let allowed: ReturnType<TeamStore["noteScope"]>;
    try {
      allowed = this.noteScope(user, org, scope.space_id, scope.folder_id);
    } catch (error) {
      if (!(error instanceof TeamError)) throw error;
      fail(
        410,
        "This conversation's sources changed or access was removed. Start a new conversation.",
      );
    }
    const turns = this.all(
      "SELECT * FROM conversation_turns WHERE conversation_id=? ORDER BY position",
      id,
    ).map((t) => {
      const sources = this.all(
        "SELECT * FROM conversation_sources WHERE turn_id=? ORDER BY length(citation),citation",
        t.id,
      ).map((s) => {
        const note = this.get(
          `SELECT n.id,n.title,n.revision FROM notes n WHERE n.id=? AND ${allowed.sql} AND n.trashed_at IS NULL`,
          s.note_id,
          ...allowed.values,
        );
        if (!note || note.revision !== s.revision)
          fail(
            410,
            "This conversation's sources changed or access was removed. Start a new conversation.",
          );
        return {
          id: String(note.id),
          title: String(note.title),
          revision: Number(note.revision),
          citation: String(s.citation),
          excerpt: "",
        };
      });
      return {
        id: String(t.id),
        question: String(t.question),
        answer: String(t.answer),
        limited: !!t.limited,
        created_at: String(t.created_at),
        sources,
      } satisfies TeamTurn;
    });
    return {
      id,
      revision: Number(row.revision),
      scope,
      turns,
      updated_at: String(row.updated_at),
    };
  }
  conversations(user: string, org: string, offset = 0) {
    this.role(user, org);
    if (!Number.isSafeInteger(offset) || offset < 0 || offset > 100_000)
      fail(400, "Invalid offset");
    const rows = this.all(
      "SELECT id,updated_at FROM conversations WHERE org_id=? AND user_id=? ORDER BY updated_at DESC,id LIMIT 30 OFFSET ?",
      org,
      user,
      offset,
    );
    return {
      conversations: rows.map((row) => {
        try {
          const c = this.conversation(user, org, String(row.id));
          return {
            id: c.id,
            question: c.turns[0]?.question ?? "New conversation",
            updated_at: c.updated_at,
            available: true,
          };
        } catch (error) {
          if (!(error instanceof TeamError) || error.status !== 410)
            throw error;
          return {
            id: String(row.id),
            question: "Sources changed or access removed",
            updated_at: String(row.updated_at),
            available: false,
          };
        }
      }),
      next_offset: rows.length === 30 ? offset + rows.length : null,
    };
  }
  appendConversation(user: string, org: string, body: Record<string, unknown>) {
    this.role(user, org);
    const question = text(body.question, "question", 6000),
      answer = text(body.answer, "answer", 20_000);
    const existing = body.conversation_id
      ? text(body.conversation_id, "conversation", 100)
      : null;
    return this.db.transaction(() => {
      // Re-read authority, the conversation and the exact source revisions in the
      // same transaction that appends the turn. Concurrent Mac requests cannot fork it.
      const packet = this.context(user, org, body);
      if (body.expected_revision !== packet.conversation_revision)
        fail(
          409,
          "The conversation changed on another device. Reopen it before asking again.",
        );
      if (packet.conversation_revision >= 20)
        fail(
          400,
          "This conversation has reached 20 answers. Start a new conversation.",
        );
      if (
        !packet.sources.length ||
        !Array.isArray(body.sources) ||
        body.sources.length !== packet.sources.length ||
        !packet.sources.every((source, i) => {
          const supplied = (body.sources as unknown[])[i] as Record<
            string,
            unknown
          > | null;
          return (
            supplied &&
            supplied.id === source.id &&
            supplied.revision === source.revision &&
            supplied.citation === source.citation
          );
        })
      )
        fail(
          409,
          "Shared sources changed while answering. Ask again for the current version.",
        );
      const id = existing ?? uid(),
        at = now(),
        turn = uid();
      if (!existing) {
        const scope = {
          space_id: typeof body.space_id === "string" ? body.space_id : "",
          folder_id: typeof body.folder_id === "string" ? body.folder_id : "",
          note_ids:
            body.note_ids == null ? [] : ids(body.note_ids, "meetings", 40),
        };
        this.run(
          "INSERT INTO conversations VALUES(?,?,?,?,?,?,?)",
          id,
          org,
          user,
          JSON.stringify(scope),
          0,
          at,
          at,
        );
      }
      this.run(
        "INSERT INTO conversation_turns VALUES(?,?,?,?,?,?,?)",
        turn,
        id,
        packet.conversation_revision + 1,
        question,
        answer,
        packet.limited ? 1 : 0,
        at,
      );
      for (const source of packet.sources)
        this.run(
          "INSERT INTO conversation_sources VALUES(?,?,?,?)",
          turn,
          source.id,
          source.revision,
          source.citation,
        );
      this.run(
        "UPDATE conversations SET revision=revision+1,updated_at=? WHERE id=?",
        at,
        id,
      );
      return this.conversation(user, org, id);
    })();
  }
  deleteConversation(user: string, org: string, id: string) {
    this.role(user, org);
    const result = this.run(
      "DELETE FROM conversations WHERE id=? AND org_id=? AND user_id=?",
      id,
      org,
      user,
    );
    if (!result.changes) fail(404, "Conversation not found");
  }
  recipes(user: string, org: string): TeamRecipe[] {
    this.role(user, org);
    return this.all("SELECT * FROM recipes WHERE org_id=? ORDER BY name", org);
  }
  saveAnswer(user: string, org: string, body: Record<string, unknown>) {
    this.role(user, org);
    const question = text(body.question, "question", 6000),
      answer = text(body.answer, "answer", 100_000);
    if (
      !Array.isArray(body.sources) ||
      body.sources.length === 0 ||
      body.sources.length > 12
    )
      fail(400, "An answer needs its meeting sources");
    const sourceIds = new Set<string>(),
      citations = new Set<string>();
    const sources = body.sources.map((value: unknown) => {
      if (!value || typeof value !== "object")
        fail(400, "Invalid answer source");
      const source = value as Record<string, unknown>;
      const id = text(source.id, "meeting", 100),
        citation = text(source.citation, "citation", 5);
      if (
        !/^S(?:[1-9]|1[0-2])$/.test(citation) ||
        sourceIds.has(id) ||
        citations.has(citation)
      )
        fail(400, "Invalid answer citations");
      sourceIds.add(id);
      citations.add(citation);
      const note = this.note(user, org, id);
      if (note.trashed_at || source.revision !== note.revision)
        fail(
          409,
          "Shared sources changed. Ask again before saving this answer.",
        );
      return { id, citation, revision: note.revision };
    });
    const id = uid();
    this.db.transaction(() => {
      this.run(
        "INSERT INTO saved_answers VALUES(?,?,?,?,?,?,?)",
        id,
        org,
        user,
        question,
        answer,
        body.limited ? 1 : 0,
        now(),
      );
      sources.forEach((s) =>
        this.run(
          "INSERT INTO answer_sources VALUES(?,?,?,?)",
          id,
          s.id,
          s.revision,
          s.citation,
        ),
      );
    })();
    return { id };
  }
  answer(user: string, org: string, id: string) {
    this.role(user, org);
    const row = this.get(
      "SELECT * FROM saved_answers WHERE id=? AND org_id=? AND user_id=?",
      id,
      org,
      user,
    );
    if (!row) fail(404, "Saved answer not found");
    const sources = this.all(
      "SELECT * FROM answer_sources WHERE answer_id=? ORDER BY length(citation),citation",
      id,
    ).map((s) => {
      let note: TeamNote;
      try {
        note = this.note(user, org, String(s.note_id));
      } catch {
        fail(410, "This answer has a source you can no longer access");
      }
      if (note.trashed_at || note.revision !== s.revision)
        fail(
          410,
          "This answer has a source that changed or was removed. Ask again for the current version.",
        );
      return {
        id: note.id,
        title: note.title,
        revision: note.revision,
        citation: String(s.citation),
        excerpt: "",
      };
    });
    return {
      id,
      question: row.question,
      answer: row.answer,
      created_at: row.created_at,
      limited: !!row.limited,
      sources,
    };
  }
  answers(user: string, org: string) {
    this.role(user, org);
    return this.all(
      "SELECT id,created_at FROM saved_answers WHERE org_id=? AND user_id=? ORDER BY created_at DESC,id LIMIT 100",
      org,
      user,
    ).map((row) => {
      try {
        const a = this.answer(user, org, String(row.id));
        return {
          id: a.id,
          question: a.question,
          created_at: a.created_at,
          available: true,
        };
      } catch (e) {
        if (!(e instanceof TeamError) || e.status !== 410) throw e;
        return {
          id: row.id,
          question: "Sources changed or access removed",
          created_at: row.created_at,
          available: false,
        };
      }
    });
  }
  deleteAnswer(user: string, org: string, id: string) {
    this.role(user, org);
    this.run(
      "DELETE FROM saved_answers WHERE id=? AND org_id=? AND user_id=?",
      id,
      org,
      user,
    );
  }
  saveRecipe(
    user: string,
    org: string,
    body: Record<string, unknown>,
    recipeId?: string,
  ) {
    this.role(user, org);
    const old = recipeId
      ? this.get<TeamRecipe>(
          "SELECT * FROM recipes WHERE id=? AND org_id=?",
          recipeId,
          org,
        )
      : null;
    if (recipeId && !old) fail(404, "Recipe not found");
    if (old && old.owner_id !== user && !admin(this.role(user, org)))
      fail(403, "Only the author or an admin can edit this prompt");
    if (old && body.revision !== old.revision)
      fail(409, "This prompt changed. Reload before saving.");
    const name = text(body.name, "name"),
      prompt = text(body.prompt, "prompt", 12_000),
      kind = choice(body.kind, ["recipe", "template"], "kind");
    const id = recipeId ?? uid();
    if (old)
      this.run(
        "UPDATE recipes SET name=?,prompt=?,kind=?,revision=revision+1 WHERE id=?",
        name,
        prompt,
        kind,
        id,
      );
    else
      this.run(
        "INSERT INTO recipes(id,org_id,owner_id,name,prompt,kind) VALUES(?,?,?,?,?,?)",
        id,
        org,
        user,
        name,
        prompt,
        kind,
      );
    this.audit(org, user, "prompt.saved", id);
    return this.get("SELECT * FROM recipes WHERE id=?", id);
  }
  deleteRecipe(user: string, org: string, id: string) {
    this.role(user, org);
    const old = this.get(
      "SELECT owner_id FROM recipes WHERE id=? AND org_id=?",
      id,
      org,
    );
    if (!old) fail(404, "Prompt not found");
    if (old.owner_id !== user && !admin(this.role(user, org)))
      fail(403, "Only the author or an admin can delete this prompt");
    this.run("DELETE FROM recipes WHERE id=?", id);
    this.audit(org, user, "prompt.deleted", id);
  }
  chatRoom(user: string, org: string, id: string): TeamChatRoom {
    const role = this.role(user, org);
    const row = this.get(
      "SELECT * FROM chat_rooms WHERE id=? AND org_id=?",
      id,
      org,
    );
    if (
      !row ||
      (row.kind === "direct" &&
        !this.get(
          "SELECT 1 FROM chat_participants WHERE room_id=? AND user_id=?",
          id,
          user,
        ))
    )
      fail(404, "Conversation not found or access removed");
    const participants =
      row.kind === "direct"
        ? this.all<{ id: string; name: string; active: number }>(
            `SELECT u.id,u.name,COALESCE((SELECT avatar_version FROM user_profiles WHERE user_id=u.id),'') AS avatar_version,EXISTS(SELECT 1 FROM members m WHERE m.org_id=? AND m.user_id=u.id) AS active
       FROM chat_participants p JOIN users u ON u.id=p.user_id WHERE p.room_id=? ORDER BY u.name,u.id`,
            org,
            id,
          ).map((p) => ({ ...p, active: !!p.active }))
        : [];
    const unread = Number(
      this.get(
        `SELECT COUNT(*) AS n FROM chat_messages
      WHERE room_id=? AND author_id<>? AND deleted_at IS NULL AND created_seq>
      COALESCE((SELECT seq FROM chat_reads WHERE room_id=? AND user_id=?),0)`,
        id,
        user,
        id,
        user,
      )!.n,
    );
    const mentionMembers = this.all<{ id: string; name: string }>(
      "SELECT u.id,u.name FROM members m JOIN users u ON u.id=m.user_id WHERE m.org_id=?",
      org,
    ).filter(
      (m) =>
        row.kind === "channel" ||
        participants.some((p) => p.id === m.id && p.active),
    );
    const unreadMessages = this.all<{
      id: string;
      body: string;
      created_seq: number;
    }>(
      `SELECT id,body,created_seq FROM chat_messages WHERE room_id=? AND author_id<>? AND deleted_at IS NULL
       AND created_seq>COALESCE((SELECT seq FROM chat_reads WHERE room_id=? AND user_id=?),0)`,
      id,
      user,
      id,
      user,
    );
    const seenMentions = new Set(
      this.all<{ message_id: string }>(
        "SELECT mr.message_id FROM chat_mention_reads mr JOIN chat_messages m ON m.id=mr.message_id WHERE mr.user_id=? AND m.room_id=?",
        user,
        id,
      ).map((row) => row.message_id),
    );
    const unreadMentions = unreadMessages.filter(
      (m) =>
        !seenMentions.has(m.id) &&
        findMentions(m.body, mentionMembers).some(
          (mention) => mention.user.id === user,
        ),
    );
    const last = this.get(
      "SELECT MAX(COALESCE(deleted_at,edited_at,created_at)) AS at FROM chat_messages WHERE room_id=?",
      id,
    )?.at;
    const preview = this.get<{
      author_id: string;
      author_name: string;
      body: string;
      created_at: string;
      deleted_at: string | null;
    }>(
      `SELECT m.author_id,u.name AS author_name,m.body,m.created_at,m.deleted_at
       FROM chat_messages m JOIN users u ON u.id=m.author_id
       WHERE m.room_id=? ORDER BY m.created_seq DESC LIMIT 1`,
      id,
    );
    return {
      id,
      org_id: org,
      kind: row.kind as "channel" | "direct",
      name: String(row.name),
      description: String(row.description),
      created_by: String(row.created_by),
      created_at: String(row.created_at),
      archived_at: row.archived_at as string | null,
      revision: Number(row.revision),
      is_default: id === `general-${org}`,
      message_extras: true,
      attachments_enabled: true,
      unread_navigation: true,
      pins_enabled: true,
      first_unread_root_id: this.get(
        "SELECT COALESCE(thread_id,id) AS id FROM chat_messages WHERE room_id=? AND author_id<>? AND deleted_at IS NULL AND created_seq>COALESCE((SELECT seq FROM chat_reads WHERE room_id=? AND user_id=?),0) ORDER BY created_seq LIMIT 1",
        id,
        user,
        id,
        user,
      )?.id as string | undefined,
      read_version: Number(
        this.get(
          "SELECT version FROM chat_read_state WHERE room_id=? AND user_id=?",
          id,
          user,
        )?.version ?? 0,
      ),
      read_held: !!this.get(
        "SELECT held FROM chat_read_state WHERE room_id=? AND user_id=?",
        id,
        user,
      )?.held,
      read_cursor: Number(
        this.get(
          "SELECT seq FROM chat_reads WHERE room_id=? AND user_id=?",
          id,
          user,
        )?.seq ?? 0,
      ),
      first_unread_id: this.get(
        "SELECT id FROM chat_messages WHERE room_id=? AND author_id<>? AND deleted_at IS NULL AND created_seq>COALESCE((SELECT seq FROM chat_reads WHERE room_id=? AND user_id=?),0) ORDER BY created_seq LIMIT 1",
        id,
        user,
        id,
        user,
      )?.id as string | undefined,
      first_unread_seq: Number(
        this.get(
          "SELECT MIN(created_seq) AS seq FROM chat_messages WHERE room_id=? AND author_id<>? AND deleted_at IS NULL AND created_seq>COALESCE((SELECT seq FROM chat_reads WHERE room_id=? AND user_id=?),0)",
          id,
          user,
          id,
          user,
        )?.seq ?? 0,
      ),
      participants,
      unread,
      unread_mentions: unreadMentions.length,
      latest_unread_message_id: unreadMessages.reduce<
        (typeof unreadMessages)[number] | undefined
      >(
        (latest, m) =>
          !latest || m.created_seq > latest.created_seq ? m : latest,
        undefined,
      )?.id,
      latest_unread_mention_id: unreadMentions.reduce<
        (typeof unreadMentions)[number] | undefined
      >(
        (latest, m) =>
          !latest || m.created_seq > latest.created_seq ? m : latest,
        undefined,
      )?.id,
      latest_unread_message_seq: unreadMessages.reduce(
        (seq, m) => Math.max(seq, m.created_seq),
        0,
      ),
      latest_unread_mention_seq: unreadMentions.reduce(
        (seq, m) => Math.max(seq, m.created_seq),
        0,
      ),
      notification_cursor: this.latestChatCursor(id),
      notification_user_id: user,
      notification_mode: (this.get(
        "SELECT mode FROM chat_notification_preferences WHERE room_id=? AND user_id=?",
        id,
        user,
      )?.mode ?? "default") as TeamChatRoom["notification_mode"],
      last_activity: String(last ?? row.created_at),
      last_message: preview
        ? {
            author_id: preview.author_id,
            author_name: preview.author_name,
            body: preview.deleted_at
              ? "Message deleted"
              : preview.body.slice(0, 160),
            created_at: preview.created_at,
          }
        : null,
      can_manage:
        row.kind === "channel" && (row.created_by === user || admin(role)),
      can_send:
        !row.archived_at &&
        (row.kind === "channel" || participants.every((p) => p.active)),
    };
  }
  chatRooms(user: string, org: string): TeamChatRoom[] {
    this.role(user, org);
    return this.all<{ id: string }>(
      `SELECT r.id FROM chat_rooms r WHERE org_id=? AND
      (kind='channel' OR EXISTS(SELECT 1 FROM chat_participants p WHERE p.room_id=r.id AND p.user_id=?))
      ORDER BY r.kind,r.name COLLATE NOCASE,r.id`,
      org,
      user,
    ).map((r) => this.chatRoom(user, org, r.id));
  }
  createChatRoom(
    user: string,
    org: string,
    body: Record<string, unknown>,
  ): TeamChatRoom {
    this.role(user, org);
    const kind = choice(body.kind, ["channel", "direct"], "conversation kind");
    return this.db.transaction(() => {
      if (kind === "direct") {
        const peer = text(body.member_id, "teammate", 100);
        if (peer === user) fail(400, "Choose another teammate");
        this.role(peer, org);
        const key = [user, peer].sort().join(":");
        const old = this.get(
          "SELECT id FROM chat_rooms WHERE org_id=? AND direct_key=?",
          org,
          key,
        );
        if (old) return this.chatRoom(user, org, String(old.id));
        const id = uid();
        this.run(
          "INSERT INTO chat_rooms(id,org_id,kind,name,direct_key,created_by,created_at) VALUES(?,?,'direct','',?,?,?)",
          id,
          org,
          key,
          user,
          now(),
        );
        for (const member of [user, peer])
          this.run("INSERT INTO chat_participants VALUES(?,?)", id, member);
        return this.chatRoom(user, org, id);
      }
      const name = this.chatChannelName(body.name);
      if (
        this.get(
          "SELECT id FROM chat_rooms WHERE org_id=? AND kind='channel' AND name=? COLLATE NOCASE",
          org,
          name,
        )
      )
        fail(409, "A channel with that name already exists");
      if (
        Number(
          this.get(
            "SELECT COUNT(*) AS n FROM chat_rooms WHERE org_id=? AND kind='channel'",
            org,
          )!.n,
        ) >= 100
      )
        fail(400, "This workspace has reached its 100-channel limit");
      const id = uid();
      this.run(
        "INSERT INTO chat_rooms(id,org_id,kind,name,description,created_by,created_at) VALUES(?,?,'channel',?,?,?,?)",
        id,
        org,
        name,
        text(body.description ?? "", "description", 500, true),
        user,
        now(),
      );
      this.audit(org, user, "channel.created", id);
      return this.chatRoom(user, org, id);
    })();
  }
  private chatChannelName(value: unknown) {
    const name = text(value, "channel name", 48)
      .toLowerCase()
      .replace(/\s+/g, "-");
    if (!/^[a-z0-9][a-z0-9_-]*$/.test(name))
      fail(
        400,
        "Use letters, numbers, hyphens or underscores for the channel name",
      );
    return name;
  }
  updateChatRoom(
    user: string,
    org: string,
    id: string,
    body: Record<string, unknown>,
  ) {
    return this.db.transaction(() => {
      const room = this.chatRoom(user, org, id);
      if (!room.can_manage)
        fail(403, "Only the channel creator or an admin can change it");
      if (body.revision !== room.revision)
        fail(409, "This channel changed. Reload before saving.");
      if (body.archived != null && typeof body.archived !== "boolean")
        fail(400, "Invalid archive setting");
      if (room.is_default && body.archived === true)
        fail(400, "The general channel stays available to everyone");
      const name =
        body.name == null ? room.name : this.chatChannelName(body.name);
      if (room.is_default && name !== "general")
        fail(400, "The general channel keeps its name");
      if (
        this.get(
          "SELECT id FROM chat_rooms WHERE org_id=? AND kind='channel' AND name=? COLLATE NOCASE AND id<>?",
          org,
          name,
          id,
        )
      )
        fail(409, "A channel with that name already exists");
      this.run(
        "UPDATE chat_rooms SET name=?,description=?,archived_at=?,revision=revision+1 WHERE id=?",
        name,
        body.description == null
          ? room.description
          : text(body.description, "description", 500, true),
        body.archived == null
          ? room.archived_at
          : body.archived
            ? (room.archived_at ?? now())
            : null,
        id,
      );
      this.audit(org, user, "channel.updated", id);
      return this.chatRoom(user, org, id);
    })();
  }
  private profileSummary(user: string): TeamUser {
    return this.get<TeamUser>(
      `SELECT u.id,u.name,COALESCE(p.title,'') AS title,COALESCE(p.about,'') AS about,
      COALESCE(p.avatar_version,'') AS avatar_version FROM users u LEFT JOIN user_profiles p ON p.user_id=u.id WHERE u.id=?`,
      user,
    )!;
  }
  profile(user: string): TeamProfile {
    return {
      ...this.profileSummary(user),
      avatar_data: String(
        this.get("SELECT avatar_data FROM user_profiles WHERE user_id=?", user)
          ?.avatar_data ?? "",
      ),
      revision: Number(
        this.get("SELECT revision FROM user_profiles WHERE user_id=?", user)
          ?.revision ?? 0,
      ),
    };
  }
  profileAvatar(user: string, org: string, member: string) {
    this.role(user, org);
    this.role(member, org);
    return {
      data:
        this.get(
          "SELECT avatar_data FROM user_profiles WHERE user_id=?",
          member,
        )?.avatar_data ?? "",
    };
  }
  updateProfile(user: string, body: Record<string, unknown>) {
    const name = text(body.name, "display name", 100);
    const title = text(body.title ?? "", "job title", 120, true);
    const about = text(body.about ?? "", "about", 500, true);
    const avatar = text(body.avatar_data ?? "", "profile photo", 100_000, true);
    if (avatar) validateAvatar(avatar);
    return this.db.transaction(() => {
      const old = this.profile(user);
      if (body.revision !== old.revision)
        fail(409, "Your profile changed. Reload before saving.");
      this.run("UPDATE users SET name=? WHERE id=?", name, user);
      this.run(
        `INSERT INTO user_profiles(user_id,title,about,avatar_data,avatar_version,revision) VALUES(?,?,?,?,?,1)
        ON CONFLICT(user_id) DO UPDATE SET title=excluded.title,about=excluded.about,avatar_data=excluded.avatar_data,
        avatar_version=excluded.avatar_version,revision=user_profiles.revision+1`,
        user,
        title,
        about,
        avatar,
        avatar ? hash(avatar) : "",
      );
      this.notifyChat();
      return this.profile(user);
    })();
  }
  private messageChanged(
    room: string,
    id: string,
    parent: string | null = null,
  ) {
    const event = this.run(
      "INSERT INTO chat_events(room_id,message_id) VALUES(?,?)",
      room,
      id,
    );
    if (parent) {
      this.run(
        "UPDATE chat_messages SET revision=revision+1 WHERE id=?",
        parent,
      );
      this.run(
        "INSERT INTO chat_events(room_id,message_id) VALUES(?,?)",
        room,
        parent,
      );
    }
    this.notifyChat(room);
    return Number(event.lastInsertRowid);
  }
  private messageRoom(user: string, org: string, id: string) {
    this.role(user, org);
    const row = this.get(
      "SELECT m.room_id FROM chat_messages m JOIN chat_rooms r ON r.id=m.room_id WHERE m.id=? AND r.org_id=?",
      id,
      org,
    );
    if (!row) fail(404, "Message not found");
    return this.chatRoom(user, org, String(row.room_id));
  }
  setConversationNotifications(
    user: string,
    org: string,
    id: string,
    body: Record<string, unknown>,
  ) {
    this.chatRoom(user, org, id);
    const mode = choice(
      body.mode,
      ["default", "messages", "mentions", "none"],
      "notification mode",
    );
    if (mode === "default")
      this.run(
        "DELETE FROM chat_notification_preferences WHERE room_id=? AND user_id=?",
        id,
        user,
      );
    else
      this.run(
        `INSERT INTO chat_notification_preferences(room_id,user_id,mode) VALUES(?,?,?)
      ON CONFLICT(room_id,user_id) DO UPDATE SET mode=excluded.mode`,
        id,
        user,
        mode,
      );
    return this.chatRoom(user, org, id);
  }
  messageLocation(user: string, org: string, id: string) {
    const room = this.messageRoom(user, org, id);
    const message = this.chatMessage(user, org, id, room);
    if (message.deleted_at) fail(404, "This message was deleted");
    const parent = message.thread_id
      ? this.chatMessage(user, org, message.thread_id, room)
      : undefined;
    return { room, message, parent };
  }
  readMention(user: string, org: string, id: string) {
    this.messageLocation(user, org, id);
    this.run(
      "INSERT OR IGNORE INTO chat_mention_reads(user_id,message_id) VALUES(?,?)",
      user,
      id,
    );
    return { ok: true };
  }
  mentions(user: string, org: string, query: URLSearchParams) {
    this.role(user, org);
    let before = query.has("before")
      ? this.chatCursor(Number(query.get("before")), "mentions cursor")
      : Number.MAX_SAFE_INTEGER;
    const unreadOnly = query.get("unread") === "true";
    const members = this.all<{ id: string; name: string }>(
      "SELECT u.id,u.name FROM members m JOIN users u ON u.id=m.user_id WHERE m.org_id=?",
      org,
    );
    const items: (ReturnType<TeamStore["messageLocation"]> & {
      unread: boolean;
    })[] = [];
    const rooms = new Map<string, TeamChatRoom>();
    while (true) {
      const rows = this.all<{
        id: string;
        room_id: string;
        body: string;
        created_seq: number;
        read_seq: number;
        mention_read: number;
      }>(
        `SELECT m.id,m.room_id,m.body,m.created_seq,COALESCE(rd.seq,0) AS read_seq,
         EXISTS(SELECT 1 FROM chat_mention_reads mr WHERE mr.user_id=? AND mr.message_id=m.id) AS mention_read
         FROM chat_messages m JOIN chat_rooms r ON r.id=m.room_id
         LEFT JOIN chat_reads rd ON rd.room_id=r.id AND rd.user_id=?
         WHERE r.org_id=? AND m.author_id<>? AND m.deleted_at IS NULL AND m.created_seq<?
         AND (r.kind='channel' OR EXISTS(SELECT 1 FROM chat_participants p WHERE p.room_id=r.id AND p.user_id=?))
         ORDER BY m.created_seq DESC LIMIT 200`,
        user,
        user,
        org,
        user,
        before,
        user,
      );
      for (const row of rows) {
        before = row.created_seq;
        const unread = row.created_seq > row.read_seq && !row.mention_read;
        if (unreadOnly && !unread) continue;
        let room = rooms.get(row.room_id);
        if (!room) {
          room = this.chatRoom(user, org, row.room_id);
          rooms.set(row.room_id, room);
        }
        const eligible =
          room.kind === "channel"
            ? members
            : members.filter((m) =>
                room!.participants.some((p) => p.id === m.id && p.active),
              );
        if (!findMentions(row.body, eligible).some((m) => m.user.id === user))
          continue;
        items.push({ ...this.messageLocation(user, org, row.id), unread });
        if (items.length === 30) return { items, next_before: before };
      }
      if (rows.length < 200) return { items, next_before: null };
    }
  }
  reactToMessage(
    user: string,
    org: string,
    id: string,
    body: Record<string, unknown>,
  ) {
    const emoji = text(body.emoji, "emoji", 24);
    if (!REACTION_NAMES.has(emoji) || typeof body.active !== "boolean")
      fail(400, "Choose a supported reaction");
    return this.db.transaction(() => {
      const room = this.messageRoom(user, org, id);
      const message = this.chatMessage(user, org, id, room);
      if (!room.can_send || message.deleted_at)
        fail(403, "This message cannot receive reactions");
      const result = body.active
        ? this.run(
            "INSERT OR IGNORE INTO chat_reactions(message_id,user_id,emoji,created_at) VALUES(?,?,?,?)",
            id,
            user,
            emoji,
            now(),
          )
        : this.run(
            "DELETE FROM chat_reactions WHERE message_id=? AND user_id=? AND emoji=?",
            id,
            user,
            emoji,
          );
      if (result.changes) {
        this.run("UPDATE chat_messages SET revision=revision+1 WHERE id=?", id);
        this.messageChanged(room.id, id);
      }
      return this.chatMessage(user, org, id, room);
    })();
  }
  private chatCursor(value: unknown, name: string) {
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0)
      fail(400, `Invalid ${name}`);
    return value;
  }
  private latestChatCursor(room: string) {
    return Number(
      this.get(
        "SELECT COALESCE(MAX(seq),0) AS seq FROM chat_events WHERE room_id=?",
        room,
      )!.seq,
    );
  }
  private chatMessage(
    user: string,
    org: string,
    id: string,
    room: TeamChatRoom,
  ): TeamChatMessage {
    const row = this.get(
      `SELECT m.*,u.name AS author_name FROM chat_messages m JOIN users u ON u.id=m.author_id
      WHERE m.id=? AND m.room_id=?`,
      id,
      room.id,
    );
    if (!row) fail(404, "Message not found");
    return {
      id,
      room_id: room.id,
      pinned: !!this.get("SELECT 1 FROM chat_pins WHERE message_id=?", id),
      author_id: String(row.author_id),
      author_name: String(row.author_name),
      body: row.deleted_at ? "" : String(row.body),
      attachments: row.deleted_at
        ? []
        : this.all<import("../../src/teams/types").TeamAttachment>(
            "SELECT id,name,mime,size FROM chat_attachments WHERE message_id=? ORDER BY rowid",
            id,
          ),
      thread_id: row.thread_id as string | null,
      reply_count: Number(
        this.get(
          "SELECT COUNT(*) AS n FROM chat_messages WHERE thread_id=? AND deleted_at IS NULL",
          id,
        )!.n,
      ),
      last_reply_at: this.get(
        "SELECT MAX(created_at) AS at FROM chat_messages WHERE thread_id=? AND deleted_at IS NULL",
        id,
      )?.at as string | null,
      reactions: row.deleted_at
        ? []
        : this.all<{ emoji: string; count: number; reacted: number }>(
            "SELECT emoji,COUNT(*) AS count,MAX(user_id=?) AS reacted FROM chat_reactions WHERE message_id=? GROUP BY emoji ORDER BY MIN(created_at),emoji",
            user,
            id,
          ).map((r) => ({
            ...r,
            reacted: !!r.reacted,
            names: this.all<{ name: string }>(
              "SELECT u.name FROM chat_reactions r JOIN users u ON u.id=r.user_id WHERE r.message_id=? AND r.emoji=? ORDER BY r.created_at LIMIT 20",
              id,
              r.emoji,
            ).map((p) => p.name),
          })),
      created_at: String(row.created_at),
      edited_at: row.edited_at as string | null,
      deleted_at: row.deleted_at as string | null,
      revision: Number(row.revision),
      created_seq: Number(row.created_seq),
      can_edit: !row.deleted_at && room.can_send && row.author_id === user,
      can_delete:
        !row.deleted_at &&
        (row.author_id === user ||
          (room.kind === "channel" && admin(this.role(user, org)))),
    };
  }
  chatMessages(
    user: string,
    org: string,
    id: string,
    query: URLSearchParams,
  ): TeamChatPage {
    return this.db.transaction(() => {
      const room = this.chatRoom(user, org, id);
      if (
        ["before", "after", "around", "newer"].filter((key) => query.has(key))
          .length > 1
      )
        fail(400, "Choose one history direction or live updates");
      const thread = query.get("thread") || null;
      const parent = thread
        ? this.chatMessage(user, org, thread, room)
        : undefined;
      if (parent?.thread_id) fail(400, "Reply in the original thread");
      const latest = this.latestChatCursor(id);
      if (query.has("around") || query.has("newer")) {
        let rows: { id: string; created_seq: number }[];
        if (query.has("around")) {
          const anchorId = query.get("around")!;
          const anchor = this.chatMessage(user, org, anchorId, room);
          if (
            anchor.room_id !== id ||
            (anchor.thread_id ?? null) !== thread ||
            anchor.deleted_at
          )
            fail(404, "Message not found in this conversation");
          const before = this.all<{ id: string; created_seq: number }>(
            "SELECT id,created_seq FROM chat_messages WHERE room_id=? AND thread_id IS ? AND created_seq<=? ORDER BY created_seq DESC LIMIT 26",
            id,
            thread,
            anchor.created_seq,
          ).reverse();
          const after = this.all<{ id: string; created_seq: number }>(
            "SELECT id,created_seq FROM chat_messages WHERE room_id=? AND thread_id IS ? AND created_seq>? ORDER BY created_seq LIMIT 25",
            id,
            thread,
            anchor.created_seq,
          );
          rows = [...before, ...after];
        } else {
          const after = this.chatCursor(
            Number(query.get("newer")),
            "history cursor",
          );
          rows = this.all<{ id: string; created_seq: number }>(
            "SELECT id,created_seq FROM chat_messages WHERE room_id=? AND thread_id IS ? AND created_seq>? ORDER BY created_seq LIMIT 50",
            id,
            thread,
            after,
          );
        }
        const first = rows[0]?.created_seq,
          last = rows.at(-1)?.created_seq;
        const older =
          first != null &&
          this.get(
            "SELECT 1 FROM chat_messages WHERE room_id=? AND thread_id IS ? AND created_seq<? LIMIT 1",
            id,
            thread,
            first,
          );
        const newer =
          last != null &&
          this.get(
            "SELECT 1 FROM chat_messages WHERE room_id=? AND thread_id IS ? AND created_seq>? LIMIT 1",
            id,
            thread,
            last,
          );
        return {
          room,
          parent,
          live: true,
          messages: rows.map((row) =>
            this.chatMessage(user, org, row.id, room),
          ),
          cursor: latest,
          has_more: false,
          older_before: older ? first! : null,
          newer_after: newer ? last! : null,
        };
      }
      if (query.has("after")) {
        const after = this.chatCursor(
          Number(query.get("after")),
          "update cursor",
        );
        if (after > latest)
          fail(409, "Conversation history changed. Reload it.");
        const events = this.all<{ seq: number; message_id: string }>(
          "SELECT seq,message_id FROM chat_events WHERE room_id=? AND seq>? ORDER BY seq LIMIT 101",
          id,
          after,
        );
        const page = events.slice(0, 100);
        return {
          room,
          parent,
          live: true,
          messages: [...new Set(page.map((e) => e.message_id))]
            .map((m) => this.chatMessage(user, org, m, room))
            .filter((m) => (m.thread_id ?? null) === thread),
          cursor: page.at(-1)?.seq ?? after,
          has_more: events.length > 100,
          older_before: null,
        };
      }
      const before = query.has("before")
        ? this.chatCursor(Number(query.get("before")), "history cursor")
        : Number.MAX_SAFE_INTEGER;
      const rows = this.all<{ id: string }>(
        "SELECT id FROM chat_messages WHERE room_id=? AND thread_id IS ? AND created_seq<? ORDER BY created_seq DESC LIMIT 51",
        id,
        thread,
        before,
      );
      const messages = rows
        .slice(0, 50)
        .reverse()
        .map((r) => this.chatMessage(user, org, r.id, room));
      return {
        room,
        parent,
        live: true,
        messages,
        cursor: latest,
        has_more: rows.length > 50,
        older_before: rows.length > 50 ? messages[0].created_seq : null,
      };
    })();
  }
  sendChatMessage(
    user: string,
    org: string,
    roomId: string,
    body: Record<string, unknown>,
  ) {
    return this.db.transaction(() => {
      const room = this.chatRoom(user, org, roomId);
      if (!room.can_send)
        fail(
          409,
          room.archived_at
            ? "This channel is archived"
            : "This teammate is no longer in the workspace",
        );
      const thread =
        body.thread_id == null ? null : text(body.thread_id, "thread", 100);
      const parent = thread ? this.chatMessage(user, org, thread, room) : null;
      if (parent?.thread_id) fail(400, "Reply in the original thread");
      if (parent?.deleted_at)
        fail(409, "This thread's original message was deleted");
      const attachments = validateAttachments(body.attachments);
      const content = text(
        body.body ?? "",
        "message",
        10_000,
        attachments.length > 0,
      );
      const fingerprint = attachments.length
        ? hash(
            JSON.stringify([
              content,
              attachments.map((a) => [
                a.name,
                a.mime,
                hash(a.bytes.toString("base64")),
              ]),
            ]),
          )
        : hash(content);
      const client = text(body.client_id, "message identifier", 80);
      if (!/^[a-zA-Z0-9_-]{16,80}$/.test(client))
        fail(400, "Invalid message identifier");
      const old = this.get(
        "SELECT id,original_hash,thread_id FROM chat_messages WHERE room_id=? AND author_id=? AND client_id=?",
        roomId,
        user,
        client,
      );
      if (old) {
        if (
          old.original_hash !== fingerprint ||
          (old.thread_id ?? null) !== thread
        )
          fail(409, "This send attempt already belongs to another message");
        return this.chatMessage(user, org, String(old.id), room);
      }
      const used = Number(
        this.get(
          "SELECT COALESCE(SUM(a.size),0) AS n FROM chat_attachments a JOIN chat_messages m ON m.id=a.message_id JOIN chat_rooms r ON r.id=m.room_id WHERE r.org_id=?",
          org,
        )!.n,
      );
      if (
        used + attachments.reduce((n, a) => n + a.size, 0) >
        TEAM_ATTACHMENT_QUOTA
      )
        fail(413, "This team has reached its attachment storage limit");
      const id = uid();
      this.run(
        "INSERT INTO chat_messages(id,room_id,author_id,client_id,original_hash,body,created_at,thread_id) VALUES(?,?,?,?,?,?,?,?)",
        id,
        roomId,
        user,
        client,
        fingerprint,
        content,
        now(),
        thread,
      );
      for (const a of attachments)
        this.run(
          "INSERT INTO chat_attachments(id,message_id,name,mime,size,data) VALUES(?,?,?,?,?,?)",
          uid(),
          id,
          a.name,
          a.mime,
          a.size,
          a.bytes,
        );
      const event = this.messageChanged(roomId, id, thread);
      this.run("UPDATE chat_messages SET created_seq=? WHERE id=?", event, id);
      return this.chatMessage(user, org, id, room);
    })();
  }
  changeChatMessage(
    user: string,
    org: string,
    id: string,
    body: Record<string, unknown>,
    remove = false,
  ) {
    return this.db.transaction(() => {
      this.role(user, org);
      const row = this.get(
        "SELECT m.room_id FROM chat_messages m JOIN chat_rooms r ON r.id=m.room_id WHERE m.id=? AND r.org_id=?",
        id,
        org,
      );
      if (!row) fail(404, "Message not found");
      const room = this.chatRoom(user, org, String(row.room_id));
      const message = this.chatMessage(user, org, id, room);
      if (!(remove ? message.can_delete : message.can_edit))
        fail(403, "You cannot change this message");
      if (body.revision !== message.revision)
        fail(409, "This message changed. Reload before saving.");
      if (remove)
        this.run(
          "UPDATE chat_messages SET body='',deleted_at=?,revision=revision+1 WHERE id=?",
          now(),
          id,
        );
      else
        this.run(
          "UPDATE chat_messages SET body=?,edited_at=?,revision=revision+1 WHERE id=?",
          text(body.body, "message", 10_000),
          now(),
          id,
        );
      if (remove) {
        this.run("DELETE FROM chat_reactions WHERE message_id=?", id);
        this.run("DELETE FROM chat_attachments WHERE message_id=?", id);
        this.run("DELETE FROM chat_pins WHERE message_id=?", id);
      }
      this.messageChanged(room.id, id, message.thread_id);
      return this.chatMessage(user, org, id, room);
    })();
  }
  pinMessage(user: string, org: string, id: string, active: unknown) {
    if (typeof active !== "boolean") fail(400, "Invalid pin state");
    return this.db.transaction(() => {
      const { room, message } = this.messageLocation(user, org, id);
      if (!room.can_send) fail(403, "This conversation is read-only");
      const old = this.get("SELECT 1 FROM chat_pins WHERE message_id=?", id);
      if (!!old === active) return message;
      if (active) {
        if (
          Number(
            this.get(
              "SELECT count(*) n FROM chat_pins p JOIN chat_messages m ON m.id=p.message_id WHERE m.room_id=?",
              room.id,
            )!.n,
          ) >= 50
        )
          fail(400, "Unpin a message before adding more (50 maximum)");
        this.run(
          "INSERT INTO chat_pins(message_id,user_id,created_at) VALUES(?,?,?)",
          id,
          user,
          now(),
        );
      } else this.run("DELETE FROM chat_pins WHERE message_id=?", id);
      this.run("UPDATE chat_messages SET revision=revision+1 WHERE id=?", id);
      this.messageChanged(room.id, id, message.thread_id);
      return this.chatMessage(user, org, id, room);
    })();
  }
  pinnedMessages(user: string, org: string, roomId: string) {
    this.chatRoom(user, org, roomId);
    return this.all<{
      message_id: string;
      pinned_at: string;
      pinned_by: string;
    }>(
      "SELECT p.message_id,p.created_at AS pinned_at,u.name AS pinned_by FROM chat_pins p JOIN chat_messages m ON m.id=p.message_id JOIN users u ON u.id=p.user_id WHERE m.room_id=? AND m.deleted_at IS NULL ORDER BY p.created_at DESC,p.message_id LIMIT 50",
      roomId,
    ).map((row) => ({
      ...this.messageLocation(user, org, row.message_id),
      pinned_at: row.pinned_at,
      pinned_by: row.pinned_by,
    }));
  }
  attachment(user: string, org: string, id: string) {
    this.role(user, org);
    const row = this.get<{
      message_id: string;
      name: string;
      mime: string;
      size: number;
      data: Uint8Array;
    }>("SELECT * FROM chat_attachments WHERE id=?", id);
    if (!row) fail(404, "Attachment unavailable");
    const location = this.messageLocation(user, org, row.message_id);
    if (location.message.deleted_at) fail(404, "Attachment unavailable");
    return {
      id,
      name: row.name,
      mime: row.mime,
      size: row.size,
      data: Buffer.from(row.data).toString("base64"),
    };
  }
  markChatUnread(
    user: string,
    org: string,
    roomId: string,
    messageId: unknown,
  ) {
    return this.db.transaction(() => {
      const room = this.chatRoom(user, org, roomId);
      const message = this.chatMessage(
        user,
        org,
        text(messageId, "message", 100),
        room,
      );
      if (message.deleted_at) fail(404, "Message unavailable");
      this.run(
        "INSERT INTO chat_reads(room_id,user_id,seq) VALUES(?,?,?) ON CONFLICT(room_id,user_id) DO UPDATE SET seq=excluded.seq",
        roomId,
        user,
        Math.max(0, message.created_seq - 1),
      );
      this.run(
        "INSERT INTO chat_read_state(room_id,user_id,version,held) VALUES(?,?,1,1) ON CONFLICT(room_id,user_id) DO UPDATE SET version=version+1,held=1",
        roomId,
        user,
      );
      return this.chatRoom(user, org, roomId);
    })();
  }
  readChat(
    user: string,
    org: string,
    id: string,
    value: unknown,
    version?: unknown,
    resume = false,
  ) {
    return this.db.transaction(() => {
      const room = this.chatRoom(user, org, id);
      if ((version ?? 0) !== room.read_version)
        fail(409, "Read state changed. Refresh the conversation.");
      const seq = this.chatCursor(value, "read cursor");
      if (seq > this.latestChatCursor(id))
        fail(400, "Read cursor is ahead of this conversation");
      if (room.read_held && !resume) return { held: true };
      this.run(
        "INSERT INTO chat_reads(room_id,user_id,seq) VALUES(?,?,?) ON CONFLICT(room_id,user_id) DO UPDATE SET seq=MAX(seq,excluded.seq)",
        id,
        user,
        seq,
      );
      if (resume)
        this.run(
          "UPDATE chat_read_state SET held=0 WHERE room_id=? AND user_id=?",
          id,
          user,
        );
      return { held: false };
    })();
  }
  snapshot(user: string, org: string) {
    const role = this.role(user, org);
    return {
      access_version: this.accessVersion(org),
      org: {
        ...this.get("SELECT id,name FROM organizations WHERE id=?", org),
        role,
      },
      user: this.profileSummary(user),
      spaces: this.spaces(user, org),
      folders: this.folders(user, org),
      members: this.members(user, org),
      recipes: this.recipes(user, org),
    };
  }
  private accessVersion(org: string) {
    return Number(
      this.get(
        "SELECT COALESCE(MAX(id),0) AS revision FROM audit WHERE org_id=? AND (action LIKE 'space.%' OR action LIKE 'member.%' OR action LIKE 'group.%' OR action='workspace.owner_changed')",
        org,
      )!.revision,
    );
  }
  integrationKeys(user: string, org: string) {
    this.requireAdmin(user, org);
    return this.all(
      "SELECT id,name,transcripts,created_at,expires_at,revoked_at FROM integration_keys WHERE org_id=? ORDER BY created_at DESC",
      org,
    ).map((k) => ({
      ...k,
      transcripts: !!k.transcripts,
      space_ids: this.all(
        "SELECT space_id FROM integration_spaces WHERE key_id=?",
        k.id,
      ).map((s) => s.space_id),
    }));
  }
  createIntegrationKey(
    user: string,
    org: string,
    body: Record<string, unknown>,
  ) {
    this.requireAdmin(user, org);
    const name = text(body.name, "integration name"),
      spaces = ids(body.space_ids, "spaces");
    if (!spaces.length) fail(400, "Choose at least one approved space");
    for (const id of spaces)
      if (!this.space(user, org, id).api_enabled)
        fail(400, "Enable integration access in the selected space first");
    const days = Number(body.days ?? 30);
    if (![7, 30, 90, 365].includes(days))
      fail(400, "Choose a supported expiry");
    const id = uid(),
      token = `nte_${secret()}`,
      expires = Date.now() + days * 86400_000;
    this.db.transaction(() => {
      this.run(
        "INSERT INTO integration_keys VALUES(?,?,?,?,?,?,?,NULL)",
        id,
        org,
        name,
        hash(token),
        body.transcripts === true ? 1 : 0,
        now(),
        expires,
      );
      spaces.forEach((space) =>
        this.run("INSERT INTO integration_spaces VALUES(?,?)", id, space),
      );
      this.audit(org, user, "integration.created", id);
    })();
    return { id, token, expires_at: expires };
  }
  revokeIntegrationKey(user: string, org: string, id: string) {
    this.requireAdmin(user, org);
    this.run(
      "UPDATE integration_keys SET revoked_at=? WHERE id=? AND org_id=?",
      now(),
      id,
      org,
    );
    this.audit(org, user, "integration.revoked", id);
  }
  authenticateIntegration(token: string): IntegrationAccess {
    const key =
      token.length <= 200
        ? this.get(
            "SELECT id,org_id,transcripts FROM integration_keys WHERE hash=? AND revoked_at IS NULL AND expires_at>?",
            hash(token),
            Date.now(),
          )
        : null;
    if (!key) fail(401, "Integration key expired, revoked, or invalid");
    const spaces = this.all(
      "SELECT s.id FROM integration_spaces i JOIN spaces s ON s.id=i.space_id WHERE i.key_id=? AND s.org_id=? AND s.api_enabled=1 ORDER BY s.id",
      key.id,
      key.org_id,
    ).map((s) => String(s.id));
    return {
      id: String(key.id),
      org: String(key.org_id),
      spaces,
      transcripts: !!key.transcripts,
    };
  }
  integrationRead(
    access: IntegrationAccess,
    resource: string,
    id: string | undefined,
    params: URLSearchParams,
  ) {
    // Re-authentication happens per HTTP call. No account session or administrator
    // privilege is ever derived from this workspace-owned, read-only key.
    const marks = access.spaces.length
      ? access.spaces.map(() => "?").join(",")
      : "NULL";
    if (resource === "spaces" && !id)
      return this.all(
        `SELECT id,name,description FROM spaces WHERE id IN(${marks}) ORDER BY name`,
        ...access.spaces,
      );
    if (resource === "folders" && !id)
      return this.all(
        `SELECT id,space_id,parent_id,name,description FROM folders WHERE space_id IN(${marks}) ORDER BY name`,
        ...access.spaces,
      );
    if (resource !== "notes") fail(404, "Not found");
    const shape = (n: Row) => ({
      id: n.id,
      space_id: n.space_id,
      title: n.title,
      summary: n.summary,
      ...(access.transcripts ? { transcript: n.transcript } : {}),
      occurred_at: n.occurred_at,
      updated_at: n.updated_at,
      revision: n.revision,
    });
    if (id) {
      const note = this.get(
        `SELECT * FROM notes WHERE id=? AND space_id IN(${marks}) AND trashed_at IS NULL`,
        id,
        ...access.spaces,
      );
      if (!note) fail(404, "Meeting not found");
      return shape(note);
    }
    const query = text(params.get("q") ?? "", "query", 500, true),
      space = params.get("space") ?? "",
      folder = params.get("folder") ?? "";
    if (space && !access.spaces.includes(space)) fail(404, "Space not found");
    if (
      folder &&
      !this.get(
        `SELECT id FROM folders WHERE id=? AND space_id IN(${marks}) AND (?='' OR space_id=?)`,
        folder,
        ...access.spaces,
        space,
        space,
      )
    )
      fail(404, "Folder not found");
    const offset = Number(params.get("offset") ?? 0);
    if (!Number.isSafeInteger(offset) || offset < 0 || offset > 100_000)
      fail(400, "Invalid offset");
    const rows = this.all(
      `SELECT id,space_id,title,substr(summary,1,240) AS excerpt,occurred_at,updated_at,revision FROM notes n WHERE space_id IN(${marks}) AND trashed_at IS NULL AND (?='' OR space_id=?)
      AND (?='' OR instr(lower(title||char(10)||summary${access.transcripts ? "||char(10)||transcript" : ""}),lower(?))>0)
      AND (?='' OR id IN(WITH RECURSIVE tree(id) AS(SELECT ? UNION SELECT f.id FROM folders f JOIN tree t ON f.parent_id=t.id) SELECT nf.note_id FROM note_folders nf JOIN tree t ON t.id=nf.folder_id))
      ORDER BY occurred_at DESC,id LIMIT 100 OFFSET ?`,
      ...access.spaces,
      space,
      space,
      query,
      query,
      folder,
      folder,
      offset,
    );
    return {
      notes: rows,
      next_offset: rows.length === 100 ? offset + rows.length : null,
    };
  }
  activity(user: string, org: string) {
    this.requireAdmin(user, org);
    return this.all(
      "SELECT a.id,a.action,a.target_id,a.at,u.name AS actor FROM audit a JOIN users u ON u.id=a.actor_id WHERE a.org_id=? ORDER BY a.id DESC LIMIT 100",
      org,
    );
  }
}
