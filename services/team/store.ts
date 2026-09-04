import { Database, type SQLQueryBindings } from "bun:sqlite";
import { readFileSync } from "node:fs";
import { randomBytes, createHash, timingSafeEqual } from "node:crypto";
import type { TeamRole, TeamSpace, TeamNote, TeamNoteRow, TeamSource, TeamRecipe } from "../../src/teams/types";

type Row = Record<string, string | number | null>;
export class TeamError extends Error {
  constructor(public status: number, message: string) { super(message); }
}
function fail(status: number, message: string): never { throw new TeamError(status, message); }
const uid = () => crypto.randomUUID();
const secret = () => randomBytes(32).toString("base64url");
const hash = (value: string) => createHash("sha256").update(value).digest("hex");
const now = () => new Date().toISOString();
export function text(value: unknown, name: string, max = 200, optional = false): string {
  if (typeof value !== "string" || value.length > max || (!optional && !value.trim())) fail(400, `Invalid ${name}`);
  return (value as string).trim();
}
function choice<T extends string>(value: unknown, allowed: readonly T[], name: string): T {
  if (!allowed.includes(value as T)) fail(400, `Invalid ${name}`);
  return value as T;
}
function ids(value: unknown, name: string, max = 100): string[] {
  if (!Array.isArray(value) || value.length > max) fail(400, `Invalid ${name}`);
  return [...new Set((value as unknown[]).map(v => text(v, name, 100)))];
}
const admin = (role: TeamRole) => role === "owner" || role === "admin";

export class TeamStore {
  readonly db: Database;
  constructor(path = ":memory:", private bootstrapKey = "") {
    this.db = new Database(path, { create: true, strict: true });
    this.db.exec(readFileSync(new URL("./schema.sql", import.meta.url), "utf8"));
  }
  all<T = Row>(sql: string, ...values: SQLQueryBindings[]): T[] { return this.db.query(sql).all(...values) as T[]; }
  get<T = Row>(sql: string, ...values: SQLQueryBindings[]): T | null { return this.db.query(sql).get(...values) as T | null; }
  run(sql: string, ...values: SQLQueryBindings[]) { return this.db.query(sql).run(...values); }
  audit(org: string, actor: string, action: string, target: string) {
    this.run("INSERT INTO audit(org_id,actor_id,action,target_id,at) VALUES(?,?,?,?,?)", org, actor, action, target, now());
  }
  session(user: string) {
    const token = secret();
    this.run("DELETE FROM sessions WHERE expires_at < ?", Date.now());
    this.run("INSERT INTO sessions VALUES(?,?,?)", hash(token), user, Date.now() + 30 * 86400_000);
    return token;
  }
  authenticate(token: string): string {
    if (token.length > 200) fail(401, "Sign in again");
    const row = this.get("SELECT user_id FROM sessions WHERE hash=? AND expires_at>?", hash(token), Date.now());
    return row ? String(row.user_id) : fail(401, "Sign in again");
  }
  signout(token: string) { this.run("DELETE FROM sessions WHERE hash=?", hash(token)); }
  bootstrap(key: string, organization: unknown, name: unknown) {
    const a = Buffer.from(hash(key)), b = Buffer.from(hash(this.bootstrapKey));
    if (!this.bootstrapKey || !timingSafeEqual(a, b)) fail(403, "Invalid setup key");
    return this.db.transaction(() => {
      if (this.get("SELECT id FROM organizations LIMIT 1")) fail(409, "This server is already set up. Use an invitation to join.");
      const user = uid();
      this.run("INSERT INTO users VALUES(?,?)", user, text(name, "name"));
      const org = this.createOrg(user, organization);
      return { token: this.session(user), org };
    })();
  }
  createOrg(user: string, name: unknown) {
    const id = uid();
    this.db.transaction(() => {
      this.run("INSERT INTO organizations VALUES(?,?,?)", id, text(name, "workspace name"), now());
      this.run("INSERT INTO members VALUES(?,?,'owner')", id, user);
      this.createSpace(user, id, { name: "Team knowledge", description: "Meetings and decisions shared with everyone in this workspace.", visibility: "team" });
      this.audit(id, user, "workspace.created", id);
    })();
    return id;
  }
  role(user: string, org: string): TeamRole {
    const row = this.get("SELECT role FROM members WHERE org_id=? AND user_id=?", org, user);
    return row ? row.role as TeamRole : fail(404, "Workspace not found or access removed");
  }
  requireAdmin(user: string, org: string) {
    if (!admin(this.role(user, org))) fail(403, "A workspace admin is required");
  }
  orgs(user: string) {
    return this.all("SELECT o.id,o.name,m.role FROM organizations o JOIN members m ON m.org_id=o.id WHERE m.user_id=? ORDER BY o.name", user);
  }
  members(user: string, org: string) {
    this.role(user, org);
    return this.all("SELECT u.id,u.name,m.role FROM members m JOIN users u ON u.id=m.user_id WHERE m.org_id=? ORDER BY u.name", org);
  }
  invite(user: string, org: string, body: Record<string, unknown>) {
    this.requireAdmin(user, org);
    const token = secret(), id = uid(), role = choice(body.role, ["member", "admin"], "role");
    const expires = Date.now() + 7 * 86400_000;
    this.run("INSERT INTO invites VALUES(?,?,?,?,?,?,?)", id, org, hash(token), text(body.name, "invitee name"), role, expires, user);
    this.audit(org, user, "member.invited", id);
    return { id, token, expires_at: new Date(expires).toISOString() };
  }
  accept(token: unknown, existingUser?: string) {
    const key = text(token, "invitation", 200);
    return this.db.transaction(() => {
      const invitation = this.get("SELECT * FROM invites WHERE hash=? AND expires_at>?", hash(key), Date.now());
      if (!invitation) fail(404, "Invitation expired, revoked, or already used");
      // Revoking the inviter also invalidates their outstanding invitations.
      if (!admin(this.role(String(invitation.created_by), String(invitation.org_id)))) fail(403, "Invitation no longer valid");
      const user = existingUser ?? uid();
      if (!existingUser) this.run("INSERT INTO users VALUES(?,?)", user, invitation.name);
      this.run("INSERT OR IGNORE INTO members VALUES(?,?,?)", invitation.org_id, user, invitation.role);
      this.run("DELETE FROM invites WHERE id=?", invitation.id);
      this.audit(String(invitation.org_id), user, "member.joined", user);
      return { token: existingUser ? undefined : this.session(user), org: invitation.org_id };
    })();
  }
  invitations(user: string, org: string) {
    this.requireAdmin(user, org);
    return this.all("SELECT id,name,role,expires_at FROM invites WHERE org_id=? AND expires_at>? ORDER BY name", org, Date.now());
  }
  revokeInvite(user: string, org: string, id: string) {
    this.requireAdmin(user, org);
    this.run("DELETE FROM invites WHERE org_id=? AND id=?", org, id);
    this.audit(org, user, "invitation.revoked", id);
  }
  changeMember(user: string, org: string, target: string, role: unknown) {
    const actorRole = this.role(user, org), targetRole = this.role(target, org);
    const remove = role === "remove";
    if (user !== target || !remove) this.requireAdmin(user, org);
    if (targetRole === "owner") fail(409, "Transfer ownership before changing the owner");
    if (actorRole !== "owner" && targetRole === "admin" && user !== target) fail(403, "Only the owner can change another admin");
    if (!remove) choice(role, ["admin", "member"], "role");
    this.db.transaction(() => {
      if (remove) {
        this.run("DELETE FROM members WHERE org_id=? AND user_id=?", org, target);
        this.run("DELETE FROM space_members WHERE user_id=? AND space_id IN(SELECT id FROM spaces WHERE org_id=?)", target, org);
        this.run("DELETE FROM group_members WHERE user_id=? AND group_id IN(SELECT id FROM groups WHERE org_id=?)", target, org);
        this.run("DELETE FROM invites WHERE org_id=? AND created_by=?", org, target);
      } else this.run("UPDATE members SET role=? WHERE org_id=? AND user_id=?", role as string, org, target);
      this.audit(org, user, remove ? "member.removed" : "member.role_changed", target);
    })();
  }
  transferOwner(user: string, org: string, target: string) {
    if (this.role(user, org) !== "owner") fail(403, "Only the owner can transfer ownership");
    this.role(target, org);
    if (target === user) fail(400, "Choose another member");
    this.db.transaction(() => {
      this.run("UPDATE members SET role='admin' WHERE org_id=? AND user_id=?", org, user);
      this.run("UPDATE members SET role='owner' WHERE org_id=? AND user_id=?", org, target);
      this.audit(org, user, "workspace.owner_changed", target);
    })();
  }
  spaceRole(user: string, org: string, space: string): "viewer" | "editor" | null {
    const role = this.role(user, org);
    const s = this.get("SELECT visibility FROM spaces WHERE org_id=? AND id=?", org, space);
    if (!s) return null;
    if (admin(role)) return "editor";
    const grants = this.all("SELECT role FROM space_members WHERE space_id=? AND user_id=? UNION ALL SELECT sg.role FROM space_groups sg JOIN group_members gm ON gm.group_id=sg.group_id WHERE sg.space_id=? AND gm.user_id=?", space, user, space, user);
    // An explicit viewer grant narrows access in a team-visible space.
    if (grants.length) return grants.some(g => g.role === "editor") ? "editor" : "viewer";
    return s.visibility === "team" ? "editor" : null;
  }
  space(user: string, org: string, id: string, write = false): TeamSpace {
    const role = this.spaceRole(user, org, id);
    if (!role) fail(404, "Space not found or access removed");
    if (write && role !== "editor") fail(403, "This space is read-only for you");
    return { ...this.get("SELECT * FROM spaces WHERE id=?", id), role } as TeamSpace;
  }
  spaces(user: string, org: string): TeamSpace[] {
    this.role(user, org);
    return this.all("SELECT * FROM spaces WHERE org_id=? ORDER BY name", org).flatMap(s => {
      const role = this.spaceRole(user, org, String(s.id));
      return role ? [{ ...s, role } as TeamSpace] : [];
    });
  }
  createSpace(user: string, org: string, body: Record<string, unknown>) {
    this.requireAdmin(user, org);
    const id = uid();
    this.run("INSERT INTO spaces VALUES(?,?,?,?,?)", id, org, text(body.name, "space name"), text(body.description ?? "", "description", 4000, true), choice(body.visibility, ["team", "restricted"], "visibility"));
    this.audit(org, user, "space.created", id);
    return this.space(user, org, id);
  }
  updateSpace(user: string, org: string, id: string, body: Record<string, unknown>) {
    this.requireAdmin(user, org); this.space(user, org, id);
    this.run("UPDATE spaces SET name=?,description=?,visibility=? WHERE id=?", text(body.name, "space name"), text(body.description ?? "", "description", 4000, true), choice(body.visibility, ["team", "restricted"], "visibility"), id);
    this.audit(org, user, "space.updated", id);
    return this.space(user, org, id);
  }
  grants(user: string, org: string, space: string) {
    this.requireAdmin(user, org); this.space(user, org, space);
    return this.all("SELECT u.id,u.name,'member' AS kind,sm.role FROM space_members sm JOIN users u ON u.id=sm.user_id WHERE sm.space_id=? UNION ALL SELECT g.id,g.name,'group' AS kind,sg.role FROM space_groups sg JOIN groups g ON g.id=sg.group_id WHERE sg.space_id=?", space, space);
  }
  grant(user: string, org: string, space: string, body: Record<string, unknown>) {
    this.requireAdmin(user, org); this.space(user, org, space);
    const kind = choice(body.kind, ["member", "group"], "grant kind"), id = text(body.id, "recipient", 100);
    if (kind === "member") this.role(id, org);
    else if (!this.get("SELECT id FROM groups WHERE id=? AND org_id=?", id, org)) fail(404, "Group not found");
    const table = kind === "member" ? "space_members" : "space_groups", key = kind === "member" ? "user_id" : "group_id";
    if (body.role === "remove") this.run(`DELETE FROM ${table} WHERE space_id=? AND ${key}=?`, space, id);
    else this.run(`INSERT INTO ${table} VALUES(?,?,?) ON CONFLICT(space_id,${key}) DO UPDATE SET role=excluded.role`, space, id, choice(body.role, ["viewer", "editor"], "space role"));
    this.audit(org, user, "space.access_changed", space);
    return this.grants(user, org, space);
  }
  groups(user: string, org: string) {
    this.requireAdmin(user, org);
    return this.all("SELECT id,name FROM groups WHERE org_id=? ORDER BY name", org).map(g => ({ ...g, member_ids: this.all("SELECT user_id FROM group_members WHERE group_id=?", g.id).map(m => m.user_id) }));
  }
  saveGroup(user: string, org: string, body: Record<string, unknown>, groupId?: string) {
    this.requireAdmin(user, org);
    const name = text(body.name, "group name"), members = ids(body.member_ids, "members");
    members.forEach(id => this.role(id, org));
    if (groupId && !this.get("SELECT id FROM groups WHERE id=? AND org_id=?", groupId, org)) fail(404, "Group not found");
    const id = groupId ?? uid();
    this.db.transaction(() => {
      if (groupId) this.run("UPDATE groups SET name=? WHERE id=?", name, id);
      else this.run("INSERT INTO groups VALUES(?,?,?)", id, org, name);
      this.run("DELETE FROM group_members WHERE group_id=?", id);
      members.forEach(member => this.run("INSERT INTO group_members VALUES(?,?)", id, member));
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
    const visible = new Set(this.spaces(user, org).map(s => s.id));
    return this.all("SELECT f.* FROM folders f JOIN spaces s ON s.id=f.space_id WHERE s.org_id=? ORDER BY f.name", org).filter(f => visible.has(String(f.space_id)));
  }
  saveFolder(user: string, org: string, body: Record<string, unknown>, folderId?: string) {
    const space = text(body.space_id, "space", 100); this.space(user, org, space, true);
    const parent = body.parent_id == null ? null : text(body.parent_id, "parent folder", 100);
    const previous = folderId ? this.get("SELECT * FROM folders WHERE id=? AND space_id=?", folderId, space) : null;
    if (folderId && !previous) fail(404, "Folder not found");
    if (parent && !this.get("SELECT id FROM folders WHERE id=? AND space_id=?", parent, space)) fail(404, "Parent folder not found");
    if (folderId && parent) {
      const descendants = this.all("WITH RECURSIVE tree(id) AS(SELECT ? UNION SELECT f.id FROM folders f JOIN tree t ON f.parent_id=t.id) SELECT id FROM tree", folderId);
      if (descendants.some(d => d.id === parent)) fail(400, "A folder cannot contain itself");
    }
    const id = folderId ?? uid(), name = text(body.name, "folder name"), description = text(body.description ?? "", "description", 4000, true);
    if (folderId) this.run("UPDATE folders SET name=?,description=?,parent_id=? WHERE id=?", name, description, parent, id);
    else this.run("INSERT INTO folders VALUES(?,?,?,?,?)", id, space, parent, name, description);
    this.audit(org, user, "folder.saved", id);
    return { id, space_id: space, parent_id: parent, name, description };
  }
  note(user: string, org: string, id: string): TeamNote {
    const n = this.get("SELECT n.*,u.name AS owner_name FROM notes n JOIN users u ON u.id=n.owner_id WHERE n.id=?", id);
    if (!n) fail(404, "Meeting not found");
    const space = this.space(user, org, String(n.space_id));
    return { ...n, folder_ids: this.all("SELECT folder_id FROM note_folders WHERE note_id=?", id).map(f => String(f.folder_id)), can_edit: space.role === "editor", can_manage: space.role === "editor" && (n.owner_id === user || admin(this.role(user, org))) } as TeamNote;
  }
  validateFolders(user: string, org: string, space: string, values: unknown): string[] {
    this.space(user, org, space, true);
    const folders = ids(values ?? [], "folders");
    for (const id of folders) if (!this.get("SELECT id FROM folders WHERE id=? AND space_id=?", id, space)) fail(400, "Folder must belong to the destination space");
    return folders;
  }
  publish(user: string, org: string, body: Record<string, unknown>) {
    const space = text(body.space_id, "space", 100), folders = this.validateFolders(user, org, space, body.folder_ids);
    if (body.expected_access_version != null && body.expected_access_version !== this.accessVersion(org)) fail(409, "Workspace access changed after the preview opened. Review the destination again before publishing.");
    const key = text(body.source_key, "source key", 200), title = text(body.title, "title", 500);
    const summary = text(body.summary, "notes", 300_000), transcript = text(body.transcript ?? "", "transcript", 1_000_000, true);
    const occurred = text(body.occurred_at, "meeting date", 50);
    if (!Number.isFinite(Date.parse(occurred))) fail(400, "Invalid meeting date");
    const previous = this.get("SELECT id FROM notes WHERE space_id=? AND owner_id=? AND source_key=?", space, user, key);
    if (previous) fail(409, "This meeting is already shared in this space. Open the shared copy to edit it.");
    const id = uid(), at = now();
    this.db.transaction(() => {
      this.run("INSERT INTO notes(id,space_id,owner_id,source_key,title,summary,transcript,occurred_at,published_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)", id, space, user, key, title, summary, transcript, occurred, at, at);
      folders.forEach(folder => this.run("INSERT INTO note_folders VALUES(?,?)", id, folder));
      this.audit(org, user, "meeting.published", id);
    })();
    return this.note(user, org, id);
  }
  updateNote(user: string, org: string, id: string, body: Record<string, unknown>) {
    const n = this.note(user, org, id);
    if (!n.can_edit) fail(403, "This meeting is read-only for you");
    if (n.trashed_at) fail(409, "Restore this meeting before editing it");
    if (body.revision !== n.revision) fail(409, "Someone updated this meeting. Reload it before saving.");
    const title = text(body.title, "title", 500), summary = text(body.summary, "notes", 300_000);
    const folders = this.validateFolders(user, org, n.space_id, body.folder_ids);
    this.db.transaction(() => {
      this.run("UPDATE notes SET title=?,summary=?,updated_at=?,revision=revision+1 WHERE id=?", title, summary, now(), id);
      this.run("DELETE FROM note_folders WHERE note_id=?", id);
      folders.forEach(folder => this.run("INSERT INTO note_folders VALUES(?,?)", id, folder));
      this.audit(org, user, "meeting.edited", id);
    })();
    return this.note(user, org, id);
  }
  trash(user: string, org: string, id: string, revision: unknown, restore = false) {
    const n = this.note(user, org, id);
    if (!n.can_manage) fail(403, "Only the publisher or an admin can remove or restore this meeting");
    if (revision !== n.revision) fail(409, "This meeting changed. Reload it before continuing.");
    this.run("UPDATE notes SET trashed_at=?,updated_at=?,revision=revision+1 WHERE id=?", restore ? null : now(), now(), id);
    this.audit(org, user, restore ? "meeting.restored" : "meeting.trashed", id);
  }
  private noteScope(user: string, org: string, spaceId: string, folderId: string) {
    let spaces = this.spaces(user, org).map(s => s.id);
    if (spaceId) { this.space(user, org, spaceId); spaces = [spaceId]; }
    if (folderId) {
      const folder = this.get("SELECT space_id FROM folders WHERE id=?", folderId);
      if (!folder) fail(404, "Folder not found");
      this.space(user, org, String(folder.space_id));
      if (!spaces.includes(String(folder.space_id))) fail(400, "Folder is outside this scope");
      spaces = [String(folder.space_id)];
    }
    return {
      sql: `n.space_id IN(${spaces.length ? spaces.map(() => "?").join(",") : "NULL"})
      AND (?='' OR n.id IN( WITH RECURSIVE tree(id) AS(SELECT ? UNION SELECT f.id FROM folders f JOIN tree t ON f.parent_id=t.id) SELECT nf.note_id FROM note_folders nf JOIN tree t ON nf.folder_id=t.id))
      `,
      values: [...spaces, folderId, folderId],
    };
  }
  listNotes(user: string, org: string, query = "", spaceId = "", folderId = "", trash = false, limit = 100, offset = 0): TeamNoteRow[] {
    const scope = this.noteScope(user, org, spaceId, folderId);
    const rows = this.all<{ id: string }>(`SELECT n.id FROM notes n WHERE ${scope.sql} AND n.trashed_at IS ${trash ? "NOT " : ""}NULL
      AND (?='' OR instr(lower(n.title||char(10)||n.summary||char(10)||n.transcript),lower(?))>0)
      ORDER BY n.occurred_at DESC,n.id LIMIT ? OFFSET ?`, ...scope.values, query, query, Math.min(100, limit), offset);
    return rows.map(({ id }) => {
      const n = this.note(user, org, id), { summary, transcript, ...meta } = n;
      const content = query && !summary.toLowerCase().includes(query.toLowerCase()) ? transcript : summary;
      const start = query ? Math.max(0, content.toLowerCase().indexOf(query.toLowerCase()) - 80) : 0;
      return { ...meta, excerpt: content.slice(start, start + 240), has_transcript: !!transcript };
    });
  }
  context(user: string, org: string, body: Record<string, unknown>) {
    const question = text(body.question, "question", 6000);
    const selected = body.note_ids == null ? [] : ids(body.note_ids, "meetings", 40);
    const space = typeof body.space_id === "string" ? body.space_id : "", folder = typeof body.folder_id === "string" ? body.folder_id : "";
    // Resolve scope before reading any content. Exact selected IDs never bypass it.
    const scope = this.noteScope(user, org, space, folder);
    const stop = new Set("the and for with from what when where which who how are was were this that these those your our any all have has had will would could should list find meeting meetings shared sources cite original conversation".split(" "));
    const terms = [...new Set(question.toLowerCase().match(/[\p{L}\p{N}]{3,}/gu) ?? [])].filter(t => !stop.has(t)).slice(0, 24);
    const scores = terms.map(() => "(5*(instr(lower(n.title),?)>0)+2*(instr(lower(n.summary),?)>0)+(instr(lower(n.transcript),?)>0))");
    const where = `${scope.sql} AND n.trashed_at IS NULL ${selected.length ? `AND n.id IN(${selected.map(() => "?").join(",")})` : ""}`;
    const total = Number(this.get(`SELECT count(*) AS count FROM notes n WHERE ${where}`, ...scope.values, ...selected)!.count);
    if (selected.length && total !== selected.length) fail(404, "Selected meeting is unavailable in this scope");
    // Rank inside the authorized SQL scope across the whole library, not just
    // the latest list page. Fetch full text only for the selected candidates.
    const candidates = this.all<{ id: string }>(`SELECT n.id, ${scores.join("+") || "0"} AS score FROM notes n WHERE ${where} ORDER BY score DESC,n.occurred_at DESC,n.id LIMIT 12`, ...terms.flatMap(t => [t, t, t]), ...scope.values, ...selected);
    let remaining = 20_000;
    let truncated = false;
    const sources: TeamSource[] = [];
    for (const { id } of candidates) {
      if (remaining < 300) break;
      const n = this.note(user, org, id);
      const budget = Math.min(4500, remaining);
      const passage = (content: string, size: number) => {
        if (content.length <= size) return content;
        truncated = true;
        const positions = terms.map(t => content.toLowerCase().indexOf(t)).filter(i => i >= 0);
        const start = Math.max(0, (positions.length ? Math.min(...positions) : 0) - 250);
        return `${start ? "…" : ""}${content.slice(start, start + size - 2)}…`;
      };
      const summaryBudget = n.transcript ? Math.min(2200, Math.floor(budget / 2)) : budget - 80;
      const summary = passage(n.summary, summaryBudget);
      const transcript = n.transcript ? passage(n.transcript, Math.max(100, budget - summary.length - 80)) : "";
      const excerpt = `${n.occurred_at}\nSummary:\n${summary}${transcript ? `\nTranscript excerpt:\n${transcript}` : ""}`;
      remaining -= excerpt.length;
      sources.push({ id: n.id, title: n.title, revision: n.revision, citation: `S${sources.length + 1}`, excerpt });
    }
    return { sources, limited: total > sources.length || truncated };
  }
  recipes(user: string, org: string): TeamRecipe[] { this.role(user, org); return this.all("SELECT * FROM recipes WHERE org_id=? ORDER BY name", org); }
  saveAnswer(user: string, org: string, body: Record<string, unknown>) {
    this.role(user, org);
    const question = text(body.question, "question", 6000), answer = text(body.answer, "answer", 100_000);
    if (!Array.isArray(body.sources) || body.sources.length === 0 || body.sources.length > 12) fail(400, "An answer needs its meeting sources");
    const sourceIds = new Set<string>(), citations = new Set<string>();
    const sources = body.sources.map((value: unknown) => {
      if (!value || typeof value !== "object") fail(400, "Invalid answer source");
      const source = value as Record<string, unknown>;
      const id = text(source.id, "meeting", 100), citation = text(source.citation, "citation", 5);
      if (!/^S(?:[1-9]|1[0-2])$/.test(citation) || sourceIds.has(id) || citations.has(citation)) fail(400, "Invalid answer citations");
      sourceIds.add(id); citations.add(citation);
      const note = this.note(user, org, id);
      if (note.trashed_at || source.revision !== note.revision) fail(409, "Shared sources changed. Ask again before saving this answer.");
      return { id, citation, revision: note.revision };
    });
    const id = uid();
    this.db.transaction(() => {
      this.run("INSERT INTO saved_answers VALUES(?,?,?,?,?,?,?)", id, org, user, question, answer, body.limited ? 1 : 0, now());
      sources.forEach(s => this.run("INSERT INTO answer_sources VALUES(?,?,?,?)", id, s.id, s.revision, s.citation));
    })();
    return { id };
  }
  answer(user: string, org: string, id: string) {
    this.role(user, org);
    const row = this.get("SELECT * FROM saved_answers WHERE id=? AND org_id=? AND user_id=?", id, org, user);
    if (!row) fail(404, "Saved answer not found");
    const sources = this.all("SELECT * FROM answer_sources WHERE answer_id=? ORDER BY length(citation),citation", id).map(s => {
      let note: TeamNote;
      try { note = this.note(user, org, String(s.note_id)); } catch { fail(410, "This answer has a source you can no longer access"); }
      if (note.trashed_at || note.revision !== s.revision) fail(410, "This answer has a source that changed or was removed. Ask again for the current version.");
      return { id: note.id, title: note.title, revision: note.revision, citation: String(s.citation), excerpt: "" };
    });
    return { id, question: row.question, answer: row.answer, created_at: row.created_at, limited: !!row.limited, sources };
  }
  answers(user: string, org: string) {
    this.role(user, org);
    return this.all("SELECT id,created_at FROM saved_answers WHERE org_id=? AND user_id=? ORDER BY created_at DESC,id LIMIT 100", org, user).map(row => {
      try { const a = this.answer(user, org, String(row.id)); return { id: a.id, question: a.question, created_at: a.created_at, available: true }; }
      catch (e) { if (!(e instanceof TeamError) || e.status !== 410) throw e; return { id: row.id, question: "Sources changed or access removed", created_at: row.created_at, available: false }; }
    });
  }
  deleteAnswer(user: string, org: string, id: string) {
    this.role(user, org);
    this.run("DELETE FROM saved_answers WHERE id=? AND org_id=? AND user_id=?", id, org, user);
  }
  saveRecipe(user: string, org: string, body: Record<string, unknown>, recipeId?: string) {
    this.role(user, org);
    const old = recipeId ? this.get<TeamRecipe>("SELECT * FROM recipes WHERE id=? AND org_id=?", recipeId, org) : null;
    if (recipeId && !old) fail(404, "Recipe not found");
    if (old && old.owner_id !== user && !admin(this.role(user, org))) fail(403, "Only the author or an admin can edit this prompt");
    if (old && body.revision !== old.revision) fail(409, "This prompt changed. Reload before saving.");
    const name = text(body.name, "name"), prompt = text(body.prompt, "prompt", 12_000), kind = choice(body.kind, ["recipe", "template"], "kind");
    const id = recipeId ?? uid();
    if (old) this.run("UPDATE recipes SET name=?,prompt=?,kind=?,revision=revision+1 WHERE id=?", name, prompt, kind, id);
    else this.run("INSERT INTO recipes(id,org_id,owner_id,name,prompt,kind) VALUES(?,?,?,?,?,?)", id, org, user, name, prompt, kind);
    this.audit(org, user, "prompt.saved", id);
    return this.get("SELECT * FROM recipes WHERE id=?", id);
  }
  deleteRecipe(user: string, org: string, id: string) {
    this.role(user, org);
    const old = this.get("SELECT owner_id FROM recipes WHERE id=? AND org_id=?", id, org);
    if (!old) fail(404, "Prompt not found");
    if (old.owner_id !== user && !admin(this.role(user, org))) fail(403, "Only the author or an admin can delete this prompt");
    this.run("DELETE FROM recipes WHERE id=?", id); this.audit(org, user, "prompt.deleted", id);
  }
  snapshot(user: string, org: string) {
    const role = this.role(user, org);
    return { access_version: this.accessVersion(org), org: { ...this.get("SELECT id,name FROM organizations WHERE id=?", org), role }, user: this.get("SELECT * FROM users WHERE id=?", user), spaces: this.spaces(user, org), folders: this.folders(user, org), members: this.members(user, org), recipes: this.recipes(user, org) };
  }
  private accessVersion(org: string) {
    return Number(this.get("SELECT COALESCE(MAX(id),0) AS revision FROM audit WHERE org_id=? AND (action LIKE 'space.%' OR action LIKE 'member.%' OR action LIKE 'group.%' OR action='workspace.owner_changed')", org)!.revision);
  }
  activity(user: string, org: string) {
    this.requireAdmin(user, org);
    return this.all("SELECT a.id,a.action,a.target_id,a.at,u.name AS actor FROM audit a JOIN users u ON u.id=a.actor_id WHERE a.org_id=? ORDER BY a.id DESC LIMIT 100", org);
  }
}
